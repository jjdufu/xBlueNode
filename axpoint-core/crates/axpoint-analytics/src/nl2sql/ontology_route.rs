//! Stage 0 ontology and certified semantic-metric slot router (`02-STEP16` §16.2).
//!
//! Intercepts metric and mapped domain-object questions via constrained `JSON_SCHEMA` decoding
//! (`axpoint_ontology_slots`) and compiles them deterministically without free-form LLM SQL.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    AskContext, FilterOp, FilterSpec, MetricDef, MetricQuery, MetricQueryResult, MetricTimeRange,
    MultiMetricQuery, OntologySlotDraft, OntologySlotFilter, QueryResult,
};
use axpoint_domain::enums::{GroundingMode, Purpose};
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::json_schema::{JsonSchemaSpec, TenantOntologyEnums};
use axpoint_domain::ports::AiPort;
use axpoint_domain::studio::{MappingLinkSpec, MappingSpec, NodeTypeSpec, SchemaDoc};
use chrono::{DateTime, NaiveDate, TimeZone as _, Utc};
use sqlparser::ast::{BinaryOperator, Expr, InTarget, Value as SqlValue};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;

use crate::nl2sql::schema_link::{
    LinkedSchemaContext, hybrid_link_score, lexical_metric_score, question_matches_mapped_node_type,
};
use crate::nl2sql::{NL2SQL_MAX_ROWS, execute_guarded_conn};
use crate::pivot::quote_ident;
use crate::semantic::{
    MetricExecuteOptions, compile_metric_query_with_policy, execute_metric_query,
    execute_metric_query_conn, execute_multi_metric_query, execute_multi_metric_query_conn,
    is_metric_broken,
};
use crate::session::TenantDb;
use crate::sql_guard::{CatalogDatasetEntry, GuardOptions, GuardedSql, TenantCatalog, guard};

const MIN_METRIC_ROUTE_SCORE: f64 = 0.45;

const RAW_DETAIL_DIRECTIVES: &[&str] = &[
    "原始流水",
    "全部字段",
    "所有字段",
    "明细流水",
    "raw rows",
    "select *",
];

/// Outcome of Stage 0 ontology / semantic-metric slot routing (`02-STEP16` §16.2.2).
#[derive(Debug, Clone)]
pub enum OntologyRouteOutcome {
    /// Compiled and executed deterministically (`SEMANTIC_METRIC` or `DOMAIN_OBJECT`).
    Executed {
        /// Grounding path (`SemanticMetric` or `DomainObject`).
        grounding_mode: GroundingMode,
        /// User-facing compiled SQL (`AskResponse.sql`).
        clean_sql: String,
        /// Rewritten `DuckDB` SQL (`AskPipelineOutput.rewritten_sql`).
        rewritten_sql: String,
        /// Executed and PII-masked [`QueryResult`].
        qr: QueryResult,
        /// Deterministic explanation of the compiled slot query.
        explanation: String,
        /// Serialized slot payload (`MetricQuery`, `MultiMetricQuery`, or domain-object JSON).
        slot_payload: serde_json::Value,
    },
    /// Model requested explicit disambiguation before compiling slots.
    Clarify {
        /// Clarification prompt surfaced to the user.
        message: String,
        /// Optional raw slot payload.
        slot_payload: Option<serde_json::Value>,
    },
    /// Out-of-scope dimensions/expressions or `FALLBACK_SQL`: proceed to Stage 3 guarded SQL.
    FallbackSql,
}

/// Inputs bundled for Stage 0 slot compilation and execution.
pub struct OntologyCompileArgs<'a> {
    /// Extracted [`OntologySlotDraft`].
    pub draft: &'a OntologySlotDraft,
    /// Schema-linked context (`metrics`, `published_schemas`, `allowed_joins`).
    pub linked: &'a LinkedSchemaContext,
    /// Active tenant catalog.
    pub catalog: &'a TenantCatalog,
    /// Optional request scoping / dashboard filters.
    pub context: Option<&'a AskContext>,
    /// Optional query execution timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Optional compiled `AxGuard` security policy.
    pub policy: Option<&'a PolicyContext>,
}

/// Returns `true` when `question` matches a healthy semantic metric (`score >= 0.45`, with a
/// `+0.15` certified lexical boost) or a mapped `NodeTypeSpec`, and does not request raw unmodeled detail rows.
#[must_use]
pub fn should_attempt_ontology_route(
    question: &str,
    linked: &LinkedSchemaContext,
    catalog: &TenantCatalog,
) -> bool {
    let q_lower = question.trim().to_lowercase();
    if q_lower.is_empty() || RAW_DETAIL_DIRECTIVES.iter().any(|kw| q_lower.contains(kw)) {
        return false;
    }

    let best_metric = linked
        .metrics
        .iter()
        .filter(|m| !is_metric_broken(m, catalog))
        .map(|m| score_metric_for_ontology_route(question, m))
        .fold(0.0_f64, f64::max);

    if best_metric >= MIN_METRIC_ROUTE_SCORE {
        return true;
    }
    question_matches_mapped_node_type(question, &linked.published_schemas, catalog)
}

/// Computes the Stage 0 routing score for `metric`, adding `+0.15` when `metric.certified` has a strong lexical hit.
#[must_use]
pub fn score_metric_for_ontology_route(question: &str, metric: &MetricDef) -> f64 {
    let lex = lexical_metric_score(question, metric);
    let mut score = hybrid_link_score(lex, 0.0, metric.certified);
    if lex >= 0.70 {
        score += 0.10;
    }
    if metric.certified && lex >= 0.50 {
        score += 0.15;
    }
    score.clamp(0.0, 1.0)
}

/// Builds [`TenantOntologyEnums`] scoped to `linked` metrics, dimensions, and mapped ontology types.
#[must_use]
pub fn build_tenant_ontology_enums(
    linked: &LinkedSchemaContext,
    catalog: &TenantCatalog,
) -> TenantOntologyEnums {
    let mut enums = TenantOntologyEnums::default();
    for m in &linked.metrics {
        if is_metric_broken(m, catalog) {
            continue;
        }
        enums.metric_names.push(m.name.clone());
        let dims: Vec<String> = m.dimensions.iter().map(|d| d.name.clone()).collect();
        if !dims.is_empty() {
            enums.metric_dimensions.insert(m.name.clone(), dims);
        }
    }

    let mut obj_dims = Vec::new();
    for schema in &linked.published_schemas {
        collect_schema_enums(schema, catalog, &mut enums, &mut obj_dims);
    }
    if !obj_dims.is_empty() {
        enums
            .metric_dimensions
            .insert(String::from("__domain_props"), obj_dims);
    }
    for tbl in &linked.tables {
        enums.dataset_names.push(tbl.dataset_name.clone());
    }
    enums
}

fn collect_schema_enums(
    schema: &SchemaDoc,
    catalog: &TenantCatalog,
    enums: &mut TenantOntologyEnums,
    obj_dims: &mut Vec<String>,
) {
    let mapped_types: BTreeSet<String> = schema
        .mappings
        .iter()
        .filter(|m| catalog.dataset(&m.dataset).is_some())
        .map(|m| m.node_type.trim().to_owned())
        .collect();

    for nt in &schema.node_types {
        if mapped_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&nt.name))
        {
            if !enums.node_types.contains(&nt.name) {
                enums.node_types.push(nt.name.clone());
            }
            for prop in &nt.properties {
                if !obj_dims.contains(&prop.name) {
                    obj_dims.push(prop.name.clone());
                }
            }
        }
    }
    for et in &schema.edge_types {
        if !enums.edge_types.contains(&et.name) {
            enums.edge_types.push(et.name.clone());
        }
    }
    for m in &schema.mappings {
        for l in &m.links {
            if !l.link_type.trim().is_empty() && !enums.edge_types.contains(&l.link_type) {
                enums.edge_types.push(l.link_type.clone());
            }
        }
    }
}

/// Extracts structured [`OntologySlotDraft`] slots via `Purpose::Nl2Sql` constrained decoding (`02-STEP16` §16.2.2).
///
/// Returns `None` if constrained decoding is unavailable or misses an offline legacy cassette so callers
/// fall back seamlessly to Stage 3 SQL generation.
pub async fn extract_ontology_slots<A: AiPort>(
    ai: &A,
    question: &str,
    linked: &LinkedSchemaContext,
    catalog: &TenantCatalog,
    timezone: &str,
    now_rfc3339: &str,
    context: Option<&AskContext>,
) -> Option<OntologySlotDraft> {
    let enums = build_tenant_ontology_enums(linked, catalog);
    if enums.metric_names.is_empty() && enums.node_types.is_empty() {
        return None;
    }
    let schema = JsonSchemaSpec::ontology_slot_extraction(&enums);
    let sys_prompt = build_slot_system_prompt(now_rfc3339, timezone);
    let usr_prompt = build_slot_user_prompt(question, linked, catalog, context);

    let val = ai
        .chat_structured(Purpose::Nl2Sql, &sys_prompt, &usr_prompt, &schema)
        .await
        .ok()?;
    OntologySlotDraft::from_structured_value(&val).ok()
}

fn build_slot_system_prompt(now_rfc3339: &str, tz: &str) -> String {
    format!(
        "你是企业本体与认证指标槽位提取器。请严格按照绑定的 JSON Schema 提取结构化查询参数，严禁编写任何裸 SQL，严禁自行心算数值。\n\
         规则：\n\
         1. 单指标汇总/分组/趋势/同环比选 route=\"SINGLE_METRIC\"；多指标对比选 route=\"MULTI_METRIC\"；\n\
         2. 查询已发布业务实体属性及 1 跳关系选 route=\"DOMAIN_OBJECT\"；\n\
         3. 口径或对象歧义需用户确认选 route=\"CLARIFY\"；超出已定义指标/对象维度范围选 route=\"FALLBACK_SQL\"。\n\
         4. 相对时间词转化为半开区间 [time_from, time_to) 的 ISO-8601 字符串。\n\
         参考当前时间：{now_rfc3339}（租户时区 {tz}）"
    )
}

fn build_slot_user_prompt(
    question: &str,
    linked: &LinkedSchemaContext,
    catalog: &TenantCatalog,
    context: Option<&AskContext>,
) -> String {
    let mut out = String::from("### 可用认证与语义指标\n");
    if linked.metrics.is_empty() {
        out.push_str("（无）\n");
    } else {
        for m in &linked.metrics {
            if is_metric_broken(m, catalog) {
                continue;
            }
            let dims: Vec<&str> = m.dimensions.iter().map(|d| d.name.as_str()).collect();
            let cert_tag = if m.certified { "[认证]" } else { "[普通]" };
            let _fmt_ok = writeln!(
                out,
                "- {} {cert_tag}（{}）: 维度=[{}] | 默认粒度={:?}",
                m.name,
                m.display_name,
                dims.join(", "),
                m.default_grain
            );
        }
    }

    out.push_str("\n### 可用本体对象类型\n");
    let mut any_node = false;
    for schema in &linked.published_schemas {
        for nt in &schema.node_types {
            let props: Vec<&str> = nt.properties.iter().map(|p| p.name.as_str()).collect();
            let _fmt_ok = writeln!(
                out,
                "- {}（{}）: 属性=[{}]",
                nt.name,
                nt.display_name,
                props.join(", ")
            );
            any_node = true;
        }
    }
    if !any_node {
        out.push_str("（无）\n");
    }

    if let Some(ctx) = context
        && !ctx.filters.is_empty()
        && let Ok(f_json) = serde_json::to_string(&ctx.filters)
    {
        let _fmt_ok = writeln!(out, "\n### 上下文筛选条件\n{f_json}");
    }

    let _fmt_ok = write!(out, "\n### 槽位提取问题\n{}", question.trim());
    out
}

/// Deterministically compiles and executes extracted ontology/metric slots against an async [`TenantDb`].
///
/// # Errors
/// Propagates `POLICY_DENIED`, `PERMISSION_DENIED`, `QUERY_TIMEOUT`, or `Database`; out-of-scope
/// slot fields return `Ok(OntologyRouteOutcome::FallbackSql)`.
pub async fn compile_and_execute_slots_session(
    session: &TenantDb,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let route = args.draft.route.trim().to_ascii_uppercase();
    match route.as_str() {
        "CLARIFY" => Ok(build_clarify_outcome(args.draft)),
        "SINGLE_METRIC" if args.draft.metric_names.len() >= 2 => {
            execute_multi_metric_session(session, args).await
        }
        "SINGLE_METRIC" => execute_single_metric_session(session, args).await,
        "MULTI_METRIC" => execute_multi_metric_session(session, args).await,
        "DOMAIN_OBJECT" => execute_domain_object_session(session, args).await,
        _ => Ok(OntologyRouteOutcome::FallbackSql),
    }
}

/// Deterministically compiles and executes extracted ontology/metric slots on an open `duckdb::Connection`.
///
/// # Errors
/// Propagates `POLICY_DENIED`, `PERMISSION_DENIED`, or `Database`; out-of-scope slot fields return
/// `Ok(OntologyRouteOutcome::FallbackSql)`.
pub fn compile_and_execute_slots_conn(
    conn: &duckdb::Connection,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let route = args.draft.route.trim().to_ascii_uppercase();
    match route.as_str() {
        "CLARIFY" => Ok(build_clarify_outcome(args.draft)),
        "SINGLE_METRIC" if args.draft.metric_names.len() >= 2 => {
            execute_multi_metric_on_conn(conn, args)
        }
        "SINGLE_METRIC" => execute_single_metric_on_conn(conn, args),
        "MULTI_METRIC" => execute_multi_metric_on_conn(conn, args),
        "DOMAIN_OBJECT" => execute_domain_object_on_conn(conn, args),
        _ => Ok(OntologyRouteOutcome::FallbackSql),
    }
}

fn build_clarify_outcome(draft: &OntologySlotDraft) -> OntologyRouteOutcome {
    let message = if draft.clarification.trim().is_empty() {
        String::from("请确认您需要查询的具体业务指标、统计维度或时间范围。")
    } else {
        draft.clarification.trim().to_owned()
    };
    let slot_payload = serde_json::to_value(draft).ok();
    OntologyRouteOutcome::Clarify {
        message,
        slot_payload,
    }
}

async fn execute_single_metric_session(
    session: &TenantDb,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let Some((metric, metric_query, raw_sql)) = prepare_single_metric_plan(args)? else {
        return Ok(OntologyRouteOutcome::FallbackSql);
    };
    let opts = MetricExecuteOptions {
        timeout_ms: args.timeout_ms,
        reveal_pii: false,
        has_pii_raw_read: false,
        policy: args.policy.cloned(),
    };
    match execute_metric_query(session, args.catalog, metric, &metric_query, opts).await {
        Ok(res) => Ok(metric_result_to_outcome(
            &metric.display_name,
            raw_sql,
            res,
            &metric_query,
        )),
        Err(AppError::Forbidden(c)) => Err(AppError::Forbidden(c)),
        Err(AppError::Timeout(m)) => Err(AppError::Timeout(m)),
        Err(AppError::Database(m)) => Err(AppError::Database(m)),
        Err(_) => Ok(OntologyRouteOutcome::FallbackSql),
    }
}

fn execute_single_metric_on_conn(
    conn: &duckdb::Connection,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let Some((metric, metric_query, raw_sql)) = prepare_single_metric_plan(args)? else {
        return Ok(OntologyRouteOutcome::FallbackSql);
    };
    let opts = MetricExecuteOptions {
        timeout_ms: args.timeout_ms,
        reveal_pii: false,
        has_pii_raw_read: false,
        policy: args.policy.cloned(),
    };
    match execute_metric_query_conn(conn, args.catalog, metric, &metric_query, opts) {
        Ok(res) => Ok(metric_result_to_outcome(
            &metric.display_name,
            raw_sql,
            res,
            &metric_query,
        )),
        Err(AppError::Forbidden(c)) => Err(AppError::Forbidden(c)),
        Err(AppError::Database(m)) => Err(AppError::Database(m)),
        Err(_) => Ok(OntologyRouteOutcome::FallbackSql),
    }
}

fn prepare_single_metric_plan<'a>(
    args: &'a OntologyCompileArgs<'_>,
) -> Result<Option<(&'a MetricDef, MetricQuery, String)>, AppError> {
    let Some(target_name) = args.draft.metric_names.first() else {
        return Ok(None);
    };
    let Some(metric) = args
        .linked
        .metrics
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(target_name))
    else {
        return Ok(None);
    };
    let Some(filters) = collect_combined_filters(args.draft, args.context) else {
        return Ok(None);
    };
    let Ok(time_range) = parse_optional_time_range(args.draft) else {
        return Ok(None);
    };
    let metric_query = MetricQuery {
        dimensions: args.draft.dimensions.clone(),
        filters,
        time_range,
        grain: args.draft.grain,
        compare: args.draft.compare,
    };
    let compiled =
        match compile_metric_query_with_policy(metric, &metric_query, args.catalog, args.policy) {
            Ok(c) => c,
            Err(AppError::Forbidden(code)) => return Err(AppError::Forbidden(code)),
            Err(_) => return Ok(None),
        };
    Ok(Some((metric, metric_query, compiled.raw_sql)))
}

async fn execute_multi_metric_session(
    session: &TenantDb,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let Some(multi_query) = prepare_multi_metric_query(args) else {
        return Ok(OntologyRouteOutcome::FallbackSql);
    };
    if multi_query.metric_names.len() == 1 {
        return execute_single_metric_session(session, args).await;
    }
    let opts = MetricExecuteOptions {
        timeout_ms: args.timeout_ms,
        reveal_pii: false,
        has_pii_raw_read: false,
        policy: args.policy.cloned(),
    };
    match execute_multi_metric_query(
        session,
        args.catalog,
        &args.linked.metrics,
        &multi_query,
        opts,
    )
    .await
    {
        Ok(res) => {
            let clean_sql = res.sql.clone();
            Ok(metric_result_to_outcome(
                &multi_query.metric_names.join(", "),
                clean_sql,
                res,
                &multi_query,
            ))
        }
        Err(AppError::Forbidden(c)) => Err(AppError::Forbidden(c)),
        Err(AppError::Timeout(m)) => Err(AppError::Timeout(m)),
        Err(AppError::Database(m)) => Err(AppError::Database(m)),
        Err(_) => Ok(OntologyRouteOutcome::FallbackSql),
    }
}

fn execute_multi_metric_on_conn(
    conn: &duckdb::Connection,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let Some(multi_query) = prepare_multi_metric_query(args) else {
        return Ok(OntologyRouteOutcome::FallbackSql);
    };
    if multi_query.metric_names.len() == 1 {
        return execute_single_metric_on_conn(conn, args);
    }
    let opts = MetricExecuteOptions {
        timeout_ms: args.timeout_ms,
        reveal_pii: false,
        has_pii_raw_read: false,
        policy: args.policy.cloned(),
    };
    match execute_multi_metric_query_conn(
        conn,
        args.catalog,
        &args.linked.metrics,
        &multi_query,
        opts,
    ) {
        Ok(res) => {
            let clean_sql = res.sql.clone();
            Ok(metric_result_to_outcome(
                &multi_query.metric_names.join(", "),
                clean_sql,
                res,
                &multi_query,
            ))
        }
        Err(AppError::Forbidden(c)) => Err(AppError::Forbidden(c)),
        Err(AppError::Database(m)) => Err(AppError::Database(m)),
        Err(_) => Ok(OntologyRouteOutcome::FallbackSql),
    }
}

fn prepare_multi_metric_query(args: &OntologyCompileArgs<'_>) -> Option<MultiMetricQuery> {
    if args.draft.metric_names.is_empty() {
        return None;
    }
    for name in &args.draft.metric_names {
        let exists = args
            .linked
            .metrics
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case(name) && !is_metric_broken(m, args.catalog));
        if !exists {
            return None;
        }
    }
    let filters = collect_combined_filters(args.draft, args.context)?;
    let time_range = parse_optional_time_range(args.draft).ok()?;
    Some(MultiMetricQuery {
        metric_names: args.draft.metric_names.clone(),
        dimensions: args.draft.dimensions.clone(),
        filters,
        time_range,
        grain: args.draft.grain,
        compare: args.draft.compare,
    })
}

fn metric_result_to_outcome<S: serde::Serialize>(
    label: &str,
    clean_sql: String,
    res: MetricQueryResult,
    payload_spec: &S,
) -> OntologyRouteOutcome {
    let row_count = res.rows.len();
    let rewritten_sql = res.sql.clone();
    let qr = QueryResult {
        columns: res.columns,
        rows: res.rows,
        row_count,
        truncated: false,
        sql: res.sql,
        duration_ms: res.duration_ms,
    };
    let slot_payload = serde_json::to_value(payload_spec).unwrap_or(serde_json::Value::Null);
    OntologyRouteOutcome::Executed {
        grounding_mode: GroundingMode::SemanticMetric,
        clean_sql,
        rewritten_sql,
        qr,
        explanation: format!("已通过认证语义指标编译器确定性执行指标：{label}"),
        slot_payload,
    }
}

struct CompiledDomainPlan {
    node_type_name: String,
    raw_sql: String,
    guarded: GuardedSql,
    slot_payload: serde_json::Value,
}

async fn execute_domain_object_session(
    session: &TenantDb,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let Some(plan) = compile_domain_object_plan(args)? else {
        return Ok(OntologyRouteOutcome::FallbackSql);
    };
    let exec_out = session
        .execute_guarded(&plan.guarded, args.catalog, args.timeout_ms, false, false)
        .await?;
    Ok(OntologyRouteOutcome::Executed {
        grounding_mode: GroundingMode::DomainObject,
        clean_sql: plan.raw_sql,
        rewritten_sql: plan.guarded.sql,
        qr: exec_out.result,
        explanation: format!(
            "已通过已发布本体对象映射确定性投影：{}",
            plan.node_type_name
        ),
        slot_payload: plan.slot_payload,
    })
}

fn execute_domain_object_on_conn(
    conn: &duckdb::Connection,
    args: &OntologyCompileArgs<'_>,
) -> Result<OntologyRouteOutcome, AppError> {
    let Some(plan) = compile_domain_object_plan(args)? else {
        return Ok(OntologyRouteOutcome::FallbackSql);
    };
    let qr = execute_guarded_conn(conn, args.catalog, &plan.guarded)?;
    Ok(OntologyRouteOutcome::Executed {
        grounding_mode: GroundingMode::DomainObject,
        clean_sql: plan.raw_sql,
        rewritten_sql: plan.guarded.sql,
        qr,
        explanation: format!(
            "已通过已发布本体对象映射确定性投影：{}",
            plan.node_type_name
        ),
        slot_payload: plan.slot_payload,
    })
}

fn compile_domain_object_plan(
    args: &OntologyCompileArgs<'_>,
) -> Result<Option<CompiledDomainPlan>, AppError> {
    let Some(target_type) = args.draft.target_node_type.as_deref() else {
        return Ok(None);
    };
    let Some((node_spec, mapping, base_ds)) =
        find_mapped_node_type(target_type, &args.linked.published_schemas, args.catalog)
    else {
        return Ok(None);
    };
    let Some(mut projections) =
        build_domain_projections(node_spec, mapping, base_ds, &args.draft.dimensions)
    else {
        return Ok(None);
    };
    let Some((join_clauses, extra_projs)) = build_domain_link_joins(
        node_spec,
        mapping,
        &args.draft.include_links,
        &args.linked.published_schemas,
        args.catalog,
    ) else {
        return Ok(None);
    };
    projections.extend(extra_projs);

    let Some(filters) = collect_combined_filters(args.draft, args.context) else {
        return Ok(None);
    };
    let mut params = BTreeMap::new();
    let Some(where_clauses) = compile_domain_where(&filters, mapping, base_ds, &mut params) else {
        return Ok(None);
    };

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let join_sql = if join_clauses.is_empty() {
        String::new()
    } else {
        format!(" {}", join_clauses.join(" "))
    };
    let raw_sql = format!(
        "SELECT {} FROM {} AS t0{join_sql}{where_sql} ORDER BY t0.{} ASC",
        projections.join(", "),
        quote_ident(&base_ds.name),
        quote_ident(&mapping.key_column)
    );

    let guarded = match guard(
        &raw_sql,
        args.catalog,
        &GuardOptions {
            max_rows: NL2SQL_MAX_ROWS,
            allow_live: false,
            params,
            policy: args.policy.cloned(),
            strict_ontology_join: args.linked.enforce_ontology_joins,
            allowed_joins: args.linked.allowed_joins.clone(),
        },
    ) {
        Ok(g) => g,
        Err(AppError::Forbidden(code)) => return Err(AppError::Forbidden(code)),
        Err(_) => return Ok(None),
    };

    let slot_payload = serde_json::json!({
        "route": "DOMAIN_OBJECT",
        "target_node_type": node_spec.name,
        "dataset": base_ds.name,
        "dimensions": args.draft.dimensions,
        "include_links": args.draft.include_links,
        "filters": filters,
    });
    Ok(Some(CompiledDomainPlan {
        node_type_name: node_spec.name.clone(),
        raw_sql,
        guarded,
        slot_payload,
    }))
}

fn find_mapped_node_type<'a>(
    target: &str,
    schemas: &'a [SchemaDoc],
    catalog: &'a TenantCatalog,
) -> Option<(&'a NodeTypeSpec, &'a MappingSpec, &'a CatalogDatasetEntry)> {
    let node_spec = schemas
        .iter()
        .flat_map(|s| s.node_types.iter())
        .find(|nt| nt.name.eq_ignore_ascii_case(target))?;
    let mapping = schemas.iter().flat_map(|s| s.mappings.iter()).find(|m| {
        m.node_type.eq_ignore_ascii_case(&node_spec.name) && catalog.dataset(&m.dataset).is_some()
    })?;
    let base_ds = catalog.dataset(&mapping.dataset)?;
    if !base_ds
        .columns
        .contains_key(&mapping.key_column.to_ascii_lowercase())
    {
        return None;
    }
    Some((node_spec, mapping, base_ds))
}

fn build_domain_projections(
    _node_spec: &NodeTypeSpec,
    mapping: &MappingSpec,
    base_ds: &CatalogDatasetEntry,
    requested_dims: &[String],
) -> Option<Vec<String>> {
    let mut projs = vec![format!(
        "t0.{} AS \"node_key\"",
        quote_ident(&mapping.key_column)
    )];
    let mut seen_aliases = BTreeSet::from([String::from("node_key")]);

    if requested_dims.is_empty() {
        for (prop_name, col_name) in mapping.attributes.iter().chain(mapping.metrics.iter()) {
            if projs.len() >= 10 {
                break;
            }
            if base_ds.columns.contains_key(&col_name.to_ascii_lowercase())
                && seen_aliases.insert(prop_name.to_ascii_lowercase())
            {
                projs.push(format!(
                    "t0.{} AS {}",
                    quote_ident(col_name),
                    quote_ident(prop_name)
                ));
            }
        }
        return Some(projs);
    }

    for req in requested_dims {
        let phys_col = resolve_node_prop_to_column(req, mapping, base_ds)?;
        if seen_aliases.insert(req.to_ascii_lowercase()) {
            projs.push(format!(
                "t0.{} AS {}",
                quote_ident(&phys_col),
                quote_ident(req)
            ));
        }
    }
    Some(projs)
}

fn build_domain_link_joins(
    node_spec: &NodeTypeSpec,
    mapping: &MappingSpec,
    include_links: &[String],
    schemas: &[SchemaDoc],
    catalog: &TenantCatalog,
) -> Option<(Vec<String>, Vec<String>)> {
    let mut joins = Vec::new();
    let mut projs = Vec::new();
    let all_mappings: Vec<&MappingSpec> = schemas.iter().flat_map(|s| s.mappings.iter()).collect();

    for (idx, link_name) in include_links.iter().enumerate() {
        let alias = format!("t{}", idx.saturating_add(1));
        if let Some((join_sql, proj_sql)) = resolve_direct_or_reverse_link(
            node_spec,
            mapping,
            link_name,
            &alias,
            &all_mappings,
            catalog,
        ) {
            joins.push(join_sql);
            projs.push(proj_sql);
        } else {
            return None;
        }
    }
    Some((joins, projs))
}

fn resolve_direct_or_reverse_link(
    node_spec: &NodeTypeSpec,
    mapping: &MappingSpec,
    link_name: &str,
    alias: &str,
    all_mappings: &[&MappingSpec],
    catalog: &TenantCatalog,
) -> Option<(String, String)> {
    if let Some(direct) = mapping
        .links
        .iter()
        .find(|l| l.link_type.eq_ignore_ascii_case(link_name))
    {
        return build_outgoing_join_clause(direct, alias, all_mappings, catalog);
    }
    for other in all_mappings {
        let Some(rev) = other.links.iter().find(|l| {
            l.link_type.eq_ignore_ascii_case(link_name)
                && l.target_type.eq_ignore_ascii_case(&node_spec.name)
        }) else {
            continue;
        };
        let Some(other_ds) = catalog.dataset(&other.dataset) else {
            continue;
        };
        if !other_ds
            .columns
            .contains_key(&rev.target_key_column.to_ascii_lowercase())
            || !other_ds
                .columns
                .contains_key(&other.key_column.to_ascii_lowercase())
        {
            continue;
        }
        let join_sql = format!(
            "LEFT JOIN {} AS {alias} ON t0.{} = {alias}.{}",
            quote_ident(&other_ds.name),
            quote_ident(&mapping.key_column),
            quote_ident(&rev.target_key_column)
        );
        let proj_alias = format!("{}_key", other.node_type.to_ascii_lowercase());
        let proj_sql = format!(
            "{alias}.{} AS {}",
            quote_ident(&other.key_column),
            quote_ident(&proj_alias)
        );
        return Some((join_sql, proj_sql));
    }
    None
}

fn build_outgoing_join_clause(
    link: &MappingLinkSpec,
    alias: &str,
    all_mappings: &[&MappingSpec],
    catalog: &TenantCatalog,
) -> Option<(String, String)> {
    let dst_map = all_mappings.iter().copied().find(|m| {
        m.node_type.eq_ignore_ascii_case(&link.target_type) && catalog.dataset(&m.dataset).is_some()
    })?;
    let dst_ds = catalog.dataset(&dst_map.dataset)?;
    if !dst_ds
        .columns
        .contains_key(&dst_map.key_column.to_ascii_lowercase())
    {
        return None;
    }
    let join_sql = format!(
        "LEFT JOIN {} AS {alias} ON t0.{} = {alias}.{}",
        quote_ident(&dst_ds.name),
        quote_ident(&link.target_key_column),
        quote_ident(&dst_map.key_column)
    );
    let proj_alias = format!("{}_key", dst_map.node_type.to_ascii_lowercase());
    let proj_sql = format!(
        "{alias}.{} AS {}",
        quote_ident(&dst_map.key_column),
        quote_ident(&proj_alias)
    );
    Some((join_sql, proj_sql))
}

fn resolve_node_prop_to_column(
    prop_or_col: &str,
    mapping: &MappingSpec,
    base_ds: &CatalogDatasetEntry,
) -> Option<String> {
    let key = prop_or_col.trim();
    if mapping.key_column.eq_ignore_ascii_case(key) || key.eq_ignore_ascii_case("node_key") {
        return Some(mapping.key_column.clone());
    }
    for (p, col) in mapping.attributes.iter().chain(mapping.metrics.iter()) {
        if p.eq_ignore_ascii_case(key) && base_ds.columns.contains_key(&col.to_ascii_lowercase()) {
            return Some(col.clone());
        }
    }
    base_ds
        .columns
        .get(&key.to_ascii_lowercase())
        .map(|c| c.column_name.clone())
}

fn compile_domain_where(
    filters: &[FilterSpec],
    mapping: &MappingSpec,
    base_ds: &CatalogDatasetEntry,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Option<Vec<String>> {
    let mut clauses = Vec::with_capacity(filters.len());
    for (idx, flt) in filters.iter().enumerate() {
        let phys_col = resolve_node_prop_to_column(&flt.field, mapping, base_ds)?;
        let col_q = format!("t0.{}", quote_ident(&phys_col));
        let clause = compile_qualified_filter(&col_q, flt, idx, params)?;
        clauses.push(clause);
    }
    Some(clauses)
}

fn compile_qualified_filter(
    col_q: &str,
    flt: &FilterSpec,
    idx: usize,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Option<String> {
    match flt.op {
        FilterOp::IsNull => Some(format!("{col_q} IS NULL")),
        FilterOp::IsNotNull => Some(format!("{col_q} IS NOT NULL")),
        FilterOp::Eq
        | FilterOp::Ne
        | FilterOp::Gt
        | FilterOp::Gte
        | FilterOp::Lt
        | FilterOp::Lte
        | FilterOp::Like => {
            if flt.value.is_null() || flt.value.is_array() || flt.value.is_object() {
                return None;
            }
            let pname = format!("__df{idx}");
            params.insert(pname.clone(), flt.value.clone());
            let sql_op = match flt.op {
                FilterOp::Ne => "<>",
                FilterOp::Gt => ">",
                FilterOp::Gte => ">=",
                FilterOp::Lt => "<",
                FilterOp::Lte => "<=",
                FilterOp::Like => "LIKE",
                _ => "=",
            };
            Some(format!("{col_q} {sql_op} :{pname}"))
        }
        FilterOp::In | FilterOp::NotIn => {
            let arr = flt.value.as_array()?;
            if arr.is_empty() {
                return None;
            }
            let mut phs = Vec::with_capacity(arr.len());
            for (sub, item) in arr.iter().enumerate() {
                let pname = format!("__df{idx}_{sub}");
                params.insert(pname.clone(), item.clone());
                phs.push(format!(":{pname}"));
            }
            let kw = if flt.op == FilterOp::In {
                "IN"
            } else {
                "NOT IN"
            };
            Some(format!("{col_q} {kw} ({})", phs.join(", ")))
        }
        FilterOp::Between => None,
    }
}

fn collect_combined_filters(
    draft: &OntologySlotDraft,
    context: Option<&AskContext>,
) -> Option<Vec<FilterSpec>> {
    let mut out = Vec::new();
    if let Some(ctx) = context {
        out.extend(ctx.filters.iter().cloned());
    }
    for sf in &draft.filters {
        out.push(convert_slot_filter(sf)?);
    }
    for expr_str in &draft.filter_exprs {
        if !expr_str.trim().is_empty() {
            out.push(parse_compact_filter_expr(expr_str)?);
        }
    }
    Some(out)
}

fn convert_slot_filter(sf: &OntologySlotFilter) -> Option<FilterSpec> {
    let field = sf.field.trim();
    if field.is_empty() {
        return None;
    }
    let op = serde_json::from_value::<FilterOp>(serde_json::Value::String(
        sf.op.trim().to_ascii_uppercase(),
    ))
    .ok()?;
    let value = match op {
        FilterOp::IsNull | FilterOp::IsNotNull => serde_json::Value::Null,
        FilterOp::In | FilterOp::NotIn => {
            let items: Vec<serde_json::Value> = sf
                .value_text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(parse_scalar_text)
                .collect();
            if items.is_empty() {
                return None;
            }
            serde_json::Value::Array(items)
        }
        _ => parse_scalar_text(sf.value_text.trim()),
    };
    Some(FilterSpec {
        field: String::from(field),
        op,
        value,
    })
}

fn parse_scalar_text(raw: &str) -> serde_json::Value {
    let unquoted = raw
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(raw);
    if raw == unquoted {
        if let Ok(i) = unquoted.parse::<i64>() {
            return serde_json::Value::Number(serde_json::Number::from(i));
        }
        if let Ok(f) = unquoted.parse::<f64>()
            && let Some(n) = serde_json::Number::from_f64(f)
        {
            return serde_json::Value::Number(n);
        }
        if unquoted.eq_ignore_ascii_case("true") {
            return serde_json::Value::Bool(true);
        }
        if unquoted.eq_ignore_ascii_case("false") {
            return serde_json::Value::Bool(false);
        }
    }
    serde_json::Value::String(String::from(unquoted))
}

fn parse_compact_filter_expr(expr_str: &str) -> Option<FilterSpec> {
    let dialect = DuckDbDialect {};
    let mut parser = Parser::new(&dialect).try_with_sql(expr_str.trim()).ok()?;
    let expr = parser.parse_expr().ok()?;
    match expr {
        Expr::IsNull(inner) => Some(FilterSpec {
            field: extract_col_ident(&inner)?,
            op: FilterOp::IsNull,
            value: serde_json::Value::Null,
        }),
        Expr::IsNotNull(inner) => Some(FilterSpec {
            field: extract_col_ident(&inner)?,
            op: FilterOp::IsNotNull,
            value: serde_json::Value::Null,
        }),
        Expr::In {
            expr: inner,
            target: InTarget::List(list),
            negated,
            ..
        } => {
            let mut vals = Vec::with_capacity(list.len());
            for item in &list {
                vals.push(sql_expr_to_json(item)?);
            }
            Some(FilterSpec {
                field: extract_col_ident(&inner)?,
                op: if negated {
                    FilterOp::NotIn
                } else {
                    FilterOp::In
                },
                value: serde_json::Value::Array(vals),
            })
        }
        Expr::Like {
            negated: false,
            expr: inner,
            pattern,
            ..
        } => Some(FilterSpec {
            field: extract_col_ident(&inner)?,
            op: FilterOp::Like,
            value: sql_expr_to_json(&pattern)?,
        }),
        Expr::BinaryOp { left, op, right } => {
            let filter_op = match op {
                BinaryOperator::Eq => FilterOp::Eq,
                BinaryOperator::NotEq => FilterOp::Ne,
                BinaryOperator::Gt => FilterOp::Gt,
                BinaryOperator::GtEq => FilterOp::Gte,
                BinaryOperator::Lt => FilterOp::Lt,
                BinaryOperator::LtEq => FilterOp::Lte,
                _ => return None,
            };
            Some(FilterSpec {
                field: extract_col_ident(&left)?,
                op: filter_op,
                value: sql_expr_to_json(&right)?,
            })
        }
        _ => None,
    }
}

fn extract_col_ident(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(id) => Some(id.value.clone()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|id| id.value.clone()),
        _ => None,
    }
}

fn sql_expr_to_json(expr: &Expr) -> Option<serde_json::Value> {
    match expr {
        Expr::Value(val_span) => match &val_span.value {
            SqlValue::SingleQuotedString(s) | SqlValue::DoubleQuotedString(s) => {
                Some(serde_json::Value::String(s.clone()))
            }
            SqlValue::Number(n, _) => Some(parse_scalar_text(n)),
            SqlValue::Boolean(b) => Some(serde_json::Value::Bool(*b)),
            _ => None,
        },
        _ => None,
    }
}

fn parse_optional_time_range(draft: &OntologySlotDraft) -> Result<Option<MetricTimeRange>, ()> {
    match (&draft.time_from, &draft.time_to) {
        (None, None) => Ok(None),
        (Some(f_str), Some(t_str)) => {
            let from = parse_iso_timestamp(f_str).ok_or(())?;
            let to = parse_iso_timestamp(t_str).ok_or(())?;
            if to <= from {
                return Err(());
            }
            Ok(Some(MetricTimeRange { from, to }))
        }
        _ => Err(()),
    }
}

fn parse_iso_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(nd) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        && let Some(ndt) = nd.and_hms_opt(0, 0, 0)
    {
        return Some(Utc.from_utc_datetime(&ndt));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::analytics::{MetricDimension, SchemaRecallBundle};
    use axpoint_domain::axhub::ColumnMeta;
    use axpoint_domain::enums::{DataType, PiiCategory, RollupGrain};
    use axpoint_domain::studio::{LinkDirection, PropertyKind, PropertySpec};
    use chrono::Utc;
    use uuid::Uuid;

    use crate::nl2sql::schema_link::{SchemaLinkExtras, build_linked_schema};
    use crate::session::register_mask_udfs;

    fn build_entry(id: Uuid, name: &str, cols: Vec<ColumnMeta>) -> CatalogDatasetEntry {
        let mut columns = BTreeMap::new();
        for c in cols {
            columns.insert(c.column_name.to_ascii_lowercase(), c);
        }
        CatalogDatasetEntry {
            id,
            name: String::from(name),
            display_name: String::from(name),
            lake_table: String::from(name),
            schema_version: 1,
            row_count: 3,
            columns,
        }
    }

    fn demo_paid_metric(orders_id: Uuid) -> MetricDef {
        MetricDef {
            id: Uuid::from_u128(10),
            name: String::from("paid_amount"),
            display_name: String::from("已支付金额"),
            description: String::from("已支付订单总金额"),
            dataset_id: orders_id,
            expression: String::from("SUM(amount)"),
            filter_sql: String::from("status = 'PAID'"),
            time_column: Some(String::from("updated_at")),
            default_grain: RollupGrain::Month,
            dimensions: vec![
                MetricDimension {
                    name: String::from("status"),
                    column: String::from("status"),
                    display_name: String::from("状态"),
                },
                MetricDimension {
                    name: String::from("region"),
                    column: String::from("region"),
                    display_name: String::from("区域"),
                },
            ],
            unit: String::from("CNY"),
            format: String::from("0,0.00"),
            synonyms: vec![String::from("实收金额")],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn demo_schema_doc() -> SchemaDoc {
        SchemaDoc {
            node_types: vec![
                NodeTypeSpec {
                    name: String::from("Order"),
                    display_name: String::from("销售订单"),
                    properties: vec![PropertySpec {
                        name: String::from("status"),
                        display_name: String::from("状态"),
                        data_type: DataType::String,
                        kind: PropertyKind::Attribute,
                        ..PropertySpec::default()
                    }],
                    ..NodeTypeSpec::default()
                },
                NodeTypeSpec {
                    name: String::from("Warehouse"),
                    display_name: String::from("履约仓库"),
                    properties: vec![PropertySpec {
                        name: String::from("region"),
                        display_name: String::from("区域"),
                        data_type: DataType::String,
                        kind: PropertyKind::Attribute,
                        ..PropertySpec::default()
                    }],
                    ..NodeTypeSpec::default()
                },
            ],
            edge_types: Vec::new(),
            mappings: vec![
                MappingSpec {
                    name: String::from("orders_map"),
                    dataset: String::from("demo_orders"),
                    node_type: String::from("Order"),
                    key_column: String::from("order_id"),
                    title: String::from("{order_id}"),
                    filter: None,
                    attributes: BTreeMap::from([(String::from("status"), String::from("status"))]),
                    metrics: BTreeMap::new(),
                    metric_time_column: None,
                    geo: axpoint_domain::studio::MappingGeoSpec::default(),
                    links: vec![MappingLinkSpec {
                        link_type: String::from("FULFILLS"),
                        direction: LinkDirection::In,
                        target_type: String::from("Warehouse"),
                        target_key_column: String::from("warehouse_code"),
                        weight_column: None,
                        create_missing_target: false,
                    }],
                    on_source_delete: axpoint_domain::studio::OnSourceDelete::Keep,
                    mode: axpoint_domain::studio::MappingMode::Full,
                },
                MappingSpec {
                    name: String::from("wh_map"),
                    dataset: String::from("demo_warehouses"),
                    node_type: String::from("Warehouse"),
                    key_column: String::from("warehouse_code"),
                    title: String::from("{title}"),
                    filter: None,
                    attributes: BTreeMap::from([(String::from("region"), String::from("region"))]),
                    metrics: BTreeMap::new(),
                    metric_time_column: None,
                    geo: axpoint_domain::studio::MappingGeoSpec::default(),
                    links: Vec::new(),
                    on_source_delete: axpoint_domain::studio::OnSourceDelete::Keep,
                    mode: axpoint_domain::studio::MappingMode::Full,
                },
            ],
        }
    }

    fn demo_fixture()
    -> Result<(duckdb::Connection, TenantCatalog, Vec<MetricDef>, SchemaDoc), AppError> {
        let conn = duckdb::Connection::open_in_memory()
            .map_err(|e| AppError::Database(format!("open duckdb: {e}")))?;
        register_mask_udfs(&conn, "t1", None)?;
        conn.execute_batch(
            "ATTACH ':memory:' AS lake;
             CREATE SCHEMA lake.\"tenant_t1\";
             CREATE TABLE lake.\"tenant_t1\".\"demo_orders\" (order_id VARCHAR, warehouse_code VARCHAR, status VARCHAR, region VARCHAR, amount DOUBLE, updated_at TIMESTAMP);
             INSERT INTO lake.\"tenant_t1\".\"demo_orders\" VALUES
               ('O-1', 'W-1', 'PAID', '华东', 1200.0, '2026-01-10 08:00:00'),
               ('O-2', 'W-1', 'PAID', '华东', 800.0, '2026-01-15 09:00:00'),
               ('O-3', 'W-2', 'PENDING', '华南', 500.0, '2026-01-20 10:00:00');
             CREATE TABLE lake.\"tenant_t1\".\"demo_warehouses\" (warehouse_code VARCHAR, region VARCHAR, title VARCHAR);
             INSERT INTO lake.\"tenant_t1\".\"demo_warehouses\" VALUES ('W-1', '华东', '华东仓'), ('W-2', '华南', '华南仓');",
        )
        .map_err(|e| AppError::Database(format!("seed duckdb: {e}")))?;

        let orders_id = Uuid::from_u128(1);
        let wh_id = Uuid::from_u128(2);
        let mk_col = |ds_id: Uuid, pos: i32, name: &str, dt: DataType| ColumnMeta {
            dataset_id: ds_id,
            column_name: String::from(name),
            ordinal: pos,
            data_type: dt,
            pii_category: PiiCategory::None,
            pii_confirmed: false,
            description: String::new(),
            business_term: String::new(),
            unit: String::new(),
        };
        let catalog = TenantCatalog::new("t1")
            .with_dataset(build_entry(
                orders_id,
                "demo_orders",
                vec![
                    mk_col(orders_id, 1, "order_id", DataType::String),
                    mk_col(orders_id, 2, "warehouse_code", DataType::String),
                    mk_col(orders_id, 3, "status", DataType::String),
                    mk_col(orders_id, 4, "region", DataType::String),
                    mk_col(orders_id, 5, "amount", DataType::Float64),
                    mk_col(orders_id, 6, "updated_at", DataType::Timestamp),
                ],
            ))
            .with_dataset(build_entry(
                wh_id,
                "demo_warehouses",
                vec![
                    mk_col(wh_id, 1, "warehouse_code", DataType::String),
                    mk_col(wh_id, 2, "region", DataType::String),
                    mk_col(wh_id, 3, "title", DataType::String),
                ],
            ));

        Ok((
            conn,
            catalog,
            vec![demo_paid_metric(orders_id)],
            demo_schema_doc(),
        ))
    }

    #[test]
    fn certified_metric_triggers_stage0_and_compiles_exact_rows() -> Result<(), AppError> {
        let (conn, catalog, metrics, schema) = demo_fixture()?;
        let extras = SchemaLinkExtras {
            all_metrics: metrics.clone(),
            published_schemas: vec![schema],
            enforce_ontology_joins: true,
            ..SchemaLinkExtras::default()
        };
        let q = "按状态统计已支付订单金额（按月汇总）";
        let linked = build_linked_schema(q, &catalog, &SchemaRecallBundle::default(), &extras);
        assert!(should_attempt_ontology_route(q, &linked, &catalog));

        let draft_val = serde_json::json!({
            "route": "SINGLE_METRIC",
            "metric_names": ["paid_amount"],
            "target_node_type": "",
            "dimensions": ["status"],
            "filter_exprs": ["region = '华东'"],
            "time_from": "2026-01-01",
            "time_to": "2026-02-01",
            "grain": "MONTH",
            "compare": "",
            "include_links": [],
            "clarification": ""
        });
        let draft = OntologySlotDraft::from_structured_value(&draft_val)?;

        let outcome = compile_and_execute_slots_conn(
            &conn,
            &OntologyCompileArgs {
                draft: &draft,
                linked: &linked,
                catalog: &catalog,
                context: None,
                timeout_ms: None,
                policy: None,
            },
        );
        assert!(matches!(
            outcome,
            Ok(OntologyRouteOutcome::Executed {
                grounding_mode: GroundingMode::SemanticMetric,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn domain_object_route_projects_node_and_mapped_link() -> Result<(), AppError> {
        let (conn, catalog, metrics, schema) = demo_fixture()?;
        let extras = SchemaLinkExtras {
            all_metrics: metrics,
            published_schemas: vec![schema],
            enforce_ontology_joins: true,
            ..SchemaLinkExtras::default()
        };
        let q = "查询销售订单状态及关联履约仓库";
        let linked = build_linked_schema(q, &catalog, &SchemaRecallBundle::default(), &extras);
        assert!(should_attempt_ontology_route(q, &linked, &catalog));

        let draft = OntologySlotDraft {
            route: String::from("DOMAIN_OBJECT"),
            metric_names: Vec::new(),
            target_node_type: Some(String::from("Order")),
            dimensions: vec![String::from("status")],
            filters: vec![OntologySlotFilter {
                field: String::from("status"),
                op: String::from("EQ"),
                value_text: String::from("PAID"),
            }],
            filter_exprs: Vec::new(),
            time_from: None,
            time_to: None,
            grain: None,
            compare: None,
            include_links: vec![String::from("FULFILLS")],
            clarification: String::new(),
        };
        let outcome = compile_and_execute_slots_conn(
            &conn,
            &OntologyCompileArgs {
                draft: &draft,
                linked: &linked,
                catalog: &catalog,
                context: None,
                timeout_ms: None,
                policy: None,
            },
        );
        assert!(matches!(
            outcome,
            Ok(OntologyRouteOutcome::Executed {
                grounding_mode: GroundingMode::DomainObject,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn undeclared_metric_dimension_falls_back_to_sql() -> Result<(), AppError> {
        let (conn, catalog, metrics, schema) = demo_fixture()?;
        let extras = SchemaLinkExtras {
            all_metrics: metrics,
            published_schemas: vec![schema],
            enforce_ontology_joins: true,
            ..SchemaLinkExtras::default()
        };
        let linked = build_linked_schema(
            "按未知列统计已支付金额",
            &catalog,
            &SchemaRecallBundle::default(),
            &extras,
        );
        let draft = OntologySlotDraft {
            route: String::from("SINGLE_METRIC"),
            metric_names: vec![String::from("paid_amount")],
            target_node_type: None,
            dimensions: vec![String::from("warehouse_code")],
            filters: Vec::new(),
            filter_exprs: Vec::new(),
            time_from: None,
            time_to: None,
            grain: None,
            compare: None,
            include_links: Vec::new(),
            clarification: String::new(),
        };
        let outcome = compile_and_execute_slots_conn(
            &conn,
            &OntologyCompileArgs {
                draft: &draft,
                linked: &linked,
                catalog: &catalog,
                context: None,
                timeout_ms: None,
                policy: None,
            },
        );
        assert!(matches!(outcome, Ok(OntologyRouteOutcome::FallbackSql)));
        Ok(())
    }
}
