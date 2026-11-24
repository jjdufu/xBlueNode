//! `AxSim` sandbox cloning, `CoW` overlay editing, causal simulation, Ask, compare, and decision routes (`/api/axsim/*`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::agent::{SimRunToolInput, SimRunToolOutput};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    AuditAction, Environment, InterventionMode, OutboxTopic, Permission, TokenKind, TriggerType,
};
use axpoint_domain::flow::{ApprovalSourceKind, ApprovalStatus, StartRunParams};
use axpoint_domain::graph::NodeSearchQuery;
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::platform::{AuditRecord, BoxFuture, OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{
    CredentialStore as _, FlowRepository, GraphStorePort, SimExecutionPort, SimRepository,
    SimVisibilityPort as _,
};
use axpoint_domain::sim::{
    CheckpointsResponse, CompareKpiCell, CompareKpiRow, CompareNodeCell, CompareNodeRow,
    CompareScenarioColumn, CompareScenariosQuery, CompareScenariosResponse, CreateDecisionRequest,
    CreateOverlayLinkRequest, CreateOverlayNodeRequest, CreateScenarioRequest, DecisionRecord,
    DecisionStatus, ImpactListQuery, ImpactRow, PatchOverlayLinkRequest, PatchOverlayNodeRequest,
    RefreshBaseResponse, SandboxGraphLink, SandboxGraphNode, SandboxGraphResponse, SandboxScope,
    ScenarioRecord, ScenarioStatus, ScopePreflightReport, ScopePreflightRequest, SimAskIntent,
    SimAskRequest, SimAskResponse, SimGraph, SimMode, SimParams, evaluate_scope_preflight,
};
use axpoint_domain::topology::{
    AxSandboxRecord, AxSimAction, AxSimImpactRow, AxSimResult, CreateAxSandboxRequest,
    SimulateAxSandboxRequest,
};
use axpoint_security::AuthPrincipal;
use axpoint_server::sim_ask::{
    DisambiguationOutcome, classify_sim_ask_intent, disambiguate_what_if_actions,
    extract_what_if_interventions, generate_grounded_briefing, graph_hint_nodes,
    query_impacts_with_nl2sql, summarize_what_if_result,
};
use axum::extract::State;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};

/// Builds the `/axsim/*` Axum router (`09-STEP10` §10.9).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/axsim/sandboxes",
            get(list_sandboxes_handler).post(create_sandbox_handler),
        )
        .route(
            "/axsim/sandboxes/preflight",
            post(preflight_sandbox_handler),
        )
        .route(
            "/axsim/sandboxes/{id}",
            get(get_sandbox_handler).delete(delete_sandbox_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/graph",
            get(get_sandbox_graph_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/simulate",
            post(simulate_sandbox_handler),
        )
        .route("/axsim/sandboxes/{id}/reset", post(reset_overlay_handler))
        .route(
            "/axsim/sandboxes/{id}/refresh-base",
            post(refresh_base_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/nodes",
            post(create_overlay_node_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/nodes/{nid}",
            patch(patch_overlay_node_handler).delete(delete_overlay_node_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/links",
            post(create_overlay_link_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/links/{lid}",
            patch(patch_overlay_link_handler).delete(delete_overlay_link_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/scenarios",
            get(list_scenarios_handler).post(create_scenario_handler),
        )
        .route("/axsim/scenarios/compare", get(compare_scenarios_handler))
        .route(
            "/axsim/scenarios/{id}",
            get(get_scenario_handler).delete(delete_scenario_handler),
        )
        .route("/axsim/scenarios/{id}/rerun", post(rerun_scenario_handler))
        .route(
            "/axsim/scenarios/{id}/impacts",
            get(list_scenario_impacts_handler),
        )
        .route(
            "/axsim/scenarios/{id}/checkpoints",
            get(list_scenario_checkpoints_handler),
        )
        .route("/axsim/ask", post(sim_ask_handler))
        .route(
            "/axsim/scenarios/{id}/decisions",
            post(create_decision_handler),
        )
        .route("/axsim/decisions/{id}", get(get_decision_handler))
        .route(
            "/axsim/decisions/{id}/rebase",
            post(rebase_decision_handler),
        )
}

#[derive(Debug, Serialize)]
struct DeleteStatus {
    deleted: bool,
}

#[derive(Debug, Serialize)]
struct ResetStatus {
    reset: bool,
}

async fn list_sandboxes_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
) -> Result<Json<Vec<AxSandboxRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut items = state.store.list_sandboxes(&principal.tenant_id).await?;
    for sb in &mut items {
        redact_sandbox_record_governed(&state.store, &principal.tenant_id, sb, &policy_ctx).await?;
    }
    Ok(Json(items))
}

async fn preflight_sandbox_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<ScopePreflightRequest>,
) -> Result<Json<ScopePreflightReport>, HttpError> {
    principal.require(Permission::SimRun)?;
    let slug = {
        let trimmed = req.subgraph_slug.trim();
        if trimmed.is_empty() { "main" } else { trimmed }
    };
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    if slug != "main" {
        let sub_action = if principal.has(Permission::DataRead) {
            Permission::DataRead
        } else {
            Permission::SimRun
        };
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            sub_action,
            axpoint_domain::guard::GrantResourceKind::Subgraph,
            slug,
            &client_ip,
        )
        .await?;
    }
    let policy_ctx = axpoint_server::policy::build_policy_context_for_subgraph(
        &state.store,
        &state.cedar,
        &principal,
        slug,
        false,
        &client_ip,
    )
    .await?;
    let slice = state
        .store
        .preview_scope_slice_governed(&principal.tenant_id, slug, &req.scope, &policy_ctx)
        .await?;
    Ok(Json(evaluate_scope_preflight(&req.scope, &slice)))
}

async fn create_sandbox_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateAxSandboxRequest>,
) -> Result<Json<AxSandboxRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy_ctx = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let created = state
        .store
        .create_sandbox_governed(
            &principal.tenant_id,
            Some(principal.user_id),
            req,
            &policy_ctx,
        )
        .await?;
    Ok(Json(created))
}

async fn get_sandbox_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AxSandboxRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut sandbox = state
        .store
        .get_sandbox(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SANDBOX_NOT_FOUND"))?;
    redact_sandbox_record_governed(
        &state.store,
        &principal.tenant_id,
        &mut sandbox,
        &policy_ctx,
    )
    .await?;
    Ok(Json(sandbox))
}

async fn delete_sandbox_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
    let deleted = state.store.delete_sandbox(&principal.tenant_id, id).await?;
    if !deleted {
        return Err(HttpError::from(AppError::NotFound("SANDBOX_NOT_FOUND")));
    }
    Ok(Json(DeleteStatus { deleted }))
}

async fn get_sandbox_graph_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<SandboxGraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy_ctx = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let graph = state
        .store
        .get_sandbox_graph_governed(&principal.tenant_id, id, &policy_ctx)
        .await?;
    Ok(Json(graph))
}

async fn build_sim_policy_ctx(
    state: &AppState,
    principal: &AuthPrincipal,
    headers: Option<&axum::http::HeaderMap>,
) -> Result<PolicyContext, AppError> {
    let ip = headers.map_or_else(
        || String::from("127.0.0.1"),
        crate::routes::auth::extract_client_ip,
    );
    axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        principal,
        false,
        &ip,
    )
    .await
}

async fn load_governed_sim_graph(
    store: &PgStore,
    tenant_id: &str,
    sandbox_id: Uuid,
    policy_ctx: &PolicyContext,
) -> Result<(SimGraph, Option<HashSet<Uuid>>), AppError> {
    let mut graph = store.load_graph(tenant_id, sandbox_id).await?;
    if policy_ctx.bypass_row_policies || policy_ctx.node_types.is_empty() {
        return Ok((graph, None));
    }
    let visible = store
        .filter_sandbox_visible_node_ids(tenant_id, sandbox_id, policy_ctx)
        .await?;
    graph.nodes.retain(|n| visible.contains(&n.id));
    graph
        .links
        .retain(|l| visible.contains(&l.src_node_id) && visible.contains(&l.dst_node_id));
    Ok((graph, Some(visible)))
}

async fn ensure_governed_sandbox_node(
    store: &PgStore,
    tenant_id: &str,
    sandbox_id: Uuid,
    node_id: Uuid,
    policy_ctx: &PolicyContext,
) -> Result<(), AppError> {
    if policy_ctx.bypass_row_policies || policy_ctx.node_types.is_empty() {
        return Ok(());
    }
    let visible = store
        .filter_sandbox_visible_node_ids(tenant_id, sandbox_id, policy_ctx)
        .await?;
    if !visible.contains(&node_id) {
        return Err(AppError::NotFound("NODE_NOT_FOUND"));
    }
    Ok(())
}

fn ensure_actions_target_visible(
    actions: &[AxSimAction],
    visible: Option<&HashSet<Uuid>>,
) -> Result<(), AppError> {
    if let Some(vis) = visible
        && actions.iter().any(|a| !vis.contains(&a.target_node_id))
    {
        return Err(AppError::NotFound("NODE_NOT_FOUND"));
    }
    Ok(())
}

fn mask_sandbox_graph_node(
    state: &AppState,
    tenant_id: &str,
    node: &mut SandboxGraphNode,
    policy_ctx: &PolicyContext,
) {
    if let Some((_, spec)) = policy_ctx
        .node_types
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&node.type_name))
        && !spec.property_masks.is_empty()
    {
        let key = state.mask_keys.tenant_key(tenant_id);
        axpoint_security::mask_json_properties(&mut node.properties, &spec.property_masks, &key);
    }
}

async fn filter_and_mask_sim_search_hits(
    state: &AppState,
    tenant_id: &str,
    hits: &mut Vec<axpoint_domain::graph::NodeSearchHit>,
    policy_ctx: &PolicyContext,
) -> Result<(), AppError> {
    if !policy_ctx.bypass_row_policies && !hits.is_empty() {
        let ids: Vec<Uuid> = hits.iter().map(|h| h.id).collect();
        let visible = state
            .store
            .filter_visible_node_ids(tenant_id, &ids, &policy_ctx.node_types)
            .await?;
        hits.retain(|h| visible.contains(&h.id));
    }
    let key = state.mask_keys.tenant_key(tenant_id);
    for hit in hits {
        axpoint_server::policy::apply_search_hit_property_masks(hit, policy_ctx, &key);
    }
    Ok(())
}

async fn redact_sandbox_record_governed(
    store: &PgStore,
    tenant_id: &str,
    sb: &mut AxSandboxRecord,
    policy_ctx: &PolicyContext,
) -> Result<(), AppError> {
    if policy_ctx.bypass_row_policies
        || policy_ctx.node_types.is_empty()
        || (sb.actions.is_empty() && sb.sim_result.is_none())
    {
        return Ok(());
    }
    let visible = store
        .filter_sandbox_visible_node_ids(tenant_id, sb.id, policy_ctx)
        .await?;
    let pre_act = sb.actions.len();
    sb.actions.retain(|a| visible.contains(&a.target_node_id));
    let act_removed = sb.actions.len() < pre_act;
    if let Some(ref mut res) = sb.sim_result {
        let pre_imp = res.impacts.len();
        res.impacts.retain(|r| visible.contains(&r.node_id));
        if res.impacts.len() < pre_imp || act_removed {
            res.affected_count = res.impacts.len();
            res.kpis.clear();
        }
    }
    Ok(())
}

fn redact_scenario_with_visible(sc: &mut ScenarioRecord, visible: &HashSet<Uuid>) {
    let pre_act = sc.actions.len();
    sc.actions.retain(|a| visible.contains(&a.target_node_id));
    let act_removed = sc.actions.len() < pre_act;
    if let Some(ref mut sum) = sc.result {
        let pre_imp = sum.top_impacts.len();
        sum.top_impacts.retain(|r| visible.contains(&r.node_id));
        if sum.top_impacts.len() < pre_imp || act_removed {
            sum.affected_count = sum.top_impacts.len();
            sum.kpis.clear();
        }
    }
}

async fn ensure_governed_sandbox_link(
    store: &PgStore,
    tenant_id: &str,
    sandbox_id: Uuid,
    link_id: Uuid,
    policy_ctx: &PolicyContext,
) -> Result<(), AppError> {
    if policy_ctx.bypass_row_policies || policy_ctx.node_types.is_empty() {
        return Ok(());
    }
    let graph = store
        .get_sandbox_graph_governed(tenant_id, sandbox_id, policy_ctx)
        .await?;
    if !graph.links.iter().any(|l| l.id == link_id) {
        return Err(AppError::NotFound("LINK_NOT_FOUND"));
    }
    Ok(())
}

async fn ensure_decision_nodes_visible(
    store: &PgStore,
    tenant_id: &str,
    decision: &DecisionRecord,
    policy_ctx: &PolicyContext,
) -> Result<(), AppError> {
    if policy_ctx.bypass_row_policies || policy_ctx.node_types.is_empty() {
        return Ok(());
    }
    let mut ids: Vec<Uuid> = decision.changes.iter().map(|c| c.node_id).collect();
    for item in &decision.conflict_report {
        if !ids.contains(&item.node_id) {
            ids.push(item.node_id);
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    let visible = store
        .filter_visible_node_ids(tenant_id, &ids, &policy_ctx.node_types)
        .await?;
    if ids.iter().any(|id| !visible.contains(id)) {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    Ok(())
}

fn solve_governed_sandbox_quick(
    graph: &SimGraph,
    req: &SimulateAxSandboxRequest,
) -> Result<AxSimResult, AppError> {
    let params = req.params.clone().unwrap_or_default();
    let sim_res = if params.uncertainty.is_some() {
        axpoint_domain::sim::uncertainty::solve(graph, &req.actions, &params)?
    } else if params.mode == SimMode::Stepped {
        axpoint_domain::sim::stepped::solve(graph, &req.actions, &params)?
    } else {
        axpoint_domain::sim::steady::solve(graph, &req.actions, &params)?
    };
    let impacts = sim_res
        .impacts
        .into_iter()
        .map(|r| AxSimImpactRow {
            node_id: r.node_id,
            type_name: r.type_name,
            title: r.title,
            metric_key: r.metric,
            before_val: r.before_val.unwrap_or(0.0),
            after_val: r.after_val,
            delta: r.delta.unwrap_or(0.0),
            delta_ratio: r.delta_ratio,
            hop_distance: r.hop_distance,
            top_path: r.top_path,
        })
        .collect();
    Ok(AxSimResult {
        impacts,
        affected_count: sim_res.affected_count,
        kpis: sim_res.kpis,
        warnings: sim_res.warnings,
        converged: sim_res.converged,
        fired_events: sim_res.fired_events,
        deadline_report: sim_res.deadline_report,
    })
}

async fn simulate_sandbox_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<SimulateAxSandboxRequest>,
) -> Result<Json<AxSimResult>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() {
        let (graph, visible) =
            load_governed_sim_graph(&state.store, &principal.tenant_id, id, &policy_ctx).await?;
        ensure_actions_target_visible(&req.actions, visible.as_ref())?;
        return Ok(Json(solve_governed_sandbox_quick(&graph, &req)?));
    }
    let res = state
        .store
        .quick_simulate_sandbox(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(res))
}

async fn reset_overlay_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ResetStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
    state.store.reset_overlay(&principal.tenant_id, id).await?;
    Ok(Json(ResetStatus { reset: true }))
}

async fn refresh_base_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RefreshBaseResponse>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let refreshed = state
        .store
        .refresh_sandbox_base_governed(&principal.tenant_id, id, &policy_ctx)
        .await?;
    Ok(Json(refreshed))
}

async fn create_overlay_node_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateOverlayNodeRequest>,
) -> Result<Json<SandboxGraphNode>, HttpError> {
    principal.require(Permission::SimRun)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::SimRun,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        req.type_name.trim(),
        &client_ip,
    )
    .await?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut node = state
        .store
        .create_overlay_node(&principal.tenant_id, id, req)
        .await?;
    if ensure_governed_sandbox_node(&state.store, &principal.tenant_id, id, node.id, &policy_ctx)
        .await
        .is_err()
    {
        let _deleted = state
            .store
            .delete_overlay_node(&principal.tenant_id, id, node.id)
            .await?;
        return Err(HttpError::from(AppError::Forbidden("PERMISSION_DENIED")));
    }
    mask_sandbox_graph_node(&state, &principal.tenant_id, &mut node, &policy_ctx);
    Ok(Json(node))
}

async fn patch_overlay_node_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((id, nid)): ApiPath<(Uuid, Uuid)>,
    ApiJson(req): ApiJson<PatchOverlayNodeRequest>,
) -> Result<Json<SandboxGraphNode>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    ensure_governed_sandbox_node(&state.store, &principal.tenant_id, id, nid, &policy_ctx).await?;
    let mut node = state
        .store
        .patch_overlay_node(&principal.tenant_id, id, nid, req)
        .await?;
    mask_sandbox_graph_node(&state, &principal.tenant_id, &mut node, &policy_ctx);
    Ok(Json(node))
}

async fn delete_overlay_node_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((id, nid)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    ensure_governed_sandbox_node(&state.store, &principal.tenant_id, id, nid, &policy_ctx).await?;
    let deleted = state
        .store
        .delete_overlay_node(&principal.tenant_id, id, nid)
        .await?;
    if !deleted {
        return Err(HttpError::from(AppError::NotFound("NODE_NOT_FOUND")));
    }
    Ok(Json(DeleteStatus { deleted }))
}

async fn create_overlay_link_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateOverlayLinkRequest>,
) -> Result<Json<SandboxGraphLink>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    ensure_governed_sandbox_node(
        &state.store,
        &principal.tenant_id,
        id,
        req.src_node_id,
        &policy_ctx,
    )
    .await?;
    ensure_governed_sandbox_node(
        &state.store,
        &principal.tenant_id,
        id,
        req.dst_node_id,
        &policy_ctx,
    )
    .await?;
    let link = state
        .store
        .create_overlay_link(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(link))
}

async fn patch_overlay_link_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((id, lid)): ApiPath<(Uuid, Uuid)>,
    ApiJson(req): ApiJson<PatchOverlayLinkRequest>,
) -> Result<Json<SandboxGraphLink>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    ensure_governed_sandbox_link(&state.store, &principal.tenant_id, id, lid, &policy_ctx).await?;
    let link = state
        .store
        .patch_overlay_link(&principal.tenant_id, id, lid, req)
        .await?;
    Ok(Json(link))
}

async fn delete_overlay_link_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((id, lid)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    ensure_governed_sandbox_link(&state.store, &principal.tenant_id, id, lid, &policy_ctx).await?;
    let deleted = state
        .store
        .delete_overlay_link(&principal.tenant_id, id, lid)
        .await?;
    if !deleted {
        return Err(HttpError::from(AppError::NotFound("LINK_NOT_FOUND")));
    }
    Ok(Json(DeleteStatus { deleted }))
}

async fn list_scenarios_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Vec<ScenarioRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut items = state.store.list_scenarios(&principal.tenant_id, id).await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() && !items.is_empty() {
        let visible = state
            .store
            .filter_sandbox_visible_node_ids(&principal.tenant_id, id, &policy_ctx)
            .await?;
        for sc in &mut items {
            redact_scenario_with_visible(sc, &visible);
        }
    }
    Ok(Json(items))
}

async fn create_scenario_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateScenarioRequest>,
) -> Result<Json<ScenarioRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    if req.name.trim().is_empty() {
        return Err(HttpError::from(AppError::bad(
            "INVALID_ARGUMENT",
            "scenario name cannot be empty",
        )));
    }
    if req.actions.is_empty() {
        return Err(HttpError::from(AppError::bad(
            "INVALID_ARGUMENT",
            "scenario actions cannot be empty",
        )));
    }

    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let overlay_hash = state
        .store
        .compute_overlay_hash(&principal.tenant_id, id)
        .await?;
    let (graph, visible) =
        load_governed_sim_graph(&state.store, &principal.tenant_id, id, &policy_ctx).await?;
    ensure_actions_target_visible(&req.actions, visible.as_ref())?;

    // Validate actions synchronously before creating scenario row so bad requests fail immediately.
    axpoint_domain::sim::action::normalize_actions(&graph, &req.actions)?;

    let created = state
        .store
        .create_scenario_record(
            &principal.tenant_id,
            id,
            Some(principal.user_id),
            &overlay_hash,
            req.clone(),
        )
        .await?;

    let is_heavy = graph.nodes.len() > 5_000
        || req
            .params
            .uncertainty
            .as_ref()
            .is_some_and(|u| u.samples > 1);
    if is_heavy {
        let store = state.store.clone();
        let tenant_id = principal.tenant_id.clone();
        let scenario_id = created.id;
        let actions = req.actions;
        let params = req.params;
        let hash = overlay_hash;
        drop(tokio::spawn(async move {
            let _res = execute_and_persist_scenario(
                &store,
                &tenant_id,
                scenario_id,
                graph,
                actions,
                params,
                &hash,
            )
            .await;
        }));
        return Ok(Json(created));
    }

    let finished = execute_and_persist_scenario(
        &state.store,
        &principal.tenant_id,
        created.id,
        graph,
        req.actions,
        req.params,
        &overlay_hash,
    )
    .await?;
    Ok(Json(finished))
}

async fn execute_and_persist_scenario(
    store: &PgStore,
    tenant_id: &str,
    scenario_id: Uuid,
    graph: SimGraph,
    actions: Vec<AxSimAction>,
    params: SimParams,
    overlay_hash: &str,
) -> Result<ScenarioRecord, AppError> {
    store
        .mark_scenario_running(tenant_id, scenario_id, overlay_hash)
        .await?;

    let solve_outcome = tokio::task::spawn_blocking(move || {
        axpoint_domain::sim::uncertainty::solve(&graph, &actions, &params)
    })
    .await
    .map_err(|err| AppError::Database(format!("sim solver join failed: {err}")))?;

    match solve_outcome {
        Ok(result) => {
            store
                .save_scenario_success(tenant_id, scenario_id, overlay_hash, &result)
                .await
        }
        Err(err) => {
            let msg = err.to_string();
            store
                .save_scenario_failure(tenant_id, scenario_id, &msg)
                .await?;
            Err(err)
        }
    }
}

async fn get_scenario_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ScenarioRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut scenario = state
        .store
        .get_scenario(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() {
        let visible = state
            .store
            .filter_sandbox_visible_node_ids(&principal.tenant_id, scenario.sandbox_id, &policy_ctx)
            .await?;
        redact_scenario_with_visible(&mut scenario, &visible);
    }
    Ok(Json(scenario))
}

async fn delete_scenario_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
    let deleted = state
        .store
        .delete_scenario(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError::from(AppError::NotFound("SCENARIO_NOT_FOUND")));
    }
    Ok(Json(DeleteStatus { deleted }))
}

async fn rerun_scenario_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ScenarioRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let existing = state
        .store
        .get_scenario(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
    let overlay_hash = state
        .store
        .compute_overlay_hash(&principal.tenant_id, existing.sandbox_id)
        .await?;
    let (graph, visible) = load_governed_sim_graph(
        &state.store,
        &principal.tenant_id,
        existing.sandbox_id,
        &policy_ctx,
    )
    .await?;
    ensure_actions_target_visible(&existing.actions, visible.as_ref())?;
    let finished = execute_and_persist_scenario(
        &state.store,
        &principal.tenant_id,
        existing.id,
        graph,
        existing.actions,
        existing.params,
        &overlay_hash,
    )
    .await?;
    Ok(Json(finished))
}

async fn list_scenario_impacts_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ImpactListQuery>,
) -> Result<Json<PaginatedList<ImpactRow>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut page = state
        .store
        .list_scenario_impacts(&principal.tenant_id, id, query)
        .await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() {
        let sc = state
            .store
            .get_scenario(&principal.tenant_id, id)
            .await?
            .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
        let visible = state
            .store
            .filter_sandbox_visible_node_ids(&principal.tenant_id, sc.sandbox_id, &policy_ctx)
            .await?;
        page.items.retain(|r| visible.contains(&r.node_id));
    }
    Ok(Json(page))
}

async fn list_scenario_checkpoints_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<CheckpointsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let mut items = state
        .store
        .list_scenario_checkpoints(&principal.tenant_id, id)
        .await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() && !items.is_empty() {
        let sc = state
            .store
            .get_scenario(&principal.tenant_id, id)
            .await?
            .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
        let visible = state
            .store
            .filter_sandbox_visible_node_ids(&principal.tenant_id, sc.sandbox_id, &policy_ctx)
            .await?;
        for cp in &mut items {
            if let serde_json::Value::Object(ref mut map) = cp.changes {
                map.retain(|k, _| Uuid::parse_str(k).is_ok_and(|nid| visible.contains(&nid)));
            }
        }
    }
    Ok(Json(CheckpointsResponse { items }))
}

fn parse_compare_ids(raw_ids: &str) -> Result<Vec<Uuid>, AppError> {
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    for part in raw_ids.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed = Uuid::parse_str(trimmed).map_err(|_| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid scenario UUID: {trimmed}"),
            )
        })?;
        if seen.insert(parsed) {
            ids.push(parsed);
        }
    }
    if !(2..=5).contains(&ids.len()) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "compare requires between 2 and 5 unique scenario ids",
        ));
    }
    Ok(ids)
}

async fn compare_scenarios_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<CompareScenariosQuery>,
) -> Result<Json<CompareScenariosResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let ids = parse_compare_ids(&query.ids)?;

    let mut scenarios = Vec::with_capacity(ids.len());
    for sc_id in &ids {
        let sc = state
            .store
            .get_scenario(&principal.tenant_id, *sc_id)
            .await?
            .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
        scenarios.push(sc);
    }

    let first_sandbox = scenarios
        .first()
        .map(|s| s.sandbox_id)
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
    if scenarios.iter().any(|s| s.sandbox_id != first_sandbox) {
        return Err(HttpError::from(AppError::bad(
            "INVALID_ARGUMENT",
            "all compared scenarios must belong to the same sandbox",
        )));
    }

    let metric_filter: Option<BTreeSet<String>> = query.metrics.as_ref().map(|m| {
        m.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    });
    let type_filter = query
        .type_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let (graph, visible) = load_governed_sim_graph(
        &state.store,
        &principal.tenant_id,
        first_sandbox,
        &policy_ctx,
    )
    .await?;
    let mut impacts_by_scenario: Vec<(Uuid, Vec<ImpactRow>)> = Vec::with_capacity(scenarios.len());
    for sc in &scenarios {
        let mut rows = state
            .store
            .list_all_scenario_impacts(&principal.tenant_id, sc.id)
            .await?;
        if let Some(ref vis) = visible {
            rows.retain(|r| vis.contains(&r.node_id));
        }
        impacts_by_scenario.push((sc.id, rows));
    }
    let (kpis, nodes) = build_comparison_matrices(
        &graph,
        &impacts_by_scenario,
        metric_filter.as_ref(),
        type_filter.as_deref(),
    );

    let columns: Vec<CompareScenarioColumn> = scenarios
        .iter()
        .map(|s| CompareScenarioColumn {
            id: s.id,
            name: s.name.clone(),
            status: s.status,
            is_stale: s.is_stale,
        })
        .collect();

    let briefing = if query.briefing.unwrap_or(false) {
        Some(generate_grounded_briefing(&state.ai, &scenarios, &kpis, &nodes).await?)
    } else {
        None
    };

    Ok(Json(CompareScenariosResponse {
        sandbox_id: first_sandbox,
        scenarios: columns,
        kpis,
        nodes,
        briefing,
    }))
}

fn compute_base_sums(
    graph: &SimGraph,
    metric_filter: Option<&BTreeSet<String>>,
    type_filter: Option<&str>,
) -> BTreeMap<(String, String), f64> {
    let mut base_sums: BTreeMap<(String, String), f64> = BTreeMap::new();
    for node in &graph.nodes {
        if let Some(tf) = type_filter
            && node.type_name != tf
        {
            continue;
        }
        for (m_key, m_val) in &node.metrics {
            if let Some(mf) = metric_filter
                && !mf.contains(m_key)
            {
                continue;
            }
            let entry = base_sums
                .entry((node.type_name.clone(), m_key.clone()))
                .or_insert(0.0);
            *entry += *m_val;
        }
    }
    base_sums
}

fn build_comparison_matrices(
    graph: &SimGraph,
    impacts_by_scenario: &[(Uuid, Vec<ImpactRow>)],
    metric_filter: Option<&BTreeSet<String>>,
    type_filter: Option<&str>,
) -> (Vec<CompareKpiRow>, Vec<CompareNodeRow>) {
    let base_sums = compute_base_sums(graph, metric_filter, type_filter);
    let mut kpi_values: BTreeMap<(String, String), BTreeMap<String, CompareKpiCell>> =
        BTreeMap::new();
    let mut node_rows_map: BTreeMap<(Uuid, String), CompareNodeRow> = BTreeMap::new();

    for (sc_id, rows) in impacts_by_scenario {
        let sc_key = sc_id.to_string();
        let mut sc_kpi_delta: BTreeMap<(String, String), (f64, usize)> = BTreeMap::new();

        for row in rows {
            if let Some(tf) = type_filter
                && row.type_name != tf
            {
                continue;
            }
            if let Some(mf) = metric_filter
                && !mf.contains(&row.metric)
            {
                continue;
            }

            let delta = row.delta.unwrap_or(row.after_val);
            let agg = sc_kpi_delta
                .entry((row.type_name.clone(), row.metric.clone()))
                .or_insert((0.0, 0));
            agg.0 += delta;
            agg.1 = agg.1.saturating_add(1);

            let n_entry = node_rows_map
                .entry((row.node_id, row.metric.clone()))
                .or_insert_with(|| CompareNodeRow {
                    node_id: row.node_id,
                    type_name: row.type_name.clone(),
                    title: row.title.clone(),
                    metric: row.metric.clone(),
                    before_val: row.before_val,
                    values: BTreeMap::new(),
                    max_spread: 0.0,
                });
            n_entry.values.insert(
                sc_key.clone(),
                CompareNodeCell {
                    after_val: row.after_val,
                    delta_ratio: row.delta_ratio,
                    delta: row.delta,
                },
            );
        }

        for ((type_name, metric), (delta_sum, cnt)) in sc_kpi_delta {
            let before_sum = base_sums
                .get(&(type_name.clone(), metric.clone()))
                .copied()
                .unwrap_or(0.0);
            let after_sum = before_sum + delta_sum;
            let delta_ratio = if before_sum.abs() > 1e-12 {
                Some(delta_sum / before_sum)
            } else {
                None
            };
            kpi_values.entry((type_name, metric)).or_default().insert(
                sc_key.clone(),
                CompareKpiCell {
                    after_sum,
                    delta: delta_sum,
                    delta_ratio,
                    affected_nodes: cnt,
                },
            );
        }
    }

    (
        finalize_kpi_rows(kpi_values, &base_sums),
        finalize_node_rows(node_rows_map, impacts_by_scenario),
    )
}

fn finalize_kpi_rows(
    kpi_values: BTreeMap<(String, String), BTreeMap<String, CompareKpiCell>>,
    base_sums: &BTreeMap<(String, String), f64>,
) -> Vec<CompareKpiRow> {
    let mut kpis: Vec<CompareKpiRow> = kpi_values
        .into_iter()
        .map(|((type_name, metric), values)| {
            let before_sum = base_sums
                .get(&(type_name.clone(), metric.clone()))
                .copied()
                .unwrap_or(0.0);
            CompareKpiRow {
                type_name,
                metric,
                before_sum,
                values,
            }
        })
        .collect();
    kpis.sort_by(|a, b| {
        let max_a = a
            .values
            .values()
            .map(|c| c.delta.abs())
            .fold(0.0_f64, f64::max);
        let max_b = b
            .values
            .values()
            .map(|c| c.delta.abs())
            .fold(0.0_f64, f64::max);
        max_b.total_cmp(&max_a)
    });
    kpis
}

fn finalize_node_rows(
    node_rows_map: BTreeMap<(Uuid, String), CompareNodeRow>,
    impacts_by_scenario: &[(Uuid, Vec<ImpactRow>)],
) -> Vec<CompareNodeRow> {
    let mut nodes: Vec<CompareNodeRow> = node_rows_map
        .into_values()
        .map(|mut r| {
            let mut min_v = f64::INFINITY;
            let mut max_v = f64::NEG_INFINITY;
            for (sc_id, _) in impacts_by_scenario {
                let key = sc_id.to_string();
                let val = r
                    .values
                    .get(&key)
                    .map(|c| c.after_val)
                    .or(r.before_val)
                    .unwrap_or(0.0);
                min_v = min_v.min(val);
                max_v = max_v.max(val);
            }
            r.max_spread = if min_v.is_finite() && max_v.is_finite() {
                (max_v - min_v).abs()
            } else {
                0.0
            };
            r
        })
        .collect();
    nodes.sort_by(|a, b| b.max_spread.total_cmp(&a.max_spread));
    nodes.truncate(200);
    nodes
}

async fn sim_ask_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<SimAskRequest>,
) -> Result<Json<SimAskResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let question = req.question.trim();
    if question.is_empty() {
        return Err(HttpError::from(AppError::bad(
            "INVALID_ARGUMENT",
            "question cannot be empty",
        )));
    }

    let _sb = state
        .store
        .get_sandbox(&principal.tenant_id, req.sandbox_id)
        .await?
        .ok_or(AppError::NotFound("SANDBOX_NOT_FOUND"))?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;

    let intent =
        classify_sim_ask_intent(&state.ai, question, req.scenario_id.is_some(), true).await?;

    match intent {
        SimAskIntent::QueryResult => {
            handle_sim_ask_query_result(&state, &principal, &req, question, &policy_ctx).await
        }
        SimAskIntent::WhatIf => {
            handle_sim_ask_what_if(&state, &principal, &req, question, &policy_ctx).await
        }
    }
}

async fn handle_sim_ask_query_result(
    state: &AppState,
    principal: &AuthPrincipal,
    req: &SimAskRequest,
    question: &str,
    policy_ctx: &PolicyContext,
) -> Result<Json<SimAskResponse>, HttpError> {
    let scenario_id = if let Some(id) = req.scenario_id {
        id
    } else {
        let list = state
            .store
            .list_scenarios(&principal.tenant_id, req.sandbox_id)
            .await?;
        list.into_iter()
            .find(|s| s.status == ScenarioStatus::Succeeded)
            .map(|s| s.id)
            .ok_or_else(|| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    "no completed scenario found for QUERY_RESULT",
                )
            })?
    };
    let mut impacts = state
        .store
        .list_all_scenario_impacts(&principal.tenant_id, scenario_id)
        .await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() {
        let visible = state
            .store
            .filter_sandbox_visible_node_ids(&principal.tenant_id, req.sandbox_id, policy_ctx)
            .await?;
        impacts.retain(|r| visible.contains(&r.node_id));
    }
    let ask_resp =
        query_impacts_with_nl2sql(&state.ai, &principal.tenant_id, &impacts, question).await?;

    record_sim_ask_audit(state, principal, scenario_id, question, &ask_resp.sql).await;

    Ok(Json(SimAskResponse {
        intent: SimAskIntent::QueryResult,
        scenario_id: Some(scenario_id),
        answer: ask_resp.answer,
        sql: (!ask_resp.sql.is_empty()).then_some(ask_resp.sql),
        columns: ask_resp.columns,
        rows: ask_resp.rows,
        need_clarification: ask_resp.need_clarification,
        candidates: Vec::new(),
        impacts: Vec::new(),
        kpis: Vec::new(),
    }))
}

async fn handle_sim_ask_what_if(
    state: &AppState,
    principal: &AuthPrincipal,
    req: &SimAskRequest,
    question: &str,
    policy_ctx: &PolicyContext,
) -> Result<Json<SimAskResponse>, HttpError> {
    let (graph, _) = load_governed_sim_graph(
        &state.store,
        &principal.tenant_id,
        req.sandbox_id,
        policy_ctx,
    )
    .await?;
    let hints = graph_hint_nodes(&graph);
    let extracted = extract_what_if_interventions(&state.ai, question, &hints).await?;

    if extracted.need_clarification {
        return Ok(Json(clarify_response(
            extracted
                .clarification_message
                .unwrap_or_else(|| String::from("请补充需要推演的目标节点与指标变动。")),
            Vec::new(),
        )));
    }

    let mut extra_hits = Vec::new();
    for item in &extracted.interventions {
        if let Ok(resp) = state
            .store
            .search_nodes(
                &principal.tenant_id,
                NodeSearchQuery {
                    q: item.node_query.clone(),
                    type_name: None,
                    k: 10,
                },
                None,
            )
            .await
        {
            extra_hits.extend(resp.items);
        }
    }
    filter_and_mask_sim_search_hits(state, &principal.tenant_id, &mut extra_hits, policy_ctx)
        .await?;

    let actions =
        match disambiguate_what_if_actions(&extracted.interventions, &graph.nodes, &extra_hits) {
            DisambiguationOutcome::Ambiguous { candidates, reason } => {
                return Ok(Json(clarify_response(reason, candidates)));
            }
            DisambiguationOutcome::NotFound { reason } => {
                return Ok(Json(clarify_response(reason, Vec::new())));
            }
            DisambiguationOutcome::Resolved(actions) => actions,
        };

    execute_what_if_actions(state, principal, req.sandbox_id, question, graph, actions).await
}

fn clarify_response(
    answer: String,
    candidates: Vec<axpoint_domain::sim::SimAskCandidate>,
) -> SimAskResponse {
    SimAskResponse {
        intent: SimAskIntent::WhatIf,
        scenario_id: None,
        answer,
        sql: None,
        columns: Vec::new(),
        rows: Vec::new(),
        need_clarification: true,
        candidates,
        impacts: Vec::new(),
        kpis: Vec::new(),
    }
}

async fn execute_what_if_actions(
    state: &AppState,
    principal: &AuthPrincipal,
    sandbox_id: Uuid,
    question: &str,
    graph: SimGraph,
    actions: Vec<AxSimAction>,
) -> Result<Json<SimAskResponse>, HttpError> {
    let sim_params = SimParams::default();
    let overlay_hash = state
        .store
        .compute_overlay_hash(&principal.tenant_id, sandbox_id)
        .await?;
    let sc_name = format!(
        "ask-{}",
        Uuid::new_v4().to_string().get(..8).unwrap_or("whatif")
    );

    let scenario_rec = if principal.has(Permission::SimRun) {
        let created = state
            .store
            .create_scenario_record(
                &principal.tenant_id,
                sandbox_id,
                Some(principal.user_id),
                &overlay_hash,
                CreateScenarioRequest {
                    name: sc_name,
                    description: String::new(),
                    actions: actions.clone(),
                    scheduled_events: Vec::new(),
                    goal_constraints: Vec::new(),
                    params: sim_params.clone(),
                },
            )
            .await?;
        Some(created)
    } else {
        None
    };

    let solved = tokio::task::spawn_blocking(move || {
        axpoint_domain::sim::uncertainty::solve(&graph, &actions, &sim_params)
    })
    .await
    .map_err(|err| AppError::Database(format!("what-if solve join failed: {err}")))??;

    if let Some(ref sc) = scenario_rec {
        let _saved = state
            .store
            .save_scenario_success(&principal.tenant_id, sc.id, &overlay_hash, &solved)
            .await?;
    }

    let answer = summarize_what_if_result(&state.ai, question, &solved).await;
    let top_impacts: Vec<ImpactRow> = solved.impacts.into_iter().take(50).collect();

    Ok(Json(SimAskResponse {
        intent: SimAskIntent::WhatIf,
        scenario_id: scenario_rec.map(|s| s.id),
        answer,
        sql: None,
        columns: Vec::new(),
        rows: Vec::new(),
        need_clarification: false,
        candidates: Vec::new(),
        impacts: top_impacts,
        kpis: solved.kpis,
    }))
}

async fn record_sim_ask_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    scenario_id: Uuid,
    question: &str,
    sql: &str,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: Some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::AskData,
        resource_ref: format!("axsim_scenario:{scenario_id}"),
        status_code: 200,
        client_ip: String::new(),
        trace_id: String::new(),
        detail_json: serde_json::json!({
            "source": "axsim.ask",
            "scenario_id": scenario_id,
            "question": question,
            "sql": sql,
        }),
        created_at: chrono::Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "axsim ask audit flush failed");
    }
}

async fn create_decision_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(scenario_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateDecisionRequest>,
) -> Result<Json<DecisionRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() {
        let sc = state
            .store
            .get_scenario(&principal.tenant_id, scenario_id)
            .await?
            .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
        let visible = state
            .store
            .filter_sandbox_visible_node_ids(&principal.tenant_id, sc.sandbox_id, &policy_ctx)
            .await?;
        let hidden = match req.changes {
            Some(ref ch) => ch.iter().any(|c| !visible.contains(&c.node_id)),
            None => sc
                .actions
                .iter()
                .any(|a| !visible.contains(&a.target_node_id)),
        };
        if hidden {
            return Err(HttpError::from(AppError::NotFound("NODE_NOT_FOUND")));
        }
    }
    let decision = state
        .store
        .create_decision(
            &principal.tenant_id,
            scenario_id,
            Some(principal.user_id),
            req,
        )
        .await?;
    Ok(Json(decision))
}

async fn get_decision_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(decision_id): ApiPath<Uuid>,
) -> Result<Json<DecisionRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    let decision = state
        .store
        .get_decision(&principal.tenant_id, decision_id)
        .await?
        .ok_or(AppError::NotFound("DECISION_NOT_FOUND"))?;
    ensure_decision_nodes_visible(&state.store, &principal.tenant_id, &decision, &policy_ctx)
        .await?;
    Ok(Json(decision))
}

async fn rebase_decision_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(decision_id): ApiPath<Uuid>,
) -> Result<Json<DecisionRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    let policy_ctx = build_sim_policy_ctx(&state, &principal, Some(&headers)).await?;
    if !policy_ctx.bypass_row_policies && !policy_ctx.node_types.is_empty() {
        let existing = state
            .store
            .get_decision(&principal.tenant_id, decision_id)
            .await?
            .ok_or(AppError::NotFound("DECISION_NOT_FOUND"))?;
        ensure_decision_nodes_visible(&state.store, &principal.tenant_id, &existing, &policy_ctx)
            .await?;
    }
    let rebased = state
        .store
        .rebase_decision(&principal.tenant_id, decision_id, Some(principal.user_id))
        .await?;
    Ok(Json(rebased))
}

/// [`SimExecutionPort`] implementation bridging `AxFlow` (`CAUSAL_SIM`) and `AxAgent` (`run_simulation`)
/// onto the Step 10 `AxSim` solver and `CoW` sandbox store (`09-STEP10` §10.10).
pub(crate) struct ServerSimPort {
    pub(crate) store: PgStore,
}

async fn build_tool_sim_policy_ctx(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
) -> Result<PolicyContext, AppError> {
    if user_id.is_nil() {
        return Ok(PolicyContext {
            bypass_row_policies: true,
            ..PolicyContext::default()
        });
    }
    let rec = store
        .load_principal(tenant_id, user_id)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
    let auth_principal = AuthPrincipal {
        user_id,
        tenant_id: String::from(tenant_id),
        username: rec.username,
        dept_id: rec.dept_id,
        roles: rec.roles,
        permissions: rec.permissions,
        token_kind: TokenKind::Access,
        token_profile: axpoint_domain::enums::TokenProfile::General,
        web_base_url: None,
    };
    let cedar = axpoint_security::cedar_authz::CedarAuthorizer::new()?;
    axpoint_server::policy::build_policy_context_for_store(
        store,
        &cedar,
        &auth_principal,
        false,
        "127.0.0.1",
    )
    .await
}

async fn resolve_or_create_tool_sandbox(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    requested_id: Option<Uuid>,
    scenario_name: Option<&str>,
    actions: &[AxSimAction],
    policy_ctx: &PolicyContext,
) -> Result<Uuid, AppError> {
    if let Some(id) = requested_id {
        return Ok(id);
    }
    let existing = store.list_sandboxes(tenant_id).await?;
    if let Some(first) = existing.first() {
        return Ok(first.id);
    }
    let root_ids: Vec<Uuid> = actions.iter().map(|a| a.target_node_id).collect();
    let created = store
        .create_sandbox_governed(
            tenant_id,
            Some(user_id),
            CreateAxSandboxRequest {
                name: String::from(scenario_name.unwrap_or("auto-sim-sandbox")),
                scope: Some(SandboxScope {
                    root_node_ids: root_ids,
                    depth: 6,
                    ..SandboxScope::default()
                }),
            },
            policy_ctx,
        )
        .await?;
    Ok(created.id)
}

impl SimExecutionPort for ServerSimPort {
    fn run_simulation_tool<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: Uuid,
        input: SimRunToolInput,
    ) -> BoxFuture<'a, SimRunToolOutput> {
        Box::pin(async move {
            let (actions, mut params) = parse_tool_interventions(&input.interventions)?;
            if let Some(hops) = input.max_hops {
                params.max_hops = Some(hops);
            }
            if actions.is_empty() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "run_simulation requires at least one intervention",
                ));
            }

            let policy_ctx = build_tool_sim_policy_ctx(&self.store, tenant_id, user_id).await?;
            let sandbox_id = resolve_or_create_tool_sandbox(
                &self.store,
                tenant_id,
                user_id,
                input.sandbox_id,
                input.scenario_name.as_deref(),
                &actions,
                &policy_ctx,
            )
            .await?;

            let (graph, visible) =
                load_governed_sim_graph(&self.store, tenant_id, sandbox_id, &policy_ctx).await?;
            ensure_actions_target_visible(&actions, visible.as_ref())?;
            let overlay_hash = self
                .store
                .compute_overlay_hash(tenant_id, sandbox_id)
                .await?;
            let sc_name = input.scenario_name.unwrap_or_else(|| {
                let id_str = Uuid::new_v4().to_string();
                format!("tool-{}", id_str.get(..8).unwrap_or("sim"))
            });
            let scenario = self
                .store
                .create_scenario_record(
                    tenant_id,
                    sandbox_id,
                    Some(user_id),
                    &overlay_hash,
                    CreateScenarioRequest {
                        name: sc_name,
                        description: String::new(),
                        actions: actions.clone(),
                        scheduled_events: Vec::new(),
                        goal_constraints: Vec::new(),
                        params: params.clone(),
                    },
                )
                .await?;

            let solved = execute_and_persist_scenario(
                &self.store,
                tenant_id,
                scenario.id,
                graph,
                actions,
                params,
                &overlay_hash,
            )
            .await?;

            let mut top_page = self
                .store
                .list_scenario_impacts(
                    tenant_id,
                    solved.id,
                    ImpactListQuery {
                        limit: Some(50),
                        ..ImpactListQuery::default()
                    },
                )
                .await?;
            if let Some(ref vis) = visible {
                top_page.items.retain(|r| vis.contains(&r.node_id));
            }

            let max_abs_delta = top_page
                .items
                .iter()
                .filter_map(|r| r.delta)
                .map(f64::abs)
                .fold(0.0_f64, f64::max);
            let affected_count = solved.result.as_ref().map_or(0, |s| s.affected_count);
            let kpis = solved.result.as_ref().map_or_default(|s| s.kpis.clone());
            let warnings = solved
                .result
                .as_ref()
                .map_or_default(|s| s.warnings.clone());

            Ok(SimRunToolOutput {
                sandbox_id,
                scenario_id: Some(solved.id),
                summary: serde_json::json!({
                    "affected_count": affected_count,
                    "max_abs_delta": max_abs_delta,
                    "kpis": kpis,
                    "warnings": warnings,
                    "impacts": top_page.items,
                }),
            })
        })
    }
}

fn parse_tool_interventions(
    raw: &serde_json::Value,
) -> Result<(Vec<AxSimAction>, SimParams), AppError> {
    if let Some(arr) = raw.as_array() {
        let mut actions = Vec::with_capacity(arr.len());
        for entry in arr {
            actions.push(parse_single_tool_action(entry)?);
        }
        return Ok((actions, SimParams::default()));
    }

    if let Some(obj) = raw.as_object() {
        let params: SimParams = match obj.get("params") {
            Some(p) if !p.is_null() => serde_json::from_value(p.clone()).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid sim params: {err}"))
            })?,
            _ => SimParams::default(),
        };
        let arr = obj
            .get("actions")
            .or_else(|| obj.get("interventions"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    "interventions object must contain an 'actions' array",
                )
            })?;
        let mut actions = Vec::with_capacity(arr.len());
        for entry in arr {
            actions.push(parse_single_tool_action(entry)?);
        }
        return Ok((actions, params));
    }

    Err(AppError::bad(
        "INVALID_ARGUMENT",
        "interventions must be a JSON array or object",
    ))
}

fn parse_single_tool_action(val: &serde_json::Value) -> Result<AxSimAction, AppError> {
    if let Ok(action) = serde_json::from_value::<AxSimAction>(val.clone()) {
        return Ok(action);
    }

    let target_str = val
        .get("target_node_id")
        .or_else(|| val.get("node_id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "intervention missing target_node_id/node_id UUID",
            )
        })?;
    let target_node_id = Uuid::parse_str(target_str.trim()).map_err(|_| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid intervention node UUID: {target_str}"),
        )
    })?;

    let metric_key = val
        .get("metric_key")
        .or_else(|| val.get("metric"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "intervention missing metric_key"))?
        .to_owned();

    let delta_ratio = val.get("delta_ratio").and_then(serde_json::Value::as_f64);
    let mode = match val.get("mode").and_then(serde_json::Value::as_str) {
        Some("ADD") => InterventionMode::Add,
        Some("ABSOLUTE") => InterventionMode::Absolute,
        _ => InterventionMode::Ratio,
    };
    let value = val
        .get("value")
        .and_then(serde_json::Value::as_f64)
        .or(delta_ratio)
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "intervention missing numeric value or delta_ratio",
            )
        })?;
    let at_step = val
        .get("at_step")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);

    Ok(AxSimAction {
        target_node_id,
        metric_key,
        mode: Some(mode),
        value: Some(value),
        delta_ratio,
        at_step,
    })
}

/// Outbox subscriber handling `approval.decided` events originating from `SIM_APPLY` decision gates (`09-STEP10` §10.8).
pub(crate) struct SimApprovalDecidedSubscriber {
    pub(crate) store: PgStore,
}

impl OutboxSubscriber for SimApprovalDecidedSubscriber {
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != OutboxTopic::ApprovalDecided {
                return Ok(());
            }

            let Some(approval_id_str) = event
                .payload
                .get("approval_id")
                .and_then(serde_json::Value::as_str)
            else {
                return Ok(());
            };
            let Ok(approval_id) = Uuid::parse_str(approval_id_str) else {
                return Ok(());
            };

            let Some(approval) = self
                .store
                .get_approval(&event.tenant_id, approval_id)
                .await?
            else {
                return Ok(());
            };

            if approval.source_kind != ApprovalSourceKind::SimApply {
                return Ok(());
            }

            let Ok(decision_id) = Uuid::parse_str(approval.source_ref.trim()) else {
                return Ok(());
            };

            let approved = approval.status == ApprovalStatus::Approved;
            let decided = self
                .store
                .apply_or_reject_decision(
                    &event.tenant_id,
                    decision_id,
                    approval.decided_by,
                    approved,
                )
                .await?;

            if decided.status == DecisionStatus::Applied
                && let Some(flow_id) = decided.trigger_flow_id
            {
                let payload = serde_json::to_value(&decided)
                    .unwrap_or_else(|_| serde_json::json!({ "decision_id": decided.id }));
                let _run = self
                    .store
                    .start_flow_run(
                        &event.tenant_id,
                        StartRunParams {
                            flow_id,
                            environment: Environment::Prod,
                            explicit_version: None,
                            trigger_source: TriggerType::SimApply,
                            trigger_payload: payload,
                            idempotency_key: Some(format!("decision:{}", decided.id)),
                            started_by: approval.decided_by,
                        },
                    )
                    .await?;
            }

            Ok(())
        })
    }
}
