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

    /// Executes Step 00 & Step 12 `RETENTION` cleanup for a tenant, deleting expired terminal
    /// `ax_jobs`, processed `ax_outbox`, and expired `EXPORT_FILE` artifacts according to
    /// `ax_retention_policies` (`sec_audit_logs` is append-only for `axpoint_app` and cleaned by
    /// owner cron in Step 14).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if reading policies or updating/deleting expired rows fails.
    pub async fn run_retention_cleanup(
        &self,
        tenant_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let env_root = std::env::var("AXPOINT_DATA_ROOT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map_or_else(
                || std::path::PathBuf::from("/var/lib/axpoint"),
                std::path::PathBuf::from,
            );
        self.run_retention_cleanup_with_data_root(tenant_id, &env_root)
            .await
    }

    /// Executes full Step 14 `RETENTION` cleanup using an explicit `data_root` directory for safe
    /// `EXPORT_FILE` artifact removal (`10-STEP11-12` §12.5.1, `12-STEP14` §14.4.1).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if reading policies or updating/deleting expired rows fails.
    pub async fn run_retention_cleanup_with_data_root(
        &self,
        tenant_id: &str,
        data_root: &std::path::Path,
    ) -> Result<serde_json::Value, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let policy_rows = sqlx::query(
            "SELECT target, keep_days
               FROM ax_retention_policies
              WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query ax_retention_policies failed: {err}")))?;

        let mut counters = RetentionCleanupCounters::default();
        for row in policy_rows {
            let target: String = row.get("target");
            let keep_days: i32 = row.get("keep_days");
            apply_retention_policy_tx(
                &mut tx,
                tenant_id,
                target.as_str(),
                keep_days.max(1),
                &mut counters,
            )
            .await?;
        }

        counters.sessions_deleted = delete_expired_sessions_tx(&mut tx, tenant_id).await?;
        counters.grants_expired = expire_overdue_grants_tx(&mut tx, tenant_id).await?;
        let export_keep_days = counters.export_keep_days.unwrap_or(30);
        let expired_exports =
            fetch_expired_exports_tx(&mut tx, tenant_id, export_keep_days).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit run_retention_cleanup tx failed: {err}"))
        })?;

        if let Err(err) = sqlx::query("DELETE FROM sec_login_states WHERE expires_at <= NOW()")
            .execute(self.pool())
            .await
        {
            tracing::warn!(error = %err, "delete expired sec_login_states failed");
        }

        let export_files_expired = self
            .expire_export_files_after_commit(tenant_id, data_root, &expired_exports)
            .await?;
        let lake_keep_days = counters.lake_snapshot_keep_days.unwrap_or(30);

        Ok(serde_json::json!({
            "jobs_deleted": counters.jobs_deleted,
            "outbox_deleted": counters.outbox_deleted,
            "webhook_buffer_deleted": counters.webhook_buffer_deleted,
            "messages_deleted": counters.messages_deleted,
            "conversations_deleted": counters.conversations_deleted,
            "flow_step_logs_deleted": counters.flow_step_logs_deleted,
            "query_history_deleted": counters.query_history_deleted,
            "ask_turns_deleted": counters.ask_turns_deleted,
            "minute_rollups_deleted": counters.minute_rollups_deleted,
            "metric_series_deleted": counters.metric_series_deleted,
            "export_files_expired": export_files_expired,
            "grants_expired": counters.grants_expired,
            "sessions_deleted": counters.sessions_deleted,
            "lake_snapshot_keep_days": lake_keep_days,
        }))
    }

    async fn expire_export_files_after_commit(
        &self,
        tenant_id: &str,
        data_root: &std::path::Path,
        expired_exports: &[(Uuid, Option<String>)],
    ) -> Result<u64, AppError> {
        if expired_exports.is_empty() {
            return Ok(0);
        }

        for (_, maybe_path) in expired_exports {
            if let Some(raw_path) = maybe_path.as_deref().filter(|s| !s.trim().is_empty()) {
                remove_export_file_safely(data_root, raw_path);
            }
        }

        let ids: Vec<Uuid> = expired_exports.iter().map(|(id, _)| *id).collect();
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE xv_export_jobs
                SET status = 'EXPIRED',
                    file_path = NULL
              WHERE tenant_id = $1
                AND id = ANY($2)
                AND status = 'SUCCEEDED'",
        )
        .bind(tenant_id)
        .bind(&ids)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark xv_export_jobs EXPIRED failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit expire xv_export_jobs tx failed: {err}"))
        })?;
        Ok(res.rows_affected())
    }
}

async fn expire_overdue_grants_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
) -> Result<u64, AppError> {
    let res = sqlx::query(
        "UPDATE sec_grants
            SET status = 'EXPIRED'
          WHERE tenant_id = $1
            AND status = 'ACTIVE'
            AND expires_at <= NOW()",
    )
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("expire overdue sec_grants failed: {err}")))?;
    let affected = res.rows_affected();
    if affected > 0 {
        sqlx::query("SELECT pg_notify('ax_policy_changed', $1)")
            .bind(tenant_id)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("notify ax_policy_changed failed: {err}")))?;
    }
    Ok(affected)
}

async fn fetch_expired_exports_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    keep_days: i32,
) -> Result<Vec<(Uuid, Option<String>)>, AppError> {
    let rows = sqlx::query(
        "SELECT id, file_path
           FROM xv_export_jobs
          WHERE tenant_id = $1
            AND status = 'SUCCEEDED'
            AND (expires_at <= NOW() OR created_at < NOW() - make_interval(days => $2::int))",
    )
    .bind(tenant_id)
    .bind(keep_days.max(1))
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query expired xv_export_jobs failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| (r.get("id"), r.get("file_path")))
        .collect())
}

fn remove_export_file_safely(data_root: &std::path::Path, raw_path: &str) {
    let candidate = std::path::Path::new(raw_path);
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        tracing::warn!(path = %raw_path, "refusing retention delete with parent-dir traversal");
        return;
    }

    let resolved = if candidate.is_absolute() {
        let canon_root =
            std::fs::canonicalize(data_root).unwrap_or_else(|_| data_root.to_path_buf());
        let canon_cand =
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        if !canon_cand.starts_with(&canon_root) && !candidate.starts_with(data_root) {
            tracing::warn!(
                path = %raw_path,
                data_root = %data_root.display(),
                "refusing retention delete outside AXPOINT_DATA_ROOT"
            );
            return;
        }
        candidate.to_path_buf()
    } else {
        data_root.join(candidate)
    };

    if let Err(err) = std::fs::remove_file(&resolved)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %resolved.display(),
            error = %err,
            "failed to delete expired export file during retention"
        );
    }
}

#[derive(Default)]
struct RetentionCleanupCounters {
    jobs_deleted: u64,
    outbox_deleted: u64,
    webhook_buffer_deleted: u64,
    messages_deleted: u64,
    conversations_deleted: u64,
    flow_step_logs_deleted: u64,
    query_history_deleted: u64,
    ask_turns_deleted: u64,
    minute_rollups_deleted: u64,
    metric_series_deleted: u64,
    sessions_deleted: u64,
    grants_expired: u64,
    export_keep_days: Option<i32>,
    lake_snapshot_keep_days: Option<i32>,
}

async fn exec_retention_sql_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    label: &str,
    sql: &'static str,
    tenant_id: &str,
    keep_days: i32,
) -> Result<u64, AppError> {
    let res = sqlx::query(sql)
        .bind(tenant_id)
        .bind(keep_days)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("retention delete {label} failed: {err}")))?;
    Ok(res.rows_affected())
}

async fn apply_retention_policy_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    target: &str,
    keep_days: i32,
    counters: &mut RetentionCleanupCounters,
) -> Result<(), AppError> {
    match target {
        "EXPORT_FILE" => counters.export_keep_days = Some(keep_days),
        "LAKE_SNAPSHOT" => counters.lake_snapshot_keep_days = Some(keep_days),
        "AUDIT" => {}
        "JOB" => {
            counters.jobs_deleted = exec_retention_sql_tx(
                tx,
                "JOB",
                "DELETE FROM ax_jobs
                  WHERE tenant_id = $1
                    AND status IN ('SUCCEEDED', 'FAILED', 'CANCELED', 'EXPIRED')
                    AND updated_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "OUTBOX" => {
            counters.outbox_deleted = exec_retention_sql_tx(
                tx,
                "OUTBOX",
                "DELETE FROM ax_outbox
                  WHERE tenant_id = $1
                    AND processed_at IS NOT NULL
                    AND processed_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "FLOW_STEP_LOG" => {
            counters.flow_step_logs_deleted = exec_retention_sql_tx(
                tx,
                "FLOW_STEP_LOG",
                "DELETE FROM ax_flow_step_logs
                  USING ax_flow_runs r
                  WHERE ax_flow_step_logs.tenant_id = $1
                    AND ax_flow_step_logs.run_id = r.id
                    AND r.status IN ('SUCCEEDED', 'FAILED', 'CANCELED')
                    AND COALESCE(
                            ax_flow_step_logs.finished_at,
                            ax_flow_step_logs.started_at,
                            r.created_at
                        ) < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        other => {
            apply_domain_retention_target_tx(tx, tenant_id, other, keep_days, counters).await?;
        }
    }
    Ok(())
}

async fn apply_domain_retention_target_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    target: &str,
    keep_days: i32,
    counters: &mut RetentionCleanupCounters,
) -> Result<(), AppError> {
    match target {
        "MESSAGE" => {
            counters.webhook_buffer_deleted = exec_retention_sql_tx(
                tx,
                "ds_webhook_buffer",
                "DELETE FROM ds_webhook_buffer
                  WHERE tenant_id = $1
                    AND flushed_at IS NOT NULL
                    AND flushed_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
            counters.messages_deleted = exec_retention_sql_tx(
                tx,
                "ax_messages",
                "DELETE FROM ax_messages
                  WHERE tenant_id = $1
                    AND created_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "CONVERSATION" => {
            counters.conversations_deleted = exec_retention_sql_tx(
                tx,
                "CONVERSATION",
                "DELETE FROM ax_conversations
                  WHERE tenant_id = $1
                    AND updated_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "QUERY_HISTORY" => {
            counters.query_history_deleted = exec_retention_sql_tx(
                tx,
                "an_query_history",
                "DELETE FROM an_query_history
                  WHERE tenant_id = $1
                    AND created_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
            counters.ask_turns_deleted = exec_retention_sql_tx(
                tx,
                "an_ask_turns",
                "DELETE FROM an_ask_turns
                  WHERE tenant_id = $1
                    AND created_at < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "METRIC_ROLLUP_MINUTE" => {
            counters.minute_rollups_deleted = exec_retention_sql_tx(
                tx,
                "METRIC_ROLLUP_MINUTE",
                "DELETE FROM xb_metric_rollups
                  WHERE tenant_id = $1
                    AND grain = 'MINUTE'
                    AND bucket_start < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "METRIC_SERIES" => {
            counters.metric_series_deleted = exec_retention_sql_tx(
                tx,
                "METRIC_SERIES",
                "DELETE FROM xb_metric_series
                  WHERE tenant_id = $1
                    AND ts < NOW() - make_interval(days => $2::int)",
                tenant_id,
                keep_days,
            )
            .await?;
        }
        "IM_EVENT_DEDUP" => {
            purge_im_dedup_and_challenges_tx(tx, tenant_id, keep_days).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn purge_im_dedup_and_challenges_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    keep_days: i32,
) -> Result<(), AppError> {
    let _dedup = exec_retention_sql_tx(
        tx,
        "IM_EVENT_DEDUP",
        "DELETE FROM ax_im_event_dedup
          WHERE tenant_id = $1
            AND received_at < NOW() - make_interval(days => $2::int)",
        tenant_id,
        keep_days,
    )
    .await?;
    let _challenges = exec_retention_sql_tx(
        tx,
        "IM_BIND_CHALLENGES",
        "DELETE FROM ax_im_bind_challenges
          WHERE tenant_id = $1
            AND expires_at < NOW() - make_interval(days => $2::int)",
        tenant_id,
        keep_days,
    )
    .await?;
    Ok(())
}

async fn delete_expired_sessions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
) -> Result<u64, AppError> {
    let res = sqlx::query(
        "DELETE FROM sec_sessions
          WHERE tenant_id = $1
            AND expires_at <= NOW()",
    )
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete expired sec_sessions failed: {err}")))?;
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

    async fn retry_job(
        &self,
        tenant_id: &str,
        job_id: Uuid,
    ) -> Result<Option<JobRecord>, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM ax_jobs WHERE id = $1 FOR UPDATE")
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock ax_jobs for retry failed: {err}"))
                })?;

        let Some(status_raw) = maybe_status else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit missing retry_job tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let current_status = JobStatus::from_str(&status_raw)?;
        if !matches!(current_status, JobStatus::Failed | JobStatus::Canceled) {
            return Err(AppError::Conflict("CONFLICT"));
        }

        let row = sqlx::query(
            "UPDATE ax_jobs
                SET status = 'PENDING',
                    attempt = 0,
                    next_run_at = NOW(),
                    last_error = NULL,
                    lease_owner = NULL,
                    lease_until = NULL,
                    updated_at = NOW()
              WHERE id = $1
             RETURNING id, kind, status, attempt, max_attempts, next_run_at,
                       last_error, result, created_at, updated_at",
        )
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            let is_unique = err
                .as_database_error()
                .and_then(sqlx::error::DatabaseError::code)
                .as_deref()
                == Some("23505");
            if is_unique {
                AppError::Conflict("CONFLICT")
            } else {
                AppError::Database(format!("retry ax_jobs update failed: {err}"))
            }
        })?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit retry_job tx failed: {err}")))?;

        Ok(Some(decode_job_record(&row)?))
    }

    async fn cancel_job(
        &self,
        tenant_id: &str,
        job_id: Uuid,
    ) -> Result<Option<JobRecord>, AppError> {
        let mut tx = crate::tenant::begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM ax_jobs WHERE id = $1 FOR UPDATE")
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock ax_jobs for cancel failed: {err}"))
                })?;

        let Some(status_raw) = maybe_status else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit missing cancel_job tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let current_status = JobStatus::from_str(&status_raw)?;
        if !matches!(current_status, JobStatus::Pending | JobStatus::Running) {
            return Err(AppError::Conflict("CONFLICT"));
        }

        let row = sqlx::query(
            "UPDATE ax_jobs
                SET status = 'CANCELED',
                    lease_owner = NULL,
                    lease_until = NULL,
                    updated_at = NOW()
              WHERE id = $1
             RETURNING id, kind, status, attempt, max_attempts, next_run_at,
                       last_error, result, created_at, updated_at",
        )
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("cancel ax_jobs update failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit cancel_job tx failed: {err}")))?;

        Ok(Some(decode_job_record(&row)?))
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
