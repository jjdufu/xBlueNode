//! Append-only tamper-evident SHA-256 chained security audit sink (`sec_audit_logs`) for [`PgStore`] (`11-STEP13` §13.9.2).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::AuditSink;
use axpoint_security::audit_chain::{AuditHashFields, compute_audit_row_hash, truncate_to_micros};
use sqlx::{Postgres, Row as _, Transaction};

use crate::PgStore;

/// Appends a single [`AuditRecord`] into `sec_audit_logs` within an active tenant-scoped transaction,
/// serializing per-tenant ordering via `sec_audit_chain_heads ... FOR UPDATE` and computing
/// `prev_hash` / `row_hash` (`11-STEP13` §13.9.2).
///
/// # Errors
/// Returns [`AppError::Database`] if locking `sec_audit_chain_heads`, allocating a sequence ID,
/// inserting into `sec_audit_logs`, or advancing the chain head fails.
pub(crate) async fn insert_chained_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    record: &AuditRecord,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO sec_audit_chain_heads (tenant_id, last_id, last_hash)
         VALUES ($1, 0, '')
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(&record.tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("init sec_audit_chain_heads failed: {err}")))?;

    let head_row = sqlx::query(
        "SELECT last_id, last_hash
           FROM sec_audit_chain_heads
          WHERE tenant_id = $1
            FOR UPDATE",
    )
    .bind(&record.tenant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock sec_audit_chain_heads failed: {err}")))?;

    let prev_hash: String = head_row
        .try_get("last_hash")
        .map_err(|err| AppError::Database(format!("decode last_hash failed: {err}")))?;

    let seq_row =
        sqlx::query("SELECT nextval(pg_get_serial_sequence('sec_audit_logs', 'id')) AS id")
            .fetch_one(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("allocate sec_audit_logs.id failed: {err}"))
            })?;
    let id: i64 = seq_row
        .try_get("id")
        .map_err(|err| AppError::Database(format!("decode allocated audit id failed: {err}")))?;

    let created_at = truncate_to_micros(record.created_at);
    let row_hash = compute_audit_row_hash(
        &prev_hash,
        &AuditHashFields {
            id,
            tenant_id: &record.tenant_id,
            user_id: record.user_id,
            username: &record.username,
            action: record.action.as_str(),
            resource_ref: &record.resource_ref,
            status_code: record.status_code,
            client_ip: &record.client_ip,
            trace_id: &record.trace_id,
            detail_json: &record.detail_json,
            created_at,
        },
    );

    sqlx::query(
        "INSERT INTO sec_audit_logs (
             id, tenant_id, user_id, username, action, resource_ref,
             status_code, client_ip, trace_id, detail_json, prev_hash, row_hash, created_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(&record.tenant_id)
    .bind(record.user_id)
    .bind(&record.username)
    .bind(record.action.as_str())
    .bind(&record.resource_ref)
    .bind(record.status_code)
    .bind(&record.client_ip)
    .bind(&record.trace_id)
    .bind(&record.detail_json)
    .bind(&prev_hash)
    .bind(&row_hash)
    .bind(created_at)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert chained sec_audit_logs failed: {err}")))?;

    sqlx::query(
        "UPDATE sec_audit_chain_heads
            SET last_id = $2,
                last_hash = $3
          WHERE tenant_id = $1",
    )
    .bind(&record.tenant_id)
    .bind(id)
    .bind(&row_hash)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("update sec_audit_chain_heads failed: {err}")))?;

    Ok(())
}

impl AuditSink for PgStore {
    async fn write_batch(&self, records: Vec<AuditRecord>) -> Result<(), AppError> {
        if records.is_empty() {
            return Ok(());
        }

        let mut by_tenant: BTreeMap<String, Vec<AuditRecord>> = BTreeMap::new();
        for rec in records {
            by_tenant
                .entry(rec.tenant_id.clone())
                .or_default()
                .push(rec);
        }

        for (tenant_id, group) in by_tenant {
            let mut tx = crate::tenant::begin_tenant_tx(self.pool(), &tenant_id).await?;
            for rec in &group {
                insert_chained_audit_tx(&mut tx, rec).await?;
            }
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit sec_audit_logs tx failed: {err}"))
            })?;
        }

        Ok(())
    }
}
