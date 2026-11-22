//! AI one-sentence `xBlueView` dashboard generator with schema/SQL validation and 1-round self-repair (`10-STEP11-12` §11.6).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use axpoint_analytics::{CatalogDatasetEntry, GuardOptions, TenantCatalog, compile_pivot, guard};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{MetricDef, PivotSpec, SavedQuery};
use axpoint_domain::axhub::{Dataset, DatasetListQuery, DatasetStatus};
use axpoint_domain::enums::{AssetKind, Environment, Purpose, WidgetKind};
use axpoint_domain::flow::FlowListQuery;
use axpoint_domain::ports::{
    AiPort, AnalyticsRepository, DatasetRepository, FlowRepository, StudioRepository,
    ViewRepository,
};
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::studio::{DraftStatus, NodeTypeSpec, PropertyKind, SchemaDoc};
use axpoint_domain::topology::AxFlowDef;
use axpoint_domain::view::{
    AiDashboardDraft, AiDraftDashboardRequest, AiGenerateDashboardRequest, AiWidgetDraft,
    AssetRecord, CanvasConfig, CreateDashboardRequest, CreateWidgetRecordParams, DashboardDetail,
    DashboardStatus, RefreshMode, WidgetActionType, WidgetDataSource, WidgetPosition,
};
use axpoint_llm::LlmRouter;
use uuid::Uuid;

use super::deps::compute_widget_deps;

/// System prompt prefix used to route and match `xBlueView` AI dashboard generation cassettes.
pub const VIEW_AI_GEN_PURPOSE_TAG: &str = "PURPOSE:VIEW_AI_GENERATE";

/// Grounded tenant metadata context supplied to the LLM and validator during dashboard generation.
#[derive(Debug, Clone, Default)]
pub struct DashboardGenContext {
    /// Published `main` `xBlueStudio` schema document.
    pub schema: SchemaDoc,
    /// Configured semantic layer metrics (`an_metrics`).
    pub metrics: Vec<MetricDef>,
    /// Active `AxHub` datasets (`ds_datasets`).
    pub datasets: Vec<Dataset>,
    /// `TenantCatalog` hydrated for `sql_guard` and `compile_pivot` validation.
    pub catalog: TenantCatalog,
    /// Shared/accessible saved queries (`an_saved_queries`).
    pub saved_queries: Vec<SavedQuery>,
    /// Workflows with an active `PROD` deployment.
    pub prod_flows: Vec<AxFlowDef>,
    /// Uploaded visual/3D assets (`xv_assets`).
    pub assets: Vec<AssetRecord>,
}

/// Validates 24-column grid coordinates (`0 <= x`, `w >= 1`, `x + w <= 24`, `y >= 0`, `h >= 1`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if any bound is violated.
pub fn validate_widget_position(pos: &WidgetPosition) -> Result<(), AppError> {
    if pos.x < 0 || pos.w < 1 || pos.x.saturating_add(pos.w) > 24 || pos.y < 0 || pos.h < 1 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "widget position {{x:{}, y:{}, w:{}, h:{}}} exceeds 24-column grid bounds",
                pos.x, pos.y, pos.w, pos.h
            ),
        ));
    }
    Ok(())
}

/// Validates a URL-safe dashboard slug (`^[a-z0-9][a-z0-9-]{0,62}$`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `slug` does not match the constraint.
pub fn validate_dashboard_slug(slug: &str) -> Result<(), AppError> {
    if !is_valid_slug(slug) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid dashboard slug '{slug}': expected ^[a-z0-9][a-z0-9-]{{0,62}}$"),
        ));
    }
    Ok(())
}

fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    let Some(first) = bytes.first() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Collects `SchemaDoc`, `an_metrics`, `Ready` `ds_datasets`, shared `SavedQuery` items,
/// `PROD` flows, and `xv_assets` for `tenant_id`.
///
/// # Errors
/// Propagates storage errors from any domain repository.
pub async fn collect_dashboard_gen_context<S>(
    store: &S,
    tenant_id: &str,
) -> Result<DashboardGenContext, AppError>
where
    S: StudioRepository
        + AnalyticsRepository
        + DatasetRepository
        + FlowRepository
        + ViewRepository
        + ?Sized,
{
    collect_dashboard_gen_context_for_user(store, tenant_id, Uuid::nil()).await
}

/// Collects grounded dashboard generation context scoped to `viewer_user_id` (`include_all = false`).
///
/// # Errors
/// Propagates storage errors from any domain repository.
pub async fn collect_dashboard_gen_context_for_user<S>(
    store: &S,
    tenant_id: &str,
    viewer_user_id: Uuid,
) -> Result<DashboardGenContext, AppError>
where
    S: StudioRepository
        + AnalyticsRepository
        + DatasetRepository
        + FlowRepository
        + ViewRepository
        + ?Sized,
{
    let published = store.get_published_schema(tenant_id, "main").await?;
    let metrics = store.list_metrics(tenant_id).await?;
    let saved_queries = store
        .list_saved_queries(tenant_id, viewer_user_id, false)
        .await?;

    let mut datasets = Vec::new();
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
        datasets.extend(
            page.items
                .into_iter()
                .filter(|ds| ds.status == DatasetStatus::Ready),
        );
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }

    let mut catalog = TenantCatalog::new(tenant_id);
    for ds in &datasets {
        let cols = store.list_columns(tenant_id, ds.id).await?;
        catalog.insert_dataset(CatalogDatasetEntry::from_dataset(ds, &cols));
    }

    let prod_flows = load_prod_flows(store, tenant_id).await?;
    let assets_page = store
        .list_assets(
            tenant_id,
            None,
            SimplePageQuery {
                limit: Some(500),
                cursor: None,
            },
        )
        .await?;

    Ok(DashboardGenContext {
        schema: published.doc,
        metrics,
        datasets,
        catalog,
        saved_queries,
        prod_flows,
        assets: assets_page.items,
    })
}

async fn load_prod_flows<S: FlowRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
) -> Result<Vec<AxFlowDef>, AppError> {
    let flows_page = store
        .list_flows(
            tenant_id,
            FlowListQuery {
                q: None,
                limit: Some(200),
                cursor: None,
            },
        )
        .await?;
    let mut prod_flows = Vec::new();
    for flow in flows_page.items {
        if store
            .get_flow_deployment(tenant_id, flow.id, Environment::Prod)
            .await?
            .is_some()
        {
            prod_flows.push(flow);
        }
    }
    Ok(prod_flows)
}

/// Builds `(system_prompt, user_prompt)` for `Purpose::Extract` dashboard generation.
#[must_use]
pub fn build_dashboard_gen_prompts(
    ctx: &DashboardGenContext,
    req: &AiGenerateDashboardRequest,
) -> (String, String) {
    let sys = format!(
        "{VIEW_AI_GEN_PURPOSE_TAG}\n\
You design xBlueView digital-twin monitoring dashboards. Output ONLY valid JSON matching:\n\
{{\"name\":\"...\",\"slug\":\"lowercase-slug\",\"theme\":\"light|dark|screen\",\"description\":\"...\",\"filters\":[],\"widgets\":[{{\"widget_kind\":\"KPI_CARD|TABLE|PIVOT_TABLE|LINE_CHART|BAR_CHART|PIE_CHART|SCATTER|SANKEY|HEATMAP|GAUGE|RADAR|GRAPH_TOPOLOGY|MAP|FLOOR_PLAN|SCENE_3D|ALERT_LIST|SIM_COMPARE|ACTION_BUTTON|ACTION_FORM|MARKDOWN_NOTE\",\"title\":\"...\",\"position\":{{\"x\":0,\"y\":0,\"w\":6,\"h\":4}},\"data_source\":{{\"kind\":\"...\"}},\"encoding\":{{}},\"options\":{{}},\"drill\":[]}}]}}\n\
Valid widget data_source shapes (\"kind\" discriminator is REQUIRED):\n\
- {{\"kind\":\"METRIC\",\"metric\":\"<metric_name>\",\"dimensions\":[]}}\n\
- {{\"kind\":\"SQL\",\"sql\":\"SELECT ... FROM <dataset_name>\"}}\n\
- {{\"kind\":\"NODE_METRIC\",\"type_name\":\"<NodeType>\",\"metric\":\"<node_metric>\",\"agg\":\"SUM|AVG|MIN|MAX|LAST|COUNT\",\"grain\":\"MINUTE|HOUR|DAY|MONTH\",\"time_range\":\"LAST_7_DAYS|LAST_30_DAYS\",\"group_by\":\"node|time\"}}\n\
- {{\"kind\":\"ALERTS\",\"status\":[\"OPEN\",\"ACK\"],\"severity\":[\"WARN\",\"CRITICAL\"],\"limit\":20}}\n\
- {{\"kind\":\"NODES_GEO\",\"type_names\":[\"<NodeType>\"],\"color_metric\":\"<node_metric>\",\"link_types\":[\"<EdgeType>\"]}}\n\
- {{\"kind\":\"GRAPH\",\"type_names\":[\"<NodeType>\"],\"link_types\":[\"<EdgeType>\"],\"color_metric\":\"<node_metric>\",\"limit\":500}}\n\
- {{\"kind\":\"SCENE_3D\",\"asset_id\":\"<model_3d_uuid>\",\"color_metric\":\"<node_metric>\"}}\n\
- {{\"kind\":\"FLOOR_PLAN\",\"asset_id\":\"<floor_plan_uuid>\",\"color_metric\":\"<node_metric>\"}}\n\
- {{\"kind\":\"ACTION\",\"action_type\":\"RUN_FLOW\",\"flow_id\":\"<prod_flow_uuid>\"}}\n\
- {{\"kind\":\"STATIC\",\"columns\":[],\"rows\":[]}}\n\
Rules:\n\
1. 24-column grid: x >= 0, w >= 1, x + w <= 24, y >= 0, h >= 1.\n\
2. SQL data_source MUST query ONLY tables listed under datasets: (never query node_types: in SQL; query node_types: via NODE_METRIC, GRAPH, or NODES_GEO).\n\
3. Use ONLY existing metrics, datasets/columns, node_types, prod_flows, and assets listed below.\n\
4. slug must match ^[a-z0-9][a-z0-9-]{{0,62}}$."
    );

    let mut usr = String::new();
    let _w = writeln!(usr, "prompt: {}", req.prompt.trim());
    if let Some(ref title) = req.title {
        let _w = writeln!(usr, "title_override: {}", title.trim());
    }
    if let Some(ref slug) = req.slug {
        let _w = writeln!(usr, "slug_override: {}", slug.trim());
    }
    if let Some(theme) = req.theme {
        let _w = writeln!(usr, "theme_override: {}", theme.as_str());
    }

    append_context_summary(&mut usr, ctx);
    (sys, usr)
}

fn append_context_summary(out: &mut String, ctx: &DashboardGenContext) {
    let _w = writeln!(out, "\nnode_types:");
    for nt in &ctx.schema.node_types {
        let props: Vec<&str> = nt
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Attribute)
            .map(|p| p.name.as_str())
            .collect();
        let metrics = collect_node_metric_names(nt);
        let _w = writeln!(
            out,
            "- {} ({}) attrs=[{}] metrics=[{}]",
            nt.name,
            nt.display_name,
            props.join(","),
            metrics.into_iter().collect::<Vec<_>>().join(",")
        );
    }

    let _w = writeln!(out, "\nedge_types:");
    for et in &ctx.schema.edge_types {
        let _w = writeln!(
            out,
            "- {} ({} -> {})",
            et.name, et.src_type_name, et.dst_type_name
        );
    }

    let _w = writeln!(out, "\nmetrics:");
    for m in &ctx.metrics {
        let dims: Vec<&str> = m.dimensions.iter().map(|d| d.name.as_str()).collect();
        let _w = writeln!(
            out,
            "- {} ({}, expr={}, dims=[{}])",
            m.name,
            m.display_name,
            m.expression,
            dims.join(",")
        );
    }

    let _w = writeln!(out, "\ndatasets:");
    for ds in &ctx.datasets {
        if ds.status != DatasetStatus::Ready {
            continue;
        }
        let cols = resolve_dataset_column_names(ds, &ctx.catalog);
        let _w = writeln!(out, "- {} cols=[{}]", ds.name, cols.join(","));
    }

    if !ctx.prod_flows.is_empty() {
        let _w = writeln!(out, "\nprod_flows:");
        for fl in &ctx.prod_flows {
            let _w = writeln!(out, "- {} ({})", fl.id, fl.name);
        }
    }

    if !ctx.assets.is_empty() {
        let _w = writeln!(out, "\nassets:");
        for a in &ctx.assets {
            let _w = writeln!(out, "- {} ({:?}, {})", a.id, a.asset_kind, a.name);
        }
    }
}

fn resolve_dataset_column_names(ds: &Dataset, catalog: &TenantCatalog) -> Vec<String> {
    if let Some(entry) = catalog.datasets.get(&ds.name.to_ascii_lowercase())
        && !entry.columns.is_empty()
    {
        let mut ordered: Vec<(i32, String)> = entry
            .columns
            .values()
            .map(|c| (c.ordinal, c.column_name.clone()))
            .collect();
        ordered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        return ordered.into_iter().map(|(_, name)| name).collect();
    }
    ds.schema_json.iter().map(|c| c.name.clone()).collect()
}

fn collect_node_metric_names(nt: &NodeTypeSpec) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for p in &nt.properties {
        if p.kind == PropertyKind::Metric {
            set.insert(p.name.clone());
        }
    }
    for rule in &nt.agg_rules {
        set.insert(rule.target_metric.clone());
    }
    for rule in &nt.derived_rules {
        set.insert(rule.target_metric.clone());
    }
    set
}

/// Leniently parses an [`AiDashboardDraft`] JSON payload, stripping markdown code fences if present.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`PARSE_ERROR`) if no valid draft JSON object can be decoded.
pub fn parse_ai_dashboard_draft(raw: &str) -> Result<AiDashboardDraft, AppError> {
    let trimmed = raw.trim();
    let direct_err = match decode_normalized_draft(trimmed) {
        Ok(draft) => return Ok(draft),
        Err(err) => err,
    };
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Some(slice) = trimmed.get(start..=end)
    {
        match decode_normalized_draft(slice) {
            Ok(draft) => return Ok(draft),
            Err(slice_err) => {
                return Err(AppError::bad(
                    "PARSE_ERROR",
                    format!("failed to decode AiDashboardDraft JSON: {slice_err}"),
                ));
            }
        }
    }
    Err(AppError::bad(
        "PARSE_ERROR",
        format!("failed to decode AiDashboardDraft JSON: {direct_err}"),
    ))
}

fn decode_normalized_draft(raw: &str) -> Result<AiDashboardDraft, serde_json::Error> {
    let mut val: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(widgets) = val
        .get_mut("widgets")
        .and_then(serde_json::Value::as_array_mut)
    {
        for w in widgets {
            if let Some(ds) = w
                .get_mut("data_source")
                .and_then(serde_json::Value::as_object_mut)
            {
                normalize_widget_data_source(ds);
            }
        }
    }
    serde_json::from_value(val)
}

fn canonical_data_source_kind(raw_kind: &str) -> String {
    let upper = raw_kind.trim().to_ascii_uppercase().replace('-', "_");
    match upper.as_str() {
        "SCENE3D" => String::from("SCENE_3D"),
        "FLOORPLAN" => String::from("FLOOR_PLAN"),
        "NODEMETRIC" => String::from("NODE_METRIC"),
        "NODESGEO" => String::from("NODES_GEO"),
        "SAVEDQUERY" => String::from("SAVED_QUERY"),
        "SIMSCENARIOS" | "SIM_COMPARE" => String::from("SIM_SCENARIOS"),
        _ => upper,
    }
}

fn normalize_widget_data_source(ds: &mut serde_json::Map<String, serde_json::Value>) {
    if !ds.contains_key("kind")
        && let Some(type_str) = ds
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(canonical_data_source_kind)
    {
        ds.remove("type");
        ds.insert(String::from("kind"), serde_json::Value::String(type_str));
    } else if let Some(kind_val) = ds.get_mut("kind")
        && let Some(existing) = kind_val.as_str()
    {
        *kind_val = serde_json::Value::String(canonical_data_source_kind(existing));
    }

    let kind = ds
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match kind {
        "SQL" => promote_alias_field(ds, "sql", &["query", "statement"]),
        "METRIC" => promote_alias_field(ds, "metric", &["metric_name", "name"]),
        "NODE_METRIC" => {
            promote_alias_field(ds, "type_name", &["node_type"]);
            promote_alias_field(ds, "metric", &["metric_name", "name"]);
        }
        _ => {}
    }
}

fn promote_alias_field(
    ds: &mut serde_json::Map<String, serde_json::Value>,
    canonical: &str,
    aliases: &[&str],
) {
    if ds.contains_key(canonical) {
        return;
    }
    for &alias in aliases {
        if let Some(val) = ds.remove(alias) {
            ds.insert(String::from(canonical), val);
            return;
        }
    }
}

/// Validates an [`AiDashboardDraft`] against 24-column bounds, schema bindings, `PROD` flows,
/// visual assets, and `sql_guard` `SQL` rules. Returns an empty vector when valid.
#[must_use]
pub fn validate_ai_dashboard_draft(
    draft: &AiDashboardDraft,
    ctx: &DashboardGenContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    if draft.name.trim().is_empty() {
        errors.push(String::from("dashboard name cannot be empty"));
    }
    if !is_valid_slug(&draft.slug) {
        errors.push(format!(
            "dashboard slug '{}' must match ^[a-z0-9][a-z0-9-]{{0,62}}$",
            draft.slug
        ));
    }
    if draft.widgets.is_empty() {
        errors.push(String::from("dashboard must contain at least 1 widget"));
        return errors;
    }

    let node_type_map: HashMap<&str, &NodeTypeSpec> = ctx
        .schema
        .node_types
        .iter()
        .map(|nt| (nt.name.as_str(), nt))
        .collect();
    let edge_type_set: BTreeSet<&str> = ctx
        .schema
        .edge_types
        .iter()
        .map(|et| et.name.as_str())
        .collect();

    for (idx, w) in draft.widgets.iter().enumerate() {
        if w.title.trim().is_empty() {
            errors.push(format!("widget[{idx}] title cannot be empty"));
        }
        if let Err(err) = validate_widget_position(&w.position) {
            errors.push(format!("widget[{idx}] {}", err.client_message()));
        }
        validate_widget_source(
            idx,
            &w.data_source,
            ctx,
            &node_type_map,
            &edge_type_set,
            &mut errors,
        );
    }
    errors
}

fn validate_widget_source(
    idx: usize,
    ds: &WidgetDataSource,
    ctx: &DashboardGenContext,
    node_types: &HashMap<&str, &NodeTypeSpec>,
    edge_types: &BTreeSet<&str>,
    errors: &mut Vec<String>,
) {
    match ds {
        WidgetDataSource::Sql { sql } => {
            if let Err(err) = guard(sql, &ctx.catalog, &GuardOptions::default()) {
                errors.push(format!(
                    "widget[{idx}] SQL rejected: {}",
                    err.client_message()
                ));
            }
        }
        WidgetDataSource::SavedQuery { saved_query_id } => {
            if !ctx.saved_queries.iter().any(|sq| sq.id == *saved_query_id) {
                errors.push(format!(
                    "widget[{idx}] unknown saved_query_id '{saved_query_id}'"
                ));
            }
        }
        WidgetDataSource::Metric { metric, .. } => {
            if !ctx.metrics.iter().any(|m| m.name == *metric) {
                errors.push(format!("widget[{idx}] unknown metric '{metric}'"));
            }
        }
        WidgetDataSource::Pivot { spec } => match serde_json::from_value::<PivotSpec>(spec.clone())
        {
            Ok(pivot) => {
                if let Err(err) = compile_pivot(&pivot, &ctx.catalog) {
                    errors.push(format!(
                        "widget[{idx}] invalid pivot spec: {}",
                        err.client_message()
                    ));
                }
            }
            Err(err) => errors.push(format!("widget[{idx}] invalid pivot JSON: {err}")),
        },
        WidgetDataSource::NodeMetric {
            type_name, metric, ..
        } => {
            let Some(nt) = node_types.get(type_name.as_str()) else {
                errors.push(format!("widget[{idx}] unknown node type '{type_name}'"));
                return;
            };
            if !collect_node_metric_names(nt).contains(metric.as_str()) {
                errors.push(format!(
                    "widget[{idx}] unknown metric '{metric}' on node type '{type_name}'"
                ));
            }
        }
        WidgetDataSource::Graph {
            type_names,
            link_types,
            color_metric,
            ..
        }
        | WidgetDataSource::NodesGeo {
            type_names,
            color_metric,
            link_types,
        } => validate_graph_binding(
            idx,
            type_names,
            link_types,
            color_metric.as_deref(),
            node_types,
            edge_types,
            errors,
        ),
        WidgetDataSource::FloorPlan { asset_id, .. } => {
            if !ctx
                .assets
                .iter()
                .any(|a| a.id == *asset_id && a.asset_kind == AssetKind::FloorPlanImage)
            {
                errors.push(format!(
                    "widget[{idx}] missing FLOOR_PLAN_IMAGE asset '{asset_id}'"
                ));
            }
        }
        WidgetDataSource::Scene3d { asset_id, .. } => {
            if !ctx
                .assets
                .iter()
                .any(|a| a.id == *asset_id && a.asset_kind == AssetKind::Model3d)
            {
                errors.push(format!("widget[{idx}] missing MODEL_3D asset '{asset_id}'"));
            }
        }
        WidgetDataSource::SimScenarios { scenario_ids, .. } => {
            if scenario_ids.len() < 2 {
                errors.push(format!(
                    "widget[{idx}] SIM_SCENARIOS requires at least 2 scenario_ids"
                ));
            }
        }
        WidgetDataSource::Action {
            action_type,
            flow_id,
            key,
            ..
        } => validate_action_binding(idx, *action_type, *flow_id, key.as_deref(), ctx, errors),
        WidgetDataSource::Alerts { .. } | WidgetDataSource::Static { .. } => {}
    }
}

fn validate_action_binding(
    idx: usize,
    action_type: WidgetActionType,
    flow_id: Option<Uuid>,
    key: Option<&str>,
    ctx: &DashboardGenContext,
    errors: &mut Vec<String>,
) {
    match action_type {
        WidgetActionType::RunFlow => {
            let valid = flow_id.is_some_and(|fid| ctx.prod_flows.iter().any(|fl| fl.id == fid));
            if !valid {
                errors.push(format!(
                    "widget[{idx}] RUN_FLOW references missing or non-PROD flow"
                ));
            }
        }
        WidgetActionType::WriteNode => {
            if key.is_none_or(|k| k.trim().is_empty()) {
                errors.push(format!("widget[{idx}] WRITE_NODE requires non-empty key"));
            }
        }
    }
}

fn validate_graph_binding(
    idx: usize,
    type_names: &[String],
    link_types: &[String],
    color_metric: Option<&str>,
    node_types: &HashMap<&str, &NodeTypeSpec>,
    edge_types: &BTreeSet<&str>,
    errors: &mut Vec<String>,
) {
    for tn in type_names {
        if !node_types.contains_key(tn.as_str()) {
            errors.push(format!("widget[{idx}] unknown node type '{tn}'"));
        }
    }
    for lt in link_types {
        if !edge_types.contains(lt.as_str()) {
            errors.push(format!("widget[{idx}] unknown edge type '{lt}'"));
        }
    }
    if let Some(cm) = color_metric.map(str::trim).filter(|s| !s.is_empty()) {
        let candidate_types: Vec<&&NodeTypeSpec> = if type_names.is_empty() {
            node_types.values().collect()
        } else {
            type_names
                .iter()
                .filter_map(|tn| node_types.get(tn.as_str()))
                .collect()
        };
        let found = candidate_types
            .iter()
            .any(|nt| collect_node_metric_names(nt).contains(cm));
        if !found {
            errors.push(format!(
                "widget[{idx}] color_metric '{cm}' not found on referenced node types"
            ));
        }
    }
}

/// Generates and validates an [`AiDashboardDraft`] via [`AiPort`], performing 1 self-repair pass on error.
///
/// # Errors
/// Returns [`AppError::Llm`] on model invocation failures or [`AppError::Unprocessable`] if
/// validation still fails after 1 repair round.
pub async fn draft_dashboard_with_ai<A: AiPort>(
    ai: &A,
    ctx: &DashboardGenContext,
    req: &AiGenerateDashboardRequest,
) -> Result<AiDashboardDraft, AppError> {
    let (sys, usr) = build_dashboard_gen_prompts(ctx, req);
    let raw = ai.chat(&sys, &usr).await?;
    match parse_and_validate(&raw, ctx) {
        Ok(draft) => Ok(draft),
        Err((repair_usr, _)) => {
            let repaired_raw = ai.chat(&sys, &repair_usr).await?;
            match parse_and_validate(&repaired_raw, ctx) {
                Ok(draft) => Ok(draft),
                Err((_, errs)) => Err(AppError::unprocessable(
                    "AI_GEN_INVALID",
                    format!(
                        "AI dashboard draft invalid after self-repair: {}",
                        errs.join("; ")
                    ),
                )),
            }
        }
    }
}

/// Generates, validates (with 1 self-repair pass via `LlmRouter(Purpose::Extract)`), and persists
/// a `DRAFT` dashboard in PostgreSQL (`POST /api/xblueview/dashboards/ai-generate`).
///
/// # Errors
/// Propagates context collection, `LlmRouter` (`Purpose::Extract`), validation, or persistence errors.
pub async fn generate_ai_dashboard<S>(
    store: &S,
    llm_router: &LlmRouter,
    tenant_id: &str,
    caller_user_id: Option<Uuid>,
    created_by: Option<Uuid>,
    req: AiGenerateDashboardRequest,
) -> Result<DashboardDetail, AppError>
where
    S: StudioRepository
        + AnalyticsRepository
        + DatasetRepository
        + FlowRepository
        + ViewRepository
        + ?Sized,
{
    if req.prompt.trim().is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "prompt cannot be empty"));
    }
    if let Some(ref explicit_slug) = req.slug {
        validate_dashboard_slug(explicit_slug.trim())?;
    }

    let ctx = collect_dashboard_gen_context_for_user(
        store,
        tenant_id,
        caller_user_id.or(created_by).unwrap_or_else(Uuid::nil),
    )
    .await?;
    let (sys, usr) = build_dashboard_gen_prompts(&ctx, &req);

    let raw = llm_router
        .complete_chat_for_user(tenant_id, caller_user_id, Purpose::Extract, &sys, &usr)
        .await?;
    let mut draft = match parse_and_validate(&raw, &ctx) {
        Ok(d) => d,
        Err((repair_usr, _)) => {
            let repaired_raw = llm_router
                .complete_chat_for_user(
                    tenant_id,
                    caller_user_id,
                    Purpose::Extract,
                    &sys,
                    &repair_usr,
                )
                .await?;
            match parse_and_validate(&repaired_raw, &ctx) {
                Ok(d) => d,
                Err((_, errs)) => {
                    return Err(AppError::unprocessable(
                        "AI_GEN_INVALID",
                        format!(
                            "AI dashboard draft invalid after self-repair: {}",
                            errs.join("; ")
                        ),
                    ));
                }
            }
        }
    };

    if let Some(ref title_override) = req.title
        && !title_override.trim().is_empty()
    {
        draft.name = title_override.trim().to_owned();
    }
    if let Some(theme_override) = req.theme {
        draft.theme = theme_override;
    }

    let final_slug = resolve_final_slug(store, tenant_id, req.slug.as_deref(), &draft.slug).await?;

    let mut widget_params = Vec::with_capacity(draft.widgets.len());
    for w in draft.widgets {
        let deps = compute_widget_deps(store, tenant_id, &w.data_source).await?;
        widget_params.push(CreateWidgetRecordParams {
            dashboard_id: Uuid::nil(),
            widget_kind: w.widget_kind,
            title: w.title,
            position: w.position,
            data_source: w.data_source,
            encoding: w.encoding,
            options: w.options,
            drill: w.drill,
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
            deps,
        });
    }

    let create_req = CreateDashboardRequest {
        name: draft.name,
        slug: final_slug,
        description: draft.description,
        filters: draft.filters,
        theme: draft.theme,
        canvas: CanvasConfig::default(),
        zone_layout: None,
    };

    store
        .create_ai_dashboard(tenant_id, created_by, create_req, widget_params)
        .await
}

/// Generates (or deterministically synthesizes on offline cassette miss) a Studio-contextual `DRAFT`
/// twin dashboard seeded with `subgraph_slug` and `seed_node_types` (`POST /api/xblueview/dashboards/ai-draft`, `05-STEP19` §19.3).
///
/// # Errors
/// Returns [`AppError::BadRequest`] on empty `name`/`prompt` or invalid `slug`,
/// [`AppError::Conflict`] if an explicit `slug` collides with an already `PUBLISHED` dashboard,
/// or storage errors from underlying repositories.
pub async fn generate_studio_ai_dashboard_draft<S>(
    store: &S,
    llm_router: &LlmRouter,
    tenant_id: &str,
    caller_user_id: Option<Uuid>,
    created_by: Option<Uuid>,
    req: AiDraftDashboardRequest,
) -> Result<DashboardDetail, AppError>
where
    S: StudioRepository
        + AnalyticsRepository
        + DatasetRepository
        + FlowRepository
        + ViewRepository
        + ?Sized,
{
    let trimmed_name = req.name.trim();
    if trimmed_name.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "dashboard name cannot be empty",
        ));
    }
    if req.prompt.trim().is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "prompt cannot be empty"));
    }
    if let Some(ref explicit_slug) = req.slug {
        validate_dashboard_slug(explicit_slug.trim())?;
    }

    let viewer_uid = caller_user_id.or(created_by).unwrap_or_else(Uuid::nil);
    let (ctx, seeded_types) =
        collect_studio_dashboard_context(store, tenant_id, viewer_uid, &req).await?;

    let mut draft = match try_llm_studio_draft(
        llm_router,
        tenant_id,
        caller_user_id,
        &ctx,
        &req,
        &seeded_types,
    )
    .await
    {
        Some(llm_draft) => llm_draft,
        None => synthesize_studio_dashboard_draft(&ctx, &req, &seeded_types),
    };
    draft.name = String::from(trimmed_name);
    if let Some(theme_override) = req.theme {
        draft.theme = theme_override;
    }

    let final_slug =
        prepare_idempotent_draft_slug(store, tenant_id, req.slug.as_deref(), &draft.slug).await?;
    persist_draft_dashboard_records(store, tenant_id, created_by, final_slug, draft).await
}

async fn collect_studio_dashboard_context<S>(
    store: &S,
    tenant_id: &str,
    viewer_uid: Uuid,
    req: &AiDraftDashboardRequest,
) -> Result<(DashboardGenContext, Vec<String>), AppError>
where
    S: StudioRepository
        + AnalyticsRepository
        + DatasetRepository
        + FlowRepository
        + ViewRepository
        + ?Sized,
{
    let mut ctx = collect_dashboard_gen_context_for_user(store, tenant_id, viewer_uid).await?;
    let subgraph_slug = req
        .subgraph_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main");

    let mut sub_doc = store
        .get_published_schema(tenant_id, subgraph_slug)
        .await?
        .doc;
    if sub_doc.node_types.is_empty() {
        let drafts = store
            .list_drafts(tenant_id, subgraph_slug, Some(DraftStatus::Draft))
            .await?;
        if let Some(latest) = drafts.into_iter().next() {
            sub_doc = latest.schema_doc;
        }
    }
    ctx.schema = sub_doc;

    let mut seeded = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in &req.seed_node_types {
        let trimmed = raw.trim();
        if !trimmed.is_empty()
            && ctx.schema.node_types.iter().any(|nt| nt.name == trimmed)
            && seen.insert(String::from(trimmed))
        {
            seeded.push(String::from(trimmed));
        }
    }
    if seeded.is_empty() {
        seeded = ctx
            .schema
            .node_types
            .iter()
            .map(|nt| nt.name.clone())
            .collect();
    } else {
        ctx.schema
            .node_types
            .sort_by_key(|nt| usize::from(!seen.contains(&nt.name)));
    }

    Ok((ctx, seeded))
}

async fn try_llm_studio_draft(
    llm_router: &LlmRouter,
    tenant_id: &str,
    caller_user_id: Option<Uuid>,
    ctx: &DashboardGenContext,
    req: &AiDraftDashboardRequest,
    seeded_types: &[String],
) -> Option<AiDashboardDraft> {
    let gen_req = AiGenerateDashboardRequest {
        prompt: req.prompt.clone(),
        title: Some(req.name.clone()),
        slug: req.slug.clone(),
        theme: req.theme,
    };
    let (sys, mut usr) = build_dashboard_gen_prompts(ctx, &gen_req);
    if !seeded_types.is_empty() {
        let _w = writeln!(usr, "seed_node_types: {}", seeded_types.join(","));
    }

    let raw = llm_router
        .complete_chat_for_user(tenant_id, caller_user_id, Purpose::Extract, &sys, &usr)
        .await
        .ok()?;
    let candidate = match parse_and_validate(&raw, ctx) {
        Ok(d) => d,
        Err((repair_usr, _)) => {
            let repaired = llm_router
                .complete_chat_for_user(
                    tenant_id,
                    caller_user_id,
                    Purpose::Extract,
                    &sys,
                    &repair_usr,
                )
                .await
                .ok()?;
            parse_and_validate(&repaired, ctx).ok()?
        }
    };

    if candidate.widgets.is_empty() {
        return None;
    }
    if !req.seed_node_types.is_empty() && !draft_covers_seeded_types(&candidate, seeded_types) {
        return None;
    }
    Some(candidate)
}

fn draft_covers_seeded_types(draft: &AiDashboardDraft, seeded_types: &[String]) -> bool {
    let mut covered = BTreeSet::new();
    for w in &draft.widgets {
        match &w.data_source {
            WidgetDataSource::NodeMetric { type_name, .. } => {
                covered.insert(type_name.as_str());
            }
            WidgetDataSource::Graph { type_names, .. }
            | WidgetDataSource::NodesGeo { type_names, .. } => {
                for tn in type_names {
                    covered.insert(tn.as_str());
                }
            }
            _ => {}
        }
    }
    seeded_types.iter().all(|s| covered.contains(s.as_str()))
}

/// Deterministically synthesizes a grounded 4-widget `xBlueView` twin dashboard draft from
/// `ctx` and `seeded_types` when `LlmRouter` is offline or omits seeded entities (`05-STEP19` §19.3).
#[must_use]
pub fn synthesize_studio_dashboard_draft(
    ctx: &DashboardGenContext,
    req: &AiDraftDashboardRequest,
    seeded_types: &[String],
) -> AiDashboardDraft {
    let primary_nt = seeded_types
        .first()
        .and_then(|name| ctx.schema.node_types.iter().find(|nt| nt.name == *name))
        .or_else(|| ctx.schema.node_types.first());
    let primary_metrics: Vec<String> =
        primary_nt.map_or_default(|nt| collect_node_metric_names(nt).into_iter().collect());

    let kpi_widget = build_fallback_kpi_widget(ctx, primary_nt, primary_metrics.first());
    let bar_widget = build_fallback_bar_widget(
        ctx,
        primary_nt,
        primary_metrics.get(1).or_else(|| primary_metrics.first()),
    );
    let graph_widget = build_fallback_graph_widget(
        req.name.trim(),
        ctx,
        seeded_types,
        primary_metrics.first().map(String::as_str),
    );
    let alert_widget = AiWidgetDraft {
        widget_kind: WidgetKind::AlertList,
        title: String::from("实时监控与风险告警"),
        position: WidgetPosition {
            x: 14,
            y: 6,
            w: 10,
            h: 10,
        },
        data_source: WidgetDataSource::Alerts {
            status: vec![String::from("OPEN"), String::from("ACK")],
            severity: vec![String::from("WARN"), String::from("CRITICAL")],
            limit: Some(20),
        },
        encoding: serde_json::json!({}),
        options: serde_json::json!({}),
        drill: Vec::new(),
    };

    let fallback_slug = req
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| derive_fallback_cockpit_slug(seeded_types), String::from);

    AiDashboardDraft {
        name: req.name.trim().to_owned(),
        slug: fallback_slug,
        theme: req.theme.unwrap_or_default(),
        description: req.prompt.trim().to_owned(),
        filters: Vec::new(),
        widgets: vec![kpi_widget, bar_widget, graph_widget, alert_widget],
    }
}

fn build_fallback_kpi_widget(
    ctx: &DashboardGenContext,
    primary_nt: Option<&NodeTypeSpec>,
    metric_opt: Option<&String>,
) -> AiWidgetDraft {
    let pos = WidgetPosition {
        x: 0,
        y: 0,
        w: 8,
        h: 6,
    };
    if let (Some(nt), Some(m1)) = (primary_nt, metric_opt) {
        let label = if nt.display_name.trim().is_empty() {
            nt.name.as_str()
        } else {
            nt.display_name.trim()
        };
        return AiWidgetDraft {
            widget_kind: WidgetKind::KpiCard,
            title: format!("{label} · {m1}"),
            position: pos,
            data_source: WidgetDataSource::NodeMetric {
                type_name: nt.name.clone(),
                metric: m1.clone(),
                agg: Some(String::from("AVG")),
                grain: Some(String::from("DAY")),
                time_range: Some(String::from("LAST_30_DAYS")),
                group_by: None,
            },
            encoding: serde_json::json!({}),
            options: serde_json::json!({}),
            drill: Vec::new(),
        };
    }
    if let Some(sm) = ctx.metrics.first() {
        let title = if sm.display_name.trim().is_empty() {
            sm.name.clone()
        } else {
            sm.display_name.clone()
        };
        return AiWidgetDraft {
            widget_kind: WidgetKind::KpiCard,
            title,
            position: pos,
            data_source: WidgetDataSource::Metric {
                metric: sm.name.clone(),
                dimensions: Vec::new(),
                time_grain: None,
                time_range: Some(String::from("LAST_30_DAYS")),
            },
            encoding: serde_json::json!({}),
            options: serde_json::json!({}),
            drill: Vec::new(),
        };
    }
    AiWidgetDraft {
        widget_kind: WidgetKind::KpiCard,
        title: String::from("待处理告警概览"),
        position: pos,
        data_source: WidgetDataSource::Alerts {
            status: vec![String::from("OPEN")],
            severity: Vec::new(),
            limit: Some(10),
        },
        encoding: serde_json::json!({}),
        options: serde_json::json!({}),
        drill: Vec::new(),
    }
}

fn build_fallback_bar_widget(
    ctx: &DashboardGenContext,
    primary_nt: Option<&NodeTypeSpec>,
    metric_opt: Option<&String>,
) -> AiWidgetDraft {
    let pos = WidgetPosition {
        x: 8,
        y: 0,
        w: 16,
        h: 6,
    };
    if let (Some(nt), Some(m2)) = (primary_nt, metric_opt) {
        let label = if nt.display_name.trim().is_empty() {
            nt.name.as_str()
        } else {
            nt.display_name.trim()
        };
        return AiWidgetDraft {
            widget_kind: WidgetKind::BarChart,
            title: format!("{label} · {m2}分布"),
            position: pos,
            data_source: WidgetDataSource::NodeMetric {
                type_name: nt.name.clone(),
                metric: m2.clone(),
                agg: Some(String::from("LAST")),
                grain: Some(String::from("DAY")),
                time_range: Some(String::from("LAST_30_DAYS")),
                group_by: Some(String::from("node")),
            },
            encoding: serde_json::json!({ "x": "node_title", "y": "value" }),
            options: serde_json::json!({}),
            drill: Vec::new(),
        };
    }
    if let Some(sm) = ctx.metrics.first() {
        let dims = sm
            .dimensions
            .first()
            .map_or_default(|d| vec![d.name.clone()]);
        return AiWidgetDraft {
            widget_kind: WidgetKind::BarChart,
            title: format!("{}趋势对比", sm.name),
            position: pos,
            data_source: WidgetDataSource::Metric {
                metric: sm.name.clone(),
                dimensions: dims,
                time_grain: Some(String::from("DAY")),
                time_range: Some(String::from("LAST_30_DAYS")),
            },
            encoding: serde_json::json!({}),
            options: serde_json::json!({}),
            drill: Vec::new(),
        };
    }
    AiWidgetDraft {
        widget_kind: WidgetKind::BarChart,
        title: String::from("风险态势分布"),
        position: pos,
        data_source: WidgetDataSource::Alerts {
            status: vec![String::from("OPEN"), String::from("ACK")],
            severity: Vec::new(),
            limit: Some(20),
        },
        encoding: serde_json::json!({}),
        options: serde_json::json!({}),
        drill: Vec::new(),
    }
}

fn build_fallback_graph_widget(
    dash_name: &str,
    ctx: &DashboardGenContext,
    seeded_types: &[String],
    color_metric: Option<&str>,
) -> AiWidgetDraft {
    let type_set: BTreeSet<&str> = seeded_types.iter().map(String::as_str).collect();
    let link_types: Vec<String> = ctx
        .schema
        .edge_types
        .iter()
        .filter(|et| {
            type_set.contains(et.src_type_name.as_str())
                && type_set.contains(et.dst_type_name.as_str())
        })
        .map(|et| et.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    AiWidgetDraft {
        widget_kind: WidgetKind::GraphTopology,
        title: format!("{dash_name} 业务拓扑"),
        position: WidgetPosition {
            x: 0,
            y: 6,
            w: 14,
            h: 10,
        },
        data_source: WidgetDataSource::Graph {
            type_names: seeded_types.to_vec(),
            link_types,
            color_metric: color_metric.map(String::from),
            limit: Some(200),
        },
        encoding: serde_json::json!({}),
        options: serde_json::json!({}),
        drill: Vec::new(),
    }
}

fn derive_fallback_cockpit_slug(seeded_types: &[String]) -> String {
    if let Some(first) = seeded_types.first() {
        let mut slug = String::new();
        for ch in first.chars() {
            if ch.is_ascii_alphanumeric() {
                if ch.is_ascii_uppercase() && !slug.is_empty() && !slug.ends_with('-') {
                    slug.push('-');
                }
                slug.push(ch.to_ascii_lowercase());
            }
        }
        let clean = slug.trim_matches('-');
        if !clean.is_empty() {
            let candidate = format!("{clean}-cockpit");
            if is_valid_slug(&candidate) {
                return candidate;
            }
        }
    }
    String::from("studio-twin-cockpit")
}

async fn prepare_idempotent_draft_slug<S: ViewRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
    explicit_slug: Option<&str>,
    draft_slug: &str,
) -> Result<String, AppError> {
    if let Some(s) = explicit_slug.map(str::trim).filter(|s| !s.is_empty()) {
        validate_dashboard_slug(s)?;
        if let Some(existing) = store.get_dashboard_by_slug(tenant_id, s, false).await? {
            if existing.dashboard.status != DashboardStatus::Draft {
                return Err(AppError::Conflict("SLUG_TAKEN"));
            }
            let _deleted = store
                .delete_dashboard(tenant_id, existing.dashboard.id)
                .await?;
        }
        return Ok(String::from(s));
    }
    resolve_final_slug(store, tenant_id, None, draft_slug).await
}

async fn persist_draft_dashboard_records<S>(
    store: &S,
    tenant_id: &str,
    created_by: Option<Uuid>,
    final_slug: String,
    draft: AiDashboardDraft,
) -> Result<DashboardDetail, AppError>
where
    S: StudioRepository + AnalyticsRepository + DatasetRepository + ViewRepository + ?Sized,
{
    let mut widget_params = Vec::with_capacity(draft.widgets.len());
    for w in draft.widgets {
        let deps = compute_widget_deps(store, tenant_id, &w.data_source).await?;
        widget_params.push(CreateWidgetRecordParams {
            dashboard_id: Uuid::nil(),
            widget_kind: w.widget_kind,
            title: w.title,
            position: w.position,
            data_source: w.data_source,
            encoding: w.encoding,
            options: w.options,
            drill: w.drill,
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
            deps,
        });
    }

    let create_req = CreateDashboardRequest {
        name: draft.name,
        slug: final_slug,
        description: draft.description,
        filters: draft.filters,
        theme: draft.theme,
        canvas: CanvasConfig::default(),
        zone_layout: None,
    };

    store
        .create_ai_dashboard(tenant_id, created_by, create_req, widget_params)
        .await
}

fn parse_and_validate(
    raw: &str,
    ctx: &DashboardGenContext,
) -> Result<AiDashboardDraft, (String, Vec<String>)> {
    match parse_ai_dashboard_draft(raw) {
        Ok(draft) => {
            let errors = validate_ai_dashboard_draft(&draft, ctx);
            if errors.is_empty() {
                Ok(draft)
            } else {
                let repair = build_repair_prompt(raw, &errors);
                Err((repair, errors))
            }
        }
        Err(err) => {
            let errs = vec![err.client_message()];
            let repair = build_repair_prompt(raw, &errs);
            Err((repair, errs))
        }
    }
}

fn build_repair_prompt(previous_raw: &str, errors: &[String]) -> String {
    let mut out = String::from(
        "The previous AiDashboardDraft JSON failed validation. Fix every issue listed below and output ONLY valid JSON.\n\
Remember: every widget data_source MUST specify \"kind\" (e.g. {\"kind\":\"METRIC\",\"metric\":\"...\"}, {\"kind\":\"SQL\",\"sql\":\"SELECT ... FROM <dataset>\"}, {\"kind\":\"NODE_METRIC\",\"type_name\":\"<NodeType>\",\"metric\":\"<metric>\"}), and SQL queries ONLY datasets: tables (never node_types:).\nErrors:\n",
    );
    for (idx, err) in errors.iter().enumerate() {
        let num = idx.saturating_add(1);
        let _w = writeln!(out, "{num}. {err}");
    }
    let _w = write!(out, "\nPrevious output:\n{previous_raw}");
    out
}

async fn resolve_final_slug<S: ViewRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
    explicit_slug: Option<&str>,
    draft_slug: &str,
) -> Result<String, AppError> {
    if let Some(s) = explicit_slug {
        let trimmed = s.trim();
        validate_dashboard_slug(trimmed)?;
        return Ok(trimmed.to_owned());
    }
    let base = if is_valid_slug(draft_slug) {
        draft_slug.to_owned()
    } else {
        format!(
            "ai-view-{}",
            Uuid::new_v4()
                .simple()
                .to_string()
                .get(..8)
                .unwrap_or("dash")
        )
    };
    if store
        .get_dashboard_by_slug(tenant_id, &base, false)
        .await?
        .is_none()
    {
        return Ok(base);
    }
    let suffix = Uuid::new_v4().simple().to_string();
    let short_suffix = suffix.get(..6).unwrap_or("v2");
    let prefix = base.get(..base.len().min(55)).unwrap_or(&base);
    Ok(format!("{prefix}-{short_suffix}"))
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use axpoint_domain::enums::{DataType, PiiCategory, WidgetKind};
    use axpoint_domain::studio::PropertySpec;
    use axpoint_domain::{ExtractedTriple, FusedHit};

    use super::*;

    struct MockRepairAi {
        responses: std::sync::Mutex<Vec<String>>,
    }

    impl AiPort for MockRepairAi {
        fn embedding_model_name(&self) -> &'static str {
            "mock"
        }

        fn current_embedding_dim(&self) -> Option<usize> {
            Some(4)
        }

        fn embed_batch(
            &self,
            inputs: &[String],
        ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
            std::future::ready(Ok((vec![vec![0.0; 4]; inputs.len()], 4)))
        }

        fn embed_one(
            &self,
            _text: &str,
        ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
            std::future::ready(Ok((vec![0.0; 4], 4)))
        }

        fn extract_triples(
            &self,
            _paragraph: &str,
        ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send {
            std::future::ready(Ok(Vec::new()))
        }

        fn extract_query_entities(
            &self,
            _query: &str,
        ) -> impl Future<Output = Result<Vec<String>, AppError>> + Send {
            std::future::ready(Ok(Vec::new()))
        }

        fn synthesize_answer(
            &self,
            _query: &str,
            _hits: &[FusedHit],
        ) -> impl Future<Output = Result<String, AppError>> + Send {
            std::future::ready(Ok(String::new()))
        }

        fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> impl Future<Output = Result<String, AppError>> + Send {
            let reply = {
                let mut guard = self
                    .responses
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if guard.is_empty() {
                    String::new()
                } else {
                    guard.remove(0)
                }
            };
            std::future::ready(Ok(reply))
        }
    }

    #[tokio::test]
    async fn draft_dashboard_self_repairs_once_on_grid_and_metric_error() -> Result<(), AppError> {
        let mut ctx = DashboardGenContext::default();
        ctx.schema.node_types.push(NodeTypeSpec {
            id: None,
            name: String::from("Warehouse"),
            display_name: String::from("仓库"),
            description: String::new(),
            icon: String::from("warehouse"),
            color: String::from("#2563eb"),
            title_template: String::from("{node_key}"),
            properties: vec![PropertySpec {
                name: String::from("inventory"),
                display_name: String::from("库存"),
                description: String::new(),
                unit: None,
                data_type: DataType::Float64,
                kind: PropertyKind::Metric,
                required: false,
                pii_category: PiiCategory::None,
                allowed_values: None,
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            }],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 0.0,
            pos_y: 0.0,
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            semantic_layer_tier: None,
            domain_group: None,
        });

        let bad = serde_json::json!({
            "name": "坏看板",
            "slug": "bad-dash",
            "widgets": [{
                "widget_kind": "LINE_CHART",
                "title": "越界组件",
                "position": {"x": 20, "y": 0, "w": 8, "h": 4},
                "data_source": {"kind": "NODE_METRIC", "type_name": "Warehouse", "metric": "inventory"}
            }]
        })
        .to_string();

        let good = serde_json::json!({
            "name": "库存看板",
            "slug": "inv-dash",
            "widgets": [{
                "widget_kind": "LINE_CHART",
                "title": "仓库库存",
                "position": {"x": 0, "y": 0, "w": 12, "h": 6},
                "data_source": {"kind": "NODE_METRIC", "type_name": "Warehouse", "metric": "inventory"}
            }]
        })
        .to_string();

        let ai = MockRepairAi {
            responses: std::sync::Mutex::new(vec![bad, good]),
        };
        let req = AiGenerateDashboardRequest {
            prompt: String::from("仓库库存趋势"),
            title: None,
            slug: None,
            theme: None,
        };
        let draft = draft_dashboard_with_ai(&ai, &ctx, &req).await?;
        assert_eq!(draft.slug, "inv-dash");
        assert_eq!(draft.widgets.len(), 1);
        assert_eq!(
            draft.widgets.first().map(|w| w.widget_kind),
            Some(WidgetKind::LineChart)
        );
        Ok(())
    }

    #[test]
    fn parse_draft_normalizes_scene3d_and_prompts_use_ready_catalog_columns() -> Result<(), AppError>
    {
        let raw_legacy = serde_json::json!({
            "name": "3D 看板",
            "slug": "twin-3d",
            "widgets": [{
                "widget_kind": "SCENE_3D",
                "title": "主仓孪生",
                "position": {"x": 0, "y": 0, "w": 24, "h": 10},
                "data_source": {"kind": "SCENE3D", "asset_id": Uuid::from_u128(0x301)}
            }]
        })
        .to_string();
        let parsed = parse_ai_dashboard_draft(&raw_legacy)?;
        let serialized = serde_json::to_string(&parsed.widgets)
            .map_err(|err| AppError::Database(err.to_string()))?;
        assert!(serialized.contains("\"kind\":\"SCENE_3D\""));

        let now = chrono::Utc::now();
        let ready_id = Uuid::from_u128(0x101);
        let syncing_id = Uuid::from_u128(0x102);
        let make_ds = |id: Uuid, name: &str, status: DatasetStatus| Dataset {
            id,
            connection_id: None,
            name: String::from(name),
            display_name: String::from(name),
            description: String::new(),
            tags: Vec::new(),
            source_kind: axpoint_domain::enums::SourceKind::SqlTable,
            source_ref: serde_json::json!({}),
            sync_mode: axpoint_domain::enums::SyncMode::Snapshot,
            cursor_column: None,
            cursor_value: None,
            primary_key: Vec::new(),
            delete_column: None,
            schema_policy: axpoint_domain::axhub::SchemaPolicy::Evolve,
            lake_schema: String::from("tenant_default"),
            schedule_cron: None,
            lake_table: String::from(name),
            schema_version: 1,
            schema_json: Vec::new(),
            row_count: 10,
            last_snapshot_id: None,
            last_synced_at: Some(now),
            status,
            owner_user_id: None,
            created_at: now,
            updated_at: now,
        };

        let mut ctx = DashboardGenContext {
            datasets: vec![
                make_ds(ready_id, "demo_orders", DatasetStatus::Ready),
                make_ds(syncing_id, "staging_raw", DatasetStatus::Syncing),
            ],
            catalog: TenantCatalog::new("default"),
            ..DashboardGenContext::default()
        };
        let mut entry = CatalogDatasetEntry::simple(ready_id, "demo_orders", "demo_orders");
        entry.columns.insert(
            String::from("amount"),
            axpoint_domain::axhub::ColumnMeta {
                dataset_id: ready_id,
                column_name: String::from("amount"),
                data_type: DataType::Float64,
                pii_category: PiiCategory::None,
                pii_confirmed: false,
                description: String::new(),
                business_term: String::new(),
                unit: String::new(),
                ordinal: 1,
            },
        );
        entry.columns.insert(
            String::from("order_id"),
            axpoint_domain::axhub::ColumnMeta {
                dataset_id: ready_id,
                column_name: String::from("order_id"),
                data_type: DataType::String,
                pii_category: PiiCategory::None,
                pii_confirmed: false,
                description: String::new(),
                business_term: String::new(),
                unit: String::new(),
                ordinal: 0,
            },
        );
        ctx.catalog.insert_dataset(entry);

        let (_sys, usr) = build_dashboard_gen_prompts(
            &ctx,
            &AiGenerateDashboardRequest {
                prompt: String::from("订单概览"),
                title: None,
                slug: None,
                theme: None,
            },
        );
        assert!(usr.contains("- demo_orders cols=[order_id,amount]"));
        assert!(!usr.contains("staging_raw"));
        Ok(())
    }

    #[test]
    fn parse_draft_normalizes_llm_data_source_aliases_and_surfaces_parse_error()
    -> Result<(), AppError> {
        let raw_aliases = serde_json::json!({
            "name": "供应链看板",
            "slug": "supply-ai-dash",
            "widgets": [
                {
                    "widget_kind": "KPI_CARD",
                    "title": "已支付金额",
                    "position": {"x": 0, "y": 0, "w": 6, "h": 4},
                    "data_source": {"type": "metric", "metric_name": "paid_amount"}
                },
                {
                    "widget_kind": "PIE_CHART",
                    "title": "订单状态分布",
                    "position": {"x": 6, "y": 0, "w": 9, "h": 6},
                    "data_source": {"type": "sql", "query": "SELECT status, COUNT(*) AS cnt FROM demo_orders GROUP BY status"}
                },
                {
                    "widget_kind": "BAR_CHART",
                    "title": "仓库库存监控",
                    "position": {"x": 15, "y": 0, "w": 9, "h": 6},
                    "data_source": {"type": "node-metric", "node_type": "Warehouse", "metric_name": "inventory"}
                }
            ]
        })
        .to_string();

        let parsed = parse_ai_dashboard_draft(&raw_aliases)?;
        assert_eq!(parsed.widgets.len(), 3);
        assert!(matches!(
            parsed.widgets.first().map(|w| &w.data_source),
            Some(WidgetDataSource::Metric { metric, .. }) if metric == "paid_amount"
        ));
        assert!(matches!(
            parsed.widgets.get(1).map(|w| &w.data_source),
            Some(WidgetDataSource::Sql { sql }) if sql.contains("FROM demo_orders")
        ));
        assert!(matches!(
            parsed.widgets.get(2).map(|w| &w.data_source),
            Some(WidgetDataSource::NodeMetric { type_name, metric, .. })
                if type_name == "Warehouse" && metric == "inventory"
        ));

        let bad_raw = r#"{"name":"坏看板","slug":"bad","widgets":[{"widget_kind":"KPI_CARD","title":"x","position":{"x":0,"y":0,"w":6,"h":4},"data_source":{"kind":"NOT_A_REAL_KIND"}}]}"#;
        let err_msg = match parse_ai_dashboard_draft(bad_raw) {
            Ok(_) => return Err(AppError::Config(String::from("expected parse error"))),
            Err(err) => err.client_message(),
        };
        assert!(err_msg.contains("failed to decode AiDashboardDraft JSON:"));
        assert!(err_msg.contains("NOT_A_REAL_KIND"));
        Ok(())
    }

    #[test]
    fn synthesize_studio_dashboard_draft_builds_valid_grounded_widgets() {
        let mut ctx = DashboardGenContext::default();
        ctx.schema.node_types.push(NodeTypeSpec {
            name: String::from("Warehouse"),
            display_name: String::from("仓库"),
            properties: vec![PropertySpec {
                name: String::from("inventory_qty"),
                display_name: String::from("可用库存量"),
                data_type: DataType::Float64,
                kind: PropertyKind::Metric,
                ..PropertySpec::default()
            }],
            ..NodeTypeSpec::default()
        });

        let req = AiDraftDashboardRequest {
            name: String::from("门禁仓库履约孪生看板"),
            slug: Some(String::from("gate-step19-warehouse-cockpit")),
            prompt: String::from("围绕 Warehouse 在库量与履约告警生成监控驾驶舱"),
            subgraph_slug: Some(String::from("main")),
            seed_node_types: vec![String::from("Warehouse")],
            theme: None,
        };
        let seeded = vec![String::from("Warehouse")];
        let draft = synthesize_studio_dashboard_draft(&ctx, &req, &seeded);
        assert_eq!(draft.widgets.len(), 4);
        assert!(draft_covers_seeded_types(&draft, &seeded));
        let issues = validate_ai_dashboard_draft(&draft, &ctx);
        assert!(
            issues.is_empty(),
            "unexpected validation issues: {issues:?}"
        );
    }
}
