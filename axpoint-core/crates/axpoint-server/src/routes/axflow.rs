use axpoint_domain::topology::{AxFlowDef, AxFlowRunResult, CreateAxFlowRequest};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::state::{AppState, HttpError, TenantQuery, default_tenant};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/axflow/flows",
            get(list_flows_handler).post(create_flow_handler),
        )
        .route("/api/axflow/flows/{id}/run", post(run_flow_handler))
}

async fn list_flows_handler(
    State(state): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Vec<AxFlowDef>>, HttpError> {
    let items = state.store.list_ax_flows(&q.tenant_id).await?;
    Ok(Json(items))
}

async fn create_flow_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateAxFlowRequest>,
) -> Result<Json<AxFlowDef>, HttpError> {
    let tenant_id = default_tenant();
    let flow = state.store.create_ax_flow(&tenant_id, req).await?;
    Ok(Json(flow))
}

async fn run_flow_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<AxFlowRunResult>, HttpError> {
    let res = state.store.run_ax_flow(&q.tenant_id, id).await?;
    Ok(Json(res))
}
