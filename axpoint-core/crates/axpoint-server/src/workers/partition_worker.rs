//! Daily advisory-locked `xb_metric_series` partition maintenance worker (Step 03 §03.3.1).

use std::time::Duration;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use sqlx::Row as _;
use tokio::time::MissedTickBehavior;

const PARTITION_MAINTENANCE_INTERVAL: Duration = Duration::from_hours(24);

/// Advisory lock key for monthly metric partition lifecycle (`"AXGR_PRT"`).
const PARTITION_ADVISORY_LOCK_KEY: i64 = 0x4158_4752_5f50_5254;

/// Ensures future monthly partitions (`+3` months) and drops expired partitions once per day.
pub(crate) async fn run_partition_worker(store: PgStore) {
    let mut ticker = tokio::time::interval(PARTITION_MAINTENANCE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        if let Err(err) = try_maintain_partitions(&store).await {
            tracing::warn!(error = %err, "metric partition maintenance tick failed");
        }
    }
}

async fn try_maintain_partitions(store: &PgStore) -> Result<(), AppError> {
    let mut conn = store
        .pool()
        .acquire()
        .await
        .map_err(|err| AppError::Database(format!("partition worker acquire failed: {err}")))?;

    let locked: bool = sqlx::query("SELECT pg_try_advisory_lock($1)")
        .bind(PARTITION_ADVISORY_LOCK_KEY)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| AppError::Database(format!("partition advisory lock failed: {err}")))?
        .get(0);

    if !locked {
        return Ok(());
    }

    let ensure_res = store.ensure_metric_partitions(24, 3).await;
    let drop_res = store.drop_expired_metric_partitions().await;

    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(PARTITION_ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await;

    ensure_res?;
    drop_res?;
    Ok(())
}
