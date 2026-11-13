//! Transactional event outbox (`ax_outbox`) helpers and [`OutboxPort`] implementation.

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::enums::OutboxTopic;
use axpoint_domain::platform::{NewOutboxEvent, OutboxEvent};
use axpoint_domain::ports::OutboxPort;
use sqlx::Row;

use crate::PgStore;

/// Inserts a [`NewOutboxEvent`] into `ax_outbox` within an active tenant-scoped transaction.
///
/// The database `AFTER INSERT` trigger `trg_ax_outbox_notify` automatically emits `NOTIFY ax_outbox`
/// when the enclosing transaction commits.
///
/// # Errors
/// Returns [`AppError::Database`] if inserting into `ax_outbox` fails.
pub async fn insert_outbox_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    event: &NewOutboxEvent,
) -> Result<i64, AppError> {
    let row = sqlx::query(
        "INSERT INTO ax_outbox (tenant_id, topic, payload)
         VALUES ($1, $2, $3)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(event.topic.as_str())
    .bind(&event.payload)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ax_outbox failed: {err}")))?;

    Ok(row.get("id"))
}

impl OutboxPort for PgStore {
    async fn claim(
        &self,
        worker: &str,
        limit: u32,
        lease_secs: u32,
    ) -> Result<Vec<OutboxEvent>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let limit_i32 = i32::try_from(limit).unwrap_or(i32::MAX);
        let lease_i32 = i32::try_from(lease_secs).unwrap_or(i32::MAX);

        let rows = sqlx::query("SELECT * FROM ax_claim_outbox($1, $2, $3)")
            .bind(worker)
            .bind(limit_i32)
            .bind(lease_i32)
            .fetch_all(self.pool())
            .await
            .map_err(|err| AppError::Database(format!("ax_claim_outbox failed: {err}")))?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let topic_raw: String = row.get("topic");
            let topic = OutboxTopic::from_str(&topic_raw)?;
            events.push(OutboxEvent {
                id: row.get("id"),
                tenant_id: row.get("tenant_id"),
                topic,
                payload: row.get("payload"),
                claimed_by: row.get("claimed_by"),
                claimed_until: row.get("claimed_until"),
                created_at: row.get("created_at"),
            });
        }

        Ok(events)
    }

    async fn ack(&self, tenant_id: &str, ids: &[i64]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }

        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        sqlx::query(
            "UPDATE ax_outbox
                SET processed_at = NOW()
              WHERE id = ANY($1)",
        )
        .bind(ids)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("ack ax_outbox failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit ack ax_outbox tx failed: {err}")))?;
        Ok(())
    }
}
