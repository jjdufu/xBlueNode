//! `AxOps` operations, deep health, backups, industry templates, demo scenarios,
//! and delivery work-item contracts (`12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md`).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::{Cardinality, RetentionTarget, SyncMode};

macro_rules! ops_str_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident => $str_val:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(
                $(#[$vmeta])*
                #[serde(rename = $str_val)]
                $variant,
            )+
        }

        impl $name {
            /// All variants in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Canonical wire and SQL string representation.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $str_val,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = AppError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($str_val => Ok(Self::$variant),)+
                    other => Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("unknown {} value: {other}", stringify!($name)),
                    )),
                }
            }
        }
    };
}

ops_str_enum! {
    /// Work-item category (`ax_work_items.item_kind`, `§14.1`).
    pub enum WorkItemKind {
        /// Operational or follow-up task.
        Task => "TASK",
        /// Incident or anomaly issue.
        Issue => "ISSUE",
        /// Controlled operational change request.
        Change => "CHANGE",
    }
}

ops_str_enum! {
    /// Work-item lifecycle state (`ax_work_items.status`, `§14.1`, `§14.7`).
    pub enum WorkItemStatus {
        /// Newly created and awaiting pickup.
        Open => "OPEN",
        /// Actively being worked on.
        InProgress => "IN_PROGRESS",
        /// Blocked on external dependency.
        Blocked => "BLOCKED",
        /// Completed (`closed_at` populated).
        Done => "DONE",
        /// Canceled without execution (`closed_at` populated).
        Canceled => "CANCELED",
    }
}

impl WorkItemStatus {
    /// Returns `true` if transitioning to this status sets `closed_at`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Canceled)
    }
}

ops_str_enum! {
    /// Work-item priority level (`ax_work_items.priority`, `§14.1`).
    pub enum WorkItemPriority {
        /// Critical urgent priority.
        P0 => "P0",
        /// High priority.
        P1 => "P1",
        /// Normal priority (default).
        P2 => "P2",
        /// Low priority.
        P3 => "P3",
    }
}

ops_str_enum! {
    /// Work-item provenance category (`ax_work_items.source_kind`, `§14.1`, `§14.7`).
    pub enum WorkItemSourceKind {
        /// Created interactively by a user.
        Manual => "MANUAL",
        /// Emitted by an `AxFlow` `ACTION_EMIT` `WORK_ITEM` operator.
        FlowAction => "FLOW_ACTION",
        /// Escalated or converted from an `AxAlerts` alert.
        Alert => "ALERT",
        /// Derived from an `AxSim` scenario decision.
        SimDecision => "SIM_DECISION",
        /// Generated as post-install checklist by a template pack.
        Template => "TEMPLATE",
    }
}

ops_str_enum! {
    /// Work-item audit timeline event type (`ax_work_item_events.event_kind`, `§14.1`).
    pub enum WorkItemEventKind {
        /// Work item created.
        Created => "CREATED",
        /// Lifecycle status updated.
        Status => "STATUS",
        /// Assignee changed.
        Assign => "ASSIGN",
        /// User comment added.
        Comment => "COMMENT",
        /// External entity/alert state linked (e.g., linked alert resolved).
        Link => "LINK",
    }
}

ops_str_enum! {
    /// Industry template installation lifecycle state (`ax_template_installs.status`, `§14.1`).
    pub enum TemplateInstallStatus {
        /// Plan generated and awaiting apply.
        Planned => "PLANNED",
        /// Apply currently executing.
        Installing => "INSTALLING",
        /// All template objects created and verified.
        Installed => "INSTALLED",
        /// Apply halted at an error; resumable from `object_map`.
        Failed => "FAILED",
        /// Uninstalled (unmodified objects removed).
        Uninstalled => "UNINSTALLED",
    }
}

ops_str_enum! {
    /// Name-conflict resolution policy during template planning (`§14.5.3`).
    #[derive(Default)]
    pub enum TemplateConflictStrategy {
        /// Reuse/skip conflicting object names.
        Skip => "SKIP",
        /// Append `_<pack_id>` suffix to conflicting names.
        #[default]
        Rename => "RENAME",
    }
}

ops_str_enum! {
    /// Platform backup scope (`ax_backups.backup_kind`, `§14.1`, `§14.3`).
    pub enum BackupKind {
        /// Database dump plus lake & asset files.
        Full => "FULL",
        /// PostgreSQL directory dump only.
        Database => "DATABASE",
        /// `DuckLake` & static asset files only.
        Lake => "LAKE",
    }
}

ops_str_enum! {
    /// Platform backup lifecycle state (`ax_backups.status`, `§14.1`, `§14.3`).
    pub enum BackupStatus {
        /// Queued for `axpoint-ops` or `AXPOINT_BACKUP_CMD`.
        Pending => "PENDING",
        /// Backup script currently executing.
        Running => "RUNNING",
        /// Backup completed with manifest SHA-256 recorded.
        Succeeded => "SUCCEEDED",
        /// Backup failed.
        Failed => "FAILED",
        /// Verified via `restore-drill.sh` & `drill_reconcile.py`.
        Verified => "VERIFIED",
    }
}

ops_str_enum! {
    /// Built-in one-click demo scenario (`§14.6`).
    pub enum DemoScenario {
        /// Multi-echelon factory-warehouse-order network.
        SupplyChain => "supply_chain",
        /// Production line equipment telemetry, OEE & overheat dispatch.
        ManufacturingEquipment => "manufacturing_equipment",
        /// Regional store sales, inventory & promotion simulation.
        RetailStore => "retail_store",
    }
}

/// Delivery tracking work item (`ax_work_items`, `§14.1`, `§14.7`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    /// Work-item UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Monotonic per-tenant sequence number (`1, 2, ...`).
    pub item_no: i64,
    /// Formatted display code (`WI-<item_no>`).
    pub code: String,
    /// Short summary title.
    pub title: String,
    /// Markdown description.
    pub description: String,
    /// Work-item category (`TASK`, `ISSUE`, `CHANGE`).
    pub item_kind: WorkItemKind,
    /// Current status.
    pub status: WorkItemStatus,
    /// Priority (`P0`..`P3`).
    pub priority: WorkItemPriority,
    /// Optional assignee user UUID.
    pub assignee: Option<Uuid>,
    /// Originating source category.
    pub source_kind: WorkItemSourceKind,
    /// Source reference (`run_id`, `alert_id`, `decision_id`, or `pack_id`).
    pub source_ref: String,
    /// Linked `BlueNode` UUIDs.
    pub node_ids: Vec<Uuid>,
    /// Optional SLA due timestamp.
    pub due_at: Option<DateTime<Utc>>,
    /// Creator user UUID, if known.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
    /// Completion/cancellation timestamp when in `DONE` or `CANCELED`.
    pub closed_at: Option<DateTime<Utc>>,
}

/// Timeline event on a [`WorkItem`] (`ax_work_item_events`, `§14.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemEvent {
    /// Event sequence ID.
    pub id: i64,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Parent work-item UUID.
    pub work_item_id: Uuid,
    /// Event kind (`CREATED`, `STATUS`, `ASSIGN`, `COMMENT`, `LINK`).
    pub event_kind: WorkItemEventKind,
    /// Structured event detail payload.
    pub detail: serde_json::Value,
    /// Acting user UUID (`None` for automated system/flow events).
    pub actor: Option<Uuid>,
    /// Event timestamp.
    pub created_at: DateTime<Utc>,
}

/// Detailed view returned by `GET /api/axops/work-items/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemDetail {
    /// Work-item fields flattened at top level.
    #[serde(flatten)]
    pub item: WorkItem,
    /// Chronological event timeline (`id ASC`).
    pub events: Vec<WorkItemEvent>,
}

/// Query parameters for `GET /api/axops/work-items`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkItemListQuery {
    /// Filter by status.
    pub status: Option<WorkItemStatus>,
    /// Filter by priority.
    pub priority: Option<WorkItemPriority>,
    /// Filter by assignee user UUID.
    pub assignee: Option<Uuid>,
    /// Filter by source category.
    pub source_kind: Option<WorkItemSourceKind>,
    /// Filter by exact `source_ref`.
    pub source_ref: Option<String>,
    /// Page size (`1..=200`, default `50`).
    pub limit: Option<u32>,
    /// Cursor (`item_no` string for keyset pagination).
    pub cursor: Option<String>,
}

/// Request body for `POST /api/axops/work-items`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkItemRequest {
    /// Summary title (non-empty).
    pub title: String,
    /// Optional detailed description.
    #[serde(default)]
    pub description: String,
    /// Work-item kind (defaults to `TASK`).
    pub item_kind: Option<WorkItemKind>,
    /// Priority (defaults to `P2`).
    pub priority: Option<WorkItemPriority>,
    /// Optional assignee user UUID.
    pub assignee: Option<Uuid>,
    /// Provenance kind (defaults to `MANUAL`).
    pub source_kind: Option<WorkItemSourceKind>,
    /// Provenance reference ID.
    #[serde(default)]
    pub source_ref: String,
    /// Linked `BlueNode` UUIDs.
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Optional due timestamp.
    pub due_at: Option<DateTime<Utc>>,
}

/// Input used by `AxFlow` `ACTION_EMIT` (`WORK_ITEM`) to create a work item (`§14.7`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowWorkItemInput {
    /// Work-item title.
    pub title: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Work-item kind (`TASK` default).
    pub item_kind: WorkItemKind,
    /// Work-item priority (`P2` default).
    pub priority: WorkItemPriority,
    /// Optional assignee UUID.
    pub assignee: Option<Uuid>,
    /// Originating `AxFlow` `run_id` stored as `source_ref`.
    pub run_id: Uuid,
    /// Resolved `BlueNode` UUIDs.
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Optional due timestamp.
    pub due_at: Option<DateTime<Utc>>,
    /// Optional initiator user UUID.
    pub actor: Option<Uuid>,
}

/// Request body for `PATCH /api/axops/work-items/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatchWorkItemRequest {
    /// Updated title.
    pub title: Option<String>,
    /// Updated description.
    pub description: Option<String>,
    /// Updated item kind.
    pub item_kind: Option<WorkItemKind>,
    /// Updated lifecycle status.
    pub status: Option<WorkItemStatus>,
    /// Updated priority.
    pub priority: Option<WorkItemPriority>,
    /// Assign to user UUID.
    pub assignee: Option<Uuid>,
    /// Explicitly clear `assignee` to `NULL` when `true`.
    #[serde(default)]
    pub clear_assignee: bool,
    /// Replace linked `node_ids`.
    pub node_ids: Option<Vec<Uuid>>,
    /// Updated due timestamp.
    pub due_at: Option<DateTime<Utc>>,
}

/// Request body for `POST /api/axops/work-items/{id}/comments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkItemCommentRequest {
    /// Non-empty Markdown comment text.
    pub comment: String,
}

/// Dataset descriptor inside `manifest.json` (`§14.5.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateDatasetEntry {
    /// Logical key referenced via `{"$ref": "dataset:<key>"}`.
    pub key: String,
    /// Optional human-readable dataset name (defaults to `key`).
    pub name: Option<String>,
    /// Relative CSV file path inside pack directory.
    pub file: String,
    /// Dataset sync mode (`SNAPSHOT`, `APPEND`, `UPSERT`).
    pub sync_mode: SyncMode,
    /// Optional primary key columns when `sync_mode == UPSERT`.
    #[serde(default)]
    pub primary_key: Vec<String>,
}

/// `contents` section of a template pack `manifest.json` (`§14.5.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateContents {
    /// Relative path to `SchemaDoc` JSON (`schema.json`).
    pub schema: String,
    /// Sample CSV datasets to upload and synchronize.
    #[serde(default)]
    pub datasets: Vec<TemplateDatasetEntry>,
    /// Optional relative path to dataset-to-graph mappings JSON (`mappings.json`).
    pub mappings: Option<String>,
    /// Optional relative path to semantic metrics JSON (`metrics.json`).
    pub metrics: Option<String>,
    /// Optional relative path to alert rules JSON (`rules.json`).
    pub rules: Option<String>,
    /// Relative paths to `AxFlow` definition JSON files.
    #[serde(default)]
    pub flows: Vec<String>,
    /// Relative paths to `xBlueView` dashboard JSON files.
    #[serde(default)]
    pub dashboards: Vec<String>,
    /// Relative paths to `AxAgent` definition JSON files.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Optional relative paths to export template descriptors.
    #[serde(default)]
    pub export_templates: Vec<String>,
}

/// Industry template pack `manifest.json` contract (`§14.0`, `§14.5.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateManifest {
    /// Stable pack identifier (`supply_chain`, `manufacturing_equipment`, `retail_store`).
    pub id: String,
    /// Human-readable Chinese pack title.
    pub name: String,
    /// Semantic version (`1.0.0`).
    pub version: String,
    /// Target industry label.
    pub industry: String,
    /// Summary description.
    pub description: String,
    /// Object file inventory.
    pub contents: TemplateContents,
    /// Map of relative file path to lowercase hex SHA-256 digest.
    pub sha256: BTreeMap<String, String>,
}

/// Summary plus optional `README.md` returned by `GET /api/axops/templates/{pack_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplatePackDetail {
    /// Parsed `manifest.json`.
    #[serde(flatten)]
    pub manifest: TemplateManifest,
    /// Markdown guide from `README.md`.
    pub readme: String,
}

/// Request body for `POST /api/axops/templates/{pack_id}/plan`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TemplatePlanRequest {
    /// Conflict handling mode (`SKIP` or `RENAME`, defaults to `RENAME`).
    pub on_conflict: Option<TemplateConflictStrategy>,
}

/// Single planned object entry stored in `ax_template_installs.plan` (`§14.5.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplatePlannedObject {
    /// Object category (`node_type`, `edge_type`, `dataset`, `mapping`, `metric`, `rule`, `flow`, `dashboard`, `agent`, `export_template`, `work_item`).
    pub category: String,
    /// Logical key inside template pack.
    pub key: String,
    /// Original name/slug in template pack.
    pub source_name: String,
    /// Effective target name/slug after applying `on_conflict`.
    pub target_name: String,
    /// Whether `source_name` collided with an existing object in the tenant.
    pub conflict: bool,
    /// Action to take (`CREATE`, `SKIP`, `RENAME`).
    pub action: String,
}

/// Structured `plan` JSON stored on `ax_template_installs` (`§14.5.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInstallPlan {
    /// Pack identifier.
    pub pack_id: String,
    /// Pack version.
    pub pack_version: String,
    /// Selected conflict resolution mode.
    pub on_conflict: TemplateConflictStrategy,
    /// Planned objects in dependency order.
    pub objects: Vec<TemplatePlannedObject>,
    /// Human-readable conflict descriptions (`category:name`).
    pub conflicts: Vec<String>,
}

/// Entry in `ax_template_installs.object_map` tracking created ID and content hash (`§14.5.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateCreatedObject {
    /// Created object ID (UUID string or name/slug for schema types/metrics).
    pub id: String,
    /// Object category (`dataset`, `mapping`, `metric`, `rule`, `flow`, `dashboard`, `agent`, `node_type`, `export_template`, `work_item`).
    pub category: String,
    /// Effective target name or slug created in the tenant.
    pub target_name: String,
    /// Canonical SHA-256 hash at creation time (checked before uninstall).
    pub content_hash: String,
}

/// Persistent template installation record (`ax_template_installs`, `§14.1`, `§14.5.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInstallRecord {
    /// Install record UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Pack identifier.
    pub pack_id: String,
    /// Pack version.
    pub pack_version: String,
    /// Current installation status.
    pub status: TemplateInstallStatus,
    /// Planned object inventory & conflict resolutions.
    pub plan: TemplateInstallPlan,
    /// Map of `<category>:<key>` to created object metadata & content hash.
    pub object_map: BTreeMap<String, TemplateCreatedObject>,
    /// Failure diagnostic when `status == FAILED`.
    pub error_message: Option<String>,
    /// User UUID who initiated the plan/install.
    pub installed_by: Option<Uuid>,
    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Completion timestamp (`INSTALLED`, `FAILED`, `UNINSTALLED`).
    pub finished_at: Option<DateTime<Utc>>,
}

/// Response returned by `POST /api/axops/templates/installs/{id}/uninstall` (`§14.5.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateUninstallResponse {
    /// Install record UUID.
    pub id: Uuid,
    /// Updated status (`UNINSTALLED`).
    pub status: TemplateInstallStatus,
    /// Object keys (`<category>:<key>`) deleted because content hash matched install snapshot.
    pub deleted: Vec<String>,
    /// Object keys (`<category>:<key>`) retained because user modified them post-install.
    pub kept: Vec<String>,
}

/// Response returned by `POST /api/axops/demo/{scenario}/load` (`§14.6`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemoLoadResponse {
    /// Loaded scenario identifier.
    pub scenario: DemoScenario,
    /// Template installation UUID (`INSTALLED`).
    pub install_id: Uuid,
    /// Triggered refresh `AxFlow` run UUID, if present.
    pub flow_run_id: Option<Uuid>,
    /// Number of 30-day historical time-series metric points replayed.
    pub metric_points_written: u64,
    /// Created `AxSim` sandbox UUID.
    pub sandbox_id: Option<Uuid>,
    /// Created pre-run `AxSim` scenario UUID.
    pub scenario_id: Option<Uuid>,
    /// Primary published dashboard slug ready under `/view/<slug>`.
    pub dashboard_slug: String,
}

/// Platform backup record (`ax_backups`, `§14.1`, `§14.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRecord {
    /// Backup UUID.
    pub id: Uuid,
    /// Scope (`FULL`, `DATABASE`, `LAKE`).
    pub backup_kind: BackupKind,
    /// Current status (`PENDING`, `RUNNING`, `SUCCEEDED`, `FAILED`, `VERIFIED`).
    pub status: BackupStatus,
    /// Backup directory path on storage volume.
    pub location: String,
    /// Total backup byte size once succeeded.
    pub size_bytes: Option<i64>,
    /// Manifest metadata (`migration`, `files`, etc.).
    pub manifest: serde_json::Value,
    /// Error summary if `FAILED`.
    pub error_message: Option<String>,
    /// Creation / start timestamp.
    pub started_at: DateTime<Utc>,
    /// Completion timestamp.
    pub finished_at: Option<DateTime<Utc>>,
    /// Restore-drill verification timestamp.
    pub verified_at: Option<DateTime<Utc>>,
}

/// Request body for `POST /api/axops/backups`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateBackupRequest {
    /// Requested backup scope (defaults to `FULL`).
    pub backup_kind: Option<BackupKind>,
}

/// Retention policy item (`ax_retention_policies`, `§14.4.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicyRecord {
    /// Target table/artifact category.
    pub target: RetentionTarget,
    /// Retention window in days (`> 0`).
    pub keep_days: i32,
}

/// Request body for `PUT /api/axops/retention/{target}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutRetentionPolicyRequest {
    /// Retention window in days (`> 0`).
    pub keep_days: i32,
}

/// Database probe section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseHealth {
    /// Connectivity & query check status.
    pub ok: bool,
    /// Round-trip query latency in milliseconds.
    pub latency_ms: u64,
    /// Streaming replication lag in seconds (`0.0` on standalone primary).
    pub replication_lag_s: Option<f64>,
    /// Diagnostic error if `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Sidecar `axpoint-parser` probe section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserHealth {
    /// Sidecar `/health` reachability status.
    pub ok: bool,
    /// Sidecar version string.
    pub version: String,
    /// Diagnostic error if `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `DuckLake` catalog probe section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LakeHealth {
    /// Catalog reachability status.
    pub ok: bool,
    /// Latest `DuckLake` snapshot ID (`0` when empty).
    pub snapshot_id: i64,
    /// Diagnostic error if `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// LLM provider probe section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmHealth {
    /// Provider availability status.
    pub ok: bool,
    /// Active provider name (`"cassette"` when offline replay is enabled).
    pub provider: String,
    /// Diagnostic error if `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Embedding model probe section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedHealth {
    /// Embedding gateway availability status.
    pub ok: bool,
    /// Active embedding provider or model identifier, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Diagnostic error if `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Data volume free-space section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskHealth {
    /// `true` when `AXPOINT_DATA_ROOT` is stat-able and has free space.
    pub ok: bool,
    /// Available free bytes on `AXPOINT_DATA_ROOT` filesystem.
    pub data_root_free_bytes: u64,
}

/// `ax_jobs` queue depth & failure summary (`§14.4.3`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobsHealthStats {
    /// Count of `PENDING` jobs.
    pub pending: i64,
    /// Count of `RUNNING` jobs.
    pub running: i64,
    /// Count of `FAILED` jobs updated within the last 24 hours.
    pub failed_24h: i64,
    /// Age in seconds of the oldest `PENDING` job (`0` if none).
    pub oldest_pending_s: i64,
}

/// `ax_outbox` backlog summary returned by `GET /api/axops/outbox/stats` & `health/deep` (`§14.4.3`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboxStats {
    /// Age in seconds of the oldest unprocessed event (`0` if none).
    pub lag_s: i64,
    /// Count of unprocessed (`processed_at IS NULL`) outbox events.
    pub pending: i64,
}

/// SQL migration state section of [`DeepHealthReport`] (`§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationsHealth {
    /// `true` when `pending == 0`.
    pub ok: bool,
    /// Highest applied `_sqlx_migrations.version`.
    pub current: i64,
    /// Count of embedded migrations not yet applied in `_sqlx_migrations`.
    pub pending: usize,
}

/// Deep operational health report (`GET /api/axops/health/deep`, `§14.4.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepHealthReport {
    /// PostgreSQL connectivity, latency, and replication lag.
    pub database: DatabaseHealth,
    /// Sidecar `axpoint-parser` connectivity and version.
    pub parser: ParserHealth,
    /// `DuckLake` catalog connectivity and snapshot ID.
    pub lake: LakeHealth,
    /// Active LLM provider health.
    pub llm: LlmHealth,
    /// Active embedding model health.
    pub embed: EmbedHealth,
    /// `AXPOINT_DATA_ROOT` filesystem free space.
    pub disk: DiskHealth,
    /// Background `ax_jobs` queue telemetry.
    pub jobs: JobsHealthStats,
    /// Transactional `ax_outbox` lag telemetry.
    pub outbox: OutboxStats,
    /// Database schema migration sync status.
    pub migrations: MigrationsHealth,
    /// `axpoint-server` binary crate version.
    pub version: String,
}

/// Runtime metadata returned by `GET /api/axops/system/info` (`§14.8`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoResponse {
    /// `axpoint-server` crate version.
    pub version: String,
    /// Deployment environment (`development`, `test`, `production`).
    pub env: String,
    /// Requesting principal's tenant ID.
    pub tenant_id: String,
    /// Process uptime in seconds.
    pub uptime_s: u64,
    /// Configured `AXPOINT_DATA_ROOT` path.
    pub data_root: String,
    /// Count of built-in industry template packs discovered on disk.
    pub template_packs_count: usize,
}

/// Parameters for creating a `ax_template_installs` plan row in `OpsRepository`.
#[derive(Debug, Clone)]
pub struct NewTemplateInstallInput {
    /// Pack identifier.
    pub pack_id: String,
    /// Pack version string.
    pub pack_version: String,
    /// Computed plan JSON.
    pub plan: TemplateInstallPlan,
    /// Initiating user UUID.
    pub installed_by: Option<Uuid>,
}

ops_str_enum! {
    /// Canonical stage identifier in the 5-step tenant onboarding tracker (`05-STEP19` §19.1.1).
    pub enum OnboardingStepKey {
        /// Stage 1: dataset or knowledge document ingestion (`ds_datasets > 0 || hg_docs > 0`).
        DataIngest => "DATA_INGEST",
        /// Stage 2: node schema modeling & publication (`xb_node_types > 0 && xb_schema_versions >= 1`).
        NodeModeling => "NODE_MODELING",
        /// Stage 3: runtime node materialization & workflow orchestration (`xb_nodes > 0`).
        MappingAndFlow => "MAPPING_AND_FLOW",
        /// Stage 4: causal simulation verification (`ax_sim_scenarios.SUCCEEDED > 0 || ax_sandboxes > 0`).
        SimVerification => "SIM_VERIFICATION",
        /// Stage 5: twin cockpit & domain agent packaging (`xv_dashboards > 0 || ax_agents > 0`).
        CockpitAndAgent => "COCKPIT_AND_AGENT",
    }
}

/// Single step item inside [`OnboardingStatusResponse`] (`05-STEP19` §19.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingStepItem {
    /// 1-based stage ordinal (`1..=5`).
    pub step_no: u8,
    /// Stage machine key.
    pub step_key: OnboardingStepKey,
    /// Chinese stage label.
    pub label_zh: String,
    /// English stage label.
    pub label_en: String,
    /// Whether this stage has met its live completion criterion.
    pub completed: bool,
    /// Primary asset count displayed in the stage pill.
    pub count_primary: u64,
    /// Secondary asset count displayed in the stage pill.
    pub count_secondary: u64,
    /// Formatted live metric badge label.
    pub metric_label: String,
    /// Frontend route path navigated to when continuing or clicking the step.
    pub target_path: String,
}

/// Response returned by `GET /api/axops/onboarding-status` (`05-STEP19` §19.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingStatusResponse {
    /// Count of completed stages (`0..=5`).
    pub completed_steps: u8,
    /// Fixed total stage count (`5`).
    pub total_steps: u8,
    /// Completion percentage (`completed_steps * 20`, in `0..=100`).
    pub progress_pct: u8,
    /// First incomplete stage key (`None` when all `5 / 5` stages are complete).
    pub recommended_next_step: Option<OnboardingStepKey>,
    /// Whether at least one `is_dev_sandbox = true` trial subgraph exists in the tenant.
    pub has_trial_subgraph: bool,
    /// Ordered 5-stage status breakdown.
    pub steps: Vec<OnboardingStepItem>,
}

/// Raw tenant asset counters queried in one read-only transaction to compute [`OnboardingStatusResponse`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingRawCounts {
    /// Count of `ds_datasets` rows.
    pub dataset_count: u64,
    /// Count of `hg_docs` rows.
    pub doc_count: u64,
    /// Count of `xb_node_types` rows.
    pub node_type_count: u64,
    /// Count of `xb_edge_types` rows.
    pub edge_type_count: u64,
    /// Count of `xb_schema_versions` rows across subgraphs.
    pub schema_version_count: u64,
    /// Count of materialized `xb_nodes` rows.
    pub node_count: u64,
    /// Count of `ax_flows` rows.
    pub flow_count: u64,
    /// Count of `ax_sim_scenarios` rows with `status = 'SUCCEEDED'`.
    pub succeeded_scenario_count: u64,
    /// Count of `ax_sandboxes` rows.
    pub sandbox_count: u64,
    /// Count of `xv_dashboards` rows.
    pub dashboard_count: u64,
    /// Count of `ax_agents` rows.
    pub agent_count: u64,
    /// Whether the tenant currently has at least one `is_dev_sandbox = true` subgraph.
    pub has_trial_subgraph: bool,
}

/// Deterministically derives the 5-step [`OnboardingStatusResponse`] from [`OnboardingRawCounts`] (`05-STEP19` §19.1.1).
#[must_use]
pub fn build_onboarding_status(counts: &OnboardingRawCounts) -> OnboardingStatusResponse {
    let steps = vec![
        OnboardingStepItem {
            step_no: 1,
            step_key: OnboardingStepKey::DataIngest,
            label_zh: String::from("1. 数据接入"),
            label_en: String::from("1. Data Ingest"),
            completed: counts.dataset_count > 0 || counts.doc_count > 0,
            count_primary: counts.dataset_count,
            count_secondary: counts.doc_count,
            metric_label: format!(
                "{} 数据集 · {} 文档",
                counts.dataset_count, counts.doc_count
            ),
            target_path: String::from("/hub"),
        },
        OnboardingStepItem {
            step_no: 2,
            step_key: OnboardingStepKey::NodeModeling,
            label_zh: String::from("2. 节点建模"),
            label_en: String::from("2. Node Modeling"),
            completed: counts.node_type_count > 0 && counts.schema_version_count >= 1,
            count_primary: counts.node_type_count,
            count_secondary: counts.edge_type_count,
            metric_label: format!(
                "{} 节点类型 · {} 关系",
                counts.node_type_count, counts.edge_type_count
            ),
            target_path: String::from("/studio"),
        },
        OnboardingStepItem {
            step_no: 3,
            step_key: OnboardingStepKey::MappingAndFlow,
            label_zh: String::from("3. 映射与编排"),
            label_en: String::from("3. Mapping & Flow"),
            completed: counts.node_count > 0,
            count_primary: counts.node_count,
            count_secondary: counts.flow_count,
            metric_label: format!(
                "{} 业务节点 · {} 流程",
                counts.node_count, counts.flow_count
            ),
            target_path: String::from("/flow"),
        },
        OnboardingStepItem {
            step_no: 4,
            step_key: OnboardingStepKey::SimVerification,
            label_zh: String::from("4. 推演验证"),
            label_en: String::from("4. Simulation"),
            completed: counts.succeeded_scenario_count > 0 || counts.sandbox_count > 0,
            count_primary: counts.succeeded_scenario_count,
            count_secondary: counts.sandbox_count,
            metric_label: format!("{} 推演方案", counts.succeeded_scenario_count),
            target_path: String::from("/sim"),
        },
        OnboardingStepItem {
            step_no: 5,
            step_key: OnboardingStepKey::CockpitAndAgent,
            label_zh: String::from("5. 驾驶舱与智能体"),
            label_en: String::from("5. Apps & Agents"),
            completed: counts.dashboard_count > 0 || counts.agent_count > 0,
            count_primary: counts.dashboard_count,
            count_secondary: counts.agent_count,
            metric_label: format!(
                "{} 驾驶舱 · {} 智能体",
                counts.dashboard_count, counts.agent_count
            ),
            target_path: String::from("/view"),
        },
    ];

    let completed_steps = u8::try_from(steps.iter().filter(|s| s.completed).count()).unwrap_or(5);
    let progress_pct = completed_steps.saturating_mul(20);
    let recommended_next_step = steps.iter().find(|s| !s.completed).map(|s| s.step_key);

    OnboardingStatusResponse {
        completed_steps,
        total_steps: 5,
        progress_pct,
        recommended_next_step,
        has_trial_subgraph: counts.has_trial_subgraph,
        steps,
    }
}

/// Node type summary item in [`TemplateTopologySummary`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateNodeTypePreview {
    /// Machine `NodeTypeSpec.name`.
    pub name: String,
    /// Human-readable display label.
    pub display_name: String,
    /// Total attribute and metric property count.
    pub property_count: usize,
    /// Numeric metric property count.
    pub metric_count: usize,
}

/// Relationship type summary item in [`TemplateTopologySummary`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateEdgeTypePreview {
    /// Relationship type identifier.
    pub name: String,
    /// Source `NodeTypeSpec.name`.
    pub src_type_name: String,
    /// Destination `NodeTypeSpec.name`.
    pub dst_type_name: String,
    /// Relationship cardinality (`ONE_TO_ONE`, `ONE_TO_MANY`, `MANY_TO_ONE`, `MANY_TO_MANY`).
    pub cardinality: Cardinality,
}

/// Entity-relationship topology summary inside [`TemplatePreviewResponse`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateTopologySummary {
    /// Node type summaries declared in `schema.json`.
    pub node_types: Vec<TemplateNodeTypePreview>,
    /// Directed edge type summaries declared in `schema.json`.
    pub edge_types: Vec<TemplateEdgeTypePreview>,
    /// Total cross-link aggregation rule count across node types.
    pub agg_rule_count: usize,
    /// Total intra-node derived formula rule count across node types.
    pub derived_rule_count: usize,
}

/// Sample semantic or node metric item inside [`TemplatePreviewResponse`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSampleMetricPreview {
    /// Metric machine identifier.
    pub name: String,
    /// Human-readable metric title.
    pub display_name: String,
    /// Measurement unit label (empty when dimensionless).
    #[serde(default)]
    pub unit: String,
}

/// Dashboard preview item inside [`TemplatePreviewResponse`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateDashboardPreviewItem {
    /// Dashboard URL slug.
    pub slug: String,
    /// Dashboard display name.
    pub name: String,
    /// Number of widgets configured on the dashboard.
    pub widget_count: usize,
}

/// Bundled `AxAgent` preview item inside [`TemplatePreviewResponse`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateAgentPreviewItem {
    /// Agent identifier / name.
    pub name: String,
    /// Agent summary description.
    #[serde(default)]
    pub description: String,
    /// Number of bound tools declared in `spec.tools`.
    pub tool_count: usize,
}

/// Tabular CSV sample preview for a dataset inside [`TemplatePreviewResponse`] (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSampleRowsPreview {
    /// Dataset name in `manifest.contents.datasets`.
    pub dataset_name: String,
    /// Ordered CSV column headers.
    pub headers: Vec<String>,
    /// Sample row string cells aligned with `headers`.
    pub rows: Vec<Vec<String>>,
    /// Total data row count in the CSV file.
    pub total_rows: usize,
}

/// Read-only preview bundle returned by `GET /api/axops/templates/{id}/preview` (`05-STEP19` §19.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplatePreviewResponse {
    /// Parsed `manifest.json` metadata.
    pub manifest: TemplateManifest,
    /// Pack `README.md` Markdown content.
    pub readme_markdown: String,
    /// Node/edge topology and rule counts extracted from `schema.json`.
    pub topology_summary: TemplateTopologySummary,
    /// Sample semantic and node metrics.
    #[serde(default)]
    pub sample_metrics: Vec<TemplateSampleMetricPreview>,
    /// Bundled twin dashboard summaries.
    #[serde(default)]
    pub dashboard_previews: Vec<TemplateDashboardPreviewItem>,
    /// Sample CSV dataset rows.
    #[serde(default)]
    pub sample_rows: Vec<TemplateSampleRowsPreview>,
    /// Bundled domain agent summaries.
    #[serde(default)]
    pub agents: Vec<TemplateAgentPreviewItem>,
}

/// Request body for production full-pack installation `POST /api/axops/templates/{id}/install` (`05-STEP19` §19.2.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTemplateRequest {
    /// When `true`, computes and records the installation plan without applying mutations.
    #[serde(default)]
    pub dry_run: bool,
    /// Conflict resolution policy (`RENAME` default, or `SKIP`).
    #[serde(default)]
    pub on_conflict: TemplateConflictStrategy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_item_enums_round_trip() {
        for status in WorkItemStatus::ALL {
            assert_eq!(
                WorkItemStatus::from_str(status.as_str()).ok(),
                Some(*status)
            );
        }
        assert!(WorkItemStatus::Done.is_terminal());
        assert!(WorkItemStatus::Canceled.is_terminal());
        assert!(!WorkItemStatus::Open.is_terminal());
    }

    #[test]
    fn demo_scenario_wire_names_match_blueprint() {
        assert_eq!(DemoScenario::SupplyChain.as_str(), "supply_chain");
        assert_eq!(
            DemoScenario::ManufacturingEquipment.as_str(),
            "manufacturing_equipment"
        );
        assert_eq!(DemoScenario::RetailStore.as_str(), "retail_store");
    }

    #[test]
    fn onboarding_status_derives_empty_partial_and_complete_states() {
        let empty = build_onboarding_status(&OnboardingRawCounts::default());
        assert_eq!(empty.total_steps, 5);
        assert_eq!(empty.completed_steps, 0);
        assert_eq!(empty.progress_pct, 0);
        assert_eq!(
            empty.recommended_next_step,
            Some(OnboardingStepKey::DataIngest)
        );
        assert!(!empty.has_trial_subgraph);
        assert_eq!(empty.steps.len(), 5);

        // Step 1 done via docs, Step 2 still incomplete without published schema_version, Step 3 done via nodes.
        let gapped = build_onboarding_status(&OnboardingRawCounts {
            doc_count: 2,
            node_type_count: 4,
            edge_type_count: 3,
            schema_version_count: 0,
            node_count: 12,
            has_trial_subgraph: true,
            ..OnboardingRawCounts::default()
        });
        assert_eq!(gapped.completed_steps, 2);
        assert_eq!(gapped.progress_pct, 40);
        assert_eq!(
            gapped.recommended_next_step,
            Some(OnboardingStepKey::NodeModeling)
        );
        assert!(gapped.has_trial_subgraph);
        assert_eq!(
            gapped.steps.get(1).map(|s| s.target_path.as_str()),
            Some("/studio")
        );

        let complete = build_onboarding_status(&OnboardingRawCounts {
            dataset_count: 4,
            doc_count: 1,
            node_type_count: 4,
            edge_type_count: 3,
            schema_version_count: 1,
            node_count: 24,
            flow_count: 1,
            succeeded_scenario_count: 1,
            sandbox_count: 1,
            dashboard_count: 1,
            agent_count: 1,
            has_trial_subgraph: true,
        });
        assert_eq!(complete.completed_steps, 5);
        assert_eq!(complete.progress_pct, 100);
        assert_eq!(complete.recommended_next_step, None);
    }

    #[test]
    fn install_template_request_defaults_cleanly() -> Result<(), serde_json::Error> {
        let parsed: InstallTemplateRequest = serde_json::from_str("{}")?;
        assert!(!parsed.dry_run);
        assert_eq!(parsed.on_conflict, TemplateConflictStrategy::Rename);
        Ok(())
    }
}
