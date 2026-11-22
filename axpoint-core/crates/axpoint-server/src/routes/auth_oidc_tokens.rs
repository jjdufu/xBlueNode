//! Federated `OIDC` login (`start` / `callback`), password change, and personal access token (`PAT`)
//! handlers mounted under `/api/auth/*` (`11-STEP13` §13.3, §13.5, §13.10).

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    CreateApiTokenRequest, CreateApiTokenResponse, NewApiTokenInput, PersonalAccessTokenSummary,
};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, Permission};
use axpoint_domain::guard::{AuthSource, ChangePasswordRequest};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{ApiTokenRepository as _, GuardRepository as _};
use axpoint_security::password::{hash_password, validate_password_policy, verify_password};
use axpoint_security::session::{
    REFRESH_MAX_AGE_SECS, REFRESH_TTL_DAYS, build_clear_refresh_cookie, build_set_refresh_cookie,
    generate_refresh_token,
};
use axpoint_security::{AuthPrincipal, sha256_hex};
use axum::Json;
use axum::extract::State;
use axum::http::header::{HeaderValue, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tracing::warn;
use uuid::Uuid;

use super::{extract_client_ip, extract_user_agent, is_secure_cookie};
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Maximum permitted `expires_in_days` (~10 years) when issuing a personal access token.
const MAX_PAT_EXPIRES_DAYS: u32 = 3_650;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OidcStartQuery {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OidcCallbackQuery {
    code: String,
    state: String,
}

pub(super) async fn oidc_start_handler(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<OidcStartQuery>,
) -> Result<Response, HttpError> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or(AppError::NotFound("OIDC_NOT_CONFIGURED"))?;
    let tenant = query
        .tenant
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    let (authorize_url, login_state) = oidc.start_login(tenant, query.return_to.as_deref()).await?;
    state.store.save_oidc_login_state(&login_state).await?;

    let loc = HeaderValue::from_str(&authorize_url)
        .map_err(|err| AppError::Config(format!("invalid OIDC redirect URL: {err}")))?;
    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(LOCATION, loc);
    Ok(resp)
}

pub(super) async fn oidc_callback_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<OidcCallbackQuery>,
) -> Result<Response, HttpError> {
    let client_ip = extract_client_ip(&headers);
    state.auth_limiters.check_oidc_callback(
        &client_ip,
        state
            .authenticator
            .env_mode()
            .eq_ignore_ascii_case("production"),
    )?;

    let oidc = state
        .oidc
        .as_ref()
        .ok_or(AppError::NotFound("OIDC_NOT_CONFIGURED"))?;
    let login_state = state
        .store
        .consume_oidc_login_state(query.state.trim())
        .await?
        .ok_or_else(|| AppError::bad("OIDC_STATE_INVALID", "invalid or expired OIDC state"))?;

    let upsert_params = oidc.exchange_code(query.code.trim(), &login_state).await?;
    let profile = state
        .store
        .upsert_oidc_user(&login_state.tenant_id, &upsert_params)
        .await?;

    state
        .authenticator
        .invalidate_user(&profile.tenant_id, profile.user_id);
    state.cedar.invalidate_tenant(&profile.tenant_id);

    let (raw_rt, rt_hash) = generate_refresh_token();
    let family_id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(REFRESH_TTL_DAYS);
    let user_agent = extract_user_agent(&headers);
    let session_id = state
        .store
        .create_session(
            &profile.tenant_id,
            profile.user_id,
            family_id,
            &rt_hash,
            AuthSource::Oidc,
            (&user_agent, &client_ip, expires_at),
        )
        .await?;

    state.audit.record(AuditRecord {
        tenant_id: profile.tenant_id.clone(),
        user_id: Some(profile.user_id),
        username: profile.username.clone(),
        action: AuditAction::Login,
        resource_ref: format!("session:{session_id}"),
        status_code: 302,
        client_ip,
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({ "auth_source": "OIDC" }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        warn!(error = %err, "oidc login audit flush failed");
    }

    let set_cookie =
        build_set_refresh_cookie(&raw_rt, REFRESH_MAX_AGE_SECS, is_secure_cookie(&state));
    let cookie_val = HeaderValue::from_str(&set_cookie)
        .map_err(|err| AppError::Config(format!("invalid Set-Cookie header: {err}")))?;
    let loc_val = HeaderValue::from_str(&login_state.return_to)
        .map_err(|err| AppError::Config(format!("invalid return_to header: {err}")))?;

    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(SET_COOKIE, cookie_val);
    resp.headers_mut().insert(LOCATION, loc_val);
    Ok(resp)
}

pub(super) async fn change_password_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    ApiJson(req): ApiJson<ChangePasswordRequest>,
) -> Result<Response, HttpError> {
    if principal.user_id.is_nil() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "cannot change password for static dev token",
        )));
    }
    validate_password_policy(&principal.username, &req.new_password)?;

    let lookup = state
        .store
        .lookup_login(&principal.tenant_id, &principal.username)
        .await?
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "account does not use local password authentication",
            )
        })?;
    let stored_hash = lookup
        .password_hash
        .ok_or(AppError::Unauthorized("PASSWORD_INCORRECT"))?;

    let old_pw = req.old_password;
    let new_pw = req.new_password;
    let new_hash = tokio::task::spawn_blocking(move || -> Result<String, AppError> {
        if !verify_password(&old_pw, &stored_hash) {
            return Err(AppError::Unauthorized("PASSWORD_INCORRECT"));
        }
        hash_password(&new_pw)
    })
    .await
    .map_err(|err| AppError::Config(format!("password task join failed: {err}")))??;

    state
        .store
        .update_user_password(&principal.tenant_id, principal.user_id, &new_hash)
        .await?;
    state
        .authenticator
        .invalidate_user(&principal.tenant_id, principal.user_id);

    record_pat_or_pw_audit(
        &state,
        &principal,
        &extract_client_ip(&headers),
        AuditAction::PolicyChange,
        format!("user:{}:password", principal.user_id),
        serde_json::json!({ "event": "password_changed" }),
    )
    .await;

    let clear_cookie = build_clear_refresh_cookie(is_secure_cookie(&state));
    let cookie_val = HeaderValue::from_str(&clear_cookie)
        .map_err(|err| AppError::Config(format!("invalid Set-Cookie header: {err}")))?;
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    resp.headers_mut().insert(SET_COOKIE, cookie_val);
    Ok(resp)
}

/// Generates a CSPRNG personal access token `(full_token, token_prefix)` in `axp_<prefix8>_<secret32>` format.
pub(super) fn generate_pat_secret() -> (String, String) {
    let mut hasher = Sha256::new();
    hasher.update(Uuid::new_v4().as_bytes());
    hasher.update(Uuid::new_v4().as_bytes());
    hasher.update(Uuid::new_v4().as_bytes());
    let digest = hasher.finalize();

    let prefix8 = hex::encode(digest.get(..4).unwrap_or_default());
    let secret32 = hex::encode(digest.get(4..20).unwrap_or_default());
    let token_prefix = format!("axp_{prefix8}");
    let full_token = format!("{token_prefix}_{secret32}");
    (full_token, token_prefix)
}

async fn record_pat_or_pw_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    action: AuditAction,
    resource_ref: String,
    detail_json: serde_json::Value,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: (!principal.user_id.is_nil()).then_some(principal.user_id),
        username: principal.username.clone(),
        action,
        resource_ref,
        status_code: 200,
        client_ip: String::from(client_ip),
        trace_id: current_trace_id(),
        detail_json,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        warn!(error = %err, "auth audit flush failed");
    }
}

pub(super) async fn list_tokens_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<PersonalAccessTokenSummary>>, HttpError> {
    let items = state
        .store
        .list_user_api_tokens(&principal.tenant_id, principal.user_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn create_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateApiTokenRequest>,
) -> Result<Json<CreateApiTokenResponse>, HttpError> {
    let trimmed_name = req.name.trim();
    if trimmed_name.is_empty() || trimmed_name.len() > 128 {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "token name must be 1..=128 non-whitespace characters",
        )));
    }

    if !req
        .scopes
        .iter()
        .all(|perm| principal.permissions.contains(perm))
    {
        return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
    }

    let dedup_scopes: Vec<Permission> = req
        .scopes
        .into_iter()
        .collect::<BTreeSet<Permission>>()
        .into_iter()
        .collect();

    let expires_at = match req.expires_in_days {
        Some(0) => {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "expires_in_days must be between 1 and 3650",
            )));
        }
        Some(days) if days > MAX_PAT_EXPIRES_DAYS => {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                format!("expires_in_days cannot exceed {MAX_PAT_EXPIRES_DAYS}"),
            )));
        }
        Some(days) => Utc::now().checked_add_signed(Duration::days(i64::from(days))),
        None => None,
    };

    let (full_token, token_prefix) = generate_pat_secret();
    let token_hash = sha256_hex(&full_token);

    let summary = state
        .store
        .create_api_token(
            &principal.tenant_id,
            NewApiTokenInput {
                user_id: principal.user_id,
                name: trimmed_name.to_owned(),
                token_prefix,
                token_hash,
                scopes: dedup_scopes,
                token_profile: req.token_profile,
                web_base_url: req.web_base_url,
                expires_at,
            },
        )
        .await?;

    record_pat_or_pw_audit(
        &state,
        &principal,
        &extract_client_ip(&headers),
        AuditAction::TokenIssue,
        format!("api_token:{}", summary.id),
        serde_json::json!({
            "name": summary.name,
            "token_prefix": summary.token_prefix,
            "scopes": summary.scopes,
            "token_profile": summary.token_profile,
            "expires_at": summary.expires_at,
        }),
    )
    .await;

    Ok(Json(CreateApiTokenResponse {
        id: summary.id,
        name: summary.name,
        token_prefix: summary.token_prefix,
        token: full_token,
        scopes: summary.scopes,
        token_profile: summary.token_profile,
        web_base_url: summary.web_base_url,
        expires_at: summary.expires_at,
        created_at: summary.created_at,
    }))
}

pub(super) async fn revoke_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(token_id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let revoked = state
        .store
        .revoke_api_token(&principal.tenant_id, principal.user_id, token_id)
        .await?;
    if !revoked {
        return Err(HttpError(AppError::NotFound("TOKEN_NOT_FOUND")));
    }

    record_pat_or_pw_audit(
        &state,
        &principal,
        &extract_client_ip(&headers),
        AuditAction::TokenRevoke,
        format!("api_token:{token_id}"),
        serde_json::json!({ "token_id": token_id }),
    )
    .await;

    Ok(Json(serde_json::json!({ "deleted": true })))
}
