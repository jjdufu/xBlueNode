//! Multi-model [`LlmRouter`] with per-tenant caching, priority selection, `rpm_limit` token buckets,
//! consecutive-failure circuit breakers, automatic failover, usage accounting, and cassette replay (`08-STEP09` §09.2).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axpoint_domain::AppError;
use axpoint_domain::agent::{LlmProviderRecord, LlmUsageDelta, ProviderTestResponse};
use axpoint_domain::enums::Purpose;
use axpoint_domain::guard::LlmEgressPreviewResponse;
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{FlowCredentialUnsealPort, LlmProviderStore, OutboundUrlGuardPort};
use axpoint_security::masking::MaskKeyRing;
use axpoint_security::pii_local::{EgressSession, preview_llm_egress, restore_from_egress};
use chrono::Utc;
use moka::future::Cache;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;
use uuid::Uuid;

use crate::cassette::{
    Cassette, CassetteMode, CassetteTurn, DEFAULT_OFFLINE_EMBED_DIM, deterministic_embedding,
};
use crate::stream::consume_sse_response;
use crate::tools::{
    TokenUsage, ToolChatMessage, ToolChatTurn, ToolDefinition, WireChatCompletionRequest,
    WireChatCompletionResponse, WireStreamOptions, canonical_request_transcript,
    extract_system_and_last_user, parse_chat_completion_response,
};
use crate::{EMBED_BATCH_CHUNK_SIZE, EmbeddingConfig, LlmConfig};

/// Previews outbound LLM PII placeholder redaction (`⟨PHONE_1⟩`, etc.) and inbound raw/masked
/// restoration for `POST /api/axguard/debug/llm-egress-preview` (`11-STEP13` §13.8.3, §13.10).
#[must_use]
pub fn preview_egress_filter(text: &str, tenant_id: &str) -> LlmEgressPreviewResponse {
    let key = resolve_tenant_mask_key(tenant_id);
    preview_llm_egress(text, &key)
}

pub(crate) fn resolve_tenant_mask_key(tenant_id: &str) -> [u8; 32] {
    MaskKeyRing::from_env()
        .unwrap_or_else(|_| MaskKeyRing::dev_default())
        .tenant_key(tenant_id)
}

fn redact_json_strings(val: &mut serde_json::Value, session: &mut EgressSession) {
    match val {
        serde_json::Value::String(s) => {
            *s = session.redact_text(s);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json_strings(item, session);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                redact_json_strings(v, session);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn restore_json_strings(
    val: &mut serde_json::Value,
    session: &EgressSession,
    allow_raw: bool,
    tenant_key: &[u8; 32],
) {
    match val {
        serde_json::Value::String(s) => {
            *s = restore_from_egress(s, session, allow_raw, tenant_key);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                restore_json_strings(item, session, allow_raw, tenant_key);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                restore_json_strings(v, session, allow_raw, tenant_key);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

pub(crate) fn redact_messages_for_egress(
    messages: &[ToolChatMessage],
) -> (Vec<ToolChatMessage>, EgressSession) {
    let mut session = EgressSession::new();
    let mut out = Vec::with_capacity(messages.len());
    for msg in messages {
        if msg.role.eq_ignore_ascii_case("system") {
            out.push(msg.clone());
            continue;
        }
        let mut cloned = msg.clone();
        cloned.content = session.redact_text(&cloned.content);
        for tc in &mut cloned.tool_calls {
            redact_json_strings(&mut tc.arguments, &mut session);
        }
        out.push(cloned);
    }
    (out, session)
}

pub(crate) fn restore_turn_from_egress(
    turn: &mut ToolChatTurn,
    session: &EgressSession,
    allow_raw: bool,
    tenant_key: &[u8; 32],
) {
    if session.is_empty() {
        return;
    }
    turn.content = restore_from_egress(&turn.content, session, allow_raw, tenant_key);
    for tc in &mut turn.tool_calls {
        restore_json_strings(&mut tc.arguments, session, allow_raw, tenant_key);
    }
}

fn wrap_egress_stream_tx(
    downstream_tx: Option<mpsc::Sender<String>>,
    session: &EgressSession,
    allow_raw: bool,
    tenant_key: [u8; 32],
) -> (
    Option<mpsc::Sender<String>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    if session.is_empty() {
        return (downstream_tx, None);
    }
    let Some(user_tx) = downstream_tx else {
        return (None, None);
    };
    let (inner_tx, mut inner_rx) = mpsc::channel::<String>(64);
    let session_cloned = session.clone();
    let handle = tokio::spawn(async move {
        let mut pending = String::new();
        while let Some(chunk) = inner_rx.recv().await {
            pending.push_str(&chunk);
            let flush_end = match pending.rfind('⟨') {
                Some(open_idx) if !pending.get(open_idx..).unwrap_or("").contains('⟩') => {
                    open_idx
                }
                _ => pending.len(),
            };
            if flush_end > 0
                && let Some(ready) = pending.get(..flush_end).map(String::from)
            {
                pending = pending.get(flush_end..).unwrap_or("").to_owned();
                let restored = restore_from_egress(&ready, &session_cloned, allow_raw, &tenant_key);
                if user_tx.send(restored).await.is_err() {
                    return;
                }
            }
        }
        if !pending.is_empty() {
            let restored = restore_from_egress(&pending, &session_cloned, allow_raw, &tenant_key);
            if user_tx.send(restored).await.is_err() {
                warn!("SSE egress stream receiver dropped on final flush");
            }
        }
    });
    (Some(inner_tx), Some(handle))
}

/// Default TTL in seconds for cached per-tenant `ax_llm_providers` lists.
pub const DEFAULT_PROVIDER_CACHE_TTL_SECS: u64 = 30;

/// Default consecutive upstream failure count before opening a provider circuit breaker (`08-STEP09` §09.2).
pub const DEFAULT_CIRCUIT_FAILURE_THRESHOLD: u32 = 5;

/// Default duration in seconds a tripped provider circuit breaker remains open (`08-STEP09` §09.2).
pub const DEFAULT_CIRCUIT_OPEN_SECS: u64 = 60;

/// Clock abstraction enabling deterministic circuit-breaker and rate-limiter unit tests.
pub trait RouterClock: Send + Sync {
    /// Returns the current monotonic [`Instant`].
    fn now(&self) -> Instant;
}

/// Default monotonic system clock backed by [`Instant::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRouterClock;

impl RouterClock for SystemRouterClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Object-safe resolver returning the decrypted bearer API key for an [`LlmProviderRecord`].
pub trait ProviderSecretResolver: Send + Sync {
    /// Resolves the plaintext bearer API key for `provider` in `tenant_id`, if configured.
    fn resolve_api_key<'a>(
        &'a self,
        tenant_id: &'a str,
        provider: &'a LlmProviderRecord,
    ) -> BoxFuture<'a, Option<String>>;
}

/// Adapter resolving provider API keys through [`FlowCredentialUnsealPort`] (`ds_credentials`).
pub struct FlowCredentialSecretResolver {
    unsealer: Arc<dyn FlowCredentialUnsealPort>,
}

impl FlowCredentialSecretResolver {
    /// Wraps a [`FlowCredentialUnsealPort`] as a [`ProviderSecretResolver`].
    #[must_use]
    pub fn new(unsealer: Arc<dyn FlowCredentialUnsealPort>) -> Self {
        Self { unsealer }
    }
}

impl ProviderSecretResolver for FlowCredentialSecretResolver {
    fn resolve_api_key<'a>(
        &'a self,
        tenant_id: &'a str,
        provider: &'a LlmProviderRecord,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let Some(cred_id) = provider.credential_id else {
                return Ok(None);
            };
            let val = self
                .unsealer
                .unseal_credential_json(tenant_id, cred_id)
                .await?;
            let extracted = match &val {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(map) => map
                    .get("api_key")
                    .or_else(|| map.get("token"))
                    .or_else(|| map.get("secret"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
                _ => None,
            };
            Ok(extracted.filter(|s| !s.is_empty()))
        })
    }
}

/// Request parameters for a single non-streaming or streaming routed LLM turn.
#[derive(Debug, Clone)]
pub struct RouterTurnRequest {
    /// Workload purpose (`CHAT`, `NL2SQL`, `EXTRACT`, `AGENT`, `SUMMARIZE`, `EMBED`).
    pub purpose: Purpose,
    /// Optional pinned `ax_llm_providers.id` (`None` uses priority failover).
    pub pinned_provider_id: Option<Uuid>,
    /// Conversation history and user/tool messages.
    pub messages: Vec<ToolChatMessage>,
    /// Optional function tools exposed to the model.
    pub tools: Vec<ToolDefinition>,
    /// Optional sampling temperature override (`None` uses provider default).
    pub temperature: Option<f32>,
    /// Optional max completion tokens override (`None` uses provider default).
    pub max_tokens: Option<i32>,
    /// Whether candidate selection requires `supports_tools == true`.
    pub require_tools: bool,
    /// Whether inbound placeholder restoration may reveal raw PII (`Permission::PiiRawRead`, `11-STEP13` §13.8.3).
    pub caller_can_read_raw_pii: bool,
}

impl RouterTurnRequest {
    /// Creates a [`RouterTurnRequest`] (`require_tools` defaults to `true` when `purpose == Purpose::Agent`).
    #[must_use]
    pub fn new(purpose: Purpose, messages: Vec<ToolChatMessage>) -> Self {
        Self {
            purpose,
            pinned_provider_id: None,
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            require_tools: purpose == Purpose::Agent,
            caller_can_read_raw_pii: false,
        }
    }

    /// Pins execution to a specific `ax_llm_providers.id`.
    #[must_use]
    pub const fn with_pinned_provider(mut self, provider_id: Option<Uuid>) -> Self {
        self.pinned_provider_id = provider_id;
        self
    }

    /// Attaches `OpenAI` function tool definitions.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        if !tools.is_empty() {
            self.require_tools = true;
        }
        self.tools = tools;
        self
    }

    /// Overrides the sampling temperature.
    #[must_use]
    pub const fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Overrides the maximum completion token budget.
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: i32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Explicitly sets whether `supports_tools` is required on candidate providers.
    #[must_use]
    pub const fn with_require_tools(mut self, require_tools: bool) -> Self {
        self.require_tools = require_tools;
        self
    }

    /// Configures whether restored `⟨CATEGORY_N⟩` placeholders emit plaintext PII (`true`) or masked PII (`false`).
    #[must_use]
    pub const fn with_raw_pii(mut self, allow: bool) -> Self {
        self.caller_can_read_raw_pii = allow;
        self
    }
}

/// Routed completion result pairing the [`ToolChatTurn`] with the selected provider identity.
#[derive(Debug, Clone)]
pub struct RoutedTurnOutcome {
    /// Assembled assistant turn.
    pub turn: ToolChatTurn,
    /// Selected `ax_llm_providers.id`, or `None` when served by the fallback `default_llm`.
    pub provider_id: Option<Uuid>,
    /// Selected provider name (`"default"` for fallback `default_llm`).
    pub provider_name: String,
}

#[derive(Debug)]
struct ProviderHealthState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
    rpm_tokens: Option<f64>,
    last_refill: Instant,
}

impl ProviderHealthState {
    fn new(now: Instant) -> Self {
        Self {
            consecutive_failures: 0,
            open_until: None,
            rpm_tokens: None,
            last_refill: now,
        }
    }
}

#[derive(Debug)]
struct ProviderCallFailure {
    error: AppError,
    counts_toward_breaker: bool,
}

#[derive(Debug, Serialize)]
struct EmbedWireReq<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbedWireResp {
    data: Vec<EmbedWireItem>,
}

#[derive(Debug, Deserialize)]
struct EmbedWireItem {
    #[serde(default)]
    index: Option<usize>,
    embedding: Vec<f32>,
}

/// Multi-model LLM and embedding router with priority failover, circuit breaking, and rate limiting.
#[derive(Clone)]
pub struct LlmRouter {
    store: Option<Arc<dyn LlmProviderStore>>,
    secret_resolver: Option<Arc<dyn ProviderSecretResolver>>,
    url_guard: Option<Arc<dyn OutboundUrlGuardPort>>,
    default_llm: LlmConfig,
    default_embed: Option<EmbeddingConfig>,
    http: Client,
    cassette: Option<Cassette>,
    tenant_cache: Cache<String, Arc<[LlmProviderRecord]>>,
    health: Arc<Mutex<HashMap<Uuid, ProviderHealthState>>>,
    clock: Arc<dyn RouterClock>,
    failure_threshold: u32,
    open_cooldown: Duration,
}

impl fmt::Debug for LlmRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmRouter")
            .field("has_store", &self.store.is_some())
            .field("default_model", &self.default_llm.model)
            .field("failure_threshold", &self.failure_threshold)
            .field("open_cooldown", &self.open_cooldown)
            .finish_non_exhaustive()
    }
}

impl LlmRouter {
    /// Constructs a new [`LlmRouter`] backed by `default_llm` and optional environment cassette.
    #[must_use]
    pub fn new(default_llm: LlmConfig) -> Self {
        let tenant_cache = Cache::builder()
            .max_capacity(1_024)
            .time_to_live(Duration::from_secs(DEFAULT_PROVIDER_CACHE_TTL_SECS))
            .build();
        Self {
            store: None,
            secret_resolver: None,
            url_guard: None,
            default_llm,
            default_embed: None,
            http: Client::new(),
            cassette: Cassette::from_env(),
            tenant_cache,
            health: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(SystemRouterClock),
            failure_threshold: DEFAULT_CIRCUIT_FAILURE_THRESHOLD,
            open_cooldown: Duration::from_secs(DEFAULT_CIRCUIT_OPEN_SECS),
        }
    }

    /// Attaches the tenant [`LlmProviderStore`] persistence port.
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn LlmProviderStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Attaches a [`FlowCredentialUnsealPort`] to unseal `ax_llm_providers.credential_id` secrets.
    #[must_use]
    pub fn with_cred_unsealer(mut self, unsealer: Arc<dyn FlowCredentialUnsealPort>) -> Self {
        self.secret_resolver = Some(Arc::new(FlowCredentialSecretResolver::new(unsealer)));
        self
    }

    /// Attaches a custom [`ProviderSecretResolver`].
    #[must_use]
    pub fn with_secret_resolver(mut self, resolver: Arc<dyn ProviderSecretResolver>) -> Self {
        self.secret_resolver = Some(resolver);
        self
    }

    /// Attaches an SSRF [`OutboundUrlGuardPort`] checked prior to calling tenant-configured URLs.
    #[must_use]
    pub fn with_url_guard(mut self, url_guard: Arc<dyn OutboundUrlGuardPort>) -> Self {
        self.url_guard = Some(url_guard);
        self
    }

    /// Attaches fallback [`EmbeddingConfig`] for tenants without `Purpose::Embed` DB providers.
    #[must_use]
    pub fn with_default_embed(mut self, embed: EmbeddingConfig) -> Self {
        self.default_embed = Some(embed);
        self
    }

    /// Overrides the offline/recording [`Cassette`] instance.
    #[must_use]
    pub fn with_cassette(mut self, cassette: Option<Cassette>) -> Self {
        self.cassette = cassette;
        self
    }

    /// Overrides the shared [`reqwest::Client`].
    #[must_use]
    pub fn with_http_client(mut self, http: Client) -> Self {
        self.http = http;
        self
    }

    /// Overrides the [`RouterClock`] (used by deterministic tests).
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn RouterClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Configures circuit-breaker consecutive failure threshold and open cooldown duration.
    #[must_use]
    pub fn with_circuit_breaker(mut self, failure_threshold: u32, open_cooldown: Duration) -> Self {
        self.failure_threshold = failure_threshold.max(1);
        self.open_cooldown = open_cooldown;
        self
    }

    /// Evicts cached `ax_llm_providers` records for `tenant_id` after provider create/update/delete.
    pub async fn invalidate_tenant(&self, tenant_id: &str) {
        self.tenant_cache.invalidate(tenant_id).await;
    }

    /// Executes a two-message `(system, user)` chat completion routed by `purpose`.
    ///
    /// # Errors
    /// Returns [`AppError`] if candidate selection, cassette lookup, or all upstream providers fail.
    pub async fn complete_chat(
        &self,
        tenant_id: &str,
        purpose: Purpose,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, AppError> {
        let candidates = self
            .select_candidates(tenant_id, purpose, None, false)
            .await?;

        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Replay
        {
            let chosen = self.pick_replay_candidate(&candidates)?;
            let res = cassette.lookup_chat(system_prompt, user_prompt);
            if let Some(p) = chosen {
                self.record_usage_delta(tenant_id, p.id, purpose, i64::from(res.is_err()), 0, 0)
                    .await;
            }
            return res;
        }

        let req = RouterTurnRequest::new(
            purpose,
            vec![
                ToolChatMessage::system(system_prompt),
                ToolChatMessage::user(user_prompt),
            ],
        )
        .with_require_tools(false);

        let outcome = self
            .execute_turn_with_candidates(tenant_id, req, candidates, None)
            .await?;

        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Record
        {
            cassette.record_chat(system_prompt, user_prompt, &outcome.turn.content)?;
        }

        Ok(outcome.turn.content)
    }

    /// Executes a non-streaming tool-capable turn routed by `req.purpose` / `req.pinned_provider_id`.
    ///
    /// # Errors
    /// Returns [`AppError`] if candidate selection, cassette lookup, or all upstream providers fail.
    pub async fn complete_tool_turn(
        &self,
        tenant_id: &str,
        req: RouterTurnRequest,
    ) -> Result<ToolChatTurn, AppError> {
        let out = self.complete_tool_turn_routed(tenant_id, req).await?;
        Ok(out.turn)
    }

    /// Executes a non-streaming tool-capable turn and returns the selected provider metadata.
    ///
    /// # Errors
    /// Returns [`AppError`] if candidate selection, cassette lookup, or all upstream providers fail.
    pub async fn complete_tool_turn_routed(
        &self,
        tenant_id: &str,
        req: RouterTurnRequest,
    ) -> Result<RoutedTurnOutcome, AppError> {
        let need_tools =
            !req.tools.is_empty() || (req.purpose == Purpose::Agent && req.require_tools);
        let candidates = self
            .select_candidates(tenant_id, req.purpose, req.pinned_provider_id, need_tools)
            .await?;
        self.execute_turn_with_candidates(tenant_id, req, candidates, None)
            .await
    }

    /// Executes an `SSE`-streaming tool-capable turn, forwarding text deltas over `token_tx`.
    ///
    /// # Errors
    /// Returns [`AppError`] if candidate selection, cassette lookup, or all upstream providers fail.
    pub async fn stream_tool_turn(
        &self,
        tenant_id: &str,
        req: RouterTurnRequest,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<ToolChatTurn, AppError> {
        let out = self
            .stream_tool_turn_routed(tenant_id, req, token_tx)
            .await?;
        Ok(out.turn)
    }

    /// Executes an `SSE`-streaming tool-capable turn and returns the selected provider metadata.
    ///
    /// # Errors
    /// Returns [`AppError`] if candidate selection, cassette lookup, or all upstream providers fail.
    pub async fn stream_tool_turn_routed(
        &self,
        tenant_id: &str,
        req: RouterTurnRequest,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<RoutedTurnOutcome, AppError> {
        let need_tools =
            !req.tools.is_empty() || (req.purpose == Purpose::Agent && req.require_tools);
        let candidates = self
            .select_candidates(tenant_id, req.purpose, req.pinned_provider_id, need_tools)
            .await?;
        self.execute_turn_with_candidates(tenant_id, req, candidates, token_tx)
            .await
    }

    /// Generates batch embeddings routed via `Purpose::Embed` providers (or fallback `default_embed`).
    ///
    /// # Errors
    /// Returns [`AppError`] if all candidate embedding providers or fallback configuration fail.
    pub async fn embed(
        &self,
        tenant_id: &str,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, usize), AppError> {
        let candidates = self
            .select_candidates(tenant_id, Purpose::Embed, None, false)
            .await?;

        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Replay
        {
            return self
                .embed_offline_replay(tenant_id, &candidates, texts)
                .await;
        }

        if candidates.is_empty() {
            return self.embed_with_default(texts).await;
        }

        let mut last_err: Option<AppError> = None;
        let mut saw_rpm = false;
        let mut saw_open = false;

        for provider in &candidates {
            let now = self.clock.now();
            if self.is_circuit_open(provider.id, now) {
                saw_open = true;
                continue;
            }
            if !self.try_acquire_rpm(provider.id, provider.rpm_limit, now) {
                saw_rpm = true;
                continue;
            }
            match self.call_embed_provider(tenant_id, provider, texts).await {
                Ok(res) => {
                    self.record_provider_success(provider.id);
                    self.record_usage_delta(tenant_id, provider.id, Purpose::Embed, 0, 0, 0)
                        .await;
                    return Ok(res);
                }
                Err(fail) => {
                    if fail.counts_toward_breaker {
                        self.record_provider_failure(provider.id, self.clock.now());
                    }
                    self.record_usage_delta(tenant_id, provider.id, Purpose::Embed, 1, 0, 0)
                        .await;
                    last_err = Some(fail.error);
                }
            }
        }

        Err(exhausted_candidates_error(last_err, saw_rpm, saw_open))
    }

    /// Probes connectivity and latency of a single [`LlmProviderRecord`] (`POST /api/axagent/providers/{id}/test`).
    pub async fn probe_provider(
        &self,
        tenant_id: &str,
        provider: &LlmProviderRecord,
        api_key: Option<&str>,
    ) -> ProviderTestResponse {
        let started = self.clock.now();
        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Replay
        {
            let latency_ms = u64::try_from(
                self.clock
                    .now()
                    .saturating_duration_since(started)
                    .as_millis(),
            )
            .unwrap_or(u64::MAX);
            return ProviderTestResponse {
                ok: true,
                latency_ms,
                model: provider.model.clone(),
                reply: String::from("OK (cassette)"),
            };
        }

        let outcome = self.probe_provider_live(tenant_id, provider, api_key).await;
        let latency_ms = u64::try_from(
            self.clock
                .now()
                .saturating_duration_since(started)
                .as_millis(),
        )
        .unwrap_or(u64::MAX);

        match outcome {
            Ok(turn) => ProviderTestResponse {
                ok: true,
                latency_ms,
                model: turn.model_echo.unwrap_or_else(|| provider.model.clone()),
                reply: turn.content,
            },
            Err(err) => ProviderTestResponse {
                ok: false,
                latency_ms,
                model: provider.model.clone(),
                reply: err.client_message(),
            },
        }
    }

    async fn probe_provider_live(
        &self,
        tenant_id: &str,
        provider: &LlmProviderRecord,
        explicit_key: Option<&str>,
    ) -> Result<ToolChatTurn, AppError> {
        let url = format!(
            "{}/chat/completions",
            provider.base_url.trim_end_matches('/')
        );
        if let Some(guard) = &self.url_guard {
            let _validated = guard
                .check_outbound_url(&url, provider.flags.allow_private_network)
                .await?;
        }

        let resolved_key = match explicit_key {
            Some(k) if !k.is_empty() => Some(k.to_owned()),
            _ => self.resolve_provider_key(tenant_id, provider).await?,
        };

        let body = WireChatCompletionRequest {
            model: provider.model.clone(),
            messages: vec![ToolChatMessage::user("Reply with OK.").to_wire()],
            temperature: 0.0,
            max_tokens: Some(8),
            tools: Vec::new(),
            stream: None,
            stream_options: None,
        };

        let timeout_dur =
            Duration::from_secs(u64::try_from(provider.timeout_s.max(1)).unwrap_or(60));
        let mut req_builder = self.http.post(&url).json(&body);
        if let Some(key) = &resolved_key {
            req_builder = req_builder.bearer_auth(key);
        }

        let resp = tokio::time::timeout(timeout_dur, req_builder.send())
            .await
            .map_err(|_| AppError::Timeout("UPSTREAM_TIMEOUT"))?
            .map_err(|err| AppError::Llm(format!("provider probe failed: {err}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!("provider returned {status}: {text}")));
        }

        let wire: WireChatCompletionResponse = resp
            .json()
            .await
            .map_err(|err| AppError::Llm(format!("invalid probe JSON: {err}")))?;
        parse_chat_completion_response(wire)
    }

    async fn load_tenant_providers(
        &self,
        tenant_id: &str,
    ) -> Result<Arc<[LlmProviderRecord]>, AppError> {
        let Some(store) = &self.store else {
            return Ok(Arc::from([]));
        };
        let store_cloned = Arc::clone(store);
        let tenant_owned = tenant_id.to_owned();
        self.tenant_cache
            .try_get_with(tenant_id.to_owned(), async move {
                let mut rows = store_cloned.list_tenant_providers(&tenant_owned).await?;
                rows.sort_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then_with(|| a.created_at.cmp(&b.created_at))
                });
                Ok::<Arc<[LlmProviderRecord]>, AppError>(Arc::from(rows))
            })
            .await
            .map_err(|arc_err| AppError::Database(arc_err.to_string()))
    }

    async fn select_candidates(
        &self,
        tenant_id: &str,
        purpose: Purpose,
        pinned_id: Option<Uuid>,
        need_tools: bool,
    ) -> Result<Vec<LlmProviderRecord>, AppError> {
        let all = self.load_tenant_providers(tenant_id).await?;
        if let Some(pid) = pinned_id {
            let found = all
                .iter()
                .find(|p| p.id == pid)
                .ok_or(AppError::NotFound("PROVIDER_NOT_FOUND"))?;
            if !found.enabled {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "pinned LLM provider is disabled",
                ));
            }
            if need_tools && !found.flags.supports_tools {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "pinned LLM provider does not support tool calling",
                ));
            }
            return Ok(vec![found.clone()]);
        }

        let filtered = all
            .iter()
            .filter(|p| {
                p.enabled
                    && p.purposes.contains(&purpose)
                    && (!need_tools || p.flags.supports_tools)
            })
            .cloned()
            .collect();
        Ok(filtered)
    }

    fn pick_replay_candidate<'a>(
        &self,
        candidates: &'a [LlmProviderRecord],
    ) -> Result<Option<&'a LlmProviderRecord>, AppError> {
        if candidates.is_empty() {
            return Ok(None);
        }
        let mut saw_rpm = false;
        let mut saw_open = false;
        for p in candidates {
            let now = self.clock.now();
            if self.is_circuit_open(p.id, now) {
                saw_open = true;
                continue;
            }
            if !self.try_acquire_rpm(p.id, p.rpm_limit, now) {
                saw_rpm = true;
                continue;
            }
            return Ok(Some(p));
        }
        Err(exhausted_candidates_error(None, saw_rpm, saw_open))
    }

    async fn execute_turn_with_candidates(
        &self,
        tenant_id: &str,
        req: RouterTurnRequest,
        candidates: Vec<LlmProviderRecord>,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<RoutedTurnOutcome, AppError> {
        let transcript = canonical_request_transcript(req.purpose, &req.messages, &req.tools);

        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Replay
        {
            return self
                .replay_tool_turn(tenant_id, &req, &candidates, &transcript, token_tx)
                .await;
        }

        if candidates.is_empty() {
            let turn = self.call_default_llm(tenant_id, &req, token_tx).await?;
            self.maybe_record_cassette_turn(&transcript, &turn)?;
            return Ok(RoutedTurnOutcome {
                turn,
                provider_id: None,
                provider_name: String::from("default"),
            });
        }

        let mut last_err: Option<AppError> = None;
        let mut saw_rpm = false;
        let mut saw_open = false;

        for provider in &candidates {
            let now = self.clock.now();
            if self.is_circuit_open(provider.id, now) {
                saw_open = true;
                continue;
            }
            if !self.try_acquire_rpm(provider.id, provider.rpm_limit, now) {
                saw_rpm = true;
                continue;
            }
            match self
                .call_provider_turn(tenant_id, provider, &req, token_tx.clone())
                .await
            {
                Ok(turn) => {
                    self.record_provider_success(provider.id);
                    Self::emit_token_metrics(tenant_id, &provider.name, &turn);
                    self.record_usage_delta(
                        tenant_id,
                        provider.id,
                        req.purpose,
                        0,
                        turn.usage.prompt_tokens.max(0),
                        turn.usage.completion_tokens.max(0),
                    )
                    .await;
                    self.maybe_record_cassette_turn(&transcript, &turn)?;
                    return Ok(RoutedTurnOutcome {
                        turn,
                        provider_id: Some(provider.id),
                        provider_name: provider.name.clone(),
                    });
                }
                Err(fail) => {
                    if fail.counts_toward_breaker {
                        self.record_provider_failure(provider.id, self.clock.now());
                    }
                    self.record_usage_delta(tenant_id, provider.id, req.purpose, 1, 0, 0)
                        .await;
                    last_err = Some(fail.error);
                }
            }
        }

        Err(exhausted_candidates_error(last_err, saw_rpm, saw_open))
    }

    async fn replay_tool_turn(
        &self,
        tenant_id: &str,
        req: &RouterTurnRequest,
        candidates: &[LlmProviderRecord],
        transcript: &str,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<RoutedTurnOutcome, AppError> {
        let Some(cassette) = &self.cassette else {
            return Err(AppError::Llm(String::from("missing cassette")));
        };
        let chosen = self.pick_replay_candidate(candidates)?;
        let (sys, last_user) = extract_system_and_last_user(&req.messages);
        let lookup = cassette.lookup_tool_turn(transcript, &sys, &last_user);

        if let Some(p) = chosen {
            self.record_usage_delta(
                tenant_id,
                p.id,
                req.purpose,
                i64::from(lookup.is_err()),
                0,
                0,
            )
            .await;
        }

        let ct = lookup?;
        if let Some(tx) = token_tx
            && !ct.content.is_empty()
            && tx.send(ct.content.clone()).await.is_err()
        {
            warn!("SSE token receiver dropped during cassette replay");
        }

        Ok(RoutedTurnOutcome {
            turn: ToolChatTurn {
                content: ct.content,
                tool_calls: ct.tool_calls,
                usage: TokenUsage::default(),
                model_echo: Some(
                    chosen.map_or_else(|| self.default_llm.model.clone(), |p| p.model.clone()),
                ),
            },
            provider_id: chosen.map(|p| p.id),
            provider_name: chosen.map_or_else(|| String::from("default"), |p| p.name.clone()),
        })
    }

    fn maybe_record_cassette_turn(
        &self,
        transcript: &str,
        turn: &ToolChatTurn,
    ) -> Result<(), AppError> {
        if let Some(cassette) = &self.cassette
            && cassette.mode() == CassetteMode::Record
        {
            let ct = CassetteTurn {
                content: turn.content.clone(),
                tool_calls: turn.tool_calls.clone(),
            };
            cassette.record_tool_turn(transcript, &ct)?;
        }
        Ok(())
    }

    async fn resolve_provider_key(
        &self,
        tenant_id: &str,
        provider: &LlmProviderRecord,
    ) -> Result<Option<String>, AppError> {
        if !provider.has_api_key && provider.credential_id.is_none() {
            return Ok(None);
        }
        let Some(resolver) = &self.secret_resolver else {
            return Ok(None);
        };
        resolver.resolve_api_key(tenant_id, provider).await
    }

    async fn call_provider_turn(
        &self,
        tenant_id: &str,
        provider: &LlmProviderRecord,
        req: &RouterTurnRequest,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<ToolChatTurn, ProviderCallFailure> {
        let url = format!(
            "{}/chat/completions",
            provider.base_url.trim_end_matches('/')
        );
        if let Some(guard) = &self.url_guard
            && let Err(err) = guard
                .check_outbound_url(&url, provider.flags.allow_private_network)
                .await
        {
            return Err(ProviderCallFailure {
                error: err,
                counts_toward_breaker: false,
            });
        }

        let api_key = self
            .resolve_provider_key(tenant_id, provider)
            .await
            .map_err(|error| ProviderCallFailure {
                error,
                counts_toward_breaker: false,
            })?;

        let (wire_msgs, egress_session) = if provider.flags.allow_pii {
            (
                req.messages.iter().map(ToolChatMessage::to_wire).collect(),
                EgressSession::new(),
            )
        } else {
            let (redacted, sess) = redact_messages_for_egress(&req.messages);
            (
                redacted.iter().map(ToolChatMessage::to_wire).collect(),
                sess,
            )
        };
        let tenant_key = resolve_tenant_mask_key(tenant_id);
        let (eff_tx, stream_join) = wrap_egress_stream_tx(
            token_tx,
            &egress_session,
            req.caller_can_read_raw_pii,
            tenant_key,
        );
        let is_stream = eff_tx.is_some();
        let body = WireChatCompletionRequest {
            model: provider.model.clone(),
            messages: wire_msgs,
            temperature: req.temperature.unwrap_or(provider.temperature),
            max_tokens: Some(req.max_tokens.unwrap_or(provider.max_tokens)),
            tools: req.tools.clone(),
            stream: is_stream.then_some(true),
            stream_options: is_stream.then_some(WireStreamOptions {
                include_usage: true,
            }),
        };

        let timeout_dur =
            Duration::from_secs(u64::try_from(provider.timeout_s.max(1)).unwrap_or(60));
        let res = self
            .send_completion_http(&url, api_key.as_deref(), &body, timeout_dur, eff_tx)
            .await;
        if let Some(jh) = stream_join
            && let Err(err) = jh.await
        {
            warn!(error = %err, "egress stream forwarder join failed");
        }
        let mut turn = res?;
        if !provider.flags.allow_pii {
            restore_turn_from_egress(
                &mut turn,
                &egress_session,
                req.caller_can_read_raw_pii,
                &tenant_key,
            );
        }
        Ok(turn)
    }

    async fn call_default_llm(
        &self,
        tenant_id: &str,
        req: &RouterTurnRequest,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<ToolChatTurn, AppError> {
        let url = format!(
            "{}/chat/completions",
            self.default_llm.api_base.trim_end_matches('/')
        );
        let (redacted, egress_session) = redact_messages_for_egress(&req.messages);
        let tenant_key = resolve_tenant_mask_key(tenant_id);
        let (eff_tx, stream_join) = wrap_egress_stream_tx(
            token_tx,
            &egress_session,
            req.caller_can_read_raw_pii,
            tenant_key,
        );
        let is_stream = eff_tx.is_some();
        let body = WireChatCompletionRequest {
            model: self.default_llm.model.clone(),
            messages: redacted.iter().map(ToolChatMessage::to_wire).collect(),
            temperature: req.temperature.unwrap_or(0.2),
            max_tokens: req.max_tokens,
            tools: req.tools.clone(),
            stream: is_stream.then_some(true),
            stream_options: is_stream.then_some(WireStreamOptions {
                include_usage: true,
            }),
        };
        let key =
            (!self.default_llm.api_key.is_empty()).then_some(self.default_llm.api_key.as_str());
        let res = self
            .send_completion_http(&url, key, &body, Duration::from_secs(60), eff_tx)
            .await;
        if let Some(jh) = stream_join
            && let Err(err) = jh.await
        {
            warn!(error = %err, "default llm egress stream forwarder join failed");
        }
        let mut turn = res.map_err(|f| f.error)?;
        restore_turn_from_egress(
            &mut turn,
            &egress_session,
            req.caller_can_read_raw_pii,
            &tenant_key,
        );
        Ok(turn)
    }

    async fn send_completion_http(
        &self,
        url: &str,
        api_key: Option<&str>,
        body: &WireChatCompletionRequest,
        timeout_dur: Duration,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> Result<ToolChatTurn, ProviderCallFailure> {
        let mut builder = self.http.post(url).json(body);
        if let Some(k) = api_key
            && !k.is_empty()
        {
            builder = builder.bearer_auth(k);
        }

        let send_res = tokio::time::timeout(timeout_dur, builder.send()).await;
        let resp = match send_res {
            Err(_) => {
                return Err(ProviderCallFailure {
                    error: AppError::Timeout("UPSTREAM_TIMEOUT"),
                    counts_toward_breaker: true,
                });
            }
            Ok(Err(err)) => {
                return Err(ProviderCallFailure {
                    error: AppError::Llm(format!("chat request failed: {err}")),
                    counts_toward_breaker: true,
                });
            }
            Ok(Ok(r)) => r,
        };

        let status = resp.status();
        if !status.is_success() {
            let counts = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderCallFailure {
                error: AppError::Llm(format!("chat API returned {status}: {text}")),
                counts_toward_breaker: counts,
            });
        }

        if token_tx.is_some() {
            return tokio::time::timeout(timeout_dur, consume_sse_response(resp, token_tx))
                .await
                .map_err(|_| ProviderCallFailure {
                    error: AppError::Timeout("UPSTREAM_TIMEOUT"),
                    counts_toward_breaker: true,
                })?
                .map_err(|error| ProviderCallFailure {
                    error,
                    counts_toward_breaker: true,
                });
        }

        let wire: WireChatCompletionResponse =
            resp.json().await.map_err(|err| ProviderCallFailure {
                error: AppError::Llm(format!("invalid chat JSON: {err}")),
                counts_toward_breaker: true,
            })?;
        parse_chat_completion_response(wire).map_err(|error| ProviderCallFailure {
            error,
            counts_toward_breaker: true,
        })
    }

    async fn embed_offline_replay(
        &self,
        tenant_id: &str,
        candidates: &[LlmProviderRecord],
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, usize), AppError> {
        let chosen = self.pick_replay_candidate(candidates)?;
        let dim = chosen
            .and_then(|p| p.embedding_dim.and_then(|d| usize::try_from(d).ok()))
            .or_else(|| self.default_embed.as_ref().and_then(|e| e.configured_dim))
            .unwrap_or(DEFAULT_OFFLINE_EMBED_DIM);
        if dim == 0 {
            return Err(AppError::Llm(String::from(
                "embedding configuration returned zero-dimension vector",
            )));
        }
        if let Some(p) = chosen {
            self.record_usage_delta(tenant_id, p.id, Purpose::Embed, 0, 0, 0)
                .await;
        }
        let vectors = texts
            .iter()
            .map(|t| deterministic_embedding(t, dim))
            .collect();
        Ok((vectors, dim))
    }

    async fn embed_with_default(
        &self,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, usize), AppError> {
        let Some(cfg) = &self.default_embed else {
            return Err(AppError::Llm(String::from(
                "no embedding provider configured for tenant",
            )));
        };
        if texts.is_empty() {
            return Ok((Vec::new(), cfg.configured_dim.unwrap_or(0)));
        }
        let url = format!("{}/embeddings", cfg.api_base.trim_end_matches('/'));
        let key = (!cfg.api_key.is_empty()).then_some(cfg.api_key.as_str());
        self.fetch_embeddings_batched(&url, &cfg.model, key, Duration::from_secs(60), texts)
            .await
            .map_err(|f| f.error)
    }

    async fn call_embed_provider(
        &self,
        tenant_id: &str,
        provider: &LlmProviderRecord,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, usize), ProviderCallFailure> {
        if texts.is_empty() {
            let dim = provider
                .embedding_dim
                .and_then(|d| usize::try_from(d).ok())
                .unwrap_or(0);
            return Ok((Vec::new(), dim));
        }
        let url = format!("{}/embeddings", provider.base_url.trim_end_matches('/'));
        if let Some(guard) = &self.url_guard
            && let Err(error) = guard
                .check_outbound_url(&url, provider.flags.allow_private_network)
                .await
        {
            return Err(ProviderCallFailure {
                error,
                counts_toward_breaker: false,
            });
        }
        let key = self
            .resolve_provider_key(tenant_id, provider)
            .await
            .map_err(|error| ProviderCallFailure {
                error,
                counts_toward_breaker: false,
            })?;
        let timeout_dur =
            Duration::from_secs(u64::try_from(provider.timeout_s.max(1)).unwrap_or(60));
        self.fetch_embeddings_batched(&url, &provider.model, key.as_deref(), timeout_dur, texts)
            .await
    }

    async fn fetch_embeddings_batched(
        &self,
        url: &str,
        model: &str,
        api_key: Option<&str>,
        timeout_dur: Duration,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, usize), ProviderCallFailure> {
        let mut all_vectors = Vec::with_capacity(texts.len());
        let mut detected_dim: Option<usize> = None;

        for chunk in texts.chunks(EMBED_BATCH_CHUNK_SIZE) {
            let body = EmbedWireReq {
                model,
                input: chunk,
            };
            let mut builder = self.http.post(url).json(&body);
            if let Some(k) = api_key
                && !k.is_empty()
            {
                builder = builder.bearer_auth(k);
            }
            let resp = tokio::time::timeout(timeout_dur, builder.send())
                .await
                .map_err(|_| ProviderCallFailure {
                    error: AppError::Timeout("UPSTREAM_TIMEOUT"),
                    counts_toward_breaker: true,
                })?
                .map_err(|err| ProviderCallFailure {
                    error: AppError::Llm(format!("embedding request failed: {err}")),
                    counts_toward_breaker: true,
                })?;
            let status = resp.status();
            if !status.is_success() {
                let counts = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                let raw = resp.text().await.unwrap_or_default();
                return Err(ProviderCallFailure {
                    error: AppError::Llm(format!("embedding API returned {status}: {raw}")),
                    counts_toward_breaker: counts,
                });
            }
            let mut parsed: EmbedWireResp =
                resp.json().await.map_err(|err| ProviderCallFailure {
                    error: AppError::Llm(format!("invalid embedding JSON: {err}")),
                    counts_toward_breaker: true,
                })?;
            if parsed.data.iter().all(|i| i.index.is_some()) {
                parsed.data.sort_by_key(|i| i.index.unwrap_or(0));
            }
            if parsed.data.len() != chunk.len() {
                return Err(ProviderCallFailure {
                    error: AppError::Llm(String::from("embedding vector count mismatch")),
                    counts_toward_breaker: true,
                });
            }
            for item in parsed.data {
                let vlen = item.embedding.len();
                if vlen == 0 || detected_dim.is_some_and(|d| d != vlen) {
                    return Err(ProviderCallFailure {
                        error: AppError::Llm(String::from("invalid or inconsistent embedding dim")),
                        counts_toward_breaker: true,
                    });
                }
                detected_dim = Some(vlen);
                all_vectors.push(item.embedding);
            }
        }

        let dim = detected_dim.ok_or_else(|| ProviderCallFailure {
            error: AppError::Llm(String::from("empty embedding result")),
            counts_toward_breaker: true,
        })?;
        Ok((all_vectors, dim))
    }

    fn is_circuit_open(&self, provider_id: Uuid, now: Instant) -> bool {
        let mut guard = self.health.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = guard.get_mut(&provider_id) else {
            return false;
        };
        let Some(until) = entry.open_until else {
            return false;
        };
        if now < until {
            true
        } else {
            entry.open_until = None;
            false
        }
    }

    fn try_acquire_rpm(&self, provider_id: Uuid, rpm_limit: Option<i32>, now: Instant) -> bool {
        let Some(limit) = rpm_limit else {
            return true;
        };
        if limit <= 0 {
            return false;
        }
        let capacity = f64::from(limit);
        let rate_per_sec = capacity / 60.0;

        let mut guard = self.health.lock().unwrap_or_else(PoisonError::into_inner);
        let state = guard
            .entry(provider_id)
            .or_insert_with(|| ProviderHealthState::new(now));
        let current = match state.rpm_tokens {
            Some(existing) => {
                let elapsed = now
                    .saturating_duration_since(state.last_refill)
                    .as_secs_f64();
                (existing + elapsed * rate_per_sec).min(capacity)
            }
            None => capacity,
        };
        state.last_refill = now;
        if current < 1.0 {
            state.rpm_tokens = Some(current);
            return false;
        }
        state.rpm_tokens = Some(current - 1.0);
        true
    }

    fn record_provider_success(&self, provider_id: Uuid) {
        let now = self.clock.now();
        let mut guard = self.health.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard
            .entry(provider_id)
            .or_insert_with(|| ProviderHealthState::new(now));
        entry.consecutive_failures = 0;
        entry.open_until = None;
    }

    fn record_provider_failure(&self, provider_id: Uuid, now: Instant) {
        let mut guard = self.health.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard
            .entry(provider_id)
            .or_insert_with(|| ProviderHealthState::new(now));
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        if entry.consecutive_failures >= self.failure_threshold {
            entry.open_until = now.checked_add(self.open_cooldown);
        }
    }

    fn emit_token_metrics(tenant_id: &str, provider_name: &str, turn: &ToolChatTurn) {
        let prompt_u64 = u64::try_from(turn.usage.prompt_tokens.max(0)).unwrap_or(0);
        let completion_u64 = u64::try_from(turn.usage.completion_tokens.max(0)).unwrap_or(0);
        metrics::counter!(
            "axpoint_llm_tokens_total",
            "tenant" => tenant_id.to_owned(),
            "provider" => provider_name.to_owned(),
            "kind" => "prompt"
        )
        .increment(prompt_u64);
        metrics::counter!(
            "axpoint_llm_tokens_total",
            "tenant" => tenant_id.to_owned(),
            "provider" => provider_name.to_owned(),
            "kind" => "completion"
        )
        .increment(completion_u64);
    }

    async fn record_usage_delta(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
        purpose: Purpose,
        errors: i64,
        prompt_tokens: i64,
        completion_tokens: i64,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let delta = LlmUsageDelta {
            provider_id,
            usage_date: Utc::now().date_naive(),
            purpose,
            requests: 1,
            errors,
            prompt_tokens,
            completion_tokens,
        };
        if let Err(err) = store.record_usage(tenant_id, delta).await {
            warn!(error = %err, %tenant_id, %provider_id, "failed to persist ax_llm_usage delta");
        }
    }
}

fn exhausted_candidates_error(
    last_err: Option<AppError>,
    saw_rpm: bool,
    saw_open: bool,
) -> AppError {
    if let Some(err) = last_err {
        return err;
    }
    if saw_rpm && !saw_open {
        return AppError::RateLimited;
    }
    if saw_open {
        return AppError::Llm(String::from(
            "all matching LLM providers currently have open circuit breakers",
        ));
    }
    AppError::Llm(String::from("no available LLM provider"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axpoint_domain::agent::LlmProviderFlags;
    use chrono::TimeZone as _;
    use serde_json::json;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Debug)]
    struct ManualClock {
        current: Mutex<Instant>,
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                current: Mutex::new(Instant::now()),
            }
        }

        fn advance(&self, delta: Duration) {
            let mut guard = self.current.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(next) = guard.checked_add(delta) {
                *guard = next;
            }
        }
    }

    impl RouterClock for ManualClock {
        fn now(&self) -> Instant {
            *self.current.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    #[derive(Default)]
    struct MockProviderStore {
        providers: Mutex<HashMap<String, Vec<LlmProviderRecord>>>,
        usages: Mutex<Vec<(String, LlmUsageDelta)>>,
    }

    impl MockProviderStore {
        fn set_providers(&self, tenant_id: &str, list: Vec<LlmProviderRecord>) {
            let mut guard = self
                .providers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.insert(tenant_id.to_owned(), list);
        }

        fn take_usages(&self) -> Vec<(String, LlmUsageDelta)> {
            let guard = self.usages.lock().unwrap_or_else(PoisonError::into_inner);
            guard.clone()
        }
    }

    impl LlmProviderStore for MockProviderStore {
        fn list_tenant_providers<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> BoxFuture<'a, Vec<LlmProviderRecord>> {
            Box::pin(async move {
                let guard = self
                    .providers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                Ok(guard.get(tenant_id).cloned().unwrap_or_default())
            })
        }

        fn record_usage<'a>(
            &'a self,
            tenant_id: &'a str,
            delta: LlmUsageDelta,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                let mut guard = self.usages.lock().unwrap_or_else(PoisonError::into_inner);
                guard.push((tenant_id.to_owned(), delta));
                Ok(())
            })
        }
    }

    async fn spawn_mock_llm_server(
        status_code: u16,
        reply_text: &'static str,
        hits: Arc<AtomicUsize>,
    ) -> Result<String, AppError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                hits.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 4096];
                if sock.read(&mut buf).await.is_err() {
                    continue;
                }
                let body = if status_code == 200 {
                    json!({
                        "model": reply_text,
                        "choices": [{ "message": { "role": "assistant", "content": reply_text } }],
                        "usage": { "prompt_tokens": 12, "completion_tokens": 6 }
                    })
                    .to_string()
                } else {
                    String::from("{\"error\":\"upstream failure\"}")
                };
                let status_line = match status_code {
                    200 => "HTTP/1.1 200 OK",
                    429 => "HTTP/1.1 429 Too Many Requests",
                    _ => "HTTP/1.1 500 Internal Server Error",
                };
                let http_resp = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _wrote = sock.write_all(http_resp.as_bytes()).await.is_ok();
            }
        });

        Ok(format!("http://{addr}"))
    }

    fn make_provider(
        name: &str,
        base_url: String,
        purposes: Vec<Purpose>,
        priority: i32,
        supports_tools: bool,
        rpm_limit: Option<i32>,
    ) -> LlmProviderRecord {
        LlmProviderRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from("t_router"),
            name: name.to_owned(),
            base_url,
            model: format!("model-{name}"),
            purposes,
            has_api_key: false,
            credential_id: None,
            priority,
            temperature: 0.2,
            max_tokens: 512,
            timeout_s: 5,
            rpm_limit,
            embedding_dim: None,
            flags: LlmProviderFlags {
                supports_tools,
                allow_private_network: true,
                allow_pii: false,
            },
            enabled: true,
            created_at: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .unwrap_or_else(Utc::now),
        }
    }

    fn dead_default_llm() -> LlmConfig {
        LlmConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("dead-default"),
        }
    }

    #[tokio::test]
    async fn router_selects_lowest_priority_and_filters_purpose_and_supports_tools()
    -> Result<(), AppError> {
        let hits_p10 = Arc::new(AtomicUsize::new(0));
        let hits_p50 = Arc::new(AtomicUsize::new(0));
        let hits_no_tools = Arc::new(AtomicUsize::new(0));

        let url_p10 = spawn_mock_llm_server(200, "from_p10", Arc::clone(&hits_p10)).await?;
        let url_p50 = spawn_mock_llm_server(200, "from_p50", Arc::clone(&hits_p50)).await?;
        let url_nt =
            spawn_mock_llm_server(200, "from_no_tools", Arc::clone(&hits_no_tools)).await?;

        let store = Arc::new(MockProviderStore::default());
        store.set_providers(
            "t_router",
            vec![
                make_provider(
                    "p50",
                    url_p50,
                    vec![Purpose::Chat, Purpose::Agent],
                    50,
                    true,
                    None,
                ),
                make_provider("no_tools_p1", url_nt, vec![Purpose::Agent], 1, false, None),
                make_provider("p10", url_p10, vec![Purpose::Chat], 10, true, None),
            ],
        );

        let router = LlmRouter::new(dead_default_llm())
            .with_store(store)
            .with_cassette(None);

        let chat_reply = router
            .complete_chat("t_router", Purpose::Chat, "sys", "hello")
            .await?;
        assert_eq!(chat_reply, "from_p10");
        assert_eq!(hits_p10.load(Ordering::SeqCst), 1);
        assert_eq!(hits_p50.load(Ordering::SeqCst), 0);

        let agent_req =
            RouterTurnRequest::new(Purpose::Agent, vec![ToolChatMessage::user("run agent")]);
        let agent_turn = router.complete_tool_turn("t_router", agent_req).await?;
        assert_eq!(agent_turn.content, "from_p50");
        assert_eq!(hits_no_tools.load(Ordering::SeqCst), 0);
        assert_eq!(hits_p50.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn router_fails_over_on_500_and_429_and_records_error_usage() -> Result<(), AppError> {
        let hits_primary = Arc::new(AtomicUsize::new(0));
        let hits_rate = Arc::new(AtomicUsize::new(0));
        let hits_backup = Arc::new(AtomicUsize::new(0));

        let url_500 = spawn_mock_llm_server(500, "err500", Arc::clone(&hits_primary)).await?;
        let url_429 = spawn_mock_llm_server(429, "err429", Arc::clone(&hits_rate)).await?;
        let url_ok = spawn_mock_llm_server(200, "backup_ok", Arc::clone(&hits_backup)).await?;

        let p_500 = make_provider("primary_500", url_500, vec![Purpose::Chat], 10, true, None);
        let p_429 = make_provider(
            "secondary_429",
            url_429,
            vec![Purpose::Chat],
            20,
            true,
            None,
        );
        let p_ok = make_provider("backup_200", url_ok, vec![Purpose::Chat], 30, true, None);
        let id_500 = p_500.id;
        let id_429 = p_429.id;
        let id_ok = p_ok.id;

        let store = Arc::new(MockProviderStore::default());
        store.set_providers("t_router", vec![p_500, p_429, p_ok]);

        let store_dyn: Arc<dyn LlmProviderStore> = store.clone();
        let router = LlmRouter::new(dead_default_llm())
            .with_store(store_dyn)
            .with_cassette(None);

        let reply = router
            .complete_chat("t_router", Purpose::Chat, "sys", "hi")
            .await?;
        assert_eq!(reply, "backup_ok");

        let usages = store.take_usages();
        assert_eq!(usages.len(), 3);
        assert_eq!(
            usages.first().map(|u| (u.1.provider_id, u.1.errors)),
            Some((id_500, 1))
        );
        assert_eq!(
            usages.get(1).map(|u| (u.1.provider_id, u.1.errors)),
            Some((id_429, 1))
        );
        assert_eq!(
            usages.get(2).map(|u| (
                u.1.provider_id,
                u.1.errors,
                u.1.prompt_tokens,
                u.1.completion_tokens
            )),
            Some((id_ok, 0, 12, 6))
        );
        Ok(())
    }

    #[tokio::test]
    async fn router_circuit_breaker_opens_after_5_failures_and_recovers_after_cooldown()
    -> Result<(), AppError> {
        let hits_primary = Arc::new(AtomicUsize::new(0));
        let hits_backup = Arc::new(AtomicUsize::new(0));

        let url_fail = spawn_mock_llm_server(500, "boom", Arc::clone(&hits_primary)).await?;
        let url_ok = spawn_mock_llm_server(200, "backup_alive", Arc::clone(&hits_backup)).await?;

        let store = Arc::new(MockProviderStore::default());
        store.set_providers(
            "t_router",
            vec![
                make_provider("primary", url_fail, vec![Purpose::Chat], 10, true, None),
                make_provider("backup", url_ok, vec![Purpose::Chat], 20, true, None),
            ],
        );

        let clock = Arc::new(ManualClock::new());
        let clock_dyn: Arc<dyn RouterClock> = clock.clone();
        let router = LlmRouter::new(dead_default_llm())
            .with_store(store)
            .with_cassette(None)
            .with_clock(clock_dyn)
            .with_circuit_breaker(5, Duration::from_secs(60));

        for _ in 0..5 {
            let out = router
                .complete_chat("t_router", Purpose::Chat, "sys", "ping")
                .await?;
            assert_eq!(out, "backup_alive");
        }
        assert_eq!(hits_primary.load(Ordering::SeqCst), 5);

        let out_open = router
            .complete_chat("t_router", Purpose::Chat, "sys", "while_open")
            .await?;
        assert_eq!(out_open, "backup_alive");
        assert_eq!(hits_primary.load(Ordering::SeqCst), 5);

        clock.advance(Duration::from_secs(61));
        let out_probe = router
            .complete_chat("t_router", Purpose::Chat, "sys", "after_cooldown")
            .await?;
        assert_eq!(out_probe, "backup_alive");
        assert_eq!(hits_primary.load(Ordering::SeqCst), 6);
        Ok(())
    }

    #[tokio::test]
    async fn router_rpm_limit_skips_throttled_primary_and_probes_provider() -> Result<(), AppError>
    {
        let hits_primary = Arc::new(AtomicUsize::new(0));
        let hits_secondary = Arc::new(AtomicUsize::new(0));

        let url_p1 = spawn_mock_llm_server(200, "primary_rpm2", Arc::clone(&hits_primary)).await?;
        let url_p2 =
            spawn_mock_llm_server(200, "secondary_unlim", Arc::clone(&hits_secondary)).await?;

        let p1 = make_provider("p1", url_p1, vec![Purpose::Chat], 10, true, Some(2));
        let p2 = make_provider("p2", url_p2, vec![Purpose::Chat], 20, true, None);
        let probe_target = p1.clone();

        let store = Arc::new(MockProviderStore::default());
        store.set_providers("t_router", vec![p1, p2]);

        let clock = Arc::new(ManualClock::new());
        let router = LlmRouter::new(dead_default_llm())
            .with_store(store)
            .with_cassette(None)
            .with_clock(clock);

        let r1 = router
            .complete_chat("t_router", Purpose::Chat, "s", "1")
            .await?;
        let r2 = router
            .complete_chat("t_router", Purpose::Chat, "s", "2")
            .await?;
        let r3 = router
            .complete_chat("t_router", Purpose::Chat, "s", "3")
            .await?;

        assert_eq!(r1, "primary_rpm2");
        assert_eq!(r2, "primary_rpm2");
        assert_eq!(r3, "secondary_unlim");
        assert_eq!(hits_primary.load(Ordering::SeqCst), 2);
        assert_eq!(hits_secondary.load(Ordering::SeqCst), 1);

        let probe = router.probe_provider("t_router", &probe_target, None).await;
        assert!(probe.ok);
        assert_eq!(probe.model, "primary_rpm2");
        assert_eq!(probe.reply, "primary_rpm2");
        Ok(())
    }

    #[tokio::test]
    async fn router_falls_back_to_default_llm_when_no_tenant_provider_matches()
    -> Result<(), AppError> {
        let hits_default = Arc::new(AtomicUsize::new(0));
        let default_url =
            spawn_mock_llm_server(200, "env_default_ok", Arc::clone(&hits_default)).await?;

        let store = Arc::new(MockProviderStore::default());
        let router = LlmRouter::new(LlmConfig {
            api_base: default_url,
            api_key: String::new(),
            model: String::from("env-default-model"),
        })
        .with_store(store)
        .with_cassette(None);

        let reply = router
            .complete_chat("t_empty_tenant", Purpose::Nl2Sql, "sys", "select 1")
            .await?;
        assert_eq!(reply, "env_default_ok");
        assert_eq!(hits_default.load(Ordering::SeqCst), 1);
        Ok(())
    }

    async fn spawn_capturing_llm_server(
        reply_text: &'static str,
        captured: Arc<Mutex<Vec<String>>>,
    ) -> Result<String, AppError> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Config(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| AppError::Config(e.to_string()))?;

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut raw_buf = vec![0_u8; 16_384];
                let n = sock.read(&mut raw_buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(raw_buf.get(..n).unwrap_or(&[])).into_owned();
                if let Ok(mut guard) = captured.lock() {
                    guard.push(req_str);
                }
                let body = serde_json::json!({
                    "model": "mock-egress",
                    "choices": [{ "message": { "role": "assistant", "content": reply_text } }],
                    "usage": { "prompt_tokens": 10, "completion_tokens": 8 }
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                if sock.write_all(resp.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        Ok(format!("http://{addr}"))
    }

    #[tokio::test]
    async fn step13_router_strips_pii_on_outbound_and_restores_masked_or_raw_inbound()
    -> Result<(), AppError> {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_capturing_llm_server(
            "请联系 ⟨PHONE_1⟩ 并核验身份证 ⟨ID_CARD_1⟩",
            Arc::clone(&captured),
        )
        .await?;

        let store = Arc::new(MockProviderStore::default());
        store.set_providers(
            "t_router",
            vec![make_provider(
                "strict_ext",
                url,
                vec![Purpose::Chat],
                10,
                true,
                None,
            )],
        );
        let router = LlmRouter::new(dead_default_llm())
            .with_store(store)
            .with_cassette(None);

        let msgs = vec![
            ToolChatMessage::system("系统提示保留参考号 13900001111"),
            ToolChatMessage::user("客户手机 13812345678，身份证 110101199003077516"),
        ];

        // 1. Caller without PII_RAW_READ gets masked values; outbound HTTP never contains raw user PII
        let masked_turn = router
            .complete_tool_turn(
                "t_router",
                RouterTurnRequest::new(Purpose::Chat, msgs.clone()).with_raw_pii(false),
            )
            .await?;
        assert_eq!(
            masked_turn.content,
            "请联系 138****5678 并核验身份证 110101********7516"
        );

        // 2. Caller with PII_RAW_READ gets restored plaintext
        let raw_turn = router
            .complete_tool_turn(
                "t_router",
                RouterTurnRequest::new(Purpose::Chat, msgs).with_raw_pii(true),
            )
            .await?;
        assert_eq!(
            raw_turn.content,
            "请联系 13812345678 并核验身份证 110101199003077516"
        );

        let payloads = captured
            .lock()
            .map_err(|_| AppError::Config(String::from("lock poisoned")))?
            .clone();
        assert_eq!(payloads.len(), 2);
        for req_payload in &payloads {
            assert!(!req_payload.contains("13812345678"));
            assert!(!req_payload.contains("110101199003077516"));
            assert!(req_payload.contains("PHONE_1"));
            assert!(req_payload.contains("ID_CARD_1"));
            // System prompt is NOT redacted per §13.8.3
            assert!(req_payload.contains("13900001111"));
        }

        let preview =
            preview_egress_filter("手机号 13812345678 身份证 110101199003077516", "t_router");
        assert!(preview.outbound.contains("⟨PHONE_1⟩"));
        assert!(preview.outbound.contains("⟨ID_CARD_1⟩"));
        assert!(preview.masked.contains("138****5678"));
        assert!(preview.restored.contains("13812345678"));
        Ok(())
    }
}
