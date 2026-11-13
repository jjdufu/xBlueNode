//! Append-only security audit sink (`sec_audit_logs`) implementation for [`PgStore`].

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::AuditSink;
use sqlx::{Postgres, QueryBuilder};

use crate::PgStore;

/// Maximum rows per multi-row `INSERT` chunk to stay well below PostgreSQL's bind limit (`65535`).
const MAX_ROWS_PER_INSERT: usize = 1000;

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
            for chunk in group.chunks(MAX_ROWS_PER_INSERT) {
                let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
                    "INSERT INTO sec_audit_logs (
                         tenant_id, user_id, username, action, resource_ref,
                         status_code, client_ip, trace_id, detail_json, created_at
                     ) ",
                );
                qb.push_values(chunk, |mut b, rec| {
                    b.push_bind(&rec.tenant_id)
                        .push_bind(rec.user_id)
                        .push_bind(&rec.username)
                        .push_bind(rec.action.as_str())
                        .push_bind(&rec.resource_ref)
                        .push_bind(rec.status_code)
                        .push_bind(&rec.client_ip)
                        .push_bind(&rec.trace_id)
                        .push_bind(&rec.detail_json)
                        .push_bind(rec.created_at);
                });

                qb.build().execute(&mut *tx).await.map_err(|err| {
                    AppError::Database(format!("insert sec_audit_logs batch failed: {err}"))
                })?;
            }

            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit sec_audit_logs tx failed: {err}"))
            })?;
        }

        Ok(())
    }
}
