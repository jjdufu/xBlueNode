//! Capability publishing engine (`CapabilityPublisher`), Draft 2020-12 schema derivation,
//! `PROD` target pinning, `moka` TTL caching, and rate-limited execution (`08-STEP09` §09.3).

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, Weak};
use std::time::{Duration, Instant};

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentChatResponse, AgentEvent, AgentSpec, CreateEndpointRequest,
    EndpointListQuery, EndpointSpec, SaveEndpointInput, SkillManifest, UpdateEndpointRequest,
};
use axpoint_domain::analytics::{
    AnalyticsPrincipal, MetricDef, MetricQuery, QueryParamSpec, SavedQuery,
};
use axpoint_domain::enums::{
    AuditAction, ConversationChannel, DataType, EndpointKind, Environment, OutputShape, Permission,
    RunStatus, TokenKind, TriggerType,
};
use axpoint_domain::flow::{RunRecord, StartRunParams};
use axpoint_domain::platform::{AuditRecord, BoxFuture};
use axpoint_domain::ports::{
    AgentCapabilityPort, AgentRepository, AnalyticsRepository, FlowRepository, StudioRepository,
};
use axpoint_domain::studio::{NodeTypeSpec, PropertyKind, SchemaDoc};
use axpoint_security::{AuditWriter, AuthPrincipal};
use chrono::Utc;
use moka::Expiry;
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::openapi::{build_openapi_spec, build_skill_manifest};
use crate::rate_limit::KeyedRateLimiter;

const DRAFT_2020_12_URI: &str = "https://json-schema.org/draft/2020-12/schema";

/// Invocation parameters passed from [`CapabilityPublisher`] to [`PublishedAgentInvoker`].
#[derive(Debug, Clone)]
pub struct PublishedAgentCall {
    /// Target `ax_agents.id`.
    pub agent_id: Uuid,
    /// Optional `PROD`-pinned version number.
    pub pinned_version: Option<i32>,
    /// Optional `PROD`-pinned [`AgentSpec`] snapshot.
    pub pinned_spec: Option<AgentSpec>,
    /// Chat request payload (`message`, `conversation_id`).
    pub request: AgentChatRequest,
    /// Conversation channel (`API` or `WORKBENCH`).
    pub channel: ConversationChannel,
}

/// Object-safe bridge decoupling [`CapabilityPublisher`] from `AgentRuntime` so neither requires circular generics.
pub trait PublishedAgentInvoker: Send + Sync {
    /// Executes an `AxAgent` turn (optionally streaming [`AgentEvent`] frames into `event_tx`).
    fn invoke_published_agent<'a>(
        &'a self,
        principal: &'a AuthPrincipal,
        call: PublishedAgentCall,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> BoxFuture<'a, AgentChatResponse>;
}

/// Outcome returned by [`CapabilityPublisher::invoke_published`] and [`CapabilityPublisher::invoke_as_app`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PublishedInvokeOutcome {
    /// Synchronous JSON result (`HTTP 200`).
    Json(serde_json::Value),
    /// Asynchronous `AxFlow` run started (`HTTP 202`).
    StartedRun {
        /// Created or reused `ax_flow_runs.id`.
        run_id: Uuid,
        /// Current run status.
        status: RunStatus,
        /// `true` when an idempotent run was reused.
        existing: bool,
    },
}

impl PublishedInvokeOutcome {
    /// Converts this outcome into a JSON value payload.
    #[must_use]
    pub fn into_json_value(self) -> serde_json::Value {
        match self {
            Self::Json(val) => val,
            Self::StartedRun {
                run_id,
                status,
                existing,
            } => serde_json::json!({
                "run_id": run_id,
                "status": status,
                "existing": existing,
            }),
        }
    }
}

/// Context headers/metadata passed alongside `POST /api/published/{env}/{tenant_id}/{slug}`.
#[derive(Debug, Clone, Default)]
pub struct PublishedCallContext {
    /// Stable rate-limit subject key (e.g., `token_hash` or `token_id`).
    pub rate_limit_key: String,
    /// Optional `Idempotency-Key` HTTP header (forwarded to `FLOW` run starts).
    pub idempotency_key: Option<String>,
    /// Client IP address for `sec_audit_logs`.
    pub client_ip: String,
    /// Request trace identifier for `sec_audit_logs`.
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    endpoint_id: Uuid,
    target_version: Option<i32>,
    body_canonical: String,
    user_id: Uuid,
}

#[derive(Clone)]
struct CachedEntry {
    ttl_secs: u32,
    payload: serde_json::Value,
}

struct PerEntryTtlExpiry;

impl Expiry<CacheKey, CachedEntry> for PerEntryTtlExpiry {
    fn expire_after_create(
        &self,
        _key: &CacheKey,
        value: &CachedEntry,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(Duration::from_secs(u64::from(value.ttl_secs.max(1))))
    }
}

#[derive(Debug)]
struct ResolvedTargetMeta {
    output_shape: OutputShape,
    auto_schema: serde_json::Value,
    pinned_version: Option<i32>,
    pinned_snapshot: Option<serde_json::Value>,
}

/// Unified publishing service managing `ax_published_endpoints`, OpenAPI/Skill exports,
/// `PROD` version pinning, rate limiting, `moka` caching, and `Query-as-App` execution (`08-STEP09` §09.3).
pub struct CapabilityPublisher<R> {
    store: Arc<R>,
    capability: Arc<dyn AgentCapabilityPort>,
    audit: Option<AuditWriter>,
    rate_limiter: KeyedRateLimiter<(Uuid, String)>,
    response_cache: Cache<CacheKey, CachedEntry>,
    agent_invoker: RwLock<Option<Weak<dyn PublishedAgentInvoker>>>,
}

impl<R> CapabilityPublisher<R> {
    /// Creates a new [`CapabilityPublisher`] backed by `store` and `capability`.
    #[must_use]
    pub fn new(
        store: Arc<R>,
        capability: Arc<dyn AgentCapabilityPort>,
        audit: Option<AuditWriter>,
    ) -> Self {
        let response_cache = Cache::builder()
            .max_capacity(4_096)
            .expire_after(PerEntryTtlExpiry)
            .support_invalidation_closures()
            .build();
        Self {
            store,
            capability,
            audit,
            rate_limiter: KeyedRateLimiter::new(16_384),
            response_cache,
            agent_invoker: RwLock::new(None),
        }
    }

    /// Registers a weak reference to [`PublishedAgentInvoker`] (`AgentRuntime`) for `EndpointKind::Agent` endpoints.
    pub fn register_agent_invoker(&self, invoker: Weak<dyn PublishedAgentInvoker>) {
        if let Ok(mut guard) = self.agent_invoker.write() {
            *guard = Some(invoker);
        }
    }

    fn resolve_agent_invoker(&self) -> Result<Arc<dyn PublishedAgentInvoker>, AppError> {
        let maybe_weak = self
            .agent_invoker
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        maybe_weak
            .and_then(|w| w.upgrade())
            .ok_or_else(|| AppError::Config(String::from("PublishedAgentInvoker is not wired")))
    }
}

impl<R> CapabilityPublisher<R>
where
    R: AgentRepository + AnalyticsRepository + StudioRepository + FlowRepository + 'static,
{
    /// Lists published endpoints in `principal.tenant_id` (`DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] if `DATA_READ` is missing, or [`AppError::Database`] on storage failure.
    pub async fn list_endpoints(
        &self,
        principal: &AuthPrincipal,
        query: EndpointListQuery,
    ) -> Result<Vec<EndpointSpec>, AppError> {
        principal.require(Permission::DataRead)?;
        self.store.list_endpoints(&principal.tenant_id, query).await
    }

    /// Fetches a single published endpoint by UUID (`DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] if `DATA_READ` is missing, or [`AppError::NotFound`] when absent.
    pub async fn get_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint_id: Uuid,
    ) -> Result<EndpointSpec, AppError> {
        principal.require(Permission::DataRead)?;
        self.store
            .get_endpoint(&principal.tenant_id, endpoint_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))
    }

    /// Validates `req`, derives Draft 2020-12 `input_schema`, pins `PROD` target snapshots, and persists the endpoint.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial (`STUDIO_PUBLISH` / `FLOW_PUBLISH`), invalid slug/schema, missing target, or slug conflict.
    pub async fn publish_endpoint(
        &self,
        principal: &AuthPrincipal,
        req: CreateEndpointRequest,
    ) -> Result<EndpointSpec, AppError> {
        require_publish_permission(principal, req.kind)?;
        let slug = validate_endpoint_slug(&req.slug)?;
        let title = req.title.trim();
        if title.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "endpoint title cannot be empty",
            ));
        }
        validate_limits(req.rate_limit_rpm, req.cache_ttl_s)?;

        let meta = self
            .resolve_target_meta(
                &principal.tenant_id,
                req.kind,
                &req.target_id,
                req.environment,
            )
            .await?;
        let input_schema = match req.input_schema {
            Some(custom) if !custom.is_null() => {
                ensure_valid_json_schema(&custom)?;
                custom
            }
            _ => meta.auto_schema,
        };

        self.store
            .create_endpoint(
                &principal.tenant_id,
                SaveEndpointInput {
                    environment: req.environment,
                    slug,
                    title: String::from(title),
                    description: String::from(req.description.trim()),
                    kind: req.kind,
                    target_id: String::from(req.target_id.trim()),
                    target_version: meta.pinned_version,
                    target_snapshot: meta.pinned_snapshot,
                    input_schema,
                    output_shape: req.output_shape.unwrap_or(meta.output_shape),
                    required_scope: req.required_scope,
                    rate_limit_rpm: req.rate_limit_rpm,
                    cache_ttl_s: req.cache_ttl_s,
                    expose_mcp: req.expose_mcp,
                    show_as_app: req.show_as_app,
                    enabled: req.enabled,
                    published_by: non_nil_user(principal.user_id),
                },
            )
            .await
    }

    /// Updates mutable configuration fields on `endpoint_id` and invalidates cached responses.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, invalid schema/limits, or missing endpoint.
    pub async fn update_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint_id: Uuid,
        req: UpdateEndpointRequest,
    ) -> Result<EndpointSpec, AppError> {
        let existing = self
            .store
            .get_endpoint(&principal.tenant_id, endpoint_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        require_publish_permission(principal, existing.kind)?;

        if let Some(rpm) = req.rate_limit_rpm {
            validate_limits(rpm, existing.cache_ttl_s)?;
        }
        if let Some(ttl) = req.cache_ttl_s {
            validate_limits(existing.rate_limit_rpm, ttl)?;
        }
        if let Some(ref schema) = req.input_schema {
            ensure_valid_json_schema(schema)?;
        }

        let updated = self
            .store
            .update_endpoint(&principal.tenant_id, endpoint_id, req)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        self.invalidate_endpoint_cache(endpoint_id);
        Ok(updated)
    }

    /// Deletes `endpoint_id` (`STUDIO_PUBLISH` / `FLOW_PUBLISH` required) and invalidates cached responses.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or missing endpoint.
    pub async fn delete_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint_id: Uuid,
    ) -> Result<(), AppError> {
        let existing = self
            .store
            .get_endpoint(&principal.tenant_id, endpoint_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        require_publish_permission(principal, existing.kind)?;

        let deleted = self
            .store
            .delete_endpoint(&principal.tenant_id, endpoint_id)
            .await?;
        if !deleted {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        self.invalidate_endpoint_cache(endpoint_id);
        Ok(())
    }

    /// Promotes an endpoint (`TEST` -> `PROD`), capturing a fresh pinned `target_version` and `target_snapshot`.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, missing endpoint, or unpublishable target.
    pub async fn promote_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint_id: Uuid,
    ) -> Result<EndpointSpec, AppError> {
        let source = self
            .store
            .get_endpoint(&principal.tenant_id, endpoint_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        require_publish_permission(principal, source.kind)?;

        let meta = self
            .resolve_target_meta(
                &principal.tenant_id,
                source.kind,
                &source.target_id,
                Environment::Prod,
            )
            .await?;

        let promoted = self
            .store
            .upsert_endpoint_by_slug(
                &principal.tenant_id,
                SaveEndpointInput {
                    environment: Environment::Prod,
                    slug: source.slug,
                    title: source.title,
                    description: source.description,
                    kind: source.kind,
                    target_id: source.target_id,
                    target_version: meta.pinned_version,
                    target_snapshot: meta.pinned_snapshot,
                    input_schema: source.input_schema,
                    output_shape: meta.output_shape,
                    required_scope: source.required_scope,
                    rate_limit_rpm: source.rate_limit_rpm,
                    cache_ttl_s: source.cache_ttl_s,
                    expose_mcp: source.expose_mcp,
                    show_as_app: source.show_as_app,
                    enabled: source.enabled,
                    published_by: non_nil_user(principal.user_id),
                },
            )
            .await?;
        self.invalidate_endpoint_cache(promoted.id);
        Ok(promoted)
    }

    /// Exports an `OpenAPI` `3.1.0` specification for `environment` (`DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] if `DATA_READ` is missing, or [`AppError::Database`] on storage failure.
    pub async fn export_openapi(
        &self,
        principal: &AuthPrincipal,
        environment: Environment,
        server_url: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        principal.require(Permission::DataRead)?;
        let endpoints = self
            .store
            .list_endpoints(
                &principal.tenant_id,
                EndpointListQuery {
                    environment: Some(environment),
                    kind: None,
                    show_as_app: None,
                },
            )
            .await?;
        Ok(build_openapi_spec(&endpoints, server_url))
    }

    /// Exports a [`SkillManifest`] for `endpoint_id` (`DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError`] if `DATA_READ` is missing or `endpoint_id` does not exist.
    pub async fn export_skill_manifest(
        &self,
        principal: &AuthPrincipal,
        endpoint_id: Uuid,
        base_url: &str,
    ) -> Result<SkillManifest, AppError> {
        let ep = self.get_endpoint(principal, endpoint_id).await?;
        Ok(build_skill_manifest(&ep, base_url))
    }
}

/// Path routing coordinates for `POST /api/published/{env}/{tenant_id}/{slug}`.
#[derive(Debug, Clone, Copy)]
pub struct PublishedRouteTarget<'a> {
    /// Path `tenant_id` segment.
    pub tenant_id: &'a str,
    /// Path `env` (`TEST` or `PROD`).
    pub environment: Environment,
    /// Path `slug` segment.
    pub slug: &'a str,
}

impl<R> CapabilityPublisher<R>
where
    R: AgentRepository + AnalyticsRepository + StudioRepository + FlowRepository + 'static,
{
    /// Executes `POST /api/published/{env}/{tenant_id}/{slug}` with PAT verification, scope check,
    /// `(endpoint_id, token_key)` rate limiting, JSON Schema validation, `moka` caching, and audit logging.
    ///
    /// # Errors
    /// Returns [`AppError`] on auth/scope mismatch, rate limit, schema violation, or execution failure.
    pub async fn invoke_published(
        &self,
        principal: &AuthPrincipal,
        path_tenant_id: &str,
        environment: Environment,
        slug: &str,
        body: serde_json::Value,
        ctx: PublishedCallContext,
    ) -> Result<PublishedInvokeOutcome, AppError> {
        self.invoke_published_with_stream(
            principal,
            PublishedRouteTarget {
                tenant_id: path_tenant_id,
                environment,
                slug,
            },
            body,
            ctx,
            None,
        )
        .await
    }

    /// Streaming-capable variant of [`Self::invoke_published`] forwarding SSE [`AgentEvent`]s when `kind == AGENT`.
    ///
    /// # Errors
    /// Returns [`AppError`] on auth/scope mismatch, rate limit, schema violation, or execution failure.
    pub async fn invoke_published_with_stream(
        &self,
        principal: &AuthPrincipal,
        target: PublishedRouteTarget<'_>,
        body: serde_json::Value,
        ctx: PublishedCallContext,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<PublishedInvokeOutcome, AppError> {
        if principal.token_kind != TokenKind::ApiToken {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }
        if principal.tenant_id != target.tenant_id {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }

        let endpoint = self
            .store
            .get_endpoint_by_slug(target.tenant_id, target.environment, target.slug)
            .await?
            .filter(|ep| ep.enabled)
            .ok_or(AppError::NotFound("NOT_FOUND"))?;

        principal.require(endpoint.required_scope)?;
        let rl_subject = if ctx.rate_limit_key.is_empty() {
            principal.user_id.to_string()
        } else {
            ctx.rate_limit_key.clone()
        };
        self.rate_limiter
            .check_and_consume((endpoint.id, rl_subject), endpoint.rate_limit_rpm)?;

        let normalized_body = normalize_body_object(body);
        validate_instance_against_schema(&normalized_body, &endpoint.input_schema)?;

        let outcome = self
            .execute_cached_or_live(
                principal,
                &endpoint,
                &normalized_body,
                ctx.idempotency_key.clone(),
                ConversationChannel::Api,
                event_tx,
            )
            .await?;

        self.emit_publish_audit(
            principal,
            &endpoint,
            &outcome,
            &ctx.client_ip,
            &ctx.trace_id,
        );
        Ok(outcome)
    }

    /// Polls `GET /api/published/{env}/{tenant_id}/{slug}/runs/{run_id}` for a published `FLOW` endpoint.
    ///
    /// # Errors
    /// Returns [`AppError`] if PAT auth fails, scope is missing, or the run does not belong to this endpoint.
    pub async fn get_published_flow_run(
        &self,
        principal: &AuthPrincipal,
        path_tenant_id: &str,
        environment: Environment,
        slug: &str,
        run_id: Uuid,
    ) -> Result<RunRecord, AppError> {
        if principal.token_kind != TokenKind::ApiToken {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }
        if principal.tenant_id != path_tenant_id {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        let endpoint = self
            .store
            .get_endpoint_by_slug(path_tenant_id, environment, slug)
            .await?
            .filter(|ep| ep.enabled && ep.kind == EndpointKind::Flow)
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        principal.require(endpoint.required_scope)?;

        let expected_flow_id = parse_target_uuid(&endpoint.target_id)?;
        let run = self
            .store
            .get_flow_run(path_tenant_id, run_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        if run.flow_id != expected_flow_id || run.environment != environment {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        Ok(run)
    }

    /// Executes `POST /api/axagent/apps/{endpoint_id}/run` ("Query-as-App" internal workbench entry, §09.3.4).
    ///
    /// # Errors
    /// Returns [`AppError`] if `endpoint_id` is absent, `required_scope` is missing, or execution fails.
    pub async fn invoke_as_app(
        &self,
        principal: &AuthPrincipal,
        endpoint_id: Uuid,
        body: serde_json::Value,
    ) -> Result<PublishedInvokeOutcome, AppError> {
        let endpoint = self
            .store
            .get_endpoint(&principal.tenant_id, endpoint_id)
            .await?
            .filter(|ep| ep.enabled)
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        principal.require(endpoint.required_scope)?;

        let normalized_body = normalize_body_object(body);
        validate_instance_against_schema(&normalized_body, &endpoint.input_schema)?;

        let outcome = self
            .execute_cached_or_live(
                principal,
                &endpoint,
                &normalized_body,
                None,
                ConversationChannel::Workbench,
                None,
            )
            .await?;

        self.emit_publish_audit(principal, &endpoint, &outcome, "127.0.0.1", "");
        Ok(outcome)
    }

    /// Executes a published [`EndpointSpec`] on behalf of `McpServerHub` or `AgentRuntime` (`ToolRef::Endpoint`),
    /// enforcing rate limits, scope, JSON Schema, and `moka` cache without emitting duplicate publish audits.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, rate limiting, schema error, or execution failure.
    pub async fn invoke_for_tool(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        arguments: serde_json::Value,
        channel: ConversationChannel,
    ) -> Result<serde_json::Value, AppError> {
        if !endpoint.enabled {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        principal.require(endpoint.required_scope)?;
        self.rate_limiter.check_and_consume(
            (endpoint.id, principal.user_id.to_string()),
            endpoint.rate_limit_rpm,
        )?;

        let normalized = normalize_body_object(arguments);
        validate_instance_against_schema(&normalized, &endpoint.input_schema)?;

        let outcome = self
            .execute_cached_or_live(principal, endpoint, &normalized, None, channel, None)
            .await?;
        Ok(outcome.into_json_value())
    }

    fn invalidate_endpoint_cache(&self, endpoint_id: Uuid) {
        if let Err(err) = self
            .response_cache
            .invalidate_entries_if(move |k, _| k.endpoint_id == endpoint_id)
        {
            tracing::warn!(%endpoint_id, error = %err, "failed to schedule endpoint cache invalidation");
        }
    }

    async fn execute_cached_or_live(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        body: &serde_json::Value,
        idempotency_key: Option<String>,
        channel: ConversationChannel,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<PublishedInvokeOutcome, AppError> {
        let start = Instant::now();
        let cacheable = is_endpoint_cacheable(endpoint, body, event_tx.is_some());
        let cache_key = if cacheable {
            let key = CacheKey {
                endpoint_id: endpoint.id,
                target_version: endpoint.target_version,
                body_canonical: canonical_json_string(body),
                user_id: principal.user_id,
            };
            if let Some(hit) = self.response_cache.get(&key).await {
                metrics::counter!(
                    "axpoint_published_invocations_total",
                    "kind" => endpoint.kind.as_str(),
                    "cache" => "hit"
                )
                .increment(1);
                return Ok(PublishedInvokeOutcome::Json(hit.payload));
            }
            Some(key)
        } else {
            None
        };

        let outcome = self
            .execute_target_direct(
                principal,
                endpoint,
                body,
                idempotency_key,
                channel,
                event_tx,
            )
            .await?;

        if let (Some(key), PublishedInvokeOutcome::Json(val)) = (cache_key, &outcome) {
            self.response_cache
                .insert(
                    key,
                    CachedEntry {
                        ttl_secs: u32::try_from(endpoint.cache_ttl_s).unwrap_or(0),
                        payload: val.clone(),
                    },
                )
                .await;
        }

        metrics::counter!(
            "axpoint_published_invocations_total",
            "kind" => endpoint.kind.as_str(),
            "cache" => "miss"
        )
        .increment(1);
        metrics::histogram!(
            "axpoint_published_latency_ms",
            "kind" => endpoint.kind.as_str()
        )
        .record(start.elapsed().as_secs_f64() * 1_000.0_f64);

        Ok(outcome)
    }

    async fn execute_target_direct(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        body: &serde_json::Value,
        idempotency_key: Option<String>,
        channel: ConversationChannel,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<PublishedInvokeOutcome, AppError> {
        match endpoint.kind {
            EndpointKind::SavedQuery | EndpointKind::Metric | EndpointKind::ObjectType => {
                let val = self
                    .execute_data_endpoint(principal, endpoint, body)
                    .await?;
                Ok(PublishedInvokeOutcome::Json(val))
            }
            EndpointKind::Flow => {
                self.execute_flow_endpoint(principal, endpoint, body, idempotency_key)
                    .await
            }
            EndpointKind::Agent => {
                let val = self
                    .execute_agent_endpoint(principal, endpoint, body, channel, event_tx)
                    .await?;
                Ok(PublishedInvokeOutcome::Json(val))
            }
        }
    }

    async fn execute_data_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let analytics_principal = to_analytics_principal(principal);
        let is_prod = endpoint.environment == Environment::Prod;
        match endpoint.kind {
            EndpointKind::SavedQuery => {
                let saved_id = parse_target_uuid(&endpoint.target_id)?;
                let pinned: Option<SavedQuery> = if is_prod {
                    decode_optional_snapshot(endpoint.target_snapshot.as_ref())?
                } else {
                    None
                };
                let params = extract_saved_query_params(body);
                self.capability
                    .run_saved_endpoint(&analytics_principal, saved_id, pinned.as_ref(), params)
                    .await
            }
            EndpointKind::Metric => {
                let pinned: Option<MetricDef> = if is_prod {
                    decode_optional_snapshot(endpoint.target_snapshot.as_ref())?
                } else {
                    None
                };
                let query: MetricQuery = serde_json::from_value(body.clone()).map_err(|err| {
                    AppError::bad(
                        "SCHEMA_INVALID",
                        format!("invalid MetricQuery payload: {err}"),
                    )
                })?;
                let res = self
                    .capability
                    .run_metric_endpoint(
                        &analytics_principal,
                        &endpoint.target_id,
                        pinned.as_ref(),
                        query,
                    )
                    .await?;
                serde_json::to_value(res).map_err(|err| {
                    AppError::Config(format!("serialize MetricQueryResult failed: {err}"))
                })
            }
            _ => {
                let pinned: Option<NodeTypeSpec> = if is_prod {
                    decode_optional_snapshot(endpoint.target_snapshot.as_ref())?
                } else {
                    None
                };
                self.capability
                    .query_domain_objects(
                        &analytics_principal,
                        &endpoint.target_id,
                        pinned.as_ref(),
                        body,
                    )
                    .await
            }
        }
    }

    async fn execute_flow_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        body: &serde_json::Value,
        idempotency_key: Option<String>,
    ) -> Result<PublishedInvokeOutcome, AppError> {
        let flow_id = parse_target_uuid(&endpoint.target_id)?;
        let is_prod = endpoint.environment == Environment::Prod;
        let trigger_payload = body.get("payload").cloned().unwrap_or_else(|| body.clone());
        let (run, existing) = self
            .store
            .start_flow_run(
                &endpoint.tenant_id,
                StartRunParams {
                    flow_id,
                    environment: endpoint.environment,
                    explicit_version: if is_prod {
                        endpoint.target_version
                    } else {
                        None
                    },
                    trigger_source: TriggerType::Api,
                    trigger_payload,
                    idempotency_key,
                    started_by: non_nil_user(principal.user_id),
                },
            )
            .await?;
        Ok(PublishedInvokeOutcome::StartedRun {
            run_id: run.id,
            status: run.status,
            existing,
        })
    }

    async fn execute_agent_endpoint(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        body: &serde_json::Value,
        channel: ConversationChannel,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<serde_json::Value, AppError> {
        let agent_id = parse_target_uuid(&endpoint.target_id)?;
        let is_prod = endpoint.environment == Environment::Prod;
        let pinned_spec: Option<AgentSpec> = if is_prod {
            decode_optional_snapshot(endpoint.target_snapshot.as_ref())?
        } else {
            None
        };
        let chat_req = parse_agent_endpoint_request(body)?;
        let invoker = self.resolve_agent_invoker()?;
        let resp = invoker
            .invoke_published_agent(
                principal,
                PublishedAgentCall {
                    agent_id,
                    pinned_version: if is_prod {
                        endpoint.target_version
                    } else {
                        None
                    },
                    pinned_spec,
                    request: chat_req,
                    channel,
                },
                event_tx,
            )
            .await?;
        serde_json::to_value(resp)
            .map_err(|err| AppError::Config(format!("serialize AgentChatResponse failed: {err}")))
    }

    async fn resolve_target_meta(
        &self,
        tenant_id: &str,
        kind: EndpointKind,
        raw_target_id: &str,
        environment: Environment,
    ) -> Result<ResolvedTargetMeta, AppError> {
        let target_id = raw_target_id.trim();
        if target_id.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "target_id cannot be empty",
            ));
        }
        let is_prod = environment == Environment::Prod;
        match kind {
            EndpointKind::SavedQuery => {
                self.resolve_saved_query_meta(tenant_id, target_id, is_prod)
                    .await
            }
            EndpointKind::Metric => {
                self.resolve_metric_meta(tenant_id, target_id, is_prod)
                    .await
            }
            EndpointKind::ObjectType => {
                self.resolve_object_type_meta(tenant_id, target_id, is_prod)
                    .await
            }
            EndpointKind::Flow => self.resolve_flow_meta(tenant_id, target_id, is_prod).await,
            EndpointKind::Agent => self.resolve_agent_meta(tenant_id, target_id, is_prod).await,
        }
    }

    async fn resolve_saved_query_meta(
        &self,
        tenant_id: &str,
        target_id: &str,
        is_prod: bool,
    ) -> Result<ResolvedTargetMeta, AppError> {
        let sq_id = parse_target_uuid(target_id)?;
        let sq = self
            .store
            .get_saved_query(tenant_id, sq_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let auto_schema = build_saved_query_schema(&sq.parameters);
        let (pinned_version, pinned_snapshot) = if is_prod {
            let snap = serde_json::to_value(&sq)
                .map_err(|e| AppError::Config(format!("serialize SavedQuery: {e}")))?;
            (Some(1), Some(snap))
        } else {
            (None, None)
        };
        Ok(ResolvedTargetMeta {
            output_shape: OutputShape::Rows,
            auto_schema,
            pinned_version,
            pinned_snapshot,
        })
    }

    async fn resolve_metric_meta(
        &self,
        tenant_id: &str,
        target_id: &str,
        is_prod: bool,
    ) -> Result<ResolvedTargetMeta, AppError> {
        let metric = self
            .store
            .get_metric_by_name(tenant_id, target_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let auto_schema = build_metric_schema(&metric);
        let (pinned_version, pinned_snapshot) = if is_prod {
            let snap = serde_json::to_value(&metric)
                .map_err(|e| AppError::Config(format!("serialize MetricDef: {e}")))?;
            (Some(metric.version), Some(snap))
        } else {
            (None, None)
        };
        Ok(ResolvedTargetMeta {
            output_shape: OutputShape::Rows,
            auto_schema,
            pinned_version,
            pinned_snapshot,
        })
    }

    async fn resolve_object_type_meta(
        &self,
        tenant_id: &str,
        target_id: &str,
        is_prod: bool,
    ) -> Result<ResolvedTargetMeta, AppError> {
        let published = self.store.get_published_schema(tenant_id, "main").await?;
        let nt = published
            .doc
            .node_types
            .iter()
            .find(|n| n.name == target_id)
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let auto_schema = build_object_type_schema(nt, &published.doc);
        let (pinned_version, pinned_snapshot) = if is_prod {
            let snap = serde_json::to_value(nt)
                .map_err(|e| AppError::Config(format!("serialize NodeTypeSpec: {e}")))?;
            (Some(published.version), Some(snap))
        } else {
            (None, None)
        };
        Ok(ResolvedTargetMeta {
            output_shape: OutputShape::Object,
            auto_schema,
            pinned_version,
            pinned_snapshot,
        })
    }

    async fn resolve_flow_meta(
        &self,
        tenant_id: &str,
        target_id: &str,
        is_prod: bool,
    ) -> Result<ResolvedTargetMeta, AppError> {
        let flow_id = parse_target_uuid(target_id)?;
        let _flow = self
            .store
            .get_flow(tenant_id, flow_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let pinned_version = if is_prod {
            let dep = self
                .store
                .get_flow_deployment(tenant_id, flow_id, Environment::Prod)
                .await?
                .ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        "FLOW endpoint in PROD requires an active PROD flow deployment",
                    )
                })?;
            Some(dep.version)
        } else {
            None
        };
        Ok(ResolvedTargetMeta {
            output_shape: OutputShape::Run,
            auto_schema: build_flow_schema(),
            pinned_version,
            pinned_snapshot: None,
        })
    }

    async fn resolve_agent_meta(
        &self,
        tenant_id: &str,
        target_id: &str,
        is_prod: bool,
    ) -> Result<ResolvedTargetMeta, AppError> {
        let agent_id = parse_target_uuid(target_id)?;
        let agent = self
            .store
            .get_agent(tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let (pinned_version, pinned_snapshot) = if is_prod {
            if agent.version < 1 {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "AGENT endpoint in PROD requires a published agent version >= 1",
                ));
            }
            let ver = self
                .store
                .get_agent_version(tenant_id, agent_id, agent.version)
                .await?
                .ok_or(AppError::NotFound("NOT_FOUND"))?;
            let snap = serde_json::to_value(&ver.spec)
                .map_err(|e| AppError::Config(format!("serialize AgentSpec: {e}")))?;
            (Some(ver.version), Some(snap))
        } else {
            (None, None)
        };
        Ok(ResolvedTargetMeta {
            output_shape: OutputShape::Text,
            auto_schema: build_agent_schema(),
            pinned_version,
            pinned_snapshot,
        })
    }

    fn emit_publish_audit(
        &self,
        principal: &AuthPrincipal,
        endpoint: &EndpointSpec,
        outcome: &PublishedInvokeOutcome,
        client_ip: &str,
        trace_id: &str,
    ) {
        let Some(ref writer) = self.audit else {
            return;
        };
        let action = match endpoint.kind {
            EndpointKind::SavedQuery | EndpointKind::Metric | EndpointKind::ObjectType => {
                AuditAction::QuerySql
            }
            EndpointKind::Flow => AuditAction::FlowRun,
            EndpointKind::Agent => AuditAction::AgentChat,
        };
        let status_code = match outcome {
            PublishedInvokeOutcome::Json(_) => 200,
            PublishedInvokeOutcome::StartedRun { .. } => 202,
        };
        writer.record(AuditRecord {
            tenant_id: endpoint.tenant_id.clone(),
            user_id: non_nil_user(principal.user_id),
            username: principal.username.clone(),
            action,
            resource_ref: format!("endpoint:{}", endpoint.id),
            status_code,
            client_ip: if client_ip.is_empty() {
                String::from("127.0.0.1")
            } else {
                String::from(client_ip)
            },
            trace_id: String::from(trace_id),
            detail_json: serde_json::json!({
                "endpoint_id": endpoint.id,
                "endpoint_slug": endpoint.slug,
                "environment": endpoint.environment,
                "kind": endpoint.kind,
            }),
            created_at: Utc::now(),
        });
    }
}

/// Validates a published endpoint `slug` against `^[a-z0-9][a-z0-9-]{2,62}$` (`3..=63` chars, no trailing `-`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when `slug` violates URL-safe naming rules.
pub fn validate_endpoint_slug(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    let Some(&first) = bytes.first() else {
        return Err(AppError::bad("INVALID_ARGUMENT", "slug cannot be empty"));
    };
    if !(3..=63).contains(&bytes.len())
        || !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || trimmed.ends_with('-')
        || !bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid endpoint slug '{trimmed}': must match ^[a-z0-9][a-z0-9-]{{2,62}}$"),
        ));
    }
    Ok(String::from(trimmed))
}

/// Validates a JSON payload against a Draft 2020-12 JSON Schema, reporting offending field paths on failure.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`SCHEMA_INVALID`) when the schema or instance fails validation.
pub fn validate_instance_against_schema(
    instance: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), AppError> {
    let validator = jsonschema::validator_for(schema).map_err(|err| {
        AppError::bad(
            "SCHEMA_INVALID",
            format!("endpoint input_schema is invalid JSON Schema: {err}"),
        )
    })?;

    let errors: Vec<String> = validator
        .iter_errors(instance)
        .take(5)
        .map(|err| {
            let path = err.instance_path().to_string();
            if path.is_empty() {
                err.to_string()
            } else {
                format!("{path}: {err}")
            }
        })
        .collect();

    if !errors.is_empty() {
        return Err(AppError::bad("SCHEMA_INVALID", errors.join("; ")));
    }
    Ok(())
}

fn ensure_valid_json_schema(schema: &serde_json::Value) -> Result<(), AppError> {
    if !schema.is_object() {
        return Err(AppError::bad(
            "SCHEMA_INVALID",
            "input_schema must be a JSON object",
        ));
    }
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|err| AppError::bad("SCHEMA_INVALID", format!("invalid input_schema: {err}")))
}

fn require_publish_permission(
    principal: &AuthPrincipal,
    kind: EndpointKind,
) -> Result<(), AppError> {
    if kind == EndpointKind::Flow {
        principal.require(Permission::FlowPublish)
    } else {
        principal.require(Permission::StudioPublish)
    }
}

fn validate_limits(rate_limit_rpm: i32, cache_ttl_s: i32) -> Result<(), AppError> {
    if !(1..=600_000).contains(&rate_limit_rpm) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "rate_limit_rpm must be in 1..=600000",
        ));
    }
    if !(0..=86_400).contains(&cache_ttl_s) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "cache_ttl_s must be in 0..=86400",
        ));
    }
    Ok(())
}

fn non_nil_user(user_id: Uuid) -> Option<Uuid> {
    if user_id.is_nil() {
        None
    } else {
        Some(user_id)
    }
}

fn parse_target_uuid(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw.trim()).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid target UUID '{raw}': {err}"),
        )
    })
}

fn decode_optional_snapshot<T: for<'de> Deserialize<'de>>(
    snapshot: Option<&serde_json::Value>,
) -> Result<Option<T>, AppError> {
    let Some(val) = snapshot.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    serde_json::from_value(val.clone())
        .map(Some)
        .map_err(|err| AppError::Config(format!("corrupt target_snapshot JSON: {err}")))
}

fn normalize_body_object(body: serde_json::Value) -> serde_json::Value {
    if body.is_null() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        body
    }
}

fn is_endpoint_cacheable(
    endpoint: &EndpointSpec,
    body: &serde_json::Value,
    is_streaming: bool,
) -> bool {
    if endpoint.cache_ttl_s <= 0 || is_streaming {
        return false;
    }
    match endpoint.kind {
        EndpointKind::SavedQuery | EndpointKind::Metric | EndpointKind::ObjectType => true,
        EndpointKind::Flow => false,
        EndpointKind::Agent => body
            .get("conversation_id")
            .is_none_or(serde_json::Value::is_null),
    }
}

fn canonical_json_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&str, String> = map
                .iter()
                .map(|(k, v)| (k.as_str(), canonical_json_string(v)))
                .collect();
            let parts: Vec<String> = sorted
                .into_iter()
                .map(|(k, v)| {
                    let key_json = serde_json::to_string(k).unwrap_or_default();
                    format!("{key_json}:{v}")
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonical_json_string).collect();
            format!("[{}]", parts.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Converts an [`AuthPrincipal`] into an [`AnalyticsPrincipal`].
#[must_use]
pub fn to_analytics_principal(principal: &AuthPrincipal) -> AnalyticsPrincipal {
    AnalyticsPrincipal {
        user_id: principal.user_id,
        tenant_id: principal.tenant_id.clone(),
        username: principal.username.clone(),
        permissions: principal.permissions.clone(),
    }
}

fn extract_saved_query_params(body: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    let source = body
        .get("params")
        .and_then(serde_json::Value::as_object)
        .or_else(|| body.as_object());
    let mut out = BTreeMap::new();
    if let Some(map) = source {
        for (k, v) in map {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn parse_agent_endpoint_request(body: &serde_json::Value) -> Result<AgentChatRequest, AppError> {
    let message = body
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("SCHEMA_INVALID", "missing non-empty 'message'"))?;

    let conversation_id = match body.get("conversation_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => None,
        Some(serde_json::Value::String(s)) => Some(Uuid::parse_str(s.trim()).map_err(|err| {
            AppError::bad(
                "SCHEMA_INVALID",
                format!("invalid conversation_id UUID: {err}"),
            )
        })?),
        _ => {
            return Err(AppError::bad(
                "SCHEMA_INVALID",
                "conversation_id must be a UUID string",
            ));
        }
    };

    Ok(AgentChatRequest {
        message: String::from(message),
        conversation_id,
        draft: false,
    })
}

fn data_type_json_schema(dt: DataType) -> serde_json::Value {
    match dt {
        DataType::Int64 => serde_json::json!({ "type": "integer" }),
        DataType::Float64 | DataType::Decimal => serde_json::json!({ "type": "number" }),
        DataType::Boolean => serde_json::json!({ "type": "boolean" }),
        DataType::Date => serde_json::json!({ "type": "string", "format": "date" }),
        DataType::Timestamp => serde_json::json!({ "type": "string", "format": "date-time" }),
        DataType::String | DataType::Binary => serde_json::json!({ "type": "string" }),
        DataType::List => serde_json::json!({ "type": "array" }),
        DataType::Struct => serde_json::json!({ "type": "object" }),
        DataType::Json => serde_json::json!({}),
    }
}

/// Derives a Draft 2020-12 JSON Schema for a `SAVED_QUERY` endpoint (`08-STEP09` §09.3.1).
#[must_use]
pub fn build_saved_query_schema(parameters: &[QueryParamSpec]) -> serde_json::Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for param in parameters {
        let mut prop_schema = data_type_json_schema(param.data_type);
        if let (Some(default_val), Some(obj)) = (&param.default, prop_schema.as_object_mut()) {
            obj.insert(String::from("default"), default_val.clone());
        } else if param.default.is_none() {
            required.push(param.name.clone());
        }
        props.insert(param.name.clone(), prop_schema);
    }

    serde_json::json!({
        "$schema": DRAFT_2020_12_URI,
        "type": "object",
        "required": required,
        "properties": props,
        "additionalProperties": false
    })
}

/// Derives a Draft 2020-12 JSON Schema for a `METRIC` endpoint (`08-STEP09` §09.3.1).
#[must_use]
pub fn build_metric_schema(metric: &MetricDef) -> serde_json::Value {
    let dim_names: Vec<String> = metric.dimensions.iter().map(|d| d.name.clone()).collect();
    let dim_item_schema = if dim_names.is_empty() {
        serde_json::json!({ "type": "string" })
    } else {
        serde_json::json!({ "type": "string", "enum": dim_names })
    };

    serde_json::json!({
        "$schema": DRAFT_2020_12_URI,
        "type": "object",
        "properties": {
            "dimensions": {
                "type": "array",
                "items": dim_item_schema
            },
            "filters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["field", "op"],
                    "properties": {
                        "field": { "type": "string", "minLength": 1 },
                        "op": { "type": "string" },
                        "value": {}
                    },
                    "additionalProperties": false
                }
            },
            "time_range": {
                "type": "object",
                "required": ["from", "to"],
                "properties": {
                    "from": { "type": "string", "format": "date-time" },
                    "to": { "type": "string", "format": "date-time" }
                },
                "additionalProperties": false
            },
            "grain": {
                "type": "string",
                "enum": ["MINUTE", "HOUR", "DAY", "MONTH"]
            },
            "compare": {
                "type": "string",
                "enum": ["PREVIOUS_PERIOD", "SAME_PERIOD_LAST_YEAR"]
            }
        },
        "additionalProperties": false
    })
}

/// Derives a Draft 2020-12 JSON Schema for an `OBJECT_TYPE` endpoint (`08-STEP09` §09.3.1).
#[must_use]
pub fn build_object_type_schema(node_type: &NodeTypeSpec, doc: &SchemaDoc) -> serde_json::Value {
    let mut attr_props = serde_json::Map::new();
    let mut metric_props = serde_json::Map::new();

    for prop in &node_type.properties {
        match prop.kind {
            PropertyKind::Attribute => {
                let mut schema = data_type_json_schema(prop.data_type);
                if let (Some(allowed), Some(obj)) = (&prop.allowed_values, schema.as_object_mut())
                    && !allowed.is_empty()
                {
                    obj.insert(String::from("enum"), serde_json::json!(allowed));
                }
                attr_props.insert(prop.name.clone(), schema);
            }
            PropertyKind::Metric => {
                metric_props.insert(prop.name.clone(), serde_json::json!({ "type": "number" }));
            }
        }
    }

    let mut link_names: Vec<String> = doc
        .edge_types
        .iter()
        .filter(|e| e.src_type_name == node_type.name || e.dst_type_name == node_type.name)
        .map(|e| e.name.clone())
        .collect();
    link_names.sort();
    link_names.dedup();
    let link_item_schema = if link_names.is_empty() {
        serde_json::json!({ "type": "string" })
    } else {
        serde_json::json!({ "type": "string", "enum": link_names })
    };

    serde_json::json!({
        "$schema": DRAFT_2020_12_URI,
        "type": "object",
        "properties": {
            "keys": {
                "type": "array",
                "items": { "type": "string", "minLength": 1 }
            },
            "q": { "type": "string" },
            "properties": {
                "type": "object",
                "properties": attr_props,
                "additionalProperties": false
            },
            "metrics_min": {
                "type": "object",
                "properties": metric_props,
                "additionalProperties": false
            },
            "include_links": {
                "type": "array",
                "items": link_item_schema
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 200,
                "default": 50
            },
            "cursor": { "type": "string" }
        },
        "additionalProperties": false
    })
}

/// Derives a Draft 2020-12 JSON Schema for a `FLOW` endpoint (`08-STEP09` §09.3.1).
#[must_use]
pub fn build_flow_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": DRAFT_2020_12_URI,
        "type": "object",
        "properties": {
            "payload": { "type": "object" }
        },
        "additionalProperties": false
    })
}

/// Derives a Draft 2020-12 JSON Schema for an `AGENT` endpoint (`08-STEP09` §09.3.1).
#[must_use]
pub fn build_agent_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": DRAFT_2020_12_URI,
        "type": "object",
        "required": ["message"],
        "properties": {
            "message": { "type": "string", "minLength": 1 },
            "conversation_id": { "type": "string" }
        },
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use axpoint_domain::AppError;
    use axpoint_domain::analytics::{MetricDef, MetricDimension};
    use axpoint_domain::enums::RollupGrain;
    use chrono::Utc;
    use uuid::Uuid;

    use super::{build_metric_schema, validate_endpoint_slug, validate_instance_against_schema};

    #[test]
    fn slug_validation_enforces_pattern() -> Result<(), AppError> {
        assert_eq!(
            validate_endpoint_slug("paid-amount-by-status")?,
            "paid-amount-by-status"
        );
        assert!(validate_endpoint_slug("ab").is_err());
        assert!(validate_endpoint_slug("-bad").is_err());
        assert!(validate_endpoint_slug("bad-").is_err());
        assert!(validate_endpoint_slug("BadSlug").is_err());
        assert!(validate_endpoint_slug("bad_slug").is_err());
        Ok(())
    }

    #[test]
    fn metric_auto_schema_rejects_invalid_grain_and_accepts_valid() -> Result<(), AppError> {
        let metric = MetricDef {
            id: Uuid::new_v4(),
            name: String::from("paid_amount"),
            display_name: String::from("Paid Amount"),
            description: String::new(),
            dataset_id: Uuid::new_v4(),
            expression: String::from("SUM(amount)"),
            filter_sql: String::new(),
            time_column: Some(String::from("updated_at")),
            default_grain: RollupGrain::Month,
            dimensions: vec![MetricDimension {
                name: String::from("status"),
                column: String::from("status"),
                display_name: String::from("Status"),
            }],
            unit: String::from("CNY"),
            format: String::new(),
            synonyms: Vec::new(),
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let schema = build_metric_schema(&metric);

        validate_instance_against_schema(
            &serde_json::json!({ "dimensions": ["status"], "grain": "MONTH" }),
            &schema,
        )?;

        let bad =
            validate_instance_against_schema(&serde_json::json!({ "grain": "WEEKLY" }), &schema);
        assert!(matches!(
            bad,
            Err(AppError::BadRequest {
                code: "SCHEMA_INVALID",
                ..
            })
        ));
        Ok(())
    }
}
