//! Streamable HTTP Model Context Protocol (`MCP`) server hub (`McpServerHub`, `08-STEP09` §09.4.1).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axpoint_domain::AppError;
use axpoint_domain::agent::{EndpointListQuery, EndpointSpec};
use axpoint_domain::axhub::DatasetListQuery;
use axpoint_domain::enums::{
    AuditAction, ConversationChannel, Environment, Permission, PiiCategory, TokenKind,
};
use axpoint_domain::graph::{NeighborQuery, NodeListQuery, TraversalDirection};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    AgentCapabilityPort, AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository,
    GraphQueryPort, GraphStorePort, StudioRepository,
};
use axpoint_domain::studio::PropertyKind;
use axpoint_security::{AuditWriter, AuthPrincipal};
use chrono::Utc;
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::publish::{
    CapabilityPublisher, to_analytics_principal, validate_instance_against_schema,
};

const MCP_IDLE_TTL_SECS: u64 = 1_800;
const DEFAULT_MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// HTTP-level response envelope returned by [`McpServerHub::handle_post`].
#[derive(Debug, Clone)]
pub struct McpHttpResponse {
    /// HTTP status code (`200` for JSON-RPC response, `202` for JSON-RPC notification).
    pub status_code: u16,
    /// Active or newly allocated `Mcp-Session-Id` header value.
    pub session_id: Option<String>,
    /// JSON-RPC 2.0 response payload (`None` when `status_code == 202`).
    pub body: Option<serde_json::Value>,
}

#[derive(Debug)]
struct McpSession {
    id: String,
    tenant_id: String,
    user_id: Uuid,
    sse_senders: Mutex<Vec<mpsc::Sender<serde_json::Value>>>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcIncoming {
    jsonrpc: String,
    #[serde(default)]
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct McpToolDescriptor {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct McpResourceDescriptor {
    uri: String,
    name: String,
    description: String,
    #[serde(rename = "mimeType")]
    mime_type: &'static str,
}

/// Streamable HTTP MCP Server dynamically exposing tenant `PROD` published endpoints (`expose_mcp = true`),
/// 7 built-in read-only platform tools, and `axpoint://` catalog resources (`08-STEP09` §09.4.1).
pub struct McpServerHub<R> {
    store: Arc<R>,
    capability: Arc<dyn AgentCapabilityPort>,
    publisher: Arc<CapabilityPublisher<R>>,
    audit: Option<AuditWriter>,
    sessions: Cache<String, Arc<McpSession>>,
}

impl<R> McpServerHub<R> {
    /// Creates a new [`McpServerHub`] with a 30-minute idle session cache.
    #[must_use]
    pub fn new(
        store: Arc<R>,
        capability: Arc<dyn AgentCapabilityPort>,
        publisher: Arc<CapabilityPublisher<R>>,
        audit: Option<AuditWriter>,
    ) -> Self {
        let sessions = Cache::builder()
            .max_capacity(8_192)
            .time_to_idle(Duration::from_secs(MCP_IDLE_TTL_SECS))
            .build();
        Self {
            store,
            capability,
            publisher,
            audit,
            sessions,
        }
    }
}

impl<R> McpServerHub<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + DatasetRepository
        + GraphStorePort
        + GraphQueryPort
        + 'static,
{
    /// Handles `POST /api/mcp` JSON-RPC 2.0 requests (`initialize`, `tools/list`, `tools/call`,
    /// `resources/list`, `resources/read`, `ping`, and `notifications/*`).
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid PAT credentials, missing/foreign `Mcp-Session-Id`, or rate limit exhaustion.
    pub async fn handle_post(
        &self,
        principal: &AuthPrincipal,
        session_header: Option<&str>,
        raw_body: serde_json::Value,
        client_ip: &str,
        trace_id: &str,
    ) -> Result<McpHttpResponse, AppError> {
        ensure_pat_principal(principal)?;

        let rpc: JsonRpcIncoming = serde_json::from_value(raw_body).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid JSON-RPC 2.0 body: {err}"),
            )
        })?;
        if rpc.jsonrpc != "2.0" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "jsonrpc version must equal '2.0'",
            ));
        }

        if rpc.method == "initialize" {
            return Ok(self.handle_initialize(principal, &rpc).await);
        }

        let session = self.require_session(principal, session_header).await?;
        if rpc.id.is_none() || rpc.method.starts_with("notifications/") {
            return Ok(McpHttpResponse {
                status_code: 202,
                session_id: Some(session.id.clone()),
                body: None,
            });
        }

        let req_id = rpc.id.clone().unwrap_or(serde_json::Value::Null);
        let body = match rpc.method.as_str() {
            "ping" => jsonrpc_ok(&req_id, &serde_json::json!({})),
            "tools/list" => {
                let tools = self.collect_visible_tools(principal).await?;
                jsonrpc_ok(&req_id, &serde_json::json!({ "tools": tools }))
            }
            "tools/call" => {
                self.dispatch_tools_call(principal, &req_id, rpc.params, client_ip, trace_id)
                    .await?
            }
            "resources/list" => {
                principal.require(Permission::DataRead)?;
                let resources = self.collect_resources(&principal.tenant_id).await?;
                jsonrpc_ok(&req_id, &serde_json::json!({ "resources": resources }))
            }
            "resources/read" => {
                principal.require(Permission::DataRead)?;
                self.dispatch_resources_read(&principal.tenant_id, &req_id, rpc.params)
                    .await?
            }
            unknown => jsonrpc_err(&req_id, -32601, format!("method not found: {unknown}")),
        };

        Ok(McpHttpResponse {
            status_code: 200,
            session_id: Some(session.id.clone()),
            body: Some(body),
        })
    }

    /// Opens a server-to-client SSE notification channel for `GET /api/mcp`.
    ///
    /// # Errors
    /// Returns [`AppError`] if PAT authentication fails or `session_header` is missing/invalid.
    pub async fn open_sse_stream(
        &self,
        principal: &AuthPrincipal,
        session_header: Option<&str>,
    ) -> Result<mpsc::Receiver<serde_json::Value>, AppError> {
        ensure_pat_principal(principal)?;
        let session = self.require_session(principal, session_header).await?;
        let (tx, rx) = mpsc::channel(32);
        let mut guard = session
            .sse_senders
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.retain(|s| !s.is_closed());
        guard.push(tx);
        Ok(rx)
    }

    /// Terminates an active `Mcp-Session-Id` (`DELETE /api/mcp`) and closes open SSE channels.
    ///
    /// # Errors
    /// Returns [`AppError`] if PAT authentication fails or `session_header` is missing/invalid.
    pub async fn close_session(
        &self,
        principal: &AuthPrincipal,
        session_header: Option<&str>,
    ) -> Result<(), AppError> {
        ensure_pat_principal(principal)?;
        let session = self.require_session(principal, session_header).await?;
        {
            let mut guard = session
                .sse_senders
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.clear();
        }
        self.sessions.invalidate(&session.id).await;
        Ok(())
    }

    async fn handle_initialize(
        &self,
        principal: &AuthPrincipal,
        rpc: &JsonRpcIncoming,
    ) -> McpHttpResponse {
        let session_id = Uuid::new_v4().to_string();
        let session = Arc::new(McpSession {
            id: session_id.clone(),
            tenant_id: principal.tenant_id.clone(),
            user_id: principal.user_id,
            sse_senders: Mutex::new(Vec::new()),
        });
        self.sessions.insert(session_id.clone(), session).await;

        let requested_proto = rpc
            .params
            .as_ref()
            .and_then(|p| p.get("protocolVersion"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(DEFAULT_MCP_PROTOCOL_VERSION);

        let req_id = rpc.id.clone().unwrap_or(serde_json::Value::Null);
        let body = jsonrpc_ok(
            &req_id,
            &serde_json::json!({
                "protocolVersion": requested_proto,
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false }
                },
                "serverInfo": {
                    "name": "axpoint-mcp",
                    "version": "0.1.0"
                }
            }),
        );

        McpHttpResponse {
            status_code: 200,
            session_id: Some(session_id),
            body: Some(body),
        }
    }

    async fn require_session(
        &self,
        principal: &AuthPrincipal,
        session_header: Option<&str>,
    ) -> Result<Arc<McpSession>, AppError> {
        let sid = session_header
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "missing Mcp-Session-Id header"))?;
        let session = self
            .sessions
            .get(sid)
            .await
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        if session.tenant_id != principal.tenant_id || session.user_id != principal.user_id {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        Ok(session)
    }

    async fn collect_visible_tools(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<Vec<McpToolDescriptor>, AppError> {
        let mut tools = Vec::new();
        if principal.has(Permission::DataRead) {
            tools.extend(builtin_readonly_mcp_tools());
        }

        let endpoints = self
            .store
            .list_endpoints(
                &principal.tenant_id,
                EndpointListQuery {
                    environment: Some(Environment::Prod),
                    kind: None,
                    show_as_app: None,
                },
            )
            .await?;

        for ep in endpoints {
            if !ep.enabled || !ep.expose_mcp || !principal.has(ep.required_scope) {
                continue;
            }
            let tool_name = ep.slug.replace('-', "_");
            if tools.iter().any(|t| t.name == tool_name) {
                continue;
            }
            let description = if ep.description.trim().is_empty() {
                ep.title
            } else {
                ep.description
            };
            tools.push(McpToolDescriptor {
                name: tool_name,
                description,
                input_schema: ep.input_schema,
            });
        }
        Ok(tools)
    }

    async fn dispatch_tools_call(
        &self,
        principal: &AuthPrincipal,
        req_id: &serde_json::Value,
        params: Option<serde_json::Value>,
        client_ip: &str,
        trace_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let Some(params_obj) = params else {
            return Ok(jsonrpc_err(req_id, -32602, "missing params object"));
        };
        let Some(tool_name) = params_obj
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
        else {
            return Ok(jsonrpc_err(req_id, -32602, "missing non-empty tool name"));
        };
        let arguments = params_obj
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let start = Instant::now();
        let exec_res = self
            .execute_mcp_tool(principal, &tool_name, arguments)
            .await;
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        match exec_res {
            Ok(Some(val)) => {
                self.emit_mcp_audit(
                    principal,
                    &tool_name,
                    duration_ms,
                    false,
                    client_ip,
                    trace_id,
                );
                let text = serde_json::to_string(&val).unwrap_or_else(|_| String::from("{}"));
                Ok(jsonrpc_ok(
                    req_id,
                    &serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }),
                ))
            }
            Ok(None) => Ok(jsonrpc_err(
                req_id,
                -32602,
                format!("unknown or unauthorized tool: {tool_name}"),
            )),
            Err(
                err @ (AppError::Unauthorized(_) | AppError::Forbidden(_) | AppError::RateLimited),
            ) => {
                self.emit_mcp_audit(
                    principal,
                    &tool_name,
                    duration_ms,
                    true,
                    client_ip,
                    trace_id,
                );
                Err(err)
            }
            Err(err) => {
                self.emit_mcp_audit(
                    principal,
                    &tool_name,
                    duration_ms,
                    true,
                    client_ip,
                    trace_id,
                );
                Ok(jsonrpc_ok(
                    req_id,
                    &serde_json::json!({
                        "content": [{ "type": "text", "text": err.client_message() }],
                        "isError": true
                    }),
                ))
            }
        }
    }

    async fn execute_mcp_tool(
        &self,
        principal: &AuthPrincipal,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AppError> {
        if is_builtin_readonly_mcp_tool(tool_name) {
            principal.require(Permission::DataRead)?;
            let schema = builtin_mcp_schema(tool_name);
            validate_instance_against_schema(&arguments, &schema)?;
            let out = self
                .execute_builtin_readonly(principal, tool_name, &arguments)
                .await?;
            return Ok(Some(out));
        }

        let maybe_ep = self
            .find_exposed_prod_endpoint(&principal.tenant_id, tool_name)
            .await?;
        let Some(ep) = maybe_ep else {
            return Ok(None);
        };
        let out = self
            .publisher
            .invoke_for_tool(principal, &ep, arguments, ConversationChannel::Mcp)
            .await?;
        Ok(Some(out))
    }

    async fn find_exposed_prod_endpoint(
        &self,
        tenant_id: &str,
        tool_name: &str,
    ) -> Result<Option<EndpointSpec>, AppError> {
        let list = self
            .store
            .list_endpoints(
                tenant_id,
                EndpointListQuery {
                    environment: Some(Environment::Prod),
                    kind: None,
                    show_as_app: None,
                },
            )
            .await?;
        Ok(list.into_iter().find(|ep| {
            ep.enabled
                && ep.expose_mcp
                && (ep.slug == tool_name || ep.slug.replace('-', "_") == tool_name)
        }))
    }

    async fn execute_builtin_readonly(
        &self,
        principal: &AuthPrincipal,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let an_principal = to_analytics_principal(principal);

        match tool_name {
            "search_knowledge" => {
                let query = required_str_arg(args, "query")?;
                let top_k = optional_usize_arg(args, "top_k", 6, 20);
                let prefix = args
                    .get("doc_title_prefix")
                    .and_then(serde_json::Value::as_str);
                let hits = self
                    .capability
                    .search_knowledge(tenant_id, query, top_k, prefix)
                    .await?;
                Ok(serde_json::json!({ "hits": hits, "count": hits.len() }))
            }
            "ask_data" => {
                let question = required_str_arg(args, "question")?;
                let ds_names: Vec<String> = args
                    .get("dataset_names")
                    .and_then(serde_json::Value::as_array)
                    .map_or_default(|arr| {
                        arr.iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(String::from)
                            .collect()
                    });
                self.capability
                    .ask_data(&an_principal, question, &ds_names)
                    .await
            }
            "list_datasets" => {
                let limit = optional_u32_arg(args, "limit", 50, 200);
                let page = self
                    .store
                    .list_datasets(
                        tenant_id,
                        DatasetListQuery {
                            q: args
                                .get("q")
                                .and_then(serde_json::Value::as_str)
                                .map(String::from),
                            tag: args
                                .get("tag")
                                .and_then(serde_json::Value::as_str)
                                .map(String::from),
                            source_kind: None,
                            limit: Some(limit),
                            cursor: None,
                        },
                    )
                    .await?;
                serde_json::to_value(page)
                    .map_err(|e| AppError::Config(format!("serialize datasets: {e}")))
            }
            "describe_dataset" => self.describe_dataset_tool(tenant_id, args).await,
            "query_objects" => {
                let type_name = required_str_arg(args, "type_name")?;
                let mut filter = args.clone();
                if let Some(obj) = filter.as_object_mut() {
                    obj.remove("type_name");
                }
                self.capability
                    .query_domain_objects(&an_principal, type_name, None, &filter)
                    .await
            }
            "graph_neighbors" => self.graph_neighbors_tool(tenant_id, args).await,
            "get_node" => self.get_node_masked_tool(principal, args).await,
            _ => Err(AppError::bad(
                "INVALID_ARGUMENT",
                "unknown built-in MCP tool",
            )),
        }
    }

    async fn describe_dataset_tool(
        &self,
        tenant_id: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let ds_ref = required_str_arg(args, "dataset")?;
        let ds = if let Ok(ds_id) = Uuid::parse_str(ds_ref) {
            self.store.get_dataset(tenant_id, ds_id).await?
        } else {
            self.store.get_dataset_by_name(tenant_id, ds_ref).await?
        }
        .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let columns = self.store.list_columns(tenant_id, ds.id).await?;
        Ok(serde_json::json!({
            "dataset": ds,
            "columns": columns,
        }))
    }

    async fn graph_neighbors_tool(
        &self,
        tenant_id: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let node_id_str = required_str_arg(args, "node_id")?;
        let node_id = Uuid::parse_str(node_id_str)
            .map_err(|e| AppError::bad("INVALID_ARGUMENT", format!("invalid node_id: {e}")))?;
        let depth = optional_u32_arg(args, "depth", 1, 4);
        let limit = optional_u32_arg(args, "limit", 100, 200);
        let direction = args
            .get("direction")
            .and_then(|v| serde_json::from_value::<TraversalDirection>(v.clone()).ok())
            .unwrap_or_default();
        let link_types = args
            .get("link_types")
            .and_then(serde_json::Value::as_str)
            .map(String::from);
        let sub = self
            .store
            .query_neighbors(
                tenant_id,
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

    async fn get_node_masked_tool(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let mut node = if let Some(id_str) = args
            .get("node_id")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            let nid = Uuid::parse_str(id_str.trim())
                .map_err(|e| AppError::bad("INVALID_ARGUMENT", format!("invalid node_id: {e}")))?;
            self.store.get_node(tenant_id, nid).await?
        } else {
            let tname = required_str_arg(args, "type_name")?;
            let nkey = required_str_arg(args, "node_key")?;
            let page = self
                .store
                .list_nodes(
                    tenant_id,
                    NodeListQuery {
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
            && let Ok(pub_schema) = self.store.get_published_schema(tenant_id, "main").await
            && let Some(nt) = pub_schema
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

    async fn collect_resources(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<McpResourceDescriptor>, AppError> {
        let mut out = Vec::new();
        let datasets = self
            .store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    q: None,
                    tag: None,
                    source_kind: None,
                    limit: Some(200),
                    cursor: None,
                },
            )
            .await?;
        for ds in datasets.items {
            out.push(McpResourceDescriptor {
                uri: format!("axpoint://datasets/{}", ds.name),
                name: ds.display_name,
                description: ds.description,
                mime_type: "application/json",
            });
        }

        let published = self.store.get_published_schema(tenant_id, "main").await?;
        for nt in published.doc.node_types {
            out.push(McpResourceDescriptor {
                uri: format!("axpoint://objects/{}", nt.name),
                name: nt.display_name,
                description: nt.description,
                mime_type: "application/json",
            });
        }
        Ok(out)
    }

    async fn dispatch_resources_read(
        &self,
        tenant_id: &str,
        req_id: &serde_json::Value,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AppError> {
        let Some(uri) = params
            .as_ref()
            .and_then(|p| p.get("uri"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(jsonrpc_err(req_id, -32602, "missing resource uri"));
        };

        if let Some(ds_name) = uri.strip_prefix("axpoint://datasets/") {
            let ds = self
                .store
                .get_dataset_by_name(tenant_id, ds_name)
                .await?
                .ok_or(AppError::NotFound("NOT_FOUND"))?;
            let cols = self.store.list_columns(tenant_id, ds.id).await?;
            let text = serde_json::to_string(&serde_json::json!({
                "dataset": ds,
                "columns": cols,
            }))
            .unwrap_or_else(|_| String::from("{}"));
            return Ok(jsonrpc_ok(
                req_id,
                &serde_json::json!({
                    "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
                }),
            ));
        }

        if let Some(type_name) = uri.strip_prefix("axpoint://objects/") {
            let published = self.store.get_published_schema(tenant_id, "main").await?;
            let nt = published
                .doc
                .node_types
                .iter()
                .find(|n| n.name == type_name)
                .ok_or(AppError::NotFound("NOT_FOUND"))?;
            let edges: Vec<_> = published
                .doc
                .edge_types
                .iter()
                .filter(|e| e.src_type_name == type_name || e.dst_type_name == type_name)
                .collect();
            let text = serde_json::to_string(&serde_json::json!({
                "version": published.version,
                "node_type": nt,
                "edge_types": edges,
            }))
            .unwrap_or_else(|_| String::from("{}"));
            return Ok(jsonrpc_ok(
                req_id,
                &serde_json::json!({
                    "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
                }),
            ));
        }

        Ok(jsonrpc_err(
            req_id,
            -32602,
            format!("unsupported resource uri: {uri}"),
        ))
    }

    fn emit_mcp_audit(
        &self,
        principal: &AuthPrincipal,
        tool_name: &str,
        duration_ms: u64,
        is_error: bool,
        client_ip: &str,
        trace_id: &str,
    ) {
        metrics::counter!(
            "axpoint_mcp_calls_total",
            "error" => if is_error { "true" } else { "false" }
        )
        .increment(1);

        let Some(ref writer) = self.audit else {
            return;
        };
        writer.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: if principal.user_id.is_nil() {
                None
            } else {
                Some(principal.user_id)
            },
            username: principal.username.clone(),
            action: AuditAction::McpCall,
            resource_ref: format!("mcp:tool:{tool_name}"),
            status_code: if is_error { 400 } else { 200 },
            client_ip: if client_ip.is_empty() {
                String::from("127.0.0.1")
            } else {
                String::from(client_ip)
            },
            trace_id: String::from(trace_id),
            detail_json: serde_json::json!({
                "tool_name": tool_name,
                "duration_ms": duration_ms,
                "is_error": is_error,
            }),
            created_at: Utc::now(),
        });
    }
}

fn ensure_pat_principal(principal: &AuthPrincipal) -> Result<(), AppError> {
    if principal.token_kind != TokenKind::ApiToken {
        return Err(AppError::Unauthorized("TOKEN_INVALID"));
    }
    Ok(())
}

fn jsonrpc_ok(id: &serde_json::Value, result: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_err(id: &serde_json::Value, code: i32, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

fn required_str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, AppError> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("SCHEMA_INVALID", format!("missing non-empty '{key}'")))
}

fn optional_usize_arg(
    args: &serde_json::Value,
    key: &str,
    default_val: usize,
    cap: usize,
) -> usize {
    args.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .map_or(default_val, |v| v.clamp(1, cap))
}

fn optional_u32_arg(args: &serde_json::Value, key: &str, default_val: u32, cap: u32) -> u32 {
    args.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .map_or(default_val, |v| v.clamp(1, cap))
}

fn is_builtin_readonly_mcp_tool(name: &str) -> bool {
    matches!(
        name,
        "search_knowledge"
            | "ask_data"
            | "list_datasets"
            | "describe_dataset"
            | "query_objects"
            | "graph_neighbors"
            | "get_node"
    )
}

fn builtin_readonly_mcp_tools() -> Vec<McpToolDescriptor> {
    let names_and_descs = [
        (
            "search_knowledge",
            "五路超图混合召回检索企业知识库段落与实体关系",
        ),
        ("ask_data", "自然语言智能问数（NL2SQL + 脱敏结果摘要）"),
        ("list_datasets", "列出当前租户的数据湖资产目录"),
        (
            "describe_dataset",
            "查询指定数据集的列结构、PII 分类与业务术语",
        ),
        (
            "query_objects",
            "按类型、键、属性与关联关系查询主图已发布领域对象",
        ),
        ("graph_neighbors", "查询业务图谱节点的 k 跳上下游邻居子图"),
        (
            "get_node",
            "按 node_id 或 (type_name, node_key) 读取单个业务节点快照",
        ),
    ];
    names_and_descs
        .into_iter()
        .map(|(name, desc)| McpToolDescriptor {
            name: String::from(name),
            description: String::from(desc),
            input_schema: builtin_mcp_schema(name),
        })
        .collect()
}

fn builtin_mcp_schema(name: &str) -> serde_json::Value {
    match name {
        "search_knowledge" => serde_json::json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string", "minLength": 1 },
                "top_k": { "type": "integer", "minimum": 1, "maximum": 20 },
                "doc_title_prefix": { "type": "string" }
            },
            "additionalProperties": false
        }),
        "ask_data" => serde_json::json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": { "type": "string", "minLength": 1 },
                "dataset_names": { "type": "array", "items": { "type": "string" } }
            },
            "additionalProperties": false
        }),
        "list_datasets" => serde_json::json!({
            "type": "object",
            "properties": {
                "q": { "type": "string" },
                "tag": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            },
            "additionalProperties": false
        }),
        "describe_dataset" => serde_json::json!({
            "type": "object",
            "required": ["dataset"],
            "properties": {
                "dataset": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        }),
        "query_objects" => serde_json::json!({
            "type": "object",
            "required": ["type_name"],
            "properties": {
                "type_name": { "type": "string", "minLength": 1 },
                "keys": { "type": "array", "items": { "type": "string" } },
                "q": { "type": "string" },
                "properties": { "type": "object" },
                "metrics_min": { "type": "object" },
                "include_links": { "type": "array", "items": { "type": "string" } },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            },
            "additionalProperties": false
        }),
        "graph_neighbors" => serde_json::json!({
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
        "get_node" => serde_json::json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "string", "format": "uuid" },
                "type_name": { "type": "string" },
                "node_key": { "type": "string" }
            },
            "additionalProperties": false
        }),
        _ => serde_json::json!({ "type": "object" }),
    }
}
