//! Step 08 operational rules, alert center, notification channels, and silences routes (`/api/axflow/*`).

use std::collections::BTreeMap;
use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{PaginatedList, SealedCredential};
use axpoint_domain::enums::{AuditAction, Permission};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::RulesRepository as _;
use axpoint_domain::rules::{
    AckBatchRequest, AckBatchResponse, AlertActionCommentRequest, AlertEventRecord,
    AlertEventsQuery, AlertListQuery, AlertRecord, AlertSilenceRecord, CreateNotifyChannelRequest,
    CreateRuleRequest, CreateSilenceRequest, NotifyChannelRecord, RuleDef, RuleDryRunResponse,
    RuleListQuery, SaveNotifyChannelInput, SimplePageQuery, TestChannelResponse,
    UpdateNotifyChannelRequest, UpdateRuleRequest,
};
use axpoint_flow::{NotificationDispatcher, RuleEvaluator};
use axpoint_security::{AuthPrincipal, CredentialKeyring, NetGuard};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::axflow::{parse_optional_json, validate_page_limit};
use crate::routes::open_secret_map;
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

const INTERNAL_SECRET_FIELDS_KEY: &str = "__secret_fields";

/// Builds the Step 08 `/axflow/{rules,alerts,channels,silences}` router (`07-STEP07-08` §08.6).
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .merge(rules_router())
        .merge(alerts_router())
        .merge(channels_router())
        .merge(silences_router())
}

fn rules_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axflow/rules",
            get(list_rules_handler).post(create_rule_handler),
        )
        .route("/axflow/rules/dry-run", post(dry_run_adhoc_rule_handler))
        .route(
            "/axflow/rules/{id}",
            get(get_rule_handler)
                .put(update_rule_handler)
                .delete(delete_rule_handler),
        )
        .route(
            "/axflow/rules/{id}/dry-run",
            post(dry_run_saved_rule_handler),
        )
}

fn alerts_router() -> Router<AppState> {
    Router::new()
        .route("/axflow/alerts", get(list_alerts_handler))
        .route("/axflow/alerts/ack-batch", post(ack_alerts_batch_handler))
        .route("/axflow/alerts/{id}", get(get_alert_handler))
        .route("/axflow/alerts/{id}/events", get(list_alert_events_handler))
        .route("/axflow/alerts/{id}/ack", post(ack_alert_handler))
        .route("/axflow/alerts/{id}/resolve", post(resolve_alert_handler))
}

fn channels_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axflow/channels",
            get(list_channels_handler).post(create_channel_handler),
        )
        .route(
            "/axflow/channels/{id}",
            put(update_channel_handler).delete(delete_channel_handler),
        )
        .route("/axflow/channels/{id}/test", post(test_channel_handler))
}

fn silences_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axflow/silences",
            get(list_silences_handler).post(create_silence_handler),
        )
        .route("/axflow/silences/{id}", delete(delete_silence_handler))
}

fn emit_policy_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    resource_type: &str,
    resource_id: &str,
    status_code: i32,
    detail_json: serde_json::Value,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: Some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::PolicyChange,
        resource_ref: format!("{resource_type}:{resource_id}"),
        status_code,
        client_ip: String::from("0.0.0.0"),
        trace_id: current_trace_id(),
        detail_json,
        created_at: Utc::now(),
    });
}

// ============================================================================
// Rules (`/api/axflow/rules*`)
// ============================================================================

async fn list_rules_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<RuleListQuery>,
) -> Result<Json<PaginatedList<RuleDef>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state.store.list_rules(&principal.tenant_id, query).await?;
    Ok(Json(page))
}

async fn get_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RuleDef>, HttpError> {
    principal.require(Permission::DataRead)?;
    let rule = state
        .store
        .get_rule(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("RULE_NOT_FOUND"))?;
    Ok(Json(rule))
}

async fn create_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateRuleRequest>,
) -> Result<(StatusCode, Json<RuleDef>), HttpError> {
    principal.require(Permission::RuleEdit)?;
    let rule = state
        .store
        .create_rule(&principal.tenant_id, Some(principal.user_id), req)
        .await?;
    emit_policy_audit(
        &state,
        &principal,
        "ax_rule",
        &rule.id.to_string(),
        201,
        serde_json::json!({ "op": "create", "name": rule.name }),
    );
    Ok((StatusCode::CREATED, Json(rule)))
}

async fn update_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateRuleRequest>,
) -> Result<Json<RuleDef>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let rule = state
        .store
        .update_rule(&principal.tenant_id, id, req)
        .await?
        .ok_or(AppError::NotFound("RULE_NOT_FOUND"))?;
    emit_policy_audit(
        &state,
        &principal,
        "ax_rule",
        &rule.id.to_string(),
        200,
        serde_json::json!({ "op": "update", "version": rule.version, "enabled": rule.enabled }),
    );
    Ok(Json(rule))
}

async fn delete_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let deleted = state.store.delete_rule(&principal.tenant_id, id).await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("RULE_NOT_FOUND")));
    }
    emit_policy_audit(
        &state,
        &principal,
        "ax_rule",
        &id.to_string(),
        200,
        serde_json::json!({ "op": "delete" }),
    );
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn dry_run_saved_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RuleDryRunResponse>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let evaluator = RuleEvaluator::new(Arc::new(state.store.clone()), None);
    let res = evaluator.dry_run_rule(&principal.tenant_id, id).await?;
    Ok(Json(res))
}

async fn dry_run_adhoc_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateRuleRequest>,
) -> Result<Json<RuleDryRunResponse>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let evaluator = RuleEvaluator::new(Arc::new(state.store.clone()), None);
    let res = evaluator
        .dry_run_request(&principal.tenant_id, &req)
        .await?;
    Ok(Json(res))
}

// ============================================================================
// Alerts (`/api/axflow/alerts*`)
// ============================================================================

async fn list_alerts_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<AlertListQuery>,
) -> Result<Json<PaginatedList<AlertRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state.store.list_alerts(&principal.tenant_id, query).await?;
    Ok(Json(page))
}

async fn get_alert_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AlertRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let alert = state
        .store
        .get_alert(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("ALERT_NOT_FOUND"))?;
    Ok(Json(alert))
}

async fn list_alert_events_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<AlertEventsQuery>,
) -> Result<Json<PaginatedList<AlertEventRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_alert_events(&principal.tenant_id, id, query)
        .await?;
    Ok(Json(page))
}

async fn ack_alert_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Json<AlertRecord>, HttpError> {
    principal.require(Permission::AlertAck)?;
    let req: AlertActionCommentRequest = parse_optional_json(&body)?;
    let alert = state
        .store
        .ack_alert(
            &principal.tenant_id,
            id,
            Some(principal.user_id),
            req.comment.trim(),
        )
        .await?;
    Ok(Json(alert))
}

async fn ack_alerts_batch_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<AckBatchRequest>,
) -> Result<Json<AckBatchResponse>, HttpError> {
    principal.require(Permission::AlertAck)?;
    let res = state
        .store
        .ack_alerts_batch(
            &principal.tenant_id,
            &req.alert_ids,
            Some(principal.user_id),
            req.comment.trim(),
        )
        .await?;
    Ok(Json(res))
}

async fn resolve_alert_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Json<AlertRecord>, HttpError> {
    principal.require(Permission::AlertAck)?;
    let req: AlertActionCommentRequest = parse_optional_json(&body)?;
    let alert = state
        .store
        .resolve_alert_manual(
            &principal.tenant_id,
            id,
            Some(principal.user_id),
            req.comment.trim(),
        )
        .await?;
    Ok(Json(alert))
}

// ============================================================================
// Notification Channels (`/api/axflow/channels*`)
// ============================================================================

async fn list_channels_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<SimplePageQuery>,
) -> Result<Json<PaginatedList<NotifyChannelRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_channels(&principal.tenant_id, query)
        .await?;
    Ok(Json(page))
}

async fn create_channel_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateNotifyChannelRequest>,
) -> Result<(StatusCode, Json<NotifyChannelRecord>), HttpError> {
    principal.require(Permission::RuleEdit)?;
    if req.allow_private_network {
        principal.require(Permission::GuardAdmin)?;
    }

    let input = build_create_channel_input(&state, &principal.tenant_id, req).await?;
    let channel = state
        .store
        .create_channel(&principal.tenant_id, input)
        .await?;
    emit_policy_audit(
        &state,
        &principal,
        "ax_notify_channel",
        &channel.id.to_string(),
        201,
        serde_json::json!({
            "op": "create",
            "kind": channel.kind.as_str(),
            "allow_private_network": channel.allow_private_network,
        }),
    );
    Ok((StatusCode::CREATED, Json(channel)))
}

async fn update_channel_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateNotifyChannelRequest>,
) -> Result<Json<NotifyChannelRecord>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let (existing, existing_sealed) = state
        .store
        .get_channel(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("CHANNEL_NOT_FOUND"))?;

    let allow_priv = req
        .allow_private_network
        .unwrap_or(existing.allow_private_network);
    if req.allow_private_network == Some(true) || (allow_priv && !existing.allow_private_network) {
        principal.require(Permission::GuardAdmin)?;
    }

    let input = build_update_channel_input(
        &state.keyring,
        &principal.tenant_id,
        &existing,
        existing_sealed.as_ref(),
        req,
    )?;
    let channel = state
        .store
        .update_channel(&principal.tenant_id, id, input)
        .await?
        .ok_or(AppError::NotFound("CHANNEL_NOT_FOUND"))?;
    emit_policy_audit(
        &state,
        &principal,
        "ax_notify_channel",
        &channel.id.to_string(),
        200,
        serde_json::json!({
            "op": "update",
            "kind": channel.kind.as_str(),
            "allow_private_network": channel.allow_private_network,
        }),
    );
    Ok(Json(channel))
}

async fn delete_channel_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let deleted = state.store.delete_channel(&principal.tenant_id, id).await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("CHANNEL_NOT_FOUND")));
    }
    emit_policy_audit(
        &state,
        &principal,
        "ax_notify_channel",
        &id.to_string(),
        200,
        serde_json::json!({ "op": "delete" }),
    );
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn test_channel_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<TestChannelResponse>, HttpError> {
    principal.require(Permission::RuleEdit)?;
    let dispatcher = NotificationDispatcher::new(
        Arc::new(state.store.clone()),
        Arc::new(NetGuard::new()),
        Arc::new(state.clone()),
    );
    let res = dispatcher.test_channel(&principal.tenant_id, id).await?;
    Ok(Json(res))
}

// ============================================================================
// Silences (`/api/axflow/silences*`)
// ============================================================================

async fn list_silences_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<SimplePageQuery>,
) -> Result<Json<PaginatedList<AlertSilenceRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_silences(&principal.tenant_id, query)
        .await?;
    Ok(Json(page))
}

async fn create_silence_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateSilenceRequest>,
) -> Result<(StatusCode, Json<AlertSilenceRecord>), HttpError> {
    principal.require(Permission::AlertAck)?;
    let rec = state
        .store
        .create_silence(&principal.tenant_id, Some(principal.user_id), req)
        .await?;
    emit_policy_audit(
        &state,
        &principal,
        "ax_alert_silence",
        &rec.id.to_string(),
        201,
        serde_json::json!({ "op": "create", "reason": rec.reason }),
    );
    Ok((StatusCode::CREATED, Json(rec)))
}

async fn delete_silence_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::AlertAck)?;
    let deleted = state.store.delete_silence(&principal.tenant_id, id).await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("SILENCE_NOT_FOUND")));
    }
    emit_policy_audit(
        &state,
        &principal,
        "ax_alert_silence",
        &id.to_string(),
        200,
        serde_json::json!({ "op": "delete" }),
    );
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ============================================================================
// Channel Credential Sealing Helpers
// ============================================================================

fn seal_channel_secrets(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    cred_id: Uuid,
    secrets: &BTreeMap<String, String>,
) -> Result<(i32, Vec<u8>, Vec<u8>), AppError> {
    let encoded = Zeroizing::new(
        serde_json::to_vec(secrets)
            .map_err(|err| AppError::Config(format!("serialize channel secrets failed: {err}")))?,
    );
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred_id);
    let sealed = keyring.seal_credential(cred_id, &aad, encoded.as_slice())?;
    Ok((sealed.key_version, sealed.nonce, sealed.ciphertext))
}

fn attach_secret_fields_meta(
    raw_config: serde_json::Value,
    secret_keys: impl Iterator<Item = String>,
) -> Result<serde_json::Value, AppError> {
    let serde_json::Value::Object(mut cfg_map) = raw_config else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "channel config must be a JSON object",
        ));
    };
    cfg_map.remove(INTERNAL_SECRET_FIELDS_KEY);
    let mut keys: Vec<String> = secret_keys.collect();
    keys.sort();
    keys.dedup();
    if !keys.is_empty() {
        let arr: Vec<serde_json::Value> = keys.into_iter().map(serde_json::Value::String).collect();
        cfg_map.insert(
            String::from(INTERNAL_SECRET_FIELDS_KEY),
            serde_json::Value::Array(arr),
        );
    }
    Ok(serde_json::Value::Object(cfg_map))
}

async fn build_create_channel_input(
    state: &AppState,
    tenant_id: &str,
    req: CreateNotifyChannelRequest,
) -> Result<SaveNotifyChannelInput, AppError> {
    let clean_secrets: BTreeMap<String, String> = req
        .secrets
        .into_iter()
        .filter_map(|(k, v)| {
            let kt = k.trim();
            let vt = v.trim();
            if kt.is_empty() || vt.is_empty() {
                None
            } else {
                Some((String::from(kt), String::from(vt)))
            }
        })
        .collect();

    if !clean_secrets.is_empty() {
        let cred_id = Uuid::new_v4();
        let sealed = seal_channel_secrets(&state.keyring, tenant_id, cred_id, &clean_secrets)?;
        let config = attach_secret_fields_meta(req.config, clean_secrets.keys().cloned())?;
        return Ok(SaveNotifyChannelInput {
            name: req.name,
            kind: req.kind,
            config,
            credential_id: Some(cred_id),
            new_sealed_secret: Some(sealed),
            clear_credential: false,
            allow_private_network: req.allow_private_network,
            enabled: req.enabled,
        });
    }

    if let Some(cid) = req.credential_id {
        let existing_cred = axpoint_domain::ports::ConnectionRepository::get_credential(
            &state.store,
            tenant_id,
            cid,
        )
        .await?
        .ok_or(AppError::NotFound("CREDENTIAL_NOT_FOUND"))?;
        let unsealed = open_secret_map(&state.keyring, tenant_id, Some(&existing_cred))?;
        let config = attach_secret_fields_meta(req.config, unsealed.keys().cloned())?;
        return Ok(SaveNotifyChannelInput {
            name: req.name,
            kind: req.kind,
            config,
            credential_id: Some(cid),
            new_sealed_secret: None,
            clear_credential: false,
            allow_private_network: req.allow_private_network,
            enabled: req.enabled,
        });
    }

    let config = attach_secret_fields_meta(req.config, std::iter::empty())?;
    Ok(SaveNotifyChannelInput {
        name: req.name,
        kind: req.kind,
        config,
        credential_id: None,
        new_sealed_secret: None,
        clear_credential: false,
        allow_private_network: req.allow_private_network,
        enabled: req.enabled,
    })
}

fn build_update_channel_input(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    existing: &NotifyChannelRecord,
    existing_sealed: Option<&SealedCredential>,
    req: UpdateNotifyChannelRequest,
) -> Result<SaveNotifyChannelInput, AppError> {
    let mut secrets_map = open_secret_map(keyring, tenant_id, existing_sealed)?;
    let mut secrets_changed = false;

    if let Some(incoming_secrets) = req.secrets {
        secrets_changed = true;
        if incoming_secrets.is_empty() {
            secrets_map.clear();
        } else {
            for (k, v) in incoming_secrets {
                let kt = k.trim();
                if kt.is_empty() {
                    continue;
                }
                let vt = v.trim();
                if vt.is_empty() {
                    secrets_map.remove(kt);
                } else {
                    secrets_map.insert(String::from(kt), String::from(vt));
                }
            }
        }
    }

    let base_config = req.config.unwrap_or_else(|| existing.config.clone());
    let config = attach_secret_fields_meta(base_config, secrets_map.keys().cloned())?;

    let (credential_id, new_sealed_secret, clear_credential) = if secrets_map.is_empty() {
        (None, None, existing.credential_id.is_some())
    } else if secrets_changed {
        let cred_id = existing.credential_id.unwrap_or_else(Uuid::new_v4);
        let sealed = seal_channel_secrets(keyring, tenant_id, cred_id, &secrets_map)?;
        (Some(cred_id), Some(sealed), false)
    } else {
        (existing.credential_id, None, false)
    };

    Ok(SaveNotifyChannelInput {
        name: req.name.unwrap_or_else(|| existing.name.clone()),
        kind: req.kind.unwrap_or(existing.kind),
        config,
        credential_id,
        new_sealed_secret,
        clear_credential,
        allow_private_network: req
            .allow_private_network
            .unwrap_or(existing.allow_private_network),
        enabled: req.enabled.unwrap_or(existing.enabled),
    })
}
