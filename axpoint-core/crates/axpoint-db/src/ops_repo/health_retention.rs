//! Deep health telemetry and `ax_retention_policies` persistence for [`PgStore`] (`§14.4`).

use std::str::FromStr;
use std::time::Instant;

use axpoint_domain::AppError;
use axpoint_domain::enums::RetentionTarget;
use axpoint_domain::ops::{
    DatabaseHealth, JobsHealthStats, MigrationsHealth, OutboxStats, RetentionPolicyRecord,
};
use sqlx::Row;

use super::is_sqlstate;
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

pub(super) async fn probe_database_health_impl(
    store: &PgStore,
) -> Result<DatabaseHealth, AppError> {
    let started = Instant::now();
    let query_res = sqlx::query(
        "SELECT CASE
                    WHEN pg_is_in_recovery() THEN
                        EXTRACT(EPOCH FROM (NOW() - pg_last_xact_replay_timestamp()))::float8
                    ELSE 0.0::float8
                END AS lag_s",
    )
    .fetch_one(store.pool())
    .await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    match query_res {
        Ok(row) => {
            let replication_lag_s: Option<f64> = row.try_get("lag_s").unwrap_or(Some(0.0));
            Ok(DatabaseHealth {
                ok: true,
                latency_ms,
                replication_lag_s,
                error: None,
            })
        }
        Err(err) => Ok(DatabaseHealth {
            ok: false,
            latency_ms,
            replication_lag_s: None,
            error: Some(err.to_string()),
        }),
    }
}

pub(super) async fn query_jobs_health_stats_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<JobsHealthStats, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "SELECT COUNT(*) FILTER (WHERE status = 'PENDING') AS pending,
                COUNT(*) FILTER (WHERE status = 'RUNNING') AS running,
                COUNT(*) FILTER (
                    WHERE status = 'FAILED'
                      AND updated_at >= NOW() - INTERVAL '24 hours'
                ) AS failed_24h,
                COALESCE(
                    EXTRACT(
                        EPOCH FROM (NOW() - (MIN(created_at) FILTER (WHERE status = 'PENDING')))
                    )::bigint,
                    0
                ) AS oldest_pending_s
           FROM ax_jobs
          WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("query ax_jobs health stats failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit query_jobs_health_stats tx failed: {err}"))
    })?;

    let oldest_raw: i64 = row.get("oldest_pending_s");
    Ok(JobsHealthStats {
        pending: row.get("pending"),
        running: row.get("running"),
        failed_24h: row.get("failed_24h"),
        oldest_pending_s: oldest_raw.max(0),
    })
}

pub(super) async fn query_outbox_stats_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<OutboxStats, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "SELECT COUNT(*) AS pending,
                COALESCE(EXTRACT(EPOCH FROM (NOW() - MIN(created_at)))::bigint, 0) AS lag_s
           FROM ax_outbox
          WHERE tenant_id = $1
            AND processed_at IS NULL",
    )
    .bind(tenant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("query ax_outbox stats failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit query_outbox_stats tx failed: {err}")))?;

    let lag_raw: i64 = row.get("lag_s");
    Ok(OutboxStats {
        lag_s: lag_raw.max(0),
        pending: row.get("pending"),
    })
}

pub(super) async fn query_migrations_health_impl(
    store: &PgStore,
    expected_total: usize,
) -> Result<MigrationsHealth, AppError> {
    let query_res = sqlx::query(
        "SELECT COALESCE(MAX(version), 0)::bigint AS max_ver,
                COUNT(*) FILTER (WHERE success) AS applied_cnt
           FROM _sqlx_migrations",
    )
    .fetch_one(store.pool())
    .await;

    match query_res {
        Ok(row) => {
            let current: i64 = row.get("max_ver");
            let applied_cnt: i64 = row.get("applied_cnt");
            let applied_usize = usize::try_from(applied_cnt.max(0)).unwrap_or(usize::MAX);
            let pending = expected_total.saturating_sub(applied_usize);
            Ok(MigrationsHealth {
                ok: pending == 0,
                current,
                pending,
            })
        }
        Err(err) if is_sqlstate(&err, "42P01") => Ok(MigrationsHealth {
            ok: expected_total == 0,
            current: 0,
            pending: expected_total,
        }),
        Err(err) if is_sqlstate(&err, "42501") => {
            fallback_migrations_health_from_catalog(store, expected_total).await
        }
        Err(err) => Err(AppError::Database(format!(
            "query _sqlx_migrations health failed: {err}"
        ))),
    }
}

async fn fallback_migrations_health_from_catalog(
    store: &PgStore,
    expected_total: usize,
) -> Result<MigrationsHealth, AppError> {
    let sentinel_tables: [&str; 14] = [
        "public.hg_docs",
        "public.ax_jobs",
        "public.ds_datasets",
        "public.xb_metric_series",
        "public.an_metrics",
        "public.xb_subgraphs",
        "public.ax_flow_runs",
        "public.ax_alerts",
        "public.ax_agents",
        "public.ax_sim_scenarios",
        "public.xv_dashboards",
        "public.xv_export_jobs",
        "public.sec_audit_chain_heads",
        "public.ax_work_items",
    ];
    let row = sqlx::query(
        "SELECT COUNT(*) FILTER (WHERE to_regclass(t) IS NOT NULL) AS present_cnt
           FROM unnest($1::text[]) AS t",
    )
    .bind(&sentinel_tables[..])
    .fetch_one(store.pool())
    .await
    .map_err(|err| AppError::Database(format!("fallback catalog migration probe failed: {err}")))?;

    let present_cnt: i64 = row.get("present_cnt");
    let present_usize = usize::try_from(present_cnt.max(0)).unwrap_or(0);
    let pending = expected_total.saturating_sub(present_usize);
    Ok(MigrationsHealth {
        ok: pending == 0,
        current: present_cnt.max(0),
        pending,
    })
}

pub(super) async fn list_retention_policies_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<RetentionPolicyRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT target, keep_days
           FROM ax_retention_policies
          WHERE tenant_id = $1
          ORDER BY target ASC",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_retention_policies failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit list_retention_policies tx failed: {err}"))
    })?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let raw_target: String = row.get("target");
        out.push(RetentionPolicyRecord {
            target: RetentionTarget::from_str(&raw_target)?,
            keep_days: row.get("keep_days"),
        });
    }
    Ok(out)
}

pub(super) async fn upsert_retention_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    target: RetentionTarget,
    keep_days: i32,
) -> Result<RetentionPolicyRecord, AppError> {
    if keep_days <= 0 {
        return Err(AppError::bad("INVALID_ARGUMENT", "keep_days must be > 0"));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO ax_retention_policies (tenant_id, target, keep_days)
         VALUES ($1, $2, $3)
         ON CONFLICT (tenant_id, target)
         DO UPDATE SET keep_days = EXCLUDED.keep_days
         RETURNING keep_days",
    )
    .bind(tenant_id)
    .bind(target.as_str())
    .bind(keep_days)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert ax_retention_policies failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit upsert_retention_policy tx failed: {err}"))
    })?;

    Ok(RetentionPolicyRecord {
        target,
        keep_days: row.get("keep_days"),
    })
}

pub(super) async fn drop_expired_metric_partitions_impl(store: &PgStore) -> Result<i64, AppError> {
    let row = sqlx::query("SELECT ax_drop_expired_metric_partitions() AS dropped")
        .fetch_one(store.pool())
        .await
        .map_err(|err| {
            AppError::Database(format!("ax_drop_expired_metric_partitions failed: {err}"))
        })?;
    let dropped: i32 = row.get("dropped");
    Ok(i64::from(dropped))
}
