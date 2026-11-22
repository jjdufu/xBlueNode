//! Enterprise IM outbound bidirectional bot connector contracts, identity-binding challenges,
//! principal hydration guardrails, and Markdown-safe message chunking (`07-STEP21` §21.1–§21.4).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::AppError;
use crate::axhub::SealedCredential;
use crate::enums::{ImConnectorStatus, ImIdentityPolicy, ImPlatform, Permission, Sensitivity};
use crate::platform::PrincipalRecord;

/// Single-leader PostgreSQL lease duration in seconds (`07-STEP21` §21.2.2).
pub const IM_LEASE_DURATION_SECS: i32 = 30;

/// Lease renewal and status heartbeat interval in seconds (`07-STEP21` §21.2.2).
pub const IM_HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// Supervisor unclaimed/expired connector scan interval in seconds (`07-STEP21` §21.2.2).
pub const IM_SUPERVISOR_SCAN_INTERVAL_SECS: u64 = 5;

/// One-time IM bind challenge validity window in seconds (`10` minutes, `07-STEP21` §21.3).
pub const IM_BIND_CHALLENGE_TTL_SECS: i64 = 600;

/// Minimum progress-card update interval in milliseconds (`07-STEP21` §21.2.3).
pub const IM_PROGRESS_THROTTLE_MS: u64 = 800;

/// Maximum UTF-8 payload bytes per outbound IM message chunk (`07-STEP21` §21.2.3).
pub const IM_MARKDOWN_MAX_CHUNK_BYTES: usize = 3_600;

/// Prefix for one-time user bind challenge tokens (`axim_<base64url>`).
pub const IM_BIND_TOKEN_PREFIX: &str = "axim_";

const fn default_true() -> bool {
    true
}

/// Non-secret connector runtime configuration stored in `ax_im_connectors.config` (`07-STEP21` §21.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImConnectorConfig {
    /// Optional private/test gateway base URL or WebSocket endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_override: Option<String>,
    /// Permits RFC1918 / loopback `endpoint_override` targets (`GUARD_ADMIN` required to set `true`).
    #[serde(default)]
    pub allow_private_network: bool,
    /// Optional platform bot code (`DingTalk` `robotCode` or custom identifier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_code: Option<String>,
    /// Optional explicit workbench base URL used when formatting `/agents?im_bind=...` deep links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_base_url: Option<String>,
}

/// Plaintext credential input sealed into `ds_credentials` (`07-STEP21` §21.1).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImConnectorSecretInput {
    /// Feishu `AppID` (or generic credential ID fallback).
    #[serde(default)]
    pub app_id: Option<String>,
    /// Feishu `AppSecret` (or generic credential secret fallback).
    #[serde(default)]
    pub app_secret: Option<String>,
    /// `DingTalk` `ClientID` (`AppKey`).
    #[serde(default)]
    pub client_id: Option<String>,
    /// `DingTalk` `ClientSecret` (`AppSecret`).
    #[serde(default)]
    pub client_secret: Option<String>,
    /// `WeCom` intelligent bot `bot_id`.
    #[serde(default)]
    pub bot_id: Option<String>,
    /// `WeCom` intelligent bot `secret`.
    #[serde(default)]
    pub secret: Option<String>,
}

impl ImConnectorSecretInput {
    /// Returns `true` when no non-empty secret fields were supplied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        [
            &self.app_id,
            &self.app_secret,
            &self.client_id,
            &self.client_secret,
            &self.bot_id,
            &self.secret,
        ]
        .into_iter()
        .all(|opt| opt.as_deref().map_or("", str::trim).is_empty())
    }

    /// Validates and normalizes credential keys for `platform` into canonical `ds_credentials` map entries.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when the required ID or secret is blank.
    pub fn normalize_for_platform(
        &self,
        platform: ImPlatform,
    ) -> Result<BTreeMap<String, String>, AppError> {
        let trim_non_empty = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let mut out = BTreeMap::new();
        match platform {
            ImPlatform::Feishu => {
                let id = trim_non_empty(&self.app_id).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "FEISHU secret requires app_id")
                })?;
                let sec = trim_non_empty(&self.app_secret).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "FEISHU secret requires app_secret")
                })?;
                out.insert(String::from("app_id"), id);
                out.insert(String::from("app_secret"), sec);
            }
            ImPlatform::Dingtalk => {
                let id = trim_non_empty(&self.client_id)
                    .or_else(|| trim_non_empty(&self.app_id))
                    .ok_or_else(|| {
                        AppError::bad("INVALID_ARGUMENT", "DINGTALK secret requires client_id")
                    })?;
                let sec = trim_non_empty(&self.client_secret)
                    .or_else(|| trim_non_empty(&self.app_secret))
                    .ok_or_else(|| {
                        AppError::bad("INVALID_ARGUMENT", "DINGTALK secret requires client_secret")
                    })?;
                out.insert(String::from("client_id"), id);
                out.insert(String::from("client_secret"), sec);
            }
            ImPlatform::Wecom => {
                let id = trim_non_empty(&self.bot_id)
                    .or_else(|| trim_non_empty(&self.app_id))
                    .ok_or_else(|| {
                        AppError::bad("INVALID_ARGUMENT", "WECOM secret requires bot_id")
                    })?;
                let sec = trim_non_empty(&self.secret)
                    .or_else(|| trim_non_empty(&self.app_secret))
                    .ok_or_else(|| {
                        AppError::bad("INVALID_ARGUMENT", "WECOM secret requires secret")
                    })?;
                out.insert(String::from("bot_id"), id);
                out.insert(String::from("secret"), sec);
            }
        }
        Ok(out)
    }
}

/// Persisted outbound IM connector record (`ax_im_connectors`, `07-STEP21` §21.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImConnectorRecord {
    /// Connector UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Unique connector name within the tenant.
    pub name: String,
    /// Target enterprise IM platform (`FEISHU`, `DINGTALK`, `WECOM`).
    pub platform: ImPlatform,
    /// Bound `ax_agents.id` assistant.
    pub agent_id: Uuid,
    /// Internal `ds_credentials.id` reference (never serialized to public HTTP responses).
    #[serde(skip_serializing, default)]
    pub credential_id: Uuid,
    /// Indicates that sealed credentials are attached (`true`).
    #[serde(default = "default_true")]
    pub has_secret: bool,
    /// Non-secret connector configuration.
    pub config: ImConnectorConfig,
    /// Identity resolution policy (`REQUIRE_USER_BIND` or `CREATOR_DELEGATED`).
    pub identity_policy: ImIdentityPolicy,
    /// Designated `sec_users.id` when `identity_policy == CREATOR_DELEGATED`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_user_id: Option<Uuid>,
    /// Administrative enable/disable toggle.
    pub enabled: bool,
    /// Current Core replica holder identifier (`None` when unleased).
    pub leader_holder: Option<String>,
    /// Current lease expiration timestamp.
    pub lease_until: Option<DateTime<Utc>>,
    /// Runtime WebSocket/Stream connection state.
    pub status: ImConnectorStatus,
    /// Most recent connection or handshake error message (empty when healthy).
    pub last_error: String,
    /// Timestamp of most recent leader heartbeat.
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axagent/im-connectors` (`07-STEP21` §21.4.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateImConnectorRequest {
    /// Unique connector name within the tenant.
    pub name: String,
    /// Target IM platform (`FEISHU`, `DINGTALK`, `WECOM`).
    pub platform: ImPlatform,
    /// Bound `ax_agents.id`.
    pub agent_id: Uuid,
    /// Identity resolution policy (defaults to `REQUIRE_USER_BIND`).
    #[serde(default)]
    pub identity_policy: ImIdentityPolicy,
    /// Required when `identity_policy == CREATOR_DELEGATED`.
    #[serde(default)]
    pub delegated_user_id: Option<Uuid>,
    /// Plaintext application credentials sealed into `ds_credentials`.
    pub secret: ImConnectorSecretInput,
    /// Optional non-secret runtime parameters.
    #[serde(default)]
    pub config: ImConnectorConfig,
    /// Whether outbound connection management starts enabled (default `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Request payload for `PATCH /api/axagent/im-connectors/{id}` (`07-STEP21` §21.4.1).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchImConnectorRequest {
    /// Optional updated connector name.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional updated bound `ax_agents.id`.
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    /// Optional updated identity policy.
    #[serde(default)]
    pub identity_policy: Option<ImIdentityPolicy>,
    /// Optional updated delegated user UUID.
    #[serde(default)]
    pub delegated_user_id: Option<Uuid>,
    /// Optional rotated plaintext secret (omitted or empty keeps existing `credential_id`).
    #[serde(default)]
    pub secret: Option<ImConnectorSecretInput>,
    /// Optional updated non-secret runtime configuration.
    #[serde(default)]
    pub config: Option<ImConnectorConfig>,
    /// Optional enable/disable toggle.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Repository input for inserting a new `ax_im_connectors` row and its `ds_credentials` entry.
#[derive(Debug, Clone)]
pub struct NewImConnectorInput {
    /// Connector name.
    pub name: String,
    /// Platform kind.
    pub platform: ImPlatform,
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// Pre-allocated `ds_credentials.id` matching the AAD used to seal `credential`.
    pub credential_id: Uuid,
    /// AES-256-GCM sealed secret map.
    pub credential: SealedCredential,
    /// Non-secret configuration.
    pub config: ImConnectorConfig,
    /// Identity policy.
    pub identity_policy: ImIdentityPolicy,
    /// Optional delegated user UUID.
    pub delegated_user_id: Option<Uuid>,
    /// Enabled flag.
    pub enabled: bool,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
}

/// Repository input for updating an existing `ax_im_connectors` row.
#[derive(Debug, Clone)]
pub struct UpdateImConnectorInput {
    /// Updated connector name.
    pub name: String,
    /// Updated target agent UUID.
    pub agent_id: Uuid,
    /// Optional newly sealed `(credential_id, SealedCredential)` when rotating credentials.
    pub rotated_credential: Option<(Uuid, SealedCredential)>,
    /// Updated non-secret configuration.
    pub config: ImConnectorConfig,
    /// Updated identity policy.
    pub identity_policy: ImIdentityPolicy,
    /// Updated delegated user UUID.
    pub delegated_user_id: Option<Uuid>,
    /// Updated enabled flag.
    pub enabled: bool,
}

/// Response payload for `POST /api/axagent/im-connectors/{id}/test` (`07-STEP21` §21.4.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImConnectorTestResponse {
    /// Whether outbound ticket exchange / auth probe succeeded.
    pub ok: bool,
    /// Probed platform.
    pub platform: ImPlatform,
    /// Measured round-trip latency in milliseconds.
    pub latency_ms: u64,
    /// Sanitized WebSocket/Stream endpoint host preview.
    pub endpoint_preview: String,
    /// Diagnostic status message.
    pub message: String,
}

/// Persisted mapping between an external IM account and a platform `sec_users` principal (`ax_im_user_bindings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImUserBindingRecord {
    /// Binding UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// IM platform (`FEISHU`, `DINGTALK`, `WECOM`).
    pub platform: ImPlatform,
    /// Platform-scoped external user ID (e.g., Feishu `open_id`, `DingTalk` `staffId`, `WeCom` `userid`).
    pub external_user_id: String,
    /// Display name reported by the IM platform at bind time.
    pub external_display_name: String,
    /// Bound platform `sec_users.id`.
    pub user_id: Uuid,
    /// Optional resolved `sec_users.username` for UI presentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Bind confirmation timestamp.
    pub bound_at: DateTime<Utc>,
}

/// Repository input for inserting a one-time challenge in `ax_im_bind_challenges`.
#[derive(Debug, Clone)]
pub struct NewImBindChallengeInput {
    /// Hex-encoded SHA-256 digest of the raw `axim_...` token.
    pub token_hash: String,
    /// Originating `ax_im_connectors.id`.
    pub connector_id: Uuid,
    /// Originating IM platform.
    pub platform: ImPlatform,
    /// External IM user identifier requesting account binding.
    pub external_user_id: String,
    /// External IM user display name.
    pub external_display_name: String,
    /// Expiration timestamp (`NOW() + 10m`).
    pub expires_at: DateTime<Utc>,
}

/// Query string parameters for `GET /api/axagent/im-bind/preview?token=`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImBindPreviewQuery {
    /// Raw `axim_...` challenge token.
    pub token: String,
}

/// Response payload for `GET /api/axagent/im-bind/preview` (`07-STEP21` §21.4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImBindChallengePreview {
    /// Originating connector UUID.
    pub connector_id: Uuid,
    /// Originating connector display name.
    pub connector_name: String,
    /// IM platform (`FEISHU`, `DINGTALK`, `WECOM`).
    pub platform: ImPlatform,
    /// External IM user identifier.
    pub external_user_id: String,
    /// External IM user display name.
    pub external_display_name: String,
    /// Challenge expiration timestamp.
    pub expires_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axagent/im-bind/confirm` (`07-STEP21` §21.4.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmImBindRequest {
    /// One-time `axim_...` challenge token.
    pub token: String,
}

/// Atomic row returned by `SECURITY DEFINER` function `ax_consume_im_bind_challenge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedImBindChallenge {
    /// Originating connector UUID.
    pub connector_id: Uuid,
    /// IM platform.
    pub platform: ImPlatform,
    /// Bound external IM user identifier.
    pub external_user_id: String,
    /// Bound external IM display name.
    pub external_display_name: String,
}

/// Response payload for `POST /api/axagent/im-bind/confirm` (`07-STEP21` §21.4.1 & §21.5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmImBindResponse {
    /// Originating connector UUID.
    pub connector_id: Uuid,
    /// Bound IM platform (`FEISHU`, `DINGTALK`, `WECOM`).
    pub platform: ImPlatform,
    /// Bound external IM user identifier.
    pub external_user_id: String,
    /// Bound external IM display name.
    pub external_display_name: String,
    /// Bound platform `sec_users.id`.
    pub user_id: Uuid,
    /// Confirmation timestamp.
    pub bound_at: DateTime<Utc>,
}

/// Normalized inbound chat/mention event decoded from a platform WebSocket/Stream frame (`07-STEP21` §21.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImInboundMessage {
    /// Platform-unique event or message ID used for `ax_im_event_dedup`.
    pub event_id: String,
    /// External sender user ID (`open_id` / `staffId` / `userid`).
    pub external_user_id: String,
    /// External sender display name (empty when omitted by platform).
    pub external_display_name: String,
    /// External conversation / group / DM thread identifier.
    pub chat_id: String,
    /// Cleaned user prompt text (leading `@bot` mention stripped).
    pub text: String,
    /// Optional upstream message ID or session webhook token for threaded replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_token: Option<String>,
    /// Whether this event originated from a group chat (`false` = direct message).
    #[serde(default)]
    pub is_group: bool,
}

/// Normalized outbound reply or progress-card update sent over a platform channel (`07-STEP21` §21.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImOutboundMessage {
    /// Target external conversation / DM thread identifier.
    pub chat_id: String,
    /// Markdown / rich-text payload (`<= IM_MARKDOWN_MAX_CHUNK_BYTES`).
    pub text: String,
    /// Optional upstream correlation token (`message_id`, `sessionWebhook`, or `WeCom` `stream_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_token: Option<String>,
    /// Optional stream/card identifier when updating an in-progress card in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    /// `true` when this frame represents a terminal answer chunk rather than an intermediate progress hint.
    #[serde(default = "default_true")]
    pub is_final: bool,
}

/// Hydrated platform user identity and ABAC attributes for an outbound IM turn (`07-STEP21` §21.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydratedImPrincipal {
    /// Sanitized user record (`Permission::PiiRawRead` stripped; `CREATOR_DELEGATED` capped).
    pub principal: PrincipalRecord,
    /// Multi-valued ABAC user attributes from `sec_user_attributes`.
    #[serde(default)]
    pub attributes: BTreeMap<String, Vec<String>>,
}

/// Computes the lowercase hex SHA-256 digest of a raw `axim_...` bind token after validation.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `raw_token` does not start with `axim_`
/// or has fewer than 12 characters after the prefix.
pub fn hash_im_bind_token(raw_token: &str) -> Result<String, AppError> {
    let trimmed = raw_token.trim();
    let Some(body) = trimmed.strip_prefix(IM_BIND_TOKEN_PREFIX) else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "IM bind token must start with axim_",
        ));
    };
    if body.len() < 12
        || !body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "IM bind token format is invalid",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Enforces mandatory `07-STEP21` §21.3 security guardrails on a hydrated [`PrincipalRecord`]:
/// 1. Unconditionally strips [`Permission::PiiRawRead`] so raw PII never traverses third-party IM clouds;
/// 2. Under [`ImIdentityPolicy::CreatorDelegated`], restricts permissions to `{DATA_READ, AGENT_RUN}`
///    and caps `clearance` to [`Sensitivity::Internal`].
pub fn apply_im_identity_guardrails(principal: &mut PrincipalRecord, policy: ImIdentityPolicy) {
    principal.permissions.remove(&Permission::PiiRawRead);
    if policy == ImIdentityPolicy::CreatorDelegated {
        let allowed = BTreeSet::from([Permission::DataRead, Permission::AgentRun]);
        principal.permissions.retain(|perm| allowed.contains(perm));
        if matches!(
            principal.clearance,
            Sensitivity::Confidential | Sensitivity::Secret
        ) {
            principal.clearance = Sensitivity::Internal;
        }
    }
}

/// Splits a Markdown reply into UTF-8 safe chunks of at most `max_bytes` (`07-STEP21` §21.2.3),
/// preserving fenced code blocks (` ``` `), Markdown table rows, and `[文N]` citation markers.
#[must_use]
pub fn split_markdown_chunks(markdown: &str, max_bytes: usize) -> Vec<String> {
    let budget = max_bytes.max(128);
    let trimmed = markdown.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.len() <= budget {
        return vec![String::from(trimmed)];
    }

    let blocks = split_atomic_markdown_blocks(trimmed);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for block in blocks {
        let sep_len = if current.is_empty() { 0 } else { 2 };
        if !current.is_empty()
            && current
                .len()
                .saturating_add(sep_len)
                .saturating_add(block.len())
                <= budget
        {
            current.push_str("\n\n");
            current.push_str(&block);
            continue;
        }
        if !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if block.len() <= budget {
            current = block;
        } else {
            append_oversized_block(&block, budget, &mut chunks);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_atomic_markdown_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut buf = String::new();
    let mut in_fence = false;

    for line in markdown.lines() {
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("```") {
            if !in_fence && !buf.trim().is_empty() {
                blocks.push(std::mem::take(&mut buf));
            }
            in_fence = !in_fence;
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
            if !in_fence {
                blocks.push(std::mem::take(&mut buf));
            }
            continue;
        }
        if in_fence {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
        } else if line.trim().is_empty() {
            if buf.trim().is_empty() {
                buf.clear();
            } else {
                blocks.push(std::mem::take(&mut buf));
            }
        } else {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
        }
    }
    if !buf.trim().is_empty() {
        blocks.push(buf);
    }
    blocks
}

fn append_oversized_block(block: &str, budget: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    for line in block.lines() {
        let sep_len = usize::from(!current.is_empty());
        if !current.is_empty()
            && current
                .len()
                .saturating_add(sep_len)
                .saturating_add(line.len())
                <= budget
        {
            current.push('\n');
            current.push_str(line);
            continue;
        }
        if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if line.len() <= budget {
            current.push_str(line);
        } else {
            split_long_line_citation_safe(line, budget, out);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
}

fn split_long_line_citation_safe(line: &str, budget: usize, out: &mut Vec<String>) {
    let mut chunk = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        let mut token = String::from(ch);
        if ch == '[' {
            let mut bracket_buf = String::new();
            let mut closed = false;
            while let Some(&next_ch) = chars.peek() {
                if bracket_buf.len() >= 24 {
                    break;
                }
                chars.next();
                bracket_buf.push(next_ch);
                if next_ch == ']' {
                    closed = true;
                    break;
                }
            }
            token.push_str(&bracket_buf);
            if !closed && token.len() > budget {
                // Safety fallback when `[` was not a short `[文N]` marker.
            }
        }
        if !chunk.is_empty() && chunk.len().saturating_add(token.len()) > budget {
            out.push(std::mem::take(&mut chunk));
        }
        chunk.push_str(&token);
    }
    if !chunk.is_empty() {
        out.push(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn im_guardrails_strip_pii_raw_read_and_cap_delegated_policy() {
        let mut record = PrincipalRecord {
            user_id: Uuid::new_v4(),
            tenant_id: String::from("default"),
            username: String::from("admin_user"),
            display_name: String::from("Admin"),
            dept_id: String::from("east"),
            roles: vec![String::from("ADMIN")],
            permissions: Permission::all_set(),
            clearance: Sensitivity::Secret,
            tokens_valid_after: Utc::now(),
        };

        apply_im_identity_guardrails(&mut record, ImIdentityPolicy::RequireUserBind);
        assert!(!record.permissions.contains(&Permission::PiiRawRead));
        assert!(record.permissions.contains(&Permission::FlowRun));
        assert_eq!(record.clearance, Sensitivity::Secret);

        apply_im_identity_guardrails(&mut record, ImIdentityPolicy::CreatorDelegated);
        assert_eq!(
            record.permissions,
            BTreeSet::from([Permission::DataRead, Permission::AgentRun])
        );
        assert_eq!(record.clearance, Sensitivity::Internal);
    }

    #[test]
    fn im_markdown_chunker_preserves_fences_tables_and_citations() {
        let doc = concat!(
            "华西区订单汇总如下 [文1]：\n\n",
            "| 仓库 | 订单数 |\n| --- | --- |\n| W-1 | 128 |\n| W-2 | 64 |\n\n",
            "```sql\nSELECT warehouse_code, COUNT(*) FROM demo_orders GROUP BY 1;\n```\n\n",
            "补充说明段落 [文12]：库存周转保持稳定。"
        );
        let chunks = split_markdown_chunks(doc, 140);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 140);
        }
        let rejoined = chunks.join("\n\n");
        assert!(rejoined.contains("[文1]"));
        assert!(rejoined.contains("[文12]"));
        assert!(
            chunks
                .iter()
                .any(|c| c.contains("```sql\n") && c.contains("\n```"))
        );
    }

    #[test]
    fn im_bind_token_hash_validates_prefix_and_charset() {
        assert!(hash_im_bind_token("invalid_token_1234567890").is_err());
        assert!(hash_im_bind_token("axim_short").is_err());
        let ok = hash_im_bind_token("axim_AbCdEfGhIjKlMnOpQrStUvWxYz012345");
        assert!(ok.is_ok());
        assert_eq!(ok.unwrap_or_default().len(), 64);
    }
}
