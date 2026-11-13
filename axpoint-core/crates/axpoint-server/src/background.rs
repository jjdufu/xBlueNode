//! Background `JobWorker`, `OutboxDispatcher`, and advisory-locked singleton retention scheduler.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::sync::Arc;
use std::time::Duration;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::JobKind;
use axpoint_domain::platform::{
    BoxFuture, JobHandler, LeasedJob, NewJob, OutboxEvent, OutboxSubscriber,
};
use axpoint_domain::ports::{JobQueuePort as _, OutboxPort as _};
use chrono::Utc;
use sqlx::Row as _;
use sqlx::postgres::PgListener;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

/// Default job lease duration in seconds.
const JOB_LEASE_SECS: u32 = 60;

/// Interval between `heartbeat` renewals while a job is running (`20s` < `60s` lease).
const JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

/// Poll interval when no `ax_jobs` work was found on the previous iteration.
const JOB_IDLE_POLL: Duration = Duration::from_millis(250);

/// Extended lease duration in seconds for long-running `SYNC_DATASET` / `PARSE_FILE` jobs (`03-STEP02` §02.5).
const SYNC_JOB_LEASE_SECS: u32 = 3600;

/// Heartbeat interval for long-running sync jobs (`60s` < `3600s` lease).
const SYNC_JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// Outbox fallback polling interval alongside `LISTEN ax_outbox`.
const OUTBOX_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Outbox claim batch size.
const OUTBOX_BATCH_LIMIT: u32 = 100;

/// Outbox claim lease duration in seconds.
const OUTBOX_LEASE_SECS: u32 = 30;

/// Interval between singleton retention-enqueue checks.
const RETENTION_SCHEDULE_INTERVAL: Duration = Duration::from_secs(3600);

/// Object-safe registry mapping [`JobKind`] variants to [`JobHandler`] implementations.
#[derive(Clone, Default)]
pub(crate) struct JobHandlerRegistry {
    handlers: HashMap<JobKind, Arc<dyn JobHandler>>,
}

impl JobHandlerRegistry {
    /// Creates an empty [`JobHandlerRegistry`].
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers a [`JobHandler`] for `kind`.
    pub(crate) fn register(&mut self, kind: JobKind, handler: Arc<dyn JobHandler>) {
        self.handlers.insert(kind, handler);
    }

    /// Returns the list of registered [`JobKind`]s eligible for leasing.
    pub(crate) fn kinds(&self) -> Vec<JobKind> {
        self.handlers.keys().copied().collect()
    }

    /// Looks up the registered handler for `kind`.
    pub(crate) fn get(&self, kind: JobKind) -> Option<&Arc<dyn JobHandler>> {
        self.handlers.get(&kind)
    }
}

/// Step 00 `RETENTION` handler cleaning expired `ax_jobs` and `ax_outbox` rows per tenant policy.
struct RetentionJobHandler {
    store: PgStore,
}

impl JobHandler for RetentionJobHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move { self.store.run_retention_cleanup(&job.tenant_id).await })
    }
}

/// Spawns `JobWorker` tasks (`AXPOINT_WORKERS`, default `4`), `OutboxDispatcher`, and singleton schedulers.
pub(crate) fn spawn_background_workers(state: &crate::state::AppState) {
    let store = state.store.clone();
    let worker_count = env::var("AXPOINT_WORKERS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(4);

    let mut registry = JobHandlerRegistry::new();
    registry.register(
        JobKind::Retention,
        Arc::new(RetentionJobHandler {
            store: store.clone(),
        }),
    );
    registry.register(
        JobKind::SyncDataset,
        Arc::new(crate::workers::SyncDatasetHandler {
            store: store.clone(),
            keyring: Arc::clone(&state.keyring),
            sidecar: Arc::clone(&state.sidecar),
        }),
    );
    registry.register(
        JobKind::ParseFile,
        Arc::new(crate::workers::ParseFileHandler {
            store: store.clone(),
            ai: state.ai.clone(),
            sidecar: Arc::clone(&state.sidecar),
            data_root: state.data_root.clone(),
        }),
    );
    registry.register(
        JobKind::CredentialRotate,
        Arc::new(crate::workers::CredentialRotateHandler {
            store: store.clone(),
            keyring: Arc::clone(&state.keyring),
        }),
    );
    registry.register(
        JobKind::LakeMaintenance,
        Arc::new(crate::workers::LakeMaintenanceHandler {
            store: store.clone(),
            sidecar: Arc::clone(&state.sidecar),
        }),
    );
    registry.register(
        JobKind::Rollup,
        Arc::new(crate::workers::RollupJobHandler {
            store: store.clone(),
        }),
    );
    registry.register(
        JobKind::EmbedNodes,
        Arc::new(crate::workers::EmbedNodesHandler {
            store: store.clone(),
            ai: state.ai.clone(),
        }),
    );
    let registry = Arc::new(registry);

    let instance_id = Uuid::new_v4().simple().to_string();
    for idx in 0..worker_count {
        let worker_id = format!("core-{instance_id}-w{idx}");
        let worker_store = store.clone();
        let worker_reg = Arc::clone(&registry);
        tokio::spawn(async move {
            run_job_worker(worker_id, worker_store, worker_reg).await;
        });
    }

    let outbox_worker_id = format!("core-{instance_id}-outbox");
    let outbox_store = store.clone();
    let subscribers: Arc<Vec<Arc<dyn OutboxSubscriber>>> = Arc::new(Vec::new());
    tokio::spawn(async move {
        run_outbox_dispatcher(outbox_worker_id, outbox_store, subscribers).await;
    });

    let cron_store = store.clone();
    tokio::spawn(async move {
        crate::workers::run_cron_scheduler(cron_store).await;
    });

    let rollup_store = store.clone();
    tokio::spawn(async move {
        crate::workers::run_rollup_scheduler(rollup_store).await;
    });

    let part_store = store.clone();
    tokio::spawn(async move {
        crate::workers::run_partition_worker(part_store).await;
    });

    tokio::spawn(async move {
        run_retention_scheduler(store).await;
    });
}

async fn run_job_worker(worker_id: String, store: PgStore, registry: Arc<JobHandlerRegistry>) {
    let kinds = registry.kinds();
    if kinds.is_empty() {
        return;
    }

    loop {
        match store.lease(&worker_id, &kinds, 1, JOB_LEASE_SECS).await {
            Ok(jobs) if jobs.is_empty() => {
                tokio::time::sleep(JOB_IDLE_POLL).await;
            }
            Ok(jobs) => {
                for job in jobs {
                    execute_leased_job(&store, &registry, &job).await;
                }
            }
            Err(err) => {
                tracing::warn!(worker = %worker_id, error = %err, "ax_jobs lease failed");
                tokio::time::sleep(JOB_IDLE_POLL).await;
            }
        }
    }
}

async fn execute_leased_job(store: &PgStore, registry: &JobHandlerRegistry, job: &LeasedJob) {
    let kind_label = job.kind.as_str();
    let Some(handler) = registry.get(job.kind) else {
        let _ = store.fail(job, "missing handler for leased job kind").await;
        metrics::counter!("axpoint_jobs_total", "kind" => kind_label, "status" => "FAILED")
            .increment(1);
        return;
    };

    let is_long_sync = matches!(job.kind, JobKind::SyncDataset | JobKind::ParseFile);
    let (lease_secs, hb_interval) = if is_long_sync {
        let _ = store.heartbeat(job, SYNC_JOB_LEASE_SECS).await;
        (SYNC_JOB_LEASE_SECS, SYNC_JOB_HEARTBEAT_INTERVAL)
    } else {
        (JOB_LEASE_SECS, JOB_HEARTBEAT_INTERVAL)
    };

    let hb_store = store.clone();
    let hb_job = job.clone();
    let hb_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(hb_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(err) = hb_store.heartbeat(&hb_job, lease_secs).await {
                tracing::warn!(job_id = %hb_job.id, error = %err, "job heartbeat failed");
            }
        }
    });

    let outcome = handler.handle(job).await;
    hb_handle.abort();

    match outcome {
        Ok(result_json) => {
            if let Err(err) = store.complete(job, result_json).await {
                tracing::error!(job_id = %job.id, error = %err, "failed to mark job SUCCEEDED");
            } else {
                metrics::counter!(
                    "axpoint_jobs_total",
                    "kind" => kind_label,
                    "status" => "SUCCEEDED"
                )
                .increment(1);
            }
        }
        Err(err) => {
            let err_text = err.to_string();
            let status_label = if job.attempt < job.max_attempts {
                "RETRY"
            } else {
                "FAILED"
            };
            if let Err(fail_err) = store.fail(job, &err_text).await {
                tracing::error!(job_id = %job.id, error = %fail_err, "failed to record job failure");
            }
            metrics::counter!(
                "axpoint_jobs_total",
                "kind" => kind_label,
                "status" => status_label
            )
            .increment(1);
        }
    }
}

async fn run_outbox_dispatcher(
    worker_id: String,
    store: PgStore,
    subscribers: Arc<Vec<Arc<dyn OutboxSubscriber>>>,
) {
    // Only dispatch/ack when subscribers are registered; in Step 00 `subscribers` is empty,
    // while `LISTEN ax_outbox` and periodic lag monitoring remain active.
    let mut listener = match PgListener::connect_with(store.pool()).await {
        Ok(mut l) => {
            if let Err(err) = l.listen("ax_outbox").await {
                tracing::warn!(error = %err, "PgListener failed to LISTEN ax_outbox");
                None
            } else {
                Some(l)
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to create PgListener for ax_outbox");
            None
        }
    };

    let mut ticker = tokio::time::interval(OUTBOX_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if let Some(ref mut pg_listener) = listener {
            tokio::select! {
                _ = ticker.tick() => {}
                recv_res = pg_listener.recv() => {
                    if let Err(err) = recv_res {
                        tracing::warn!(error = %err, "PgListener ax_outbox connection interrupted");
                        listener = None;
                    }
                }
            }
        } else {
            ticker.tick().await;
        }

        if subscribers.is_empty() {
            continue;
        }

        if let Err(err) = drain_outbox_batch(&worker_id, &store, &subscribers).await {
            tracing::warn!(worker = %worker_id, error = %err, "outbox dispatch cycle failed");
        }
    }
}

async fn drain_outbox_batch(
    worker_id: &str,
    store: &PgStore,
    subscribers: &[Arc<dyn OutboxSubscriber>],
) -> Result<(), AppError> {
    let events: Vec<OutboxEvent> = store
        .claim(worker_id, OUTBOX_BATCH_LIMIT, OUTBOX_LEASE_SECS)
        .await?;
    if events.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    let mut ack_by_tenant: BTreeMap<String, Vec<i64>> = BTreeMap::new();

    for event in &events {
        let lag_ms = (now - event.created_at).num_milliseconds().max(0);
        let lag_secs = f64::from(u32::try_from(lag_ms).unwrap_or(u32::MAX)) / 1000.0;
        metrics::histogram!("axpoint_outbox_lag_seconds").record(lag_secs);

        let mut all_ok = true;
        for sub in subscribers {
            if let Err(err) = sub.on_event(event).await {
                tracing::error!(
                    event_id = event.id,
                    topic = event.topic.as_str(),
                    error = %err,
                    "outbox subscriber failed"
                );
                all_ok = false;
                break;
            }
        }
        if all_ok {
            ack_by_tenant
                .entry(event.tenant_id.clone())
                .or_default()
                .push(event.id);
        }
    }

    for (tenant_id, ids) in ack_by_tenant {
        store.ack(&tenant_id, &ids).await?;
    }

    Ok(())
}

async fn run_retention_scheduler(store: PgStore) {
    let mut ticker = tokio::time::interval(RETENTION_SCHEDULE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Delay initial run by 60s after startup so cold start stays instantaneous.
    ticker.tick().await;
    tokio::time::sleep(Duration::from_secs(60)).await;

    loop {
        if let Err(err) =
            run_with_advisory_singleton(store.pool(), "axpoint:retention_cron", || {
                enqueue_daily_retention(&store)
            })
            .await
        {
            tracing::warn!(error = %err, "retention singleton scheduler iteration failed");
        }
        ticker.tick().await;
    }
}

async fn enqueue_daily_retention(store: &PgStore) -> Result<(), AppError> {
    let day_key = Utc::now().format("retention:%Y-%m-%d").to_string();
    let tenants = store.list_active_tenants().await?;
    for tenant_id in tenants {
        let job = NewJob::new(JobKind::Retention, serde_json::json!({}))
            .with_idempotency_key(day_key.clone());
        store.enqueue(&tenant_id, job).await?;
    }
    Ok(())
}

/// Executes `task` on a single dedicated PostgreSQL session only if `pg_try_advisory_lock(hashtext($1))` succeeds.
async fn run_with_advisory_singleton<F, Fut>(
    pool: &sqlx::PgPool,
    lock_key: &str,
    task: F,
) -> Result<bool, AppError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), AppError>>,
{
    let mut conn = pool
        .acquire()
        .await
        .map_err(|err| AppError::Database(format!("acquire advisory lock conn failed: {err}")))?;

    let row = sqlx::query("SELECT pg_try_advisory_lock(hashtext($1)) AS locked")
        .bind(lock_key)
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| AppError::Database(format!("pg_try_advisory_lock failed: {err}")))?;

    let locked: bool = row.get("locked");
    if !locked {
        return Ok(false);
    }

    let result = task().await;

    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(lock_key)
        .execute(&mut *conn)
        .await;

    result.map(|()| true)
}
