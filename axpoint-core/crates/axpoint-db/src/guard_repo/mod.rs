//! `AxGuard` security governance persistence (`GuardRepository` implementation on [`PgStore`], `11-STEP13`).

mod auth;
mod grants_audit;
mod policies;
mod users;

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{Permission, Sensitivity};
use axpoint_domain::guard::{
    AuditLogEntry, AuditQueryFilter, AuditStatsResponse, AuditVerifyQuery, AuditVerifyResponse,
    AuthSource, CedarPolicyRecord, ColumnPolicyRecord, CreateGrantRequest, CreateGuardRoleRequest,
    CreateGuardUserRequest, DepartmentRecord, GrantRecord, GrantResourceKind,
    GuardPrincipalProfile, GuardRoleRecord, GuardUserRecord, LoginLookupRow, OidcLoginState,
    PatchGuardRoleRequest, PatchGuardUserRequest, PutCedarPolicyRequest, PutColumnPolicyRequest,
    PutRowPolicyDefaultRequest, ResolvedSessionRow, ResourceRef, RowPolicyDefaultRecord,
    RowPolicyRecord, SaveRowPolicyRequest, SessionSummaryRecord, UpsertDepartmentRequest,
    UpsertOidcUserParams, UserAccountStatus,
};
use axpoint_domain::ports::GuardRepository;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::PgStore;

pub(super) async fn notify_policy_changed_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_notify('ax_policy_changed', $1)")
        .bind(tenant_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("pg_notify ax_policy_changed failed: {err}")))?;
    Ok(())
}

pub(super) fn parse_auth_source(raw: &str) -> Result<AuthSource, AppError> {
    match raw {
        "LOCAL" => Ok(AuthSource::Local),
        "OIDC" => Ok(AuthSource::Oidc),
        other => Err(AppError::Database(format!(
            "unexpected auth_source value: {other}"
        ))),
    }
}

pub(super) fn parse_user_status(raw: &str) -> Result<UserAccountStatus, AppError> {
    match raw {
        "ACTIVE" => Ok(UserAccountStatus::Active),
        "DISABLED" => Ok(UserAccountStatus::Disabled),
        other => Err(AppError::Database(format!(
            "unexpected sec_users.status value: {other}"
        ))),
    }
}

pub(super) fn parse_permissions_vec(raw_values: Vec<String>) -> Result<Vec<Permission>, AppError> {
    let mut out = Vec::with_capacity(raw_values.len());
    for raw in raw_values {
        out.push(raw.parse()?);
    }
    Ok(out)
}

pub(super) fn parse_sensitivity(raw: &str) -> Result<Sensitivity, AppError> {
    raw.parse()
}

pub(super) fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("23505")
}

impl GuardRepository for PgStore {
    async fn lookup_login(
        &self,
        tenant_id: &str,
        username: &str,
    ) -> Result<Option<LoginLookupRow>, AppError> {
        auth::lookup_login_impl(self, tenant_id, username).await
    }

    async fn record_login_failure(&self, tenant_id: &str, user_id: Uuid) -> Result<i32, AppError> {
        auth::record_login_failure_impl(self, tenant_id, user_id).await
    }

    async fn record_login_success(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        upgraded_password_hash: Option<&str>,
    ) -> Result<(), AppError> {
        auth::record_login_success_impl(self, tenant_id, user_id, upgraded_password_hash).await
    }

    async fn load_guard_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Option<GuardPrincipalProfile>, AppError> {
        auth::load_guard_principal_impl(self, tenant_id, user_id).await
    }

    async fn create_session(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        family_id: Uuid,
        refresh_hash: &str,
        auth_source: AuthSource,
        meta: (&str, &str, DateTime<Utc>),
    ) -> Result<Uuid, AppError> {
        auth::create_session_impl(
            self,
            tenant_id,
            user_id,
            family_id,
            refresh_hash,
            auth_source,
            meta,
        )
        .await
    }

    async fn resolve_session_by_hash(
        &self,
        refresh_hash: &str,
    ) -> Result<Option<ResolvedSessionRow>, AppError> {
        auth::resolve_session_by_hash_impl(self, refresh_hash).await
    }

    async fn rotate_session(
        &self,
        tenant_id: &str,
        old_session_id: Uuid,
        user_id: Uuid,
        family_id: Uuid,
        new_refresh_hash: &str,
        meta: (&str, &str, DateTime<Utc>),
    ) -> Result<bool, AppError> {
        auth::rotate_session_impl(
            self,
            tenant_id,
            old_session_id,
            user_id,
            family_id,
            new_refresh_hash,
            meta,
        )
        .await
    }

    async fn revoke_session_family(
        &self,
        tenant_id: &str,
        family_id: Uuid,
    ) -> Result<u64, AppError> {
        auth::revoke_session_family_impl(self, tenant_id, family_id).await
    }

    async fn revoke_all_user_sessions(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<u64, AppError> {
        auth::revoke_all_user_sessions_impl(self, tenant_id, user_id).await
    }

    async fn update_user_password(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        new_password_hash: &str,
    ) -> Result<(), AppError> {
        auth::update_user_password_impl(self, tenant_id, user_id, new_password_hash).await
    }

    async fn save_oidc_login_state(&self, state: &OidcLoginState) -> Result<(), AppError> {
        auth::save_oidc_login_state_impl(self, state).await
    }

    async fn consume_oidc_login_state(
        &self,
        state_key: &str,
    ) -> Result<Option<OidcLoginState>, AppError> {
        auth::consume_oidc_login_state_impl(self, state_key).await
    }

    async fn upsert_oidc_user(
        &self,
        tenant_id: &str,
        params: &UpsertOidcUserParams,
    ) -> Result<GuardPrincipalProfile, AppError> {
        auth::upsert_oidc_user_impl(self, tenant_id, params).await
    }

    async fn list_departments(&self, tenant_id: &str) -> Result<Vec<DepartmentRecord>, AppError> {
        users::list_departments_impl(self, tenant_id).await
    }

    async fn upsert_department(
        &self,
        tenant_id: &str,
        dept_id: &str,
        req: UpsertDepartmentRequest,
    ) -> Result<DepartmentRecord, AppError> {
        users::upsert_department_impl(self, tenant_id, dept_id, req).await
    }

    async fn delete_department(&self, tenant_id: &str, dept_id: &str) -> Result<bool, AppError> {
        users::delete_department_impl(self, tenant_id, dept_id).await
    }

    async fn list_guard_users(&self, tenant_id: &str) -> Result<Vec<GuardUserRecord>, AppError> {
        users::list_guard_users_impl(self, tenant_id).await
    }

    async fn create_guard_user(
        &self,
        tenant_id: &str,
        password_hash: &str,
        req: CreateGuardUserRequest,
    ) -> Result<GuardUserRecord, AppError> {
        users::create_guard_user_impl(self, tenant_id, password_hash, req).await
    }

    async fn patch_guard_user(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        req: PatchGuardUserRequest,
    ) -> Result<Option<GuardUserRecord>, AppError> {
        users::patch_guard_user_impl(self, tenant_id, user_id, req).await
    }

    async fn put_user_attributes(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        attributes: &BTreeMap<String, Vec<String>>,
    ) -> Result<BTreeMap<String, Vec<String>>, AppError> {
        users::put_user_attributes_impl(self, tenant_id, user_id, attributes).await
    }

    async fn list_guard_roles(&self, tenant_id: &str) -> Result<Vec<GuardRoleRecord>, AppError> {
        users::list_guard_roles_impl(self, tenant_id).await
    }

    async fn create_guard_role(
        &self,
        tenant_id: &str,
        req: CreateGuardRoleRequest,
    ) -> Result<GuardRoleRecord, AppError> {
        users::create_guard_role_impl(self, tenant_id, req).await
    }

    async fn patch_guard_role(
        &self,
        tenant_id: &str,
        role_id: Uuid,
        req: PatchGuardRoleRequest,
    ) -> Result<Option<GuardRoleRecord>, AppError> {
        users::patch_guard_role_impl(self, tenant_id, role_id, req).await
    }

    async fn delete_guard_role(&self, tenant_id: &str, role_id: Uuid) -> Result<bool, AppError> {
        users::delete_guard_role_impl(self, tenant_id, role_id).await
    }

    async fn list_cedar_policies(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<CedarPolicyRecord>, AppError> {
        policies::list_cedar_policies_impl(self, tenant_id).await
    }

    async fn put_cedar_policy(
        &self,
        tenant_id: &str,
        name: &str,
        updated_by: Option<Uuid>,
        req: PutCedarPolicyRequest,
    ) -> Result<CedarPolicyRecord, AppError> {
        policies::put_cedar_policy_impl(self, tenant_id, name, updated_by, req).await
    }

    async fn delete_cedar_policy(&self, tenant_id: &str, name: &str) -> Result<bool, AppError> {
        policies::delete_cedar_policy_impl(self, tenant_id, name).await
    }

    async fn list_row_policies(&self, tenant_id: &str) -> Result<Vec<RowPolicyRecord>, AppError> {
        policies::list_row_policies_impl(self, tenant_id).await
    }

    async fn create_row_policy(
        &self,
        tenant_id: &str,
        req: SaveRowPolicyRequest,
    ) -> Result<RowPolicyRecord, AppError> {
        policies::create_row_policy_impl(self, tenant_id, req).await
    }

    async fn update_row_policy(
        &self,
        tenant_id: &str,
        policy_id: Uuid,
        req: SaveRowPolicyRequest,
    ) -> Result<Option<RowPolicyRecord>, AppError> {
        policies::update_row_policy_impl(self, tenant_id, policy_id, req).await
    }

    async fn delete_row_policy(&self, tenant_id: &str, policy_id: Uuid) -> Result<bool, AppError> {
        policies::delete_row_policy_impl(self, tenant_id, policy_id).await
    }

    async fn list_row_policy_defaults(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<RowPolicyDefaultRecord>, AppError> {
        policies::list_row_policy_defaults_impl(self, tenant_id).await
    }

    async fn put_row_policy_default(
        &self,
        tenant_id: &str,
        req: PutRowPolicyDefaultRequest,
    ) -> Result<RowPolicyDefaultRecord, AppError> {
        policies::put_row_policy_default_impl(self, tenant_id, req).await
    }

    async fn list_column_policies(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ColumnPolicyRecord>, AppError> {
        policies::list_column_policies_impl(self, tenant_id).await
    }

    async fn put_column_policy(
        &self,
        tenant_id: &str,
        req: PutColumnPolicyRequest,
    ) -> Result<ColumnPolicyRecord, AppError> {
        policies::put_column_policy_impl(self, tenant_id, req).await
    }

    async fn delete_column_policy(
        &self,
        tenant_id: &str,
        policy_id: Uuid,
    ) -> Result<bool, AppError> {
        policies::delete_column_policy_impl(self, tenant_id, policy_id).await
    }

    async fn resolve_resource_ref(
        &self,
        tenant_id: &str,
        kind: GrantResourceKind,
        resource_ref: &str,
    ) -> Result<Option<ResourceRef>, AppError> {
        policies::resolve_resource_ref_impl(self, tenant_id, kind, resource_ref).await
    }

    async fn create_grant(
        &self,
        tenant_id: &str,
        requested_by: Uuid,
        req: CreateGrantRequest,
    ) -> Result<GrantRecord, AppError> {
        grants_audit::create_grant_impl(self, tenant_id, requested_by, req).await
    }

    async fn list_grants(
        &self,
        tenant_id: &str,
        viewer_filter: Option<(Uuid, &str)>,
    ) -> Result<Vec<GrantRecord>, AppError> {
        grants_audit::list_grants_impl(self, tenant_id, viewer_filter).await
    }

    async fn get_grant_by_approval(
        &self,
        tenant_id: &str,
        approval_id: Uuid,
    ) -> Result<Option<GrantRecord>, AppError> {
        grants_audit::get_grant_by_approval_impl(self, tenant_id, approval_id).await
    }

    async fn apply_grant_decision(
        &self,
        tenant_id: &str,
        approval_id: Uuid,
        approved: bool,
    ) -> Result<Option<GrantRecord>, AppError> {
        grants_audit::apply_grant_decision_impl(self, tenant_id, approval_id, approved).await
    }

    async fn revoke_grant(&self, tenant_id: &str, grant_id: Uuid) -> Result<bool, AppError> {
        grants_audit::revoke_grant_impl(self, tenant_id, grant_id).await
    }

    async fn list_active_grants(&self, tenant_id: &str) -> Result<Vec<GrantRecord>, AppError> {
        grants_audit::list_active_grants_impl(self, tenant_id).await
    }

    async fn list_sessions(
        &self,
        tenant_id: &str,
        user_id_filter: Option<Uuid>,
    ) -> Result<Vec<SessionSummaryRecord>, AppError> {
        grants_audit::list_sessions_impl(self, tenant_id, user_id_filter).await
    }

    async fn query_audit_logs(
        &self,
        tenant_id: &str,
        filter: &AuditQueryFilter,
    ) -> Result<PaginatedList<AuditLogEntry>, AppError> {
        grants_audit::query_audit_logs_impl(self, tenant_id, filter).await
    }

    async fn verify_audit_chain(
        &self,
        tenant_id: &str,
        query: &AuditVerifyQuery,
    ) -> Result<AuditVerifyResponse, AppError> {
        grants_audit::verify_audit_chain_impl(self, tenant_id, query).await
    }

    async fn query_audit_stats(
        &self,
        tenant_id: &str,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<AuditStatsResponse, AppError> {
        grants_audit::query_audit_stats_impl(self, tenant_id, from, to).await
    }
}
