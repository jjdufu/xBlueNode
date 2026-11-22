//! Domain repository trait implementations and enterprise seed data for [`super::MockStep09Store`].

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    AskFeedbackKind, AskTurnRecord, CreateMetricRequest, CreateSavedQueryRequest, FewShotTurn,
    MetricDef, NewAskTurnInput, NewQueryHistoryEntry, NewTransformDraftInput, QueryHistoryEntry,
    QueryHistoryListQuery, SavedQuery, SchemaRecallBundle, TransformDraft, TransformPreview,
    UpdateMetricRequest, UpdateSavedQueryRequest,
};
use axpoint_domain::axhub::{
    CatalogSearchHit, ColumnMeta, CreateDatasetRequest, CreateGlossaryTermRequest,
    CreateQualityRuleRequest, CreateWebhookRequest, Dataset, DatasetListQuery, GlossaryTerm,
    LineageEdge, NewUploadInput, PaginatedList, QualityReport, QualityRule, ResolvedWebhook,
    ScheduledDataset, SealedCredential, SidecarSyncResult, SyncDatasetResponse, SyncJob,
    UpdateColumnMetaRequest, UpdateDatasetRequest, UploadRecord, WebhookBufferItem, WebhookConfig,
};
use axpoint_domain::enums::{AlertStatus, Environment, GraphAlgo, Permission, RunStatus};
use axpoint_domain::flow::{
    ActionListQuery, ActionLogRecord, ApprovalDecision, ApprovalListQuery, ApprovalRecord,
    ApprovalStatus, DecideApprovalParams, DeployFlowRequest, EventFlowItem, FlowDeploymentRecord,
    FlowListQuery, FlowVersionRecord, FlowVersionsListResponse, NewActionLogInput,
    NewApprovalInput, ProgressionPlannerFn, PublishFlowVersionRequest, ResolvedFlowHook,
    RunListQuery, RunRecord, ScheduledFlowItem, StartRunParams, StepMutation, StepRecord,
    UpdateAxFlowRequest,
};
use axpoint_domain::graph::{
    AlgoResultsQuery, AlgoResultsResponse, AlgoRunRequest, AlgoRunResponse, BulkNodesRequest,
    BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery, LinkRecord,
    MetricCatalogEntry, MetricSeriesPoint, NeighborQuery, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchHit, NodeSearchQuery, NodeSearchResponse,
    PatchNodeRequest, PathQuery, PathResponse, ResolveEntitiesRequest, ResolveEntitiesResponse,
    SubgraphQuery, SubgraphResponse, UpsertLinkRequest, WindowQuery, WindowQueryResponse,
    WriteMetricsRequest, WriteMetricsResponse,
};
use axpoint_domain::platform::{ApiTokenRecord, PrincipalRecord};
use axpoint_domain::ports::{
    AnalyticsRepository, CredentialStore, DatasetRepository, FlowRepository, GraphQueryPort,
    GraphStorePort, MetricStorePort, RulesRepository, StudioRepository,
};
use axpoint_domain::rules::{
    AbsenceRuleItem, AckBatchResponse, AlertEventKind, AlertEventRecord, AlertEventsQuery,
    AlertListQuery, AlertRecord, AlertSilenceRecord, CreateRuleRequest, CreateSilenceRequest,
    DeliveryFailureUpdate, DueDeliveryItem, EnqueueDeliveryInput, FalseConditionOutcome,
    NotifyChannelRecord, NotifyDeliveryRecord, RuleDef, RuleListQuery, RuleScopeKind,
    SaveNotifyChannelInput, SimplePageQuery, TrueConditionInput, TrueConditionOutcome,
    UpdateRuleRequest,
};
use axpoint_domain::studio::{
    CreateDraftRequest, CreateSubgraphRequest, DocEntityGroup, DraftStatus, MappingRecord,
    MappingRunRecord, MergePreview, MergePreviewRequest, MergeSubgraphRequest, PublishDraftRequest,
    PublishDraftResponse, PublishedSchemaResponse, SaveSchemaDirectRequest, SchemaDiff,
    SchemaDraft, SchemaVersionDetail, SchemaVersionSummary, SubgraphInfo, TriggerMappingRunRequest,
    TriggerMappingRunResponse, UpdateDraftRequest, UpdateSubgraphRequest, ValidationContext,
};
use axpoint_domain::topology::{AxFlowDef, BlueNode, CreateAxFlowRequest, UpsertBlueNodeRequest};
use chrono::Utc;
use uuid::Uuid;

use super::{ALERT_A1_ID, FLOW_F1_ID, MockStep09Store, SUPPLIER_S1_ID, WAREHOUSE_W1_ID};

fn from_json<T: serde::de::DeserializeOwned>(val: serde_json::Value) -> Result<T, AppError> {
    serde_json::from_value(val).map_err(|e| AppError::Config(format!("seed deserialize: {e}")))
}

fn build_seed_graph(
    w1_id: Uuid,
    s1_id: Uuid,
) -> Result<(PublishedSchemaResponse, Vec<BlueNode>, Vec<LinkRecord>), AppError> {
    let schema: PublishedSchemaResponse = from_json(serde_json::json!({
        "subgraph_slug": "main",
        "version": 3,
        "node_types": [
            {
                "name": "Warehouse", "display_name": "仓库", "description": "区域履约中心",
                "key_property": "code", "title_property": "name",
                "properties": [
                    { "name": "code", "display_name": "编码", "data_type": "STRING", "required": true },
                    { "name": "region", "display_name": "大区", "data_type": "STRING", "required": false },
                    { "name": "manager_phone", "display_name": "负责人手机", "data_type": "STRING", "required": false, "pii_category": "PHONE" }
                ],
                "metrics": [
                    { "name": "inventory_qty", "display_name": "库存数量", "unit": "pcs", "kind": "SYNC" },
                    { "name": "safety_score", "display_name": "安全评分", "unit": "pt", "kind": "MANUAL" }
                ]
            },
            {
                "name": "Supplier", "display_name": "供应商", "description": "核心物料供应商",
                "key_property": "code", "title_property": "name", "properties": [], "metrics": []
            }
        ],
        "edge_types": [{
            "name": "SUPPLIED_BY", "display_name": "由其供货",
            "src_type_name": "Warehouse", "dst_type_name": "Supplier", "properties": []
        }],
        "mappings": []
    }))?;
    let w1: BlueNode = from_json(serde_json::json!({
        "id": w1_id, "type_name": "Warehouse", "node_key": "W-1", "title": "华东一号仓",
        "properties": { "code": "W-1", "region": "华东", "manager_phone": "13812345678" },
        "metrics": { "inventory_qty": 320.0, "safety_threshold": 500.0 }, "row_version": 1
    }))?;
    let s1: BlueNode = from_json(serde_json::json!({
        "id": s1_id, "type_name": "Supplier", "node_key": "S-1", "title": "苏州精密供应商",
        "properties": { "code": "S-1" }, "metrics": { "on_time_rate": 0.98 }, "row_version": 1
    }))?;
    let link: LinkRecord = from_json(serde_json::json!({
        "id": Uuid::new_v4(), "link_type": "SUPPLIED_BY", "src_node_id": w1_id, "dst_node_id": s1_id,
        "weight": 0.85, "properties": {},
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
    }))?;
    Ok((schema, vec![w1, s1], vec![link]))
}

fn build_seed_analytics(ds_id: Uuid) -> Result<(Dataset, ColumnMeta, MetricDef), AppError> {
    let ds: Dataset = from_json(serde_json::json!({
        "id": ds_id, "connection_id": null, "name": "demo_orders",
        "display_name": "销售订单宽表", "description": "企业全渠道订单事实表",
        "source_kind": "FILE_TABLE", "source_ref": {}, "sync_mode": "SNAPSHOT",
        "primary_key": ["order_id"], "cursor_column": null, "cursor_value": null,
        "delete_column": null, "schema_policy": "ADDITIVE",
        "lake_schema": "tenant_default", "lake_table": "demo_orders",
        "schema_json": [], "schema_version": 1, "row_count": 1200, "last_snapshot_id": 1,
        "last_synced_at": "2026-01-01T00:00:00Z", "schedule_cron": null, "tags": ["orders"],
        "owner_user_id": null, "status": "READY",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
    }))?;
    let col: ColumnMeta = from_json(serde_json::json!({
        "dataset_id": ds_id, "column_name": "amount", "data_type": "DECIMAL",
        "pii_category": "NONE", "pii_confirmed": true, "description": "订单实付金额",
        "business_term": "GMV", "unit": "CNY", "ordinal": 1
    }))?;
    let metric: MetricDef = from_json(serde_json::json!({
        "id": Uuid::new_v4(), "name": "paid_amount", "display_name": "已支付金额",
        "description": "已支付状态订单金额合计", "dataset_id": ds_id,
        "expression": "SUM(amount)", "filter_sql": "status = 'PAID'", "time_column": "paid_at",
        "default_grain": "MONTH",
        "dimensions": [
            { "name": "status", "column": "status", "display_name": "订单状态" },
            { "name": "region", "column": "region", "display_name": "大区" }
        ],
        "unit": "CNY", "format": "0,0.00", "synonyms": ["实收金额"],
        "certified": true, "schema_version": 1, "broken": false, "owner_user_id": null, "version": 2,
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
    }))?;
    Ok((ds, col, metric))
}

/// Populates `store` with realistic enterprise fixtures (`Warehouse` `W-1`, `Supplier` `S-1`,
/// `demo_orders` dataset, `paid_amount` metric, `Alert` `A-1`, and `PROD` flow `F-1`).
///
/// # Errors
/// Returns [`AppError`] if any embedded JSON fixture fails deserialization.
pub async fn populate_enterprise_seed(store: &MockStep09Store) -> Result<(), AppError> {
    let w1_id = Uuid::parse_str(WAREHOUSE_W1_ID).map_err(|e| AppError::Config(e.to_string()))?;
    let s1_id = Uuid::parse_str(SUPPLIER_S1_ID).map_err(|e| AppError::Config(e.to_string()))?;
    let a1_id = Uuid::parse_str(ALERT_A1_ID).map_err(|e| AppError::Config(e.to_string()))?;
    let f1_id = Uuid::parse_str(FLOW_F1_ID).map_err(|e| AppError::Config(e.to_string()))?;
    let ds_id = Uuid::from_u128(0x401);

    let (schema, nodes, links) = build_seed_graph(w1_id, s1_id)?;
    let (ds, col, metric) = build_seed_analytics(ds_id)?;
    let alert: AlertRecord = from_json(serde_json::json!({
        "id": a1_id, "rule_id": Uuid::new_v4(), "dedup_key": format!("rule:{w1_id}"),
        "severity": "CRITICAL", "status": "OPEN",
        "title": "华东一号仓安全库存跌破红线", "body": "当前库存 320 低于安全红线 500",
        "node_id": w1_id, "dataset_id": null, "last_value": 320.0,
        "detail": { "warehouse": "W-1" }, "occurrences": 2,
        "first_seen_at": "2026-01-02T00:00:00Z", "last_seen_at": "2026-01-02T01:00:00Z",
        "last_false_at": null, "acked_by": null, "acked_at": null, "escalated_at": null,
        "resolved_at": null, "resolve_reason": null, "deliveries": []
    }))?;
    let p1: MetricSeriesPoint =
        from_json(serde_json::json!({ "ts": "2026-01-01T00:00:00Z", "value": 360.0 }))?;
    let p2: MetricSeriesPoint =
        from_json(serde_json::json!({ "ts": "2026-01-02T00:00:00Z", "value": 320.0 }))?;

    let mut guard = store.inner.lock().await;
    guard.schema = Some(schema);
    guard.subgraphs = vec![SubgraphInfo {
        id: Uuid::from_u128(0x501),
        slug: String::from("main"),
        display_name: String::from("企业主图"),
        description: String::from("企业全局标准模型主图"),
        owner_dept: String::new(),
        is_main: true,
        is_dev_sandbox: false,
        source_template_id: None,
        last_reset_at: None,
        current_version: 3,
        created_by: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }];
    guard.nodes = nodes;
    guard.links = links;
    guard
        .series
        .insert((w1_id, String::from("inventory_qty")), vec![p1, p2]);
    guard.datasets = vec![ds];
    guard.columns.insert(ds_id, vec![col]);
    guard.metrics.insert(String::from("paid_amount"), metric);
    guard.alerts = vec![alert];
    guard.flow_deployments.insert(
        (f1_id, Environment::Prod),
        FlowDeploymentRecord {
            flow_id: f1_id,
            environment: Environment::Prod,
            version: 1,
            deployed_by: None,
            deployed_at: Utc::now(),
        },
    );
    guard.principals.insert(
        (String::from("default"), Uuid::from_u128(1)),
        PrincipalRecord {
            user_id: Uuid::from_u128(1),
            tenant_id: String::from("default"),
            username: String::from("ops_admin"),
            display_name: String::from("Ops Admin"),
            dept_id: String::from("east_china"),
            roles: vec![String::from("ADMIN")],
            permissions: Permission::ALL.iter().copied().collect(),
            clearance: axpoint_domain::enums::Sensitivity::Secret,
            tokens_valid_after: chrono::DateTime::UNIX_EPOCH,
        },
    );
    Ok(())
}

impl CredentialStore for MockStep09Store {
    fn resolve_api_token(
        &self,
        _token_hash: &str,
    ) -> impl Future<Output = Result<Option<ApiTokenRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn touch_api_token(
        &self,
        _tenant_id: &str,
        _token_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    async fn load_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Option<PrincipalRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .principals
            .get(&(String::from(tenant_id), user_id))
            .cloned())
    }
}

impl DatasetRepository for MockStep09Store {
    async fn list_datasets(
        &self,
        _tenant_id: &str,
        _query: DatasetListQuery,
    ) -> Result<PaginatedList<Dataset>, AppError> {
        let guard = self.inner.lock().await;
        Ok(PaginatedList {
            items: guard.datasets.clone(),
            next_cursor: None,
        })
    }

    async fn get_dataset(&self, _tenant_id: &str, id: Uuid) -> Result<Option<Dataset>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.datasets.iter().find(|d| d.id == id).cloned())
    }

    async fn get_dataset_by_name(
        &self,
        _tenant_id: &str,
        name: &str,
    ) -> Result<Option<Dataset>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.datasets.iter().find(|d| d.name == name).cloned())
    }

    fn create_dataset(
        &self,
        _tenant_id: &str,
        _owner_user_id: Option<Uuid>,
        _req: CreateDatasetRequest,
    ) -> impl Future<Output = Result<Dataset, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn update_dataset(
        &self,
        _tenant_id: &str,
        _id: Uuid,
        _req: UpdateDatasetRequest,
    ) -> impl Future<Output = Result<Option<Dataset>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn delete_dataset(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<Option<Dataset>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn enqueue_sync_job(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
    ) -> impl Future<Output = Result<SyncDatasetResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn start_sync_execution(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
        _job_id: Uuid,
    ) -> impl Future<Output = Result<SyncJob, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn complete_sync_execution(
        &self,
        _tenant_id: &str,
        _sync_job_id: Uuid,
        _dataset_id: Uuid,
        _result: &SidecarSyncResult,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn fail_sync_execution(
        &self,
        _tenant_id: &str,
        _sync_job_id: Uuid,
        _dataset_id: Uuid,
        _error_code: &str,
        _error_message: &str,
        _terminal: bool,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn list_sync_jobs(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
        _limit: u32,
        _cursor: Option<&str>,
    ) -> impl Future<Output = Result<PaginatedList<SyncJob>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn get_stream_offsets(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, AppError>> {
        std::future::ready(Ok(BTreeMap::new()))
    }

    async fn list_columns(
        &self,
        _tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<Vec<ColumnMeta>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.columns.get(&dataset_id).cloned().unwrap_or_default())
    }

    fn update_column_meta(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
        _column_name: &str,
        _req: UpdateColumnMetaRequest,
    ) -> impl Future<Output = Result<Option<ColumnMeta>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_quality_rules(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<QualityRule>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn create_quality_rule(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
        _req: CreateQualityRuleRequest,
    ) -> impl Future<Output = Result<QualityRule, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn delete_quality_rule(
        &self,
        _tenant_id: &str,
        _rule_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn find_committed_upload_by_sha(
        &self,
        _tenant_id: &str,
        _sha256: &str,
    ) -> impl Future<Output = Result<Option<UploadRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn create_upload(
        &self,
        _tenant_id: &str,
        _input: NewUploadInput,
    ) -> impl Future<Output = Result<UploadRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn get_upload(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<Option<UploadRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn complete_upload(
        &self,
        _tenant_id: &str,
        _upload_id: Uuid,
        _parse_id: Uuid,
        _doc_id: Option<Uuid>,
        _dataset_ids: &[Uuid],
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn fail_upload(
        &self,
        _tenant_id: &str,
        _upload_id: Uuid,
        _error_code: &str,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn create_webhook(
        &self,
        _tenant_id: &str,
        _owner_user_id: Option<Uuid>,
        _req: &CreateWebhookRequest,
        _credential: SealedCredential,
    ) -> impl Future<Output = Result<(WebhookConfig, Dataset), AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_webhooks(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<WebhookConfig>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn delete_webhook(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn resolve_webhook(
        &self,
        _tenant_id: &str,
        _slug: &str,
    ) -> impl Future<Output = Result<Option<ResolvedWebhook>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn get_webhook_by_dataset(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
    ) -> impl Future<Output = Result<Option<WebhookConfig>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn ingest_webhook_event(
        &self,
        _tenant_id: &str,
        _webhook_id: Uuid,
        _delivery_id: &str,
        _payload: serde_json::Value,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn list_pending_webhook_events(
        &self,
        _tenant_id: &str,
        _webhook_id: Uuid,
        _limit: u32,
    ) -> impl Future<Output = Result<Vec<WebhookBufferItem>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn mark_webhook_events_flushed(
        &self,
        _tenant_id: &str,
        _ids: &[i64],
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn upsert_lineage(
        &self,
        _tenant_id: &str,
        _src_kind: &str,
        _src_id: &str,
        _dst_kind: &str,
        _dst_id: &str,
        _via: &str,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn get_dataset_lineage(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
        _depth: u32,
    ) -> impl Future<Output = Result<Vec<LineageEdge>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn search_catalog(
        &self,
        _tenant_id: &str,
        _query: &str,
        _limit: u32,
    ) -> impl Future<Output = Result<PaginatedList<CatalogSearchHit>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn list_glossary(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<GlossaryTerm>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn create_glossary_term(
        &self,
        _tenant_id: &str,
        _req: CreateGlossaryTermRequest,
    ) -> impl Future<Output = Result<GlossaryTerm, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn delete_glossary_term(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn list_scheduled_datasets(
        &self,
    ) -> impl Future<Output = Result<Vec<ScheduledDataset>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_tenant_timezone(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<String, AppError>> {
        std::future::ready(Ok(String::from("Asia/Shanghai")))
    }

    fn get_lake_snapshot_keep_days(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<i32, AppError>> {
        std::future::ready(Ok(30))
    }
}

impl AnalyticsRepository for MockStep09Store {
    async fn list_metrics(&self, _tenant_id: &str) -> Result<Vec<MetricDef>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.metrics.values().cloned().collect())
    }

    async fn get_metric_by_name(
        &self,
        _tenant_id: &str,
        name: &str,
    ) -> Result<Option<MetricDef>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.metrics.get(name).cloned())
    }

    fn create_metric(
        &self,
        _tenant_id: &str,
        _owner_user_id: Option<Uuid>,
        _schema_version: i32,
        _req: CreateMetricRequest,
    ) -> impl Future<Output = Result<MetricDef, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    async fn update_metric(
        &self,
        _tenant_id: &str,
        name: &str,
        _schema_version: i32,
        req: UpdateMetricRequest,
    ) -> Result<Option<MetricDef>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(m) = guard.metrics.get_mut(name) else {
            return Ok(None);
        };
        if let Some(f) = req.filter_sql {
            m.filter_sql = f;
        }
        m.version = m.version.saturating_add(1);
        Ok(Some(m.clone()))
    }

    fn delete_metric(
        &self,
        _tenant_id: &str,
        _name: &str,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn list_saved_queries(
        &self,
        _tenant_id: &str,
        _viewer_user_id: Uuid,
        _include_all: bool,
    ) -> impl Future<Output = Result<Vec<SavedQuery>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn get_saved_query(
        &self,
        _tenant_id: &str,
        id: Uuid,
    ) -> Result<Option<SavedQuery>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.saved_queries.get(&id).cloned())
    }

    fn create_saved_query(
        &self,
        _tenant_id: &str,
        _owner_user_id: Option<Uuid>,
        _req: CreateSavedQueryRequest,
    ) -> impl Future<Output = Result<SavedQuery, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn update_saved_query(
        &self,
        _tenant_id: &str,
        _id: Uuid,
        _req: UpdateSavedQueryRequest,
    ) -> impl Future<Output = Result<Option<SavedQuery>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn delete_saved_query(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn record_query_history(
        &self,
        _tenant_id: &str,
        _entry: NewQueryHistoryEntry,
    ) -> impl Future<Output = Result<i64, AppError>> {
        std::future::ready(Ok(1))
    }

    fn list_query_history(
        &self,
        _tenant_id: &str,
        _only_user_id: Option<Uuid>,
        _query: QueryHistoryListQuery,
    ) -> impl Future<Output = Result<PaginatedList<QueryHistoryEntry>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn insert_ask_turn(
        &self,
        _tenant_id: &str,
        _turn: NewAskTurnInput,
    ) -> impl Future<Output = Result<AskTurnRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn get_ask_turn(
        &self,
        _tenant_id: &str,
        _turn_id: Uuid,
    ) -> impl Future<Output = Result<Option<AskTurnRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn update_ask_feedback(
        &self,
        _tenant_id: &str,
        _turn_id: Uuid,
        _feedback: AskFeedbackKind,
        _corrected_sql: Option<&str>,
    ) -> impl Future<Output = Result<Option<AskTurnRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_session_turns(
        &self,
        _tenant_id: &str,
        _session_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AskTurnRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn search_few_shot_turns(
        &self,
        _tenant_id: &str,
        _question: &str,
        _limit: u32,
    ) -> impl Future<Output = Result<Vec<FewShotTurn>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn recall_schema_candidates(
        &self,
        _tenant_id: &str,
        _question: &str,
        _query_embedding: Option<&[f32]>,
        _restrict_datasets: &[Uuid],
    ) -> impl Future<Output = Result<SchemaRecallBundle, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_unembedded_analytics_items(
        &self,
        _tenant_id: &str,
        _model_name: &str,
        _limit: u32,
    ) -> impl Future<Output = Result<Vec<(String, Uuid, String)>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn upsert_analytics_embeddings(
        &self,
        _tenant_id: &str,
        _model_name: &str,
        _dim: usize,
        _items: &[(String, Uuid, Vec<f32>)],
    ) -> impl Future<Output = Result<usize, AppError>> {
        std::future::ready(Ok(0))
    }

    fn create_transform_draft(
        &self,
        _tenant_id: &str,
        _input: NewTransformDraftInput,
    ) -> impl Future<Output = Result<TransformDraft, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn get_transform_draft(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn update_transform_draft(
        &self,
        _tenant_id: &str,
        _id: Uuid,
        _sql_text: &str,
        _preview: &TransformPreview,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn mark_transform_applied(
        &self,
        _tenant_id: &str,
        _id: Uuid,
        _dataset_id: Uuid,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn mark_transform_discarded(
        &self,
        _tenant_id: &str,
        _id: Uuid,
    ) -> impl Future<Output = Result<Option<TransformDraft>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_dependent_derived_datasets(
        &self,
        _tenant_id: &str,
        _upstream_dataset_id: Uuid,
    ) -> impl Future<Output = Result<Vec<Dataset>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
}

impl StudioRepository for MockStep09Store {
    async fn list_subgraphs(&self, _tenant_id: &str) -> Result<Vec<SubgraphInfo>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.subgraphs.clone())
    }

    async fn create_subgraph(
        &self,
        _tenant_id: &str,
        actor_user_id: Option<Uuid>,
        req: CreateSubgraphRequest,
    ) -> Result<SubgraphInfo, AppError> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        let info = SubgraphInfo {
            id: Uuid::new_v4(),
            slug: req.slug,
            display_name: req.display_name,
            description: req.description,
            owner_dept: req.owner_dept,
            is_main: false,
            is_dev_sandbox: req.is_dev_sandbox,
            source_template_id: req.source_template_id,
            last_reset_at: None,
            current_version: 0,
            created_by: actor_user_id,
            created_at: now,
            updated_at: now,
        };
        guard.subgraphs.push(info.clone());
        Ok(info)
    }

    fn update_subgraph(
        &self,
        _tenant_id: &str,
        _slug: &str,
        _req: UpdateSubgraphRequest,
    ) -> impl Future<Output = Result<SubgraphInfo, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn delete_subgraph(
        &self,
        _tenant_id: &str,
        _slug: &str,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    async fn reset_dev_subgraph_nodes(
        &self,
        _tenant_id: &str,
        slug: &str,
        web_base_url: Option<&str>,
    ) -> Result<axpoint_domain::builder::DevSubgraphResetResponse, AppError> {
        let mut guard = self.inner.lock().await;
        let sg = guard
            .subgraphs
            .iter_mut()
            .find(|s| s.slug == slug)
            .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;
        if sg.is_main || !sg.is_dev_sandbox {
            return Err(AppError::bad(
                "DEV_SANDBOX_REQUIRED",
                "target subgraph must be a non-main dev sandbox",
            ));
        }
        let now = Utc::now();
        sg.last_reset_at = Some(now);
        sg.updated_at = now;
        Ok(axpoint_domain::builder::DevSubgraphResetResponse {
            subgraph_slug: String::from(slug),
            nodes_deleted: 2,
            links_deleted: 1,
            series_points_deleted: 4,
            last_reset_at: now,
            deep_links: axpoint_domain::builder::resolve_deep_links(
                web_base_url,
                None,
                slug,
                None,
                None,
                None,
            ),
        })
    }

    async fn get_published_schema(
        &self,
        _tenant_id: &str,
        subgraph_slug: &str,
    ) -> Result<PublishedSchemaResponse, AppError> {
        let guard = self.inner.lock().await;
        if subgraph_slug != "main" {
            if !guard.subgraphs.iter().any(|s| s.slug == subgraph_slug) {
                return Err(AppError::NotFound("SUBGRAPH_NOT_FOUND"));
            }
            return Ok(PublishedSchemaResponse {
                subgraph_slug: String::from(subgraph_slug),
                version: 0,
                doc: axpoint_domain::studio::SchemaDoc::default(),
            });
        }
        guard.schema.clone().ok_or(AppError::NotFound("NOT_FOUND"))
    }

    fn save_schema_direct(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
        _actor_user_id: Option<Uuid>,
        _req: SaveSchemaDirectRequest,
    ) -> impl Future<Output = Result<PublishedSchemaResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    async fn load_validation_context(
        &self,
        _tenant_id: &str,
        subgraph_slug: &str,
    ) -> Result<ValidationContext, AppError> {
        let guard = self.inner.lock().await;
        let current_doc = if subgraph_slug == "main" {
            guard.schema.as_ref().map(|s| s.doc.clone())
        } else {
            None
        };
        Ok(ValidationContext {
            subgraph_slug: String::from(subgraph_slug),
            main_node_types: BTreeMap::new(),
            current_doc,
            existing_node_counts: BTreeMap::new(),
            existing_link_counts: BTreeMap::new(),
            datasets: BTreeMap::new(),
            cardinality_violations: std::collections::BTreeSet::new(),
        })
    }

    fn list_schema_versions(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
    ) -> impl Future<Output = Result<Vec<SchemaVersionSummary>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_schema_version(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
        _version: i32,
    ) -> impl Future<Output = Result<Option<SchemaVersionDetail>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn diff_schema_versions(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
        _from_version: i32,
        _to_version: i32,
    ) -> impl Future<Output = Result<SchemaDiff, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn restore_schema_version(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
        _version: i32,
        _actor_user_id: Option<Uuid>,
    ) -> impl Future<Output = Result<SchemaDraft, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    async fn list_drafts(
        &self,
        _tenant_id: &str,
        subgraph_slug: &str,
        status: Option<DraftStatus>,
    ) -> Result<Vec<SchemaDraft>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .drafts
            .iter()
            .filter(|d| d.subgraph_slug == subgraph_slug)
            .filter(|d| status.is_none_or(|st| d.status == st))
            .cloned()
            .collect())
    }

    async fn get_draft(
        &self,
        _tenant_id: &str,
        draft_id: Uuid,
    ) -> Result<Option<SchemaDraft>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.drafts.iter().find(|d| d.id == draft_id).cloned())
    }

    async fn create_draft(
        &self,
        _tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: CreateDraftRequest,
    ) -> Result<SchemaDraft, AppError> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        let draft = SchemaDraft {
            id: Uuid::new_v4(),
            subgraph_slug: String::from(subgraph_slug),
            base_version: req.base_version.unwrap_or(0),
            title: req.title,
            schema_doc: req.schema_doc,
            status: DraftStatus::Draft,
            source: req
                .source
                .unwrap_or(axpoint_domain::studio::DraftSource::Manual),
            ai_context: req.ai_context.unwrap_or_else(|| serde_json::json!({})),
            issues: Vec::new(),
            pass_count: 1,
            created_by: actor_user_id,
            created_at: now,
            updated_at: now,
        };
        guard.drafts.push(draft.clone());
        Ok(draft)
    }

    async fn update_draft(
        &self,
        _tenant_id: &str,
        draft_id: Uuid,
        req: UpdateDraftRequest,
    ) -> Result<SchemaDraft, AppError> {
        let mut guard = self.inner.lock().await;
        let existing = guard
            .drafts
            .iter_mut()
            .find(|d| d.id == draft_id)
            .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
        if let Some(title) = req.title {
            existing.title = title;
        }
        existing.schema_doc = req.schema_doc;
        existing.updated_at = Utc::now();
        Ok(existing.clone())
    }

    fn discard_draft(
        &self,
        _tenant_id: &str,
        _draft_id: Uuid,
    ) -> impl Future<Output = Result<SchemaDraft, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn publish_draft(
        &self,
        _tenant_id: &str,
        _draft_id: Uuid,
        _actor_user_id: Option<Uuid>,
        _req: PublishDraftRequest,
    ) -> impl Future<Output = Result<PublishDraftResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn preview_subgraph_merge(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
        _req: MergePreviewRequest,
    ) -> impl Future<Output = Result<MergePreview, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn merge_subgraph_into_main(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
        _actor_user_id: Option<Uuid>,
        _req: MergeSubgraphRequest,
    ) -> impl Future<Output = Result<PublishDraftResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_mappings(
        &self,
        _tenant_id: &str,
        _subgraph_slug: &str,
    ) -> impl Future<Output = Result<Vec<MappingRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_mapping(
        &self,
        _tenant_id: &str,
        _mapping_id: Uuid,
    ) -> impl Future<Output = Result<Option<MappingRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_mapping_runs(
        &self,
        _tenant_id: &str,
        _mapping_id: Uuid,
        _limit: u32,
    ) -> impl Future<Output = Result<Vec<MappingRunRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn enqueue_mapping_run(
        &self,
        _tenant_id: &str,
        _mapping_id: Uuid,
        _req: TriggerMappingRunRequest,
    ) -> impl Future<Output = Result<TriggerMappingRunResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn load_doc_entity_groups(
        &self,
        _tenant_id: &str,
        _doc_ids: &[Uuid],
    ) -> impl Future<Output = Result<Vec<DocEntityGroup>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
}

impl GraphStorePort for MockStep09Store {
    async fn list_nodes(
        &self,
        _tenant_id: &str,
        query: NodeListQuery,
    ) -> Result<NodeListResponse, AppError> {
        let delay = self.inner.lock().await.slow_node_delay_ms;
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let guard = self.inner.lock().await;
        let items: Vec<BlueNode> = guard
            .nodes
            .iter()
            .filter(|n| {
                query
                    .type_name
                    .as_deref()
                    .is_none_or(|tn| n.type_name == tn)
            })
            .filter(|n| {
                query
                    .q
                    .as_deref()
                    .is_none_or(|q| n.node_key.contains(q) || n.title.contains(q))
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

    async fn get_node(
        &self,
        _tenant_id: &str,
        node_id: Uuid,
    ) -> Result<Option<BlueNode>, AppError> {
        let delay = self.inner.lock().await.slow_node_delay_ms;
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let guard = self.inner.lock().await;
        Ok(guard.nodes.iter().find(|n| n.id == node_id).cloned())
    }

    fn upsert_node(
        &self,
        _tenant_id: &str,
        _req: UpsertBlueNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn patch_node(
        &self,
        _tenant_id: &str,
        _node_id: Uuid,
        _req: PatchNodeRequest,
    ) -> impl Future<Output = Result<BlueNode, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn delete_node(
        &self,
        _tenant_id: &str,
        _node_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn bulk_upsert(
        &self,
        _tenant_id: &str,
        _req: BulkNodesRequest,
    ) -> impl Future<Output = Result<BulkNodesResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_links(
        &self,
        _tenant_id: &str,
        _query: LinkListQuery,
    ) -> impl Future<Output = Result<Vec<LinkRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn upsert_link(
        &self,
        _tenant_id: &str,
        _req: UpsertLinkRequest,
    ) -> impl Future<Output = Result<LinkRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn delete_link(
        &self,
        _tenant_id: &str,
        _link_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    async fn search_nodes(
        &self,
        _tenant_id: &str,
        query: NodeSearchQuery,
        _query_embedding: Option<&[f32]>,
    ) -> Result<NodeSearchResponse, AppError> {
        let guard = self.inner.lock().await;
        let items = guard
            .nodes
            .iter()
            .filter(|n| {
                query
                    .type_name
                    .as_deref()
                    .is_none_or(|tn| n.type_name == tn)
            })
            .filter(|n| n.title.contains(&query.q) || n.node_key.contains(&query.q))
            .map(|n| NodeSearchHit {
                id: n.id,
                type_name: n.type_name.clone(),
                node_key: n.node_key.clone(),
                title: n.title.clone(),
                properties: n.properties.clone(),
                metrics: n.metrics.clone(),
                geo: n.geo,
                score: 0.95,
                channels: vec![String::from("FTS")],
            })
            .collect();
        Ok(NodeSearchResponse { items })
    }

    fn list_node_documents(
        &self,
        _tenant_id: &str,
        _node_id: Uuid,
    ) -> impl Future<Output = Result<NodeDocumentsResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn resolve_entity_links(
        &self,
        _tenant_id: &str,
        _req: ResolveEntitiesRequest,
    ) -> impl Future<Output = Result<ResolveEntitiesResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_entity_candidates(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<EntityCandidatesResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn confirm_entity_link(
        &self,
        _tenant_id: &str,
        _entity_id: Uuid,
        _node_id: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn reject_entity_link(
        &self,
        _tenant_id: &str,
        _entity_id: Uuid,
        _node_id: Uuid,
    ) -> impl Future<Output = Result<EntityLink, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }
}

impl GraphQueryPort for MockStep09Store {
    async fn query_neighbors(
        &self,
        _tenant_id: &str,
        _query: NeighborQuery,
    ) -> Result<SubgraphResponse, AppError> {
        let guard = self.inner.lock().await;
        Ok(SubgraphResponse {
            nodes: guard.nodes.clone(),
            links: guard.links.clone(),
            truncated: false,
        })
    }

    fn query_shortest_path(
        &self,
        _tenant_id: &str,
        _query: PathQuery,
    ) -> impl Future<Output = Result<PathResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn query_subgraph(
        &self,
        _tenant_id: &str,
        _query: SubgraphQuery,
    ) -> impl Future<Output = Result<SubgraphResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn run_algorithm(
        &self,
        _tenant_id: &str,
        _algo: GraphAlgo,
        _req: AlgoRunRequest,
    ) -> impl Future<Output = Result<AlgoRunResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_algorithm_results(
        &self,
        _tenant_id: &str,
        _algo: GraphAlgo,
        _query: AlgoResultsQuery,
    ) -> impl Future<Output = Result<AlgoResultsResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }
}

impl MetricStorePort for MockStep09Store {
    async fn write_metrics(
        &self,
        _tenant_id: &str,
        req: WriteMetricsRequest,
    ) -> Result<WriteMetricsResponse, AppError> {
        let mut guard = self.inner.lock().await;
        let written = req.points.len();
        for pt in req.points {
            if let Some(val) = pt.value {
                if let Some(nid) = pt.node_id
                    && let Some(node) = guard.nodes.iter_mut().find(|n| n.id == nid)
                {
                    node.metrics.insert(pt.metric.clone(), val);
                } else if let Some(nr) = pt.node
                    && let Some(node) = guard
                        .nodes
                        .iter_mut()
                        .find(|n| n.type_name == nr.type_name && n.node_key == nr.node_key)
                {
                    node.metrics.insert(pt.metric, val);
                }
            }
        }
        Ok(WriteMetricsResponse {
            written,
            dirty_minutes: 0,
            rollup_job_id: Uuid::new_v4(),
        })
    }

    async fn query_node_metrics(
        &self,
        _tenant_id: &str,
        node_id: Uuid,
        metric: &str,
        _query: NodeMetricSeriesQuery,
    ) -> Result<Vec<MetricSeriesPoint>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .series
            .get(&(node_id, String::from(metric)))
            .cloned()
            .unwrap_or_default())
    }

    fn query_window(
        &self,
        _tenant_id: &str,
        _query: WindowQuery,
    ) -> impl Future<Output = Result<WindowQueryResponse, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_metric_catalog(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<MetricCatalogEntry>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn run_rollup_for_tenant(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<u64, AppError>> {
        std::future::ready(Ok(0))
    }
}

impl FlowRepository for MockStep09Store {
    async fn list_flows(
        &self,
        _tenant_id: &str,
        _query: FlowListQuery,
    ) -> Result<PaginatedList<AxFlowDef>, AppError> {
        let guard = self.inner.lock().await;
        Ok(PaginatedList {
            items: guard.flows.clone(),
            next_cursor: None,
        })
    }

    async fn get_flow(
        &self,
        _tenant_id: &str,
        flow_id: Uuid,
    ) -> Result<Option<AxFlowDef>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.flows.iter().find(|f| f.id == flow_id).cloned())
    }

    async fn create_flow(
        &self,
        _tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxFlowRequest,
    ) -> Result<AxFlowDef, AppError> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        let (nodes, edges) = req.resolve_dag();
        let rec = AxFlowDef {
            id: Uuid::new_v4(),
            name: req.name,
            description: req.description,
            slug: req.slug,
            nodes,
            edges,
            trigger_cfg: req.trigger_cfg,
            status: String::from("DRAFT"),
            created_by,
            last_run_at: None,
            created_at: now,
            updated_at: now,
            issues: Vec::new(),
        };
        guard.flows.push(rec.clone());
        Ok(rec)
    }

    async fn update_flow(
        &self,
        _tenant_id: &str,
        flow_id: Uuid,
        req: UpdateAxFlowRequest,
    ) -> Result<Option<AxFlowDef>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(existing) = guard.flows.iter_mut().find(|f| f.id == flow_id) else {
            return Ok(None);
        };
        if let Some(name) = req.name {
            existing.name = name;
        }
        if let Some(desc) = req.description {
            existing.description = desc;
        }
        if let Some(slug) = req.slug {
            existing.slug = (!slug.is_empty()).then_some(slug);
        }
        if let Some(nodes) = req.nodes {
            existing.nodes = nodes;
        }
        if let Some(edges) = req.edges {
            existing.edges = edges;
        }
        if let Some(tc) = req.trigger_cfg {
            existing.trigger_cfg = tc;
        }
        existing.updated_at = Utc::now();
        Ok(Some(existing.clone()))
    }

    fn delete_flow(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn publish_flow_version(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
        _published_by: Option<Uuid>,
        _req: PublishFlowVersionRequest,
    ) -> impl Future<Output = Result<FlowVersionRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_flow_versions(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
    ) -> impl Future<Output = Result<FlowVersionsListResponse, AppError>> {
        std::future::ready(Ok(FlowVersionsListResponse {
            items: Vec::new(),
            deployments: Vec::new(),
            next_cursor: None,
        }))
    }

    fn get_flow_version(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
        _version: i32,
    ) -> impl Future<Output = Result<Option<FlowVersionRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn deploy_flow(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
        _deployed_by: Option<Uuid>,
        _req: DeployFlowRequest,
    ) -> impl Future<Output = Result<FlowDeploymentRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    async fn get_flow_deployment(
        &self,
        _tenant_id: &str,
        flow_id: Uuid,
        environment: Environment,
    ) -> Result<Option<FlowDeploymentRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.flow_deployments.get(&(flow_id, environment)).cloned())
    }

    async fn start_flow_run(
        &self,
        _tenant_id: &str,
        params: StartRunParams,
    ) -> Result<(RunRecord, bool), AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(ref idem) = params.idempotency_key
            && let Some(existing) = guard
                .flow_runs
                .iter()
                .find(|r| r.flow_id == params.flow_id && r.idempotency_key.as_ref() == Some(idem))
        {
            return Ok((existing.clone(), true));
        }
        let now = Utc::now();
        let rec = RunRecord {
            id: Uuid::new_v4(),
            flow_id: params.flow_id,
            version: params.explicit_version.unwrap_or(1),
            environment: params.environment,
            trigger_source: params.trigger_source,
            trigger_payload: params.trigger_payload,
            idempotency_key: params.idempotency_key,
            status: RunStatus::Running,
            outputs: serde_json::json!({}),
            error_message: None,
            started_by: params.started_by,
            created_at: now,
            started_at: Some(now),
            finished_at: None,
        };
        guard.flow_runs.push(rec.clone());
        Ok((rec, false))
    }

    async fn get_flow_run(
        &self,
        _tenant_id: &str,
        run_id: Uuid,
    ) -> Result<Option<RunRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.flow_runs.iter().find(|r| r.id == run_id).cloned())
    }

    fn list_flow_runs(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
        _query: RunListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RunRecord>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn list_run_steps(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
    ) -> impl Future<Output = Result<Vec<StepRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn list_latest_run_steps(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
    ) -> impl Future<Output = Result<Vec<StepRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn mark_step_running(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
        _node_id: &str,
        _attempt: i32,
        _job_id: Option<Uuid>,
        _rendered_input: &serde_json::Value,
    ) -> impl Future<Output = Result<StepRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn find_cached_step_output(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
        _node_id: &str,
        _rendered_input: &serde_json::Value,
    ) -> impl Future<Output = Result<Option<serde_json::Value>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn apply_step_mutation_and_progress(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
        _mutation: StepMutation,
        _planner: ProgressionPlannerFn<'_>,
    ) -> impl Future<Output = Result<RunRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn cancel_flow_run(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
    ) -> impl Future<Output = Result<RunRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn retry_flow_run(
        &self,
        _tenant_id: &str,
        _run_id: Uuid,
    ) -> impl Future<Output = Result<RunRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    async fn create_or_get_approval(
        &self,
        _tenant_id: &str,
        input: NewApprovalInput,
    ) -> Result<ApprovalRecord, AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard
            .approvals
            .iter()
            .find(|a| a.source_kind == input.source_kind && a.source_ref == input.source_ref)
        {
            return Ok(existing.clone());
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
        _tenant_id: &str,
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
        _tenant_id: &str,
        _viewer_user_id: Uuid,
        _viewer_roles: &[String],
        _query: ApprovalListQuery,
    ) -> Result<PaginatedList<ApprovalRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(PaginatedList {
            items: guard.approvals.clone(),
            next_cursor: None,
        })
    }

    async fn decide_approval(
        &self,
        _tenant_id: &str,
        params: DecideApprovalParams,
    ) -> Result<ApprovalRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(rec) = guard
            .approvals
            .iter_mut()
            .find(|a| a.id == params.approval_id)
        else {
            return Err(AppError::NotFound("NOT_FOUND"));
        };
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
        _tenant_id: &str,
        input: NewActionLogInput,
    ) -> Result<(ActionLogRecord, bool), AppError> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard
            .action_logs
            .iter()
            .find(|l| l.idempotency_key == input.idempotency_key)
        {
            return Ok((existing.clone(), true));
        }
        let next_id = i64::try_from(guard.action_logs.len())
            .unwrap_or(0)
            .saturating_add(1);
        let rec = ActionLogRecord {
            id: next_id,
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
        guard.action_logs.push(rec.clone());
        Ok((rec, false))
    }

    async fn get_action_log_by_key(
        &self,
        _tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ActionLogRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .action_logs
            .iter()
            .find(|l| l.idempotency_key == idempotency_key)
            .cloned())
    }

    fn list_action_logs(
        &self,
        _tenant_id: &str,
        _query: ActionListQuery,
    ) -> impl Future<Output = Result<PaginatedList<ActionLogRecord>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn upsert_flow_hook(
        &self,
        _tenant_id: &str,
        _flow_id: Uuid,
        _credential_id: Uuid,
    ) -> impl Future<Output = Result<(Uuid, chrono::DateTime<Utc>), AppError>> {
        std::future::ready(Ok((Uuid::new_v4(), Utc::now())))
    }

    fn resolve_flow_hook(
        &self,
        _tenant_id: &str,
        _slug: &str,
    ) -> impl Future<Output = Result<Option<ResolvedFlowHook>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_scheduled_flows(
        &self,
    ) -> impl Future<Output = Result<Vec<ScheduledFlowItem>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn list_event_flows(
        &self,
        _topic: &str,
    ) -> impl Future<Output = Result<Vec<EventFlowItem>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn list_active_tenants(&self) -> impl Future<Output = Result<Vec<String>, AppError>> {
        std::future::ready(Ok(vec![String::from("default")]))
    }
}

impl RulesRepository for MockStep09Store {
    fn list_rules(
        &self,
        _tenant_id: &str,
        _query: RuleListQuery,
    ) -> impl Future<Output = Result<PaginatedList<RuleDef>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn list_enabled_rules_by_scope(
        &self,
        _tenant_id: &str,
        _scope_kind: RuleScopeKind,
    ) -> impl Future<Output = Result<Vec<RuleDef>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_rule(
        &self,
        _tenant_id: &str,
        _rule_id: Uuid,
    ) -> impl Future<Output = Result<Option<RuleDef>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn create_rule(
        &self,
        _tenant_id: &str,
        _created_by: Option<Uuid>,
        _req: CreateRuleRequest,
    ) -> impl Future<Output = Result<RuleDef, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn update_rule(
        &self,
        _tenant_id: &str,
        _rule_id: Uuid,
        _req: UpdateRuleRequest,
    ) -> impl Future<Output = Result<Option<RuleDef>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn delete_rule(
        &self,
        _tenant_id: &str,
        _rule_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    async fn list_alerts(
        &self,
        _tenant_id: &str,
        query: AlertListQuery,
    ) -> Result<PaginatedList<AlertRecord>, AppError> {
        let delay = self.inner.lock().await.slow_node_delay_ms;
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let guard = self.inner.lock().await;
        let items = guard
            .alerts
            .iter()
            .filter(|a| query.status.is_none_or(|st| a.status == st))
            .filter(|a| query.severity.is_none_or(|sev| a.severity == sev))
            .cloned()
            .collect();
        Ok(PaginatedList {
            items,
            next_cursor: None,
        })
    }

    async fn get_alert(
        &self,
        _tenant_id: &str,
        alert_id: Uuid,
    ) -> Result<Option<AlertRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.alerts.iter().find(|a| a.id == alert_id).cloned())
    }

    fn list_active_alerts_for_rule(
        &self,
        _tenant_id: &str,
        _rule_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn list_alert_events(
        &self,
        _tenant_id: &str,
        _alert_id: Uuid,
        _query: AlertEventsQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertEventRecord>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn append_alert_event(
        &self,
        _tenant_id: &str,
        _alert_id: Uuid,
        _event: AlertEventKind,
        _detail: serde_json::Value,
        _actor: Option<Uuid>,
    ) -> impl Future<Output = Result<AlertEventRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn set_alert_flow_run(
        &self,
        _tenant_id: &str,
        _alert_id: Uuid,
        _flow_id: Uuid,
        _run_id: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn record_true_condition(
        &self,
        _tenant_id: &str,
        _input: TrueConditionInput,
    ) -> impl Future<Output = Result<TrueConditionOutcome, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn record_false_condition(
        &self,
        _tenant_id: &str,
        _dedup_key: &str,
        _resolve_after_minutes: i32,
        _now: chrono::DateTime<Utc>,
    ) -> impl Future<Output = Result<FalseConditionOutcome, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn escalate_due_alerts(
        &self,
        _tenant_id: &str,
        _rule_id: Uuid,
        _escalate_after_minutes: i32,
        _now: chrono::DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn ack_alert(
        &self,
        _tenant_id: &str,
        alert_id: Uuid,
        actor: Option<Uuid>,
        _comment: &str,
    ) -> Result<AlertRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(rec) = guard.alerts.iter_mut().find(|a| a.id == alert_id) else {
            return Err(AppError::NotFound("NOT_FOUND"));
        };
        rec.status = AlertStatus::Ack;
        rec.acked_by = actor;
        rec.acked_at = Some(Utc::now());
        Ok(rec.clone())
    }

    fn ack_alerts_batch(
        &self,
        _tenant_id: &str,
        _alert_ids: &[Uuid],
        _actor: Option<Uuid>,
        _comment: &str,
    ) -> impl Future<Output = Result<AckBatchResponse, AppError>> {
        std::future::ready(Ok(AckBatchResponse {
            acked_count: 0,
            items: Vec::new(),
        }))
    }

    fn resolve_alert_manual(
        &self,
        _tenant_id: &str,
        _alert_id: Uuid,
        _actor: Option<Uuid>,
        _comment: &str,
    ) -> impl Future<Output = Result<AlertRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_channels(
        &self,
        _tenant_id: &str,
        _query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<NotifyChannelRecord>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn get_channel(
        &self,
        _tenant_id: &str,
        _channel_id: Uuid,
    ) -> impl Future<Output = Result<Option<(NotifyChannelRecord, Option<SealedCredential>)>, AppError>>
    {
        std::future::ready(Ok(None))
    }

    fn create_channel(
        &self,
        _tenant_id: &str,
        _input: SaveNotifyChannelInput,
    ) -> impl Future<Output = Result<NotifyChannelRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn update_channel(
        &self,
        _tenant_id: &str,
        _channel_id: Uuid,
        _input: SaveNotifyChannelInput,
    ) -> impl Future<Output = Result<Option<NotifyChannelRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn delete_channel(
        &self,
        _tenant_id: &str,
        _channel_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn enqueue_delivery(
        &self,
        _tenant_id: &str,
        _input: EnqueueDeliveryInput,
    ) -> impl Future<Output = Result<(NotifyDeliveryRecord, bool), AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn get_delivery(
        &self,
        _tenant_id: &str,
        _delivery_id: Uuid,
    ) -> impl Future<Output = Result<Option<NotifyDeliveryRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn mark_delivery_sent(
        &self,
        _tenant_id: &str,
        _delivery_id: Uuid,
        _attempt: i32,
        _response_code: Option<i32>,
    ) -> impl Future<Output = Result<NotifyDeliveryRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn mark_delivery_failed(
        &self,
        _tenant_id: &str,
        _delivery_id: Uuid,
        _update: DeliveryFailureUpdate,
    ) -> impl Future<Output = Result<NotifyDeliveryRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn list_silences(
        &self,
        _tenant_id: &str,
        _query: SimplePageQuery,
    ) -> impl Future<Output = Result<PaginatedList<AlertSilenceRecord>, AppError>> {
        std::future::ready(Ok(PaginatedList {
            items: Vec::new(),
            next_cursor: None,
        }))
    }

    fn list_active_silences(
        &self,
        _tenant_id: &str,
        _at: chrono::DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<AlertSilenceRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn create_silence(
        &self,
        _tenant_id: &str,
        _created_by: Option<Uuid>,
        _req: CreateSilenceRequest,
    ) -> impl Future<Output = Result<AlertSilenceRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn delete_silence(
        &self,
        _tenant_id: &str,
        _silence_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn query_raw_metric_points(
        &self,
        _tenant_id: &str,
        _node_id: Uuid,
        _metric: &str,
        _since: Option<chrono::DateTime<Utc>>,
        _limit: u32,
    ) -> impl Future<Output = Result<Vec<(chrono::DateTime<Utc>, f64)>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_latest_dataset_quality(
        &self,
        _tenant_id: &str,
        _dataset_id: Uuid,
    ) -> impl Future<Output = Result<Option<(Dataset, Option<QualityReport>)>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn list_absence_rules(&self) -> impl Future<Output = Result<Vec<AbsenceRuleItem>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn list_due_deliveries(
        &self,
        _limit: i32,
    ) -> impl Future<Output = Result<Vec<DueDeliveryItem>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }
}
