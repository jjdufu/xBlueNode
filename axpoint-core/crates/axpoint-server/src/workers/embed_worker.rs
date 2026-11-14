//! `EMBED_NODES` background worker computing `XB_NODE`, `DS_COLUMN`, and `AN_METRIC` embeddings (`04-STEP03` §03.4, `05-STEP04` §04.6.3).

use std::collections::HashMap;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::graph::ResolveEntitiesRequest;
use axpoint_domain::platform::{BoxFuture, JobHandler, LeasedJob};
use axpoint_domain::ports::{AiPort as _, AnalyticsRepository as _, GraphStorePort as _};
use axpoint_llm::AiGateway;
use serde::Deserialize;
use uuid::Uuid;

const EMBED_BATCH_CHUNK: usize = 64;

#[derive(Debug, Default, Deserialize)]
struct EmbedNodesPayload {
    #[serde(default)]
    node_ids: Vec<Uuid>,
    #[serde(default)]
    node_id: Option<Uuid>,
}

/// `JobKind::EmbedNodes` handler generating `XB_NODE`, `DS_COLUMN`, and `AN_METRIC` embeddings in `hg_embeddings`.
pub(crate) struct EmbedNodesHandler {
    pub(crate) store: PgStore,
    pub(crate) ai: AiGateway,
}

impl JobHandler for EmbedNodesHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move { self.execute(job).await })
    }
}

impl EmbedNodesHandler {
    async fn embed_graph_nodes(
        &self,
        tenant_id: &str,
        target_ids: Vec<Uuid>,
    ) -> Result<usize, AppError> {
        if target_ids.is_empty() {
            return Ok(0);
        }

        let schema = self.store.get_xblue_schema(tenant_id).await?;
        let display_names: HashMap<String, String> = schema
            .node_types
            .into_iter()
            .map(|nt| (nt.name, nt.display_name))
            .collect();

        let mut nodes_with_texts: Vec<(Uuid, String)> = Vec::with_capacity(target_ids.len());
        for nid in target_ids {
            if let Some(node) = self.store.get_node(tenant_id, nid).await? {
                let disp = display_names
                    .get(&node.type_name)
                    .map_or(node.type_name.as_str(), String::as_str);
                let text = PgStore::build_node_embedding_text(disp, &node);
                nodes_with_texts.push((node.id, text));
            }
        }

        if nodes_with_texts.is_empty() {
            return Ok(0);
        }

        let mut total_embedded: usize = 0;
        for chunk in nodes_with_texts.chunks(EMBED_BATCH_CHUNK) {
            let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
            let (vectors, dim) = self.ai.embed_batch(&texts).await?;
            let items: Vec<(Uuid, Vec<f32>)> =
                chunk.iter().map(|(id, _)| *id).zip(vectors).collect();
            self.store
                .upsert_node_embeddings(tenant_id, self.ai.embedding_model_name(), dim, &items)
                .await?;
            total_embedded = total_embedded.saturating_add(items.len());
        }

        let _ = self
            .store
            .resolve_entity_links(tenant_id, ResolveEntitiesRequest::default())
            .await;

        Ok(total_embedded)
    }

    async fn embed_analytics_items(&self, tenant_id: &str) -> Result<usize, AppError> {
        let pending = self
            .store
            .list_unembedded_analytics_items(tenant_id, self.ai.embedding_model_name(), 500)
            .await?;
        if pending.is_empty() {
            return Ok(0);
        }

        let mut total_embedded: usize = 0;
        for chunk in pending.chunks(EMBED_BATCH_CHUNK) {
            let texts: Vec<String> = chunk.iter().map(|(_, _, t)| t.clone()).collect();
            let (vectors, dim) = self.ai.embed_batch(&texts).await?;
            let items: Vec<(String, Uuid, Vec<f32>)> = chunk
                .iter()
                .zip(vectors)
                .map(|((nt, nid, _), vec)| (nt.clone(), *nid, vec))
                .collect();
            let n = self
                .store
                .upsert_analytics_embeddings(tenant_id, self.ai.embedding_model_name(), dim, &items)
                .await?;
            total_embedded = total_embedded.saturating_add(n);
        }
        Ok(total_embedded)
    }

    async fn execute(&self, job: &LeasedJob) -> Result<serde_json::Value, AppError> {
        let payload: EmbedNodesPayload =
            serde_json::from_value(job.payload.clone()).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid EMBED_NODES payload: {err}"),
                )
            })?;

        let mut target_ids = payload.node_ids;
        if let Some(single) = payload.node_id
            && !target_ids.contains(&single)
        {
            target_ids.push(single);
        }

        let node_count = self.embed_graph_nodes(&job.tenant_id, target_ids).await?;
        let analytics_count = self.embed_analytics_items(&job.tenant_id).await?;

        Ok(serde_json::json!({
            "embedded": node_count.saturating_add(analytics_count),
            "nodes_embedded": node_count,
            "analytics_embedded": analytics_count,
        }))
    }
}
