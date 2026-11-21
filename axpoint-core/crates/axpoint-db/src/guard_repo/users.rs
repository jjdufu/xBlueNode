//! Department hierarchy, local/OIDC user management, user attributes, and RBAC role persistence (`11-STEP13` §13.10).

use std::collections::{BTreeMap, HashMap, HashSet};

use axpoint_domain::AppError;
use axpoint_domain::guard::{
    CreateGuardRoleRequest, CreateGuardUserRequest, DepartmentRecord, GuardRoleRecord,
    GuardUserRecord, PatchGuardRoleRequest, PatchGuardUserRequest, UpsertDepartmentRequest,
    UserAccountStatus,
};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row as _, Transaction};
use uuid::Uuid;

use super::{
    is_unique_violation, notify_policy_changed_tx, parse_auth_source, parse_permissions_vec,
    parse_sensitivity, parse_user_status,
};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn is_valid_dept_id(dept_id: &str) -> bool {
    (1..=64).contains(&dept_id.len())
        && dept_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn is_valid_attr_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && key.len() <= 63
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

pub(super) async fn list_departments_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<DepartmentRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT tenant_id, dept_id, name, parent_id
           FROM sec_departments
          ORDER BY dept_id ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_departments failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_departments tx: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| DepartmentRecord {
            tenant_id: r.get("tenant_id"),
            dept_id: r.get("dept_id"),
            name: r.get("name"),
            parent_id: r.get("parent_id"),
        })
        .collect())
}

pub(super) async fn upsert_department_impl(
    store: &PgStore,
    tenant_id: &str,
    dept_id: &str,
    req: UpsertDepartmentRequest,
) -> Result<DepartmentRecord, AppError> {
    let clean_id = dept_id.trim();
    if !is_valid_dept_id(clean_id) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "dept_id must match ^[a-z0-9_-]{1,64}$",
        ));
    }
    let clean_name = req.name.trim();
    if clean_name.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "department name cannot be empty",
        ));
    }
    let parent_id = req
        .parent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    if let Some(ref pid) = parent_id {
        validate_acyclic_parent_tx(&mut tx, clean_id, pid).await?;
    }

    let row = sqlx::query(
        "INSERT INTO sec_departments (tenant_id, dept_id, name, parent_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id, dept_id) DO UPDATE SET
             name = EXCLUDED.name,
             parent_id = EXCLUDED.parent_id
         RETURNING tenant_id, dept_id, name, parent_id",
    )
    .bind(tenant_id)
    .bind(clean_id)
    .bind(clean_name)
    .bind(&parent_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert sec_departments failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit upsert_department tx: {err}")))?;

    Ok(DepartmentRecord {
        tenant_id: row.get("tenant_id"),
        dept_id: row.get("dept_id"),
        name: row.get("name"),
        parent_id: row.get("parent_id"),
    })
}

async fn validate_acyclic_parent_tx(
    tx: &mut Transaction<'_, Postgres>,
    dept_id: &str,
    parent_id: &str,
) -> Result<(), AppError> {
    if dept_id == parent_id {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "department cannot be its own parent",
        ));
    }
    let rows = sqlx::query("SELECT dept_id, parent_id FROM sec_departments")
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load sec_departments tree: {err}")))?;

    let mut parents: HashMap<String, Option<String>> = HashMap::with_capacity(rows.len());
    for r in rows {
        parents.insert(r.get("dept_id"), r.get("parent_id"));
    }
    if !parents.contains_key(parent_id) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("parent department '{parent_id}' does not exist"),
        ));
    }

    let mut cur = Some(String::from(parent_id));
    let mut visited = HashSet::new();
    while let Some(node) = cur {
        if node == dept_id {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "department hierarchy cycle detected",
            ));
        }
        if !visited.insert(node.clone()) {
            break;
        }
        cur = parents.get(&node).cloned().flatten();
    }
    Ok(())
}

pub(super) async fn delete_department_impl(
    store: &PgStore,
    tenant_id: &str,
    dept_id: &str,
) -> Result<bool, AppError> {
    if dept_id == "default" {
        return Err(AppError::Conflict("DEPARTMENT_IN_USE"));
    }
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let in_use_row = sqlx::query(
        "SELECT EXISTS(SELECT 1 FROM sec_departments WHERE parent_id = $1)
             OR EXISTS(SELECT 1 FROM sec_users WHERE dept_id = $1)
             OR EXISTS(SELECT 1 FROM ds_datasets WHERE owner_dept = $1) AS in_use",
    )
    .bind(dept_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check department references failed: {err}")))?;

    let in_use: bool = in_use_row.get("in_use");
    if in_use {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit in_use dept check: {err}")))?;
        return Err(AppError::Conflict("DEPARTMENT_IN_USE"));
    }

    let res = sqlx::query("DELETE FROM sec_departments WHERE dept_id = $1")
        .bind(dept_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete sec_departments failed: {err}")))?;

    let deleted = res.rows_affected() > 0;
    if deleted {
        notify_policy_changed_tx(&mut tx, tenant_id).await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_department tx: {err}")))?;
    Ok(deleted)
}

fn decode_guard_user_base(
    row: &PgRow,
    roles: Vec<String>,
    attributes: BTreeMap<String, Vec<String>>,
) -> Result<GuardUserRecord, AppError> {
    let raw_clearance: String = row.get("clearance");
    let raw_source: String = row.get("auth_source");
    let raw_status: String = row.get("status");
    Ok(GuardUserRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        username: row.get("username"),
        display_name: row.get("display_name"),
        email: row.get("email"),
        dept_id: row.get("dept_id"),
        clearance: parse_sensitivity(&raw_clearance)?,
        auth_source: parse_auth_source(&raw_source)?,
        status: parse_user_status(&raw_status)?,
        roles,
        attributes,
        failed_logins: row.get("failed_logins"),
        locked_until: row.get("locked_until"),
        last_login_at: row.get("last_login_at"),
        created_at: row.get("created_at"),
    })
}

async fn load_single_guard_user_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<GuardUserRecord>, AppError> {
    let row_opt = sqlx::query(
        "SELECT id, tenant_id, username, display_name, email, dept_id, clearance,
                auth_source, status, failed_logins, locked_until, last_login_at, created_at
           FROM sec_users
          WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load sec_users row failed: {err}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };

    let role_rows = sqlx::query(
        "SELECT r.name
           FROM sec_user_roles ur
           JOIN sec_roles r ON r.id = ur.role_id
          WHERE ur.user_id = $1
          ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load user roles failed: {err}")))?;
    let roles: Vec<String> = role_rows.into_iter().map(|r| r.get("name")).collect();

    let attr_rows = sqlx::query(
        "SELECT attr_key, attr_values
           FROM sec_user_attributes
          WHERE user_id = $1
          ORDER BY attr_key",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load user attributes failed: {err}")))?;
    let mut attrs = BTreeMap::new();
    for r in attr_rows {
        attrs.insert(r.get("attr_key"), r.get("attr_values"));
    }

    Ok(Some(decode_guard_user_base(&row, roles, attrs)?))
}

pub(super) async fn list_guard_users_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<GuardUserRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let user_rows = sqlx::query(
        "SELECT id, tenant_id, username, display_name, email, dept_id, clearance,
                auth_source, status, failed_logins, locked_until, last_login_at, created_at
           FROM sec_users
          ORDER BY created_at ASC, username ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_users failed: {err}")))?;

    let role_rows = sqlx::query(
        "SELECT ur.user_id, r.name
           FROM sec_user_roles ur
           JOIN sec_roles r ON r.id = ur.role_id
          ORDER BY r.name",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_user_roles failed: {err}")))?;
    let mut roles_by_user: HashMap<Uuid, Vec<String>> = HashMap::new();
    for r in role_rows {
        roles_by_user
            .entry(r.get("user_id"))
            .or_default()
            .push(r.get("name"));
    }

    let attr_rows = sqlx::query(
        "SELECT user_id, attr_key, attr_values FROM sec_user_attributes ORDER BY attr_key",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_user_attributes failed: {err}")))?;
    let mut attrs_by_user: HashMap<Uuid, BTreeMap<String, Vec<String>>> = HashMap::new();
    for r in attr_rows {
        attrs_by_user
            .entry(r.get("user_id"))
            .or_default()
            .insert(r.get("attr_key"), r.get("attr_values"));
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_guard_users tx: {err}")))?;

    let mut out = Vec::with_capacity(user_rows.len());
    for row in &user_rows {
        let uid: Uuid = row.get("id");
        let roles = roles_by_user.remove(&uid).unwrap_or_default();
        let attrs = attrs_by_user.remove(&uid).unwrap_or_default();
        out.push(decode_guard_user_base(row, roles, attrs)?);
    }
    Ok(out)
}

async fn replace_user_roles_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    user_id: Uuid,
    role_names: &[String],
) -> Result<(), AppError> {
    let role_rows = sqlx::query("SELECT id, name FROM sec_roles WHERE name = ANY($1)")
        .bind(role_names)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("resolve role names failed: {err}")))?;

    let found_names: HashSet<String> = role_rows.iter().map(|r| r.get("name")).collect();
    for wanted in role_names {
        if !found_names.contains(wanted) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown role '{wanted}'"),
            ));
        }
    }

    sqlx::query("DELETE FROM sec_user_roles WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clear sec_user_roles failed: {err}")))?;

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
        .map_err(|err| AppError::Database(format!("insert sec_user_roles failed: {err}")))?;
    }
    Ok(())
}

async fn replace_user_attributes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    user_id: Uuid,
    attributes: &BTreeMap<String, Vec<String>>,
) -> Result<(), AppError> {
    for key in attributes.keys() {
        if !is_valid_attr_key(key) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("attribute key '{key}' must match ^[a-z][a-z0-9_]{{0,62}}$"),
            ));
        }
    }
    sqlx::query("DELETE FROM sec_user_attributes WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clear sec_user_attributes failed: {err}")))?;

    for (key, vals) in attributes {
        sqlx::query(
            "INSERT INTO sec_user_attributes (tenant_id, user_id, attr_key, attr_values)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(key)
        .bind(vals)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert sec_user_attributes failed: {err}")))?;
    }
    Ok(())
}

async fn ensure_dept_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    dept_id: &str,
) -> Result<(), AppError> {
    let exists = sqlx::query("SELECT 1 FROM sec_departments WHERE dept_id = $1")
        .bind(dept_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("verify sec_departments failed: {err}")))?;
    if exists.is_none() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("department '{dept_id}' does not exist"),
        ));
    }
    Ok(())
}

pub(super) async fn create_guard_user_impl(
    store: &PgStore,
    tenant_id: &str,
    password_hash: &str,
    req: CreateGuardUserRequest,
) -> Result<GuardUserRecord, AppError> {
    let uname = req.username.trim();
    if uname.is_empty() || uname.len() > 64 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "username must be 1..=64 characters",
        ));
    }
    let display = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(uname);
    let dept_id = req.dept_id.trim();

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    ensure_dept_exists_tx(&mut tx, dept_id).await?;

    let row = sqlx::query(
        "INSERT INTO sec_users (
             tenant_id, username, display_name, email, dept_id, clearance,
             password_hash, auth_source, status, password_changed_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'LOCAL', 'ACTIVE', NOW())
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(uname)
    .bind(display)
    .bind(req.email.trim())
    .bind(dept_id)
    .bind(req.clearance.as_str())
    .bind(password_hash)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("USERNAME_TAKEN")
        } else {
            AppError::Database(format!("insert sec_users failed: {err}"))
        }
    })?;

    let uid: Uuid = row.get("id");
    replace_user_roles_tx(&mut tx, tenant_id, uid, &req.roles).await?;
    replace_user_attributes_tx(&mut tx, tenant_id, uid, &req.attributes).await?;
    notify_policy_changed_tx(&mut tx, tenant_id).await?;

    let created = load_single_guard_user_tx(&mut tx, uid)
        .await?
        .ok_or_else(|| AppError::Database(String::from("created user missing")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_guard_user tx: {err}")))?;
    Ok(created)
}

pub(super) async fn patch_guard_user_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    req: PatchGuardUserRequest,
) -> Result<Option<GuardUserRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let exists = sqlx::query("SELECT 1 FROM sec_users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock sec_users failed: {err}")))?;
    if exists.is_none() {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing patch user tx: {err}")))?;
        return Ok(None);
    }

    if let Some(ref d) = req.dept_id {
        ensure_dept_exists_tx(&mut tx, d.trim()).await?;
    }

    let disable_bump = req.status == Some(UserAccountStatus::Disabled);
    sqlx::query(
        "UPDATE sec_users
            SET display_name = COALESCE($2, display_name),
                email = COALESCE($3, email),
                dept_id = COALESCE($4, dept_id),
                clearance = COALESCE($5, clearance),
                status = COALESCE($6, status),
                failed_logins = CASE WHEN $7 THEN 0 ELSE failed_logins END,
                locked_until = CASE WHEN $7 THEN NULL ELSE locked_until END,
                tokens_valid_after = CASE WHEN $8 THEN NOW() ELSE tokens_valid_after END,
                updated_at = NOW()
          WHERE id = $1",
    )
    .bind(user_id)
    .bind(req.display_name.as_deref().map(str::trim))
    .bind(req.email.as_deref().map(str::trim))
    .bind(req.dept_id.as_deref().map(str::trim))
    .bind(
        req.clearance
            .map(axpoint_domain::enums::Sensitivity::as_str),
    )
    .bind(req.status.map(UserAccountStatus::as_str))
    .bind(req.unlock)
    .bind(disable_bump)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("patch sec_users failed: {err}")))?;

    if disable_bump {
        sqlx::query(
            "UPDATE sec_sessions
                SET revoked_at = COALESCE(revoked_at, NOW())
              WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("revoke disabled user sessions: {err}")))?;
    }
    if let Some(ref roles) = req.roles {
        replace_user_roles_tx(&mut tx, tenant_id, user_id, roles).await?;
    }

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    let updated = load_single_guard_user_tx(&mut tx, user_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit patch_guard_user tx: {err}")))?;
    Ok(updated)
}

pub(super) async fn put_user_attributes_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    attributes: &BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<String>>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let exists = sqlx::query("SELECT 1 FROM sec_users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("verify user for attributes: {err}")))?;
    if exists.is_none() {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing user attr tx: {err}")))?;
        return Err(AppError::NotFound("USER_NOT_FOUND"));
    }

    replace_user_attributes_tx(&mut tx, tenant_id, user_id, attributes).await?;
    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit put_user_attributes tx: {err}")))?;
    Ok(attributes.clone())
}

fn decode_role_row(row: &PgRow) -> Result<GuardRoleRecord, AppError> {
    let raw_perms: Vec<String> = row.get("permissions");
    Ok(GuardRoleRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        name: row.get("name"),
        permissions: parse_permissions_vec(raw_perms)?,
        is_builtin: row.get("is_builtin"),
        created_at: row.get("created_at"),
    })
}

pub(super) async fn list_guard_roles_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<GuardRoleRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, name, permissions, is_builtin, created_at
           FROM sec_roles
          ORDER BY is_builtin DESC, name ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_roles failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_guard_roles tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_role_row(r)?);
    }
    Ok(out)
}

pub(super) async fn create_guard_role_impl(
    store: &PgStore,
    tenant_id: &str,
    req: CreateGuardRoleRequest,
) -> Result<GuardRoleRecord, AppError> {
    let clean_name = req.name.trim();
    if clean_name.is_empty() || clean_name.len() > 64 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "role name must be 1..=64 characters",
        ));
    }
    let perm_strs: Vec<String> = req
        .permissions
        .iter()
        .map(|p| String::from(p.as_str()))
        .collect();

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO sec_roles (tenant_id, name, permissions, is_builtin)
         VALUES ($1, $2, $3, FALSE)
         RETURNING id, tenant_id, name, permissions, is_builtin, created_at",
    )
    .bind(tenant_id)
    .bind(clean_name)
    .bind(&perm_strs)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("NAME_TAKEN")
        } else {
            AppError::Database(format!("insert sec_roles failed: {err}"))
        }
    })?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    let rec = decode_role_row(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_guard_role tx: {err}")))?;
    Ok(rec)
}

pub(super) async fn patch_guard_role_impl(
    store: &PgStore,
    tenant_id: &str,
    role_id: Uuid,
    req: PatchGuardRoleRequest,
) -> Result<Option<GuardRoleRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing_opt =
        sqlx::query("SELECT name, is_builtin FROM sec_roles WHERE id = $1 FOR UPDATE")
            .bind(role_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("lock sec_roles failed: {err}")))?;

    let Some(existing) = existing_opt else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing role tx: {err}")))?;
        return Ok(None);
    };

    let cur_name: String = existing.get("name");
    let is_builtin: bool = existing.get("is_builtin");
    if let Some(ref next_name) = req.name {
        let trimmed = next_name.trim();
        if trimmed.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "role name cannot be empty",
            ));
        }
        if is_builtin && trimmed != cur_name {
            return Err(AppError::Conflict("BUILTIN_ROLE"));
        }
    }

    let perm_strs: Option<Vec<String>> = req
        .permissions
        .map(|list| list.into_iter().map(|p| String::from(p.as_str())).collect());

    let row = sqlx::query(
        "UPDATE sec_roles
            SET name = COALESCE($2, name),
                permissions = COALESCE($3, permissions)
          WHERE id = $1
      RETURNING id, tenant_id, name, permissions, is_builtin, created_at",
    )
    .bind(role_id)
    .bind(req.name.as_deref().map(str::trim))
    .bind(perm_strs.as_ref())
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("NAME_TAKEN")
        } else {
            AppError::Database(format!("update sec_roles failed: {err}"))
        }
    })?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    let rec = decode_role_row(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit patch_guard_role tx: {err}")))?;
    Ok(Some(rec))
}

pub(super) async fn delete_guard_role_impl(
    store: &PgStore,
    tenant_id: &str,
    role_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing_opt = sqlx::query("SELECT is_builtin FROM sec_roles WHERE id = $1 FOR UPDATE")
        .bind(role_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock sec_roles for delete: {err}")))?;

    let Some(existing) = existing_opt else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing role delete tx: {err}")))?;
        return Ok(false);
    };

    let is_builtin: bool = existing.get("is_builtin");
    if is_builtin {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit builtin role reject tx: {err}")))?;
        return Err(AppError::Conflict("BUILTIN_ROLE"));
    }

    sqlx::query("DELETE FROM sec_roles WHERE id = $1")
        .bind(role_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete sec_roles failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_guard_role tx: {err}")))?;
    Ok(true)
}
