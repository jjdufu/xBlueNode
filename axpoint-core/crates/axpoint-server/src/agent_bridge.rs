//! Dependency-inversion adapters wiring `AxAgent` (`CapabilityPublisher`, `McpServerHub`,
//! `McpClientManager`, `AgentRuntime`, `EmbedService`) and `LlmRouter` into `axpoint-server`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use axpoint_agent::{
    AgentRuntime, AgentRuntimeDeps, BuiltinToolRegistry, CapabilityPublisher, EmbedService,
    ImGatewayDeps, ImGatewaySupervisor, McpClientManager, McpServerHub, PublishedAgentInvoker,
};
use axpoint_db::PgStore;
use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::analytics::{
    AnalyticsPrincipal, AskContext, AskRequest, MetricDef, MetricQuery, MetricQueryResult,
    QueryRequest, QueryResult, SavedQuery,
};
use axpoint_domain::builder::{
    BuilderDashboardBrief, CloneTemplateSandboxResponse, DevSubgraphRunRequest,
    DevSubgraphRunResponse, TemplateResourcePayload, UpsertDashboardDraftInput,
    UpsertDashboardDraftResponse, resolve_deep_links,
};
use axpoint_domain::enums::{AggFn, MaskRule, Permission, PiiCategory, TokenKind};
use axpoint_domain::expr::{EvalEnv, ExprValue, eval as eval_expr, parse as parse_expr};
use axpoint_domain::flow::{FlowGraph, FlowIssue, TriggerCfg};
use axpoint_domain::graph::{GeoPoint, NodeListQuery};
use axpoint_domain::ops::TemplateManifest;
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{
    AgentCapabilityPort, AnalyticsQueryPort as _, BuilderDashboardPort, ConnectionRepository as _,
    DatasetRepository as _, FlowCredentialUnsealPort, GraphStorePort as _, LlmProviderStore,
    SimExecutionPort, StudioRepository as _, TemplateSandboxPort, ViewRepository as _,
};
use axpoint_domain::rollup::{MetricDependencyPlan, MetricRuleStep};
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::studio::{
    AggRuleSpec, DomainObjectListQuery, DomainObjectRecord, DraftStatus, LinkDirection,
    MappingLinkSpec, MappingRejectSample, MappingSpec, NodeTypeSpec, PropertyKind, SchemaDoc,
};
use axpoint_domain::topology::BlueNode;
use axpoint_domain::view::{
    CanvasConfig, CreateDashboardRequest, CreateWidgetRecordParams, DashboardStatus,
    DashboardTheme, RefreshMode, UpdateDashboardRequest,
};
use axpoint_domain::{AppError, FusedHit};
use axpoint_llm::{EmbeddingConfig, LlmConfig, LlmRouter};
use axpoint_security::masking::mask_json_properties;
use axpoint_security::{AuditWriter, AuthPrincipal, CredentialKeyring, NetGuard};
use axpoint_server::view::ai_gen::{validate_dashboard_slug, validate_widget_position};
use axpoint_server::view::deps::compute_widget_deps;
use chrono::Utc;
use sqlx::Row as _;
use uuid::Uuid;

use crate::routes::analytics::{
    acquire_tenant_session, execute_ask_for_principal, execute_published_metric,
    execute_published_saved_query, load_tenant_catalog_and_live,
};
use crate::routes::documents::{mask_recalled_hits, recall_knowledge_hits};
use crate::routes::open_secret_map;
use crate::routes::xblue::xblue_graphql::{
    compute_neighbor_sigs, mask_node_attributes_with_policy,
};
use crate::state::AppState;

/// Unseals `ds_credentials` rows using `PgStore` and `CredentialKeyring` without circular `AppState` references.
pub(crate) struct KeyringSecretUnsealer {
    store: PgStore,
    keyring: Arc<CredentialKeyring>,
}

impl KeyringSecretUnsealer {
    pub(crate) const fn new(store: PgStore, keyring: Arc<CredentialKeyring>) -> Self {
        Self { store, keyring }
    }
}

impl FlowCredentialUnsealPort for KeyringSecretUnsealer {
    fn unseal_credential_json<'a>(
        &'a self,
        tenant_id: &'a str,
        credential_id: Uuid,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let sealed = self
                .store
                .get_credential(tenant_id, credential_id)
                .await?
                .ok_or(AppError::NotFound("CREDENTIAL_NOT_FOUND"))?;
            let secret_map = open_secret_map(&self.keyring, tenant_id, Some(&sealed))?;
            let mut obj = serde_json::Map::with_capacity(secret_map.len());
            for (k, v) in &secret_map {
                obj.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            Ok(serde_json::Value::Object(obj))
        })
    }
}

/// Builds a tenant-aware [`LlmRouter`] wired to `PgStore`, `CredentialKeyring`, and `NetGuard`.
pub(crate) fn build_llm_router(
    store: &PgStore,
    keyring: &Arc<CredentialKeyring>,
    default_llm: LlmConfig,
    default_embed: EmbeddingConfig,
) -> Arc<LlmRouter> {
    let provider_store: Arc<dyn LlmProviderStore> = Arc::new(store.clone());
    let unsealer = Arc::new(KeyringSecretUnsealer::new(
        store.clone(),
        Arc::clone(keyring),
    ));
    let url_guard = Arc::new(NetGuard::new());
    Arc::new(
        LlmRouter::new(default_llm)
            .with_default_embed(default_embed)
            .with_store(provider_store)
            .with_cred_unsealer(unsealer)
            .with_url_guard(url_guard),
    )
}

/// Assembled Step 09, Step 17 & Step 21 `AxAgent` services shared across HTTP routes and background workers.
pub(crate) struct AxAgentServices {
    pub(crate) capability: Arc<ServerAgentCapability>,
    pub(crate) publisher: Arc<CapabilityPublisher<PgStore>>,
    pub(crate) mcp_hub: Arc<McpServerHub<PgStore>>,
    pub(crate) mcp_clients: Arc<McpClientManager<PgStore>>,
    pub(crate) runtime: Arc<AgentRuntime<PgStore>>,
    pub(crate) embed: Arc<EmbedService<PgStore>>,
    pub(crate) im_supervisor: Arc<ImGatewaySupervisor<PgStore>>,
    pub(crate) public_host: Option<String>,
}

/// Constructs the complete Step 09 & Step 17 [`AxAgentServices`] stack.
pub(crate) fn build_axagent_services(
    store: &PgStore,
    llm_router: &Arc<LlmRouter>,
    keyring: &Arc<CredentialKeyring>,
    audit: &AuditWriter,
    sim_port: Option<Arc<dyn SimExecutionPort>>,
) -> Arc<AxAgentServices> {
    let store_arc = Arc::new(store.clone());
    let capability = Arc::new(ServerAgentCapability::new());
    let cap_concrete = Arc::clone(&capability);
    let cap_port: Arc<dyn AgentCapabilityPort> = cap_concrete;
    let dash_concrete = Arc::clone(&capability);
    let dash_port: Arc<dyn BuilderDashboardPort> = dash_concrete;
    let sandbox_concrete = Arc::clone(&capability);
    let sandbox_port: Arc<dyn TemplateSandboxPort> = sandbox_concrete;
    let public_host = std::env::var("AXPOINT_PUBLIC_HOST")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let url_guard = Arc::new(NetGuard::new());

    let publisher = Arc::new(CapabilityPublisher::new(
        Arc::clone(&store_arc),
        Arc::clone(&cap_port),
        Some(audit.clone()),
    ));
    let mcp_clients = Arc::new(McpClientManager::new(
        Arc::clone(&store_arc),
        url_guard,
        Some(Arc::clone(keyring)),
    ));
    let builtin_tools = Arc::new(BuiltinToolRegistry::new(
        Arc::clone(&store_arc),
        Arc::clone(&cap_port),
        sim_port,
    ));
    let model_concrete = Arc::clone(llm_router);
    let model_port: Arc<dyn axpoint_agent::AgentModelPort> = model_concrete;
    let runtime = Arc::new(AgentRuntime::new(AgentRuntimeDeps {
        store: Arc::clone(&store_arc),
        model: model_port,
        builtin_tools,
        publisher: Arc::clone(&publisher),
        mcp_clients: Arc::clone(&mcp_clients),
        audit: Some(audit.clone()),
    }));
    let invoker_concrete = Arc::clone(&runtime);
    let invoker_arc: Arc<dyn PublishedAgentInvoker> = invoker_concrete;
    publisher.register_agent_invoker(Arc::downgrade(&invoker_arc));

    let mcp_hub = Arc::new(
        McpServerHub::new(
            Arc::clone(&store_arc),
            cap_port,
            Arc::clone(&publisher),
            Some(audit.clone()),
        )
        .with_builder_ports(Some(dash_port), Some(sandbox_port), public_host.clone()),
    );
    let embed = Arc::new(EmbedService::new(
        Arc::clone(&store_arc),
        Arc::clone(&runtime),
        Some(audit.clone()),
    ));
    let im_url_guard: Arc<dyn axpoint_domain::ports::OutboundUrlGuardPort> =
        Arc::new(NetGuard::new());
    let im_supervisor = ImGatewaySupervisor::new(ImGatewayDeps {
        store: store_arc,
        runtime: Arc::clone(&runtime),
        keyring: Arc::clone(keyring),
        url_guard: im_url_guard,
        holder_id: None,
        default_web_base_url: public_host.clone(),
    });

    Arc::new(AxAgentServices {
        capability,
        publisher,
        mcp_hub,
        mcp_clients,
        runtime,
        embed,
        im_supervisor,
        public_host,
    })
}

/// Server implementation of [`AgentCapabilityPort`] bridging `AxAgent` into `AxQuery`, `AxAnalytics`, and `xBlueStudio`.
pub(crate) struct ServerAgentCapability {
    state: OnceLock<AppState>,
}

impl ServerAgentCapability {
    pub(crate) const fn new() -> Self {
        Self {
            state: OnceLock::new(),
        }
    }

    pub(crate) fn bind_state(&self, state: AppState) {
        let _already_set = self.state.set(state).is_err();
    }

    fn require_state(&self) -> Result<&AppState, AppError> {
        self.state.get().ok_or_else(|| {
            AppError::Config(String::from("ServerAgentCapability AppState is not bound"))
        })
    }
}

impl AgentCapabilityPort for ServerAgentCapability {
    fn search_knowledge<'a>(
        &'a self,
        tenant_id: &'a str,
        query: &'a str,
        top_k: usize,
        doc_title_prefix: Option<&'a str>,
    ) -> BoxFuture<'a, Vec<FusedHit>> {
        Box::pin(async move {
            let state = self.require_state()?;
            let (mut hits, _seeds) =
                recall_knowledge_hits(state, tenant_id, query, top_k, doc_title_prefix).await?;
            mask_recalled_hits(state, tenant_id, &mut hits, false).await?;
            Ok(hits)
        })
    }

    fn ask_data<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        question: &'a str,
        dataset_names: &'a [String],
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            if !principal.has_permission(Permission::DataRead) {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
            let state = self.require_state()?;
            let context =
                resolve_ask_dataset_context(state, &principal.tenant_id, dataset_names).await?;
            let auth = resolve_auth_principal(state, principal).await?;
            let resp = execute_ask_for_principal(
                state,
                &auth,
                AskRequest {
                    question: String::from(question),
                    session_id: None,
                    context,
                },
                None,
                true,
            )
            .await?;
            serde_json::to_value(resp)
                .map_err(|err| AppError::Config(format!("serialize AskResponse failed: {err}")))
        })
    }

    fn run_sql_bounded<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        sql: &'a str,
        max_rows: usize,
    ) -> BoxFuture<'a, QueryResult> {
        Box::pin(async move {
            let state = self.require_state()?;
            state
                .run_sql(
                    principal,
                    QueryRequest {
                        sql: String::from(sql),
                        max_rows: Some(u32::try_from(max_rows.clamp(1, 200)).unwrap_or(200)),
                        ..QueryRequest::default()
                    },
                )
                .await
        })
    }

    fn run_metric_endpoint<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        metric_name: &'a str,
        pinned_snapshot: Option<&'a MetricDef>,
        query: MetricQuery,
    ) -> BoxFuture<'a, MetricQueryResult> {
        Box::pin(async move {
            let state = self.require_state()?;
            execute_published_metric(state, principal, metric_name, pinned_snapshot, query).await
        })
    }

    fn run_saved_endpoint<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        saved_id: Uuid,
        pinned_snapshot: Option<&'a SavedQuery>,
        params: BTreeMap<String, serde_json::Value>,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let state = self.require_state()?;
            execute_published_saved_query(state, principal, saved_id, pinned_snapshot, params).await
        })
    }

    fn query_domain_objects<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        type_name: &'a str,
        pinned_snapshot: Option<&'a NodeTypeSpec>,
        payload: &'a serde_json::Value,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let state = self.require_state()?;
            execute_domain_object_query(state, principal, type_name, pinned_snapshot, payload).await
        })
    }

    fn list_published_object_actions<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> BoxFuture<'a, Vec<axpoint_domain::studio::PublishedObjectActionDescriptor>> {
        Box::pin(async move {
            let state = self.require_state()?;
            let published = state.store.get_published_schema(tenant_id, "main").await?;
            let mut out = Vec::new();
            for nt in published.doc.node_types {
                for action in nt.actions {
                    if action.maturity == axpoint_domain::enums::DesignMaturity::Confirmed {
                        out.push(axpoint_domain::studio::PublishedObjectActionDescriptor {
                            type_name: nt.name.clone(),
                            type_display_name: nt.display_name.clone(),
                            action,
                        });
                    }
                }
            }
            Ok(out)
        })
    }

    fn invoke_published_object_action<'a>(
        &'a self,
        principal: &'a AnalyticsPrincipal,
        type_name: &'a str,
        node_key: &'a str,
        action_name: &'a str,
        req: axpoint_domain::studio::InvokeObjectActionRequest,
    ) -> BoxFuture<'a, axpoint_domain::studio::InvokeObjectActionResponse> {
        Box::pin(async move {
            if !principal.has_permission(Permission::FlowRun) {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
            let state = self.require_state()?;
            let actor = (!principal.user_id.is_nil()).then_some(principal.user_id);
            state
                .store
                .invoke_object_action(
                    &principal.tenant_id,
                    actor,
                    type_name,
                    node_key,
                    action_name,
                    req,
                )
                .await
        })
    }
}

async fn resolve_ask_dataset_context(
    state: &AppState,
    tenant_id: &str,
    dataset_names: &[String],
) -> Result<Option<AskContext>, AppError> {
    if dataset_names.is_empty() {
        return Ok(None);
    }
    let (catalog, _, _, _) = load_tenant_catalog_and_live(state, tenant_id, false).await?;
    let mut ids = Vec::new();
    for ds_entry in catalog.datasets.values() {
        if dataset_names.iter().any(|wanted| wanted == &ds_entry.name) {
            ids.push(ds_entry.id);
        }
    }
    if ids.is_empty() {
        Ok(None)
    } else {
        Ok(Some(AskContext {
            dataset_ids: ids,
            ..AskContext::default()
        }))
    }
}

pub(crate) async fn resolve_auth_principal(
    state: &AppState,
    p: &AnalyticsPrincipal,
) -> Result<AuthPrincipal, AppError> {
    if !p.user_id.is_nil()
        && let Some(rec) = axpoint_domain::ports::CredentialStore::load_principal(
            &state.store,
            &p.tenant_id,
            p.user_id,
        )
        .await?
    {
        return Ok(AuthPrincipal {
            user_id: p.user_id,
            tenant_id: p.tenant_id.clone(),
            username: p.username.clone(),
            dept_id: rec.dept_id,
            roles: rec.roles,
            permissions: p.permissions.clone(),
            token_kind: TokenKind::Access,
            token_profile: axpoint_domain::enums::TokenProfile::General,
            web_base_url: None,
        });
    }
    Ok(AuthPrincipal {
        user_id: p.user_id,
        tenant_id: p.tenant_id.clone(),
        username: p.username.clone(),
        dept_id: String::from("default"),
        roles: Vec::new(),
        permissions: p.permissions.clone(),
        token_kind: TokenKind::Access,
        token_profile: axpoint_domain::enums::TokenProfile::General,
        web_base_url: None,
    })
}

struct ParsedObjectFilter {
    keys: Option<Vec<String>>,
    q: Option<String>,
    properties_eq: serde_json::Map<String, serde_json::Value>,
    metrics_min: BTreeMap<String, f64>,
    include_links: BTreeSet<String>,
    limit: u32,
    cursor: Option<String>,
}

fn parse_object_filter(payload: &serde_json::Value) -> ParsedObjectFilter {
    let keys = payload
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    let q = payload
        .get("q")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let properties_eq = payload
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut metrics_min = BTreeMap::new();
    if let Some(m_obj) = payload
        .get("metrics_min")
        .and_then(serde_json::Value::as_object)
    {
        for (k, v) in m_obj {
            if let Some(num) = v.as_f64() {
                metrics_min.insert(k.clone(), num);
            }
        }
    }

    let include_links = payload
        .get("include_links")
        .and_then(serde_json::Value::as_array)
        .map_or_default(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        });

    let limit = payload
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .map_or(50, |v| v.clamp(1, 200));

    let cursor = payload
        .get("cursor")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    ParsedObjectFilter {
        keys,
        q,
        properties_eq,
        metrics_min,
        include_links,
        limit,
        cursor,
    }
}

fn node_matches_extra_filters(node: &BlueNode, filter: &ParsedObjectFilter) -> bool {
    if let Some(ref q) = filter.q {
        let q_low = q.to_lowercase();
        if !node.node_key.to_lowercase().contains(&q_low)
            && !node.title.to_lowercase().contains(&q_low)
        {
            return false;
        }
    }
    for (k, expected) in &filter.properties_eq {
        if node.properties.get(k) != Some(expected) {
            return false;
        }
    }
    for (m_key, min_val) in &filter.metrics_min {
        let Some(&actual) = node.metrics.get(m_key) else {
            return false;
        };
        if actual < *min_val {
            return false;
        }
    }
    true
}

struct AgentNodeMaskCtx<'a> {
    policy: &'a axpoint_domain::guard::PolicyContext,
    has_pii_raw: bool,
    tenant_key: &'a [u8; 32],
}

async fn execute_domain_object_query(
    state: &AppState,
    principal: &AnalyticsPrincipal,
    type_name: &str,
    pinned_snapshot: Option<&NodeTypeSpec>,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    if !principal.has_permission(Permission::DataRead) {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let tenant_id = &principal.tenant_id;
    let published = state.store.get_published_schema(tenant_id, "main").await?;
    let active_nt = match pinned_snapshot {
        Some(snap) => snap.clone(),
        None => published
            .doc
            .node_types
            .iter()
            .find(|n| n.name == type_name.trim())
            .cloned()
            .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?,
    };

    let auth_principal = resolve_auth_principal(state, principal).await?;
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &auth_principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        &active_nt.name,
        "127.0.0.1",
    )
    .await?;
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &auth_principal,
        false,
        "127.0.0.1",
    )
    .await?;
    let tenant_key = state.mask_keys.tenant_key(tenant_id);

    let filter = parse_object_filter(payload);
    let (mut matched_nodes, next_cursor) =
        fetch_filtered_domain_nodes(state, tenant_id, &active_nt.name, &filter).await?;
    if !policy.bypass_row_policies && !matched_nodes.is_empty() {
        let ids: Vec<Uuid> = matched_nodes.iter().map(|n| n.id).collect();
        let visible = state
            .store
            .filter_visible_node_ids(tenant_id, &ids, &policy.node_types)
            .await?;
        matched_nodes.retain(|n| visible.contains(&n.id));
    }

    let mask_ctx = AgentNodeMaskCtx {
        policy: &policy,
        has_pii_raw: principal.has_permission(Permission::PiiRawRead),
        tenant_key: &tenant_key,
    };
    let neighbors_by_parent = if filter.include_links.is_empty() || matched_nodes.is_empty() {
        BTreeMap::new()
    } else {
        load_selected_neighbors(
            state,
            tenant_id,
            &matched_nodes,
            &active_nt,
            &published.doc,
            &filter.include_links,
            &mask_ctx,
        )
        .await?
    };

    let items: Vec<DomainObjectRecord> = matched_nodes
        .iter()
        .map(|n| {
            project_domain_record(
                n,
                &active_nt,
                pinned_snapshot.is_some(),
                &mask_ctx,
                neighbors_by_parent.get(&n.id).cloned(),
            )
        })
        .collect();

    Ok(serde_json::json!({
        "items": items,
        "next_cursor": next_cursor,
    }))
}

async fn fetch_filtered_domain_nodes(
    state: &AppState,
    tenant_id: &str,
    type_name: &str,
    filter: &ParsedObjectFilter,
) -> Result<(Vec<BlueNode>, Option<String>), AppError> {
    if let Some(ref keys) = filter.keys {
        let cap = usize::try_from(filter.limit).unwrap_or(50);
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for key in keys {
            if !seen.insert(key.as_str()) {
                continue;
            }
            if let Some(node) = state
                .store
                .get_domain_object_by_key(tenant_id, type_name, key)
                .await?
                && node_matches_extra_filters(&node, filter)
            {
                out.push(node);
                if out.len() >= cap {
                    break;
                }
            }
        }
        return Ok((out, None));
    }

    let filter_str = (!filter.properties_eq.is_empty())
        .then(|| serde_json::to_string(&filter.properties_eq).ok())
        .flatten();

    if filter.metrics_min.is_empty() {
        return state
            .store
            .query_domain_objects_list(
                tenant_id,
                type_name,
                &DomainObjectListQuery {
                    q: filter.q.clone(),
                    filter: filter_str,
                    limit: Some(filter.limit),
                    cursor: filter.cursor.clone(),
                },
            )
            .await;
    }

    let target_cap = usize::try_from(filter.limit).unwrap_or(50);
    let mut collected = Vec::with_capacity(target_cap);
    let mut cur = filter.cursor.clone();
    for _ in 0_u8..10_u8 {
        let (batch, next) = state
            .store
            .query_domain_objects_list(
                tenant_id,
                type_name,
                &DomainObjectListQuery {
                    q: filter.q.clone(),
                    filter: filter_str.clone(),
                    limit: Some(200),
                    cursor: cur,
                },
            )
            .await?;
        for node in batch {
            if node_matches_extra_filters(&node, filter) {
                collected.push(node);
                if collected.len() >= target_cap {
                    return Ok((collected, next));
                }
            }
        }
        if next.is_none() {
            break;
        }
        cur = next;
    }
    Ok((collected, None))
}

async fn load_selected_neighbors(
    state: &AppState,
    tenant_id: &str,
    parents: &[BlueNode],
    nt: &NodeTypeSpec,
    doc: &SchemaDoc,
    include_links: &BTreeSet<String>,
    mask_ctx: &AgentNodeMaskCtx<'_>,
) -> Result<BTreeMap<Uuid, BTreeMap<String, Vec<DomainObjectRecord>>>, AppError> {
    let sigs: Vec<_> = compute_neighbor_sigs(nt, doc)
        .into_iter()
        .filter(|s| include_links.contains(&s.link_type))
        .collect();
    if sigs.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut batch_keys = Vec::with_capacity(parents.len().saturating_mul(sigs.len()));
    for parent in parents {
        for sig in &sigs {
            batch_keys.push(NeighborBatchKey {
                parent_id: parent.id,
                link_type: sig.link_type.clone(),
                direction: sig.direction,
                target_type: sig.target_type.clone(),
                limit: 50,
            });
        }
    }

    let mut loaded = state
        .store
        .batch_load_neighbors(tenant_id, &batch_keys)
        .await?;
    if !mask_ctx.policy.bypass_row_policies {
        let mut all_ids = Vec::new();
        for list in loaded.values() {
            for nb in list {
                all_ids.push(nb.id);
            }
        }
        if !all_ids.is_empty() {
            let visible = state
                .store
                .filter_visible_node_ids(tenant_id, &all_ids, &mask_ctx.policy.node_types)
                .await?;
            for list in loaded.values_mut() {
                list.retain(|nb| visible.contains(&nb.id));
            }
        }
    }

    let nt_by_name: BTreeMap<&str, &NodeTypeSpec> = doc
        .node_types
        .iter()
        .map(|n| (n.name.as_str(), n))
        .collect();

    let mut by_parent = BTreeMap::new();
    for parent in parents {
        let mut field_map = BTreeMap::new();
        for sig in &sigs {
            let key = NeighborBatchKey {
                parent_id: parent.id,
                link_type: sig.link_type.clone(),
                direction: sig.direction,
                target_type: sig.target_type.clone(),
                limit: 50,
            };
            let target_spec = nt_by_name.get(sig.target_type.as_str()).copied();
            let recs: Vec<DomainObjectRecord> = loaded.get(&key).map_or_default(|list| {
                list.iter()
                    .map(|nb| DomainObjectRecord {
                        id: nb.id,
                        type_name: nb.type_name.clone(),
                        node_key: nb.node_key.clone(),
                        title: nb.title.clone(),
                        attributes: mask_node_attributes_with_policy(
                            nb,
                            target_spec,
                            Some(mask_ctx.policy),
                            mask_ctx.has_pii_raw,
                            mask_ctx.tenant_key,
                        ),
                        metrics: nb.metrics.clone(),
                        geo: nb.geo,
                        updated_at: nb.updated_at,
                        neighbors: None,
                    })
                    .collect()
            });
            field_map.insert(sig.field_name.clone(), recs);
        }
        by_parent.insert(parent.id, field_map);
    }
    Ok(by_parent)
}

fn project_domain_record(
    node: &BlueNode,
    nt: &NodeTypeSpec,
    strict_snapshot_projection: bool,
    mask_ctx: &AgentNodeMaskCtx<'_>,
    neighbors: Option<BTreeMap<String, Vec<DomainObjectRecord>>>,
) -> DomainObjectRecord {
    let mut attributes = mask_node_attributes_with_policy(
        node,
        Some(nt),
        Some(mask_ctx.policy),
        mask_ctx.has_pii_raw,
        mask_ctx.tenant_key,
    );
    let mut metrics = node.metrics.clone();

    if strict_snapshot_projection && !nt.properties.is_empty() {
        let allowed_attrs: BTreeSet<&str> = nt
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Attribute)
            .map(|p| p.name.as_str())
            .collect();
        if !allowed_attrs.is_empty() {
            attributes.retain(|k, _| allowed_attrs.contains(k.as_str()));
        }

        let mut allowed_metrics: BTreeSet<&str> = nt
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Metric)
            .map(|p| p.name.as_str())
            .collect();
        for rule in &nt.agg_rules {
            allowed_metrics.insert(rule.target_metric.as_str());
        }
        for rule in &nt.derived_rules {
            allowed_metrics.insert(rule.target_metric.as_str());
        }
        if !allowed_metrics.is_empty() {
            metrics.retain(|k, _| allowed_metrics.contains(k.as_str()));
        }
    }

    DomainObjectRecord {
        id: node.id,
        type_name: node.type_name.clone(),
        node_key: node.node_key.clone(),
        title: node.title.clone(),
        attributes,
        metrics,
        geo: node.geo,
        updated_at: node.updated_at,
        neighbors,
    }
}

impl BuilderDashboardPort for ServerAgentCapability {
    fn list_dashboard_briefs<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> BoxFuture<'a, Vec<BuilderDashboardBrief>> {
        Box::pin(async move {
            let state = self.require_state()?;
            let page = state
                .store
                .list_dashboards(
                    tenant_id,
                    false,
                    SimplePageQuery {
                        limit: Some(200),
                        cursor: None,
                    },
                )
                .await?;
            Ok(page
                .items
                .into_iter()
                .map(|d| BuilderDashboardBrief {
                    id: d.id,
                    name: d.name,
                    slug: d.slug,
                    status: String::from(d.status.as_str()),
                })
                .collect())
        })
    }

    fn upsert_dashboard_draft<'a>(
        &'a self,
        tenant_id: &'a str,
        actor_user_id: Option<Uuid>,
        _actor_dept: &'a str,
        input: UpsertDashboardDraftInput,
        web_base_url: Option<&'a str>,
    ) -> BoxFuture<'a, UpsertDashboardDraftResponse> {
        Box::pin(async move {
            let state = self.require_state()?;
            upsert_dashboard_draft_impl(state, tenant_id, actor_user_id, input, web_base_url).await
        })
    }

    fn validate_flow_draft<'a>(
        &'a self,
        graph: &'a FlowGraph,
        trigger_cfg: &'a TriggerCfg,
        slug: Option<&'a str>,
    ) -> BoxFuture<'a, Vec<FlowIssue>> {
        Box::pin(
            async move { Ok(axpoint_flow::validate_flow_default(graph, trigger_cfg, slug).issues) },
        )
    }
}

impl TemplateSandboxPort for ServerAgentCapability {
    fn list_template_packs(&self) -> BoxFuture<'_, Vec<TemplateManifest>> {
        Box::pin(async move {
            let state = self.require_state()?;
            Ok(state.ops.list_templates()?.items)
        })
    }

    fn read_template_resource<'a>(
        &'a self,
        pack_id: &'a str,
    ) -> BoxFuture<'a, TemplateResourcePayload> {
        Box::pin(
            async move { axpoint_server::ops::templates::read_template_resource_payload(pack_id) },
        )
    }

    fn clone_template_sandbox<'a>(
        &'a self,
        tenant_id: &'a str,
        actor_user_id: Option<Uuid>,
        pack_id: &'a str,
        target_slug: Option<&'a str>,
        display_name: Option<&'a str>,
        web_base_url: Option<&'a str>,
    ) -> BoxFuture<'a, CloneTemplateSandboxResponse> {
        Box::pin(async move {
            let state = self.require_state()?;
            let eff_web = web_base_url.or(state.axagent.public_host.as_deref());
            state
                .ops
                .clone_template_sandbox(
                    tenant_id,
                    actor_user_id,
                    pack_id,
                    target_slug,
                    display_name,
                    eff_web,
                )
                .await
        })
    }

    fn dry_run_dev_subgraph<'a>(
        &'a self,
        tenant_id: &'a str,
        subgraph_slug: &'a str,
        req: DevSubgraphRunRequest,
        web_base_url: Option<&'a str>,
    ) -> BoxFuture<'a, DevSubgraphRunResponse> {
        Box::pin(async move {
            self.execute_dev_subgraph_dry_run(tenant_id, subgraph_slug, req, web_base_url)
                .await
        })
    }
}

impl ServerAgentCapability {
    pub(crate) async fn execute_dev_subgraph_dry_run(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        req: DevSubgraphRunRequest,
        web_base_url: Option<&str>,
    ) -> Result<DevSubgraphRunResponse, AppError> {
        let state = self.require_state()?;
        dry_run_dev_subgraph_impl(state, tenant_id, subgraph_slug, req, web_base_url).await
    }

    pub(crate) async fn mask_dev_sample_nodes(
        &self,
        principal: &AuthPrincipal,
        client_ip: &str,
        subgraph_slug: &str,
        draft_id: Option<Uuid>,
        nodes: &mut [BlueNode],
    ) {
        if principal.has(Permission::PiiRawRead) || nodes.is_empty() {
            return;
        }
        let Ok(state) = self.require_state() else {
            return;
        };
        let tenant_id = &principal.tenant_id;
        let policy_opt = axpoint_server::policy::build_policy_context_for_subgraph(
            &state.store,
            &state.cedar,
            principal,
            subgraph_slug,
            false,
            client_ip,
        )
        .await
        .ok();
        let doc_opt = resolve_sandbox_schema(state, tenant_id, subgraph_slug, draft_id)
            .await
            .ok()
            .map(|(_, d, _)| d);
        let tenant_key = state.mask_keys.tenant_key(tenant_id);

        for node in nodes.iter_mut() {
            let nt_opt = doc_opt
                .as_ref()
                .and_then(|d| d.node_types.iter().find(|n| n.name == node.type_name));
            let mut masks = axpoint_server::policy::effective_property_masks(
                &node.type_name,
                nt_opt,
                policy_opt.as_ref(),
                false,
            );
            if let Some(nt) = nt_opt {
                for prop in &nt.properties {
                    if prop.pii_category != PiiCategory::None && !masks.contains_key(&prop.name) {
                        masks.insert(
                            prop.name.clone(),
                            MaskRule::from_pii_category(prop.pii_category),
                        );
                    }
                }
            }
            if !masks.is_empty() {
                mask_json_properties(&mut node.properties, &masks, &tenant_key);
            }
        }
    }
}

fn parse_dashboard_theme(raw: Option<&str>) -> Result<DashboardTheme, AppError> {
    let Some(val) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DashboardTheme::Light);
    };
    match val {
        "light" => Ok(DashboardTheme::Light),
        "dark" => Ok(DashboardTheme::Dark),
        "screen" => Ok(DashboardTheme::Screen),
        other => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid dashboard theme '{other}'; expected light, dark, or screen"),
        )),
    }
}

async fn replace_dashboard_draft_widgets(
    state: &AppState,
    tenant_id: &str,
    dashboard_id: Uuid,
    widgets: Vec<axpoint_domain::builder::WidgetDraftSpec>,
) -> Result<usize, AppError> {
    let mut prepared_widgets = Vec::with_capacity(widgets.len());
    for w in widgets {
        validate_widget_position(&w.position)?;
        let deps = compute_widget_deps(&state.store, tenant_id, &w.data_source).await?;
        prepared_widgets.push((w, deps));
    }
    let widget_count = prepared_widgets.len();
    for (w, deps) in prepared_widgets {
        let _created_w = state
            .store
            .create_widget(
                tenant_id,
                CreateWidgetRecordParams {
                    dashboard_id,
                    widget_kind: w.widget_kind,
                    title: w.title,
                    position: w.position,
                    data_source: w.data_source,
                    encoding: w.encoding,
                    options: w.options,
                    drill: Vec::new(),
                    refresh_mode: RefreshMode::Manual,
                    refresh_seconds: 60,
                    deps,
                },
            )
            .await?;
    }
    Ok(widget_count)
}

async fn upsert_dashboard_draft_impl(
    state: &AppState,
    tenant_id: &str,
    actor_user_id: Option<Uuid>,
    input: UpsertDashboardDraftInput,
    web_base_url: Option<&str>,
) -> Result<UpsertDashboardDraftResponse, AppError> {
    let name = input.name.trim();
    let slug = input.slug.trim();
    if name.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "dashboard name cannot be empty",
        ));
    }
    validate_dashboard_slug(slug)?;
    let theme = parse_dashboard_theme(input.theme.as_deref())?;
    for w in &input.widgets {
        validate_widget_position(&w.position)?;
    }

    let existing = if let Some(did) = input.dashboard_id {
        Some(
            state
                .store
                .get_dashboard(tenant_id, did, false)
                .await?
                .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?,
        )
    } else {
        state
            .store
            .get_dashboard_by_slug(tenant_id, slug, false)
            .await?
    };

    let (dashboard_id, final_slug) = if let Some(detail) = existing {
        if detail.dashboard.status != DashboardStatus::Draft {
            return Err(AppError::Conflict("DASHBOARD_ALREADY_PUBLISHED"));
        }
        let updated = state
            .store
            .update_dashboard(
                tenant_id,
                detail.dashboard.id,
                UpdateDashboardRequest {
                    name: Some(String::from(name)),
                    slug: Some(String::from(slug)),
                    description: None,
                    filters: Some(input.filters),
                    theme: Some(theme),
                    canvas: None,
                },
            )
            .await?;
        for old_w in &detail.widgets {
            let _deleted = state.store.delete_widget(tenant_id, old_w.id).await?;
        }
        (updated.id, updated.slug)
    } else {
        let created = state
            .store
            .create_dashboard(
                tenant_id,
                actor_user_id,
                CreateDashboardRequest {
                    name: String::from(name),
                    slug: String::from(slug),
                    description: String::new(),
                    filters: input.filters,
                    theme,
                    canvas: CanvasConfig::default(),
                    zone_layout: None,
                },
            )
            .await?;
        (created.dashboard.id, created.dashboard.slug)
    };

    let widget_count =
        replace_dashboard_draft_widgets(state, tenant_id, dashboard_id, input.widgets).await?;
    let deep_links = resolve_deep_links(
        web_base_url,
        state.axagent.public_host.as_deref(),
        "main",
        None,
        None,
        Some(dashboard_id),
    );
    Ok(UpsertDashboardDraftResponse {
        dashboard_id,
        slug: final_slug,
        status: String::from("DRAFT"),
        widget_count,
        deep_links,
    })
}

#[derive(Clone)]
struct SandboxLinkPending {
    link_type: String,
    direction: LinkDirection,
    src_type: String,
    src_key: String,
    target_type: String,
    target_key: String,
    weight: f64,
    create_missing_target: bool,
}

struct SandboxMappedNode {
    type_name: String,
    node_key: String,
    title: String,
    attributes: serde_json::Map<String, serde_json::Value>,
    metrics: BTreeMap<String, f64>,
    geo: Option<GeoPoint>,
    dataset_id: Option<Uuid>,
}

async fn resolve_sandbox_schema(
    state: &AppState,
    tenant_id: &str,
    subgraph_slug: &str,
    draft_id: Option<Uuid>,
) -> Result<
    (
        axpoint_domain::studio::SubgraphInfo,
        SchemaDoc,
        Option<Uuid>,
    ),
    AppError,
> {
    let subgraphs = state.store.list_subgraphs(tenant_id).await?;
    let sg = subgraphs
        .into_iter()
        .find(|s| s.slug == subgraph_slug)
        .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;
    if sg.is_main || !sg.is_dev_sandbox {
        return Err(AppError::bad(
            "DEV_SANDBOX_REQUIRED",
            "target subgraph must be a non-main dev sandbox",
        ));
    }

    if let Some(did) = draft_id {
        let draft = state
            .store
            .get_draft(tenant_id, did)
            .await?
            .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
        if draft.subgraph_slug != subgraph_slug {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "draft_id does not belong to target subgraph",
            ));
        }
        return Ok((sg, draft.schema_doc, Some(did)));
    }

    let drafts = state
        .store
        .list_drafts(tenant_id, subgraph_slug, Some(DraftStatus::Draft))
        .await?;
    if let Some(latest) = drafts.into_iter().next() {
        return Ok((sg, latest.schema_doc, Some(latest.id)));
    }

    let published = state
        .store
        .get_published_schema(tenant_id, subgraph_slug)
        .await?;
    Ok((sg, published.doc, None))
}

async fn load_sandbox_source_rows(
    state: &AppState,
    tenant_id: &str,
    source_template_id: Option<&str>,
    dataset_name: &str,
    limit: usize,
) -> (
    Option<Uuid>,
    Vec<serde_json::Map<String, serde_json::Value>>,
) {
    let ds_opt = state
        .store
        .get_dataset_by_name(tenant_id, dataset_name)
        .await
        .ok()
        .flatten();
    let ds_id = ds_opt.as_ref().map(|d| d.id);

    if let Ok((session, _)) = acquire_tenant_session(state, tenant_id, false).await {
        let escaped = dataset_name.replace('"', "\"\"");
        let sql = format!("SELECT * FROM \"{escaped}\" LIMIT {limit}");
        if let Ok(Ok(qr)) =
            tokio::task::spawn_blocking(move || session.run_internal(&sql, limit)).await
            && !qr.rows.is_empty()
        {
            return (ds_id, qr.rows);
        }
    }

    if let Some(pack_id) = source_template_id
        && let Some(csv_rows) = axpoint_server::ops::templates::read_template_pack_csv_rows(
            pack_id,
            dataset_name,
            limit,
        )
    {
        return (ds_id, csv_rows);
    }

    (ds_id, Vec::new())
}

fn lookup_cell_ci<'a>(
    row: &'a serde_json::Map<String, serde_json::Value>,
    col: &str,
) -> Option<&'a serde_json::Value> {
    row.get(col).or_else(|| {
        row.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(col))
            .map(|(_, v)| v)
    })
}

fn cell_to_string(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => {
            let t = s.trim();
            (!t.is_empty()).then(|| String::from(t))
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}

fn cell_to_f64(val: &serde_json::Value) -> Option<f64> {
    match val {
        serde_json::Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn render_sandbox_title(
    spec: &MappingSpec,
    nt: Option<&NodeTypeSpec>,
    node_key: &str,
    row: &serde_json::Map<String, serde_json::Value>,
    attrs: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let tpl = if !spec.title.trim().is_empty() {
        spec.title.trim()
    } else if let Some(n) = nt
        && !n.title_template.trim().is_empty()
    {
        n.title_template.trim()
    } else {
        return String::from(node_key);
    };

    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        if let Some(prefix) = rest.get(..open) {
            out.push_str(prefix);
        }
        let after_open = rest.get(open.saturating_add(1)..).unwrap_or("");
        if let Some(close) = after_open.find('}') {
            let token = after_open.get(..close).unwrap_or("").trim();
            let resolved = if token == "key" {
                Some(String::from(node_key))
            } else if let Some(prop_name) = token.strip_prefix("prop.") {
                attrs.get(prop_name).and_then(cell_to_string)
            } else {
                lookup_cell_ci(row, token)
                    .and_then(cell_to_string)
                    .or_else(|| attrs.get(token).and_then(cell_to_string))
            };
            out.push_str(resolved.as_deref().unwrap_or(node_key));
            rest = after_open.get(close.saturating_add(1)..).unwrap_or("");
        } else {
            out.push('{');
            out.push_str(after_open);
            rest = "";
        }
    }
    out.push_str(rest);
    let trimmed = out.trim();
    if trimmed.is_empty() {
        String::from(node_key)
    } else {
        String::from(trimmed)
    }
}

fn coerce_sandbox_row(
    row: &serde_json::Map<String, serde_json::Value>,
    spec: &MappingSpec,
    nt: Option<&NodeTypeSpec>,
    edge_weights: &BTreeMap<String, f64>,
    dataset_id: Option<Uuid>,
) -> Result<(SandboxMappedNode, Vec<SandboxLinkPending>), String> {
    let key_col = spec.key_column.trim();
    let node_key = lookup_cell_ci(row, key_col)
        .and_then(cell_to_string)
        .ok_or_else(|| format!("missing or empty key_column '{key_col}'"))?;

    let mut attributes = serde_json::Map::new();
    for (attr_name, col_name) in &spec.attributes {
        if let Some(val) = lookup_cell_ci(row, col_name)
            && !val.is_null()
        {
            attributes.insert(attr_name.clone(), val.clone());
        }
    }

    let mut metrics = BTreeMap::new();
    for (m_name, col_name) in &spec.metrics {
        if let Some(val) = lookup_cell_ci(row, col_name)
            && !val.is_null()
        {
            let num = cell_to_f64(val)
                .ok_or_else(|| format!("invalid numeric metric '{m_name}' in '{col_name}'"))?;
            metrics.insert(m_name.clone(), num);
        }
    }

    let geo = match (
        spec.geo.lng_column.as_deref().map(str::trim),
        spec.geo.lat_column.as_deref().map(str::trim),
    ) {
        (Some(lng_c), Some(lat_c)) if !lng_c.is_empty() && !lat_c.is_empty() => {
            match (lookup_cell_ci(row, lng_c), lookup_cell_ci(row, lat_c)) {
                (Some(vl), Some(vt)) if !vl.is_null() && !vt.is_null() => {
                    let lng = cell_to_f64(vl).ok_or_else(|| String::from("invalid geo lng"))?;
                    let lat = cell_to_f64(vt).ok_or_else(|| String::from("invalid geo lat"))?;
                    let pt = GeoPoint { lng, lat };
                    pt.validate().map_err(|e| e.client_message())?;
                    Some(pt)
                }
                _ => None,
            }
        }
        _ => None,
    };

    let links =
        extract_sandbox_row_links(row, &spec.node_type, &node_key, &spec.links, edge_weights);
    let title = render_sandbox_title(spec, nt, &node_key, row, &attributes);

    Ok((
        SandboxMappedNode {
            type_name: spec.node_type.clone(),
            node_key,
            title,
            attributes,
            metrics,
            geo,
            dataset_id,
        },
        links,
    ))
}

fn extract_sandbox_row_links(
    row: &serde_json::Map<String, serde_json::Value>,
    src_type: &str,
    src_key: &str,
    specs: &[MappingLinkSpec],
    edge_weights: &BTreeMap<String, f64>,
) -> Vec<SandboxLinkPending> {
    let mut out = Vec::new();
    for ls in specs {
        let Some(target_key) =
            lookup_cell_ci(row, ls.target_key_column.trim()).and_then(cell_to_string)
        else {
            continue;
        };
        let weight = ls
            .weight_column
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|wc| lookup_cell_ci(row, wc))
            .and_then(cell_to_f64)
            .or_else(|| edge_weights.get(&ls.link_type).copied())
            .unwrap_or(0.5);
        out.push(SandboxLinkPending {
            link_type: ls.link_type.clone(),
            direction: ls.direction,
            src_type: String::from(src_type),
            src_key: String::from(src_key),
            target_type: ls.target_type.clone(),
            target_key,
            weight,
            create_missing_target: ls.create_missing_target,
        });
    }
    out
}

async fn dry_run_dev_subgraph_impl(
    state: &AppState,
    tenant_id: &str,
    subgraph_slug: &str,
    req: DevSubgraphRunRequest,
    web_base_url: Option<&str>,
) -> Result<DevSubgraphRunResponse, AppError> {
    let (sg, schema_doc, resolved_draft_id) =
        resolve_sandbox_schema(state, tenant_id, subgraph_slug, req.draft_id).await?;
    let limit_rows = usize::try_from(req.limit_rows.clamp(1, 5_000)).unwrap_or(200);

    let nt_map: BTreeMap<&str, &NodeTypeSpec> = schema_doc
        .node_types
        .iter()
        .map(|n| (n.name.as_str(), n))
        .collect();
    let edge_weights: BTreeMap<String, f64> = schema_doc
        .edge_types
        .iter()
        .map(|e| (e.name.clone(), e.impact_weight))
        .collect();

    let mut mapped_nodes = Vec::new();
    let mut pending_links = Vec::new();
    let mut reject_samples = Vec::new();

    for mapping in &schema_doc.mappings {
        let (ds_id, rows) = load_sandbox_source_rows(
            state,
            tenant_id,
            sg.source_template_id.as_deref(),
            &mapping.dataset,
            limit_rows,
        )
        .await;
        let nt_opt = nt_map.get(mapping.node_type.as_str()).copied();
        for row in rows {
            match coerce_sandbox_row(&row, mapping, nt_opt, &edge_weights, ds_id) {
                Ok((node, links)) => {
                    mapped_nodes.push(node);
                    pending_links.extend(links);
                }
                Err(reason) => {
                    if reject_samples.len() < 20 {
                        reject_samples.push(MappingRejectSample {
                            row: serde_json::Value::Object(row),
                            reason,
                        });
                    }
                }
            }
        }
    }

    let (nodes_upserted, links_upserted, points_computed) = persist_and_evaluate_sandbox_tx(
        state,
        tenant_id,
        subgraph_slug,
        &schema_doc,
        &mapped_nodes,
        &pending_links,
    )
    .await?;

    let sample_page = state
        .store
        .list_nodes(
            tenant_id,
            NodeListQuery {
                subgraph: Some(String::from(subgraph_slug)),
                limit: Some(10),
                ..NodeListQuery::default()
            },
        )
        .await?;

    let deep_links = resolve_deep_links(
        web_base_url,
        state.axagent.public_host.as_deref(),
        subgraph_slug,
        resolved_draft_id,
        None,
        None,
    );
    Ok(DevSubgraphRunResponse {
        subgraph_slug: String::from(subgraph_slug),
        nodes_upserted,
        links_upserted,
        points_computed,
        sample_nodes: sample_page.nodes,
        reject_samples,
        deep_links,
    })
}

async fn persist_and_evaluate_sandbox_tx(
    state: &AppState,
    tenant_id: &str,
    subgraph_slug: &str,
    schema_doc: &SchemaDoc,
    mapped_nodes: &[SandboxMappedNode],
    pending_links: &[SandboxLinkPending],
) -> Result<(u64, u64, u64), AppError> {
    let mut tx = axpoint_db::begin_tenant_tx(state.store.pool(), tenant_id).await?;
    let now = Utc::now();
    let mut id_by_key: BTreeMap<(String, String), Uuid> = BTreeMap::new();
    let mut nodes_upserted = 0_u64;
    let mut points_computed = 0_u64;

    for n in mapped_nodes {
        let props_val = serde_json::Value::Object(n.attributes.clone());
        let metrics_val = serde_json::to_value(&n.metrics)
            .map_err(|e| AppError::Database(format!("serialize sandbox metrics: {e}")))?;
        let row = sqlx::query(
            "INSERT INTO xb_nodes (
                 tenant_id, subgraph_slug, type_name, node_key, title,
                 properties, metrics, geo_lng, geo_lat, source_dataset_id, source_row_key
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $4)
             ON CONFLICT (tenant_id, subgraph_slug, type_name, node_key) DO UPDATE SET
                 title = EXCLUDED.title,
                 properties = EXCLUDED.properties,
                 metrics = EXCLUDED.metrics,
                 geo_lng = COALESCE(EXCLUDED.geo_lng, xb_nodes.geo_lng),
                 geo_lat = COALESCE(EXCLUDED.geo_lat, xb_nodes.geo_lat),
                 source_dataset_id = COALESCE(EXCLUDED.source_dataset_id, xb_nodes.source_dataset_id),
                 row_version = xb_nodes.row_version + 1,
                 updated_at = NOW()
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(&n.type_name)
        .bind(&n.node_key)
        .bind(&n.title)
        .bind(props_val)
        .bind(metrics_val)
        .bind(n.geo.map(|g| g.lng))
        .bind(n.geo.map(|g| g.lat))
        .bind(n.dataset_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("upsert sandbox xb_nodes failed: {e}")))?;

        let nid: Uuid = row.get("id");
        id_by_key.insert((n.type_name.clone(), n.node_key.clone()), nid);
        nodes_upserted = nodes_upserted.saturating_add(1);

        for (m_name, &m_val) in &n.metrics {
            insert_sandbox_metric_point_tx(&mut tx, tenant_id, nid, m_name, now, m_val, "SYNC")
                .await?;
        }
    }

    let (extra_nodes, links_upserted) = upsert_sandbox_links_tx(
        &mut tx,
        tenant_id,
        subgraph_slug,
        pending_links,
        &mut id_by_key,
    )
    .await?;
    nodes_upserted = nodes_upserted.saturating_add(extra_nodes);

    points_computed = points_computed.saturating_add(
        evaluate_sandbox_rules_tx(&mut tx, tenant_id, subgraph_slug, schema_doc, now).await?,
    );

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit sandbox dry-run tx failed: {e}")))?;
    Ok((nodes_upserted, links_upserted, points_computed))
}

fn is_valid_sandbox_series_metric(metric: &str) -> bool {
    !metric.is_empty()
        && metric.len() <= 63
        && metric
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        && metric
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

async fn insert_sandbox_metric_point_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    node_id: Uuid,
    metric: &str,
    ts: chrono::DateTime<Utc>,
    val: f64,
    tag: &str,
) -> Result<(), AppError> {
    if !is_valid_sandbox_series_metric(metric) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
             value = EXCLUDED.value",
    )
    .bind(tenant_id)
    .bind(node_id)
    .bind(metric)
    .bind(ts)
    .bind(val)
    .bind(tag)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("insert sandbox xb_metric_series failed: {e}")))?;
    Ok(())
}

async fn upsert_sandbox_links_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    pending_links: &[SandboxLinkPending],
    id_by_key: &mut BTreeMap<(String, String), Uuid>,
) -> Result<(u64, u64), AppError> {
    let existing_rows = sqlx::query(
        "SELECT id, type_name, node_key FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("load sandbox node keys failed: {e}")))?;
    for r in existing_rows {
        id_by_key.insert((r.get("type_name"), r.get("node_key")), r.get("id"));
    }

    let mut extra_nodes = 0_u64;
    let mut links_upserted = 0_u64;

    for l in pending_links {
        let Some(&origin_id) = id_by_key.get(&(l.src_type.clone(), l.src_key.clone())) else {
            continue;
        };
        let target_pair = (l.target_type.clone(), l.target_key.clone());
        let target_id = if let Some(&tid) = id_by_key.get(&target_pair) {
            tid
        } else if l.create_missing_target {
            let row = sqlx::query(
                "INSERT INTO xb_nodes (tenant_id, subgraph_slug, type_name, node_key, title)
                 VALUES ($1, $2, $3, $4, $4)
                 ON CONFLICT (tenant_id, subgraph_slug, type_name, node_key) DO UPDATE SET
                     updated_at = NOW()
                 RETURNING id",
            )
            .bind(tenant_id)
            .bind(subgraph_slug)
            .bind(&l.target_type)
            .bind(&l.target_key)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| AppError::Database(format!("insert placeholder sandbox node: {e}")))?;
            let tid: Uuid = row.get("id");
            id_by_key.insert(target_pair, tid);
            extra_nodes = extra_nodes.saturating_add(1);
            tid
        } else {
            continue;
        };

        let (src_id, dst_id) = match l.direction {
            LinkDirection::Out => (origin_id, target_id),
            LinkDirection::In => (target_id, origin_id),
        };
        sqlx::query(
            "INSERT INTO xb_links (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id, weight)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id) DO UPDATE SET
                 weight = EXCLUDED.weight,
                 updated_at = NOW()",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(&l.link_type)
        .bind(src_id)
        .bind(dst_id)
        .bind(l.weight)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("upsert sandbox xb_links failed: {e}")))?;
        links_upserted = links_upserted.saturating_add(1);
    }

    Ok((extra_nodes, links_upserted))
}

struct SandboxMemNode {
    id: Uuid,
    type_name: String,
    props: serde_json::Map<String, serde_json::Value>,
    metrics: BTreeMap<String, f64>,
}

struct SandboxMemLink {
    link_type: String,
    src_id: Uuid,
    dst_id: Uuid,
    weight: f64,
}

async fn evaluate_sandbox_rules_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    schema_doc: &SchemaDoc,
    ts: chrono::DateTime<Utc>,
) -> Result<u64, AppError> {
    let layers = MetricDependencyPlan::from_schema(schema_doc).layers();
    if layers.is_empty() {
        return Ok(0);
    }

    let node_rows = sqlx::query(
        "SELECT id, type_name, properties, metrics
           FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = $2",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("load sandbox nodes for rules: {e}")))?;

    let mut nodes: BTreeMap<Uuid, SandboxMemNode> = BTreeMap::new();
    for r in node_rows {
        let id: Uuid = r.get("id");
        let props = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let metrics: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();
        nodes.insert(
            id,
            SandboxMemNode {
                id,
                type_name: r.get("type_name"),
                props,
                metrics,
            },
        );
    }

    let link_rows = sqlx::query(
        "SELECT link_type, src_node_id, dst_node_id, weight
           FROM xb_links
          WHERE tenant_id = $1 AND subgraph_slug = $2",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("load sandbox links for rules: {e}")))?;
    let links: Vec<SandboxMemLink> = link_rows
        .into_iter()
        .map(|r| SandboxMemLink {
            link_type: r.get("link_type"),
            src_id: r.get("src_node_id"),
            dst_id: r.get("dst_node_id"),
            weight: r.get("weight"),
        })
        .collect();

    let mut computed_points = 0_u64;
    for layer in &layers {
        for step in layer {
            computed_points = computed_points.saturating_add(
                apply_sandbox_rule_step_tx(tx, tenant_id, step, &mut nodes, &links, ts).await?,
            );
        }
    }
    Ok(computed_points)
}

async fn apply_sandbox_rule_step_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: &str,
    step: &MetricRuleStep,
    nodes: &mut BTreeMap<Uuid, SandboxMemNode>,
    links: &[SandboxMemLink],
    ts: chrono::DateTime<Utc>,
) -> Result<u64, AppError> {
    let target_v = step.target_vertex();
    let target_ids: Vec<Uuid> = nodes
        .values()
        .filter(|n| n.type_name == target_v.type_name)
        .map(|n| n.id)
        .collect();

    let mut updates: Vec<(Uuid, f64)> = Vec::new();
    match step {
        MetricRuleStep::Derived { expression, .. } => {
            let Ok(ast) = parse_expr(expression) else {
                return Ok(0);
            };
            for nid in target_ids {
                if let Some(n) = nodes.get(&nid) {
                    let env = EvalEnv::from_node(&n.metrics, &n.props);
                    if let ExprValue::Number(v) = eval_expr(&ast, &env).value
                        && v.is_finite()
                    {
                        updates.push((nid, v));
                    }
                }
            }
        }
        MetricRuleStep::Aggregation { rule, .. } => {
            for nid in target_ids {
                if let Some(val) = eval_sandbox_agg_for_parent(nid, rule, nodes, links) {
                    updates.push((nid, val));
                }
            }
        }
    }

    let rule_tag = match step {
        MetricRuleStep::Derived { .. } => "DERIVED",
        MetricRuleStep::Aggregation { .. } => "AGGREGATION",
    };
    let mut written = 0_u64;
    for (nid, val) in updates {
        if let Some(n) = nodes.get_mut(&nid) {
            n.metrics.insert(target_v.metric.clone(), val);
            let m_json = serde_json::to_value(&n.metrics)
                .map_err(|e| AppError::Database(format!("serialize updated metrics: {e}")))?;
            sqlx::query("UPDATE xb_nodes SET metrics = $2, updated_at = NOW() WHERE id = $1")
                .bind(nid)
                .bind(m_json)
                .execute(&mut **tx)
                .await
                .map_err(|e| AppError::Database(format!("update sandbox computed metric: {e}")))?;
            insert_sandbox_metric_point_tx(tx, tenant_id, nid, &target_v.metric, ts, val, rule_tag)
                .await?;
            written = written.saturating_add(1);
        }
    }
    Ok(written)
}

fn eval_sandbox_agg_for_parent(
    parent_id: Uuid,
    rule: &AggRuleSpec,
    nodes: &BTreeMap<Uuid, SandboxMemNode>,
    links: &[SandboxMemLink],
) -> Option<f64> {
    let filter_ast = rule
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|f| parse_expr(f).ok());

    let mut vals: Vec<(f64, f64)> = Vec::new();
    let mut matched_children = 0_u32;
    for l in links {
        if l.link_type != rule.via_link_type {
            continue;
        }
        let child_id = match rule.direction {
            LinkDirection::Out if l.src_id == parent_id => l.dst_id,
            LinkDirection::In if l.dst_id == parent_id => l.src_id,
            _ => continue,
        };
        let Some(child) = nodes.get(&child_id) else {
            continue;
        };
        if child.type_name != rule.source_type_name {
            continue;
        }
        if let Some(ref ast) = filter_ast {
            let env = EvalEnv::from_node(&child.metrics, &child.props);
            if !matches!(eval_expr(ast, &env).value, ExprValue::Bool(true)) {
                continue;
            }
        }
        matched_children = matched_children.saturating_add(1);
        if let Some(&v) = child.metrics.get(&rule.source_metric)
            && v.is_finite()
        {
            vals.push((v, l.weight.max(0.0)));
        }
    }

    match rule.op {
        AggFn::Count => Some(f64::from(matched_children)),
        AggFn::Sum => Some(vals.iter().fold(0.0, |acc, &(v, w)| {
            acc + if rule.weight_by_link { v * w } else { v }
        })),
        AggFn::Avg if !vals.is_empty() => {
            if rule.weight_by_link {
                let w_sum: f64 = vals.iter().map(|&(_, w)| w).sum();
                (w_sum > 0.0).then(|| vals.iter().map(|&(v, w)| v * w).sum::<f64>() / w_sum)
            } else {
                let cnt = u32::try_from(vals.len()).ok()?;
                Some(vals.iter().map(|&(v, _)| v).sum::<f64>() / f64::from(cnt))
            }
        }
        AggFn::Min => vals.iter().map(|&(v, _)| v).reduce(f64::min),
        AggFn::Max => vals.iter().map(|&(v, _)| v).reduce(f64::max),
        AggFn::Last => vals.last().map(|&(v, _)| v),
        _ => None,
    }
}
