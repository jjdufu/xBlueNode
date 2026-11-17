//! Operator and scheduled alert lifecycle actions: escalation, single/batch acknowledgment,
//! and manual resolution (`07-STEP07-08` §08.3.2 & §08.6).

use axpoint_domain::AppError;
use axpoint_domain::enums::{AlertStatus, OutboxTopic};
use axpoint_domain::platform::NewOutboxEvent;
use axpoint_domain::rules::{AckBatchResponse, AlertEventKind, AlertRecord, AlertResolveReason};
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::alerts::{decode_alert_row, hydrate_deliveries_tx, insert_alert_event_tx};
use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

pub(super) async fn escalate_due_alerts(
    store: &PgStore,
    tenant_id: &str,
    rule_id: Uuid,
    escalate_after_minutes: i32,
    now: DateTime<Utc>,
) -> Result<Vec<AlertRecord>, AppError> {
    if escalate_after_minutes <= 0 {
        return Ok(Vec::new());
    }
    let cutoff = now - Duration::minutes(i64::from(escalate_after_minutes));
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "UPDATE ax_alerts
            SET escalated_at = $3
          WHERE tenant_id = $1
            AND rule_id = $2
            AND status = 'OPEN'
            AND acked_at IS NULL
            AND escalated_at IS NULL
            AND first_seen_at <= $4
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .bind(now)
    .bind(cutoff)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("escalate_due_alerts failed: {err}")))?;

    let mut alerts = Vec::with_capacity(rows.len());
    for row in rows {
        let alert = decode_alert_row(&row)?;
        insert_alert_event_tx(
            &mut tx,
            tenant_id,
            alert.id,
            AlertEventKind::Escalated,
            serde_json::json!({ "escalate_after_minutes": escalate_after_minutes }),
            None,
            Some(now),
        )
        .await?;
        alerts.push(alert);
    }
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut alerts).await?;
    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit escalate_due_alerts tx failed: {err}"))
    })?;
    Ok(alerts)
}

pub(super) async fn ack_alert(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
    actor: Option<Uuid>,
    comment: &str,
) -> Result<AlertRecord, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing_row =
        sqlx::query("SELECT * FROM ax_alerts WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
            .bind(tenant_id)
            .bind(alert_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("lock ax_alerts for ack failed: {err}")))?;

    let Some(existing_row) = existing_row else {
        return Err(AppError::NotFound("ALERT_NOT_FOUND"));
    };
    let current = decode_alert_row(&existing_row)?;

    let alert = match current.status {
        AlertStatus::Resolved => return Err(AppError::Conflict("ALERT_ALREADY_RESOLVED")),
        AlertStatus::Ack => {
            if !comment.trim().is_empty() {
                insert_alert_event_tx(
                    &mut tx,
                    tenant_id,
                    alert_id,
                    AlertEventKind::Acked,
                    serde_json::json!({ "comment": comment }),
                    actor,
                    None,
                )
                .await?;
            }
            current
        }
        AlertStatus::Open => {
            let updated_row = sqlx::query(
                "UPDATE ax_alerts
                    SET status = 'ACK',
                        acked_by = $3,
                        acked_at = NOW()
                  WHERE tenant_id = $1 AND id = $2
                  RETURNING *",
            )
            .bind(tenant_id)
            .bind(alert_id)
            .bind(actor)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("ack ax_alerts failed: {err}")))?;
            insert_alert_event_tx(
                &mut tx,
                tenant_id,
                alert_id,
                AlertEventKind::Acked,
                serde_json::json!({ "comment": comment }),
                actor,
                None,
            )
            .await?;
            decode_alert_row(&updated_row)?
        }
    };

    let mut alerts = vec![alert];
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut alerts).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit ack_alert tx failed: {err}")))?;
    alerts
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Database(String::from("missing acked alert")))
}

pub(super) async fn ack_alerts_batch(
    store: &PgStore,
    tenant_id: &str,
    alert_ids: &[Uuid],
    actor: Option<Uuid>,
    comment: &str,
) -> Result<AckBatchResponse, AppError> {
    if alert_ids.is_empty() {
        return Ok(AckBatchResponse {
            acked_count: 0,
            items: Vec::new(),
        });
    }
    if alert_ids.len() > 500 {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("alert_ids length must be <= 500, got {}", alert_ids.len()),
        ));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "WITH updated AS (
            UPDATE ax_alerts
               SET status = 'ACK',
                   acked_by = $3,
                   acked_at = NOW()
             WHERE tenant_id = $1
               AND id = ANY($2)
               AND status = 'OPEN'
             RETURNING *
         )
         SELECT * FROM updated
         ORDER BY last_seen_at DESC, id DESC",
    )
    .bind(tenant_id)
    .bind(alert_ids)
    .bind(actor)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("ack_alerts_batch failed: {err}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let alert = decode_alert_row(&row)?;
        insert_alert_event_tx(
            &mut tx,
            tenant_id,
            alert.id,
            AlertEventKind::Acked,
            serde_json::json!({ "comment": comment }),
            actor,
            None,
        )
        .await?;
        items.push(alert);
    }
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut items).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit ack_alerts_batch tx failed: {err}")))?;
    Ok(AckBatchResponse {
        acked_count: items.len(),
        items,
    })
}

pub(super) async fn resolve_alert_manual(
    store: &PgStore,
    tenant_id: &str,
    alert_id: Uuid,
    actor: Option<Uuid>,
    comment: &str,
) -> Result<AlertRecord, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing_row =
        sqlx::query("SELECT * FROM ax_alerts WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
            .bind(tenant_id)
            .bind(alert_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("lock ax_alerts for resolve failed: {err}"))
            })?;

    let Some(existing_row) = existing_row else {
        return Err(AppError::NotFound("ALERT_NOT_FOUND"));
    };
    let current = decode_alert_row(&existing_row)?;
    if current.status == AlertStatus::Resolved {
        return Err(AppError::Conflict("ALERT_ALREADY_RESOLVED"));
    }

    let updated_row = sqlx::query(
        "UPDATE ax_alerts
            SET status = 'RESOLVED',
                resolve_reason = 'MANUAL',
                resolved_at = NOW(),
                last_false_at = NULL
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(alert_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("manual resolve ax_alerts failed: {err}")))?;

    let alert = decode_alert_row(&updated_row)?;
    insert_alert_event_tx(
        &mut tx,
        tenant_id,
        alert_id,
        AlertEventKind::Resolved,
        serde_json::json!({
            "resolve_reason": AlertResolveReason::Manual,
            "comment": comment,
        }),
        actor,
        None,
    )
    .await?;
    insert_outbox_tx(
        &mut tx,
        tenant_id,
        &NewOutboxEvent {
            topic: OutboxTopic::AlertResolved,
            payload: serde_json::json!({
                "alert_id": alert.id,
                "rule_id": alert.rule_id,
                "severity": alert.severity,
                "node_id": alert.node_id,
                "dataset_id": alert.dataset_id,
                "resolve_reason": AlertResolveReason::Manual,
            }),
        },
    )
    .await?;

    let mut alerts = vec![alert];
    hydrate_deliveries_tx(&mut tx, tenant_id, &mut alerts).await?;
    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit resolve_alert_manual tx failed: {err}"))
    })?;
    alerts
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Database(String::from("missing resolved alert")))
}
