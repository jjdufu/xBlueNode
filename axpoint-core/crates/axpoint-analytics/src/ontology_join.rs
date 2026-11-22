//! AST-level multi-dataset ontology foreign-key `JOIN` path validator (`02-STEP16` §16.3.3).

use std::collections::{BTreeSet, HashMap};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::analytics::AllowedDatasetJoinEdge;
use sqlparser::ast::{
    BinaryOperator, Expr, GroupByExpr, JoinConstraint, JoinOperator, ObjectNamePart, Query, Select,
    SelectItem, SelectItemQualifiedWildcardKind, SetExpr, TableAlias, TableFactor, TableWithJoins,
    Visit, Visitor,
};

use crate::sql_guard::{CatalogDatasetEntry, TenantCatalog};

const MAX_JOIN_VALIDATION_DEPTH: usize = 64;

#[derive(Debug, Clone, Default)]
struct RelationJoinOrigin {
    backing_datasets: BTreeSet<String>,
    passthrough_cols: HashMap<String, (String, String)>,
    wildcard_dataset: Option<String>,
}

#[derive(Debug, Clone)]
enum JoinRelSource {
    Dataset { dataset_name: String },
    Derived { origin: RelationJoinOrigin },
}

#[derive(Debug, Clone)]
struct JoinRelInstance {
    alias: String,
    source: JoinRelSource,
}

impl JoinRelInstance {
    fn backing_datasets(&self) -> BTreeSet<String> {
        match &self.source {
            JoinRelSource::Dataset { dataset_name } => BTreeSet::from([dataset_name.clone()]),
            JoinRelSource::Derived { origin } => origin.backing_datasets.clone(),
        }
    }

    fn resolve_column(
        &self,
        col_name: &str,
        catalog: &TenantCatalog,
    ) -> Result<(String, String), AppError> {
        let col_lower = col_name.trim().to_ascii_lowercase();
        match &self.source {
            JoinRelSource::Dataset { dataset_name } => {
                if let Some(ds) = catalog.dataset(dataset_name)
                    && !ds.columns.is_empty()
                    && !ds.columns.contains_key(&col_lower)
                {
                    return Err(ungrounded_join(format!(
                        "JOIN ON references unknown column '{dataset_name}.{col_lower}'"
                    )));
                }
                Ok((dataset_name.clone(), col_lower))
            }
            JoinRelSource::Derived { origin } => {
                if let Some(found) = origin.passthrough_cols.get(&col_lower) {
                    return Ok(found.clone());
                }
                if let Some(ref ds_name) = origin.wildcard_dataset {
                    return Ok((ds_name.clone(), col_lower));
                }
                Err(ungrounded_join(format!(
                    "JOIN ON column '{}.{col_lower}' is derived or aggregated and cannot act as an ontology join key",
                    self.alias
                )))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinSide {
    Connected,
    NewRhs,
}

/// Validates that every multi-dataset `SELECT` inside `query` joins exclusively along
/// `allowed_joins` edges (or same-column self-joins) (`02-STEP16` §16.3.3).
///
/// # Errors
/// Returns `AppError::BadRequest` with `code == "JOIN_PATH_UNGROUNDED"` when an implicit comma
/// join, `CROSS`/`NATURAL`/unconditional join, unqualified/non-equality `ON` predicate, or
/// unmapped cross-dataset column pair is encountered.
pub(crate) fn validate_ontology_joins(
    query: &Query,
    catalog: &TenantCatalog,
    allowed_joins: &[AllowedDatasetJoinEdge],
) -> Result<(), AppError> {
    let empty_ctes = HashMap::new();
    let _ = validate_query_joins(query, catalog, &empty_ctes, allowed_joins, 0)?;
    Ok(())
}

fn ungrounded_join(message: impl Into<String>) -> AppError {
    AppError::bad("JOIN_PATH_UNGROUNDED", message.into())
}

fn validate_query_joins(
    query: &Query,
    catalog: &TenantCatalog,
    outer_ctes: &HashMap<String, RelationJoinOrigin>,
    allowed_joins: &[AllowedDatasetJoinEdge],
    depth: usize,
) -> Result<RelationJoinOrigin, AppError> {
    if depth > MAX_JOIN_VALIDATION_DEPTH {
        return Ok(RelationJoinOrigin::default());
    }
    let mut cte_scope = outer_ctes.clone();
    if let Some(ref with_clause) = query.with {
        if with_clause.recursive {
            for cte in &with_clause.cte_tables {
                let name = cte.alias.name.value.trim().to_ascii_lowercase();
                if !name.is_empty() {
                    cte_scope.insert(name, RelationJoinOrigin::default());
                }
            }
        }
        for cte in &with_clause.cte_tables {
            let name = cte.alias.name.value.trim().to_ascii_lowercase();
            let mut origin = validate_query_joins(
                &cte.query,
                catalog,
                &cte_scope,
                allowed_joins,
                depth.saturating_add(1),
            )?;
            apply_cte_column_aliases(&mut origin, &cte.alias);
            if !name.is_empty() {
                cte_scope.insert(name, origin);
            }
        }
    }
    validate_set_expr_joins(&query.body, catalog, &cte_scope, allowed_joins, depth)
}

fn apply_cte_column_aliases(origin: &mut RelationJoinOrigin, alias: &TableAlias) {
    if alias.columns.is_empty() {
        return;
    }
    // Positional table-column aliasing renames columns; clear by-name map unless 1:1 preserved.
    let mut remapped = HashMap::new();
    for col_def in &alias.columns {
        let lower = col_def.name.value.trim().to_ascii_lowercase();
        if let Some(existing) = origin.passthrough_cols.get(&lower) {
            remapped.insert(lower, existing.clone());
        }
    }
    origin.passthrough_cols = remapped;
    origin.wildcard_dataset = None;
}

fn validate_set_expr_joins(
    set_expr: &SetExpr,
    catalog: &TenantCatalog,
    cte_scope: &HashMap<String, RelationJoinOrigin>,
    allowed_joins: &[AllowedDatasetJoinEdge],
    depth: usize,
) -> Result<RelationJoinOrigin, AppError> {
    if depth > MAX_JOIN_VALIDATION_DEPTH {
        return Ok(RelationJoinOrigin::default());
    }
    match set_expr {
        SetExpr::Select(select) => {
            validate_select_joins(select, catalog, cte_scope, allowed_joins, depth)
        }
        SetExpr::Query(inner) => validate_query_joins(
            inner,
            catalog,
            cte_scope,
            allowed_joins,
            depth.saturating_add(1),
        ),
        SetExpr::SetOperation { left, right, .. } => {
            let l_orig = validate_set_expr_joins(
                left,
                catalog,
                cte_scope,
                allowed_joins,
                depth.saturating_add(1),
            )?;
            let r_orig = validate_set_expr_joins(
                right,
                catalog,
                cte_scope,
                allowed_joins,
                depth.saturating_add(1),
            )?;
            let mut backing = l_orig.backing_datasets;
            backing.extend(r_orig.backing_datasets);
            Ok(RelationJoinOrigin {
                backing_datasets: backing,
                passthrough_cols: HashMap::new(),
                wildcard_dataset: None,
            })
        }
        _ => Ok(RelationJoinOrigin::default()),
    }
}

fn validate_select_joins(
    select: &Select,
    catalog: &TenantCatalog,
    cte_scope: &HashMap<String, RelationJoinOrigin>,
    allowed_joins: &[AllowedDatasetJoinEdge],
    depth: usize,
) -> Result<RelationJoinOrigin, AppError> {
    validate_nested_expr_subqueries(select, catalog, cte_scope, allowed_joins, depth)?;

    let mut twj_groups: Vec<(Vec<JoinRelInstance>, &TableWithJoins)> = Vec::new();
    let mut total_ds_instances: usize = 0;

    for twj in &select.from {
        let mut group_rels =
            resolve_table_factor_rels(&twj.relation, catalog, cte_scope, allowed_joins, depth)?;
        for j in &twj.joins {
            let j_rels =
                resolve_table_factor_rels(&j.relation, catalog, cte_scope, allowed_joins, depth)?;
            group_rels.extend(j_rels);
        }
        total_ds_instances = total_ds_instances.saturating_add(group_rels.len());
        twj_groups.push((group_rels, twj));
    }

    if total_ds_instances >= 2 && select.from.len() > 1 {
        return Err(ungrounded_join(
            "cross join or implicit multi-table join without ontology MappingLinkSpec is forbidden",
        ));
    }

    let mut all_rels: Vec<JoinRelInstance> = Vec::new();
    for (_, twj) in &twj_groups {
        let stepped = validate_single_twj(twj, catalog, cte_scope, allowed_joins, depth)?;
        all_rels.extend(stepped);
    }

    Ok(compute_select_origin(select, &all_rels, catalog))
}

fn validate_single_twj(
    twj: &TableWithJoins,
    catalog: &TenantCatalog,
    cte_scope: &HashMap<String, RelationJoinOrigin>,
    allowed_joins: &[AllowedDatasetJoinEdge],
    depth: usize,
) -> Result<Vec<JoinRelInstance>, AppError> {
    let mut connected =
        resolve_table_factor_rels(&twj.relation, catalog, cte_scope, allowed_joins, depth)?;

    for join in &twj.joins {
        let rhs_rels =
            resolve_table_factor_rels(&join.relation, catalog, cte_scope, allowed_joins, depth)?;
        if rhs_rels.is_empty() {
            continue;
        }
        if connected.is_empty() {
            connected.extend(rhs_rels);
            continue;
        }
        let on_expr = extract_conditional_on_expr(&join.join_operator)?;
        verify_join_step_on_expr(on_expr, &connected, &rhs_rels, catalog, allowed_joins)?;
        connected.extend(rhs_rels);
    }

    Ok(connected)
}

fn resolve_table_factor_rels(
    tf: &TableFactor,
    catalog: &TenantCatalog,
    cte_scope: &HashMap<String, RelationJoinOrigin>,
    allowed_joins: &[AllowedDatasetJoinEdge],
    depth: usize,
) -> Result<Vec<JoinRelInstance>, AppError> {
    match tf {
        TableFactor::Table {
            name,
            alias,
            args: None,
            ..
        } => Ok(resolve_named_table_rel(
            name,
            alias.as_ref(),
            catalog,
            cte_scope,
        )),
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            let mut sub_orig = validate_query_joins(
                subquery,
                catalog,
                cte_scope,
                allowed_joins,
                depth.saturating_add(1),
            )?;
            if let Some(tbl_alias) = alias.as_ref() {
                apply_cte_column_aliases(&mut sub_orig, tbl_alias);
            }
            if sub_orig.backing_datasets.is_empty() {
                return Ok(Vec::new());
            }
            let eff_alias = alias
                .as_ref()
                .map_or_else(String::new, |a| a.name.value.trim().to_ascii_lowercase());
            Ok(vec![JoinRelInstance {
                alias: eff_alias,
                source: JoinRelSource::Derived { origin: sub_orig },
            }])
        }
        TableFactor::NestedJoin {
            table_with_joins,
            alias,
        } => {
            let inner_rels =
                validate_single_twj(table_with_joins, catalog, cte_scope, allowed_joins, depth)?;
            if let Some(tbl_alias) = alias.as_ref() {
                let mut backing = BTreeSet::new();
                for r in &inner_rels {
                    backing.extend(r.backing_datasets());
                }
                if backing.is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![JoinRelInstance {
                    alias: tbl_alias.name.value.trim().to_ascii_lowercase(),
                    source: JoinRelSource::Derived {
                        origin: RelationJoinOrigin {
                            backing_datasets: backing,
                            passthrough_cols: HashMap::new(),
                            wildcard_dataset: None,
                        },
                    },
                }])
            } else {
                Ok(inner_rels)
            }
        }
        _ => Ok(Vec::new()),
    }
}

fn resolve_named_table_rel(
    name: &sqlparser::ast::ObjectName,
    alias: Option<&TableAlias>,
    catalog: &TenantCatalog,
    cte_scope: &HashMap<String, RelationJoinOrigin>,
) -> Vec<JoinRelInstance> {
    let [ObjectNamePart::Identifier(ident)] = name.0.as_slice() else {
        return Vec::new();
    };
    let lower_name = ident.value.trim().to_ascii_lowercase();
    let eff_alias = alias.map_or_else(
        || lower_name.clone(),
        |a| a.name.value.trim().to_ascii_lowercase(),
    );

    if let Some(cte_orig) = cte_scope.get(&lower_name) {
        if cte_orig.backing_datasets.is_empty() {
            return Vec::new();
        }
        let mut orig = cte_orig.clone();
        if let Some(tbl_alias) = alias {
            apply_cte_column_aliases(&mut orig, tbl_alias);
        }
        return vec![JoinRelInstance {
            alias: eff_alias,
            source: JoinRelSource::Derived { origin: orig },
        }];
    }

    if let Some(ds) = catalog.dataset(&lower_name) {
        return vec![JoinRelInstance {
            alias: eff_alias,
            source: JoinRelSource::Dataset {
                dataset_name: ds.name.to_ascii_lowercase(),
            },
        }];
    }
    Vec::new()
}

fn extract_conditional_on_expr(op: &JoinOperator) -> Result<&Expr, AppError> {
    let (JoinOperator::Join(constraint)
    | JoinOperator::Inner(constraint)
    | JoinOperator::Left(constraint)
    | JoinOperator::LeftOuter(constraint)
    | JoinOperator::Right(constraint)
    | JoinOperator::RightOuter(constraint)
    | JoinOperator::FullOuter(constraint)
    | JoinOperator::Semi(constraint)
    | JoinOperator::LeftSemi(constraint)
    | JoinOperator::RightSemi(constraint)
    | JoinOperator::Anti(constraint)
    | JoinOperator::LeftAnti(constraint)
    | JoinOperator::RightAnti(constraint)) = op
    else {
        return Err(ungrounded_join(
            "cross join or implicit multi-table join without ontology MappingLinkSpec is forbidden",
        ));
    };
    match constraint {
        JoinConstraint::On(expr) => Ok(expr),
        _ => Err(ungrounded_join(
            "NATURAL, USING, or unconditional JOIN without explicit ON ontology key equality is forbidden",
        )),
    }
}

fn verify_join_step_on_expr(
    on_expr: &Expr,
    connected: &[JoinRelInstance],
    rhs_rels: &[JoinRelInstance],
    catalog: &TenantCatalog,
    allowed_joins: &[AllowedDatasetJoinEdge],
) -> Result<(), AppError> {
    ensure_no_unqualified_columns_in_on(on_expr)?;

    let mut conjuncts = Vec::new();
    collect_top_level_and_conjuncts(on_expr, &mut conjuncts);

    let mut eq_pairs = Vec::new();
    for conj in conjuncts {
        if let Some(pair) = extract_qualified_eq_pair(conj) {
            eq_pairs.push(pair);
        }
    }

    if eq_pairs.is_empty() {
        return Err(ungrounded_join(
            "JOIN ON must contain a qualified alias_a.col_a = alias_b.col_b equality on a published MappingLinkSpec",
        ));
    }

    let mut bridged = false;
    for (lq, lc, rq, rc) in eq_pairs {
        let (l_side, l_ds, l_col) = lookup_qualified_col(&lq, &lc, connected, rhs_rels, catalog)?;
        let (r_side, r_ds, r_col) = lookup_qualified_col(&rq, &rc, connected, rhs_rels, catalog)?;

        if l_side != r_side {
            let (conn_ds, conn_col, new_ds, new_col) = if l_side == JoinSide::Connected {
                (l_ds, l_col, r_ds, r_col)
            } else {
                (r_ds, r_col, l_ds, l_col)
            };
            if !is_legal_join_pair(&conn_ds, &conn_col, &new_ds, &new_col, allowed_joins) {
                return Err(ungrounded_join(format!(
                    "ungrounded join between '{conn_ds}.{conn_col}' and '{new_ds}.{new_col}': no published MappingLinkSpec connects these columns"
                )));
            }
            bridged = true;
        }
    }

    if !bridged {
        return Err(ungrounded_join(
            "JOIN ON does not connect the newly joined dataset to an existing FROM dataset",
        ));
    }
    Ok(())
}

fn is_legal_join_pair(
    ds_a: &str,
    col_a: &str,
    ds_b: &str,
    col_b: &str,
    allowed_joins: &[AllowedDatasetJoinEdge],
) -> bool {
    if ds_a.eq_ignore_ascii_case(ds_b) {
        return col_a.eq_ignore_ascii_case(col_b);
    }
    allowed_joins
        .iter()
        .any(|edge| edge.matches_undirected(ds_a, col_a, ds_b, col_b))
}

fn lookup_qualified_col(
    qual: &str,
    col: &str,
    connected: &[JoinRelInstance],
    rhs_rels: &[JoinRelInstance],
    catalog: &TenantCatalog,
) -> Result<(JoinSide, String, String), AppError> {
    let q_lower = qual.trim().to_ascii_lowercase();
    let mut matched: Option<(JoinSide, &JoinRelInstance)> = None;

    for rel in connected {
        if !rel.alias.is_empty() && rel.alias == q_lower {
            if matched.is_some() {
                return Err(ungrounded_join(format!(
                    "ambiguous table alias '{q_lower}' in JOIN ON"
                )));
            }
            matched = Some((JoinSide::Connected, rel));
        }
    }
    for rel in rhs_rels {
        if !rel.alias.is_empty() && rel.alias == q_lower {
            if matched.is_some() {
                return Err(ungrounded_join(format!(
                    "ambiguous table alias '{q_lower}' in JOIN ON"
                )));
            }
            matched = Some((JoinSide::NewRhs, rel));
        }
    }

    let Some((side, rel)) = matched else {
        return Err(ungrounded_join(format!(
            "unknown or out-of-scope table alias '{q_lower}' in JOIN ON"
        )));
    };
    let (ds_name, phys_col) = rel.resolve_column(col, catalog)?;
    Ok((side, ds_name, phys_col))
}

fn collect_top_level_and_conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match unwrap_nested_expr(expr) {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            collect_top_level_and_conjuncts(left, out);
            collect_top_level_and_conjuncts(right, out);
        }
        other => out.push(other),
    }
}

fn extract_qualified_eq_pair(expr: &Expr) -> Option<(String, String, String, String)> {
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right,
    } = unwrap_nested_expr(expr)
    else {
        return None;
    };
    let (lq, lc) = extract_two_part_col(left)?;
    let (rq, rc) = extract_two_part_col(right)?;
    Some((lq, lc, rq, rc))
}

fn extract_two_part_col(expr: &Expr) -> Option<(String, String)> {
    let Expr::CompoundIdentifier(parts) = unwrap_nested_expr(expr) else {
        return None;
    };
    let [qual, col] = parts.as_slice() else {
        return None;
    };
    Some((qual.value.clone(), col.value.clone()))
}

fn unwrap_nested_expr(mut expr: &Expr) -> &Expr {
    while let Expr::Nested(inner) = expr {
        expr = inner;
    }
    expr
}

struct UnqualifiedOnInspector;

impl Visitor for UnqualifiedOnInspector {
    type Break = AppError;

    fn pre_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        // Subqueries inside ON are rejected as non-key-equality predicates.
        ControlFlow::Break(ungrounded_join(
            "subqueries inside JOIN ON are forbidden under strict_ontology_join",
        ))
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Identifier(ident) = expr {
            return ControlFlow::Break(ungrounded_join(format!(
                "unqualified column '{}' in multi-table JOIN ON is forbidden",
                ident.value
            )));
        }
        ControlFlow::Continue(())
    }
}

fn ensure_no_unqualified_columns_in_on(on_expr: &Expr) -> Result<(), AppError> {
    let mut inspector = UnqualifiedOnInspector;
    if let ControlFlow::Break(err) = on_expr.visit(&mut inspector) {
        return Err(err);
    }
    Ok(())
}

struct NestedSubqueryVisitor<'a> {
    catalog: &'a TenantCatalog,
    cte_scope: &'a HashMap<String, RelationJoinOrigin>,
    allowed_joins: &'a [AllowedDatasetJoinEdge],
    depth: usize,
}

impl Visitor for NestedSubqueryVisitor<'_> {
    type Break = AppError;

    fn pre_visit_table_factor(&mut self, _tf: &TableFactor) -> ControlFlow<Self::Break> {
        // Table factors in FROM/JOIN are already traversed directly by `validate_select_joins`.
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if let Err(err) = validate_query_joins(
            query,
            self.catalog,
            self.cte_scope,
            self.allowed_joins,
            self.depth.saturating_add(1),
        ) {
            return ControlFlow::Break(err);
        }
        ControlFlow::Continue(())
    }
}

fn validate_nested_expr_subqueries(
    select: &Select,
    catalog: &TenantCatalog,
    cte_scope: &HashMap<String, RelationJoinOrigin>,
    allowed_joins: &[AllowedDatasetJoinEdge],
    depth: usize,
) -> Result<(), AppError> {
    let mut v = NestedSubqueryVisitor {
        catalog,
        cte_scope,
        allowed_joins,
        depth,
    };
    for item in &select.projection {
        if let ControlFlow::Break(err) = item.visit(&mut v) {
            return Err(err);
        }
    }
    if let Some(ref sel_where) = select.selection
        && let ControlFlow::Break(err) = sel_where.visit(&mut v)
    {
        return Err(err);
    }
    if let Some(ref having) = select.having
        && let ControlFlow::Break(err) = having.visit(&mut v)
    {
        return Err(err);
    }
    Ok(())
}

fn has_explicit_group_by_or_distinct(select: &Select) -> bool {
    if select.distinct.is_some() || select.having.is_some() {
        return true;
    }
    match &select.group_by {
        GroupByExpr::All(_) => true,
        GroupByExpr::Expressions(exprs, _) => !exprs.is_empty(),
    }
}

fn compute_select_origin(
    select: &Select,
    rels: &[JoinRelInstance],
    catalog: &TenantCatalog,
) -> RelationJoinOrigin {
    let mut backing = BTreeSet::new();
    for r in rels {
        backing.extend(r.backing_datasets());
    }
    // Only single-source, non-aggregated passthrough queries expose join-capable foreign keys.
    let [only_rel] = rels else {
        return RelationJoinOrigin {
            backing_datasets: backing,
            passthrough_cols: HashMap::new(),
            wildcard_dataset: None,
        };
    };
    if backing.len() != 1 || has_explicit_group_by_or_distinct(select) {
        return RelationJoinOrigin {
            backing_datasets: backing,
            passthrough_cols: HashMap::new(),
            wildcard_dataset: None,
        };
    }

    let mut passthrough = HashMap::new();
    let mut wildcard_dataset = None;
    for item in &select.projection {
        collect_passthrough_projection_item(
            item,
            only_rel,
            catalog,
            &mut passthrough,
            &mut wildcard_dataset,
        );
    }

    RelationJoinOrigin {
        backing_datasets: backing,
        passthrough_cols: passthrough,
        wildcard_dataset,
    }
}

fn collect_passthrough_projection_item(
    item: &SelectItem,
    only_rel: &JoinRelInstance,
    catalog: &TenantCatalog,
    out: &mut HashMap<String, (String, String)>,
    wildcard_dataset: &mut Option<String>,
) {
    match item {
        SelectItem::Wildcard(_) => {
            expand_all_rel_passthrough(only_rel, catalog, out, wildcard_dataset);
        }
        SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::ObjectName(obj), _) => {
            if let [ObjectNamePart::Identifier(q)] = obj.0.as_slice()
                && q.value.eq_ignore_ascii_case(&only_rel.alias)
            {
                expand_all_rel_passthrough(only_rel, catalog, out, wildcard_dataset);
            }
        }
        SelectItem::UnnamedExpr(expr) => {
            if let Some((out_col, origin_pair)) =
                resolve_direct_passthrough_expr(expr, None, only_rel, catalog)
            {
                out.insert(out_col, origin_pair);
            }
        }
        SelectItem::ExprWithAlias { expr, alias } => {
            if let Some((out_col, origin_pair)) =
                resolve_direct_passthrough_expr(expr, Some(alias.value.as_str()), only_rel, catalog)
            {
                out.insert(out_col, origin_pair);
            }
        }
        _ => {}
    }
}

fn expand_all_rel_passthrough(
    only_rel: &JoinRelInstance,
    catalog: &TenantCatalog,
    out: &mut HashMap<String, (String, String)>,
    wildcard_dataset: &mut Option<String>,
) {
    match &only_rel.source {
        JoinRelSource::Dataset { dataset_name } => {
            if let Some(ds) = catalog.dataset(dataset_name) {
                if ds.columns.is_empty() {
                    *wildcard_dataset = Some(ds.name.to_ascii_lowercase());
                } else {
                    populate_ds_passthrough(ds, out);
                }
            }
        }
        JoinRelSource::Derived { origin } => {
            for (k, v) in &origin.passthrough_cols {
                out.insert(k.clone(), v.clone());
            }
            if origin.wildcard_dataset.is_some() {
                wildcard_dataset.clone_from(&origin.wildcard_dataset);
            }
        }
    }
}

fn populate_ds_passthrough(ds: &CatalogDatasetEntry, out: &mut HashMap<String, (String, String)>) {
    let ds_lower = ds.name.to_ascii_lowercase();
    for col_lower in ds.columns.keys() {
        out.insert(col_lower.clone(), (ds_lower.clone(), col_lower.clone()));
    }
}

fn resolve_direct_passthrough_expr(
    expr: &Expr,
    explicit_alias: Option<&str>,
    only_rel: &JoinRelInstance,
    catalog: &TenantCatalog,
) -> Option<(String, (String, String))> {
    let col_ident = match unwrap_nested_expr(expr) {
        Expr::Identifier(ident) => Some(ident.value.as_str()),
        Expr::CompoundIdentifier(parts) => match parts.as_slice() {
            [qual, col] if qual.value.eq_ignore_ascii_case(&only_rel.alias) => {
                Some(col.value.as_str())
            }
            _ => None,
        },
        _ => None,
    }?;
    let origin_pair = only_rel.resolve_column(col_ident, catalog).ok()?;
    let out_name = explicit_alias
        .unwrap_or(col_ident)
        .trim()
        .to_ascii_lowercase();
    if out_name.is_empty() {
        return None;
    }
    Some((out_name, origin_pair))
}
