//! `AST` visitor rewriting `<alias>.<field>` / bare `<field>` symbols and `MATCH_KNOWLEDGE`
//! sentinel calls inside `AxQL` clauses (`05-STEP27` §27.2).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::studio::{NodeTypeSpec, SchemaDoc};
use sqlparser::ast::{
    Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr, Ident, InTarget,
    OrderByExpr, Select, SelectItem, Value, ValueWithSpan, VisitMut, VisitorMut,
};
use uuid::Uuid;

use crate::axql::match_plan::{
    MatchFieldBinding, ValidatedMatchPath, resolve_match_qualified_field,
    resolve_match_unqualified_field,
};
use crate::axql::object_plan::{ObjectFieldBinding, resolve_object_field};
use crate::axql::parser::{
    FN_AVG_BY_ENTITY, FN_MATCH_KNOWLEDGE, FN_SUM_BY_ENTITY, KnowledgeCallSpec, bad_axql,
};

pub(crate) enum SourceBindMode<'a> {
    Object {
        schema_doc: &'a SchemaDoc,
        main_spec: &'a NodeTypeSpec,
        alias: &'a str,
        bindings: &'a mut BTreeMap<String, ObjectFieldBinding>,
    },
    Match {
        validated: &'a ValidatedMatchPath,
        bindings: &'a mut BTreeMap<(String, String), MatchFieldBinding>,
    },
}

pub(crate) struct FieldRewriter<'a> {
    pub(crate) mode: SourceBindMode<'a>,
    pub(crate) table_qualifier: &'a str,
    pub(crate) select_aliases: &'a BTreeSet<String>,
    pub(crate) knowledge_calls: &'a [KnowledgeCallSpec],
    pub(crate) knowledge_results: &'a BTreeMap<usize, Vec<Uuid>>,
    pub(crate) bound_aliases: &'a BTreeSet<String>,
    pub(crate) default_knowledge_alias: &'a str,
    pub(crate) recalled_nodes: &'a mut BTreeSet<Uuid>,
    pub(crate) in_by_entity_second_arg: bool,
}

pub(crate) fn rewrite_outer_clauses(
    select: &mut Select,
    order_by_exprs: &mut [OrderByExpr],
    vis: &mut FieldRewriter<'_>,
) -> Result<(), AppError> {
    for item in &mut select.projection {
        match item {
            SelectItem::UnnamedExpr(expr) => {
                let direct_name = extract_direct_field_output_name(expr);
                if let ControlFlow::Break(err) = VisitMut::visit(expr, vis) {
                    return Err(err);
                }
                if let Some(col_alias) = direct_name {
                    *item = SelectItem::ExprWithAlias {
                        expr: expr.clone(),
                        alias: Ident::with_quote('"', col_alias),
                    };
                }
            }
            SelectItem::ExprWithAlias { expr, .. } | SelectItem::ExprWithAliases { expr, .. } => {
                if let ControlFlow::Break(err) = VisitMut::visit(expr, vis) {
                    return Err(err);
                }
            }
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => {
                return Err(bad_axql(
                    "AXQL_UNMAPPED_SYMBOL",
                    "AXQL_UNMAPPED_SYMBOL: wildcard SELECT * is forbidden in AxQL; list explicit ontology fields",
                ));
            }
        }
    }
    if let GroupByExpr::Expressions(ref mut g_exprs, _) = select.group_by {
        for ge in g_exprs {
            if let ControlFlow::Break(err) = VisitMut::visit(ge, vis) {
                return Err(err);
            }
        }
    }
    if let Some(ref mut having) = select.having
        && let ControlFlow::Break(err) = VisitMut::visit(having, vis)
    {
        return Err(err);
    }
    for ob in order_by_exprs {
        if let ControlFlow::Break(err) = VisitMut::visit(&mut ob.expr, vis) {
            return Err(err);
        }
    }
    Ok(())
}

fn extract_direct_field_output_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::CompoundIdentifier(idents) => idents.last().map(|i| i.value.clone()),
        Expr::Identifier(ident) => Some(ident.value.clone()),
        _ => None,
    }
}

impl VisitorMut for FieldRewriter<'_> {
    type Break = AppError;

    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(func) = expr {
            let fn_name = func.name.to_string();
            let clean = fn_name.trim_matches('"');
            if clean.eq_ignore_ascii_case(FN_MATCH_KNOWLEDGE) {
                return match self.expand_knowledge_call(func) {
                    Ok(expanded) => {
                        *expr = expanded;
                        ControlFlow::Continue(())
                    }
                    Err(err) => ControlFlow::Break(err),
                };
            }
            if clean.eq_ignore_ascii_case(FN_SUM_BY_ENTITY)
                || clean.eq_ignore_ascii_case(FN_AVG_BY_ENTITY)
            {
                if let FunctionArguments::List(ref mut list) = func.args
                    && let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(first_arg))) =
                        list.args.first_mut()
                    && let ControlFlow::Break(err) = VisitMut::visit(first_arg, self)
                {
                    return ControlFlow::Break(err);
                }
                self.in_by_entity_second_arg = true;
                return ControlFlow::Continue(());
            }
        }

        if self.in_by_entity_second_arg {
            return ControlFlow::Continue(());
        }

        match expr {
            Expr::CompoundIdentifier(idents) => match self.rewrite_compound(idents) {
                Ok(new_expr) => {
                    *expr = new_expr;
                    ControlFlow::Continue(())
                }
                Err(err) => ControlFlow::Break(err),
            },
            Expr::Identifier(ident) => match self.rewrite_bare_ident(ident) {
                Ok(Some(new_expr)) => {
                    *expr = new_expr;
                    ControlFlow::Continue(())
                }
                Ok(None) => ControlFlow::Continue(()),
                Err(err) => ControlFlow::Break(err),
            },
            _ => ControlFlow::Continue(()),
        }
    }

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(func) = expr {
            let clean = func.name.to_string();
            let trimmed = clean.trim_matches('"');
            if trimmed.eq_ignore_ascii_case(FN_SUM_BY_ENTITY)
                || trimmed.eq_ignore_ascii_case(FN_AVG_BY_ENTITY)
            {
                self.in_by_entity_second_arg = false;
            }
        }
        ControlFlow::Continue(())
    }
}

impl FieldRewriter<'_> {
    fn rewrite_compound(&mut self, idents: &[Ident]) -> Result<Expr, AppError> {
        let [raw_alias, raw_field] = idents else {
            return Err(bad_axql(
                "AXQL_UNMAPPED_SYMBOL",
                "AXQL_UNMAPPED_SYMBOL: expected <alias>.<field> reference in AxQL",
            ));
        };
        if raw_alias.value == self.table_qualifier {
            return Ok(Expr::CompoundIdentifier(idents.to_vec()));
        }
        match &mut self.mode {
            SourceBindMode::Object {
                schema_doc,
                main_spec,
                alias,
                bindings,
            } => {
                if !raw_alias.value.eq_ignore_ascii_case(alias) {
                    return Err(bad_axql(
                        "AXQL_UNMAPPED_SYMBOL",
                        format!(
                            "AXQL_UNMAPPED_SYMBOL: unknown object alias '{}'",
                            raw_alias.value
                        ),
                    ));
                }
                let resolved = resolve_object_field(schema_doc, main_spec, &raw_field.value)?;
                let canon = String::from(resolved.canonical_name());
                bindings.entry(canon.clone()).or_insert(resolved);
                Ok(Expr::CompoundIdentifier(vec![
                    Ident::with_quote('"', self.table_qualifier),
                    Ident::with_quote('"', format!("{alias}__{canon}")),
                ]))
            }
            SourceBindMode::Match {
                validated,
                bindings,
            } => {
                let (canon_alias, resolved) =
                    resolve_match_qualified_field(validated, &raw_alias.value, &raw_field.value)?;
                let canon_field = String::from(resolved.canonical_name());
                bindings
                    .entry((canon_alias.clone(), canon_field.clone()))
                    .or_insert(resolved);
                Ok(Expr::CompoundIdentifier(vec![
                    Ident::with_quote('"', self.table_qualifier),
                    Ident::with_quote('"', format!("{canon_alias}__{canon_field}")),
                ]))
            }
        }
    }

    fn rewrite_bare_ident(&mut self, ident: &Ident) -> Result<Option<Expr>, AppError> {
        if self
            .select_aliases
            .contains(&ident.value.to_ascii_lowercase())
        {
            return Ok(None);
        }
        match &mut self.mode {
            SourceBindMode::Object {
                schema_doc,
                main_spec,
                alias,
                bindings,
            } => {
                let resolved = resolve_object_field(schema_doc, main_spec, &ident.value)?;
                let canon = String::from(resolved.canonical_name());
                bindings.entry(canon.clone()).or_insert(resolved);
                Ok(Some(Expr::CompoundIdentifier(vec![
                    Ident::with_quote('"', self.table_qualifier),
                    Ident::with_quote('"', format!("{alias}__{canon}")),
                ])))
            }
            SourceBindMode::Match {
                validated,
                bindings,
            } => {
                let (canon_alias, resolved) =
                    resolve_match_unqualified_field(validated, &ident.value)?;
                let canon_field = String::from(resolved.canonical_name());
                bindings
                    .entry((canon_alias.clone(), canon_field.clone()))
                    .or_insert(resolved);
                Ok(Some(Expr::CompoundIdentifier(vec![
                    Ident::with_quote('"', self.table_qualifier),
                    Ident::with_quote('"', format!("{canon_alias}__{canon_field}")),
                ])))
            }
        }
    }

    fn expand_knowledge_call(&mut self, func: &Function) -> Result<Expr, AppError> {
        let FunctionArguments::List(list) = &func.args else {
            return Err(bad_axql("SQL_REJECTED", "invalid MATCH_KNOWLEDGE call"));
        };
        let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(val_span)))) =
            list.args.first()
        else {
            return Err(bad_axql("SQL_REJECTED", "invalid MATCH_KNOWLEDGE index"));
        };
        let Value::Number(idx_s, false) = &val_span.value else {
            return Err(bad_axql("SQL_REJECTED", "invalid MATCH_KNOWLEDGE index"));
        };
        let call_id: usize = idx_s
            .parse()
            .map_err(|_| bad_axql("SQL_REJECTED", "invalid MATCH_KNOWLEDGE call_id"))?;
        let spec = self
            .knowledge_calls
            .get(call_id)
            .ok_or_else(|| bad_axql("SQL_REJECTED", "unknown MATCH_KNOWLEDGE call_id"))?;

        let target_alias = if let Some(explicit) = &spec.target_alias {
            self.bound_aliases
                .iter()
                .find(|a| a.eq_ignore_ascii_case(explicit))
                .cloned()
                .ok_or_else(|| {
                    bad_axql(
                        "AXQL_UNMAPPED_SYMBOL",
                        format!(
                            "AXQL_UNMAPPED_SYMBOL: unknown alias '{explicit}' in MATCH_KNOWLEDGE"
                        ),
                    )
                })?
        } else if self.default_knowledge_alias.is_empty() {
            return Err(bad_axql("SQL_REJECTED", "missing target alias"));
        } else {
            String::from(self.default_knowledge_alias)
        };

        let ids = self
            .knowledge_results
            .get(&call_id)
            .map_or(&[][..], Vec::as_slice);
        for &u in ids {
            self.recalled_nodes.insert(u);
        }
        if ids.is_empty() {
            return Ok(Expr::Value(ValueWithSpan::from(Value::Boolean(false))));
        }

        let in_items: Vec<Expr> = ids
            .iter()
            .map(|id| {
                Expr::Value(ValueWithSpan::from(Value::SingleQuotedString(
                    id.to_string(),
                )))
            })
            .collect();
        Ok(Expr::In {
            expr: Box::new(Expr::CompoundIdentifier(vec![
                Ident::with_quote('"', self.table_qualifier),
                Ident::with_quote('"', format!("{target_alias}____entity_id")),
            ])),
            global: false,
            target: InTarget::List(in_items),
            negated: false,
        })
    }
}
