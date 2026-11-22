//! `xBlueView` five-zone preset context builder and pre-publish / pre-share widget health preflight (`06-STEP20` §20.3).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_analytics::{
    CatalogDatasetEntry, GuardOptions, LiveConnectionSpec, TenantCatalog, compile_pivot, guard,
    guard_and_rewrite, is_metric_broken_for_dataset,
};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{PivotSpec, SavedQuery, SavedQueryKind};
use axpoint_domain::axhub::{Dataset, DatasetListQuery, DatasetStatus};
use axpoint_domain::enums::{AssetKind, Environment, Permission};
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::ports::{
    AnalyticsRepository, ConnectionRepository, DatasetRepository, FlowRepository, GraphStorePort,
    GuardRepository, SimRepository, StudioRepository, ViewRepository,
};
use axpoint_domain::sim::ScenarioStatus;
use axpoint_domain::studio::{NodeTypeSpec, PropertyKind, SchemaDoc};
use axpoint_domain::view::{
    AssetPreflightState, CreateWidgetRecordParams, DashboardDetail, DashboardPreflightContext,
    DashboardPreflightReport, FiveZonePresetContext, FlowPreflightState, MetricPreflightState,
    NodePreflightState, SavedQueryPreflightState, ScenarioPreflightState, SqlPreflightOutcome,
    WidgetDataSource, evaluate_dashboard_preflight, generate_five_zone_preset,
};
use axpoint_security::AuthPrincipal;
use axpoint_security::cedar_authz::CedarAuthorizer;
use uuid::Uuid;

use super::deps::compute_widget_deps;

#[derive(Default)]
struct ReferencedIds {
    saved_queries: BTreeSet<Uuid>,
    assets: BTreeSet<Uuid>,
    scenarios: BTreeSet<Uuid>,
    flows: BTreeSet<Uuid>,
    nodes: BTreeSet<Uuid>,
}

impl ReferencedIds {
    fn record_source(&mut self, source: &WidgetDataSource) {
        match source {
            WidgetDataSource::SavedQuery { saved_query_id } => {
                self.saved_queries.insert(*saved_query_id);
            }
            WidgetDataSource::FloorPlan { asset_id, .. }
            | WidgetDataSource::Scene3d { asset_id, .. } => {
                self.assets.insert(*asset_id);
            }
            WidgetDataSource::SimScenarios { scenario_ids, .. } => {
                self.scenarios.extend(scenario_ids.iter().copied());
            }
            WidgetDataSource::Action {
                flow_id, node_id, ..
            } => {
                if let Some(fid) = flow_id {
                    self.flows.insert(*fid);
                }
                if let Some(nid) = node_id {
                    self.nodes.insert(*nid);
                }
            }
            _ => {}
        }
    }

    fn from_dashboard(detail: &DashboardDetail) -> Self {
        let mut refs = Self::default();
        for filter in &detail.dashboard.filters {
            if let Some(ref src) = filter.options_source {
                refs.record_source(src);
            }
        }
        for widget in &detail.widgets {
            refs.record_source(&widget.data_source);
        }
        refs
    }
}

async fn load_active_datasets<S: DatasetRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
) -> Result<Vec<Dataset>, AppError> {
    let mut out = Vec::new();
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
        for ds in page.items {
            if ds.status != DatasetStatus::Disabled {
                out.push(ds);
            }
        }
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(out)
}

async fn build_tenant_catalog<S: DatasetRepository + ConnectionRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
    datasets: &[Dataset],
    has_live_perm: bool,
) -> Result<TenantCatalog, AppError> {
    let mut catalog = TenantCatalog::new(tenant_id).with_live_permission(has_live_perm);
    for ds in datasets {
        let cols = store.list_columns(tenant_id, ds.id).await?;
        catalog.insert_dataset(CatalogDatasetEntry::from_dataset(ds, &cols));
    }
    let conns = store.list_connections(tenant_id).await?;
    for spec in LiveConnectionSpec::from_connections(&conns) {
        catalog.insert_live_connection(&spec.name);
    }
    Ok(catalog)
}

fn collect_schema_node_metrics(
    schema: &SchemaDoc,
    populated_types: &BTreeSet<String>,
) -> Vec<(String, String)> {
    let mut populated_pairs = Vec::new();
    let mut unpopulated_pairs = Vec::new();

    for nt in &schema.node_types {
        let metrics = extract_declared_node_metrics(nt);
        let is_pop = populated_types.contains(&nt.name);
        for metric_name in metrics {
            if is_pop {
                populated_pairs.push((nt.name.clone(), metric_name));
            } else {
                unpopulated_pairs.push((nt.name.clone(), metric_name));
            }
        }
    }
    populated_pairs.extend(unpopulated_pairs);
    populated_pairs
}

fn extract_declared_node_metrics(nt: &NodeTypeSpec) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for p in &nt.properties {
        if p.kind == PropertyKind::Metric && seen.insert(p.name.clone()) {
            ordered.push(p.name.clone());
        }
    }
    for r in &nt.agg_rules {
        if !r.target_metric.trim().is_empty() && seen.insert(r.target_metric.clone()) {
            ordered.push(r.target_metric.clone());
        }
    }
    for d in &nt.derived_rules {
        if d.enabled && !d.target_metric.trim().is_empty() && seen.insert(d.target_metric.clone()) {
            ordered.push(d.target_metric.clone());
        }
    }
    ordered
}

async fn find_succeeded_scenario_pair<S: SimRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
) -> Result<Vec<Uuid>, AppError> {
    let sandboxes = store.list_sandboxes(tenant_id).await?;
    let mut stale_fallback: Option<Vec<Uuid>> = None;

    for sb in sandboxes.into_iter().take(20) {
        let scenarios = store.list_scenarios(tenant_id, sb.id).await?;
        let fresh: Vec<Uuid> = scenarios
            .iter()
            .filter(|sc| sc.status == ScenarioStatus::Succeeded && !sc.is_stale)
            .map(|sc| sc.id)
            .take(5)
            .collect();
        if fresh.len() >= 2 {
            return Ok(fresh);
        }
        if stale_fallback.is_none() {
            let all_ok: Vec<Uuid> = scenarios
                .iter()
                .filter(|sc| sc.status == ScenarioStatus::Succeeded)
                .map(|sc| sc.id)
                .take(5)
                .collect();
            if all_ok.len() >= 2 {
                stale_fallback = Some(all_ok);
            }
        }
    }

    Ok(stale_fallback.unwrap_or_default())
}

/// Builds a [`FiveZonePresetContext`] from the tenant's certified `an_metrics`, `SUCCEEDED`
/// `ax_sim_scenarios` (when `>= 2` exist in the same sandbox), and published `main` `SchemaDoc` (`06-STEP20` §20.3.1).
///
/// # Errors
/// Propagates storage errors from `DatasetRepository`, `AnalyticsRepository`, `StudioRepository`, or `SimRepository`.
pub async fn build_five_zone_preset_context<S>(
    store: &S,
    tenant_id: &str,
) -> Result<FiveZonePresetContext, AppError>
where
    S: AnalyticsRepository + DatasetRepository + SimRepository + StudioRepository + ?Sized,
{
    let datasets = load_active_datasets(store, tenant_id).await?;
    let mut catalog = TenantCatalog::new(tenant_id);
    for ds in &datasets {
        let cols = store.list_columns(tenant_id, ds.id).await?;
        catalog.insert_dataset(CatalogDatasetEntry::from_dataset(ds, &cols));
    }

    let all_metrics = store.list_metrics(tenant_id).await?;
    let mut metric_names = Vec::new();
    let mut metric_labels = BTreeMap::new();
    for m in all_metrics {
        let Some(ds_entry) = catalog.dataset_by_id(m.dataset_id) else {
            continue;
        };
        if m.certified && !is_metric_broken_for_dataset(&m, ds_entry) {
            if !m.display_name.trim().is_empty() {
                metric_labels.insert(m.name.clone(), m.display_name.clone());
            }
            metric_names.push(m.name);
            if metric_names.len() >= 4 {
                break;
            }
        }
    }

    let published = store.get_published_schema(tenant_id, "main").await?;
    let val_ctx = store.load_validation_context(tenant_id, "main").await?;
    let populated_types: BTreeSet<String> = val_ctx
        .existing_node_counts
        .into_iter()
        .filter_map(|(name, cnt)| (cnt > 0).then_some(name))
        .collect();
    let node_metrics = collect_schema_node_metrics(&published.doc, &populated_types);
    let succeeded_scenario_ids = find_succeeded_scenario_pair(store, tenant_id).await?;

    Ok(FiveZonePresetContext {
        metric_names,
        metric_labels,
        node_metrics,
        succeeded_scenario_ids,
    })
}

/// Generates the 10 canonical `STANDARD_FIVE_ZONE` starter widgets with precomputed [`WidgetDeps`](axpoint_domain::view::WidgetDeps).
///
/// # Errors
/// Propagates storage or dependency computation errors.
pub async fn build_five_zone_preset_widgets<S>(
    store: &S,
    tenant_id: &str,
) -> Result<Vec<CreateWidgetRecordParams>, AppError>
where
    S: AnalyticsRepository
        + DatasetRepository
        + SimRepository
        + StudioRepository
        + ViewRepository
        + ?Sized,
{
    let ctx = build_five_zone_preset_context(store, tenant_id).await?;
    let slots = generate_five_zone_preset(&ctx);
    let mut params = Vec::with_capacity(slots.len());
    for slot in slots {
        let deps = compute_widget_deps(store, tenant_id, &slot.data_source).await?;
        params.push(CreateWidgetRecordParams {
            dashboard_id: Uuid::nil(),
            widget_kind: slot.widget_kind,
            title: slot.title,
            position: slot.position,
            data_source: slot.data_source,
            encoding: slot.encoding,
            options: slot.options,
            drill: Vec::new(),
            refresh_mode: slot.refresh_mode,
            refresh_seconds: slot.refresh_seconds,
            deps,
        });
    }
    Ok(params)
}

struct SqlEvalEnv<'a> {
    catalog: &'a TenantCatalog,
    policy: &'a PolicyContext,
    unauthorized_dataset_ids: &'a BTreeSet<Uuid>,
    has_data_read: bool,
}

fn evaluate_sql_or_pivot_source(
    source: &WidgetDataSource,
    env: &SqlEvalEnv<'_>,
) -> Option<SqlPreflightOutcome> {
    match source {
        WidgetDataSource::Sql { sql } => {
            if sql.trim().is_empty() {
                return None;
            }
            Some(evaluate_raw_sql(sql, env))
        }
        WidgetDataSource::Pivot { spec } => evaluate_pivot_ast(spec, env),
        _ => None,
    }
}

fn evaluate_raw_sql(sql: &str, env: &SqlEvalEnv<'_>) -> SqlPreflightOutcome {
    let opts = GuardOptions {
        allow_live: env.catalog.has_live_permission,
        policy: Some(env.policy.clone()),
        ..GuardOptions::default()
    };
    match guard(sql, env.catalog, &opts) {
        Ok(guarded) => {
            let needs_read = !guarded.referenced_datasets.is_empty()
                || guarded.uses_platform_views
                || !guarded.referenced_live_connections.is_empty();
            if needs_read && !env.has_data_read {
                return SqlPreflightOutcome::UnauthorizedDataset {
                    message: String::from("publisher lacks DATA_READ permission"),
                };
            }
            if guarded
                .referenced_datasets
                .iter()
                .any(|id| env.unauthorized_dataset_ids.contains(id))
            {
                return SqlPreflightOutcome::UnauthorizedDataset {
                    message: String::from("publisher lacks access to referenced dataset"),
                };
            }
            SqlPreflightOutcome::Valid {
                referenced_dataset_ids: guarded.referenced_datasets,
            }
        }
        Err(AppError::Forbidden(_)) => SqlPreflightOutcome::UnauthorizedDataset {
            message: String::from("publisher lacks policy access to referenced dataset"),
        },
        Err(err) => {
            let message = err.client_message();
            if message.contains("unknown or unauthorized dataset") {
                SqlPreflightOutcome::MissingDataset { message }
            } else {
                SqlPreflightOutcome::InvalidAst { message }
            }
        }
    }
}

fn evaluate_pivot_ast(
    spec: &serde_json::Value,
    env: &SqlEvalEnv<'_>,
) -> Option<SqlPreflightOutcome> {
    let Ok(pivot) = serde_json::from_value::<PivotSpec>(spec.clone()) else {
        return None;
    };
    let ds_name = pivot.dataset.trim();
    if ds_name.is_empty() || pivot.values.is_empty() {
        return None;
    }
    if !env.has_data_read {
        return Some(SqlPreflightOutcome::UnauthorizedDataset {
            message: String::from("publisher lacks DATA_READ permission"),
        });
    }
    match compile_pivot(&pivot, env.catalog) {
        Ok(_) => {
            let referenced_dataset_ids =
                env.catalog.dataset(ds_name).map_or_default(|d| vec![d.id]);
            Some(SqlPreflightOutcome::Valid {
                referenced_dataset_ids,
            })
        }
        Err(err) => {
            let message = err.client_message();
            if message.contains("unknown or unauthorized dataset")
                || (!ds_name.starts_with("ax.") && env.catalog.dataset(ds_name).is_none())
            {
                Some(SqlPreflightOutcome::MissingDataset { message })
            } else {
                Some(SqlPreflightOutcome::InvalidAst { message })
            }
        }
    }
}

fn resolve_saved_query_state(
    sq: &SavedQuery,
    catalog: &TenantCatalog,
    existing_dataset_ids: &BTreeSet<Uuid>,
    dataset_ids_by_name: &BTreeMap<String, Uuid>,
    metrics: &BTreeMap<String, MetricPreflightState>,
) -> SavedQueryPreflightState {
    match sq.kind {
        SavedQueryKind::Sql | SavedQueryKind::Ask => {
            let Some(sql) = sq.spec.get("sql").and_then(serde_json::Value::as_str) else {
                return SavedQueryPreflightState {
                    referenced_dataset_ids: Vec::new(),
                    dataset_missing: true,
                };
            };
            match guard_and_rewrite(sql, catalog, 100) {
                Ok(gq) => SavedQueryPreflightState {
                    dataset_missing: gq
                        .referenced_datasets
                        .iter()
                        .any(|id| !existing_dataset_ids.contains(id)),
                    referenced_dataset_ids: gq.referenced_datasets,
                },
                Err(_) => SavedQueryPreflightState {
                    referenced_dataset_ids: Vec::new(),
                    dataset_missing: true,
                },
            }
        }
        SavedQueryKind::Metric => {
            let name = sq
                .spec
                .get("metric_name")
                .or_else(|| sq.spec.get("metric"))
                .and_then(serde_json::Value::as_str)
                .map_or("", str::trim);
            if let Some(m) = metrics.get(name)
                && existing_dataset_ids.contains(&m.dataset_id)
            {
                SavedQueryPreflightState {
                    referenced_dataset_ids: vec![m.dataset_id],
                    dataset_missing: false,
                }
            } else {
                SavedQueryPreflightState {
                    referenced_dataset_ids: Vec::new(),
                    dataset_missing: true,
                }
            }
        }
        SavedQueryKind::Pivot => {
            let Ok(pivot) = serde_json::from_value::<PivotSpec>(sq.spec.clone()) else {
                return SavedQueryPreflightState {
                    referenced_dataset_ids: Vec::new(),
                    dataset_missing: true,
                };
            };
            let ds_name = pivot.dataset.trim();
            if ds_name.starts_with("ax.") {
                SavedQueryPreflightState {
                    referenced_dataset_ids: Vec::new(),
                    dataset_missing: false,
                }
            } else if let Some(&ds_id) = dataset_ids_by_name.get(ds_name) {
                SavedQueryPreflightState {
                    referenced_dataset_ids: vec![ds_id],
                    dataset_missing: !existing_dataset_ids.contains(&ds_id),
                }
            } else {
                SavedQueryPreflightState {
                    referenced_dataset_ids: Vec::new(),
                    dataset_missing: true,
                }
            }
        }
    }
}

async fn load_referenced_assets<S: ViewRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
    asset_ids: &BTreeSet<Uuid>,
) -> Result<BTreeMap<Uuid, AssetPreflightState>, AppError> {
    let mut out = BTreeMap::new();
    for &aid in asset_ids {
        let Some(asset) = store.get_asset(tenant_id, aid).await? else {
            continue;
        };
        let binding_count = match asset.asset_kind {
            AssetKind::FloorPlanImage => store.list_floor_placements(tenant_id, aid).await?.len(),
            AssetKind::Model3d => store.list_scene3d_bindings(tenant_id, aid).await?.len(),
            _ => 0,
        };
        out.insert(
            aid,
            AssetPreflightState {
                asset_kind: asset.asset_kind,
                binding_count,
            },
        );
    }
    Ok(out)
}

async fn load_referenced_scenarios_and_flows<S: SimRepository + FlowRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
    scenario_ids: &BTreeSet<Uuid>,
    flow_ids: &BTreeSet<Uuid>,
) -> Result<
    (
        BTreeMap<Uuid, ScenarioPreflightState>,
        BTreeMap<Uuid, FlowPreflightState>,
    ),
    AppError,
> {
    let mut scenarios = BTreeMap::new();
    for &sid in scenario_ids {
        if let Some(sc) = store.get_scenario(tenant_id, sid).await? {
            scenarios.insert(
                sid,
                ScenarioPreflightState {
                    status: sc.status,
                    is_stale: sc.is_stale,
                },
            );
        }
    }

    let mut flows = BTreeMap::new();
    for &fid in flow_ids {
        if store.get_flow(tenant_id, fid).await?.is_some() {
            let has_prod_deployment = store
                .get_flow_deployment(tenant_id, fid, Environment::Prod)
                .await?
                .is_some();
            flows.insert(
                fid,
                FlowPreflightState {
                    has_prod_deployment,
                },
            );
        }
    }
    Ok((scenarios, flows))
}

async fn load_referenced_nodes<S: GraphStorePort + ?Sized>(
    store: &S,
    principal: &AuthPrincipal,
    policy: &PolicyContext,
    node_ids: &BTreeSet<Uuid>,
) -> Result<BTreeMap<Uuid, NodePreflightState>, AppError> {
    let mut out = BTreeMap::new();
    let bypass_dept = principal.has(Permission::DataAdmin) || principal.has(Permission::GuardAdmin);
    let caller_dept = principal.dept_id.trim();

    for &nid in node_ids {
        let Some(node) = store.get_node(&principal.tenant_id, nid).await? else {
            continue;
        };
        let dept_ok = bypass_dept
            || caller_dept.is_empty()
            || node
                .properties
                .get("dept_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none_or(|d| d == caller_dept);
        let type_ok = policy.bypass_row_policies
            || policy
                .node_types
                .get(&node.type_name)
                .is_none_or(|spec| spec.pg_row_filter.as_deref() != Some("FALSE"));
        out.insert(
            nid,
            NodePreflightState {
                type_name: node.type_name,
                authorized: dept_ok && type_ok,
            },
        );
    }
    Ok(out)
}

fn index_datasets_and_unauthorized(
    datasets: &[Dataset],
    policy: &PolicyContext,
    has_data_read: bool,
) -> (BTreeSet<Uuid>, BTreeMap<String, Uuid>, BTreeSet<Uuid>) {
    let mut existing = BTreeSet::new();
    let mut by_name = BTreeMap::new();
    let mut unauthorized = BTreeSet::new();

    for ds in datasets {
        existing.insert(ds.id);
        by_name.insert(ds.name.clone(), ds.id);
        by_name.insert(ds.name.to_ascii_lowercase(), ds.id);
        by_name.insert(ds.lake_table.clone(), ds.id);
        let denied = !has_data_read
            || policy.denied_datasets.contains(&ds.id.to_string())
            || policy
                .denied_datasets
                .contains(&ds.name.to_ascii_lowercase());
        if denied {
            unauthorized.insert(ds.id);
        }
    }
    (existing, by_name, unauthorized)
}

type SqlOutcomePair = (
    BTreeMap<Uuid, SqlPreflightOutcome>,
    BTreeMap<String, SqlPreflightOutcome>,
);

fn evaluate_dashboard_sql_sources(
    detail: &DashboardDetail,
    env: &SqlEvalEnv<'_>,
) -> SqlOutcomePair {
    let mut filter_sql_outcomes = BTreeMap::new();
    for f in &detail.dashboard.filters {
        if let Some(ref src) = f.options_source
            && let Some(out) = evaluate_sql_or_pivot_source(src, env)
        {
            filter_sql_outcomes.insert(f.key.clone(), out);
        }
    }
    let mut sql_outcomes = BTreeMap::new();
    for w in &detail.widgets {
        if let Some(out) = evaluate_sql_or_pivot_source(&w.data_source, env) {
            sql_outcomes.insert(w.id, out);
        }
    }
    (sql_outcomes, filter_sql_outcomes)
}

/// Hydrates a [`DashboardPreflightContext`] from live storage, `TenantCatalog`, `sql_guard`, and `AxGuard` `PolicyContext`.
///
/// # Errors
/// Propagates database or policy compilation errors.
pub async fn build_dashboard_preflight_context<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    principal: &AuthPrincipal,
    detail: &DashboardDetail,
    client_ip: &str,
) -> Result<DashboardPreflightContext, AppError>
where
    S: AnalyticsRepository
        + ConnectionRepository
        + DatasetRepository
        + FlowRepository
        + GraphStorePort
        + GuardRepository
        + SimRepository
        + StudioRepository
        + ViewRepository
        + ?Sized,
{
    let tenant_id = &principal.tenant_id;
    let has_data_read = principal.has(Permission::DataRead);
    let datasets = load_active_datasets(store, tenant_id).await?;
    let catalog = build_tenant_catalog(
        store,
        tenant_id,
        &datasets,
        principal.has(Permission::DataSync),
    )
    .await?;
    let policy =
        crate::policy::build_policy_context_for_store(store, cedar, principal, false, client_ip)
            .await?;

    let (existing_dataset_ids, dataset_ids_by_name, unauthorized_dataset_ids) =
        index_datasets_and_unauthorized(&datasets, &policy, has_data_read);

    let mut metrics = BTreeMap::new();
    for m in store.list_metrics(tenant_id).await? {
        let broken = m.broken
            || catalog
                .dataset_by_id(m.dataset_id)
                .is_some_and(|ds_entry| is_metric_broken_for_dataset(&m, ds_entry));
        metrics.insert(
            m.name.clone(),
            MetricPreflightState {
                dataset_id: m.dataset_id,
                broken,
                certified: m.certified,
            },
        );
    }

    let (sql_outcomes, filter_sql_outcomes) = evaluate_dashboard_sql_sources(
        detail,
        &SqlEvalEnv {
            catalog: &catalog,
            policy: &policy,
            unauthorized_dataset_ids: &unauthorized_dataset_ids,
            has_data_read,
        },
    );

    let refs = ReferencedIds::from_dashboard(detail);
    let mut saved_queries = BTreeMap::new();
    for &sq_id in &refs.saved_queries {
        if let Some(sq) = store.get_saved_query(tenant_id, sq_id).await? {
            saved_queries.insert(
                sq_id,
                resolve_saved_query_state(
                    &sq,
                    &catalog,
                    &existing_dataset_ids,
                    &dataset_ids_by_name,
                    &metrics,
                ),
            );
        }
    }

    let main_schema = store.get_published_schema(tenant_id, "main").await?.doc;
    let val_ctx = store.load_validation_context(tenant_id, "main").await?;
    let populated_node_types = val_ctx
        .existing_node_counts
        .into_iter()
        .filter_map(|(name, cnt)| (cnt > 0).then_some(name))
        .collect();

    let assets = load_referenced_assets(store, tenant_id, &refs.assets).await?;
    let (scenarios, flows) =
        load_referenced_scenarios_and_flows(store, tenant_id, &refs.scenarios, &refs.flows).await?;
    let nodes = load_referenced_nodes(store, principal, &policy, &refs.nodes).await?;

    Ok(DashboardPreflightContext {
        existing_dataset_ids,
        dataset_ids_by_name,
        unauthorized_dataset_ids,
        saved_queries,
        metrics,
        sql_outcomes,
        filter_sql_outcomes,
        main_schema,
        populated_node_types,
        assets,
        scenarios,
        flows,
        nodes,
        caller_has_flow_run: principal.has(Permission::FlowRun),
    })
}

/// Runs full widget and filter preflight validation on `detail` (`POST /api/xblueview/dashboards/{id}/preflight`).
///
/// # Errors
/// Propagates database or policy compilation errors.
pub async fn preflight_dashboard<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    principal: &AuthPrincipal,
    detail: &DashboardDetail,
    client_ip: &str,
) -> Result<DashboardPreflightReport, AppError>
where
    S: AnalyticsRepository
        + ConnectionRepository
        + DatasetRepository
        + FlowRepository
        + GraphStorePort
        + GuardRepository
        + SimRepository
        + StudioRepository
        + ViewRepository
        + ?Sized,
{
    let ctx = build_dashboard_preflight_context(store, cedar, principal, detail, client_ip).await?;
    Ok(evaluate_dashboard_preflight(detail, &ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::enums::{DataType, PiiCategory, WidgetKind};
    use axpoint_domain::studio::PropertySpec;
    use axpoint_domain::view::{
        CanvasConfig, DashboardPreflightStatus, DashboardRecord, DashboardStatus, DashboardTheme,
        RefreshMode, WidgetActionType, WidgetDeps, WidgetPosition, WidgetRecord,
        WriteNodeTargetKind,
    };
    use chrono::Utc;

    fn make_test_widget(
        dash_id: Uuid,
        kind: WidgetKind,
        title: &str,
        pos: (i32, i32, i32, i32),
        ds: WidgetDataSource,
    ) -> WidgetRecord {
        let now = Utc::now();
        WidgetRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from("default"),
            dashboard_id: dash_id,
            widget_kind: kind,
            title: String::from(title),
            position: WidgetPosition {
                x: pos.0,
                y: pos.1,
                w: pos.2,
                h: pos.3,
            },
            data_source: ds,
            encoding: serde_json::json!({}),
            options: serde_json::json!({}),
            drill: Vec::new(),
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
            deps: WidgetDeps::default(),
            created_at: now,
            updated_at: now,
        }
    }

    fn seeded_analytical_widgets(dash_id: Uuid) -> Vec<WidgetRecord> {
        vec![
            make_test_widget(
                dash_id,
                WidgetKind::KpiCard,
                "已支付金额",
                (0, 0, 6, 4),
                WidgetDataSource::Metric {
                    metric: String::from("paid_amount"),
                    dimensions: Vec::new(),
                    time_grain: None,
                    time_range: None,
                },
            ),
            make_test_widget(
                dash_id,
                WidgetKind::BarChart,
                "按状态订单金额",
                (6, 0, 10, 6),
                WidgetDataSource::Sql {
                    sql: String::from(
                        "SELECT status, warehouse_code, sum(amount) AS total_amount FROM demo_orders GROUP BY status, warehouse_code",
                    ),
                },
            ),
            make_test_widget(
                dash_id,
                WidgetKind::LineChart,
                "仓库库存趋势",
                (16, 0, 8, 6),
                WidgetDataSource::NodeMetric {
                    type_name: String::from("Warehouse"),
                    metric: String::from("inventory"),
                    agg: Some(String::from("AVG")),
                    grain: Some(String::from("HOUR")),
                    time_range: Some(String::from("LAST_7_DAYS")),
                    group_by: Some(String::from("time")),
                },
            ),
            make_test_widget(
                dash_id,
                WidgetKind::Map,
                "供应链地理态势",
                (0, 4, 6, 6),
                WidgetDataSource::NodesGeo {
                    type_names: vec![String::from("Factory"), String::from("Warehouse")],
                    color_metric: Some(String::from("inventory")),
                    link_types: vec![String::from("SUPPLIES")],
                },
            ),
        ]
    }

    fn seeded_twin_and_decision_widgets(
        dash_id: Uuid,
        asset_id: Uuid,
        wh_node_id: Uuid,
        s1_id: Uuid,
        s2_id: Uuid,
    ) -> Vec<WidgetRecord> {
        vec![
            make_test_widget(
                dash_id,
                WidgetKind::Scene3d,
                "仓库三维孪生",
                (6, 6, 10, 8),
                WidgetDataSource::Scene3d {
                    asset_id,
                    color_metric: Some(String::from("inventory")),
                },
            ),
            make_test_widget(
                dash_id,
                WidgetKind::AlertList,
                "运营告警列表",
                (16, 6, 8, 4),
                WidgetDataSource::Alerts {
                    status: vec![String::from("OPEN"), String::from("ACK")],
                    severity: Vec::new(),
                    limit: Some(20),
                },
            ),
            make_test_widget(
                dash_id,
                WidgetKind::SimCompare,
                "推演方案对比",
                (0, 10, 6, 4),
                WidgetDataSource::SimScenarios {
                    scenario_ids: vec![s1_id, s2_id],
                    metrics: vec![String::from("fulfillment")],
                },
            ),
            make_test_widget(
                dash_id,
                WidgetKind::ActionForm,
                "库存盘点回写",
                (16, 10, 8, 4),
                WidgetDataSource::Action {
                    action_type: WidgetActionType::WriteNode,
                    flow_id: None,
                    node_id: Some(wh_node_id),
                    target_kind: Some(WriteNodeTargetKind::Metric),
                    key: Some(String::from("inventory")),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "required": ["value"],
                        "properties": { "value": { "type": "number" } }
                    }),
                    confirm_text: None,
                },
            ),
        ]
    }

    fn seeded_supply_overview_widgets(
        dash_id: Uuid,
        asset_id: Uuid,
        wh_node_id: Uuid,
        s1_id: Uuid,
        s2_id: Uuid,
    ) -> Vec<WidgetRecord> {
        let mut widgets = seeded_analytical_widgets(dash_id);
        widgets.extend(seeded_twin_and_decision_widgets(
            dash_id, asset_id, wh_node_id, s1_id, s2_id,
        ));
        widgets
    }

    fn build_step11_test_context(
        ds_id: Uuid,
        sql_wid: Uuid,
        asset_id: Uuid,
        wh_node_id: Uuid,
        scenarios_pair: (Uuid, Uuid),
    ) -> DashboardPreflightContext {
        let catalog = TenantCatalog::new("default").with_dataset(CatalogDatasetEntry::simple(
            ds_id,
            "demo_orders",
            "demo_orders",
        ));
        let policy = PolicyContext {
            bypass_row_policies: true,
            ..PolicyContext::default()
        };
        let unauthorized = BTreeSet::new();
        let sql_out = evaluate_raw_sql(
            "SELECT status, warehouse_code, sum(amount) AS total_amount FROM demo_orders GROUP BY status, warehouse_code",
            &SqlEvalEnv {
                catalog: &catalog,
                policy: &policy,
                unauthorized_dataset_ids: &unauthorized,
                has_data_read: true,
            },
        );

        DashboardPreflightContext {
            existing_dataset_ids: BTreeSet::from([ds_id]),
            dataset_ids_by_name: BTreeMap::from([(String::from("demo_orders"), ds_id)]),
            unauthorized_dataset_ids: unauthorized,
            saved_queries: BTreeMap::new(),
            metrics: BTreeMap::from([(
                String::from("paid_amount"),
                MetricPreflightState {
                    dataset_id: ds_id,
                    broken: false,
                    certified: true,
                },
            )]),
            sql_outcomes: BTreeMap::from([(sql_wid, sql_out)]),
            filter_sql_outcomes: BTreeMap::new(),
            main_schema: SchemaDoc {
                node_types: vec![NodeTypeSpec {
                    name: String::from("Warehouse"),
                    display_name: String::from("仓库"),
                    properties: vec![PropertySpec {
                        name: String::from("inventory"),
                        display_name: String::from("库存"),
                        data_type: DataType::Float64,
                        kind: PropertyKind::Metric,
                        pii_category: PiiCategory::None,
                        ..PropertySpec::default()
                    }],
                    ..NodeTypeSpec::default()
                }],
                edge_types: Vec::new(),
                mappings: Vec::new(),
            },
            populated_node_types: BTreeSet::from([String::from("Warehouse")]),
            assets: BTreeMap::from([(
                asset_id,
                AssetPreflightState {
                    asset_kind: AssetKind::Model3d,
                    binding_count: 1,
                },
            )]),
            scenarios: BTreeMap::from([
                (
                    scenarios_pair.0,
                    ScenarioPreflightState {
                        status: ScenarioStatus::Succeeded,
                        is_stale: false,
                    },
                ),
                (
                    scenarios_pair.1,
                    ScenarioPreflightState {
                        status: ScenarioStatus::Succeeded,
                        is_stale: false,
                    },
                ),
            ]),
            flows: BTreeMap::new(),
            nodes: BTreeMap::from([(
                wh_node_id,
                NodePreflightState {
                    type_name: String::from("Warehouse"),
                    authorized: true,
                },
            )]),
            caller_has_flow_run: true,
        }
    }

    #[test]
    fn step11_supply_overview_widgets_produce_zero_blocking_issues() {
        let now = Utc::now();
        let dash_id = Uuid::new_v4();
        let ds_id = Uuid::new_v4();
        let asset_id = Uuid::new_v4();
        let wh_node_id = Uuid::new_v4();
        let s1_id = Uuid::new_v4();
        let s2_id = Uuid::new_v4();

        let widgets = seeded_supply_overview_widgets(dash_id, asset_id, wh_node_id, s1_id, s2_id);
        let sql_wid = widgets.get(1).map_or_else(Uuid::nil, |w| w.id);
        let detail = DashboardDetail {
            dashboard: DashboardRecord {
                id: dash_id,
                tenant_id: String::from("default"),
                name: String::from("供应链全景驾驶舱"),
                slug: String::from("supply-overview"),
                description: String::new(),
                filters: Vec::new(),
                theme: DashboardTheme::Light,
                canvas: CanvasConfig::default(),
                status: DashboardStatus::Draft,
                version: 1,
                created_by: None,
                created_at: now,
                updated_at: now,
            },
            widgets,
        };

        let ctx = build_step11_test_context(ds_id, sql_wid, asset_id, wh_node_id, (s1_id, s2_id));
        let report = evaluate_dashboard_preflight(&detail, &ctx);
        assert_eq!(report.status, DashboardPreflightStatus::Pass);
        assert_eq!(report.blocking_count, 0);
        assert_eq!(report.warning_count, 0);
        assert!(report.ensure_publishable().is_ok());
    }
}
