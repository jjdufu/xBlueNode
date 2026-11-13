use axpoint_domain::topology::{
    AxSandboxRecord, AxSimResult, CreateAxSandboxRequest, SimulateAxSandboxRequest,
};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::state::{AppState, HttpError, TenantQuery};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/axsim/sandboxes",
            get(list_sandboxes_handler).post(create_sandbox_handler),
        )
        .route(
            "/api/axsim/sandboxes/{id}/simulate",
            post(simulate_sandbox_handler),
        )
}

async fn list_sandboxes_handler(
    State(state): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Vec<AxSandboxRecord>>, HttpError> {
    let items = state.store.list_ax_sandboxes(&q.tenant_id).await?;
    Ok(Json(items))
}

async fn create_sandbox_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateAxSandboxRequest>,
) -> Result<Json<AxSandboxRecord>, HttpError> {
    let sb = state.store.create_ax_sandbox(req).await?;
    Ok(Json(sb))
}

async fn simulate_sandbox_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<TenantQuery>,
    Json(req): Json<SimulateAxSandboxRequest>,
) -> Result<Json<AxSimResult>, HttpError> {
    let res = state
        .store
        .simulate_ax_sandbox(&q.tenant_id, id, req.actions)
        .await?;
    Ok(Json(res))
}
