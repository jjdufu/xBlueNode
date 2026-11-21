//! Dependency-inversion adapters wiring `AxAgent` (`CapabilityPublisher`, `McpServerHub`,
//! `McpClientManager`, `AgentRuntime`, `EmbedService`) and `LlmRouter` into `axpoint-server`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use axpoint_agent::{
    AgentRuntime, AgentRuntimeDeps, BuiltinToolRegistry, CapabilityPublisher, EmbedService,
    McpClientManager, McpServerHub, PublishedAgentInvoker,
};
use axpoint_db::PgStore;
use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::analytics::{
    AnalyticsPrincipal, AskContext, AskRequest, MetricDef, MetricQuery, MetricQueryResult,
    QueryRequest, QueryResult, SavedQuery,
};
use axpoint_domain::enums::{Permission, TokenKind};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{
    AgentCapabilityPort, AnalyticsQueryPort as _, ConnectionRepository as _,
    FlowCredentialUnsealPort, LlmProviderStore, SimExecutionPort, StudioRepository as _,
};
use axpoint_domain::studio::{
    DomainObjectListQuery, DomainObjectRecord, NodeTypeSpec, PropertyKind, SchemaDoc,
};
use axpoint_domain::topology::BlueNode;
use axpoint_domain::{AppError, FusedHit};
use axpoint_llm::{EmbeddingConfig, LlmConfig, LlmRouter};
use axpoint_security::{AuditWriter, AuthPrincipal, CredentialKeyring, NetGuard};
use uuid::Uuid;

use crate::routes::analytics::{
    execute_ask_for_principal, execute_published_metric, execute_published_saved_query,
    load_tenant_catalog_and_live,
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

/// Assembled Step 09 `AxAgent` services shared across HTTP routes and background workers.
pub(crate) struct AxAgentServices {
    pub(crate) capability: Arc<ServerAgentCapability>,
    pub(crate) publisher: Arc<CapabilityPublisher<PgStore>>,
    pub(crate) mcp_hub: Arc<McpServerHub<PgStore>>,
    pub(crate) mcp_clients: Arc<McpClientManager<PgStore>>,
    pub(crate) runtime: Arc<AgentRuntime<PgStore>>,
    pub(crate) embed: Arc<EmbedService<PgStore>>,
}

/// Constructs the complete Step 09 [`AxAgentServices`] stack.
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

    let mcp_hub = Arc::new(McpServerHub::new(
        Arc::clone(&store_arc),
        cap_port,
        Arc::clone(&publisher),
        Some(audit.clone()),
    ));
    let embed = Arc::new(EmbedService::new(
        store_arc,
        Arc::clone(&runtime),
        Some(audit.clone()),
    ));

    Arc::new(AxAgentServices {
        capability,
        publisher,
        mcp_hub,
        mcp_clients,
        runtime,
        embed,
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
                        params: BTreeMap::new(),
                        max_rows: Some(u32::try_from(max_rows.clamp(1, 200)).unwrap_or(200)),
                        timeout_ms: None,
                        allow_live: false,
                        reveal_pii: false,
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
            filters: Vec::new(),
            drill_path: Vec::new(),
            dashboard_id: None,
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
