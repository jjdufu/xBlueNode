//! `AxOps` operations, deep health, backups, industry templates, demo scenarios,
//! and delivery work-item contracts (`12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md`).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::{RetentionTarget, SyncMode};

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
    pub enum TemplateConflictStrategy {
        /// Reuse/skip conflicting object names.
        Skip => "SKIP",
        /// Append `_<pack_id>` suffix to conflicting names.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
