//! Notification channels (`ax_notify_channels`), `ds_credentials` binding, and idempotent
//! delivery retries (`ax_notify_deliveries`, `07-STEP07-08` §08.4).

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{PaginatedList, SealedCredential};
use axpoint_domain::enums::NotifyKind;
use axpoint_domain::rules::{
    AlertEventKind, DeliveryFailureUpdate, EnqueueDeliveryInput, NotifyChannelRecord,
    NotifyDeliveryEvent, NotifyDeliveryRecord, NotifyDeliveryStatus, SaveNotifyChannelInput,
    SimplePageQuery,
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::alerts::insert_alert_event_tx;
use super::{
    encode_ts_uuid_cursor, is_unique_violation, parse_ts_uuid_cursor, validate_page_limit,
};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

const INTERNAL_SECRET_FIELDS_KEY: &str = "__secret_fields";

fn split_channel_config(raw: serde_json::Value) -> (serde_json::Value, Vec<String>) {
    match raw {
        serde_json::Value::Object(mut map) => {
            let mut secret_fields: Vec<String> = match map.remove(INTERNAL_SECRET_FIELDS_KEY) {
                Some(serde_json::Value::Array(arr)) => arr
                    .into_iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                _ => Vec::new(),
            };
            secret_fields.sort();
            secret_fields.dedup();
            (serde_json::Value::Object(map), secret_fields)
        }
        other => (other, Vec::new()),
    }
}

fn strip_secret_fields_meta(raw: serde_json::Value) -> serde_json::Value {
    match raw {
        serde_json::Value::Object(mut map) => {
            map.remove(INTERNAL_SECRET_FIELDS_KEY);
            serde_json::Value::Object(map)
        }
        other => other,
    }
}

fn decode_channel_row(row: &sqlx::postgres::PgRow) -> Result<NotifyChannelRecord, AppError> {
    let kind = NotifyKind::from_str(&row.get::<String, _>("kind"))?;
    let raw_cfg: serde_json::Value = row.get("config");
    let (config, secret_fields_set) = split_channel_config(raw_cfg);
    Ok(NotifyChannelRecord {
        id: row.get("id"),
        name: row.get("name"),
        kind,
        config,
        credential_id: row.get("credential_id"),
        secret_fields_set,
        allow_private_network: row.get("allow_private_network"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
    })
}

pub(super) fn decode_delivery_row(
    row: &sqlx::postgres::PgRow,
) -> Result<NotifyDeliveryRecord, AppError> {
    let event = NotifyDeliveryEvent::from_str(&row.get::<String, _>("event"))?;
    let status = NotifyDeliveryStatus::from_str(&row.get::<String, _>("status"))?;
    Ok(NotifyDeliveryRecord {
        id: row.get("id"),
        alert_id: row.get("alert_id"),
        channel_id: row.get("channel_id"),
        event,
        status,
        attempt: row.get("attempt"),
        response_code: row.get("response_code"),
        error_message: row.get("error_message"),
        next_retry_at: row.get("next_retry_at"),
        created_at: row.get("created_at"),
        sent_at: row.get("sent_at"),
    })
}

fn validate_channel_input(input: &SaveNotifyChannelInput) -> Result<(), AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "channel name must not be empty",
        ));
    }
    if !input.config.is_object() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "channel config must be a JSON object",
        ));
    }
    Ok(())
}

async fn upsert_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    cred_id: Uuid,
    key_version: i32,
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ds_credentials (id, tenant_id, key_version, nonce, ciphertext)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
             key_version = EXCLUDED.key_version,
             nonce = EXCLUDED.nonce,
             ciphertext = EXCLUDED.ciphertext,
             rotated_at = NOW()",
    )
    .bind(cred_id)
    .bind(tenant_id)
    .bind(key_version)
    .bind(nonce)
    .bind(ciphertext)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert ds_credentials failed: {err}")))?;
    Ok(())
}

async fn ensure_credential_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    cred_id: Uuid,
) -> Result<(), AppError> {
    let exists = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM ds_credentials WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(cred_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check ds_credentials failed: {err}")))?;
    if exists.is_none() {
        return Err(AppError::NotFound("CREDENTIAL_NOT_FOUND"));
    }
    Ok(())
}

async fn delete_orphan_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    cred_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM ds_credentials c
          WHERE c.id = $1
            AND NOT EXISTS (SELECT 1 FROM ds_connections conn WHERE conn.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ds_webhooks wh WHERE wh.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ax_notify_channels ch WHERE ch.credential_id = c.id)",
    )
    .bind(cred_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete orphan ds_credentials failed: {err}")))?;
    Ok(())
}

async fn resolve_channel_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    existing_cred_id: Option<Uuid>,
    input: &SaveNotifyChannelInput,
) -> Result<Option<Uuid>, AppError> {
    if input.clear_credential {
        return Ok(None);
    }
    if let Some((key_version, nonce, ciphertext)) = &input.new_sealed_secret {
        let cid = input
            .credential_id
            .or(existing_cred_id)
            .unwrap_or_else(Uuid::new_v4);
        upsert_credential_tx(tx, tenant_id, cid, *key_version, nonce, ciphertext).await?;
        return Ok(Some(cid));
    }
    if let Some(cid) = input.credential_id {
        if Some(cid) != existing_cred_id {
            ensure_credential_exists_tx(tx, tenant_id, cid).await?;
        }
        return Ok(Some(cid));
    }
    Ok(existing_cred_id)
}

pub(super) async fn list_channels(
    store: &PgStore,
    tenant_id: &str,
    query: SimplePageQuery,
) -> Result<PaginatedList<NotifyChannelRecord>, AppError> {
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
        "SELECT * FROM ax_notify_channels
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
    .map_err(|err| AppError::Database(format!("list ax_notify_channels failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_channels tx failed: {err}")))?;

    let has_more = rows.len() > limit_usize;
    let mut items = Vec::with_capacity(rows.len().min(limit_usize));
    for row in rows.into_iter().take(limit_usize) {
        items.push(decode_channel_row(&row)?);
    }
    let next_cursor = if has_more {
        items
            .last()
            .map(|ch| encode_ts_uuid_cursor(ch.created_at, ch.id))
    } else {
        None
    };
    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn get_channel(
    store: &PgStore,
    tenant_id: &str,
    channel_id: Uuid,
) -> Result<Option<(NotifyChannelRecord, Option<SealedCredential>)>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "SELECT ch.*,
                c.key_version AS cred_key_version,
                c.nonce       AS cred_nonce,
                c.ciphertext  AS cred_ciphertext
           FROM ax_notify_channels ch
           LEFT JOIN ds_credentials c
                  ON c.id = ch.credential_id AND c.tenant_id = ch.tenant_id
          WHERE ch.tenant_id = $1 AND ch.id = $2",
    )
    .bind(tenant_id)
    .bind(channel_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("get ax_notify_channels failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get_channel tx failed: {err}")))?;

    let Some(row) = row else {
        return Ok(None);
    };
    let channel = decode_channel_row(&row)?;
    let sealed = match channel.credential_id {
        Some(cid) => {
            let kv: Option<i32> = row.try_get("cred_key_version").ok().flatten();
            let nonce: Option<Vec<u8>> = row.try_get("cred_nonce").ok().flatten();
            let ciphertext: Option<Vec<u8>> = row.try_get("cred_ciphertext").ok().flatten();
            match (kv, nonce, ciphertext) {
                (Some(key_version), Some(n), Some(ct)) => Some(SealedCredential {
                    id: cid,
                    key_version,
                    nonce: n,
                    ciphertext: ct,
                }),
                _ => None,
            }
        }
        None => None,
    };
    Ok(Some((channel, sealed)))
}

pub(super) async fn create_channel(
    store: &PgStore,
    tenant_id: &str,
    input: SaveNotifyChannelInput,
) -> Result<NotifyChannelRecord, AppError> {
    validate_channel_input(&input)?;
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let cred_id = resolve_channel_credential_tx(&mut tx, tenant_id, None, &input).await?;
    let cfg_to_store = if cred_id.is_none() {
        strip_secret_fields_meta(input.config)
    } else {
        input.config
    };

    let row = sqlx::query(
        "INSERT INTO ax_notify_channels (
             tenant_id, name, kind, config, credential_id, allow_private_network, enabled
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(input.name.trim())
    .bind(input.kind.as_str())
    .bind(&cfg_to_store)
    .bind(cred_id)
    .bind(input.allow_private_network)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("CHANNEL_NAME_CONFLICT")
        } else {
            AppError::Database(format!("insert ax_notify_channels failed: {err}"))
        }
    })?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_channel tx failed: {err}")))?;
    decode_channel_row(&row)
}

pub(super) async fn update_channel(
    store: &PgStore,
    tenant_id: &str,
    channel_id: Uuid,
    input: SaveNotifyChannelInput,
) -> Result<Option<NotifyChannelRecord>, AppError> {
    validate_channel_input(&input)?;
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing = sqlx::query(
        "SELECT credential_id FROM ax_notify_channels
          WHERE tenant_id = $1 AND id = $2
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(channel_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("lock ax_notify_channels failed: {err}")))?;

    let Some(existing) = existing else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit update_channel tx failed: {err}")))?;
        return Ok(None);
    };
    let old_cred_id: Option<Uuid> = existing.get("credential_id");
    let final_cred_id =
        resolve_channel_credential_tx(&mut tx, tenant_id, old_cred_id, &input).await?;
    let cfg_to_store = if final_cred_id.is_none() {
        strip_secret_fields_meta(input.config)
    } else {
        input.config
    };

    let row = sqlx::query(
        "UPDATE ax_notify_channels
            SET name = $3,
                kind = $4,
                config = $5,
                credential_id = $6,
                allow_private_network = $7,
                enabled = $8
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(channel_id)
    .bind(input.name.trim())
    .bind(input.kind.as_str())
    .bind(&cfg_to_store)
    .bind(final_cred_id)
    .bind(input.allow_private_network)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("CHANNEL_NAME_CONFLICT")
        } else {
            AppError::Database(format!("update ax_notify_channels failed: {err}"))
        }
    })?;

    if let Some(old_cid) = old_cred_id
        && final_cred_id != Some(old_cid)
    {
        delete_orphan_credential_tx(&mut tx, old_cid).await?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit update_channel tx failed: {err}")))?;
    Ok(Some(decode_channel_row(&row)?))
}

pub(super) async fn delete_channel(
    store: &PgStore,
    tenant_id: &str,
    channel_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let deleted = sqlx::query(
        "DELETE FROM ax_notify_channels
          WHERE tenant_id = $1 AND id = $2
          RETURNING credential_id",
    )
    .bind(tenant_id)
    .bind(channel_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("delete ax_notify_channels failed: {err}")))?;

    let Some(row) = deleted else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_channel tx failed: {err}")))?;
        return Ok(false);
    };
    if let Some(old_cid) = row.get::<Option<Uuid>, _>("credential_id") {
        delete_orphan_credential_tx(&mut tx, old_cid).await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_channel tx failed: {err}")))?;
    Ok(true)
}

pub(super) async fn enqueue_delivery(
    store: &PgStore,
    tenant_id: &str,
    input: EnqueueDeliveryInput,
) -> Result<(NotifyDeliveryRecord, bool), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let ch_exists = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM ax_notify_channels WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(input.channel_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| {
        AppError::Database(format!("check channel in enqueue_delivery failed: {err}"))
    })?;
    if ch_exists.is_none() {
        return Err(AppError::NotFound("CHANNEL_NOT_FOUND"));
    }

    if let Some(alert_id) = input.alert_id {
        let alert_exists = sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM ax_alerts WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(alert_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("check alert in enqueue_delivery failed: {err}"))
        })?;
        if alert_exists.is_none() {
            return Err(AppError::NotFound("ALERT_NOT_FOUND"));
        }

        let inserted = sqlx::query(
            "INSERT INTO ax_notify_deliveries (
                 tenant_id, alert_id, channel_id, event, status, attempt, next_retry_at
             ) VALUES ($1, $2, $3, $4, 'PENDING', 0, $5)
             ON CONFLICT (alert_id, channel_id, event) DO NOTHING
             RETURNING *",
        )
        .bind(tenant_id)
        .bind(alert_id)
        .bind(input.channel_id)
        .bind(input.event.as_str())
        .bind(input.now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("enqueue alert delivery failed: {err}")))?;

        let (record, existing) = if let Some(row) = inserted {
            (decode_delivery_row(&row)?, false)
        } else {
            let existing_row = sqlx::query(
                "SELECT * FROM ax_notify_deliveries
                  WHERE tenant_id = $1
                    AND alert_id = $2
                    AND channel_id = $3
                    AND event = $4",
            )
            .bind(tenant_id)
            .bind(alert_id)
            .bind(input.channel_id)
            .bind(input.event.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("load existing delivery failed: {err}")))?;
            (decode_delivery_row(&existing_row)?, true)
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit enqueue_delivery tx failed: {err}"))
        })?;
        return Ok((record, existing));
    }

    let row = sqlx::query(
        "INSERT INTO ax_notify_deliveries (
             tenant_id, alert_id, channel_id, event, status, attempt, next_retry_at
         ) VALUES ($1, NULL, $2, $3, 'PENDING', 0, $4)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(input.channel_id)
    .bind(input.event.as_str())
    .bind(input.now)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert non-alert delivery failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit enqueue_delivery tx failed: {err}")))?;
    Ok((decode_delivery_row(&row)?, false))
}

pub(super) async fn get_delivery(
    store: &PgStore,
    tenant_id: &str,
    delivery_id: Uuid,
) -> Result<Option<NotifyDeliveryRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query("SELECT * FROM ax_notify_deliveries WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(delivery_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_notify_deliveries failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get_delivery tx failed: {err}")))?;
    row.as_ref().map(decode_delivery_row).transpose()
}

pub(super) async fn mark_delivery_sent(
    store: &PgStore,
    tenant_id: &str,
    delivery_id: Uuid,
    attempt: i32,
    response_code: Option<i32>,
) -> Result<NotifyDeliveryRecord, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "UPDATE ax_notify_deliveries
            SET status = 'SENT',
                attempt = $3,
                response_code = $4,
                error_message = NULL,
                sent_at = NOW()
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(delivery_id)
    .bind(attempt)
    .bind(response_code)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("mark_delivery_sent failed: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::NotFound("DELIVERY_NOT_FOUND"));
    };
    let delivery = decode_delivery_row(&row)?;
    if let Some(alert_id) = delivery.alert_id {
        insert_alert_event_tx(
            &mut tx,
            tenant_id,
            alert_id,
            AlertEventKind::Notified,
            serde_json::json!({
                "delivery_id": delivery.id,
                "channel_id": delivery.channel_id,
                "delivery_event": delivery.event,
                "attempt": attempt,
                "response_code": response_code,
            }),
            None,
            None,
        )
        .await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit mark_delivery_sent tx failed: {err}")))?;
    Ok(delivery)
}

pub(super) async fn mark_delivery_failed(
    store: &PgStore,
    tenant_id: &str,
    delivery_id: Uuid,
    update: DeliveryFailureUpdate,
) -> Result<NotifyDeliveryRecord, AppError> {
    let new_status = if update.next_retry_at.is_some() {
        NotifyDeliveryStatus::Pending
    } else {
        NotifyDeliveryStatus::Failed
    };

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "UPDATE ax_notify_deliveries
            SET status = $3,
                attempt = $4,
                response_code = $5,
                error_message = $6,
                next_retry_at = COALESCE($7, next_retry_at)
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(delivery_id)
    .bind(new_status.as_str())
    .bind(update.attempt)
    .bind(update.response_code)
    .bind(&update.error_message)
    .bind(update.next_retry_at)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("mark_delivery_failed failed: {err}")))?;

    let Some(row) = row else {
        return Err(AppError::NotFound("DELIVERY_NOT_FOUND"));
    };
    let delivery = decode_delivery_row(&row)?;
    if update.next_retry_at.is_none()
        && let Some(alert_id) = delivery.alert_id
    {
        insert_alert_event_tx(
            &mut tx,
            tenant_id,
            alert_id,
            AlertEventKind::NotifyFailed,
            serde_json::json!({
                "delivery_id": delivery.id,
                "channel_id": delivery.channel_id,
                "delivery_event": delivery.event,
                "attempt": update.attempt,
                "response_code": update.response_code,
                "error_message": update.error_message,
            }),
            None,
            None,
        )
        .await?;
    }
    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit mark_delivery_failed tx failed: {err}"))
    })?;
    Ok(delivery)
}
