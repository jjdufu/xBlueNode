//! External agent embed token service (`axe_<32 base62>`), `Origin` policy enforcement,
//! per-`(embed_id, client_ip)` rate limiting, and anonymous chat entry (`08-STEP09` §09.7).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentChatResponse, AgentEmbedRecord, AgentEvent, CreateAgentEmbedRequest,
    CreateAgentEmbedResponse, NewAgentEmbedInput, ResolvedAgentEmbed,
};
use axpoint_domain::enums::{AuditAction, ConversationChannel, Permission, TokenKind};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, CredentialStore, FlowRepository, GraphQueryPort,
    GraphStorePort, MetricStorePort, RulesRepository, StudioRepository,
};
use axpoint_llm::sha256_hex;
use axpoint_security::{AuditWriter, AuthPrincipal};
use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::rate_limit::KeyedRateLimiter;
use crate::runtime::{AgentRuntime, ChannelChatCall};

const BASE62_ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const EMBED_TOKEN_RANDOM_LEN: usize = 32;

/// HTTP request metadata extracted for `POST /api/axagent/embed/{token}/chat`.
#[derive(Debug, Clone, Default)]
pub struct EmbedRequestMeta {
    /// Optional `Origin` request header.
    pub origin: Option<String>,
    /// Optional `Host` request header (used for same-origin verification when `allowed_origins` is empty).
    pub host: Option<String>,
    /// Client IP address used for `(embed_id, client_ip)` rate limiting and `SHARE_ACCESS` audit.
    pub client_ip: String,
    /// Distributed trace ID for `sec_audit_logs`.
    pub trace_id: String,
}

/// External `AxAgent` embed link issuer, validator, rate-limiter, and chat dispatcher (`08-STEP09` §09.7).
pub struct EmbedService<R> {
    store: Arc<R>,
    runtime: Arc<AgentRuntime<R>>,
    audit: Option<AuditWriter>,
    rate_limiter: KeyedRateLimiter<(Uuid, String)>,
}

impl<R> EmbedService<R> {
    /// Creates a new [`EmbedService`] backed by `store` and `runtime`.
    #[must_use]
    pub fn new(store: Arc<R>, runtime: Arc<AgentRuntime<R>>, audit: Option<AuditWriter>) -> Self {
        Self {
            store,
            runtime,
            audit,
            rate_limiter: KeyedRateLimiter::new(16_384),
        }
    }
}

impl<R> EmbedService<R>
where
    R: AgentRepository + CredentialStore + 'static,
{
    /// Issues a new `axe_<32 base62>` external embed token (`VIEW_SHARE` required, permissions subset enforced).
    ///
    /// # Errors
    /// Returns [`AppError`] on missing `VIEW_SHARE`, privilege escalation (`permissions` not subset of caller),
    /// invalid `allowed_origins`, invalid `rate_limit_rpm`, or missing agent.
    pub async fn create_embed(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
        req: CreateAgentEmbedRequest,
        client_ip: &str,
        trace_id: &str,
    ) -> Result<CreateAgentEmbedResponse, AppError> {
        principal.require(Permission::ViewShare)?;
        for &perm in &req.permissions {
            if !principal.has(perm) {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
        }
        if !(1..=6_000).contains(&req.rate_limit_rpm) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "embed rate_limit_rpm must be in 1..=6000",
            ));
        }
        let clean_origins = validate_allowed_origins(&req.allowed_origins)?;

        let _agent = self
            .store
            .get_agent(&principal.tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;

        let raw_token = generate_embed_token();
        let token_hash = sha256_hex(&raw_token);
        let record = self
            .store
            .create_agent_embed(
                &principal.tenant_id,
                NewAgentEmbedInput {
                    agent_id,
                    token_hash,
                    allowed_origins: clean_origins,
                    permissions: req.permissions,
                    rate_limit_rpm: req.rate_limit_rpm,
                    expires_at: req.expires_at,
                    created_by: principal.user_id,
                },
            )
            .await?;

        self.emit_embed_audit(&EmbedAuditEvent {
            tenant_id: &principal.tenant_id,
            user_id: Some(principal.user_id),
            username: &principal.username,
            action: AuditAction::ShareCreate,
            embed_id: record.id,
            client_ip,
            trace_id,
        });

        Ok(CreateAgentEmbedResponse {
            id: record.id,
            agent_id: record.agent_id,
            url: format!("/share/agent/{raw_token}"),
            token: raw_token,
            allowed_origins: record.allowed_origins,
            permissions: record.permissions,
            rate_limit_rpm: record.rate_limit_rpm,
            expires_at: record.expires_at,
            created_at: record.created_at,
        })
    }

    /// Lists embed token records for `agent_id` without exposing secret hashes (`VIEW_SHARE` required).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or missing agent.
    pub async fn list_embeds(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
    ) -> Result<Vec<AgentEmbedRecord>, AppError> {
        principal.require(Permission::ViewShare)?;
        let _agent = self
            .store
            .get_agent(&principal.tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        self.store
            .list_agent_embeds(&principal.tenant_id, agent_id)
            .await
    }

    /// Revokes `embed_id` (`DELETE /api/axagent/embeds/{embed_id}`, `VIEW_SHARE` required).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or when `embed_id` is absent.
    pub async fn revoke_embed(
        &self,
        principal: &AuthPrincipal,
        embed_id: Uuid,
    ) -> Result<(), AppError> {
        principal.require(Permission::ViewShare)?;
        let revoked = self
            .store
            .revoke_agent_embed(&principal.tenant_id, embed_id)
            .await?;
        if !revoked {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        Ok(())
    }

    /// Authenticates an incoming `axe_...` token, enforces `Origin` policy (`403 PERMISSION_DENIED`),
    /// checks `(embed_id, client_ip)` rate limits (`429 RATE_LIMITED`), and emits `SHARE_ACCESS` audit.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] (`NOT_FOUND`) when the token is malformed, unknown, revoked, or expired (`08-STEP09` §09.7),
    /// [`AppError::Forbidden`] (`PERMISSION_DENIED`) on disallowed `Origin`, or [`AppError::RateLimited`].
    pub async fn authenticate_embed(
        &self,
        raw_token: &str,
        meta: &EmbedRequestMeta,
    ) -> Result<(ResolvedAgentEmbed, AuthPrincipal), AppError> {
        let trimmed = raw_token.trim();
        if !trimmed.starts_with("axe_") || trimmed.len() < 8 {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        let token_hash = sha256_hex(trimmed);
        let resolved = self
            .store
            .resolve_agent_embed(&token_hash)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;

        verify_embed_origin(
            &resolved.allowed_origins,
            meta.origin.as_deref(),
            meta.host.as_deref(),
        )?;

        let ip_key = if meta.client_ip.trim().is_empty() {
            String::from("127.0.0.1")
        } else {
            String::from(meta.client_ip.trim())
        };
        self.rate_limiter
            .check_and_consume((resolved.embed_id, ip_key.clone()), resolved.rate_limit_rpm)?;

        let effective_perms = match self
            .store
            .load_principal(&resolved.tenant_id, resolved.created_by)
            .await?
        {
            Some(creator) => resolved
                .permissions
                .intersection(&creator.permissions)
                .copied()
                .collect(),
            None => resolved.permissions.clone(),
        };

        let username = format!("embed:{}", resolved.embed_id);
        self.emit_embed_audit(&EmbedAuditEvent {
            tenant_id: &resolved.tenant_id,
            user_id: None,
            username: &username,
            action: AuditAction::ShareAccess,
            embed_id: resolved.embed_id,
            client_ip: &ip_key,
            trace_id: &meta.trace_id,
        });

        let principal = AuthPrincipal {
            user_id: resolved.created_by,
            tenant_id: resolved.tenant_id.clone(),
            username,
            dept_id: String::from("default"),
            roles: Vec::new(),
            permissions: effective_perms,
            token_kind: TokenKind::Embed,
            token_profile: axpoint_domain::enums::TokenProfile::General,
            web_base_url: None,
        };
        Ok((resolved, principal))
    }
}

struct EmbedAuditEvent<'a> {
    tenant_id: &'a str,
    user_id: Option<Uuid>,
    username: &'a str,
    action: AuditAction,
    embed_id: Uuid,
    client_ip: &'a str,
    trace_id: &'a str,
}

impl<R> EmbedService<R> {
    fn emit_embed_audit(&self, ev: &EmbedAuditEvent<'_>) {
        let Some(ref writer) = self.audit else {
            return;
        };
        writer.record(AuditRecord {
            tenant_id: String::from(ev.tenant_id),
            user_id: ev.user_id.filter(|u| !u.is_nil()),
            username: String::from(ev.username),
            action: ev.action,
            resource_ref: format!("agent_embed:{}", ev.embed_id),
            status_code: 200,
            client_ip: if ev.client_ip.is_empty() {
                String::from("127.0.0.1")
            } else {
                String::from(ev.client_ip)
            },
            trace_id: String::from(ev.trace_id),
            detail_json: serde_json::json!({ "embed_id": ev.embed_id }),
            created_at: Utc::now(),
        });
    }
}

impl<R> EmbedService<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + CredentialStore
        + 'static,
{
    /// Executes `POST /api/axagent/embed/{token}/chat` (JSON or SSE streaming when `event_tx` is `Some`).
    ///
    /// Draft mode is forcibly disabled (`draft = false`) for external embed visitors.
    ///
    /// # Errors
    /// Returns [`AppError`] on token/origin/rate-limit rejection or downstream agent failure.
    pub async fn chat_via_embed(
        &self,
        raw_token: &str,
        mut req: AgentChatRequest,
        meta: EmbedRequestMeta,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentChatResponse, AppError> {
        let (resolved, principal) = self.authenticate_embed(raw_token, &meta).await?;
        req.draft = false;

        self.runtime
            .chat_in_channel(
                &principal,
                ChannelChatCall {
                    agent_id: resolved.agent_id,
                    explicit_version: None,
                    explicit_spec: None,
                    request: req,
                    channel: ConversationChannel::Embed,
                    embed_id: Some(resolved.embed_id),
                },
                event_tx,
            )
            .await
    }
}

/// Generates a cryptographically random `axe_<32 base62 chars>` external embed token using OS CSPRNG (`Uuid::new_v4`).
#[must_use]
pub fn generate_embed_token() -> String {
    let mut out = String::with_capacity(4 + EMBED_TOKEN_RANDOM_LEN);
    out.push_str("axe_");
    while out.len() < 4 + EMBED_TOKEN_RANDOM_LEN {
        let random_bytes = *Uuid::new_v4().as_bytes();
        for byte in random_bytes {
            if out.len() >= 4 + EMBED_TOKEN_RANDOM_LEN {
                break;
            }
            // Rejection-sample < 248 (62 * 4 = 248) so modulo 62 is strictly uniform.
            if byte < 248 {
                let idx = usize::from(byte % 62);
                if let Some(&ch) = BASE62_ALPHABET.get(idx) {
                    out.push(char::from(ch));
                }
            }
        }
    }
    out
}

fn validate_allowed_origins(origins: &[String]) -> Result<Vec<String>, AppError> {
    let mut out = Vec::with_capacity(origins.len());
    for raw in origins {
        let normalized = normalize_origin(raw)?;
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    Ok(out)
}

fn normalize_origin(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.contains('*') {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid allowed origin '{raw}': wildcards and empty strings are forbidden"),
        ));
    }
    let parsed = reqwest::Url::parse(trimmed).map_err(|err| {
        AppError::bad("INVALID_ARGUMENT", format!("invalid origin '{raw}': {err}"))
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "allowed origin scheme must be http or https",
        ));
    }
    if parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("allowed origin '{raw}' must contain only scheme://host[:port]"),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "origin missing host"))?;
    let formatted = match parsed.port() {
        Some(port) => format!(
            "{}://{}:{}",
            parsed.scheme(),
            host.to_ascii_lowercase(),
            port
        ),
        None => format!("{}://{}", parsed.scheme(), host.to_ascii_lowercase()),
    };
    Ok(formatted)
}

fn verify_embed_origin(
    allowed_origins: &[String],
    origin_header: Option<&str>,
    host_header: Option<&str>,
) -> Result<(), AppError> {
    if allowed_origins.is_empty() {
        // Empty `allowed_origins` permits same-origin requests only (§09.7).
        if let Some(raw_origin) = origin_header.map(str::trim).filter(|s| !s.is_empty()) {
            let norm = normalize_origin(raw_origin)
                .map_err(|_| AppError::Forbidden("PERMISSION_DENIED"))?;
            let origin_authority = norm.split_once("://").map_or("", |(_, rest)| rest);
            let expected_host = host_header.map_or("", str::trim).to_ascii_lowercase();
            if expected_host.is_empty() || origin_authority != expected_host {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
        }
        return Ok(());
    }

    let Some(raw_origin) = origin_header.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    };
    let candidate =
        normalize_origin(raw_origin).map_err(|_| AppError::Forbidden("PERMISSION_DENIED"))?;
    let matched = allowed_origins
        .iter()
        .any(|allowed| normalize_origin(allowed).is_ok_and(|norm| norm == candidate));
    if !matched {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axpoint_domain::AppError;

    use super::{generate_embed_token, verify_embed_origin};

    #[test]
    fn generated_embed_token_has_axe_prefix_and_base62_body() {
        let tok = generate_embed_token();
        assert!(tok.starts_with("axe_"));
        assert_eq!(tok.len(), 36);
        assert!(
            tok.strip_prefix("axe_")
                .is_some_and(|b| b.chars().all(|c| c.is_ascii_alphanumeric()))
        );
    }

    #[test]
    fn origin_verification_enforces_whitelist_and_same_origin() -> Result<(), AppError> {
        let whitelist = vec![String::from("https://portal.example.com")];
        verify_embed_origin(&whitelist, Some("https://portal.example.com/"), None)?;
        assert!(matches!(
            verify_embed_origin(&whitelist, Some("https://evil.example.com"), None),
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));
        assert!(matches!(
            verify_embed_origin(&whitelist, None, None),
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));

        // Same-origin mode when whitelist is empty.
        verify_embed_origin(
            &[],
            Some("https://app.example.com"),
            Some("app.example.com"),
        )?;
        assert!(matches!(
            verify_embed_origin(
                &[],
                Some("https://evil.example.com"),
                Some("app.example.com")
            ),
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));
        Ok(())
    }
}
