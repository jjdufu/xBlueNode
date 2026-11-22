//! `AxAgent` multi-model provider management, capability publishing, Query-as-App, external MCP clients,
//! multi-turn agent workbench, and external embed share routes (`/api/axagent/*`, `08-STEP09` §09.9).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axpoint_agent::{
    ChannelChatCall, EmbedRequestMeta, PublishedInvokeOutcome, normalize_url_for_guard,
    probe_im_connector,
};
use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentEmbedRecord, AgentEvent, AgentRecord, AgentVersionRecord,
    ConversationListQuery, ConversationRecord, CreateAgentEmbedRequest, CreateAgentEmbedResponse,
    CreateAgentRequest, CreateEndpointRequest, CreateMcpClientRequest, CredentialMutation,
    EmbedChatRequest, EndpointListQuery, EndpointSpec, LlmProviderRecord, LlmUsageQuery,
    LlmUsageRecord, McpClientRecord, MessageRecord, ProviderScope, ProviderTestResponse,
    PutUserLlmPreferencesRequest, SaveLlmProviderInput, SaveLlmProviderRequest, SkillManifest,
    UpdateAgentRequest, UpdateEndpointRequest, UserLlmPreferenceRecord,
};
use axpoint_domain::axhub::{PaginatedList, SealedCredential};
use axpoint_domain::enums::{
    AuditAction, ConversationChannel, Environment, ImIdentityPolicy, Permission,
};
use axpoint_domain::im::{
    ConfirmImBindRequest, ConfirmImBindResponse, CreateImConnectorRequest, ImBindChallengePreview,
    ImBindPreviewQuery, ImConnectorConfig, ImConnectorRecord, ImConnectorTestResponse,
    ImUserBindingRecord, NewImConnectorInput, PatchImConnectorRequest, UpdateImConnectorInput,
    hash_im_bind_token,
};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    AgentRepository as _, ConnectionRepository as _, CredentialStore as _,
    ImGatewayRepository as _, OutboundUrlGuardPort as _,
};
use axpoint_security::{AuthPrincipal, CredentialKeyring, NetGuard};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::routes::published::agent_events_to_sse;
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Query parameter for `GET /api/axagent/endpoints/openapi.json?environment=PROD`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenApiExportQuery {
    #[serde(default)]
    environment: Option<Environment>,
}

/// Builds the `/axagent/*` sub-router mounted under `/api` (`A4` §7).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .merge(providers_router())
        .merge(endpoints_and_mcp_router())
        .merge(agents_and_embeds_router())
        .merge(im_gateway_router())
}

fn providers_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axagent/providers",
            get(list_providers_handler).post(create_provider_handler),
        )
        .route(
            "/axagent/providers/{id}",
            put(update_provider_handler).delete(delete_provider_handler),
        )
        .route("/axagent/providers/{id}/test", post(test_provider_handler))
        .route(
            "/axagent/llm-preferences",
            get(get_llm_preferences_handler).put(put_llm_preferences_handler),
        )
        .route("/axagent/usage", get(query_usage_handler))
}

fn endpoints_and_mcp_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axagent/endpoints",
            get(list_endpoints_handler).post(create_endpoint_handler),
        )
        .route(
            "/axagent/endpoints/openapi.json",
            get(export_openapi_handler),
        )
        .route(
            "/axagent/endpoints/{id}",
            put(update_endpoint_handler).delete(delete_endpoint_handler),
        )
        .route(
            "/axagent/endpoints/{id}/promote",
            post(promote_endpoint_handler),
        )
        .route(
            "/axagent/endpoints/{id}/skill.json",
            get(export_skill_handler),
        )
        .route("/axagent/apps/{endpoint_id}/run", post(run_app_handler))
        .route(
            "/axagent/mcp-clients",
            get(list_mcp_clients_handler).post(create_mcp_client_handler),
        )
        .route(
            "/axagent/mcp-clients/{id}",
            delete(delete_mcp_client_handler),
        )
        .route(
            "/axagent/mcp-clients/{id}/sync",
            post(sync_mcp_client_handler),
        )
}

fn agents_and_embeds_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axagent/agents",
            get(list_agents_handler).post(create_agent_handler),
        )
        .route(
            "/axagent/agents/{id}",
            get(get_agent_handler)
                .put(update_agent_handler)
                .delete(delete_agent_handler),
        )
        .route("/axagent/agents/{id}/publish", post(publish_agent_handler))
        .route("/axagent/agents/{id}/chat", post(chat_agent_handler))
        .route("/axagent/conversations", get(list_conversations_handler))
        .route(
            "/axagent/conversations/{id}/messages",
            get(list_messages_handler),
        )
        .route(
            "/axagent/conversations/{id}",
            delete(delete_conversation_handler),
        )
        .route(
            "/axagent/agents/{id}/embeds",
            get(list_embeds_handler).post(create_embed_handler),
        )
        .route("/axagent/embeds/{embed_id}", delete(revoke_embed_handler))
        .route("/axagent/embed/{token}/chat", post(embed_chat_handler))
}

fn im_gateway_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axagent/im-connectors",
            get(list_im_connectors_handler).post(create_im_connector_handler),
        )
        .route(
            "/axagent/im-connectors/{id}",
            patch(patch_im_connector_handler).delete(delete_im_connector_handler),
        )
        .route(
            "/axagent/im-connectors/{id}/test",
            post(test_im_connector_handler),
        )
        .route("/axagent/im-bindings", get(list_im_bindings_handler))
        .route(
            "/axagent/im-bindings/{id}",
            delete(delete_im_binding_handler),
        )
        .route("/axagent/im-bind/preview", get(preview_im_bind_handler))
        .route("/axagent/im-bind/confirm", post(confirm_im_bind_handler))
}

fn seal_provider_api_key(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    api_key: &str,
) -> Result<SealedCredential, AppError> {
    let cred_id = Uuid::new_v4();
    let mut map = BTreeMap::new();
    map.insert(String::from("api_key"), String::from(api_key));
    let encoded = Zeroizing::new(
        serde_json::to_vec(&map)
            .map_err(|err| AppError::Config(format!("serialize provider api_key: {err}")))?,
    );
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred_id);
    keyring.seal_credential(cred_id, &aad, encoded.as_slice())
}

fn is_system_provider_admin(principal: &AuthPrincipal) -> bool {
    principal.has(Permission::GuardAdmin) || principal.has(Permission::OpsAdmin)
}

fn can_access_providers(principal: &AuthPrincipal) -> bool {
    principal.has(Permission::DataRead)
        || principal.has(Permission::AgentRun)
        || principal.has(Permission::AgentEdit)
        || is_system_provider_admin(principal)
}

fn decorate_provider_readonly(mut p: LlmProviderRecord, is_sys_admin: bool) -> LlmProviderRecord {
    p.is_readonly = p.owner_user_id.is_none() && !is_sys_admin;
    p
}

async fn validate_and_build_provider_input(
    state: &AppState,
    principal: &AuthPrincipal,
    req: SaveLlmProviderRequest,
    is_create: bool,
    owner_user_id: Option<Uuid>,
) -> Result<SaveLlmProviderInput, AppError> {
    if req.flags.allow_private_network || req.flags.allow_pii {
        principal.require(Permission::GuardAdmin)?;
    }

    let name = req.name.trim();
    let base_url = req.base_url.trim().trim_end_matches('/');
    let model = req.model.trim();
    if name.is_empty() || base_url.is_empty() || model.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "provider name, base_url, and model must be non-empty",
        ));
    }
    if req.purposes.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "provider purposes must contain at least one purpose",
        ));
    }
    if req.max_tokens < 1 || req.timeout_s < 1 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "max_tokens and timeout_s must be positive integers",
        ));
    }
    if !(2_048..=2_097_152).contains(&req.structured_caps.max_context_tokens) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "max_context_tokens must be in 2048..=2097152",
        ));
    }
    if let Some(rpm) = req.rpm_limit
        && rpm < 1
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "rpm_limit must be >= 1 when specified",
        ));
    }

    let probe_url = format!("{base_url}/chat/completions");
    let guard = NetGuard::new();
    let _validated = guard
        .check_outbound_url(&probe_url, req.flags.allow_private_network)
        .await?;

    let credential = match req.api_key {
        Some(raw) if raw.trim().is_empty() => CredentialMutation::Clear,
        Some(raw) => CredentialMutation::Set(seal_provider_api_key(
            &state.keyring,
            &principal.tenant_id,
            raw.trim(),
        )?),
        None if is_create => CredentialMutation::Clear,
        None => CredentialMutation::Keep,
    };

    Ok(SaveLlmProviderInput {
        name: String::from(name),
        base_url: String::from(base_url),
        model: String::from(model),
        purposes: req.purposes,
        credential,
        priority: req.priority,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        timeout_s: req.timeout_s,
        rpm_limit: req.rpm_limit,
        embedding_dim: req.embedding_dim,
        flags: req.flags,
        structured_caps: req.structured_caps,
        owner_user_id,
        enabled: req.enabled,
    })
}

async fn list_providers_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<LlmProviderRecord>>, HttpError> {
    if !can_access_providers(&principal) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let is_sys_admin = is_system_provider_admin(&principal);
    let visible = state
        .store
        .list_visible_llm_providers(&principal.tenant_id, principal.user_id)
        .await?;
    let items = visible
        .into_iter()
        .map(|p| decorate_provider_readonly(p, is_sys_admin))
        .collect();
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_provider_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<SaveLlmProviderRequest>,
) -> Result<Json<LlmProviderRecord>, HttpError> {
    let is_sys_admin = is_system_provider_admin(&principal);
    let scope = req.scope.unwrap_or(if is_sys_admin {
        ProviderScope::System
    } else {
        ProviderScope::Personal
    });
    let owner_user_id = match scope {
        ProviderScope::System => {
            if !is_sys_admin {
                return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
            }
            None
        }
        ProviderScope::Personal => {
            if !can_access_providers(&principal) {
                return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
            }
            Some(principal.user_id)
        }
    };

    let input =
        validate_and_build_provider_input(&state, &principal, req, true, owner_user_id).await?;
    let created = state
        .store
        .create_llm_provider(&principal.tenant_id, input)
        .await?;
    state
        .llm_router
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(decorate_provider_readonly(created, is_sys_admin)))
}

async fn update_provider_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<SaveLlmProviderRequest>,
) -> Result<Json<LlmProviderRecord>, HttpError> {
    let (existing, _cred) = state
        .store
        .get_llm_provider(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("PROVIDER_NOT_FOUND"))?;

    let is_sys_admin = is_system_provider_admin(&principal);
    if existing.owner_user_id.is_none() {
        if !is_sys_admin {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
        if req.scope == Some(ProviderScope::Personal) {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "cannot change provider scope across SYSTEM and PERSONAL",
            )));
        }
    } else if existing.owner_user_id == Some(principal.user_id) {
        if !can_access_providers(&principal) {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
        if req.scope == Some(ProviderScope::System) {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "cannot change provider scope across SYSTEM and PERSONAL",
            )));
        }
    } else {
        return Err(HttpError(AppError::NotFound("PROVIDER_NOT_FOUND")));
    }

    let input =
        validate_and_build_provider_input(&state, &principal, req, false, existing.owner_user_id)
            .await?;
    let updated = state
        .store
        .update_llm_provider(&principal.tenant_id, id, input)
        .await?
        .ok_or(AppError::NotFound("PROVIDER_NOT_FOUND"))?;
    state
        .llm_router
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(decorate_provider_readonly(updated, is_sys_admin)))
}

async fn delete_provider_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let (existing, _cred) = state
        .store
        .get_llm_provider(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("PROVIDER_NOT_FOUND"))?;

    if existing.owner_user_id.is_none() {
        if !is_system_provider_admin(&principal) {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
    } else if existing.owner_user_id == Some(principal.user_id) {
        if !can_access_providers(&principal) {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
    } else {
        return Err(HttpError(AppError::NotFound("PROVIDER_NOT_FOUND")));
    }

    let deleted = state
        .store
        .delete_llm_provider(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("PROVIDER_NOT_FOUND")));
    }
    state
        .llm_router
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn test_provider_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ProviderTestResponse>, HttpError> {
    let (provider, _cred) = state
        .store
        .get_llm_provider(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("PROVIDER_NOT_FOUND"))?;

    if provider.owner_user_id.is_none() {
        if !principal.has(Permission::AgentEdit) && !is_system_provider_admin(&principal) {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
    } else if provider.owner_user_id != Some(principal.user_id) {
        return Err(HttpError(AppError::NotFound("PROVIDER_NOT_FOUND")));
    }

    let resp = state
        .llm_router
        .probe_provider(&principal.tenant_id, &provider, None)
        .await;
    Ok(Json(resp))
}

async fn get_llm_preferences_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<UserLlmPreferenceRecord>>, HttpError> {
    let items = state
        .store
        .list_user_llm_preferences(&principal.tenant_id, principal.user_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn put_llm_preferences_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<PutUserLlmPreferencesRequest>,
) -> Result<Json<PaginatedList<UserLlmPreferenceRecord>>, HttpError> {
    let mut seen_purposes = BTreeSet::new();
    for item in &req.preferences {
        if !seen_purposes.insert(item.purpose) {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "duplicate purpose '{}' in preferences",
                    item.purpose.as_str()
                ),
            )));
        }
    }

    let visible = state
        .store
        .list_visible_llm_providers(&principal.tenant_id, principal.user_id)
        .await?;
    let mut pairs = Vec::with_capacity(req.preferences.len());
    for item in &req.preferences {
        let found = visible
            .iter()
            .find(|p| p.id == item.provider_id)
            .ok_or(AppError::NotFound("PROVIDER_NOT_FOUND"))?;
        if !found.enabled {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "preferred LLM provider must be enabled",
            )));
        }
        if !found.purposes.contains(&item.purpose) {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "provider '{}' does not support purpose '{}'",
                    found.name,
                    item.purpose.as_str()
                ),
            )));
        }
        pairs.push((item.purpose, item.provider_id));
    }

    let items = state
        .store
        .replace_user_llm_preferences(&principal.tenant_id, principal.user_id, &pairs)
        .await?;
    state
        .llm_router
        .invalidate_user_preferences(&principal.tenant_id, principal.user_id)
        .await;

    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: (!principal.user_id.is_nil()).then_some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::PolicyChange,
        resource_ref: String::from("llm_preferences"),
        status_code: 200,
        client_ip: extract_client_ip(&headers),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({ "count": items.len() }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "put_llm_preferences audit flush failed");
    }

    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn query_usage_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<LlmUsageQuery>,
) -> Result<Json<PaginatedList<LlmUsageRecord>>, HttpError> {
    principal.require(Permission::AgentEdit)?;
    let mut items = state
        .store
        .query_llm_usage(&principal.tenant_id, query)
        .await?;
    if !is_system_provider_admin(&principal) {
        let visible_ids: BTreeSet<Uuid> = state
            .store
            .list_visible_llm_providers(&principal.tenant_id, principal.user_id)
            .await?
            .into_iter()
            .map(|p| p.id)
            .collect();
        items.retain(|row| visible_ids.contains(&row.provider_id));
    }
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn list_endpoints_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<EndpointListQuery>,
) -> Result<Json<PaginatedList<EndpointSpec>>, HttpError> {
    let items = state
        .axagent
        .publisher
        .list_endpoints(&principal, query)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_endpoint_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateEndpointRequest>,
) -> Result<Json<EndpointSpec>, HttpError> {
    if req.environment == Environment::Prod {
        principal.enforce_no_prod_mutation()?;
    }
    let spec = state
        .axagent
        .publisher
        .publish_endpoint(&principal, req)
        .await?;
    Ok(Json(spec))
}

async fn update_endpoint_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateEndpointRequest>,
) -> Result<Json<EndpointSpec>, HttpError> {
    if let Some(existing) = state.store.get_endpoint(&principal.tenant_id, id).await?
        && existing.environment == Environment::Prod
    {
        principal.enforce_no_prod_mutation()?;
    }
    let spec = state
        .axagent
        .publisher
        .update_endpoint(&principal, id, req)
        .await?;
    Ok(Json(spec))
}

async fn delete_endpoint_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    if let Some(existing) = state.store.get_endpoint(&principal.tenant_id, id).await?
        && existing.environment == Environment::Prod
    {
        principal.enforce_no_prod_mutation()?;
    }
    state
        .axagent
        .publisher
        .delete_endpoint(&principal, id)
        .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn promote_endpoint_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<EndpointSpec>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    let promoted = state
        .axagent
        .publisher
        .promote_endpoint(&principal, id)
        .await?;
    Ok(Json(promoted))
}

async fn export_openapi_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<OpenApiExportQuery>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let env = query.environment.unwrap_or(Environment::Prod);
    let server_url = infer_base_url(&headers);
    let doc = state
        .axagent
        .publisher
        .export_openapi(&principal, env, server_url.as_deref())
        .await?;
    Ok(Json(doc))
}

async fn export_skill_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<SkillManifest>, HttpError> {
    let base_url = infer_base_url(&headers).unwrap_or_default();
    let skill = state
        .axagent
        .publisher
        .export_skill_manifest(&principal, id, &base_url)
        .await?;
    Ok(Json(skill))
}

async fn run_app_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(endpoint_id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Response, HttpError> {
    let payload = if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).map_err(|err| {
            HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid JSON payload: {err}"),
            ))
        })?
    };

    let outcome = state
        .axagent
        .publisher
        .invoke_as_app(&principal, endpoint_id, payload)
        .await?;
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "run_app audit flush failed");
    }

    match outcome {
        PublishedInvokeOutcome::Json(val) => Ok((StatusCode::OK, Json(val)).into_response()),
        start @ PublishedInvokeOutcome::StartedRun { .. } => {
            Ok((StatusCode::ACCEPTED, Json(start.into_json_value())).into_response())
        }
    }
}

async fn list_mcp_clients_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<McpClientRecord>>, HttpError> {
    let items = state.axagent.mcp_clients.list_clients(&principal).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_mcp_client_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateMcpClientRequest>,
) -> Result<Json<McpClientRecord>, HttpError> {
    let rec = state
        .axagent
        .mcp_clients
        .create_from_request(&principal, req)
        .await?;
    Ok(Json(rec))
}

async fn delete_mcp_client_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    state
        .axagent
        .mcp_clients
        .delete_client(&principal, id)
        .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn sync_mcp_client_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<McpClientRecord>, HttpError> {
    let synced = state
        .axagent
        .mcp_clients
        .sync_client_tools(&principal, id)
        .await?;
    Ok(Json(synced))
}

async fn list_agents_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<AgentRecord>>, HttpError> {
    let items = state.axagent.runtime.list_agents(&principal).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn get_agent_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AgentRecord>, HttpError> {
    let rec = state.axagent.runtime.get_agent(&principal, id).await?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _res_ref = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::Agent,
        &id.to_string(),
        &client_ip,
    )
    .await?;
    Ok(Json(rec))
}

async fn create_agent_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateAgentRequest>,
) -> Result<Json<AgentRecord>, HttpError> {
    let rec = state.axagent.runtime.create_agent(&principal, req).await?;
    Ok(Json(rec))
}

async fn update_agent_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateAgentRequest>,
) -> Result<Json<AgentRecord>, HttpError> {
    let rec = state
        .axagent
        .runtime
        .update_agent(&principal, id, req)
        .await?;
    Ok(Json(rec))
}

async fn delete_agent_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    state.axagent.runtime.delete_agent(&principal, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn publish_agent_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AgentVersionRecord>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    let ver = state.axagent.runtime.publish_agent(&principal, id).await?;
    Ok(Json(ver))
}

async fn chat_agent_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<AgentChatRequest>,
) -> Result<Response, HttpError> {
    principal.require(Permission::AgentRun)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _res_ref = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::AgentRun,
        axpoint_domain::guard::GrantResourceKind::Agent,
        &id.to_string(),
        &client_ip,
    )
    .await?;

    if !wants_sse(&headers) {
        let resp = state.axagent.runtime.chat(&principal, id, req).await?;
        if let Err(err) = state.audit.flush().await {
            tracing::warn!(error = %err, "agent chat audit flush failed");
        }
        return Ok((StatusCode::OK, Json(resp)).into_response());
    }
    if req.draft {
        principal.require(Permission::AgentEdit)?;
    }
    if req.message.trim().is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "message cannot be empty",
        )));
    }
    let agent = state
        .store
        .get_agent(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;
    if !req.draft && agent.version < 1 {
        return Err(HttpError(AppError::Conflict("CONFLICT")));
    }

    let (tx, rx) = mpsc::channel(64);
    let runtime = Arc::clone(&state.axagent.runtime);
    let audit = state.audit.clone();
    tokio::spawn(async move {
        if let Err(err) = runtime.chat_stream(&principal, id, req, tx.clone()).await {
            let _closed = tx
                .send(AgentEvent::Error {
                    error_code: String::from(err.error_code()),
                    message: err.client_message(),
                })
                .await
                .is_err();
        }
        if let Err(err) = audit.flush().await {
            tracing::warn!(error = %err, "agent SSE chat audit flush failed");
        }
    });
    Ok(agent_events_to_sse(rx))
}

async fn list_conversations_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ConversationListQuery>,
) -> Result<Json<PaginatedList<ConversationRecord>>, HttpError> {
    if let Some(lim) = query.limit
        && !(1..=200).contains(&lim)
    {
        return Err(HttpError(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=200, got {lim}"),
        )));
    }
    let page = state
        .axagent
        .runtime
        .list_conversations(&principal, query)
        .await?;
    Ok(Json(page))
}

async fn list_messages_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<PaginatedList<MessageRecord>>, HttpError> {
    let items = state
        .axagent
        .runtime
        .list_conversation_messages(&principal, id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn delete_conversation_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    state
        .axagent
        .runtime
        .delete_conversation(&principal, id)
        .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn list_embeds_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<PaginatedList<AgentEmbedRecord>>, HttpError> {
    let items = state.axagent.embed.list_embeds(&principal, id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_embed_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateAgentEmbedRequest>,
) -> Result<Json<CreateAgentEmbedResponse>, HttpError> {
    let client_ip = extract_client_ip(&headers);
    let trace_id = current_trace_id();
    let created = state
        .axagent
        .embed
        .create_embed(&principal, id, req, &client_ip, &trace_id)
        .await?;
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "create_embed audit flush failed");
    }
    Ok(Json(created))
}

async fn revoke_embed_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(embed_id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    state
        .axagent
        .embed
        .revoke_embed(&principal, embed_id)
        .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn embed_chat_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiPath(token): ApiPath<String>,
    ApiJson(req): ApiJson<EmbedChatRequest>,
) -> Result<Response, HttpError> {
    let meta = EmbedRequestMeta {
        origin: headers
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        host: headers
            .get("host")
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        client_ip: extract_client_ip(&headers),
        trace_id: current_trace_id(),
    };
    let chat_req = AgentChatRequest {
        message: req.message,
        conversation_id: req.conversation_id,
        draft: false,
    };

    if !wants_sse(&headers) {
        let resp = state
            .axagent
            .embed
            .chat_via_embed(&token, chat_req, meta, None)
            .await?;
        if let Err(err) = state.audit.flush().await {
            tracing::warn!(error = %err, "embed chat audit flush failed");
        }
        return Ok((StatusCode::OK, Json(resp)).into_response());
    }

    if chat_req.message.trim().is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "message cannot be empty",
        )));
    }
    let (resolved, principal) = state
        .axagent
        .embed
        .authenticate_embed(&token, &meta)
        .await?;
    let (tx, rx) = mpsc::channel(64);
    let runtime = Arc::clone(&state.axagent.runtime);
    let audit = state.audit.clone();
    tokio::spawn(async move {
        let call = ChannelChatCall {
            agent_id: resolved.agent_id,
            explicit_version: None,
            explicit_spec: None,
            request: chat_req,
            channel: ConversationChannel::Embed,
            embed_id: Some(resolved.embed_id),
            im_connector_id: None,
            external_chat_id: None,
        };
        if let Err(err) = runtime
            .chat_in_channel(&principal, call, Some(tx.clone()))
            .await
        {
            let _closed = tx
                .send(AgentEvent::Error {
                    error_code: String::from(err.error_code()),
                    message: err.client_message(),
                })
                .await
                .is_err();
        }
        if let Err(err) = audit.flush().await {
            tracing::warn!(error = %err, "embed SSE chat audit flush failed");
        }
    });
    Ok(agent_events_to_sse(rx))
}

fn wants_sse(headers: &HeaderMap) -> bool {
    headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.to_ascii_lowercase().contains("text/event-stream"))
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| String::from("127.0.0.1"), String::from)
}

fn infer_base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| *s == "http" || *s == "https")
        .unwrap_or("http");
    Some(format!("{proto}://{host}"))
}

fn seal_im_secret_map(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    cred_id: Uuid,
    secret_map: &BTreeMap<String, String>,
) -> Result<SealedCredential, AppError> {
    let encoded = Zeroizing::new(
        serde_json::to_vec(secret_map)
            .map_err(|err| AppError::Config(format!("serialize IM connector secret: {err}")))?,
    );
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred_id);
    keyring.seal_credential(cred_id, &aad, encoded.as_slice())
}

async fn validate_im_connector_fields(
    state: &AppState,
    principal: &AuthPrincipal,
    name: &str,
    agent_id: Uuid,
    identity_policy: ImIdentityPolicy,
    delegated_user_id: Option<Uuid>,
    config: &ImConnectorConfig,
) -> Result<(String, Option<Uuid>, ImConnectorConfig), AppError> {
    if config.allow_private_network {
        principal.require(Permission::GuardAdmin)?;
    }
    let clean_name = name.trim();
    if clean_name.is_empty() || clean_name.len() > 128 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "IM connector name must be 1..=128 characters",
        ));
    }

    let resolved_delegated = match identity_policy {
        ImIdentityPolicy::RequireUserBind => None,
        ImIdentityPolicy::CreatorDelegated => {
            let uid = delegated_user_id.filter(|u| !u.is_nil()).ok_or_else(|| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    "CREATOR_DELEGATED policy requires delegated_user_id",
                )
            })?;
            let _user = state
                .store
                .load_principal(&principal.tenant_id, uid)
                .await?
                .ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        "delegated_user_id does not exist or is inactive",
                    )
                })?;
            Some(uid)
        }
    };

    let _agent = state
        .store
        .get_agent(&principal.tenant_id, agent_id)
        .await?
        .ok_or(AppError::NotFound("AGENT_NOT_FOUND"))?;

    let mut clean_cfg = config.clone();
    clean_cfg.endpoint_override = clean_cfg
        .endpoint_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    clean_cfg.bot_code = clean_cfg
        .bot_code
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    clean_cfg.web_base_url = clean_cfg
        .web_base_url
        .as_deref()
        .map(str::trim)
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .map(String::from);

    if let Some(ref raw_override) = clean_cfg.endpoint_override {
        let http_equiv = normalize_url_for_guard(raw_override)?;
        let _checked = NetGuard::new()
            .check_outbound_url(&http_equiv, clean_cfg.allow_private_network)
            .await?;
    }

    Ok((String::from(clean_name), resolved_delegated, clean_cfg))
}

async fn list_im_connectors_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<ImConnectorRecord>>, HttpError> {
    if !principal.has(Permission::DataRead) && !principal.has(Permission::AgentEdit) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let items = state.store.list_im_connectors(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_im_connector_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateImConnectorRequest>,
) -> Result<(StatusCode, Json<ImConnectorRecord>), HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::AgentEdit)?;

    let (name, delegated_user_id, config) = validate_im_connector_fields(
        &state,
        &principal,
        &req.name,
        req.agent_id,
        req.identity_policy,
        req.delegated_user_id,
        &req.config,
    )
    .await?;

    let secret_map = req.secret.normalize_for_platform(req.platform)?;
    let credential_id = Uuid::new_v4();
    let credential = seal_im_secret_map(
        &state.keyring,
        &principal.tenant_id,
        credential_id,
        &secret_map,
    )?;
    let created_by = (!principal.user_id.is_nil()).then_some(principal.user_id);

    let created = state
        .store
        .create_im_connector(
            &principal.tenant_id,
            NewImConnectorInput {
                name,
                platform: req.platform,
                agent_id: req.agent_id,
                credential_id,
                credential,
                config,
                identity_policy: req.identity_policy,
                delegated_user_id,
                enabled: req.enabled,
                created_by,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn patch_im_connector_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchImConnectorRequest>,
) -> Result<Json<ImConnectorRecord>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::AgentEdit)?;

    let existing = state
        .store
        .get_im_connector(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;

    let next_name = req.name.as_deref().unwrap_or(&existing.name);
    let next_agent_id = req.agent_id.unwrap_or(existing.agent_id);
    let next_policy = req.identity_policy.unwrap_or(existing.identity_policy);
    let next_delegated = req.delegated_user_id.or(existing.delegated_user_id);
    let next_cfg = req.config.unwrap_or_else(|| existing.config.clone());
    let next_enabled = req.enabled.unwrap_or(existing.enabled);

    let (name, delegated_user_id, config) = validate_im_connector_fields(
        &state,
        &principal,
        next_name,
        next_agent_id,
        next_policy,
        next_delegated,
        &next_cfg,
    )
    .await?;

    let rotated_credential = match req.secret {
        Some(ref sec_in) if !sec_in.is_empty() => {
            let secret_map = sec_in.normalize_for_platform(existing.platform)?;
            let new_cred_id = Uuid::new_v4();
            let sealed = seal_im_secret_map(
                &state.keyring,
                &principal.tenant_id,
                new_cred_id,
                &secret_map,
            )?;
            Some((new_cred_id, sealed))
        }
        _ => None,
    };

    let updated = state
        .store
        .update_im_connector(
            &principal.tenant_id,
            id,
            UpdateImConnectorInput {
                name,
                agent_id: next_agent_id,
                rotated_credential,
                config,
                identity_policy: next_policy,
                delegated_user_id,
                enabled: next_enabled,
            },
        )
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;
    Ok(Json(updated))
}

async fn delete_im_connector_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::AgentEdit)?;

    let deleted = state
        .store
        .delete_im_connector(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn test_im_connector_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ImConnectorTestResponse>, HttpError> {
    principal.require(Permission::AgentEdit)?;
    let connector = state
        .store
        .get_im_connector(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;
    if connector.config.allow_private_network {
        principal.require(Permission::GuardAdmin)?;
    }

    let sealed = state
        .store
        .get_credential(&principal.tenant_id, connector.credential_id)
        .await?
        .ok_or(AppError::NotFound("CREDENTIAL_NOT_FOUND"))?;
    let secrets =
        crate::routes::open_secret_map(&state.keyring, &principal.tenant_id, Some(&sealed))?;
    let resp = probe_im_connector(&connector, &secrets, &NetGuard::new()).await?;
    Ok(Json(resp))
}

async fn list_im_bindings_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<ImUserBindingRecord>>, HttpError> {
    let only_user_id = if principal.has(Permission::AgentEdit) {
        None
    } else {
        principal.require(Permission::DataRead)?;
        Some(principal.user_id)
    };
    let items = state
        .store
        .list_im_user_bindings(&principal.tenant_id, only_user_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn delete_im_binding_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let only_user_id = if principal.has(Permission::AgentEdit) {
        None
    } else if !principal.user_id.is_nil() {
        Some(principal.user_id)
    } else {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    };

    let removed = state
        .store
        .delete_im_user_binding(&principal.tenant_id, id, only_user_id)
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;

    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: (!principal.user_id.is_nil()).then_some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::ImAccountBind,
        resource_ref: format!("im_binding:{id}"),
        status_code: 200,
        client_ip: extract_client_ip(&headers),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "op": "unbind",
            "platform": removed.platform,
            "external_user_id": removed.external_user_id,
            "bound_user_id": removed.user_id,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "delete_im_binding audit flush failed");
    }

    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn preview_im_bind_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ImBindPreviewQuery>,
) -> Result<Json<ImBindChallengePreview>, HttpError> {
    let token_hash = hash_im_bind_token(&query.token)?;
    let preview = state
        .store
        .preview_im_bind_challenge(&principal.tenant_id, &token_hash)
        .await?
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "IM bind token is expired or already consumed",
            )
        })?;
    Ok(Json(preview))
}

async fn confirm_im_bind_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<ConfirmImBindRequest>,
) -> Result<Json<ConfirmImBindResponse>, HttpError> {
    if principal.user_id.is_nil() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "authenticated user account is required to confirm IM binding",
        )));
    }
    let token_hash = hash_im_bind_token(&req.token)?;
    let consumed = state
        .store
        .consume_im_bind_challenge(&principal.tenant_id, &token_hash, principal.user_id)
        .await?
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "IM bind token is expired or already consumed",
            )
        })?;
    let bound_at = Utc::now();

    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: Some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::ImAccountBind,
        resource_ref: format!("im_connector:{}", consumed.connector_id),
        status_code: 200,
        client_ip: extract_client_ip(&headers),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "connector_id": consumed.connector_id,
            "platform": consumed.platform,
            "external_user_id": consumed.external_user_id,
            "external_display_name": consumed.external_display_name,
            "user_id": principal.user_id,
        }),
        created_at: bound_at,
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "confirm_im_bind audit flush failed");
    }

    Ok(Json(ConfirmImBindResponse {
        connector_id: consumed.connector_id,
        platform: consumed.platform,
        external_user_id: consumed.external_user_id,
        external_display_name: consumed.external_display_name,
        user_id: principal.user_id,
        bound_at,
    }))
}
