//! Platform `ax_backups` and tenant-scoped `ax_template_installs` persistence (`§14.3`, `§14.5`).

use std::collections::BTreeMap;
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::ops::{
    BackupKind, BackupRecord, BackupStatus, NewTemplateInstallInput, TemplateCreatedObject,
    TemplateInstallPlan, TemplateInstallRecord, TemplateInstallStatus,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::{is_sqlstate, validate_page_limit};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn decode_backup_record(row: &sqlx::postgres::PgRow) -> Result<BackupRecord, AppError> {
    let kind_raw: String = row.get("backup_kind");
    let status_raw: String = row.get("status");
    Ok(BackupRecord {
        id: row.get("id"),
        backup_kind: BackupKind::from_str(&kind_raw)?,
        status: BackupStatus::from_str(&status_raw)?,
        location: row.get("location"),
        size_bytes: row.get("size_bytes"),
        manifest: row.get("manifest"),
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        verified_at: row.get("verified_at"),
    })
}

fn decode_template_install(row: &sqlx::postgres::PgRow) -> Result<TemplateInstallRecord, AppError> {
    let status_raw: String = row.get("status");
    let plan_val: serde_json::Value = row.get("plan");
    let map_val: serde_json::Value = row.get("object_map");

    let plan: TemplateInstallPlan = serde_json::from_value(plan_val).map_err(|err| {
        AppError::Database(format!("decode ax_template_installs.plan failed: {err}"))
    })?;
    let object_map: BTreeMap<String, TemplateCreatedObject> = serde_json::from_value(map_val)
        .map_err(|err| {
            AppError::Database(format!(
                "decode ax_template_installs.object_map failed: {err}"
            ))
        })?;

    Ok(TemplateInstallRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        pack_id: row.get("pack_id"),
        pack_version: row.get("pack_version"),
        status: TemplateInstallStatus::from_str(&status_raw)?,
        plan,
        object_map,
        error_message: row.get("error_message"),
        installed_by: row.get("installed_by"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
    })
}

async fn resolve_backup_cursor(
    store: &PgStore,
    raw_cursor: &str,
) -> Result<(DateTime<Utc>, Uuid), AppError> {
    if let Some((ts_part, id_part)) = raw_cursor.split_once('|') {
        let parsed_ts = DateTime::parse_from_rfc3339(ts_part)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
            .with_timezone(&Utc);
        let parsed_id = Uuid::parse_str(id_part).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}"))
        })?;
        return Ok((parsed_ts, parsed_id));
    }

    if let Ok(parsed_id) = Uuid::parse_str(raw_cursor) {
        let row = sqlx::query("SELECT started_at FROM ax_backups WHERE id = $1")
            .bind(parsed_id)
            .fetch_optional(store.pool())
            .await
            .map_err(|err| AppError::Database(format!("resolve backup cursor failed: {err}")))?
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "cursor backup not found"))?;
        let started_at: DateTime<Utc> = row.get("started_at");
        return Ok((started_at, parsed_id));
    }

    let parsed_ts = DateTime::parse_from_rfc3339(raw_cursor)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid backup cursor: {err}")))?
        .with_timezone(&Utc);
    Ok((parsed_ts, Uuid::max()))
}

pub(super) async fn create_backup_impl(
    store: &PgStore,
    backup_kind: BackupKind,
) -> Result<BackupRecord, AppError> {
    let row = sqlx::query(
        "INSERT INTO ax_backups (backup_kind, status, location, manifest)
         VALUES ($1, 'PENDING', '', '{}'::jsonb)
         RETURNING id, backup_kind, status, location, size_bytes, manifest,
                   error_message, started_at, finished_at, verified_at",
    )
    .bind(backup_kind.as_str())
    .fetch_one(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("insert ax_backups failed: {err}")))?;

    decode_backup_record(&row)
}

pub(super) async fn list_backups_impl(
    store: &PgStore,
    limit: u32,
    cursor: Option<&str>,
) -> Result<PaginatedList<BackupRecord>, AppError> {
    let (limit_usize, fetch_limit) = validate_page_limit(Some(limit))?;
    let (cursor_ts, cursor_id) = match cursor.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let (ts, cid) = resolve_backup_cursor(store, raw).await?;
            (Some(ts), Some(cid))
        }
        None => (None, None),
    };

    let rows = sqlx::query(
        "SELECT id, backup_kind, status, location, size_bytes, manifest,
                error_message, started_at, finished_at, verified_at
           FROM ax_backups
          WHERE ($1::timestamptz IS NULL OR (started_at, id) < ($1, $2::uuid))
          ORDER BY started_at DESC, id DESC
          LIMIT $3",
    )
    .bind(cursor_ts)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("list ax_backups failed: {err}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(decode_backup_record(row)?);
    }

    let next_cursor = if items.len() > limit_usize {
        items.truncate(limit_usize);
        items
            .last()
            .map(|last| format!("{}|{}", last.started_at.to_rfc3339(), last.id))
    } else {
        None
    };

    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn get_backup_impl(
    store: &PgStore,
    backup_id: Uuid,
) -> Result<Option<BackupRecord>, AppError> {
    let maybe_row = sqlx::query(
        "SELECT id, backup_kind, status, location, size_bytes, manifest,
                error_message, started_at, finished_at, verified_at
           FROM ax_backups
          WHERE id = $1",
    )
    .bind(backup_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("get ax_backups failed: {err}")))?;

    maybe_row.as_ref().map(decode_backup_record).transpose()
}

pub(super) async fn create_template_install_impl(
    store: &PgStore,
    tenant_id: &str,
    input: &NewTemplateInstallInput,
) -> Result<TemplateInstallRecord, AppError> {
    let plan_json = serde_json::to_value(&input.plan).map_err(|err| {
        AppError::Database(format!("serialize TemplateInstallPlan failed: {err}"))
    })?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO ax_template_installs (
             tenant_id, pack_id, pack_version, status, plan, object_map, installed_by
         )
         VALUES ($1, $2, $3, 'PLANNED', $4, '{}'::jsonb, $5)
         RETURNING id, tenant_id, pack_id, pack_version, status, plan, object_map,
                   error_message, installed_by, created_at, finished_at",
    )
    .bind(tenant_id)
    .bind(&input.pack_id)
    .bind(&input.pack_version)
    .bind(plan_json)
    .bind(input.installed_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_sqlstate(&err, "23505") {
            AppError::Conflict("CONFLICT")
        } else {
            AppError::Database(format!("insert ax_template_installs failed: {err}"))
        }
    })?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit create_template_install tx failed: {err}"))
    })?;

    decode_template_install(&row)
}

pub(super) async fn list_template_installs_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<TemplateInstallRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, pack_id, pack_version, status, plan, object_map,
                error_message, installed_by, created_at, finished_at
           FROM ax_template_installs
          WHERE tenant_id = $1
          ORDER BY created_at DESC, id DESC",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_template_installs failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit list_template_installs tx failed: {err}"))
    })?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_template_install(row)?);
    }
    Ok(out)
}

pub(super) async fn get_template_install_impl(
    store: &PgStore,
    tenant_id: &str,
    install_id: Uuid,
) -> Result<Option<TemplateInstallRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let maybe_row = sqlx::query(
        "SELECT id, tenant_id, pack_id, pack_version, status, plan, object_map,
                error_message, installed_by, created_at, finished_at
           FROM ax_template_installs
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(install_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("get ax_template_installs failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit get_template_install tx failed: {err}"))
    })?;

    maybe_row.as_ref().map(decode_template_install).transpose()
}

pub(super) async fn update_template_install_progress_impl(
    store: &PgStore,
    tenant_id: &str,
    install_id: Uuid,
    status: TemplateInstallStatus,
    object_map: &BTreeMap<String, TemplateCreatedObject>,
    error_message: Option<&str>,
) -> Result<TemplateInstallRecord, AppError> {
    let map_json = serde_json::to_value(object_map).map_err(|err| {
        AppError::Database(format!(
            "serialize ax_template_installs.object_map failed: {err}"
        ))
    })?;
    let is_finished = matches!(
        status,
        TemplateInstallStatus::Installed
            | TemplateInstallStatus::Failed
            | TemplateInstallStatus::Uninstalled
    );

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let maybe_row = sqlx::query(
        "UPDATE ax_template_installs
            SET status = $3,
                object_map = $4,
                error_message = $5,
                finished_at = CASE WHEN $6 THEN NOW() ELSE NULL END
          WHERE tenant_id = $1
            AND id = $2
         RETURNING id, tenant_id, pack_id, pack_version, status, plan, object_map,
                   error_message, installed_by, created_at, finished_at",
    )
    .bind(tenant_id)
    .bind(install_id)
    .bind(status.as_str())
    .bind(map_json)
    .bind(error_message)
    .bind(is_finished)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| {
        if is_sqlstate(&err, "23505") {
            AppError::Conflict("CONFLICT")
        } else {
            AppError::Database(format!("update ax_template_installs failed: {err}"))
        }
    })?;

    let Some(row) = maybe_row else {
        return Err(AppError::NotFound("NOT_FOUND"));
    };

    tx.commit().await.map_err(|err| {
        AppError::Database(format!(
            "commit update_template_install_progress tx failed: {err}"
        ))
    })?;

    decode_template_install(&row)
}
