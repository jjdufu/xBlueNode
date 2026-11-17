//! `JobKind::RuleEval` job worker, `ax_outbox` rule trigger subscriber (`node.metrics_changed`,
//! `dataset.synced`, and `EVENT` scope), and 60s `AbsenceRuleScanner` (`07-STEP07-08` §08.3.1).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::axhub::QualityReport;
use axpoint_domain::enums::{JobKind, OutboxTopic};
use axpoint_domain::platform::{
    BoxFuture, JobHandler, LeasedJob, NewJob, OutboxEvent, OutboxSubscriber,
};
use axpoint_domain::ports::{GraphStorePort, JobQueuePort, RulesRepository};
use axpoint_domain::rules::{
    Condition, ParsedRuleTarget, RuleDef, RuleEvalJobPayload, RuleScopeKind,
};
use axpoint_domain::topology::BlueNode;
use chrono::{DateTime, Utc};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use super::evaluator::RuleEvaluator;

/// `JobKind::RuleEval` worker adapter delegating leased `ax_jobs` rows to [`RuleEvaluator::evaluate_job`].
pub struct RuleEvalJobHandler<R, G = R> {
    evaluator: Arc<RuleEvaluator<R, G>>,
}

impl<R, G> RuleEvalJobHandler<R, G> {
    /// Constructs a new [`RuleEvalJobHandler`] wrapping `evaluator`.
    #[must_use]
    pub const fn new(evaluator: Arc<RuleEvaluator<R, G>>) -> Self {
        Self { evaluator }
    }
}

impl<R, G> JobHandler for RuleEvalJobHandler<R, G>
where
    R: RulesRepository + 'static,
    G: GraphStorePort + 'static,
{
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let payload: RuleEvalJobPayload =
                serde_json::from_value(job.payload.clone()).map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid RuleEvalJobPayload: {err}"),
                    )
                })?;
            self.evaluator.evaluate_job(&job.tenant_id, &payload).await
        })
    }
}

/// Outbox subscriber enqueueing `JobKind::RuleEval` jobs on `node.metrics_changed`, `dataset.synced`,
/// and matching `EVENT`-scope outbox topics (`07-STEP07-08` §08.3.1).
pub struct RuleTriggerSubscriber<R, J = R, G = R> {
    rules_repo: Arc<R>,
    job_queue: Arc<J>,
    graph_store: Arc<G>,
}

impl<S> RuleTriggerSubscriber<S, S, S>
where
    S: RulesRepository + JobQueuePort + GraphStorePort + 'static,
{
    /// Constructs a [`RuleTriggerSubscriber`] backed by a unified repository `store`.
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        let jq = Arc::clone(&store);
        let gs = Arc::clone(&store);
        Self::with_ports(store, jq, gs)
    }
}

impl<R, J, G> RuleTriggerSubscriber<R, J, G>
where
    R: RulesRepository + 'static,
    J: JobQueuePort + 'static,
    G: GraphStorePort + 'static,
{
    /// Constructs a [`RuleTriggerSubscriber`] from explicit port implementations.
    #[must_use]
    pub const fn with_ports(rules_repo: Arc<R>, job_queue: Arc<J>, graph_store: Arc<G>) -> Self {
        Self {
            rules_repo,
            job_queue,
            graph_store,
        }
    }

    /// Inspects `event` and enqueues deduplicated `RULE_EVAL` jobs (`<rule_id>:<event.id>`),
    /// returning the number of jobs enqueued.
    ///
    /// # Errors
    /// Returns [`AppError`] if loading enabled rules, resolving nodes, or enqueueing jobs fails.
    pub async fn dispatch_event(&self, event: &OutboxEvent) -> Result<usize, AppError> {
        let mut enqueued: usize = 0;

        if event.topic == OutboxTopic::NodeMetricsChanged {
            enqueued = enqueued.saturating_add(self.handle_metrics_changed(event).await?);
        } else if event.topic == OutboxTopic::DatasetSynced {
            enqueued = enqueued.saturating_add(self.handle_dataset_synced(event).await?);
        }

        enqueued = enqueued.saturating_add(self.handle_event_rules(event).await?);
        Ok(enqueued)
    }

    async fn handle_metrics_changed(&self, event: &OutboxEvent) -> Result<usize, AppError> {
        let node_ids = extract_node_ids(&event.payload);
        if node_ids.is_empty() {
            return Ok(0);
        }
        let rules = self
            .rules_repo
            .list_enabled_rules_by_scope(&event.tenant_id, RuleScopeKind::Metric)
            .await?;
        if rules.is_empty() {
            return Ok(0);
        }

        let changed_metrics = extract_string_list(&event.payload, "metrics");
        let mut node_cache: HashMap<Uuid, BlueNode> = HashMap::with_capacity(node_ids.len());
        for &nid in &node_ids {
            if let Some(node) = self.graph_store.get_node(&event.tenant_id, nid).await? {
                node_cache.insert(nid, node);
            }
        }

        let mut count: usize = 0;
        for rule in &rules {
            let Ok(ParsedRuleTarget::Metric(target)) =
                ParsedRuleTarget::parse(RuleScopeKind::Metric, &rule.target)
            else {
                continue;
            };
            if !changed_metrics.is_empty()
                && !matches!(rule.condition, Condition::Expr { .. })
                && !changed_metrics.iter().any(|m| m == &target.metric)
            {
                continue;
            }
            let matched_nodes: Vec<Uuid> = node_ids
                .iter()
                .copied()
                .filter(|nid| {
                    node_cache
                        .get(nid)
                        .is_some_and(|n| n.type_name == target.type_name)
                })
                .collect();
            if matched_nodes.is_empty() {
                continue;
            }
            self.enqueue_rule_eval(
                &event.tenant_id,
                RuleEvalJobPayload {
                    rule_id: rule.id,
                    node_ids: matched_nodes,
                    ..RuleEvalJobPayload::default()
                },
                format!("{}:{}", rule.id, event.id),
            )
            .await?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    async fn handle_dataset_synced(&self, event: &OutboxEvent) -> Result<usize, AppError> {
        let Some(dataset_id) = event
            .payload
            .get("dataset_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            return Ok(0);
        };
        let rules = self
            .rules_repo
            .list_enabled_rules_by_scope(&event.tenant_id, RuleScopeKind::DatasetQuality)
            .await?;
        if rules.is_empty() {
            return Ok(0);
        }
        let quality: Option<QualityReport> = event
            .payload
            .get("quality")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let mut count: usize = 0;
        for rule in &rules {
            let Ok(ParsedRuleTarget::DatasetQuality(target)) =
                ParsedRuleTarget::parse(RuleScopeKind::DatasetQuality, &rule.target)
            else {
                continue;
            };
            if target.dataset_id.is_some_and(|tid| tid != dataset_id) {
                continue;
            }
            self.enqueue_rule_eval(
                &event.tenant_id,
                RuleEvalJobPayload {
                    rule_id: rule.id,
                    dataset_id: Some(dataset_id),
                    quality: quality.clone(),
                    ..RuleEvalJobPayload::default()
                },
                format!("{}:{}", rule.id, event.id),
            )
            .await?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    async fn handle_event_rules(&self, event: &OutboxEvent) -> Result<usize, AppError> {
        let rules = self
            .rules_repo
            .list_enabled_rules_by_scope(&event.tenant_id, RuleScopeKind::Event)
            .await?;
        if rules.is_empty() {
            return Ok(0);
        }

        let origin_rule_id = event
            .payload
            .get("rule_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok());

        let mut count: usize = 0;
        for rule in &rules {
            if is_self_originated_rule_event(rule, origin_rule_id) {
                continue;
            }
            let Ok(ParsedRuleTarget::Event(target)) =
                ParsedRuleTarget::parse(RuleScopeKind::Event, &rule.target)
            else {
                continue;
            };
            if target.topic.trim() != event.topic.as_str() {
                continue;
            }
            self.enqueue_rule_eval(
                &event.tenant_id,
                RuleEvalJobPayload {
                    rule_id: rule.id,
                    event_topic: Some(String::from(event.topic.as_str())),
                    event_payload: Some(event.payload.clone()),
                    event_key: Some(event.id.to_string()),
                    ..RuleEvalJobPayload::default()
                },
                format!("{}:{}", rule.id, event.id),
            )
            .await?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    async fn enqueue_rule_eval(
        &self,
        tenant_id: &str,
        payload: RuleEvalJobPayload,
        idem_key: String,
    ) -> Result<Uuid, AppError> {
        let json_payload = serde_json::to_value(&payload)
            .map_err(|err| AppError::Config(format!("serialize RuleEvalJobPayload: {err}")))?;
        let mut job = NewJob::new(JobKind::RuleEval, json_payload).with_idempotency_key(idem_key);
        job.priority = 20;
        job.max_attempts = 3;
        self.job_queue.enqueue(tenant_id, job).await
    }
}

impl<R, J, G> OutboxSubscriber for RuleTriggerSubscriber<R, J, G>
where
    R: RulesRepository + 'static,
    J: JobQueuePort + 'static,
    G: GraphStorePort + 'static,
{
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _ = self.dispatch_event(event).await?;
            Ok(())
        })
    }
}

/// Periodic 60-second scanner querying `ax_absence_rules()` across tenants to enqueue `lifecycle_scan`
/// `RULE_EVAL` jobs for `ABSENCE` rules, auto-resolve checks, and overdue alert escalations (`07-STEP07-08` §08.3.1).
pub struct AbsenceRuleScanner<R, J = R> {
    rules_repo: Arc<R>,
    job_queue: Arc<J>,
}

impl<S> AbsenceRuleScanner<S, S>
where
    S: RulesRepository + JobQueuePort + 'static,
{
    /// Constructs an [`AbsenceRuleScanner`] backed by a unified repository `store`.
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        let jq = Arc::clone(&store);
        Self::with_ports(store, jq)
    }
}

impl<R, J> AbsenceRuleScanner<R, J>
where
    R: RulesRepository + 'static,
    J: JobQueuePort + 'static,
{
    /// Constructs an [`AbsenceRuleScanner`] from explicit repository and job queue ports.
    #[must_use]
    pub const fn with_ports(rules_repo: Arc<R>, job_queue: Arc<J>) -> Self {
        Self {
            rules_repo,
            job_queue,
        }
    }

    /// Scans `ax_absence_rules()` once at `now_utc` and enqueues minute-deduplicated `RuleEval` jobs.
    ///
    /// # Errors
    /// Returns [`AppError`] if querying `ax_absence_rules()` or enqueueing jobs fails.
    pub async fn scan_once(&self, now_utc: DateTime<Utc>) -> Result<usize, AppError> {
        let items = self.rules_repo.list_absence_rules().await?;
        let slot = now_utc.format("%Y-%m-%dT%H:%MZ");
        let mut count: usize = 0;

        for item in items {
            let payload = RuleEvalJobPayload {
                rule_id: item.rule_id,
                lifecycle_scan: true,
                ..RuleEvalJobPayload::default()
            };
            let json_payload = serde_json::to_value(&payload)
                .map_err(|err| AppError::Config(format!("serialize RuleEvalJobPayload: {err}")))?;
            let idem_key = format!("absence:{}:{slot}", item.rule_id);
            let mut job =
                NewJob::new(JobKind::RuleEval, json_payload).with_idempotency_key(idem_key);
            job.priority = 30;
            job.max_attempts = 3;
            let _ = self.job_queue.enqueue(&item.tenant_id, job).await?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    /// Runs the 60s absence and lifecycle scan loop until `shutdown_rx` signals `true`.
    pub async fn run_loop(
        &self,
        interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while !*shutdown_rx.borrow_and_update() {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shutdown_rx.changed() => break,
            }
            if let Err(err) = self.scan_once(Utc::now()).await {
                tracing::warn!(error = %err, "AbsenceRuleScanner tick failed");
            }
        }
    }
}

fn is_self_originated_rule_event(rule: &RuleDef, origin_rule_id: Option<Uuid>) -> bool {
    origin_rule_id == Some(rule.id)
}

fn extract_node_ids(payload: &serde_json::Value) -> Vec<Uuid> {
    let mut ids = Vec::new();
    if let Some(arr) = payload
        .get("node_ids")
        .and_then(serde_json::Value::as_array)
    {
        for item in arr {
            if let Some(uid) = item.as_str().and_then(|s| Uuid::parse_str(s).ok())
                && !ids.contains(&uid)
            {
                ids.push(uid);
            }
        }
    }
    if let Some(single) = payload
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        && !ids.contains(&single)
    {
        ids.push(single);
    }
    ids
}

fn extract_string_list(payload: &serde_json::Value, key: &str) -> Vec<String> {
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
