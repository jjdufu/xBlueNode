//! Unified `xBlueView` widget execution engine (`execute_widget_data` and `execute_widget_preview`) (`10-STEP11-12` §11.2–§11.3).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axpoint_analytics::{
    CatalogDatasetEntry, LiveConnectionSpec, MetricExecuteOptions, PivotExecuteOptions,
    SessionPool, TenantCatalog, TenantDb, execute_metric_query, execute_pivot,
};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    FilterOp, FilterSpec, MetricDef, MetricQuery, MetricQueryResult, MetricTimeRange, PivotResult,
    PivotSpec, QueryColumn, QueryRequest, QueryResult, SavedQueryKind,
};
use axpoint_domain::axhub::{DatasetListQuery, DatasetStatus};
use axpoint_domain::enums::{
    AggFn, AlertStatus, DataType, Permission, RollupGrain, Severity, WidgetKind,
};
use axpoint_domain::graph::{SubgraphQuery, WindowGroupBy, WindowKind, WindowQuery, WindowSpec};
use axpoint_domain::ports::{
    AnalyticsRepository, ConnectionRepository, DatasetRepository, GraphQueryPort, MetricStorePort,
    RulesRepository, SimRepository, ViewRepository,
};
use axpoint_domain::rules::AlertListQuery;
use axpoint_domain::view::{
    DashboardRecord, DrillLevel, DrillPathItem, FilterDef, FilterKind, WidgetColumn,
    WidgetDataRequest, WidgetDataResponse, WidgetDataSource, WidgetPreviewRequest, WidgetRecord,
};
use axpoint_security::AuthPrincipal;
use chrono::{Datelike as _, Duration as ChronoDuration, Utc};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::cache::{ViewCache, principal_fingerprint_from_auth};

/// Hard server-side row cap enforced on all widget data responses (`10-STEP11-12` §11.2.1).
pub const WIDGET_MAX_ROWS: usize = 5_000;

/// Typed `u32` representation of [`WIDGET_MAX_ROWS`].
pub const WIDGET_MAX_ROWS_U32: u32 = 5_000;

/// Shared execution environment passed into widget data and preview runners.
pub struct WidgetExecEnv<'a, S: ?Sized> {
    /// Persistence store implementing domain repository and query ports.
    pub store: &'a S,
    /// Hardened per-tenant `DuckDB` session pool.
    pub analytics_pool: &'a Arc<SessionPool>,
    /// `moka` widget response cache.
    pub cache: &'a ViewCache,
    /// Authenticated caller identity.
    pub principal: &'a AuthPrincipal,
}

#[derive(Debug, Clone)]
struct ResolvedFilter<'a> {
    column: String,
    kind: FilterKind,
    value: &'a serde_json::Value,
}

struct WidgetExecSpec<'a> {
    widget_id: Option<Uuid>,
    widget_kind: WidgetKind,
    data_source: &'a WidgetDataSource,
    encoding: &'a serde_json::Value,
    drill: &'a [DrillLevel],
    filters: Vec<ResolvedFilter<'a>>,
    drill_path: &'a [DrillPathItem],
}

/// Executes a saved dashboard widget with `moka` principal-scoped caching (`POST /api/xblueview/widgets/{id}/data`).
///
/// # Errors
/// Propagates validation (`400`), authorization (`403`), missing resource (`404`), `SQL` guard,
/// or database errors.
pub async fn execute_widget_data<S>(
    env: &WidgetExecEnv<'_, S>,
    dashboard: &DashboardRecord,
    widget: &WidgetRecord,
    req: &WidgetDataRequest,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository
        + AnalyticsRepository
        + DatasetRepository
        + ConnectionRepository
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + SimRepository
        + ?Sized,
{
    let principal_fp = principal_fingerprint_from_auth(env.principal);
    let cache_key = env.cache.build_key(
        &env.principal.tenant_id,
        widget.id,
        &req.filters,
        &req.drill_path,
        &principal_fp,
    );

    if let Some(cached) = env.cache.get(&cache_key).await {
        return Ok(cached);
    }

    let filters = resolve_dashboard_filters(dashboard, widget.id, &req.filters);
    let spec = WidgetExecSpec {
        widget_id: Some(widget.id),
        widget_kind: widget.widget_kind,
        data_source: &widget.data_source,
        encoding: &widget.encoding,
        drill: &widget.drill,
        filters,
        drill_path: &req.drill_path,
    };
    let response = execute_widget_spec(env, spec).await?;
    env.cache.insert(cache_key, response.clone()).await;
    Ok(response)
}

/// Executes an unsaved widget preview without caching (`POST /api/xblueview/widgets/preview`).
///
/// # Errors
/// Propagates validation (`400`), authorization (`403`), missing resource (`404`), `SQL` guard,
/// or database errors.
pub async fn execute_widget_preview<S>(
    env: &WidgetExecEnv<'_, S>,
    req: &WidgetPreviewRequest,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository
        + AnalyticsRepository
        + DatasetRepository
        + ConnectionRepository
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + SimRepository
        + ?Sized,
{
    let filters = resolve_preview_filters(&req.filters);
    let spec = WidgetExecSpec {
        widget_id: None,
        widget_kind: req.widget_kind,
        data_source: &req.data_source,
        encoding: &req.encoding,
        drill: &req.drill,
        filters,
        drill_path: &req.drill_path,
    };
    execute_widget_spec(env, spec).await
}

fn resolve_dashboard_filters<'a>(
    dashboard: &DashboardRecord,
    widget_id: Uuid,
    raw_filters: &'a BTreeMap<String, serde_json::Value>,
) -> Vec<ResolvedFilter<'a>> {
    let widget_key = widget_id.to_string();
    let def_by_key: HashMap<&str, &FilterDef> = dashboard
        .filters
        .iter()
        .map(|d| (d.key.as_str(), d))
        .collect();

    let mut out = Vec::new();
    for (key, value) in raw_filters {
        if is_empty_filter_value(value) {
            continue;
        }
        let Some(def) = def_by_key.get(key.as_str()) else {
            continue;
        };
        let Some(target_col) = def.field_map.get(&widget_key) else {
            continue;
        };
        let trimmed = target_col.trim();
        if !trimmed.is_empty() {
            out.push(ResolvedFilter {
                column: trimmed.to_owned(),
                kind: def.kind,
                value,
            });
        }
    }
    out
}

fn resolve_preview_filters(
    raw_filters: &BTreeMap<String, serde_json::Value>,
) -> Vec<ResolvedFilter<'_>> {
    let mut out = Vec::new();
    for (col, value) in raw_filters {
        if is_empty_filter_value(value) {
            continue;
        }
        let kind = infer_filter_kind(value);
        out.push(ResolvedFilter {
            column: col.trim().to_owned(),
            kind,
            value,
        });
    }
    out
}

fn is_empty_filter_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(arr) => {
            arr.is_empty() || arr.iter().all(serde_json::Value::is_null)
        }
        _ => false,
    }
}

fn infer_filter_kind(value: &serde_json::Value) -> FilterKind {
    match value {
        serde_json::Value::Array(arr) => {
            if arr.len() == 2 && arr.iter().all(serde_json::Value::is_number) {
                FilterKind::NumberRange
            } else {
                FilterKind::MultiSelect
            }
        }
        _ => FilterKind::Select,
    }
}

async fn execute_widget_spec<S>(
    env: &WidgetExecEnv<'_, S>,
    spec: WidgetExecSpec<'_>,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository
        + AnalyticsRepository
        + DatasetRepository
        + ConnectionRepository
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + SimRepository
        + ?Sized,
{
    let drill_level = spec.drill_path.len();
    match spec.data_source {
        WidgetDataSource::Sql { sql } => {
            execute_sql_widget(env, sql, BTreeMap::new(), &spec, drill_level).await
        }
        WidgetDataSource::SavedQuery { saved_query_id } => {
            execute_saved_query_widget(env, *saved_query_id, &spec, drill_level).await
        }
        WidgetDataSource::Metric {
            metric,
            dimensions,
            time_grain,
            time_range,
        } => {
            execute_metric_widget(
                env,
                metric,
                dimensions,
                time_grain.as_deref(),
                time_range.as_deref(),
                &spec,
                drill_level,
            )
            .await
        }
        WidgetDataSource::Pivot { spec: pivot_json } => {
            execute_pivot_widget(env, pivot_json, &spec, drill_level).await
        }
        WidgetDataSource::NodeMetric {
            type_name,
            metric,
            agg,
            grain,
            time_range,
            group_by,
        } => {
            let input = NodeMetricInput {
                type_name,
                metric,
                agg: agg.as_deref(),
                grain: grain.as_deref(),
                time_range: time_range.as_deref(),
                group_by: group_by.as_deref(),
            };
            execute_node_metric_widget(env, &input, drill_level).await
        }
        WidgetDataSource::Graph {
            type_names,
            link_types,
            color_metric,
            limit,
        } => {
            execute_graph_widget(
                env,
                type_names,
                link_types,
                color_metric.as_deref(),
                *limit,
                drill_level,
            )
            .await
        }
        WidgetDataSource::Alerts {
            status,
            severity,
            limit,
        } => execute_alerts_widget(env, status, severity, *limit, drill_level).await,
        WidgetDataSource::SimScenarios {
            scenario_ids,
            metrics,
        } => execute_sim_scenarios_widget(env, scenario_ids, metrics, drill_level).await,
        WidgetDataSource::NodesGeo {
            type_names,
            color_metric,
            link_types: _,
        } => execute_nodes_geo_widget(env, type_names, color_metric.as_deref(), drill_level).await,
        WidgetDataSource::FloorPlan {
            asset_id,
            color_metric,
        } => execute_floor_plan_widget(env, *asset_id, color_metric.as_deref(), drill_level).await,
        WidgetDataSource::Scene3d {
            asset_id,
            color_metric,
        } => execute_scene3d_widget(env, *asset_id, color_metric.as_deref(), drill_level).await,
        WidgetDataSource::Action { .. } => {
            execute_action_logs_widget(env, spec.widget_id, drill_level).await
        }
        WidgetDataSource::Static { columns, rows } => {
            Ok(build_static_response(columns, rows, drill_level))
        }
    }
}

async fn acquire_tenant_session<S>(
    store: &S,
    analytics_pool: &SessionPool,
    tenant_id: &str,
    has_live_permission: bool,
) -> Result<(Arc<TenantDb>, TenantCatalog), AppError>
where
    S: DatasetRepository + ConnectionRepository + ?Sized,
{
    let mut all_datasets = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store
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
    for ds in &all_datasets {
        if ds.status == DatasetStatus::Disabled {
            continue;
        }
        let col_metas = store.list_columns(tenant_id, ds.id).await?;
        catalog.insert_dataset(CatalogDatasetEntry::from_dataset(ds, &col_metas));
    }

    let conns = store.list_connections(tenant_id).await?;
    let live_specs = LiveConnectionSpec::from_connections(&conns);
    for live in &live_specs {
        catalog.insert_live_connection(&live.name);
    }

    let session = analytics_pool
        .get_or_try_init(tenant_id, catalog, &live_specs)
        .await?;
    let mut snap = session.catalog();
    snap.has_live_permission = has_live_permission;
    Ok((session, snap))
}

async fn execute_sql_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    raw_sql: &str,
    mut params: BTreeMap<String, serde_json::Value>,
    spec: &WidgetExecSpec<'_>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: DatasetRepository + ConnectionRepository + ?Sized,
{
    let clean_sql = raw_sql.trim().trim_end_matches(';').trim();
    if clean_sql.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "widget SQL cannot be empty",
        ));
    }

    let has_live = env.principal.has(Permission::DataSync);
    let has_pii_raw = env.principal.has(Permission::PiiRawRead);
    let (session, _) = acquire_tenant_session(
        env.store,
        env.analytics_pool,
        &env.principal.tenant_id,
        has_live,
    )
    .await?;

    let needs_wrap =
        !spec.filters.is_empty() || !spec.drill_path.is_empty() || !spec.drill.is_empty();
    let final_sql = if needs_wrap {
        let probe_sql = format!("SELECT * FROM ({clean_sql}) AS _w LIMIT 0");
        let probe_res = session
            .execute_sql(
                &QueryRequest {
                    sql: probe_sql,
                    params: params.clone(),
                    max_rows: Some(1),
                    timeout_ms: None,
                    allow_live: has_live,
                    reveal_pii: false,
                },
                has_live,
                has_pii_raw,
            )
            .await?;
        build_wrapped_sql(clean_sql, &probe_res.result.columns, spec, &mut params)?
    } else {
        clean_sql.to_owned()
    };

    let out = session
        .execute_sql(
            &QueryRequest {
                sql: final_sql,
                params,
                max_rows: Some(WIDGET_MAX_ROWS_U32),
                timeout_ms: None,
                allow_live: has_live,
                reveal_pii: false,
            },
            has_live,
            has_pii_raw,
        )
        .await?;

    Ok(query_result_to_widget_response(out.result, drill_level))
}

fn build_wrapped_sql(
    clean_sql: &str,
    base_cols: &[QueryColumn],
    spec: &WidgetExecSpec<'_>,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Result<String, AppError> {
    let col_lookup: HashMap<String, &QueryColumn> = base_cols
        .iter()
        .map(|c| (c.name.to_ascii_lowercase(), c))
        .collect();

    let mut where_clauses = Vec::new();
    for (idx, flt) in spec.filters.iter().enumerate() {
        let col = resolve_whitelisted_col(&col_lookup, &flt.column)?;
        if let Some(clause) = compile_view_filter_clause(col, flt.kind, flt.value, idx, params)? {
            where_clauses.push(clause);
        }
    }

    for (idx, item) in spec.drill_path.iter().enumerate() {
        let col = resolve_whitelisted_col(&col_lookup, &item.field)?;
        let qcol = quote_ident(col);
        if item.value.is_null() {
            where_clauses.push(format!("{qcol} IS NULL"));
        } else {
            let pname = format!("_vd{idx}");
            params.insert(pname.clone(), item.value.clone());
            where_clauses.push(format!("{qcol} = :{pname}"));
        }
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };

    let drill_level = spec.drill_path.len();
    if let Some(next_level) = spec.drill.get(drill_level) {
        let group_col = resolve_whitelisted_col(&col_lookup, &next_level.field)?;
        let y_fields = extract_y_fields(
            spec.encoding,
            base_cols,
            spec.drill,
            &col_lookup,
            drill_level,
        )?;
        if !y_fields.is_empty() {
            let mut group_cols = vec![quote_ident(group_col)];
            let mut select_cols = vec![quote_ident(group_col)];
            if let Some(series_raw) = spec
                .encoding
                .get("series")
                .and_then(serde_json::Value::as_str)
                && let Ok(series_col) = resolve_whitelisted_col(&col_lookup, series_raw)
                && !series_col.eq_ignore_ascii_case(group_col)
                && !spec
                    .drill_path
                    .iter()
                    .any(|d| d.field.eq_ignore_ascii_case(series_col))
            {
                let qs = quote_ident(series_col);
                select_cols.push(qs.clone());
                group_cols.push(qs);
            }
            for y in y_fields {
                let qy = quote_ident(y);
                select_cols.push(format!("SUM({qy}) AS {qy}"));
            }
            return Ok(format!(
                "SELECT {} FROM ({clean_sql}) AS _w{where_sql} GROUP BY {}",
                select_cols.join(", "),
                group_cols.join(", ")
            ));
        }
    }

    Ok(format!("SELECT * FROM ({clean_sql}) AS _w{where_sql}"))
}

fn resolve_whitelisted_col<'a>(
    lookup: &HashMap<String, &'a QueryColumn>,
    raw_name: &str,
) -> Result<&'a str, AppError> {
    let key = raw_name.trim().to_ascii_lowercase();
    lookup
        .get(&key)
        .map(|c| c.name.as_str())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("unknown column '{raw_name}'")))
}

fn extract_y_fields<'a>(
    encoding: &serde_json::Value,
    base_cols: &'a [QueryColumn],
    drill: &[DrillLevel],
    lookup: &HashMap<String, &'a QueryColumn>,
    drill_level: usize,
) -> Result<Vec<&'a str>, AppError> {
    let mut out = Vec::new();
    if let Some(y_val) = encoding.get("y") {
        match y_val {
            serde_json::Value::String(s) if !s.trim().is_empty() => {
                out.push(resolve_whitelisted_col(lookup, s)?);
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str()
                        && !s.trim().is_empty()
                    {
                        out.push(resolve_whitelisted_col(lookup, s)?);
                    }
                }
            }
            _ => {}
        }
    }

    if out.is_empty() && drill_level > 0 {
        for col in base_cols {
            let is_numeric = matches!(
                col.data_type,
                DataType::Int64 | DataType::Float64 | DataType::Decimal
            );
            let is_drill_col = drill
                .iter()
                .any(|d| d.field.eq_ignore_ascii_case(&col.name));
            if is_numeric && !is_drill_col {
                out.push(col.name.as_str());
            }
        }
    }

    Ok(out)
}

fn compile_view_filter_clause(
    col: &str,
    kind: FilterKind,
    val: &serde_json::Value,
    idx: usize,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Result<Option<String>, AppError> {
    let qcol = quote_ident(col);
    match kind {
        FilterKind::Select | FilterKind::MultiSelect => {
            if let serde_json::Value::Array(arr) = val {
                Ok(compile_in_list(&qcol, arr, idx, params))
            } else {
                let pname = format!("_vf{idx}");
                params.insert(pname.clone(), val.clone());
                Ok(Some(format!("{qcol} = :{pname}")))
            }
        }
        FilterKind::DateRange | FilterKind::NumberRange => {
            let serde_json::Value::Array(arr) = val else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "range filter requires a 2-element array [min, max]",
                ));
            };
            let lo = arr.first().filter(|v| !v.is_null());
            let hi = arr.get(1).filter(|v| !v.is_null());
            let mut parts = Vec::new();
            if let Some(lo_v) = lo {
                let plo = format!("_vf{idx}_lo");
                params.insert(plo.clone(), lo_v.clone());
                parts.push(format!("{qcol} >= :{plo}"));
            }
            if let Some(hi_v) = hi {
                let phi = format!("_vf{idx}_hi");
                params.insert(phi.clone(), hi_v.clone());
                parts.push(format!("{qcol} <= :{phi}"));
            }
            if parts.is_empty() {
                Ok(None)
            } else {
                Ok(Some(parts.join(" AND ")))
            }
        }
        FilterKind::Text => {
            let Some(text) = val.as_str() else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "TEXT filter requires a string value",
                ));
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let pname = format!("_vf{idx}");
            params.insert(
                pname.clone(),
                serde_json::Value::String(format!("%{trimmed}%")),
            );
            Ok(Some(format!("CAST({qcol} AS VARCHAR) LIKE :{pname}")))
        }
    }
}

fn compile_in_list(
    qcol: &str,
    arr: &[serde_json::Value],
    idx: usize,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Option<String> {
    let mut placeholders = Vec::new();
    for (elem_idx, item) in arr.iter().enumerate() {
        if item.is_null() {
            continue;
        }
        let pname = format!("_vf{idx}_{elem_idx}");
        params.insert(pname.clone(), item.clone());
        placeholders.push(format!(":{pname}"));
    }
    if placeholders.is_empty() {
        None
    } else {
        Some(format!("{qcol} IN ({})", placeholders.join(", ")))
    }
}

async fn execute_saved_query_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    saved_query_id: Uuid,
    spec: &WidgetExecSpec<'_>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: AnalyticsRepository + DatasetRepository + ConnectionRepository + ?Sized,
{
    let sq = env
        .store
        .get_saved_query(&env.principal.tenant_id, saved_query_id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;

    if !sq.shared
        && sq.owner_user_id != Some(env.principal.user_id)
        && !env.principal.has(Permission::DataAdmin)
    {
        return Err(AppError::NotFound("SAVED_QUERY_NOT_FOUND"));
    }

    let mut params = BTreeMap::new();
    for decl in &sq.parameters {
        if let Some(ref def_val) = decl.default {
            params.insert(decl.name.clone(), def_val.clone());
        }
    }

    match sq.kind {
        SavedQueryKind::Sql | SavedQueryKind::Ask => {
            let sql_text = sq
                .spec
                .get("sql")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "saved query spec missing sql"))?;
            execute_sql_widget(env, sql_text, params, spec, drill_level).await
        }
        SavedQueryKind::Pivot => execute_pivot_widget(env, &sq.spec, spec, drill_level).await,
        SavedQueryKind::Metric => {
            let metric_name = sq
                .spec
                .get("metric_name")
                .or_else(|| sq.spec.get("metric"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "saved metric query missing metric_name")
                })?;
            let query_val = sq
                .spec
                .get("query")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let mq: MetricQuery = serde_json::from_value(query_val).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid saved metric query: {err}"),
                )
            })?;
            let dims = mq.dimensions;
            let grain_str = mq.grain.map(|g| String::from(g.as_str()));
            execute_metric_widget(
                env,
                metric_name,
                &dims,
                grain_str.as_deref(),
                None,
                spec,
                drill_level,
            )
            .await
        }
    }
}

async fn execute_metric_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    metric_name: &str,
    dimensions: &[String],
    time_grain: Option<&str>,
    time_range: Option<&str>,
    spec: &WidgetExecSpec<'_>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: AnalyticsRepository + DatasetRepository + ConnectionRepository + ?Sized,
{
    let metric_def: MetricDef = env
        .store
        .get_metric_by_name(&env.principal.tenant_id, metric_name.trim())
        .await?
        .ok_or(AppError::NotFound("METRIC_NOT_FOUND"))?;

    let effective_dims = if !spec.drill.is_empty()
        && drill_level < spec.drill.len()
        && (drill_level > 0 || dimensions.is_empty())
    {
        spec.drill
            .get(drill_level)
            .map_or_default(|d| vec![d.field.clone()])
    } else {
        dimensions.to_vec()
    };

    let parsed_grain = parse_rollup_grain_opt(time_grain)?;
    let parsed_range = parse_metric_time_range_opt(time_range)?;
    let mut metric_filters = filters_to_specs(&spec.filters, spec.drill_path);

    let mut effective_def = metric_def.clone();
    let effective_range = if parsed_grain.is_none()
        && !matches!(spec.widget_kind, WidgetKind::LineChart)
    {
        if let (Some(tr), Some(tc)) = (parsed_range.as_ref(), metric_def.time_column.as_deref()) {
            metric_filters.push(FilterSpec {
                field: tc.to_owned(),
                op: FilterOp::Between,
                value: serde_json::json!([tr.from.to_rfc3339(), tr.to.to_rfc3339()]),
            });
        }
        effective_def.time_column = None;
        None
    } else {
        parsed_range
    };

    let query = MetricQuery {
        dimensions: effective_dims,
        filters: metric_filters,
        time_range: effective_range,
        grain: parsed_grain,
        compare: None,
    };

    let (session, catalog) = acquire_tenant_session(
        env.store,
        env.analytics_pool,
        &env.principal.tenant_id,
        false,
    )
    .await?;

    let res = execute_metric_query(
        &session,
        &catalog,
        &effective_def,
        &query,
        MetricExecuteOptions {
            timeout_ms: None,
            reveal_pii: false,
            has_pii_raw_read: env.principal.has(Permission::PiiRawRead),
        },
    )
    .await?;

    Ok(metric_result_to_widget_response(res, drill_level))
}

async fn execute_pivot_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    pivot_json: &serde_json::Value,
    spec: &WidgetExecSpec<'_>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: DatasetRepository + ConnectionRepository + ?Sized,
{
    let mut pivot_spec: PivotSpec = serde_json::from_value(pivot_json.clone())
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid pivot spec: {err}")))?;
    let extra_filters = filters_to_specs(&spec.filters, spec.drill_path);
    pivot_spec.filters.extend(extra_filters);
    pivot_spec.row_limit = if pivot_spec.row_limit == 0 {
        WIDGET_MAX_ROWS_U32.min(1_000)
    } else {
        pivot_spec.row_limit.min(WIDGET_MAX_ROWS_U32)
    };

    let (session, catalog) = acquire_tenant_session(
        env.store,
        env.analytics_pool,
        &env.principal.tenant_id,
        false,
    )
    .await?;

    let res = execute_pivot(
        &session,
        &catalog,
        &pivot_spec,
        PivotExecuteOptions {
            timeout_ms: None,
            reveal_pii: false,
            has_pii_raw_read: env.principal.has(Permission::PiiRawRead),
        },
    )
    .await?;

    Ok(pivot_result_to_widget_response(
        &pivot_spec,
        res,
        drill_level,
    ))
}

struct NodeMetricInput<'a> {
    type_name: &'a str,
    metric: &'a str,
    agg: Option<&'a str>,
    grain: Option<&'a str>,
    time_range: Option<&'a str>,
    group_by: Option<&'a str>,
}

async fn execute_node_metric_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    input: &NodeMetricInput<'_>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: MetricStorePort + ?Sized,
{
    let range = parse_metric_time_range_opt(input.time_range)?.unwrap_or_else(|| {
        let to = Utc::now();
        MetricTimeRange {
            from: to - ChronoDuration::days(7),
            to,
        }
    });
    let window_size = match input
        .grain
        .map_or("HOUR", str::trim)
        .to_ascii_uppercase()
        .as_str()
    {
        "MINUTE" => "1m",
        "DAY" => "1d",
        _ => "1h",
    };
    let parsed_agg = parse_agg_fn(input.agg.unwrap_or("AVG"))?;
    let parsed_group_by = match input
        .group_by
        .map_or("none", str::trim)
        .to_ascii_lowercase()
        .as_str()
    {
        "node" => WindowGroupBy::Node,
        "type" => WindowGroupBy::Type,
        _ => WindowGroupBy::None,
    };

    let resp = env
        .store
        .query_window(
            &env.principal.tenant_id,
            WindowQuery {
                node_ids: None,
                type_name: Some(input.type_name.to_owned()),
                metric: input.metric.to_owned(),
                from: range.from,
                to: range.to,
                window: WindowSpec {
                    kind: WindowKind::Tumbling,
                    size: String::from(window_size),
                    step: None,
                },
                agg: parsed_agg,
                group_by: parsed_group_by,
            },
        )
        .await?;

    let columns = vec![
        WidgetColumn {
            name: String::from("ts"),
            data_type: DataType::Timestamp,
        },
        WidgetColumn {
            name: String::from("group_key"),
            data_type: DataType::String,
        },
        WidgetColumn {
            name: String::from("value"),
            data_type: DataType::Float64,
        },
    ];
    let mut rows: Vec<Vec<serde_json::Value>> = resp
        .items
        .into_iter()
        .map(|item| {
            vec![
                serde_json::Value::String(item.ts.to_rfc3339()),
                item.group_key
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
                item.value
                    .and_then(serde_json::Number::from_f64)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number),
            ]
        })
        .collect();
    let truncated = cap_rows(&mut rows);
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_graph_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    type_names: &[String],
    link_types: &[String],
    color_metric: Option<&str>,
    limit: Option<usize>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: GraphQueryPort + ?Sized,
{
    let lim = u32::try_from(limit.unwrap_or(2_000).clamp(1, 2_000)).unwrap_or(2_000);
    let sub = env
        .store
        .query_subgraph(
            &env.principal.tenant_id,
            SubgraphQuery {
                type_names: (!type_names.is_empty()).then(|| type_names.join(",")),
                link_types: (!link_types.is_empty()).then(|| link_types.join(",")),
                limit: lim,
            },
        )
        .await?;

    let columns = vec![
        col_def("record_kind", DataType::String),
        col_def("id", DataType::String),
        col_def("type_name", DataType::String),
        col_def("node_key", DataType::String),
        col_def("title", DataType::String),
        col_def("metric_value", DataType::Float64),
        col_def("src_node_id", DataType::String),
        col_def("dst_node_id", DataType::String),
        col_def("link_type", DataType::String),
        col_def("weight", DataType::Float64),
    ];

    let mut rows = Vec::with_capacity(sub.nodes.len().saturating_add(sub.links.len()));
    for node in sub.nodes {
        let mval = color_metric
            .and_then(|m| node.metrics.get(m).copied())
            .and_then(serde_json::Number::from_f64)
            .map_or(serde_json::Value::Null, serde_json::Value::Number);
        rows.push(vec![
            serde_json::Value::String(String::from("NODE")),
            serde_json::Value::String(node.id.to_string()),
            serde_json::Value::String(node.type_name),
            serde_json::Value::String(node.node_key),
            serde_json::Value::String(node.title),
            mval,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
        ]);
    }
    for link in sub.links {
        let wval = serde_json::Number::from_f64(link.weight)
            .map_or(serde_json::Value::Null, serde_json::Value::Number);
        rows.push(vec![
            serde_json::Value::String(String::from("LINK")),
            serde_json::Value::String(link.id.to_string()),
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::String(link.src_node_id.to_string()),
            serde_json::Value::String(link.dst_node_id.to_string()),
            serde_json::Value::String(link.link_type),
            wval,
        ]);
    }

    let truncated = cap_rows(&mut rows) || sub.truncated;
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_alerts_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    statuses: &[String],
    severities: &[String],
    limit: Option<i64>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: RulesRepository + ?Sized,
{
    let single_status = if statuses.len() == 1 {
        statuses.first().and_then(|s| s.parse::<AlertStatus>().ok())
    } else {
        None
    };
    let single_sev = if severities.len() == 1 {
        severities.first().and_then(|s| s.parse::<Severity>().ok())
    } else {
        None
    };
    let page_limit = u32::try_from(limit.unwrap_or(50).clamp(1, 500)).unwrap_or(50);

    let page = env
        .store
        .list_alerts(
            &env.principal.tenant_id,
            AlertListQuery {
                status: single_status,
                severity: single_sev,
                limit: Some(page_limit),
                ..AlertListQuery::default()
            },
        )
        .await?;

    let columns = vec![
        col_def("id", DataType::String),
        col_def("rule_id", DataType::String),
        col_def("severity", DataType::String),
        col_def("status", DataType::String),
        col_def("title", DataType::String),
        col_def("node_id", DataType::String),
        col_def("dataset_id", DataType::String),
        col_def("first_seen_at", DataType::Timestamp),
        col_def("last_seen_at", DataType::Timestamp),
        col_def("occurrences", DataType::Int64),
    ];

    let mut rows = Vec::new();
    for alert in page.items {
        if !statuses.is_empty()
            && !statuses
                .iter()
                .any(|s| s.eq_ignore_ascii_case(alert.status.as_str()))
        {
            continue;
        }
        if !severities.is_empty()
            && !severities
                .iter()
                .any(|s| s.eq_ignore_ascii_case(alert.severity.as_str()))
        {
            continue;
        }
        rows.push(vec![
            serde_json::Value::String(alert.id.to_string()),
            serde_json::Value::String(alert.rule_id.to_string()),
            serde_json::Value::String(String::from(alert.severity.as_str())),
            serde_json::Value::String(String::from(alert.status.as_str())),
            serde_json::Value::String(alert.title),
            alert.node_id.map_or(serde_json::Value::Null, |id| {
                serde_json::Value::String(id.to_string())
            }),
            alert.dataset_id.map_or(serde_json::Value::Null, |id| {
                serde_json::Value::String(id.to_string())
            }),
            serde_json::Value::String(alert.first_seen_at.to_rfc3339()),
            serde_json::Value::String(alert.last_seen_at.to_rfc3339()),
            serde_json::Value::Number(serde_json::Number::from(alert.occurrences)),
        ]);
    }
    let truncated = cap_rows(&mut rows);
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_sim_scenarios_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    scenario_ids: &[Uuid],
    metrics: &[String],
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: SimRepository + ?Sized,
{
    let columns = vec![
        col_def("scenario_id", DataType::String),
        col_def("scenario_name", DataType::String),
        col_def("node_id", DataType::String),
        col_def("type_name", DataType::String),
        col_def("title", DataType::String),
        col_def("metric", DataType::String),
        col_def("before_val", DataType::Float64),
        col_def("after_val", DataType::Float64),
        col_def("delta", DataType::Float64),
        col_def("delta_ratio", DataType::Float64),
    ];

    let mut rows = Vec::new();
    for sc_id in scenario_ids {
        let Some(sc) = env
            .store
            .get_scenario(&env.principal.tenant_id, *sc_id)
            .await?
        else {
            continue;
        };
        let impacts = env
            .store
            .list_all_scenario_impacts(&env.principal.tenant_id, *sc_id)
            .await?;
        for imp in impacts {
            if !metrics.is_empty() && !metrics.iter().any(|m| m == &imp.metric) {
                continue;
            }
            rows.push(vec![
                serde_json::Value::String(sc.id.to_string()),
                serde_json::Value::String(sc.name.clone()),
                serde_json::Value::String(imp.node_id.to_string()),
                serde_json::Value::String(imp.type_name),
                serde_json::Value::String(imp.title),
                serde_json::Value::String(imp.metric),
                opt_f64_json(imp.before_val),
                opt_f64_json(Some(imp.after_val)),
                opt_f64_json(imp.delta),
                opt_f64_json(imp.delta_ratio),
            ]);
        }
    }
    let truncated = cap_rows(&mut rows);
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_nodes_geo_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    type_names: &[String],
    color_metric: Option<&str>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository + ?Sized,
{
    let items = env
        .store
        .query_nodes_geo(&env.principal.tenant_id, type_names, color_metric)
        .await?;
    let columns = vec![
        col_def("node_id", DataType::String),
        col_def("type_name", DataType::String),
        col_def("node_key", DataType::String),
        col_def("title", DataType::String),
        col_def("lng", DataType::Float64),
        col_def("lat", DataType::Float64),
        col_def("metric_value", DataType::Float64),
        col_def("open_alerts", DataType::Int64),
    ];
    let mut rows: Vec<Vec<serde_json::Value>> = items
        .into_iter()
        .map(|r| {
            vec![
                serde_json::Value::String(r.node_id.to_string()),
                serde_json::Value::String(r.type_name),
                serde_json::Value::String(r.node_key),
                serde_json::Value::String(r.title),
                opt_f64_json(Some(r.lng)),
                opt_f64_json(Some(r.lat)),
                opt_f64_json(r.metric_value),
                serde_json::Value::Number(serde_json::Number::from(r.open_alerts)),
            ]
        })
        .collect();
    let truncated = cap_rows(&mut rows);
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_floor_plan_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    asset_id: Uuid,
    color_metric: Option<&str>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository + ?Sized,
{
    let items = env
        .store
        .query_floor_plan_rows(&env.principal.tenant_id, asset_id, color_metric)
        .await?;
    let columns = vec![
        col_def("node_id", DataType::String),
        col_def("type_name", DataType::String),
        col_def("node_key", DataType::String),
        col_def("title", DataType::String),
        col_def("x_ratio", DataType::Float64),
        col_def("y_ratio", DataType::Float64),
        col_def("metric_value", DataType::Float64),
        col_def("open_alerts", DataType::Int64),
    ];
    let mut rows: Vec<Vec<serde_json::Value>> = items
        .into_iter()
        .map(|r| {
            vec![
                serde_json::Value::String(r.node_id.to_string()),
                serde_json::Value::String(r.type_name),
                serde_json::Value::String(r.node_key),
                serde_json::Value::String(r.title),
                opt_f64_json(Some(r.x_ratio)),
                opt_f64_json(Some(r.y_ratio)),
                opt_f64_json(r.metric_value),
                serde_json::Value::Number(serde_json::Number::from(r.open_alerts)),
            ]
        })
        .collect();
    let truncated = cap_rows(&mut rows);
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_scene3d_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    asset_id: Uuid,
    color_metric: Option<&str>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository + ?Sized,
{
    let items = env
        .store
        .query_scene3d_rows(&env.principal.tenant_id, asset_id, color_metric)
        .await?;
    let columns = vec![
        col_def("node_id", DataType::String),
        col_def("type_name", DataType::String),
        col_def("node_key", DataType::String),
        col_def("title", DataType::String),
        col_def("mesh_name", DataType::String),
        col_def("pos_x", DataType::Float64),
        col_def("pos_y", DataType::Float64),
        col_def("pos_z", DataType::Float64),
        col_def("scale", DataType::Float64),
        col_def("metric_value", DataType::Float64),
        col_def("open_alerts", DataType::Int64),
    ];
    let mut rows: Vec<Vec<serde_json::Value>> = items
        .into_iter()
        .map(|r| {
            vec![
                serde_json::Value::String(r.node_id.to_string()),
                serde_json::Value::String(r.type_name),
                serde_json::Value::String(r.node_key),
                serde_json::Value::String(r.title),
                serde_json::Value::String(r.mesh_name),
                opt_f64_json(Some(r.pos_x)),
                opt_f64_json(Some(r.pos_y)),
                opt_f64_json(Some(r.pos_z)),
                opt_f64_json(Some(r.scale)),
                opt_f64_json(r.metric_value),
                serde_json::Value::Number(serde_json::Number::from(r.open_alerts)),
            ]
        })
        .collect();
    let truncated = cap_rows(&mut rows);
    Ok(finalize_response(columns, rows, truncated, drill_level))
}

async fn execute_action_logs_widget<S>(
    env: &WidgetExecEnv<'_, S>,
    widget_id: Option<Uuid>,
    drill_level: usize,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository + ?Sized,
{
    let logs = match widget_id {
        Some(wid) => {
            env.store
                .list_widget_action_logs(&env.principal.tenant_id, wid, 10)
                .await?
        }
        None => Vec::new(),
    };
    let columns = vec![
        col_def("id", DataType::Int64),
        col_def("action_kind", DataType::String),
        col_def("target", DataType::String),
        col_def("status", DataType::String),
        col_def("payload", DataType::Json),
        col_def("error_message", DataType::String),
        col_def("created_at", DataType::Timestamp),
    ];
    let rows: Vec<Vec<serde_json::Value>> = logs
        .into_iter()
        .map(|log| {
            vec![
                serde_json::Value::Number(serde_json::Number::from(log.id)),
                serde_json::to_value(log.action_kind).unwrap_or(serde_json::Value::Null),
                serde_json::Value::String(log.target),
                serde_json::to_value(log.status).unwrap_or(serde_json::Value::Null),
                log.payload,
                log.error_message
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
                serde_json::Value::String(log.created_at.to_rfc3339()),
            ]
        })
        .collect();
    Ok(finalize_response(columns, rows, false, drill_level))
}

fn build_static_response(
    raw_cols: &[QueryColumn],
    raw_rows: &[Vec<serde_json::Value>],
    drill_level: usize,
) -> WidgetDataResponse {
    let columns = if raw_cols.is_empty() {
        let width = raw_rows.first().map_or(0, Vec::len);
        (0..width)
            .map(|idx| col_def(&format!("col_{idx}"), DataType::String))
            .collect()
    } else {
        raw_cols
            .iter()
            .map(|c| WidgetColumn {
                name: c.name.clone(),
                data_type: c.data_type,
            })
            .collect()
    };
    let mut rows = raw_rows.to_vec();
    let truncated = cap_rows(&mut rows);
    finalize_response(columns, rows, truncated, drill_level)
}

fn query_result_to_widget_response(qr: QueryResult, drill_level: usize) -> WidgetDataResponse {
    let columns: Vec<WidgetColumn> = qr
        .columns
        .iter()
        .map(|c| WidgetColumn {
            name: c.name.clone(),
            data_type: c.data_type,
        })
        .collect();
    let mut rows: Vec<Vec<serde_json::Value>> = qr
        .rows
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(&c.name).cloned().unwrap_or(serde_json::Value::Null))
                .collect()
        })
        .collect();
    let capped = cap_rows(&mut rows);
    finalize_response(columns, rows, qr.truncated || capped, drill_level)
}

fn metric_result_to_widget_response(
    mr: MetricQueryResult,
    drill_level: usize,
) -> WidgetDataResponse {
    let columns: Vec<WidgetColumn> = mr
        .columns
        .iter()
        .map(|c| WidgetColumn {
            name: c.name.clone(),
            data_type: c.data_type,
        })
        .collect();
    let mut rows: Vec<Vec<serde_json::Value>> = mr
        .rows
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(&c.name).cloned().unwrap_or(serde_json::Value::Null))
                .collect()
        })
        .collect();
    let truncated = cap_rows(&mut rows);
    finalize_response(columns, rows, truncated, drill_level)
}

fn pivot_result_to_widget_response(
    spec: &PivotSpec,
    pr: PivotResult,
    drill_level: usize,
) -> WidgetDataResponse {
    let row_dim_count = pr.row_headers.first().map_or(spec.rows.len(), Vec::len);
    let mut columns = Vec::with_capacity(row_dim_count.saturating_add(pr.column_headers.len()));
    for idx in 0..row_dim_count {
        let name = spec
            .rows
            .get(idx)
            .map_or_else(|| format!("row_{idx}"), |r| r.field.clone());
        columns.push(col_def(&name, DataType::String));
    }
    for ch in &pr.column_headers {
        let label = if ch.is_empty() {
            String::from("value")
        } else {
            ch.join(" / ")
        };
        columns.push(col_def(&label, DataType::String));
    }

    let mut rows = Vec::with_capacity(pr.cells.len());
    for (rh, cells) in pr.row_headers.into_iter().zip(pr.cells) {
        let mut row = Vec::with_capacity(columns.len());
        for label in rh {
            row.push(serde_json::Value::String(label));
        }
        for cell in cells {
            row.push(serde_json::Value::String(cell));
        }
        rows.push(row);
    }
    let truncated = cap_rows(&mut rows) || pr.columns_truncated;
    finalize_response(columns, rows, truncated, drill_level)
}

fn filters_to_specs(
    filters: &[ResolvedFilter<'_>],
    drill_path: &[DrillPathItem],
) -> Vec<FilterSpec> {
    let mut out = Vec::new();
    for flt in filters {
        match flt.kind {
            FilterKind::Select | FilterKind::MultiSelect => {
                if let serde_json::Value::Array(arr) = flt.value {
                    out.push(FilterSpec {
                        field: flt.column.clone(),
                        op: FilterOp::In,
                        value: serde_json::Value::Array(arr.clone()),
                    });
                } else {
                    out.push(FilterSpec {
                        field: flt.column.clone(),
                        op: FilterOp::Eq,
                        value: flt.value.clone(),
                    });
                }
            }
            FilterKind::DateRange | FilterKind::NumberRange => {
                out.push(FilterSpec {
                    field: flt.column.clone(),
                    op: FilterOp::Between,
                    value: flt.value.clone(),
                });
            }
            FilterKind::Text => {
                let text = flt.value.as_str().unwrap_or_default().trim();
                if !text.is_empty() {
                    out.push(FilterSpec {
                        field: flt.column.clone(),
                        op: FilterOp::Like,
                        value: serde_json::Value::String(format!("%{text}%")),
                    });
                }
            }
        }
    }
    for item in drill_path {
        if item.value.is_null() {
            out.push(FilterSpec {
                field: item.field.clone(),
                op: FilterOp::IsNull,
                value: serde_json::Value::Null,
            });
        } else {
            out.push(FilterSpec {
                field: item.field.clone(),
                op: FilterOp::Eq,
                value: item.value.clone(),
            });
        }
    }
    out
}

fn parse_rollup_grain_opt(raw: Option<&str>) -> Result<Option<RollupGrain>, AppError> {
    let Some(s) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    match s.to_ascii_uppercase().as_str() {
        "MINUTE" => Ok(Some(RollupGrain::Minute)),
        "HOUR" => Ok(Some(RollupGrain::Hour)),
        "DAY" | "WEEK" => Ok(Some(RollupGrain::Day)),
        "MONTH" | "YEAR" => Ok(Some(RollupGrain::Month)),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unsupported time_grain '{s}'"),
        )),
    }
}

fn parse_metric_time_range_opt(raw: Option<&str>) -> Result<Option<MetricTimeRange>, AppError> {
    let Some(s) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let now = Utc::now();
    let from = match s.to_ascii_uppercase().as_str() {
        "LAST_24_HOURS" | "LAST_1_DAY" => now - ChronoDuration::hours(24),
        "LAST_7_DAYS" => now - ChronoDuration::days(7),
        "LAST_30_DAYS" => now - ChronoDuration::days(30),
        "LAST_90_DAYS" => now - ChronoDuration::days(90),
        "TODAY" => now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .map_or(now - ChronoDuration::hours(24), |dt| dt.and_utc()),
        "THIS_MONTH" => now
            .date_naive()
            .with_day(1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map_or(now - ChronoDuration::days(30), |dt| dt.and_utc()),
        "THIS_YEAR" => now
            .date_naive()
            .with_ordinal(1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map_or(now - ChronoDuration::days(365), |dt| dt.and_utc()),
        _ => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unsupported time_range preset '{s}'"),
            ));
        }
    };
    Ok(Some(MetricTimeRange { from, to: now }))
}

fn parse_agg_fn(raw: &str) -> Result<AggFn, AppError> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "SUM" => Ok(AggFn::Sum),
        "AVG" => Ok(AggFn::Avg),
        "COUNT" => Ok(AggFn::Count),
        "COUNT_DISTINCT" => Ok(AggFn::CountDistinct),
        "MIN" => Ok(AggFn::Min),
        "MAX" => Ok(AggFn::Max),
        "MEDIAN" => Ok(AggFn::Median),
        "LAST" => Ok(AggFn::Last),
        other => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unsupported agg '{other}'"),
        )),
    }
}

fn col_def(name: &str, data_type: DataType) -> WidgetColumn {
    WidgetColumn {
        name: name.to_owned(),
        data_type,
    }
}

fn opt_f64_json(val: Option<f64>) -> serde_json::Value {
    val.and_then(serde_json::Number::from_f64)
        .map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn cap_rows(rows: &mut Vec<Vec<serde_json::Value>>) -> bool {
    if rows.len() > WIDGET_MAX_ROWS {
        rows.truncate(WIDGET_MAX_ROWS);
        true
    } else {
        false
    }
}

fn finalize_response(
    columns: Vec<WidgetColumn>,
    rows: Vec<Vec<serde_json::Value>>,
    truncated: bool,
    drill_level: usize,
) -> WidgetDataResponse {
    let row_count = rows.len();
    let data_version = compute_data_version(&columns, &rows, drill_level);
    WidgetDataResponse {
        columns,
        rows,
        row_count,
        truncated,
        generated_at: Utc::now(),
        data_version,
        drill_level,
    }
}

fn compute_data_version(
    columns: &[WidgetColumn],
    rows: &[Vec<serde_json::Value>],
    drill_level: usize,
) -> String {
    let mut hasher = Sha256::new();
    if let Ok(cols_json) = serde_json::to_vec(columns) {
        hasher.update(&cols_json);
    }
    if let Ok(rows_json) = serde_json::to_vec(rows) {
        hasher.update(&rows_json);
    }
    hasher.update(drill_level.to_le_bytes());
    let full = hex::encode(hasher.finalize());
    full.get(..16).unwrap_or(&full).to_owned()
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::enums::PiiCategory;
    use axpoint_domain::view::{CanvasConfig, DashboardStatus, DashboardTheme};

    #[test]
    fn build_wrapped_sql_applies_filters_and_drill_grouping() -> Result<(), AppError> {
        let base_cols = vec![
            QueryColumn {
                name: String::from("status"),
                data_type: DataType::String,
                pii_category: PiiCategory::None,
                masked: false,
            },
            QueryColumn {
                name: String::from("warehouse_code"),
                data_type: DataType::String,
                pii_category: PiiCategory::None,
                masked: false,
            },
            QueryColumn {
                name: String::from("amount"),
                data_type: DataType::Float64,
                pii_category: PiiCategory::None,
                masked: false,
            },
        ];
        let drill = vec![
            DrillLevel {
                field: String::from("status"),
                label: None,
                node_type: None,
                node_key_field: None,
            },
            DrillLevel {
                field: String::from("warehouse_code"),
                label: None,
                node_type: None,
                node_key_field: None,
            },
        ];
        let drill_path = vec![DrillPathItem {
            field: String::from("status"),
            value: serde_json::json!("PAID"),
        }];
        let encoding = serde_json::json!({"x": "status", "y": ["amount"]});
        let ds = WidgetDataSource::Sql {
            sql: String::from("SELECT status, warehouse_code, amount FROM demo_orders"),
        };
        let spec = WidgetExecSpec {
            widget_id: None,
            widget_kind: WidgetKind::BarChart,
            data_source: &ds,
            encoding: &encoding,
            drill: &drill,
            filters: Vec::new(),
            drill_path: &drill_path,
        };
        let mut params = BTreeMap::new();
        let wrapped = build_wrapped_sql(
            "SELECT status, warehouse_code, amount FROM demo_orders",
            &base_cols,
            &spec,
            &mut params,
        )?;
        assert!(wrapped.contains("SELECT \"warehouse_code\", SUM(\"amount\") AS \"amount\""));
        assert!(wrapped.contains("WHERE \"status\" = :_vd0"));
        assert!(wrapped.contains("GROUP BY \"warehouse_code\""));
        assert_eq!(params.get("_vd0"), Some(&serde_json::json!("PAID")));
        Ok(())
    }

    #[test]
    fn resolve_dashboard_filters_ignores_unmapped_widgets() {
        let now = Utc::now();
        let w1 = Uuid::from_u128(1);
        let w2 = Uuid::from_u128(2);
        let dash = DashboardRecord {
            id: Uuid::from_u128(99),
            tenant_id: String::from("default"),
            name: String::from("Ops"),
            slug: String::from("ops"),
            description: String::new(),
            filters: vec![FilterDef {
                key: String::from("status"),
                label: String::from("Status"),
                kind: FilterKind::MultiSelect,
                field_map: BTreeMap::from([(w1.to_string(), String::from("status"))]),
                options_source: None,
                default: None,
            }],
            theme: DashboardTheme::Light,
            canvas: CanvasConfig::default(),
            status: DashboardStatus::Published,
            version: 1,
            created_by: None,
            created_at: now,
            updated_at: now,
        };
        let raw = BTreeMap::from([(String::from("status"), serde_json::json!(["PAID"]))]);
        assert_eq!(resolve_dashboard_filters(&dash, w1, &raw).len(), 1);
        assert!(resolve_dashboard_filters(&dash, w2, &raw).is_empty());
    }
}
