//! Department hierarchy, local/OIDC user management, user attributes, and RBAC role handlers
//! under `/api/axguard/*` (`11-STEP13` §13.10).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, Permission};
use axpoint_domain::guard::{
    CreateGuardRoleRequest, CreateGuardUserRequest, DepartmentRecord, GuardRoleRecord,
    GuardUserRecord, PatchGuardRoleRequest, PatchGuardUserRequest, PutUserAttributesRequest,
    ResetUserPasswordRequest, UpsertDepartmentRequest,
};
use axpoint_domain::ports::GuardRepository as _;
use axpoint_security::AuthPrincipal;
use axpoint_security::password::{hash_password, validate_password_policy};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use super::{DeleteResponse, record_guard_audit};
use crate::state::{ApiJson, ApiPath, AppState, HttpError};

pub(super) async fn list_departments_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<DepartmentRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state.store.list_departments(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn upsert_department_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(dept_id): ApiPath<String>,
    ApiJson(req): ApiJson<UpsertDepartmentRequest>,
) -> Result<Json<DepartmentRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let rec = state
        .store
        .upsert_department(&principal.tenant_id, dept_id.trim(), req)
        .await?;
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("dept:{}", rec.dept_id),
        serde_json::json!({ "name": rec.name, "parent_id": rec.parent_id }),
    )
    .await;
    Ok(Json(rec))
}

pub(super) async fn delete_department_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(dept_id): ApiPath<String>,
) -> Result<Json<DeleteResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let clean = dept_id.trim();
    let deleted = state
        .store
        .delete_department(&principal.tenant_id, clean)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("DEPARTMENT_NOT_FOUND")));
    }
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("dept:{clean}"),
        serde_json::json!({ "deleted": true }),
    )
    .await;
    Ok(Json(DeleteResponse { deleted: true }))
}

pub(super) async fn list_users_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<GuardUserRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state.store.list_guard_users(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn create_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateGuardUserRequest>,
) -> Result<(StatusCode, Json<GuardUserRecord>), HttpError> {
    principal.require(Permission::GuardAdmin)?;
    validate_password_policy(&req.username, &req.password)?;

    let pw = req.password.clone();
    let pw_hash = tokio::task::spawn_blocking(move || hash_password(&pw))
        .await
        .map_err(|err| AppError::Config(format!("hash_password join failed: {err}")))??;

    let created = state
        .store
        .create_guard_user(&principal.tenant_id, &pw_hash, req)
        .await?;
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.authenticator.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("user:{}", created.id),
        serde_json::json!({
            "event": "user_created",
            "username": created.username,
            "dept_id": created.dept_id,
            "clearance": created.clearance,
            "roles": created.roles,
        }),
    )
    .await;

    Ok((StatusCode::CREATED, Json(created)))
}

pub(super) async fn patch_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(user_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchGuardUserRequest>,
) -> Result<Json<GuardUserRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let updated = state
        .store
        .patch_guard_user(&principal.tenant_id, user_id, req)
        .await?
        .ok_or(AppError::NotFound("USER_NOT_FOUND"))?;

    state
        .authenticator
        .invalidate_user(&principal.tenant_id, user_id);
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("user:{user_id}"),
        serde_json::json!({
            "event": "user_patched",
            "dept_id": updated.dept_id,
            "clearance": updated.clearance,
            "status": updated.status,
            "roles": updated.roles,
        }),
    )
    .await;
    Ok(Json(updated))
}

pub(super) async fn reset_user_password_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(user_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<ResetUserPasswordRequest>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let users = state.store.list_guard_users(&principal.tenant_id).await?;
    let target = users
        .iter()
        .find(|u| u.id == user_id)
        .ok_or(AppError::NotFound("USER_NOT_FOUND"))?;

    validate_password_policy(&target.username, &req.new_password)?;
    let pw = req.new_password;
    let pw_hash = tokio::task::spawn_blocking(move || hash_password(&pw))
        .await
        .map_err(|err| AppError::Config(format!("hash_password join failed: {err}")))??;

    state
        .store
        .update_user_password(&principal.tenant_id, user_id, &pw_hash)
        .await?;
    state
        .authenticator
        .invalidate_user(&principal.tenant_id, user_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("user:{user_id}:reset_password"),
        serde_json::json!({ "target_username": target.username }),
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(super) async fn put_user_attributes_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(user_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PutUserAttributesRequest>,
) -> Result<Json<BTreeMap<String, Vec<String>>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let attrs = state
        .store
        .put_user_attributes(&principal.tenant_id, user_id, &req.attributes)
        .await?;
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("user:{user_id}:attributes"),
        serde_json::json!({ "keys": attrs.keys().collect::<Vec<_>>() }),
    )
    .await;
    Ok(Json(attrs))
}

pub(super) async fn list_roles_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<GuardRoleRecord>>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let items = state.store.list_guard_roles(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

pub(super) async fn create_role_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateGuardRoleRequest>,
) -> Result<(StatusCode, Json<GuardRoleRecord>), HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let created = state
        .store
        .create_guard_role(&principal.tenant_id, req)
        .await?;
    state.authenticator.invalidate_tenant(&principal.tenant_id);
    state.cedar.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("role:{}", created.id),
        serde_json::json!({ "name": created.name, "permissions": created.permissions }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(super) async fn patch_role_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(role_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchGuardRoleRequest>,
) -> Result<Json<GuardRoleRecord>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let updated = state
        .store
        .patch_guard_role(&principal.tenant_id, role_id, req)
        .await?
        .ok_or(AppError::NotFound("ROLE_NOT_FOUND"))?;
    state.authenticator.invalidate_tenant(&principal.tenant_id);
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("role:{role_id}"),
        serde_json::json!({ "name": updated.name, "permissions": updated.permissions }),
    )
    .await;
    Ok(Json(updated))
}

pub(super) async fn delete_role_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(role_id): ApiPath<Uuid>,
) -> Result<Json<DeleteResponse>, HttpError> {
    principal.require(Permission::GuardAdmin)?;
    let deleted = state
        .store
        .delete_guard_role(&principal.tenant_id, role_id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("ROLE_NOT_FOUND")));
    }
    state.authenticator.invalidate_tenant(&principal.tenant_id);
    state.cedar.invalidate_tenant(&principal.tenant_id);
    state.view.cache.invalidate_tenant(&principal.tenant_id);

    record_guard_audit(
        &state,
        &principal,
        &headers,
        AuditAction::PolicyChange,
        format!("role:{role_id}"),
        serde_json::json!({ "deleted": true }),
    )
    .await;
    Ok(Json(DeleteResponse { deleted: true }))
}
