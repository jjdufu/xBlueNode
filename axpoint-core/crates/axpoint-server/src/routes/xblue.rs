use axpoint_domain::topology::{
    AxSimSnapshot, BlueNode, SaveXBlueSchemaRequest, UpsertBlueNodeRequest, XBlueSchema,
};
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::state::{AppState, HttpError, TenantQuery};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/xblue/schema",
            get(get_schema_handler).post(save_schema_handler),
        )
        .route(
            "/api/xblue/nodes",
            get(list_nodes_handler).post(upsert_node_handler),
        )
}

async fn get_schema_handler(
    State(state): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<XBlueSchema>, HttpError> {
    let schema = state.store.get_xblue_schema(&q.tenant_id).await?;
    Ok(Json(schema))
}

async fn save_schema_handler(
    State(state): State<AppState>,
    Json(req): Json<SaveXBlueSchemaRequest>,
) -> Result<Json<XBlueSchema>, HttpError> {
    let schema = state.store.save_xblue_schema(req).await?;
    Ok(Json(schema))
}

async fn list_nodes_handler(
    State(state): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<AxSimSnapshot>, HttpError> {
    let snap = state.store.get_runtime_snapshot(&q.tenant_id).await?;
    Ok(Json(snap))
}

async fn upsert_node_handler(
    State(state): State<AppState>,
    Json(req): Json<UpsertBlueNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    let node = state.store.upsert_blue_node(req).await?;
    Ok(Json(node))
}
