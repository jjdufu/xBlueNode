//! Deduplicated alerts (`ax_alerts`), timeline events (`ax_alert_events`), delivery hydration,
//! and true/false condition transitions (`07-STEP07-08` §08.3.2).

use std::collections::HashMap;
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AlertStatus, OutboxTopic, Severity};
use axpoint_domain::platform::NewOutboxEvent;
use axpoint_domain::rules::{
    AlertEventKind, AlertEventRecord, AlertEventsQuery, AlertListQuery, AlertRecord,
    AlertResolveReason, FalseConditionOutcome, NotifyDeliveryRecord, TrueConditionInput,
    TrueConditionOutcome,
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::channels::decode_delivery_row;
use super::{
    encode_ts_uuid_cursor, parse_event_id_cursor, parse_ts_uuid_cursor, validate_page_limit,
};
use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

pub(super) fn decode_alert_row(row: &sqlx::postgres::PgRow) -> Result<AlertRecord, AppError> {
    let severity = Severity::from_str(&row.get::<String, _>("severity"))?;
    let status = AlertStatus::from_str(&row.get::<String, _>("status"))?;
    let resolve_reason = row
        .get::<Option<String>, _>("resolve_reason")
        .as_deref()
        .map(AlertResolveReason::from_str)
        .transpose()?;

    Ok(AlertRecord {
        id: row.get("id"),
        rule_id: row.get("rule_id"),
        dedup_key: row.get("dedup_key"),
        severity,
        status,
        title: row.get("title"),
        body: row.get("body"),
        node_id: row.get("node_id"),
        dataset_id: row.get("dataset_id"),
        last_value: row.get("last_value"),
        detail: row.get("detail"),
        occurrences: row.get("occurrences"),
        first_seen_at: row.get("first_seen_at"),
        last_seen_at: row.get("last_seen_at"),
        last_false_at: row.get("last_false_at"),
        acked_by: row.get("acked_by"),
        acked_at: row.get("acked_at"),
        escalated_at: row.get("escalated_at"),
        resolved_at: row.get("resolved_at"),
        resolve_reason,
        deliveries: Vec::new(),
    })
}

pub(super) fn decode_alert_event_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AlertEventRecord, AppError> {
    let event = AlertEventKind::from_str(&row.get::<String, _>("event"))?;
    Ok(AlertEventRecord {
        id: row.get("id"),
        alert_id: row.get("alert_id"),
        event,
        detail: row.get("detail"),
        actor: row.get("actor"),
        created_at: row.get("created_at"),
    })
}

pub(super) async fn hydrate_deliveries_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    alerts: &mut [AlertRecord],
) -> Result<(), AppError> {
    if alerts.is_empty() {
        return Ok(());
    }
    let alert_ids: Vec<Uuid> = alerts.iter().map(|a| a.id).collect();
    let rows = sqlx::query(
        "SELECT * FROM ax_notify_deliveries
          WHERE tenant_id = $1 AND alert_id = ANY($2)
          ORDER BY created_at ASC, id ASC",
    )
    .bind(tenant_id)
    .bind(&alert_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("hydrate ax_notify_deliveries failed: {err}")))?;

    let mut by_alert: HashMap<Uuid, Vec<NotifyDeliveryRecord>> = HashMap::new();
    for row in rows {
        let delivery = decode_delivery_row(&row)?;
        if let Some(aid) = delivery.alert_id {
            by_alert.entry(aid).or_default().push(delivery);
        }
    }
    for alert in alerts {
        if let Some(list) = by_alert.remove(&alert.id) {
            alert.deliveries = list;
        }
    }
    Ok(())
}

pub(super) async fn insert_alert_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    alert_id: Uuid,
    event: AlertEventKind,
    detail: serde_json::Value,
    actor: Option<Uuid>,
    created_at: Option<DateTime<Utc>>,
) -> Result<AlertEventRecord, AppError> {
    let row = sqlx::query(
        "INSERT INTO ax_alert_events (tenant_id, alert_id, event, detail, actor, created_at)
         VALUES ($1, $2, $3, $4, $5, COALESCE($6, NOW()))
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(alert_id)
    .bind(event.as_str())
    .bind(&detail)
    .bind(actor)
    .bind(created_at)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ax_alert_events failed: {err}")))?;
    decode_alert_event_row(&row)
}

async fn acquire_dedup_lock_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dedup_key: &str,
) -> Result<(), AppError> {
    let lock_key = format!("{tenant_id}:alert:{dedup_key}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&lock_key)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("pg_advisory_xact_lock failed: {err}")))?;
    Ok(())
}

pub(super) async fn list_alerts(
    store: &PgStore,
    tenant_id: &str,
    query: AlertListQuery,
) -> Result<PaginatedList<AlertRecord>, AppError> {
    let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
    let cursor = query
        .cursor
        .as_deref()
        .map(parse_ts_uuid_cursor)
        .transpose()?;
    let cursor_ts = cursor.map(|(ts, _)| ts);
    let cursor_id = cursor.map(|(_, id)| id);
    let type_name = query
        .type_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT a.* FROM ax_alerts a
          WHERE a.tenant_id = $1
            AND ($2::text IS NULL OR a.status = $2)
            AND ($3::text IS NULL OR a.severity = $3)
            AND ($4::uuid IS NULL OR a.rule_id = $4)
            AND ($5::uuid IS NULL OR a.node_id = $5)
            AND ($6::text IS NULL OR a.detail->>'type_name' = $6
                 OR EXISTS (
                     SELECT 1 FROM xb_nodes n
                      WHERE n.tenant_id = a.tenant_id
                        AND n.id = a.node_id
                        AND n.type_name = $6
                 ))
            AND ($7::timestamptz IS NULL OR (a.last_seen_at, a.id) < ($7, $8))
          ORDER BY a.last_seen_at DESC, a.id DESC
          LIMIT $9",
    )
    .bind(tenant_id)
    .bind(query.status.map(AlertStatus::as_str))
    .bind(query.severity.map(Severity::as_str))
    .bind(query.rule_id)
    .bind(query.node_id)
    .bind(type_name)
    .bind(cursor_ts)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_alerts failed: {err}")))?;

    let has_more = rows.len() > limit_usize;
    let mut items = Vec::with_capacity(rows.len().min(limit_usize));
    for row in rows.into_iter().take(limit_usize) {
        items.push(decode_alert_row(&row)?);
    }
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut items).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_alerts tx failed: {err}")))?;

    let next_cursor = if has_more {
        items
            .last()
            .map(|a| encode_ts_uuid_cursor(a.last_seen_at, a.id))
    } else {
        None
    };
    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn get_alert(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
) -> Result<Option<AlertRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query("SELECT * FROM ax_alerts WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(alert_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_alerts failed: {err}")))?;

    let Some(row) = row else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_alert tx failed: {err}")))?;
        return Ok(None);
    };
    let mut alerts = vec![decode_alert_row(&row)?];
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut alerts).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get_alert tx failed: {err}")))?;
    Ok(alerts.into_iter().next())
}

pub(super) async fn list_active_alerts_for_rule(
    store: &PgStore,
    tenant_id: &str,
    rule_id: Uuid,
) -> Result<Vec<AlertRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_alerts
          WHERE tenant_id = $1 AND rule_id = $2 AND status <> 'RESOLVED'
          ORDER BY first_seen_at ASC, id ASC",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list active ax_alerts failed: {err}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(decode_alert_row(&row)?);
    }
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut items).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit active alerts tx failed: {err}")))?;
    Ok(items)
}

pub(super) async fn list_alert_events(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
    query: AlertEventsQuery,
) -> Result<PaginatedList<AlertEventRecord>, AppError> {
    let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 100)?;
    let cursor_id = query
        .cursor
        .as_deref()
        .map(parse_event_id_cursor)
        .transpose()?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let exists =
        sqlx::query_scalar::<_, i32>("SELECT 1 FROM ax_alerts WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(alert_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("check ax_alerts failed: {err}")))?;
    if exists.is_none() {
        return Err(AppError::NotFound("ALERT_NOT_FOUND"));
    }

    let rows = sqlx::query(
        "SELECT * FROM ax_alert_events
          WHERE tenant_id = $1
            AND alert_id = $2
            AND ($3::bigint IS NULL OR id > $3)
          ORDER BY id ASC
          LIMIT $4",
    )
    .bind(tenant_id)
    .bind(alert_id)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_alert_events failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_alert_events tx failed: {err}")))?;

    let has_more = rows.len() > limit_usize;
    let mut items = Vec::with_capacity(rows.len().min(limit_usize));
    for row in rows.into_iter().take(limit_usize) {
        items.push(decode_alert_event_row(&row)?);
    }
    let next_cursor = if has_more {
        items.last().map(|ev| ev.id.to_string())
    } else {
        None
    };
    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn append_alert_event(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
    event: AlertEventKind,
    detail: serde_json::Value,
    actor: Option<Uuid>,
) -> Result<AlertEventRecord, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let exists =
        sqlx::query_scalar::<_, i32>("SELECT 1 FROM ax_alerts WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(alert_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("check ax_alerts failed: {err}")))?;
    if exists.is_none() {
        return Err(AppError::NotFound("ALERT_NOT_FOUND"));
    }
    let ev =
        insert_alert_event_tx(&mut tx, tenant_id, alert_id, event, detail, actor, None).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit append_alert_event tx failed: {err}")))?;
    Ok(ev)
}

pub(super) async fn set_alert_flow_run(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
    flow_id: Uuid,
    run_id: Uuid,
) -> Result<(), AppError> {
    let patch = serde_json::json!({
        "flow_id": flow_id,
        "flow_run_id": run_id,
    });
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query(
        "UPDATE ax_alerts
            SET detail = detail || $3::jsonb
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(alert_id)
    .bind(&patch)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("set_alert_flow_run update failed: {err}")))?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("ALERT_NOT_FOUND"));
    }
    insert_alert_event_tx(
        &mut tx,
        tenant_id,
        alert_id,
        AlertEventKind::FlowStarted,
        patch,
        None,
        None,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit set_alert_flow_run tx failed: {err}")))?;
    Ok(())
}

async fn retrigger_active_alert_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    active_id: Uuid,
    input: &TrueConditionInput,
) -> Result<AlertRecord, AppError> {
    let updated = sqlx::query(
        "UPDATE ax_alerts
            SET occurrences = occurrences + 1,
                last_seen_at = $3,
                last_value = $4,
                detail = CASE
                    WHEN jsonb_typeof(detail) = 'object' AND jsonb_typeof($5::jsonb) = 'object'
                    THEN jsonb_strip_nulls(jsonb_build_object(
                        'flow_id', detail->'flow_id',
                        'flow_run_id', detail->'flow_run_id'
                    )) || $5::jsonb
                    ELSE $5::jsonb
                END,
                last_false_at = NULL
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(active_id)
    .bind(input.now)
    .bind(input.last_value)
    .bind(&input.detail)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("retrigger active ax_alerts failed: {err}")))?;

    let recent_retrig = sqlx::query(
        "SELECT id, detail FROM ax_alert_events
          WHERE tenant_id = $1
            AND alert_id = $2
            AND event = 'RETRIGGERED'
            AND created_at >= ($3::timestamptz - interval '60 seconds')
          ORDER BY id DESC
          LIMIT 1
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(active_id)
    .bind(input.now)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query recent RETRIGGERED failed: {err}")))?;

    if let Some(ev_row) = recent_retrig {
        let ev_id: i64 = ev_row.get("id");
        let prev_detail: serde_json::Value = ev_row.get("detail");
        let prev_count = prev_detail
            .get("count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(1);
        let merged_detail = serde_json::json!({
            "count": prev_count.saturating_add(1),
            "last_value": input.last_value,
            "last_seen_at": input.now,
        });
        sqlx::query(
            "UPDATE ax_alert_events
                SET detail = $3
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(ev_id)
        .bind(&merged_detail)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("merge RETRIGGERED event failed: {err}")))?;
    } else {
        insert_alert_event_tx(
            tx,
            tenant_id,
            active_id,
            AlertEventKind::Retriggered,
            serde_json::json!({ "count": 1, "last_value": input.last_value }),
            None,
            Some(input.now),
        )
        .await?;
    }

    let mut alerts = vec![decode_alert_row(&updated)?];
    hydrate_deliveries_tx(tx, tenant_id, &mut alerts).await?;
    alerts
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Database(String::from("missing retriggered alert")))
}

async fn check_cooldown_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    input: &TrueConditionInput,
) -> Result<Option<AlertRecord>, AppError> {
    if input.cooldown_minutes <= 0 {
        return Ok(None);
    }
    let resolved_row = sqlx::query(
        "SELECT * FROM ax_alerts
          WHERE tenant_id = $1
            AND dedup_key = $2
            AND status = 'RESOLVED'
            AND resolved_at IS NOT NULL
          ORDER BY resolved_at DESC, id DESC
          LIMIT 1",
    )
    .bind(tenant_id)
    .bind(&input.dedup_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query cooldown ax_alerts failed: {err}")))?;

    let Some(res_row) = resolved_row else {
        return Ok(None);
    };
    let resolved_alert = decode_alert_row(&res_row)?;
    let Some(resolved_at) = resolved_alert.resolved_at else {
        return Ok(None);
    };
    if input.now.signed_duration_since(resolved_at)
        >= Duration::minutes(i64::from(input.cooldown_minutes))
    {
        return Ok(None);
    }
    insert_alert_event_tx(
        tx,
        tenant_id,
        resolved_alert.id,
        AlertEventKind::Retriggered,
        serde_json::json!({
            "suppressed_by_cooldown": true,
            "last_value": input.last_value,
        }),
        None,
        Some(input.now),
    )
    .await?;
    let mut alerts = vec![resolved_alert];
    hydrate_deliveries_tx(tx, tenant_id, &mut alerts).await?;
    Ok(alerts.into_iter().next())
}

async fn insert_open_alert_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    input: &TrueConditionInput,
) -> Result<AlertRecord, AppError> {
    let inserted = sqlx::query(
        "INSERT INTO ax_alerts (
             tenant_id, rule_id, dedup_key, severity, status, title, body,
             node_id, dataset_id, last_value, detail, occurrences,
             first_seen_at, last_seen_at
         ) VALUES (
             $1, $2, $3, $4, 'OPEN', $5, $6,
             $7, $8, $9, $10, 1,
             $11, $11
         )
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(input.rule_id)
    .bind(&input.dedup_key)
    .bind(input.severity.as_str())
    .bind(&input.title)
    .bind(&input.body)
    .bind(input.node_id)
    .bind(input.dataset_id)
    .bind(input.last_value)
    .bind(&input.detail)
    .bind(input.now)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert OPEN ax_alerts failed: {err}")))?;

    let alert = decode_alert_row(&inserted)?;
    insert_alert_event_tx(
        tx,
        tenant_id,
        alert.id,
        AlertEventKind::Opened,
        input.detail.clone(),
        None,
        Some(input.now),
    )
    .await?;
    insert_outbox_tx(
        tx,
        tenant_id,
        &NewOutboxEvent {
            topic: OutboxTopic::AlertOpened,
            payload: serde_json::json!({
                "alert_id": alert.id,
                "rule_id": input.rule_id,
                "severity": input.severity,
                "node_id": input.node_id,
                "dataset_id": input.dataset_id,
            }),
        },
    )
    .await?;
    Ok(alert)
}

pub(super) async fn record_true_condition(
    store: &PgStore,
    tenant_id: &str,
    input: TrueConditionInput,
) -> Result<TrueConditionOutcome, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    acquire_dedup_lock_tx(&mut tx, tenant_id, &input.dedup_key).await?;

    let active_row = sqlx::query(
        "SELECT id FROM ax_alerts
          WHERE tenant_id = $1 AND dedup_key = $2 AND status <> 'RESOLVED'
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(&input.dedup_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check active ax_alerts failed: {err}")))?;

    if let Some(row) = active_row {
        let active_id: Uuid = row.get("id");
        let alert = retrigger_active_alert_tx(&mut tx, tenant_id, active_id, &input).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit retrigger tx failed: {err}")))?;
        return Ok(TrueConditionOutcome::RetriggeredActive(alert));
    }

    if let Some(cooldown_alert) = check_cooldown_tx(&mut tx, tenant_id, &input).await? {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit cooldown retrigger tx failed: {err}"))
        })?;
        return Ok(TrueConditionOutcome::SuppressedByCooldown(cooldown_alert));
    }

    let alert = insert_open_alert_tx(&mut tx, tenant_id, &input).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit open alert tx failed: {err}")))?;
    Ok(TrueConditionOutcome::Opened(alert))
}

async fn auto_resolve_alert_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    current: &AlertRecord,
    now: DateTime<Utc>,
) -> Result<AlertRecord, AppError> {
    let resolved_row = sqlx::query(
        "UPDATE ax_alerts
            SET status = 'RESOLVED',
                resolve_reason = 'AUTO',
                resolved_at = $3,
                last_false_at = COALESCE(last_false_at, $3)
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(current.id)
    .bind(now)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("auto-resolve ax_alerts failed: {err}")))?;

    let mut alerts = vec![decode_alert_row(&resolved_row)?];
    insert_alert_event_tx(
        tx,
        tenant_id,
        current.id,
        AlertEventKind::Resolved,
        serde_json::json!({ "resolve_reason": AlertResolveReason::Auto }),
        None,
        Some(now),
    )
    .await?;
    insert_outbox_tx(
        tx,
        tenant_id,
        &NewOutboxEvent {
            topic: OutboxTopic::AlertResolved,
            payload: serde_json::json!({
                "alert_id": current.id,
                "rule_id": current.rule_id,
                "severity": current.severity,
                "node_id": current.node_id,
                "dataset_id": current.dataset_id,
                "resolve_reason": AlertResolveReason::Auto,
            }),
        },
    )
    .await?;
    hydrate_deliveries_tx(tx, tenant_id, &mut alerts).await?;
    alerts
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Database(String::from("missing auto-resolved alert")))
}

pub(super) async fn record_false_condition(
    store: &PgStore,
    tenant_id: &str,
    dedup_key: &str,
    resolve_after_minutes: i32,
    now: DateTime<Utc>,
) -> Result<FalseConditionOutcome, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    acquire_dedup_lock_tx(&mut tx, tenant_id, dedup_key).await?;

    let active_row = sqlx::query(
        "SELECT * FROM ax_alerts
          WHERE tenant_id = $1 AND dedup_key = $2 AND status <> 'RESOLVED'
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(dedup_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load active alert for false cond failed: {err}")))?;

    let Some(active_row) = active_row else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit no-active-alert tx failed: {err}"))
        })?;
        return Ok(FalseConditionOutcome::NoActiveAlert);
    };

    let current = decode_alert_row(&active_row)?;
    let false_since = current.last_false_at.unwrap_or(now);
    let should_resolve = resolve_after_minutes <= 0
        || now.signed_duration_since(false_since)
            >= Duration::minutes(i64::from(resolve_after_minutes));

    if should_resolve {
        let alert = auto_resolve_alert_tx(&mut tx, tenant_id, &current, now).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit auto-resolve tx failed: {err}")))?;
        Ok(FalseConditionOutcome::AutoResolved(alert))
    } else {
        let pending_row = sqlx::query(
            "UPDATE ax_alerts
                SET last_false_at = COALESCE(last_false_at, $3)
              WHERE tenant_id = $1 AND id = $2
              RETURNING *",
        )
        .bind(tenant_id)
        .bind(current.id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("set last_false_at failed: {err}")))?;
        let mut alerts = vec![decode_alert_row(&pending_row)?];
        hydrate_deliveries_tx(&mut tx, tenant_id, &mut alerts).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit pending-auto-resolve tx failed: {err}"))
        })?;
        let alert = alerts
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Database(String::from("missing pending alert")))?;
        Ok(FalseConditionOutcome::PendingAutoResolve(alert))
    }
}
