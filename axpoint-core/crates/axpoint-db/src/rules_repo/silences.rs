//! Alert silence windows (`ax_alert_silences`) CRUD and active-window lookups (`07-STEP07-08` §08.4).

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::rules::{
    AlertSilenceRecord, CreateSilenceRequest, SilenceMatcher, SimplePageQuery,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::{encode_ts_uuid_cursor, parse_ts_uuid_cursor, validate_page_limit};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn decode_silence_row(row: &sqlx::postgres::PgRow) -> Result<AlertSilenceRecord, AppError> {
    let matcher_raw: serde_json::Value = row.get("matcher");
    let matcher: SilenceMatcher = serde_json::from_value(matcher_raw).map_err(|err| {
        AppError::Database(format!("decode ax_alert_silences.matcher failed: {err}"))
    })?;
    Ok(AlertSilenceRecord {
        id: row.get("id"),
        matcher,
        starts_at: row.get("starts_at"),
        ends_at: row.get("ends_at"),
        reason: row.get("reason"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    })
}

pub(super) async fn list_silences(
    store: &PgStore,
    tenant_id: &str,
    query: SimplePageQuery,
) -> Result<PaginatedList<AlertSilenceRecord>, AppError> {
    let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
    let cursor = query
        .cursor
        .as_deref()
        .map(parse_ts_uuid_cursor)
        .transpose()?;
    let cursor_ts = cursor.map(|(ts, _)| ts);
    let cursor_id = cursor.map(|(_, id)| id);

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_alert_silences
          WHERE tenant_id = $1
            AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
          ORDER BY created_at DESC, id DESC
          LIMIT $4",
    )
    .bind(tenant_id)
    .bind(cursor_ts)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_alert_silences failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_silences tx failed: {err}")))?;

    let has_more = rows.len() > limit_usize;
    let mut items = Vec::with_capacity(rows.len().min(limit_usize));
    for row in rows.into_iter().take(limit_usize) {
        items.push(decode_silence_row(&row)?);
    }
    let next_cursor = if has_more {
        items
            .last()
            .map(|s| encode_ts_uuid_cursor(s.created_at, s.id))
    } else {
        None
    };
    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn list_active_silences(
    store: &PgStore,
    tenant_id: &str,
    at: DateTime<Utc>,
) -> Result<Vec<AlertSilenceRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_alert_silences
          WHERE tenant_id = $1
            AND starts_at <= $2
            AND $2 < ends_at
          ORDER BY created_at DESC, id DESC",
    )
    .bind(tenant_id)
    .bind(at)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list active ax_alert_silences failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit active silences tx failed: {err}")))?;

    rows.iter().map(decode_silence_row).collect()
}

pub(super) async fn create_silence(
    store: &PgStore,
    tenant_id: &str,
    created_by: Option<Uuid>,
    req: CreateSilenceRequest,
) -> Result<AlertSilenceRecord, AppError> {
    if req.ends_at <= req.starts_at {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "ends_at must be strictly greater than starts_at",
        ));
    }
    let matcher_json = serde_json::to_value(&req.matcher)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize matcher: {err}")))?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO ax_alert_silences (tenant_id, matcher, starts_at, ends_at, reason, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(&matcher_json)
    .bind(req.starts_at)
    .bind(req.ends_at)
    .bind(&req.reason)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ax_alert_silences failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_silence tx failed: {err}")))?;
    decode_silence_row(&row)
}

pub(super) async fn delete_silence(
    store: &PgStore,
    tenant_id: &str,
    silence_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query("DELETE FROM ax_alert_silences WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(silence_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete ax_alert_silences failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_silence tx failed: {err}")))?;
    Ok(res.rows_affected() > 0)
}
