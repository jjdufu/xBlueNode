//! Pure port traits decoupling domain consumers from concrete storage and AI adapters.

use std::collections::BTreeMap;
use std::future::Future;

use uuid::Uuid;

use crate::agent::{
    AgentEmbedRecord, AgentRecord, AgentTaskInput, AgentTaskOutcome, AgentVersionRecord,
    ConversationListQuery, ConversationRecord, CreateAgentRequest, EndpointListQuery, EndpointSpec,
    LlmProviderRecord, LlmUsageDelta, LlmUsageQuery, LlmUsageRecord, McpClientRecord, McpToolInfo,
    MessageRecord, NewAgentEmbedInput, NewApiTokenInput, NewConversationInput, NewMessageInput,
    PersonalAccessTokenSummary, ResolvedAgentEmbed, SaveEndpointInput, SaveLlmProviderInput,
    SaveMcpClientInput, SimRunToolInput, SimRunToolOutput, UpdateAgentRequest,
    UpdateEndpointRequest,
};
use crate::analytics::{
    AnalyticsPrincipal, AskFeedbackKind, AskTurnRecord, CreateMetricRequest,
    CreateSavedQueryRequest, FewShotTurn, MaterializeRequest, MetricDef, MetricQuery,
    MetricQueryResult, NewAskTurnInput, NewQueryHistoryEntry, NewTransformDraftInput, PivotResult,
    PivotSpec, QueryHistoryEntry, QueryHistoryListQuery, QueryRequest, QueryResult, SavedQuery,
    SchemaRecallBundle, TransformDraft, TransformPreview, UpdateMetricRequest,
    UpdateSavedQueryRequest,
};
use crate::axhub::{
    CatalogSearchHit, ColumnMeta, CommitStagingRequest, Connection, ConnectionStatus,
    ConnectionTestResult, ConnectorSpecDto, CreateDatasetRequest, CreateGlossaryTermRequest,
    CreateQualityRuleRequest, CreateWebhookRequest, Dataset, DatasetListQuery, DiscoveredObject,
    GlossaryTerm, LakeMaintenanceRequest, LakeMaintenanceResult, LakePreviewRequest,
    LakeSnapshotInfo, LineageEdge, NewConnectionInput, NewUploadInput, PaginatedList,
    PreviewResponse, QualityReport, QualityRule, ResolvedWebhook, ScheduledDataset,
    SealedCredential, SidecarConnectorRequest, SidecarPreviewRequest, SidecarSyncRequest,
    SidecarSyncResult, SyncDatasetResponse, SyncJob, UnifiedParseResponse, UpdateColumnMetaRequest,
    UpdateConnectionInput, UpdateDatasetRequest, UploadRecord, WebhookBufferItem, WebhookConfig,
};
use crate::enums::{ConversationStatus, Environment, ExportFormat, GraphAlgo, JobKind};
use crate::export::{
    CompleteExportJobParams, CreateExportJobParams, CreateExportTemplateParams,
    CreateShareLinkParams, ExportJobRecord, ExportListQuery, ExportTemplateRecord,
    RenderChartRequest, ResolvedShare, ShareLinkRecord, ShareListQuery, SidecarExportRequest,
    SidecarExportResponse,
};
use crate::flow::{
    ActionListQuery, ActionLogRecord, ApprovalListQuery, ApprovalRecord, DecideApprovalParams,
    DeployFlowRequest, EventFlowItem, FlowDeploymentRecord, FlowListQuery, FlowVersionRecord,
    FlowVersionsListResponse, NewActionLogInput, NewApprovalInput, ProgressionPlannerFn,
    PublishFlowVersionRequest, ResolvedFlowHook, RunListQuery, RunRecord, ScheduledFlowItem,
    StartRunParams, StepMutation, StepRecord, UpdateAxFlowRequest,
};
use crate::graph::{
    AlgoResultsQuery, AlgoResultsResponse, AlgoRunRequest, AlgoRunResponse, BulkNodesRequest,
    BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery, LinkRecord,
    MetricCatalogEntry, MetricSeriesPoint, NeighborQuery, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchQuery, NodeSearchResponse, PatchNodeRequest,
    PathQuery, PathResponse, ResolveEntitiesRequest, ResolveEntitiesResponse, SubgraphQuery,
    SubgraphResponse, UpsertLinkRequest, WindowQuery, WindowQueryResponse, WriteMetricsRequest,
    WriteMetricsResponse,
};
use crate::platform::{
    ApiTokenRecord, AuditRecord, BoxFuture, JobListQuery, JobListResponse, JobRecord, LeasedJob,
    NewJob, OutboxEvent, PrincipalRecord,
};
use crate::rollup::{
    AcceptNodeCandidateRequest, AcceptNodeCandidateResponse, GraphAggregateJobPayload,
    KnowledgeNodeCandidatesResponse, KnowledgeSyncJobPayload, MapMaterializeJobPayload,
    MetricExplainResponse,
};
use crate::rules::{
    AbsenceRuleItem, AckBatchResponse, AlertEventKind, AlertEventRecord, AlertEventsQuery,
    AlertListQuery, AlertRecord, AlertSilenceRecord, CreateRuleRequest, CreateSilenceRequest,
    DeliveryFailureUpdate, DueDeliveryItem, EnqueueDeliveryInput, FalseConditionOutcome,
    NotifyChannelRecord, NotifyDeliveryRecord, RuleDef, RuleListQuery, RuleScopeKind,
    SaveNotifyChannelInput, SimplePageQuery, TrueConditionInput, TrueConditionOutcome,
    UpdateRuleRequest,
};
use crate::sim::{
    Checkpoint, CreateDecisionRequest, CreateOverlayLinkRequest, CreateOverlayNodeRequest,
    CreateScenarioRequest, DecisionRecord, ImpactListQuery, ImpactRow, PatchOverlayLinkRequest,
    PatchOverlayNodeRequest, RefreshBaseResponse, SandboxGraphLink, SandboxGraphNode,
    SandboxGraphResponse, ScenarioRecord, SimGraph, SimResult,
};
use crate::studio::{
    CreateDraftRequest, CreateSubgraphRequest, DocEntityGroup, DraftStatus, MappingRecord,
    MappingRunRecord, MergePreview, MergePreviewRequest, MergeSubgraphRequest, NodeTypeSpec,
    PublishDraftRequest, PublishDraftResponse, PublishedSchemaResponse, SaveSchemaDirectRequest,
    SchemaDiff, SchemaDraft, SchemaVersionDetail, SchemaVersionSummary, SubgraphInfo,
    TriggerMappingRunRequest, TriggerMappingRunResponse, UpdateDraftRequest, UpdateSubgraphRequest,
    ValidationContext,
};
use crate::topology::{
    AxFlowDef, AxFlowRunResult, AxSandboxRecord, AxSimAction, AxSimResult, AxSimSnapshot, BlueNode,
    CreateAxFlowRequest, CreateAxSandboxRequest, SaveXBlueSchemaRequest, SimulateAxSandboxRequest,
    UpsertBlueNodeRequest, XBlueSchema,
};
use crate::view::{
    AffectedWidgetRef, AssetRecord, CreateAssetParams, CreateDashboardRequest,
    CreateWidgetRecordParams, DashboardDetail, DashboardRecord, FloorPlacementRecord, FloorPlanRow,
    LayoutUpdateItem, NodesGeoLinkRow, NodesGeoRow, PutFloorPlacementItem, PutScene3dBindingItem,
    Scene3dBindingRecord, Scene3dRow, UpdateDashboardRequest, UpdateWidgetRecordParams,
    ViewWriteNodeOutcome, ViewWriteNodeParams, WidgetDeps, WidgetRecord,
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

    /// Executes a general system + user chat completion (with cassette replay/record support).
    fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
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

/// Persistence port for `AxHub` connections and sealed `ds_credentials` (`03-STEP02`).
pub trait ConnectionRepository: Send + Sync {
    /// Lists all connections in `tenant_id` paired with their optional sealed credentials.
    fn list_connections(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<(Connection, Option<SealedCredential>)>, AppError>> + Send;

    /// Loads a single connection and its optional sealed credential in `tenant_id`.
    fn get_connection(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<(Connection, Option<SealedCredential>)>, AppError>> + Send;

    /// Creates a connection (and inserts `ds_credentials` if `input.credential` is `Some`).
    fn create_connection(
        &self,
        tenant_id: &str,
        input: NewConnectionInput,
    ) -> impl Future<Output = Result<Connection, AppError>> + Send;

    /// Updates a connection (`input.credential`: `None` = unchanged, `Some(None)` = clear, `Some(Some(c))` = upsert).
    fn update_connection(
        &self,
        tenant_id: &str,
        id: Uuid,
        input: UpdateConnectionInput,
    ) -> impl Future<Output = Result<Option<Connection>, AppError>> + Send;

    /// Deletes a connection; returns `AppError::Conflict("CONNECTION_IN_USE")` if referenced by datasets.
    fn delete_connection(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Records the result of `POST /api/axhub/connections/{id}/test`.
    fn update_test_status(
        &self,
        tenant_id: &str,
        id: Uuid,
        status: ConnectionStatus,
        message: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Loads a sealed credential row by ID within `tenant_id`.
    fn get_credential(
        &self,
        tenant_id: &str,
        credential_id: Uuid,
    ) -> impl Future<Output = Result<Option<SealedCredential>, AppError>> + Send;

    /// Lists all sealed credentials in `tenant_id` for `CREDENTIAL_ROTATE`.
    fn list_credentials(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<SealedCredential>, AppError>> + Send;

    /// Re-encrypts and updates a credential row (`key_version`, `nonce`, `ciphertext`, `rotated_at`).
    fn update_rotated_credential(
        &self,
        tenant_id: &str,
        credential: SealedCredential,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
}

/// Persistence port for `AxHub` datasets, sync jobs, uploads, webhooks, lineage, and catalog (`03-STEP02`).
pub trait DatasetRepository: Send + Sync {
    /// Lists datasets in `tenant_id` with optional filters and pagination.
    fn list_datasets(
        &self,
        tenant_id: &str,
        query: DatasetListQuery,
    ) -> impl Future<Output = Result<PaginatedList<Dataset>, AppError>> + Send;

    /// Loads a dataset by UUID within `tenant_id`.
    fn get_dataset(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<Dataset>, AppError>> + Send;

    /// Loads a dataset by unique machine name within `tenant_id`.
    fn get_dataset_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<Option<Dataset>, AppError>> + Send;

    /// Creates a new dataset in `tenant_id`.
    fn create_dataset(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        req: CreateDatasetRequest,
    ) -> impl Future<Output = Result<Dataset, AppError>> + Send;

    /// Updates mutable fields of a dataset in `tenant_id`.
    fn update_dataset(
        &self,
        tenant_id: &str,
        id: Uuid,
        req: UpdateDatasetRequest,
    ) -> impl Future<Output = Result<Option<Dataset>, AppError>> + Send;

    /// Deletes a dataset in `tenant_id`, returning the deleted record if found.
    fn delete_dataset(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<Dataset>, AppError>> + Send;

    /// Enqueues a single-flight `SYNC_DATASET` job (`existing = true` on `uq_ax_jobs_sync_dataset` conflict).
    fn enqueue_sync_job(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<SyncDatasetResponse, AppError>> + Send;

    /// Starts or resumes a `ds_sync_jobs` row (`RUNNING`) for `job_id`, reusing `batch_id` on retry.
    fn start_sync_execution(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        job_id: Uuid,
    ) -> impl Future<Output = Result<SyncJob, AppError>> + Send;

    /// Commits sync metrics, dataset `cursor`/`schema`/`row_count`, column meta, stream offsets, outbox, and audit.
    fn complete_sync_execution(
        &self,
        tenant_id: &str,
        sync_job_id: Uuid,
        dataset_id: Uuid,
        result: &SidecarSyncResult,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Records a failed sync execution (`terminal = true` also marks `ds_datasets.status = FAILED`).
    fn fail_sync_execution(
        &self,
        tenant_id: &str,
        sync_job_id: Uuid,
        dataset_id: Uuid,
        error_code: &str,
        error_message: &str,
        terminal: bool,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Lists synchronization history for a dataset.
    fn list_sync_jobs(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        limit: u32,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<PaginatedList<SyncJob>, AppError>> + Send;

    /// Loads current stream partition offsets for a dataset.
    fn get_stream_offsets(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, AppError>> + Send;

    /// Lists column metadata for a dataset.
    fn list_columns(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<ColumnMeta>, AppError>> + Send;

    /// Updates user-editable column metadata (`description`, `business_term`, `unit`, `pii_*`).
    fn update_column_meta(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        column_name: &str,
        req: UpdateColumnMetaRequest,
    ) -> impl Future<Output = Result<Option<ColumnMeta>, AppError>> + Send;

    /// Lists quality assertion rules for a dataset.
    fn list_quality_rules(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<QualityRule>, AppError>> + Send;

    /// Creates a quality assertion rule on a dataset.
    fn create_quality_rule(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        req: CreateQualityRuleRequest,
    ) -> impl Future<Output = Result<QualityRule, AppError>> + Send;

    /// Deletes a quality rule by UUID within `tenant_id`.
    fn delete_quality_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Finds an already `COMMITTED` upload with matching SHA-256 in `tenant_id`.
    fn find_committed_upload_by_sha(
        &self,
        tenant_id: &str,
        sha256: &str,
    ) -> impl Future<Output = Result<Option<UploadRecord>, AppError>> + Send;

    /// Inserts a `PENDING` upload record.
    fn create_upload(
        &self,
        tenant_id: &str,
        input: NewUploadInput,
    ) -> impl Future<Output = Result<UploadRecord, AppError>> + Send;

    /// Loads an upload record by UUID within `tenant_id`.
    fn get_upload(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<UploadRecord>, AppError>> + Send;

    /// Marks an upload `COMMITTED`, links `hg_docs.upload_id`, records lineage, and emits audit/outbox.
    fn complete_upload(
        &self,
        tenant_id: &str,
        upload_id: Uuid,
        parse_id: Uuid,
        doc_id: Option<Uuid>,
        dataset_ids: &[Uuid],
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Marks an upload `FAILED`.
    fn fail_upload(
        &self,
        tenant_id: &str,
        upload_id: Uuid,
        error_code: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Creates a `WEBHOOK_STREAM` dataset, sealed secret credential, and `ds_webhooks` row.
    fn create_webhook(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        req: &CreateWebhookRequest,
        credential: SealedCredential,
    ) -> impl Future<Output = Result<(WebhookConfig, Dataset), AppError>> + Send;

    /// Lists webhooks in `tenant_id`.
    fn list_webhooks(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<WebhookConfig>, AppError>> + Send;

    /// Deletes a webhook by UUID in `tenant_id`.
    fn delete_webhook(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Resolves `(tenant_id, slug)` via `SECURITY DEFINER` function `ax_resolve_webhook`.
    fn resolve_webhook(
        &self,
        tenant_id: &str,
        slug: &str,
    ) -> impl Future<Output = Result<Option<ResolvedWebhook>, AppError>> + Send;

    /// Finds the webhook backing a `WEBHOOK_STREAM` dataset.
    fn get_webhook_by_dataset(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<Option<WebhookConfig>, AppError>> + Send;

    /// Buffers an inbound webhook event (`Ok(true)` = newly inserted, `Ok(false)` = duplicate `delivery_id`).
    fn ingest_webhook_event(
        &self,
        tenant_id: &str,
        webhook_id: Uuid,
        delivery_id: &str,
        payload: serde_json::Value,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Lists unflushed `ds_webhook_buffer` items for a webhook.
    fn list_pending_webhook_events(
        &self,
        tenant_id: &str,
        webhook_id: Uuid,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<WebhookBufferItem>, AppError>> + Send;

    /// Marks `ds_webhook_buffer` rows as flushed.
    fn mark_webhook_events_flushed(
        &self,
        tenant_id: &str,
        ids: &[i64],
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Upserts a directed edge in `ds_lineage`.
    fn upsert_lineage(
        &self,
        tenant_id: &str,
        src_kind: &str,
        src_id: &str,
        dst_kind: &str,
        dst_id: &str,
        via: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Traverses upstream and downstream lineage edges around `dataset_id` up to `depth` hops.
    fn get_dataset_lineage(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        depth: u32,
    ) -> impl Future<Output = Result<Vec<LineageEdge>, AppError>> + Send;

    /// Searches the data catalog across `ds_datasets.fts`, `ds_column_meta`, and `ds_glossary_terms` synonyms.
    fn search_catalog(
        &self,
        tenant_id: &str,
        query: &str,
        limit: u32,
    ) -> impl Future<Output = Result<PaginatedList<CatalogSearchHit>, AppError>> + Send;

    /// Lists glossary terms in `tenant_id`.
    fn list_glossary(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<GlossaryTerm>, AppError>> + Send;

    /// Creates or upserts a glossary term in `tenant_id`.
    fn create_glossary_term(
        &self,
        tenant_id: &str,
        req: CreateGlossaryTermRequest,
    ) -> impl Future<Output = Result<GlossaryTerm, AppError>> + Send;

    /// Deletes a glossary term in `tenant_id`.
    fn delete_glossary_term(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Scans cron-enabled datasets across tenants via `ax_scheduled_datasets()`.
    fn list_scheduled_datasets(
        &self,
    ) -> impl Future<Output = Result<Vec<ScheduledDataset>, AppError>> + Send;

    /// Reads `sec_tenants.timezone` for `tenant_id` (defaults to `"Asia/Shanghai"`).
    fn get_tenant_timezone(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send;

    /// Reads `LAKE_SNAPSHOT` retention days for `tenant_id` (defaults to `30`).
    fn get_lake_snapshot_keep_days(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<i32, AppError>> + Send;
}

/// Internal HTTP port to the `axpoint-parser` Sidecar (`02-STEP01` & `03-STEP02`).
pub trait SidecarPort: Send + Sync {
    /// Invokes `POST /parse/unified` on the Sidecar.
    fn parse_unified(
        &self,
        tenant_id: &str,
        timezone: &str,
        filename: &str,
        bytes: Vec<u8>,
        options: &BTreeMap<String, String>,
    ) -> impl Future<Output = Result<UnifiedParseResponse, AppError>> + Send;

    /// Invokes `GET /connectors/types`.
    fn list_connector_types(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<ConnectorSpecDto>, AppError>> + Send;

    /// Invokes `POST /connectors/test`.
    fn test_connector(
        &self,
        tenant_id: &str,
        req: &SidecarConnectorRequest,
    ) -> impl Future<Output = Result<ConnectionTestResult, AppError>> + Send;

    /// Invokes `POST /connectors/discover`.
    fn discover_connector(
        &self,
        tenant_id: &str,
        req: &SidecarConnectorRequest,
    ) -> impl Future<Output = Result<Vec<DiscoveredObject>, AppError>> + Send;

    /// Invokes `POST /connectors/preview`.
    fn preview_connector(
        &self,
        tenant_id: &str,
        req: &SidecarPreviewRequest,
    ) -> impl Future<Output = Result<PreviewResponse, AppError>> + Send;

    /// Invokes `POST /connectors/sync` with explicit timeout.
    fn sync_dataset(
        &self,
        tenant_id: &str,
        timezone: &str,
        req: &SidecarSyncRequest,
        timeout_secs: u64,
    ) -> impl Future<Output = Result<SidecarSyncResult, AppError>> + Send;

    /// Invokes `POST /lake/commit_staging`.
    fn commit_staging(
        &self,
        tenant_id: &str,
        timezone: &str,
        req: &CommitStagingRequest,
    ) -> impl Future<Output = Result<Vec<SidecarSyncResult>, AppError>> + Send;

    /// Invokes `POST /lake/preview`.
    fn preview_lake(
        &self,
        tenant_id: &str,
        req: &LakePreviewRequest,
    ) -> impl Future<Output = Result<PreviewResponse, AppError>> + Send;

    /// Invokes `GET /lake/snapshots`.
    fn list_snapshots(
        &self,
        tenant_id: &str,
        lake_table: &str,
    ) -> impl Future<Output = Result<Vec<LakeSnapshotInfo>, AppError>> + Send;

    /// Invokes `POST /lake/maintenance`.
    fn run_maintenance(
        &self,
        tenant_id: &str,
        req: &LakeMaintenanceRequest,
    ) -> impl Future<Output = Result<LakeMaintenanceResult, AppError>> + Send;

    /// Drops a `DuckLake` table when a dataset is deleted.
    fn drop_lake_table(
        &self,
        tenant_id: &str,
        lake_table: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Invokes `POST /lake/materialize` to materialize a `DERIVED_VIEW` SQL query into `DuckLake` (`05-STEP04` §04.7).
    fn materialize_derived_view(
        &self,
        tenant_id: &str,
        timezone: &str,
        req: &MaterializeRequest,
    ) -> impl Future<Output = Result<SidecarSyncResult, AppError>> + Send;

    /// Invokes `POST /render/chart` on the Sidecar to render an `ECharts` option as `SVG` or `PNG` (`10-STEP11-12` §12.3).
    fn render_chart(
        &self,
        _tenant_id: &str,
        _req: &RenderChartRequest,
    ) -> impl Future<Output = Result<Vec<u8>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "render_chart not implemented",
            ))
        }
    }

    /// Invokes `POST /export/{format}` on the Sidecar to generate an export file (`10-STEP11-12` §12.4).
    fn export_document(
        &self,
        _tenant_id: &str,
        _timezone: &str,
        _format: ExportFormat,
        _req: &SidecarExportRequest,
    ) -> impl Future<Output = Result<SidecarExportResponse, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "export_document not implemented",
            ))
        }
    }
}

/// Persistence port for `AxGraph` business nodes, links, hybrid search, and entity links (Step 03).
pub trait GraphStorePort: Send + Sync {
    /// Lists business nodes with optional type, query, geographic `bbox`, and cursor pagination.
    fn list_nodes(
        &self,
        tenant_id: &str,
        query: NodeListQuery,
    ) -> impl Future<Output = Result<NodeListResponse, AppError>> + Send;

    /// Fetches a single business node by UUID.
    fn get_node(
        &self,
        tenant_id: &str,
        node_id: Uuid,
    ) -> impl Future<Output = Result<Option<BlueNode>, AppError>> + Send;

    /// Upserts a business node with `metrics_merge`, optimistic `if_row_version`, outbox events, and `EMBED_NODES` enqueueing.
    fn upsert_node(
        &self,
        tenant_id: &str,
        req: UpsertBlueNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send;

    /// Partially updates an existing business node by UUID.
    fn patch_node(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        req: PatchNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send;

    /// Deletes a business node, cascading its incident `xb_links` and `xb_metric_series` rows.
    fn delete_node(
        &self,
        tenant_id: &str,
        node_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Bulk-upserts up to 5,000 nodes and 20,000 links in a single tenant transaction.
    fn bulk_upsert(
        &self,
        tenant_id: &str,
        req: BulkNodesRequest,
    ) -> impl Future<Output = Result<BulkNodesResponse, AppError>> + Send;

    /// Lists `xb_links` filtered by incident `node_id` and/or `link_type`.
    fn list_links(
        &self,
        tenant_id: &str,
        query: LinkListQuery,
    ) -> impl Future<Output = Result<Vec<LinkRecord>, AppError>> + Send;

    /// Idempotently upserts a link on `(tenant_id, link_type, src_node_id, dst_node_id)`.
    fn upsert_link(
        &self,
        tenant_id: &str,
        req: UpsertLinkRequest,
    ) -> impl Future<Output = Result<LinkRecord, AppError>> + Send;

    /// Deletes a link by UUID.
    fn delete_link(
        &self,
        tenant_id: &str,
        link_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Performs RRF hybrid retrieval combining PostgreSQL FTS and `pgvector` HNSW cosine similarity.
    fn search_nodes(
        &self,
        tenant_id: &str,
        query: NodeSearchQuery,
        query_embedding: Option<&[f32]>,
    ) -> impl Future<Output = Result<NodeSearchResponse, AppError>> + Send;

    /// Returns hypergraph documents and paragraph excerpts mentioning a node via confirmed `xb_entity_links`.
    fn list_node_documents(
        &self,
        tenant_id: &str,
        node_id: Uuid,
    ) -> impl Future<Output = Result<NodeDocumentsResponse, AppError>> + Send;

    /// Incrementally resolves `hg_entities` ↔ `xb_nodes` links (`EXACT_KEY`, `EXACT_TITLE`, `SYNONYM`, `EMBEDDING`).
    fn resolve_entity_links(
        &self,
        tenant_id: &str,
        req: ResolveEntitiesRequest,
    ) -> impl Future<Output = Result<ResolveEntitiesResponse, AppError>> + Send;

    /// Lists unconfirmed, non-rejected `xb_entity_links` candidates (`confidence < 0.9`).
    fn list_entity_candidates(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<EntityCandidatesResponse, AppError>> + Send;

    /// Marks an `(entity_id, node_id)` link as manually confirmed.
    fn confirm_entity_link(
        &self,
        tenant_id: &str,
        entity_id: Uuid,
        node_id: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> + Send;

    /// Marks an `(entity_id, node_id)` link as rejected so automatic resolution will not recreate it.
    fn reject_entity_link(
        &self,
        tenant_id: &str,
        entity_id: Uuid,
        node_id: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> + Send;
}

/// Persistence port for partitioned `xb_metric_series` ingestion, multi-grain `xb_metric_rollups`, and window queries (Step 03).
pub trait MetricStorePort: Send + Sync {
    /// Writes up to 50,000 raw metric points, updates latest `xb_nodes.metrics`, marks dirty minutes, and enqueues `ROLLUP`.
    fn write_metrics(
        &self,
        tenant_id: &str,
        req: WriteMetricsRequest,
    ) -> impl Future<Output = Result<WriteMetricsResponse, AppError>> + Send;

    /// Queries time-series points for one node and metric with automatic or explicit grain selection and `> 5,000` promotion.
    fn query_node_metrics(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        metric: &str,
        query: NodeMetricSeriesQuery,
    ) -> impl Future<Output = Result<Vec<MetricSeriesPoint>, AppError>> + Send;

    /// Evaluates `TUMBLING` or `SLIDING` window aggregations across nodes/types.
    fn query_window(
        &self,
        tenant_id: &str,
        query: WindowQuery,
    ) -> impl Future<Output = Result<WindowQueryResponse, AppError>> + Send;

    /// Lists all `(type_name, metric, point_count, min_ts, max_ts)` tuples in the tenant.
    fn list_metric_catalog(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<MetricCatalogEntry>, AppError>> + Send;

    /// Executes incremental + dirty-bucket `MINUTE` → `HOUR` → `DAY` → `MONTH` rollup computation for a tenant.
    fn run_rollup_for_tenant(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<u64, AppError>> + Send;
}

/// Graph traversal (`k`-hop neighbors, shortest path, subgraph) and `petgraph` algorithm port (Step 03).
pub trait GraphQueryPort: Send + Sync {
    /// Expands `1..=4` hop neighbors around `query.node_id` using cycle-safe recursive CTEs.
    fn query_neighbors(
        &self,
        tenant_id: &str,
        query: NeighborQuery,
    ) -> impl Future<Output = Result<SubgraphResponse, AppError>> + Send;

    /// Computes unweighted (bidirectional BFS) or weighted (`petgraph::algo::dijkstra` with `1 / weight`) shortest path.
    fn query_shortest_path(
        &self,
        tenant_id: &str,
        query: PathQuery,
    ) -> impl Future<Output = Result<PathResponse, AppError>> + Send;

    /// Loads an initial canvas subgraph filtered by `type_names` and `link_types`.
    fn query_subgraph(
        &self,
        tenant_id: &str,
        query: SubgraphQuery,
    ) -> impl Future<Output = Result<SubgraphResponse, AppError>> + Send;

    /// Executes a `GraphAlgo`, persists `xb_graph_algo_results`, and writes `algo_<name>` metric points.
    fn run_algorithm(
        &self,
        tenant_id: &str,
        algo: GraphAlgo,
        req: AlgoRunRequest,
    ) -> impl Future<Output = Result<AlgoRunResponse, AppError>> + Send;

    /// Reads paginated persisted results from `xb_graph_algo_results`.
    fn list_algorithm_results(
        &self,
        tenant_id: &str,
        algo: GraphAlgo,
        query: AlgoResultsQuery,
    ) -> impl Future<Output = Result<AlgoResultsResponse, AppError>> + Send;
}

/// Persistence port for `AxAnalytics` semantic metrics, saved queries, history, ask turns, and transform drafts (`05-STEP04`).
pub trait AnalyticsRepository: Send + Sync {
    /// Lists all semantic metrics in `tenant_id`, computing `broken = true` when referenced columns are missing from current dataset schema.
    fn list_metrics(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<MetricDef>, AppError>> + Send;

    /// Loads a semantic metric by unique `name` in `tenant_id`.
    fn get_metric_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<Option<MetricDef>, AppError>> + Send;

    /// Creates a semantic metric in `an_metrics`, records `ds_lineage(DATASET -> METRIC)`, and enqueues `EMBED_NODES`.
    fn create_metric(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        schema_version: i32,
        req: CreateMetricRequest,
    ) -> impl Future<Output = Result<MetricDef, AppError>> + Send;

    /// Updates a semantic metric by `name`, incrementing `version` when definition fields change.
    fn update_metric(
        &self,
        tenant_id: &str,
        name: &str,
        schema_version: i32,
        req: UpdateMetricRequest,
    ) -> impl Future<Output = Result<Option<MetricDef>, AppError>> + Send;

    /// Deletes a semantic metric by `name` in `tenant_id`, cleaning up its `AN_METRIC` embeddings and lineage.
    fn delete_metric(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Lists saved queries visible to `viewer_user_id` (`shared = true` OR `owner_user_id = viewer_user_id`, or all when `include_all = true`).
    fn list_saved_queries(
        &self,
        tenant_id: &str,
        viewer_user_id: Uuid,
        include_all: bool,
    ) -> impl Future<Output = Result<Vec<SavedQuery>, AppError>> + Send;

    /// Loads a single saved query by UUID in `tenant_id`.
    fn get_saved_query(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<SavedQuery>, AppError>> + Send;

    /// Creates a saved query in `an_saved_queries`.
    fn create_saved_query(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        req: CreateSavedQueryRequest,
    ) -> impl Future<Output = Result<SavedQuery, AppError>> + Send;

    /// Updates a saved query in `an_saved_queries`.
    fn update_saved_query(
        &self,
        tenant_id: &str,
        id: Uuid,
        req: UpdateSavedQueryRequest,
    ) -> impl Future<Output = Result<Option<SavedQuery>, AppError>> + Send;

    /// Deletes a saved query by UUID in `tenant_id`.
    fn delete_saved_query(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Appends an execution record to `an_query_history` (truncating `sql_text` to 4,000 characters).
    fn record_query_history(
        &self,
        tenant_id: &str,
        entry: NewQueryHistoryEntry,
    ) -> impl Future<Output = Result<i64, AppError>> + Send;

    /// Lists query execution history (`only_user_id = Some(uid)` restricts to caller; `None` returns whole tenant for `DATA_ADMIN`).
    fn list_query_history(
        &self,
        tenant_id: &str,
        only_user_id: Option<Uuid>,
        query: QueryHistoryListQuery,
    ) -> impl Future<Output = Result<PaginatedList<QueryHistoryEntry>, AppError>> + Send;

    /// Inserts a conversation turn into `an_ask_turns`.
    fn insert_ask_turn(
        &self,
        tenant_id: &str,
        turn: NewAskTurnInput,
    ) -> impl Future<Output = Result<AskTurnRecord, AppError>> + Send;

    /// Fetches a single `an_ask_turns` row by UUID.
    fn get_ask_turn(
        &self,
        tenant_id: &str,
        turn_id: Uuid,
    ) -> impl Future<Output = Result<Option<AskTurnRecord>, AppError>> + Send;

    /// Records `UP`/`DOWN` feedback and optional guarded `corrected_sql` on an `an_ask_turns` row.
    fn update_ask_feedback(
        &self,
        tenant_id: &str,
        turn_id: Uuid,
        feedback: AskFeedbackKind,
        corrected_sql: Option<&str>,
    ) -> impl Future<Output = Result<Option<AskTurnRecord>, AppError>> + Send;

    /// Lists all turns in `session_id` ordered chronologically (`created_at ASC`).
    fn list_session_turns(
        &self,
        tenant_id: &str,
        session_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AskTurnRecord>, AppError>> + Send;

    /// Searches up to `limit` positive-feedback (`feedback = 'UP'` or `corrected_sql IS NOT NULL`) turns by FTS similarity.
    fn search_few_shot_turns(
        &self,
        tenant_id: &str,
        question: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<FewShotTurn>, AppError>> + Send;

    /// Recalls candidate columns, semantic metrics, and business nodes for NL2SQL schema linking (`§04.6.3`).
    fn recall_schema_candidates(
        &self,
        tenant_id: &str,
        question: &str,
        query_embedding: Option<&[f32]>,
        restrict_datasets: &[Uuid],
    ) -> impl Future<Output = Result<SchemaRecallBundle, AppError>> + Send;

    /// Lists `(node_type, node_id, text)` items (`DS_COLUMN` and `AN_METRIC`) missing embeddings for `model_name`.
    fn list_unembedded_analytics_items(
        &self,
        tenant_id: &str,
        model_name: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(String, Uuid, String)>, AppError>> + Send;

    /// Upserts `DS_COLUMN` and `AN_METRIC` vectors into `hg_embeddings`.
    fn upsert_analytics_embeddings(
        &self,
        tenant_id: &str,
        model_name: &str,
        dim: usize,
        items: &[(String, Uuid, Vec<f32>)],
    ) -> impl Future<Output = Result<usize, AppError>> + Send;

    /// Creates a `DRAFT` row in `an_transform_drafts`.
    fn create_transform_draft(
        &self,
        tenant_id: &str,
        input: NewTransformDraftInput,
    ) -> impl Future<Output = Result<TransformDraft, AppError>> + Send;

    /// Loads a transform draft by UUID in `tenant_id`.
    fn get_transform_draft(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> + Send;

    /// Updates `sql_text` and `preview` on an editable (`DRAFT`) transform draft.
    fn update_transform_draft(
        &self,
        tenant_id: &str,
        id: Uuid,
        sql_text: &str,
        preview: &TransformPreview,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> + Send;

    /// Marks a transform draft `APPLIED` and links the created `dataset_id`.
    fn mark_transform_applied(
        &self,
        tenant_id: &str,
        id: Uuid,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> + Send;

    /// Marks a transform draft `DISCARDED`.
    fn mark_transform_discarded(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> + Send;

    /// Returns `DERIVED_VIEW` datasets in `tenant_id` whose `source_ref.depends_on` includes `upstream_dataset_id`, ordered topologically.
    fn list_dependent_derived_datasets(
        &self,
        tenant_id: &str,
        upstream_dataset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<Dataset>, AppError>> + Send;
}

/// Domain query execution port exposed by `AxAnalytics` to `AxFlow` (Step 07), `AxAgent` (Step 09), and `xBlueView` (Step 11) (`05-STEP04` §04.9).
pub trait AnalyticsQueryPort: Send + Sync {
    /// Executes a guarded parameterized SQL query under `principal`.
    fn run_sql(
        &self,
        principal: &AnalyticsPrincipal,
        req: QueryRequest,
    ) -> impl Future<Output = Result<QueryResult, AppError>> + Send;

    /// Executes a saved query (`SQL`, `PIVOT`, `METRIC`, or `ASK`) by UUID with runtime `:params`.
    fn run_saved(
        &self,
        principal: &AnalyticsPrincipal,
        id: Uuid,
        params: BTreeMap<String, serde_json::Value>,
    ) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send;

    /// Compiles and executes a semantic metric query by metric `name`.
    fn run_metric(
        &self,
        principal: &AnalyticsPrincipal,
        name: &str,
        query: MetricQuery,
    ) -> impl Future<Output = Result<MetricQueryResult, AppError>> + Send;

    /// Compiles, executes, and cross-tabulates a pivot specification.
    fn run_pivot(
        &self,
        principal: &AnalyticsPrincipal,
        spec: PivotSpec,
    ) -> impl Future<Output = Result<PivotResult, AppError>> + Send;
}

/// Persistence port for `xBlueStudio` subgraphs, drafts, optimistic schema publication,
/// cross-subgraph ontology alignment/merging, and dataset mappings (`06-STEP05-06` §05.4–§05.7).
pub trait StudioRepository: Send + Sync {
    /// Lists all subgraphs (`main` first, then department subgraphs) in `tenant_id`.
    fn list_subgraphs(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<SubgraphInfo>, AppError>> + Send;

    /// Creates a department subgraph (optionally seeding initial node types from `main`).
    fn create_subgraph(
        &self,
        tenant_id: &str,
        actor_user_id: Option<Uuid>,
        req: CreateSubgraphRequest,
    ) -> impl Future<Output = Result<SubgraphInfo, AppError>> + Send;

    /// Updates mutable display metadata of a subgraph.
    fn update_subgraph(
        &self,
        tenant_id: &str,
        slug: &str,
        req: UpdateSubgraphRequest,
    ) -> impl Future<Output = Result<SubgraphInfo, AppError>> + Send;

    /// Deletes a non-`main` department subgraph and its isolated drafts/versions/types/mappings.
    fn delete_subgraph(
        &self,
        tenant_id: &str,
        slug: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Reads the currently published `SchemaDoc` and head `version` for `subgraph_slug` (ensuring `main` exists).
    fn get_published_schema(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> impl Future<Output = Result<PublishedSchemaResponse, AppError>> + Send;

    /// Convenience direct save (`POST /api/xblue/schema?subgraph=`): creates a draft on current head and publishes immediately.
    fn save_schema_direct(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: SaveSchemaDirectRequest,
    ) -> impl Future<Output = Result<PublishedSchemaResponse, AppError>> + Send;

    /// Builds a [`ValidationContext`] hydrated from `tenant_id`'s `main` types, current subgraph doc, node/link counts, and `AxHub` datasets.
    fn load_validation_context(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> impl Future<Output = Result<ValidationContext, AppError>> + Send;

    /// Lists published schema versions for `subgraph_slug` ordered by `version DESC`.
    fn list_schema_versions(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> impl Future<Output = Result<Vec<SchemaVersionSummary>, AppError>> + Send;

    /// Fetches a specific published schema version snapshot (`None` if missing).
    fn get_schema_version(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        version: i32,
    ) -> impl Future<Output = Result<Option<SchemaVersionDetail>, AppError>> + Send;

    /// Computes structural diff between version `from_version` and `to_version` in `subgraph_slug`.
    fn diff_schema_versions(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        from_version: i32,
        to_version: i32,
    ) -> impl Future<Output = Result<SchemaDiff, AppError>> + Send;

    /// Creates a `source = RESTORE` draft from historical snapshot `version` based on current head version.
    fn restore_schema_version(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        version: i32,
        actor_user_id: Option<Uuid>,
    ) -> impl Future<Output = Result<SchemaDraft, AppError>> + Send;

    /// Lists schema drafts in `subgraph_slug` (optionally filtered by `status`).
    fn list_drafts(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        status: Option<DraftStatus>,
    ) -> impl Future<Output = Result<Vec<SchemaDraft>, AppError>> + Send;

    /// Fetches a single schema draft by UUID.
    fn get_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
    ) -> impl Future<Output = Result<Option<SchemaDraft>, AppError>> + Send;

    /// Creates a new schema draft, running `studio::validate` and storing computed `issues`.
    fn create_draft(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: CreateDraftRequest,
    ) -> impl Future<Output = Result<SchemaDraft, AppError>> + Send;

    /// Replaces `schema_doc` (and optional `title`) on an editable `DRAFT` and recomputes `issues`.
    fn update_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        req: UpdateDraftRequest,
    ) -> impl Future<Output = Result<SchemaDraft, AppError>> + Send;

    /// Marks an editable `DRAFT` as `DISCARDED`.
    fn discard_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
    ) -> impl Future<Output = Result<SchemaDraft, AppError>> + Send;

    /// Atomically validates and publishes `draft_id` under `pg_advisory_xact_lock` (`06-STEP05-06` §05.4.1).
    fn publish_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        actor_user_id: Option<Uuid>,
        req: PublishDraftRequest,
    ) -> impl Future<Output = Result<PublishDraftResponse, AppError>> + Send;

    /// Computes cross-subgraph entity alignment & conflict preview against `main` (`06-STEP05-06` §05.4.5).
    fn preview_subgraph_merge(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        req: MergePreviewRequest,
    ) -> impl Future<Output = Result<MergePreview, AppError>> + Send;

    /// Merges a department subgraph's published `SchemaDoc` into `main` atomically (`06-STEP05-06` §05.4.5).
    fn merge_subgraph_into_main(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: MergeSubgraphRequest,
    ) -> impl Future<Output = Result<PublishDraftResponse, AppError>> + Send;

    /// Lists persisted `xb_mappings` in `subgraph_slug`.
    fn list_mappings(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> impl Future<Output = Result<Vec<MappingRecord>, AppError>> + Send;

    /// Fetches a single `xb_mappings` row by UUID.
    fn get_mapping(
        &self,
        tenant_id: &str,
        mapping_id: Uuid,
    ) -> impl Future<Output = Result<Option<MappingRecord>, AppError>> + Send;

    /// Lists execution runs for `mapping_id` ordered by `started_at DESC`.
    fn list_mapping_runs(
        &self,
        tenant_id: &str,
        mapping_id: Uuid,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<MappingRunRecord>, AppError>> + Send;

    /// Enqueues a `MAP_MATERIALIZE` job for a `main` mapping (`POST /api/xblue/mappings/{id}/run`).
    fn enqueue_mapping_run(
        &self,
        tenant_id: &str,
        mapping_id: Uuid,
        req: TriggerMappingRunRequest,
    ) -> impl Future<Output = Result<TriggerMappingRunResponse, AppError>> + Send;

    /// Extracts hypergraph entity groups and `RELATES` triples from `doc_ids` for AI schema draft generation (`06-STEP05-06` §05.5 step 2).
    fn load_doc_entity_groups(
        &self,
        tenant_id: &str,
        doc_ids: &[Uuid],
    ) -> impl Future<Output = Result<Vec<DocEntityGroup>, AppError>> + Send;
}

/// Internal trusted query port for reading `DuckLake` tables and change feeds inside background workers (`06-STEP05-06` §06.2.1).
pub trait LakeInternalQueryPort: Send + Sync {
    /// Executes a trusted code-generated `SELECT` statement against `tenant_id`'s `DuckLake` session (capped at `max_rows`).
    fn query_internal(
        &self,
        tenant_id: &str,
        sql: &str,
        max_rows: usize,
    ) -> impl Future<Output = Result<QueryResult, AppError>> + Send;

    /// Invalidates any cached `DuckDB` session for `tenant_id` so subsequent reads observe newly committed `DuckLake` snapshots.
    fn invalidate_tenant_session(&self, tenant_id: &str) -> impl Future<Output = bool> + Send;
}

/// Batch text embedding port decoupling `axpoint-db` knowledge sync from `axpoint-llm` (`06-STEP05-06` §06.5).
pub trait TextEmbedderPort: Send + Sync {
    /// Returns the configured embedding model identifier.
    fn embedding_model_name(&self) -> &str;

    /// Generates embeddings for a batch of texts, returning `(vectors, embedding_dim)`.
    fn embed_texts(
        &self,
        texts: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send;
}

/// Execution port for `xBlueStudio` mapping materialization, dynamic metric rollup, explainability, and knowledge sync (`06-STEP05-06` §06.2–§06.6).
pub trait StudioExecutionPort: Send + Sync {
    /// Executes `MAP_MATERIALIZE` (`FULL` or `INCREMENTAL`) in 5,000-row chunks and records `xb_mapping_runs`.
    fn execute_mapping_materialize<L: LakeInternalQueryPort>(
        &self,
        tenant_id: &str,
        job_id: Uuid,
        payload: &MapMaterializeJobPayload,
        lake: &L,
    ) -> impl Future<Output = Result<MappingRunRecord, AppError>> + Send;

    /// Lists enabled `main` subgraph mappings bound to `dataset_id` (used by `dataset.synced` subscriber).
    fn list_enabled_main_mappings_for_dataset(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<MappingRecord>, AppError>> + Send;

    /// Executes `GRAPH_AGGREGATE` under session-level `pg_advisory_lock(hashtext('xb_agg:' || tenant))`.
    fn execute_graph_aggregate(
        &self,
        tenant_id: &str,
        payload: &GraphAggregateJobPayload,
    ) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send;

    /// Enqueues a `JobKind::GraphAggregate` job with optional deduplicating `idempotency_key`.
    fn enqueue_graph_aggregate(
        &self,
        tenant_id: &str,
        payload: GraphAggregateJobPayload,
        idempotency_key: Option<String>,
    ) -> impl Future<Output = Result<Uuid, AppError>> + Send;

    /// Explains how `metric` on `node_id` was produced (`AGG`, `DERIVED`, `SYNC`, `MANUAL`, `ACTION`, `ALGORITHM`) along with upstream inputs and warnings.
    fn explain_node_metric(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        metric: &str,
    ) -> impl Future<Output = Result<MetricExplainResponse, AppError>> + Send;

    /// Executes `KNOWLEDGE_SYNC` (upserting or deleting `xb_node:<node_id>` hypergraph dossiers and `EXACT_KEY` links).
    fn execute_knowledge_sync<E: TextEmbedderPort>(
        &self,
        tenant_id: &str,
        payload: &KnowledgeSyncJobPayload,
        embedder: &E,
    ) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send;

    /// Lists unlinked hypergraph entities (`occur_count >= 3`) whose `entity_type` matches a `main` `NodeType` (`name` or `display_name`).
    fn list_knowledge_node_candidates(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<KnowledgeNodeCandidatesResponse, AppError>> + Send;

    /// Promotes a candidate `hg_entities` row into a `BlueNode` and confirms its `xb_entity_links` entry.
    fn accept_knowledge_node_candidate(
        &self,
        tenant_id: &str,
        entity_id: Uuid,
        req: AcceptNodeCandidateRequest,
    ) -> impl Future<Output = Result<AcceptNodeCandidateResponse, AppError>> + Send;
}

/// Persistence port for `AxFlow` DAG definitions, versions, deployments, runs, step logs,
/// human approvals, writeback action logs, and inbound flow webhooks (`07-STEP07-08` §07).
pub trait FlowRepository: Send + Sync {
    /// Lists `AxFlow` pipelines in `tenant_id` with optional search and cursor pagination.
    fn list_flows(
        &self,
        tenant_id: &str,
        query: FlowListQuery,
    ) -> impl Future<Output = Result<PaginatedList<AxFlowDef>, AppError>> + Send;

    /// Fetches a single `AxFlow` draft/definition by UUID in `tenant_id`.
    fn get_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> impl Future<Output = Result<Option<AxFlowDef>, AppError>> + Send;

    /// Creates a new `AxFlow` draft (`status = 'DRAFT'`) in `tenant_id`.
    fn create_flow(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxFlowRequest,
    ) -> impl Future<Output = Result<AxFlowDef, AppError>> + Send;

    /// Updates mutable draft fields (`name`, `description`, `slug`, `nodes`, `edges`, `trigger_cfg`) on an `AxFlow`.
    fn update_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        req: UpdateAxFlowRequest,
    ) -> impl Future<Output = Result<Option<AxFlowDef>, AppError>> + Send;

    /// Deletes an `AxFlow` and cascades its versions, deployments, runs, step logs, approvals, and hooks.
    fn delete_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Snapshots the current flow draft into `ax_flow_versions(version = max + 1)` and sets `ax_flows.status = 'PUBLISHED'`.
    fn publish_flow_version(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        published_by: Option<Uuid>,
        req: PublishFlowVersionRequest,
    ) -> impl Future<Output = Result<FlowVersionRecord, AppError>> + Send;

    /// Lists published versions (`version DESC`) together with active `TEST`/`PROD` deployments for `flow_id`.
    fn list_flow_versions(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> impl Future<Output = Result<FlowVersionsListResponse, AppError>> + Send;

    /// Fetches a specific published [`FlowVersionRecord`] `(flow_id, version)` in `tenant_id`.
    fn get_flow_version(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        version: i32,
    ) -> impl Future<Output = Result<Option<FlowVersionRecord>, AppError>> + Send;

    /// Upserts `(flow_id, environment)` in `ax_flow_deployments` pointing to an existing published `version`.
    fn deploy_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        deployed_by: Option<Uuid>,
        req: DeployFlowRequest,
    ) -> impl Future<Output = Result<FlowDeploymentRecord, AppError>> + Send;

    /// Fetches the active [`FlowDeploymentRecord`] for `(flow_id, environment)`, if deployed.
    fn get_flow_deployment(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        environment: Environment,
    ) -> impl Future<Output = Result<Option<FlowDeploymentRecord>, AppError>> + Send;

    /// Resolves target version, deduplicates on `(flow_id, idempotency_key)`, inserts `ax_flow_runs`,
    /// creates root `PENDING` step rows, and enqueues initial `FLOW_STEP` jobs (`(run, existing)`).
    fn start_flow_run(
        &self,
        tenant_id: &str,
        params: StartRunParams,
    ) -> impl Future<Output = Result<(RunRecord, bool), AppError>> + Send;

    /// Fetches a single [`RunRecord`] by UUID in `tenant_id`.
    fn get_flow_run(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> impl Future<Output = Result<Option<RunRecord>, AppError>> + Send;

    /// Lists runs for `flow_id` ordered by `created_at DESC` with cursor pagination.
    fn list_flow_runs(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        query: RunListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RunRecord>, AppError>> + Send;

    /// Lists all step attempt records for `run_id` ordered by `(id ASC)`.
    fn list_run_steps(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> impl Future<Output = Result<Vec<StepRecord>, AppError>> + Send;

    /// Lists the highest-`attempt` [`StepRecord`] per `node_id` for `run_id`.
    fn list_latest_run_steps(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> impl Future<Output = Result<Vec<StepRecord>, AppError>> + Send;

    /// Marks `(run_id, node_id, attempt)` as `RUNNING` and records rendered `input` and `job_id`.
    fn mark_step_running(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        node_id: &str,
        attempt: i32,
        job_id: Option<Uuid>,
        rendered_input: &serde_json::Value,
    ) -> impl Future<Output = Result<StepRecord, AppError>> + Send;

    /// Looks up a previously cached inline `output` for `(run_id, node_id)` matching `rendered_input` (used by `LLM_EXTRACT` retries).
    fn find_cached_step_output(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        node_id: &str,
        rendered_input: &serde_json::Value,
    ) -> impl Future<Output = Result<Option<serde_json::Value>, AppError>> + Send;

    /// Locks `ax_flow_runs` via `SELECT ... FOR UPDATE`, applies `mutation`, runs `planner`,
    /// and commits skipped nodes, newly ready `FLOW_STEP` jobs, and terminal run status in one transaction.
    fn apply_step_mutation_and_progress(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        mutation: StepMutation,
        planner: ProgressionPlannerFn<'_>,
    ) -> impl Future<Output = Result<RunRecord, AppError>> + Send;

    ///Cancels an active run (`CANCELED`), marks `PENDING` steps `SKIPPED`, and cancels queued `FLOW_STEP` jobs.
    fn cancel_flow_run(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> impl Future<Output = Result<RunRecord, AppError>> + Send;

    /// Re-queues terminal `FAILED` nodes on a `FAILED` run as `attempt + 1` and transitions the run back to `RUNNING`.
    fn retry_flow_run(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> impl Future<Output = Result<RunRecord, AppError>> + Send;

    /// Idempotently creates or returns an existing `ax_approvals` row for `(run_id, node_id)` (`07-STEP07-08` §07.4).
    fn create_or_get_approval(
        &self,
        tenant_id: &str,
        input: NewApprovalInput,
    ) -> impl Future<Output = Result<ApprovalRecord, AppError>> + Send;

    /// Fetches a single [`ApprovalRecord`] by UUID in `tenant_id`.
    fn get_approval(
        &self,
        tenant_id: &str,
        approval_id: Uuid,
    ) -> impl Future<Output = Result<Option<ApprovalRecord>, AppError>> + Send;

    /// Lists approvals in `tenant_id` (optionally filtered by `status`, `run_id`, and `mine` against `viewer_user_id` / `viewer_roles`).
    fn list_approvals(
        &self,
        tenant_id: &str,
        viewer_user_id: Uuid,
        viewer_roles: &[String],
        query: ApprovalListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ApprovalRecord>, AppError>> + Send;

    /// Enforces assignee authorization (`403 PERMISSION_DENIED` if ineligible), updates `ax_approvals`,
    /// emits `approval.decided` to `ax_outbox`, and writes `APPROVAL_DECIDE` audit in one transaction.
    fn decide_approval(
        &self,
        tenant_id: &str,
        params: DecideApprovalParams,
    ) -> impl Future<Output = Result<ApprovalRecord, AppError>> + Send;

    /// Marks overdue `PENDING` approvals in `tenant_id` (`expires_at <= NOW()`) as `EXPIRED` and emits `approval.decided` outbox events.
    fn expire_overdue_approvals(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<u64, AppError>> + Send;

    /// Inserts an `ax_action_log` row or returns `(existing_row, true)` on `(tenant_id, idempotency_key)` conflict.
    fn record_action_log(
        &self,
        tenant_id: &str,
        input: NewActionLogInput,
    ) -> impl Future<Output = Result<(ActionLogRecord, bool), AppError>> + Send;

    /// Looks up an existing `ax_action_log` row by `(tenant_id, idempotency_key)`.
    fn get_action_log_by_key(
        &self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> impl Future<Output = Result<Option<ActionLogRecord>, AppError>> + Send;

    /// Lists `ax_action_log` entries in `tenant_id` (optionally filtered by `run_id` / `status`).
    fn list_action_logs(
        &self,
        tenant_id: &str,
        query: ActionListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ActionLogRecord>, AppError>> + Send;

    /// Upserts `ax_flow_hooks` for `flow_id` pointing to sealed `credential_id` (returning `(hook_id, created_at)`).
    fn upsert_flow_hook(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        credential_id: Uuid,
    ) -> impl Future<Output = Result<(Uuid, chrono::DateTime<chrono::Utc>), AppError>> + Send;

    /// Resolves `(flow_id, credential_id)` for an inbound flow webhook via `ax_resolve_flow_hook(tenant, slug)`.
    fn resolve_flow_hook(
        &self,
        tenant_id: &str,
        slug: &str,
    ) -> impl Future<Output = Result<Option<ResolvedFlowHook>, AppError>> + Send;

    /// Lists all `PROD`-deployed `CRON` flows across tenants via `ax_scheduled_flows()`.
    fn list_scheduled_flows(
        &self,
    ) -> impl Future<Output = Result<Vec<ScheduledFlowItem>, AppError>> + Send;

    /// Lists all `PROD`-deployed `OUTBOX_EVENT` flows matching `topic` across tenants via `ax_event_flows(topic)`.
    fn list_event_flows(
        &self,
        topic: &str,
    ) -> impl Future<Output = Result<Vec<EventFlowItem>, AppError>> + Send;

    /// Lists active tenant IDs from `sec_tenants` for periodic approval-expiration scanning.
    fn list_active_tenants(&self) -> impl Future<Output = Result<Vec<String>, AppError>> + Send;
}

/// Object-safe SSRF outbound URL guard port (`07-STEP07-08` §07.0 & Step 01 §01.1).
pub trait OutboundUrlGuardPort: Send + Sync {
    /// Validates URL scheme, hostname, and resolved DNS IP addresses against SSRF rules.
    fn check_outbound_url<'a>(
        &'a self,
        url: &'a str,
        allow_private_network: bool,
    ) -> BoxFuture<'a, String>;
}

/// Object-safe port materializing a guarded `SQL_TRANSFORM` query into a `DERIVED_VIEW` dataset (`07-STEP07-08` §07.4).
pub trait FlowSqlMaterializePort: Send + Sync {
    /// Creates or refreshes a `DERIVED_VIEW` dataset named `dataset_name` backed by `sql` and returns its `dataset_id`.
    fn materialize_sql_dataset<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        dataset_name: &'a str,
        sql: &'a str,
    ) -> BoxFuture<'a, Uuid>;
}

/// Object-safe port unsealing `ds_credentials` JSON secrets for `HTTP_CALL` authentication (`07-STEP07-08` §07.4).
pub trait FlowCredentialUnsealPort: Send + Sync {
    /// Loads and decrypts `credential_id` within `tenant_id` into a JSON object.
    fn unseal_credential_json<'a>(
        &'a self,
        tenant_id: &'a str,
        credential_id: Uuid,
    ) -> BoxFuture<'a, serde_json::Value>;
}

/// Persistence port for operational alerting rules, deduplicated alerts, timeline events,
/// notification channels, delivery retries, and silences (`07-STEP07-08` §08).
pub trait RulesRepository: Send + Sync {
    /// Lists alerting rules in `tenant_id` with optional filters and cursor pagination.
    fn list_rules(
        &self,
        tenant_id: &str,
        query: RuleListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RuleDef>, AppError>> + Send;

    /// Lists all enabled rules matching `scope_kind` in `tenant_id`.
    fn list_enabled_rules_by_scope(
        &self,
        tenant_id: &str,
        scope_kind: RuleScopeKind,
    ) -> impl Future<Output = Result<Vec<RuleDef>, AppError>> + Send;

    /// Fetches a single [`RuleDef`] by UUID in `tenant_id`.
    fn get_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
    ) -> impl Future<Output = Result<Option<RuleDef>, AppError>> + Send;

    /// Creates a new alerting rule in `tenant_id` (`409` on duplicate `(tenant_id, name)`).
    fn create_rule(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateRuleRequest,
    ) -> impl Future<Output = Result<RuleDef, AppError>> + Send;

    /// Updates mutable fields on `rule_id` (`version = version + 1`); if `enabled` becomes `false`,
    /// resolves all active alerts of `rule_id` with `resolve_reason = RULE_DISABLED` in the same transaction.
    fn update_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
        req: UpdateRuleRequest,
    ) -> impl Future<Output = Result<Option<RuleDef>, AppError>> + Send;

    /// Deletes `rule_id` in `tenant_id` (cascading associated alerts and events).
    fn delete_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Lists alerts in `tenant_id` filtered by `status`, `severity`, `rule_id`, `node_id`, or `type_name`.
    fn list_alerts(
        &self,
        tenant_id: &str,
        query: AlertListQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertRecord>, AppError>> + Send;

    /// Fetches a single [`AlertRecord`] (including its `deliveries`) by UUID in `tenant_id`.
    fn get_alert(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
    ) -> impl Future<Output = Result<Option<AlertRecord>, AppError>> + Send;

    /// Lists all active (`status <> 'RESOLVED'`) alerts owned by `rule_id` in `tenant_id`.
    fn list_active_alerts_for_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, AppError>> + Send;

    /// Lists timeline events (`ax_alert_events`) for `alert_id` ordered by `id ASC`.
    fn list_alert_events(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        query: AlertEventsQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertEventRecord>, AppError>> + Send;

    /// Appends a timeline event to `ax_alert_events` for `alert_id`.
    fn append_alert_event(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        event: AlertEventKind,
        detail: serde_json::Value,
        actor: Option<Uuid>,
    ) -> impl Future<Output = Result<AlertEventRecord, AppError>> + Send;

    /// Records the triggered `AxFlow` `(flow_id, run_id)` on `alert_id` (`detail` + `FLOW_STARTED` event).
    fn set_alert_flow_run(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        flow_id: Uuid,
        run_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Applies the `true`-condition alert transition (`07-STEP07-08` §08.3.2):
    /// checks cooldown on the latest `RESOLVED` alert, inserts a new `OPEN` alert guarded by `uq_ax_alerts_active`
    /// (falling back to `RETRIGGERED` with 60s event merge on conflict), and emits `alert.opened` on new open.
    fn record_true_condition(
        &self,
        tenant_id: &str,
        input: TrueConditionInput,
    ) -> impl Future<Output = Result<TrueConditionOutcome, AppError>> + Send;

    /// Applies the `false`-condition alert transition (`07-STEP07-08` §08.3.2):
    /// sets `last_false_at` if empty, and when `now - last_false_at >= resolve_after_minutes`,
    /// resolves the active alert (`AUTO`), appends `RESOLVED`, and emits `alert.resolved`.
    fn record_false_condition(
        &self,
        tenant_id: &str,
        dedup_key: &str,
        resolve_after_minutes: i32,
        now: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<FalseConditionOutcome, AppError>> + Send;

    /// Marks overdue unacknowledged `OPEN` alerts (`escalated_at IS NULL` and `now - first_seen_at >= escalate_after_minutes`)
    /// on `rule_id` as `ESCALATED`, appends `ESCALATED` events, and returns the escalated alerts.
    fn escalate_due_alerts(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
        escalate_after_minutes: i32,
        now: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, AppError>> + Send;

    /// Transitions an `OPEN` (or idempotently `ACK`) alert to `ACK` and appends an `ACKED` event.
    fn ack_alert(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        actor: Option<Uuid>,
        comment: &str,
    ) -> impl Future<Output = Result<AlertRecord, AppError>> + Send;

    /// Batch-acknowledges `OPEN` alerts in `alert_ids` within `tenant_id`.
    fn ack_alerts_batch(
        &self,
        tenant_id: &str,
        alert_ids: &[Uuid],
        actor: Option<Uuid>,
        comment: &str,
    ) -> impl Future<Output = Result<AckBatchResponse, AppError>> + Send;

    /// Manually resolves an active alert (`resolve_reason = MANUAL`), appends `RESOLVED`, and emits `alert.resolved`.
    fn resolve_alert_manual(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        actor: Option<Uuid>,
        comment: &str,
    ) -> impl Future<Output = Result<AlertRecord, AppError>> + Send;

    /// Lists notification channels in `tenant_id` (populating `secret_fields_set` from credential metadata or unsealing at route layer).
    fn list_channels(
        &self,
        tenant_id: &str,
        query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<NotifyChannelRecord>, AppError>> + Send;

    /// Loads a notification channel and its optional sealed `ds_credentials` row in `tenant_id`.
    fn get_channel(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
    ) -> impl Future<
        Output = Result<Option<(NotifyChannelRecord, Option<SealedCredential>)>, AppError>,
    > + Send;

    /// Creates a notification channel (and inserts `ds_credentials` when `new_sealed_secret` is provided).
    fn create_channel(
        &self,
        tenant_id: &str,
        input: SaveNotifyChannelInput,
    ) -> impl Future<Output = Result<NotifyChannelRecord, AppError>> + Send;

    /// Updates a notification channel and its optional sealed credential binding.
    fn update_channel(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
        input: SaveNotifyChannelInput,
    ) -> impl Future<Output = Result<Option<NotifyChannelRecord>, AppError>> + Send;

    /// Deletes a notification channel in `tenant_id`.
    fn delete_channel(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Idempotently creates or returns an existing `ax_notify_deliveries` row (`(delivery, existing)`).
    fn enqueue_delivery(
        &self,
        tenant_id: &str,
        input: EnqueueDeliveryInput,
    ) -> impl Future<Output = Result<(NotifyDeliveryRecord, bool), AppError>> + Send;

    /// Fetches a single [`NotifyDeliveryRecord`] by UUID in `tenant_id`.
    fn get_delivery(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
    ) -> impl Future<Output = Result<Option<NotifyDeliveryRecord>, AppError>> + Send;

    /// Marks `delivery_id` as `SENT` (`sent_at = NOW()`) and, when bound to an `alert_id`, appends `NOTIFIED` to `ax_alert_events`.
    fn mark_delivery_sent(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
        attempt: i32,
        response_code: Option<i32>,
    ) -> impl Future<Output = Result<NotifyDeliveryRecord, AppError>> + Send;

    /// Records a failed delivery attempt: keeps `PENDING` with updated `next_retry_at` if `Some`,
    /// or marks terminal `FAILED` (and appends `NOTIFY_FAILED` to `ax_alert_events` if `alert_id` is set) when `None`.
    fn mark_delivery_failed(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
        update: DeliveryFailureUpdate,
    ) -> impl Future<Output = Result<NotifyDeliveryRecord, AppError>> + Send;

    /// Lists alert silence windows in `tenant_id`.
    fn list_silences(
        &self,
        tenant_id: &str,
        query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertSilenceRecord>, AppError>> + Send;

    /// Lists currently active silences (`starts_at <= at && at < ends_at`) in `tenant_id`.
    fn list_active_silences(
        &self,
        tenant_id: &str,
        at: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<Vec<AlertSilenceRecord>, AppError>> + Send;

    /// Creates a new [`AlertSilenceRecord`] (`ends_at > starts_at` validated).
    fn create_silence(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateSilenceRequest,
    ) -> impl Future<Output = Result<AlertSilenceRecord, AppError>> + Send;

    /// Deletes an alert silence window by UUID in `tenant_id`.
    fn delete_silence(
        &self,
        tenant_id: &str,
        silence_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Queries raw numeric points `(ts, value)` from `xb_metric_series` for `(node_id, metric)` ordered newest-first (`ts DESC`),
    /// optionally restricted to `ts >= since`, capped at `limit`.
    fn query_raw_metric_points(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        metric: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(chrono::DateTime<chrono::Utc>, f64)>, AppError>> + Send;

    /// Loads `dataset_id` together with the `quality_result` of its most recent `SUCCEEDED` `ds_sync_jobs` row.
    fn get_latest_dataset_quality(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> impl Future<Output = Result<Option<(Dataset, Option<QualityReport>)>, AppError>> + Send;

    /// Scans `ax_absence_rules()` across tenants (`ABSENCE` rules and rules with active alerts).
    fn list_absence_rules(
        &self,
    ) -> impl Future<Output = Result<Vec<AbsenceRuleItem>, AppError>> + Send;

    /// Scans `ax_due_notify_deliveries(limit)` across tenants for due `PENDING` notification deliveries.
    fn list_due_deliveries(
        &self,
        limit: i32,
    ) -> impl Future<Output = Result<Vec<DueDeliveryItem>, AppError>> + Send;
}

/// Object-safe simulation execution port decoupling `AxAgent` (`BuiltinToolRegistry`) from `AxSim` Step 10 (`08-STEP09` §09.0 & §09.5.1).
///
/// When `None` is injected into `BuiltinToolRegistry` (Step 09), `run_simulation` is hidden from the tool catalog;
/// when `Some(Arc<dyn SimExecutionPort>)` is wired in Step 10, `run_simulation` becomes available without modifying `axpoint-agent`.
pub trait SimExecutionPort: Send + Sync {
    /// Executes an `AxSim` scenario on behalf of `(tenant_id, user_id)` and returns a structured KPI/diff summary.
    fn run_simulation_tool<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: Uuid,
        input: SimRunToolInput,
    ) -> BoxFuture<'a, SimRunToolOutput>;
}

/// Object-safe port decoupling `AxFlow`'s `AGENT_TASK` operator (`axpoint-flow`) from `AgentRuntime` (`axpoint-agent`) (`08-STEP09` §09.6).
pub trait AgentTaskPort: Send + Sync {
    /// Runs or idempotently reuses an `AxAgent` conversation (`channel = FLOW`), returning `Completed` or `WaitingApproval`.
    fn execute_flow_agent_task(&self, input: AgentTaskInput) -> BoxFuture<'_, AgentTaskOutcome>;
}

/// Object-safe capability bridge used by `AxAgent` publishing, MCP Server, and built-in tools (`08-STEP09` §09.3–§09.5).
pub trait AgentCapabilityPort: Send + Sync {
    /// Executes five-way hypergraph document retrieval (`search_knowledge`), optionally scoped by `doc_title_prefix`.
    fn search_knowledge<'a>(
        &'a self,
        tenant_id: &'a str,
        query: &'a str,
        top_k: usize,
        doc_title_prefix: Option<&'a str>,
    ) -> BoxFuture<'a, Vec<FusedHit>>;

    /// Executes Step 04 NL2SQL `ask_data` under `principal`, optionally restricted to `dataset_names`.
    fn ask_data<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        question: &'a str,
        dataset_names: &'a [String],
    ) -> BoxFuture<'a, serde_json::Value>;

    /// Executes a `sql_guard`-validated `DuckDB` SQL query capped at `max_rows` (`run_sql` tool, max 200 rows).
    fn run_sql_bounded<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        sql: &'a str,
        max_rows: usize,
    ) -> BoxFuture<'a, QueryResult>;

    /// Executes a semantic metric query against either a `PROD`-pinned `MetricDef` snapshot or the live `an_metrics` definition.
    fn run_metric_endpoint<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        metric_name: &'a str,
        pinned_snapshot: Option<&'a MetricDef>,
        query: MetricQuery,
    ) -> BoxFuture<'a, MetricQueryResult>;

    /// Executes a saved query against either a `PROD`-pinned `SavedQuery` snapshot or the live `an_saved_queries` definition.
    fn run_saved_endpoint<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        saved_id: Uuid,
        pinned_snapshot: Option<&'a SavedQuery>,
        params: BTreeMap<String, serde_json::Value>,
    ) -> BoxFuture<'a, serde_json::Value>;

    /// Queries published `main` domain objects (`OBJECT_TYPE` endpoint and `query_objects` MCP/built-in tool),
    /// applying property masking, optional `pinned_snapshot` projection, and 1-hop `include_links` expansion.
    fn query_domain_objects<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        type_name: &'a str,
        pinned_snapshot: Option<&'a NodeTypeSpec>,
        payload: &'a serde_json::Value,
    ) -> BoxFuture<'a, serde_json::Value>;
}

/// Object-safe LLM provider lookup and daily token usage persistence port consumed by `axpoint-llm::router::LlmRouter` (`08-STEP09` §09.2).
pub trait LlmProviderStore: Send + Sync {
    /// Lists all `ax_llm_providers` rows for `tenant_id` ordered by `priority ASC, created_at ASC`.
    fn list_tenant_providers<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> BoxFuture<'a, Vec<LlmProviderRecord>>;

    /// Atomically increments daily request/error/token counters in `ax_llm_usage`.
    fn record_usage<'a>(&'a self, tenant_id: &'a str, delta: LlmUsageDelta) -> BoxFuture<'a, ()>;
}

/// Personal access token management port (`GET/POST /api/auth/tokens`, `DELETE /api/auth/tokens/{id}`, `08-STEP09` §09.8).
pub trait ApiTokenRepository: Send + Sync {
    /// Lists personal access tokens owned by `user_id` in `tenant_id` (excluding plaintext/hash).
    fn list_user_api_tokens(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> impl Future<Output = Result<Vec<PersonalAccessTokenSummary>, AppError>> + Send;

    /// Inserts a newly issued `sec_api_tokens` row in `tenant_id`.
    fn create_api_token(
        &self,
        tenant_id: &str,
        input: NewApiTokenInput,
    ) -> impl Future<Output = Result<PersonalAccessTokenSummary, AppError>> + Send;

    /// Revokes (`revoked_at = NOW()`) a token owned by `user_id` in `tenant_id` (`Ok(false)` if not found or owned by another user).
    fn revoke_api_token(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        token_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;
}

/// Persistence port for `AxAgent` providers, usage, published endpoints, MCP clients,
/// agents, versions, conversations, messages, and embed links (`08-STEP09` §09.1–§09.7).
pub trait AgentRepository: Send + Sync {
    /// Lists LLM providers in `tenant_id` ordered by `priority ASC, created_at ASC`.
    fn list_llm_providers(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<LlmProviderRecord>, AppError>> + Send;

    /// Fetches a single LLM provider (and its optional sealed API key credential) in `tenant_id`.
    fn get_llm_provider(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
    ) -> impl Future<
        Output = Result<Option<(LlmProviderRecord, Option<SealedCredential>)>, AppError>,
    > + Send;

    /// Creates a new LLM provider in `tenant_id` (`409` on duplicate `(tenant_id, name)`).
    fn create_llm_provider(
        &self,
        tenant_id: &str,
        input: SaveLlmProviderInput,
    ) -> impl Future<Output = Result<LlmProviderRecord, AppError>> + Send;

    /// Updates an existing LLM provider in `tenant_id`.
    fn update_llm_provider(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
        input: SaveLlmProviderInput,
    ) -> impl Future<Output = Result<Option<LlmProviderRecord>, AppError>> + Send;

    /// Deletes an LLM provider in `tenant_id`.
    fn delete_llm_provider(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Increments daily `ax_llm_usage` counters via `INSERT ... ON CONFLICT DO UPDATE`.
    fn record_llm_usage(
        &self,
        tenant_id: &str,
        delta: LlmUsageDelta,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Queries aggregated `ax_llm_usage` rows in `tenant_id` filtered by `[from, to]`.
    fn query_llm_usage(
        &self,
        tenant_id: &str,
        query: LlmUsageQuery,
    ) -> impl Future<Output = Result<Vec<LlmUsageRecord>, AppError>> + Send;

    /// Lists published endpoints in `tenant_id` with optional `environment`, `kind`, and `show_as_app` filters.
    fn list_endpoints(
        &self,
        tenant_id: &str,
        query: EndpointListQuery,
    ) -> impl Future<Output = Result<Vec<EndpointSpec>, AppError>> + Send;

    /// Fetches a single published endpoint by UUID in `tenant_id`.
    fn get_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
    ) -> impl Future<Output = Result<Option<EndpointSpec>, AppError>> + Send;

    /// Fetches a published endpoint by `(environment, slug)` in `tenant_id`.
    fn get_endpoint_by_slug(
        &self,
        tenant_id: &str,
        environment: Environment,
        slug: &str,
    ) -> impl Future<Output = Result<Option<EndpointSpec>, AppError>> + Send;

    /// Inserts a new published endpoint (`409` on duplicate `(tenant_id, environment, slug)`).
    fn create_endpoint(
        &self,
        tenant_id: &str,
        input: SaveEndpointInput,
    ) -> impl Future<Output = Result<EndpointSpec, AppError>> + Send;

    /// Updates mutable configuration fields on an endpoint by UUID.
    fn update_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
        req: UpdateEndpointRequest,
    ) -> impl Future<Output = Result<Option<EndpointSpec>, AppError>> + Send;

    /// Upserts a `PROD` endpoint on `(tenant_id, environment, slug)` during `/promote`.
    fn upsert_endpoint_by_slug(
        &self,
        tenant_id: &str,
        input: SaveEndpointInput,
    ) -> impl Future<Output = Result<EndpointSpec, AppError>> + Send;

    /// Deletes a published endpoint by UUID in `tenant_id`.
    fn delete_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Lists registered external MCP clients in `tenant_id`.
    fn list_mcp_clients(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<McpClientRecord>, AppError>> + Send;

    /// Fetches an external MCP client (and its optional sealed headers credential) by UUID.
    fn get_mcp_client(
        &self,
        tenant_id: &str,
        client_id: Uuid,
    ) -> impl Future<Output = Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError>> + Send;

    /// Fetches an external MCP client (and its optional sealed headers credential) by unique `name`.
    fn get_mcp_client_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError>> + Send;

    /// Registers a new external MCP client in `tenant_id`.
    fn create_mcp_client(
        &self,
        tenant_id: &str,
        input: SaveMcpClientInput,
    ) -> impl Future<Output = Result<McpClientRecord, AppError>> + Send;

    /// Deletes an external MCP client by UUID in `tenant_id`.
    fn delete_mcp_client(
        &self,
        tenant_id: &str,
        client_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Updates `tools_cache` and `last_synced_at = NOW()` after `POST /api/axagent/mcp-clients/{id}/sync`.
    fn update_mcp_client_tools(
        &self,
        tenant_id: &str,
        client_id: Uuid,
        tools: &[McpToolInfo],
    ) -> impl Future<Output = Result<Option<McpClientRecord>, AppError>> + Send;

    /// Lists `AxAgent` definitions in `tenant_id`.
    fn list_agents(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<AgentRecord>, AppError>> + Send;

    /// Fetches a single `AxAgent` definition by UUID in `tenant_id`.
    fn get_agent(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> impl Future<Output = Result<Option<AgentRecord>, AppError>> + Send;

    /// Fetches a single `AxAgent` definition by unique `name` in `tenant_id`.
    fn get_agent_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> impl Future<Output = Result<Option<AgentRecord>, AppError>> + Send;

    /// Creates a new `AxAgent` draft (`version = 0`) in `tenant_id`.
    fn create_agent(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAgentRequest,
    ) -> impl Future<Output = Result<AgentRecord, AppError>> + Send;

    /// Updates the draft `spec` (and optional `name`/`description`) of an `AxAgent`.
    fn update_agent(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        req: UpdateAgentRequest,
    ) -> impl Future<Output = Result<Option<AgentRecord>, AppError>> + Send;

    /// Deletes an `AxAgent` (cascading versions, conversations, messages, and embeds).
    fn delete_agent(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Snapshots `ax_agents.spec` into `ax_agent_versions(version + 1)` and increments `ax_agents.version`.
    fn publish_agent_version(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        published_by: Option<Uuid>,
    ) -> impl Future<Output = Result<AgentVersionRecord, AppError>> + Send;

    /// Fetches a specific published [`AgentVersionRecord`] `(agent_id, version)` in `tenant_id`.
    fn get_agent_version(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        version: i32,
    ) -> impl Future<Output = Result<Option<AgentVersionRecord>, AppError>> + Send;

    /// Lists all published versions of `agent_id` ordered by `version DESC`.
    fn list_agent_versions(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AgentVersionRecord>, AppError>> + Send;

    /// Creates a new `ax_conversations` session.
    fn create_conversation(
        &self,
        tenant_id: &str,
        input: NewConversationInput,
    ) -> impl Future<Output = Result<ConversationRecord, AppError>> + Send;

    /// Fetches a single `ax_conversations` row by UUID in `tenant_id`.
    fn get_conversation(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
    ) -> impl Future<Output = Result<Option<ConversationRecord>, AppError>> + Send;

    /// Looks up an existing `FLOW`-channel conversation by deterministic `title` (`flow:<idempotency_key>`).
    fn find_flow_conversation(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        idempotency_title: &str,
    ) -> impl Future<Output = Result<Option<ConversationRecord>, AppError>> + Send;

    /// Lists conversations owned by `user_id` (`GET /api/axagent/conversations` — caller sees only their own).
    fn list_user_conversations(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        query: ConversationListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ConversationRecord>, AppError>> + Send;

    /// Updates `status` and `updated_at = NOW()` on `conversation_id`.
    fn update_conversation_status(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
        status: ConversationStatus,
    ) -> impl Future<Output = Result<Option<ConversationRecord>, AppError>> + Send;

    /// Deletes a conversation owned by `user_id` (`None` skips user ownership check for admin/internal cleanup).
    fn delete_conversation(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
        owner_user_id: Option<Uuid>,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Appends a message to `ax_messages` and touches `ax_conversations.updated_at`.
    fn append_message(
        &self,
        tenant_id: &str,
        input: NewMessageInput,
    ) -> impl Future<Output = Result<MessageRecord, AppError>> + Send;

    /// Lists all messages in `conversation_id` ordered chronologically (`id ASC`).
    fn list_conversation_messages(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
    ) -> impl Future<Output = Result<Vec<MessageRecord>, AppError>> + Send;

    /// Inserts an `ax_agent_embeds` row in `tenant_id`.
    fn create_agent_embed(
        &self,
        tenant_id: &str,
        input: NewAgentEmbedInput,
    ) -> impl Future<Output = Result<AgentEmbedRecord, AppError>> + Send;

    /// Lists active and revoked embed links for `agent_id` in `tenant_id`.
    fn list_agent_embeds(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AgentEmbedRecord>, AppError>> + Send;

    /// Revokes (`revoked_at = NOW()`) an `ax_agent_embeds` row by UUID in `tenant_id`.
    fn revoke_agent_embed(
        &self,
        tenant_id: &str,
        embed_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Resolves an unauthenticated `axe_...` embed token digest via `ax_resolve_agent_embed`.
    fn resolve_agent_embed(
        &self,
        token_hash: &str,
    ) -> impl Future<Output = Result<Option<ResolvedAgentEmbed>, AppError>> + Send;
}

/// Persistence port for `AxSim` sandboxes, Copy-on-Write overlays, scenarios, checkpoints,
/// and production decision writebacks (`09-STEP10` §10.2–§10.8).
pub trait SimRepository: Send + Sync {
    /// Lists `AxSim` sandboxes in `tenant_id` ordered by `created_at DESC`.
    fn list_sandboxes(
        &self,
        tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<AxSandboxRecord>, AppError>> + Send;

    /// Fetches a single `AxSim` sandbox by UUID in `tenant_id`.
    fn get_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<Option<AxSandboxRecord>, AppError>> + Send;

    /// Creates a new `AxSim` sandbox and clones the scoped production subgraph into `ax_sim_base_*`.
    fn create_sandbox(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxSandboxRequest,
    ) -> impl Future<Output = Result<AxSandboxRecord, AppError>> + Send;

    /// Deletes an `AxSim` sandbox and all associated base, overlay, scenario, and decision rows.
    fn delete_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Loads the effective Copy-on-Write `SimGraph` (`base` merged with `overlay` plus `edge_specs`).
    fn load_graph(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<SimGraph, AppError>> + Send;

    /// Returns the merged Copy-on-Write sandbox graph view for canvas rendering.
    fn get_sandbox_graph(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<SandboxGraphResponse, AppError>> + Send;

    /// Computes the canonical overlay content hash for staleness tracking.
    fn compute_overlay_hash(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<String, AppError>> + Send;

    /// Upserts key-level node metric/property/title patches into `ax_sim_overlay_nodes`.
    fn patch_overlay_node(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        node_id: Uuid,
        req: PatchOverlayNodeRequest,
    ) -> impl Future<Output = Result<SandboxGraphNode, AppError>> + Send;

    /// Creates a sandbox-local node (`is_new = true`) inside `ax_sim_overlay_nodes`.
    fn create_overlay_node(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        req: CreateOverlayNodeRequest,
    ) -> impl Future<Output = Result<SandboxGraphNode, AppError>> + Send;

    /// Marks a node deleted in the sandbox overlay and hides incident sandbox links.
    fn delete_overlay_node(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        node_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Creates a sandbox-local link (`is_new = true`) inside `ax_sim_overlay_links`.
    fn create_overlay_link(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        req: CreateOverlayLinkRequest,
    ) -> impl Future<Output = Result<SandboxGraphLink, AppError>> + Send;

    /// Updates a link's type or weight inside `ax_sim_overlay_links`.
    fn patch_overlay_link(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        link_id: Uuid,
        req: PatchOverlayLinkRequest,
    ) -> impl Future<Output = Result<SandboxGraphLink, AppError>> + Send;

    /// Marks a link deleted in `ax_sim_overlay_links`.
    fn delete_overlay_link(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        link_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Clears all overlay edits for `sandbox_id`.
    fn reset_overlay(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Re-clones production baseline into `ax_sim_base_*` while preserving overlays and reporting orphans.
    fn refresh_sandbox_base(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<RefreshBaseResponse, AppError>> + Send;

    /// Runs a quick simulation without creating a named scenario and saves `ax_sandboxes.actions`/`sim_result`.
    fn quick_simulate_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        req: SimulateAxSandboxRequest,
    ) -> impl Future<Output = Result<AxSimResult, AppError>> + Send;

    /// Lists scenarios belonging to `sandbox_id` with live `is_stale` computation.
    fn list_scenarios(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> impl Future<Output = Result<Vec<ScenarioRecord>, AppError>> + Send;

    /// Fetches a single scenario by UUID in `tenant_id`.
    fn get_scenario(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> impl Future<Output = Result<Option<ScenarioRecord>, AppError>> + Send;

    /// Inserts a new `ax_sim_scenarios` record (`PENDING` or `RUNNING`).
    fn create_scenario_record(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        created_by: Option<Uuid>,
        overlay_hash: &str,
        req: CreateScenarioRequest,
    ) -> impl Future<Output = Result<ScenarioRecord, AppError>> + Send;

    /// Marks an existing scenario `RUNNING` ahead of recomputation.
    fn mark_scenario_running(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        overlay_hash: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Atomically persists solved `SimResult`, `ax_sim_impacts`, and `ax_sim_checkpoints`.
    fn save_scenario_success(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        overlay_hash: &str,
        result: &SimResult,
    ) -> impl Future<Output = Result<ScenarioRecord, AppError>> + Send;

    /// Marks a scenario `FAILED` with diagnostic `error_message`.
    fn save_scenario_failure(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        error_message: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Deletes a scenario by UUID in `tenant_id`.
    fn delete_scenario(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Lists paginated impact rows for `scenario_id` ordered by `|delta|` descending.
    fn list_scenario_impacts(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        query: ImpactListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ImpactRow>, AppError>> + Send;

    /// Loads all impact rows for `scenario_id` (used for scenario comparison and `DuckDB` `sim_ask`).
    fn list_all_scenario_impacts(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> impl Future<Output = Result<Vec<ImpactRow>, AppError>> + Send;

    /// Lists ordered replay checkpoints for `scenario_id`.
    fn list_scenario_checkpoints(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, AppError>> + Send;

    /// Creates a decision proposal and linked `ax_approvals(source_kind = SIM_APPLY)` gate.
    fn create_decision(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        created_by: Option<Uuid>,
        req: CreateDecisionRequest,
    ) -> impl Future<Output = Result<DecisionRecord, AppError>> + Send;

    /// Fetches a decision record by UUID in `tenant_id`.
    fn get_decision(
        &self,
        tenant_id: &str,
        decision_id: Uuid,
    ) -> impl Future<Output = Result<Option<DecisionRecord>, AppError>> + Send;

    /// Executes production writeback upon `APPROVE` (with `SELECT ... FOR UPDATE` conflict check)
    /// or marks the decision `REJECTED`.
    fn apply_or_reject_decision(
        &self,
        tenant_id: &str,
        decision_id: Uuid,
        decided_by: Option<Uuid>,
        approved: bool,
    ) -> impl Future<Output = Result<DecisionRecord, AppError>> + Send;

    /// Rebases a conflicted decision onto current production `xb_nodes.metrics` and reopens approval.
    fn rebase_decision(
        &self,
        tenant_id: &str,
        decision_id: Uuid,
        actor_user_id: Option<Uuid>,
    ) -> impl Future<Output = Result<DecisionRecord, AppError>> + Send;
}

/// Persistence port for `xBlueView` dashboards, widgets, assets, spatial twin bindings,
/// GIN dependency resolution, and transactional node action writebacks (`10-STEP11-12`).
pub trait ViewRepository: Send + Sync {
    /// Lists dashboards in `tenant_id` (`only_published = true` restricts to `PUBLISHED` status).
    fn list_dashboards(
        &self,
        tenant_id: &str,
        only_published: bool,
        query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<DashboardRecord>, AppError>> + Send;

    /// Creates a new `DRAFT` dashboard and returns its initial detail view.
    fn create_dashboard(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateDashboardRequest,
    ) -> impl Future<Output = Result<DashboardDetail, AppError>> + Send;

    /// Fetches a dashboard and its ordered widgets by UUID.
    fn get_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        require_published: bool,
    ) -> impl Future<Output = Result<Option<DashboardDetail>, AppError>> + Send;

    /// Fetches a dashboard and its ordered widgets by URL slug.
    fn get_dashboard_by_slug(
        &self,
        tenant_id: &str,
        slug: &str,
        require_published: bool,
    ) -> impl Future<Output = Result<Option<DashboardDetail>, AppError>> + Send;

    /// Updates mutable metadata and global filters on a dashboard.
    fn update_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        req: UpdateDashboardRequest,
    ) -> impl Future<Output = Result<DashboardRecord, AppError>> + Send;

    /// Deletes a dashboard and cascades to its widgets.
    fn delete_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Transitions a dashboard to `PUBLISHED` and increments its `version`.
    fn publish_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
    ) -> impl Future<Output = Result<DashboardRecord, AppError>> + Send;

    /// Duplicates a dashboard and all of its widgets into a new `DRAFT` dashboard.
    fn duplicate_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        new_name: Option<String>,
        new_slug: Option<String>,
        created_by: Option<Uuid>,
    ) -> impl Future<Output = Result<DashboardDetail, AppError>> + Send;

    /// Fetches a single widget by UUID in `tenant_id`.
    fn get_widget(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
    ) -> impl Future<Output = Result<Option<WidgetRecord>, AppError>> + Send;

    /// Creates a new widget inside an existing dashboard with precomputed `deps`.
    fn create_widget(
        &self,
        tenant_id: &str,
        params: CreateWidgetRecordParams,
    ) -> impl Future<Output = Result<WidgetRecord, AppError>> + Send;

    /// Updates a widget's configuration and optional recomputed `deps`.
    fn update_widget(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
        params: UpdateWidgetRecordParams,
    ) -> impl Future<Output = Result<WidgetRecord, AppError>> + Send;

    /// Deletes a widget by UUID in `tenant_id`.
    fn delete_widget(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send;

    /// Batch-updates widget `{x, y, w, h}` positions for a dashboard in one transaction.
    fn update_dashboard_layout(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        items: &[LayoutUpdateItem],
    ) -> impl Future<Output = Result<Vec<WidgetRecord>, AppError>> + Send;

    /// Atomically creates an AI-generated `DRAFT` dashboard along with all drafted widgets.
    fn create_ai_dashboard(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        dashboard: CreateDashboardRequest,
        widgets: Vec<CreateWidgetRecordParams>,
    ) -> impl Future<Output = Result<DashboardDetail, AppError>> + Send;

    /// Finds widgets in `tenant_id` whose GIN-indexed `deps` match any dependency in `filter_deps`.
    fn find_widgets_by_deps(
        &self,
        tenant_id: &str,
        filter_deps: &WidgetDeps,
    ) -> impl Future<Output = Result<Vec<AffectedWidgetRef>, AppError>> + Send;

    /// Lists uploaded visual assets in `tenant_id`, optionally filtered by `AssetKind`.
    fn list_assets(
        &self,
        tenant_id: &str,
        asset_kind: Option<crate::enums::AssetKind>,
        query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<AssetRecord>, AppError>> + Send;

    /// Persists uploaded visual asset metadata in `xv_assets`.
    fn create_asset(
        &self,
        tenant_id: &str,
        params: CreateAssetParams,
    ) -> impl Future<Output = Result<AssetRecord, AppError>> + Send;

    /// Fetches a visual asset record by UUID in `tenant_id`.
    fn get_asset(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> impl Future<Output = Result<Option<AssetRecord>, AppError>> + Send;

    /// Deletes a visual asset record and returns the removed record for disk cleanup.
    fn delete_asset(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> impl Future<Output = Result<Option<AssetRecord>, AppError>> + Send;

    /// Lists 2D floor-plan node placements for `asset_id`.
    fn list_floor_placements(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<FloorPlacementRecord>, AppError>> + Send;

    /// Atomically replaces all 2D floor-plan placements for `asset_id`.
    fn replace_floor_placements(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        items: &[PutFloorPlacementItem],
    ) -> impl Future<Output = Result<Vec<FloorPlacementRecord>, AppError>> + Send;

    /// Lists 3D twin mesh/coordinate bindings for `asset_id`.
    fn list_scene3d_bindings(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<Scene3dBindingRecord>, AppError>> + Send;

    /// Atomically replaces all 3D twin bindings for `asset_id`.
    fn replace_scene3d_bindings(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        items: &[PutScene3dBindingItem],
    ) -> impl Future<Output = Result<Vec<Scene3dBindingRecord>, AppError>> + Send;

    /// Lists widgets in `tenant_id` whose `FLOOR_PLAN` or `SCENE_3D` `data_source` references `asset_id`.
    fn list_widgets_by_asset(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<WidgetRecord>, AppError>> + Send;

    /// Queries geo-located nodes with optional `color_metric` and active open alert counts.
    fn query_nodes_geo(
        &self,
        tenant_id: &str,
        type_names: &[String],
        color_metric: Option<&str>,
    ) -> impl Future<Output = Result<Vec<NodesGeoRow>, AppError>> + Send;

    /// Queries geo-located relationships connecting `node_ids` filtered by `link_types` for fly-line rendering.
    fn query_nodes_geo_links(
        &self,
        tenant_id: &str,
        node_ids: &[Uuid],
        link_types: &[String],
    ) -> impl Future<Output = Result<Vec<NodesGeoLinkRow>, AppError>> + Send;

    /// Queries floor-plan placements joined with node titles, live metric, and alert counts.
    fn query_floor_plan_rows(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        color_metric: Option<&str>,
    ) -> impl Future<Output = Result<Vec<FloorPlanRow>, AppError>> + Send;

    /// Queries 3D scene bindings joined with node titles, live metric, and active alert counts.
    fn query_scene3d_rows(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        color_metric: Option<&str>,
    ) -> impl Future<Output = Result<Vec<Scene3dRow>, AppError>> + Send;

    /// Lists the latest `ax_action_log` rows generated by action widget `widget_id`.
    fn list_widget_action_logs(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<ActionLogRecord>, AppError>> + Send;

    /// Atomically writes a property or metric onto `xb_nodes`, inserts `xb_metric_series` (`ACTION`),
    /// logs `ax_action_log(source_kind = 'VIEW')`, and emits `node.upserted` / `node.metrics_changed`.
    fn execute_view_write_node(
        &self,
        tenant_id: &str,
        params: ViewWriteNodeParams,
    ) -> impl Future<Output = Result<ViewWriteNodeOutcome, AppError>> + Send;

    /// Lists uploaded `DOCX`/`PPTX` export templates in `tenant_id`.
    fn list_export_templates(
        &self,
        _tenant_id: &str,
        _query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<ExportTemplateRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "list_export_templates not implemented",
            ))
        }
    }

    /// Inserts a new `DOCX`/`PPTX` export template record (`xv_export_templates`).
    fn create_export_template(
        &self,
        _tenant_id: &str,
        _params: CreateExportTemplateParams,
    ) -> impl Future<Output = Result<ExportTemplateRecord, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "create_export_template not implemented",
            ))
        }
    }

    /// Fetches a single export template by UUID in `tenant_id`.
    fn get_export_template(
        &self,
        _tenant_id: &str,
        _template_id: Uuid,
    ) -> impl Future<Output = Result<Option<ExportTemplateRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "get_export_template not implemented",
            ))
        }
    }

    /// Deletes an export template and returns the removed record for disk cleanup.
    fn delete_export_template(
        &self,
        _tenant_id: &str,
        _template_id: Uuid,
    ) -> impl Future<Output = Result<Option<ExportTemplateRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "delete_export_template not implemented",
            ))
        }
    }

    /// Atomically inserts `xv_export_jobs(PENDING)` and enqueues `ax_jobs(kind = EXPORT, idempotency_key = export_id)`.
    fn create_export_job(
        &self,
        _tenant_id: &str,
        _params: CreateExportJobParams,
    ) -> impl Future<Output = Result<ExportJobRecord, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "create_export_job not implemented",
            ))
        }
    }

    /// Fetches a single export job by UUID in `tenant_id`.
    fn get_export_job(
        &self,
        _tenant_id: &str,
        _export_id: Uuid,
    ) -> impl Future<Output = Result<Option<ExportJobRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "get_export_job not implemented",
            ))
        }
    }

    /// Lists export jobs in `tenant_id` (optionally restricted to `created_by_filter`).
    fn list_export_jobs(
        &self,
        _tenant_id: &str,
        _created_by_filter: Option<Uuid>,
        _query: ExportListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ExportJobRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "list_export_jobs not implemented",
            ))
        }
    }

    /// Marks `xv_export_jobs` as `RUNNING` when picked up by `ExportJobHandler`.
    fn mark_export_job_running(
        &self,
        _tenant_id: &str,
        _export_id: Uuid,
    ) -> impl Future<Output = Result<Option<ExportJobRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "mark_export_job_running not implemented",
            ))
        }
    }

    /// Marks `xv_export_jobs` as `SUCCEEDED`, writes artifact metadata & `expires_at`, and emits `export.finished`.
    fn complete_export_job(
        &self,
        _tenant_id: &str,
        _export_id: Uuid,
        _params: CompleteExportJobParams,
    ) -> impl Future<Output = Result<ExportJobRecord, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "complete_export_job not implemented",
            ))
        }
    }

    /// Marks `xv_export_jobs` as `FAILED` with `error_message`.
    fn fail_export_job(
        &self,
        _tenant_id: &str,
        _export_id: Uuid,
        _error_message: &str,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "fail_export_job not implemented",
            ))
        }
    }

    /// Deletes an export job record and returns the removed record for artifact cleanup.
    fn delete_export_job(
        &self,
        _tenant_id: &str,
        _export_id: Uuid,
    ) -> impl Future<Output = Result<Option<ExportJobRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "delete_export_job not implemented",
            ))
        }
    }

    /// Reads the configured `EXPORT_FILE` retention `keep_days` for `tenant_id` (defaulting to `30`).
    fn get_export_keep_days(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<i32, AppError>> + Send {
        async { Ok(30) }
    }

    /// Inserts a new `xv_share_links` row.
    fn create_share_link(
        &self,
        _tenant_id: &str,
        _params: CreateShareLinkParams,
    ) -> impl Future<Output = Result<ShareLinkRecord, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "create_share_link not implemented",
            ))
        }
    }

    /// Lists share links in `tenant_id` (optionally filtered by `target_id`).
    fn list_share_links(
        &self,
        _tenant_id: &str,
        _query: ShareListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ShareLinkRecord>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "list_share_links not implemented",
            ))
        }
    }

    /// Revokes a share link immediately (`revoked_at = NOW()`).
    fn revoke_share_link(
        &self,
        _tenant_id: &str,
        _share_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "revoke_share_link not implemented",
            ))
        }
    }

    /// Resolves an active, non-revoked, non-expired share link via `ax_resolve_share(token_hash)`.
    fn resolve_share_by_hash(
        &self,
        _token_hash: &str,
    ) -> impl Future<Output = Result<Option<ResolvedShare>, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "resolve_share_by_hash not implemented",
            ))
        }
    }

    /// Atomically increments `view_count` inside a tenant transaction if below `max_views`.
    /// Returns `true` if incremented, or `false` if `max_views` was already reached/revoked.
    fn increment_share_view_count(
        &self,
        _tenant_id: &str,
        _share_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send {
        async {
            Err(AppError::unprocessable(
                "UNSUPPORTED",
                "increment_share_view_count not implemented",
            ))
        }
    }
}
