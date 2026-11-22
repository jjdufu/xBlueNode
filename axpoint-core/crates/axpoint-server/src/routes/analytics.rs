//! `AxAnalytics` OLAP SQL, pivot, semantic layer, saved queries, NL2SQL, and AI transform routes (`05-STEP04`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use axpoint_analytics::nl2sql::schema_link::SchemaLinkExtras;
use axpoint_analytics::{
    AskPipelineInput, CatalogDatasetEntry, LiveConnectionSpec, MetricExecuteOptions,
    PivotExecuteOptions, TenantCatalog, TenantDb, compile_ontology_join_edges,
    ensure_no_dependency_cycle, execute_metric_query, execute_multi_metric_query, execute_pivot,
    generate_transform_sql, preview_transform_sql, run_grounding_eval_suite,
    validate_corrected_sql, validate_create_metric_request, validate_metric_def,
    validate_target_dataset_name,
};
use axpoint_domain::agent::{EndpointListQuery, EndpointSpec};
use axpoint_domain::analytics::{
    AnalyticsCatalogResponse, AnalyticsPrincipal, ApplyTransformResponse, AskContext,
    AskFeedbackRequest, AskIntent, AskRequest, AskResponse, AskTurnRecord, CatalogDatasetNode,
    CreateMetricRequest, CreateSavedQueryRequest, CreateTransformDraftRequest, ExplainResult,
    FewShotTurn, GroundingEvalRunRecord, GroundingEvalRunResponse, LiveConnectionNode, MetricDef,
    MetricQuery, MetricQueryResult, MultiMetricQuery, NewAskTurnInput, NewQueryHistoryEntry,
    NewTransformDraftInput, PivotResult, PivotSpec, PlatformViewNode, QueryColumn,
    QueryHistoryEntry, QueryHistoryKind, QueryHistoryListQuery, QueryHistoryStatus, QueryRequest,
    QueryResult, RunSavedQueryRequest, SavedQuery, SavedQueryKind, TransformDraft,
    TransformDraftStatus, UpdateMetricRequest, UpdateSavedQueryRequest,
    UpdateTransformDraftRequest,
};
use axpoint_domain::axhub::{
    CreateDatasetRequest, Dataset, DatasetListQuery, DatasetStatus, PaginatedList, SchemaPolicy,
};
use axpoint_domain::enums::{
    AuditAction, DataType, Environment, Permission, PiiCategory, SourceKind, SyncMode,
};
use axpoint_domain::guard::GrantResourceKind;
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    AgentRepository as _, AnalyticsQueryPort, AnalyticsRepository as _, ConnectionRepository as _,
    DatasetRepository as _, GuardRepository as _, StudioRepository as _,
};
use axpoint_domain::studio::SchemaDoc;
use axpoint_domain::{AppError, ChatQueryScope};
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::Utc;
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Builds the `/analytics/*` sub-router mounted under `/api`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/analytics/catalog", get(get_catalog_handler))
        .route("/analytics/sql/run", post(run_sql_handler))
        .route("/analytics/sql/explain", post(explain_sql_handler))
        .route("/analytics/pivot", post(run_pivot_handler))
        .route(
            "/analytics/metrics",
            get(list_metrics_handler).post(create_metric_handler),
        )
        .route(
            "/analytics/metrics/query-multi",
            post(query_multi_metrics_handler),
        )
        .route(
            "/analytics/metrics/{name}",
            get(get_metric_handler)
                .patch(update_metric_handler)
                .delete(delete_metric_handler),
        )
        .route(
            "/analytics/metrics/{name}/query",
            post(query_metric_handler),
        )
        .route(
            "/analytics/saved-queries",
            get(list_saved_queries_handler).post(create_saved_query_handler),
        )
        .route(
            "/analytics/saved-queries/{id}",
            get(get_saved_query_handler)
                .patch(update_saved_query_handler)
                .delete(delete_saved_query_handler),
        )
        .route(
            "/analytics/saved-queries/{id}/run",
            post(run_saved_query_handler),
        )
        .route("/analytics/ask", post(ask_data_handler))
        .route(
            "/analytics/ask/{turn_id}/feedback",
            post(submit_ask_feedback_handler),
        )
        .route(
            "/analytics/ask/sessions/{session_id}",
            get(get_ask_session_handler),
        )
        .route(
            "/analytics/transform/draft",
            post(create_transform_draft_handler),
        )
        .route(
            "/analytics/transform/{id}",
            patch(update_transform_draft_handler),
        )
        .route(
            "/analytics/transform/{id}/apply",
            post(apply_transform_draft_handler),
        )
        .route(
            "/analytics/transform/{id}/discard",
            post(discard_transform_draft_handler),
        )
        .route("/analytics/history", get(list_history_handler))
        .route(
            "/analytics/grounding-eval/run",
            post(run_grounding_eval_handler),
        )
        .route(
            "/analytics/grounding-eval/runs",
            get(list_grounding_eval_runs_handler),
        )
}

/// Loads all active datasets, column metadata, and live-query connection specs for `tenant_id`.
pub(crate) async fn load_tenant_catalog_and_live(
    state: &AppState,
    tenant_id: &str,
    has_live_permission: bool,
) -> Result<
    (
        TenantCatalog,
        Vec<Dataset>,
        Vec<CatalogDatasetNode>,
        Vec<LiveConnectionSpec>,
    ),
    AppError,
> {
    let mut all_datasets = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = state
            .store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    limit: Some(500),
                    cursor,
                    ..DatasetListQuery::default()
                },
            )
            .await?;
        all_datasets.extend(page.items);
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }

    let mut catalog = TenantCatalog::new(tenant_id);
    catalog.has_live_permission = has_live_permission;
    let mut nodes = Vec::new();

    for ds in &all_datasets {
        if ds.status == DatasetStatus::Disabled {
            continue;
        }
        let col_metas = state.store.list_columns(tenant_id, ds.id).await?;
        let entry = CatalogDatasetEntry::from_dataset(ds, &col_metas);
        let mut sorted_cols: Vec<_> = entry.columns.values().cloned().collect();
        sorted_cols.sort_by(|a, b| {
            a.ordinal
                .cmp(&b.ordinal)
                .then_with(|| a.column_name.cmp(&b.column_name))
        });
        nodes.push(CatalogDatasetNode {
            id: entry.id,
            name: entry.name.clone(),
            display_name: entry.display_name.clone(),
            lake_table: entry.lake_table.clone(),
            schema_version: entry.schema_version,
            row_count: entry.row_count,
            columns: sorted_cols,
        });
        catalog.insert_dataset(entry);
    }

    let conns = state.store.list_connections(tenant_id).await?;
    let live_specs = LiveConnectionSpec::from_connections(&conns);
    for spec in &live_specs {
        catalog.insert_live_connection(&spec.name);
    }

    let published_schemas = load_published_tenant_schemas(state, tenant_id, &catalog).await;
    catalog.allowed_joins = compile_ontology_join_edges(&published_schemas);
    catalog.published_schemas = published_schemas;

    Ok((catalog, all_datasets, nodes, live_specs))
}

async fn load_published_tenant_schemas(
    state: &AppState,
    tenant_id: &str,
    catalog: &TenantCatalog,
) -> Vec<SchemaDoc> {
    let mut slugs = vec![String::from("main")];
    if let Ok(subs) = state.store.list_subgraphs(tenant_id).await {
        for sg in subs {
            if !slugs.iter().any(|s| s == &sg.slug) {
                slugs.push(sg.slug);
            }
        }
    }
    let mut out = Vec::new();
    for slug in &slugs {
        if let Some(doc) = load_single_published_schema(state, tenant_id, slug, catalog).await {
            out.push(doc);
        }
    }
    out
}

pub(crate) async fn load_single_published_schema(
    state: &AppState,
    tenant_id: &str,
    slug: &str,
    catalog: &TenantCatalog,
) -> Option<SchemaDoc> {
    let resp = state
        .store
        .get_published_schema(tenant_id, slug)
        .await
        .ok()?;
    let mut doc = resp.doc;
    for m in &mut doc.mappings {
        if let Ok(ds_uuid) = Uuid::parse_str(m.dataset.trim())
            && let Some(entry) = catalog.dataset_by_id(ds_uuid)
        {
            m.dataset.clone_from(&entry.name);
        }
    }
    (!doc.node_types.is_empty() || !doc.mappings.is_empty()).then_some(doc)
}

/// Acquires or initializes the hardened `TenantDb` session for `tenant_id` with a fresh catalog.
pub(crate) async fn acquire_tenant_session(
    state: &AppState,
    tenant_id: &str,
    has_live_permission: bool,
) -> Result<(Arc<TenantDb>, TenantCatalog), AppError> {
    let (session, snap, _) =
        acquire_tenant_session_with_datasets(state, tenant_id, has_live_permission).await?;
    Ok((session, snap))
}

pub(crate) async fn acquire_tenant_session_with_datasets(
    state: &AppState,
    tenant_id: &str,
    has_live_permission: bool,
) -> Result<(Arc<TenantDb>, TenantCatalog, Vec<Dataset>), AppError> {
    let (catalog, all_datasets, _, live_specs) =
        load_tenant_catalog_and_live(state, tenant_id, has_live_permission).await?;
    let session = state
        .analytics_pool
        .get_or_try_init(tenant_id, catalog, &live_specs)
        .await?;
    let mut snap = session.catalog();
    snap.has_live_permission = has_live_permission;
    Ok((session, snap, all_datasets))
}

/// Refreshes the cached `TenantCatalog` inside `SessionPool` if `tenant_id` has an active session.
pub(crate) async fn refresh_cached_tenant_catalog(state: &AppState, tenant_id: &str) {
    if state.analytics_pool.get_cached(tenant_id).await.is_some()
        && let Ok((catalog, _, _, _)) = load_tenant_catalog_and_live(state, tenant_id, false).await
    {
        let _ = state
            .analytics_pool
            .refresh_catalog(tenant_id, catalog)
            .await;
    }
}

fn build_platform_view_nodes() -> Vec<PlatformViewNode> {
    let col = |name: &str, data_type: DataType| QueryColumn {
        name: String::from(name),
        data_type,
        pii_category: PiiCategory::None,
        masked: false,
    };
    vec![
        PlatformViewNode {
            name: String::from("ax.nodes"),
            description: String::from("业务图谱节点只读联邦视图"),
            columns: vec![
                col("id", DataType::String),
                col("type_name", DataType::String),
                col("node_key", DataType::String),
                col("title", DataType::String),
                col("status", DataType::String),
                col("props", DataType::Json),
                col("metrics", DataType::Json),
                col("tags", DataType::List),
                col("updated_at", DataType::Timestamp),
            ],
        },
        PlatformViewNode {
            name: String::from("ax.links"),
            description: String::from("业务图谱关系边只读联邦视图"),
            columns: vec![
                col("id", DataType::String),
                col("link_type", DataType::String),
                col("from_id", DataType::String),
                col("to_id", DataType::String),
                col("weight", DataType::Float64),
                col("props", DataType::Json),
                col("created_at", DataType::Timestamp),
            ],
        },
        PlatformViewNode {
            name: String::from("ax.metric_points"),
            description: String::from("节点时序指标明细只读联邦视图"),
            columns: vec![
                col("node_id", DataType::String),
                col("metric", DataType::String),
                col("ts", DataType::Timestamp),
                col("value", DataType::Float64),
                col("labels", DataType::Json),
            ],
        },
        PlatformViewNode {
            name: String::from("ax.metric_rollups"),
            description: String::from("节点时序预聚合只读联邦视图"),
            columns: vec![
                col("node_id", DataType::String),
                col("metric", DataType::String),
                col("grain", DataType::String),
                col("bucket", DataType::Timestamp),
                col("cnt", DataType::Int64),
                col("sum_val", DataType::Float64),
                col("min_val", DataType::Float64),
                col("max_val", DataType::Float64),
                col("last_val", DataType::Float64),
            ],
        },
    ]
}

fn error_http_status_i32(err: &AppError) -> i32 {
    match err {
        AppError::BadRequest { .. } => 400,
        AppError::Unauthorized(_) => 401,
        AppError::Forbidden(_) => 403,
        AppError::NotFound(_) => 404,
        AppError::Conflict(_) => 409,
        AppError::PayloadTooLarge(_) => 413,
        AppError::Unsupported(_) => 415,
        AppError::Unprocessable { .. } | AppError::UnprocessableDetails { .. } => 422,
        AppError::RateLimited => 429,
        AppError::Timeout(_) => 504,
        AppError::Config(_) | AppError::Database(_) => 500,
        AppError::Llm(_) | AppError::Parser(_) | AppError::Upstream(_) => 502,
    }
}

fn classify_history_status(err: &AppError) -> QueryHistoryStatus {
    if matches!(err.error_code(), "SQL_REJECTED" | "JOIN_PATH_UNGROUNDED")
        || matches!(err, AppError::Forbidden(_))
    {
        QueryHistoryStatus::Rejected
    } else if matches!(err, AppError::Timeout(_)) {
        QueryHistoryStatus::Timeout
    } else {
        QueryHistoryStatus::Failed
    }
}

#[derive(Clone)]
struct CallerCtx<'a> {
    tenant_id: &'a str,
    user_id: Uuid,
    username: &'a str,
    has_live: bool,
    has_pii_raw: bool,
    has_data_admin: bool,
    client_ip: String,
    auth: Option<&'a AuthPrincipal>,
    analytics: Option<&'a AnalyticsPrincipal>,
}

impl<'a> CallerCtx<'a> {
    fn from_auth(p: &'a AuthPrincipal) -> Self {
        Self::from_auth_with_ip(p, String::from("127.0.0.1"))
    }

    fn from_auth_with_ip(p: &'a AuthPrincipal, client_ip: String) -> Self {
        Self {
            tenant_id: &p.tenant_id,
            user_id: p.user_id,
            username: &p.username,
            has_live: p.has(Permission::DataSync),
            has_pii_raw: p.has(Permission::PiiRawRead),
            has_data_admin: p.has(Permission::DataAdmin),
            client_ip,
            auth: Some(p),
            analytics: None,
        }
    }

    fn from_analytics(p: &'a AnalyticsPrincipal) -> Self {
        Self {
            tenant_id: &p.tenant_id,
            user_id: p.user_id,
            username: &p.username,
            has_live: p.has_permission(Permission::DataSync),
            has_pii_raw: p.has_permission(Permission::PiiRawRead),
            has_data_admin: p.has_permission(Permission::DataAdmin),
            client_ip: String::from("127.0.0.1"),
            auth: None,
            analytics: Some(p),
        }
    }
}

async fn build_caller_policy(
    state: &AppState,
    caller: &CallerCtx<'_>,
    reveal_pii: bool,
) -> Result<axpoint_domain::guard::PolicyContext, AppError> {
    if let Some(auth) = caller.auth {
        return axpoint_server::policy::build_policy_context_for_store(
            &state.store,
            &state.cedar,
            auth,
            reveal_pii,
            &caller.client_ip,
        )
        .await;
    }
    if let Some(ap) = caller.analytics {
        let resolved = crate::agent_bridge::resolve_auth_principal(state, ap).await?;
        return axpoint_server::policy::build_policy_context_for_store(
            &state.store,
            &state.cedar,
            &resolved,
            reveal_pii,
            &caller.client_ip,
        )
        .await;
    }
    Err(AppError::Unauthorized("TOKEN_INVALID"))
}

async fn record_audit_and_flush(
    state: &AppState,
    caller: &CallerCtx<'_>,
    action: AuditAction,
    resource_ref: String,
    status_code: i32,
    detail_json: serde_json::Value,
) {
    let user_id = (!caller.user_id.is_nil()).then_some(caller.user_id);
    state.audit.record(AuditRecord {
        tenant_id: String::from(caller.tenant_id),
        user_id,
        username: String::from(caller.username),
        action,
        resource_ref,
        status_code,
        client_ip: caller.client_ip.clone(),
        trace_id: current_trace_id(),
        detail_json,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "analytics audit flush failed");
    }
}

async fn get_catalog_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<AnalyticsCatalogResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let (_, _, datasets, live_specs) = load_tenant_catalog_and_live(
        &state,
        &principal.tenant_id,
        principal.has(Permission::DataSync),
    )
    .await?;

    let live_connections = live_specs
        .into_iter()
        .map(|s| LiveConnectionNode {
            id: s.id,
            alias: format!("live_{}", s.name),
            name: s.name,
            connector_type: s.connector_type,
        })
        .collect();

    Ok(Json(AnalyticsCatalogResponse {
        datasets,
        platform_views: build_platform_view_nodes(),
        live_connections,
    }))
}

async fn execute_sql_with_telemetry(
    state: &AppState,
    caller: CallerCtx<'_>,
    req: QueryRequest,
) -> Result<QueryResult, AppError> {
    let started = Instant::now();
    let (session, _) = acquire_tenant_session(state, caller.tenant_id, caller.has_live).await?;

    let exec_outcome = async {
        let policy = build_caller_policy(state, &caller, req.reveal_pii).await?;
        let raw_pii = policy.reveal_pii_authorized;
        let out = session
            .execute_sql_with_policy(
                &req,
                caller.has_live,
                caller.has_pii_raw || raw_pii,
                Some(&policy),
            )
            .await?;
        Ok::<_, AppError>((out, raw_pii))
    }
    .await;

    match exec_outcome {
        Ok((out, raw_pii)) => {
            if let Err(err) = state
                .store
                .record_query_history(
                    caller.tenant_id,
                    NewQueryHistoryEntry {
                        user_id: (!caller.user_id.is_nil()).then_some(caller.user_id),
                        kind: QueryHistoryKind::Sql,
                        sql_text: req.sql.clone(),
                        status: QueryHistoryStatus::Succeeded,
                        row_count: i64::try_from(out.result.row_count).ok(),
                        duration_ms: i64::try_from(out.result.duration_ms).ok(),
                        error_code: None,
                    },
                )
                .await
            {
                tracing::warn!(error = %err, "record_query_history failed");
            }
            record_audit_and_flush(
                state,
                &caller,
                AuditAction::QuerySql,
                String::from("analytics:sql"),
                200,
                serde_json::json!({
                    "row_count": out.result.row_count,
                    "truncated": out.result.truncated,
                    "reveal_pii": req.reveal_pii,
                    "raw_pii": raw_pii,
                }),
            )
            .await;
            Ok(out.result)
        }
        Err(err) => {
            let elapsed_ms = i64::try_from(started.elapsed().as_millis()).ok();
            if let Err(hist_err) = state
                .store
                .record_query_history(
                    caller.tenant_id,
                    NewQueryHistoryEntry {
                        user_id: (!caller.user_id.is_nil()).then_some(caller.user_id),
                        kind: QueryHistoryKind::Sql,
                        sql_text: req.sql,
                        status: classify_history_status(&err),
                        row_count: None,
                        duration_ms: elapsed_ms,
                        error_code: Some(String::from(err.error_code())),
                    },
                )
                .await
            {
                tracing::warn!(error = %hist_err, "record_query_history failed");
            }
            record_audit_and_flush(
                state,
                &caller,
                AuditAction::QuerySql,
                String::from("analytics:sql"),
                error_http_status_i32(&err),
                serde_json::json!({ "error_code": err.error_code() }),
            )
            .await;
            Err(err)
        }
    }
}

async fn run_sql_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<QueryRequest>,
) -> Result<Json<QueryResult>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let res = execute_sql_with_telemetry(
        &state,
        CallerCtx::from_auth_with_ip(&principal, client_ip),
        req,
    )
    .await?;
    Ok(Json(res))
}

async fn explain_sql_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<QueryRequest>,
) -> Result<Json<ExplainResult>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let has_live = principal.has(Permission::DataSync);
    let (session, _) = acquire_tenant_session(&state, &principal.tenant_id, has_live).await?;
    let explained = session
        .explain_sql_with_policy(&req, has_live, Some(&policy))
        .await?;
    Ok(Json(explained))
}

async fn execute_pivot_with_telemetry(
    state: &AppState,
    caller: CallerCtx<'_>,
    spec: PivotSpec,
) -> Result<PivotResult, AppError> {
    let started = Instant::now();
    let (session, catalog) = acquire_tenant_session(state, caller.tenant_id, false).await?;
    let exec_outcome = async {
        let policy = build_caller_policy(state, &caller, false).await?;
        execute_pivot(
            &session,
            &catalog,
            &spec,
            PivotExecuteOptions {
                timeout_ms: None,
                reveal_pii: false,
                has_pii_raw_read: caller.has_pii_raw,
                policy: Some(policy),
            },
        )
        .await
    }
    .await;

    match exec_outcome {
        Ok(res) => {
            if let Err(err) = state
                .store
                .record_query_history(
                    caller.tenant_id,
                    NewQueryHistoryEntry {
                        user_id: (!caller.user_id.is_nil()).then_some(caller.user_id),
                        kind: QueryHistoryKind::Pivot,
                        sql_text: res.sql.clone(),
                        status: QueryHistoryStatus::Succeeded,
                        row_count: i64::try_from(res.cells.len()).ok(),
                        duration_ms: i64::try_from(res.duration_ms).ok(),
                        error_code: None,
                    },
                )
                .await
            {
                tracing::warn!(error = %err, "record_query_history failed");
            }
            Ok(res)
        }
        Err(err) => {
            if let Err(hist_err) = state
                .store
                .record_query_history(
                    caller.tenant_id,
                    NewQueryHistoryEntry {
                        user_id: (!caller.user_id.is_nil()).then_some(caller.user_id),
                        kind: QueryHistoryKind::Pivot,
                        sql_text: format!("PIVOT {}", spec.dataset),
                        status: classify_history_status(&err),
                        row_count: None,
                        duration_ms: i64::try_from(started.elapsed().as_millis()).ok(),
                        error_code: Some(String::from(err.error_code())),
                    },
                )
                .await
            {
                tracing::warn!(error = %hist_err, "record_query_history failed");
            }
            Err(err)
        }
    }
}

async fn run_pivot_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(spec): ApiJson<PivotSpec>,
) -> Result<Json<PivotResult>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let res = execute_pivot_with_telemetry(
        &state,
        CallerCtx::from_auth_with_ip(&principal, client_ip),
        spec,
    )
    .await?;
    Ok(Json(res))
}

async fn list_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<MetricDef>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let metrics = state.store.list_metrics(&principal.tenant_id).await?;
    Ok(Json(metrics))
}

async fn get_metric_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<MetricDef>, HttpError> {
    principal.require(Permission::DataRead)?;
    let metric = state
        .store
        .get_metric_by_name(&principal.tenant_id, &name)
        .await?
        .ok_or(AppError::NotFound("METRIC_NOT_FOUND"))?;
    Ok(Json(metric))
}

async fn create_metric_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateMetricRequest>,
) -> Result<Json<MetricDef>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    if req.certified {
        principal.require(Permission::StudioPublish)?;
    }
    let (catalog, _, _, _) =
        load_tenant_catalog_and_live(&state, &principal.tenant_id, false).await?;
    let schema_version = validate_create_metric_request(&req, &catalog)?;
    let created = state
        .store
        .create_metric(
            &principal.tenant_id,
            Some(principal.user_id),
            schema_version,
            req,
        )
        .await?;
    Ok(Json(created))
}

async fn update_metric_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(name): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateMetricRequest>,
) -> Result<Json<MetricDef>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    if req.certified.is_some() {
        principal.require(Permission::StudioPublish)?;
    }

    let existing = state
        .store
        .get_metric_by_name(&principal.tenant_id, &name)
        .await?
        .ok_or(AppError::NotFound("METRIC_NOT_FOUND"))?;

    let (catalog, _, _, _) =
        load_tenant_catalog_and_live(&state, &principal.tenant_id, false).await?;
    let ds_entry = catalog
        .dataset_by_id(existing.dataset_id)
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;

    let mut preview_def = existing.clone();
    if let Some(ref d) = req.display_name {
        preview_def.display_name.clone_from(d);
    }
    if let Some(ref d) = req.description {
        preview_def.description.clone_from(d);
    }
    if let Some(ref e) = req.expression {
        preview_def.expression.clone_from(e);
    }
    if let Some(ref f) = req.filter_sql {
        preview_def.filter_sql.clone_from(f);
    }
    if let Some(ref tc) = req.time_column {
        preview_def.time_column.clone_from(tc);
    }
    if let Some(g) = req.default_grain {
        preview_def.default_grain = g;
    }
    if let Some(ref dims) = req.dimensions {
        preview_def.dimensions.clone_from(dims);
    }
    validate_metric_def(&preview_def, &catalog)?;

    let updated = state
        .store
        .update_metric(&principal.tenant_id, &name, ds_entry.schema_version, req)
        .await?
        .ok_or(AppError::NotFound("METRIC_NOT_FOUND"))?;
    Ok(Json(updated))
}

async fn delete_metric_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let deleted = state
        .store
        .delete_metric(&principal.tenant_id, &name)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("METRIC_NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn execute_metric_def_with_telemetry(
    state: &AppState,
    caller: &CallerCtx<'_>,
    metric: &MetricDef,
    query: MetricQuery,
) -> Result<MetricQueryResult, AppError> {
    let policy = build_caller_policy(state, caller, false).await?;
    let (session, catalog) = acquire_tenant_session(state, caller.tenant_id, false).await?;
    let res = execute_metric_query(
        &session,
        &catalog,
        metric,
        &query,
        MetricExecuteOptions {
            timeout_ms: None,
            reveal_pii: false,
            has_pii_raw_read: caller.has_pii_raw,
            policy: Some(policy),
        },
    )
    .await?;
    if let Err(err) = state
        .store
        .record_query_history(
            caller.tenant_id,
            NewQueryHistoryEntry {
                user_id: (!caller.user_id.is_nil()).then_some(caller.user_id),
                kind: QueryHistoryKind::Metric,
                sql_text: res.sql.clone(),
                status: QueryHistoryStatus::Succeeded,
                row_count: i64::try_from(res.rows.len()).ok(),
                duration_ms: i64::try_from(res.duration_ms).ok(),
                error_code: None,
            },
        )
        .await
    {
        tracing::warn!(error = %err, "record_query_history failed");
    }
    Ok(res)
}

async fn execute_single_metric_with_telemetry(
    state: &AppState,
    caller: &CallerCtx<'_>,
    name: &str,
    query: MetricQuery,
) -> Result<MetricQueryResult, AppError> {
    let metric = state
        .store
        .get_metric_by_name(caller.tenant_id, name)
        .await?
        .ok_or(AppError::NotFound("METRIC_NOT_FOUND"))?;
    execute_metric_def_with_telemetry(state, caller, &metric, query).await
}

/// Executes a semantic metric against either a `PROD`-pinned [`MetricDef`] snapshot or live `an_metrics`.
pub(crate) async fn execute_published_metric(
    state: &AppState,
    principal: &AnalyticsPrincipal,
    metric_name: &str,
    pinned_snapshot: Option<&MetricDef>,
    query: MetricQuery,
) -> Result<MetricQueryResult, AppError> {
    if !principal.has_permission(Permission::DataRead) {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let caller = CallerCtx::from_analytics(principal);
    if let Some(pinned) = pinned_snapshot {
        return execute_metric_def_with_telemetry(state, &caller, pinned, query).await;
    }
    execute_single_metric_with_telemetry(state, &caller, metric_name, query).await
}

async fn query_metric_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(name): ApiPath<String>,
    ApiJson(query): ApiJson<MetricQuery>,
) -> Result<Json<MetricQueryResult>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let caller = CallerCtx::from_auth_with_ip(&principal, client_ip);
    let res = execute_single_metric_with_telemetry(&state, &caller, &name, query).await?;
    Ok(Json(res))
}

async fn query_multi_metrics_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(query): ApiJson<MultiMetricQuery>,
) -> Result<Json<MetricQueryResult>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let all_metrics = state.store.list_metrics(&principal.tenant_id).await?;
    let (session, catalog) = acquire_tenant_session(&state, &principal.tenant_id, false).await?;
    let res = execute_multi_metric_query(
        &session,
        &catalog,
        &all_metrics,
        &query,
        MetricExecuteOptions {
            timeout_ms: None,
            reveal_pii: false,
            has_pii_raw_read: principal.has(Permission::PiiRawRead),
            policy: Some(policy),
        },
    )
    .await?;
    if let Err(err) = state
        .store
        .record_query_history(
            &principal.tenant_id,
            NewQueryHistoryEntry {
                user_id: Some(principal.user_id),
                kind: QueryHistoryKind::Metric,
                sql_text: res.sql.clone(),
                status: QueryHistoryStatus::Succeeded,
                row_count: i64::try_from(res.rows.len()).ok(),
                duration_ms: i64::try_from(res.duration_ms).ok(),
                error_code: None,
            },
        )
        .await
    {
        tracing::warn!(error = %err, "record_query_history failed");
    }
    Ok(Json(res))
}

async fn list_saved_queries_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<SavedQuery>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let list = state
        .store
        .list_saved_queries(
            &principal.tenant_id,
            principal.user_id,
            principal.has(Permission::DataAdmin),
        )
        .await?;
    Ok(Json(list))
}

async fn get_saved_query_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<SavedQuery>, HttpError> {
    principal.require(Permission::DataRead)?;
    let sq = state
        .store
        .get_saved_query(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    if !sq.shared
        && sq.owner_user_id != Some(principal.user_id)
        && !principal.has(Permission::DataAdmin)
    {
        return Err(HttpError(AppError::NotFound("SAVED_QUERY_NOT_FOUND")));
    }
    Ok(Json(sq))
}

async fn create_saved_query_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateSavedQueryRequest>,
) -> Result<Json<SavedQuery>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    let created = state
        .store
        .create_saved_query(&principal.tenant_id, Some(principal.user_id), req)
        .await?;
    Ok(Json(created))
}

async fn update_saved_query_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateSavedQueryRequest>,
) -> Result<Json<SavedQuery>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    let existing = state
        .store
        .get_saved_query(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    if existing.owner_user_id != Some(principal.user_id) && !principal.has(Permission::DataAdmin) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let updated = state
        .store
        .update_saved_query(&principal.tenant_id, id, req)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    Ok(Json(updated))
}

async fn delete_saved_query_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::ViewEdit)?;
    let existing = state
        .store
        .get_saved_query(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    if existing.owner_user_id != Some(principal.user_id) && !principal.has(Permission::DataAdmin) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let deleted = state
        .store
        .delete_saved_query(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("SAVED_QUERY_NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn execute_saved_query_def(
    state: &AppState,
    caller: CallerCtx<'_>,
    sq: SavedQuery,
    mut params: BTreeMap<String, serde_json::Value>,
) -> Result<serde_json::Value, AppError> {
    for decl in &sq.parameters {
        if !params.contains_key(&decl.name)
            && let Some(ref def_val) = decl.default
        {
            params.insert(decl.name.clone(), def_val.clone());
        }
    }

    match sq.kind {
        SavedQueryKind::Sql | SavedQueryKind::Ask => {
            let sql_text = sq
                .spec
                .get("sql")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "saved query spec missing sql"))?
                .to_owned();
            let qr = execute_sql_with_telemetry(
                state,
                caller,
                QueryRequest {
                    sql: sql_text,
                    params,
                    ..QueryRequest::default()
                },
            )
            .await?;
            serde_json::to_value(qr)
                .map_err(|err| AppError::Config(format!("serialize QueryResult failed: {err}")))
        }
        SavedQueryKind::Pivot => {
            let spec: PivotSpec = serde_json::from_value(sq.spec).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid pivot spec: {err}"))
            })?;
            let pr = execute_pivot_with_telemetry(state, caller, spec).await?;
            serde_json::to_value(pr)
                .map_err(|err| AppError::Config(format!("serialize PivotResult failed: {err}")))
        }
        SavedQueryKind::Metric => {
            let metric_name = sq
                .spec
                .get("metric_name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "saved metric query missing metric_name")
                })?
                .to_owned();
            let query: MetricQuery = sq
                .spec
                .get("query")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|err| {
                    AppError::bad("INVALID_ARGUMENT", format!("invalid metric query: {err}"))
                })?
                .unwrap_or_default();
            let mr =
                execute_single_metric_with_telemetry(state, &caller, &metric_name, query).await?;
            serde_json::to_value(mr).map_err(|err| {
                AppError::Config(format!("serialize MetricQueryResult failed: {err}"))
            })
        }
    }
}

async fn run_saved_query_inner(
    state: &AppState,
    caller: CallerCtx<'_>,
    id: Uuid,
    params: BTreeMap<String, serde_json::Value>,
) -> Result<serde_json::Value, AppError> {
    let sq = state
        .store
        .get_saved_query(caller.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    if !sq.shared && sq.owner_user_id != Some(caller.user_id) && !caller.has_data_admin {
        return Err(AppError::NotFound("SAVED_QUERY_NOT_FOUND"));
    }
    execute_saved_query_def(state, caller, sq, params).await
}

/// Executes a published saved query against either a `PROD`-pinned [`SavedQuery`] snapshot or live `an_saved_queries`.
pub(crate) async fn execute_published_saved_query(
    state: &AppState,
    principal: &AnalyticsPrincipal,
    saved_id: Uuid,
    pinned_snapshot: Option<&SavedQuery>,
    params: BTreeMap<String, serde_json::Value>,
) -> Result<serde_json::Value, AppError> {
    if !principal.has_permission(Permission::DataRead) {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let caller = CallerCtx::from_analytics(principal);
    if let Some(pinned) = pinned_snapshot {
        return execute_saved_query_def(state, caller, pinned.clone(), params).await;
    }
    let sq = state
        .store
        .get_saved_query(caller.tenant_id, saved_id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    execute_saved_query_def(state, caller, sq, params).await
}

async fn run_saved_query_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<RunSavedQueryRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let val = run_saved_query_inner(
        &state,
        CallerCtx::from_auth_with_ip(&principal, client_ip),
        id,
        req.params,
    )
    .await?;
    Ok(Json(val))
}

/// Extracts and normalizes [`ChatQueryScope`] fields from an [`AskContext`] (`06-STEP20` §20.2.1).
pub(crate) fn scope_from_ask_context(ctx: Option<&AskContext>) -> Option<ChatQueryScope> {
    let c = ctx?;
    ChatQueryScope {
        dataset_ids: c.dataset_ids.clone(),
        doc_ids: c.doc_ids.clone(),
        doc_title_prefix: c.doc_title_prefix.clone(),
        subgraph_slug: c.subgraph_slug.clone(),
        endpoint_slugs: c.endpoint_slugs.clone(),
    }
    .normalized()
}

/// Converts a normalized [`ChatQueryScope`] into an [`AskContext`] (`06-STEP20` §20.2.1).
pub(crate) fn ask_context_from_scope(scope: Option<&ChatQueryScope>) -> Option<AskContext> {
    let s = scope?;
    Some(AskContext {
        dataset_ids: s.dataset_ids.clone(),
        doc_ids: s.doc_ids.clone(),
        doc_title_prefix: s.doc_title_prefix.clone(),
        subgraph_slug: s.subgraph_slug.clone(),
        endpoint_slugs: s.endpoint_slugs.clone(),
        ..AskContext::default()
    })
}

/// Enforces `AxGuard` `ABAC` intersection on `scope` (`403 POLICY_DENIED` on missing/denied datasets
/// or subgraphs) and returns pruned `PROD` [`EndpointSpec`]s when `endpoint_slugs` is non-empty (`06-STEP20` §20.2.2).
pub(crate) async fn authorize_and_resolve_query_scope(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    scope: Option<&ChatQueryScope>,
    all_datasets: &[Dataset],
    policy: &axpoint_domain::guard::PolicyContext,
) -> Result<Vec<EndpointSpec>, AppError> {
    let Some(s) = scope else {
        return Ok(Vec::new());
    };

    for ds_id in &s.dataset_ids {
        let Some(ds) = all_datasets
            .iter()
            .find(|d| d.id == *ds_id && d.status != DatasetStatus::Disabled)
        else {
            return Err(AppError::Forbidden("POLICY_DENIED"));
        };
        let ds_id_str = ds_id.to_string();
        if policy.denied_datasets.contains(&ds_id_str)
            || policy
                .denied_datasets
                .contains(&ds.name.to_ascii_lowercase())
        {
            return Err(AppError::Forbidden("POLICY_DENIED"));
        }
        let _res = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            principal,
            Permission::DataRead,
            GrantResourceKind::Dataset,
            &ds_id_str,
            client_ip,
        )
        .await?;
    }

    if let Some(ref sg) = s.subgraph_slug {
        let clean_sg = sg.trim();
        if !clean_sg.is_empty() && clean_sg != "main" {
            let exists = state
                .store
                .resolve_resource_ref(&principal.tenant_id, GrantResourceKind::Subgraph, clean_sg)
                .await?
                .is_some();
            if !exists {
                return Err(AppError::Forbidden("POLICY_DENIED"));
            }
            let _res = axpoint_server::policy::authorize_resource_for_store(
                &state.store,
                &state.cedar,
                principal,
                Permission::DataRead,
                GrantResourceKind::Subgraph,
                clean_sg,
                client_ip,
            )
            .await?;
        }
    }

    resolve_scoped_prod_endpoints(state, principal, &s.endpoint_slugs).await
}

/// Resolves and prunes candidate `PROD` [`EndpointSpec`]s to `endpoint_slugs` (`06-STEP20` §20.2.2).
pub(crate) async fn resolve_scoped_prod_endpoints(
    state: &AppState,
    principal: &AuthPrincipal,
    endpoint_slugs: &[String],
) -> Result<Vec<EndpointSpec>, AppError> {
    if endpoint_slugs.is_empty() {
        return Ok(Vec::new());
    }
    let prod_eps = state
        .store
        .list_endpoints(
            &principal.tenant_id,
            EndpointListQuery {
                environment: Some(Environment::Prod),
                ..EndpointListQuery::default()
            },
        )
        .await?;
    let mut matched = Vec::new();
    for ep in prod_eps {
        if ep.enabled
            && endpoint_slugs
                .iter()
                .any(|s| s.trim().eq_ignore_ascii_case(&ep.slug))
        {
            principal.require(ep.required_scope)?;
            matched.push(ep);
        }
    }
    Ok(matched)
}

/// Prunes `catalog` datasets/schemas and `all_metrics` to `scope`, returning excluded dataset names (`06-STEP20` §20.2.2).
pub(crate) async fn prune_catalog_and_metrics_for_scope(
    state: &AppState,
    tenant_id: &str,
    catalog: &mut TenantCatalog,
    all_datasets: &[Dataset],
    all_metrics: &mut Vec<MetricDef>,
    scope: Option<&ChatQueryScope>,
) -> Vec<String> {
    let Some(s) = scope else {
        return Vec::new();
    };

    let restrict_set: BTreeSet<Uuid> = s.dataset_ids.iter().copied().collect();
    let mut excluded_names = Vec::new();
    if !restrict_set.is_empty() {
        let mut seen_lower = BTreeSet::new();
        for ds in all_datasets {
            if ds.status == DatasetStatus::Disabled || restrict_set.contains(&ds.id) {
                continue;
            }
            if seen_lower.insert(ds.name.trim().to_lowercase()) {
                excluded_names.push(ds.name.clone());
            }
            let disp = ds.display_name.trim();
            if !disp.is_empty() && seen_lower.insert(disp.to_lowercase()) {
                excluded_names.push(String::from(disp));
            }
        }
        catalog.datasets.retain(|_, d| restrict_set.contains(&d.id));
        all_metrics.retain(|m| restrict_set.contains(&m.dataset_id));
    }

    if let Some(ref sg) = s.subgraph_slug {
        let clean_sg = sg.trim();
        if !clean_sg.is_empty() {
            let scoped_schemas = load_single_published_schema(state, tenant_id, clean_sg, catalog)
                .await
                .into_iter()
                .collect::<Vec<_>>();
            catalog.published_schemas = scoped_schemas;
        }
    }

    if !restrict_set.is_empty() || s.subgraph_slug.is_some() {
        catalog.allowed_joins = compile_ontology_join_edges(&catalog.published_schemas)
            .into_iter()
            .filter(|e| {
                catalog.dataset(&e.left_dataset).is_some()
                    && catalog.dataset(&e.right_dataset).is_some()
            })
            .collect();
    }

    excluded_names
}

fn filter_few_shots_for_excluded_datasets(
    few_shots: Vec<FewShotTurn>,
    excluded_dataset_names: &[String],
) -> Vec<FewShotTurn> {
    if excluded_dataset_names.is_empty() {
        return few_shots;
    }
    let lowered: Vec<String> = excluded_dataset_names
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| s.len() >= 3)
        .collect();
    few_shots
        .into_iter()
        .filter(|fs| {
            let sql_low = fs.sql_text.to_lowercase();
            !lowered.iter().any(|ex| sql_low.contains(ex))
        })
        .collect()
}

async fn build_ask_link_extras(
    session: &TenantDb,
    catalog: &TenantCatalog,
    all_metrics: Vec<MetricDef>,
    restrict_ids: Vec<Uuid>,
    excluded_dataset_names: Vec<String>,
    policy: &axpoint_domain::guard::PolicyContext,
) -> SchemaLinkExtras {
    let sample_rows_by_dataset =
        collect_sample_rows_by_dataset(session, catalog, &restrict_ids, policy).await;
    let enforce_ontology_joins = catalog
        .published_schemas
        .iter()
        .any(|s| !s.mappings.is_empty());
    SchemaLinkExtras {
        all_metrics,
        sample_rows_by_dataset,
        restrict_dataset_ids: restrict_ids,
        excluded_dataset_names,
        prior_session_sqls: Vec::new(),
        published_schemas: catalog.published_schemas.clone(),
        enforce_ontology_joins,
    }
}

async fn collect_sample_rows_by_dataset(
    session: &TenantDb,
    catalog: &TenantCatalog,
    restrict_ids: &[Uuid],
    policy: &axpoint_domain::guard::PolicyContext,
) -> HashMap<Uuid, Vec<serde_json::Map<String, serde_json::Value>>> {
    let mut out = HashMap::new();
    let mut count = 0_usize;
    for entry in catalog.datasets.values() {
        if !restrict_ids.is_empty() && !restrict_ids.contains(&entry.id) {
            continue;
        }
        if policy.denied_datasets.contains(&entry.id.to_string())
            || policy
                .denied_datasets
                .contains(&entry.name.to_ascii_lowercase())
        {
            continue;
        }
        if entry.row_count <= 0 {
            continue;
        }
        let sample_req = QueryRequest {
            sql: format!(
                "SELECT * FROM \"{}\" LIMIT 3",
                entry.name.replace('"', "\"\"")
            ),
            max_rows: Some(3),
            timeout_ms: Some(2_000),
            ..QueryRequest::default()
        };
        if let Ok(res) = session
            .execute_sql_with_policy(&sample_req, false, false, Some(policy))
            .await
        {
            out.insert(entry.id, res.result.rows);
        }
        count = count.saturating_add(1);
        if count >= 5 {
            break;
        }
    }
    out
}

async fn persist_ask_turn_and_history(
    state: &AppState,
    principal: &AuthPrincipal,
    question: &str,
    persist_turn: bool,
    duration_ms: u64,
    repair_rounds: usize,
    resp: &mut AskResponse,
) -> Result<(), AppError> {
    if persist_turn {
        let preview = (!resp.sql.is_empty()).then(|| {
            serde_json::json!({
                "columns": resp.columns,
                "rows": resp.rows,
                "truncated": resp.truncated,
                "chart": resp.chart,
            })
        });
        let turn_row = state
            .store
            .insert_ask_turn(
                &principal.tenant_id,
                NewAskTurnInput {
                    session_id: resp.session_id,
                    user_id: (!principal.user_id.is_nil()).then_some(principal.user_id),
                    question: question.trim().to_owned(),
                    intent: resp.intent,
                    linked_objects: resp.linked_objects.clone(),
                    sql_text: (!resp.sql.is_empty()).then(|| resp.sql.clone()),
                    result_preview: preview,
                    answer: resp.answer.clone(),
                    citations: resp.citations.clone(),
                    model_name: String::from("axpoint-llm"),
                    grounding_mode: resp.grounding_mode,
                    verified_numbers: resp.verified_numbers,
                    verified_citations: resp.verified_citations,
                    abstained: resp.abstained,
                    slot_payload: resp.slot_payload.clone(),
                },
            )
            .await?;
        resp.turn_id = turn_row.id;
    }

    if (!resp.sql.is_empty()
        || matches!(
            resp.status,
            QueryHistoryStatus::Failed | QueryHistoryStatus::Rejected
        ))
        && let Err(err) = state
            .store
            .record_query_history(
                &principal.tenant_id,
                NewQueryHistoryEntry {
                    user_id: (!principal.user_id.is_nil()).then_some(principal.user_id),
                    kind: QueryHistoryKind::Ask,
                    sql_text: if resp.sql.is_empty() {
                        String::from(question)
                    } else {
                        resp.sql.clone()
                    },
                    status: resp.status,
                    row_count: i64::try_from(resp.rows.len()).ok(),
                    duration_ms: i64::try_from(duration_ms).ok(),
                    error_code: match resp.status {
                        QueryHistoryStatus::Rejected => Some(String::from("JOIN_PATH_UNGROUNDED")),
                        QueryHistoryStatus::Failed => Some(String::from("SQL_REPAIR_EXHAUSTED")),
                        _ => None,
                    },
                },
            )
            .await
    {
        tracing::warn!(error = %err, "record_query_history failed");
    }

    let caller = CallerCtx::from_auth(principal);
    record_audit_and_flush(
        state,
        &caller,
        AuditAction::AskData,
        format!("ask_turn:{}", resp.turn_id),
        200,
        serde_json::json!({
            "session_id": resp.session_id,
            "intent": resp.intent,
            "status": resp.status,
            "repair_rounds": repair_rounds,
        }),
    )
    .await;
    Ok(())
}

/// Runs the controlled NL2SQL pipeline for `principal`, persisting `an_ask_turns` (when `persist_turn`
/// is `true`), `an_query_history`, and `AuditAction::AskData`.
pub(crate) async fn execute_ask_for_principal(
    state: &AppState,
    principal: &AuthPrincipal,
    req: AskRequest,
    force_intent: Option<AskIntent>,
    persist_turn: bool,
) -> Result<AskResponse, AppError> {
    execute_ask_for_principal_with_ip(
        state,
        principal,
        "127.0.0.1",
        req,
        force_intent,
        persist_turn,
    )
    .await
}

struct PreparedScopedAskContext {
    session: Arc<TenantDb>,
    catalog: TenantCatalog,
    policy: axpoint_domain::guard::PolicyContext,
    db_recall: axpoint_domain::analytics::SchemaRecallBundle,
    link_extras: SchemaLinkExtras,
    few_shots: Vec<FewShotTurn>,
    session_turns: Vec<AskTurnRecord>,
    timezone: String,
}

async fn prepare_scoped_ask_context(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    req: &AskRequest,
) -> Result<PreparedScopedAskContext, AppError> {
    let scope = scope_from_ask_context(req.context.as_ref());
    let subgraph_slug = scope
        .as_ref()
        .and_then(|s| s.subgraph_slug.as_deref())
        .unwrap_or("main");
    let policy = axpoint_server::policy::build_policy_context_for_subgraph(
        &state.store,
        &state.cedar,
        principal,
        subgraph_slug,
        false,
        client_ip,
    )
    .await?;
    let (session, mut catalog, all_datasets) =
        acquire_tenant_session_with_datasets(state, &principal.tenant_id, false).await?;
    let _pinned_endpoints = authorize_and_resolve_query_scope(
        state,
        principal,
        client_ip,
        scope.as_ref(),
        &all_datasets,
        &policy,
    )
    .await?;

    let restrict_ids: Vec<Uuid> = scope
        .as_ref()
        .map_or_else(Vec::new, |s| s.dataset_ids.clone());
    let recall_subgraph = scope.as_ref().and_then(|s| s.subgraph_slug.as_deref());
    let q_embed = state
        .ai
        .embed_one(req.question.trim())
        .await
        .ok()
        .map(|(v, _)| v);

    let (db_recall, mut all_metrics, raw_few_shots, timezone) = tokio::try_join!(
        state.store.recall_schema_candidates_scoped(
            &principal.tenant_id,
            req.question.trim(),
            q_embed.as_deref(),
            &restrict_ids,
            recall_subgraph,
        ),
        state.store.list_metrics(&principal.tenant_id),
        state
            .store
            .search_few_shot_turns(&principal.tenant_id, req.question.trim(), 5),
        state.store.get_tenant_timezone(&principal.tenant_id),
    )?;

    let excluded_names = prune_catalog_and_metrics_for_scope(
        state,
        &principal.tenant_id,
        &mut catalog,
        &all_datasets,
        &mut all_metrics,
        scope.as_ref(),
    )
    .await;
    let few_shots = filter_few_shots_for_excluded_datasets(raw_few_shots, &excluded_names);
    let session_turns = match req.session_id {
        Some(sid) => {
            state
                .store
                .list_session_turns(&principal.tenant_id, sid)
                .await?
        }
        None => Vec::new(),
    };
    let link_extras = build_ask_link_extras(
        &session,
        &catalog,
        all_metrics,
        restrict_ids,
        excluded_names,
        &policy,
    )
    .await;

    Ok(PreparedScopedAskContext {
        session,
        catalog,
        policy,
        db_recall,
        link_extras,
        few_shots,
        session_turns,
        timezone,
    })
}

pub(crate) async fn execute_ask_for_principal_with_ip(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    req: AskRequest,
    force_intent: Option<AskIntent>,
    persist_turn: bool,
) -> Result<AskResponse, AppError> {
    if req.question.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "question cannot be empty",
        ));
    }

    let ctx = prepare_scoped_ask_context(state, principal, client_ip, &req).await?;
    let nl2sql_ai = state
        .ai
        .for_tenant(&principal.tenant_id)
        .for_user(principal.user_id)
        .with_purpose_override(axpoint_domain::enums::Purpose::Nl2Sql);
    let out = axpoint_analytics::nl2sql::run_ask_pipeline_with_policy(
        &nl2sql_ai,
        &ctx.session,
        &AskPipelineInput {
            request: &req,
            catalog: &ctx.catalog,
            db_recall: &ctx.db_recall,
            link_extras: &ctx.link_extras,
            few_shots: &ctx.few_shots,
            session_turns: &ctx.session_turns,
            timezone: &ctx.timezone,
            now_rfc3339_override: None,
            timeout_ms: None,
            force_intent,
        },
        Some(&ctx.policy),
    )
    .await?;

    let mut resp = out.response;
    persist_ask_turn_and_history(
        state,
        principal,
        &req.question,
        persist_turn,
        out.duration_ms,
        out.repair_rounds,
        &mut resp,
    )
    .await?;

    Ok(resp)
}

async fn ask_data_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<AskRequest>,
) -> Result<Json<AskResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let resp =
        execute_ask_for_principal_with_ip(&state, &principal, &client_ip, req, None, true).await?;
    Ok(Json(resp))
}

async fn submit_ask_feedback_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(turn_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<AskFeedbackRequest>,
) -> Result<Json<AskTurnRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let cleaned_sql = if let Some(ref raw_sql) = req.corrected_sql {
        let (catalog, _, _, _) =
            load_tenant_catalog_and_live(&state, &principal.tenant_id, false).await?;
        Some(validate_corrected_sql(raw_sql, &catalog)?)
    } else {
        None
    };

    let updated = state
        .store
        .update_ask_feedback(
            &principal.tenant_id,
            turn_id,
            req.feedback,
            cleaned_sql.as_deref(),
        )
        .await?
        .ok_or(AppError::NotFound("ASK_TURN_NOT_FOUND"))?;
    Ok(Json(updated))
}

async fn get_ask_session_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(session_id): ApiPath<Uuid>,
) -> Result<Json<Vec<AskTurnRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let turns = state
        .store
        .list_session_turns(&principal.tenant_id, session_id)
        .await?;
    Ok(Json(turns))
}

async fn create_transform_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateTransformDraftRequest>,
) -> Result<Json<TransformDraft>, HttpError> {
    principal.require(Permission::DataSync)?;
    let (session, catalog) = acquire_tenant_session(&state, &principal.tenant_id, false).await?;
    validate_target_dataset_name(&req.target_dataset_name, &catalog)?;

    let transform_ai = state
        .ai
        .for_tenant(&principal.tenant_id)
        .for_user(principal.user_id)
        .with_purpose_override(axpoint_domain::enums::Purpose::Nl2Sql);
    let generated_sql = generate_transform_sql(
        &transform_ai,
        &req.instruction,
        &req.source_datasets,
        &catalog,
    )
    .await?;
    let plan = preview_transform_sql(
        &session,
        &catalog,
        &generated_sql,
        &req.source_datasets,
        None,
    )
    .await?;

    if let Err(err) = state
        .store
        .record_query_history(
            &principal.tenant_id,
            NewQueryHistoryEntry {
                user_id: Some(principal.user_id),
                kind: QueryHistoryKind::Transform,
                sql_text: plan.clean_sql.clone(),
                status: QueryHistoryStatus::Succeeded,
                row_count: i64::try_from(plan.preview.rows.len()).ok(),
                duration_ms: None,
                error_code: None,
            },
        )
        .await
    {
        tracing::warn!(error = %err, "record_query_history for transform draft failed");
    }

    let draft = state
        .store
        .create_transform_draft(
            &principal.tenant_id,
            NewTransformDraftInput {
                user_id: Some(principal.user_id),
                instruction: req.instruction.trim().to_owned(),
                source_dataset_ids: req.source_datasets,
                sql_text: plan.clean_sql,
                preview: plan.preview,
                target_dataset_name: req.target_dataset_name,
            },
        )
        .await?;
    Ok(Json(draft))
}

async fn update_transform_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateTransformDraftRequest>,
) -> Result<Json<TransformDraft>, HttpError> {
    principal.require(Permission::DataSync)?;
    let existing = state
        .store
        .get_transform_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("TRANSFORM_DRAFT_NOT_FOUND"))?;
    if existing.status != TransformDraftStatus::Draft {
        return Err(HttpError(AppError::Conflict("TRANSFORM_NOT_EDITABLE")));
    }

    let (session, catalog) = acquire_tenant_session(&state, &principal.tenant_id, false).await?;
    let plan = preview_transform_sql(
        &session,
        &catalog,
        &req.sql_text,
        &existing.source_dataset_ids,
        None,
    )
    .await?;

    let updated = state
        .store
        .update_transform_draft(&principal.tenant_id, id, &plan.clean_sql, &plan.preview)
        .await?
        .ok_or(AppError::NotFound("TRANSFORM_DRAFT_NOT_FOUND"))?;
    Ok(Json(updated))
}

fn extract_existing_derived_deps(datasets: &[Dataset]) -> HashMap<Uuid, Vec<Uuid>> {
    let mut map = HashMap::new();
    for ds in datasets {
        if ds.source_kind != SourceKind::DerivedView {
            continue;
        }
        let deps: Vec<Uuid> = ds
            .source_ref
            .get("depends_on")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(|s| Uuid::parse_str(s).ok())
                    .collect()
            });
        map.insert(ds.id, deps);
    }
    map
}

async fn apply_transform_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ApplyTransformResponse>, HttpError> {
    principal.require(Permission::DataSync)?;
    let existing = state
        .store
        .get_transform_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("TRANSFORM_DRAFT_NOT_FOUND"))?;
    if existing.status != TransformDraftStatus::Draft {
        return Err(HttpError(AppError::Conflict("TRANSFORM_NOT_EDITABLE")));
    }

    let (catalog, all_datasets, _, live_specs) =
        load_tenant_catalog_and_live(&state, &principal.tenant_id, false).await?;
    validate_target_dataset_name(&existing.target_dataset_name, &catalog)?;

    let session = state
        .analytics_pool
        .get_or_try_init(&principal.tenant_id, catalog.clone(), &live_specs)
        .await?;
    let plan = preview_transform_sql(
        &session,
        &catalog,
        &existing.sql_text,
        &existing.source_dataset_ids,
        None,
    )
    .await?;

    let existing_deps = extract_existing_derived_deps(&all_datasets);
    ensure_no_dependency_cycle(Uuid::new_v4(), &plan.depends_on, &existing_deps)?;

    let dataset = state
        .store
        .create_dataset(
            &principal.tenant_id,
            Some(principal.user_id),
            CreateDatasetRequest {
                connection_id: None,
                name: existing.target_dataset_name.clone(),
                display_name: existing.target_dataset_name.clone(),
                description: existing.instruction.clone(),
                source_kind: SourceKind::DerivedView,
                source_ref: serde_json::json!({
                    "sql": plan.materialized_sql,
                    "clean_sql": plan.clean_sql,
                    "depends_on": plan.depends_on,
                }),
                sync_mode: SyncMode::Snapshot,
                primary_key: Vec::new(),
                cursor_column: None,
                delete_column: None,
                schema_policy: SchemaPolicy::Evolve,
                schedule_cron: None,
                tags: vec![String::from("derived")],
            },
        )
        .await?;

    let via = format!("transform:{id}");
    for dep_id in &plan.depends_on {
        if let Err(err) = state
            .store
            .upsert_lineage(
                &principal.tenant_id,
                "DATASET",
                &dep_id.to_string(),
                "DATASET",
                &dataset.id.to_string(),
                &via,
            )
            .await
        {
            tracing::warn!(error = %err, "upsert_lineage for transform failed");
        }
    }

    let sync_res = state
        .store
        .enqueue_sync_job(&principal.tenant_id, dataset.id)
        .await?;
    let draft = state
        .store
        .mark_transform_applied(&principal.tenant_id, id, dataset.id)
        .await?
        .ok_or(AppError::NotFound("TRANSFORM_DRAFT_NOT_FOUND"))?;

    refresh_cached_tenant_catalog(&state, &principal.tenant_id).await;

    Ok(Json(ApplyTransformResponse {
        draft,
        dataset,
        job_id: sync_res.job_id,
    }))
}

async fn discard_transform_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<TransformDraft>, HttpError> {
    principal.require(Permission::DataSync)?;
    let discarded = state
        .store
        .mark_transform_discarded(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("TRANSFORM_DRAFT_NOT_FOUND"))?;
    Ok(Json(discarded))
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GroundingEvalRunsQuery {
    #[serde(default)]
    limit: Option<u32>,
}

async fn run_grounding_eval_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<GroundingEvalRunResponse>, HttpError> {
    if !principal.has(Permission::GuardAdmin) && !principal.has(Permission::OpsAdmin) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let [cloud_inp, local_inp] = tokio::task::spawn_blocking(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| AppError::Config(format!("build eval runtime failed: {e}")))?;
        rt.block_on(run_grounding_eval_suite())
    })
    .await
    .map_err(|e| {
        HttpError(AppError::Config(format!(
            "grounding eval task join failed: {e}"
        )))
    })??;
    let cloud_rec = state
        .store
        .insert_grounding_eval_run(&principal.tenant_id, cloud_inp)
        .await?;
    let local_rec = state
        .store
        .insert_grounding_eval_run(&principal.tenant_id, local_inp)
        .await?;
    Ok(Json(GroundingEvalRunResponse {
        runs: vec![cloud_rec, local_rec],
    }))
}

async fn list_grounding_eval_runs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<GroundingEvalRunsQuery>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataRead)?;
    let limit = query.limit.unwrap_or(20).clamp(1, 200);
    let items: Vec<GroundingEvalRunRecord> = state
        .store
        .list_grounding_eval_runs(&principal.tenant_id, limit)
        .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

async fn list_history_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<QueryHistoryListQuery>,
) -> Result<Json<PaginatedList<QueryHistoryEntry>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let only_user_id = if principal.has(Permission::DataAdmin) {
        None
    } else {
        Some(principal.user_id)
    };
    let page = state
        .store
        .list_query_history(&principal.tenant_id, only_user_id, query)
        .await?;
    Ok(Json(page))
}

impl AnalyticsQueryPort for AppState {
    async fn run_sql(
        &self,
        principal: &AnalyticsPrincipal,
        req: QueryRequest,
    ) -> Result<QueryResult, AppError> {
        if !principal.has_permission(Permission::DataRead) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        execute_sql_with_telemetry(self, CallerCtx::from_analytics(principal), req).await
    }

    async fn run_saved(
        &self,
        principal: &AnalyticsPrincipal,
        id: Uuid,
        params: BTreeMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, AppError> {
        if !principal.has_permission(Permission::DataRead) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        run_saved_query_inner(self, CallerCtx::from_analytics(principal), id, params).await
    }

    async fn run_metric(
        &self,
        principal: &AnalyticsPrincipal,
        name: &str,
        query: MetricQuery,
    ) -> Result<MetricQueryResult, AppError> {
        if !principal.has_permission(Permission::DataRead) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        let caller = CallerCtx::from_analytics(principal);
        execute_single_metric_with_telemetry(self, &caller, name, query).await
    }

    async fn run_pivot(
        &self,
        principal: &AnalyticsPrincipal,
        spec: PivotSpec,
    ) -> Result<PivotResult, AppError> {
        if !principal.has_permission(Permission::DataRead) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        execute_pivot_with_telemetry(self, CallerCtx::from_analytics(principal), spec).await
    }
}

impl axpoint_domain::ports::FlowSqlMaterializePort for AppState {
    fn materialize_sql_dataset<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        dataset_name: &'a str,
        sql: &'a str,
    ) -> axpoint_domain::platform::BoxFuture<'a, Uuid> {
        Box::pin(async move {
            if !principal.has_permission(Permission::DataSync) {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }

            let (catalog, all_datasets, _, live_specs) =
                load_tenant_catalog_and_live(self, &principal.tenant_id, false).await?;

            if let Some(existing) = all_datasets.iter().find(|ds| ds.name == dataset_name) {
                if existing.source_kind != SourceKind::DerivedView {
                    return Err(AppError::Conflict("NAME_CONFLICT"));
                }
                let _ = self
                    .store
                    .enqueue_sync_job(&principal.tenant_id, existing.id)
                    .await?;
                return Ok(existing.id);
            }

            validate_target_dataset_name(dataset_name, &catalog)?;
            let session = self
                .analytics_pool
                .get_or_try_init(&principal.tenant_id, catalog.clone(), &live_specs)
                .await?;
            let plan = preview_transform_sql(&session, &catalog, sql, &[], None).await?;

            let existing_deps = extract_existing_derived_deps(&all_datasets);
            ensure_no_dependency_cycle(Uuid::new_v4(), &plan.depends_on, &existing_deps)?;

            let dataset = self
                .store
                .create_dataset(
                    &principal.tenant_id,
                    Some(principal.user_id),
                    CreateDatasetRequest {
                        connection_id: None,
                        name: String::from(dataset_name),
                        display_name: String::from(dataset_name),
                        description: String::from("Materialized by AxFlow SQL_TRANSFORM"),
                        source_kind: SourceKind::DerivedView,
                        source_ref: serde_json::json!({
                            "sql": plan.materialized_sql,
                            "clean_sql": plan.clean_sql,
                            "depends_on": plan.depends_on,
                        }),
                        sync_mode: SyncMode::Snapshot,
                        primary_key: Vec::new(),
                        cursor_column: None,
                        delete_column: None,
                        schema_policy: SchemaPolicy::Evolve,
                        schedule_cron: None,
                        tags: vec![String::from("derived"), String::from("axflow")],
                    },
                )
                .await?;

            let _ = self
                .store
                .enqueue_sync_job(&principal.tenant_id, dataset.id)
                .await?;
            refresh_cached_tenant_catalog(self, &principal.tenant_id).await;
            Ok(dataset.id)
        })
    }
}
