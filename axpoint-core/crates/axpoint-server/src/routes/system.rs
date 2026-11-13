//! System health, statistics, and hypergraph topology overview routes.

use axpoint_domain::enums::Permission;
use axpoint_domain::{SystemStats, TopologyGraph};
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::state::{AppState, HealthResponse, HttpError};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health_handler))
        .route("/stats", get(stats_handler))
        .route("/graph/topology", get(topology_handler))
}

async fn health_handler(State(state): State<AppState>) -> Result<Json<HealthResponse>, HttpError> {
    state.store.ping().await?;
    Ok(Json(HealthResponse {
        status: "ok",
        service: "axpoint-core",
        embedding_model: state.ai.embedding_model_name().to_owned(),
        embedding_dim: state.ai.current_embedding_dim(),
    }))
}

async fn stats_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<SystemStats>, HttpError> {
    principal.require(Permission::DataRead)?;
    let metrics = state.store.get_stats(&principal.tenant_id).await?;
    Ok(Json(metrics))
}

async fn topology_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<TopologyGraph>, HttpError> {
    principal.require(Permission::DataRead)?;
    let graph = state.store.get_topology(&principal.tenant_id, 60).await?;
    Ok(Json(graph))
}
