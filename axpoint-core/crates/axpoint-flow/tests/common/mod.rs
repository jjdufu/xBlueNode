//! Shared in-memory repository, SSRF guard, and HTTP sink harness for Step 08 integration tests.

use std::collections::{BTreeMap, HashMap};
use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use axpoint_domain::AppError;
use axpoint_domain::axhub::{Dataset, PaginatedList, QualityReport, SealedCredential};
use axpoint_domain::enums::{AlertStatus, JobKind, NotifyKind, Severity};
use axpoint_domain::graph::{
    BulkNodesRequest, BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery,
    LinkRecord, NodeDocumentsResponse, NodeListQuery, NodeListResponse, NodeSearchQuery,
    NodeSearchResponse, PatchNodeRequest, ResolveEntitiesRequest, ResolveEntitiesResponse,
    UpsertLinkRequest,
};
use axpoint_domain::platform::{
    BoxFuture, JobListQuery, JobListResponse, JobRecord, LeasedJob, NewJob,
};
use axpoint_domain::ports::{
    FlowCredentialUnsealPort, GraphStorePort, JobQueuePort, OutboundUrlGuardPort, RulesRepository,
};
use axpoint_domain::rules::{
    AbsenceRuleItem, AckBatchResponse, AlertEventKind, AlertEventRecord, AlertEventsQuery,
    AlertListQuery, AlertRecord, AlertResolveReason, AlertSilenceRecord, Condition,
    CreateRuleRequest, CreateSilenceRequest, DeliveryFailureUpdate, DueDeliveryItem,
    EnqueueDeliveryInput, FalseConditionOutcome, NotifyChannelRecord, NotifyDeliveryRecord,
    NotifyDeliveryStatus, RuleDef, RuleListQuery, RuleScopeKind, SaveNotifyChannelInput,
    SimplePageQuery, TrueConditionInput, TrueConditionOutcome, UpdateRuleRequest,
};
use axpoint_domain::topology::{BlueNode, UpsertBlueNodeRequest};
use axpoint_flow::{RuleFlowLauncherPort, is_loopback_url};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use uuid::Uuid;

pub type RawPointsMap = HashMap<(Uuid, String), Vec<(DateTime<Utc>, f64)>>;
pub type CapturedRequests = Arc<Mutex<Vec<(String, String)>>>;

#[derive(Default)]
pub struct MemState {
    pub rules: HashMap<Uuid, RuleDef>,
    pub nodes: HashMap<Uuid, BlueNode>,
    pub raw_points: RawPointsMap,
    pub alerts: HashMap<Uuid, AlertRecord>,
    pub events: Vec<AlertEventRecord>,
    pub channels: HashMap<Uuid, NotifyChannelRecord>,
    pub secrets: HashMap<Uuid, serde_json::Value>,
    pub deliveries: HashMap<Uuid, NotifyDeliveryRecord>,
    pub silences: Vec<AlertSilenceRecord>,
    pub jobs: Vec<(String, NewJob)>,
    pub launched_flows: Vec<(Uuid, Uuid)>,
}

#[derive(Default)]
pub struct MemStore {
    st: Mutex<MemState>,
}

impl MemStore {
    pub fn lock_st(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.st
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn unimpl<T>() -> Result<T, AppError> {
    Err(AppError::Config(String::from("unused in unit test")))
}

impl RulesRepository for MemStore {
    fn list_rules(
        &self,
        _: &str,
        _: RuleListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RuleDef>, AppError>> + Send {
        ready(unimpl())
    }

    fn list_enabled_rules_by_scope(
        &self,
        _: &str,
        scope: RuleScopeKind,
    ) -> impl Future<Output = Result<Vec<RuleDef>, AppError>> + Send {
        let out = self
            .lock_st()
            .rules
            .values()
            .filter(|r| r.enabled && r.scope_kind == scope)
            .cloned()
            .collect();
        ready(Ok(out))
    }

    fn get_rule(
        &self,
        _: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<RuleDef>, AppError>> + Send {
        ready(Ok(self.lock_st().rules.get(&id).cloned()))
    }

    fn create_rule(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: CreateRuleRequest,
    ) -> impl Future<Output = Result<RuleDef, AppError>> + Send {
        ready(unimpl())
    }

    fn update_rule(
        &self,
        _: &str,
        _: Uuid,
        _: UpdateRuleRequest,
    ) -> impl Future<Output = Result<Option<RuleDef>, AppError>> + Send {
        ready(unimpl())
    }

    fn delete_rule(&self, _: &str, _: Uuid) -> impl Future<Output = Result<bool, AppError>> + Send {
        ready(unimpl())
    }

    fn list_alerts(
        &self,
        _: &str,
        _: AlertListQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertRecord>, AppError>> + Send {
        ready(unimpl())
    }

    fn get_alert(
        &self,
        _: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<AlertRecord>, AppError>> + Send {
        ready(Ok(self.lock_st().alerts.get(&id).cloned()))
    }

    fn list_active_alerts_for_rule(
        &self,
        _: &str,
        rule_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, AppError>> + Send {
        let out = self
            .lock_st()
            .alerts
            .values()
            .filter(|a| a.rule_id == rule_id && a.status != AlertStatus::Resolved)
            .cloned()
            .collect();
        ready(Ok(out))
    }

    fn list_alert_events(
        &self,
        _: &str,
        _: Uuid,
        _: AlertEventsQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertEventRecord>, AppError>> + Send {
        ready(unimpl())
    }

    fn append_alert_event(
        &self,
        _: &str,
        alert_id: Uuid,
        event: AlertEventKind,
        detail: serde_json::Value,
        actor: Option<Uuid>,
    ) -> impl Future<Output = Result<AlertEventRecord, AppError>> + Send {
        let mut g = self.lock_st();
        let id = i64::try_from(g.events.len().saturating_add(1)).unwrap_or(1);
        let rec = AlertEventRecord {
            id,
            alert_id,
            event,
            detail,
            actor,
            created_at: Utc::now(),
        };
        g.events.push(rec.clone());
        ready(Ok(rec))
    }

    fn set_alert_flow_run(
        &self,
        _: &str,
        alert_id: Uuid,
        flow_id: Uuid,
        run_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        let mut g = self.lock_st();
        g.events.push(AlertEventRecord {
            id: 1,
            alert_id,
            event: AlertEventKind::FlowStarted,
            detail: serde_json::json!({ "flow_id": flow_id, "run_id": run_id }),
            actor: None,
            created_at: Utc::now(),
        });
        ready(Ok(()))
    }

    fn record_true_condition(
        &self,
        _: &str,
        inp: TrueConditionInput,
    ) -> impl Future<Output = Result<TrueConditionOutcome, AppError>> + Send {
        let mut g = self.lock_st();
        let outcome = if let Some(ex) = g
            .alerts
            .values_mut()
            .find(|a| a.dedup_key == inp.dedup_key && a.status != AlertStatus::Resolved)
        {
            ex.occurrences = ex.occurrences.saturating_add(1);
            ex.last_seen_at = inp.now;
            ex.last_false_at = None;
            TrueConditionOutcome::RetriggeredActive(ex.clone())
        } else {
            let rec = AlertRecord {
                id: Uuid::new_v4(),
                rule_id: inp.rule_id,
                dedup_key: inp.dedup_key,
                severity: inp.severity,
                status: AlertStatus::Open,
                title: inp.title,
                body: inp.body,
                node_id: inp.node_id,
                dataset_id: inp.dataset_id,
                last_value: inp.last_value,
                detail: inp.detail,
                occurrences: 1,
                first_seen_at: inp.now,
                last_seen_at: inp.now,
                last_false_at: None,
                acked_by: None,
                acked_at: None,
                escalated_at: None,
                resolved_at: None,
                resolve_reason: None,
                deliveries: Vec::new(),
            };
            g.alerts.insert(rec.id, rec.clone());
            TrueConditionOutcome::Opened(rec)
        };
        ready(Ok(outcome))
    }

    fn record_false_condition(
        &self,
        _: &str,
        dedup_key: &str,
        resolve_after: i32,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<FalseConditionOutcome, AppError>> + Send {
        let mut g = self.lock_st();
        let outcome = if let Some(ex) = g
            .alerts
            .values_mut()
            .find(|a| a.dedup_key == dedup_key && a.status != AlertStatus::Resolved)
        {
            let false_since = *ex.last_false_at.get_or_insert(now);
            if now - false_since >= ChronoDuration::minutes(i64::from(resolve_after)) {
                ex.status = AlertStatus::Resolved;
                ex.resolved_at = Some(now);
                ex.resolve_reason = Some(AlertResolveReason::Auto);
                FalseConditionOutcome::AutoResolved(ex.clone())
            } else {
                FalseConditionOutcome::PendingAutoResolve(ex.clone())
            }
        } else {
            FalseConditionOutcome::NoActiveAlert
        };
        ready(Ok(outcome))
    }

    fn escalate_due_alerts(
        &self,
        _: &str,
        rule_id: Uuid,
        esc_mins: i32,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, AppError>> + Send {
        let mut g = self.lock_st();
        let mut out = Vec::new();
        for a in g.alerts.values_mut() {
            if a.rule_id == rule_id
                && a.status == AlertStatus::Open
                && a.escalated_at.is_none()
                && now - a.first_seen_at >= ChronoDuration::minutes(i64::from(esc_mins))
            {
                a.escalated_at = Some(now);
                out.push(a.clone());
            }
        }
        ready(Ok(out))
    }

    fn ack_alert(
        &self,
        _: &str,
        _: Uuid,
        _: Option<Uuid>,
        _: &str,
    ) -> impl Future<Output = Result<AlertRecord, AppError>> + Send {
        ready(unimpl())
    }

    fn ack_alerts_batch(
        &self,
        _: &str,
        _: &[Uuid],
        _: Option<Uuid>,
        _: &str,
    ) -> impl Future<Output = Result<AckBatchResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn resolve_alert_manual(
        &self,
        _: &str,
        _: Uuid,
        _: Option<Uuid>,
        _: &str,
    ) -> impl Future<Output = Result<AlertRecord, AppError>> + Send {
        ready(unimpl())
    }

    fn list_channels(
        &self,
        _: &str,
        _: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<NotifyChannelRecord>, AppError>> + Send {
        ready(unimpl())
    }

    fn get_channel(
        &self,
        _: &str,
        id: Uuid,
    ) -> impl Future<
        Output = Result<Option<(NotifyChannelRecord, Option<SealedCredential>)>, AppError>,
    > + Send {
        let res = self.lock_st().channels.get(&id).cloned().map(|c| (c, None));
        ready(Ok(res))
    }

    fn create_channel(
        &self,
        _: &str,
        _: SaveNotifyChannelInput,
    ) -> impl Future<Output = Result<NotifyChannelRecord, AppError>> + Send {
        ready(unimpl())
    }

    fn update_channel(
        &self,
        _: &str,
        _: Uuid,
        _: SaveNotifyChannelInput,
    ) -> impl Future<Output = Result<Option<NotifyChannelRecord>, AppError>> + Send {
        ready(unimpl())
    }

    fn delete_channel(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send {
        ready(unimpl())
    }

    fn enqueue_delivery(
        &self,
        _: &str,
        inp: EnqueueDeliveryInput,
    ) -> impl Future<Output = Result<(NotifyDeliveryRecord, bool), AppError>> + Send {
        let mut g = self.lock_st();
        let res = if let Some(aid) = inp.alert_id
            && let Some(ex) = g.deliveries.values().find(|d| {
                d.alert_id == Some(aid) && d.channel_id == inp.channel_id && d.event == inp.event
            }) {
            (ex.clone(), true)
        } else {
            let rec = NotifyDeliveryRecord {
                id: Uuid::new_v4(),
                alert_id: inp.alert_id,
                channel_id: inp.channel_id,
                event: inp.event,
                status: NotifyDeliveryStatus::Pending,
                attempt: 0,
                response_code: None,
                error_message: None,
                next_retry_at: inp.now,
                created_at: inp.now,
                sent_at: None,
            };
            g.deliveries.insert(rec.id, rec.clone());
            (rec, false)
        };
        ready(Ok(res))
    }

    fn get_delivery(
        &self,
        _: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<NotifyDeliveryRecord>, AppError>> + Send {
        ready(Ok(self.lock_st().deliveries.get(&id).cloned()))
    }

    fn mark_delivery_sent(
        &self,
        _: &str,
        id: Uuid,
        attempt: i32,
        code: Option<i32>,
    ) -> impl Future<Output = Result<NotifyDeliveryRecord, AppError>> + Send {
        let mut g = self.lock_st();
        let res = if let Some(d) = g.deliveries.get_mut(&id) {
            d.status = NotifyDeliveryStatus::Sent;
            d.attempt = attempt;
            d.response_code = code;
            d.sent_at = Some(Utc::now());
            Ok(d.clone())
        } else {
            Err(AppError::NotFound("DELIVERY_NOT_FOUND"))
        };
        ready(res)
    }

    fn mark_delivery_failed(
        &self,
        _: &str,
        id: Uuid,
        upd: DeliveryFailureUpdate,
    ) -> impl Future<Output = Result<NotifyDeliveryRecord, AppError>> + Send {
        let mut g = self.lock_st();
        let res = if let Some(d) = g.deliveries.get_mut(&id) {
            d.attempt = upd.attempt;
            d.response_code = upd.response_code;
            d.error_message = Some(upd.error_message);
            if let Some(ts) = upd.next_retry_at {
                d.next_retry_at = ts;
            } else {
                d.status = NotifyDeliveryStatus::Failed;
            }
            Ok(d.clone())
        } else {
            Err(AppError::NotFound("DELIVERY_NOT_FOUND"))
        };
        ready(res)
    }

    fn list_silences(
        &self,
        _: &str,
        _: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertSilenceRecord>, AppError>> + Send {
        ready(unimpl())
    }

    fn list_active_silences(
        &self,
        _: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<AlertSilenceRecord>, AppError>> + Send {
        let out = self
            .lock_st()
            .silences
            .iter()
            .filter(|s| s.starts_at <= at && at < s.ends_at)
            .cloned()
            .collect();
        ready(Ok(out))
    }

    fn create_silence(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: CreateSilenceRequest,
    ) -> impl Future<Output = Result<AlertSilenceRecord, AppError>> + Send {
        ready(unimpl())
    }

    fn delete_silence(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> + Send {
        ready(unimpl())
    }

    fn query_raw_metric_points(
        &self,
        _: &str,
        nid: Uuid,
        metric: &str,
        since: Option<DateTime<Utc>>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(DateTime<Utc>, f64)>, AppError>> + Send {
        let g = self.lock_st();
        let cap = usize::try_from(limit).unwrap_or(500);
        let out = g
            .raw_points
            .get(&(nid, String::from(metric)))
            .into_iter()
            .flatten()
            .copied()
            .filter(|(ts, _)| since.is_none_or(|s| *ts >= s))
            .take(cap)
            .collect();
        ready(Ok(out))
    }

    fn get_latest_dataset_quality(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Option<(Dataset, Option<QualityReport>)>, AppError>> + Send
    {
        ready(Ok(None))
    }

    fn list_absence_rules(
        &self,
    ) -> impl Future<Output = Result<Vec<AbsenceRuleItem>, AppError>> + Send {
        let out = self
            .lock_st()
            .rules
            .keys()
            .map(|&rule_id| AbsenceRuleItem {
                tenant_id: String::from("t1"),
                rule_id,
            })
            .collect();
        ready(Ok(out))
    }

    fn list_due_deliveries(
        &self,
        _: i32,
    ) -> impl Future<Output = Result<Vec<DueDeliveryItem>, AppError>> + Send {
        let out = self
            .lock_st()
            .deliveries
            .values()
            .filter(|d| d.status == NotifyDeliveryStatus::Pending)
            .map(|d| DueDeliveryItem {
                tenant_id: String::from("t1"),
                delivery_id: d.id,
            })
            .collect();
        ready(Ok(out))
    }
}

impl GraphStorePort for MemStore {
    fn list_nodes(
        &self,
        _: &str,
        q: NodeListQuery,
    ) -> impl Future<Output = Result<NodeListResponse, AppError>> + Send {
        let items: Vec<BlueNode> = self
            .lock_st()
            .nodes
            .values()
            .filter(|n| q.type_name.as_deref().is_none_or(|t| n.type_name == t))
            .cloned()
            .collect();
        ready(Ok(NodeListResponse {
            nodes: items.clone(),
            items,
            links: Vec::new(),
            next_cursor: None,
        }))
    }

    fn get_node(
        &self,
        _: &str,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<BlueNode>, AppError>> + Send {
        ready(Ok(self.lock_st().nodes.get(&id).cloned()))
    }

    fn upsert_node(
        &self,
        _: &str,
        _: UpsertBlueNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send {
        ready(unimpl())
    }

    fn patch_node(
        &self,
        _: &str,
        _: Uuid,
        _: PatchNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send {
        ready(unimpl())
    }

    fn delete_node(&self, _: &str, _: Uuid) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unimpl())
    }

    fn bulk_upsert(
        &self,
        _: &str,
        _: BulkNodesRequest,
    ) -> impl Future<Output = Result<BulkNodesResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn list_links(
        &self,
        _: &str,
        _: LinkListQuery,
    ) -> impl Future<Output = Result<Vec<LinkRecord>, AppError>> + Send {
        ready(unimpl())
    }

    fn upsert_link(
        &self,
        _: &str,
        _: UpsertLinkRequest,
    ) -> impl Future<Output = Result<LinkRecord, AppError>> + Send {
        ready(unimpl())
    }

    fn delete_link(&self, _: &str, _: Uuid) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unimpl())
    }

    fn search_nodes(
        &self,
        _: &str,
        _: NodeSearchQuery,
        _: Option<&[f32]>,
    ) -> impl Future<Output = Result<NodeSearchResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn list_node_documents(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<NodeDocumentsResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn resolve_entity_links(
        &self,
        _: &str,
        _: ResolveEntitiesRequest,
    ) -> impl Future<Output = Result<ResolveEntitiesResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn list_entity_candidates(
        &self,
        _: &str,
    ) -> impl Future<Output = Result<EntityCandidatesResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn confirm_entity_link(
        &self,
        _: &str,
        _: Uuid,
        _: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> + Send {
        ready(unimpl())
    }

    fn reject_entity_link(
        &self,
        _: &str,
        _: Uuid,
        _: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> + Send {
        ready(unimpl())
    }
}

impl JobQueuePort for MemStore {
    fn enqueue(
        &self,
        tid: &str,
        job: NewJob,
    ) -> impl Future<Output = Result<Uuid, AppError>> + Send {
        self.lock_st().jobs.push((String::from(tid), job));
        ready(Ok(Uuid::new_v4()))
    }

    fn lease(
        &self,
        _: &str,
        _: &[JobKind],
        _: u32,
        _: u32,
    ) -> impl Future<Output = Result<Vec<LeasedJob>, AppError>> + Send {
        ready(unimpl())
    }

    fn heartbeat(
        &self,
        _: &LeasedJob,
        _: u32,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unimpl())
    }

    fn complete(
        &self,
        _: &LeasedJob,
        _: serde_json::Value,
    ) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unimpl())
    }

    fn fail(&self, _: &LeasedJob, _: &str) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unimpl())
    }

    fn list_jobs(
        &self,
        _: &str,
        _: JobListQuery,
    ) -> impl Future<Output = Result<JobListResponse, AppError>> + Send {
        ready(unimpl())
    }

    fn get_job(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Option<JobRecord>, AppError>> + Send {
        ready(unimpl())
    }
}

impl FlowCredentialUnsealPort for MemStore {
    fn unseal_credential_json<'a>(
        &'a self,
        _: &'a str,
        cid: Uuid,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            Ok(self
                .lock_st()
                .secrets
                .get(&cid)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})))
        })
    }
}

impl RuleFlowLauncherPort for MemStore {
    fn launch_rule_flow<'a>(
        &'a self,
        _: &'a str,
        fid: Uuid,
        alert: &'a AlertRecord,
    ) -> BoxFuture<'a, Uuid> {
        Box::pin(async move {
            let run_id = Uuid::new_v4();
            self.lock_st().launched_flows.push((fid, alert.id));
            Ok(run_id)
        })
    }
}

pub struct MockSsrfGuard;

impl OutboundUrlGuardPort for MockSsrfGuard {
    fn check_outbound_url<'a>(&'a self, url: &'a str, allow_priv: bool) -> BoxFuture<'a, String> {
        Box::pin(async move {
            if url.contains("169.254.") || (!allow_priv && is_loopback_url(url)) {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
            Ok(String::from(url))
        })
    }
}

pub async fn spawn_http_sink(
    status: u16,
    resp_json: &'static str,
    max_reqs: usize,
) -> Result<(String, CapturedRequests), AppError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AppError::Config(e.to_string()))?;
    let addr = listener
        .local_addr()
        .map_err(|e| AppError::Config(e.to_string()))?;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let cap_clone = Arc::clone(&captured);
    tokio::spawn(async move {
        for _ in 0..max_reqs {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0_u8; 16_384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let raw = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned();
            if let Some((hdrs, body)) = raw.split_once("\r\n\r\n") {
                cap_clone
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((hdrs.to_ascii_lowercase(), String::from(body)));
            }
            let reply = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_json}",
                resp_json.len()
            );
            if let Err(err) = sock.write_all(reply.as_bytes()).await {
                tracing::warn!(error = %err, "mock sink write failed");
            }
        }
    });
    Ok((format!("http://{addr}/hook"), captured))
}

pub fn make_channel(
    id: Uuid,
    kind: NotifyKind,
    config: serde_json::Value,
    credential_id: Option<Uuid>,
    allow_private_network: bool,
) -> NotifyChannelRecord {
    NotifyChannelRecord {
        id,
        name: format!("ch-{id}"),
        kind,
        config,
        credential_id,
        secret_fields_set: Vec::new(),
        allow_private_network,
        enabled: true,
        created_at: Utc::now(),
    }
}

pub fn make_warehouse_node(
    id: Uuid,
    title: &str,
    inventory: f64,
    props: serde_json::Map<String, serde_json::Value>,
    now: DateTime<Utc>,
) -> BlueNode {
    BlueNode {
        id,
        type_name: String::from("Warehouse"),
        node_key: String::from("W-1"),
        title: String::from(title),
        properties: props,
        metrics: BTreeMap::from([(String::from("inventory"), inventory)]),
        geo: None,
        geo_shape: None,
        source_dataset_id: None,
        source_row_key: None,
        row_version: 1,
        created_at: now,
        updated_at: now,
    }
}

pub fn make_metric_rule(
    id: Uuid,
    condition: Condition,
    channel_id: Uuid,
    flow_id: Option<Uuid>,
    now: DateTime<Utc>,
) -> RuleDef {
    RuleDef {
        id,
        name: String::from("低库存"),
        description: String::new(),
        scope_kind: RuleScopeKind::Metric,
        target: serde_json::json!({ "type_name": "Warehouse", "metric": "inventory" }),
        condition,
        severity: Severity::Critical,
        title_template: String::from("{{node.title}}:{{value}}"),
        body_template: String::from("th={{threshold}}"),
        cooldown_minutes: 0,
        resolve_after_minutes: 0,
        escalate_after_minutes: Some(1),
        channel_ids: vec![channel_id],
        escalation_channel_ids: vec![channel_id],
        run_flow_id: flow_id,
        enabled: true,
        version: 1,
        created_by: None,
        created_at: now,
        updated_at: now,
    }
}
