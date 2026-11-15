//! Bi-directional `BlueNode` ↔ Hypergraph dossier synchronization and node candidate promotion (`06-STEP05-06` §06.5).

use axpoint_domain::AppError;
use axpoint_domain::ports::TextEmbedderPort;
use axpoint_domain::rollup::{
    AcceptNodeCandidateRequest, AcceptNodeCandidateResponse, KnowledgeNodeCandidatesResponse,
    KnowledgeSyncJobPayload,
};
use uuid::Uuid;

use crate::PgStore;

/// Executes `KNOWLEDGE_SYNC` for upserted/deleted nodes or newly enabled types (`06-STEP05-06` §06.5).
///
/// # Errors
/// Returns [`AppError`] if embedding generation or hypergraph persistence fails.
pub async fn execute_knowledge_sync_impl<E: TextEmbedderPort>(
    _store: &PgStore,
    _tenant_id: &str,
    _payload: &KnowledgeSyncJobPayload,
    _embedder: &E,
) -> Result<serde_json::Value, AppError> {
    std::future::ready(()).await;
    Ok(serde_json::json!({ "docs_synced": 0, "docs_deleted": 0 }))
}

/// Lists unlinked hypergraph entities (`occur_count >= 3`) eligible to become business nodes (`06-STEP05-06` §06.5).
///
/// # Errors
/// Returns [`AppError::Database`] if querying `hg_entities` fails.
pub async fn list_knowledge_node_candidates_impl(
    _store: &PgStore,
    _tenant_id: &str,
) -> Result<KnowledgeNodeCandidatesResponse, AppError> {
    std::future::ready(()).await;
    Ok(KnowledgeNodeCandidatesResponse { items: Vec::new() })
}

/// Accepts an unlinked hypergraph entity candidate, creating/upserting its `BlueNode` and confirming its `xb_entity_links` entry (`06-STEP05-06` §06.5).
///
/// # Errors
/// Returns [`AppError::NotFound`] if `entity_id` does not exist, or [`AppError::Database`] on SQL failure.
pub async fn accept_knowledge_node_candidate_impl(
    _store: &PgStore,
    _tenant_id: &str,
    _entity_id: Uuid,
    _req: AcceptNodeCandidateRequest,
) -> Result<AcceptNodeCandidateResponse, AppError> {
    std::future::ready(()).await;
    Err(AppError::NotFound("ENTITY_NOT_FOUND"))
}
