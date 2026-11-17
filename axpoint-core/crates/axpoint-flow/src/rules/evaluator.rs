//! Operational rule evaluator (`dry-run`, job evaluation, alert state transitions, `run_flow_id` launch,
//! and escalation scanning) (`07-STEP07-08` §08.3).

use std::collections::HashSet;
use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, TriggerType};
use axpoint_domain::expr::parse as parse_expr;
use axpoint_domain::flow::StartRunParams;
use axpoint_domain::graph::NodeListQuery;
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{CredentialStore, FlowRepository, GraphStorePort, RulesRepository};
use axpoint_domain::rules::{
    AlertEventKind, AlertRecord, CreateRuleRequest, DatasetQualityRuleTarget, EnqueueDeliveryInput,
    EventRuleTarget, FalseConditionOutcome, MetricRuleTarget, NotifyDeliveryEvent,
    ParsedRuleTarget, RuleDef, RuleDryRunHit, RuleDryRunResponse, RuleEvalJobPayload,
    RuleScopeKind, TrueConditionInput, TrueConditionOutcome,
};
use axpoint_domain::topology::BlueNode;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::engine::FlowEngine;
use crate::notify::AlertEscalationNotifier;
use crate::notify::template::{AlertTemplateVars, render_alert_template};

use super::condition::{
    ConditionOutcome, eval_event_condition, eval_metric_condition_for_node, eval_node_filter,
    eval_quality_condition,
};

/// Object-safe port launching a `PROD` `AxFlow` deployment when a rule alert newly opens (`07-STEP07-08` §08.3.2).
pub trait RuleFlowLauncherPort: Send + Sync {
    /// Starts `flow_id` in `Environment::Prod` (`trigger_source = TriggerType::Rule`, `idempotency_key = "alert:<alert_id>"`),
    /// returning the created or deduplicated `run_id`.
    fn launch_rule_flow<'a>(
        &'a self,
        tenant_id: &'a str,
        flow_id: Uuid,
        alert: &'a AlertRecord,
    ) -> BoxFuture<'a, Uuid>;
}

/// [`RuleFlowLauncherPort`] implementation backed by [`FlowRepository::start_flow_run`].
pub struct FlowEngineLauncher<F> {
    flow_repo: Arc<F>,
}

impl<F> FlowEngineLauncher<F> {
    /// Constructs a [`FlowEngineLauncher`] wrapping `flow_repo`.
    #[must_use]
    pub const fn new(flow_repo: Arc<F>) -> Self {
        Self { flow_repo }
    }
}

impl<F> FlowEngineLauncher<F>
where
    F: FlowRepository + CredentialStore + 'static,
{
    /// Constructs a [`FlowEngineLauncher`] sharing the repository inside `engine`.
    #[must_use]
    pub fn from_engine(engine: &FlowEngine<F>) -> Self {
        Self {
            flow_repo: Arc::clone(engine.repo_arc()),
        }
    }
}

impl<F> RuleFlowLauncherPort for FlowEngineLauncher<F>
where
    F: FlowRepository + 'static,
{
    fn launch_rule_flow<'a>(
        &'a self,
        tenant_id: &'a str,
        flow_id: Uuid,
        alert: &'a AlertRecord,
    ) -> BoxFuture<'a, Uuid> {
        Box::pin(async move {
            let payload = serde_json::json!({
                "alert_id": alert.id,
                "rule_id": alert.rule_id,
                "node_id": alert.node_id,
                "dataset_id": alert.dataset_id,
                "value": alert.last_value,
            });
            let (run, _existing) = self
                .flow_repo
                .start_flow_run(
                    tenant_id,
                    StartRunParams {
                        flow_id,
                        environment: Environment::Prod,
                        explicit_version: None,
                        trigger_source: TriggerType::Rule,
                        trigger_payload: payload,
                        idempotency_key: Some(format!("alert:{}", alert.id)),
                        started_by: None,
                    },
                )
                .await?;
            Ok(run.id)
        })
    }
}

#[derive(Default)]
struct EvalStats {
    opened: usize,
    retriggered: usize,
    resolved: usize,
    escalated: usize,
}

/// Operational rule evaluation service supporting read-only `dry-run` and stateful `RULE_EVAL` jobs.
pub struct RuleEvaluator<R, G = R> {
    rules_repo: Arc<R>,
    graph_store: Arc<G>,
    flow_launcher: Option<Arc<dyn RuleFlowLauncherPort>>,
    escalation_notifier: Option<Arc<dyn AlertEscalationNotifier>>,
}

impl<S> RuleEvaluator<S, S>
where
    S: RulesRepository + GraphStorePort + 'static,
{
    /// Constructs a [`RuleEvaluator`] using a unified repository `store`.
    #[must_use]
    pub fn new(store: Arc<S>, flow_launcher: Option<Arc<dyn RuleFlowLauncherPort>>) -> Self {
        let graph_store = Arc::clone(&store);
        Self::with_stores(store, graph_store, flow_launcher)
    }
}

impl<R, G> RuleEvaluator<R, G>
where
    R: RulesRepository + 'static,
    G: GraphStorePort + 'static,
{
    /// Constructs a [`RuleEvaluator`] from distinct rules and graph store ports.
    #[must_use]
    pub fn with_stores(
        rules_repo: Arc<R>,
        graph_store: Arc<G>,
        flow_launcher: Option<Arc<dyn RuleFlowLauncherPort>>,
    ) -> Self {
        Self {
            rules_repo,
            graph_store,
            flow_launcher,
            escalation_notifier: None,
        }
    }

    /// Attaches an [`AlertEscalationNotifier`] for immediate escalation delivery during `lifecycle_scan`.
    #[must_use]
    pub fn with_escalation_notifier(mut self, notifier: Arc<dyn AlertEscalationNotifier>) -> Self {
        self.escalation_notifier = Some(notifier);
        self
    }

    /// Executes a non-persisted `dry-run` preview on a saved rule (`POST /api/axflow/rules/{id}/dry-run`).
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] if `rule_id` does not exist or [`AppError`] on query/expression errors.
    pub async fn dry_run_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
    ) -> Result<RuleDryRunResponse, AppError> {
        let rule = self
            .rules_repo
            .get_rule(tenant_id, rule_id)
            .await?
            .ok_or(AppError::NotFound("RULE_NOT_FOUND"))?;
        self.dry_run_def(tenant_id, &rule).await
    }

    /// Executes a non-persisted `dry-run` preview on an unsaved [`CreateRuleRequest`].
    ///
    /// # Errors
    /// Returns [`AppError`] if the rule target/condition is invalid or data queries fail.
    pub async fn dry_run_request(
        &self,
        tenant_id: &str,
        req: &CreateRuleRequest,
    ) -> Result<RuleDryRunResponse, AppError> {
        let now = Utc::now();
        let ephemeral = RuleDef {
            id: Uuid::nil(),
            name: req.name.clone(),
            description: req.description.clone(),
            scope_kind: req.scope_kind,
            target: req.target.clone(),
            condition: req.condition.clone(),
            severity: req.severity,
            title_template: req.title_template.clone(),
            body_template: req.body_template.clone(),
            cooldown_minutes: req.cooldown_minutes,
            resolve_after_minutes: req.resolve_after_minutes,
            escalate_after_minutes: req.escalate_after_minutes,
            channel_ids: req.channel_ids.clone(),
            escalation_channel_ids: req.escalation_channel_ids.clone(),
            run_flow_id: req.run_flow_id,
            enabled: req.enabled,
            version: 1,
            created_by: None,
            created_at: now,
            updated_at: now,
        };
        self.dry_run_def(tenant_id, &ephemeral).await
    }

    /// Evaluates `rule` against live tenant data without mutating alerts or sending notifications.
    ///
    /// # Errors
    /// Returns [`AppError`] if target parsing, expression evaluation, or repository queries fail.
    pub async fn dry_run_def(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
    ) -> Result<RuleDryRunResponse, AppError> {
        let now = Utc::now();
        let parsed = ParsedRuleTarget::parse(rule.scope_kind, &rule.target)?;
        match parsed {
            ParsedRuleTarget::Metric(target) => {
                self.dry_run_metric(tenant_id, rule, &target, now).await
            }
            ParsedRuleTarget::DatasetQuality(target) => {
                self.dry_run_dataset(tenant_id, rule, &target).await
            }
            ParsedRuleTarget::Event(target) => {
                if let Some(filter) = target.filter.as_deref().filter(|s| !s.trim().is_empty()) {
                    let _ = parse_expr(filter)
                        .map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;
                }
                Ok(RuleDryRunResponse {
                    rule_id: rule.id,
                    evaluated_objects: 0,
                    matched_count: 0,
                    would_trigger: Vec::new(),
                })
            }
        }
    }

    /// Evaluates a background [`RuleEvalJobPayload`], persisting alert transitions, launching linked `PROD` flows,
    /// and escalating overdue unacknowledged alerts on `lifecycle_scan`.
    ///
    /// # Errors
    /// Returns [`AppError`] if storage queries or state transitions fail.
    pub async fn evaluate_job(
        &self,
        tenant_id: &str,
        payload: &RuleEvalJobPayload,
    ) -> Result<serde_json::Value, AppError> {
        let Some(rule) = self.rules_repo.get_rule(tenant_id, payload.rule_id).await? else {
            return Ok(serde_json::json!({ "skipped": true, "reason": "rule_not_found" }));
        };
        if !rule.enabled {
            return Ok(serde_json::json!({ "skipped": true, "reason": "rule_disabled" }));
        }

        let now = Utc::now();
        let mut stats = EvalStats::default();
        match ParsedRuleTarget::parse(rule.scope_kind, &rule.target)? {
            ParsedRuleTarget::Metric(target) => {
                self.eval_metric_job(tenant_id, &rule, &target, payload, now, &mut stats)
                    .await?;
            }
            ParsedRuleTarget::DatasetQuality(target) => {
                self.eval_dataset_job(tenant_id, &rule, &target, payload, now, &mut stats)
                    .await?;
            }
            ParsedRuleTarget::Event(target) => {
                self.eval_event_job(tenant_id, &rule, &target, payload, now, &mut stats)
                    .await?;
            }
        }

        if payload.lifecycle_scan
            && let Some(esc_mins) = rule.escalate_after_minutes.filter(|&m| m > 0)
        {
            let escalated = self
                .rules_repo
                .escalate_due_alerts(tenant_id, rule.id, esc_mins, now)
                .await?;
            stats.escalated = escalated.len();
            for alert in &escalated {
                self.dispatch_or_enqueue_escalation(tenant_id, alert, &rule, now)
                    .await?;
            }
        }

        Ok(serde_json::json!({
            "rule_id": rule.id,
            "opened": stats.opened,
            "retriggered": stats.retriggered,
            "resolved": stats.resolved,
            "escalated": stats.escalated,
        }))
    }

    async fn dry_run_metric(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        target: &MetricRuleTarget,
        now: DateTime<Utc>,
    ) -> Result<RuleDryRunResponse, AppError> {
        let nodes = self
            .list_nodes_of_type(tenant_id, &target.type_name)
            .await?;
        let mut evaluated_objects: usize = 0;
        let mut hits = Vec::new();

        for node in &nodes {
            if let Some(f) = target.node_filter.as_deref()
                && !eval_node_filter(f, node)?
            {
                continue;
            }
            evaluated_objects = evaluated_objects.saturating_add(1);
            let outcome = eval_metric_condition_for_node(
                &*self.rules_repo,
                tenant_id,
                node,
                &target.metric,
                &rule.condition,
                now,
            )
            .await?;
            if outcome.matched {
                let (title, body, detail) =
                    render_metric_alert_fields(rule, node, &target.metric, &outcome);
                hits.push(RuleDryRunHit {
                    dedup_key: format!("{}:{}", rule.id, node.id),
                    severity: rule.severity,
                    title,
                    body,
                    node_id: Some(node.id),
                    dataset_id: None,
                    value: outcome.value,
                    detail,
                });
            }
        }

        Ok(RuleDryRunResponse {
            rule_id: rule.id,
            evaluated_objects,
            matched_count: hits.len(),
            would_trigger: hits,
        })
    }

    async fn dry_run_dataset(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        target: &DatasetQualityRuleTarget,
    ) -> Result<RuleDryRunResponse, AppError> {
        let mut ds_ids = Vec::new();
        if let Some(ds_id) = target.dataset_id {
            ds_ids.push(ds_id);
        } else if rule.id != Uuid::nil() {
            for active in self
                .rules_repo
                .list_active_alerts_for_rule(tenant_id, rule.id)
                .await?
            {
                if let Some(ds_id) = active.dataset_id
                    && !ds_ids.contains(&ds_id)
                {
                    ds_ids.push(ds_id);
                }
            }
        }

        let mut evaluated_objects: usize = 0;
        let mut hits = Vec::new();
        for ds_id in ds_ids {
            let Some((dataset, quality_opt)) = self
                .rules_repo
                .get_latest_dataset_quality(tenant_id, ds_id)
                .await?
            else {
                continue;
            };
            evaluated_objects = evaluated_objects.saturating_add(1);
            let outcome = eval_quality_condition(&rule.condition, quality_opt.as_ref())?;
            if outcome.matched {
                let (title, body, detail) =
                    render_dataset_alert_fields(rule, ds_id, Some(&dataset.name), &outcome);
                hits.push(RuleDryRunHit {
                    dedup_key: format!("{}:{ds_id}", rule.id),
                    severity: rule.severity,
                    title,
                    body,
                    node_id: None,
                    dataset_id: Some(ds_id),
                    value: outcome.value,
                    detail,
                });
            }
        }

        Ok(RuleDryRunResponse {
            rule_id: rule.id,
            evaluated_objects,
            matched_count: hits.len(),
            would_trigger: hits,
        })
    }

    async fn eval_metric_job(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        target: &MetricRuleTarget,
        payload: &RuleEvalJobPayload,
        now: DateTime<Utc>,
        stats: &mut EvalStats,
    ) -> Result<(), AppError> {
        let nodes = if payload.node_ids.is_empty() {
            self.list_nodes_of_type(tenant_id, &target.type_name)
                .await?
        } else {
            let mut subset = Vec::with_capacity(payload.node_ids.len());
            for &nid in &payload.node_ids {
                if let Some(n) = self.graph_store.get_node(tenant_id, nid).await?
                    && n.type_name == target.type_name
                {
                    subset.push(n);
                }
            }
            subset
        };

        let mut seen_ids = HashSet::with_capacity(nodes.len());
        for node in &nodes {
            seen_ids.insert(node.id);
            let dedup_key = format!("{}:{}", rule.id, node.id);
            if let Some(f) = target.node_filter.as_deref()
                && !eval_node_filter(f, node)?
            {
                self.apply_false_outcome(tenant_id, rule, &dedup_key, now, stats)
                    .await?;
                continue;
            }
            let outcome = eval_metric_condition_for_node(
                &*self.rules_repo,
                tenant_id,
                node,
                &target.metric,
                &rule.condition,
                now,
            )
            .await?;
            if outcome.matched {
                let (title, body, detail) =
                    render_metric_alert_fields(rule, node, &target.metric, &outcome);
                self.apply_true_outcome(
                    tenant_id,
                    rule,
                    TrueConditionInput {
                        rule_id: rule.id,
                        dedup_key,
                        severity: rule.severity,
                        title,
                        body,
                        node_id: Some(node.id),
                        dataset_id: None,
                        last_value: outcome.value,
                        detail,
                        cooldown_minutes: rule.cooldown_minutes,
                        now,
                    },
                    stats,
                )
                .await?;
            } else {
                self.apply_false_outcome(tenant_id, rule, &dedup_key, now, stats)
                    .await?;
            }
        }

        if payload.lifecycle_scan && payload.node_ids.is_empty() {
            let actives = self
                .rules_repo
                .list_active_alerts_for_rule(tenant_id, rule.id)
                .await?;
            for active in actives {
                if active.node_id.is_some_and(|nid| !seen_ids.contains(&nid)) {
                    self.apply_false_outcome(tenant_id, rule, &active.dedup_key, now, stats)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn eval_dataset_job(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        target: &DatasetQualityRuleTarget,
        payload: &RuleEvalJobPayload,
        now: DateTime<Utc>,
        stats: &mut EvalStats,
    ) -> Result<(), AppError> {
        let mut ds_ids = Vec::new();
        if let Some(ds_id) = payload.dataset_id {
            if target.dataset_id.is_none_or(|tid| tid == ds_id) {
                ds_ids.push(ds_id);
            }
        } else if payload.lifecycle_scan {
            if let Some(ds_id) = target.dataset_id {
                ds_ids.push(ds_id);
            }
            for active in self
                .rules_repo
                .list_active_alerts_for_rule(tenant_id, rule.id)
                .await?
            {
                if let Some(ds_id) = active.dataset_id
                    && !ds_ids.contains(&ds_id)
                {
                    ds_ids.push(ds_id);
                }
            }
        }

        for ds_id in ds_ids {
            let loaded = self
                .rules_repo
                .get_latest_dataset_quality(tenant_id, ds_id)
                .await?;
            let ds_name = loaded.as_ref().map(|(ds, _)| ds.name.as_str());
            let inline_or_db = payload
                .quality
                .as_ref()
                .or_else(|| loaded.as_ref().and_then(|(_, q)| q.as_ref()));
            let outcome = eval_quality_condition(&rule.condition, inline_or_db)?;
            let dedup_key = format!("{}:{ds_id}", rule.id);

            if outcome.matched {
                let (title, body, detail) =
                    render_dataset_alert_fields(rule, ds_id, ds_name, &outcome);
                self.apply_true_outcome(
                    tenant_id,
                    rule,
                    TrueConditionInput {
                        rule_id: rule.id,
                        dedup_key,
                        severity: rule.severity,
                        title,
                        body,
                        node_id: None,
                        dataset_id: Some(ds_id),
                        last_value: outcome.value,
                        detail,
                        cooldown_minutes: rule.cooldown_minutes,
                        now,
                    },
                    stats,
                )
                .await?;
            } else {
                self.apply_false_outcome(tenant_id, rule, &dedup_key, now, stats)
                    .await?;
            }
        }
        Ok(())
    }

    async fn eval_event_job(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        target: &EventRuleTarget,
        payload: &RuleEvalJobPayload,
        now: DateTime<Utc>,
        stats: &mut EvalStats,
    ) -> Result<(), AppError> {
        if let Some(topic) = payload.event_topic.as_deref() {
            let ev_payload = payload
                .event_payload
                .as_ref()
                .unwrap_or(&serde_json::Value::Null);
            let outcome = eval_event_condition(target, &rule.condition, topic, ev_payload)?;
            if outcome.matched {
                let key_suffix = payload.event_key.as_deref().unwrap_or(topic);
                let dedup_key = format!("{}:{key_suffix}", rule.id);
                let vars = AlertTemplateVars {
                    rule_name: rule.name.clone(),
                    severity: Some(rule.severity),
                    link: Some(format!("/alerts?rule_id={}", rule.id)),
                    ..AlertTemplateVars::default()
                };
                let title = render_alert_template(&rule.title_template, &vars);
                let body = render_alert_template(&rule.body_template, &vars);
                self.apply_true_outcome(
                    tenant_id,
                    rule,
                    TrueConditionInput {
                        rule_id: rule.id,
                        dedup_key,
                        severity: rule.severity,
                        title,
                        body,
                        node_id: None,
                        dataset_id: None,
                        last_value: None,
                        detail: serde_json::json!({
                            "rule_name": rule.name,
                            "topic": topic,
                            "event_payload": ev_payload,
                        }),
                        cooldown_minutes: rule.cooldown_minutes,
                        now,
                    },
                    stats,
                )
                .await?;
            }
        } else if payload.lifecycle_scan && rule.scope_kind == RuleScopeKind::Event {
            for active in self
                .rules_repo
                .list_active_alerts_for_rule(tenant_id, rule.id)
                .await?
            {
                self.apply_false_outcome(tenant_id, rule, &active.dedup_key, now, stats)
                    .await?;
            }
        }
        Ok(())
    }

    async fn apply_true_outcome(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        input: TrueConditionInput,
        stats: &mut EvalStats,
    ) -> Result<(), AppError> {
        match self
            .rules_repo
            .record_true_condition(tenant_id, input)
            .await?
        {
            TrueConditionOutcome::Opened(alert) => {
                stats.opened = stats.opened.saturating_add(1);
                if let (Some(flow_id), Some(launcher)) = (rule.run_flow_id, &self.flow_launcher) {
                    match launcher.launch_rule_flow(tenant_id, flow_id, &alert).await {
                        Ok(run_id) => {
                            self.rules_repo
                                .set_alert_flow_run(tenant_id, alert.id, flow_id, run_id)
                                .await?;
                        }
                        Err(err) => {
                            tracing::warn!(
                                tenant_id = %tenant_id,
                                alert_id = %alert.id,
                                flow_id = %flow_id,
                                error = %err,
                                "failed launching rule-linked PROD flow"
                            );
                        }
                    }
                }
            }
            TrueConditionOutcome::RetriggeredActive(_)
            | TrueConditionOutcome::SuppressedByCooldown(_) => {
                stats.retriggered = stats.retriggered.saturating_add(1);
            }
        }
        Ok(())
    }

    async fn apply_false_outcome(
        &self,
        tenant_id: &str,
        rule: &RuleDef,
        dedup_key: &str,
        now: DateTime<Utc>,
        stats: &mut EvalStats,
    ) -> Result<(), AppError> {
        if let FalseConditionOutcome::AutoResolved(_) = self
            .rules_repo
            .record_false_condition(tenant_id, dedup_key, rule.resolve_after_minutes, now)
            .await?
        {
            stats.resolved = stats.resolved.saturating_add(1);
        }
        Ok(())
    }

    async fn dispatch_or_enqueue_escalation(
        &self,
        tenant_id: &str,
        alert: &AlertRecord,
        rule: &RuleDef,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        if let Some(notifier) = &self.escalation_notifier {
            let _ = notifier.notify_escalated(tenant_id, alert, rule).await?;
            return Ok(());
        }
        if rule.escalation_channel_ids.is_empty() {
            return Ok(());
        }
        let silences = self.rules_repo.list_active_silences(tenant_id, now).await?;
        let type_name = alert
            .detail
            .get("type_name")
            .and_then(serde_json::Value::as_str);
        if let Some(sil) = silences.into_iter().find(|s| {
            s.matcher
                .matches(alert.rule_id, alert.severity, alert.node_id, type_name)
        }) {
            let _ = self
                .rules_repo
                .append_alert_event(
                    tenant_id,
                    alert.id,
                    AlertEventKind::Silenced,
                    serde_json::json!({
                        "silence_id": sil.id,
                        "event": NotifyDeliveryEvent::Escalated.as_str(),
                        "reason": sil.reason,
                    }),
                    None,
                )
                .await?;
            return Ok(());
        }
        for &channel_id in &rule.escalation_channel_ids {
            let _ = self
                .rules_repo
                .enqueue_delivery(
                    tenant_id,
                    EnqueueDeliveryInput {
                        alert_id: Some(alert.id),
                        channel_id,
                        event: NotifyDeliveryEvent::Escalated,
                        now,
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn list_nodes_of_type(
        &self,
        tenant_id: &str,
        type_name: &str,
    ) -> Result<Vec<BlueNode>, AppError> {
        let mut cursor: Option<String> = None;
        let mut all = Vec::new();
        for _ in 0..200 {
            let resp = self
                .graph_store
                .list_nodes(
                    tenant_id,
                    NodeListQuery {
                        type_name: Some(String::from(type_name)),
                        limit: Some(500),
                        cursor,
                        ..NodeListQuery::default()
                    },
                )
                .await?;
            all.extend(resp.items);
            match resp.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => break,
            }
        }
        Ok(all)
    }
}

fn render_metric_alert_fields(
    rule: &RuleDef,
    node: &BlueNode,
    metric: &str,
    outcome: &ConditionOutcome,
) -> (String, String, serde_json::Value) {
    let threshold = rule.condition.threshold_number();
    let link = format!("/alerts?rule_id={}", rule.id);
    let vars = AlertTemplateVars {
        rule_name: rule.name.clone(),
        node_title: Some(node.title.clone()),
        node_type_name: Some(node.type_name.clone()),
        node_key: Some(node.node_key.clone()),
        metric: Some(String::from(metric)),
        value: outcome.value,
        threshold,
        severity: Some(rule.severity),
        dataset_name: None,
        link: Some(link.clone()),
    };
    let title = render_alert_template(&rule.title_template, &vars);
    let body = render_alert_template(&rule.body_template, &vars);
    let detail = serde_json::json!({
        "rule_name": rule.name,
        "type_name": node.type_name,
        "node_key": node.node_key,
        "node_title": node.title,
        "metric": metric,
        "threshold": threshold,
        "link": link,
        "condition": outcome.detail,
    });
    (title, body, detail)
}

fn render_dataset_alert_fields(
    rule: &RuleDef,
    dataset_id: Uuid,
    dataset_name: Option<&str>,
    outcome: &ConditionOutcome,
) -> (String, String, serde_json::Value) {
    let link = format!("/alerts?rule_id={}", rule.id);
    let ds_label = dataset_name.map_or_else(|| dataset_id.to_string(), String::from);
    let vars = AlertTemplateVars {
        rule_name: rule.name.clone(),
        severity: Some(rule.severity),
        value: outcome.value,
        dataset_name: Some(ds_label.clone()),
        link: Some(link.clone()),
        ..AlertTemplateVars::default()
    };
    let title = render_alert_template(&rule.title_template, &vars);
    let body = render_alert_template(&rule.body_template, &vars);
    let detail = serde_json::json!({
        "rule_name": rule.name,
        "dataset_id": dataset_id,
        "dataset_name": ds_label,
        "link": link,
        "condition": outcome.detail,
    });
    (title, body, detail)
}
