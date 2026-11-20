//! AI one-sentence `xBlueView` dashboard generator with schema/SQL validation and 1-round self-repair (`10-STEP11-12` §11.6).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use axpoint_analytics::{CatalogDatasetEntry, GuardOptions, TenantCatalog, compile_pivot, guard};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{MetricDef, PivotSpec, SavedQuery};
use axpoint_domain::axhub::{Dataset, DatasetListQuery, DatasetStatus};
use axpoint_domain::enums::{AssetKind, Environment, Purpose};
use axpoint_domain::flow::FlowListQuery;
use axpoint_domain::ports::{
    AiPort, AnalyticsRepository, DatasetRepository, FlowRepository, StudioRepository,
    ViewRepository,
};
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::studio::{NodeTypeSpec, PropertyKind, SchemaDoc};
use axpoint_domain::topology::AxFlowDef;
use axpoint_domain::view::{
    AiDashboardDraft, AiGenerateDashboardRequest, AssetRecord, CanvasConfig,
    CreateDashboardRequest, CreateWidgetRecordParams, DashboardDetail, RefreshMode,
    WidgetActionType, WidgetDataSource, WidgetPosition,
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

/// Collects `SchemaDoc`, `an_metrics`, `ds_datasets`, `PROD` flows, and `xv_assets` for `tenant_id`.
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
    let published = store.get_published_schema(tenant_id, "main").await?;
    let metrics = store.list_metrics(tenant_id).await?;
    let saved_queries = store
        .list_saved_queries(tenant_id, Uuid::nil(), true)
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
        datasets.extend(page.items);
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }

    let mut catalog = TenantCatalog::new(tenant_id);
    for ds in &datasets {
        if ds.status == DatasetStatus::Disabled {
            continue;
        }
        let cols = store.list_columns(tenant_id, ds.id).await?;
        catalog.insert_dataset(CatalogDatasetEntry::from_dataset(ds, &cols));
    }

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

/// Builds `(system_prompt, user_prompt)` for `Purpose::Extract` dashboard generation.
#[must_use]
pub fn build_dashboard_gen_prompts(
    ctx: &DashboardGenContext,
    req: &AiGenerateDashboardRequest,
) -> (String, String) {
    let sys = format!(
        "{VIEW_AI_GEN_PURPOSE_TAG}\n\
You design xBlueView digital-twin monitoring dashboards. Output ONLY valid JSON matching:\n\
{{\"name\":\"...\",\"slug\":\"lowercase-slug\",\"theme\":\"light|dark|screen\",\"description\":\"...\",\"filters\":[],\"widgets\":[{{\"widget_kind\":\"KPI_CARD|TABLE|PIVOT_TABLE|LINE_CHART|BAR_CHART|PIE_CHART|SCATTER|SANKEY|HEATMAP|GAUGE|RADAR|GRAPH_TOPOLOGY|MAP|FLOOR_PLAN|SCENE_3D|ALERT_LIST|SIM_COMPARE|ACTION_BUTTON|ACTION_FORM|MARKDOWN_NOTE\",\"title\":\"...\",\"position\":{{\"x\":0,\"y\":0,\"w\":6,\"h\":4}},\"data_source\":{{...}},\"encoding\":{{}},\"options\":{{}},\"drill\":[]}}]}}\n\
Rules:\n\
1. 24-column grid: x >= 0, w >= 1, x + w <= 24, y >= 0, h >= 1.\n\
2. Use ONLY existing metrics, datasets/columns (valid DuckDB SELECT SQL), node types, PROD flows, and assets listed below.\n\
3. slug must match ^[a-z0-9][a-z0-9-]{{0,62}}$."
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
        if ds.status == DatasetStatus::Disabled {
            continue;
        }
        let cols: Vec<&str> = ds.schema_json.iter().map(|c| c.name.as_str()).collect();
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
    if let Ok(draft) = decode_normalized_draft(trimmed) {
        return Ok(draft);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Some(slice) = trimmed.get(start..=end)
        && let Ok(draft) = decode_normalized_draft(slice)
    {
        return Ok(draft);
    }
    Err(AppError::bad(
        "PARSE_ERROR",
        "failed to decode AiDashboardDraft JSON",
    ))
}

fn decode_normalized_draft(raw: &str) -> Result<AiDashboardDraft, serde_json::Error> {
    let mut val: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(widgets) = val
        .get_mut("widgets")
        .and_then(serde_json::Value::as_array_mut)
    {
        for w in widgets {
            if let Some(kind_val) = w.get_mut("data_source").and_then(|ds| ds.get_mut("kind"))
                && kind_val.as_str() == Some("SCENE_3D")
            {
                *kind_val = serde_json::Value::String(String::from("SCENE3D"));
            }
        }
    }
    serde_json::from_value(val)
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

    let ctx = collect_dashboard_gen_context(store, tenant_id).await?;
    let (sys, usr) = build_dashboard_gen_prompts(&ctx, &req);

    let raw = llm_router
        .complete_chat(tenant_id, Purpose::Extract, &sys, &usr)
        .await?;
    let mut draft = match parse_and_validate(&raw, &ctx) {
        Ok(d) => d,
        Err((repair_usr, _)) => {
            let repaired_raw = llm_router
                .complete_chat(tenant_id, Purpose::Extract, &sys, &repair_usr)
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
        "The previous AiDashboardDraft JSON failed validation. Fix every issue listed below and output ONLY valid JSON.\nErrors:\n",
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
            }],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 0.0,
            pos_y: 0.0,
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
}
