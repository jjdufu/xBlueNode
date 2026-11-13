//! Pure port traits decoupling domain consumers from concrete storage and AI adapters.

use std::future::Future;

use uuid::Uuid;

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

    /// Extracts seed entities from a query.
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
