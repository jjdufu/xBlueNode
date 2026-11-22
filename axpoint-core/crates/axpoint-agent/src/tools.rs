//! Built-in `AxAgent` tool registry (`13` tools), caller permission intersection,
//! conditional `SimExecutionPort` visibility, and `16 KB` / `50`-row output truncation (`08-STEP09` §09.5.1).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::agent::{KnowledgeScope, SimRunToolInput};
use axpoint_domain::analytics::MetricQuery;
use axpoint_domain::enums::{
    AggFn, AlertStatus, Environment, MetricSourceTag, Permission, PiiCategory, Severity,
    TriggerType,
};
use axpoint_domain::flow::{
    ActionKind, ActionLogStatus, ActionSourceKind, NewActionLogInput, StartRunParams,
};
use axpoint_domain::graph::{
    MetricPointInput, MetricQueryGrain, NeighborQuery, NodeListQuery, NodeMetricSeriesQuery,
    NodeRef, NodeSearchQuery, TraversalDirection, WriteMetricsRequest,
};
use axpoint_domain::ports::{
    AgentCapabilityPort, FlowRepository, GraphQueryPort, GraphStorePort, MetricStorePort,
    RulesRepository, SimExecutionPort, StudioRepository,
};
use axpoint_domain::rules::AlertListQuery;
use axpoint_domain::studio::PropertyKind;
use axpoint_security::AuthPrincipal;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::model::ModelToolDef;
use crate::publish::to_analytics_principal;

/// Maximum serialized byte size of a single tool result returned to the model (`16 KB`).
pub const MAX_TOOL_OUTPUT_BYTES: usize = 16_384;

/// Maximum number of rows/items retained in a single tool result before truncation (`50`).
pub const MAX_TOOL_OUTPUT_ROWS: usize = 50;

/// Guidance message appended when a tool output is truncated (`08-STEP09` §09.5.1).
pub const TRUNCATION_HINT: &str = "请缩小过滤条件";

/// Canonical list of all `13` built-in `AxAgent` tool identifiers.
pub const ALL_BUILTIN_TOOL_NAMES: &[&str] = &[
    "search_knowledge",
    "ask_data",
    "run_sql",
    "query_metric",
    "get_node",
    "search_nodes",
    "graph_neighbors",
    "node_timeseries",
    "list_alerts",
    "ack_alert",
    "trigger_flow",
    "write_node_metric",
    "run_simulation",
];

/// Returns `true` if `name` is a recognized built-in `AxAgent` tool identifier (including `08-STEP09` §09.5.1 aliases).
#[must_use]
pub fn is_known_builtin_tool(name: &str) -> bool {
    ALL_BUILTIN_TOOL_NAMES.contains(&name)
        || matches!(name, "query_objects" | "node_metric_series" | "start_flow")
}

/// Returns the [`Permission`] required to see and invoke a built-in tool (`08-STEP09` §09.5.1).
#[must_use]
pub fn required_builtin_permission(name: &str) -> Option<Permission> {
    match name {
        "search_knowledge" | "ask_data" | "run_sql" | "query_metric" | "query_objects"
        | "get_node" | "search_nodes" | "graph_neighbors" | "node_timeseries"
        | "node_metric_series" | "list_alerts" => Some(Permission::DataRead),
        "ack_alert" => Some(Permission::AlertAck),
        "trigger_flow" | "start_flow" => Some(Permission::FlowRun),
        "write_node_metric" => Some(Permission::DataSync),
        "run_simulation" => Some(Permission::SimRun),
        _ => None,
    }
}

/// Truncates a structured tool result when row count exceeds [`MAX_TOOL_OUTPUT_ROWS`] (`50`)
/// or serialized JSON size exceeds [`MAX_TOOL_OUTPUT_BYTES`] (`16 KB`), returning `(json_text, truncated)`.
#[must_use]
pub fn truncate_tool_output(value: serde_json::Value) -> (String, bool) {
    let (row_adjusted, truncated, detected_rows) = apply_row_cap(value);
    let serialized = serde_json::to_string(&row_adjusted).unwrap_or_else(|_| String::from("{}"));

    if serialized.len() <= MAX_TOOL_OUTPUT_BYTES {
        return (serialized, truncated);
    }

    let total_bytes = serialized.len();
    let total_rows = detected_rows.unwrap_or(0);
    // Reserve ~256 bytes for the JSON envelope and JSON string escaping overhead.
    let mut char_budget = MAX_TOOL_OUTPUT_BYTES.saturating_sub(512);
    while char_budget > 0 {
        let preview = utf8_prefix(&serialized, char_budget);
        let candidate = serde_json::json!({
            "preview": preview,
            "truncated": true,
            "total_rows": total_rows,
            "total_bytes": total_bytes,
            "hint": TRUNCATION_HINT,
        });
        let encoded = serde_json::to_string(&candidate).unwrap_or_else(|_| String::from("{}"));
        if encoded.len() <= MAX_TOOL_OUTPUT_BYTES {
            return (encoded, true);
        }
        char_budget /= 2;
    }

    let fallback = serde_json::json!({
        "truncated": true,
        "total_rows": total_rows,
        "total_bytes": total_bytes,
        "hint": TRUNCATION_HINT,
    });
    (
        serde_json::to_string(&fallback).unwrap_or_else(|_| String::from("{}")),
        true,
    )
}

fn apply_row_cap(value: serde_json::Value) -> (serde_json::Value, bool, Option<usize>) {
    match value {
        serde_json::Value::Array(mut arr) => {
            let total = arr.len();
            if total > MAX_TOOL_OUTPUT_ROWS {
                arr.truncate(MAX_TOOL_OUTPUT_ROWS);
                return (
                    serde_json::json!({
                        "items": arr,
                        "truncated": true,
                        "total_rows": total,
                        "hint": TRUNCATION_HINT,
                    }),
                    true,
                    Some(total),
                );
            }
            (serde_json::Value::Array(arr), false, Some(total))
        }
        serde_json::Value::Object(mut obj) => {
            let list_keys = ["rows", "items", "hits", "points"];
            for key in list_keys {
                if let Some(serde_json::Value::Array(arr)) = obj.get_mut(key) {
                    let total = arr.len();
                    if total > MAX_TOOL_OUTPUT_ROWS {
                        arr.truncate(MAX_TOOL_OUTPUT_ROWS);
                        obj.insert(String::from("truncated"), serde_json::Value::Bool(true));
                        obj.insert(String::from("total_rows"), serde_json::json!(total));
                        obj.insert(
                            String::from("hint"),
                            serde_json::Value::String(String::from(TRUNCATION_HINT)),
                        );
                        return (serde_json::Value::Object(obj), true, Some(total));
                    }
                    return (serde_json::Value::Object(obj), false, Some(total));
                }
            }
            (serde_json::Value::Object(obj), false, None)
        }
        other => (other, false, None),
    }
}

fn utf8_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0_usize;
    for (idx, ch) in s.char_indices() {
        let next = idx.saturating_add(ch.len_utf8());
        if next > max_bytes {
            break;
        }
        end = next;
    }
    s.get(..end).unwrap_or("")
}

/// Registry of the `13` built-in `AxAgent` platform tools (`08-STEP09` §09.5.1).
pub struct BuiltinToolRegistry<R> {
    store: Arc<R>,
    capability: Arc<dyn AgentCapabilityPort>,
    sim_port: Option<Arc<dyn SimExecutionPort>>,
}

impl<R> BuiltinToolRegistry<R> {
    /// Creates a new [`BuiltinToolRegistry`].
    ///
    /// When `sim_port` is `None` (Step 09), `run_simulation` is hidden from the model tool list.
    #[must_use]
    pub fn new(
        store: Arc<R>,
        capability: Arc<dyn AgentCapabilityPort>,
        sim_port: Option<Arc<dyn SimExecutionPort>>,
    ) -> Self {
        Self {
            store,
            capability,
            sim_port,
        }
    }

    /// Returns `true` if `sim_port` is registered (`Step 10+`).
    #[must_use]
    pub fn has_simulation_port(&self) -> bool {
        self.sim_port.is_some()
    }

    /// Resolves a built-in tool's [`ModelToolDef`] if `name` is available in this deployment stage
    /// AND `principal` holds the required [`Permission`].
    #[must_use]
    pub fn resolve_visible_tool(
        &self,
        principal: &AuthPrincipal,
        name: &str,
    ) -> Option<ModelToolDef> {
        if name == "run_simulation" && self.sim_port.is_none() {
            return None;
        }
        let perm = required_builtin_permission(name)?;
        if !principal.has(perm) {
            return None;
        }
        builtin_tool_def(name)
    }

    /// Lists all built-in tools visible to `principal` at the current system stage.
    #[must_use]
    pub fn list_visible_tools(&self, principal: &AuthPrincipal) -> Vec<ModelToolDef> {
        ALL_BUILTIN_TOOL_NAMES
            .iter()
            .filter_map(|name| self.resolve_visible_tool(principal, name))
            .collect()
    }
}

impl<R> BuiltinToolRegistry<R>
where
    R: GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + FlowRepository
        + StudioRepository
        + 'static,
{
    /// Executes a built-in tool on behalf of `principal`, enforcing permission checks and stage availability.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, unsupported stage (`run_simulation` when `None`),
    /// invalid tool arguments, or downstream repository/capability errors.
    pub async fn execute_builtin(
        &self,
        principal: &AuthPrincipal,
        name: &str,
        args: &serde_json::Value,
        knowledge_scope: &KnowledgeScope,
        idempotency_prefix: &str,
    ) -> Result<serde_json::Value, AppError> {
        let perm = required_builtin_permission(name)
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("unknown tool '{name}'")))?;
        principal.require(perm)?;

        match name {
            "search_knowledge" => {
                self.exec_search_knowledge(principal, args, knowledge_scope)
                    .await
            }
            "ask_data" => self.exec_ask_data(principal, args, knowledge_scope).await,
            "run_sql" => self.exec_run_sql(principal, args).await,
            "query_metric" => self.exec_query_metric(principal, args).await,
            "query_objects" => self.exec_query_objects(principal, args).await,
            "get_node" => self.exec_get_node(principal, args).await,
            "search_nodes" => self.exec_search_nodes(principal, args).await,
            "graph_neighbors" => self.exec_graph_neighbors(principal, args).await,
            "node_timeseries" | "node_metric_series" => {
                self.exec_node_timeseries(principal, args).await
            }
            "list_alerts" => self.exec_list_alerts(principal, args).await,
            "ack_alert" => self.exec_ack_alert(principal, args).await,
            "trigger_flow" | "start_flow" => self.exec_trigger_flow(principal, args).await,
            "write_node_metric" => {
                self.exec_write_node_metric(principal, args, idempotency_prefix)
                    .await
            }
            "run_simulation" => self.exec_run_simulation(principal, args).await,
            _ => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown tool '{name}'"),
            )),
        }
    }

    async fn exec_query_objects(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let type_name = req_str(args, "type_name")?;
        let mut payload = args.clone();
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("type_name");
        }
        self.capability
            .query_domain_objects(
                &to_analytics_principal(principal),
                type_name,
                None,
                &payload,
            )
            .await
    }

    async fn exec_search_knowledge(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        scope: &KnowledgeScope,
    ) -> Result<serde_json::Value, AppError> {
        let query = req_str(args, "query")?;
        let top_k = opt_u64(args, "top_k")
            .and_then(|v| usize::try_from(v).ok())
            .map_or(6, |v| v.clamp(1, 20));
        let prefix = args
            .get("doc_title_prefix")
            .and_then(serde_json::Value::as_str)
            .or(scope.doc_title_prefix.as_deref());
        let hits = self
            .capability
            .search_knowledge(&principal.tenant_id, query, top_k, prefix)
            .await?;
        Ok(serde_json::json!({ "hits": hits, "count": hits.len() }))
    }

    async fn exec_ask_data(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        scope: &KnowledgeScope,
    ) -> Result<serde_json::Value, AppError> {
        let question = req_str(args, "question")?;
        let mut ds_names: Vec<String> = args
            .get("dataset_names")
            .and_then(serde_json::Value::as_array)
            .map_or_default(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(String::from)
                    .collect()
            });
        if ds_names.is_empty() && !scope.dataset_names.is_empty() {
            ds_names.clone_from(&scope.dataset_names);
        }
        self.capability
            .ask_data(&to_analytics_principal(principal), question, &ds_names)
            .await
    }

    async fn exec_run_sql(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let sql = req_str(args, "sql")?;
        let max_rows = opt_u64(args, "max_rows")
            .and_then(|v| usize::try_from(v).ok())
            .map_or(200, |v| v.clamp(1, 200));
        let res = self
            .capability
            .run_sql_bounded(&to_analytics_principal(principal), sql, max_rows)
            .await?;
        serde_json::to_value(res)
            .map_err(|e| AppError::Config(format!("serialize QueryResult: {e}")))
    }

    async fn exec_query_metric(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let metric_name = req_str(args, "metric_name")?;
        let mut query_val = args.get("query").cloned().unwrap_or_else(|| args.clone());
        if let Some(obj) = query_val.as_object_mut() {
            obj.remove("metric_name");
            obj.remove("query");
        }
        let mquery: MetricQuery = serde_json::from_value(query_val)
            .map_err(|e| AppError::bad("INVALID_ARGUMENT", format!("invalid MetricQuery: {e}")))?;
        let res = self
            .capability
            .run_metric_endpoint(
                &to_analytics_principal(principal),
                metric_name,
                None,
                mquery,
            )
            .await?;
        serde_json::to_value(res)
            .map_err(|e| AppError::Config(format!("serialize MetricQueryResult: {e}")))
    }

    async fn exec_get_node(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let mut node = if let Some(id_raw) = opt_str(args, "node_id") {
            let nid = req_uuid_from_str(id_raw, "node_id")?;
            self.store.get_node(tenant_id, nid).await?
        } else {
            let tname = req_str(args, "type_name")?;
            let nkey = req_str(args, "node_key")?;
            let page = self
                .store
                .list_nodes(
                    tenant_id,
                    NodeListQuery {
                        subgraph: None,
                        type_name: Some(String::from(tname)),
                        q: Some(String::from(nkey)),
                        bbox: None,
                        limit: Some(50),
                        cursor: None,
                        reveal_pii: false,
                    },
                )
                .await?;
            page.items.into_iter().find(|n| n.node_key == nkey)
        }
        .ok_or(AppError::NotFound("NOT_FOUND"))?;

        if !principal.has(Permission::PiiRawRead)
            && let Ok(published) = self.store.get_published_schema(tenant_id, "main").await
            && let Some(nt) = published
                .doc
                .node_types
                .iter()
                .find(|n| n.name == node.type_name)
        {
            for prop in &nt.properties {
                if prop.kind == PropertyKind::Attribute
                    && prop.pii_category != PiiCategory::None
                    && node.properties.contains_key(&prop.name)
                {
                    node.properties.insert(
                        prop.name.clone(),
                        serde_json::Value::String(String::from("***")),
                    );
                }
            }
        }
        serde_json::to_value(node).map_err(|e| AppError::Config(format!("serialize BlueNode: {e}")))
    }

    async fn exec_search_nodes(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let q = req_str(args, "q")?;
        let type_name = opt_str(args, "type_name").map(String::from);
        let k = opt_u64(args, "k")
            .and_then(|v| u32::try_from(v).ok())
            .map_or(20, |v| v.clamp(1, 50));
        let res = self
            .store
            .search_nodes(
                &principal.tenant_id,
                NodeSearchQuery {
                    q: String::from(q),
                    type_name,
                    k,
                },
                None,
            )
            .await?;
        serde_json::to_value(res)
            .map_err(|e| AppError::Config(format!("serialize NodeSearchResponse: {e}")))
    }

    async fn exec_graph_neighbors(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let node_id = req_uuid(args, "node_id")?;
        let depth = opt_u64(args, "depth")
            .and_then(|v| u32::try_from(v).ok())
            .map_or(1, |v| v.clamp(1, 4));
        let limit = opt_u64(args, "limit")
            .and_then(|v| u32::try_from(v).ok())
            .map_or(50, |v| v.clamp(1, 200));
        let direction = args
            .get("direction")
            .and_then(|v| serde_json::from_value::<TraversalDirection>(v.clone()).ok())
            .unwrap_or_default();
        let link_types = opt_str(args, "link_types").map(String::from);
        let sub = self
            .store
            .query_neighbors(
                &principal.tenant_id,
                NeighborQuery {
                    node_id,
                    depth,
                    direction,
                    link_types,
                    limit,
                },
            )
            .await?;
        serde_json::to_value(sub)
            .map_err(|e| AppError::Config(format!("serialize SubgraphResponse: {e}")))
    }

    async fn exec_node_timeseries(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let node_id = req_uuid(args, "node_id")?;
        let metric = req_str(args, "metric")?;
        let now = Utc::now();
        let to = parse_opt_dt(args, "to")?.unwrap_or(now);
        let from = parse_opt_dt(args, "from")?.unwrap_or_else(|| to - Duration::hours(24));
        let grain = args
            .get("grain")
            .and_then(|v| serde_json::from_value::<MetricQueryGrain>(v.clone()).ok());
        let agg = args
            .get("agg")
            .and_then(|v| serde_json::from_value::<AggFn>(v.clone()).ok())
            .unwrap_or(AggFn::Avg);

        let points = self
            .store
            .query_node_metrics(
                &principal.tenant_id,
                node_id,
                metric,
                NodeMetricSeriesQuery {
                    from,
                    to,
                    grain,
                    agg,
                },
            )
            .await?;
        Ok(serde_json::json!({ "points": points, "count": points.len() }))
    }

    async fn exec_list_alerts(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let status = args
            .get("status")
            .and_then(|v| serde_json::from_value::<AlertStatus>(v.clone()).ok());
        let severity = args
            .get("severity")
            .and_then(|v| serde_json::from_value::<Severity>(v.clone()).ok());
        let rule_id = opt_str(args, "rule_id")
            .map(|s| req_uuid_from_str(s, "rule_id"))
            .transpose()?;
        let node_id = opt_str(args, "node_id")
            .map(|s| req_uuid_from_str(s, "node_id"))
            .transpose()?;
        let type_name = opt_str(args, "type_name").map(String::from);
        let limit = opt_u64(args, "limit")
            .and_then(|v| u32::try_from(v).ok())
            .map_or(20, |v| v.clamp(1, 50));

        let page = self
            .store
            .list_alerts(
                &principal.tenant_id,
                AlertListQuery {
                    status,
                    severity,
                    rule_id,
                    node_id,
                    type_name,
                    limit: Some(limit),
                    cursor: None,
                },
            )
            .await?;
        serde_json::to_value(page)
            .map_err(|e| AppError::Config(format!("serialize alerts page: {e}")))
    }

    async fn exec_ack_alert(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let alert_id = req_uuid(args, "alert_id")?;
        let comment = opt_str(args, "comment").unwrap_or("");
        let actor = if principal.user_id.is_nil() {
            None
        } else {
            Some(principal.user_id)
        };
        let rec = self
            .store
            .ack_alert(&principal.tenant_id, alert_id, actor, comment)
            .await?;
        serde_json::to_value(rec)
            .map_err(|e| AppError::Config(format!("serialize AlertRecord: {e}")))
    }

    async fn exec_trigger_flow(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let flow_id = req_uuid(args, "flow_id")?;
        let env = match opt_str(args, "environment") {
            Some("TEST" | "test") => Environment::Test,
            Some("PROD" | "prod") | None => Environment::Prod,
            Some(other) => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid environment '{other}'"),
                ));
            }
        };
        let dep = self
            .store
            .get_flow_deployment(&principal.tenant_id, flow_id, env)
            .await?
            .ok_or(AppError::Forbidden("PERMISSION_DENIED"))?;

        let payload = args
            .get("payload")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let idem = opt_str(args, "idempotency_key").map(String::from);
        let started_by = if principal.user_id.is_nil() {
            None
        } else {
            Some(principal.user_id)
        };

        let (run, existing) = self
            .store
            .start_flow_run(
                &principal.tenant_id,
                StartRunParams {
                    flow_id,
                    environment: env,
                    explicit_version: Some(dep.version),
                    trigger_source: TriggerType::Api,
                    trigger_payload: payload,
                    idempotency_key: idem,
                    started_by,
                },
            )
            .await?;
        Ok(serde_json::json!({
            "run_id": run.id,
            "status": run.status,
            "version": run.version,
            "existing": existing,
        }))
    }

    async fn exec_write_node_metric(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        idempotency_prefix: &str,
    ) -> Result<serde_json::Value, AppError> {
        let metric = req_str(args, "metric")?;
        let value = args
            .get("value")
            .and_then(serde_json::Value::as_f64)
            .filter(|v| v.is_finite())
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing finite numeric 'value'"))?;
        let ts = parse_opt_dt(args, "ts")?.unwrap_or_else(Utc::now);

        let (node_id, node_ref, target_desc) = if let Some(id_str) = opt_str(args, "node_id") {
            let nid = req_uuid_from_str(id_str, "node_id")?;
            (Some(nid), None, nid.to_string())
        } else {
            let tname = req_str(args, "type_name")?;
            let nkey = req_str(args, "node_key")?;
            (
                None,
                Some(NodeRef {
                    type_name: String::from(tname),
                    node_key: String::from(nkey),
                }),
                format!("{tname}:{nkey}"),
            )
        };

        let write_res = self
            .store
            .write_metrics(
                &principal.tenant_id,
                WriteMetricsRequest {
                    points: vec![MetricPointInput {
                        node_id,
                        node: node_ref,
                        metric: String::from(metric),
                        ts,
                        value: Some(value),
                        value_text: None,
                    }],
                    source_tag: MetricSourceTag::Action,
                    batch_id: None,
                },
            )
            .await?;

        let _ = self
            .store
            .record_action_log(
                &principal.tenant_id,
                NewActionLogInput {
                    run_id: None,
                    node_id: None,
                    source_kind: ActionSourceKind::Agent,
                    action_kind: ActionKind::WriteNodeMetric,
                    target: target_desc.clone(),
                    payload: serde_json::json!({ "metric": metric, "value": value, "ts": ts }),
                    status: ActionLogStatus::Done,
                    idempotency_key: format!("agent:{idempotency_prefix}:{target_desc}:{metric}"),
                    error_message: None,
                },
            )
            .await?;

        Ok(serde_json::json!({
            "written": write_res.written,
            "target": target_desc,
            "metric": metric,
            "value": value,
        }))
    }

    async fn exec_run_simulation(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let Some(ref port) = self.sim_port else {
            return Err(AppError::Unsupported("UNSUPPORTED_FORMAT"));
        };
        let input: SimRunToolInput = serde_json::from_value(args.clone()).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid SimRunToolInput: {err}"),
            )
        })?;
        let out = port
            .run_simulation_tool(&principal.tenant_id, principal.user_id, input)
            .await?;
        serde_json::to_value(out)
            .map_err(|err| AppError::Config(format!("serialize SimRunToolOutput: {err}")))
    }
}

fn req_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, AppError> {
    opt_str(args, key)
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("missing non-empty '{key}'")))
}

fn opt_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn opt_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    args.get(key).and_then(serde_json::Value::as_u64)
}

fn req_uuid(args: &serde_json::Value, key: &str) -> Result<Uuid, AppError> {
    let raw = req_str(args, key)?;
    req_uuid_from_str(raw, key)
}

fn req_uuid_from_str(raw: &str, key: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw.trim())
        .map_err(|e| AppError::bad("INVALID_ARGUMENT", format!("invalid UUID in '{key}': {e}")))
}

fn parse_opt_dt(args: &serde_json::Value, key: &str) -> Result<Option<DateTime<Utc>>, AppError> {
    let Some(raw) = opt_str(args, key) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| Some(dt.with_timezone(&Utc)))
        .map_err(|e| AppError::bad("INVALID_ARGUMENT", format!("invalid RFC3339 '{key}': {e}")))
}

fn builtin_tool_def(name: &str) -> Option<ModelToolDef> {
    builtin_data_tool_def(name)
        .or_else(|| builtin_graph_tool_def(name))
        .or_else(|| builtin_action_tool_def(name))
}

fn builtin_data_tool_def(name: &str) -> Option<ModelToolDef> {
    let (desc, params) = match name {
        "search_knowledge" => (
            "检索企业超图知识库段落、文档与关联实体",
            serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "minLength": 1 },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 20 },
                    "doc_title_prefix": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        "ask_data" => (
            "使用自然语言对数据湖数据集发起智能问数（NL2SQL）",
            serde_json::json!({
                "type": "object",
                "required": ["question"],
                "properties": {
                    "question": { "type": "string", "minLength": 1 },
                    "dataset_names": { "type": "array", "items": { "type": "string" } }
                },
                "additionalProperties": false
            }),
        ),
        "run_sql" => (
            "执行经 sql_guard 校验与脱敏的只读 DuckDB SQL（最多返回 200 行）",
            serde_json::json!({
                "type": "object",
                "required": ["sql"],
                "properties": {
                    "sql": { "type": "string", "minLength": 1 },
                    "max_rows": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "additionalProperties": false
            }),
        ),
        "query_metric" => (
            "按维度、时间粒度与同比/环比查询指标语义层认证指标",
            serde_json::json!({
                "type": "object",
                "required": ["metric_name"],
                "properties": {
                    "metric_name": { "type": "string", "minLength": 1 },
                    "dimensions": { "type": "array", "items": { "type": "string" } },
                    "grain": { "type": "string", "enum": ["MINUTE", "HOUR", "DAY", "MONTH"] },
                    "compare": { "type": "string", "enum": ["PREVIOUS_PERIOD", "SAME_PERIOD_LAST_YEAR"] }
                },
                "additionalProperties": false
            }),
        ),
        "query_objects" => (
            "按主图已发布对象类型、属性与指标过滤查询业务对象及 1 跳关联",
            serde_json::json!({
                "type": "object",
                "required": ["type_name"],
                "properties": {
                    "type_name": { "type": "string", "minLength": 1 },
                    "q": { "type": "string" },
                    "keys": { "type": "array", "items": { "type": "string" } },
                    "properties": { "type": "object" },
                    "metrics_min": { "type": "object" },
                    "include_links": { "type": "array", "items": { "type": "string" } },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "additionalProperties": false
            }),
        ),
        _ => return None,
    };

    Some(ModelToolDef {
        name: String::from(name),
        description: String::from(desc),
        parameters: params,
    })
}

fn builtin_graph_tool_def(name: &str) -> Option<ModelToolDef> {
    let (desc, params) = match name {
        "get_node" => (
            "按 node_id 或 (type_name, node_key) 获取单个业务图谱节点属性与最新指标",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "node_id": { "type": "string", "format": "uuid" },
                    "type_name": { "type": "string" },
                    "node_key": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        "search_nodes" => (
            "混合检索（全文 + 向量 RRF）业务图谱节点",
            serde_json::json!({
                "type": "object",
                "required": ["q"],
                "properties": {
                    "q": { "type": "string", "minLength": 1 },
                    "type_name": { "type": "string" },
                    "k": { "type": "integer", "minimum": 1, "maximum": 50 }
                },
                "additionalProperties": false
            }),
        ),
        "graph_neighbors" => (
            "查询指定业务节点的 k 跳（1..=4）上下游关联子图",
            serde_json::json!({
                "type": "object",
                "required": ["node_id"],
                "properties": {
                    "node_id": { "type": "string", "format": "uuid" },
                    "depth": { "type": "integer", "minimum": 1, "maximum": 4 },
                    "direction": { "type": "string", "enum": ["OUT", "IN", "BOTH"] },
                    "link_types": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "additionalProperties": false
            }),
        ),
        "node_timeseries" | "node_metric_series" => (
            "查询业务节点指定指标的历史时序数据",
            serde_json::json!({
                "type": "object",
                "required": ["node_id", "metric"],
                "properties": {
                    "node_id": { "type": "string", "format": "uuid" },
                    "metric": { "type": "string", "minLength": 1 },
                    "from": { "type": "string", "format": "date-time" },
                    "to": { "type": "string", "format": "date-time" },
                    "grain": { "type": "string", "enum": ["RAW", "MINUTE", "HOUR", "DAY", "MONTH"] },
                    "agg": { "type": "string", "enum": ["SUM", "AVG", "COUNT", "COUNT_DISTINCT", "MIN", "MAX", "MEDIAN", "LAST"] }
                },
                "additionalProperties": false
            }),
        ),
        _ => return None,
    };

    Some(ModelToolDef {
        name: String::from(name),
        description: String::from(desc),
        parameters: params,
    })
}

fn builtin_action_tool_def(name: &str) -> Option<ModelToolDef> {
    let (desc, params) = match name {
        "list_alerts" => (
            "查询当前租户的运营告警列表（支持按状态、级别、节点过滤）",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["OPEN", "ACK", "RESOLVED"] },
                    "severity": { "type": "string", "enum": ["INFO", "WARN", "CRITICAL"] },
                    "rule_id": { "type": "string", "format": "uuid" },
                    "node_id": { "type": "string", "format": "uuid" },
                    "type_name": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                },
                "additionalProperties": false
            }),
        ),
        "ack_alert" => (
            "确认一条 OPEN 状态的运营告警并记录处理说明",
            serde_json::json!({
                "type": "object",
                "required": ["alert_id"],
                "properties": {
                    "alert_id": { "type": "string", "format": "uuid" },
                    "comment": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        "trigger_flow" | "start_flow" => (
            "触发一个已部署到 TEST 或 PROD 环境的 AxFlow 自动化流程",
            serde_json::json!({
                "type": "object",
                "required": ["flow_id"],
                "properties": {
                    "flow_id": { "type": "string", "format": "uuid" },
                    "environment": { "type": "string", "enum": ["TEST", "PROD"] },
                    "payload": { "type": "object" },
                    "idempotency_key": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        "write_node_metric" => (
            "向指定业务节点回写一条数值型指标记录（ACTION 来源）",
            serde_json::json!({
                "type": "object",
                "required": ["metric", "value"],
                "properties": {
                    "node_id": { "type": "string", "format": "uuid" },
                    "type_name": { "type": "string" },
                    "node_key": { "type": "string" },
                    "metric": { "type": "string", "minLength": 1 },
                    "value": { "type": "number" },
                    "ts": { "type": "string", "format": "date-time" }
                },
                "additionalProperties": false
            }),
        ),
        "run_simulation" => (
            "在 AxSim 因果推演沙盘上执行干预仿真并返回传导影响摘要",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "sandbox_id": { "type": "string", "format": "uuid" },
                    "scenario_name": { "type": "string" },
                    "interventions": {},
                    "max_hops": { "type": "integer", "minimum": 1, "maximum": 10 }
                },
                "additionalProperties": true
            }),
        ),
        _ => return None,
    };

    Some(ModelToolDef {
        name: String::from(name),
        description: String::from(desc),
        parameters: params,
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_TOOL_OUTPUT_BYTES, TRUNCATION_HINT, truncate_tool_output};

    #[test]
    fn truncates_over_50_rows_and_over_16kb_payloads() {
        let rows: Vec<serde_json::Value> = (0..75_u32)
            .map(|i| serde_json::json!({ "idx": i, "val": "ok" }))
            .collect();
        let (out_rows, truncated_rows) = truncate_tool_output(serde_json::json!({ "rows": rows }));
        assert!(truncated_rows);
        let parsed: serde_json::Value = serde_json::from_str(&out_rows).unwrap_or_default();
        assert_eq!(
            parsed.get("truncated").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            parsed.get("total_rows").and_then(serde_json::Value::as_u64),
            Some(75)
        );
        assert_eq!(
            parsed.get("hint").and_then(serde_json::Value::as_str),
            Some(TRUNCATION_HINT)
        );
        assert_eq!(
            parsed
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(50)
        );

        let huge_text = "华东仓安全库存分析报告-".repeat(2_000);
        let (out_bytes, truncated_bytes) =
            truncate_tool_output(serde_json::json!({ "report": huge_text }));
        assert!(truncated_bytes);
        assert!(out_bytes.len() <= MAX_TOOL_OUTPUT_BYTES);
        let parsed_bytes: serde_json::Value = serde_json::from_str(&out_bytes).unwrap_or_default();
        assert_eq!(
            parsed_bytes
                .get("truncated")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }
}
