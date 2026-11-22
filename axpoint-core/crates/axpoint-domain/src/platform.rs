//! Foundation domain DTOs and object-safe worker traits for Step 00 (`ax_jobs`, `ax_outbox`, `sec_*`).

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::{AuditAction, JobKind, JobStatus, OutboxTopic, Permission, TokenKind};
pub use crate::ops::{
    InstallTemplateRequest, OnboardingRawCounts, OnboardingStatusResponse, OnboardingStepItem,
    OnboardingStepKey, TemplateAgentPreviewItem, TemplateDashboardPreviewItem,
    TemplateEdgeTypePreview, TemplateNodeTypePreview, TemplatePreviewResponse,
    TemplateSampleMetricPreview, TemplateSampleRowsPreview, TemplateTopologySummary,
    build_onboarding_status,
};

/// Boxed asynchronous result used by object-safe registries (`JobHandler`, `OutboxSubscriber`).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AppError>> + Send + 'a>>;

/// Payload to enqueue a new background job in `ax_jobs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewJob {
    /// Job category.
    pub kind: JobKind,
    /// Arbitrary JSON parameters consumed by the handler.
    pub payload: serde_json::Value,
    /// Scheduling priority (lower runs first; default `100`).
    pub priority: i32,
    /// Maximum execution attempts before transitioning to `FAILED` (default `5`).
    pub max_attempts: i32,
    /// Earliest execution timestamp (`None` defaults to `NOW()`).
    pub next_run_at: Option<DateTime<Utc>>,
    /// Optional per-`(tenant_id, kind)` deduplication key.
    pub idempotency_key: Option<String>,
}

impl NewJob {
    /// Constructs a [`NewJob`] with default priority (`100`) and max attempts (`5`).
    #[must_use]
    pub fn new(kind: JobKind, payload: serde_json::Value) -> Self {
        Self {
            kind,
            payload,
            priority: 100,
            max_attempts: 5,
            next_run_at: None,
            idempotency_key: None,
        }
    }

    /// Attaches an idempotency key to prevent duplicate pending/running jobs.
    #[must_use]
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }
}

/// A leased job row returned by `ax_lease_jobs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeasedJob {
    /// Job UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Job category.
    pub kind: JobKind,
    /// Job input payload.
    pub payload: serde_json::Value,
    /// Current status (`RUNNING`).
    pub status: JobStatus,
    /// Priority value.
    pub priority: i32,
    /// 1-based current attempt counter.
    pub attempt: i32,
    /// Maximum allowed attempts.
    pub max_attempts: i32,
    /// Scheduled run timestamp.
    pub next_run_at: DateTime<Utc>,
    /// Worker identifier holding the lease.
    pub lease_owner: String,
    /// Lease expiration timestamp.
    pub lease_until: DateTime<Utc>,
    /// Optional idempotency key.
    pub idempotency_key: Option<String>,
    /// Previous attempt error, if any.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Public API view of an `ax_jobs` record returned by `/api/axops/jobs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    /// Job UUID.
    pub id: Uuid,
    /// Job category.
    pub kind: JobKind,
    /// Current lifecycle status.
    pub status: JobStatus,
    /// Number of attempts started so far.
    pub attempt: i32,
    /// Maximum attempts configured.
    pub max_attempts: i32,
    /// Next scheduled execution timestamp.
    pub next_run_at: DateTime<Utc>,
    /// Error message from the most recent failed attempt.
    pub last_error: Option<String>,
    /// Handler output JSON when succeeded.
    pub result: Option<serde_json::Value>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Query filter for `GET /api/axops/jobs`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobListQuery {
    /// Optional filter by job kind.
    pub kind: Option<JobKind>,
    /// Optional filter by status.
    pub status: Option<JobStatus>,
    /// Page size (`1..=500`, default `50`).
    pub limit: Option<u32>,
    /// Opaque pagination cursor (UUID string or timestamp cursor).
    pub cursor: Option<String>,
}

/// Paginated response envelope for `GET /api/axops/jobs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobListResponse {
    /// Page items.
    pub items: Vec<JobRecord>,
    /// Cursor for the next page, if more items exist.
    pub next_cursor: Option<String>,
}

/// Event to insert into `ax_outbox` inside an active business transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewOutboxEvent {
    /// Event topic.
    pub topic: OutboxTopic,
    /// Event JSON payload.
    pub payload: serde_json::Value,
}

/// Claimed transactional outbox event returned by `ax_claim_outbox`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEvent {
    /// Monotonic event sequence ID.
    pub id: i64,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Event topic.
    pub topic: OutboxTopic,
    /// Event JSON payload.
    pub payload: serde_json::Value,
    /// Worker identifier holding the claim.
    pub claimed_by: Option<String>,
    /// Claim expiration timestamp.
    pub claimed_until: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Security audit record persisted into `sec_audit_logs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Optional user UUID (`None` for unauthenticated failures).
    pub user_id: Option<Uuid>,
    /// Username or anonymous subject marker.
    pub username: String,
    /// Audited action enum.
    pub action: AuditAction,
    /// Target resource reference (e.g., `dataset:uuid`).
    pub resource_ref: String,
    /// HTTP status code returned.
    pub status_code: i32,
    /// Client IP address string.
    pub client_ip: String,
    /// Distributed trace / request identifier.
    pub trace_id: String,
    /// Structured metadata details.
    pub detail_json: serde_json::Value,
    /// Event timestamp.
    pub created_at: DateTime<Utc>,
}

/// Resolved personal access token metadata from `ax_resolve_api_token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTokenRecord {
    /// Token row UUID.
    pub token_id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Owning user UUID.
    pub user_id: Uuid,
    /// Optional permission scope restriction (empty inherits user permissions).
    pub scopes: BTreeSet<Permission>,
    /// Token capability profile (`GENERAL` or `BUILDER`).
    #[serde(default)]
    pub token_profile: crate::enums::TokenProfile,
    /// Optional frontend base URL bound to the token for deep-link generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_base_url: Option<String>,
}

/// User identity and role union loaded from `sec_users` + `sec_roles`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipalRecord {
    /// User UUID.
    pub user_id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Login username.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Department identifier.
    pub dept_id: String,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// Union of permissions granted by assigned roles.
    pub permissions: BTreeSet<Permission>,
    /// User clearance classification (`sec_users.clearance`, default `INTERNAL`).
    #[serde(default = "default_clearance")]
    pub clearance: crate::enums::Sensitivity,
    /// Minimum valid `iat` cutoff (`sec_users.tokens_valid_after`, default Unix epoch).
    #[serde(default)]
    pub tokens_valid_after: DateTime<Utc>,
}

const fn default_clearance() -> crate::enums::Sensitivity {
    crate::enums::Sensitivity::Internal
}

/// Response payload for `GET /api/auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeResponse {
    /// User UUID.
    pub user_id: Uuid,
    /// Active tenant identifier.
    pub tenant_id: String,
    /// Username.
    pub username: String,
    /// Department identifier.
    pub dept_id: String,
    /// Role names.
    pub roles: Vec<String>,
    /// Effective permissions.
    pub permissions: BTreeSet<Permission>,
    /// Credential type used on this request.
    pub token_kind: TokenKind,
    /// Personal access token profile (`GENERAL` or `BUILDER`).
    #[serde(default)]
    pub token_profile: crate::enums::TokenProfile,
}

/// Object-safe handler registered in `JobHandlerRegistry` for a specific [`JobKind`].
pub trait JobHandler: Send + Sync {
    /// Executes the leased job and returns its JSON result value.
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value>;
}

/// Object-safe subscriber notified by `OutboxDispatcher` when outbox events arrive.
pub trait OutboxSubscriber: Send + Sync {
    /// Processes a single outbox event idempotently.
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()>;
}
