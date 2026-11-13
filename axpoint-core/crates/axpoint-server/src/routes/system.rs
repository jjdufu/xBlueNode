use axpoint_domain::{SystemStats, TopologyGraph};
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::state::{AppState, HealthResponse, HttpError, TenantQuery};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health_handler))
        .route("/api/stats", get(stats_handler))
        .route("/api/graph/topology", get(topology_handler))
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
    Query(q): Query<TenantQuery>,
) -> Result<Json<SystemStats>, HttpError> {
    let metrics = state.store.get_stats(&q.tenant_id).await?;
    Ok(Json(metrics))
}

async fn topology_handler(
    State(state): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<TopologyGraph>, HttpError> {
    let graph = state.store.get_topology(&q.tenant_id, 60).await?;
    Ok(Json(graph))
}
