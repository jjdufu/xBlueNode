//! `AxFlow` DAG definitions, execution state records, approval/action DTOs, and object-safe [`Operator`] contract (`07-STEP07-08` §07).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::{Environment, FlowOperator, Permission, RunStatus, StepStatus, TriggerType};
use crate::platform::BoxFuture;

const fn default_retry_attempts() -> u32 {
    3
}

const fn default_retry_backoff_s() -> u32 {
    10
}

const fn default_timeout_s() -> u32 {
    600
}

fn default_empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// 2D canvas coordinate for a [`FlowNode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    /// Horizontal canvas coordinate.
    #[serde(default)]
    pub x: f64,
    /// Vertical canvas coordinate.
    #[serde(default)]
    pub y: f64,
}

/// Retry delay progression strategy for a [`FlowNode`] (`07-STEP07-08` §07.2).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum BackoffStrategy {
    /// Constant `backoff_s` delay between attempts.
    #[default]
    #[serde(rename = "FIXED")]
    Fixed,
    /// Exponential `backoff_s * 2^(attempt - 1)` delay capped at 3,600 seconds.
    #[serde(rename = "EXPONENTIAL")]
    Exponential,
}

/// Node-level retry policy (`07-STEP07-08` §07.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum total attempts (`1..=10`, default `3`).
    #[serde(default = "default_retry_attempts")]
    pub max_attempts: u32,
    /// Base backoff delay in seconds (`1..=3600`, default `10`).
    #[serde(default = "default_retry_backoff_s")]
    pub backoff_s: u32,
    /// Delay scaling mode (`FIXED` or `EXPONENTIAL`).
    #[serde(default)]
    pub backoff: BackoffStrategy,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_retry_attempts(),
            backoff_s: default_retry_backoff_s(),
            backoff: BackoffStrategy::Fixed,
        }
    }
}

impl RetryPolicy {
    /// Computes the backoff delay in seconds before scheduling the next retry after `failed_attempt` (`1`-based).
    #[must_use]
    pub fn delay_secs_after(self, failed_attempt: u32) -> u32 {
        let base = self.backoff_s.clamp(1, 3_600);
        match self.backoff {
            BackoffStrategy::Fixed => base,
            BackoffStrategy::Exponential => {
                let shift = failed_attempt.saturating_sub(1).min(12);
                base.saturating_mul(1_u32 << shift).min(3_600)
            }
        }
    }
}

/// Run-level behavior when a [`FlowNode`] exhausts all retries (`07-STEP07-08` §07.2).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum OnFailure {
    /// Immediately mark the run `FAILED` and skip remaining `PENDING` steps.
    #[default]
    #[serde(rename = "FAIL_RUN")]
    FailRun,
    /// Allow independent branches and `ALL_DONE` / `ANY_SUCCESS` successors to proceed.
    #[serde(rename = "CONTINUE")]
    Continue,
}

/// Predecessor join semantics for a [`FlowNode`] (`07-STEP07-08` §07.2 & §07.3.2).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum JoinMode {
    /// Execute only when every incoming edge is active (`SUCCEEDED` and `when` true).
    #[default]
    #[serde(rename = "ALL_SUCCESS")]
    AllSuccess,
    /// Always execute once all predecessors reach any terminal state.
    #[serde(rename = "ALL_DONE")]
    AllDone,
    /// Execute when at least one incoming edge is active once all predecessors finish.
    #[serde(rename = "ANY_SUCCESS")]
    AnySuccess,
}

/// Single operator node in an `AxFlow` DAG (`07-STEP07-08` §07.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNode {
    /// Flow-unique identifier matching `^[a-z][a-z0-9_]{0,31}$`.
    pub id: String,
    /// Operator kind.
    pub kind: FlowOperator,
    /// Human-readable node label.
    #[serde(default)]
    pub label: String,
    /// Operator-specific JSON configuration (may contain `{{ ... }}` templates).
    #[serde(default = "default_empty_object")]
    pub config: serde_json::Value,
    /// Node retry policy.
    #[serde(default)]
    pub retry: RetryPolicy,
    /// Per-attempt execution timeout in seconds (`1..=86_400`, default `600`).
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u32,
    /// Terminal failure handling mode.
    #[serde(default)]
    pub on_failure: OnFailure,
    /// Predecessor join mode.
    #[serde(default)]
    pub join: JoinMode,
    /// Canvas position.
    #[serde(default)]
    pub pos: Pos,
}

/// Directed dependency edge between two [`FlowNode`]s (`07-STEP07-08` §07.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowEdge {
    /// Upstream predecessor node ID.
    pub from: String,
    /// Downstream successor node ID.
    pub to: String,
    /// Optional Step 06 boolean guard expression evaluated on upstream top-level output fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// Directed acyclic workflow graph bundle (`07-STEP07-08` §07.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlowGraph {
    /// Workflow operator nodes.
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    /// Directed dependency edges.
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
}

/// Trigger configuration attached to an `AxFlow` draft or version (`07-STEP07-08` §07.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerCfg {
    /// Trigger source discriminant (`MANUAL`, `CRON`, `OUTBOX_EVENT`, `WEBHOOK`, etc.).
    #[serde(rename = "type")]
    pub kind: TriggerType,
    /// Cron schedule expression (`5` or `6` fields) when `type == "CRON"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Optional IANA timezone (defaults to tenant timezone / `Asia/Shanghai`) for `CRON`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Whether missed cron intervals catch up sequentially (`false` by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up: Option<bool>,
    /// Target `OutboxTopic` wire string when `type == "OUTBOX_EVENT"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Optional Step 06 filter expression evaluated on outbox event payload fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

impl Default for TriggerCfg {
    fn default() -> Self {
        Self::manual()
    }
}

impl TriggerCfg {
    /// Returns the default `MANUAL` trigger configuration.
    #[must_use]
    pub const fn manual() -> Self {
        Self {
            kind: TriggerType::Manual,
            cron: None,
            timezone: None,
            catch_up: None,
            topic: None,
            filter: None,
        }
    }
}

/// Severity of a static [`FlowIssue`] diagnostic.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum FlowIssueSeverity {
    /// Blocks version publication.
    #[default]
    #[serde(rename = "ERROR")]
    Error,
    /// Non-blocking advisory diagnostic.
    #[serde(rename = "WARNING")]
    Warning,
}

/// Validation diagnostic produced by `axpoint-flow::validate` (`07-STEP07-08` §07.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowIssue {
    /// Canonical error code (`INVALID_ARGUMENT`, `CYCLE_DETECTED`, `UNSUPPORTED_FORMAT`, `SCHEMA_INVALID`, `EXPR_INVALID`).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Diagnostic severity.
    #[serde(default)]
    pub severity: FlowIssueSeverity,
    /// Optional offending node ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

/// Response payload for `POST /api/axflow/flows/{id}/validate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateFlowResponse {
    /// `true` when no `ERROR`-severity issues exist.
    pub valid: bool,
    /// Validation diagnostics.
    pub issues: Vec<FlowIssue>,
}

/// Query parameters for `GET /api/axflow/flows`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowListQuery {
    /// Optional name/description substring filter.
    #[serde(default)]
    pub q: Option<String>,
    /// Page size (`1..=500`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Request payload for `PUT /api/axflow/flows/{id}` (`07-STEP07-08` §07.6).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAxFlowRequest {
    /// Updated flow name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated flow description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated webhook slug (`Some("")` clears slug to `NULL`).
    #[serde(default)]
    pub slug: Option<String>,
    /// Updated draft DAG nodes.
    #[serde(default)]
    pub nodes: Option<Vec<FlowNode>>,
    /// Updated draft DAG edges.
    #[serde(default)]
    pub edges: Option<Vec<FlowEdge>>,
    /// Updated draft trigger configuration.
    #[serde(default)]
    pub trigger_cfg: Option<TriggerCfg>,
}

/// Immutable published flow version record (`ax_flow_versions`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowVersionRecord {
    /// Version row UUID.
    pub id: Uuid,
    /// Owning flow UUID.
    pub flow_id: Uuid,
    /// Monotonic 1-based version number.
    pub version: i32,
    /// Snapshot of DAG nodes.
    pub nodes: Vec<FlowNode>,
    /// Snapshot of DAG edges.
    pub edges: Vec<FlowEdge>,
    /// Snapshot of trigger configuration.
    pub trigger_cfg: TriggerCfg,
    /// Release note.
    pub note: String,
    /// Publisher user UUID.
    pub published_by: Option<Uuid>,
    /// Publication timestamp.
    pub published_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axflow/flows/{id}/versions`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishFlowVersionRequest {
    /// Optional release note.
    #[serde(default)]
    pub note: String,
}

/// Environment deployment binding (`ax_flow_deployments`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowDeploymentRecord {
    /// Flow UUID.
    pub flow_id: Uuid,
    /// Target environment (`TEST` or `PROD`).
    pub environment: Environment,
    /// Deployed version number.
    pub version: i32,
    /// Actor user UUID.
    pub deployed_by: Option<Uuid>,
    /// Deployment timestamp.
    pub deployed_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axflow/flows/{id}/deploy`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployFlowRequest {
    /// Target environment (`TEST` requires `FLOW_EDIT`; `PROD` requires `FLOW_PUBLISH`).
    pub environment: Environment,
    /// Published version number to bind.
    pub version: i32,
}

/// Response payload for `GET /api/axflow/flows/{id}/versions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowVersionsListResponse {
    /// Published versions ordered by `version DESC`.
    pub items: Vec<FlowVersionRecord>,
    /// Active environment bindings (`TEST` / `PROD`) for this flow.
    pub deployments: Vec<FlowDeploymentRecord>,
    /// Opaque pagination cursor (`None` when all versions fit).
    pub next_cursor: Option<String>,
}

/// Request payload for `POST /api/axflow/flows/{id}/runs`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartFlowRunRequest {
    /// Target environment (defaults to `TEST` when omitted for manual runs).
    #[serde(default)]
    pub environment: Option<Environment>,
    /// Optional explicit version override.
    #[serde(default)]
    pub version: Option<i32>,
    /// Trigger input payload accessible via `{{ trigger.* }}`.
    #[serde(default = "default_empty_object")]
    pub payload: serde_json::Value,
    /// Optional deduplication key scoped to `(flow_id, idempotency_key)`.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Response payload for `POST /api/axflow/flows/{id}/runs` and `/run` (`HTTP 202` or `200` on existing idempotent run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartFlowRunResponse {
    /// Run UUID.
    pub run_id: Uuid,
    /// Current run status.
    pub status: RunStatus,
    /// `true` when an existing run was returned due to `(flow_id, idempotency_key)` deduplication.
    #[serde(default)]
    pub existing: bool,
}

/// Persistent workflow run record (`ax_flow_runs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Run UUID.
    pub id: Uuid,
    /// Flow UUID.
    pub flow_id: Uuid,
    /// Pinned flow version number.
    pub version: i32,
    /// Execution environment (`TEST` or `PROD`).
    pub environment: Environment,
    /// Trigger source.
    pub trigger_source: TriggerType,
    /// Trigger input JSON payload.
    pub trigger_payload: serde_json::Value,
    /// Optional caller-supplied idempotency key.
    pub idempotency_key: Option<String>,
    /// Current lifecycle status.
    pub status: RunStatus,
    /// Terminal leaf node outputs map (`{ node_id: output }`).
    pub outputs: serde_json::Value,
    /// Optional run-level failure reason.
    pub error_message: Option<String>,
    /// Initiating user UUID (`None` for system/cron/webhook triggers).
    pub started_by: Option<Uuid>,
    /// Row creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Execution start timestamp.
    pub started_at: Option<DateTime<Utc>>,
    /// Terminal completion timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Query parameters for `GET /api/axflow/flows/{id}/runs`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunListQuery {
    /// Optional environment filter.
    #[serde(default)]
    pub environment: Option<Environment>,
    /// Optional run status filter.
    #[serde(default)]
    pub status: Option<RunStatus>,
    /// Page size (`1..=500`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Persistent step attempt execution log (`ax_flow_step_logs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    /// Step log `BIGINT` primary key.
    pub id: i64,
    /// Owning run UUID.
    pub run_id: Uuid,
    /// DAG node identifier.
    pub node_id: String,
    /// 1-based attempt counter.
    pub attempt: i32,
    /// Step execution status.
    pub status: StepStatus,
    /// Backing `ax_jobs` task UUID (`FLOW_STEP`).
    pub job_id: Option<Uuid>,
    /// Rendered operator configuration input.
    pub input: serde_json::Value,
    /// Inline JSON output when `<= 1 MiB`.
    pub output: Option<serde_json::Value>,
    /// Relative path under `AXPOINT_DATA_ROOT` when output exceeds `1 MiB`.
    pub output_ref: Option<String>,
    /// Machine-readable error code on failure.
    pub error_code: Option<String>,
    /// Diagnostic error message on failure.
    pub error_message: Option<String>,
    /// Attempt start timestamp.
    pub started_at: Option<DateTime<Utc>>,
    /// Attempt completion timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Payload serialized into `ax_jobs.payload` for [`crate::enums::JobKind::FlowStep`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowStepJobPayload {
    /// Target run UUID.
    pub run_id: Uuid,
    /// Target DAG node ID.
    pub node_id: String,
    /// 1-based attempt number.
    pub attempt: i32,
}

impl FlowStepJobPayload {
    /// Canonical `ax_jobs` idempotency key `<run_id>:<node_id>:<attempt>` (`07-STEP07-08` §07.3.2).
    #[must_use]
    pub fn idempotency_key(&self) -> String {
        format!("{}:{}:{}", self.run_id, self.node_id, self.attempt)
    }
}

/// Source subsystem of an `ax_approvals` gate.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum ApprovalSourceKind {
    /// `AxFlow` `HUMAN_APPROVAL` operator.
    #[default]
    #[serde(rename = "FLOW")]
    Flow,
    /// `AxSim` scenario apply gate (Step 10).
    #[serde(rename = "SIM_APPLY")]
    SimApply,
    /// `AxAgent` tool approval gate (Step 09).
    #[serde(rename = "AGENT")]
    Agent,
}

/// Lifecycle status of an `ax_approvals` row.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum ApprovalStatus {
    /// Waiting for an eligible assignee decision.
    #[default]
    #[serde(rename = "PENDING")]
    Pending,
    /// Approved by an assignee.
    #[serde(rename = "APPROVED")]
    Approved,
    /// Rejected by an assignee.
    #[serde(rename = "REJECTED")]
    Rejected,
    /// Expired past `expires_at`.
    #[serde(rename = "EXPIRED")]
    Expired,
}

/// Human approval gate record (`ax_approvals`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// Approval UUID.
    pub id: Uuid,
    /// Associated `AxFlow` run UUID, if `source_kind == FLOW`.
    pub run_id: Option<Uuid>,
    /// Associated DAG node ID, if `source_kind == FLOW`.
    pub node_id: Option<String>,
    /// Originating subsystem.
    pub source_kind: ApprovalSourceKind,
    /// Subsystem-specific reference identifier.
    pub source_ref: String,
    /// Approval title.
    pub title: String,
    /// Structured context detail JSON.
    pub detail: serde_json::Value,
    /// Role names allowed to decide this approval.
    pub assignee_roles: Vec<String>,
    /// User UUIDs allowed to decide this approval.
    pub assignee_users: Vec<Uuid>,
    /// Current status.
    pub status: ApprovalStatus,
    /// Deciding user UUID.
    pub decided_by: Option<Uuid>,
    /// Decision timestamp.
    pub decided_at: Option<DateTime<Utc>>,
    /// Reviewer comment.
    pub comment: String,
    /// Optional expiration deadline.
    pub expires_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Input parameters for creating (or idempotently reusing) an `ax_approvals` row.
#[derive(Debug, Clone)]
pub struct NewApprovalInput {
    /// Associated flow run UUID.
    pub run_id: Option<Uuid>,
    /// Associated DAG node ID.
    pub node_id: Option<String>,
    /// Originating subsystem.
    pub source_kind: ApprovalSourceKind,
    /// Subsystem-specific reference key.
    pub source_ref: String,
    /// Approval title.
    pub title: String,
    /// Structured context detail JSON.
    pub detail: serde_json::Value,
    /// Eligible role names.
    pub assignee_roles: Vec<String>,
    /// Eligible user UUIDs.
    pub assignee_users: Vec<Uuid>,
    /// Optional deadline.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Query parameters for `GET /api/axflow/approvals`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalListQuery {
    /// Optional status filter (e.g., `PENDING`).
    #[serde(default)]
    pub status: Option<ApprovalStatus>,
    /// When `true`, restrict to approvals matching the caller's roles or `user_id`.
    #[serde(default)]
    pub mine: Option<bool>,
    /// Optional flow run UUID filter.
    #[serde(default)]
    pub run_id: Option<Uuid>,
    /// Page size (`1..=500`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Decision action submitted to `POST /api/axflow/approvals/{id}/decide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// Approve and resume downstream execution.
    #[serde(rename = "APPROVE")]
    Approve,
    /// Reject and fail the suspended step with `APPROVAL_REJECTED`.
    #[serde(rename = "REJECT")]
    Reject,
}

/// Request payload for `POST /api/axflow/approvals/{id}/decide`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideApprovalRequest {
    /// Approval decision (`APPROVE` or `REJECT`).
    pub decision: ApprovalDecision,
    /// Optional reviewer comment.
    #[serde(default)]
    pub comment: String,
}

/// Source subsystem of an `ax_action_log` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ActionSourceKind {
    /// Emitted by an `AxFlow` `ACTION_EMIT` or `HTTP_CALL` step.
    #[serde(rename = "FLOW")]
    Flow,
    /// Emitted when applying an `AxSim` decision.
    #[serde(rename = "SIM_APPLY")]
    SimApply,
    /// Emitted by an `AxAgent` tool action.
    #[serde(rename = "AGENT")]
    Agent,
    /// Emitted by an operational rule.
    #[serde(rename = "RULE")]
    Rule,
    /// Emitted by an `xBlueView` action widget.
    #[serde(rename = "VIEW")]
    View,
}

/// Writeback action category in `ax_action_log`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ActionKind {
    /// Write a numeric metric point to `xb_metric_series` (`source_tag = ACTION`).
    #[serde(rename = "WRITE_NODE_METRIC")]
    WriteNodeMetric,
    /// Patch a static property on `xb_nodes`.
    #[serde(rename = "WRITE_NODE_PROPERTY")]
    WriteNodeProperty,
    /// Outbound HTTP request.
    #[serde(rename = "HTTP")]
    Http,
    /// Operational notification dispatch.
    #[serde(rename = "NOTIFY")]
    Notify,
    /// Operational work item creation (Step 14; rejected with `UNSUPPORTED_FORMAT` before Step 14).
    #[serde(rename = "WORK_ITEM")]
    WorkItem,
}

/// Execution outcome status in `ax_action_log`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ActionLogStatus {
    /// Side effect executed in `PROD`.
    #[serde(rename = "DONE")]
    Done,
    /// Recorded without side effects (`TEST` environment or `dry_run = true`).
    #[serde(rename = "DRY_RUN")]
    DryRun,
    /// Execution failed.
    #[serde(rename = "FAILED")]
    Failed,
}

/// Persistent writeback action audit row (`ax_action_log`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionLogRecord {
    /// Action log `BIGINT` primary key.
    pub id: i64,
    /// Associated flow run UUID.
    pub run_id: Option<Uuid>,
    /// Associated flow node ID.
    pub node_id: Option<String>,
    /// Originating subsystem.
    pub source_kind: ActionSourceKind,
    /// Action kind.
    pub action_kind: ActionKind,
    /// Human/machine target descriptor (e.g., node UUID or URL).
    pub target: String,
    /// Action payload JSON.
    pub payload: serde_json::Value,
    /// Outcome status (`DONE`, `DRY_RUN`, `FAILED`).
    pub status: ActionLogStatus,
    /// Tenant-unique idempotency key (`<run_id>:<node_id>:<index>`).
    pub idempotency_key: String,
    /// Optional error message when `status == FAILED`.
    pub error_message: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Input for recording an `ax_action_log` entry idempotently on `(tenant_id, idempotency_key)`.
#[derive(Debug, Clone)]
pub struct NewActionLogInput {
    /// Optional flow run UUID.
    pub run_id: Option<Uuid>,
    /// Optional flow node ID.
    pub node_id: Option<String>,
    /// Originating subsystem.
    pub source_kind: ActionSourceKind,
    /// Action kind.
    pub action_kind: ActionKind,
    /// Target identifier.
    pub target: String,
    /// Action payload JSON.
    pub payload: serde_json::Value,
    /// Outcome status.
    pub status: ActionLogStatus,
    /// Tenant-unique idempotency key.
    pub idempotency_key: String,
    /// Optional failure message.
    pub error_message: Option<String>,
}

/// Query parameters for `GET /api/axflow/actions`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionListQuery {
    /// Optional flow run UUID filter.
    #[serde(default)]
    pub run_id: Option<Uuid>,
    /// Optional status filter.
    #[serde(default)]
    pub status: Option<ActionLogStatus>,
    /// Page size (`1..=500`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Response payload for `POST /api/axflow/flows/{id}/hook` (returns plaintext Bearer secret once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowHookSecretResponse {
    /// Hook record UUID.
    pub id: Uuid,
    /// Flow UUID.
    pub flow_id: Uuid,
    /// Flow slug used in `/api/axflow/hooks/{tenant_id}/{slug}`.
    pub slug: String,
    /// Relative webhook invocation path.
    pub path: String,
    /// Newly generated Bearer secret (returned once at creation/rotation).
    pub secret: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Cross-tenant hook lookup row returned by `SECURITY DEFINER` function `ax_resolve_flow_hook`.
#[derive(Debug, Clone)]
pub struct ResolvedFlowHook {
    /// Target flow UUID.
    pub flow_id: Uuid,
    /// Sealed `ds_credentials` UUID holding the Bearer secret.
    pub credential_id: Uuid,
}

/// Cross-tenant scheduled PROD flow row returned by `ax_scheduled_flows()`.
#[derive(Debug, Clone)]
pub struct ScheduledFlowItem {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Flow UUID.
    pub flow_id: Uuid,
    /// Deployed `PROD` version.
    pub version: i32,
    /// Trigger configuration JSON.
    pub trigger_cfg: TriggerCfg,
    /// Most recent `CRON`-triggered run `created_at`.
    pub last_run_at: Option<DateTime<Utc>>,
}

/// Cross-tenant event-driven PROD flow row returned by `ax_event_flows(topic)`.
#[derive(Debug, Clone)]
pub struct EventFlowItem {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Flow UUID.
    pub flow_id: Uuid,
    /// Deployed `PROD` version.
    pub version: i32,
    /// Trigger configuration JSON.
    pub trigger_cfg: TriggerCfg,
}

/// Public operator metadata and JSON Schema returned by `GET /api/axflow/operators`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorSpecDto {
    /// Operator enum discriminant.
    pub kind: FlowOperator,
    /// Human-readable display name.
    pub display_name: String,
    /// Short operator description.
    pub description: String,
    /// JSON Schema (`Draft 2020-12` / `Draft-07`) validating `FlowNode.config`.
    pub config_schema: serde_json::Value,
}

/// Principal identity propagated into [`OperatorContext`] during background step execution.
#[derive(Debug, Clone)]
pub struct FlowActorPrincipal {
    /// Initiating user UUID (`Uuid::nil()` for system/service triggers).
    pub user_id: Uuid,
    /// Display username.
    pub username: String,
    /// Department identifier.
    pub dept_id: String,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// Effective permission set.
    pub permissions: BTreeSet<Permission>,
}

impl FlowActorPrincipal {
    /// Constructs a service/system principal in `default` department with full operator permissions.
    #[must_use]
    pub fn service(user_id: Option<Uuid>) -> Self {
        Self {
            user_id: user_id.unwrap_or_else(Uuid::nil),
            username: String::from("axflow"),
            dept_id: String::from("default"),
            roles: vec![String::from("ADMIN")],
            permissions: Permission::all_set(),
        }
    }
}

/// Execution context passed to [`Operator::execute`] (`07-STEP07-08` §07.3.2).
#[derive(Debug, Clone)]
pub struct OperatorContext {
    /// Isolated tenant identifier.
    pub tenant_id: String,
    /// Owning flow UUID.
    pub flow_id: Uuid,
    /// Pinned flow version number.
    pub flow_version: i32,
    /// Active flow run UUID.
    pub run_id: Uuid,
    /// Current DAG node identifier.
    pub node_id: String,
    /// 1-based attempt number.
    pub attempt: i32,
    /// Execution environment (`TEST` or `PROD`).
    pub environment: Environment,
    /// Node-level idempotency key `<run_id>:<node_id>`.
    pub idempotency_key: String,
    /// Principal executing the run.
    pub principal: FlowActorPrincipal,
    /// Template-rendered operator `config` JSON.
    pub config: serde_json::Value,
    /// Cooperative cancellation flag polled by long-running operators at checkpoints.
    cancel_flag: Arc<AtomicBool>,
}

impl OperatorContext {
    /// Creates a new [`OperatorContext`] with a shared cancellation flag.
    #[must_use]
    pub fn new(
        tenant_id: String,
        flow_id: Uuid,
        flow_version: i32,
        run_id: Uuid,
        node_id: String,
        attempt: i32,
        environment: Environment,
    ) -> Self {
        let idempotency_key = format!("{run_id}:{node_id}");
        Self {
            tenant_id,
            flow_id,
            flow_version,
            run_id,
            node_id,
            attempt,
            environment,
            idempotency_key,
            principal: FlowActorPrincipal::service(None),
            config: default_empty_object(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Attaches a rendered `config`, [`FlowActorPrincipal`], and cooperative cancellation handle.
    #[must_use]
    pub fn with_execution_inputs(
        mut self,
        config: serde_json::Value,
        principal: FlowActorPrincipal,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        self.config = config;
        self.principal = principal;
        self.cancel_flag = cancel_flag;
        self
    }

    /// Returns `true` if `POST /runs/{id}/cancel` has marked this run canceled (`07-STEP07-08` §07.3.2).
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }
}

/// Result returned by [`Operator::execute`] (`07-STEP07-08` §07.3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperatorOutcome {
    /// Step completed synchronously with JSON output.
    Completed(serde_json::Value),
    /// Step created or reused `approval_id` and suspended the branch (`WAITING_APPROVAL`).
    Suspend {
        /// Created or reused `ax_approvals.id`.
        approval_id: Uuid,
    },
}

/// Object-safe `AxFlow` DAG operator trait (`07-STEP07-08` §07.4).
pub trait Operator: Send + Sync {
    /// Returns the canonical [`FlowOperator`] kind handled by this implementation.
    fn kind(&self) -> FlowOperator;

    /// Returns the human-readable operator title.
    fn display_name(&self) -> &'static str;

    /// Returns a concise operator description for the studio palette.
    fn description(&self) -> &'static str;

    /// Returns the JSON Schema validating `FlowNode.config`.
    fn config_schema(&self) -> serde_json::Value;

    /// Executes the operator for a single DAG node attempt.
    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome>;
}

/// Registry mapping [`FlowOperator`] discriminants to object-safe [`Operator`] implementations (`07-STEP07-08` §07.4).
#[derive(Clone, Default)]
pub struct OperatorRegistry {
    operators: HashMap<FlowOperator, Arc<dyn Operator>>,
}

impl std::fmt::Debug for OperatorRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut kinds: Vec<FlowOperator> = self.operators.keys().copied().collect();
        kinds.sort();
        f.debug_struct("OperatorRegistry")
            .field("registered_kinds", &kinds)
            .finish()
    }
}

impl OperatorRegistry {
    /// Creates an empty [`OperatorRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an [`Operator`] implementation under its [`Operator::kind`].
    pub fn register(&mut self, operator: Arc<dyn Operator>) {
        self.operators.insert(operator.kind(), operator);
    }

    /// Looks up a registered [`Operator`] by [`FlowOperator`].
    #[must_use]
    pub fn get(&self, kind: FlowOperator) -> Option<&Arc<dyn Operator>> {
        self.operators.get(&kind)
    }

    /// Returns `true` if `kind` is registered.
    #[must_use]
    pub fn contains(&self, kind: FlowOperator) -> bool {
        self.operators.contains_key(&kind)
    }

    /// Exports sorted [`OperatorSpecDto`] descriptors for `GET /api/axflow/operators`.
    #[must_use]
    pub fn specs(&self) -> Vec<OperatorSpecDto> {
        let mut out = Vec::with_capacity(self.operators.len());
        for kind in FlowOperator::ALL {
            if let Some(op) = self.operators.get(kind) {
                out.push(OperatorSpecDto {
                    kind: *kind,
                    display_name: String::from(op.display_name()),
                    description: String::from(op.description()),
                    config_schema: op.config_schema(),
                });
            }
        }
        out
    }
}

/// Step mutation applied before running [`RunProgressionPlan`] inside a locked `ax_flow_runs` transaction.
#[derive(Debug, Clone)]
pub enum StepMutation {
    /// Mark `(node_id, attempt)` as `SUCCEEDED` with inline `output` or offloaded `output_ref`.
    Succeeded {
        /// Node ID.
        node_id: String,
        /// 1-based attempt number.
        attempt: i32,
        /// Inline JSON output (`None` when `output_ref` is used).
        output: Option<serde_json::Value>,
        /// Relative file path under `AXPOINT_DATA_ROOT` when output exceeds `1 MiB`.
        output_ref: Option<String>,
    },
    /// Mark `(node_id, attempt)` as `FAILED` and enqueue `attempt + 1` at `next_run_at`.
    Retry {
        /// Node ID.
        node_id: String,
        /// Failed attempt number.
        attempt: i32,
        /// Error code.
        error_code: String,
        /// Error message.
        error_message: String,
        /// Scheduled timestamp for `attempt + 1`.
        next_run_at: DateTime<Utc>,
    },
    /// Mark `(node_id, attempt)` as terminal `FAILED` (all retries exhausted).
    TerminalFailed {
        /// Node ID.
        node_id: String,
        /// Final attempt number.
        attempt: i32,
        /// Error code.
        error_code: String,
        /// Error message.
        error_message: String,
    },
    /// Mark `(node_id, attempt)` as `WAITING_APPROVAL`.
    Suspended {
        /// Node ID.
        node_id: String,
        /// Current attempt number.
        attempt: i32,
    },
}

/// Deterministic state transition batch applied atomically under `SELECT ... FOR UPDATE` on `ax_flow_runs`.
#[derive(Debug, Clone, Default)]
pub struct RunProgressionPlan {
    /// Downstream node IDs whose `attempt = 1` row should be inserted as `SKIPPED`.
    pub nodes_to_skip: Vec<String>,
    /// Ready downstream `(node_id, attempt)` pairs to insert as `PENDING` and enqueue as `FLOW_STEP`.
    pub steps_to_enqueue: Vec<(String, i32)>,
    /// Updated run status (`RUNNING`, `WAITING_APPROVAL`, `SUCCEEDED`, `FAILED`, or `CANCELED`).
    pub next_run_status: Option<RunStatus>,
    /// Terminal leaf outputs map when `next_run_status == Some(RunStatus::Succeeded)`.
    pub run_outputs: Option<serde_json::Value>,
    /// Run-level error message when `next_run_status == Some(RunStatus::Failed)`.
    pub run_error_message: Option<String>,
}

/// Callback computing [`RunProgressionPlan`] from the locked [`RunRecord`], [`FlowVersionRecord`], and latest-attempt [`StepRecord`]s.
pub type ProgressionPlannerFn<'a> = &'a (
        dyn Fn(
    &RunRecord,
    &FlowVersionRecord,
    &[StepRecord],
) -> Result<RunProgressionPlan, AppError>
            + Send
            + Sync
    );

/// Persistence parameters passed to [`crate::ports::FlowRepository::start_flow_run`].
#[derive(Debug, Clone)]
pub struct StartRunParams {
    /// Target flow UUID.
    pub flow_id: Uuid,
    /// Target environment (`TEST` or `PROD`).
    pub environment: Environment,
    /// Optional explicit version override.
    pub explicit_version: Option<i32>,
    /// Trigger source discriminant.
    pub trigger_source: TriggerType,
    /// Trigger JSON payload.
    pub trigger_payload: serde_json::Value,
    /// Optional deduplication key scoped to `(flow_id, idempotency_key)`.
    pub idempotency_key: Option<String>,
    /// Optional initiating user UUID.
    pub started_by: Option<Uuid>,
}

/// Persistence parameters passed to [`crate::ports::FlowRepository::decide_approval`].
#[derive(Debug, Clone)]
pub struct DecideApprovalParams {
    /// Target `ax_approvals.id`.
    pub approval_id: Uuid,
    /// Deciding user UUID.
    pub actor_user_id: Uuid,
    /// Deciding user's assigned role names (checked against `assignee_roles`).
    pub actor_roles: Vec<String>,
    /// Decision (`APPROVE` or `REJECT`).
    pub decision: ApprovalDecision,
    /// Optional reviewer comment.
    pub comment: String,
}
