//! `AxGuard` security governance management routes (`/api/axguard/*`, `11-STEP13` §13.10).

mod grants_audit;
mod policies;
mod users_roles;

use axpoint_domain::enums::AuditAction;
use axpoint_domain::platform::AuditRecord;
use axpoint_security::AuthPrincipal;
use axum::Router;
use axum::http::HeaderMap;
use axum::routing::{delete, get, patch, post, put};
use chrono::Utc;
use serde::Serialize;
use tracing::warn;
use uuid::Uuid;

use crate::routes::auth::extract_client_ip;
use crate::state::AppState;
use crate::telemetry::current_trace_id;

#[derive(Debug, Serialize)]
pub(super) struct DeleteResponse {
    pub(super) deleted: bool,
}

pub(super) fn actor_uuid(principal: &AuthPrincipal) -> Option<Uuid> {
    (!principal.user_id.is_nil()).then_some(principal.user_id)
}

pub(super) async fn record_guard_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    headers: &HeaderMap,
    action: AuditAction,
    resource_ref: String,
    detail_json: serde_json::Value,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(principal),
        username: principal.username.clone(),
        action,
        resource_ref,
        status_code: 200,
        client_ip: extract_client_ip(headers),
        trace_id: current_trace_id(),
        detail_json,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        warn!(error = %err, "axguard audit flush failed");
    }
}

/// Builds the `/axguard/*` sub-router mounted under `/api` (`11-STEP13` §13.10).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .merge(identity_router())
        .merge(policy_router())
        .merge(grants_sessions_audit_router())
}

fn identity_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axguard/departments",
            get(users_roles::list_departments_handler),
        )
        .route(
            "/axguard/departments/{dept_id}",
            put(users_roles::upsert_department_handler)
                .delete(users_roles::delete_department_handler),
        )
        .route(
            "/axguard/users",
            get(users_roles::list_users_handler).post(users_roles::create_user_handler),
        )
        .route(
            "/axguard/users/{id}",
            patch(users_roles::patch_user_handler),
        )
        .route(
            "/axguard/users/{id}/reset-password",
            post(users_roles::reset_user_password_handler),
        )
        .route(
            "/axguard/users/{id}/attributes",
            put(users_roles::put_user_attributes_handler),
        )
        .route(
            "/axguard/roles",
            get(users_roles::list_roles_handler).post(users_roles::create_role_handler),
        )
        .route(
            "/axguard/roles/{id}",
            patch(users_roles::patch_role_handler).delete(users_roles::delete_role_handler),
        )
}

fn policy_router() -> Router<AppState> {
    Router::new()
        .route("/axguard/policies", get(policies::list_policies_handler))
        .route(
            "/axguard/policies/validate",
            post(policies::validate_policy_handler),
        )
        .route(
            "/axguard/policies/simulate",
            post(policies::simulate_policy_handler),
        )
        .route(
            "/axguard/policies/{name}",
            put(policies::put_policy_handler).delete(policies::delete_policy_handler),
        )
        .route(
            "/axguard/row-policies",
            get(policies::list_row_policies_handler).post(policies::create_row_policy_handler),
        )
        .route(
            "/axguard/row-policies/preview",
            post(policies::preview_row_policy_handler),
        )
        .route(
            "/axguard/row-policies/simulate-state",
            post(policies::simulate_state_row_policy_handler),
        )
        .route(
            "/axguard/row-policies/{id}",
            put(policies::update_row_policy_handler).delete(policies::delete_row_policy_handler),
        )
        .route(
            "/axguard/row-policy-defaults",
            get(policies::list_row_policy_defaults_handler)
                .put(policies::put_row_policy_default_handler),
        )
        .route(
            "/axguard/column-policies",
            get(policies::list_column_policies_handler).put(policies::put_column_policy_handler),
        )
        .route(
            "/axguard/column-policies/{id}",
            delete(policies::delete_column_policy_handler),
        )
}

fn grants_sessions_audit_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axguard/grants",
            get(grants_audit::list_grants_handler).post(grants_audit::create_grant_handler),
        )
        .route(
            "/axguard/grants/{id}",
            delete(grants_audit::revoke_grant_handler),
        )
        .route(
            "/axguard/sessions",
            get(grants_audit::list_sessions_handler),
        )
        .route(
            "/axguard/sessions/{family_id}",
            delete(grants_audit::revoke_session_family_handler),
        )
        .route("/axguard/audit", get(grants_audit::query_audit_handler))
        .route(
            "/axguard/audit/verify",
            get(grants_audit::verify_audit_chain_handler),
        )
        .route(
            "/axguard/audit/stats",
            get(grants_audit::query_audit_stats_handler),
        )
        .route(
            "/axguard/audit/export.csv",
            get(grants_audit::export_audit_csv_handler),
        )
        .route(
            "/axguard/debug/llm-egress-preview",
            post(grants_audit::llm_egress_preview_handler),
        )
}
