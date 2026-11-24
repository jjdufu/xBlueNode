//! `AxQL` AST rewriter assembling cross-tier `OBJECT(...)` folding, `MATCH` graph expansion,
//! `MATCH_KNOWLEDGE(...)` `IN (...)` pushdown, and `SUM_BY_ENTITY` window CTEs (`05-STEP27` §27.2).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::analytics::AxqlCompilePlan;
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::studio::{EdgeTypeSpec, SchemaDoc};
use sqlparser::ast::{
    Cte, Expr, Ident, ObjectName, OrderByExpr, Select, SelectItem, SetExpr, TableAlias,
    TableFactor, TableWithJoins, VisitMut, With,
};
use uuid::Uuid;

use crate::axql::fanout_guard::{
    build_ranked_cte, has_unprotected_fanout_agg, parse_cte, restore_order_by_exprs,
    rewrite_by_entity_aggregations, take_order_by_exprs,
};
use crate::axql::field_rewriter::{FieldRewriter, SourceBindMode, rewrite_outer_clauses};
use crate::axql::match_plan::{
    MatchFieldBinding, ValidatedMatchPath, build_match_base_cte, validate_match_path,
};
use crate::axql::object_plan::{
    ObjectBuildCtx, ObjectFieldBinding, build_object_ctes, lookup_node_type,
};
use crate::axql::parser::{AxqlFromSource, KnowledgeCallSpec, ParsedAxql, bad_axql};
use crate::sql_guard::TenantCatalog;

/// Rewrites a [`ParsedAxql`] into raw `DuckDB` SQL (prior to `sql_guard` wrapping) and
/// collects [`AxqlCompilePlan`] metadata.
///
/// # Errors
/// Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`, `JOIN_PATH_UNGROUNDED`, or `SQL_REJECTED`)
/// on any ontology symbol, edge path, or syntax violation.
pub fn rewrite_parsed_axql(
    parsed: &ParsedAxql,
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    policy: Option<&PolicyContext>,
    reveal_pii: bool,
    knowledge_results: &BTreeMap<usize, Vec<Uuid>>,
) -> Result<(String, AxqlCompilePlan), AppError> {
    let mut query = parsed.query.clone();
    if query.with.is_some() {
        return Err(bad_axql(
            "SQL_REJECTED",
            "manual WITH clauses are not supported in AxQL",
        ));
    }
    let mut order_by_exprs = take_order_by_exprs(&mut query.order_by);
    let SetExpr::Select(ref mut select_box) = *query.body else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "AxQL top-level query body must be SELECT",
        ));
    };
    let select: &mut Select = select_box.as_mut();
    let select_aliases = collect_select_aliases(select);

    match &parsed.source {
        AxqlFromSource::Object { type_name, alias } => rewrite_object_query(
            &mut query,
            order_by_exprs,
            &select_aliases,
            ObjectRewriteInput {
                type_name,
                alias,
                knowledge_calls: &parsed.knowledge_calls,
                knowledge_results,
                schema_doc,
                catalog,
                policy,
                reveal_pii,
            },
        ),
        AxqlFromSource::Match(path_spec) => {
            let validated = validate_match_path(schema_doc, path_spec)?;
            rewrite_match_query(
                &mut query,
                &mut order_by_exprs,
                &select_aliases,
                MatchRewriteInput {
                    path_spec,
                    validated: &validated,
                    knowledge_calls: &parsed.knowledge_calls,
                    knowledge_results,
                    schema_doc,
                    catalog,
                    policy,
                    reveal_pii,
                },
            )
        }
    }
}

#[derive(Clone, Copy)]
struct ObjectRewriteInput<'a> {
    type_name: &'a str,
    alias: &'a str,
    knowledge_calls: &'a [KnowledgeCallSpec],
    knowledge_results: &'a BTreeMap<usize, Vec<Uuid>>,
    schema_doc: &'a SchemaDoc,
    catalog: &'a TenantCatalog,
    policy: Option<&'a PolicyContext>,
    reveal_pii: bool,
}

#[derive(Clone, Copy)]
struct MatchRewriteInput<'a> {
    path_spec: &'a crate::axql::parser::MatchPathSpec,
    validated: &'a ValidatedMatchPath,
    knowledge_calls: &'a [KnowledgeCallSpec],
    knowledge_results: &'a BTreeMap<usize, Vec<Uuid>>,
    schema_doc: &'a SchemaDoc,
    catalog: &'a TenantCatalog,
    policy: Option<&'a PolicyContext>,
    reveal_pii: bool,
}

fn rewrite_object_query(
    query: &mut sqlparser::ast::Query,
    mut order_by_exprs: Vec<OrderByExpr>,
    select_aliases: &BTreeSet<String>,
    inp: ObjectRewriteInput<'_>,
) -> Result<(String, AxqlCompilePlan), AppError> {
    let main_spec = lookup_node_type(inp.schema_doc, inp.type_name)?;
    let bound_aliases = BTreeSet::from([String::from(inp.alias)]);
    let mut bindings: BTreeMap<String, ObjectFieldBinding> = BTreeMap::new();
    let mut recalled_set: BTreeSet<Uuid> = BTreeSet::new();

    let SetExpr::Select(ref mut select_box) = *query.body else {
        return Err(bad_axql("SQL_REJECTED", "expected SELECT"));
    };
    let select = select_box.as_mut();

    let rewritten_where = if let Some(mut w_expr) = select.selection.take() {
        let mut vis = FieldRewriter {
            mode: SourceBindMode::Object {
                schema_doc: inp.schema_doc,
                main_spec,
                alias: inp.alias,
                bindings: &mut bindings,
            },
            table_qualifier: "__base",
            select_aliases: &BTreeSet::new(),
            knowledge_calls: inp.knowledge_calls,
            knowledge_results: inp.knowledge_results,
            bound_aliases: &bound_aliases,
            default_knowledge_alias: inp.alias,
            recalled_nodes: &mut recalled_set,
            in_by_entity_second_arg: false,
        };
        if let ControlFlow::Break(err) = VisitMut::visit(&mut w_expr, &mut vis) {
            return Err(err);
        }
        Some(w_expr)
    } else {
        None
    };

    {
        let mut vis = FieldRewriter {
            mode: SourceBindMode::Object {
                schema_doc: inp.schema_doc,
                main_spec,
                alias: inp.alias,
                bindings: &mut bindings,
            },
            table_qualifier: "__ranked",
            select_aliases,
            knowledge_calls: inp.knowledge_calls,
            knowledge_results: inp.knowledge_results,
            bound_aliases: &bound_aliases,
            default_knowledge_alias: inp.alias,
            recalled_nodes: &mut recalled_set,
            in_by_entity_second_arg: false,
        };
        rewrite_outer_clauses(select, &mut order_by_exprs, &mut vis)?;
    }

    let dedup_entities =
        rewrite_by_entity_aggregations(select, &mut order_by_exprs, &bound_aliases, "__ranked")?;

    let effective_catalog = if inp.knowledge_calls.is_empty() {
        inp.catalog.clone()
    } else {
        TenantCatalog::new(&inp.catalog.tenant_id)
    };
    let obj_plan = build_object_ctes(
        ObjectBuildCtx {
            schema_doc: inp.schema_doc,
            catalog: &effective_catalog,
            policy: inp.policy,
            reveal_pii: inp.reveal_pii,
        },
        main_spec,
        inp.alias,
        &bindings,
    )?;

    let mut ctes = obj_plan.ctes;
    let fanout_protected = obj_plan.folded_child_tiers || !dedup_entities.is_empty();
    finalize_query_ctes(
        query,
        order_by_exprs,
        &mut ctes,
        rewritten_where,
        &dedup_entities,
    )?;

    let sql = query.to_string();
    let plan = AxqlCompilePlan {
        compiled_sql: sql.clone(),
        referenced_node_types: obj_plan.referenced_node_types,
        referenced_edge_types: obj_plan.referenced_edge_types,
        referenced_datasets: obj_plan.referenced_relations,
        dedup_entities,
        knowledge_recall_nodes: recalled_set.len(),
        fanout_protected,
    };
    Ok((sql, plan))
}

fn rewrite_match_query(
    query: &mut sqlparser::ast::Query,
    order_by_exprs: &mut Vec<OrderByExpr>,
    select_aliases: &BTreeSet<String>,
    inp: MatchRewriteInput<'_>,
) -> Result<(String, AxqlCompilePlan), AppError> {
    let mut field_bindings: BTreeMap<(String, String), MatchFieldBinding> = BTreeMap::new();
    let mut recalled_set: BTreeSet<Uuid> = BTreeSet::new();
    let head_alias = inp
        .validated
        .vertices
        .first()
        .map_or("", |(a, _)| a.as_str());

    let SetExpr::Select(ref mut select_box) = *query.body else {
        return Err(bad_axql("SQL_REJECTED", "expected SELECT"));
    };
    let select = select_box.as_mut();

    let rewritten_where = if let Some(mut w_expr) = select.selection.take() {
        let mut vis = FieldRewriter {
            mode: SourceBindMode::Match {
                validated: inp.validated,
                bindings: &mut field_bindings,
            },
            table_qualifier: "__base",
            select_aliases: &BTreeSet::new(),
            knowledge_calls: inp.knowledge_calls,
            knowledge_results: inp.knowledge_results,
            bound_aliases: &inp.validated.vertex_aliases,
            default_knowledge_alias: head_alias,
            recalled_nodes: &mut recalled_set,
            in_by_entity_second_arg: false,
        };
        if let ControlFlow::Break(err) = VisitMut::visit(&mut w_expr, &mut vis) {
            return Err(err);
        }
        Some(w_expr)
    } else {
        None
    };

    {
        let mut vis = FieldRewriter {
            mode: SourceBindMode::Match {
                validated: inp.validated,
                bindings: &mut field_bindings,
            },
            table_qualifier: "__ranked",
            select_aliases,
            knowledge_calls: inp.knowledge_calls,
            knowledge_results: inp.knowledge_results,
            bound_aliases: &inp.validated.vertex_aliases,
            default_knowledge_alias: head_alias,
            recalled_nodes: &mut recalled_set,
            in_by_entity_second_arg: false,
        };
        rewrite_outer_clauses(select, order_by_exprs, &mut vis)?;
    }

    let dedup_entities = rewrite_by_entity_aggregations(
        select,
        order_by_exprs,
        &inp.validated.vertex_aliases,
        "__ranked",
    )?;

    let match_plan = build_match_base_cte(
        inp.schema_doc,
        inp.catalog,
        inp.validated,
        &field_bindings,
        inp.policy,
        inp.reveal_pii,
        !inp.knowledge_calls.is_empty(),
    )?;

    let resolved_edges: Vec<EdgeTypeSpec> = inp
        .validated
        .edges
        .iter()
        .map(|(_, _, _, es)| es.clone())
        .collect();
    let unprotected =
        has_unprotected_fanout_agg(select, inp.path_spec, &resolved_edges, "__ranked");
    let fanout_protected = !dedup_entities.is_empty() && !unprotected;

    let mut ctes = vec![match_plan.base_cte];
    let taken_order = std::mem::take(order_by_exprs);
    finalize_query_ctes(
        query,
        taken_order,
        &mut ctes,
        rewritten_where,
        &dedup_entities,
    )?;

    let sql = query.to_string();
    let plan = AxqlCompilePlan {
        compiled_sql: sql.clone(),
        referenced_node_types: match_plan.referenced_node_types,
        referenced_edge_types: match_plan.referenced_edge_types,
        referenced_datasets: match_plan.referenced_relations,
        dedup_entities,
        knowledge_recall_nodes: recalled_set.len(),
        fanout_protected,
    };
    Ok((sql, plan))
}

fn finalize_query_ctes(
    query: &mut sqlparser::ast::Query,
    order_by_exprs: Vec<OrderByExpr>,
    ctes: &mut Vec<Cte>,
    rewritten_where: Option<Expr>,
    dedup_entities: &[String],
) -> Result<(), AppError> {
    let filtered_cte_name = if let Some(w_expr) = rewritten_where {
        let filtered_sql = format!("SELECT * FROM \"__axql_base\" AS \"__base\" WHERE {w_expr}");
        ctes.push(parse_cte("__axql_filtered", &filtered_sql)?);
        "__axql_filtered"
    } else {
        "__axql_base"
    };

    let SetExpr::Select(ref mut select_box) = *query.body else {
        return Err(bad_axql("SQL_REJECTED", "expected SELECT"));
    };
    let select = select_box.as_mut();

    let final_cte_name = if dedup_entities.is_empty() {
        filtered_cte_name
    } else {
        ctes.push(build_ranked_cte(
            "__axql_ranked",
            filtered_cte_name,
            dedup_entities,
            select,
            "__ranked",
        )?);
        "__axql_ranked"
    };

    select.from = vec![TableWithJoins {
        relation: TableFactor::Table {
            name: ObjectName::from(vec![Ident::with_quote('"', final_cte_name)]),
            alias: Some(TableAlias {
                explicit: true,
                name: Ident::with_quote('"', "__ranked"),
                columns: Vec::new(),
                at: None,
            }),
            args: None,
            with_hints: Vec::new(),
            version: None,
            with_ordinality: false,
            partitions: Vec::new(),
            json_path: None,
            sample: None,
            index_directives: Vec::new(),
            inheritance: None,
        },
        joins: Vec::new(),
    }];

    query.with = Some(With {
        with_token: sqlparser::ast::helpers::attached_token::AttachedToken::empty(),
        recursive: false,
        cte_tables: std::mem::take(ctes),
    });
    restore_order_by_exprs(&mut query.order_by, order_by_exprs);
    Ok(())
}

fn collect_select_aliases(select: &Select) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in &select.projection {
        if let SelectItem::ExprWithAlias { alias, .. } = item {
            out.insert(alias.value.to_ascii_lowercase());
        }
    }
    out
}
