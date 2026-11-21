//! `AxAgent` multi-model provider management, capability publishing, Query-as-App, external MCP clients,
//! multi-turn agent workbench, and external embed share routes (`/api/axagent/*`, `08-STEP09` §09.9).

use std::collections::BTreeMap;
use std::sync::Arc;

use axpoint_agent::{ChannelChatCall, EmbedRequestMeta, PublishedInvokeOutcome};
use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentEmbedRecord, AgentEvent, AgentRecord, AgentVersionRecord,
    ConversationListQuery, ConversationRecord, CreateAgentEmbedRequest, CreateAgentEmbedResponse,
    CreateAgentRequest, CreateEndpointRequest, CreateMcpClientRequest, CredentialMutation,
    EmbedChatRequest, EndpointListQuery, EndpointSpec, LlmProviderRecord, LlmUsageQuery,
    LlmUsageRecord, McpClientRecord, MessageRecord, ProviderTestResponse, SaveLlmProviderInput,
    SaveLlmProviderRequest, SkillManifest, UpdateAgentRequest, UpdateEndpointRequest,
};
use axpoint_domain::axhub::{PaginatedList, SealedCredential};
use axpoint_domain::enums::{ConversationChannel, Environment, Permission};
use axpoint_domain::ports::{AgentRepository as _, OutboundUrlGuardPort as _};
use axpoint_security::{AuthPrincipal, CredentialKeyring, NetGuard};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
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

async fn validate_and_build_provider_input(
    state: &AppState,
    principal: &AuthPrincipal,
    req: SaveLlmProviderRequest,
    is_create: bool,
) -> Result<SaveLlmProviderInput, AppError> {
    principal.require(Permission::AgentEdit)?;
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
        enabled: req.enabled,
    })
}

async fn list_providers_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<LlmProviderRecord>>, HttpError> {
    if !principal.has(Permission::AgentEdit) && !principal.has(Permission::DataRead) {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }
    let items = state.store.list_llm_providers(&principal.tenant_id).await?;
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
    let input = validate_and_build_provider_input(&state, &principal, req, true).await?;
    let created = state
        .store
        .create_llm_provider(&principal.tenant_id, input)
        .await?;
    state
        .llm_router
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(created))
}

async fn update_provider_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<SaveLlmProviderRequest>,
) -> Result<Json<LlmProviderRecord>, HttpError> {
    let input = validate_and_build_provider_input(&state, &principal, req, false).await?;
    let updated = state
        .store
        .update_llm_provider(&principal.tenant_id, id, input)
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;
    state
        .llm_router
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(updated))
}

async fn delete_provider_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::AgentEdit)?;
    let deleted = state
        .store
        .delete_llm_provider(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("NOT_FOUND")));
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
    principal.require(Permission::AgentEdit)?;
    let (provider, _cred) = state
        .store
        .get_llm_provider(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;
    let resp = state
        .llm_router
        .probe_provider(&principal.tenant_id, &provider, None)
        .await;
    Ok(Json(resp))
}

async fn query_usage_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<LlmUsageQuery>,
) -> Result<Json<PaginatedList<LlmUsageRecord>>, HttpError> {
    principal.require(Permission::AgentEdit)?;
    let items = state
        .store
        .query_llm_usage(&principal.tenant_id, query)
        .await?;
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
