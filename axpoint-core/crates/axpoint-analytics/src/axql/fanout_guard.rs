//! `SUM_BY_ENTITY` and `AVG_BY_ENTITY` fanout deduplication guard (`05-STEP27` §27.2.2).
//!
//! Rewrites `SUM_BY_ENTITY(expr BY alias)` and `AVG_BY_ENTITY(expr BY alias)` into
//! entity-deduplicated aggregations backed by `ROW_NUMBER() OVER (PARTITION BY <group_keys...,>
//! <alias>____entity_id)` window columns so 1:N and N:M ontology paths never inflate 1-side
//! entity metrics.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::enums::Cardinality;
use axpoint_domain::studio::{EdgeTypeSpec, LinkDirection};
use sqlparser::ast::{
    BinaryOperator, Cte, DuplicateTreatment, Expr, Function, FunctionArg, FunctionArgExpr,
    FunctionArgumentList, FunctionArguments, GroupByExpr, Ident, ObjectName, OrderByExpr, Select,
    SelectItem, Value, ValueWithSpan, Visit, VisitMut, Visitor, VisitorMut,
};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;

use crate::axql::parser::{FN_AVG_BY_ENTITY, FN_SUM_BY_ENTITY, MatchPathSpec, bad_axql};

/// Kind of entity-deduplicated aggregation operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityAggOp {
    /// `SUM_BY_ENTITY(expr BY alias)`
    Sum,
    /// `AVG_BY_ENTITY(expr BY alias)`
    Avg,
}

impl EntityAggOp {
    const fn sql_fn_name(self) -> &'static str {
        match self {
            Self::Sum => "SUM",
            Self::Avg => "AVG",
        }
    }
}

/// Summary of `*_BY_ENTITY` deduplication rewrites applied to a query.
#[derive(Debug, Clone, Default)]
pub struct FanoutRewriteSummary {
    /// Distinct entity aliases deduplicated via `ROW_NUMBER() OVER (...)`.
    pub dedup_entities: Vec<String>,
    /// `true` if `*_BY_ENTITY` deduplication was applied or no 1-side fanout aggregation exists.
    pub fanout_protected: bool,
}

struct ByEntityMutator<'a> {
    bound_aliases: &'a BTreeSet<String>,
    ranked_table_alias: &'a str,
    dedup_entities: BTreeSet<String>,
}

impl VisitorMut for ByEntityMutator<'_> {
    type Break = AppError;

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        let maybe_op = match expr {
            Expr::Function(func) => classify_sentinel_agg_fn(func),
            _ => None,
        };
        let Some(op) = maybe_op else {
            return ControlFlow::Continue(());
        };
        let Expr::Function(func) = expr else {
            return ControlFlow::Continue(());
        };
        match rewrite_by_entity_call(func, op, self.bound_aliases, self.ranked_table_alias) {
            Ok((entity_alias, rewritten)) => {
                self.dedup_entities.insert(entity_alias);
                *expr = rewritten;
                ControlFlow::Continue(())
            }
            Err(err) => ControlFlow::Break(err),
        }
    }
}

fn classify_sentinel_agg_fn(func: &Function) -> Option<EntityAggOp> {
    let raw = func.name.to_string();
    let clean = raw.trim_matches('"');
    if clean.eq_ignore_ascii_case(FN_SUM_BY_ENTITY) {
        Some(EntityAggOp::Sum)
    } else if clean.eq_ignore_ascii_case(FN_AVG_BY_ENTITY) {
        Some(EntityAggOp::Avg)
    } else {
        None
    }
}

fn rewrite_by_entity_call(
    func: &Function,
    op: EntityAggOp,
    bound_aliases: &BTreeSet<String>,
    ranked_table_alias: &str,
) -> Result<(String, Expr), AppError> {
    let FunctionArguments::List(arg_list) = &func.args else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "malformed *_BY_ENTITY argument list",
        ));
    };
    if arg_list.args.len() != 2 {
        return Err(bad_axql(
            "SQL_REJECTED",
            "expected (expr, entity_alias) in *_BY_ENTITY",
        ));
    }
    let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(metric_expr))) = arg_list.args.first()
    else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "invalid metric expression in *_BY_ENTITY",
        ));
    };
    let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(val_span)))) =
        arg_list.args.get(1)
    else {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            "entity alias in BY clause must be a valid identifier",
        ));
    };
    let Value::SingleQuotedString(raw_alias) = &val_span.value else {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            "entity alias in BY clause must be a valid identifier",
        ));
    };
    let canonical_alias = bound_aliases
        .iter()
        .find(|a| a.eq_ignore_ascii_case(raw_alias))
        .cloned()
        .ok_or_else(|| {
            bad_axql(
                "AXQL_UNMAPPED_SYMBOL",
                format!("AXQL_UNMAPPED_SYMBOL: unknown entity alias '{raw_alias}' in BY clause"),
            )
        })?;

    let rn_col = format!("__rn_{canonical_alias}");
    let cond = Expr::BinaryOp {
        left: Box::new(Expr::CompoundIdentifier(vec![
            Ident::with_quote('"', ranked_table_alias),
            Ident::with_quote('"', rn_col),
        ])),
        op: BinaryOperator::Eq,
        right: Box::new(Expr::Value(ValueWithSpan::from(Value::Number(
            String::from("1"),
            false,
        )))),
    };
    let case_expr = Expr::Case {
        case_token: sqlparser::ast::helpers::attached_token::AttachedToken::empty(),
        end_token: sqlparser::ast::helpers::attached_token::AttachedToken::empty(),
        operand: None,
        conditions: vec![sqlparser::ast::CaseWhen {
            condition: cond,
            result: metric_expr.clone(),
        }],
        else_result: Some(Box::new(Expr::Value(ValueWithSpan::from(Value::Null)))),
    };
    let agg_expr = Expr::Function(Function {
        name: ObjectName::from(vec![Ident::new(op.sql_fn_name())]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None::<DuplicateTreatment>,
            args: vec![FunctionArg::Unnamed(FunctionArgExpr::Expr(case_expr))],
            clauses: Vec::new(),
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: Vec::new(),
    });

    Ok((canonical_alias, agg_expr))
}

/// Rewrites all `SUM_BY_ENTITY` and `AVG_BY_ENTITY` sentinel calls inside `select` and
/// `order_by`, returning the deduplicated entity aliases.
///
/// # Errors
/// Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL` or `SQL_REJECTED`) if an unknown
/// entity alias is referenced.
pub fn rewrite_by_entity_aggregations(
    select: &mut Select,
    order_by_exprs: &mut [OrderByExpr],
    bound_aliases: &BTreeSet<String>,
    ranked_table_alias: &str,
) -> Result<Vec<String>, AppError> {
    let mut mutator = ByEntityMutator {
        bound_aliases,
        ranked_table_alias,
        dedup_entities: BTreeSet::new(),
    };
    for item in &mut select.projection {
        if let ControlFlow::Break(err) = VisitMut::visit(item, &mut mutator) {
            return Err(err);
        }
    }
    if let Some(ref mut having) = select.having
        && let ControlFlow::Break(err) = VisitMut::visit(having, &mut mutator)
    {
        return Err(err);
    }
    for ob in order_by_exprs {
        if let ControlFlow::Break(err) = VisitMut::visit(&mut ob.expr, &mut mutator) {
            return Err(err);
        }
    }
    Ok(mutator.dedup_entities.into_iter().collect())
}

/// Builds the `"__axql_ranked"` CTE SQL adding `ROW_NUMBER() OVER (PARTITION BY ...)` columns
/// for each deduplicated entity alias.
///
/// # Errors
/// Returns `AppError::BadRequest` (`SQL_REJECTED`) if the generated CTE fails `DuckDbDialect` parsing.
pub fn build_ranked_cte(
    cte_name: &str,
    input_cte: &str,
    dedup_entities: &[String],
    select: &Select,
    ranked_table_alias: &str,
) -> Result<Cte, AppError> {
    let group_partition_exprs = extract_partition_group_sql(select, ranked_table_alias, "__f");
    let mut window_cols = Vec::with_capacity(dedup_entities.len());
    for alias in dedup_entities {
        let entity_id_col = format!("\"__f\".\"{alias}____entity_id\"");
        let mut part_items = group_partition_exprs.clone();
        part_items.push(entity_id_col.clone());
        let part_clause = part_items.join(", ");
        window_cols.push(format!(
            "ROW_NUMBER() OVER (PARTITION BY {part_clause} ORDER BY {entity_id_col}) AS \"__rn_{alias}\""
        ));
    }
    let cte_sql = format!(
        "SELECT \"__f\".*, {} FROM \"{input_cte}\" AS \"__f\"",
        window_cols.join(", ")
    );
    parse_cte(cte_name, &cte_sql)
}

fn extract_partition_group_sql(
    select: &Select,
    outer_alias: &str,
    inner_alias: &str,
) -> Vec<String> {
    let GroupByExpr::Expressions(exprs, _) = &select.group_by else {
        return Vec::new();
    };
    if exprs.is_empty() {
        return Vec::new();
    }
    let alias_map = collect_select_alias_exprs(select);
    let mut out = Vec::with_capacity(exprs.len());
    for g_expr in exprs {
        let resolved = resolve_group_by_item(g_expr, &select.projection, &alias_map);
        let replaced = retarget_table_qualifier(resolved, outer_alias, inner_alias);
        out.push(replaced.to_string());
    }
    out
}

fn collect_select_alias_exprs(select: &Select) -> BTreeMap<String, Expr> {
    let mut map = BTreeMap::new();
    for item in &select.projection {
        if let SelectItem::ExprWithAlias { expr, alias } = item {
            map.insert(alias.value.to_ascii_lowercase(), expr.clone());
        }
    }
    map
}

fn resolve_group_by_item(
    g_expr: &Expr,
    projection: &[SelectItem],
    alias_map: &BTreeMap<String, Expr>,
) -> Expr {
    if let Expr::Value(val_span) = g_expr
        && let Value::Number(num_s, false) = &val_span.value
        && let Ok(pos) = num_s.parse::<usize>()
        && pos >= 1
        && let Some(item) = projection.get(pos.saturating_sub(1))
    {
        match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                return e.clone();
            }
            _ => {}
        }
    }
    if let Expr::Identifier(ident) = g_expr
        && let Some(mapped) = alias_map.get(&ident.value.to_ascii_lowercase())
    {
        return mapped.clone();
    }
    g_expr.clone()
}

fn retarget_table_qualifier(mut expr: Expr, from_alias: &str, to_alias: &str) -> Expr {
    struct Retarget<'a> {
        from_alias: &'a str,
        to_alias: &'a str,
    }
    impl VisitorMut for Retarget<'_> {
        type Break = ();
        fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
            if let Expr::CompoundIdentifier(idents) = expr
                && idents.len() == 2
                && let Some(first) = idents.first_mut()
                && first.value == self.from_alias
            {
                *first = Ident::with_quote('"', self.to_alias);
            }
            ControlFlow::Continue(())
        }
    }
    let _flow = VisitMut::visit(
        &mut expr,
        &mut Retarget {
            from_alias,
            to_alias,
        },
    );
    expr
}

/// Checks whether a `FROM MATCH` query performs raw `SUM`/`AVG` over a 1-side vertex alias
/// along a `ONE_TO_MANY` or `MANY_TO_MANY` path without `*_BY_ENTITY` protection.
#[must_use]
pub fn has_unprotected_fanout_agg(
    select: &Select,
    path: &MatchPathSpec,
    resolved_edges: &[EdgeTypeSpec],
    ranked_table_alias: &str,
) -> bool {
    let mut risky_aliases = BTreeSet::new();
    for (idx, hop) in path.hops.iter().enumerate() {
        let Some(edge_spec) = resolved_edges.get(idx) else {
            continue;
        };
        let Some(left_node) = path.nodes.get(idx) else {
            continue;
        };
        let Some(right_node) = path.nodes.get(idx.saturating_add(1)) else {
            continue;
        };
        match (edge_spec.cardinality, hop.direction) {
            (Cardinality::ManyToMany, _) => {
                risky_aliases.insert(left_node.alias.clone());
                risky_aliases.insert(right_node.alias.clone());
            }
            (Cardinality::OneToMany, LinkDirection::Out)
            | (Cardinality::ManyToOne, LinkDirection::In) => {
                risky_aliases.insert(left_node.alias.clone());
            }
            (Cardinality::OneToMany, LinkDirection::In)
            | (Cardinality::ManyToOne, LinkDirection::Out) => {
                risky_aliases.insert(right_node.alias.clone());
            }
            (Cardinality::OneToOne, _) => {}
        }
    }
    if risky_aliases.is_empty() {
        return false;
    }

    let mut detector = RawFanoutDetector {
        risky_aliases: &risky_aliases,
        ranked_table_alias,
        found_unprotected: false,
    };
    for item in &select.projection {
        let _flow = Visit::visit(item, &mut detector);
    }
    detector.found_unprotected
}

struct RawFanoutDetector<'a> {
    risky_aliases: &'a BTreeSet<String>,
    ranked_table_alias: &'a str,
    found_unprotected: bool,
}

impl Visitor for RawFanoutDetector<'_> {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        let Expr::Function(func) = expr else {
            return ControlFlow::Continue(());
        };
        let fn_name = func.name.to_string().trim_matches('"').to_ascii_uppercase();
        if !matches!(fn_name.as_str(), "SUM" | "AVG") {
            return ControlFlow::Continue(());
        }
        let FunctionArguments::List(arg_list) = &func.args else {
            return ControlFlow::Continue(());
        };
        if matches!(
            arg_list.duplicate_treatment,
            Some(DuplicateTreatment::Distinct)
        ) {
            return ControlFlow::Continue(());
        }
        if arg_list.args.iter().any(|a| {
            matches!(
                a,
                FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Case { .. }))
            )
        }) {
            return ControlFlow::Continue(());
        }
        let mut col_scan = AliasPrefixScanner {
            ranked_table_alias: self.ranked_table_alias,
            risky_aliases: self.risky_aliases,
            hit: false,
        };
        let _flow = Visit::visit(&func.args, &mut col_scan);
        if col_scan.hit {
            self.found_unprotected = true;
        }
        ControlFlow::Continue(())
    }
}

struct AliasPrefixScanner<'a> {
    ranked_table_alias: &'a str,
    risky_aliases: &'a BTreeSet<String>,
    hit: bool,
}

impl Visitor for AliasPrefixScanner<'_> {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::CompoundIdentifier(idents) = expr
            && let [tbl, col] = idents.as_slice()
            && tbl.value == self.ranked_table_alias
            && let Some((prefix, _)) = col.value.split_once("__")
            && self.risky_aliases.contains(prefix)
        {
            self.hit = true;
        }
        ControlFlow::Continue(())
    }
}

pub(crate) fn parse_cte(cte_name: &str, body_sql: &str) -> Result<Cte, AppError> {
    let wrapped = format!("WITH \"{cte_name}\" AS ({body_sql}) SELECT 1");
    let mut stmts = Parser::parse_sql(&DuckDbDialect {}, &wrapped).map_err(|err| {
        bad_axql(
            "SQL_REJECTED",
            format!("internal CTE compile error on '{cte_name}': {err}"),
        )
    })?;
    if stmts.len() != 1 {
        return Err(bad_axql("SQL_REJECTED", "expected single CTE query"));
    }
    let Some(sqlparser::ast::Statement::Query(mut q)) = stmts.pop() else {
        return Err(bad_axql("SQL_REJECTED", "expected single CTE query"));
    };
    let Some(mut w) = q.with.take() else {
        return Err(bad_axql("SQL_REJECTED", "missing WITH clause in CTE"));
    };
    w.cte_tables
        .pop()
        .ok_or_else(|| bad_axql("SQL_REJECTED", "empty CTE list"))
}

/// Extracts `OrderByExpr` items from an optional `Query.order_by` clause.
#[must_use]
pub fn take_order_by_exprs(order_by: &mut Option<sqlparser::ast::OrderBy>) -> Vec<OrderByExpr> {
    let Some(ob) = order_by else {
        return Vec::new();
    };
    match &mut ob.kind {
        sqlparser::ast::OrderByKind::Expressions(exprs) => std::mem::take(exprs),
        sqlparser::ast::OrderByKind::All(_) => Vec::new(),
    }
}

/// Restores `OrderByExpr` items back into `Query.order_by`.
pub fn restore_order_by_exprs(
    order_by: &mut Option<sqlparser::ast::OrderBy>,
    exprs: Vec<OrderByExpr>,
) {
    if exprs.is_empty() {
        return;
    }
    if let Some(ob) = order_by {
        ob.kind = sqlparser::ast::OrderByKind::Expressions(exprs);
    } else {
        *order_by = Some(sqlparser::ast::OrderBy {
            kind: sqlparser::ast::OrderByKind::Expressions(exprs),
            interpolate: None,
        });
    }
}
