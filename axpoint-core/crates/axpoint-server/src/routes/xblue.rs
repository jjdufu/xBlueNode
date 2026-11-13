//! `xBlueStudio` schema and `AxGraph` runtime node, link, time-series, entity-link, and topology routes (`/api/xblue/*`).

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::enums::{GraphAlgo, Permission};
use axpoint_domain::graph::{
    AlgoResultsQuery, AlgoResultsResponse, AlgoRunRequest, AlgoRunResponse, BulkNodesRequest,
    BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery, LinkRecord,
    MetricCatalogEntry, MetricSeriesPoint, NeighborQuery, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchQuery, NodeSearchResponse, PatchNodeRequest,
    PathQuery, PathResponse, ResolveEntitiesRequest, ResolveEntitiesResponse, SubgraphQuery,
    SubgraphResponse, UpsertLinkRequest, WindowQuery, WindowQueryResponse, WriteMetricsRequest,
    WriteMetricsResponse,
};
use axpoint_domain::ports::{GraphQueryPort as _, GraphStorePort as _, MetricStorePort as _};
use axpoint_domain::topology::{
    BlueNode, SaveXBlueSchemaRequest, UpsertBlueNodeRequest, XBlueSchema,
};
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};

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
        .route("/xblue/nodes/bulk", post(bulk_upsert_nodes_handler))
        .route("/xblue/nodes/search", get(search_nodes_handler))
        .route(
            "/xblue/nodes/{id}",
            get(get_node_handler)
                .patch(patch_node_handler)
                .delete(delete_node_handler),
        )
        .route(
            "/xblue/nodes/{id}/documents",
            get(list_node_documents_handler),
        )
        .route(
            "/xblue/nodes/{id}/metrics/{metric}",
            get(query_node_metrics_handler),
        )
        .route(
            "/xblue/links",
            get(list_links_handler).post(upsert_link_handler),
        )
        .route("/xblue/links/{id}", delete(delete_link_handler))
        .route("/xblue/metrics/write", post(write_metrics_handler))
        .route("/xblue/metrics/window", post(query_window_handler))
        .route("/xblue/metrics/catalog", get(list_metric_catalog_handler))
        .route("/xblue/graph/neighbors", get(query_neighbors_handler))
        .route("/xblue/graph/path", get(query_path_handler))
        .route("/xblue/graph/subgraph", get(query_subgraph_handler))
        .route(
            "/xblue/graph/algorithms/{algo}",
            post(run_algorithm_handler),
        )
        .route(
            "/xblue/graph/algorithms/{algo}/results",
            get(list_algorithm_results_handler),
        )
        .route("/xblue/entities/resolve", post(resolve_entities_handler))
        .route(
            "/xblue/entities/candidates",
            get(list_entity_candidates_handler),
        )
        .route(
            "/xblue/entities/links/{entity_id}/{node_id}/confirm",
            post(confirm_entity_link_handler),
        )
        .route(
            "/xblue/entities/links/{entity_id}/{node_id}/reject",
            post(reject_entity_link_handler),
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
    ApiQuery(query): ApiQuery<NodeListQuery>,
) -> Result<Json<NodeListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state.store.list_nodes(&principal.tenant_id, query).await?;
    Ok(Json(resp))
}

async fn upsert_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<UpsertBlueNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let node = state.store.upsert_node(&principal.tenant_id, req).await?;
    let _ = state
        .store
        .resolve_entity_links(
            &principal.tenant_id,
            ResolveEntitiesRequest {
                node_id: Some(node.id),
            },
        )
        .await;
    Ok(Json(node))
}

async fn bulk_upsert_nodes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<BulkNodesRequest>,
) -> Result<Json<BulkNodesResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let resp = state.store.bulk_upsert(&principal.tenant_id, req).await?;
    if resp.upserted_nodes > 0 {
        let _ = state
            .store
            .resolve_entity_links(&principal.tenant_id, ResolveEntitiesRequest::default())
            .await;
    }
    Ok(Json(resp))
}

async fn search_nodes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NodeSearchQuery>,
) -> Result<Json<NodeSearchResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    if query.q.trim().is_empty() {
        return Ok(Json(NodeSearchResponse { items: Vec::new() }));
    }
    let query_vec = state
        .ai
        .embed_one(query.q.trim())
        .await
        .ok()
        .map(|(v, _)| v);
    let resp = state
        .store
        .search_nodes(&principal.tenant_id, query, query_vec.as_deref())
        .await?;
    Ok(Json(resp))
}

async fn get_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::DataRead)?;
    let node = state
        .store
        .get_node(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    Ok(Json(node))
}

async fn patch_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let node = state
        .store
        .patch_node(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(node))
}

async fn delete_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    state.store.delete_node(&principal.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn list_node_documents_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<NodeDocumentsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .list_node_documents(&principal.tenant_id, id)
        .await?;
    Ok(Json(resp))
}

async fn query_node_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, metric)): ApiPath<(Uuid, String)>,
    ApiQuery(query): ApiQuery<NodeMetricSeriesQuery>,
) -> Result<Json<Vec<MetricSeriesPoint>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let points = state
        .store
        .query_node_metrics(&principal.tenant_id, id, &metric, query)
        .await?;
    Ok(Json(points))
}

async fn list_links_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<LinkListQuery>,
) -> Result<Json<Vec<LinkRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let links = state.store.list_links(&principal.tenant_id, query).await?;
    Ok(Json(links))
}

async fn upsert_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<UpsertLinkRequest>,
) -> Result<Json<LinkRecord>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let link = state.store.upsert_link(&principal.tenant_id, req).await?;
    Ok(Json(link))
}

async fn delete_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    state.store.delete_link(&principal.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn write_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<WriteMetricsRequest>,
) -> Result<Json<WriteMetricsResponse>, HttpError> {
    principal.require(Permission::DataSync)?;
    let resp = state.store.write_metrics(&principal.tenant_id, req).await?;
    Ok(Json(resp))
}

async fn query_window_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(query): ApiJson<WindowQuery>,
) -> Result<Json<WindowQueryResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_window(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn list_metric_catalog_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<MetricCatalogEntry>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_metric_catalog(&principal.tenant_id)
        .await?;
    Ok(Json(items))
}

async fn query_neighbors_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NeighborQuery>,
) -> Result<Json<SubgraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_neighbors(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn query_path_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> Result<Json<PathResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_shortest_path(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn query_subgraph_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<SubgraphQuery>,
) -> Result<Json<SubgraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_subgraph(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn run_algorithm_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(algo_raw): ApiPath<String>,
    ApiJson(req): ApiJson<AlgoRunRequest>,
) -> Result<Json<AlgoRunResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let algo = GraphAlgo::from_str(algo_raw.trim())?;
    let resp = state
        .store
        .run_algorithm(&principal.tenant_id, algo, req)
        .await?;
    Ok(Json(resp))
}

async fn list_algorithm_results_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(algo_raw): ApiPath<String>,
    ApiQuery(query): ApiQuery<AlgoResultsQuery>,
) -> Result<Json<AlgoResultsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let algo = GraphAlgo::from_str(algo_raw.trim())?;
    let resp = state
        .store
        .list_algorithm_results(&principal.tenant_id, algo, query)
        .await?;
    Ok(Json(resp))
}

async fn resolve_entities_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<ResolveEntitiesRequest>,
) -> Result<Json<ResolveEntitiesResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let resp = state
        .store
        .resolve_entity_links(&principal.tenant_id, req)
        .await?;
    Ok(Json(resp))
}

async fn list_entity_candidates_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<EntityCandidatesResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .list_entity_candidates(&principal.tenant_id)
        .await?;
    Ok(Json(resp))
}

async fn confirm_entity_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((entity_id, node_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<EntityLink>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let link = state
        .store
        .confirm_entity_link(&principal.tenant_id, entity_id, node_id)
        .await?;
    Ok(Json(link))
}

async fn reject_entity_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((entity_id, node_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<EntityLink>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let link = state
        .store
        .reject_entity_link(&principal.tenant_id, entity_id, node_id)
        .await?;
    Ok(Json(link))
}
