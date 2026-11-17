//! Authentication identity and personal access token (`PAT`) routes (`/api/auth/*`).

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    CreateApiTokenRequest, CreateApiTokenResponse, NewApiTokenInput, PersonalAccessTokenSummary,
};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, Permission};
use axpoint_domain::platform::{AuditRecord, MeResponse};
use axpoint_domain::ports::ApiTokenRepository as _;
use axpoint_security::{AuthPrincipal, sha256_hex};
use axum::extract::State;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use sha2::{Digest as _, Sha256};
use tracing::warn;
use uuid::Uuid;

use crate::state::{ApiJson, ApiPath, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Maximum permitted `expires_in_days` (~10 years) when issuing a personal access token.
const MAX_PAT_EXPIRES_DAYS: u32 = 3_650;

/// Sub-router mounted under `/api` for `/api/auth/me` and `/api/auth/tokens*`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(me_handler))
        .route(
            "/auth/tokens",
            get(list_tokens_handler).post(create_token_handler),
        )
        .route("/auth/tokens/{id}", delete(revoke_token_handler))
}

async fn me_handler(principal: AuthPrincipal) -> Result<Json<MeResponse>, HttpError> {
    Ok(Json(MeResponse {
        user_id: principal.user_id,
        tenant_id: principal.tenant_id,
        username: principal.username,
        dept_id: principal.dept_id,
        roles: principal.roles,
        permissions: principal.permissions,
        token_kind: principal.token_kind,
    }))
}

/// Generates a CSPRNG personal access token `(full_token, token_prefix)` in `axp_<prefix8>_<secret32>` format.
fn generate_pat_secret() -> (String, String) {
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

async fn record_auth_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    action: AuditAction,
    resource_ref: String,
    detail_json: serde_json::Value,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: Some(principal.user_id),
        username: principal.username.clone(),
        action,
        resource_ref,
        status_code: 200,
        client_ip: String::from("127.0.0.1"),
        trace_id: current_trace_id(),
        detail_json,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        warn!(error = %err, "auth audit flush failed");
    }
}

async fn list_tokens_handler(
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

async fn create_token_handler(
    State(state): State<AppState>,
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
                expires_at,
            },
        )
        .await?;

    record_auth_audit(
        &state,
        &principal,
        AuditAction::TokenIssue,
        format!("api_token:{}", summary.id),
        serde_json::json!({
            "name": summary.name,
            "token_prefix": summary.token_prefix,
            "scopes": summary.scopes,
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
        expires_at: summary.expires_at,
        created_at: summary.created_at,
    }))
}

async fn revoke_token_handler(
    State(state): State<AppState>,
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

    record_auth_audit(
        &state,
        &principal,
        AuditAction::TokenRevoke,
        format!("api_token:{token_id}"),
        serde_json::json!({ "token_id": token_id }),
    )
    .await;

    Ok(Json(serde_json::json!({ "deleted": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

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
