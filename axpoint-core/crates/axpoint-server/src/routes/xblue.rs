//! `xBlueStudio` schema and runtime `BlueNode` routes (`/api/xblue/*`).

use axpoint_domain::enums::Permission;
use axpoint_domain::topology::{
    AxSimSnapshot, BlueNode, SaveXBlueSchemaRequest, UpsertBlueNodeRequest, XBlueSchema,
};
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::state::{ApiJson, AppState, HttpError};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/xblue/schema",
            get(get_schema_handler).post(save_schema_handler),
        )
        .route(
            "/xblue/nodes",
            get(list_nodes_handler).post(upsert_node_handler),
        )
}

async fn get_schema_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<XBlueSchema>, HttpError> {
    principal.require(Permission::DataRead)?;
    let schema = state.store.get_xblue_schema(&principal.tenant_id).await?;
    Ok(Json(schema))
}

async fn save_schema_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<SaveXBlueSchemaRequest>,
) -> Result<Json<XBlueSchema>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let schema = state
        .store
        .save_xblue_schema(&principal.tenant_id, req)
        .await?;
    Ok(Json(schema))
}

async fn list_nodes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<AxSimSnapshot>, HttpError> {
    principal.require(Permission::DataRead)?;
    let snap = state
        .store
        .get_runtime_snapshot(&principal.tenant_id)
        .await?;
    Ok(Json(snap))
}

async fn upsert_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<UpsertBlueNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let node = state
        .store
        .upsert_blue_node(&principal.tenant_id, req)
        .await?;
    Ok(Json(node))
}
