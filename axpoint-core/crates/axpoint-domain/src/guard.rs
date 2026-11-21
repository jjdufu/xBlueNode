//! `AxGuard` complete security governance domain models, auth/session/OIDC DTOs,
//! Cedar ABAC & row/column policy specs, cross-department grants, PII spans, and
//! tamper-evident audit chain contracts (`11-STEP13-AXGUARD-SECURITY-GOVERNANCE`).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{AuditAction, MaskRule, Permission, PiiCategory, Sensitivity};

impl Sensitivity {
    /// Numeric lattice level used in Cedar ABAC (`PUBLIC=0`, `INTERNAL=1`, `CONFIDENTIAL=2`, `SECRET=3`).
    #[must_use]
    pub const fn level(self) -> i64 {
        match self {
            Self::Public => 0,
            Self::Internal => 1,
            Self::Confidential => 2,
            Self::Secret => 3,
        }
    }
}

impl MaskRule {
    /// Default masking rule inferred when a column/property has `pii_category != NONE` and no explicit override (`§13.7`).
    #[must_use]
    pub const fn from_pii_category(category: PiiCategory) -> Self {
        match category {
            PiiCategory::None => Self::None,
            PiiCategory::Phone => Self::MaskPhone,
            PiiCategory::IdCard => Self::MaskIdCard,
            PiiCategory::Email => Self::MaskEmail,
            PiiCategory::BankCard => Self::MaskBankCard,
            PiiCategory::Name => Self::MaskName,
            PiiCategory::Address => Self::MaskAddress,
            PiiCategory::Secret => Self::Redact,
        }
    }

    /// Returns the registered `DuckDB` vectorized scalar UDF name (`§13.7`), or `None` for [`MaskRule::None`].
    #[must_use]
    pub const fn duckdb_udf_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::MaskPhone => Some("ax_mask_phone"),
            Self::MaskIdCard => Some("ax_mask_id_card"),
            Self::MaskEmail => Some("ax_mask_email"),
            Self::MaskBankCard => Some("ax_mask_bank_card"),
            Self::MaskName => Some("ax_mask_name"),
            Self::MaskAddress => Some("ax_mask_address"),
            Self::HmacHash => Some("ax_mask_hmac_hash"),
            Self::Redact => Some("ax_mask_redact"),
        }
    }
}

/// Account authentication source (`sec_users.auth_source`, `sec_sessions.auth_source`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthSource {
    /// Local username + `argon2id` password account.
    #[default]
    Local,
    /// Federated `OpenID` Connect account.
    Oidc,
}

impl AuthSource {
    /// Canonical uppercase SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Oidc => "OIDC",
        }
    }
}

/// Account lifecycle status (`sec_users.status`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserAccountStatus {
    /// Account permitted to authenticate and execute requests.
    #[default]
    Active,
    /// Account disabled by `GUARD_ADMIN`.
    Disabled,
}

impl UserAccountStatus {
    /// Canonical uppercase SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Disabled => "DISABLED",
        }
    }
}

/// Target category for row-level and column-level data policies (`§13.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PolicyTargetKind {
    /// `AxHub` dataset (`target_ref` = dataset UUID string).
    Dataset,
    /// `xBlueStudio` business node type (`target_ref` = `type_name`).
    NodeType,
}

impl PolicyTargetKind {
    /// Canonical uppercase SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dataset => "DATASET",
            Self::NodeType => "NODE_TYPE",
        }
    }
}

/// Governed resource category for Cedar ABAC and cross-department `sec_grants` (`§13.1`, `§13.6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GrantResourceKind {
    /// `ds_datasets` (`resource_ref` = dataset UUID).
    Dataset,
    /// `xb_node_types` (`resource_ref` = `type_name`).
    NodeType,
    /// `xb_subgraphs` (`resource_ref` = subgraph `slug`).
    Subgraph,
    /// `xv_dashboards` (`resource_ref` = dashboard UUID).
    Dashboard,
    /// `ax_agents` (`resource_ref` = agent UUID).
    Agent,
}

impl GrantResourceKind {
    /// Canonical uppercase SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dataset => "DATASET",
            Self::NodeType => "NODE_TYPE",
            Self::Subgraph => "SUBGRAPH",
            Self::Dashboard => "DASHBOARD",
            Self::Agent => "AGENT",
        }
    }
}

/// Subject selector category for `sec_grants.grantee_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GranteeKind {
    /// Entire department (`grantee_ref` = `dept_id`).
    Dept,
    /// Role name (`grantee_ref` = `role_name`).
    Role,
    /// Individual user (`grantee_ref` = `user_id` UUID string).
    User,
}

impl GranteeKind {
    /// Canonical uppercase SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dept => "DEPT",
            Self::Role => "ROLE",
            Self::User => "USER",
        }
    }
}

/// Lifecycle status of a cross-department grant (`sec_grants.status`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GrantStatus {
    /// Waiting for owner-department / `GUARD_ADMIN` approval.
    #[default]
    Pending,
    /// Approved and currently active.
    Active,
    /// Rejected during approval.
    Rejected,
    /// Explicitly revoked before expiration.
    Revoked,
    /// Automatically expired by `RETENTION` worker or `expires_at <= NOW()`.
    Expired,
}

impl GrantStatus {
    /// Canonical uppercase SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Active => "ACTIVE",
            Self::Rejected => "REJECTED",
            Self::Revoked => "REVOKED",
            Self::Expired => "EXPIRED",
        }
    }
}

/// Subject matching filter stored in `sec_row_policies.applies_to` and `sec_column_policies.exempt`.
///
/// For `applies_to`, an empty selector matches **all** non-exempt subjects (`§13.1`).
/// For `exempt`, an empty selector exempts **no** subjects (`§13.7`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectSelector {
    /// Matching role names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// Matching department IDs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depts: Vec<String>,
    /// Matching user UUIDs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<Uuid>,
}

impl SubjectSelector {
    /// Returns `true` when no roles, departments, or users are specified.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty() && self.depts.is_empty() && self.users.is_empty()
    }

    /// Evaluates whether a subject `(user_id, dept_lineage, roles)` matches when empty means "match all" (`applies_to`).
    #[must_use]
    pub fn matches_applies_to(
        &self,
        user_id: Uuid,
        dept_lineage: &[String],
        roles: &[String],
    ) -> bool {
        if self.is_empty() {
            return true;
        }
        self.matches_explicit(user_id, dept_lineage, roles)
    }

    /// Evaluates whether a subject explicitly matches at least one listed user, department, or role (`exempt`).
    #[must_use]
    pub fn matches_explicit(
        &self,
        user_id: Uuid,
        dept_lineage: &[String],
        roles: &[String],
    ) -> bool {
        self.users.contains(&user_id)
            || self.depts.iter().any(|d| dept_lineage.contains(d))
            || self.roles.iter().any(|r| roles.contains(r))
    }
}

/// Resource metadata reference passed into `CedarAuthorizer::authorize` (`§13.6.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    /// Resource category (`DATASET`, `NODE_TYPE`, `SUBGRAPH`, `DASHBOARD`, `AGENT`).
    pub kind: GrantResourceKind,
    /// Resource identifier string (UUID, slug, or node `type_name`).
    pub id: String,
    /// Owning department ID (`default` when unset).
    pub owner_dept: String,
    /// Resource classification level.
    pub sensitivity: Sensitivity,
}

/// Enriched principal profile loaded for JWT validation, Cedar ABAC, and row/column policy compilation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardPrincipalProfile {
    /// User UUID.
    pub user_id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Username.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Email address.
    pub email: String,
    /// Primary department ID.
    pub dept_id: String,
    /// Account clearance level.
    pub clearance: Sensitivity,
    /// Account status (`ACTIVE` / `DISABLED`).
    pub status: UserAccountStatus,
    /// Timestamp invalidating any JWT issued strictly before `tokens_valid_after`.
    pub tokens_valid_after: DateTime<Utc>,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// Union of role permissions.
    pub permissions: BTreeSet<Permission>,
    /// User attribute map (`attr_key -> attr_values`) from `sec_user_attributes`.
    pub attributes: BTreeMap<String, Vec<String>>,
}

/// Pre-Lookup row returned by `SECURITY DEFINER` function `ax_lookup_login(tenant, username)` (`§13.1`).
#[derive(Debug, Clone)]
pub struct LoginLookupRow {
    /// User UUID.
    pub user_id: Uuid,
    /// Stored `argon2id` PHC string (if password set).
    pub password_hash: Option<String>,
    /// Account status (`ACTIVE` or `DISABLED`).
    pub status: String,
    /// Optional lockout deadline (`locked_until > NOW()`).
    pub locked_until: Option<DateTime<Utc>>,
    /// Consecutive failed login count.
    pub failed_logins: i32,
}

/// Session row returned by `SECURITY DEFINER` function `ax_resolve_session(refresh_hash)` (`§13.1`).
#[derive(Debug, Clone)]
pub struct ResolvedSessionRow {
    /// Session UUID.
    pub session_id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Owning user UUID.
    pub user_id: Uuid,
    /// Refresh rotation family UUID.
    pub family_id: Uuid,
    /// Refresh token expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Timestamp when this session row was rotated to a successor (`Some` => replay!).
    pub rotated_at: Option<DateTime<Utc>>,
    /// Timestamp when this session row was revoked (`Some` => replay/revoked!).
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Public configuration returned by `GET /api/auth/config` (`§13.10`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfigResponse {
    /// Runtime environment (`development`, `test`, `production`).
    pub env: String,
    /// Whether federated OIDC login is configured and enabled.
    pub oidc_enabled: bool,
    /// Optional OIDC provider display label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oidc_display_name: Option<String>,
    /// Default tenant identifier (`"default"`).
    pub default_tenant: String,
}

/// Request body for `POST /api/auth/login` (`§13.2`, `§13.3`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// Tenant identifier (defaults to `"default"`).
    #[serde(default = "default_tenant_id")]
    pub tenant: String,
    /// Account username.
    pub username: String,
    /// Plaintext password.
    pub password: String,
}

fn default_tenant_id() -> String {
    String::from("default")
}

/// Response body returned by `POST /api/auth/login` and `POST /api/auth/refresh` (`§13.4`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenResponse {
    /// Signed `EdDSA` access JWT (`900s` TTL).
    pub access_token: String,
    /// Token type (`"Bearer"`).
    pub token_type: String,
    /// Lifetime in seconds (`900`).
    pub expires_in: i64,
}

/// Request body for `POST /api/auth/password` (`§13.3`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePasswordRequest {
    /// Current plaintext password.
    pub old_password: String,
    /// New plaintext password (`>= 12` chars, must not contain username).
    pub new_password: String,
}

/// Stored OIDC PKCE state row (`sec_login_states`, `§13.5`).
#[derive(Debug, Clone)]
pub struct OidcLoginState {
    /// Random `state` parameter.
    pub state: String,
    /// Target tenant identifier.
    pub tenant_id: String,
    /// Random `nonce` parameter.
    pub nonce: String,
    /// PKCE `S256` code verifier.
    pub pkce_verifier: String,
    /// Validated internal redirect path starting with `'/'`.
    pub return_to: String,
    /// State expiration timestamp (10 minutes).
    pub expires_at: DateTime<Utc>,
}

/// Parameters for provisioning or syncing an `OIDC` user (`§13.5`).
#[derive(Debug, Clone)]
pub struct UpsertOidcUserParams {
    /// Subject claim (`sub`).
    pub oidc_subject: String,
    /// Username claim (`preferred_username` fallback).
    pub username_hint: String,
    /// Email claim.
    pub email: String,
    /// Mapped department ID (if `dept_claim` matched).
    pub mapped_dept: Option<String>,
    /// Mapped role names (from `groups_claim` map).
    pub mapped_roles: Vec<String>,
    /// Whether `AXPOINT_OIDC_AUTO_PROVISION` allows creating new users.
    pub auto_provision: bool,
}

/// Department hierarchy record (`sec_departments`, `§13.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepartmentRecord {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Department slug (`^[a-z0-9_-]{1,64}$`).
    pub dept_id: String,
    /// Human-readable department name.
    pub name: String,
    /// Optional parent department slug.
    pub parent_id: Option<String>,
}

/// Request body for `PUT /api/axguard/departments/{dept_id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertDepartmentRequest {
    /// Department display name.
    pub name: String,
    /// Optional parent department slug (`None` for root department).
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// User management view returned by `/api/axguard/users` (`§13.10`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardUserRecord {
    /// User UUID.
    pub id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Username.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Email address.
    pub email: String,
    /// Department identifier.
    pub dept_id: String,
    /// Clearance level.
    pub clearance: Sensitivity,
    /// Authentication source (`LOCAL` / `OIDC`).
    pub auth_source: AuthSource,
    /// Account status (`ACTIVE` / `DISABLED`).
    pub status: UserAccountStatus,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// User attributes (`attr_key -> attr_values`).
    pub attributes: BTreeMap<String, Vec<String>>,
    /// Failed consecutive login attempts.
    pub failed_logins: i32,
    /// Lockout expiration timestamp if currently locked.
    pub locked_until: Option<DateTime<Utc>>,
    /// Last successful login timestamp.
    pub last_login_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request body for `POST /api/axguard/users`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGuardUserRequest {
    /// Unique username within tenant.
    pub username: String,
    /// Optional display name (defaults to `username`).
    #[serde(default)]
    pub display_name: Option<String>,
    /// Optional email.
    #[serde(default)]
    pub email: String,
    /// Initial plaintext password (`>= 12` chars).
    pub password: String,
    /// Primary department ID (defaults to `"default"`).
    #[serde(default = "default_tenant_id")]
    pub dept_id: String,
    /// Security clearance level (defaults to `INTERNAL`).
    #[serde(default = "default_sensitivity")]
    pub clearance: Sensitivity,
    /// Initial role names to assign.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Optional initial user attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, Vec<String>>,
}

const fn default_sensitivity() -> Sensitivity {
    Sensitivity::Internal
}

/// Request body for `PATCH /api/axguard/users/{id}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchGuardUserRequest {
    /// Updated display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Updated email address.
    #[serde(default)]
    pub email: Option<String>,
    /// Updated department ID.
    #[serde(default)]
    pub dept_id: Option<String>,
    /// Updated clearance level.
    #[serde(default)]
    pub clearance: Option<Sensitivity>,
    /// Updated account status (`ACTIVE` or `DISABLED`).
    #[serde(default)]
    pub status: Option<UserAccountStatus>,
    /// Replacement role names list.
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    /// Clear active login lockout when `true`.
    #[serde(default)]
    pub unlock: bool,
}

/// Request body for `POST /api/axguard/users/{id}/reset-password`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetUserPasswordRequest {
    /// New plaintext password (`>= 12` chars).
    pub new_password: String,
}

/// Request body for `PUT /api/axguard/users/{id}/attributes`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutUserAttributesRequest {
    /// Complete replacement attribute map (`attr_key -> values`).
    pub attributes: BTreeMap<String, Vec<String>>,
}

/// Role record (`sec_roles`, `§13.10`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardRoleRecord {
    /// Role UUID.
    pub id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Unique role name.
    pub name: String,
    /// Granted permissions.
    pub permissions: Vec<Permission>,
    /// `true` for built-in platform roles (`ADMIN`, `ANALYST`, `OPERATOR`, `VIEWER`).
    pub is_builtin: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request body for `POST /api/axguard/roles`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGuardRoleRequest {
    /// Unique role name.
    pub name: String,
    /// Granted permissions.
    pub permissions: Vec<Permission>,
}

/// Request body for `PATCH /api/axguard/roles/{id}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchGuardRoleRequest {
    /// Optional renamed role name (disallowed on built-in roles).
    #[serde(default)]
    pub name: Option<String>,
    /// Replacement permission list.
    #[serde(default)]
    pub permissions: Option<Vec<Permission>>,
}

/// Stored custom Cedar policy (`sec_policies`, `§13.1`, `§13.6.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CedarPolicyRecord {
    /// Policy UUID.
    pub id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Unique policy name within tenant.
    pub name: String,
    /// Human-readable policy description.
    pub description: String,
    /// Validated Cedar policy text.
    pub policy_text: String,
    /// Whether this policy is evaluated during authorization.
    pub enabled: bool,
    /// Monotonic revision number.
    pub version: i32,
    /// Last editor user UUID.
    pub updated_by: Option<Uuid>,
    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Request body for `PUT /api/axguard/policies/{name}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutCedarPolicyRequest {
    /// Cedar policy source text.
    pub policy_text: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: String,
    /// Whether the policy is active (defaults to `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

/// Request body for `POST /api/axguard/policies/validate`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateCedarPolicyRequest {
    /// Candidate Cedar policy source text.
    pub policy_text: String,
}

/// Response body for `POST /api/axguard/policies/validate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateCedarPolicyResponse {
    /// `true` when syntax and `AxPoint` schema validation pass.
    pub valid: bool,
    /// Validation error diagnostics (empty when `valid == true`).
    pub errors: Vec<String>,
}

/// Request body for `POST /api/axguard/policies/simulate` (`§13.10`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulateCedarPolicyRequest {
    /// Simulated user UUID in current tenant.
    pub user_id: Uuid,
    /// Simulated action permission.
    pub action: Permission,
    /// Target resource category.
    pub resource_kind: GrantResourceKind,
    /// Target resource identifier.
    pub resource_ref: String,
    /// Optional simulated resource owner department override.
    #[serde(default)]
    pub owner_dept: Option<String>,
    /// Optional simulated resource sensitivity override.
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
    /// Optional simulated hour of day (`0..=23`, defaults to current UTC hour).
    #[serde(default)]
    pub hour: Option<u8>,
    /// Optional simulated client IP (`"127.0.0.1"` default).
    #[serde(default)]
    pub client_ip: Option<String>,
}

/// Response body for `POST /api/axguard/policies/simulate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulateCedarPolicyResponse {
    /// `"ALLOW"` or `"DENY"`.
    pub decision: String,
    /// `true` when permitted.
    pub allowed: bool,
    /// Policy IDs contributing to the decision (`permit` or `forbid` hits).
    pub determining_policies: Vec<String>,
    /// Evaluation diagnostics / errors if any.
    pub diagnostics: Vec<String>,
}

/// Stored row-level security policy (`sec_row_policies`, `§13.1`, `§13.6.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowPolicyRecord {
    /// Policy UUID.
    pub id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Unique policy name.
    pub name: String,
    /// Target kind (`DATASET` or `NODE_TYPE`).
    pub target_kind: PolicyTargetKind,
    /// Dataset UUID string or node `type_name`.
    pub target_ref: String,
    /// Subject selector (`{}` = all subjects).
    pub applies_to: SubjectSelector,
    /// Validated row predicate expression (`§13.6.2`).
    pub predicate: String,
    /// Whether policy is active.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request body for `POST /api/axguard/row-policies` and `PUT /api/axguard/row-policies/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRowPolicyRequest {
    /// Unique policy name.
    pub name: String,
    /// Target kind (`DATASET` or `NODE_TYPE`).
    pub target_kind: PolicyTargetKind,
    /// Dataset UUID string or node `type_name`.
    pub target_ref: String,
    /// Subject selector (`{}` = all subjects).
    #[serde(default)]
    pub applies_to: SubjectSelector,
    /// Row predicate expression (`§13.6.2`).
    pub predicate: String,
    /// Whether policy is active (defaults to `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Stored default-deny toggle (`sec_row_policy_defaults`, `§13.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowPolicyDefaultRecord {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Target kind (`DATASET` or `NODE_TYPE`).
    pub target_kind: PolicyTargetKind,
    /// Target identifier.
    pub target_ref: String,
    /// When `true`, subjects matching zero row policies see zero rows (`FALSE`).
    pub default_deny: bool,
}

/// Request body for `PUT /api/axguard/row-policy-defaults`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutRowPolicyDefaultRequest {
    /// Target kind (`DATASET` or `NODE_TYPE`).
    pub target_kind: PolicyTargetKind,
    /// Target identifier.
    pub target_ref: String,
    /// Default-deny flag.
    pub default_deny: bool,
}

/// Request body for `POST /api/axguard/row-policies/preview` (`§13.10`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowPolicyPreviewRequest {
    /// Simulated user UUID whose row & column policies will be applied.
    pub user_id: Uuid,
    /// Target dataset UUID to preview (top 20 rows).
    pub dataset_id: Uuid,
}

/// Stored column/property masking policy (`sec_column_policies`, `§13.1`, `§13.7`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnPolicyRecord {
    /// Policy UUID.
    pub id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Target kind (`DATASET` or `NODE_TYPE`).
    pub target_kind: PolicyTargetKind,
    /// Dataset UUID string or node `type_name`.
    pub target_ref: String,
    /// Column name or node property name.
    pub column_name: String,
    /// Masking rule (`NONE` explicitly disables default PII profile masking).
    pub mask_rule: MaskRule,
    /// Subjects eligible to view unmasked plaintext when `reveal_pii: true` is passed.
    pub exempt: SubjectSelector,
}

/// Request body for `PUT /api/axguard/column-policies`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutColumnPolicyRequest {
    /// Target kind (`DATASET` or `NODE_TYPE`).
    pub target_kind: PolicyTargetKind,
    /// Dataset UUID string or node `type_name`.
    pub target_ref: String,
    /// Column name or node property name.
    pub column_name: String,
    /// Masking rule (`NONE`, `MASK_PHONE`, ..., `HMAC_HASH`, `REDACT`).
    pub mask_rule: MaskRule,
    /// Exempt subject selector.
    #[serde(default)]
    pub exempt: SubjectSelector,
}

/// Cross-department access grant (`sec_grants`, `§13.1`, `§13.9.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRecord {
    /// Grant UUID.
    pub id: Uuid,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Target resource category.
    pub resource_kind: GrantResourceKind,
    /// Target resource reference.
    pub resource_ref: String,
    /// Grantee subject category (`DEPT`, `ROLE`, `USER`).
    pub grantee_kind: GranteeKind,
    /// Grantee identifier (`dept_id`, `role_name`, or `user_id` string).
    pub grantee_ref: String,
    /// Permitted actions (e.g. `["DATA_READ"]`).
    pub actions: Vec<Permission>,
    /// Optional additional row filter predicate (`§13.6.2`).
    pub row_predicate: Option<String>,
    /// Business justification provided by requester.
    pub reason: String,
    /// Current lifecycle status.
    pub status: GrantStatus,
    /// Associated `ax_approvals.id` gate UUID.
    pub approval_id: Option<Uuid>,
    /// Requester user UUID.
    pub requested_by: Uuid,
    /// Grant expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Approval/rejection decision timestamp.
    pub decided_at: Option<DateTime<Utc>>,
}

/// Request body for `POST /api/axguard/grants` (`§13.9.1`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGrantRequest {
    /// Target resource category.
    pub resource_kind: GrantResourceKind,
    /// Target resource identifier.
    pub resource_ref: String,
    /// Grantee subject category.
    pub grantee_kind: GranteeKind,
    /// Grantee reference.
    pub grantee_ref: String,
    /// Requested actions (defaults to `[Permission::DataRead]`).
    #[serde(default = "default_grant_actions")]
    pub actions: Vec<Permission>,
    /// Optional row-level restriction predicate.
    #[serde(default)]
    pub row_predicate: Option<String>,
    /// Business justification.
    pub reason: String,
    /// Validity window in days (`1..=365`).
    pub expires_in_days: u32,
}

fn default_grant_actions() -> Vec<Permission> {
    vec![Permission::DataRead]
}

/// Active/historical refresh session summary (`GET /api/axguard/sessions`, `§13.10`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryRecord {
    /// Session row UUID.
    pub id: Uuid,
    /// Rotation family UUID.
    pub family_id: Uuid,
    /// User UUID.
    pub user_id: Uuid,
    /// Username.
    pub username: String,
    /// Authentication source (`LOCAL` / `OIDC`).
    pub auth_source: AuthSource,
    /// User-Agent string.
    pub user_agent: String,
    /// Client IP string.
    pub client_ip: String,
    /// Expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Rotation timestamp if superseded.
    pub rotated_at: Option<DateTime<Utc>>,
    /// Revocation timestamp if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Stored audit row with tamper-evident hash chain columns (`GET /api/axguard/audit`, `§13.9.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Monotonic audit log ID (`BIGSERIAL`).
    pub id: i64,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Optional user UUID.
    pub user_id: Option<Uuid>,
    /// Username or anonymous token label.
    pub username: String,
    /// Audited action.
    pub action: AuditAction,
    /// Target resource reference.
    pub resource_ref: String,
    /// HTTP status code.
    pub status_code: i32,
    /// Client IP address.
    pub client_ip: String,
    /// Distributed trace ID.
    pub trace_id: String,
    /// Structured audit detail JSON.
    pub detail_json: serde_json::Value,
    /// Previous row hash in tenant chain (`""` for genesis row).
    pub prev_hash: String,
    /// SHA-256 chain hash of this row.
    pub row_hash: String,
    /// Event creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Query filter for `GET /api/axguard/audit` and `GET /api/axguard/audit/export.csv` (`§13.9.2`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditQueryFilter {
    /// Optional filter by action.
    #[serde(default)]
    pub action: Option<AuditAction>,
    /// Optional filter by username substring/exact match.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional filter by `resource_ref` prefix/substring.
    #[serde(default)]
    pub resource: Option<String>,
    /// Optional inclusive lower timestamp bound.
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    /// Optional inclusive upper timestamp bound.
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    /// Optional HTTP status code filter.
    #[serde(default)]
    pub status_code: Option<i32>,
    /// Page size (`1..=500`, default `100`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Pagination cursor (`id < cursor`).
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Query parameters for `GET /api/axguard/audit/verify` (`§13.9.2`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditVerifyQuery {
    /// Optional starting `sec_audit_logs.id`.
    #[serde(default)]
    pub from_id: Option<i64>,
    /// Optional ending `sec_audit_logs.id`.
    #[serde(default)]
    pub to_id: Option<i64>,
}

/// Response body returned by `GET /api/axguard/audit/verify` (`§13.9.2`, `step13.sh:532-538`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditVerifyResponse {
    /// `true` when every verified row matches `prev_hash` and recomputed `row_hash`.
    pub ok: bool,
    /// Number of audit rows verified.
    pub checked_count: u64,
    /// First `sec_audit_logs.id` where tampering or chain breakage was detected (`None` when `ok == true`).
    pub first_broken_id: Option<i64>,
}

/// Aggregated audit metrics returned by `GET /api/axguard/audit/stats` (`§13.9.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditStatsResponse {
    /// Counts grouped by `action`.
    pub by_action: BTreeMap<String, i64>,
    /// Top active usernames (`(username, count)`).
    pub by_user: Vec<(String, i64)>,
    /// Daily volume series (`(yyyy-mm-dd, count)`).
    pub by_day: Vec<(String, i64)>,
}

/// Detected PII character span stored in `hg_paras.pii_spans` and returned by `/pii/analyze` (`§13.8.1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PiiSpan {
    /// Detected PII category.
    pub category: PiiCategory,
    /// Character start offset (0-indexed Unicode scalar offset).
    pub start: usize,
    /// Character end offset (exclusive Unicode scalar offset).
    pub end: usize,
    /// Detection confidence score in `[0.0, 1.0]`.
    pub score: f64,
}

/// Request payload for `Sidecar` `POST /pii/analyze` (`§13.8.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiAnalyzeRequest {
    /// Input text batch to inspect.
    pub texts: Vec<String>,
    /// ISO language code (`"zh"` default).
    #[serde(default = "default_pii_lang")]
    pub language: String,
    /// Minimum confidence threshold (`0.6` default).
    #[serde(default = "default_score_threshold")]
    pub score_threshold: f64,
}

fn default_pii_lang() -> String {
    String::from("zh")
}

const fn default_score_threshold() -> f64 {
    0.6
}

/// Response payload from `Sidecar` `POST /pii/analyze`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiAnalyzeResponse {
    /// Per-text detected spans parallel to `req.texts`.
    pub results: Vec<Vec<PiiSpan>>,
}

/// Request payload for `Sidecar` `POST /pii/anonymize` (`§13.8.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiAnonymizeRequest {
    /// Input text batch to mask.
    pub texts: Vec<String>,
    /// Explicit category -> `MaskRule` overrides.
    #[serde(default)]
    pub rules: BTreeMap<PiiCategory, MaskRule>,
    /// ISO language code (`"zh"` default).
    #[serde(default = "default_pii_lang")]
    pub language: String,
    /// Minimum confidence threshold (`0.6` default).
    #[serde(default = "default_score_threshold")]
    pub score_threshold: f64,
}

/// Response payload from `Sidecar` `POST /pii/anonymize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiAnonymizeResponse {
    /// Masked texts parallel to `req.texts`.
    pub texts: Vec<String>,
}

/// Compiled per-dataset row and column policy instructions injected into `sql_guard` (`§13.6.3`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetPolicySpec {
    /// Pre-compiled `DuckDB` SQL boolean predicate (`None` = unrestricted; `Some("FALSE")` = default-deny).
    pub duckdb_row_filter: Option<String>,
    /// Effective column masking map (`lowercase_column_name -> MaskRule`) after resolving profile defaults,
    /// explicit `sec_column_policies`, `exempt` selectors, and `reveal_pii`.
    pub column_masks: BTreeMap<String, MaskRule>,
}

/// Compiled per-node-type row and property policy instructions for `ax.nodes` / `xb_nodes` (`§13.6.3`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeTypePolicySpec {
    /// Pre-compiled `DuckDB` SQL boolean predicate over `ax.nodes` columns (`None` = unrestricted; `Some("FALSE")` = deny).
    pub duckdb_row_filter: Option<String>,
    /// Pre-compiled `PostgreSQL` SQL boolean predicate over `xb_nodes` (`None` = unrestricted; `Some("FALSE")` = deny).
    pub pg_row_filter: Option<String>,
    /// Effective property masking map (`property_name -> MaskRule`).
    pub property_masks: BTreeMap<String, MaskRule>,
}

/// Compiled security policy context threaded into `sql_guard::GuardOptions` and node query ports (`§13.6.3`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyContext {
    /// `true` when caller holds `DATA_ADMIN` or `GUARD_ADMIN` (row policies bypassed).
    pub bypass_row_policies: bool,
    /// `true` when caller explicitly requested `reveal_pii: true` AND was verified eligible.
    pub reveal_pii_authorized: bool,
    /// Dataset-keyed policy specifications (`dataset_id` -> spec) AND (`lowercase_dataset_name` -> spec).
    pub datasets: BTreeMap<String, DatasetPolicySpec>,
    /// Node-type-keyed policy specifications (`type_name` -> spec).
    pub node_types: BTreeMap<String, NodeTypePolicySpec>,
    /// Explicit set of dataset names/IDs denied by Cedar ABAC (`403 POLICY_DENIED` when referenced in SQL!).
    pub denied_datasets: BTreeSet<String>,
}

/// Request body for `POST /api/axguard/debug/llm-egress-preview` (`11-STEP13` §13.10, `step13.sh:62`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmEgressPreviewRequest {
    /// Sample user prompt text containing potential PII.
    pub text: String,
}

/// Response body returned by `POST /api/axguard/debug/llm-egress-preview` (`11-STEP13` §13.8.3, `§13.10`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEgressPreviewResponse {
    /// Outbound prompt after replacing detected PII spans with `⟨CATEGORY_N⟩` placeholders.
    pub outbound: String,
    /// Restored text when caller holds `PII_RAW_READ`.
    pub restored: String,
    /// Masked text when caller lacks `PII_RAW_READ`.
    pub masked: String,
    /// Mapping from placeholder token (`⟨PHONE_1⟩`) to original matched substring.
    pub placeholders: BTreeMap<String, String>,
}
