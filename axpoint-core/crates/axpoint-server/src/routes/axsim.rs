//! `AxSim` sandbox cloning and causal simulation routes (`/api/axsim/*`).

use axpoint_domain::enums::Permission;
use axpoint_domain::topology::{
    AxSandboxRecord, AxSimResult, CreateAxSandboxRequest, SimulateAxSandboxRequest,
};
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, AppState, HttpError};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/axsim/sandboxes",
            get(list_sandboxes_handler).post(create_sandbox_handler),
        )
        .route(
            "/axsim/sandboxes/{id}/simulate",
            post(simulate_sandbox_handler),
        )
}

async fn list_sandboxes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<AxSandboxRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state.store.list_ax_sandboxes(&principal.tenant_id).await?;
    Ok(Json(items))
}

async fn create_sandbox_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateAxSandboxRequest>,
) -> Result<Json<AxSandboxRecord>, HttpError> {
    principal.require(Permission::SimRun)?;
    let sb = state
        .store
        .create_ax_sandbox(&principal.tenant_id, req)
        .await?;
    Ok(Json(sb))
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
        .simulate_ax_sandbox(&principal.tenant_id, id, req.actions)
        .await?;
    Ok(Json(res))
}
