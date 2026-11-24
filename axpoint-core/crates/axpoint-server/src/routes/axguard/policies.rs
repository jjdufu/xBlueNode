//! `Cedar` `ABAC` policy management, simulation, row-level policy CRUD & 20-row live preview,
//! default-deny toggles, and column-masking policy handlers (`/api/axguard/*`, `11-STEP13` §13.6–§13.7, §13.10).

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::analytics::QueryRequest;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, DataType, Permission, Sensitivity};
use axpoint_domain::guard::{
    CedarPolicyRecord, ColumnPolicyRecord, PolicyTargetKind, PutCedarPolicyRequest,
    PutColumnPolicyRequest, PutRowPolicyDefaultRequest, ResourceRef, RowPolicyDefaultRecord,
    RowPolicyPreviewRequest, RowPolicyRecord, SaveRowPolicyRequest, SimulateCedarPolicyRequest,
    SimulateCedarPolicyResponse, StateRowPolicySimulateRequest, StateRowPolicySimulateResponse,
    ValidateCedarPolicyRequest, ValidateCedarPolicyResponse,
};
use axpoint_domain::ports::{DatasetRepository as _, GuardRepository as _, StudioRepository as _};
use axpoint_security::AuthPrincipal;
use axpoint_security::cedar_authz::{CedarContext, dept_lineage};
use axpoint_security::row_policy::validate_predicate_syntax;
use axpoint_server::policy::{
    build_policy_context_for_profile, current_utc_hour, has_matching_grant,
};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{DeleteResponse, actor_uuid, record_guard_audit};
use crate::routes::analytics::acquire_tenant_session;
use crate::routes::auth::extract_client_ip;
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ColumnPolicyListQuery {
    #[serde(default)]
    target_kind: Option<PolicyTargetKind>,
    #[serde(default)]
    target_ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct PreviewColumnInfo {
    name: String,
    data_type: DataType,
}

#[derive(Debug, Serialize)]
pub(super) struct RowPolicyPreviewResponse {
    columns: Vec<PreviewColumnInfo>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    row_count: usize,
}

pub(super) async fn list_policies_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<CedarPolicyRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state
        .store
        .list_cedar_policies(&principal.tenant_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn put_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(name): ApiPath<String>,
    ApiJson(req): ApiJson<PutCedarPolicyRequest>,
) -> Result<Json<CedarPolicyRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    state.cedar.ensure_valid_policy_text(&req.policy_text)?;

    let saved = state
        .store
        .put_cedar_policy(
            &principal.tenant_id,
            name.trim(),
            actor_uuid(&principal),
            req,
        )
        .await?;
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("cedar_policy:{}", saved.name),
        serde_json::json!({ "version": saved.version, "enabled": saved.enabled }),
    )
    .await;
    Ok(Json(saved))
}

pub(super) async fn delete_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<DeleteResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let clean = name.trim();
    let deleted = state
        .store
        .delete_cedar_policy(&principal.tenant_id, clean)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("POLICY_NOT_FOUND")));
    }
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("cedar_policy:{clean}"),
        serde_json::json!({ "deleted": true }),
    )
    .await;
    Ok(Json(DeleteResponse { deleted: true }))
}

pub(super) async fn validate_policy_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<ValidateCedarPolicyRequest>,
) -> Result<Json<ValidateCedarPolicyResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    Ok(Json(state.cedar.validate_policy_text(&req.policy_text)))
}

pub(super) async fn simulate_policy_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<SimulateCedarPolicyRequest>,
) -> Result<Json<SimulateCedarPolicyResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let profile = state
        .store
        .load_guard_principal(&principal.tenant_id, req.user_id)
        .await?
        .ok_or(AppError::NotFound("USER_NOT_FOUND"))?;

    let mut res_ref = state
        .store
        .resolve_resource_ref(&principal.tenant_id, req.resource_kind, &req.resource_ref)
        .await?
        .unwrap_or_else(|| ResourceRef {
            kind: req.resource_kind,
            id: req.resource_ref.trim().to_owned(),
            owner_dept: String::from("default"),
            sensitivity: Sensitivity::Internal,
        });
    if let Some(ref dept) = req.owner_dept
        && !dept.trim().is_empty()
    {
        res_ref.owner_dept = dept.trim().to_owned();
    }
    if let Some(sens) = req.sensitivity {
        res_ref.sensitivity = sens;
    }

    let (departments, custom_policies, active_grants) = tokio::try_join!(
        state.store.list_departments(&principal.tenant_id),
        state.store.list_cedar_policies(&principal.tenant_id),
        state.store.list_active_grants(&principal.tenant_id),
    )?;
    let lineage = dept_lineage(&profile.dept_id, &departments);
    let granted = has_matching_grant(
        &active_grants,
        req.resource_kind,
        &res_ref.id,
        &req.resource_ref,
        req.action,
        &profile,
        &lineage,
    );
    let ctx = CedarContext {
        ip: req.client_ip.unwrap_or_else(|| String::from("127.0.0.1")),
        hour: req.hour.unwrap_or_else(current_utc_hour).min(23),
        mfa: false,
        granted,
    };

    Ok(Json(state.cedar.evaluate(
        &profile,
        req.action,
        &res_ref,
        &ctx,
        &departments,
        &custom_policies,
    )))
}

async fn validate_row_policy_target(
    state: &AppState,
    tenant_id: &str,
    target_kind: PolicyTargetKind,
    target_ref: &str,
    predicate: &str,
) -> Result<(), AppError> {
    let clean_ref = target_ref.trim();
    if clean_ref.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "target_ref cannot be empty",
        ));
    }
    match target_kind {
        PolicyTargetKind::Dataset => {
            let ds = if let Ok(ds_uuid) = Uuid::parse_str(clean_ref) {
                state.store.get_dataset(tenant_id, ds_uuid).await?
            } else {
                state
                    .store
                    .get_dataset_by_name(tenant_id, clean_ref)
                    .await?
            };
            if let Some(found) = ds {
                let cols = state.store.list_columns(tenant_id, found.id).await?;
                let allowed: BTreeSet<String> = cols
                    .iter()
                    .map(|c| c.column_name.to_ascii_lowercase())
                    .collect();
                validate_predicate_syntax(predicate, (!allowed.is_empty()).then_some(&allowed))
            } else {
                validate_predicate_syntax(predicate, None)
            }
        }
        PolicyTargetKind::NodeType => {
            let published = state.store.get_published_schema(tenant_id, "main").await?;
            if let Some(nt) = published
                .doc
                .node_types
                .iter()
                .find(|n| n.name.eq_ignore_ascii_case(clean_ref))
            {
                let mut allowed: BTreeSet<String> = nt
                    .properties
                    .iter()
                    .map(|p| p.name.to_ascii_lowercase())
                    .collect();
                allowed.insert(String::from("node_key"));
                allowed.insert(String::from("title"));
                validate_predicate_syntax(predicate, Some(&allowed))
            } else {
                validate_predicate_syntax(predicate, None)
            }
        }
    }
}

pub(super) async fn list_row_policies_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<RowPolicyRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state.store.list_row_policies(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn create_row_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<SaveRowPolicyRequest>,
) -> Result<(StatusCode, Json<RowPolicyRecord>), HttpError> {
    principal.require(Permission::GuardAdmin)?;
    validate_row_policy_target(
        &state,
        &principal.tenant_id,
        req.target_kind,
        &req.target_ref,
        &req.predicate,
    )
    .await?;

    let created = state
        .store
        .create_row_policy(&principal.tenant_id, req)
        .await?;
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("row_policy:{}", created.id),
        serde_json::json!({
            "name": created.name,
            "target_kind": created.target_kind,
            "target_ref": created.target_ref,
        }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(super) async fn update_row_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(policy_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<SaveRowPolicyRequest>,
) -> Result<Json<RowPolicyRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    validate_row_policy_target(
        &state,
        &principal.tenant_id,
        req.target_kind,
        &req.target_ref,
        &req.predicate,
    )
    .await?;

    let updated = state
        .store
        .update_row_policy(&principal.tenant_id, policy_id, req)
        .await?
        .ok_or(AppError::NotFound("POLICY_NOT_FOUND"))?;
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("row_policy:{policy_id}"),
        serde_json::json!({ "name": updated.name, "enabled": updated.enabled }),
    )
    .await;
    Ok(Json(updated))
}

pub(super) async fn delete_row_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(policy_id): ApiPath<Uuid>,
) -> Result<Json<DeleteResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let deleted = state
        .store
        .delete_row_policy(&principal.tenant_id, policy_id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("POLICY_NOT_FOUND")));
    }
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("row_policy:{policy_id}"),
        serde_json::json!({ "deleted": true }),
    )
    .await;
    Ok(Json(DeleteResponse { deleted: true }))
}

pub(super) async fn list_row_policy_defaults_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<RowPolicyDefaultRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state
        .store
        .list_row_policy_defaults(&principal.tenant_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn put_row_policy_default_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<PutRowPolicyDefaultRequest>,
) -> Result<Json<RowPolicyDefaultRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let saved = state
        .store
        .put_row_policy_default(&principal.tenant_id, req)
        .await?;
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!(
            "row_policy_default:{}:{}",
            saved.target_kind.as_str(),
            saved.target_ref
        ),
        serde_json::json!({ "default_deny": saved.default_deny }),
    )
    .await;
    Ok(Json(saved))
}

pub(super) async fn preview_row_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<RowPolicyPreviewRequest>,
) -> Result<Json<RowPolicyPreviewResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let target_profile = state
        .store
        .load_guard_principal(&principal.tenant_id, req.user_id)
        .await?
        .ok_or(AppError::NotFound("USER_NOT_FOUND"))?;
    let ds = state
        .store
        .get_dataset(&principal.tenant_id, req.dataset_id)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;

    let client_ip = extract_client_ip(&headers);
    let policy_ctx = build_policy_context_for_profile(
        &state.store,
        &state.cedar,
        &target_profile,
        false,
        &client_ip,
    )
    .await?;

    let (session, _) = acquire_tenant_session(&state, &principal.tenant_id, false).await?;
    let sql = format!(
        "SELECT * FROM \"{}\" LIMIT 20",
        ds.name.replace('"', "\"\"")
    );
    let out = session
        .execute_sql_with_policy(
            &QueryRequest {
                sql,
                max_rows: Some(20),
                timeout_ms: Some(15_000),
                ..QueryRequest::default()
            },
            false,
            false,
            Some(&policy_ctx),
        )
        .await?;

    let columns = out
        .result
        .columns
        .into_iter()
        .map(|c| PreviewColumnInfo {
            name: c.name,
            data_type: c.data_type,
        })
        .collect();
    Ok(Json(RowPolicyPreviewResponse {
        columns,
        row_count: out.result.row_count,
        rows: out.result.rows,
    }))
}

pub(super) async fn list_column_policies_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ColumnPolicyListQuery>,
) -> Result<Json<PaginatedList<ColumnPolicyRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let mut items = state
        .store
        .list_column_policies(&principal.tenant_id)
        .await?;
    if let Some(kind) = query.target_kind {
        items.retain(|cp| cp.target_kind == kind);
    }
    if let Some(ref tref) = query.target_ref
        && !tref.trim().is_empty()
    {
        let clean = tref.trim();
        items.retain(|cp| cp.target_ref.eq_ignore_ascii_case(clean));
    }
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn put_column_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<PutColumnPolicyRequest>,
) -> Result<Json<ColumnPolicyRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let saved = state
        .store
        .put_column_policy(&principal.tenant_id, req)
        .await?;
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("column_policy:{}", saved.id),
        serde_json::json!({
            "target_kind": saved.target_kind,
            "target_ref": saved.target_ref,
            "column_name": saved.column_name,
            "mask_rule": saved.mask_rule,
        }),
    )
    .await;
    Ok(Json(saved))
}

pub(super) async fn delete_column_policy_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(policy_id): ApiPath<Uuid>,
) -> Result<Json<DeleteResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let deleted = state
        .store
        .delete_column_policy(&principal.tenant_id, policy_id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("POLICY_NOT_FOUND")));
    }
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("column_policy:{policy_id}"),
        serde_json::json!({ "deleted": true }),
    )
    .await;
    Ok(Json(DeleteResponse { deleted: true }))
}

pub(super) async fn simulate_state_row_policy_handler(
    State(_state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(_req): ApiJson<StateRowPolicySimulateRequest>,
) -> Result<Json<StateRowPolicySimulateResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    Err(HttpError(AppError::Unsupported(
        "STATE_ROW_POLICY_SIMULATE_UNSUPPORTED",
    )))
}
