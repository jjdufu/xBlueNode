//! In-memory Step 09 test harness (`MockStep09Store`, `MockCapabilityPort`, `MockUrlGuard`, `MockAuditSink`)
//! backing `axpoint-agent` offline evaluation and integration tests.

pub mod repo_stubs;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use axpoint_agent::{
    AgentModelPort, AgentRuntime, AgentRuntimeDeps, BuiltinToolRegistry, CapabilityPublisher,
    EmbedService, McpClientManager, McpServerHub, PublishedAgentInvoker,
};
use axpoint_domain::agent::{
    AgentEmbedRecord, AgentRecord, AgentVersionRecord, ConversationListQuery, ConversationRecord,
    CreateAgentRequest, EndpointListQuery, EndpointSpec, LlmProviderRecord, LlmUsageDelta,
    LlmUsageQuery, LlmUsageRecord, McpClientRecord, McpToolInfo, MessageRecord, NewAgentEmbedInput,
    NewConversationInput, NewMessageInput, ResolvedAgentEmbed, SaveEndpointInput,
    SaveLlmProviderInput, SaveMcpClientInput, SimRunToolInput, SimRunToolOutput,
    UpdateAgentRequest, UpdateEndpointRequest,
};
use axpoint_domain::analytics::{
    AnalyticsPrincipal, MetricDef, MetricQuery, MetricQueryResult, QueryColumn, QueryResult,
    SavedQuery,
};
use axpoint_domain::axhub::{ColumnMeta, Dataset, PaginatedList, SealedCredential};
use axpoint_domain::enums::{
    ConversationStatus, DataType, Environment, Permission, PiiCategory, TokenKind,
};
use axpoint_domain::flow::{ActionLogRecord, ApprovalRecord, FlowDeploymentRecord, RunRecord};
use axpoint_domain::graph::{LinkRecord, MetricSeriesPoint};
use axpoint_domain::platform::{AuditRecord, BoxFuture, PrincipalRecord};
use axpoint_domain::ports::{
    AgentCapabilityPort, AgentRepository, AuditSink, BuilderDashboardPort, OutboundUrlGuardPort,
    SimExecutionPort, TemplateSandboxPort,
};
use axpoint_domain::rules::AlertRecord;
use axpoint_domain::studio::doc::{
    NodeTypeSpec, PublishedSchemaResponse, SchemaDraft, SubgraphInfo,
};
use axpoint_domain::topology::BlueNode;
use axpoint_domain::{AppError, FusedHit, RecallChannel};
use axpoint_security::{AuditWriter, AuthPrincipal, CredentialKeyring};
use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Fixed UUID of seeded `Warehouse` node `W-1`.
pub const WAREHOUSE_W1_ID: &str = "00000000-0000-0000-0000-000000000101";
/// Fixed UUID of seeded `Supplier` node `S-1`.
pub const SUPPLIER_S1_ID: &str = "00000000-0000-0000-0000-000000000102";
/// Fixed UUID of seeded `CRITICAL` `OPEN` alert `A-1`.
pub const ALERT_A1_ID: &str = "00000000-0000-0000-0000-000000000201";
/// Fixed UUID of seeded `PROD`-deployed replenishment flow `F-1`.
pub const FLOW_F1_ID: &str = "00000000-0000-0000-0000-000000000301";

/// Mutable state inside [`MockStep09Store`].
#[derive(Default)]
pub struct StoreInner {
    pub endpoints: Vec<EndpointSpec>,
    pub mcp_clients: Vec<(McpClientRecord, Option<SealedCredential>)>,
    pub agents: Vec<AgentRecord>,
    pub agent_versions: Vec<AgentVersionRecord>,
    pub conversations: Vec<ConversationRecord>,
    pub messages: Vec<MessageRecord>,
    pub next_msg_id: i64,
    pub embeds: Vec<(String, AgentEmbedRecord)>,
    pub metrics: HashMap<String, MetricDef>,
    pub saved_queries: HashMap<Uuid, SavedQuery>,
    pub datasets: Vec<Dataset>,
    pub columns: HashMap<Uuid, Vec<ColumnMeta>>,
    pub schema: Option<PublishedSchemaResponse>,
    pub subgraphs: Vec<SubgraphInfo>,
    pub drafts: Vec<SchemaDraft>,
    pub nodes: Vec<BlueNode>,
    pub links: Vec<LinkRecord>,
    pub series: HashMap<(Uuid, String), Vec<MetricSeriesPoint>>,
    pub alerts: Vec<AlertRecord>,
    pub flows: Vec<axpoint_domain::topology::AxFlowDef>,
    pub flow_deployments: HashMap<(Uuid, Environment), FlowDeploymentRecord>,
    pub flow_runs: Vec<RunRecord>,
    pub approvals: Vec<ApprovalRecord>,
    pub action_logs: Vec<ActionLogRecord>,
    pub principals: HashMap<(String, Uuid), PrincipalRecord>,
    pub slow_node_delay_ms: u64,
}

/// Shared in-memory repository implementing all Step 09 storage ports.
#[derive(Clone, Default)]
pub struct MockStep09Store {
    pub inner: Arc<Mutex<StoreInner>>,
}

impl MockStep09Store {
    /// Creates a [`MockStep09Store`] pre-populated with enterprise warehouse, order, metric, alert, and flow fixtures.
    ///
    /// # Errors
    /// Returns [`AppError`] if any embedded JSON fixture fails deserialization.
    pub async fn seeded() -> Result<Arc<Self>, AppError> {
        let store = Arc::new(Self::default());
        repo_stubs::populate_enterprise_seed(&store).await?;
        Ok(store)
    }
}

/// Buffered in-memory [`AuditSink`] recording all emitted [`AuditRecord`] rows.
#[derive(Clone, Default)]
pub struct MockAuditSink {
    pub records: Arc<Mutex<Vec<AuditRecord>>>,
}

impl AuditSink for MockAuditSink {
    async fn write_batch(&self, records: Vec<AuditRecord>) -> Result<(), AppError> {
        let mut guard = self.records.lock().await;
        guard.extend(records);
        Ok(())
    }
}

/// Deterministic [`OutboundUrlGuardPort`] blocking `127.0.0.1` / `localhost` unless `allow_private_network == true`.
#[derive(Clone, Default)]
pub struct MockUrlGuard;

impl OutboundUrlGuardPort for MockUrlGuard {
    fn check_outbound_url<'a>(
        &'a self,
        url: &'a str,
        allow_private_network: bool,
    ) -> BoxFuture<'a, String> {
        Box::pin(async move {
            let trimmed = url.trim();
            if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
                return Err(AppError::bad("INVALID_ARGUMENT", "unsupported URL scheme"));
            }
            let is_loopback = trimmed.contains("127.0.0.1") || trimmed.contains("localhost");
            if is_loopback && !allow_private_network {
                return Err(AppError::Forbidden("SSRF_BLOCKED"));
            }
            Ok(String::from(trimmed))
        })
    }
}

/// Optional mock [`SimExecutionPort`] for testing Step 10 stage-gating of `run_simulation`.
#[derive(Clone, Default)]
pub struct MockSimPort;

impl SimExecutionPort for MockSimPort {
    fn run_simulation_tool<'a>(
        &'a self,
        _tenant_id: &'a str,
        _user_id: Uuid,
        input: SimRunToolInput,
    ) -> BoxFuture<'a, SimRunToolOutput> {
        Box::pin(async move {
            Ok(SimRunToolOutput {
                sandbox_id: input.sandbox_id.unwrap_or_else(Uuid::nil),
                scenario_id: Some(Uuid::new_v4()),
                summary: serde_json::json!({
                    "scenario_name": input.scenario_name.unwrap_or_else(|| String::from("baseline")),
                    "impacted_nodes": 3,
                    "stockout_risk_delta": -0.42
                }),
            })
        })
    }
}

/// Deterministic [`AgentCapabilityPort`] reading metrics, domain objects, knowledge hits, and SQL from [`MockStep09Store`].
pub struct MockCapabilityPort {
    pub store: Arc<MockStep09Store>,
}

impl MockCapabilityPort {
    /// Creates a new [`MockCapabilityPort`] backed by `store`.
    #[must_use]
    pub const fn new(store: Arc<MockStep09Store>) -> Self {
        Self { store }
    }
}

impl AgentCapabilityPort for MockCapabilityPort {
    fn search_knowledge<'a>(
        &'a self,
        _tenant_id: &'a str,
        _query: &'a str,
        _top_k: usize,
        doc_title_prefix: Option<&'a str>,
    ) -> BoxFuture<'a, Vec<FusedHit>> {
        Box::pin(async move {
            let title = String::from("仓储应急补货管理规范 SOP-WH-04");
            if let Some(pref) = doc_title_prefix
                && !title.starts_with(pref)
            {
                return Ok(Vec::new());
            }
            Ok(vec![FusedHit {
                para_id: Uuid::new_v4(),
                doc_id: Uuid::new_v4(),
                doc_title: title,
                content: String::from(
                    "根据 SOP-WH-04 第四条：当仓库安全库存低于红线阈值时，须在 48 小时内触发紧急补货流程。",
                ),
                rrf_score: 0.92,
                channels: vec![RecallChannel::Vector],
                traces: vec![String::from("FTS+Vector RRF hit")],
            }])
        })
    }

    fn ask_data<'a>(
        &'a self,
        _principal: &'a AnalyticsPrincipal,
        question: &'a str,
        dataset_names: &'a [String],
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            Ok(serde_json::json!({
                "question": question,
                "datasets": dataset_names,
                "sql": "SELECT customer_name, SUM(amount) AS total_sales FROM demo_orders GROUP BY 1 ORDER BY 2 DESC LIMIT 1",
                "rows": [{ "customer_name": "星海集团", "total_sales": 52000 }],
                "answer": "贡献最高的大客户为星海集团（累计销售额 52000 元）。"
            }))
        })
    }

    fn run_sql_bounded<'a>(
        &'a self,
        _principal: &'a AnalyticsPrincipal,
        sql: &'a str,
        max_rows: usize,
    ) -> BoxFuture<'a, QueryResult> {
        Box::pin(async move {
            let mut row = serde_json::Map::new();
            row.insert(String::from("overdue_cnt"), serde_json::json!(7));
            Ok(QueryResult {
                columns: vec![QueryColumn {
                    name: String::from("overdue_cnt"),
                    data_type: DataType::Int64,
                    pii_category: PiiCategory::None,
                    masked: false,
                }],
                rows: vec![row],
                row_count: 1.min(max_rows),
                truncated: false,
                sql: String::from(sql),
                duration_ms: 4,
            })
        })
    }

    fn run_metric_endpoint<'a>(
        &'a self,
        _principal: &'a AnalyticsPrincipal,
        metric_name: &'a str,
        pinned_snapshot: Option<&'a MetricDef>,
        _query: MetricQuery,
    ) -> BoxFuture<'a, MetricQueryResult> {
        Box::pin(async move {
            let def = if let Some(snap) = pinned_snapshot {
                snap.clone()
            } else {
                let guard = self.store.inner.lock().await;
                guard
                    .metrics
                    .get(metric_name)
                    .cloned()
                    .ok_or(AppError::NotFound("NOT_FOUND"))?
            };
            let val = if def.filter_sql.contains("PENDING") {
                19_300.0
            } else {
                128_500.0
            };
            let mut row = serde_json::Map::new();
            row.insert(String::from("status"), serde_json::json!("PAID"));
            row.insert(String::from("value"), serde_json::json!(val));
            Ok(MetricQueryResult {
                metrics: vec![String::from(metric_name)],
                columns: vec![
                    QueryColumn {
                        name: String::from("status"),
                        data_type: DataType::String,
                        pii_category: PiiCategory::None,
                        masked: false,
                    },
                    QueryColumn {
                        name: String::from("value"),
                        data_type: DataType::Decimal,
                        pii_category: PiiCategory::None,
                        masked: false,
                    },
                ],
                rows: vec![row],
                sql: format!(
                    "SELECT status, {} FROM demo_orders WHERE {}",
                    def.expression, def.filter_sql
                ),
                duration_ms: 3,
            })
        })
    }

    fn run_saved_endpoint<'a>(
        &'a self,
        _principal: &'a AnalyticsPrincipal,
        _saved_id: Uuid,
        _pinned_snapshot: Option<&'a SavedQuery>,
        params: BTreeMap<String, serde_json::Value>,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            Ok(serde_json::json!({
                "rows": [{ "echo_params": params, "total": 42 }],
                "row_count": 1
            }))
        })
    }

    fn query_domain_objects<'a>(
        &'a self,
        _principal: &'a AnalyticsPrincipal,
        type_name: &'a str,
        _pinned_snapshot: Option<&'a NodeTypeSpec>,
        payload: &'a serde_json::Value,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let keys_filter: Option<Vec<&str>> = payload
                .get("keys")
                .and_then(serde_json::Value::as_array)
                .map(|arr| arr.iter().filter_map(serde_json::Value::as_str).collect());
            let guard = self.store.inner.lock().await;
            let items: Vec<serde_json::Value> = guard
                .nodes
                .iter()
                .filter(|n| n.type_name == type_name)
                .filter(|n| {
                    keys_filter
                        .as_ref()
                        .is_none_or(|ks| ks.contains(&n.node_key.as_str()))
                })
                .map(|n| {
                    serde_json::json!({
                        "id": n.id,
                        "type_name": n.type_name,
                        "node_key": n.node_key,
                        "title": n.title,
                        "properties": n.properties,
                        "metrics": n.metrics,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "items": items,
                "next_cursor": serde_json::Value::Null
            }))
        })
    }
}

impl BuilderDashboardPort for MockCapabilityPort {
    fn list_dashboard_briefs<'a>(
        &'a self,
        _tenant_id: &'a str,
    ) -> BoxFuture<'a, Vec<axpoint_domain::builder::BuilderDashboardBrief>> {
        Box::pin(async move {
            Ok(vec![axpoint_domain::builder::BuilderDashboardBrief {
                id: Uuid::from_u128(0x901),
                name: String::from("供应链总览"),
                slug: String::from("supply-overview"),
                status: String::from("DRAFT"),
            }])
        })
    }

    fn upsert_dashboard_draft<'a>(
        &'a self,
        _tenant_id: &'a str,
        _actor_user_id: Option<Uuid>,
        _actor_dept: &'a str,
        input: axpoint_domain::builder::UpsertDashboardDraftInput,
        web_base_url: Option<&'a str>,
    ) -> BoxFuture<'a, axpoint_domain::builder::UpsertDashboardDraftResponse> {
        Box::pin(async move {
            let dashboard_id = input.dashboard_id.unwrap_or_else(Uuid::new_v4);
            let deep_links = axpoint_domain::builder::resolve_deep_links(
                web_base_url,
                None,
                "main",
                None,
                None,
                Some(dashboard_id),
            );
            Ok(axpoint_domain::builder::UpsertDashboardDraftResponse {
                dashboard_id,
                slug: input.slug,
                status: String::from("DRAFT"),
                widget_count: input.widgets.len(),
                deep_links,
            })
        })
    }

    fn validate_flow_draft<'a>(
        &'a self,
        _graph: &'a axpoint_domain::flow::FlowGraph,
        _trigger_cfg: &'a axpoint_domain::flow::TriggerCfg,
        _slug: Option<&'a str>,
    ) -> BoxFuture<'a, Vec<axpoint_domain::flow::FlowIssue>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

impl TemplateSandboxPort for MockCapabilityPort {
    fn list_template_packs(&self) -> BoxFuture<'_, Vec<axpoint_domain::ops::TemplateManifest>> {
        Box::pin(async move {
            let manifest: axpoint_domain::ops::TemplateManifest =
                serde_json::from_value(serde_json::json!({
                    "id": "supply_chain",
                    "name": "供应链协同",
                    "version": "1.0.0",
                    "industry": "制造 / 流通",
                    "description": "供应商—工厂—仓库—订单四层网络",
                    "contents": {
                        "schema": "schema.json",
                        "datasets": [],
                        "flows": [],
                        "dashboards": [],
                        "agents": [],
                        "export_templates": []
                    },
                    "sha256": {}
                }))
                .map_err(|e| AppError::Config(format!("mock manifest: {e}")))?;
            Ok(vec![manifest])
        })
    }

    fn read_template_resource<'a>(
        &'a self,
        pack_id: &'a str,
    ) -> BoxFuture<'a, axpoint_domain::builder::TemplateResourcePayload> {
        Box::pin(async move {
            if pack_id != "supply_chain" {
                return Err(AppError::NotFound("TEMPLATE_NOT_FOUND"));
            }
            Ok(axpoint_domain::builder::TemplateResourcePayload {
                manifest: serde_json::json!({ "id": "supply_chain", "name": "供应链协同" }),
                readme_markdown: String::from("# 供应链协同模板"),
                schema_doc: axpoint_domain::studio::SchemaDoc::default(),
                metrics: serde_json::json!([]),
                mappings: serde_json::json!([]),
            })
        })
    }

    fn clone_template_sandbox<'a>(
        &'a self,
        _tenant_id: &'a str,
        _actor_user_id: Option<Uuid>,
        pack_id: &'a str,
        target_slug: Option<&'a str>,
        _display_name: Option<&'a str>,
        web_base_url: Option<&'a str>,
    ) -> BoxFuture<'a, axpoint_domain::builder::CloneTemplateSandboxResponse> {
        Box::pin(async move {
            let slug = target_slug.unwrap_or("dev_supply_chain").to_owned();
            let draft_id = Uuid::new_v4();
            let deep_links = axpoint_domain::builder::resolve_deep_links(
                web_base_url,
                None,
                &slug,
                Some(draft_id),
                None,
                None,
            );
            Ok(axpoint_domain::builder::CloneTemplateSandboxResponse {
                subgraph_slug: slug,
                is_dev_sandbox: true,
                source_template_id: String::from(pack_id),
                node_type_count: 4,
                edge_type_count: 3,
                draft_id,
                deep_links,
            })
        })
    }

    fn dry_run_dev_subgraph<'a>(
        &'a self,
        _tenant_id: &'a str,
        subgraph_slug: &'a str,
        req: axpoint_domain::builder::DevSubgraphRunRequest,
        web_base_url: Option<&'a str>,
    ) -> BoxFuture<'a, axpoint_domain::builder::DevSubgraphRunResponse> {
        Box::pin(async move {
            if subgraph_slug == "main" {
                return Err(AppError::bad(
                    "DEV_SANDBOX_REQUIRED",
                    "target subgraph must be a non-main dev sandbox",
                ));
            }
            let guard = self.store.inner.lock().await;
            let sample_nodes: Vec<BlueNode> = guard.nodes.iter().take(2).cloned().collect();
            drop(guard);
            let deep_links = axpoint_domain::builder::resolve_deep_links(
                web_base_url,
                None,
                subgraph_slug,
                req.draft_id,
                None,
                None,
            );
            Ok(axpoint_domain::builder::DevSubgraphRunResponse {
                subgraph_slug: String::from(subgraph_slug),
                nodes_upserted: u64::try_from(sample_nodes.len()).unwrap_or(0),
                links_upserted: 1,
                points_computed: 2,
                sample_nodes,
                reject_samples: Vec::new(),
                deep_links,
            })
        })
    }
}

/// Complete assembled Step 09 service stack for integration tests.
pub struct HarnessStack {
    pub store: Arc<MockStep09Store>,
    pub publisher: Arc<CapabilityPublisher<MockStep09Store>>,
    pub mcp_hub: Arc<McpServerHub<MockStep09Store>>,
    pub mcp_clients: Arc<McpClientManager<MockStep09Store>>,
    pub runtime: Arc<AgentRuntime<MockStep09Store>>,
    pub embed: Arc<EmbedService<MockStep09Store>>,
    pub audit_sink: MockAuditSink,
    _audit_writer: AuditWriter,
}

impl HarnessStack {
    /// Builds a complete wired Step 09 service stack over `model` and optional `sim`.
    ///
    /// # Errors
    /// Returns [`AppError`] if seeding or ephemeral keyring generation fails.
    pub async fn build(
        model: Arc<dyn AgentModelPort>,
        sim: Option<Arc<dyn SimExecutionPort>>,
    ) -> Result<Self, AppError> {
        let store = MockStep09Store::seeded().await?;
        let capability = Arc::new(MockCapabilityPort::new(store.clone()));
        let audit_sink = MockAuditSink::default();
        let audit_writer = AuditWriter::new(audit_sink.clone());
        let keyring = Arc::new(CredentialKeyring::ephemeral()?);

        let publisher = Arc::new(CapabilityPublisher::new(
            store.clone(),
            capability.clone(),
            Some(audit_writer.clone()),
        ));
        let mcp_clients = Arc::new(McpClientManager::new(
            store.clone(),
            Arc::new(MockUrlGuard),
            Some(keyring),
        ));
        let builtin_tools = Arc::new(BuiltinToolRegistry::new(
            store.clone(),
            capability.clone(),
            sim,
        ));
        let runtime = Arc::new(AgentRuntime::new(AgentRuntimeDeps {
            store: store.clone(),
            model,
            builtin_tools,
            publisher: publisher.clone(),
            mcp_clients: mcp_clients.clone(),
            audit: Some(audit_writer.clone()),
        }));
        let dyn_invoker: Arc<dyn PublishedAgentInvoker> = runtime.clone();
        publisher.register_agent_invoker(Arc::downgrade(&dyn_invoker));

        let dashboard_port: Arc<dyn BuilderDashboardPort> = capability.clone();
        let sandbox_port: Arc<dyn TemplateSandboxPort> = capability.clone();
        let mcp_hub = Arc::new(
            McpServerHub::new(
                store.clone(),
                capability,
                publisher.clone(),
                Some(audit_writer.clone()),
            )
            .with_builder_ports(
                Some(dashboard_port),
                Some(sandbox_port),
                Some(String::from("https://public.xblue.example")),
            ),
        );
        let embed = Arc::new(EmbedService::new(
            store.clone(),
            runtime.clone(),
            Some(audit_writer.clone()),
        ));

        Ok(Self {
            store,
            publisher,
            mcp_hub,
            mcp_clients,
            runtime,
            embed,
            audit_sink,
            _audit_writer: audit_writer,
        })
    }
}

/// Builds an admin [`AuthPrincipal`] with all permissions and the requested [`TokenKind`].
#[must_use]
pub fn make_principal(token_kind: TokenKind, perms: &[Permission]) -> AuthPrincipal {
    let permissions: BTreeSet<Permission> = if perms.is_empty() {
        Permission::ALL.iter().copied().collect()
    } else {
        perms.iter().copied().collect()
    };
    AuthPrincipal {
        user_id: Uuid::from_u128(1),
        tenant_id: String::from("default"),
        username: String::from("ops_admin"),
        dept_id: String::from("east_china"),
        roles: vec![String::from("ADMIN")],
        permissions,
        token_kind,
        token_profile: axpoint_domain::enums::TokenProfile::General,
        web_base_url: None,
    }
}

impl AgentRepository for MockStep09Store {
    fn list_llm_providers(
        &self,
        _tenant_id: &str,
    ) -> impl Future<Output = Result<Vec<LlmProviderRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn get_llm_provider(
        &self,
        _tenant_id: &str,
        _provider_id: Uuid,
    ) -> impl Future<Output = Result<Option<(LlmProviderRecord, Option<SealedCredential>)>, AppError>>
    {
        std::future::ready(Ok(None))
    }

    fn create_llm_provider(
        &self,
        _tenant_id: &str,
        _input: SaveLlmProviderInput,
    ) -> impl Future<Output = Result<LlmProviderRecord, AppError>> {
        std::future::ready(Err(AppError::NotFound("NOT_FOUND")))
    }

    fn update_llm_provider(
        &self,
        _tenant_id: &str,
        _provider_id: Uuid,
        _input: SaveLlmProviderInput,
    ) -> impl Future<Output = Result<Option<LlmProviderRecord>, AppError>> {
        std::future::ready(Ok(None))
    }

    fn delete_llm_provider(
        &self,
        _tenant_id: &str,
        _provider_id: Uuid,
    ) -> impl Future<Output = Result<bool, AppError>> {
        std::future::ready(Ok(false))
    }

    fn record_llm_usage(
        &self,
        _tenant_id: &str,
        _delta: LlmUsageDelta,
    ) -> impl Future<Output = Result<(), AppError>> {
        std::future::ready(Ok(()))
    }

    fn query_llm_usage(
        &self,
        _tenant_id: &str,
        _query: LlmUsageQuery,
    ) -> impl Future<Output = Result<Vec<LlmUsageRecord>, AppError>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn list_endpoints(
        &self,
        tenant_id: &str,
        query: EndpointListQuery,
    ) -> Result<Vec<EndpointSpec>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .endpoints
            .iter()
            .filter(|e| e.tenant_id == tenant_id)
            .filter(|e| query.environment.is_none_or(|env| e.environment == env))
            .filter(|e| query.kind.is_none_or(|k| e.kind == k))
            .filter(|e| query.show_as_app.is_none_or(|s| e.show_as_app == s))
            .cloned()
            .collect())
    }

    async fn get_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
    ) -> Result<Option<EndpointSpec>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .endpoints
            .iter()
            .find(|e| e.tenant_id == tenant_id && e.id == endpoint_id)
            .cloned())
    }

    async fn get_endpoint_by_slug(
        &self,
        tenant_id: &str,
        environment: Environment,
        slug: &str,
    ) -> Result<Option<EndpointSpec>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .endpoints
            .iter()
            .find(|e| e.tenant_id == tenant_id && e.environment == environment && e.slug == slug)
            .cloned())
    }

    async fn create_endpoint(
        &self,
        tenant_id: &str,
        input: SaveEndpointInput,
    ) -> Result<EndpointSpec, AppError> {
        let mut guard = self.inner.lock().await;
        if guard.endpoints.iter().any(|e| {
            e.tenant_id == tenant_id && e.environment == input.environment && e.slug == input.slug
        }) {
            return Err(AppError::Conflict("CONFLICT"));
        }
        let now = Utc::now();
        let rec = EndpointSpec {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            environment: input.environment,
            slug: input.slug,
            title: input.title,
            description: input.description,
            kind: input.kind,
            target_id: input.target_id,
            target_version: input.target_version,
            target_snapshot: input.target_snapshot,
            input_schema: input.input_schema,
            output_shape: input.output_shape,
            required_scope: input.required_scope,
            rate_limit_rpm: input.rate_limit_rpm,
            cache_ttl_s: input.cache_ttl_s,
            expose_mcp: input.expose_mcp,
            show_as_app: input.show_as_app,
            enabled: input.enabled,
            published_by: input.published_by,
            published_at: now,
        };
        guard.endpoints.push(rec.clone());
        Ok(rec)
    }

    async fn update_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
        req: UpdateEndpointRequest,
    ) -> Result<Option<EndpointSpec>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(ep) = guard
            .endpoints
            .iter_mut()
            .find(|e| e.tenant_id == tenant_id && e.id == endpoint_id)
        else {
            return Ok(None);
        };
        if let Some(t) = req.title {
            ep.title = t;
        }
        if let Some(d) = req.description {
            ep.description = d;
        }
        if let Some(s) = req.input_schema {
            ep.input_schema = s;
        }
        if let Some(sc) = req.required_scope {
            ep.required_scope = sc;
        }
        if let Some(r) = req.rate_limit_rpm {
            ep.rate_limit_rpm = r;
        }
        if let Some(c) = req.cache_ttl_s {
            ep.cache_ttl_s = c;
        }
        if let Some(m) = req.expose_mcp {
            ep.expose_mcp = m;
        }
        if let Some(a) = req.show_as_app {
            ep.show_as_app = a;
        }
        if let Some(en) = req.enabled {
            ep.enabled = en;
        }
        Ok(Some(ep.clone()))
    }

    async fn upsert_endpoint_by_slug(
        &self,
        tenant_id: &str,
        input: SaveEndpointInput,
    ) -> Result<EndpointSpec, AppError> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        if let Some(existing) = guard.endpoints.iter_mut().find(|e| {
            e.tenant_id == tenant_id && e.environment == input.environment && e.slug == input.slug
        }) {
            existing.title = input.title;
            existing.description = input.description;
            existing.target_id = input.target_id;
            existing.target_version = input.target_version;
            existing.target_snapshot = input.target_snapshot;
            existing.input_schema = input.input_schema;
            existing.output_shape = input.output_shape;
            existing.required_scope = input.required_scope;
            existing.rate_limit_rpm = input.rate_limit_rpm;
            existing.cache_ttl_s = input.cache_ttl_s;
            existing.expose_mcp = input.expose_mcp;
            existing.show_as_app = input.show_as_app;
            existing.enabled = input.enabled;
            existing.published_by = input.published_by;
            existing.published_at = now;
            return Ok(existing.clone());
        }
        drop(guard);
        self.create_endpoint(tenant_id, input).await
    }

    async fn delete_endpoint(&self, tenant_id: &str, endpoint_id: Uuid) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        let before = guard.endpoints.len();
        guard
            .endpoints
            .retain(|e| !(e.tenant_id == tenant_id && e.id == endpoint_id));
        Ok(guard.endpoints.len() < before)
    }

    async fn list_mcp_clients(&self, _tenant_id: &str) -> Result<Vec<McpClientRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.mcp_clients.iter().map(|(c, _)| c.clone()).collect())
    }

    async fn get_mcp_client(
        &self,
        _tenant_id: &str,
        client_id: Uuid,
    ) -> Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .mcp_clients
            .iter()
            .find(|(c, _)| c.id == client_id)
            .cloned())
    }

    async fn get_mcp_client_by_name(
        &self,
        _tenant_id: &str,
        name: &str,
    ) -> Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .mcp_clients
            .iter()
            .find(|(c, _)| c.name == name)
            .cloned())
    }

    async fn create_mcp_client(
        &self,
        tenant_id: &str,
        input: SaveMcpClientInput,
    ) -> Result<McpClientRecord, AppError> {
        let mut guard = self.inner.lock().await;
        if guard.mcp_clients.iter().any(|(c, _)| c.name == input.name) {
            return Err(AppError::Conflict("CONFLICT"));
        }
        let rec = McpClientRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            name: input.name,
            url: input.url,
            has_headers: input.credential.is_some(),
            credential_id: input.credential.as_ref().map(|c| c.id),
            allowed_tools: input.allowed_tools,
            tools_cache: Vec::new(),
            allow_private_network: input.allow_private_network,
            enabled: input.enabled,
            last_synced_at: None,
            created_at: Utc::now(),
        };
        guard.mcp_clients.push((rec.clone(), input.credential));
        Ok(rec)
    }

    async fn delete_mcp_client(&self, _tenant_id: &str, client_id: Uuid) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        let before = guard.mcp_clients.len();
        guard.mcp_clients.retain(|(c, _)| c.id != client_id);
        Ok(guard.mcp_clients.len() < before)
    }

    async fn update_mcp_client_tools(
        &self,
        _tenant_id: &str,
        client_id: Uuid,
        tools: &[McpToolInfo],
    ) -> Result<Option<McpClientRecord>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some((rec, _)) = guard
            .mcp_clients
            .iter_mut()
            .find(|(c, _)| c.id == client_id)
        else {
            return Ok(None);
        };
        rec.tools_cache = tools.to_vec();
        rec.last_synced_at = Some(Utc::now());
        Ok(Some(rec.clone()))
    }

    async fn list_agents(&self, _tenant_id: &str) -> Result<Vec<AgentRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.agents.clone())
    }

    async fn get_agent(
        &self,
        _tenant_id: &str,
        agent_id: Uuid,
    ) -> Result<Option<AgentRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.agents.iter().find(|a| a.id == agent_id).cloned())
    }

    async fn get_agent_by_name(
        &self,
        _tenant_id: &str,
        name: &str,
    ) -> Result<Option<AgentRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard.agents.iter().find(|a| a.name == name).cloned())
    }

    async fn create_agent(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAgentRequest,
    ) -> Result<AgentRecord, AppError> {
        let mut guard = self.inner.lock().await;
        if guard.agents.iter().any(|a| a.name == req.name) {
            return Err(AppError::Conflict("CONFLICT"));
        }
        let now = Utc::now();
        let rec = AgentRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            name: req.name,
            description: req.description,
            version: 0,
            spec: req.spec,
            created_by,
            created_at: now,
            updated_at: now,
        };
        guard.agents.push(rec.clone());
        Ok(rec)
    }

    async fn update_agent(
        &self,
        _tenant_id: &str,
        agent_id: Uuid,
        req: UpdateAgentRequest,
    ) -> Result<Option<AgentRecord>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(rec) = guard.agents.iter_mut().find(|a| a.id == agent_id) else {
            return Ok(None);
        };
        if let Some(n) = req.name {
            rec.name = n;
        }
        if let Some(d) = req.description {
            rec.description = d;
        }
        rec.spec = req.spec;
        rec.updated_at = Utc::now();
        Ok(Some(rec.clone()))
    }

    async fn delete_agent(&self, _tenant_id: &str, agent_id: Uuid) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        let before = guard.agents.len();
        guard.agents.retain(|a| a.id != agent_id);
        Ok(guard.agents.len() < before)
    }

    async fn publish_agent_version(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        published_by: Option<Uuid>,
    ) -> Result<AgentVersionRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(agent) = guard.agents.iter_mut().find(|a| a.id == agent_id) else {
            return Err(AppError::NotFound("NOT_FOUND"));
        };
        agent.version = agent.version.saturating_add(1);
        agent.updated_at = Utc::now();
        let ver = AgentVersionRecord {
            tenant_id: String::from(tenant_id),
            agent_id,
            version: agent.version,
            spec: agent.spec.clone(),
            published_by,
            published_at: Utc::now(),
        };
        guard.agent_versions.push(ver.clone());
        Ok(ver)
    }

    async fn get_agent_version(
        &self,
        _tenant_id: &str,
        agent_id: Uuid,
        version: i32,
    ) -> Result<Option<AgentVersionRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .agent_versions
            .iter()
            .find(|v| v.agent_id == agent_id && v.version == version)
            .cloned())
    }

    async fn list_agent_versions(
        &self,
        _tenant_id: &str,
        agent_id: Uuid,
    ) -> Result<Vec<AgentVersionRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .agent_versions
            .iter()
            .filter(|v| v.agent_id == agent_id)
            .cloned()
            .collect())
    }

    async fn create_conversation(
        &self,
        tenant_id: &str,
        input: NewConversationInput,
    ) -> Result<ConversationRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let now = Utc::now();
        let rec = ConversationRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            agent_id: input.agent_id,
            agent_version: input.agent_version,
            user_id: input.user_id,
            channel: input.channel,
            embed_id: input.embed_id,
            title: input.title,
            status: ConversationStatus::Active,
            created_at: now,
            updated_at: now,
        };
        guard.conversations.push(rec.clone());
        Ok(rec)
    }

    async fn get_conversation(
        &self,
        _tenant_id: &str,
        conversation_id: Uuid,
    ) -> Result<Option<ConversationRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .conversations
            .iter()
            .find(|c| c.id == conversation_id)
            .cloned())
    }

    async fn find_flow_conversation(
        &self,
        _tenant_id: &str,
        agent_id: Uuid,
        idempotency_title: &str,
    ) -> Result<Option<ConversationRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .conversations
            .iter()
            .find(|c| c.agent_id == agent_id && c.title == idempotency_title)
            .cloned())
    }

    async fn list_user_conversations(
        &self,
        _tenant_id: &str,
        user_id: Uuid,
        query: ConversationListQuery,
    ) -> Result<PaginatedList<ConversationRecord>, AppError> {
        let guard = self.inner.lock().await;
        let items = guard
            .conversations
            .iter()
            .filter(|c| c.user_id == Some(user_id))
            .filter(|c| query.agent_id.is_none_or(|aid| c.agent_id == aid))
            .cloned()
            .collect();
        Ok(PaginatedList {
            items,
            next_cursor: None,
        })
    }

    async fn update_conversation_status(
        &self,
        _tenant_id: &str,
        conversation_id: Uuid,
        status: ConversationStatus,
    ) -> Result<Option<ConversationRecord>, AppError> {
        let mut guard = self.inner.lock().await;
        let Some(conv) = guard
            .conversations
            .iter_mut()
            .find(|c| c.id == conversation_id)
        else {
            return Ok(None);
        };
        conv.status = status;
        conv.updated_at = Utc::now();
        Ok(Some(conv.clone()))
    }

    async fn delete_conversation(
        &self,
        _tenant_id: &str,
        conversation_id: Uuid,
        owner_user_id: Option<Uuid>,
    ) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        let before = guard.conversations.len();
        guard.conversations.retain(|c| {
            if c.id != conversation_id {
                return true;
            }
            owner_user_id.is_some_and(|uid| c.user_id != Some(uid))
        });
        Ok(guard.conversations.len() < before)
    }

    async fn append_message(
        &self,
        tenant_id: &str,
        input: NewMessageInput,
    ) -> Result<MessageRecord, AppError> {
        let mut guard = self.inner.lock().await;
        guard.next_msg_id = guard.next_msg_id.saturating_add(1);
        let rec = MessageRecord {
            id: guard.next_msg_id,
            tenant_id: String::from(tenant_id),
            conversation_id: input.conversation_id,
            role: input.role,
            content: input.content,
            tool_calls: input.tool_calls,
            tool_call_id: input.tool_call_id,
            agent_name: input.agent_name,
            provider_id: input.provider_id,
            prompt_tokens: input.prompt_tokens,
            completion_tokens: input.completion_tokens,
            latency_ms: input.latency_ms,
            created_at: Utc::now(),
        };
        guard.messages.push(rec.clone());
        Ok(rec)
    }

    async fn list_conversation_messages(
        &self,
        _tenant_id: &str,
        conversation_id: Uuid,
    ) -> Result<Vec<MessageRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .messages
            .iter()
            .filter(|m| m.conversation_id == conversation_id)
            .cloned()
            .collect())
    }

    async fn create_agent_embed(
        &self,
        tenant_id: &str,
        input: NewAgentEmbedInput,
    ) -> Result<AgentEmbedRecord, AppError> {
        let mut guard = self.inner.lock().await;
        let rec = AgentEmbedRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            agent_id: input.agent_id,
            allowed_origins: input.allowed_origins,
            permissions: input.permissions,
            expires_at: input.expires_at,
            revoked_at: None,
            rate_limit_rpm: input.rate_limit_rpm,
            created_by: input.created_by,
            created_at: Utc::now(),
        };
        guard.embeds.push((input.token_hash, rec.clone()));
        Ok(rec)
    }

    async fn list_agent_embeds(
        &self,
        _tenant_id: &str,
        agent_id: Uuid,
    ) -> Result<Vec<AgentEmbedRecord>, AppError> {
        let guard = self.inner.lock().await;
        Ok(guard
            .embeds
            .iter()
            .filter(|(_, r)| r.agent_id == agent_id)
            .map(|(_, r)| r.clone())
            .collect())
    }

    async fn revoke_agent_embed(&self, _tenant_id: &str, embed_id: Uuid) -> Result<bool, AppError> {
        let mut guard = self.inner.lock().await;
        for (_, rec) in &mut guard.embeds {
            if rec.id == embed_id && rec.revoked_at.is_none() {
                rec.revoked_at = Some(Utc::now());
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn resolve_agent_embed(
        &self,
        token_hash: &str,
    ) -> Result<Option<ResolvedAgentEmbed>, AppError> {
        let guard = self.inner.lock().await;
        let now = Utc::now();
        let Some((_, rec)) = guard.embeds.iter().find(|(h, r)| {
            h == token_hash && r.revoked_at.is_none() && r.expires_at.is_none_or(|exp| exp > now)
        }) else {
            return Ok(None);
        };
        Ok(Some(ResolvedAgentEmbed {
            embed_id: rec.id,
            tenant_id: rec.tenant_id.clone(),
            agent_id: rec.agent_id,
            allowed_origins: rec.allowed_origins.clone(),
            permissions: rec.permissions.iter().copied().collect(),
            rate_limit_rpm: rec.rate_limit_rpm,
            created_by: rec.created_by,
        }))
    }
}
