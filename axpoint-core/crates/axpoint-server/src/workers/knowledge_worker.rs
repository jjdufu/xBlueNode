//! `KNOWLEDGE_SYNC` background job handler (`06-STEP05-06` §06.5).

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::platform::{BoxFuture, JobHandler, LeasedJob};
use axpoint_domain::ports::StudioExecutionPort as _;
use axpoint_domain::rollup::KnowledgeSyncJobPayload;
use axpoint_llm::AiGateway;

/// `JobKind::KnowledgeSync` handler syncing `BlueNode` dossiers (`xb_node:<id>`) into the hypergraph.
pub(crate) struct KnowledgeSyncHandler {
    pub(crate) store: PgStore,
    pub(crate) ai: AiGateway,
}

impl JobHandler for KnowledgeSyncHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let payload: KnowledgeSyncJobPayload = serde_json::from_value(job.payload.clone())
                .map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid KNOWLEDGE_SYNC payload: {err}"),
                    )
                })?;

            self.store
                .execute_knowledge_sync(&job.tenant_id, &payload, &self.ai)
                .await
        })
    }
}
