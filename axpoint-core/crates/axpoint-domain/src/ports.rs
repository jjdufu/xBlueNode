//! Pure port traits decoupling domain consumers from concrete storage and AI adapters.

use std::collections::BTreeMap;
use std::future::Future;

use uuid::Uuid;

use crate::axhub::{
    CatalogSearchHit, ColumnMeta, CommitStagingRequest, Connection, ConnectionStatus,
    ConnectionTestResult, ConnectorSpecDto, CreateDatasetRequest, CreateGlossaryTermRequest,
    CreateQualityRuleRequest, CreateWebhookRequest, Dataset, DatasetListQuery, DiscoveredObject,
    GlossaryTerm, LakeMaintenanceRequest, LakeMaintenanceResult, LakePreviewRequest,
    LakeSnapshotInfo, LineageEdge, NewConnectionInput, NewUploadInput, PaginatedList,
    PreviewResponse, QualityRule, ResolvedWebhook, ScheduledDataset, SealedCredential,
    SidecarConnectorRequest, SidecarPreviewRequest, SidecarSyncRequest, SidecarSyncResult,
    SyncDatasetResponse, SyncJob, UnifiedParseResponse, UpdateColumnMetaRequest,
    UpdateConnectionInput, UpdateDatasetRequest, UploadRecord, WebhookBufferItem, WebhookConfig,
};
use crate::enums::{GraphAlgo, JobKind};
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
