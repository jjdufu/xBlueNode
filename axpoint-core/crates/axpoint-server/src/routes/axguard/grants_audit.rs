//! Cross-department `sec_grants`, active `sec_sessions`, tamper-evident `sec_audit_logs` explorer /
//! chain verifier / CSV export, and non-production LLM egress preview handlers (`11-STEP13` §13.8.3–§13.10).

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, Permission};
use axpoint_domain::guard::{
    AuditLogEntry, AuditQueryFilter, AuditStatsResponse, AuditVerifyQuery, AuditVerifyResponse,
    CreateGrantRequest, GrantRecord, GranteeKind, LlmEgressPreviewRequest,
    LlmEgressPreviewResponse, SessionSummaryRecord,
};
use axpoint_domain::ports::GuardRepository as _;
use axpoint_security::AuthPrincipal;
use axpoint_security::row_policy::validate_predicate_syntax;
use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, HeaderValue, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tracing::warn;
use uuid::Uuid;

use super::{DeleteResponse, record_guard_audit};
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};

const MAX_AUDIT_EXPORT_ROWS: usize = 100_000;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionsListQuery {
    #[serde(default)]
    user_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuditStatsQuery {
    #[serde(default)]
    from: Option<DateTime<Utc>>,
    #[serde(default)]
    to: Option<DateTime<Utc>>,
}

pub(super) async fn list_grants_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<GrantRecord>>, HttpError> {
    let viewer_filter = if principal.has(Permission::GuardAdmin) {
        None
    } else {
        principal.require(Permission::DataRead)?;
        Some((principal.user_id, principal.dept_id.as_str()))
    };
    let items = state
        .store
        .list_grants(&principal.tenant_id, viewer_filter)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn create_grant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateGrantRequest>,
) -> Result<(StatusCode, Json<GrantRecord>), HttpError> {
    principal.require(Permission::DataRead)?;

    if !principal.has(Permission::GuardAdmin) {
        let g_ref = req.grantee_ref.trim();
        let allowed_self_or_dept = match req.grantee_kind {
            GranteeKind::User => g_ref.eq_ignore_ascii_case(&principal.user_id.to_string()),
            GranteeKind::Dept => g_ref.eq_ignore_ascii_case(&principal.dept_id),
            GranteeKind::Role => principal
                .roles
                .iter()
                .any(|r| r.eq_ignore_ascii_case(g_ref)),
        };
        if !allowed_self_or_dept {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
    }

    if let Some(ref pred) = req.row_predicate
        && !pred.trim().is_empty()
    {
        validate_predicate_syntax(pred, None)?;
    }

    let created = state
        .store
        .create_grant(&principal.tenant_id, principal.user_id, req)
        .await?;

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("grant:{}", created.id),
        serde_json::json!({
            "event": "grant_requested",
            "resource_kind": created.resource_kind,
            "resource_ref": created.resource_ref,
            "grantee_kind": created.grantee_kind,
            "grantee_ref": created.grantee_ref,
            "approval_id": created.approval_id,
        }),
    )
    .await;

    Ok((StatusCode::CREATED, Json(created)))
}

pub(super) async fn revoke_grant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(grant_id): ApiPath<Uuid>,
) -> Result<Json<DeleteResponse>, HttpError> {
    if !principal.has(Permission::GuardAdmin) {
        principal.require(Permission::DataRead)?;
        let visible = state
            .store
            .list_grants(
                &principal.tenant_id,
                Some((principal.user_id, principal.dept_id.as_str())),
            )
            .await?;
        let owns_grant = visible
            .iter()
            .any(|g| g.id == grant_id && g.requested_by == principal.user_id);
        if !owns_grant {
            return Err(HttpError(AppError::Forbidden("PERMISSION_DENIED")));
        }
    }

    let revoked = state
        .store
        .revoke_grant(&principal.tenant_id, grant_id)
        .await?;
    if !revoked {
        return Err(HttpError(AppError::NotFound("GRANT_NOT_FOUND")));
    }
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("grant:{grant_id}"),
        serde_json::json!({ "event": "grant_revoked" }),
    )
    .await;

    Ok(Json(DeleteResponse { deleted: true }))
}

pub(super) async fn list_sessions_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<SessionsListQuery>,
) -> Result<Json<PaginatedList<SessionSummaryRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state
        .store
        .list_sessions(&principal.tenant_id, query.user_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn revoke_session_family_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(target_id): ApiPath<Uuid>,
) -> Result<Json<DeleteResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let mut count = state
        .store
        .revoke_session_family(&principal.tenant_id, target_id)
        .await?;

    if count == 0 {
        let all = state
            .store
            .list_sessions(&principal.tenant_id, None)
            .await?;
        if let Some(found) = all.iter().find(|s| s.id == target_id) {
            count = state
                .store
                .revoke_session_family(&principal.tenant_id, found.family_id)
                .await?;
        }
    }

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::TokenRevoke,
        format!("session_family:{target_id}"),
        serde_json::json!({ "revoked_rows": count }),
    )
    .await;

    Ok(Json(DeleteResponse { deleted: true }))
}

async fn flush_pending_audit(state: &AppState) {
    if let Err(err) = state.audit.flush().await {
        warn!(error = %err, "pre-query audit flush failed");
    }
}

fn require_audit_access(principal: &AuthPrincipal) -> Result<(), AppError> {
    if principal.has(Permission::GuardAdmin) || principal.has(Permission::OpsAdmin) {
        Ok(())
    } else {
        Err(AppError::Forbidden("PERMISSION_DENIED"))
    }
}

pub(super) async fn query_audit_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(filter): ApiQuery<AuditQueryFilter>,
) -> Result<Json<PaginatedList<AuditLogEntry>>, HttpError> {
    require_audit_access(&principal)?;
    flush_pending_audit(&state).await;
    let page = state
        .store
        .query_audit_logs(&principal.tenant_id, &filter)
        .await?;
    Ok(Json(page))
}

pub(super) async fn verify_audit_chain_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<AuditVerifyQuery>,
) -> Result<Json<AuditVerifyResponse>, HttpError> {
    require_audit_access(&principal)?;
    flush_pending_audit(&state).await;
    let res = state
        .store
        .verify_audit_chain(&principal.tenant_id, &query)
        .await?;
    Ok(Json(res))
}

pub(super) async fn query_audit_stats_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<AuditStatsQuery>,
) -> Result<Json<AuditStatsResponse>, HttpError> {
    require_audit_access(&principal)?;
    flush_pending_audit(&state).await;
    let audit_stats = state
        .store
        .query_audit_stats(&principal.tenant_id, query.from, query.to)
        .await?;
    Ok(Json(audit_stats))
}

pub(super) async fn export_audit_csv_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(mut filter): ApiQuery<AuditQueryFilter>,
) -> Result<Response, HttpError> {
    require_audit_access(&principal)?;
    flush_pending_audit(&state).await;

    filter.limit = Some(500);
    let mut csv_buf = String::from(
        "id,created_at,username,user_id,action,resource_ref,status_code,client_ip,trace_id,prev_hash,row_hash,detail_json\n",
    );
    let mut exported_rows = 0_usize;

    loop {
        let page = state
            .store
            .query_audit_logs(&principal.tenant_id, &filter)
            .await?;
        for item in &page.items {
            append_audit_csv_row(&mut csv_buf, item);
            exported_rows = exported_rows.saturating_add(1);
            if exported_rows >= MAX_AUDIT_EXPORT_ROWS {
                break;
            }
        }
        if exported_rows >= MAX_AUDIT_EXPORT_ROWS || page.next_cursor.is_none() {
            break;
        }
        filter.cursor = page.next_cursor;
    }

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::ExportFile,
        String::from("axguard:audit:export.csv"),
        serde_json::json!({ "rows": exported_rows, "format": "CSV" }),
    )
    .await;

    let mut resp = Response::new(Body::from(csv_buf));
    let resp_headers = resp.headers_mut();
    resp_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    resp_headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"audit_logs.csv\""),
    );
    resp_headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(resp)
}

fn csv_escape_into(out: &mut String, val: &str) {
    if val.contains([',', '"', '\n', '\r']) {
        out.push('"');
        for ch in val.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(val);
    }
}

fn append_audit_csv_row(out: &mut String, row: &AuditLogEntry) {
    use std::fmt::Write as _;

    let _ok_id = write!(out, "{},", row.id);
    csv_escape_into(out, &row.created_at.to_rfc3339());
    out.push(',');
    csv_escape_into(out, &row.username);
    out.push(',');
    if let Some(uid) = row.user_id {
        let _ok_uid = write!(out, "{uid}");
    }
    out.push(',');
    csv_escape_into(out, row.action.as_str());
    out.push(',');
    csv_escape_into(out, &row.resource_ref);
    let _ok_sc = write!(out, ",{},", row.status_code);
    csv_escape_into(out, &row.client_ip);
    out.push(',');
    csv_escape_into(out, &row.trace_id);
    out.push(',');
    csv_escape_into(out, &row.prev_hash);
    out.push(',');
    csv_escape_into(out, &row.row_hash);
    out.push(',');
    let detail_raw = serde_json::to_string(&row.detail_json).unwrap_or_default();
    csv_escape_into(out, &detail_raw);
    out.push('\n');
}

pub(super) async fn llm_egress_preview_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<LlmEgressPreviewRequest>,
) -> Result<Json<LlmEgressPreviewResponse>, HttpError> {
    if state
        .authenticator
        .env_mode()
        .eq_ignore_ascii_case("production")
    {
        return Err(HttpError(AppError::NotFound("ROUTE_NOT_FOUND")));
    }
    principal.require(Permission::GuardAdmin)?;
    let resp = axpoint_llm::router::preview_egress_filter(&req.text, &principal.tenant_id);
    Ok(Json(resp))
}
