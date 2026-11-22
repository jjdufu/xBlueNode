//! Complete `/api/auth/*` routes: public config & `JWKS`, local `Argon2id` login with 5-strike
//! lockout, `EdDSA` `JWT` + `HttpOnly` refresh rotation with replay family revocation, `OIDC` SSO,
//! logout, password change, and personal access tokens (`11-STEP13` §13.2–§13.5, §13.10).

#[path = "auth_oidc_tokens.rs"]
mod auth_oidc_tokens;

use axpoint_agent::rate_limit::KeyedRateLimiter;
use axpoint_domain::AppError;
use axpoint_domain::enums::AuditAction;
use axpoint_domain::guard::{
    AuthConfigResponse, AuthSource, LoginLookupRow, LoginRequest, UserAccountStatus,
};
use axpoint_domain::platform::{AuditRecord, MeResponse};
use axpoint_domain::ports::GuardRepository as _;
use axpoint_security::AuthPrincipal;
use axpoint_security::password::{
    hash_password, needs_rehash, verify_dummy_password, verify_password,
};
use axpoint_security::session::{
    REFRESH_MAX_AGE_SECS, REFRESH_TTL_DAYS, build_clear_refresh_cookie, build_set_refresh_cookie,
    extract_refresh_cookie, generate_refresh_token, hash_refresh_token,
};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::{COOKIE, HeaderValue, SET_COOKIE, USER_AGENT};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use tracing::warn;
use uuid::Uuid;

use crate::state::{ApiJson, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Per-IP token-bucket rate limiters guarding unauthenticated `/api/auth/*` endpoints (`§13.2`).
#[derive(Debug)]
pub(crate) struct AuthRateLimiters {
    login: KeyedRateLimiter<String>,
    refresh: KeyedRateLimiter<String>,
    oidc_callback: KeyedRateLimiter<String>,
}

impl Default for AuthRateLimiters {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthRateLimiters {
    pub(crate) fn new() -> Self {
        Self {
            login: KeyedRateLimiter::new(16_384),
            refresh: KeyedRateLimiter::new(32_768),
            oidc_callback: KeyedRateLimiter::new(16_384),
        }
    }

    pub(crate) fn check_login(&self, ip: &str, is_prod: bool) -> Result<(), AppError> {
        let rpm = if is_prod { 10 } else { 120 };
        self.login.check_and_consume(String::from(ip), rpm)
    }

    pub(crate) fn check_refresh(&self, ip: &str, is_prod: bool) -> Result<(), AppError> {
        let rpm = if is_prod { 60 } else { 300 };
        self.refresh.check_and_consume(String::from(ip), rpm)
    }

    pub(crate) fn check_oidc_callback(&self, ip: &str, is_prod: bool) -> Result<(), AppError> {
        let rpm = if is_prod { 30 } else { 120 };
        self.oidc_callback.check_and_consume(String::from(ip), rpm)
    }
}

/// Sub-router mounted under `/api` for `/api/auth/*` (`11-STEP13` §13.10).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/config", get(auth_config_handler))
        .route("/auth/jwks.json", get(jwks_handler))
        .route("/auth/login", post(login_handler))
        .route("/auth/refresh", post(refresh_handler))
        .route("/auth/logout", post(logout_handler))
        .route("/auth/logout-all", post(logout_all_handler))
        .route(
            "/auth/password",
            post(auth_oidc_tokens::change_password_handler),
        )
        .route(
            "/auth/oidc/start",
            get(auth_oidc_tokens::oidc_start_handler),
        )
        .route(
            "/auth/oidc/callback",
            get(auth_oidc_tokens::oidc_callback_handler),
        )
        .route("/auth/me", get(me_handler))
        .route(
            "/auth/tokens",
            get(auth_oidc_tokens::list_tokens_handler).post(auth_oidc_tokens::create_token_handler),
        )
        .route(
            "/auth/tokens/{id}",
            delete(auth_oidc_tokens::revoke_token_handler),
        )
}

pub(crate) fn extract_client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| String::from("127.0.0.1"), String::from)
}

pub(crate) fn extract_user_agent(headers: &HeaderMap) -> String {
    headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .map_or_else(String::new, |s| s.chars().take(256).collect())
}

pub(crate) fn is_secure_cookie(state: &AppState) -> bool {
    state
        .authenticator
        .env_mode()
        .eq_ignore_ascii_case("production")
}

async fn auth_config_handler(State(state): State<AppState>) -> Json<AuthConfigResponse> {
    Json(AuthConfigResponse {
        env: state.authenticator.env_mode().to_owned(),
        oidc_enabled: state.oidc.is_some(),
        oidc_display_name: state.oidc.as_ref().map(|o| o.display_name().to_owned()),
        default_tenant: String::from("default"),
    })
}

async fn jwks_handler(State(state): State<AppState>) -> Result<Json<serde_json::Value>, HttpError> {
    let jwks = state.jwt_keys.jwks_json()?;
    Ok(Json(jwks))
}

async fn me_handler(principal: AuthPrincipal) -> Result<Json<MeResponse>, HttpError> {
    Ok(Json(principal.to_me_response()))
}

async fn run_dummy_argon2(password: String) {
    if let Err(err) = tokio::task::spawn_blocking(move || verify_dummy_password(&password)).await {
        warn!(error = %err, "dummy password verify join failed");
    }
}

struct AuthAuditEntry<'a> {
    tenant_id: &'a str,
    user_id: Option<Uuid>,
    username: &'a str,
    action: AuditAction,
    resource_ref: String,
    status_code: i32,
    client_ip: &'a str,
    detail_json: serde_json::Value,
}

async fn record_unauth_audit(state: &AppState, entry: AuthAuditEntry<'_>) {
    state.audit.record(AuditRecord {
        tenant_id: String::from(entry.tenant_id),
        user_id: entry.user_id,
        username: String::from(entry.username),
        action: entry.action,
        resource_ref: entry.resource_ref,
        status_code: entry.status_code,
        client_ip: String::from(entry.client_ip),
        trace_id: current_trace_id(),
        detail_json: entry.detail_json,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        warn!(error = %err, "auth event audit flush failed");
    }
}

async fn login_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<LoginRequest>,
) -> Result<Response, HttpError> {
    let client_ip = extract_client_ip(&headers);
    let is_prod = is_secure_cookie(&state);
    state.auth_limiters.check_login(&client_ip, is_prod)?;

    let tenant = if req.tenant.trim().is_empty() {
        "default"
    } else {
        req.tenant.trim()
    };
    let username = req.username.trim();
    if username.is_empty() || req.password.is_empty() {
        run_dummy_argon2(req.password).await;
        return Err(HttpError(AppError::Unauthorized("INVALID_CREDENTIALS")));
    }

    let Some(lookup) = state.store.lookup_login(tenant, username).await? else {
        run_dummy_argon2(req.password).await;
        record_unauth_audit(
            &state,
            AuthAuditEntry {
                tenant_id: tenant,
                user_id: None,
                username,
                action: AuditAction::Login,
                resource_ref: String::from("auth:login"),
                status_code: 401,
                client_ip: &client_ip,
                detail_json: serde_json::json!({ "reason": "unknown_user" }),
            },
        )
        .await;
        return Err(HttpError(AppError::Unauthorized("INVALID_CREDENTIALS")));
    };

    complete_local_login(
        &state,
        &headers,
        (&client_ip, tenant, username),
        req.password,
        lookup,
    )
    .await
}

async fn verify_and_upgrade_password(
    password: String,
    stored_phc: String,
) -> Result<(bool, Option<String>), AppError> {
    tokio::task::spawn_blocking(move || -> Result<(bool, Option<String>), AppError> {
        if !verify_password(&password, &stored_phc) {
            return Ok((false, None));
        }
        let next_hash = if needs_rehash(&stored_phc) {
            Some(hash_password(&password)?)
        } else {
            None
        };
        Ok((true, next_hash))
    })
    .await
    .map_err(|err| AppError::Config(format!("login verify join failed: {err}")))?
}

async fn complete_local_login(
    state: &AppState,
    headers: &HeaderMap,
    caller: (&str, &str, &str),
    password: String,
    lookup: LoginLookupRow,
) -> Result<Response, HttpError> {
    let (client_ip, tenant, username) = caller;
    let is_locked = lookup.locked_until.is_some_and(|until| until > Utc::now());
    if lookup.status != "ACTIVE" || is_locked {
        run_dummy_argon2(password).await;
        record_unauth_audit(
            state,
            AuthAuditEntry {
                tenant_id: tenant,
                user_id: Some(lookup.user_id),
                username,
                action: AuditAction::Login,
                resource_ref: format!("user:{}", lookup.user_id),
                status_code: 401,
                client_ip,
                detail_json: serde_json::json!({ "locked": is_locked, "status": lookup.status }),
            },
        )
        .await;
        return Err(HttpError(AppError::Unauthorized("INVALID_CREDENTIALS")));
    }

    let Some(stored_phc) = lookup.password_hash else {
        run_dummy_argon2(password).await;
        return Err(HttpError(AppError::Unauthorized("INVALID_CREDENTIALS")));
    };

    let (matched, upgraded_hash) = verify_and_upgrade_password(password, stored_phc).await?;
    if !matched {
        let failures = state
            .store
            .record_login_failure(tenant, lookup.user_id)
            .await?;
        record_unauth_audit(
            state,
            AuthAuditEntry {
                tenant_id: tenant,
                user_id: Some(lookup.user_id),
                username,
                action: AuditAction::Login,
                resource_ref: format!("user:{}", lookup.user_id),
                status_code: 401,
                client_ip,
                detail_json: serde_json::json!({ "failed_logins": failures, "locked": failures >= 5 }),
            },
        )
        .await;
        return Err(HttpError(AppError::Unauthorized("INVALID_CREDENTIALS")));
    }

    state
        .store
        .record_login_success(tenant, lookup.user_id, upgraded_hash.as_deref())
        .await?;
    let profile = state
        .store
        .load_guard_principal(tenant, lookup.user_id)
        .await?
        .ok_or(AppError::Unauthorized("INVALID_CREDENTIALS"))?;

    let (raw_rt, rt_hash) = generate_refresh_token();
    let family_id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(REFRESH_TTL_DAYS);
    let user_agent = extract_user_agent(headers);
    let session_id = state
        .store
        .create_session(
            tenant,
            profile.user_id,
            family_id,
            &rt_hash,
            AuthSource::Local,
            (&user_agent, client_ip, expires_at),
        )
        .await?;

    let token_resp = state.jwt_keys.sign_access_token(&profile)?;
    record_unauth_audit(
        state,
        AuthAuditEntry {
            tenant_id: tenant,
            user_id: Some(profile.user_id),
            username: &profile.username,
            action: AuditAction::Login,
            resource_ref: format!("session:{session_id}"),
            status_code: 200,
            client_ip,
            detail_json: serde_json::json!({ "auth_source": "LOCAL" }),
        },
    )
    .await;

    build_token_cookie_response(state, &raw_rt, &token_resp)
}

fn build_token_cookie_response(
    state: &AppState,
    raw_refresh: &str,
    token_resp: &axpoint_domain::guard::AccessTokenResponse,
) -> Result<Response, HttpError> {
    let set_cookie =
        build_set_refresh_cookie(raw_refresh, REFRESH_MAX_AGE_SECS, is_secure_cookie(state));
    let cookie_val = HeaderValue::from_str(&set_cookie)
        .map_err(|err| AppError::Config(format!("invalid Set-Cookie header: {err}")))?;
    let mut resp = Json(token_resp.clone()).into_response();
    resp.headers_mut().insert(SET_COOKIE, cookie_val);
    Ok(resp)
}

async fn refresh_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    let client_ip = extract_client_ip(&headers);
    state
        .auth_limiters
        .check_refresh(&client_ip, is_secure_cookie(&state))?;

    let cookie_hdr = headers.get(COOKIE).and_then(|v| v.to_str().ok());
    let raw_rt =
        extract_refresh_cookie(cookie_hdr).ok_or(AppError::Unauthorized("TOKEN_MISSING"))?;
    let rt_hash = hash_refresh_token(&raw_rt);

    let resolved = state
        .store
        .resolve_session_by_hash(&rt_hash)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;

    if resolved.rotated_at.is_some() || resolved.revoked_at.is_some() {
        revoke_replayed_family(&state, &resolved, &client_ip).await?;
        return Err(HttpError(AppError::Unauthorized("TOKEN_INVALID")));
    }

    if resolved.expires_at <= Utc::now() {
        return Err(HttpError(AppError::Unauthorized("TOKEN_EXPIRED")));
    }

    let profile = state
        .store
        .load_guard_principal(&resolved.tenant_id, resolved.user_id)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
    if profile.status != UserAccountStatus::Active {
        return Err(HttpError(AppError::Unauthorized("TOKEN_INVALID")));
    }

    let (new_raw_rt, new_rt_hash) = generate_refresh_token();
    let new_expires_at = Utc::now() + Duration::days(REFRESH_TTL_DAYS);
    let user_agent = extract_user_agent(&headers);
    let rotated = state
        .store
        .rotate_session(
            &resolved.tenant_id,
            resolved.session_id,
            resolved.user_id,
            resolved.family_id,
            &new_rt_hash,
            (&user_agent, &client_ip, new_expires_at),
        )
        .await?;
    if !rotated {
        revoke_replayed_family(&state, &resolved, &client_ip).await?;
        return Err(HttpError(AppError::Unauthorized("TOKEN_INVALID")));
    }

    let token_resp = state.jwt_keys.sign_access_token(&profile)?;
    build_token_cookie_response(&state, &new_raw_rt, &token_resp)
}

async fn revoke_replayed_family(
    state: &AppState,
    resolved: &axpoint_domain::guard::ResolvedSessionRow,
    client_ip: &str,
) -> Result<(), AppError> {
    let _revoked_count = state
        .store
        .revoke_session_family(&resolved.tenant_id, resolved.family_id)
        .await?;
    record_unauth_audit(
        state,
        AuthAuditEntry {
            tenant_id: &resolved.tenant_id,
            user_id: Some(resolved.user_id),
            username: "session",
            action: AuditAction::TokenRevoke,
            resource_ref: format!("session_family:{}", resolved.family_id),
            status_code: 401,
            client_ip,
            detail_json: serde_json::json!({
                "reason": "refresh_reuse",
                "session_id": resolved.session_id,
                "family_id": resolved.family_id,
            }),
        },
    )
    .await;
    Ok(())
}

async fn logout_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    let client_ip = extract_client_ip(&headers);
    let cookie_hdr = headers.get(COOKIE).and_then(|v| v.to_str().ok());
    if let Some(raw_rt) = extract_refresh_cookie(cookie_hdr) {
        let rt_hash = hash_refresh_token(&raw_rt);
        if let Some(resolved) = state.store.resolve_session_by_hash(&rt_hash).await? {
            let _ = state
                .store
                .revoke_session_family(&resolved.tenant_id, resolved.family_id)
                .await?;
            record_unauth_audit(
                &state,
                AuthAuditEntry {
                    tenant_id: &resolved.tenant_id,
                    user_id: Some(resolved.user_id),
                    username: "session",
                    action: AuditAction::Logout,
                    resource_ref: format!("session_family:{}", resolved.family_id),
                    status_code: 200,
                    client_ip: &client_ip,
                    detail_json: serde_json::json!({ "scope": "family" }),
                },
            )
            .await;
        }
    }

    let clear_cookie = build_clear_refresh_cookie(is_secure_cookie(&state));
    let cookie_val = HeaderValue::from_str(&clear_cookie)
        .map_err(|err| AppError::Config(format!("invalid Set-Cookie header: {err}")))?;
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    resp.headers_mut().insert(SET_COOKIE, cookie_val);
    Ok(resp)
}

async fn logout_all_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
) -> Result<Response, HttpError> {
    let client_ip = extract_client_ip(&headers);
    if !principal.user_id.is_nil() {
        let revoked = state
            .store
            .revoke_all_user_sessions(&principal.tenant_id, principal.user_id)
            .await?;
        state
            .authenticator
            .invalidate_user(&principal.tenant_id, principal.user_id);
        record_unauth_audit(
            &state,
            AuthAuditEntry {
                tenant_id: &principal.tenant_id,
                user_id: Some(principal.user_id),
                username: &principal.username,
                action: AuditAction::Logout,
                resource_ref: format!("user:{}", principal.user_id),
                status_code: 200,
                client_ip: &client_ip,
                detail_json: serde_json::json!({ "scope": "all", "revoked_sessions": revoked }),
            },
        )
        .await;
    }

    let clear_cookie = build_clear_refresh_cookie(is_secure_cookie(&state));
    let cookie_val = HeaderValue::from_str(&clear_cookie)
        .map_err(|err| AppError::Config(format!("invalid Set-Cookie header: {err}")))?;
    let mut resp = Json(serde_json::json!({ "ok": true })).into_response();
    resp.headers_mut().insert(SET_COOKIE, cookie_val);
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::auth_oidc_tokens::generate_pat_secret;

    #[test]
    fn generated_pat_matches_axp_prefix8_secret32_format() {
        let (tok_a, pref_a) = generate_pat_secret();
        let (tok_b, pref_b) = generate_pat_secret();
        assert_ne!(tok_a, tok_b);
        assert_ne!(pref_a, pref_b);
        assert!(tok_a.starts_with("axp_"));
        assert_eq!(pref_a.len(), 12);
        assert_eq!(tok_a.len(), 45);
        let parts: Vec<&str> = tok_a.split('_').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.first().copied(), Some("axp"));
        assert_eq!(parts.get(1).copied().map(str::len), Some(8));
        assert_eq!(parts.get(2).copied().map(str::len), Some(32));
    }
}
