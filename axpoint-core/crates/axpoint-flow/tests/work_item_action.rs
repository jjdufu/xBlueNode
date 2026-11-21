//! Step 14 integration tests for `ACTION_EMIT` `WORK_ITEM` execution (`12-STEP14` §14.7).

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{Environment, FlowOperator};
use axpoint_domain::flow::{
    ActionKind, ActionListQuery, ActionLogRecord, ActionLogStatus, ApprovalListQuery,
    ApprovalRecord, DecideApprovalParams, DeployFlowRequest, EventFlowItem, FlowDeploymentRecord,
    FlowGraph, FlowListQuery, FlowNode, FlowVersionRecord, FlowVersionsListResponse, JoinMode,
    NewActionLogInput, NewApprovalInput, OnFailure, Operator as _, OperatorContext,
    OperatorOutcome, Pos, ProgressionPlannerFn, PublishFlowVersionRequest, ResolvedFlowHook,
    RetryPolicy, RunListQuery, RunRecord, ScheduledFlowItem, StartRunParams, StepMutation,
    StepRecord, TriggerCfg, UpdateAxFlowRequest,
};
use axpoint_domain::graph::{
    BulkNodesRequest, BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery,
    LinkRecord, MetricCatalogEntry, MetricSeriesPoint, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchQuery, NodeSearchResponse, PatchNodeRequest,
    ResolveEntitiesRequest, ResolveEntitiesResponse, UpsertLinkRequest, WindowQuery,
    WindowQueryResponse, WriteMetricsRequest, WriteMetricsResponse,
};
use axpoint_domain::ops::{
    FlowWorkItemInput, WorkItem, WorkItemKind, WorkItemPriority, WorkItemSourceKind, WorkItemStatus,
};
use axpoint_domain::ports::{FlowRepository, GraphStorePort, MetricStorePort};
use axpoint_domain::topology::{AxFlowDef, BlueNode, CreateAxFlowRequest, UpsertBlueNodeRequest};
use axpoint_flow::{ActionEmitOperator, validate_flow_default};
use chrono::Utc;
use uuid::Uuid;

#[derive(Default)]
struct WorkItemTestState {
    nodes: Vec<BlueNode>,
    action_logs: Vec<ActionLogRecord>,
    created_items: Vec<WorkItem>,
    next_action_id: i64,
    next_item_no: i64,
}

#[derive(Default)]
struct WorkItemMockStore {
    st: Mutex<WorkItemTestState>,
}

impl WorkItemMockStore {
    fn lock_st(&self) -> std::sync::MutexGuard<'_, WorkItemTestState> {
        self.st
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn unused<T>() -> Result<T, AppError> {
    Err(AppError::Config(String::from("unused in WORK_ITEM test")))
}

impl FlowRepository for WorkItemMockStore {
    fn list_flows(
        &self,
        _: &str,
        _: FlowListQuery,
    ) -> impl Future<Output = Result<PaginatedList<AxFlowDef>, AppError>> + Send {
        ready(unused())
    }

    fn get_flow(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Option<AxFlowDef>, AppError>> + Send {
        ready(unused())
    }

    fn create_flow(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: CreateAxFlowRequest,
    ) -> impl Future<Output = Result<AxFlowDef, AppError>> + Send {
        ready(unused())
    }

    fn update_flow(
        &self,
        _: &str,
        _: Uuid,
        _: UpdateAxFlowRequest,
    ) -> impl Future<Output = Result<Option<AxFlowDef>, AppError>> + Send {
        ready(unused())
    }

    fn delete_flow(&self, _: &str, _: Uuid) -> impl Future<Output = Result<bool, AppError>> + Send {
        ready(unused())
    }

    fn publish_flow_version(
        &self,
        _: &str,
        _: Uuid,
        _: Option<Uuid>,
        _: PublishFlowVersionRequest,
    ) -> impl Future<Output = Result<FlowVersionRecord, AppError>> + Send {
        ready(unused())
    }

    fn list_flow_versions(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<FlowVersionsListResponse, AppError>> + Send {
        ready(unused())
    }

    fn get_flow_version(
        &self,
        _: &str,
        _: Uuid,
        _: i32,
    ) -> impl Future<Output = Result<Option<FlowVersionRecord>, AppError>> + Send {
        ready(unused())
    }

    fn deploy_flow(
        &self,
        _: &str,
        _: Uuid,
        _: Option<Uuid>,
        _: DeployFlowRequest,
    ) -> impl Future<Output = Result<FlowDeploymentRecord, AppError>> + Send {
        ready(unused())
    }

    fn get_flow_deployment(
        &self,
        _: &str,
        _: Uuid,
        _: Environment,
    ) -> impl Future<Output = Result<Option<FlowDeploymentRecord>, AppError>> + Send {
        ready(unused())
    }

    fn start_flow_run(
        &self,
        _: &str,
        _: StartRunParams,
    ) -> impl Future<Output = Result<(RunRecord, bool), AppError>> + Send {
        ready(unused())
    }

    fn get_flow_run(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Option<RunRecord>, AppError>> + Send {
        ready(unused())
    }

    fn list_flow_runs(
        &self,
        _: &str,
        _: Uuid,
        _: RunListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RunRecord>, AppError>> + Send {
        ready(unused())
    }

    fn list_run_steps(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Vec<StepRecord>, AppError>> + Send {
        ready(unused())
    }

    fn list_latest_run_steps(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Vec<StepRecord>, AppError>> + Send {
        ready(unused())
    }

    fn mark_step_running(
        &self,
        _: &str,
        _: Uuid,
        _: &str,
        _: i32,
        _: Option<Uuid>,
        _: &serde_json::Value,
    ) -> impl Future<Output = Result<StepRecord, AppError>> + Send {
        ready(unused())
    }

    fn find_cached_step_output(
        &self,
        _: &str,
        _: Uuid,
        _: &str,
        _: &serde_json::Value,
    ) -> impl Future<Output = Result<Option<serde_json::Value>, AppError>> + Send {
        ready(unused())
    }

    fn apply_step_mutation_and_progress(
        &self,
        _: &str,
        _: Uuid,
        _: StepMutation,
        _: ProgressionPlannerFn<'_>,
    ) -> impl Future<Output = Result<RunRecord, AppError>> + Send {
        ready(unused())
    }

    fn cancel_flow_run(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<RunRecord, AppError>> + Send {
        ready(unused())
    }

    fn retry_flow_run(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<RunRecord, AppError>> + Send {
        ready(unused())
    }

    fn create_or_get_approval(
        &self,
        _: &str,
        _: NewApprovalInput,
    ) -> impl Future<Output = Result<ApprovalRecord, AppError>> + Send {
        ready(unused())
    }

    fn get_approval(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<Option<ApprovalRecord>, AppError>> + Send {
        ready(unused())
    }

    fn list_approvals(
        &self,
        _: &str,
        _: Uuid,
        _: &[String],
        _: ApprovalListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ApprovalRecord>, AppError>> + Send {
        ready(unused())
    }

    fn decide_approval(
        &self,
        _: &str,
        _: DecideApprovalParams,
    ) -> impl Future<Output = Result<ApprovalRecord, AppError>> + Send {
        ready(unused())
    }

    fn expire_overdue_approvals(
        &self,
        _: &str,
    ) -> impl Future<Output = Result<u64, AppError>> + Send {
        ready(unused())
    }

    fn record_action_log(
        &self,
        _: &str,
        input: NewActionLogInput,
    ) -> impl Future<Output = Result<(ActionLogRecord, bool), AppError>> + Send {
        let mut g = self.lock_st();
        if let Some(ex) = g
            .action_logs
            .iter()
            .find(|a| a.idempotency_key == input.idempotency_key)
            .cloned()
        {
            return ready(Ok((ex, true)));
        }
        g.next_action_id = g.next_action_id.saturating_add(1);
        let rec = ActionLogRecord {
            id: g.next_action_id,
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
        g.action_logs.push(rec.clone());
        ready(Ok((rec, false)))
    }

    fn get_action_log_by_key(
        &self,
        _: &str,
        idempotency_key: &str,
    ) -> impl Future<Output = Result<Option<ActionLogRecord>, AppError>> + Send {
        let found = self
            .lock_st()
            .action_logs
            .iter()
            .find(|a| a.idempotency_key == idempotency_key)
            .cloned();
        ready(Ok(found))
    }

    fn list_action_logs(
        &self,
        _: &str,
        _: ActionListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ActionLogRecord>, AppError>> + Send {
        ready(Ok(PaginatedList {
            items: self.lock_st().action_logs.clone(),
            next_cursor: None,
        }))
    }

    fn upsert_flow_hook(
        &self,
        _: &str,
        _: Uuid,
        _: Uuid,
    ) -> impl Future<Output = Result<(Uuid, chrono::DateTime<Utc>), AppError>> + Send {
        ready(unused())
    }

    fn resolve_flow_hook(
        &self,
        _: &str,
        _: &str,
    ) -> impl Future<Output = Result<Option<ResolvedFlowHook>, AppError>> + Send {
        ready(unused())
    }

    fn list_scheduled_flows(
        &self,
    ) -> impl Future<Output = Result<Vec<ScheduledFlowItem>, AppError>> + Send {
        ready(unused())
    }

    fn list_event_flows(
        &self,
        _: &str,
    ) -> impl Future<Output = Result<Vec<EventFlowItem>, AppError>> + Send {
        ready(unused())
    }

    fn list_active_tenants(&self) -> impl Future<Output = Result<Vec<String>, AppError>> + Send {
        ready(unused())
    }

    fn create_work_item_from_flow(
        &self,
        tenant_id: &str,
        input: FlowWorkItemInput,
    ) -> impl Future<Output = Result<WorkItem, AppError>> + Send {
        let mut g = self.lock_st();
        g.next_item_no = g.next_item_no.saturating_add(1);
        let item_no = g.next_item_no;
        let now = Utc::now();
        let item = WorkItem {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            item_no,
            code: format!("WI-{item_no}"),
            title: input.title,
            description: input.description,
            item_kind: input.item_kind,
            status: WorkItemStatus::Open,
            priority: input.priority,
            assignee: input.assignee,
            source_kind: WorkItemSourceKind::FlowAction,
            source_ref: input.run_id.to_string(),
            node_ids: input.node_ids,
            due_at: input.due_at,
            created_by: input.actor,
            created_at: now,
            updated_at: now,
            closed_at: None,
        };
        g.created_items.push(item.clone());
        ready(Ok(item))
    }
}

impl GraphStorePort for WorkItemMockStore {
    fn list_nodes(
        &self,
        _: &str,
        q: NodeListQuery,
    ) -> impl Future<Output = Result<NodeListResponse, AppError>> + Send {
        let items: Vec<BlueNode> = self
            .lock_st()
            .nodes
            .iter()
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
        _: Uuid,
    ) -> impl Future<Output = Result<Option<BlueNode>, AppError>> + Send {
        ready(unused())
    }

    fn upsert_node(
        &self,
        _: &str,
        _: UpsertBlueNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send {
        ready(unused())
    }

    fn patch_node(
        &self,
        _: &str,
        _: Uuid,
        _: PatchNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> + Send {
        ready(unused())
    }

    fn delete_node(&self, _: &str, _: Uuid) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unused())
    }

    fn bulk_upsert(
        &self,
        _: &str,
        _: BulkNodesRequest,
    ) -> impl Future<Output = Result<BulkNodesResponse, AppError>> + Send {
        ready(unused())
    }

    fn list_links(
        &self,
        _: &str,
        _: LinkListQuery,
    ) -> impl Future<Output = Result<Vec<LinkRecord>, AppError>> + Send {
        ready(unused())
    }

    fn upsert_link(
        &self,
        _: &str,
        _: UpsertLinkRequest,
    ) -> impl Future<Output = Result<LinkRecord, AppError>> + Send {
        ready(unused())
    }

    fn delete_link(&self, _: &str, _: Uuid) -> impl Future<Output = Result<(), AppError>> + Send {
        ready(unused())
    }

    fn search_nodes(
        &self,
        _: &str,
        _: NodeSearchQuery,
        _: Option<&[f32]>,
    ) -> impl Future<Output = Result<NodeSearchResponse, AppError>> + Send {
        ready(unused())
    }

    fn list_node_documents(
        &self,
        _: &str,
        _: Uuid,
    ) -> impl Future<Output = Result<NodeDocumentsResponse, AppError>> + Send {
        ready(unused())
    }

    fn resolve_entity_links(
        &self,
        _: &str,
        _: ResolveEntitiesRequest,
    ) -> impl Future<Output = Result<ResolveEntitiesResponse, AppError>> + Send {
        ready(unused())
    }

    fn list_entity_candidates(
        &self,
        _: &str,
    ) -> impl Future<Output = Result<EntityCandidatesResponse, AppError>> + Send {
        ready(unused())
    }

    fn confirm_entity_link(
        &self,
        _: &str,
        _: Uuid,
        _: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> + Send {
        ready(unused())
    }

    fn reject_entity_link(
        &self,
        _: &str,
        _: Uuid,
        _: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> + Send {
        ready(unused())
    }
}

impl MetricStorePort for WorkItemMockStore {
    fn write_metrics(
        &self,
        _: &str,
        _: WriteMetricsRequest,
    ) -> impl Future<Output = Result<WriteMetricsResponse, AppError>> + Send {
        ready(unused())
    }

    fn query_node_metrics(
        &self,
        _: &str,
        _: Uuid,
        _: &str,
        _: NodeMetricSeriesQuery,
    ) -> impl Future<Output = Result<Vec<MetricSeriesPoint>, AppError>> + Send {
        ready(unused())
    }

    fn query_window(
        &self,
        _: &str,
        _: WindowQuery,
    ) -> impl Future<Output = Result<WindowQueryResponse, AppError>> + Send {
        ready(unused())
    }

    fn list_metric_catalog(
        &self,
        _: &str,
    ) -> impl Future<Output = Result<Vec<MetricCatalogEntry>, AppError>> + Send {
        ready(unused())
    }

    fn run_rollup_for_tenant(&self, _: &str) -> impl Future<Output = Result<u64, AppError>> + Send {
        ready(unused())
    }
}

fn sample_work_item_cfg_and_dag() -> (serde_json::Value, FlowGraph) {
    let cfg = serde_json::json!({
        "actions": [{
            "kind": "WORK_ITEM",
            "title": "门禁工作项",
            "priority": "P1",
            "item_kind": "ISSUE",
            "node_refs": ["Machine:M-1"]
        }]
    });
    let dag = FlowGraph {
        nodes: vec![FlowNode {
            id: String::from("emit_wi"),
            kind: FlowOperator::ActionEmit,
            label: String::from("建工作项"),
            config: cfg.clone(),
            retry: RetryPolicy::default(),
            timeout_s: 60,
            on_failure: OnFailure::FailRun,
            join: JoinMode::AllSuccess,
            pos: Pos { x: 0.0, y: 0.0 },
        }],
        edges: Vec::new(),
    };
    (cfg, dag)
}

#[tokio::test]
async fn action_emit_work_item_records_dry_run_in_test_and_executes_idempotently_in_prod()
-> Result<(), AppError> {
    let store = Arc::new(WorkItemMockStore::default());
    let machine_id = Uuid::new_v4();
    store.lock_st().nodes.push(BlueNode {
        id: machine_id,
        type_name: String::from("Machine"),
        node_key: String::from("M-1"),
        title: String::from("一号机床"),
        properties: serde_json::Map::new(),
        metrics: std::collections::BTreeMap::new(),
        geo: None,
        source_dataset_id: None,
        source_row_key: None,
        row_version: 1,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });

    let (cfg, dag) = sample_work_item_cfg_and_dag();
    assert!(validate_flow_default(&dag, &TriggerCfg::default(), None).valid);

    let op = ActionEmitOperator::new(Arc::clone(&store), Arc::clone(&store), Arc::clone(&store));
    let flow_id = Uuid::new_v4();

    // 1. TEST environment -> DRY_RUN without calling create_work_item_from_flow.
    let mut test_ctx = OperatorContext::new(
        String::from("default"),
        flow_id,
        1,
        Uuid::new_v4(),
        String::from("emit_wi"),
        1,
        Environment::Test,
    );
    test_ctx.config = cfg.clone();
    let test_out = op.execute(&test_ctx).await?;
    assert!(matches!(test_out, OperatorOutcome::Completed(_)));
    assert!(store.lock_st().created_items.is_empty());
    assert_eq!(
        store
            .lock_st()
            .action_logs
            .first()
            .map(|a| (a.action_kind, a.status)),
        Some((ActionKind::WorkItem, ActionLogStatus::DryRun))
    );

    // 2. PROD environment -> invokes create_work_item_from_flow and records DONE.
    let prod_run_id = Uuid::new_v4();
    let mut prod_ctx = OperatorContext::new(
        String::from("default"),
        flow_id,
        1,
        prod_run_id,
        String::from("emit_wi"),
        1,
        Environment::Prod,
    );
    prod_ctx.config = cfg;
    let _prod_out = op.execute(&prod_ctx).await?;
    let _retry_out = op.execute(&prod_ctx).await?;

    let st = store.lock_st();
    assert_eq!(st.created_items.len(), 1);
    let wi = st
        .created_items
        .first()
        .ok_or_else(|| AppError::Config(String::new()))?;
    assert_eq!(wi.title, "门禁工作项");
    assert_eq!(wi.priority, WorkItemPriority::P1);
    assert_eq!(wi.item_kind, WorkItemKind::Issue);
    assert_eq!(wi.source_kind, WorkItemSourceKind::FlowAction);
    assert_eq!(wi.source_ref, prod_run_id.to_string());
    assert_eq!(wi.node_ids, vec![machine_id]);
    assert_eq!(
        st.action_logs.last().map(|a| (a.action_kind, a.status)),
        Some((ActionKind::WorkItem, ActionLogStatus::Done))
    );
    Ok(())
}
