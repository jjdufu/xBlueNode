//! `AxHub` data fabric domain types, request/response DTOs, and Sidecar wire contracts.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{
    ConnectorType, DataType, JobStatus, PiiCategory, QualityRuleKind, SourceKind, SyncMode,
};

/// Schema evolution policy for `AxHub` dataset ingestion (`03-STEP02` §02.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SchemaPolicy {
    /// Reject any column addition, removal, widening, or narrowing.
    Strict,
    /// Allow additive columns and lossless widening; reject removals or narrowing.
    #[default]
    Additive,
    /// Allow additions, retain dropped columns with nulls, and widen narrowed columns to `STRING`.
    Evolve,
}

impl SchemaPolicy {
    /// Canonical string representation stored in PostgreSQL and sent to Sidecar.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "STRICT",
            Self::Additive => "ADDITIVE",
            Self::Evolve => "EVOLVE",
        }
    }
}

/// Connectivity test status of an `AxHub` connection (`ds_connections.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionStatus {
    /// Never tested.
    #[default]
    Untested,
    /// Last connectivity probe succeeded.
    Ok,
    /// Last connectivity probe failed.
    Failed,
}

impl ConnectionStatus {
    /// Canonical string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Untested => "UNTESTED",
            Self::Ok => "OK",
            Self::Failed => "FAILED",
        }
    }
}

/// Lifecycle state of an `AxHub` dataset (`ds_datasets.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DatasetStatus {
    /// Newly created and not yet synchronized.
    #[default]
    Draft,
    /// At least one snapshot committed and ready for query.
    Ready,
    /// Synchronization job currently running.
    Syncing,
    /// Latest synchronization failed with a terminal error.
    Failed,
    /// Disabled from scheduled synchronization.
    Disabled,
}

impl DatasetStatus {
    /// Canonical string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Ready => "READY",
            Self::Syncing => "SYNCING",
            Self::Failed => "FAILED",
            Self::Disabled => "DISABLED",
        }
    }
}

/// Lifecycle status of a unified file upload (`ds_uploads.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UploadStatus {
    /// Upload queued for parsing.
    #[default]
    Pending,
    /// Dual-track parse succeeded; awaiting commit.
    Parsed,
    /// Committed to knowledge hypergraph and/or `DuckLake`.
    Committed,
    /// Terminal parse or commit error.
    Failed,
}

impl UploadStatus {
    /// Canonical string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Parsed => "PARSED",
            Self::Committed => "COMMITTED",
            Self::Failed => "FAILED",
        }
    }
}

/// Inbound `AxHub` webhook authentication scheme (`ds_webhooks.auth_scheme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WebhookAuthScheme {
    /// `X-AxPoint-Signature: sha256=<hex>` over `<timestamp>.<body>`.
    #[default]
    HmacSha256,
    /// `Authorization: Bearer <secret>`.
    Bearer,
}

impl WebhookAuthScheme {
    /// Canonical string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HmacSha256 => "HMAC_SHA256",
            Self::Bearer => "BEARER",
        }
    }
}

/// Failure handling action on a `QualityRule` (`ds_quality_rules.on_fail`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QualityOnFail {
    /// Record warning in `quality_result` without aborting lake commit.
    #[default]
    Warn,
    /// Roll back lake transaction and fail with `QUALITY_BLOCKED`.
    Block,
}

impl QualityOnFail {
    /// Canonical string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "WARN",
            Self::Block => "BLOCK",
        }
    }
}

/// Column profile inferred by `AxParser` / `AxHub` (`02-STEP01` §01.2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnProfile {
    /// Normalized column name.
    pub name: String,
    /// 0-based or 1-based ordinal position.
    pub ordinal: i32,
    /// Inferred logical data type.
    pub data_type: DataType,
    /// Whether null values were observed or permitted.
    pub nullable: bool,
    /// Count of null values.
    #[serde(default)]
    pub null_count: i64,
    /// Fraction of null values in `[0.0, 1.0]`.
    #[serde(default)]
    pub null_ratio: f64,
    /// Approximate distinct count.
    #[serde(default)]
    pub distinct_estimate: i64,
    /// Masked sample values (at most 10).
    #[serde(default)]
    pub sample_values: Vec<String>,
    /// Detected PII category.
    #[serde(default = "default_pii_none")]
    pub pii_category: PiiCategory,
    /// Hit ratio among sampled rows.
    #[serde(default)]
    pub pii_hit_ratio: f64,
    /// Source column comment or pre-rename original header.
    #[serde(default)]
    pub source_comment: String,
}

const fn default_pii_none() -> PiiCategory {
    PiiCategory::None
}

/// Extracted structured table metadata from `AxParser` (`02-STEP01` §01.2.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedTable {
    /// Normalized table identifier (`^[a-z][a-z0-9_]{0,62}$`).
    pub table_name: String,
    /// Provenance locator (e.g. `sheet=订单;block=1`).
    pub origin: String,
    /// Extracted data row count.
    pub row_count: i64,
    /// Relative path under `AXPOINT_DATA_ROOT`.
    pub staging_path: String,
    /// Column profiles.
    pub columns: Vec<ColumnProfile>,
    /// Header row count.
    #[serde(default = "default_header_rows")]
    pub header_rows: i32,
}

const fn default_header_rows() -> i32 {
    1
}

/// Non-fatal parse warning emitted by `AxParser` (`02-STEP01` §01.2.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseWarning {
    /// Warning code (e.g. `ENCODING_FALLBACK`, `ROW_MALFORMED`).
    pub code: String,
    /// Human-readable detail.
    pub detail: String,
}

/// Dual-track response returned by Sidecar `POST /parse/unified`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedParseResponse {
    /// Unique parse session UUID string.
    pub parse_id: String,
    /// Extracted document title.
    pub title: String,
    /// Detected format slug.
    pub source_format: String,
    /// Engine combination used.
    pub engine: String,
    /// Hex SHA-256 of raw uploaded bytes.
    pub sha256: String,
    /// Structured Markdown text track.
    pub markdown: String,
    /// Extracted Parquet tables.
    pub tables: Vec<ExtractedTable>,
    /// Non-fatal warnings.
    #[serde(default)]
    pub warnings: Vec<ParseWarning>,
}

/// Connector specification card returned by Sidecar `GET /connectors/types`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorSpecDto {
    /// Canonical connector type.
    pub connector_type: ConnectorType,
    /// Human-readable display name.
    pub display_name: String,
    /// Connector category (`FILE`, `RELATIONAL`, `DOMESTIC`, `ANALYTIC`, `NOSQL`, `TIMESERIES`, `STREAM`, `API`).
    pub category: String,
    /// JSON Schema for rendering connection forms.
    pub config_schema: serde_json::Value,
    /// Secret field keys stripped from `config` and stored in `ds_credentials`.
    pub secret_fields: Vec<String>,
    /// Supported synchronization modes.
    pub supports_modes: Vec<SyncMode>,
    /// Whether `discover` is supported.
    pub supports_discover: bool,
    /// Image tier (`lite` or `full`).
    pub image_tier: String,
    /// Whether the driver is installed in the running Sidecar image.
    #[serde(default = "default_true")]
    pub available: bool,
}

const fn default_true() -> bool {
    true
}

/// Encrypted credential envelope persisted in `ds_credentials`.
#[derive(Debug, Clone)]
pub struct SealedCredential {
    /// Credential UUID (`ds_credentials.id`).
    pub id: Uuid,
    /// Key version used to seal the secret JSON.
    pub key_version: i32,
    /// 12-byte AES-256-GCM nonce.
    pub nonce: Vec<u8>,
    /// AES-256-GCM ciphertext + tag.
    pub ciphertext: Vec<u8>,
}

/// Repository input for inserting a new `ds_connections` row.
#[derive(Debug, Clone)]
pub struct NewConnectionInput {
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Unique connection name.
    pub name: String,
    /// Connector type.
    pub connector_type: ConnectorType,
    /// Plain non-secret config JSON.
    pub config: serde_json::Value,
    /// Private network flag.
    pub allow_private_network: bool,
    /// Optional sealed credential.
    pub credential: Option<SealedCredential>,
}

/// Repository input for patching an existing `ds_connections` row.
#[derive(Debug, Clone)]
pub struct UpdateConnectionInput {
    /// Optional updated name.
    pub name: Option<String>,
    /// Optional updated plain config JSON.
    pub config: Option<serde_json::Value>,
    /// Optional updated private network flag.
    pub allow_private_network: Option<bool>,
    /// Credential mutation (`None` = unchanged, `Some(None)` = clear, `Some(Some(c))` = replace).
    pub credential: Option<Option<SealedCredential>>,
}

/// Repository input for creating a `PENDING` `ds_uploads` row.
#[derive(Debug, Clone)]
pub struct NewUploadInput {
    /// Upload UUID.
    pub id: Uuid,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Original filename.
    pub filename: String,
    /// Hex SHA-256 checksum.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: i64,
}

/// Public-safe `AxHub` connection representation (secrets never echoed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// Connection UUID.
    pub id: Uuid,
    /// Unique connection name within tenant.
    pub name: String,
    /// Connector type.
    pub connector_type: ConnectorType,
    /// Non-secret configuration fields.
    pub config: serde_json::Map<String, serde_json::Value>,
    /// Names of secret fields that are currently populated (`["password"]`, etc.).
    pub secret_fields_set: Vec<String>,
    /// Whether outbound private/RFC1918 addresses are permitted.
    pub allow_private_network: bool,
    /// Latest test status (`UNTESTED`, `OK`, `FAILED`).
    pub status: ConnectionStatus,
    /// Diagnostic message from latest test.
    pub last_test_message: String,
    /// Timestamp of latest test.
    pub last_tested_at: Option<DateTime<Utc>>,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Client request to create a new `AxHub` connection (`POST /api/axhub/connections`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConnectionRequest {
    /// Unique connection name within tenant.
    pub name: String,
    /// Connector type.
    pub connector_type: ConnectorType,
    /// Mixed plain and secret configuration fields (split by Core using `secret_fields`).
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
    /// Optional explicit secrets map (merged with any secret keys in `config`).
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// Allow connecting to private networks (`DATA_ADMIN` required).
    #[serde(default)]
    pub allow_private_network: bool,
}

/// Client request to patch an existing `AxHub` connection (`PATCH /api/axhub/connections/{id}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateConnectionRequest {
    /// Updated display/unique name.
    pub name: Option<String>,
    /// Updated configuration fields (`null` on a secret field clears it; omitted keeps existing).
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    /// Explicit secret field updates (`None` value clears that secret key).
    pub secrets: Option<BTreeMap<String, Option<String>>>,
    /// Updated private network flag.
    pub allow_private_network: Option<bool>,
}

/// Connectivity test result (`POST /api/axhub/connections/{id}/test`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionTestResult {
    /// Whether connection succeeded.
    pub ok: bool,
    /// Diagnostic status message.
    pub message: String,
    /// Round-trip latency in milliseconds.
    pub latency_ms: u64,
}

/// Discovered column metadata on a remote object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredColumn {
    /// Column name.
    pub name: String,
    /// Native source type string.
    pub native_type: String,
    /// Mapped canonical `DataType`.
    pub data_type: DataType,
    /// Whether column allows nulls.
    pub nullable: bool,
}

/// Discovered table, collection, topic, or file group (`GET /api/axhub/connections/{id}/discover`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredObject {
    /// Opaque locator written to `ds_datasets.source_ref`.
    #[serde(rename = "ref", default)]
    pub source_ref: serde_json::Value,
    /// Human-readable display name.
    pub display_name: String,
    /// Dataset source category.
    pub source_kind: SourceKind,
    /// Discovered columns.
    pub columns: Vec<DiscoveredColumn>,
    /// Primary key column names.
    #[serde(default)]
    pub primary_key: Vec<String>,
    /// Suggested incremental cursor column.
    pub suggested_cursor: Option<String>,
    /// Estimated row count if available.
    pub estimated_rows: Option<i64>,
}

/// Client request to preview an object on a connection (`POST /api/axhub/connections/{id}/preview`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionPreviewRequest {
    /// Source object locator.
    #[serde(rename = "ref")]
    pub source_ref: serde_json::Value,
    /// Maximum rows (`<= 1000`).
    #[serde(default = "default_preview_limit")]
    pub limit: u32,
}

const fn default_preview_limit() -> u32 {
    100
}

/// Tabular preview payload (`columns` + `rows`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewResponse {
    /// Column profiles.
    pub columns: Vec<ColumnProfile>,
    /// Row objects.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// Registered `AxHub` dataset record (`ds_datasets`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    /// Dataset UUID.
    pub id: Uuid,
    /// Owning connection UUID (optional for direct uploads / webhooks).
    pub connection_id: Option<Uuid>,
    /// Unique machine name (`^[a-z][a-z0-9_]{0,62}$`).
    pub name: String,
    /// Display title.
    pub display_name: String,
    /// Description.
    pub description: String,
    /// Source kind.
    pub source_kind: SourceKind,
    /// Source locator JSON.
    pub source_ref: serde_json::Value,
    /// Synchronization mode (`SNAPSHOT`, `APPEND`, `UPSERT`).
    pub sync_mode: SyncMode,
    /// Primary key columns (non-empty for `UPSERT`).
    pub primary_key: Vec<String>,
    /// Incremental cursor column.
    pub cursor_column: Option<String>,
    /// Last committed cursor value.
    pub cursor_value: Option<String>,
    /// Soft-delete boolean column for `UPSERT`.
    pub delete_column: Option<String>,
    /// Schema evolution policy.
    pub schema_policy: SchemaPolicy,
    /// Lake schema (`tenant_<id>`).
    pub lake_schema: String,
    /// Lake table name.
    pub lake_table: String,
    /// Current column profiles.
    pub schema_json: Vec<ColumnProfile>,
    /// Monotonic schema version.
    pub schema_version: i32,
    /// Latest committed row count.
    pub row_count: i64,
    /// Latest `DuckLake` snapshot ID.
    pub last_snapshot_id: Option<i64>,
    /// Timestamp of latest successful sync.
    pub last_synced_at: Option<DateTime<Utc>>,
    /// Optional 5-field cron schedule.
    pub schedule_cron: Option<String>,
    /// Classification tags.
    pub tags: Vec<String>,
    /// Owner user UUID.
    pub owner_user_id: Option<Uuid>,
    /// Dataset status.
    pub status: DatasetStatus,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Query parameters for `GET /api/axhub/datasets`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetListQuery {
    /// Full-text / substring filter.
    pub q: Option<String>,
    /// Tag filter.
    pub tag: Option<String>,
    /// Source kind filter.
    pub source_kind: Option<SourceKind>,
    /// Page size (`1..=500`).
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    pub cursor: Option<String>,
}

/// Paginated list envelope (`{items, next_cursor}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedList<T> {
    /// Page items.
    pub items: Vec<T>,
    /// Opaque cursor for the next page, if any.
    pub next_cursor: Option<String>,
}

/// Client request to create a dataset (`POST /api/axhub/datasets`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDatasetRequest {
    /// Source connection UUID.
    pub connection_id: Option<Uuid>,
    /// Machine name (`^[a-z][a-z0-9_]{0,62}$`).
    pub name: String,
    /// Human-readable display name.
    #[serde(default)]
    pub display_name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Source kind.
    pub source_kind: SourceKind,
    /// Source object reference.
    #[serde(default)]
    pub source_ref: serde_json::Value,
    /// Synchronization mode.
    #[serde(default = "default_sync_snapshot")]
    pub sync_mode: SyncMode,
    /// Primary key columns.
    #[serde(default)]
    pub primary_key: Vec<String>,
    /// Incremental cursor column.
    pub cursor_column: Option<String>,
    /// Soft-delete flag column.
    pub delete_column: Option<String>,
    /// Schema evolution policy.
    #[serde(default)]
    pub schema_policy: SchemaPolicy,
    /// Optional 5-field cron schedule.
    pub schedule_cron: Option<String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

const fn default_sync_snapshot() -> SyncMode {
    SyncMode::Snapshot
}

/// Client request to update a dataset (`PATCH /api/axhub/datasets/{id}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDatasetRequest {
    /// Updated display name.
    pub display_name: Option<String>,
    /// Updated description.
    pub description: Option<String>,
    /// Updated sync mode.
    pub sync_mode: Option<SyncMode>,
    /// Updated primary key columns.
    pub primary_key: Option<Vec<String>>,
    /// Updated cursor column.
    pub cursor_column: Option<Option<String>>,
    /// Updated delete column.
    pub delete_column: Option<Option<String>>,
    /// Updated schema policy.
    pub schema_policy: Option<SchemaPolicy>,
    /// Updated cron schedule (`Some(None)` clears schedule).
    pub schedule_cron: Option<Option<String>>,
    /// Updated tags.
    pub tags: Option<Vec<String>>,
    /// Updated status (`DISABLED` / `READY`).
    pub status: Option<DatasetStatus>,
}

/// Column catalog metadata (`ds_column_meta`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Column name.
    pub column_name: String,
    /// Logical data type.
    pub data_type: DataType,
    /// Sensitive data classification.
    pub pii_category: PiiCategory,
    /// Whether manually confirmed (prevents auto-detection overwrite).
    pub pii_confirmed: bool,
    /// Column description.
    pub description: String,
    /// Linked business glossary term.
    pub business_term: String,
    /// Measurement unit.
    pub unit: String,
    /// Column ordinal.
    pub ordinal: i32,
}

/// Request to update column metadata (`PUT /api/axhub/datasets/{id}/columns/{column}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateColumnMetaRequest {
    /// Updated PII classification.
    pub pii_category: Option<PiiCategory>,
    /// Mark PII classification as human-confirmed.
    pub pii_confirmed: Option<bool>,
    /// Updated description.
    pub description: Option<String>,
    /// Updated business term.
    pub business_term: Option<String>,
    /// Updated unit.
    pub unit: Option<String>,
}

/// Data quality assertion rule (`ds_quality_rules`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRule {
    /// Rule UUID.
    pub id: Uuid,
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Assertion kind.
    pub rule_kind: QualityRuleKind,
    /// Target column name (`None` for table-level rules like `ROW_COUNT_MIN`).
    pub column_name: Option<String>,
    /// Rule parameters (`{min, max}`, `{pattern}`, `{values}`, `{max_lag_minutes}`).
    pub params: serde_json::Value,
    /// Action on failure (`WARN` or `BLOCK`).
    pub on_fail: QualityOnFail,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Client request to create a quality rule (`POST /api/axhub/datasets/{id}/quality-rules`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateQualityRuleRequest {
    /// Assertion kind.
    pub rule_kind: QualityRuleKind,
    /// Optional column name.
    pub column_name: Option<String>,
    /// Parameters object.
    #[serde(default = "empty_json_obj")]
    pub params: serde_json::Value,
    /// Failure handling policy.
    #[serde(default)]
    pub on_fail: QualityOnFail,
}

fn empty_json_obj() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Individual rule evaluation outcome in a `QualityReport`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRuleOutcome {
    /// Rule UUID or synthetic ID.
    #[serde(default)]
    pub rule_id: String,
    /// Rule kind.
    pub rule_kind: String,
    /// Target column name, if any.
    pub column_name: Option<String>,
    /// Whether the rule passed.
    pub passed: bool,
    /// Policy (`WARN` or `BLOCK`).
    pub on_fail: QualityOnFail,
    /// Violating row count or diagnostic detail.
    #[serde(default)]
    pub detail: String,
}

/// Aggregate quality report returned by Sidecar after batch validation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityReport {
    /// Overall pass flag (no rules failed).
    #[serde(default = "default_true")]
    pub passed: bool,
    /// Whether any `BLOCK` rule failed.
    #[serde(default)]
    pub blocked: bool,
    /// Per-rule outcomes.
    #[serde(default)]
    pub rules: Vec<QualityRuleOutcome>,
}

/// Detected schema diff during synchronization (`03-STEP02` §02.9.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaChange {
    /// Newly added columns.
    #[serde(default)]
    pub added: Vec<ColumnProfile>,
    /// Removed column names.
    #[serde(default)]
    pub removed: Vec<String>,
    /// Losslessly widened columns `(column, old_type, new_type)`.
    #[serde(default)]
    pub widened: Vec<(String, String, String)>,
    /// Narrowed columns `(column, old_type, new_type)`.
    #[serde(default)]
    pub narrowed: Vec<(String, String, String)>,
    /// Whether the change violates the dataset's `SchemaPolicy`.
    #[serde(default)]
    pub breaking: bool,
}

impl SchemaChange {
    /// Returns `true` when any schema addition, removal, widening, or narrowing occurred.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        !self.added.is_empty()
            || !self.removed.is_empty()
            || !self.widened.is_empty()
            || !self.narrowed.is_empty()
    }
}

/// Dataset synchronization execution history row (`ds_sync_jobs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJob {
    /// Sync job row UUID.
    pub id: Uuid,
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Underlying `ax_jobs.id`.
    pub job_id: Uuid,
    /// Lake `_ax_batch_id` (reused across retries).
    pub batch_id: Uuid,
    /// Sync mode used.
    pub sync_mode: SyncMode,
    /// Execution status.
    pub status: JobStatus,
    /// Starting cursor.
    pub cursor_from: Option<String>,
    /// Ending cursor committed.
    pub cursor_to: Option<String>,
    /// Source rows read.
    pub rows_read: i64,
    /// Lake rows written.
    pub rows_written: i64,
    /// Lake rows deleted.
    pub rows_deleted: i64,
    /// Committed `DuckLake` snapshot ID.
    pub snapshot_id: Option<i64>,
    /// Detected schema diff.
    pub schema_change: SchemaChange,
    /// Quality assertion report.
    pub quality_result: QualityReport,
    /// Machine-readable error code if failed.
    pub error_code: Option<String>,
    /// Diagnostic error message if failed.
    pub error_message: Option<String>,
    /// Execution start time.
    pub started_at: Option<DateTime<Utc>>,
    /// Execution finish time.
    pub finished_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Response to `POST /api/axhub/datasets/{id}/sync`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDatasetResponse {
    /// Background `ax_jobs.id`.
    pub job_id: Uuid,
    /// `true` when deduplicated against an already `PENDING`/`RUNNING` sync job (`HTTP 200`).
    #[serde(default)]
    pub existing: bool,
}

/// `DuckLake` snapshot metadata (`GET /api/axhub/datasets/{id}/snapshots`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeSnapshotInfo {
    /// Snapshot version number.
    pub snapshot_id: i64,
    /// Commit timestamp (RFC 3339).
    pub committed_at: String,
    /// Rows added in snapshot.
    #[serde(default)]
    pub rows_added: i64,
    /// Rows deleted in snapshot.
    #[serde(default)]
    pub rows_deleted: i64,
}

/// Unified file upload record (`ds_uploads`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRecord {
    /// Upload UUID.
    pub id: Uuid,
    /// Original filename.
    pub filename: String,
    /// Hex SHA-256 checksum.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: i64,
    /// Sidecar `parse_id`, once parsed.
    pub parse_id: Option<Uuid>,
    /// Knowledge hypergraph `hg_docs.id` when `to_knowledge=true`.
    pub doc_id: Option<Uuid>,
    /// Created/updated dataset UUIDs when `to_datasets=true`.
    pub dataset_ids: Vec<Uuid>,
    /// Upload lifecycle status.
    pub status: UploadStatus,
    /// Terminal error code if failed.
    pub error_code: Option<String>,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Enqueued upload response (`POST /api/axhub/uploads`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueUploadResponse {
    /// Upload record UUID.
    pub upload_id: Uuid,
    /// Background `PARSE_FILE` job UUID (or `None` when deduplicated).
    pub job_id: Option<Uuid>,
    /// `true` if identical SHA-256 already committed in this tenant.
    #[serde(default)]
    pub deduplicated: bool,
}

/// Inbound webhook receiver metadata (`ds_webhooks`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Webhook UUID.
    pub id: Uuid,
    /// Backing `WEBHOOK_STREAM` dataset UUID.
    pub dataset_id: Uuid,
    /// URL slug (`^[a-z0-9-]{3,64}$`).
    pub slug: String,
    /// Signature/token verification scheme.
    pub auth_scheme: WebhookAuthScheme,
    /// `JSONPath` expression extracting record array from request body.
    pub record_path: String,
    /// Maximum body size in bytes.
    pub max_body_bytes: i32,
    /// Whether ingestion is enabled.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Client request to create an inbound webhook (`POST /api/axhub/webhooks`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookRequest {
    /// Target dataset machine name (`^[a-z][a-z0-9_]{0,62}$`).
    pub dataset_name: String,
    /// Endpoint slug (`^[a-z0-9-]{3,64}$`).
    pub slug: String,
    /// Authentication scheme (`HMAC_SHA256` or `BEARER`).
    #[serde(default)]
    pub auth_scheme: WebhookAuthScheme,
    /// `JSONPath` expression for records (defaults to `"$"`).
    #[serde(default = "default_root_jsonpath")]
    pub record_path: String,
    /// Maximum body size in bytes (defaults to 1 MiB).
    #[serde(default = "default_webhook_max_bytes")]
    pub max_body_bytes: i32,
}

fn default_root_jsonpath() -> String {
    String::from("$")
}

const fn default_webhook_max_bytes() -> i32 {
    1_048_576
}

/// One-time secret response from `POST /api/axhub/webhooks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebhookResponse {
    /// Created webhook metadata.
    pub webhook: WebhookConfig,
    /// Backing dataset ID.
    pub dataset_id: Uuid,
    /// Relative or full ingest URL (`/api/axhub/webhooks/{tenant_id}/{slug}/ingest`).
    pub url: String,
    /// Generated 32-byte hex secret (returned ONLY once at creation).
    pub secret: String,
}

/// Pre-auth webhook lookup row returned by `ax_resolve_webhook(tenant, slug)`.
#[derive(Debug, Clone)]
pub struct ResolvedWebhook {
    /// Webhook UUID.
    pub webhook_id: Uuid,
    /// Target dataset UUID.
    pub dataset_id: Uuid,
    /// Auth scheme.
    pub auth_scheme: WebhookAuthScheme,
    /// Credential UUID holding the secret.
    pub credential_id: Uuid,
    /// Record `JSONPath`.
    pub record_path: String,
    /// Max request body size.
    pub max_body_bytes: i32,
}

/// Buffered webhook event awaiting lake flush (`ds_webhook_buffer`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookBufferItem {
    /// Buffer primary key.
    pub id: i64,
    /// Idempotent delivery identifier.
    pub delivery_id: String,
    /// Raw JSON payload.
    pub payload: serde_json::Value,
    /// Receive timestamp.
    pub received_at: DateTime<Utc>,
}

/// Business glossary term (`ds_glossary_terms`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryTerm {
    /// Term UUID.
    pub id: Uuid,
    /// Canonical business term.
    pub term: String,
    /// Definition text.
    pub definition: String,
    /// Synonym strings used during catalog search expansion.
    pub synonyms: Vec<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request to create a glossary term (`POST /api/axhub/glossary`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGlossaryTermRequest {
    /// Canonical term.
    pub term: String,
    /// Definition.
    #[serde(default)]
    pub definition: String,
    /// Synonyms.
    #[serde(default)]
    pub synonyms: Vec<String>,
}

/// Data catalog search hit (`GET /api/axhub/catalog/search?q=`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSearchHit {
    /// Dataset UUID.
    pub id: Uuid,
    /// Dataset machine name.
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: String,
    /// Source kind.
    pub source_kind: SourceKind,
    /// Row count.
    pub row_count: i64,
    /// Tags.
    pub tags: Vec<String>,
    /// Columns whose name, description, or business term matched.
    pub matched_columns: Vec<String>,
}

/// Data lineage directed edge (`ds_lineage`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEdge {
    /// Lineage row ID.
    pub id: i64,
    /// Upstream entity kind (`CONNECTION`, `UPLOAD`, `DATASET`, ...).
    pub src_kind: String,
    /// Upstream entity ID.
    pub src_id: String,
    /// Downstream entity kind (`DATASET`, `METRIC`, `NODE_TYPE`, `DOC`, ...).
    pub dst_kind: String,
    /// Downstream entity ID.
    pub dst_id: String,
    /// Transformation or mechanism description.
    pub via: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Scheduled dataset row returned by `ax_scheduled_datasets()`.
#[derive(Debug, Clone)]
pub struct ScheduledDataset {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// 5-field cron expression.
    pub schedule_cron: String,
    /// Last successful sync timestamp.
    pub last_synced_at: Option<DateTime<Utc>>,
}

/// Sidecar `/connectors/test` and `/connectors/discover` request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarConnectorRequest {
    /// Connector type.
    pub connector_type: ConnectorType,
    /// Plain configuration fields.
    pub config: serde_json::Map<String, serde_json::Value>,
    /// Decrypted secret fields (transmitted over internal network only, never logged).
    pub secrets: BTreeMap<String, String>,
    /// Whether private/RFC1918 addresses are permitted.
    pub allow_private_network: bool,
}

/// Sidecar `/connectors/preview` request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarPreviewRequest {
    /// Connector type.
    pub connector_type: ConnectorType,
    /// Plain configuration fields.
    pub config: serde_json::Map<String, serde_json::Value>,
    /// Decrypted secret fields.
    pub secrets: BTreeMap<String, String>,
    /// Private network flag.
    pub allow_private_network: bool,
    /// Object reference.
    #[serde(rename = "ref")]
    pub source_ref: serde_json::Value,
    /// Row limit (`<= 1000`).
    pub limit: u32,
}

/// Sidecar `POST /connectors/sync` request (`03-STEP02` §02.9.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarSyncRequest {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Dataset UUID string.
    pub dataset_id: String,
    /// Idempotent batch UUID string.
    pub batch_id: String,
    /// Connector type.
    pub connector_type: ConnectorType,
    /// Plain connector configuration.
    pub config: serde_json::Map<String, serde_json::Value>,
    /// Decrypted secrets.
    pub secrets: BTreeMap<String, String>,
    /// Private network flag.
    pub allow_private_network: bool,
    /// Dataset source locator (for `WEBHOOK`, includes buffered items).
    pub source_ref: serde_json::Value,
    /// Sync mode.
    pub sync_mode: SyncMode,
    /// Primary key columns.
    pub primary_key: Vec<String>,
    /// Cursor column.
    pub cursor_column: Option<String>,
    /// Starting cursor value.
    pub cursor_from: Option<String>,
    /// Soft-delete boolean column.
    pub delete_column: Option<String>,
    /// Current stream offsets (`partition -> offset`).
    pub stream_offsets: BTreeMap<String, String>,
    /// Target lake table name.
    pub lake_table: String,
    /// Schema evolution policy.
    pub schema_policy: SchemaPolicy,
    /// Current dataset schema (empty on first sync).
    pub expected_schema: Vec<ColumnProfile>,
    /// Quality assertion rules.
    pub quality_rules: Vec<QualityRule>,
    /// Arrow batch row size.
    pub batch_rows: u32,
}

/// Sidecar `POST /connectors/sync` and `/lake/commit_staging` item result (`03-STEP02` §02.9.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarSyncResult {
    /// Dataset UUID string.
    pub dataset_id: String,
    /// Rows read from source.
    pub rows_read: i64,
    /// Rows written to lake.
    pub rows_written: i64,
    /// Rows deleted in lake.
    pub rows_deleted: i64,
    /// Exact total live row count in the committed `DuckLake` table, when reported.
    #[serde(default)]
    pub total_rows: Option<i64>,
    /// Maximum cursor value observed.
    pub cursor_to: Option<String>,
    /// Updated stream offsets.
    #[serde(default)]
    pub stream_offsets: BTreeMap<String, String>,
    /// New `DuckLake` snapshot ID.
    pub snapshot_id: i64,
    /// Resulting table column profiles.
    pub schema: Vec<ColumnProfile>,
    /// Detected schema evolution diff.
    pub schema_change: SchemaChange,
    /// Quality validation report.
    pub quality: QualityReport,
}

/// Single item in `CommitStagingRequest` (`03-STEP02` §02.6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitStagingItem {
    /// Relative Parquet path under `AXPOINT_DATA_ROOT`.
    pub staging_path: String,
    /// Target dataset UUID string.
    pub dataset_id: String,
    /// Target lake table name.
    pub lake_table: String,
    /// Batch UUID string.
    pub batch_id: String,
    /// Schema evolution policy.
    pub schema_policy: SchemaPolicy,
    /// Existing dataset schema (empty if new).
    pub expected_schema: Vec<ColumnProfile>,
}

/// Sidecar `POST /lake/commit_staging` request (`03-STEP02` §02.6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitStagingRequest {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Parse session UUID string.
    pub parse_id: String,
    /// Staging tables to commit.
    pub items: Vec<CommitStagingItem>,
}

/// Sidecar `POST /lake/preview` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakePreviewRequest {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Lake table name.
    pub lake_table: String,
    /// Row limit.
    pub limit: u32,
    /// Optional historical snapshot version.
    pub snapshot_id: Option<i64>,
}

/// Sidecar `POST /lake/maintenance` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeMaintenanceRequest {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Snapshot retention window in days.
    pub keep_days: i32,
}

/// Sidecar `POST /lake/maintenance` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeMaintenanceResult {
    /// Adjacent data files compacted.
    pub merged_files: i64,
    /// Snapshots expired.
    pub expired_snapshots: i64,
    /// Orphan Parquet files removed.
    pub deleted_files: i64,
}
