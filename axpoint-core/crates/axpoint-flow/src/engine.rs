//! Persistent `AxFlow` DAG progression planner, step executor, `1 MiB` output offloading,
//! cooperative cancellation, and approval resumption (`07-STEP07-08` §07.3).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::{RunStatus, StepStatus};
use axpoint_domain::expr::{
    BinaryOp, EvalEnv, Expr, ExprValue, UnaryOp, eval as eval_expr, parse as parse_expr,
};
use axpoint_domain::flow::{
    ApprovalSourceKind, ApprovalStatus, BackoffStrategy, FlowActorPrincipal, FlowEdge, FlowGraph,
    FlowNode, FlowStepJobPayload, FlowVersionRecord, JoinMode, OnFailure, OperatorContext,
    OperatorOutcome, OperatorRegistry, RetryPolicy, RunProgressionPlan, RunRecord, StepMutation,
    StepRecord,
};
use axpoint_domain::platform::{BoxFuture, JobHandler, LeasedJob, OutboxEvent};
use axpoint_domain::ports::{CredentialStore, FlowRepository};
use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use crate::graph::{leaf_nodes, node_map, predecessors, topological_order_or_cycle};
use crate::template::{TemplateContext, render_config_value};

/// Inline PostgreSQL JSONB cap (`1 MiB`) before offloading step output to disk (`07-STEP07-08` §07.3.1).
pub const INLINE_OUTPUT_MAX_BYTES: usize = 1_048_576;

static ACTIVE_RUN_CANCEL_FLAGS: LazyLock<Mutex<HashMap<Uuid, Vec<Weak<AtomicBool>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Signals cooperative cancellation to all in-flight step executors belonging to `run_id`.
pub fn signal_run_canceled(run_id: Uuid) {
    let mut guard = ACTIVE_RUN_CANCEL_FLAGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(flags) = guard.get_mut(&run_id) {
        flags.retain(|weak| {
            if let Some(flag) = weak.upgrade() {
                flag.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        });
        if flags.is_empty() {
            guard.remove(&run_id);
        }
    }
}

struct ActiveCancelGuard {
    run_id: Uuid,
    flag: Arc<AtomicBool>,
}

impl ActiveCancelGuard {
    fn register(run_id: Uuid) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let mut guard = ACTIVE_RUN_CANCEL_FLAGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let list = guard.entry(run_id).or_default();
        list.retain(|w| w.strong_count() > 0);
        list.push(Arc::downgrade(&flag));
        Self { run_id, flag }
    }
}

impl Drop for ActiveCancelGuard {
    fn drop(&mut self) {
        let mut guard = ACTIVE_RUN_CANCEL_FLAGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(list) = guard.get_mut(&self.run_id) {
            list.retain(|w| w.strong_count() > 0 && !Weak::ptr_eq(w, &Arc::downgrade(&self.flag)));
            if list.is_empty() {
                guard.remove(&self.run_id);
            }
        }
    }
}

/// Evaluates a Step 06 boolean guard expression (`when` or `OUTBOX_EVENT` `filter`) against a JSON object.
///
/// Top-level scalar fields (`Number`, `String`, `Bool`) are exposed as bare identifiers,
/// while nested `properties.<key>` fields are also accessible via `prop.<key>`.
/// Three-valued `Null` and `Bool(false)` evaluate to `false`; only `Bool(true)` returns `true`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`EXPR_INVALID`) if `expr_str` fails to parse.
pub fn eval_json_guard_expr(
    expr_str: &str,
    json_obj: &serde_json::Value,
) -> Result<bool, AppError> {
    let ast = parse_expr(expr_str).map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;
    let rewritten = rewrite_metric_refs_to_props(ast);
    let env = build_eval_env_from_json(json_obj);
    let outcome = eval_expr(&rewritten, &env);
    Ok(outcome.value == ExprValue::Bool(true))
}

fn rewrite_metric_refs_to_props(expr: Expr) -> Expr {
    match expr {
        Expr::MetricRef(name) => Expr::PropRef(format!("__top.{name}")),
        Expr::PropRef(name) => Expr::PropRef(name),
        Expr::Literal(_) => expr,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => Expr::Unary {
            op: UnaryOp::Neg,
            expr: Box::new(rewrite_metric_refs_to_props(*inner)),
        },
        Expr::Unary {
            op: UnaryOp::Not,
            expr: inner,
        } => Expr::Unary {
            op: UnaryOp::Not,
            expr: Box::new(rewrite_metric_refs_to_props(*inner)),
        },
        Expr::Binary {
            op: bin_op,
            left,
            right,
        } => rewrite_binary(bin_op, *left, *right),
        Expr::Call { name, args } => Expr::Call {
            name,
            args: args.into_iter().map(rewrite_metric_refs_to_props).collect(),
        },
    }
}

fn rewrite_binary(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op,
        left: Box::new(rewrite_metric_refs_to_props(left)),
        right: Box::new(rewrite_metric_refs_to_props(right)),
    }
}

fn build_eval_env_from_json(json_obj: &serde_json::Value) -> EvalEnv {
    let mut properties = BTreeMap::new();
    if let Some(map) = json_obj.as_object() {
        for (k, v) in map {
            let ev = json_scalar_to_expr_value(v);
            properties.insert(format!("__top.{k}"), ev.clone());
            properties.insert(k.clone(), ev);
        }
        if let Some(first_row) = map
            .get("rows")
            .and_then(serde_json::Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(serde_json::Value::as_object)
        {
            for (col_key, col_val) in first_row {
                let ev = json_scalar_to_expr_value(col_val);
                let top_key = format!("__top.{col_key}");
                if matches!(properties.get(&top_key), None | Some(ExprValue::Null)) {
                    properties.insert(top_key, ev.clone());
                }
                if matches!(properties.get(col_key), None | Some(ExprValue::Null)) {
                    properties.insert(col_key.clone(), ev);
                }
            }
        }
        if let Some(nested_props) = map.get("properties").and_then(serde_json::Value::as_object) {
            for (pk, pv) in nested_props {
                properties.insert(pk.clone(), json_scalar_to_expr_value(pv));
            }
        }
    }
    EvalEnv {
        metrics: BTreeMap::new(),
        properties,
    }
}

fn json_scalar_to_expr_value(val: &serde_json::Value) -> ExprValue {
    match val {
        serde_json::Value::Bool(b) => ExprValue::Bool(*b),
        serde_json::Value::String(s) => ExprValue::Text(s.clone()),
        serde_json::Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map_or(ExprValue::Null, ExprValue::Number),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            ExprValue::Null
        }
    }
}

/// Pure, deterministic DAG progression planner computing ready downstream nodes, cascading `SKIPPED` nodes,
/// terminal `outputs`, and run status transitions (`07-STEP07-08` §07.3.1 & §07.3.2).
///
/// # Errors
/// Returns [`AppError`] if an edge `when` expression cannot be parsed.
pub fn plan_progression<S: std::hash::BuildHasher>(
    run: &RunRecord,
    version: &FlowVersionRecord,
    latest_steps: &[StepRecord],
    hydrated_outputs: &HashMap<String, serde_json::Value, S>,
) -> Result<RunProgressionPlan, AppError> {
    if run.status == RunStatus::Canceled {
        return Ok(RunProgressionPlan::default());
    }

    let graph = FlowGraph {
        nodes: version.nodes.clone(),
        edges: version.edges.clone(),
    };
    let nodes_by_id = node_map(&graph);

    let mut effective_status: HashMap<String, StepStatus> = HashMap::new();
    let mut step_by_node: HashMap<&str, &StepRecord> = HashMap::new();
    for step in latest_steps {
        effective_status.insert(step.node_id.clone(), step.status);
        step_by_node.insert(step.node_id.as_str(), step);
    }

    if let Some((fail_node, fail_msg)) = find_fail_run_terminal(&nodes_by_id, &step_by_node) {
        let err_msg = fail_msg.unwrap_or_else(|| format!("step '{fail_node}' failed"));
        return Ok(RunProgressionPlan {
            steps_to_enqueue: Vec::new(),
            nodes_to_skip: Vec::new(),
            next_run_status: Some(RunStatus::Failed),
            run_outputs: None,
            run_error_message: Some(err_msg),
        });
    }

    let topo_order = topological_order_or_cycle(&graph)
        .unwrap_or_else(|_| graph.nodes.iter().map(|n| n.id.clone()).collect());

    let mut steps_to_enqueue = Vec::new();
    let mut nodes_to_skip = Vec::new();

    for node_id in &topo_order {
        if effective_status.contains_key(node_id.as_str()) {
            continue;
        }
        let Some(&node) = nodes_by_id.get(node_id.as_str()) else {
            continue;
        };
        let in_edges = predecessors(&graph, node_id);
        if in_edges.is_empty() {
            steps_to_enqueue.push((node_id.clone(), 1));
            effective_status.insert(node_id.clone(), StepStatus::Pending);
            continue;
        }

        let all_preds_terminal = in_edges.iter().all(|edge| {
            effective_status
                .get(edge.from.as_str())
                .copied()
                .is_some_and(is_terminal_step_status)
        });
        if !all_preds_terminal {
            continue;
        }

        let should_run = evaluate_join_readiness(
            node,
            &in_edges,
            &effective_status,
            &step_by_node,
            hydrated_outputs,
        )?;
        if should_run {
            steps_to_enqueue.push((node_id.clone(), 1));
            effective_status.insert(node_id.clone(), StepStatus::Pending);
        } else {
            nodes_to_skip.push(node_id.clone());
            effective_status.insert(node_id.clone(), StepStatus::Skipped);
        }
    }

    let (next_run_status, run_outputs) =
        compute_run_status_and_outputs(&graph, &effective_status, &step_by_node, hydrated_outputs);

    Ok(RunProgressionPlan {
        steps_to_enqueue,
        nodes_to_skip,
        next_run_status,
        run_outputs,
        run_error_message: None,
    })
}

fn find_fail_run_terminal(
    nodes_by_id: &HashMap<&str, &FlowNode>,
    step_by_node: &HashMap<&str, &StepRecord>,
) -> Option<(String, Option<String>)> {
    for (&nid, &step) in step_by_node {
        if step.status == StepStatus::Failed
            && let Some(node) = nodes_by_id.get(nid)
            && node.on_failure == OnFailure::FailRun
        {
            return Some((String::from(nid), step.error_message.clone()));
        }
    }
    None
}

fn evaluate_join_readiness<S: std::hash::BuildHasher>(
    node: &FlowNode,
    in_edges: &[&FlowEdge],
    effective_status: &HashMap<String, StepStatus>,
    step_by_node: &HashMap<&str, &StepRecord>,
    hydrated_outputs: &HashMap<String, serde_json::Value, S>,
) -> Result<bool, AppError> {
    if node.join == JoinMode::AllDone {
        return Ok(true);
    }

    let mut active_count: usize = 0;
    for edge in in_edges {
        let pred_status = effective_status.get(edge.from.as_str()).copied();
        if pred_status != Some(StepStatus::Succeeded) {
            continue;
        }
        let active = match &edge.when {
            None => true,
            Some(when_str) => {
                let empty_obj = serde_json::json!({});
                let out_val = lookup_node_output(&edge.from, step_by_node, hydrated_outputs)
                    .unwrap_or(&empty_obj);
                eval_json_guard_expr(when_str, out_val)?
            }
        };
        if active {
            active_count = active_count.saturating_add(1);
        }
    }

    Ok(match node.join {
        JoinMode::AllSuccess => active_count == in_edges.len(),
        JoinMode::AnySuccess => active_count > 0,
        JoinMode::AllDone => true,
    })
}

fn compute_run_status_and_outputs<S: std::hash::BuildHasher>(
    graph: &FlowGraph,
    effective_status: &HashMap<String, StepStatus>,
    step_by_node: &HashMap<&str, &StepRecord>,
    hydrated_outputs: &HashMap<String, serde_json::Value, S>,
) -> (Option<RunStatus>, Option<serde_json::Value>) {
    let mut any_waiting_approval = false;
    let mut all_terminal = true;

    for node in &graph.nodes {
        match effective_status.get(node.id.as_str()).copied() {
            Some(StepStatus::WaitingApproval) => {
                any_waiting_approval = true;
                all_terminal = false;
            }
            Some(StepStatus::Pending | StepStatus::Running) | None => {
                all_terminal = false;
            }
            Some(StepStatus::Succeeded | StepStatus::Failed | StepStatus::Skipped) => {}
        }
    }

    if all_terminal {
        let mut leaf_map = serde_json::Map::new();
        for leaf in leaf_nodes(graph) {
            if effective_status.get(leaf.id.as_str()).copied() == Some(StepStatus::Succeeded) {
                let val = lookup_node_output(&leaf.id, step_by_node, hydrated_outputs)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                leaf_map.insert(leaf.id.clone(), val);
            }
        }
        return (
            Some(RunStatus::Succeeded),
            Some(serde_json::Value::Object(leaf_map)),
        );
    }

    if any_waiting_approval {
        (Some(RunStatus::WaitingApproval), None)
    } else {
        (Some(RunStatus::Running), None)
    }
}

fn lookup_node_output<'a, S: std::hash::BuildHasher>(
    node_id: &str,
    step_by_node: &'a HashMap<&str, &'a StepRecord>,
    hydrated_outputs: &'a HashMap<String, serde_json::Value, S>,
) -> Option<&'a serde_json::Value> {
    if let Some(val) = hydrated_outputs.get(node_id) {
        return Some(val);
    }
    step_by_node.get(node_id).and_then(|s| s.output.as_ref())
}

const fn is_terminal_step_status(status: StepStatus) -> bool {
    matches!(
        status,
        StepStatus::Succeeded | StepStatus::Failed | StepStatus::Skipped
    )
}

/// Computes the next retry timestamp according to `RetryPolicy` (`FIXED` or `EXPONENTIAL`, capped at `3600s`).
#[must_use]
pub fn compute_retry_backoff_secs(policy: &RetryPolicy, attempt: i32) -> i64 {
    let base = i64::from(policy.backoff_s.clamp(1, 3_600));
    match policy.backoff {
        BackoffStrategy::Fixed => base,
        BackoffStrategy::Exponential => {
            let exp = u32::try_from(attempt.saturating_sub(1).max(0))
                .unwrap_or(0)
                .min(12);
            let factor = 1_i64.checked_shl(exp).unwrap_or(3_600);
            base.saturating_mul(factor).min(3_600)
        }
    }
}

/// Persistent DAG orchestration engine coordinating `FLOW_STEP` execution, output offloading,
/// retry scheduling, cooperative cancellation, and human approval resumption.
pub struct FlowEngine<R> {
    repo: Arc<R>,
    operators: Arc<OperatorRegistry>,
    data_root: PathBuf,
}

impl<R> FlowEngine<R>
where
    R: FlowRepository + CredentialStore + 'static,
{
    /// Constructs a new [`FlowEngine`].
    #[must_use]
    pub const fn new(repo: Arc<R>, operators: Arc<OperatorRegistry>, data_root: PathBuf) -> Self {
        Self {
            repo,
            operators,
            data_root,
        }
    }

    /// Returns a reference to the registered [`OperatorRegistry`].
    #[must_use]
    pub fn operators(&self) -> &OperatorRegistry {
        &self.operators
    }

    /// Cancels `run_id` in storage and flips all in-flight cooperative `cancel_flag`s.
    ///
    /// # Errors
    /// Returns [`AppError`] if the run does not exist or cannot be canceled.
    pub async fn cancel_run(&self, tenant_id: &str, run_id: Uuid) -> Result<RunRecord, AppError> {
        let rec = self.repo.cancel_flow_run(tenant_id, run_id).await?;
        signal_run_canceled(run_id);
        Ok(rec)
    }

    /// Retries a `FAILED` run from its failed nodes (`attempt + 1`).
    ///
    /// # Errors
    /// Returns [`AppError`] if the run is missing or not in `FAILED` status.
    pub async fn retry_run(&self, tenant_id: &str, run_id: Uuid) -> Result<RunRecord, AppError> {
        self.repo.retry_flow_run(tenant_id, run_id).await
    }

    /// Executes a single leased [`FlowStepJobPayload`] and advances the owning DAG run.
    ///
    /// # Errors
    /// Returns [`AppError`] only on transient storage errors so `JobWorker` can re-lease;
    /// operator failures are persisted onto `ax_flow_step_logs` per the node's [`RetryPolicy`].
    pub async fn execute_step(
        &self,
        tenant_id: &str,
        payload: FlowStepJobPayload,
    ) -> Result<serde_json::Value, AppError> {
        self.execute_step_with_job(tenant_id, None, payload).await
    }

    /// Executes a leased [`FlowStepJobPayload`] recording its backing `ax_jobs.id`.
    ///
    /// # Errors
    /// Returns [`AppError`] on transient storage errors so `JobWorker` can re-lease.
    pub async fn execute_step_with_job(
        &self,
        tenant_id: &str,
        job_id: Option<Uuid>,
        payload: FlowStepJobPayload,
    ) -> Result<serde_json::Value, AppError> {
        let Some(run) = self.repo.get_flow_run(tenant_id, payload.run_id).await? else {
            return Ok(serde_json::json!({ "missing_run": true }));
        };
        if run.status == RunStatus::Canceled {
            return Ok(serde_json::json!({ "canceled": true }));
        }

        let steps = self.repo.list_run_steps(tenant_id, payload.run_id).await?;
        let claimable = steps
            .iter()
            .find(|s| s.node_id == payload.node_id && s.attempt == payload.attempt)
            .is_some_and(|s| matches!(s.status, StepStatus::Pending | StepStatus::Running));
        if !claimable {
            return Ok(serde_json::json!({ "skipped_claim": true }));
        }

        let version = self
            .repo
            .get_flow_version(tenant_id, run.flow_id, run.version)
            .await?
            .ok_or(AppError::NotFound("VERSION_NOT_FOUND"))?;

        let Some(node) = version
            .nodes
            .iter()
            .find(|n| n.id == payload.node_id)
            .cloned()
        else {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        };

        let mut hydrated = self.hydrate_outputs_from_steps(&steps).await;
        let rendered_cfg = self
            .prepare_and_mark_running(tenant_id, &run, &node, payload.attempt, job_id, &hydrated)
            .await;

        let mutation = match rendered_cfg {
            Ok(cfg) => {
                let op_res = self
                    .invoke_operator(tenant_id, &run, &node, payload.attempt, cfg)
                    .await;
                self.outcome_to_mutation(
                    tenant_id,
                    payload.run_id,
                    &node,
                    payload.attempt,
                    op_res,
                    &mut hydrated,
                )
                .await?
            }
            Err(err) => Some(StepMutation::TerminalFailed {
                node_id: node.id.clone(),
                attempt: payload.attempt,
                error_code: String::from(err.error_code()),
                error_message: err.client_message(),
            }),
        };

        let Some(step_mut) = mutation else {
            return Ok(serde_json::json!({ "status": "CANCELED" }));
        };

        let updated_run = self
            .repo
            .apply_step_mutation_and_progress(
                tenant_id,
                payload.run_id,
                step_mut,
                &|locked_run, ver, latest| plan_progression(locked_run, ver, latest, &hydrated),
            )
            .await?;

        Ok(serde_json::json!({
            "run_id": updated_run.id,
            "run_status": updated_run.status,
            "node_id": payload.node_id,
            "attempt": payload.attempt,
        }))
    }

    /// Handles an `approval.decided` outbox event to resume or fail a `WAITING_APPROVAL` flow step (`07-STEP07-08` §07.4).
    ///
    /// Idempotent across duplicate outbox deliveries.
    ///
    /// # Errors
    /// Returns [`AppError`] if storage updates fail.
    pub async fn handle_approval_decided_event(&self, event: &OutboxEvent) -> Result<(), AppError> {
        let src_kind: ApprovalSourceKind = match event.payload.get("source_kind") {
            None | Some(serde_json::Value::Null) => ApprovalSourceKind::Flow,
            Some(v) => {
                let Ok(parsed) = serde_json::from_value(v.clone()) else {
                    return Ok(());
                };
                parsed
            }
        };
        if src_kind != ApprovalSourceKind::Flow {
            return Ok(());
        }

        let Some(run_id) = event
            .payload
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            return Ok(());
        };
        let Some(node_id) = event
            .payload
            .get("node_id")
            .and_then(serde_json::Value::as_str)
            .map(String::from)
        else {
            return Ok(());
        };
        let Some(status_val) = event.payload.get("status") else {
            return Ok(());
        };
        let approval_status: ApprovalStatus =
            serde_json::from_value(status_val.clone()).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid approval status: {err}"),
                )
            })?;

        let steps = self.repo.list_run_steps(&event.tenant_id, run_id).await?;
        let Some(waiting_step) = steps
            .iter()
            .filter(|s| s.node_id == node_id)
            .max_by_key(|s| s.attempt)
        else {
            return Ok(());
        };
        if waiting_step.status != StepStatus::WaitingApproval {
            return Ok(());
        }

        let mut hydrated = self.hydrate_outputs_from_steps(&steps).await;
        let mutation = build_approval_mutation(
            &node_id,
            waiting_step.attempt,
            approval_status,
            &event.payload,
            &mut hydrated,
        );

        let _ = self
            .repo
            .apply_step_mutation_and_progress(
                &event.tenant_id,
                run_id,
                mutation,
                &|locked_run, ver, latest| plan_progression(locked_run, ver, latest, &hydrated),
            )
            .await?;
        Ok(())
    }

    async fn prepare_and_mark_running(
        &self,
        tenant_id: &str,
        run: &RunRecord,
        node: &FlowNode,
        attempt: i32,
        job_id: Option<Uuid>,
        hydrated: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, AppError> {
        let tpl_ctx = TemplateContext {
            trigger: &run.trigger_payload,
            step_outputs: hydrated,
            run_id: run.id,
            run_environment: run.environment,
            run_started_at: run.started_at,
            flow_id: run.flow_id,
            flow_version: run.version,
        };
        match render_config_value(&node.config, &tpl_ctx) {
            Ok(rendered) => {
                let _ = self
                    .repo
                    .mark_step_running(tenant_id, run.id, &node.id, attempt, job_id, &rendered)
                    .await?;
                Ok(rendered)
            }
            Err(err) => {
                let _ = self
                    .repo
                    .mark_step_running(tenant_id, run.id, &node.id, attempt, job_id, &node.config)
                    .await?;
                Err(err)
            }
        }
    }

    async fn invoke_operator(
        &self,
        tenant_id: &str,
        run: &RunRecord,
        node: &FlowNode,
        attempt: i32,
        rendered_cfg: serde_json::Value,
    ) -> Result<OperatorOutcome, AppError> {
        let op = self
            .operators
            .get(node.kind)
            .ok_or(AppError::Unsupported("UNSUPPORTED_FORMAT"))?;

        let principal = self
            .resolve_actor_principal(tenant_id, run.started_by)
            .await?;
        let cancel_guard = ActiveCancelGuard::register(run.id);
        let cancel_flag = Arc::clone(&cancel_guard.flag);

        let op_ctx = OperatorContext::new(
            String::from(tenant_id),
            run.flow_id,
            run.version,
            run.id,
            node.id.clone(),
            attempt,
            run.environment,
        )
        .with_execution_inputs(rendered_cfg, principal, Arc::clone(&cancel_flag));

        let timeout_secs = u64::from(node.timeout_s.clamp(1, 86_400));
        let timed =
            tokio::time::timeout(Duration::from_secs(timeout_secs), op.execute(&op_ctx)).await;
        if let Ok(res) = timed {
            res
        } else {
            cancel_flag.store(true, Ordering::Relaxed);
            Err(AppError::Timeout("UPSTREAM_TIMEOUT"))
        }
    }

    async fn outcome_to_mutation(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        node: &FlowNode,
        attempt: i32,
        outcome: Result<OperatorOutcome, AppError>,
        hydrated: &mut HashMap<String, serde_json::Value>,
    ) -> Result<Option<StepMutation>, AppError> {
        match outcome {
            Ok(OperatorOutcome::Completed(output_val)) => {
                hydrated.insert(node.id.clone(), output_val.clone());
                let (inline_out, out_ref) = self
                    .persist_or_inline_output(tenant_id, run_id, &node.id, attempt, output_val)
                    .await?;
                Ok(Some(StepMutation::Succeeded {
                    node_id: node.id.clone(),
                    attempt,
                    output: inline_out,
                    output_ref: out_ref,
                }))
            }
            Ok(OperatorOutcome::Suspend { .. }) => Ok(Some(StepMutation::Suspended {
                node_id: node.id.clone(),
                attempt,
            })),
            Err(AppError::Conflict("RUN_CANCELED")) => Ok(None),
            Err(err) => {
                let code = String::from(err.error_code());
                let msg = err.client_message();
                let non_retryable = matches!(
                    code.as_str(),
                    "APPROVAL_REJECTED" | "APPROVAL_EXPIRED" | "UNSUPPORTED_FORMAT"
                );
                let max_attempts = i32::try_from(node.retry.max_attempts).unwrap_or(10);
                if !non_retryable && attempt < max_attempts {
                    let delay_s = compute_retry_backoff_secs(&node.retry, attempt);
                    let next_run_at = Utc::now() + ChronoDuration::seconds(delay_s);
                    Ok(Some(StepMutation::Retry {
                        node_id: node.id.clone(),
                        attempt,
                        error_code: code,
                        error_message: msg,
                        next_run_at,
                    }))
                } else {
                    Ok(Some(StepMutation::TerminalFailed {
                        node_id: node.id.clone(),
                        attempt,
                        error_code: code,
                        error_message: msg,
                    }))
                }
            }
        }
    }

    async fn persist_or_inline_output(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        node_id: &str,
        attempt: i32,
        output_val: serde_json::Value,
    ) -> Result<(Option<serde_json::Value>, Option<String>), AppError> {
        let encoded = serde_json::to_vec(&output_val)
            .map_err(|err| AppError::Config(format!("serialize step output failed: {err}")))?;
        if encoded.len() <= INLINE_OUTPUT_MAX_BYTES {
            return Ok((Some(output_val), None));
        }

        let rel_path = format!("flow_outputs/{tenant_id}/{run_id}/{node_id}_{attempt}.json");
        let full_path = self.data_root.join(&rel_path);
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|err| {
                AppError::Config(format!("create flow_outputs dir failed: {err}"))
            })?;
        }
        tokio::fs::write(&full_path, &encoded)
            .await
            .map_err(|err| {
                AppError::Config(format!("write offloaded step output failed: {err}"))
            })?;

        Ok((None, Some(rel_path)))
    }

    async fn hydrate_outputs_from_steps(
        &self,
        steps: &[StepRecord],
    ) -> HashMap<String, serde_json::Value> {
        let mut latest_by_node: HashMap<&str, &StepRecord> = HashMap::new();
        for step in steps {
            match latest_by_node.get(step.node_id.as_str()) {
                Some(prev) if prev.attempt >= step.attempt => {}
                _ => {
                    latest_by_node.insert(step.node_id.as_str(), step);
                }
            }
        }

        let mut map = HashMap::with_capacity(latest_by_node.len());
        for (&nid, step) in &latest_by_node {
            if step.status != StepStatus::Succeeded {
                continue;
            }
            if let Some(inline) = &step.output {
                map.insert(String::from(nid), inline.clone());
            } else if let Some(rel_ref) = &step.output_ref
                && let Some(offloaded) = read_offloaded_json(&self.data_root, rel_ref).await
            {
                map.insert(String::from(nid), offloaded);
            }
        }
        map
    }

    async fn resolve_actor_principal(
        &self,
        tenant_id: &str,
        started_by: Option<Uuid>,
    ) -> Result<FlowActorPrincipal, AppError> {
        let Some(uid) = started_by.filter(|id| !id.is_nil()) else {
            return Ok(FlowActorPrincipal::service(started_by));
        };
        match self.repo.load_principal(tenant_id, uid).await? {
            Some(rec) => Ok(FlowActorPrincipal {
                user_id: rec.user_id,
                username: rec.username,
                dept_id: rec.dept_id,
                roles: rec.roles,
                permissions: rec.permissions,
            }),
            None => Ok(FlowActorPrincipal::service(Some(uid))),
        }
    }
}

fn build_approval_mutation(
    node_id: &str,
    attempt: i32,
    approval_status: ApprovalStatus,
    payload: &serde_json::Value,
    hydrated: &mut HashMap<String, serde_json::Value>,
) -> StepMutation {
    match approval_status {
        ApprovalStatus::Approved => {
            let out_val = serde_json::json!({
                "approved": true,
                "approval_id": payload.get("approval_id").cloned().unwrap_or(serde_json::Value::Null),
                "decision": "APPROVE",
                "status": "APPROVED",
                "decided_by": payload.get("decided_by").cloned().unwrap_or(serde_json::Value::Null),
                "decided_at": payload.get("decided_at").cloned().unwrap_or(serde_json::Value::Null),
                "comment": payload.get("comment").cloned().unwrap_or(serde_json::Value::String(String::new())),
            });
            hydrated.insert(String::from(node_id), out_val.clone());
            StepMutation::Succeeded {
                node_id: String::from(node_id),
                attempt,
                output: Some(out_val),
                output_ref: None,
            }
        }
        ApprovalStatus::Rejected => StepMutation::TerminalFailed {
            node_id: String::from(node_id),
            attempt,
            error_code: String::from("APPROVAL_REJECTED"),
            error_message: payload
                .get("comment")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map_or_else(|| String::from("approval rejected"), String::from),
        },
        ApprovalStatus::Expired | ApprovalStatus::Pending => StepMutation::TerminalFailed {
            node_id: String::from(node_id),
            attempt,
            error_code: String::from("APPROVAL_EXPIRED"),
            error_message: String::from("approval expired before decision"),
        },
    }
}

async fn read_offloaded_json(data_root: &Path, rel_ref: &str) -> Option<serde_json::Value> {
    if rel_ref.contains("..") || rel_ref.starts_with('/') {
        return None;
    }
    let full = data_root.join(rel_ref);
    let bytes = tokio::fs::read(full).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `JobKind::FlowStep` worker adapter delegating leased jobs to [`FlowEngine::execute_step_with_job`].
pub struct FlowStepJobHandler<R> {
    engine: Arc<FlowEngine<R>>,
}

impl<R> FlowStepJobHandler<R> {
    /// Creates a [`FlowStepJobHandler`] wrapping `engine`.
    #[must_use]
    pub const fn new(engine: Arc<FlowEngine<R>>) -> Self {
        Self { engine }
    }
}

impl<R> JobHandler for FlowStepJobHandler<R>
where
    R: FlowRepository + CredentialStore + 'static,
{
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let payload: FlowStepJobPayload =
                serde_json::from_value(job.payload.clone()).map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid FlowStepJobPayload: {err}"),
                    )
                })?;
            self.engine
                .execute_step_with_job(&job.tenant_id, Some(job.id), payload)
                .await
        })
    }
}
