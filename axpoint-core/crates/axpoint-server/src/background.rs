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

/// Default job lease duration in seconds (`20s` lease renewed every `7s` for fast crash recovery).
const JOB_LEASE_SECS: u32 = 20;

/// Interval between `heartbeat` renewals while a job is running (`7s` < `20s` lease).
const JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(7);

/// Poll interval when no `ax_jobs` work was found on the previous iteration.
const JOB_IDLE_POLL: Duration = Duration::from_millis(250);

/// Lease duration in seconds for sync/parse/map jobs (`30s` lease renewed every `10s` for fast crash recovery).
const SYNC_JOB_LEASE_SECS: u32 = 30;

/// Heartbeat interval for sync/parse/map jobs (`10s` < `30s` lease).
const SYNC_JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Outbox fallback polling interval alongside `LISTEN ax_outbox`.
const OUTBOX_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Outbox claim batch size.
const OUTBOX_BATCH_LIMIT: u32 = 100;

/// Outbox claim lease duration in seconds.
const OUTBOX_LEASE_SECS: u32 = 30;

/// Interval between singleton `AxFlow` cron & overdue-approval checks (`07-STEP07-08` §07.5).
const FLOW_CRON_INTERVAL: Duration = Duration::from_secs(30);

/// Interval between singleton Step 08 absence/auto-resolve/escalation rule scans (`07-STEP07-08` §08.3.1).
const ABSENCE_SCAN_INTERVAL: Duration = Duration::from_secs(60);

/// Interval between singleton Step 08 pending notification retry polls (`07-STEP07-08` §08.4).
const NOTIFY_RETRY_INTERVAL: Duration = Duration::from_secs(5);

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

/// Step 00, Step 12 & Step 14 `RETENTION` handler cleaning expired DB rows, `EXPORT_FILE` artifacts, and `DuckLake` snapshots.
struct RetentionJobHandler {
    store: PgStore,
    sidecar: Arc<crate::sidecar::HttpSidecarClient>,
    data_root: std::path::PathBuf,
}

impl JobHandler for RetentionJobHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let mut report = self
                .store
                .run_retention_cleanup_with_data_root(&job.tenant_id, &self.data_root)
                .await?;

            let keep_days = resolve_lake_keep_days(&self.store, &job.tenant_id).await;
            let maint_req = axpoint_domain::axhub::LakeMaintenanceRequest {
                tenant_id: job.tenant_id.clone(),
                keep_days,
            };
            match axpoint_domain::ports::SidecarPort::run_maintenance(
                self.sidecar.as_ref(),
                &job.tenant_id,
                &maint_req,
            )
            .await
            {
                Ok(lake_res) => {
                    if let (serde_json::Value::Object(map), Ok(val)) =
                        (&mut report, serde_json::to_value(&lake_res))
                    {
                        map.insert(String::from("lake_maintenance"), val);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        tenant_id = %job.tenant_id,
                        error = %err,
                        "sidecar lake maintenance skipped during retention"
                    );
                }
            }
            Ok(report)
        })
    }
}

async fn resolve_lake_keep_days(store: &PgStore, tenant_id: &str) -> i32 {
    if let Ok(policies) =
        axpoint_domain::ports::OpsRepository::list_retention_policies(store, tenant_id).await
        && let Some(found) = policies
            .iter()
            .find(|p| p.target == axpoint_domain::enums::RetentionTarget::LakeSnapshot)
    {
        return found.keep_days.max(1);
    }
    axpoint_domain::ports::DatasetRepository::get_lake_snapshot_keep_days(store, tenant_id)
        .await
        .unwrap_or(30)
        .max(1)
}

/// Step 14 `JobKind::Backup` handler executing `AXPOINT_BACKUP_CMD` when configured (`§14.3.1`).
struct BackupJobHandler;

impl JobHandler for BackupJobHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let backup_id = job
                .payload
                .get("backup_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "missing valid backup_id in payload")
                })?;
            let backup_kind = job
                .payload
                .get("backup_kind")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| s.parse::<axpoint_domain::ops::BackupKind>().ok())
                .unwrap_or(axpoint_domain::ops::BackupKind::Full);

            let Some(cmd) = env::var("AXPOINT_BACKUP_CMD")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
            else {
                return Ok(serde_json::json!({
                    "backup_id": backup_id,
                    "backup_kind": backup_kind,
                    "skipped": true,
                }));
            };

            let shell_line = format!("{cmd} {} {backup_id}", backup_kind.as_str());
            let output = tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&shell_line)
                .env("BACKUP_ID", backup_id.to_string())
                .env("BACKUP_KIND", backup_kind.as_str())
                .output()
                .await
                .map_err(|err| {
                    AppError::Config(format!("spawn AXPOINT_BACKUP_CMD failed: {err}"))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(AppError::Config(format!(
                    "AXPOINT_BACKUP_CMD exited with {}: {}",
                    output.status,
                    stderr.trim()
                )));
            }

            Ok(serde_json::json!({
                "backup_id": backup_id,
                "backup_kind": backup_kind,
                "executed": true,
            }))
        })
    }
}

/// Step 14 `alert.resolved` subscriber appending `LINK(ALERT_RESOLVED)` timeline events to linked work items (`§14.7`).
struct AlertResolvedWorkItemSubscriber {
    store: PgStore,
}

impl OutboxSubscriber for AlertResolvedWorkItemSubscriber {
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != axpoint_domain::enums::OutboxTopic::AlertResolved {
                return Ok(());
            }
            let Some(alert_id) = event
                .payload
                .get("alert_id")
                .or_else(|| event.payload.get("id"))
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return Ok(());
            };
            let _linked = axpoint_domain::ports::OpsRepository::link_resolved_alert_work_items(
                &self.store,
                &event.tenant_id,
                alert_id,
            )
            .await?;
            Ok(())
        })
    }
}

/// Step 04 & Step 06 `dataset.synced` subscriber refreshing `SessionPool`, cascading `DERIVED_VIEW` syncs,
/// and enqueueing incremental `MAP_MATERIALIZE` jobs for enabled `main` mappings (`06-STEP05-06` §06.2.2).
struct DatasetSyncedSubscriber {
    store: PgStore,
    analytics_pool: Arc<axpoint_analytics::SessionPool>,
}

impl OutboxSubscriber for DatasetSyncedSubscriber {
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != axpoint_domain::enums::OutboxTopic::DatasetSynced {
                return Ok(());
            }
            let _ = self
                .analytics_pool
                .invalidate_tenant(&event.tenant_id)
                .await;

            let Some(ds_id) = event
                .payload
                .get("dataset_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return Ok(());
            };

            let dependents =
                axpoint_domain::ports::AnalyticsRepository::list_dependent_derived_datasets(
                    &self.store,
                    &event.tenant_id,
                    ds_id,
                )
                .await?;

            let ds_id_str = ds_id.to_string();
            for dep in dependents {
                let is_direct = dep
                    .source_ref
                    .get("depends_on")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some(ds_id_str.as_str())));
                if is_direct {
                    let _ = axpoint_domain::ports::DatasetRepository::enqueue_sync_job(
                        &self.store,
                        &event.tenant_id,
                        dep.id,
                    )
                    .await?;
                }
            }

            let mappings =
                axpoint_domain::ports::StudioExecutionPort::list_enabled_main_mappings_for_dataset(
                    &self.store,
                    &event.tenant_id,
                    ds_id,
                )
                .await?;
            let snap_opt = event
                .payload
                .get("snapshot_id")
                .and_then(serde_json::Value::as_i64);

            for mapping in mappings {
                let payload = axpoint_domain::rollup::MapMaterializeJobPayload {
                    mapping_id: mapping.id,
                    mode: axpoint_domain::studio::MappingMode::Incremental,
                    schema_version: None,
                };
                let payload_json = serde_json::to_value(&payload).map_err(|err| {
                    AppError::Database(format!("serialize MapMaterializeJobPayload: {err}"))
                })?;
                let idem_key = match snap_opt {
                    Some(snap) => {
                        axpoint_domain::rollup::MapMaterializeJobPayload::idempotency_key_for_snapshot(
                            mapping.id, snap,
                        )
                    }
                    None => format!("outbox:{}:{}", event.id, mapping.id),
                };
                let mut job = NewJob::new(JobKind::MapMaterialize, payload_json)
                    .with_idempotency_key(idem_key);
                job.priority = 10;
                job.max_attempts = 3;
                let _ = self.store.enqueue(&event.tenant_id, job).await?;
            }

            Ok(())
        })
    }
}

fn parse_uuid_array(payload: &serde_json::Value, key: &str) -> Vec<Uuid> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .filter_map(|s| Uuid::parse_str(s).ok())
                .collect()
        })
}

fn parse_string_array(payload: &serde_json::Value, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(String::from)
                .collect()
        })
}

/// Step 06 `node.metrics_changed` subscriber triggering incremental `GRAPH_AGGREGATE` (`06-STEP05-06` §06.3.3)
/// while skipping `origin == "AGGREGATE"` events (and refreshing knowledge dossiers when aggregated nodes belong to `sync_to_knowledge = true` types).
struct NodeMetricsChangedSubscriber {
    store: PgStore,
}

impl OutboxSubscriber for NodeMetricsChangedSubscriber {
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != axpoint_domain::enums::OutboxTopic::NodeMetricsChanged {
                return Ok(());
            }

            let node_ids = parse_uuid_array(&event.payload, "node_ids");
            if node_ids.is_empty() {
                return Ok(());
            }

            let origin = event
                .payload
                .get("origin")
                .and_then(serde_json::Value::as_str);

            if origin == Some("AGGREGATE") {
                enqueue_knowledge_sync_if_needed(
                    &self.store,
                    &event.tenant_id,
                    event.id,
                    None,
                    &node_ids,
                    None,
                )
                .await?;
                return Ok(());
            }

            let metrics = parse_string_array(&event.payload, "metrics");
            let payload = axpoint_domain::rollup::GraphAggregateJobPayload {
                full: false,
                full_type: None,
                node_ids,
                metrics,
                schema_version: None,
            };
            let idem_key = format!("agg:{}", payload.canonical_hash());
            let _ = axpoint_domain::ports::StudioExecutionPort::enqueue_graph_aggregate(
                &self.store,
                &event.tenant_id,
                payload,
                Some(idem_key),
            )
            .await?;

            Ok(())
        })
    }
}

/// Step 06 `node.upserted` subscriber triggering `KNOWLEDGE_SYNC` for `sync_to_knowledge = true` types and deleted nodes (`06-STEP05-06` §06.5).
struct NodeUpsertedSubscriber {
    store: PgStore,
}

impl OutboxSubscriber for NodeUpsertedSubscriber {
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != axpoint_domain::enums::OutboxTopic::NodeUpserted {
                return Ok(());
            }

            let deleted_node_id = event
                .payload
                .get("deleted_node_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());

            let mut node_ids = parse_uuid_array(&event.payload, "node_ids");
            if let Some(single_id) = event
                .payload
                .get("node_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                && !node_ids.contains(&single_id)
            {
                node_ids.push(single_id);
            }

            let type_hint = event
                .payload
                .get("type_name")
                .and_then(serde_json::Value::as_str);

            enqueue_knowledge_sync_if_needed(
                &self.store,
                &event.tenant_id,
                event.id,
                deleted_node_id,
                &node_ids,
                type_hint,
            )
            .await
        })
    }
}

async fn enqueue_knowledge_sync_if_needed(
    store: &PgStore,
    tenant_id: &str,
    outbox_id: i64,
    deleted_node_id: Option<Uuid>,
    node_ids: &[Uuid],
    type_hint: Option<&str>,
) -> Result<(), AppError> {
    if deleted_node_id.is_none() && node_ids.is_empty() {
        return Ok(());
    }

    if deleted_node_id.is_none() {
        let published =
            axpoint_domain::ports::StudioRepository::get_published_schema(store, tenant_id, "main")
                .await?;
        let any_enabled = match type_hint {
            Some(tn) => published
                .doc
                .node_types
                .iter()
                .any(|nt| nt.name == tn && nt.sync_to_knowledge),
            None => published
                .doc
                .node_types
                .iter()
                .any(|nt| nt.sync_to_knowledge),
        };
        if !any_enabled {
            return Ok(());
        }
    }

    let payload = axpoint_domain::rollup::KnowledgeSyncJobPayload {
        node_id: if node_ids.len() == 1 {
            node_ids.first().copied()
        } else {
            None
        },
        node_ids: if node_ids.len() > 1 {
            node_ids.to_vec()
        } else {
            Vec::new()
        },
        deleted_node_id,
        type_names: Vec::new(),
        schema_version: None,
    };
    let payload_json = serde_json::to_value(&payload)
        .map_err(|err| AppError::Database(format!("serialize KnowledgeSyncJobPayload: {err}")))?;
    let mut job = NewJob::new(JobKind::KnowledgeSync, payload_json)
        .with_idempotency_key(format!("ksync:outbox:{outbox_id}"));
    job.priority = 30;
    job.max_attempts = 3;
    let _ = store.enqueue(tenant_id, job).await?;
    Ok(())
}

fn build_flow_engine(state: &crate::state::AppState) -> Arc<axpoint_flow::FlowEngine<PgStore>> {
    let store_arc = Arc::new(state.store.clone());
    let state_arc = Arc::new(state.clone());
    let ai_arc = Arc::new(state.ai.clone());
    let url_guard = Arc::new(axpoint_security::NetGuard::new());
    let runtime_arc = Arc::clone(&state.axagent.runtime);
    let agent_task: Arc<dyn axpoint_domain::ports::AgentTaskPort> = runtime_arc;
    let sim_port: Arc<dyn axpoint_domain::ports::SimExecutionPort> =
        Arc::new(crate::routes::axsim::ServerSimPort {
            store: state.store.clone(),
        });
    let operators = Arc::new(axpoint_flow::build_step10_operator_registry(
        axpoint_flow::Step10OperatorDeps {
            step09: axpoint_flow::Step09OperatorDeps {
                step07: axpoint_flow::Step07OperatorDeps {
                    store: Arc::clone(&store_arc),
                    analytics: Arc::clone(&state_arc),
                    sql_materializer: Some(Arc::clone(&state_arc)),
                    ai: ai_arc,
                    lake: Arc::clone(&state.analytics_pool),
                    url_guard,
                    cred_unsealer: Some(state_arc),
                },
                agent_task,
            },
            sim_port,
        },
    ));
    Arc::new(axpoint_flow::FlowEngine::new(
        store_arc,
        operators,
        state.data_root.clone(),
    ))
}

type ServerNotifyDispatcher = axpoint_flow::NotificationDispatcher<
    PgStore,
    axpoint_security::NetGuard,
    crate::state::AppState,
>;

fn build_job_registry(
    state: &crate::state::AppState,
    flow_engine: &Arc<axpoint_flow::FlowEngine<PgStore>>,
    rule_evaluator: &Arc<axpoint_flow::RuleEvaluator<PgStore>>,
) -> Arc<JobHandlerRegistry> {
    let store = &state.store;
    let mut registry = JobHandlerRegistry::new();
    registry.register(
        JobKind::Retention,
        Arc::new(RetentionJobHandler {
            store: store.clone(),
            sidecar: Arc::clone(&state.sidecar),
            data_root: state.data_root.clone(),
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
    register_domain_job_handlers(&mut registry, state, flow_engine, rule_evaluator);
    Arc::new(registry)
}

fn register_domain_job_handlers(
    registry: &mut JobHandlerRegistry,
    state: &crate::state::AppState,
    flow_engine: &Arc<axpoint_flow::FlowEngine<PgStore>>,
    rule_evaluator: &Arc<axpoint_flow::RuleEvaluator<PgStore>>,
) {
    let store = &state.store;
    registry.register(
        JobKind::EmbedNodes,
        Arc::new(crate::workers::EmbedNodesHandler {
            store: store.clone(),
            ai: state.ai.clone(),
        }),
    );
    registry.register(
        JobKind::MapMaterialize,
        Arc::new(crate::workers::MapMaterializeHandler {
            store: store.clone(),
            analytics_pool: Arc::clone(&state.analytics_pool),
        }),
    );
    registry.register(
        JobKind::GraphAggregate,
        Arc::new(crate::workers::GraphAggregateHandler {
            store: store.clone(),
        }),
    );
    registry.register(
        JobKind::KnowledgeSync,
        Arc::new(crate::workers::KnowledgeSyncHandler {
            store: store.clone(),
            ai: state.ai.clone(),
        }),
    );
    registry.register(
        JobKind::FlowStep,
        Arc::new(axpoint_flow::FlowStepJobHandler::new(Arc::clone(
            flow_engine,
        ))),
    );
    registry.register(
        JobKind::RuleEval,
        Arc::new(axpoint_flow::RuleEvalJobHandler::new(Arc::clone(
            rule_evaluator,
        ))),
    );
    registry.register(
        JobKind::Export,
        Arc::new(axpoint_server::view::export::ExportJobHandler {
            store: store.clone(),
            analytics_pool: Arc::clone(&state.analytics_pool),
            cache: Arc::clone(&state.view.cache),
            cedar: Arc::clone(&state.cedar),
            sidecar: Arc::clone(&state.sidecar),
            audit: state.audit.clone(),
            data_root: state.data_root.clone(),
        }),
    );
    registry.register(JobKind::Backup, Arc::new(BackupJobHandler));
}

/// Spawns `JobWorker` tasks (`AXPOINT_WORKERS`, default `4`), `OutboxDispatcher`, and singleton schedulers.
pub(crate) fn spawn_background_workers(
    state: &crate::state::AppState,
    shutdown_rx: &tokio::sync::watch::Receiver<bool>,
) {
    let store = state.store.clone();
    let store_arc = Arc::new(store.clone());
    let flow_engine = build_flow_engine(state);
    let notify_dispatcher = Arc::new(axpoint_flow::NotificationDispatcher::new(
        Arc::clone(&store_arc),
        Arc::new(axpoint_security::NetGuard::new()),
        Arc::new(state.clone()),
    ));
    let launcher: Arc<dyn axpoint_flow::RuleFlowLauncherPort> =
        Arc::new(axpoint_flow::FlowEngineLauncher::from_engine(&flow_engine));
    let escalation_concrete = Arc::clone(&notify_dispatcher);
    let escalation_notifier: Arc<dyn axpoint_flow::AlertEscalationNotifier> = escalation_concrete;
    let rule_evaluator = Arc::new(
        axpoint_flow::RuleEvaluator::new(Arc::clone(&store_arc), Some(launcher))
            .with_escalation_notifier(escalation_notifier),
    );

    let worker_count = env::var("AXPOINT_WORKERS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(4);
    let registry = build_job_registry(state, &flow_engine, &rule_evaluator);

    let instance_id = Uuid::new_v4().simple().to_string();
    for idx in 0..worker_count {
        let worker_id = format!("core-{instance_id}-w{idx}");
        let worker_store = store.clone();
        let worker_reg = Arc::clone(&registry);
        let worker_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            run_job_worker(worker_id, worker_store, worker_reg, worker_shutdown).await;
        });
    }

    let outbox_worker_id = format!("core-{instance_id}-outbox");
    let outbox_store = store.clone();
    let subscribers =
        build_outbox_subscribers(state, &store, &store_arc, flow_engine, &notify_dispatcher);
    let outbox_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        run_outbox_dispatcher(outbox_worker_id, outbox_store, subscribers, outbox_shutdown).await;
    });

    spawn_pg_listeners(state, shutdown_rx);
    spawn_singleton_schedulers(store, store_arc, notify_dispatcher, shutdown_rx);
}

fn build_outbox_subscribers(
    state: &crate::state::AppState,
    store: &PgStore,
    store_arc: &Arc<PgStore>,
    flow_engine: Arc<axpoint_flow::FlowEngine>,
    notify_dispatcher: &Arc<axpoint_flow::NotificationDispatcher<PgStore, crate::state::AppState>>,
) -> Arc<[Arc<dyn OutboxSubscriber>]> {
    let ds_subscriber: Arc<dyn OutboxSubscriber> = Arc::new(DatasetSyncedSubscriber {
        store: store.clone(),
        analytics_pool: Arc::clone(&state.analytics_pool),
    });
    let view_sub: Arc<dyn OutboxSubscriber> =
        Arc::new(axpoint_server::view::events::ViewEventSubscriber::new(
            store.clone(),
            store.pool().clone(),
            Arc::clone(&state.view.cache),
        ));
    Arc::from([
        ds_subscriber,
        Arc::new(NodeMetricsChangedSubscriber {
            store: store.clone(),
        }),
        Arc::new(NodeUpsertedSubscriber {
            store: store.clone(),
        }),
        Arc::new(axpoint_flow::ApprovalDecidedSubscriber::new(flow_engine)),
        Arc::new(crate::routes::axsim::SimApprovalDecidedSubscriber {
            store: store.clone(),
        }),
        Arc::new(axpoint_agent::AgentApprovalDecidedSubscriber::new(
            Arc::clone(&state.axagent.runtime),
        )),
        Arc::new(axpoint_flow::FlowOutboxTriggerSubscriber::new(Arc::clone(
            store_arc,
        ))),
        Arc::new(axpoint_flow::RuleTriggerSubscriber::new(Arc::clone(
            store_arc,
        ))),
        Arc::new(axpoint_flow::AlertNotifySubscriber::new(Arc::clone(
            notify_dispatcher,
        ))),
        Arc::new(AlertResolvedWorkItemSubscriber {
            store: store.clone(),
        }),
        view_sub,
    ])
}

fn spawn_pg_listeners(
    state: &crate::state::AppState,
    shutdown_rx: &tokio::sync::watch::Receiver<bool>,
) {
    let view_pool = state.store.pool().clone();
    let view_bus = Arc::clone(&state.view.event_bus);
    let view_cache = Arc::clone(&state.view.cache);
    let view_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        axpoint_server::view::events::run_view_event_listener_with_cache(
            view_pool,
            view_bus,
            Some(view_cache),
            view_shutdown,
        )
        .await;
    });

    let policy_pool = state.store.pool().clone();
    let policy_cedar = Arc::clone(&state.cedar);
    let policy_auth = Arc::clone(&state.authenticator);
    let policy_cache = Arc::clone(&state.view.cache);
    let policy_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        run_policy_changed_listener(
            policy_pool,
            policy_cedar,
            policy_auth,
            policy_cache,
            policy_shutdown,
        )
        .await;
    });
}

fn spawn_singleton_schedulers(
    store: PgStore,
    store_arc: Arc<PgStore>,
    notify_dispatcher: Arc<ServerNotifyDispatcher>,
    shutdown_rx: &tokio::sync::watch::Receiver<bool>,
) {
    let cron_store = store.clone();
    let cron_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        crate::workers::run_cron_scheduler(cron_store, cron_shutdown).await;
    });

    let flow_cron_store = Arc::clone(&store_arc);
    let flow_cron_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        run_flow_cron_scheduler(flow_cron_store, flow_cron_shutdown).await;
    });

    let absence_store = Arc::clone(&store_arc);
    let absence_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        run_absence_rule_scheduler(absence_store, absence_shutdown).await;
    });

    let retry_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        run_notify_retry_scheduler(store_arc, notify_dispatcher, retry_shutdown).await;
    });

    let rollup_store = store.clone();
    let rollup_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        crate::workers::run_rollup_scheduler(rollup_store, rollup_shutdown).await;
    });

    let part_store = store.clone();
    let part_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        crate::workers::run_partition_worker(part_store, part_shutdown).await;
    });

    let ret_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        run_retention_scheduler(store, ret_shutdown).await;
    });
}

async fn run_flow_cron_scheduler(
    store: Arc<PgStore>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let scheduler = axpoint_flow::FlowCronScheduler::new(Arc::clone(&store));
    let mut ticker = tokio::time::interval(FLOW_CRON_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;

    while !*shutdown_rx.borrow_and_update() {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => break,
        }
        if let Err(err) =
            run_with_advisory_singleton(store.pool(), "axpoint:axflow_cron", || async {
                let _ = scheduler.tick_once(Utc::now()).await?;
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %err, "axflow cron singleton scheduler iteration failed");
        }
    }
}

async fn run_absence_rule_scheduler(
    store: Arc<PgStore>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let scanner = axpoint_flow::AbsenceRuleScanner::new(Arc::clone(&store));
    let mut ticker = tokio::time::interval(ABSENCE_SCAN_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;

    while !*shutdown_rx.borrow_and_update() {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => break,
        }
        if let Err(err) =
            run_with_advisory_singleton(store.pool(), "axpoint:absence_rules", || async {
                let _ = scanner.scan_once(Utc::now()).await?;
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %err, "absence rule singleton scheduler iteration failed");
        }
    }
}

async fn run_notify_retry_scheduler(
    store: Arc<PgStore>,
    dispatcher: Arc<ServerNotifyDispatcher>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(NOTIFY_RETRY_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;

    while !*shutdown_rx.borrow_and_update() {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => break,
        }
        if let Err(err) =
            run_with_advisory_singleton(store.pool(), "axpoint:notify_retries", || async {
                let _ = dispatcher.poll_due_deliveries(50).await?;
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %err, "notify retry singleton scheduler iteration failed");
        }
    }
}

async fn run_job_worker(
    worker_id: String,
    store: PgStore,
    registry: Arc<JobHandlerRegistry>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let kinds = registry.kinds();
    if kinds.is_empty() {
        return;
    }

    while !*shutdown_rx.borrow_and_update() {
        match store.lease(&worker_id, &kinds, 1, JOB_LEASE_SECS).await {
            Ok(jobs) if jobs.is_empty() => {
                tokio::select! {
                    () = tokio::time::sleep(JOB_IDLE_POLL) => {}
                    _ = shutdown_rx.changed() => break,
                }
            }
            Ok(jobs) => {
                for job in jobs {
                    execute_leased_job(&store, &registry, &job).await;
                }
            }
            Err(err) => {
                tracing::warn!(worker = %worker_id, error = %err, "ax_jobs lease failed");
                tokio::select! {
                    () = tokio::time::sleep(JOB_IDLE_POLL) => {}
                    _ = shutdown_rx.changed() => break,
                }
            }
        }
    }
}

async fn execute_leased_job(store: &PgStore, registry: &JobHandlerRegistry, job: &LeasedJob) {
    let kind_label = job.kind.as_str();
    let Some(handler) = registry.get(job.kind) else {
        if let Err(err) = store.fail(job, "missing handler for leased job kind").await {
            tracing::warn!(job_id = %job.id, error = %err, "failed to mark unhandled job as failed");
        }
        metrics::counter!("axpoint_jobs_total", "kind" => kind_label, "status" => "FAILED")
            .increment(1);
        return;
    };

    let is_long_sync = matches!(
        job.kind,
        JobKind::SyncDataset
            | JobKind::ParseFile
            | JobKind::MapMaterialize
            | JobKind::Export
            | JobKind::Backup
    );
    let (lease_secs, hb_interval) = if is_long_sync {
        if let Err(err) = store.heartbeat(job, SYNC_JOB_LEASE_SECS).await {
            tracing::warn!(job_id = %job.id, error = %err, "initial sync job heartbeat failed");
        }
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
                break;
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
    subscribers: Arc<[Arc<dyn OutboxSubscriber>]>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
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

    while !*shutdown_rx.borrow_and_update() {
        if let Some(ref mut pg_listener) = listener {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shutdown_rx.changed() => break,
                recv_res = pg_listener.recv() => {
                    if let Err(err) = recv_res {
                        tracing::warn!(error = %err, "PgListener ax_outbox connection interrupted");
                        listener = None;
                    }
                }
            }
        } else {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shutdown_rx.changed() => break,
            }
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

async fn run_retention_scheduler(
    store: PgStore,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(RETENTION_SCHEDULE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Delay initial run by 60s after startup so cold start stays instantaneous.
    ticker.tick().await;
    tokio::select! {
        () = tokio::time::sleep(Duration::from_secs(60)) => {}
        _ = shutdown_rx.changed() => return,
    }

    while !*shutdown_rx.borrow_and_update() {
        if let Err(err) =
            run_with_advisory_singleton(store.pool(), "axpoint:retention_cron", || {
                enqueue_daily_retention(&store)
            })
            .await
        {
            tracing::warn!(error = %err, "retention singleton scheduler iteration failed");
        }
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => break,
        }
    }
}

async fn enqueue_daily_retention(store: &PgStore) -> Result<(), AppError> {
    if let Err(err) =
        axpoint_domain::ports::OpsRepository::drop_expired_metric_partitions(store).await
    {
        tracing::warn!(error = %err, "drop_expired_metric_partitions skipped");
    }
    let hour_key = Utc::now().format("retention:%Y-%m-%dT%H").to_string();
    let tenants = store.list_active_tenants().await?;
    for tenant_id in tenants {
        let job = NewJob::new(JobKind::Retention, serde_json::json!({}))
            .with_idempotency_key(hour_key.clone());
        let _job_id = store.enqueue(&tenant_id, job).await?;
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
    Fut: Future<Output = Result<(), AppError>>,
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

    if let Err(err) = sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(lock_key)
        .execute(&mut *conn)
        .await
    {
        tracing::warn!(lock_key = %lock_key, error = %err, "pg_advisory_unlock failed");
    }

    result.map(|()| true)
}

async fn run_policy_changed_listener(
    pool: sqlx::PgPool,
    cedar: Arc<axpoint_security::cedar_authz::CedarAuthorizer>,
    authenticator: Arc<axpoint_security::Authenticator<PgStore>>,
    cache: Arc<axpoint_server::view::cache::ViewCache>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    while !*shutdown_rx.borrow_and_update() {
        let mut listener = match PgListener::connect_with(&pool).await {
            Ok(l) => l,
            Err(err) => {
                tracing::warn!(error = %err, "ax_policy_changed PgListener connect failed");
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(2)) => continue,
                    _ = shutdown_rx.changed() => break,
                }
            }
        };
        if let Err(err) = listener.listen("ax_policy_changed").await {
            tracing::warn!(error = %err, "LISTEN ax_policy_changed failed");
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(2)) => continue,
                _ = shutdown_rx.changed() => break,
            }
        }

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => return,
                msg = listener.recv() => {
                    match msg {
                        Ok(notification) => {
                            let tenant_id = notification.payload().trim();
                            if !tenant_id.is_empty() {
                                cedar.invalidate_tenant(tenant_id);
                                authenticator.invalidate_tenant(tenant_id);
                                cache.invalidate_tenant(tenant_id);
                                cache.run_pending_tasks().await;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "ax_policy_changed recv error; reconnecting");
                            break;
                        }
                    }
                }
            }
        }
    }
}
