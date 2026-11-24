//! Unit, integration, and `proptest` property tests for `axpoint-flow` (`07-STEP07-08` §07.9).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axpoint_domain::analytics::{
    AnalyticsPrincipal, MetricQuery, MetricQueryResult, PivotResult, PivotSpec, QueryColumn,
    QueryRequest, QueryResult,
};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    DataType, Environment, FlowOperator, OutboxTopic, PiiCategory, RunStatus, StepStatus,
    TriggerType,
};
use axpoint_domain::flow::{
    ActionListQuery, ActionLogRecord, ActionLogStatus, ApprovalDecision, ApprovalListQuery,
    ApprovalRecord, ApprovalSourceKind, ApprovalStatus, BackoffStrategy, DecideApprovalParams,
    DeployFlowRequest, EventFlowItem, FlowActorPrincipal, FlowDeploymentRecord, FlowEdge,
    FlowGraph, FlowListQuery, FlowNode, FlowStepJobPayload, FlowVersionRecord,
    FlowVersionsListResponse, JoinMode, NewActionLogInput, NewApprovalInput, OnFailure, Operator,
    OperatorContext, OperatorOutcome, OperatorRegistry, Pos, ProgressionPlannerFn,
    PublishFlowVersionRequest, ResolvedFlowHook, RetryPolicy, RunListQuery, RunRecord,
    ScheduledFlowItem, StartRunParams, StepMutation, StepRecord, TriggerCfg, UpdateAxFlowRequest,
};
use axpoint_domain::graph::{
    BulkNodesRequest, BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery,
    LinkRecord, MetricCatalogEntry, MetricSeriesPoint, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchQuery, NodeSearchResponse, PatchNodeRequest,
    ResolveEntitiesRequest, ResolveEntitiesResponse, UpsertLinkRequest, WindowQuery,
    WindowQueryResponse, WriteMetricsRequest, WriteMetricsResponse,
};
use axpoint_domain::platform::{ApiTokenRecord, BoxFuture, OutboxEvent, PrincipalRecord};
use axpoint_domain::ports::{
    AiPort, AnalyticsQueryPort, CredentialStore, FlowCredentialUnsealPort, FlowRepository,
    FlowSqlMaterializePort, GraphStorePort, MetricStorePort, OutboundUrlGuardPort,
};
use axpoint_domain::topology::{AxFlowDef, BlueNode, CreateAxFlowRequest, UpsertBlueNodeRequest};
use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
use chrono::{TimeZone as _, Utc};
use proptest::prelude::*;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::{
    ActionEmitOperator, ApprovalDecidedSubscriber, FlowEngine, FlowOutboxTriggerSubscriber,
    HttpCallOperator, HumanApprovalOperator, LlmExtractOperator, SqlTransformOperator,
    TemplateContext, compute_retry_backoff_secs, eval_json_guard_expr, parse_timezone_offset,
    plan_progression, render_config_value, topological_order_or_cycle, validate_flow_default,
};

fn make_node(id: &str, kind: FlowOperator, config: serde_json::Value) -> FlowNode {
    FlowNode {
        id: String::from(id),
        kind,
        label: String::from(id),
        config,
        retry: RetryPolicy::default(),
        timeout_s: 60,
        on_failure: OnFailure::FailRun,
        join: JoinMode::AllSuccess,
        pos: Pos { x: 0.0, y: 0.0 },
    }
}

fn make_edge(from: &str, to: &str, when: Option<&str>) -> FlowEdge {
    FlowEdge {
        from: String::from(from),
        to: String::from(to),
        when: when.map(String::from),
    }
}

fn make_version(flow_id: Uuid, nodes: Vec<FlowNode>, edges: Vec<FlowEdge>) -> FlowVersionRecord {
    FlowVersionRecord {
        id: Uuid::new_v4(),
        flow_id,
        version: 1,
        nodes,
        edges,
        trigger_cfg: TriggerCfg::default(),
        note: String::new(),
        published_by: None,
        published_at: Utc::now(),
    }
}

fn make_pending_step(id: i64, run_id: Uuid, node_id: String, attempt: i32) -> StepRecord {
    StepRecord {
        id,
        run_id,
        node_id,
        attempt,
        status: StepStatus::Pending,
        job_id: None,
        input: serde_json::json!({}),
        output: None,
        output_ref: None,
        error_code: None,
        error_message: None,
        started_at: None,
        finished_at: None,
    }
}

fn make_start_params(flow_id: Uuid, environment: Environment, key: Option<&str>) -> StartRunParams {
    StartRunParams {
        flow_id,
        environment,
        explicit_version: Some(1),
        trigger_source: TriggerType::Manual,
        trigger_payload: serde_json::json!({}),
        idempotency_key: key.map(String::from),
        started_by: None,
    }
}

fn make_outbox_event(
    id: i64,
    tenant_id: &str,
    topic: OutboxTopic,
    payload: serde_json::Value,
) -> OutboxEvent {
    OutboxEvent {
        id,
        tenant_id: String::from(tenant_id),
        topic,
        payload,
        claimed_by: None,
        claimed_until: None,
        created_at: Utc::now(),
    }
}

#[test]
fn static_validation_catches_cycles_and_reports_closed_path() {
    let ds_id = Uuid::new_v4().to_string();
    let graph = FlowGraph {
        nodes: vec![
            make_node(
                "ingest",
                FlowOperator::SourceIngest,
                serde_json::json!({ "dataset_id": ds_id }),
            ),
            make_node(
                "sql_step",
                FlowOperator::SqlTransform,
                serde_json::json!({ "sql": "SELECT 1" }),
            ),
            make_node("agg_step", FlowOperator::AggTrigger, serde_json::json!({})),
            make_node(
                "leaf_after",
                FlowOperator::AggTrigger,
                serde_json::json!({}),
            ),
        ],
        edges: vec![
            make_edge("ingest", "sql_step", None),
            make_edge("sql_step", "agg_step", None),
            make_edge("agg_step", "ingest", None),
            make_edge("agg_step", "leaf_after", None),
        ],
    };

    let res = validate_flow_default(&graph, &TriggerCfg::default(), None);
    assert!(!res.valid);
    let cycle_issue = res.issues.iter().find(|i| i.code == "CYCLE_DETECTED");
    assert!(cycle_issue.is_some());
    let msg = cycle_issue.map_or("", |i| i.message.as_str());
    assert!(
        msg.contains("ingest -> sql_step -> agg_step -> ingest"),
        "got: {msg}"
    );
}

#[test]
fn static_validation_checks_all_eight_rules() {
    let g_unsupported = FlowGraph {
        nodes: vec![
            make_node("ag", FlowOperator::AgentTask, serde_json::json!({})),
            make_node(
                "wi",
                FlowOperator::ActionEmit,
                serde_json::json!({ "actions": [{ "kind": "WORK_ITEM" }] }),
            ),
            make_node(
                "wi_ok",
                FlowOperator::ActionEmit,
                serde_json::json!({
                    "actions": [{ "kind": "WORK_ITEM", "title": "门禁工作项", "priority": "P1" }]
                }),
            ),
        ],
        edges: Vec::new(),
    };
    let r5_step07 = crate::validate_flow(
        &g_unsupported,
        &TriggerCfg::default(),
        None,
        &crate::step07_schema_only_registry(),
    );
    assert_eq!(
        r5_step07
            .issues
            .iter()
            .filter(|i| i.code == "UNSUPPORTED_FORMAT")
            .count(),
        1
    );
    let r5_step09 = validate_flow_default(&g_unsupported, &TriggerCfg::default(), None);
    assert_eq!(
        r5_step09
            .issues
            .iter()
            .filter(|i| i.code == "UNSUPPORTED_FORMAT")
            .count(),
        0
    );
    assert!(
        r5_step09
            .issues
            .iter()
            .any(|i| i.node_id.as_deref() == Some("ag") && i.code == "SCHEMA_INVALID")
    );
    assert!(
        r5_step09
            .issues
            .iter()
            .any(|i| i.node_id.as_deref() == Some("wi") && i.code == "SCHEMA_INVALID")
    );
    assert!(
        !r5_step09
            .issues
            .iter()
            .any(|i| i.node_id.as_deref() == Some("wi_ok"))
    );

    assert_rule6_7_8_validation();
}

fn assert_rule6_7_8_validation() {
    let g_schema_tpl = FlowGraph {
        nodes: vec![
            make_node("n1", FlowOperator::SourceIngest, serde_json::json!({})),
            make_node(
                "n2",
                FlowOperator::SqlTransform,
                serde_json::json!({ "sql": "SELECT '{{ nodes.sibling.output.x }}'" }),
            ),
            make_node("sibling", FlowOperator::AggTrigger, serde_json::json!({})),
        ],
        edges: vec![make_edge("n1", "n2", Some("row_count >"))],
    };
    let r67 = validate_flow_default(&g_schema_tpl, &TriggerCfg::default(), None);
    assert!(r67.issues.iter().any(|i| i.code == "SCHEMA_INVALID"));
    assert_eq!(
        r67.issues
            .iter()
            .filter(|i| i.code == "EXPR_INVALID")
            .count(),
        2
    );

    let valid_dag = FlowGraph {
        nodes: vec![make_node(
            "root",
            FlowOperator::AggTrigger,
            serde_json::json!({}),
        )],
        edges: Vec::new(),
    };
    let webhook_cfg = TriggerCfg {
        kind: TriggerType::Webhook,
        ..TriggerCfg::default()
    };
    assert!(!validate_flow_default(&valid_dag, &webhook_cfg, None).valid);
    assert!(validate_flow_default(&valid_dag, &webhook_cfg, Some("replenish-hook")).valid);

    let bad_cron = TriggerCfg {
        kind: TriggerType::Cron,
        cron: Some(String::from("bad cron")),
        ..TriggerCfg::default()
    };
    assert!(!validate_flow_default(&valid_dag, &bad_cron, None).valid);
}

proptest! {
    #[test]
    fn prop_forward_only_dags_never_trigger_cycle_detected(
        node_count in 1_usize..16,
        edge_mask in prop::collection::vec(any::<bool>(), 0..120),
    ) {
        let nodes: Vec<FlowNode> = (0..node_count)
            .map(|i| make_node(&format!("n_{i}"), FlowOperator::AggTrigger, serde_json::json!({})))
            .collect();
        let mut edges = Vec::new();
        let mut mask_idx = 0_usize;
        for i in 0..node_count {
            for j in (i + 1)..node_count {
                if edge_mask.get(mask_idx).copied().unwrap_or(false) {
                    edges.push(make_edge(&format!("n_{i}"), &format!("n_{j}"), None));
                }
                mask_idx = mask_idx.saturating_add(1);
            }
        }
        let graph = FlowGraph { nodes, edges };
        let order = topological_order_or_cycle(&graph);
        prop_assert!(order.is_ok());
        let val = validate_flow_default(&graph, &TriggerCfg::default(), None);
        prop_assert!(val.valid, "issues: {:?}", val.issues);
    }

    #[test]
    fn prop_closing_chain_always_detects_cycle(chain_len in 2_usize..10) {
        let nodes: Vec<FlowNode> = (0..chain_len)
            .map(|i| make_node(&format!("c_{i}"), FlowOperator::AggTrigger, serde_json::json!({})))
            .collect();
        let mut edges = Vec::new();
        for i in 0..(chain_len - 1) {
            edges.push(make_edge(&format!("c_{i}"), &format!("c_{}", i + 1), None));
        }
        edges.push(make_edge(&format!("c_{}", chain_len - 1), "c_0", None));
        let graph = FlowGraph { nodes, edges };
        prop_assert!(topological_order_or_cycle(&graph).is_err());
        let val = validate_flow_default(&graph, &TriggerCfg::default(), None);
        prop_assert!(val.issues.iter().any(|i| i.code == "CYCLE_DETECTED"));
    }

    #[test]
    fn prop_whole_value_template_preserves_json_types(num in -1_000_000_i64..1_000_000_i64, flag in any::<bool>()) {
        let trigger = serde_json::json!({ "n": num, "b": flag, "arr": [1, 2] });
        let outputs = HashMap::new();
        let ctx = TemplateContext {
            trigger: &trigger,
            step_outputs: &outputs,
            run_id: Uuid::nil(),
            run_environment: Environment::Test,
            run_started_at: None,
            flow_id: Uuid::nil(),
            flow_version: 1,
        };
        let cfg = serde_json::json!({
            "num_val": "{{ trigger.n }}",
            "bool_val": "{{ trigger.b }}",
            "arr_val": "{{ trigger.arr }}",
            "missing_whole": "{{ trigger.missing }}",
            "inline_str": "n={{ trigger.n }},m={{ trigger.missing }}"
        });
        let rendered = render_config_value(&cfg, &ctx).ok();
        prop_assert_eq!(
            rendered,
            Some(serde_json::json!({
                "num_val": num,
                "bool_val": flag,
                "arr_val": [1, 2],
                "missing_whole": null,
                "inline_str": format!("n={num},m=")
            }))
        );
    }
}

#[test]
fn guard_expr_supports_numbers_strings_booleans_nulls_and_sql_rows() -> Result<(), AppError> {
    let payload = serde_json::json!({
        "deficit": 42.5,
        "status": "CRITICAL",
        "approved": true
    });
    assert!(eval_json_guard_expr(
        "deficit > 10 and status == 'CRITICAL'",
        &payload
    )?);
    assert!(eval_json_guard_expr("approved == true", &payload)?);
    assert!(!eval_json_guard_expr("missing_key > 0", &payload)?);

    // SQL_TRANSFORM output binding rows[0] columns for `when: "n > 0"`.
    let sql_out = serde_json::json!({
        "columns": ["n"],
        "rows": [{ "n": 5 }],
        "row_count": 1,
        "truncated": false
    });
    assert!(eval_json_guard_expr("n > 0", &sql_out)?);
    assert!(!eval_json_guard_expr("n > 10", &sql_out)?);

    let sql_zero = serde_json::json!({
        "columns": ["n"],
        "rows": [{ "n": 0 }],
        "row_count": 1,
        "truncated": false
    });
    assert!(!eval_json_guard_expr("n > 0", &sql_zero)?);

    let sql_empty = serde_json::json!({
        "columns": ["n"],
        "rows": [],
        "row_count": 0,
        "truncated": false
    });
    assert!(!eval_json_guard_expr("n > 0", &sql_empty)?);
    Ok(())
}

#[test]
fn retry_backoff_fixed_and_exponential_caps_at_3600s() {
    let fixed = RetryPolicy {
        max_attempts: 4,
        backoff_s: 15,
        backoff: BackoffStrategy::Fixed,
    };
    assert_eq!(compute_retry_backoff_secs(&fixed, 1), 15);
    assert_eq!(compute_retry_backoff_secs(&fixed, 3), 15);
    assert_eq!(fixed.delay_secs_after(3), 15);

    let exp = RetryPolicy {
        max_attempts: 6,
        backoff_s: 10,
        backoff: BackoffStrategy::Exponential,
    };
    assert_eq!(compute_retry_backoff_secs(&exp, 1), 10);
    assert_eq!(compute_retry_backoff_secs(&exp, 2), 20);
    assert_eq!(compute_retry_backoff_secs(&exp, 3), 40);
    assert_eq!(compute_retry_backoff_secs(&exp, 10), 3_600);
    assert_eq!(exp.delay_secs_after(10), 3_600);
}

#[test]
fn plan_progression_cascades_skips_and_honors_join_modes() -> Result<(), AppError> {
    let run_id = Uuid::new_v4();
    let flow_id = Uuid::new_v4();
    let mut cleanup = make_node("cleanup", FlowOperator::AggTrigger, serde_json::json!({}));
    cleanup.join = JoinMode::AllDone;
    let mut any_join = make_node("any_join", FlowOperator::AggTrigger, serde_json::json!({}));
    any_join.join = JoinMode::AnySuccess;

    let version = make_version(
        flow_id,
        vec![
            make_node(
                "root",
                FlowOperator::SqlTransform,
                serde_json::json!({ "sql": "SELECT 1" }),
            ),
            make_node("taken", FlowOperator::AggTrigger, serde_json::json!({})),
            make_node("skipped_b", FlowOperator::AggTrigger, serde_json::json!({})),
            make_node(
                "cascaded_skip",
                FlowOperator::AggTrigger,
                serde_json::json!({}),
            ),
            any_join,
            cleanup,
        ],
        vec![
            make_edge("root", "taken", Some("deficit > 0")),
            make_edge("root", "skipped_b", Some("deficit > 100")),
            make_edge("skipped_b", "cascaded_skip", None),
            make_edge("taken", "any_join", None),
            make_edge("cascaded_skip", "any_join", None),
            make_edge("cascaded_skip", "cleanup", None),
        ],
    );

    let run = RunRecord {
        id: run_id,
        flow_id,
        version: 1,
        environment: Environment::Test,
        trigger_source: TriggerType::Manual,
        trigger_payload: serde_json::json!({}),
        idempotency_key: None,
        status: RunStatus::Running,
        outputs: serde_json::json!({}),
        error_message: None,
        started_by: None,
        created_at: Utc::now(),
        started_at: Some(Utc::now()),
        finished_at: None,
    };

    let root_out = serde_json::json!({ "deficit": 25 });
    let steps = vec![StepRecord {
        id: 1,
        run_id,
        node_id: String::from("root"),
        attempt: 1,
        status: StepStatus::Succeeded,
        job_id: None,
        input: serde_json::json!({}),
        output: Some(root_out.clone()),
        output_ref: None,
        error_code: None,
        error_message: None,
        started_at: Some(Utc::now()),
        finished_at: Some(Utc::now()),
    }];
    let hydrated = HashMap::from([(String::from("root"), root_out)]);

    let plan = plan_progression(&run, &version, &steps, &hydrated)?;
    assert_eq!(
        plan.nodes_to_skip,
        vec![String::from("skipped_b"), String::from("cascaded_skip")]
    );
    assert_eq!(
        plan.steps_to_enqueue,
        vec![(String::from("taken"), 1), (String::from("cleanup"), 1)]
    );
    assert_eq!(plan.next_run_status, Some(RunStatus::Running));
    Ok(())
}

// --- In-memory mock repository and ports for full FlowEngine & operator tests ---

#[derive(Default)]
struct MemFlowState {
    versions: HashMap<(Uuid, i32), FlowVersionRecord>,
    deployments: HashMap<(Uuid, Environment), FlowDeploymentRecord>,
    runs: HashMap<Uuid, RunRecord>,
    steps: HashMap<Uuid, Vec<StepRecord>>,
    enqueued_jobs: Vec<FlowStepJobPayload>,
    approvals: Vec<ApprovalRecord>,
    actions: Vec<ActionLogRecord>,
    event_flows: Vec<EventFlowItem>,
    nodes: HashMap<Uuid, BlueNode>,
    metrics: HashMap<(Uuid, String), f64>,
    next_step_id: i64,
    next_action_id: i64,
}

#[derive(Clone, Default)]
struct MockStore {
    inner: Arc<Mutex<MemFlowState>>,
}

impl CredentialStore for MockStore {
    fn resolve_api_token(
        &self,
        _h: &str,
    ) -> impl Future<Output = Result<Option<ApiTokenRecord>, AppError>> {
        std::future::ready(Ok(None))
    }
    fn touch_api_token(&self, _t: &str, _id: Uuid) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }
    fn load_principal(
        &self,
        _tenant_id: &str,
        _user_id: Uuid,
    ) -> impl Future<Output = Result<Option<PrincipalRecord>, AppError>> {
        std::future::ready(Ok(None))
    }
}

impl FlowRepository for MockStore {
    fn list_flows(
        &self,
        _t: &str,
        _q: FlowListQuery,
    ) -> impl Future<Output = Result<PaginatedList<AxFlowDef>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn get_flow(
        &self,
        _t: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<Option<AxFlowDef>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn create_flow(
        &self,
        _t: &str,
        _u: Option<Uuid>,
        _r: CreateAxFlowRequest,
    ) -> impl Future<Output = Result<AxFlowDef, AppError>> {
        std::future::ready(Err(AppError::NotFound("FLOW_NOT_FOUND")))
    }

    fn update_flow(
        &self,
        _t: &str,
        _id: Uuid,
        _r: UpdateAxFlowRequest,
    ) -> impl Future<Output = Result<Option<AxFlowDef>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn delete_flow(&self, _t: &str, _id: Uuid) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn publish_flow_version(
        &self,
        _t: &str,
        _id: Uuid,
        _u: Option<Uuid>,
        _r: PublishFlowVersionRequest,
    ) -> impl Future<Output = Result<FlowVersionRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("FLOW_NOT_FOUND")))
    }

    fn list_flow_versions(
        &self,
        _t: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<FlowVersionsListResponse, AppError>> {
        std::future::ready(Ok(FlowVersionsListResponse {
            items: Vec::new(),
            deployments: Vec::new(),
            next_cursor: None,
        }))
    }

    async fn get_flow_version(
        &self,
        _t: &str,
        flow_id: Uuid,
        version: i32,
    ) -> Result<Option<FlowVersionRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.versions.get(&(flow_id, version)).cloned())
    }

    async fn deploy_flow(
        &self,
        _t: &str,
        flow_id: Uuid,
        deployed_by: Option<Uuid>,
        req: DeployFlowRequest,
    ) -> Result<FlowDeploymentRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let rec = FlowDeploymentRecord {
            flow_id,
            environment: req.environment,
            version: req.version,
            deployed_by,
            deployed_at: Utc::now(),
        };
        guard
            .deployments
            .insert((flow_id, req.environment), rec.clone());
        Ok(rec)
    }

    async fn get_flow_deployment(
        &self,
        _t: &str,
        flow_id: Uuid,
        environment: Environment,
    ) -> Result<Option<FlowDeploymentRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.deployments.get(&(flow_id, environment)).cloned())
    }

    async fn start_flow_run(
        &self,
        _tenant_id: &str,
        params: StartRunParams,
    ) -> Result<(RunRecord, bool), AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(idem) = &params.idempotency_key
            && let Some(existing) = guard
                .runs
                .values()
                .find(|r| r.flow_id == params.flow_id && r.idempotency_key.as_ref() == Some(idem))
                .cloned()
        {
            return Ok((existing, true));
        }
        let ver = params.explicit_version.unwrap_or(1);
        let run_id = Uuid::new_v4();
        let rec = RunRecord {
            id: run_id,
            flow_id: params.flow_id,
            version: ver,
            environment: params.environment,
            trigger_source: params.trigger_source,
            trigger_payload: params.trigger_payload,
            idempotency_key: params.idempotency_key,
            status: RunStatus::Running,
            outputs: serde_json::json!({}),
            error_message: None,
            started_by: params.started_by,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
        };
        if let Some(vrec) = guard.versions.get(&(params.flow_id, ver)).cloned() {
            let fg = FlowGraph {
                nodes: vrec.nodes,
                edges: vrec.edges,
            };
            for root in crate::root_nodes(&fg) {
                guard.next_step_id += 1;
                let sid = guard.next_step_id;
                let nid = root.id.clone();
                guard
                    .steps
                    .entry(run_id)
                    .or_default()
                    .push(make_pending_step(sid, run_id, nid.clone(), 1));
                guard.enqueued_jobs.push(FlowStepJobPayload {
                    run_id,
                    node_id: nid,
                    attempt: 1,
                });
            }
        }
        guard.runs.insert(run_id, rec.clone());
        Ok((rec, false))
    }

    async fn get_flow_run(&self, _t: &str, run_id: Uuid) -> Result<Option<RunRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.runs.get(&run_id).cloned())
    }

    fn list_flow_runs(
        &self,
        _t: &str,
        _f: Uuid,
        _q: RunListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RunRecord>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    async fn list_run_steps(&self, _t: &str, run_id: Uuid) -> Result<Vec<StepRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.steps.get(&run_id).cloned().unwrap_or_default())
    }

    async fn list_latest_run_steps(
        &self,
        _t: &str,
        run_id: Uuid,
    ) -> Result<Vec<StepRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(latest_steps_for_mem(&guard, run_id))
    }

    async fn mark_step_running(
        &self,
        _t: &str,
        run_id: Uuid,
        node_id: &str,
        attempt: i32,
        job_id: Option<Uuid>,
        rendered_input: &serde_json::Value,
    ) -> Result<StepRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let list = guard
            .steps
            .get_mut(&run_id)
            .ok_or(AppError::NotFound("STEP_NOT_FOUND"))?;
        let step = list
            .iter_mut()
            .find(|s| s.node_id == node_id && s.attempt == attempt)
            .ok_or(AppError::NotFound("STEP_NOT_FOUND"))?;
        step.status = StepStatus::Running;
        step.job_id = job_id;
        step.input = rendered_input.clone();
        step.started_at = Some(Utc::now());
        Ok(step.clone())
    }

    async fn find_cached_step_output(
        &self,
        _t: &str,
        run_id: Uuid,
        node_id: &str,
        input: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AppError> {
        let guard = self.inner.lock().await;
        let found = guard.steps.get(&run_id).and_then(|list| {
            list.iter()
                .rev()
                .find(|s| s.node_id == node_id && &s.input == input && s.output.is_some())
                .and_then(|s| s.output.clone())
        });
        Ok(found)
    }

    async fn apply_step_mutation_and_progress(
        &self,
        _t: &str,
        run_id: Uuid,
        mutation: StepMutation,
        planner: ProgressionPlannerFn<'_>,
    ) -> Result<RunRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(run) = guard.runs.get(&run_id).cloned() else {
            return Err(AppError::NotFound("RUN_NOT_FOUND"));
        };
        if run.status == RunStatus::Canceled {
            return Ok(run);
        }
        apply_mem_mutation(&mut guard, run_id, mutation);

        let ver = guard
            .versions
            .get(&(run.flow_id, run.version))
            .cloned()
            .ok_or(AppError::NotFound("VERSION_NOT_FOUND"))?;
        let latest = latest_steps_for_mem(&guard, run_id);
        let plan = planner(&run, &ver, &latest)?;
        let updated = commit_mem_progression(&mut guard, run_id, run, plan);
        Ok(updated)
    }

    async fn cancel_flow_run(&self, _t: &str, run_id: Uuid) -> Result<RunRecord, AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(list) = guard.steps.get_mut(&run_id) {
            for s in list {
                if s.status == StepStatus::Pending {
                    s.status = StepStatus::Skipped;
                    s.finished_at = Some(Utc::now());
                }
            }
        }
        let run = guard
            .runs
            .get_mut(&run_id)
            .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
        run.status = RunStatus::Canceled;
        run.finished_at = Some(Utc::now());
        Ok(run.clone())
    }

    async fn retry_flow_run(&self, _t: &str, run_id: Uuid) -> Result<RunRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let latest = latest_steps_for_mem(&guard, run_id);
        for step in latest {
            if step.status == StepStatus::Failed {
                guard.next_step_id += 1;
                let sid = guard.next_step_id;
                let next_att = step.attempt + 1;
                guard
                    .steps
                    .entry(run_id)
                    .or_default()
                    .push(make_pending_step(
                        sid,
                        run_id,
                        step.node_id.clone(),
                        next_att,
                    ));
                guard.enqueued_jobs.push(FlowStepJobPayload {
                    run_id,
                    node_id: step.node_id,
                    attempt: next_att,
                });
            }
        }
        let run = guard
            .runs
            .get_mut(&run_id)
            .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
        run.status = RunStatus::Running;
        run.finished_at = None;
        Ok(run.clone())
    }

    async fn create_or_get_approval(
        &self,
        _t: &str,
        input: NewApprovalInput,
    ) -> Result<ApprovalRecord, AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard
            .approvals
            .iter()
            .find(|a| a.run_id == input.run_id && a.node_id == input.node_id)
            .cloned()
        {
            return Ok(existing);
        }
        let rec = ApprovalRecord {
            id: Uuid::new_v4(),
            run_id: input.run_id,
            node_id: input.node_id,
            source_kind: input.source_kind,
            source_ref: input.source_ref,
            title: input.title,
            detail: input.detail,
            assignee_roles: input.assignee_roles,
            assignee_users: input.assignee_users,
            status: ApprovalStatus::Pending,
            decided_by: None,
            decided_at: None,
            comment: String::new(),
            expires_at: input.expires_at,
            created_at: Utc::now(),
        };
        guard.approvals.push(rec.clone());
        Ok(rec)
    }

    async fn get_approval(
        &self,
        _t: &str,
        approval_id: Uuid,
    ) -> Result<Option<ApprovalRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .approvals
            .iter()
            .find(|a| a.id == approval_id)
            .cloned())
    }

    async fn list_approvals(
        &self,
        _t: &str,
        _viewer_user_id: Uuid,
        _viewer_roles: &[String],
        _q: ApprovalListQuery,
    ) -> Result<PaginatedList<ApprovalRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(PaginatedList {
            items: guard.approvals.clone(),
            next_cursor: None,
        })
    }

    async fn decide_approval(
        &self,
        _t: &str,
        params: DecideApprovalParams,
    ) -> Result<ApprovalRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let rec = guard
            .approvals
            .iter_mut()
            .find(|a| a.id == params.approval_id)
            .ok_or(AppError::NotFound("APPROVAL_NOT_FOUND"))?;
        rec.status = match params.decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Reject => ApprovalStatus::Rejected,
        };
        rec.decided_by = Some(params.actor_user_id);
        rec.decided_at = Some(Utc::now());
        rec.comment = params.comment;
        Ok(rec.clone())
    }

    fn expire_overdue_approvals(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<u64, AppError>> {
        std::future::ready(Ok(0))
    }

    async fn record_action_log(
        &self,
        _t: &str,
        input: NewActionLogInput,
    ) -> Result<(ActionLogRecord, bool), AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(pos) = guard
            .actions
            .iter()
            .position(|a| a.idempotency_key == input.idempotency_key)
            && let Some(existing) = guard.actions.get_mut(pos)
        {
            if existing.status == ActionLogStatus::Failed {
                existing.status = input.status;
                existing.error_message = input.error_message;
                return Ok((existing.clone(), false));
            }
            return Ok((existing.clone(), true));
        }
        guard.next_action_id += 1;
        let id = guard.next_action_id;
        let rec = ActionLogRecord {
            id,
            run_id: input.run_id,
            node_id: input.node_id,
            source_kind: input.source_kind,
            action_kind: input.action_kind,
            target: input.target,
            payload: input.payload,
            status: input.status,
            idempotency_key: input.idempotency_key,
            error_message: input.error_message,
            created_at: Utc::now(),
        };
        guard.actions.push(rec.clone());
        Ok((rec, false))
    }

    async fn get_action_log_by_key(
        &self,
        _t: &str,
        key: &str,
    ) -> Result<Option<ActionLogRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .actions
            .iter()
            .find(|a| a.idempotency_key == key)
            .cloned())
    }

    async fn list_action_logs(
        &self,
        _t: &str,
        _q: ActionListQuery,
    ) -> Result<PaginatedList<ActionLogRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(PaginatedList {
            items: guard.actions.clone(),
            next_cursor: None,
        })
    }

    fn upsert_flow_hook(
        &self,
        _t: &str,
        _flow_id: Uuid,
        _credential_id: Uuid,
    ) -> impl Future<Output = Result<(Uuid, chrono::DateTime<Utc>), AppError>> {
        std::future::ready(Ok((Uuid::new_v4(), Utc::now())))
    }

    fn resolve_flow_hook(
        &self,
        _t: &str,
        _slug: &str,
    ) -> impl Future<Output = Result<Option<ResolvedFlowHook>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_scheduled_flows(
        &self,
    ) -> impl Future<Output = Result<Vec<ScheduledFlowItem>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn list_event_flows(&self, _topic: &str) -> Result<Vec<EventFlowItem>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.event_flows.clone())
    }

    fn list_active_tenants(&self) -> impl Future<Output = Result<Vec<String>, AppError>> {
        std::future::ready(Ok(vec![String::from("default")]))
    }
}

fn apply_mem_mutation(state: &mut MemFlowState, run_id: Uuid, mutation: StepMutation) {
    let mut push_retry: Option<(String, i32)> = None;
    if let Some(list) = state.steps.get_mut(&run_id) {
        match mutation {
            StepMutation::Succeeded {
                node_id,
                attempt,
                output,
                output_ref,
            } => {
                if let Some(s) = list
                    .iter_mut()
                    .find(|s| s.node_id == node_id && s.attempt == attempt)
                {
                    s.status = StepStatus::Succeeded;
                    s.output = output;
                    s.output_ref = output_ref;
                    s.finished_at = Some(Utc::now());
                }
            }
            StepMutation::Retry {
                node_id,
                attempt,
                error_code,
                error_message,
                ..
            } => {
                if let Some(s) = list
                    .iter_mut()
                    .find(|s| s.node_id == node_id && s.attempt == attempt)
                {
                    s.status = StepStatus::Failed;
                    s.error_code = Some(error_code);
                    s.error_message = Some(error_message);
                    s.finished_at = Some(Utc::now());
                }
                push_retry = Some((node_id, attempt + 1));
            }
            StepMutation::TerminalFailed {
                node_id,
                attempt,
                error_code,
                error_message,
            } => {
                if let Some(s) = list
                    .iter_mut()
                    .find(|s| s.node_id == node_id && s.attempt == attempt)
                {
                    s.status = StepStatus::Failed;
                    s.error_code = Some(error_code);
                    s.error_message = Some(error_message);
                    s.finished_at = Some(Utc::now());
                }
            }
            StepMutation::Suspended { node_id, attempt } => {
                if let Some(s) = list
                    .iter_mut()
                    .find(|s| s.node_id == node_id && s.attempt == attempt)
                {
                    s.status = StepStatus::WaitingApproval;
                }
            }
        }
    }
    if let Some((nid, next_att)) = push_retry {
        state.next_step_id += 1;
        let sid = state.next_step_id;
        state
            .steps
            .entry(run_id)
            .or_default()
            .push(make_pending_step(sid, run_id, nid.clone(), next_att));
        state.enqueued_jobs.push(FlowStepJobPayload {
            run_id,
            node_id: nid,
            attempt: next_att,
        });
    }
}

fn commit_mem_progression(
    state: &mut MemFlowState,
    run_id: Uuid,
    fallback_run: RunRecord,
    plan: axpoint_domain::flow::RunProgressionPlan,
) -> RunRecord {
    if plan.next_run_status == Some(RunStatus::Failed)
        && let Some(list) = state.steps.get_mut(&run_id)
    {
        for s in list {
            if s.status == StepStatus::Pending {
                s.status = StepStatus::Skipped;
                s.finished_at = Some(Utc::now());
            }
        }
    }

    for skip_nid in plan.nodes_to_skip {
        let already = state
            .steps
            .get(&run_id)
            .is_some_and(|l| l.iter().any(|s| s.node_id == skip_nid));
        if !already {
            state.next_step_id += 1;
            let sid = state.next_step_id;
            state.steps.entry(run_id).or_default().push(StepRecord {
                id: sid,
                run_id,
                node_id: skip_nid,
                attempt: 1,
                status: StepStatus::Skipped,
                job_id: None,
                input: serde_json::json!({}),
                output: None,
                output_ref: None,
                error_code: None,
                error_message: None,
                started_at: Some(Utc::now()),
                finished_at: Some(Utc::now()),
            });
        }
    }

    for (ready_nid, ready_att) in plan.steps_to_enqueue {
        let already = state.steps.get(&run_id).is_some_and(|l| {
            l.iter()
                .any(|s| s.node_id == ready_nid && s.attempt == ready_att)
        });
        if !already {
            state.next_step_id += 1;
            let sid = state.next_step_id;
            state
                .steps
                .entry(run_id)
                .or_default()
                .push(make_pending_step(sid, run_id, ready_nid.clone(), ready_att));
            state.enqueued_jobs.push(FlowStepJobPayload {
                run_id,
                node_id: ready_nid,
                attempt: ready_att,
            });
        }
    }

    if let Some(mut r) = state.runs.get(&run_id).cloned() {
        if let Some(st) = plan.next_run_status {
            r.status = st;
            if matches!(
                st,
                RunStatus::Succeeded | RunStatus::Failed | RunStatus::Canceled
            ) {
                r.finished_at = Some(Utc::now());
            }
        }
        if let Some(outs) = plan.run_outputs {
            r.outputs = outs;
        }
        if let Some(err) = plan.run_error_message {
            r.error_message = Some(err);
        }
        state.runs.insert(run_id, r.clone());
        r
    } else {
        fallback_run
    }
}

fn latest_steps_for_mem(state: &MemFlowState, run_id: Uuid) -> Vec<StepRecord> {
    let mut map: BTreeMap<String, StepRecord> = BTreeMap::new();
    if let Some(list) = state.steps.get(&run_id) {
        for s in list {
            match map.get(&s.node_id) {
                Some(prev) if prev.attempt >= s.attempt => {}
                _ => {
                    map.insert(s.node_id.clone(), s.clone());
                }
            }
        }
    }
    map.into_values().collect()
}

struct MockAnalytics;
impl AnalyticsQueryPort for MockAnalytics {
    fn run_sql(
        &self,
        _p: &AnalyticsPrincipal,
        req: QueryRequest,
    ) -> impl Future<Output = Result<QueryResult, AppError>> {
        let mut row = serde_json::Map::new();
        let col_name = if req.sql.contains("AS n") {
            row.insert(String::from("n"), serde_json::json!(5));
            "n"
        } else {
            row.insert(String::from("deficit"), serde_json::json!(15.0));
            "deficit"
        };
        std::future::ready(Ok(QueryResult {
            columns: vec![QueryColumn {
                name: String::from(col_name),
                data_type: DataType::Float64,
                pii_category: PiiCategory::None,
                masked: false,
            }],
            rows: vec![row],
            row_count: 1,
            truncated: false,
            sql: req.sql,
            duration_ms: 2,
        }))
    }
    fn run_saved(
        &self,
        _p: &AnalyticsPrincipal,
        _id: Uuid,
        _params: BTreeMap<String, serde_json::Value>,
    ) -> impl Future<Output = Result<serde_json::Value, AppError>> {
        std::future::ready(Ok(serde_json::Value::Null))
    }
    fn run_metric(
        &self,
        _p: &AnalyticsPrincipal,
        _n: &str,
        _q: MetricQuery,
    ) -> impl Future<Output = Result<MetricQueryResult, AppError>> {
        std::future::ready(Err(AppError::NotFound("METRIC_NOT_FOUND")))
    }
    fn run_pivot(
        &self,
        _p: &AnalyticsPrincipal,
        _s: PivotSpec,
    ) -> impl Future<Output = Result<PivotResult, AppError>> {
        std::future::ready(Err(AppError::NotFound("DATASET_NOT_FOUND")))
    }
}

struct NoopMat;
impl FlowSqlMaterializePort for NoopMat {
    fn materialize_sql_dataset<'a>(
        &'a self,
        _p: &'a AnalyticsPrincipal,
        _n: &'a str,
        _sql: &'a str,
    ) -> BoxFuture<'a, Uuid> {
        Box::pin(async move { Ok(Uuid::new_v4()) })
    }
}

impl MetricStorePort for MockStore {
    async fn write_metrics(
        &self,
        _t: &str,
        req: WriteMetricsRequest,
    ) -> Result<WriteMetricsResponse, AppError> {
        let mut guard = self.inner.lock().await;
        for pt in &req.points {
            let resolved_id = pt.node_id.or_else(|| {
                let nr = pt.node.as_ref()?;
                guard
                    .nodes
                    .values()
                    .find(|n| n.type_name == nr.type_name && n.node_key == nr.node_key)
                    .map(|n| n.id)
            });
            if let (Some(nid), Some(v)) = (resolved_id, pt.value) {
                guard.metrics.insert((nid, pt.metric.clone()), v);
            }
        }
        Ok(WriteMetricsResponse {
            written: req.points.len(),
            dirty_minutes: 0,
            rollup_job_id: Uuid::nil(),
        })
    }
    fn query_node_metrics(
        &self,
        _t: &str,
        _n: Uuid,
        _m: &str,
        _q: NodeMetricSeriesQuery,
    ) -> impl Future<Output = Result<Vec<MetricSeriesPoint>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
    fn query_window(
        &self,
        _t: &str,
        _q: WindowQuery,
    ) -> impl Future<Output = Result<WindowQueryResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn list_metric_catalog(
        &self,
        _t: &str,
    ) -> impl Future<Output = Result<Vec<MetricCatalogEntry>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
    fn run_rollup_for_tenant(&self, _t: &str) -> impl Future<Output = Result<u64, AppError>> {
        std::future::ready(Ok(0))
    }
}

impl GraphStorePort for MockStore {
    async fn list_nodes(&self, _t: &str, q: NodeListQuery) -> Result<NodeListResponse, AppError> {
        let guard = self.inner.lock().await;
        let q_lower = q.q.as_deref().map(str::to_lowercase);
        let items: Vec<BlueNode> = guard
            .nodes
            .values()
            .filter(|n| {
                q.type_name.as_deref().is_none_or(|tn| n.type_name == tn)
                    && q_lower.as_deref().is_none_or(|ql| {
                        n.node_key.to_lowercase().contains(ql)
                            || n.title.to_lowercase().contains(ql)
                    })
            })
            .cloned()
            .collect();
        Ok(NodeListResponse {
            nodes: items.clone(),
            items,
            links: Vec::new(),
            next_cursor: None,
        })
    }

    async fn get_node(&self, _t: &str, id: Uuid) -> Result<Option<BlueNode>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.nodes.get(&id).cloned())
    }

    async fn upsert_node(
        &self,
        _t: &str,
        req: UpsertBlueNodeRequest,
    ) -> Result<BlueNode, AppError> {
        let mut guard = self.inner.lock().await;
        let id = Uuid::new_v4();
        let node = BlueNode {
            id,
            type_name: req.type_name,
            node_key: req.node_key,
            title: req.title,
            properties: req.properties,
            metrics: req.metrics,
            geo: req.geo,
            geo_shape: req.geo_shape,
            source_dataset_id: None,
            source_row_key: None,
            row_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        guard.nodes.insert(id, node.clone());
        Ok(node)
    }

    async fn patch_node(
        &self,
        _t: &str,
        id: Uuid,
        req: PatchNodeRequest,
    ) -> Result<BlueNode, AppError> {
        let mut guard = self.inner.lock().await;
        let node = guard
            .nodes
            .get_mut(&id)
            .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
        if let Some(title) = req.title {
            node.title = title;
        }
        if let Some(props) = req.properties {
            for (k, v) in props {
                node.properties.insert(k, v);
            }
        }
        if let Some(metrics) = req.metrics {
            for (k, v) in metrics {
                node.metrics.insert(k, v);
            }
        }
        node.row_version += 1;
        node.updated_at = Utc::now();
        Ok(node.clone())
    }

    fn delete_node(&self, _t: &str, _id: Uuid) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }
    fn bulk_upsert(
        &self,
        _t: &str,
        _r: BulkNodesRequest,
    ) -> impl Future<Output = Result<BulkNodesResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn list_links(
        &self,
        _t: &str,
        _q: LinkListQuery,
    ) -> impl Future<Output = Result<Vec<LinkRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
    fn upsert_link(
        &self,
        _t: &str,
        _r: UpsertLinkRequest,
    ) -> impl Future<Output = Result<LinkRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn delete_link(&self, _t: &str, _id: Uuid) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }
    fn search_nodes(
        &self,
        _t: &str,
        _q: NodeSearchQuery,
        _e: Option<&[f32]>,
    ) -> impl Future<Output = Result<NodeSearchResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn list_node_documents(
        &self,
        _t: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<NodeDocumentsResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn resolve_entity_links(
        &self,
        _t: &str,
        _r: ResolveEntitiesRequest,
    ) -> impl Future<Output = Result<ResolveEntitiesResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn list_entity_candidates(
        &self,
        _t: &str,
    ) -> impl Future<Output = Result<EntityCandidatesResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn confirm_entity_link(
        &self,
        _t: &str,
        _e: Uuid,
        _n: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
    fn reject_entity_link(
        &self,
        _t: &str,
        _e: Uuid,
        _n: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> {
        std::future::ready(Err(AppError::NotFound("NODE_NOT_FOUND")))
    }
}

struct EchoOperator;
impl Operator for EchoOperator {
    fn kind(&self) -> FlowOperator {
        FlowOperator::AggTrigger
    }
    fn display_name(&self) -> &'static str {
        "Echo"
    }
    fn description(&self) -> &'static str {
        "test echo"
    }
    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move {
            if ctx.config.get("fail").and_then(serde_json::Value::as_bool) == Some(true) {
                return Err(AppError::Upstream(String::from("simulated failure")));
            }
            Ok(OperatorOutcome::Completed(ctx.config.clone()))
        })
    }
}

fn build_test_engine(
    store: &Arc<MockStore>,
    extra_agg: Option<Arc<dyn Operator>>,
) -> (Arc<FlowEngine<MockStore>>, std::path::PathBuf) {
    let mut reg = OperatorRegistry::new();
    reg.register(Arc::new(
        SqlTransformOperator::<MockAnalytics, NoopMat>::new(Arc::new(MockAnalytics), None),
    ));
    reg.register(Arc::new(HumanApprovalOperator::new(Arc::clone(store))));
    reg.register(Arc::new(ActionEmitOperator::new(
        Arc::clone(store),
        Arc::clone(store),
        Arc::clone(store),
    )));
    if let Some(op) = extra_agg {
        reg.register(op);
    } else {
        reg.register(Arc::new(EchoOperator));
    }
    let tmp_dir = std::env::temp_dir().join(format!("axflow_test_{}", Uuid::new_v4()));
    let engine = Arc::new(FlowEngine::new(
        Arc::clone(store),
        Arc::new(reg),
        tmp_dir.clone(),
    ));
    (engine, tmp_dir)
}

async fn cleanup_dir(dir: &std::path::Path) {
    if tokio::fs::try_exists(dir).await.unwrap_or(false)
        && let Err(err) = tokio::fs::remove_dir_all(dir).await
    {
        tracing::warn!(error = %err, "failed to remove temporary test dir");
    }
}

#[tokio::test]
async fn sql_transform_when_guard_skips_false_branch_in_engine() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let flow_id = Uuid::new_v4();
    let version = make_version(
        flow_id,
        vec![
            make_node(
                "sql_cnt",
                FlowOperator::SqlTransform,
                serde_json::json!({ "sql": "SELECT 5 AS n" }),
            ),
            make_node(
                "branch_true",
                FlowOperator::AggTrigger,
                serde_json::json!({ "picked": "yes" }),
            ),
            make_node(
                "branch_false",
                FlowOperator::AggTrigger,
                serde_json::json!({ "picked": "no" }),
            ),
        ],
        vec![
            make_edge("sql_cnt", "branch_true", Some("n > 0")),
            make_edge("sql_cnt", "branch_false", Some("n > 10")),
        ],
    );
    store
        .inner
        .lock()
        .await
        .versions
        .insert((flow_id, 1), version);
    let (engine, tmp_dir) = build_test_engine(&store, None);

    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Test, None),
        )
        .await?;
    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("sql_cnt"),
                attempt: 1,
            },
        )
        .await?;

    let steps = store.list_run_steps("default", run.id).await?;
    assert!(
        steps
            .iter()
            .any(|s| s.node_id == "branch_false" && s.status == StepStatus::Skipped)
    );
    assert!(
        steps
            .iter()
            .any(|s| s.node_id == "branch_true" && s.status == StepStatus::Pending)
    );

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("branch_true"),
                attempt: 1,
            },
        )
        .await?;
    let final_run = store
        .get_flow_run("default", run.id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    assert_eq!(final_run.status, RunStatus::Succeeded);
    assert_eq!(
        final_run.outputs.get("branch_true"),
        Some(&serde_json::json!({ "picked": "yes" }))
    );
    assert!(final_run.outputs.get("branch_false").is_none());
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

#[tokio::test]
async fn concurrent_two_predecessors_enqueue_successor_only_once() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let flow_id = Uuid::new_v4();
    let version = make_version(
        flow_id,
        vec![
            make_node(
                "pred_a",
                FlowOperator::AggTrigger,
                serde_json::json!({ "a": 1 }),
            ),
            make_node(
                "pred_b",
                FlowOperator::AggTrigger,
                serde_json::json!({ "b": 2 }),
            ),
            make_node(
                "join_leaf",
                FlowOperator::AggTrigger,
                serde_json::json!({
                    "sum_a": "{{ nodes.pred_a.output.a }}",
                    "sum_b": "{{ nodes.pred_b.output.b }}"
                }),
            ),
        ],
        vec![
            make_edge("pred_a", "join_leaf", None),
            make_edge("pred_b", "join_leaf", None),
        ],
    );
    store
        .inner
        .lock()
        .await
        .versions
        .insert((flow_id, 1), version);
    let (engine, tmp_dir) = build_test_engine(&store, None);

    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Test, None),
        )
        .await?;

    let (res_a, res_b) = tokio::join!(
        engine.execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("pred_a"),
                attempt: 1,
            },
        ),
        engine.execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("pred_b"),
                attempt: 1,
            },
        )
    );
    let _ = res_a?;
    let _ = res_b?;

    let enqueued_for_join = store
        .inner
        .lock()
        .await
        .enqueued_jobs
        .iter()
        .filter(|j| j.run_id == run.id && j.node_id == "join_leaf")
        .count();
    assert_eq!(enqueued_for_join, 1);

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("join_leaf"),
                attempt: 1,
            },
        )
        .await?;
    let done = store
        .get_flow_run("default", run.id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    assert_eq!(done.status, RunStatus::Succeeded);
    assert_eq!(
        done.outputs.get("join_leaf"),
        Some(&serde_json::json!({ "sum_a": 1, "sum_b": 2 }))
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

async fn run_single_step(
    engine: &FlowEngine<MockStore>,
    run_id: Uuid,
    node_id: &str,
    attempt: i32,
) -> Result<serde_json::Value, AppError> {
    engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id,
                node_id: String::from(node_id),
                attempt,
            },
        )
        .await
}

#[tokio::test]
async fn any_success_join_runs_on_one_active_and_skips_when_none_active() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let flow_id = Uuid::new_v4();
    let mut soft_fail = make_node(
        "soft_fail",
        FlowOperator::AggTrigger,
        serde_json::json!({ "fail": true }),
    );
    soft_fail.retry.max_attempts = 1;
    soft_fail.on_failure = OnFailure::Continue;
    let mut any_sink = make_node(
        "any_sink",
        FlowOperator::AggTrigger,
        serde_json::json!({ "joined": true }),
    );
    any_sink.join = JoinMode::AnySuccess;
    let mut none_sink = make_node(
        "none_sink",
        FlowOperator::AggTrigger,
        serde_json::json!({ "joined": false }),
    );
    none_sink.join = JoinMode::AnySuccess;

    let version = make_version(
        flow_id,
        vec![
            make_node(
                "ok_pred",
                FlowOperator::AggTrigger,
                serde_json::json!({ "v": 3 }),
            ),
            soft_fail,
            any_sink,
            none_sink,
        ],
        vec![
            make_edge("ok_pred", "any_sink", Some("v > 0")),
            make_edge("soft_fail", "any_sink", None),
            make_edge("ok_pred", "none_sink", Some("v > 100")),
            make_edge("soft_fail", "none_sink", None),
        ],
    );
    store
        .inner
        .lock()
        .await
        .versions
        .insert((flow_id, 1), version);
    let (engine, tmp_dir) = build_test_engine(&store, None);

    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Test, None),
        )
        .await?;
    let _ = run_single_step(&engine, run.id, "ok_pred", 1).await?;
    let _ = run_single_step(&engine, run.id, "soft_fail", 1).await?;

    let steps = store.list_run_steps("default", run.id).await?;
    assert!(
        steps
            .iter()
            .any(|s| s.node_id == "none_sink" && s.status == StepStatus::Skipped)
    );
    assert!(
        steps
            .iter()
            .any(|s| s.node_id == "any_sink" && s.status == StepStatus::Pending)
    );

    let _ = run_single_step(&engine, run.id, "any_sink", 1).await?;
    let final_run = store
        .get_flow_run("default", run.id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    assert_eq!(final_run.status, RunStatus::Succeeded);
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

fn sample_node_ref_actions_cfg(wh_uuid_str: &str) -> serde_json::Value {
    serde_json::json!({
        "actions": [
            { "kind": "WRITE_NODE_METRIC", "node_ref": "Warehouse:W-1", "metric": "safety_stock", "value": 42 },
            { "kind": "WRITE_NODE_PROPERTY", "node_ref": "Warehouse:W-1", "property": "tier", "value": "GOLD" },
            { "kind": "WRITE_NODE_METRIC", "node_ref": wh_uuid_str, "metric": "reorder_point", "value": 18 },
            { "kind": "WRITE_NODE_PROPERTY", "node_ref": { "type_name": "Warehouse", "node_key": "W-1" }, "property": "expedite", "value": true }
        ]
    })
}

#[tokio::test]
async fn action_emit_resolves_node_ref_type_key_uuid_and_object() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let wh_node = store
        .upsert_node(
            "default",
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("East Hub"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let wh_uuid_str = wh_node.id.to_string();

    let flow_id = Uuid::new_v4();
    let graph = FlowGraph {
        nodes: vec![make_node(
            "emit",
            FlowOperator::ActionEmit,
            sample_node_ref_actions_cfg(&wh_uuid_str),
        )],
        edges: Vec::new(),
    };
    assert!(validate_flow_default(&graph, &TriggerCfg::default(), None).valid);
    store.inner.lock().await.versions.insert(
        (flow_id, 1),
        make_version(flow_id, graph.nodes, graph.edges),
    );

    let (engine, tmp_dir) = build_test_engine(&store, None);
    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Prod, None),
        )
        .await?;
    let _ = run_single_step(&engine, run.id, "emit", 1).await?;

    let patched = store
        .get_node("default", wh_node.id)
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    assert_eq!(
        patched.properties.get("tier"),
        Some(&serde_json::json!("GOLD"))
    );
    assert_eq!(
        patched.properties.get("expedite"),
        Some(&serde_json::json!(true))
    );

    let metrics_snapshot = store.inner.lock().await.metrics.clone();
    assert!(
        metrics_snapshot
            .get(&(wh_node.id, String::from("safety_stock")))
            .is_some_and(|v| (*v - 42.0).abs() < f64::EPSILON)
    );
    assert!(
        metrics_snapshot
            .get(&(wh_node.id, String::from("reorder_point")))
            .is_some_and(|v| (*v - 18.0).abs() < f64::EPSILON)
    );

    let logs = store
        .list_action_logs("default", ActionListQuery::default())
        .await?
        .items;
    let targets: Vec<&str> = logs.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(
        targets,
        vec![
            "Warehouse:W-1",
            "Warehouse:W-1",
            wh_uuid_str.as_str(),
            "Warehouse:W-1"
        ]
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

async fn run_approval_flow_once(
    store: &Arc<MockStore>,
    engine: &Arc<FlowEngine<MockStore>>,
    sub: &ApprovalDecidedSubscriber<MockStore>,
    flow_id: Uuid,
    env: Environment,
    event_id: i64,
) -> Result<Uuid, AppError> {
    let (run, _) = store
        .start_flow_run("default", make_start_params(flow_id, env, None))
        .await?;
    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("sql_check"),
                attempt: 1,
            },
        )
        .await?;
    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("approve"),
                attempt: 1,
            },
        )
        .await?;
    assert_eq!(
        store
            .get_flow_run("default", run.id)
            .await?
            .map(|r| r.status),
        Some(RunStatus::WaitingApproval)
    );

    let approval_id = store
        .list_approvals("default", Uuid::nil(), &[], ApprovalListQuery::default())
        .await?
        .items
        .iter()
        .find(|a| a.run_id == Some(run.id))
        .map(|a| a.id)
        .ok_or(AppError::NotFound("APPROVAL_NOT_FOUND"))?;
    let decided = store
        .decide_approval(
            "default",
            DecideApprovalParams {
                approval_id,
                actor_user_id: Uuid::nil(),
                actor_roles: vec![String::from("ADMIN")],
                decision: ApprovalDecision::Approve,
                comment: String::from("ok"),
            },
        )
        .await?;

    axpoint_domain::platform::OutboxSubscriber::on_event(
        sub,
        &make_outbox_event(
            event_id,
            "default",
            OutboxTopic::ApprovalDecided,
            serde_json::json!({
                "approval_id": decided.id,
                "source_kind": ApprovalSourceKind::Flow,
                "run_id": run.id,
                "node_id": "approve",
                "status": "APPROVED",
                "comment": "ok"
            }),
        ),
    )
    .await?;

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("emit"),
                attempt: 1,
            },
        )
        .await?;
    Ok(run.id)
}

#[tokio::test]
async fn engine_executes_approval_pause_prod_writeback_and_test_dry_run() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let warehouse_id = Uuid::new_v4();
    let flow_id = Uuid::new_v4();
    let version = make_version(
        flow_id,
        vec![
            make_node(
                "sql_check",
                FlowOperator::SqlTransform,
                serde_json::json!({ "sql": "SELECT 15 AS deficit" }),
            ),
            make_node(
                "approve",
                FlowOperator::HumanApproval,
                serde_json::json!({ "title": "Approve replenish" }),
            ),
            make_node(
                "emit",
                FlowOperator::ActionEmit,
                serde_json::json!({
                    "actions": [{
                        "kind": "WRITE_NODE_METRIC",
                        "node_id": warehouse_id.to_string(),
                        "metric": "replenish_flag",
                        "value": 1
                    }]
                }),
            ),
        ],
        vec![
            make_edge("sql_check", "approve", Some("deficit > 0")),
            make_edge("approve", "emit", None),
        ],
    );
    store
        .inner
        .lock()
        .await
        .versions
        .insert((flow_id, 1), version);

    let (engine, tmp_dir) = build_test_engine(&store, None);
    let sub = ApprovalDecidedSubscriber::new(Arc::clone(&engine));

    let prod_run_id =
        run_approval_flow_once(&store, &engine, &sub, flow_id, Environment::Prod, 1).await?;
    assert_eq!(
        store
            .get_flow_run("default", prod_run_id)
            .await?
            .map(|r| r.status),
        Some(RunStatus::Succeeded)
    );
    assert!(
        store
            .inner
            .lock()
            .await
            .metrics
            .get(&(warehouse_id, String::from("replenish_flag")))
            .is_some_and(|v| (*v - 1.0).abs() < f64::EPSILON)
    );

    store.inner.lock().await.metrics.clear();
    let test_run_id =
        run_approval_flow_once(&store, &engine, &sub, flow_id, Environment::Test, 2).await?;
    assert!(store.inner.lock().await.metrics.is_empty());
    let actions = store
        .list_action_logs("default", ActionListQuery::default())
        .await?;
    assert!(
        actions
            .items
            .iter()
            .any(|a| a.run_id == Some(test_run_id) && a.status == ActionLogStatus::DryRun)
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

struct LargeAndFlakyOperator {
    calls: AtomicUsize,
}

impl Operator for LargeAndFlakyOperator {
    fn kind(&self) -> FlowOperator {
        FlowOperator::AggTrigger
    }
    fn display_name(&self) -> &'static str {
        "LargeFlaky"
    }
    fn description(&self) -> &'static str {
        "test"
    }
    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if ctx.node_id == "producer" {
                if n == 0 {
                    return Err(AppError::Upstream(String::from("transient 503")));
                }
                let big = "x".repeat(crate::INLINE_OUTPUT_MAX_BYTES + 2_048);
                return Ok(OperatorOutcome::Completed(serde_json::json!({
                    "blob": big,
                    "tag": "hydrated-ok"
                })));
            }
            Ok(OperatorOutcome::Completed(ctx.config.clone()))
        })
    }
}

#[tokio::test]
async fn engine_retries_exponentially_and_offloads_outputs_over_one_mib() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let flow_id = Uuid::new_v4();
    let mut producer = make_node("producer", FlowOperator::AggTrigger, serde_json::json!({}));
    producer.retry = RetryPolicy {
        max_attempts: 2,
        backoff_s: 1,
        backoff: BackoffStrategy::Exponential,
    };
    let consumer = make_node(
        "consumer",
        FlowOperator::AggTrigger,
        serde_json::json!({ "seen_tag": "{{ nodes.producer.output.tag }}" }),
    );
    store.inner.lock().await.versions.insert(
        (flow_id, 1),
        make_version(
            flow_id,
            vec![producer, consumer],
            vec![make_edge("producer", "consumer", None)],
        ),
    );

    let flaky: Arc<dyn Operator> = Arc::new(LargeAndFlakyOperator {
        calls: AtomicUsize::new(0),
    });
    let (engine, tmp_dir) = build_test_engine(&store, Some(flaky));

    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Test, None),
        )
        .await?;

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("producer"),
                attempt: 1,
            },
        )
        .await?;
    let steps_after_1 = store.list_run_steps("default", run.id).await?;
    assert!(
        steps_after_1
            .iter()
            .any(|s| s.attempt == 2 && s.status == StepStatus::Pending)
    );

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("producer"),
                attempt: 2,
            },
        )
        .await?;
    let steps_after_2 = store.list_run_steps("default", run.id).await?;
    let prod_step2 = steps_after_2
        .iter()
        .find(|s| s.node_id == "producer" && s.attempt == 2)
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    assert!(prod_step2.output.is_none());
    assert!(prod_step2.output_ref.is_some());

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("consumer"),
                attempt: 1,
            },
        )
        .await?;
    let final_run = store
        .get_flow_run("default", run.id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    assert_eq!(final_run.status, RunStatus::Succeeded);
    assert_eq!(
        final_run.outputs.get("consumer"),
        Some(&serde_json::json!({ "seen_tag": "hydrated-ok" }))
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

struct SlowCancellableOperator {
    started: Arc<Notify>,
    observed_cancel: Arc<AtomicBool>,
}

impl Operator for SlowCancellableOperator {
    fn kind(&self) -> FlowOperator {
        FlowOperator::AggTrigger
    }
    fn display_name(&self) -> &'static str {
        "SlowCancel"
    }
    fn description(&self) -> &'static str {
        "test slow"
    }
    fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move {
            self.started.notify_waiters();
            for _ in 0..400 {
                if ctx.is_canceled() {
                    self.observed_cancel.store(true, Ordering::SeqCst);
                    return Err(AppError::Conflict("RUN_CANCELED"));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(OperatorOutcome::Completed(
                serde_json::json!({ "done": true }),
            ))
        })
    }
}

#[tokio::test]
async fn engine_step_timeout_flips_cancel_flag_and_records_upstream_timeout() -> Result<(), AppError>
{
    let store = Arc::new(MockStore::default());
    let flow_id = Uuid::new_v4();
    let mut slow_node = make_node("slow_step", FlowOperator::AggTrigger, serde_json::json!({}));
    slow_node.timeout_s = 1;
    slow_node.retry.max_attempts = 1;
    store.inner.lock().await.versions.insert(
        (flow_id, 1),
        make_version(flow_id, vec![slow_node], Vec::new()),
    );

    let op: Arc<dyn Operator> = Arc::new(SlowCancellableOperator {
        started: Arc::new(Notify::new()),
        observed_cancel: Arc::new(AtomicBool::new(false)),
    });
    let (engine, tmp_dir) = build_test_engine(&store, Some(op));
    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Test, None),
        )
        .await?;

    let _ = engine
        .execute_step(
            "default",
            FlowStepJobPayload {
                run_id: run.id,
                node_id: String::from("slow_step"),
                attempt: 1,
            },
        )
        .await?;

    let steps = store.list_run_steps("default", run.id).await?;
    let step = steps.first().ok_or(AppError::NotFound("STEP_NOT_FOUND"))?;
    assert_eq!(step.status, StepStatus::Failed);
    assert_eq!(step.error_code.as_deref(), Some("UPSTREAM_TIMEOUT"));
    assert_eq!(
        store
            .get_flow_run("default", run.id)
            .await?
            .map(|r| r.status),
        Some(RunStatus::Failed)
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

#[tokio::test]
async fn engine_cancel_run_signals_inflight_operator_and_skips_pending() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let flow_id = Uuid::new_v4();
    store.inner.lock().await.versions.insert(
        (flow_id, 1),
        make_version(
            flow_id,
            vec![
                make_node("inflight", FlowOperator::AggTrigger, serde_json::json!({})),
                make_node(
                    "queued_root",
                    FlowOperator::AggTrigger,
                    serde_json::json!({}),
                ),
            ],
            Vec::new(),
        ),
    );

    let started = Arc::new(Notify::new());
    let observed_cancel = Arc::new(AtomicBool::new(false));
    let op: Arc<dyn Operator> = Arc::new(SlowCancellableOperator {
        started: Arc::clone(&started),
        observed_cancel: Arc::clone(&observed_cancel),
    });
    let (engine, tmp_dir) = build_test_engine(&store, Some(op));
    let (run, _) = store
        .start_flow_run(
            "default",
            make_start_params(flow_id, Environment::Test, None),
        )
        .await?;

    let eng_bg = Arc::clone(&engine);
    let notified = started.notified();
    let step_task = tokio::spawn(async move {
        eng_bg
            .execute_step(
                "default",
                FlowStepJobPayload {
                    run_id: run.id,
                    node_id: String::from("inflight"),
                    attempt: 1,
                },
            )
            .await
    });

    notified.await;
    let canceled_run = engine.cancel_run("default", run.id).await?;
    assert_eq!(canceled_run.status, RunStatus::Canceled);

    let step_res = step_task
        .await
        .map_err(|err| AppError::Config(format!("join error: {err}")))??;
    assert_eq!(step_res.get("status"), Some(&serde_json::json!("CANCELED")));
    assert!(observed_cancel.load(Ordering::SeqCst));

    let steps = store.list_run_steps("default", run.id).await?;
    assert!(
        steps
            .iter()
            .any(|s| s.node_id == "queued_root" && s.status == StepStatus::Skipped)
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

#[tokio::test]
async fn on_failure_fail_run_immediately_fails_and_skips_pending() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let (engine, tmp_dir) = build_test_engine(&store, None);

    let fail_flow_id = Uuid::new_v4();
    let mut hard_fail = make_node(
        "hard_fail",
        FlowOperator::AggTrigger,
        serde_json::json!({ "fail": true }),
    );
    hard_fail.retry.max_attempts = 1;
    hard_fail.on_failure = OnFailure::FailRun;
    store.inner.lock().await.versions.insert(
        (fail_flow_id, 1),
        make_version(
            fail_flow_id,
            vec![
                hard_fail,
                make_node(
                    "sibling_pending",
                    FlowOperator::AggTrigger,
                    serde_json::json!({}),
                ),
            ],
            Vec::new(),
        ),
    );
    let (r1, _) = store
        .start_flow_run(
            "default",
            make_start_params(fail_flow_id, Environment::Test, None),
        )
        .await?;
    let _ = run_single_step(&engine, r1.id, "hard_fail", 1).await?;
    assert_eq!(
        store
            .get_flow_run("default", r1.id)
            .await?
            .map(|r| r.status),
        Some(RunStatus::Failed)
    );
    assert!(
        store
            .list_run_steps("default", r1.id)
            .await?
            .iter()
            .any(|s| s.node_id == "sibling_pending" && s.status == StepStatus::Skipped)
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

#[tokio::test]
async fn on_failure_continue_completes_healthy_and_all_done_branches() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let (engine, tmp_dir) = build_test_engine(&store, None);

    let cont_flow_id = Uuid::new_v4();
    let mut soft_fail = make_node(
        "soft_fail",
        FlowOperator::AggTrigger,
        serde_json::json!({ "fail": true }),
    );
    soft_fail.retry.max_attempts = 1;
    soft_fail.on_failure = OnFailure::Continue;
    let mut all_done = make_node(
        "all_done",
        FlowOperator::AggTrigger,
        serde_json::json!({ "cleanup": true }),
    );
    all_done.join = JoinMode::AllDone;
    store.inner.lock().await.versions.insert(
        (cont_flow_id, 1),
        make_version(
            cont_flow_id,
            vec![
                soft_fail,
                make_node(
                    "healthy",
                    FlowOperator::AggTrigger,
                    serde_json::json!({ "ok": 1 }),
                ),
                all_done,
            ],
            vec![make_edge("soft_fail", "all_done", None)],
        ),
    );
    let (r2, _) = store
        .start_flow_run(
            "default",
            make_start_params(cont_flow_id, Environment::Test, None),
        )
        .await?;
    for nid in ["soft_fail", "healthy", "all_done"] {
        let _ = run_single_step(&engine, r2.id, nid, 1).await?;
    }
    let finished = store
        .get_flow_run("default", r2.id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    assert_eq!(finished.status, RunStatus::Succeeded);
    assert_eq!(
        finished.outputs.get("healthy"),
        Some(&serde_json::json!({ "ok": 1 }))
    );
    assert_eq!(
        finished.outputs.get("all_done"),
        Some(&serde_json::json!({ "cleanup": true }))
    );
    cleanup_dir(&tmp_dir).await;
    Ok(())
}

struct StrictSsrfGuard;
impl OutboundUrlGuardPort for StrictSsrfGuard {
    fn check_outbound_url<'a>(
        &'a self,
        url: &'a str,
        allow_private: bool,
    ) -> BoxFuture<'a, String> {
        Box::pin(async move {
            if !allow_private && (url.contains("169.254.") || url.contains("127.0.0.1")) {
                return Err(AppError::Forbidden("POLICY_DENIED"));
            }
            Ok(String::from(url))
        })
    }
}

struct NoopUnsealer;
impl FlowCredentialUnsealPort for NoopUnsealer {
    fn unseal_credential_json<'a>(
        &'a self,
        _t: &'a str,
        _id: Uuid,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async { Ok(serde_json::json!({})) })
    }
}

struct CountingAi {
    chats: AtomicUsize,
}
impl AiPort for CountingAi {
    fn embedding_model_name(&self) -> &'static str {
        "mock"
    }
    fn current_embedding_dim(&self) -> Option<usize> {
        Some(4)
    }
    fn embed_batch(
        &self,
        _i: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> {
        std::future::ready(Ok((Vec::new(), 4)))
    }
    fn embed_one(&self, _t: &str) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> {
        std::future::ready(Ok((vec![0.0; 4], 4)))
    }
    fn extract_triples(
        &self,
        _p: &str,
    ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
    fn extract_query_entities(
        &self,
        _q: &str,
    ) -> impl Future<Output = Result<Vec<String>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
    fn synthesize_answer(
        &self,
        _q: &str,
        _h: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> {
        std::future::ready(Ok(String::new()))
    }
    fn chat(&self, _sys: &str, _user: &str) -> impl Future<Output = Result<String, AppError>> {
        self.chats.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(String::from(r#"{"urgency": "HIGH"}"#)))
    }
}

#[tokio::test]
async fn http_call_ssrf_and_llm_extract_retry_cache_and_outbox_triggers() -> Result<(), AppError> {
    let store = Arc::new(MockStore::default());
    let http_op = HttpCallOperator::<MockStore, StrictSsrfGuard, NoopUnsealer>::new(
        Arc::clone(&store),
        Arc::new(StrictSsrfGuard),
        None,
    );
    let ssrf_ctx = OperatorContext::new(
        String::from("default"),
        Uuid::new_v4(),
        1,
        Uuid::new_v4(),
        String::from("h1"),
        1,
        Environment::Prod,
    )
    .with_execution_inputs(
        serde_json::json!({ "url": "http://169.254.169.254/latest/meta-data" }),
        FlowActorPrincipal::service(None),
        Arc::new(AtomicBool::new(false)),
    );
    let ssrf_res = http_op.execute(&ssrf_ctx).await;
    assert!(matches!(
        ssrf_res,
        Err(AppError::Forbidden("POLICY_DENIED"))
    ));

    let ai = Arc::new(CountingAi {
        chats: AtomicUsize::new(0),
    });
    let llm_op = LlmExtractOperator::new(Arc::clone(&ai), Arc::clone(&store));
    let run_id = Uuid::new_v4();
    let cfg = serde_json::json!({
        "text": "Urgent warehouse shortage",
        "output_schema": {
            "type": "object",
            "required": ["urgency"],
            "properties": { "urgency": { "type": "string" } }
        }
    });
    store.inner.lock().await.steps.insert(
        run_id,
        vec![StepRecord {
            id: 1,
            run_id,
            node_id: String::from("llm1"),
            attempt: 1,
            status: StepStatus::Succeeded,
            job_id: None,
            input: cfg.clone(),
            output: Some(serde_json::json!({ "urgency": "CACHED" })),
            output_ref: None,
            error_code: None,
            error_message: None,
            started_at: None,
            finished_at: None,
        }],
    );
    let llm_ctx = OperatorContext::new(
        String::from("default"),
        Uuid::new_v4(),
        1,
        run_id,
        String::from("llm1"),
        2,
        Environment::Test,
    )
    .with_execution_inputs(
        cfg,
        FlowActorPrincipal::service(None),
        Arc::new(AtomicBool::new(false)),
    );
    let cached_out = llm_op.execute(&llm_ctx).await?;
    assert_eq!(ai.chats.load(Ordering::SeqCst), 0);
    assert_eq!(
        cached_out,
        OperatorOutcome::Completed(serde_json::json!({ "urgency": "CACHED" }))
    );

    verify_outbox_triggers_and_cron_slot(&store).await
}

async fn verify_outbox_triggers_and_cron_slot(store: &Arc<MockStore>) -> Result<(), AppError> {
    store.inner.lock().await.event_flows = vec![
        EventFlowItem {
            tenant_id: String::from("tenant_a"),
            flow_id: Uuid::new_v4(),
            version: 1,
            trigger_cfg: TriggerCfg {
                kind: TriggerType::OutboxEvent,
                cron: None,
                timezone: None,
                catch_up: None,
                topic: Some(String::from("dataset.synced")),
                filter: Some(String::from("rows_written > 10")),
            },
        },
        EventFlowItem {
            tenant_id: String::from("tenant_b"),
            flow_id: Uuid::new_v4(),
            version: 1,
            trigger_cfg: TriggerCfg {
                kind: TriggerType::OutboxEvent,
                cron: None,
                timezone: None,
                catch_up: None,
                topic: Some(String::from("dataset.synced")),
                filter: None,
            },
        },
    ];
    let trig_sub = FlowOutboxTriggerSubscriber::new(Arc::clone(store));
    let ev = make_outbox_event(
        77,
        "tenant_a",
        OutboxTopic::DatasetSynced,
        serde_json::json!({ "rows_written": 50 }),
    );
    assert_eq!(trig_sub.dispatch_event(&ev).await?, 1);
    assert_eq!(trig_sub.dispatch_event(&ev).await?, 0);

    let tz = parse_timezone_offset("Asia/Shanghai")?;
    let t0 = Utc
        .with_ymd_and_hms(2026, 5, 1, 0, 0, 30)
        .single()
        .ok_or(AppError::NotFound("T"))?;
    let t1 = Utc
        .with_ymd_and_hms(2026, 5, 1, 0, 1, 5)
        .single()
        .ok_or(AppError::NotFound("T"))?;
    let slot = crate::due_cron_minute_slot("* * * * *", tz, t0, t1)?;
    assert!(slot.is_some());
    Ok(())
}

mod step10_causal_sim_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use axpoint_domain::AppError;
    use axpoint_domain::agent::{
        AgentTaskInput, AgentTaskOutcome, SimRunToolInput, SimRunToolOutput,
    };
    use axpoint_domain::axhub::{
        CatalogSearchHit, ColumnMeta, CreateDatasetRequest, CreateGlossaryTermRequest,
        CreateQualityRuleRequest, CreateWebhookRequest, Dataset, DatasetListQuery, GlossaryTerm,
        LineageEdge, NewUploadInput, PaginatedList, QualityRule, ResolvedWebhook, ScheduledDataset,
        SealedCredential, SidecarSyncResult, SyncDatasetResponse, SyncJob, UpdateColumnMetaRequest,
        UpdateDatasetRequest, UploadRecord, WebhookBufferItem, WebhookConfig,
    };
    use axpoint_domain::enums::{Environment, FlowOperator, InterventionMode};
    use axpoint_domain::flow::{
        FlowActorPrincipal, FlowGraph, Operator, OperatorContext, OperatorOutcome, TriggerCfg,
    };
    use axpoint_domain::platform::{
        BoxFuture, JobListQuery, JobListResponse, JobRecord, LeasedJob, NewJob,
    };
    use axpoint_domain::ports::{
        AgentTaskPort, DatasetRepository, JobQueuePort, LakeInternalQueryPort, SimExecutionPort,
        StudioExecutionPort, StudioRepository, TopologyRepository,
    };
    use axpoint_domain::rollup::{
        AcceptNodeCandidateRequest, AcceptNodeCandidateResponse, GraphAggregateJobPayload,
        KnowledgeNodeCandidatesResponse, KnowledgeSyncJobPayload, MapMaterializeJobPayload,
        MetricExplainResponse,
    };
    use axpoint_domain::sim::SimMode;
    use axpoint_domain::studio::{
        CreateDraftRequest, CreateSubgraphRequest, DocEntityGroup, DraftStatus, MappingRecord,
        MappingRunRecord, MergePreview, MergePreviewRequest, MergeSubgraphRequest,
        PublishDraftRequest, PublishDraftResponse, PublishedSchemaResponse,
        SaveSchemaDirectRequest, SchemaDiff, SchemaDraft, SchemaVersionDetail,
        SchemaVersionSummary, SubgraphInfo, TriggerMappingRunRequest, TriggerMappingRunResponse,
        UpdateDraftRequest, UpdateSubgraphRequest, ValidationContext,
    };
    use axpoint_domain::topology::{
        AxFlowDef, AxFlowRunResult, AxSandboxRecord, AxSimAction, AxSimImpactRow, AxSimResult,
        AxSimSnapshot, BlueLink, BlueNode, CreateAxFlowRequest, CreateAxSandboxRequest,
        SaveXBlueSchemaRequest, UpsertBlueNodeRequest, XBlueSchema,
    };
    use chrono::Utc;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    use super::{
        CountingAi, MockAnalytics, MockStore, NoopMat, NoopUnsealer, StrictSsrfGuard, make_node,
    };
    use crate::operators::causal_sim::{parse_sim_actions, parse_sim_params};
    use crate::{
        CausalSimOperator, Step07OperatorDeps, Step09OperatorDeps, Step10OperatorDeps,
        build_step10_operator_registry, validate_flow_default,
    };

    #[derive(Clone, Default)]
    struct MockSimPort {
        calls: Arc<Mutex<Vec<(String, Uuid, SimRunToolInput)>>>,
        use_top_impacts: bool,
    }

    impl SimExecutionPort for MockSimPort {
        fn run_simulation_tool<'a>(
            &'a self,
            tenant_id: &'a str,
            user_id: Uuid,
            input: SimRunToolInput,
        ) -> BoxFuture<'a, SimRunToolOutput> {
            Box::pin(async move {
                let sb_id = input.sandbox_id.unwrap_or_else(Uuid::new_v4);
                self.calls
                    .lock()
                    .await
                    .push((String::from(tenant_id), user_id, input));
                let scenario_id = Uuid::new_v4();
                let impact_row = serde_json::json!({
                    "node_id": Uuid::new_v4(),
                    "type_name": "Order",
                    "title": "重点客户订单",
                    "metric": "fulfillment",
                    "before_val": 1.0,
                    "after_val": 0.75,
                    "delta": -0.25,
                    "delta_ratio": -0.25,
                    "hop_distance": 2
                });
                let summary = if self.use_top_impacts {
                    serde_json::json!({
                        "top_impacts": [impact_row],
                        "kpis": [{ "type_name": "Order", "metric": "fulfillment", "before_sum": 1.0, "after_sum": 0.75, "delta": -0.25, "delta_ratio": -0.25, "affected_nodes": 1 }],
                        "converged": true,
                        "iterations": 4
                    })
                } else {
                    serde_json::json!({
                        "affected_count": 2,
                        "impacts": [impact_row],
                        "kpis": [{ "type_name": "Order", "metric": "fulfillment", "before_sum": 1.0, "after_sum": 0.75, "delta": -0.25, "delta_ratio": -0.25, "affected_nodes": 1 }],
                        "warnings": [],
                        "converged": true,
                        "iterations": 3
                    })
                };
                Ok(SimRunToolOutput {
                    sandbox_id: sb_id,
                    scenario_id: Some(scenario_id),
                    summary,
                })
            })
        }
    }

    #[derive(Default)]
    struct MockTopologyState {
        snapshot: Option<AxSimSnapshot>,
        sandbox_calls: Vec<(Uuid, Vec<AxSimAction>)>,
    }

    #[derive(Clone, Default)]
    struct MockTopologyRepo {
        state: Arc<Mutex<MockTopologyState>>,
    }

    impl TopologyRepository for MockTopologyRepo {
        fn get_xblue_schema(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<XBlueSchema, AppError>> {
            std::future::ready(Ok(XBlueSchema {
                node_types: Vec::new(),
                edge_types: Vec::new(),
            }))
        }
        fn save_xblue_schema(
            &self,
            _t: &str,
            _req: SaveXBlueSchemaRequest,
        ) -> impl Future<Output = Result<XBlueSchema, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        async fn get_runtime_snapshot(&self, _t: &str) -> Result<AxSimSnapshot, AppError> {
            let guard = self.state.lock().await;
            Ok(guard.snapshot.clone().unwrap_or(AxSimSnapshot {
                nodes: Vec::new(),
                links: Vec::new(),
            }))
        }
        fn upsert_blue_node(
            &self,
            _t: &str,
            _req: UpsertBlueNodeRequest,
        ) -> impl Future<Output = Result<BlueNode, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_ax_flows(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<AxFlowDef>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_ax_flow(
            &self,
            _t: &str,
            _req: CreateAxFlowRequest,
        ) -> impl Future<Output = Result<AxFlowDef, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn run_ax_flow(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<AxFlowRunResult, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_ax_sandboxes(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<AxSandboxRecord>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_ax_sandbox(
            &self,
            _t: &str,
            _req: CreateAxSandboxRequest,
        ) -> impl Future<Output = Result<AxSandboxRecord, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        async fn simulate_ax_sandbox(
            &self,
            _t: &str,
            sandbox_id: Uuid,
            actions: Vec<AxSimAction>,
        ) -> Result<AxSimResult, AppError> {
            self.state
                .lock()
                .await
                .sandbox_calls
                .push((sandbox_id, actions.clone()));
            let node_id = actions.first().map_or_else(Uuid::nil, |a| a.target_node_id);
            Ok(AxSimResult {
                impacts: vec![AxSimImpactRow {
                    node_id,
                    type_name: String::from("Factory"),
                    title: String::from("F-1"),
                    metric_key: String::from("output"),
                    before_val: 100.0,
                    after_val: 70.0,
                    delta: -30.0,
                    delta_ratio: Some(-0.3),
                    hop_distance: 0,
                    top_path: vec![node_id],
                }],
                affected_count: 1,
                kpis: Vec::new(),
                warnings: Vec::new(),
                converged: true,
                fired_events: Vec::new(),
                deadline_report: None,
            })
        }
    }

    impl TopologyRepository for MockStore {
        fn get_xblue_schema(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<XBlueSchema, AppError>> {
            std::future::ready(Ok(XBlueSchema {
                node_types: Vec::new(),
                edge_types: Vec::new(),
            }))
        }
        fn save_xblue_schema(
            &self,
            _t: &str,
            _req: SaveXBlueSchemaRequest,
        ) -> impl Future<Output = Result<XBlueSchema, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn get_runtime_snapshot(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<AxSimSnapshot, AppError>> {
            std::future::ready(Ok(AxSimSnapshot {
                nodes: Vec::new(),
                links: Vec::new(),
            }))
        }
        fn upsert_blue_node(
            &self,
            _t: &str,
            _req: UpsertBlueNodeRequest,
        ) -> impl Future<Output = Result<BlueNode, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_ax_flows(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<AxFlowDef>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_ax_flow(
            &self,
            _t: &str,
            _req: CreateAxFlowRequest,
        ) -> impl Future<Output = Result<AxFlowDef, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn run_ax_flow(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<AxFlowRunResult, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_ax_sandboxes(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<AxSandboxRecord>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_ax_sandbox(
            &self,
            _t: &str,
            _req: CreateAxSandboxRequest,
        ) -> impl Future<Output = Result<AxSandboxRecord, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn simulate_ax_sandbox(
            &self,
            _t: &str,
            _id: Uuid,
            _actions: Vec<AxSimAction>,
        ) -> impl Future<Output = Result<AxSimResult, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
    }

    impl JobQueuePort for MockStore {
        fn enqueue(&self, _t: &str, _j: NewJob) -> impl Future<Output = Result<Uuid, AppError>> {
            std::future::ready(Ok(Uuid::new_v4()))
        }
        fn lease(
            &self,
            _w: &str,
            _k: &[axpoint_domain::enums::JobKind],
            _l: u32,
            _ls: u32,
        ) -> impl Future<Output = Result<Vec<LeasedJob>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn heartbeat(
            &self,
            _j: &LeasedJob,
            _ls: u32,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn complete(
            &self,
            _j: &LeasedJob,
            _r: serde_json::Value,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn fail(&self, _j: &LeasedJob, _e: &str) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn list_jobs(
            &self,
            _t: &str,
            _q: JobListQuery,
        ) -> impl Future<Output = Result<JobListResponse, AppError>> {
            std::future::ready(Ok(JobListResponse {
                items: Vec::new(),
                next_cursor: None,
            }))
        }
        fn get_job(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<Option<JobRecord>, AppError>> {
            std::future::ready(Ok(None))
        }
    }

    impl StudioExecutionPort for MockStore {
        fn execute_mapping_materialize<L: LakeInternalQueryPort>(
            &self,
            _t: &str,
            _j: Uuid,
            _p: &MapMaterializeJobPayload,
            _l: &L,
        ) -> impl Future<Output = Result<MappingRunRecord, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_enabled_main_mappings_for_dataset(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<Vec<MappingRecord>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn execute_graph_aggregate(
            &self,
            _t: &str,
            _p: &GraphAggregateJobPayload,
        ) -> impl Future<Output = Result<serde_json::Value, AppError>> {
            std::future::ready(Ok(serde_json::json!({ "status": "SUCCEEDED" })))
        }
        fn enqueue_graph_aggregate(
            &self,
            _t: &str,
            _p: GraphAggregateJobPayload,
            _k: Option<String>,
        ) -> impl Future<Output = Result<Uuid, AppError>> {
            std::future::ready(Ok(Uuid::new_v4()))
        }
        fn explain_node_metric(
            &self,
            _t: &str,
            _n: Uuid,
            _m: &str,
        ) -> impl Future<Output = Result<MetricExplainResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn execute_knowledge_sync<E: axpoint_domain::ports::TextEmbedderPort>(
            &self,
            _t: &str,
            _p: &KnowledgeSyncJobPayload,
            _e: &E,
        ) -> impl Future<Output = Result<serde_json::Value, AppError>> {
            std::future::ready(Ok(serde_json::json!({})))
        }
        fn list_knowledge_node_candidates(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<KnowledgeNodeCandidatesResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn accept_knowledge_node_candidate(
            &self,
            _t: &str,
            _e: Uuid,
            _r: AcceptNodeCandidateRequest,
        ) -> impl Future<Output = Result<AcceptNodeCandidateResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
    }

    impl DatasetRepository for MockStore {
        fn list_datasets(
            &self,
            _t: &str,
            _q: DatasetListQuery,
        ) -> impl Future<Output = Result<PaginatedList<Dataset>, AppError>> {
            std::future::ready(Ok(PaginatedList {
                items: Vec::new(),
                next_cursor: None,
            }))
        }
        fn get_dataset(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<Option<Dataset>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn get_dataset_by_name(
            &self,
            _t: &str,
            _n: &str,
        ) -> impl Future<Output = Result<Option<Dataset>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn create_dataset(
            &self,
            _t: &str,
            _o: Option<Uuid>,
            _r: CreateDatasetRequest,
        ) -> impl Future<Output = Result<Dataset, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn update_dataset(
            &self,
            _t: &str,
            _id: Uuid,
            _r: UpdateDatasetRequest,
        ) -> impl Future<Output = Result<Option<Dataset>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn delete_dataset(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<Option<Dataset>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn enqueue_sync_job(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<SyncDatasetResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn start_sync_execution(
            &self,
            _t: &str,
            _d: Uuid,
            _j: Uuid,
        ) -> impl Future<Output = Result<SyncJob, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn complete_sync_execution(
            &self,
            _t: &str,
            _s: Uuid,
            _d: Uuid,
            _r: &SidecarSyncResult,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn fail_sync_execution(
            &self,
            _t: &str,
            _s: Uuid,
            _d: Uuid,
            _c: &str,
            _m: &str,
            _term: bool,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn list_sync_jobs(
            &self,
            _t: &str,
            _d: Uuid,
            _l: u32,
            _c: Option<&str>,
        ) -> impl Future<Output = Result<PaginatedList<SyncJob>, AppError>> {
            std::future::ready(Ok(PaginatedList {
                items: Vec::new(),
                next_cursor: None,
            }))
        }
        fn get_stream_offsets(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<BTreeMap<String, String>, AppError>> {
            std::future::ready(Ok(BTreeMap::new()))
        }
        fn list_columns(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<Vec<ColumnMeta>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn update_column_meta(
            &self,
            _t: &str,
            _d: Uuid,
            _c: &str,
            _r: UpdateColumnMetaRequest,
        ) -> impl Future<Output = Result<Option<ColumnMeta>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn list_quality_rules(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<Vec<QualityRule>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_quality_rule(
            &self,
            _t: &str,
            _d: Uuid,
            _r: CreateQualityRuleRequest,
        ) -> impl Future<Output = Result<QualityRule, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn delete_quality_rule(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<bool, AppError>> {
            std::future::ready(Ok(false))
        }
        fn find_committed_upload_by_sha(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<Option<UploadRecord>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn create_upload(
            &self,
            _t: &str,
            _i: NewUploadInput,
        ) -> impl Future<Output = Result<UploadRecord, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn get_upload(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<Option<UploadRecord>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn complete_upload(
            &self,
            _t: &str,
            _u: Uuid,
            _p: Uuid,
            _d: Option<Uuid>,
            _ds: &[Uuid],
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn fail_upload(
            &self,
            _t: &str,
            _u: Uuid,
            _c: &str,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn create_webhook(
            &self,
            _t: &str,
            _o: Option<Uuid>,
            _r: &CreateWebhookRequest,
            _c: SealedCredential,
        ) -> impl Future<Output = Result<(WebhookConfig, Dataset), AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_webhooks(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<WebhookConfig>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn delete_webhook(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<bool, AppError>> {
            std::future::ready(Ok(false))
        }
        fn resolve_webhook(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<Option<ResolvedWebhook>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn get_webhook_by_dataset(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<Option<WebhookConfig>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn ingest_webhook_event(
            &self,
            _t: &str,
            _w: Uuid,
            _d: &str,
            _p: serde_json::Value,
        ) -> impl Future<Output = Result<bool, AppError>> {
            std::future::ready(Ok(true))
        }
        fn list_pending_webhook_events(
            &self,
            _t: &str,
            _w: Uuid,
            _l: u32,
        ) -> impl Future<Output = Result<Vec<WebhookBufferItem>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn mark_webhook_events_flushed(
            &self,
            _t: &str,
            _ids: &[i64],
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn upsert_lineage(
            &self,
            _t: &str,
            _sk: &str,
            _si: &str,
            _dk: &str,
            _di: &str,
            _v: &str,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn get_dataset_lineage(
            &self,
            _t: &str,
            _d: Uuid,
            _dp: u32,
        ) -> impl Future<Output = Result<Vec<LineageEdge>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn search_catalog(
            &self,
            _t: &str,
            _q: &str,
            _l: u32,
        ) -> impl Future<Output = Result<PaginatedList<CatalogSearchHit>, AppError>> {
            std::future::ready(Ok(PaginatedList {
                items: Vec::new(),
                next_cursor: None,
            }))
        }
        fn list_glossary(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<GlossaryTerm>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_glossary_term(
            &self,
            _t: &str,
            _r: CreateGlossaryTermRequest,
        ) -> impl Future<Output = Result<GlossaryTerm, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn delete_glossary_term(
            &self,
            _t: &str,
            _id: Uuid,
        ) -> impl Future<Output = Result<bool, AppError>> {
            std::future::ready(Ok(false))
        }
        fn list_scheduled_datasets(
            &self,
        ) -> impl Future<Output = Result<Vec<ScheduledDataset>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn get_tenant_timezone(&self, _t: &str) -> impl Future<Output = Result<String, AppError>> {
            std::future::ready(Ok(String::from("Asia/Shanghai")))
        }
        fn get_lake_snapshot_keep_days(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<i32, AppError>> {
            std::future::ready(Ok(30))
        }
    }

    impl StudioRepository for MockStore {
        fn list_subgraphs(
            &self,
            _t: &str,
        ) -> impl Future<Output = Result<Vec<SubgraphInfo>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn create_subgraph(
            &self,
            _t: &str,
            _a: Option<Uuid>,
            _r: CreateSubgraphRequest,
        ) -> impl Future<Output = Result<SubgraphInfo, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn update_subgraph(
            &self,
            _t: &str,
            _s: &str,
            _r: UpdateSubgraphRequest,
        ) -> impl Future<Output = Result<SubgraphInfo, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn delete_subgraph(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<(), AppError>> {
            std::future::ready(Ok(()))
        }
        fn get_published_schema(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<PublishedSchemaResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn save_schema_direct(
            &self,
            _t: &str,
            _s: &str,
            _a: Option<Uuid>,
            _r: SaveSchemaDirectRequest,
        ) -> impl Future<Output = Result<PublishedSchemaResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn load_validation_context(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<ValidationContext, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_schema_versions(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<Vec<SchemaVersionSummary>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn get_schema_version(
            &self,
            _t: &str,
            _s: &str,
            _v: i32,
        ) -> impl Future<Output = Result<Option<SchemaVersionDetail>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn diff_schema_versions(
            &self,
            _t: &str,
            _s: &str,
            _f: i32,
            _to: i32,
        ) -> impl Future<Output = Result<SchemaDiff, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn restore_schema_version(
            &self,
            _t: &str,
            _s: &str,
            _v: i32,
            _a: Option<Uuid>,
        ) -> impl Future<Output = Result<SchemaDraft, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_drafts(
            &self,
            _t: &str,
            _s: &str,
            _st: Option<DraftStatus>,
        ) -> impl Future<Output = Result<Vec<SchemaDraft>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn get_draft(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<Option<SchemaDraft>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn create_draft(
            &self,
            _t: &str,
            _s: &str,
            _a: Option<Uuid>,
            _r: CreateDraftRequest,
        ) -> impl Future<Output = Result<SchemaDraft, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn update_draft(
            &self,
            _t: &str,
            _d: Uuid,
            _r: UpdateDraftRequest,
        ) -> impl Future<Output = Result<SchemaDraft, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn discard_draft(
            &self,
            _t: &str,
            _d: Uuid,
        ) -> impl Future<Output = Result<SchemaDraft, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn publish_draft(
            &self,
            _t: &str,
            _d: Uuid,
            _a: Option<Uuid>,
            _r: PublishDraftRequest,
        ) -> impl Future<Output = Result<PublishDraftResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn preview_subgraph_merge(
            &self,
            _t: &str,
            _s: &str,
            _r: MergePreviewRequest,
        ) -> impl Future<Output = Result<MergePreview, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn merge_subgraph_into_main(
            &self,
            _t: &str,
            _s: &str,
            _a: Option<Uuid>,
            _r: MergeSubgraphRequest,
        ) -> impl Future<Output = Result<PublishDraftResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn list_mappings(
            &self,
            _t: &str,
            _s: &str,
        ) -> impl Future<Output = Result<Vec<MappingRecord>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn get_mapping(
            &self,
            _t: &str,
            _m: Uuid,
        ) -> impl Future<Output = Result<Option<MappingRecord>, AppError>> {
            std::future::ready(Ok(None))
        }
        fn list_mapping_runs(
            &self,
            _t: &str,
            _m: Uuid,
            _l: u32,
        ) -> impl Future<Output = Result<Vec<MappingRunRecord>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
        fn enqueue_mapping_run(
            &self,
            _t: &str,
            _m: Uuid,
            _r: TriggerMappingRunRequest,
        ) -> impl Future<Output = Result<TriggerMappingRunResponse, AppError>> {
            std::future::ready(Err(AppError::NotFound("MOCK")))
        }
        fn load_doc_entity_groups(
            &self,
            _t: &str,
            _d: &[Uuid],
        ) -> impl Future<Output = Result<Vec<DocEntityGroup>, AppError>> {
            std::future::ready(Ok(Vec::new()))
        }
    }

    struct MockLake;
    impl LakeInternalQueryPort for MockLake {
        fn query_internal(
            &self,
            _t: &str,
            sql: &str,
            _max_rows: usize,
        ) -> impl Future<Output = Result<axpoint_domain::analytics::QueryResult, AppError>>
        {
            std::future::ready(Ok(axpoint_domain::analytics::QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                row_count: 0,
                truncated: false,
                sql: String::from(sql),
                duration_ms: 1,
            }))
        }
        fn invalidate_tenant_session(&self, _t: &str) -> impl Future<Output = bool> {
            std::future::ready(false)
        }
    }

    struct MockAgentTaskPort;
    impl AgentTaskPort for MockAgentTaskPort {
        fn execute_flow_agent_task(
            &self,
            input: AgentTaskInput,
        ) -> BoxFuture<'_, AgentTaskOutcome> {
            Box::pin(async move {
                Ok(AgentTaskOutcome::Completed {
                    conversation_id: Uuid::nil(),
                    answer: input.instruction,
                    structured: None,
                })
            })
        }
    }

    fn make_ctx(user_id: Uuid, env: Environment, config: serde_json::Value) -> OperatorContext {
        OperatorContext::new(
            String::from("default"),
            Uuid::new_v4(),
            1,
            Uuid::new_v4(),
            String::from("sim_step"),
            1,
            env,
        )
        .with_execution_inputs(
            config,
            FlowActorPrincipal::service(Some(user_id)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn parse_sim_actions_supports_ratio_add_absolute_at_step_and_legacy_delta_ratio()
    -> Result<(), AppError> {
        let n1 = Uuid::new_v4();
        let n2 = Uuid::new_v4();
        let n3 = Uuid::new_v4();
        let n4 = Uuid::new_v4();

        let cfg = serde_json::json!({
            "actions": [
                { "target_node_id": n1.to_string(), "metric_key": "output", "mode": "RATIO", "value": -0.3, "at_step": 1 },
                { "target_node_id": n2.to_string(), "metric_key": "inventory", "mode": "ADD", "value": 40.0, "at_step": 2 },
                { "target_node_id": n3.to_string(), "metric_key": "fulfillment", "mode": "ABSOLUTE", "value": 0.95, "at_step": 3 },
                { "node_id": n4.to_string(), "metric": "output", "delta_ratio": -0.5 }
            ]
        });

        let parsed = parse_sim_actions(&cfg)?;
        assert_eq!(parsed.len(), 4);

        let a0 = parsed.first().ok_or(AppError::NotFound("A0"))?;
        assert_eq!(a0.target_node_id, n1);
        assert_eq!(a0.metric_key, "output");
        assert_eq!(a0.mode, Some(InterventionMode::Ratio));
        assert_eq!(a0.value, Some(-0.3));
        assert_eq!(a0.delta_ratio, None);
        assert_eq!(a0.at_step, 1);

        let a1 = parsed.get(1).ok_or(AppError::NotFound("A1"))?;
        assert_eq!(a1.mode, Some(InterventionMode::Add));
        assert_eq!(a1.value, Some(40.0));
        assert_eq!(a1.at_step, 2);

        let a2 = parsed.get(2).ok_or(AppError::NotFound("A2"))?;
        assert_eq!(a2.mode, Some(InterventionMode::Absolute));
        assert_eq!(a2.value, Some(0.95));
        assert_eq!(a2.at_step, 3);

        let a3 = parsed.get(3).ok_or(AppError::NotFound("A3"))?;
        assert_eq!(a3.target_node_id, n4);
        assert_eq!(a3.metric_key, "output");
        assert_eq!(a3.mode, None);
        assert_eq!(a3.delta_ratio, Some(-0.5));
        assert_eq!(a3.at_step, 0);

        let conflict = serde_json::json!({
            "actions": [{ "target_node_id": n1.to_string(), "metric_key": "output", "mode": "RATIO", "value": -0.2, "delta_ratio": -0.2 }]
        });
        assert!(parse_sim_actions(&conflict).is_err());

        let too_low = serde_json::json!({
            "actions": [{ "target_node_id": n1.to_string(), "metric_key": "output", "mode": "RATIO", "value": -1.0 }]
        });
        assert!(parse_sim_actions(&too_low).is_err());
        Ok(())
    }

    #[test]
    fn parse_sim_params_and_schema_validation_support_step10_options() -> Result<(), AppError> {
        let cfg = serde_json::json!({
            "sandbox_id": "{{ trigger.sandbox_id }}",
            "actions": [
                { "target_node_id": Uuid::new_v4().to_string(), "metric_key": "output", "mode": "ADD", "value": -20.0, "at_step": 2 }
            ],
            "params": {
                "mode": "STEPPED",
                "horizon_steps": 24,
                "damping": 0.9,
                "max_iters": 300,
                "tol": 1e-7,
                "max_hops": 4,
                "uncertainty": { "enabled": true, "samples": 150, "sigma_rel": 0.15, "seed": 77 }
            }
        });

        let params = parse_sim_params(&cfg)?;
        assert_eq!(params.mode, SimMode::Stepped);
        assert_eq!(params.horizon_steps, 24);
        assert!((params.damping - 0.9).abs() < 1e-12);
        assert_eq!(params.max_iterations, 300);
        assert!((params.tolerance - 1e-7).abs() < 1e-15);
        assert_eq!(params.max_hops, Some(4));
        let unc = params.uncertainty.ok_or(AppError::NotFound("UNC"))?;
        assert_eq!(unc.samples, 150);
        assert!((unc.coefficient_cv - 0.15).abs() < 1e-12);
        assert_eq!(unc.seed, 77);

        let disabled_unc = parse_sim_params(&serde_json::json!({
            "params": { "uncertainty": { "enabled": false, "samples": 100 } }
        }))?;
        assert!(disabled_unc.uncertainty.is_none());
        assert!(
            parse_sim_params(&serde_json::json!({ "params": { "horizon_steps": 200 } })).is_err()
        );

        let valid_graph = FlowGraph {
            nodes: vec![make_node("sim", FlowOperator::CausalSim, cfg)],
            edges: Vec::new(),
        };
        let validation = validate_flow_default(&valid_graph, &TriggerCfg::default(), None);
        assert!(validation.valid, "issues: {:?}", validation.issues);

        let invalid_graph = FlowGraph {
            nodes: vec![make_node(
                "sim_bad",
                FlowOperator::CausalSim,
                serde_json::json!({
                    "actions": [{ "target_node_id": Uuid::new_v4().to_string(), "metric_key": "output" }]
                }),
            )],
            edges: Vec::new(),
        };
        let bad_val = validate_flow_default(&invalid_graph, &TriggerCfg::default(), None);
        assert!(!bad_val.valid);
        assert!(bad_val.issues.iter().any(|i| i.code == "SCHEMA_INVALID"));
        Ok(())
    }

    #[tokio::test]
    async fn causal_sim_with_sim_port_verifies_input_and_propagates_output() -> Result<(), AppError>
    {
        let topo = Arc::new(MockTopologyRepo::default());
        let sim_port = Arc::new(MockSimPort::default());
        let sim_port_ref = Arc::clone(&sim_port);
        let sim_dyn: Arc<dyn SimExecutionPort> = sim_port_ref;
        let op = CausalSimOperator::with_sim_port(topo, sim_dyn);

        let sb_id = Uuid::new_v4();
        let node_a = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        let ctx = make_ctx(
            actor_id,
            Environment::Prod,
            serde_json::json!({
                "sandbox_id": sb_id.to_string(),
                "scenario_name": "east_china_cut",
                "actions": [
                    { "target_node_id": node_a.to_string(), "metric_key": "output", "mode": "RATIO", "value": -0.3, "at_step": 1 }
                ],
                "params": {
                    "mode": "STEPPED",
                    "horizon_steps": 16,
                    "damping": 0.8,
                    "max_iters": 200,
                    "tol": 1e-8,
                    "max_hops": 5,
                    "uncertainty": { "enabled": true, "samples": 80, "sigma_rel": 0.1, "seed": 99 }
                }
            }),
        );

        let OperatorOutcome::Completed(out) = op.execute(&ctx).await? else {
            return Err(AppError::Config(String::from("expected Completed")));
        };
        assert_eq!(out.get("sandbox_id"), Some(&serde_json::json!(sb_id)));
        assert!(out.get("scenario_id").is_some());
        assert_eq!(out.get("affected_count"), Some(&serde_json::json!(2)));
        assert_eq!(out.get("converged"), Some(&serde_json::json!(true)));
        assert_eq!(out.get("iterations"), Some(&serde_json::json!(3)));
        assert!(
            out.get("impacts")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|rows| rows.len() == 1)
        );
        assert!(
            out.get("kpis")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|rows| rows.len() == 1)
        );

        let recorded = sim_port.calls.lock().await;
        let (tenant, uid, input) = recorded.first().ok_or(AppError::NotFound("CALL"))?;
        assert_eq!(tenant, "default");
        assert_eq!(*uid, actor_id);
        assert_eq!(input.sandbox_id, Some(sb_id));
        assert_eq!(input.scenario_name.as_deref(), Some("east_china_cut"));
        assert_eq!(input.max_hops, Some(5));
        assert_eq!(
            input
                .interventions
                .get("params")
                .and_then(|p| p.get("horizon_steps")),
            Some(&serde_json::json!(16))
        );
        assert_eq!(
            input
                .interventions
                .get("params")
                .and_then(|p| p.get("uncertainty"))
                .and_then(|u| u.get("coefficient_cv")),
            Some(&serde_json::json!(0.1))
        );
        Ok(())
    }

    #[tokio::test]
    async fn causal_sim_with_sim_port_handles_top_impacts_and_array_interventions()
    -> Result<(), AppError> {
        let top_port = Arc::new(MockSimPort {
            calls: Arc::new(Mutex::new(Vec::new())),
            use_top_impacts: true,
        });
        let top_port_ref = Arc::clone(&top_port);
        let top_dyn: Arc<dyn SimExecutionPort> = top_port_ref;
        let op = CausalSimOperator::with_sim_port(Arc::new(MockTopologyRepo::default()), top_dyn);
        let node_a = Uuid::new_v4();
        let actor_id = Uuid::new_v4();
        let ctx = make_ctx(
            actor_id,
            Environment::Test,
            serde_json::json!({
                "actions": [{ "target_node_id": node_a.to_string(), "metric_key": "output", "delta_ratio": -0.25 }]
            }),
        );
        let OperatorOutcome::Completed(out) = op.execute(&ctx).await? else {
            return Err(AppError::Config(String::from("expected Completed")));
        };
        assert_eq!(out.get("affected_count"), Some(&serde_json::json!(1)));
        assert!(
            out.get("impacts")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|rows| rows.len() == 1)
        );
        let rec = top_port.calls.lock().await;
        let (_, _, input) = rec.first().ok_or(AppError::NotFound("CALL2"))?;
        assert!(input.interventions.is_array());
        Ok(())
    }

    #[tokio::test]
    async fn causal_sim_without_sim_port_falls_back_to_topology_snapshot() -> Result<(), AppError> {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let now = Utc::now();
        let snapshot = AxSimSnapshot {
            nodes: vec![
                BlueNode {
                    id: id_a,
                    type_name: String::from("Factory"),
                    node_key: String::from("F-1"),
                    title: String::from("工厂1"),
                    properties: serde_json::Map::new(),
                    metrics: BTreeMap::from([(String::from("output"), 100.0)]),
                    geo: None,
                    geo_shape: None,
                    source_dataset_id: None,
                    source_row_key: None,
                    row_version: 1,
                    created_at: now,
                    updated_at: now,
                },
                BlueNode {
                    id: id_b,
                    type_name: String::from("Warehouse"),
                    node_key: String::from("W-1"),
                    title: String::from("仓库1"),
                    properties: serde_json::Map::new(),
                    metrics: BTreeMap::from([(String::from("output"), 50.0)]),
                    geo: None,
                    geo_shape: None,
                    source_dataset_id: None,
                    source_row_key: None,
                    row_version: 1,
                    created_at: now,
                    updated_at: now,
                },
            ],
            links: vec![BlueLink {
                id: Uuid::new_v4(),
                link_type: String::from("SUPPLIES"),
                src_node_id: id_a,
                dst_node_id: id_b,
                weight: 0.8,
            }],
        };
        let topo = Arc::new(MockTopologyRepo {
            state: Arc::new(Mutex::new(MockTopologyState {
                snapshot: Some(snapshot),
                sandbox_calls: Vec::new(),
            })),
        });
        let op = CausalSimOperator::new(Arc::clone(&topo));

        let ctx = make_ctx(
            Uuid::nil(),
            Environment::Test,
            serde_json::json!({
                "actions": [{ "target_node_id": id_a.to_string(), "metric_key": "output", "delta_ratio": -0.5 }]
            }),
        );
        let OperatorOutcome::Completed(out) = op.execute(&ctx).await? else {
            return Err(AppError::Config(String::from("expected Completed")));
        };
        assert_eq!(out.get("affected_count"), Some(&serde_json::json!(2)));
        assert_eq!(out.get("converged"), Some(&serde_json::json!(true)));
        Ok(())
    }

    #[tokio::test]
    async fn build_step10_operator_registry_wires_causal_sim_with_sim_port() -> Result<(), AppError>
    {
        let store = Arc::new(MockStore::default());
        let sim_port = Arc::new(MockSimPort::default());
        let sim_port_ref = Arc::clone(&sim_port);
        let sim_dyn: Arc<dyn SimExecutionPort> = sim_port_ref;
        let registry = build_step10_operator_registry(Step10OperatorDeps {
            step09: Step09OperatorDeps {
                step07: Step07OperatorDeps {
                    store: Arc::clone(&store),
                    analytics: Arc::new(MockAnalytics),
                    sql_materializer: Some(Arc::new(NoopMat)),
                    ai: Arc::new(CountingAi {
                        chats: std::sync::atomic::AtomicUsize::new(0),
                    }),
                    lake: Arc::new(MockLake),
                    url_guard: Arc::new(StrictSsrfGuard),
                    cred_unsealer: Some(Arc::new(NoopUnsealer)),
                },
                agent_task: Arc::new(MockAgentTaskPort),
            },
            sim_port: sim_dyn,
        });

        for kind in FlowOperator::ALL {
            assert!(registry.contains(*kind), "missing operator {kind}");
        }

        let op = registry
            .get(FlowOperator::CausalSim)
            .ok_or(AppError::NotFound("CAUSAL_SIM_OP"))?;
        let sb_id = Uuid::new_v4();
        let node_id = Uuid::new_v4();
        let ctx = make_ctx(
            Uuid::new_v4(),
            Environment::Prod,
            serde_json::json!({
                "sandbox_id": sb_id.to_string(),
                "actions": [
                    { "target_node_id": node_id.to_string(), "metric_key": "inventory", "mode": "ADD", "value": -10.0, "at_step": 0 }
                ]
            }),
        );
        let OperatorOutcome::Completed(out) = op.execute(&ctx).await? else {
            return Err(AppError::Config(String::from("expected Completed")));
        };
        assert_eq!(out.get("sandbox_id"), Some(&serde_json::json!(sb_id)));
        assert_eq!(sim_port.calls.lock().await.len(), 1);
        Ok(())
    }
}
