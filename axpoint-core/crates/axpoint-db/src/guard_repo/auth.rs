//! Authentication, session family rotation/replay detection, and OIDC state/user persistence (`11-STEP13` §13.3–§13.5).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::Permission;
use axpoint_domain::guard::{
    AuthSource, GuardPrincipalProfile, LoginLookupRow, OidcLoginState, ResolvedSessionRow,
    UpsertOidcUserParams, UserAccountStatus,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row as _, Transaction};
use uuid::Uuid;

use super::{notify_policy_changed_tx, parse_auth_source, parse_sensitivity, parse_user_status};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

pub(super) async fn lookup_login_impl(
    store: &PgStore,
    tenant_id: &str,
    username: &str,
) -> Result<Option<LoginLookupRow>, AppError> {
    let row_opt = sqlx::query(
        "SELECT user_id, password_hash, status, locked_until, failed_logins
           FROM ax_lookup_login($1, $2)",
    )
    .bind(tenant_id)
    .bind(username)
    .fetch_optional(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("ax_lookup_login failed: {err}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };

    Ok(Some(LoginLookupRow {
        user_id: row.get("user_id"),
        password_hash: row.get("password_hash"),
        status: row.get("status"),
        locked_until: row.get("locked_until"),
        failed_logins: row.get("failed_logins"),
    }))
}

pub(super) async fn record_login_failure_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
) -> Result<i32, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row_opt = sqlx::query(
        "UPDATE sec_users
            SET failed_logins = failed_logins + 1,
                locked_until = CASE
                    WHEN failed_logins + 1 >= 5 THEN NOW() + INTERVAL '15 minutes'
                    ELSE locked_until
                END,
                updated_at = NOW()
          WHERE id = $1
      RETURNING failed_logins",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("record_login_failure failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit record_login_failure tx: {err}")))?;

    Ok(row_opt.map_or(0, |r| r.get("failed_logins")))
}

pub(super) async fn record_login_success_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    upgraded_password_hash: Option<&str>,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    sqlx::query(
        "UPDATE sec_users
            SET failed_logins = 0,
                locked_until = NULL,
                last_login_at = NOW(),
                password_hash = COALESCE($2, password_hash),
                updated_at = NOW()
          WHERE id = $1",
    )
    .bind(user_id)
    .bind(upgraded_password_hash)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("record_login_success failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit record_login_success tx: {err}")))?;
    Ok(())
}

pub(super) async fn load_guard_principal_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<GuardPrincipalProfile>, AppError> {
    let user_opt = sqlx::query(
        "SELECT id, tenant_id, username, display_name, email, dept_id,
                clearance, status, tokens_valid_after
           FROM sec_users
          WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load sec_users profile failed: {err}")))?;

    let Some(u) = user_opt else {
        return Ok(None);
    };

    let role_rows = sqlx::query(
        "SELECT r.name, r.permissions
           FROM sec_user_roles ur
           JOIN sec_roles r ON r.id = ur.role_id
          WHERE ur.user_id = $1
          ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load sec_user_roles failed: {err}")))?;

    let mut roles = Vec::with_capacity(role_rows.len());
    let mut permissions = BTreeSet::new();
    for row in role_rows {
        let rname: String = row.get("name");
        let raw_perms: Vec<String> = row.get("permissions");
        roles.push(rname);
        for raw in raw_perms {
            let p: Permission = raw.parse()?;
            permissions.insert(p);
        }
    }

    let attr_rows = sqlx::query(
        "SELECT attr_key, attr_values
           FROM sec_user_attributes
          WHERE user_id = $1
          ORDER BY attr_key",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load sec_user_attributes failed: {err}")))?;

    let mut attributes = BTreeMap::new();
    for row in attr_rows {
        let key: String = row.get("attr_key");
        let vals: Vec<String> = row.get("attr_values");
        attributes.insert(key, vals);
    }

    let raw_clearance: String = u.get("clearance");
    let raw_status: String = u.get("status");

    Ok(Some(GuardPrincipalProfile {
        user_id: u.get("id"),
        tenant_id: u.get("tenant_id"),
        username: u.get("username"),
        display_name: u.get("display_name"),
        email: u.get("email"),
        dept_id: u.get("dept_id"),
        clearance: parse_sensitivity(&raw_clearance)?,
        status: parse_user_status(&raw_status)?,
        tokens_valid_after: u.get("tokens_valid_after"),
        roles,
        permissions,
        attributes,
    }))
}

pub(super) async fn load_guard_principal_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
) -> Result<Option<GuardPrincipalProfile>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let profile = load_guard_principal_tx(&mut tx, user_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit load_guard_principal tx: {err}")))?;
    Ok(profile)
}

pub(super) async fn create_session_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    family_id: Uuid,
    refresh_hash: &str,
    auth_source: AuthSource,
    meta: (&str, &str, DateTime<Utc>),
) -> Result<Uuid, AppError> {
    let (user_agent, client_ip, expires_at) = meta;
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO sec_sessions (
             tenant_id, user_id, family_id, refresh_hash, auth_source,
             user_agent, client_ip, expires_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(family_id)
    .bind(refresh_hash)
    .bind(auth_source.as_str())
    .bind(user_agent)
    .bind(client_ip)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert sec_sessions failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_session tx: {err}")))?;
    Ok(row.get("id"))
}

pub(super) async fn resolve_session_by_hash_impl(
    store: &PgStore,
    refresh_hash: &str,
) -> Result<Option<ResolvedSessionRow>, AppError> {
    let row_opt = sqlx::query(
        "SELECT session_id, tenant_id, user_id, family_id, expires_at, rotated_at, revoked_at
           FROM ax_resolve_session($1)",
    )
    .bind(refresh_hash)
    .fetch_optional(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("ax_resolve_session failed: {err}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };

    Ok(Some(ResolvedSessionRow {
        session_id: row.get("session_id"),
        tenant_id: row.get("tenant_id"),
        user_id: row.get("user_id"),
        family_id: row.get("family_id"),
        expires_at: row.get("expires_at"),
        rotated_at: row.get("rotated_at"),
        revoked_at: row.get("revoked_at"),
    }))
}

pub(super) async fn rotate_session_impl(
    store: &PgStore,
    tenant_id: &str,
    old_session_id: Uuid,
    user_id: Uuid,
    family_id: Uuid,
    new_refresh_hash: &str,
    meta: (&str, &str, DateTime<Utc>),
) -> Result<bool, AppError> {
    let (user_agent, client_ip, expires_at) = meta;
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    let locked_opt = sqlx::query(
        "SELECT auth_source, expires_at, rotated_at, revoked_at
           FROM sec_sessions
          WHERE id = $1
            FOR UPDATE",
    )
    .bind(old_session_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("lock sec_sessions failed: {err}")))?;

    let Some(locked) = locked_opt else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing session tx: {err}")))?;
        return Ok(false);
    };

    let rotated_at: Option<DateTime<Utc>> = locked.get("rotated_at");
    let revoked_at: Option<DateTime<Utc>> = locked.get("revoked_at");
    if rotated_at.is_some() || revoked_at.is_some() {
        sqlx::query(
            "UPDATE sec_sessions
                SET revoked_at = COALESCE(revoked_at, NOW())
              WHERE family_id = $1",
        )
        .bind(family_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("revoke replayed family failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit family revoke tx: {err}")))?;
        return Ok(false);
    }

    let row_exp: DateTime<Utc> = locked.get("expires_at");
    if row_exp <= Utc::now() {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit expired session tx: {err}")))?;
        return Ok(false);
    }

    let raw_src: String = locked.get("auth_source");
    let auth_source = parse_auth_source(&raw_src)?;

    sqlx::query("UPDATE sec_sessions SET rotated_at = NOW() WHERE id = $1")
        .bind(old_session_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark session rotated failed: {err}")))?;

    sqlx::query(
        "INSERT INTO sec_sessions (
             tenant_id, user_id, family_id, refresh_hash, auth_source,
             user_agent, client_ip, expires_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(family_id)
    .bind(new_refresh_hash)
    .bind(auth_source.as_str())
    .bind(user_agent)
    .bind(client_ip)
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert rotated session failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit rotate_session tx: {err}")))?;
    Ok(true)
}

pub(super) async fn revoke_session_family_impl(
    store: &PgStore,
    tenant_id: &str,
    family_id: Uuid,
) -> Result<u64, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query(
        "UPDATE sec_sessions
            SET revoked_at = COALESCE(revoked_at, NOW())
          WHERE family_id = $1 AND revoked_at IS NULL",
    )
    .bind(family_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("revoke_session_family failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit revoke_session_family tx: {err}")))?;
    Ok(res.rows_affected())
}

pub(super) async fn revoke_all_user_sessions_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
) -> Result<u64, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    sqlx::query(
        "UPDATE sec_users SET tokens_valid_after = NOW(), updated_at = NOW() WHERE id = $1",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("bump tokens_valid_after failed: {err}")))?;

    let res = sqlx::query(
        "UPDATE sec_sessions
            SET revoked_at = COALESCE(revoked_at, NOW())
          WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("revoke_all_user_sessions failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit revoke_all_user_sessions tx: {err}")))?;
    Ok(res.rows_affected())
}

pub(super) async fn update_user_password_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    new_password_hash: &str,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let updated = sqlx::query(
        "UPDATE sec_users
            SET password_hash = $2,
                password_changed_at = NOW(),
                tokens_valid_after = NOW(),
                failed_logins = 0,
                locked_until = NULL,
                updated_at = NOW()
          WHERE id = $1",
    )
    .bind(user_id)
    .bind(new_password_hash)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("update_user_password failed: {err}")))?;

    if updated.rows_affected() == 0 {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing user password tx: {err}")))?;
        return Err(AppError::NotFound("USER_NOT_FOUND"));
    }

    sqlx::query(
        "UPDATE sec_sessions
            SET revoked_at = COALESCE(revoked_at, NOW())
          WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("revoke sessions after pw change failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit update_user_password tx: {err}")))?;
    Ok(())
}

pub(super) async fn save_oidc_login_state_impl(
    store: &PgStore,
    state: &OidcLoginState,
) -> Result<(), AppError> {
    if let Err(err) = sqlx::query("DELETE FROM sec_login_states WHERE expires_at <= NOW()")
        .execute(store.pool())
        .await
    {
        tracing::warn!(error = %err, "Opportunistic sec_login_states cleanup failed");
    }

    sqlx::query(
        "INSERT INTO sec_login_states (state, tenant_id, nonce, pkce_verifier, return_to, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&state.state)
    .bind(&state.tenant_id)
    .bind(&state.nonce)
    .bind(&state.pkce_verifier)
    .bind(&state.return_to)
    .bind(state.expires_at)
    .execute(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("insert sec_login_states failed: {err}")))?;
    Ok(())
}

pub(super) async fn consume_oidc_login_state_impl(
    store: &PgStore,
    state_key: &str,
) -> Result<Option<OidcLoginState>, AppError> {
    let row_opt = sqlx::query(
        "DELETE FROM sec_login_states
          WHERE state = $1 AND expires_at > NOW()
      RETURNING state, tenant_id, nonce, pkce_verifier, return_to, expires_at",
    )
    .bind(state_key)
    .fetch_optional(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("consume sec_login_states failed: {err}")))?;

    Ok(row_opt.map(|row| OidcLoginState {
        state: row.get("state"),
        tenant_id: row.get("tenant_id"),
        nonce: row.get("nonce"),
        pkce_verifier: row.get("pkce_verifier"),
        return_to: row.get("return_to"),
        expires_at: row.get("expires_at"),
    }))
}

pub(super) async fn upsert_oidc_user_impl(
    store: &PgStore,
    tenant_id: &str,
    params: &UpsertOidcUserParams,
) -> Result<GuardPrincipalProfile, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing =
        sqlx::query("SELECT id, status FROM sec_users WHERE oidc_subject = $1 FOR UPDATE")
            .bind(&params.oidc_subject)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("lookup oidc user failed: {err}")))?;

    let resolved_dept = resolve_oidc_dept_tx(&mut tx, params.mapped_dept.as_deref()).await?;

    let (user_id, is_new) = if let Some(row) = existing {
        let status_raw: String = row.get("status");
        if parse_user_status(&status_raw)? != UserAccountStatus::Active {
            tx.commit()
                .await
                .map_err(|err| AppError::Database(format!("commit disabled oidc tx: {err}")))?;
            return Err(AppError::Unauthorized("ACCOUNT_DISABLED"));
        }
        let uid: Uuid = row.get("id");
        sqlx::query(
            "UPDATE sec_users
                SET email = CASE WHEN $2 = '' THEN email ELSE $2 END,
                    dept_id = COALESCE($3, dept_id),
                    last_login_at = NOW(),
                    updated_at = NOW()
              WHERE id = $1",
        )
        .bind(uid)
        .bind(params.email.trim())
        .bind(resolved_dept.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update oidc user failed: {err}")))?;
        (uid, false)
    } else {
        if !params.auto_provision {
            tx.commit()
                .await
                .map_err(|err| AppError::Database(format!("commit unprovisioned oidc: {err}")))?;
            return Err(AppError::Forbidden("OIDC_NOT_PROVISIONED"));
        }
        let uid =
            insert_new_oidc_user_tx(&mut tx, tenant_id, params, resolved_dept.as_deref()).await?;
        (uid, true)
    };

    sync_oidc_roles_tx(&mut tx, tenant_id, user_id, &params.mapped_roles, is_new).await?;
    notify_policy_changed_tx(&mut tx, tenant_id).await?;

    let profile = load_guard_principal_tx(&mut tx, user_id)
        .await?
        .ok_or_else(|| AppError::Database(String::from("OIDC user missing after upsert")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit upsert_oidc_user tx: {err}")))?;
    Ok(profile)
}

async fn resolve_oidc_dept_tx(
    tx: &mut Transaction<'_, Postgres>,
    mapped: Option<&str>,
) -> Result<Option<String>, AppError> {
    let Some(candidate) = mapped.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let exists = sqlx::query("SELECT 1 FROM sec_departments WHERE dept_id = $1")
        .bind(candidate)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("check oidc sec_departments: {err}")))?;
    Ok(exists.map(|_| String::from(candidate)))
}

async fn insert_new_oidc_user_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    params: &UpsertOidcUserParams,
    resolved_dept: Option<&str>,
) -> Result<Uuid, AppError> {
    let base_hint = params.username_hint.trim();
    let stem = if base_hint.is_empty() {
        "oidc_user"
    } else {
        base_hint
    };
    let dept = resolved_dept.unwrap_or("default");

    let mut chosen = String::from(stem);
    for suffix in 0_u32..20 {
        if suffix > 0 {
            chosen = format!("{stem}_{suffix}");
        }
        let taken = sqlx::query("SELECT 1 FROM sec_users WHERE username = $1")
            .bind(&chosen)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("probe oidc username: {err}")))?;
        if taken.is_none() {
            break;
        }
    }

    let row = sqlx::query(
        "INSERT INTO sec_users (
             tenant_id, username, display_name, email, dept_id,
             auth_source, oidc_subject, status, last_login_at
         )
         VALUES ($1, $2, $3, $4, $5, 'OIDC', $6, 'ACTIVE', NOW())
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(&chosen)
    .bind(&chosen)
    .bind(params.email.trim())
    .bind(dept)
    .bind(&params.oidc_subject)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert oidc sec_users failed: {err}")))?;

    Ok(row.get("id"))
}

async fn sync_oidc_roles_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    user_id: Uuid,
    mapped_roles: &[String],
    is_new: bool,
) -> Result<(), AppError> {
    let target_names: Vec<String> = if !mapped_roles.is_empty() {
        mapped_roles.to_vec()
    } else if is_new {
        vec![String::from("VIEWER")]
    } else {
        return Ok(());
    };

    let role_rows = sqlx::query("SELECT id FROM sec_roles WHERE name = ANY($1)")
        .bind(&target_names)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("resolve oidc roles failed: {err}")))?;

    sqlx::query("DELETE FROM sec_user_roles WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clear oidc user roles failed: {err}")))?;

    for row in role_rows {
        let rid: Uuid = row.get("id");
        sqlx::query(
            "INSERT INTO sec_user_roles (tenant_id, user_id, role_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(rid)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert oidc user role failed: {err}")))?;
    }
    Ok(())
}
