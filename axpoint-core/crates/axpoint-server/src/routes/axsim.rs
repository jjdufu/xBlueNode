//! `AxSim` sandbox cloning, `CoW` overlay editing, causal simulation, Ask, compare, and decision routes (`/api/axsim/*`).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::agent::{SimRunToolInput, SimRunToolOutput};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    AuditAction, Environment, InterventionMode, OutboxTopic, Permission, TriggerType,
};
use axpoint_domain::flow::{ApprovalSourceKind, ApprovalStatus, StartRunParams};
use axpoint_domain::graph::NodeSearchQuery;
use axpoint_domain::platform::{AuditRecord, BoxFuture, OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{
    FlowRepository, GraphStorePort, SimExecutionPort, SimRepository, SimVisibilityPort as _,
};
use axpoint_domain::sim::{
    CheckpointsResponse, CompareKpiCell, CompareKpiRow, CompareNodeCell, CompareNodeRow,
    CompareScenarioColumn, CompareScenariosQuery, CompareScenariosResponse, CreateDecisionRequest,
    CreateOverlayLinkRequest, CreateOverlayNodeRequest, CreateScenarioRequest, DecisionRecord,
    DecisionStatus, ImpactListQuery, ImpactRow, PatchOverlayLinkRequest, PatchOverlayNodeRequest,
    RefreshBaseResponse, SandboxGraphLink, SandboxGraphNode, SandboxGraphResponse, SandboxScope,
    ScenarioRecord, ScenarioStatus, ScopePreflightReport, ScopePreflightRequest, SimAskIntent,
    SimAskRequest, SimAskResponse, SimGraph, SimParams, evaluate_scope_preflight,
};
use axpoint_domain::topology::{
    AxSandboxRecord, AxSimAction, AxSimResult, CreateAxSandboxRequest, SimulateAxSandboxRequest,
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
    principal: AuthPrincipal,
) -> Result<Json<Vec<AxSandboxRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state.store.list_sandboxes(&principal.tenant_id).await?;
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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AxSandboxRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let sandbox = state
        .store
        .get_sandbox(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SANDBOX_NOT_FOUND"))?;
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

async fn simulate_sandbox_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<SimulateAxSandboxRequest>,
) -> Result<Json<AxSimResult>, HttpError> {
    principal.require(Permission::SimRun)?;
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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RefreshBaseResponse>, HttpError> {
    principal.require(Permission::SimRun)?;
    let refreshed = state
        .store
        .refresh_sandbox_base(&principal.tenant_id, id)
        .await?;
    Ok(Json(refreshed))
}

async fn create_overlay_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateOverlayNodeRequest>,
) -> Result<Json<SandboxGraphNode>, HttpError> {
    principal.require(Permission::SimRun)?;
    let node = state
        .store
        .create_overlay_node(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(node))
}

async fn patch_overlay_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, nid)): ApiPath<(Uuid, Uuid)>,
    ApiJson(req): ApiJson<PatchOverlayNodeRequest>,
) -> Result<Json<SandboxGraphNode>, HttpError> {
    principal.require(Permission::SimRun)?;
    let node = state
        .store
        .patch_overlay_node(&principal.tenant_id, id, nid, req)
        .await?;
    Ok(Json(node))
}

async fn delete_overlay_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, nid)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateOverlayLinkRequest>,
) -> Result<Json<SandboxGraphLink>, HttpError> {
    principal.require(Permission::SimRun)?;
    let link = state
        .store
        .create_overlay_link(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(link))
}

async fn patch_overlay_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, lid)): ApiPath<(Uuid, Uuid)>,
    ApiJson(req): ApiJson<PatchOverlayLinkRequest>,
) -> Result<Json<SandboxGraphLink>, HttpError> {
    principal.require(Permission::SimRun)?;
    let link = state
        .store
        .patch_overlay_link(&principal.tenant_id, id, lid, req)
        .await?;
    Ok(Json(link))
}

async fn delete_overlay_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, lid)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<DeleteStatus>, HttpError> {
    principal.require(Permission::SimRun)?;
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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Vec<ScenarioRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state.store.list_scenarios(&principal.tenant_id, id).await?;
    Ok(Json(items))
}

async fn create_scenario_handler(
    State(state): State<AppState>,
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

    let overlay_hash = state
        .store
        .compute_overlay_hash(&principal.tenant_id, id)
        .await?;
    let graph = state.store.load_graph(&principal.tenant_id, id).await?;

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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ScenarioRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let scenario = state
        .store
        .get_scenario(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ScenarioRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    let existing = state
        .store
        .get_scenario(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
    let overlay_hash = state
        .store
        .compute_overlay_hash(&principal.tenant_id, existing.sandbox_id)
        .await?;
    let graph = state
        .store
        .load_graph(&principal.tenant_id, existing.sandbox_id)
        .await?;
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
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ImpactListQuery>,
) -> Result<Json<PaginatedList<ImpactRow>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let page = state
        .store
        .list_scenario_impacts(&principal.tenant_id, id, query)
        .await?;
    Ok(Json(page))
}

async fn list_scenario_checkpoints_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<CheckpointsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_scenario_checkpoints(&principal.tenant_id, id)
        .await?;
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
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<CompareScenariosQuery>,
) -> Result<Json<CompareScenariosResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
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

    let mut impacts_by_scenario: Vec<(Uuid, Vec<ImpactRow>)> = Vec::with_capacity(scenarios.len());
    for sc in &scenarios {
        let rows = state
            .store
            .list_all_scenario_impacts(&principal.tenant_id, sc.id)
            .await?;
        impacts_by_scenario.push((sc.id, rows));
    }

    let graph = state
        .store
        .load_graph(&principal.tenant_id, first_sandbox)
        .await?;
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

    let intent =
        classify_sim_ask_intent(&state.ai, question, req.scenario_id.is_some(), true).await?;

    match intent {
        SimAskIntent::QueryResult => {
            handle_sim_ask_query_result(&state, &principal, &req, question).await
        }
        SimAskIntent::WhatIf => handle_sim_ask_what_if(&state, &principal, &req, question).await,
    }
}

async fn handle_sim_ask_query_result(
    state: &AppState,
    principal: &AuthPrincipal,
    req: &SimAskRequest,
    question: &str,
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
    let impacts = state
        .store
        .list_all_scenario_impacts(&principal.tenant_id, scenario_id)
        .await?;
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
) -> Result<Json<SimAskResponse>, HttpError> {
    let graph = state
        .store
        .load_graph(&principal.tenant_id, req.sandbox_id)
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
    principal: AuthPrincipal,
    ApiPath(scenario_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateDecisionRequest>,
) -> Result<Json<DecisionRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
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
    principal: AuthPrincipal,
    ApiPath(decision_id): ApiPath<Uuid>,
) -> Result<Json<DecisionRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let decision = state
        .store
        .get_decision(&principal.tenant_id, decision_id)
        .await?
        .ok_or(AppError::NotFound("DECISION_NOT_FOUND"))?;
    Ok(Json(decision))
}

async fn rebase_decision_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(decision_id): ApiPath<Uuid>,
) -> Result<Json<DecisionRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
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

async fn resolve_or_create_tool_sandbox(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    requested_id: Option<Uuid>,
    scenario_name: Option<&str>,
    actions: &[AxSimAction],
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
        .create_sandbox(
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

            let sandbox_id = resolve_or_create_tool_sandbox(
                &self.store,
                tenant_id,
                user_id,
                input.sandbox_id,
                input.scenario_name.as_deref(),
                &actions,
            )
            .await?;

            let graph = self.store.load_graph(tenant_id, sandbox_id).await?;
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

            let top_page = self
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
