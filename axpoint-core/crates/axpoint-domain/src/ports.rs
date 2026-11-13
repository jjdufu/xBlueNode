//! Pure port traits decoupling domain consumers from concrete storage and AI adapters.

use std::future::Future;

use uuid::Uuid;

use crate::enums::JobKind;
use crate::platform::{
    ApiTokenRecord, AuditRecord, JobListQuery, JobListResponse, JobRecord, LeasedJob, NewJob,
    OutboxEvent, PrincipalRecord,
};
use crate::topology::{
    AxFlowDef, AxFlowRunResult, AxSandboxRecord, AxSimAction, AxSimResult, AxSimSnapshot, BlueNode,
    CreateAxFlowRequest, CreateAxSandboxRequest, SaveXBlueSchemaRequest, UpsertBlueNodeRequest,
    XBlueSchema,
};
use crate::{AppError, ExtractedTriple, FusedHit, SystemStats, TopologyGraph};

/// Abstract persistence port for `xBlueStudio`, `AxFlow`, and `AxSim` operations.
pub trait TopologyRepository: Send + Sync {
    /// Reads the `xBlueStudio` schema for a tenant.
    fn get_xblue_schema(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<XBlueSchema, AppError>> + Send;

    /// Upserts `xBlueStudio` schema definitions transactionally.
    fn save_xblue_schema(
        &self,
        tenant_id: &str,
        req: SaveXBlueSchemaRequest,
    ) -> impl Future<Output = Result<XBlueSchema, AppError>> + Send;

    /// Returns the runtime snapshot of `BlueNode`s and `BlueLink`s for a tenant.
    fn get_runtime_snapshot(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<AxSimSnapshot, AppError>> + Send;

    /// Upserts a runtime `BlueNode` and optional link.
    fn upsert_blue_node(
        &self,
        tenant_id: &str,
        req: UpsertBlueNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send;

    /// Lists `AxFlow` pipelines for a tenant.
    fn list_ax_flows(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<AxFlowDef>, AppError>> + Send;

    /// Creates a new `AxFlow` pipeline.
    fn create_ax_flow(
        &self,
        tenant_id: &str,
        req: CreateAxFlowRequest,
    ) -> impl Future<Output = Result<AxFlowDef, AppError>> + Send;

    /// Executes an `AxFlow` pipeline.
    fn run_ax_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> impl Future<Output = Result<AxFlowRunResult, AppError>> + Send;

    /// Lists `AxSim` sandboxes for a tenant.
    fn list_ax_sandboxes(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<AxSandboxRecord>, AppError>> + Send;

    /// Creates an isolated `AxSim` sandbox cloned from production state.
    fn create_ax_sandbox(
        &self,
        tenant_id: &str,
        req: CreateAxSandboxRequest,
    ) -> impl Future<Output = Result<AxSandboxRecord, AppError>> + Send;

    /// Runs causal propagation inside an `AxSim` sandbox.
    fn simulate_ax_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        actions: Vec<AxSimAction>,
    ) -> impl Future<Output = Result<AxSimResult, AppError>> + Send;
}

/// Abstract read-only hypergraph metrics and topology port.
pub trait HypergraphQueryPort: Send + Sync {
    /// Checks underlying storage connectivity.
    fn ping(&self) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Returns hypergraph counts and active embedding dimensions.
    fn get_stats(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<SystemStats, AppError>> + Send;

    /// Returns a subgraph of nodes and edges for topology exploration.
    fn get_topology(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> impl Future<Output = Result<TopologyGraph, AppError>> + Send;
}

/// Abstract AI gateway port for embeddings, triple extraction, and RAG synthesis.
pub trait AiPort: Send + Sync {
    /// Returns the configured embedding model identifier.
    fn embedding_model_name(&self) -> &str;

    /// Returns the detected embedding dimension, if known.
    fn current_embedding_dim(&self) -> Option<usize>;

    /// Generates embeddings for a batch of inputs.
    fn embed_batch(
        &self,
        inputs: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send;

    /// Generates an embedding vector for a single string.
    fn embed_one(
        &self,
        text: &str,
    ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send;

    /// Extracts knowledge triples from a paragraph.
    fn extract_triples(
        &self,
        paragraph: &str,
    ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send;

    /// Extracts seed entity names from a query.
    fn extract_query_entities(
        &self,
        query: &str,
    ) -> impl Future<Output = Result<Vec<String>, AppError>> + Send;

    /// Synthesizes a grounded answer from fused retrieval hits.
    fn synthesize_answer(
        &self,
        query: &str,
        hits: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> + Send;
}

/// Persistent background job queue port backed by `ax_jobs`.
pub trait JobQueuePort: Send + Sync {
    /// Enqueues a job; returns the existing job ID on `(tenant_id, kind, idempotency_key)` conflict.
    fn enqueue(
        &self,
        tenant_id: &str,
        job: NewJob,
    ) -> impl Future<Output = Result<Uuid, AppError>> + Send;

    /// Leases ready or expired jobs across tenants via `SECURITY DEFINER` function `ax_lease_jobs`.
    fn lease(
        &self,
        worker: &str,
        kinds: &[JobKind],
        limit: u32,
        lease_secs: u32,
    ) -> impl Future<Output = Result<Vec<LeasedJob>, AppError>> + Send;

    /// Extends the lease on an in-flight job.
    fn heartbeat(
        &self,
        job: &LeasedJob,
        lease_secs: u32,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Marks a leased job as `SUCCEEDED` and stores its JSON result.
    fn complete(
        &self,
        job: &LeasedJob,
        result: serde_json::Value,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Records a failure; reschedules with exponential backoff (`min(2^attempt, 300)`s) if `attempt < max_attempts`, else marks `FAILED`.
    fn fail(
        &self,
        job: &LeasedJob,
        error: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Lists tenant-scoped jobs with optional filter and cursor pagination.
    fn list_jobs(
        &self,
        tenant_id: &str,
        query: JobListQuery,
    ) -> impl Future<Output = Result<JobListResponse, AppError>> + Send;

    /// Fetches a single tenant-scoped job by ID (`None` if missing or owned by another tenant).
    fn get_job(
        &self,
        tenant_id: &str,
        job_id: Uuid,
    ) -> impl Future<Output = Result<Option<JobRecord>, AppError>> + Send;
}

/// Transactional event outbox port backed by `ax_outbox`.
pub trait OutboxPort: Send + Sync {
    /// Claims unprocessed outbox events across tenants via `ax_claim_outbox`.
    fn claim(
        &self,
        worker: &str,
        limit: u32,
        lease_secs: u32,
    ) -> impl Future<Output = Result<Vec<OutboxEvent>, AppError>> + Send;

    /// Marks outbox event IDs as processed within the owning tenant context.
    fn ack(
        &self,
        tenant_id: &str,
        ids: &[i64],
    ) -> impl Future<Output = Result<(), AppError>> + Send;
}

/// Append-only security audit sink port backed by `sec_audit_logs`.
pub trait AuditSink: Send + Sync {
    /// Writes a batch of audit records (grouped by `tenant_id` internally).
    fn write_batch(
        &self,
        records: Vec<AuditRecord>,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
}

/// Token and principal lookup port for `AxGuard` authentication.
pub trait CredentialStore: Send + Sync {
    /// Resolves a personal access token by its hex SHA-256 digest via `ax_resolve_api_token`.
    fn resolve_api_token(
        &self,
        token_hash: &str,
    ) -> impl Future<Output = Result<Option<ApiTokenRecord>, AppError>> + Send;

    /// Refreshes `last_used_at` on a personal access token within its owning tenant context.
    fn touch_api_token(
        &self,
        tenant_id: &str,
        token_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Loads a user and the union of permissions from their assigned roles.
    fn load_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> impl Future<Output = Result<Option<PrincipalRecord>, AppError>> + Send;
}
