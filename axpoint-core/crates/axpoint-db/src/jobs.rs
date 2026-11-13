//! Persistent background job queue (`ax_jobs`) implementation for [`PgStore`].

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::enums::{JobKind, JobStatus};
use axpoint_domain::platform::{JobListQuery, JobListResponse, JobRecord, LeasedJob, NewJob};
use axpoint_domain::ports::JobQueuePort;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::PgStore;

/// Enqueues a [`NewJob`] inside an existing tenant-scoped PostgreSQL transaction.
///
/// If `(tenant_id, kind, idempotency_key)` conflicts with an existing row, returns the existing job ID.
///
/// # Errors
/// Returns [`AppError::Database`] if executing the `INSERT` statement fails.
pub async fn enqueue_job_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    job: &NewJob,
) -> Result<Uuid, AppError> {
    let idempotency_key = job.idempotency_key.as_deref().filter(|k| !k.is_empty());
    let row = sqlx::query(
        "INSERT INTO ax_jobs (
             tenant_id, kind, payload, priority, max_attempts, next_run_at, idempotency_key
         )
         VALUES ($1, $2, $3, $4, $5, COALESCE($6, NOW()), $7)
         ON CONFLICT (tenant_id, kind, idempotency_key)
         DO UPDATE SET updated_at = ax_jobs.updated_at
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(job.kind.as_str())
    .bind(&job.payload)
    .bind(job.priority)
    .bind(job.max_attempts)
    .bind(job.next_run_at)
    .bind(idempotency_key)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("enqueue ax_job failed: {err}")))?;

    Ok(row.get("id"))
}

impl PgStore {
    /// Lists active tenant identifiers from `sec_tenants` (used by background retention schedulers).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `sec_tenants` fails.
    pub async fn list_active_tenants(&self) -> Result<Vec<String>, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), "default").await?;
        let rows = sqlx::query(
            "SELECT tenant_id FROM sec_tenants WHERE status = 'ACTIVE' ORDER BY tenant_id",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list active tenants failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_active_tenants tx failed: {err}"))
        })?;
        Ok(rows.into_iter().map(|r| r.get("tenant_id")).collect())
    }

    /// Executes Step 00 `RETENTION` cleanup for a tenant, deleting expired terminal `ax_jobs`
    /// and processed `ax_outbox` rows according to `ax_retention_policies` (`sec_audit_logs` is
    /// append-only for `axpoint_app` and cleaned by owner cron in Step 14).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if reading policies or deleting expired rows fails.
    pub async fn run_retention_cleanup(
        &self,
        tenant_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let policy_rows = sqlx::query(
            "SELECT target, keep_days
               FROM ax_retention_policies
              WHERE tenant_id = $1
                AND target IN ('JOB', 'OUTBOX', 'MESSAGE', 'METRIC_ROLLUP_MINUTE', 'METRIC_SERIES')",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query ax_retention_policies failed: {err}")))?;

        let mut jobs_deleted: u64 = 0;
        let mut outbox_deleted: u64 = 0;
        let mut webhook_buffer_deleted: u64 = 0;
        let mut minute_rollups_deleted: u64 = 0;
        let mut metric_series_deleted: u64 = 0;

        for row in policy_rows {
            let target: String = row.get("target");
            let keep_days: i32 = row.get("keep_days");
            let affected =
                delete_expired_target_tx(&mut tx, tenant_id, target.as_str(), keep_days).await?;
            match target.as_str() {
                "JOB" => jobs_deleted = affected,
                "OUTBOX" => outbox_deleted = affected,
                "MESSAGE" => webhook_buffer_deleted = affected,
                "METRIC_ROLLUP_MINUTE" => minute_rollups_deleted = affected,
                "METRIC_SERIES" => metric_series_deleted = affected,
                _ => {}
            }
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit run_retention_cleanup tx failed: {err}"))
        })?;

        Ok(serde_json::json!({
            "jobs_deleted": jobs_deleted,
            "outbox_deleted": outbox_deleted,
            "webhook_buffer_deleted": webhook_buffer_deleted,
            "minute_rollups_deleted": minute_rollups_deleted,
            "metric_series_deleted": metric_series_deleted,
        }))
    }
}

async fn delete_expired_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    target: &str,
    keep_days: i32,
) -> Result<u64, AppError> {
    let sql: &'static str = match target {
        "JOB" => {
            "DELETE FROM ax_jobs
              WHERE tenant_id = $1
                AND status IN ('SUCCEEDED', 'FAILED', 'CANCELED', 'EXPIRED')
                AND updated_at < NOW() - make_interval(days => $2::int)"
        }
        "OUTBOX" => {
            "DELETE FROM ax_outbox
              WHERE tenant_id = $1
                AND processed_at IS NOT NULL
                AND processed_at < NOW() - make_interval(days => $2::int)"
        }
        "MESSAGE" => {
            "DELETE FROM ds_webhook_buffer
              WHERE tenant_id = $1
                AND flushed_at IS NOT NULL
                AND flushed_at < NOW() - make_interval(days => $2::int)"
        }
        "METRIC_ROLLUP_MINUTE" => {
            "DELETE FROM xb_metric_rollups
              WHERE tenant_id = $1
                AND grain = 'MINUTE'
                AND bucket_start < NOW() - make_interval(days => $2::int)"
        }
        "METRIC_SERIES" => {
            "DELETE FROM xb_metric_series
              WHERE tenant_id = $1
                AND ts < NOW() - make_interval(days => $2::int)"
        }
        _ => return Ok(0),
    };

    let res = sqlx::query(sql)
        .bind(tenant_id)
        .bind(keep_days)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("retention delete {target} failed: {err}")))?;
    Ok(res.rows_affected())
}

impl JobQueuePort for PgStore {
    async fn enqueue(&self, tenant_id: &str, job: NewJob) -> Result<Uuid, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let job_id = enqueue_job_tx(&mut tx, tenant_id, &job).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit enqueue ax_job tx failed: {err}")))?;
        Ok(job_id)
    }

    async fn lease(
        &self,
        worker: &str,
        kinds: &[JobKind],
        limit: u32,
        lease_secs: u32,
    ) -> Result<Vec<LeasedJob>, AppError> {
        if kinds.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let kind_strs: Vec<&str> = kinds.iter().copied().map(JobKind::as_str).collect();
        let limit_i32 = i32::try_from(limit).unwrap_or(i32::MAX);
        let lease_i32 = i32::try_from(lease_secs).unwrap_or(i32::MAX);

        let rows = sqlx::query("SELECT * FROM ax_lease_jobs($1, $2, $3, $4)")
            .bind(worker)
            .bind(&kind_strs)
            .bind(limit_i32)
            .bind(lease_i32)
            .fetch_all(self.pool())
            .await
            .map_err(|err| AppError::Database(format!("ax_lease_jobs failed: {err}")))?;

        let mut leased = Vec::with_capacity(rows.len());
        for row in rows {
            leased.push(decode_leased_job(&row, worker)?);
        }
        Ok(leased)
    }

    async fn heartbeat(&self, job: &LeasedJob, lease_secs: u32) -> Result<(), AppError> {
        let lease_i32 = i32::try_from(lease_secs).unwrap_or(i32::MAX);
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), &job.tenant_id).await?;
        sqlx::query(
            "UPDATE ax_jobs
                SET lease_until = NOW() + make_interval(secs => $1::int),
                    updated_at = NOW()
              WHERE id = $2
                AND lease_owner = $3
                AND status = 'RUNNING'",
        )
        .bind(lease_i32)
        .bind(job.id)
        .bind(&job.lease_owner)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("heartbeat ax_job failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit heartbeat tx failed: {err}")))?;
        Ok(())
    }

    async fn complete(&self, job: &LeasedJob, result: serde_json::Value) -> Result<(), AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), &job.tenant_id).await?;
        sqlx::query(
            "UPDATE ax_jobs
                SET status = 'SUCCEEDED',
                    result = $1,
                    lease_owner = NULL,
                    lease_until = NULL,
                    updated_at = NOW()
              WHERE id = $2",
        )
        .bind(result)
        .bind(job.id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("complete ax_job failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit complete tx failed: {err}")))?;
        Ok(())
    }

    async fn fail(&self, job: &LeasedJob, error: &str) -> Result<(), AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), &job.tenant_id).await?;
        if job.attempt < job.max_attempts {
            let exp = u32::try_from(job.attempt.max(0)).unwrap_or(0);
            let backoff = 2_i32.saturating_pow(exp).min(300);
            sqlx::query(
                "UPDATE ax_jobs
                    SET status = 'PENDING',
                        next_run_at = NOW() + make_interval(secs => $1::int),
                        last_error = $2,
                        lease_owner = NULL,
                        lease_until = NULL,
                        updated_at = NOW()
                  WHERE id = $3",
            )
            .bind(backoff)
            .bind(error)
            .bind(job.id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("reschedule failed ax_job: {err}")))?;
        } else {
            sqlx::query(
                "UPDATE ax_jobs
                    SET status = 'FAILED',
                        last_error = $1,
                        lease_owner = NULL,
                        lease_until = NULL,
                        updated_at = NOW()
                  WHERE id = $2",
            )
            .bind(error)
            .bind(job.id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("mark ax_job FAILED: {err}")))?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit fail tx failed: {err}")))?;
        Ok(())
    }

    async fn list_jobs(
        &self,
        tenant_id: &str,
        query: JobListQuery,
    ) -> Result<JobListResponse, AppError> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=500, got {limit}"),
            ));
        }

        let limit_usize = usize::try_from(limit).unwrap_or(50);
        let fetch_limit = i64::from(limit).saturating_add(1);
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;

        let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(raw_cursor) => {
                let (ts, cid) = resolve_cursor_tx(&mut tx, raw_cursor.trim()).await?;
                (Some(ts), Some(cid))
            }
            None => (None, None),
        };

        let kind_filter = query.kind.map(JobKind::as_str);
        let status_filter = query.status.map(JobStatus::as_str);

        let rows = sqlx::query(
            "SELECT id, kind, status, attempt, max_attempts, next_run_at,
                    last_error, result, created_at, updated_at
               FROM ax_jobs
              WHERE ($1::text IS NULL OR kind = $1)
                AND ($2::text IS NULL OR status = $2)
                AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $5",
        )
        .bind(kind_filter)
        .bind(status_filter)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_jobs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list ax_jobs tx failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(decode_job_record(&row)?);
        }

        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|last| format!("{}|{}", last.created_at.to_rfc3339(), last.id))
        } else {
            None
        };

        Ok(JobListResponse { items, next_cursor })
    }

    async fn get_job(&self, tenant_id: &str, job_id: Uuid) -> Result<Option<JobRecord>, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, kind, status, attempt, max_attempts, next_run_at,
                    last_error, result, created_at, updated_at
               FROM ax_jobs
              WHERE id = $1",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_job failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get ax_job tx failed: {err}")))?;

        maybe_row.as_ref().map(decode_job_record).transpose()
    }
}

async fn resolve_cursor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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

    let parsed_id = Uuid::parse_str(raw_cursor)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor: {err}")))?;

    let row = sqlx::query("SELECT created_at FROM ax_jobs WHERE id = $1")
        .bind(parsed_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("resolve job cursor failed: {err}")))?
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "cursor job not found"))?;

    let created_at: DateTime<Utc> = row.get("created_at");
    Ok((created_at, parsed_id))
}

fn decode_leased_job(
    row: &sqlx::postgres::PgRow,
    fallback_worker: &str,
) -> Result<LeasedJob, AppError> {
    let kind_raw: String = row.get("kind");
    let status_raw: String = row.get("status");
    let kind = JobKind::from_str(&kind_raw)?;
    let status = JobStatus::from_str(&status_raw)?;
    let lease_owner: Option<String> = row.get("lease_owner");
    let lease_until: Option<DateTime<Utc>> = row.get("lease_until");

    Ok(LeasedJob {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        kind,
        payload: row.get("payload"),
        status,
        priority: row.get("priority"),
        attempt: row.get("attempt"),
        max_attempts: row.get("max_attempts"),
        next_run_at: row.get("next_run_at"),
        lease_owner: lease_owner.unwrap_or_else(|| String::from(fallback_worker)),
        lease_until: lease_until.unwrap_or_else(Utc::now),
        idempotency_key: row.get("idempotency_key"),
        last_error: row.get("last_error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_job_record(row: &sqlx::postgres::PgRow) -> Result<JobRecord, AppError> {
    let kind_raw: String = row.get("kind");
    let status_raw: String = row.get("status");
    let kind = JobKind::from_str(&kind_raw)?;
    let status = JobStatus::from_str(&status_raw)?;

    Ok(JobRecord {
        id: row.get("id"),
        kind,
        status,
        attempt: row.get("attempt"),
        max_attempts: row.get("max_attempts"),
        next_run_at: row.get("next_run_at"),
        last_error: row.get("last_error"),
        result: row.get("result"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
