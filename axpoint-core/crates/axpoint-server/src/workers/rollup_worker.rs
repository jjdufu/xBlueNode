//! `ROLLUP` job handler and 60-second singleton rollup scheduler (Step 03 §03.3.2).

use std::time::Duration;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::JobKind;
use axpoint_domain::platform::{BoxFuture, JobHandler, LeasedJob, NewJob};
use axpoint_domain::ports::{JobQueuePort as _, MetricStorePort as _};
use chrono::Utc;
use sqlx::Row as _;
use tokio::time::MissedTickBehavior;

const ROLLUP_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Advisory lock key for the singleton rollup scheduler (`"AXGR_RLP"`).
const ROLLUP_ADVISORY_LOCK_KEY: i64 = 0x4158_4752_5f52_4c50;

/// `JobKind::Rollup` handler executing `MINUTE -> HOUR -> DAY -> MONTH` rollup cascade for a tenant.
pub(crate) struct RollupJobHandler {
    pub(crate) store: PgStore,
}

impl JobHandler for RollupJobHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let upserted = self.store.run_rollup_for_tenant(&job.tenant_id).await?;
            Ok(serde_json::json!({
                "tenant_id": job.tenant_id,
                "rows_upserted": upserted,
            }))
        })
    }
}

/// Periodically enqueues `ROLLUP` jobs across active tenants every 60 seconds.
pub(crate) async fn run_rollup_scheduler(store: PgStore) {
    let mut ticker = tokio::time::interval(ROLLUP_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Skip immediate tick so startup stays quiet until 60s elapses.
    ticker.tick().await;

    loop {
        ticker.tick().await;
        if let Err(err) = try_enqueue_periodic_rollups(&store).await {
            tracing::warn!(error = %err, "periodic rollup scheduler tick failed");
        }
    }
}

async fn try_enqueue_periodic_rollups(store: &PgStore) -> Result<(), AppError> {
    let mut conn = store
        .pool()
        .acquire()
        .await
        .map_err(|err| AppError::Database(format!("rollup scheduler acquire failed: {err}")))?;

    let locked: bool = sqlx::query("SELECT pg_try_advisory_lock($1)")
        .bind(ROLLUP_ADVISORY_LOCK_KEY)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| AppError::Database(format!("rollup advisory lock failed: {err}")))?
        .get(0);

    if !locked {
        return Ok(());
    }

    let minute_key = Utc::now().format("%Y-%m-%dT%H:%M").to_string();
    let tenants = store.list_rollup_tenants().await?;
    for tenant_id in tenants {
        let job = NewJob::new(
            JobKind::Rollup,
            serde_json::json!({ "trigger": "periodic", "minute": minute_key }),
        )
        .with_idempotency_key(format!("rollup:{minute_key}"));
        let _ = store.enqueue(&tenant_id, job).await;
    }

    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ROLLUP_ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await;

    Ok(())
}
