//! `AxFlow` pipeline management and execution routes (`/api/axflow/*`).

use axpoint_domain::enums::Permission;
use axpoint_domain::topology::{AxFlowDef, AxFlowRunResult, CreateAxFlowRequest};
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, AppState, HttpError};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/axflow/flows",
            get(list_flows_handler).post(create_flow_handler),
        )
        .route("/axflow/flows/{id}/run", post(run_flow_handler))
}

async fn list_flows_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<AxFlowDef>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state.store.list_ax_flows(&principal.tenant_id).await?;
    Ok(Json(items))
}

async fn create_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateAxFlowRequest>,
) -> Result<Json<AxFlowDef>, HttpError> {
    principal.require(Permission::FlowEdit)?;
    let flow = state
        .store
        .create_ax_flow(&principal.tenant_id, req)
        .await?;
    Ok(Json(flow))
}

async fn run_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AxFlowRunResult>, HttpError> {
    principal.require(Permission::FlowRun)?;
    let res = state.store.run_ax_flow(&principal.tenant_id, id).await?;
    Ok(Json(res))
}
