//! Singleton advisory-locked cron dataset scheduler and daily `LAKE_MAINTENANCE` dispatcher (Step 02 §02.5).

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::JobKind;
use axpoint_domain::platform::NewJob;
use axpoint_domain::ports::{DatasetRepository as _, JobQueuePort as _};
use chrono::{FixedOffset, Utc};
use sqlx::Row as _;
use tokio::time::MissedTickBehavior;

use crate::routes::to_six_field_cron;

const CRON_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Advisory lock key for the singleton `AxHub` cron scheduler (`"AXHB_CRN"`).
const SCHEDULER_ADVISORY_LOCK_KEY: i64 = 0x4158_4842_5f43_524e;

/// Runs the periodic cron dataset sync & daily lake maintenance scheduler.
pub(crate) async fn run_cron_scheduler(
    store: PgStore,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(CRON_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    while !*shutdown_rx.borrow_and_update() {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => break,
        }
        if let Err(err) = try_tick_cron(&store).await {
            tracing::warn!(error = %err, "cron scheduler tick failed");
        }
    }
}

async fn resolve_tz_offset_secs(store: &PgStore, tz: &str) -> i32 {
    let secs: Option<i64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM ((NOW() AT TIME ZONE $1) - (NOW() AT TIME ZONE 'UTC')))::bigint",
    )
    .bind(tz)
    .fetch_optional(store.pool())
    .await
    .ok()
    .flatten();

    secs.and_then(|s| i32::try_from(s).ok()).unwrap_or(8 * 3600)
}

async fn try_tick_cron(store: &PgStore) -> Result<(), AppError> {
    let mut conn = store
        .pool()
        .acquire()
        .await
        .map_err(|err| AppError::Database(format!("scheduler acquire conn failed: {err}")))?;

    let locked: bool = sqlx::query("SELECT pg_try_advisory_lock($1)")
        .bind(SCHEDULER_ADVISORY_LOCK_KEY)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| AppError::Database(format!("scheduler advisory lock failed: {err}")))?
        .get(0);

    if !locked {
        return Ok(());
    }

    let res = evaluate_scheduled_datasets(store).await;
    if let Err(err) = enqueue_daily_lake_maintenance(store).await {
        tracing::warn!(error = %err, "daily lake_maintenance enqueue failed");
    }

    if let Err(err) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SCHEDULER_ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await
    {
        tracing::warn!(error = %err, "cron scheduler pg_advisory_unlock failed");
    }

    res
}

async fn evaluate_scheduled_datasets(store: &PgStore) -> Result<(), AppError> {
    let scheduled = store.list_scheduled_datasets().await?;
    if scheduled.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    let mut tz_cache: HashMap<String, FixedOffset> = HashMap::new();

    for item in scheduled {
        let offset = if let Some(&cached) = tz_cache.get(&item.tenant_id) {
            cached
        } else {
            let tz_name = store
                .get_tenant_timezone(&item.tenant_id)
                .await
                .unwrap_or_else(|_| String::from("Asia/Shanghai"));
            let secs = resolve_tz_offset_secs(store, &tz_name).await;
            let fixed = FixedOffset::east_opt(secs)
                .or_else(|| FixedOffset::east_opt(8 * 3600))
                .unwrap_or_else(|| {
                    FixedOffset::east_opt(0)
                        .unwrap_or(FixedOffset::west_opt(0).unwrap_or_else(unreachable_utc_offset))
                });
            tz_cache.insert(item.tenant_id.clone(), fixed);
            fixed
        };

        let Ok(six_field) = to_six_field_cron(&item.schedule_cron) else {
            continue;
        };
        let Ok(schedule) = cron::Schedule::from_str(&six_field) else {
            continue;
        };

        let base_utc = item
            .last_synced_at
            .unwrap_or_else(|| now - chrono::Duration::minutes(1));
        let base_local = base_utc.with_timezone(&offset);

        if let Some(next_local) = schedule.after(&base_local).next()
            && next_local.with_timezone(&Utc) <= now
            && let Err(err) = store
                .enqueue_sync_job(&item.tenant_id, item.dataset_id)
                .await
        {
            tracing::warn!(
                tenant_id = %item.tenant_id,
                dataset_id = %item.dataset_id,
                error = %err,
                "scheduled dataset sync enqueue failed"
            );
        }
    }

    Ok(())
}

fn unreachable_utc_offset() -> FixedOffset {
    // FixedOffset::east_opt(0) is mathematically guaranteed to return Some;
    // chrono::Offset::fix on Utc provides a total, panic-free fallback.
    use chrono::Offset as _;
    Utc.fix()
}

async fn enqueue_daily_lake_maintenance(store: &PgStore) -> Result<(), AppError> {
    let tenants = store.list_active_tenants().await?;
    let day_key = Utc::now().format("%Y-%m-%d").to_string();
    for tenant_id in tenants {
        let job = NewJob::new(
            JobKind::LakeMaintenance,
            serde_json::json!({ "day": day_key }),
        )
        .with_idempotency_key(format!("lake_maint:{day_key}"));
        if let Err(err) = store.enqueue(&tenant_id, job).await {
            tracing::warn!(tenant_id = %tenant_id, error = %err, "lake_maintenance enqueue failed");
        }
    }
    Ok(())
}

/// Sub-second continuous micro-batch stream coordinator claiming `continuous_stream = true` datasets
/// under a `30s` single-leader lease (`07-STEP29` §29.1 & §29.2).
pub(crate) async fn run_continuous_stream_coordinator(
    holder_id: String,
    store: PgStore,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(750));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    while !*shutdown_rx.borrow_and_update() {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => break,
        }
        match store.claim_stream_datasets(&holder_id, 32, 30).await {
            Ok(leased) => {
                for item in leased {
                    if let Err(err) = store
                        .enqueue_sync_job(&item.tenant_id, item.dataset_id)
                        .await
                    {
                        tracing::warn!(
                            tenant_id = %item.tenant_id,
                            dataset_id = %item.dataset_id,
                            error = %err,
                            "continuous stream micro-batch sync enqueue failed"
                        );
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "claim_stream_datasets failed");
            }
        }
    }

    if let Err(err) = store.release_stream_datasets(&holder_id).await {
        tracing::warn!(holder_id = %holder_id, error = %err, "release_stream_datasets failed");
    }
}
