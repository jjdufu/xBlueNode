//! Outbound Streamable HTTP MCP client manager (`McpClientManager`, `08-STEP09` §09.4.2).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    CreateMcpClientRequest, McpClientRecord, McpToolInfo, SaveMcpClientInput,
};
use axpoint_domain::axhub::SealedCredential;
use axpoint_domain::enums::Permission;
use axpoint_domain::ports::{AgentRepository, OutboundUrlGuardPort};
use axpoint_security::net_guard::safe_redirect_policy;
use axpoint_security::{AuthPrincipal, CredentialKeyring};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use rmcp::model::ProtocolVersion;
use uuid::Uuid;

const MCP_HTTP_TIMEOUT_SECS: u64 = 30;
const MCP_SESSION_HEADER: &str = "mcp-session-id";

/// Outbound Streamable HTTP MCP client manager enforcing `NetGuard` SSRF checks,
/// optional credential header unsealing, and `tools/list` / `tools/call` transport (`08-STEP09` §09.4.2).
pub struct McpClientManager<R> {
    store: Arc<R>,
    url_guard: Arc<dyn OutboundUrlGuardPort>,
    keyring: Option<Arc<CredentialKeyring>>,
    http_public: reqwest::Client,
    http_private: reqwest::Client,
}

impl<R> McpClientManager<R> {
    /// Creates a new [`McpClientManager`] with a 30-second timeout and SSRF-safe redirect policies.
    #[must_use]
    pub fn new(
        store: Arc<R>,
        url_guard: Arc<dyn OutboundUrlGuardPort>,
        keyring: Option<Arc<CredentialKeyring>>,
    ) -> Self {
        let http_public = reqwest::Client::builder()
            .timeout(Duration::from_secs(MCP_HTTP_TIMEOUT_SECS))
            .redirect(safe_redirect_policy(false, 3))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let http_private = reqwest::Client::builder()
            .timeout(Duration::from_secs(MCP_HTTP_TIMEOUT_SECS))
            .redirect(safe_redirect_policy(true, 3))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            store,
            url_guard,
            keyring,
            http_public,
            http_private,
        }
    }

    const fn client_for(&self, allow_private_network: bool) -> &reqwest::Client {
        if allow_private_network {
            &self.http_private
        } else {
            &self.http_public
        }
    }
}

impl<R> McpClientManager<R>
where
    R: AgentRepository + 'static,
{
    /// Lists registered external MCP client servers (`AGENT_EDIT` or `DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] if permission is missing, or [`AppError::Database`] on storage failure.
    pub async fn list_clients(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<Vec<McpClientRecord>, AppError> {
        if !principal.has(Permission::AgentEdit) && !principal.has(Permission::DataRead) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        self.store.list_mcp_clients(&principal.tenant_id).await
    }

    /// Seals `req.headers` (if non-empty), validates `req.url` via `NetGuard`, and registers the MCP client.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, missing keyring when headers are set, SSRF block, or duplicate name.
    pub async fn create_from_request(
        &self,
        principal: &AuthPrincipal,
        req: CreateMcpClientRequest,
    ) -> Result<McpClientRecord, AppError> {
        let credential = match req.headers {
            Some(ref map) if !map.is_empty() => {
                let kr = self.keyring.as_ref().ok_or_else(|| {
                    AppError::Config(String::from(
                        "CredentialKeyring required to seal MCP headers",
                    ))
                })?;
                let cid = Uuid::new_v4();
                let aad = CredentialKeyring::aad_for_credential(&principal.tenant_id, cid);
                let bytes = serde_json::to_vec(map)
                    .map_err(|e| AppError::Config(format!("serialize MCP headers: {e}")))?;
                Some(kr.seal_credential(cid, &aad, &bytes)?)
            }
            _ => None,
        };
        self.register_client(
            principal,
            SaveMcpClientInput {
                name: req.name,
                url: req.url,
                credential,
                allowed_tools: req.allowed_tools,
                allow_private_network: req.allow_private_network,
                enabled: req.enabled,
            },
        )
        .await
    }

    /// Validates `input.url` through `NetGuard` and registers an external MCP server (`AGENT_EDIT` required;
    /// `GUARD_ADMIN` additionally required when `allow_private_network == true`).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, SSRF block, invalid name, or uniqueness conflict.
    pub async fn register_client(
        &self,
        principal: &AuthPrincipal,
        mut input: SaveMcpClientInput,
    ) -> Result<McpClientRecord, AppError> {
        principal.require(Permission::AgentEdit)?;
        if input.allow_private_network {
            principal.require(Permission::GuardAdmin)?;
        }
        input.name = validate_mcp_client_name(&input.name)?;
        input.url = self
            .url_guard
            .check_outbound_url(input.url.trim(), input.allow_private_network)
            .await?;
        self.store
            .create_mcp_client(&principal.tenant_id, input)
            .await
    }

    /// Deletes a registered external MCP server (`AGENT_EDIT` required).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or when `client_id` does not exist.
    pub async fn delete_client(
        &self,
        principal: &AuthPrincipal,
        client_id: Uuid,
    ) -> Result<(), AppError> {
        principal.require(Permission::AgentEdit)?;
        let deleted = self
            .store
            .delete_mcp_client(&principal.tenant_id, client_id)
            .await?;
        if !deleted {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        Ok(())
    }

    /// Re-validates the server URL via `NetGuard`, performs MCP `initialize` + `tools/list`,
    /// filters by `allowed_tools` when non-empty, and caches discovered tools in `ax_mcp_clients.tools_cache`.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, SSRF block, upstream timeout/error, or missing client.
    pub async fn sync_client_tools(
        &self,
        principal: &AuthPrincipal,
        client_id: Uuid,
    ) -> Result<McpClientRecord, AppError> {
        principal.require(Permission::AgentEdit)?;
        let (client, sealed_cred) = self
            .store
            .get_mcp_client(&principal.tenant_id, client_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;

        let checked_url = self
            .url_guard
            .check_outbound_url(&client.url, client.allow_private_network)
            .await?;
        let extra_headers = self.build_auth_headers(&principal.tenant_id, sealed_cred.as_ref())?;
        let http = self.client_for(client.allow_private_network);
        let session_id = handshake_initialize(http, &checked_url, &extra_headers).await?;

        let list_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });
        let (_sid, resp) = post_mcp_message(
            http,
            &checked_url,
            &extra_headers,
            session_id.as_deref(),
            &list_req,
            Some(2),
        )
        .await?;

        let mut tools = parse_tools_list_result(&resp)?;
        if !client.allowed_tools.is_empty() {
            tools.retain(|t| client.allowed_tools.iter().any(|a| a == &t.name));
        }

        self.store
            .update_mcp_client_tools(&principal.tenant_id, client_id, &tools)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))
    }

    /// Invokes `tools/call` on a registered external MCP server by `client_id`.
    ///
    /// # Errors
    /// Returns [`AppError`] if the client is missing/disabled, tool is disallowed, blocked by `NetGuard`, or upstream fails.
    pub async fn call_external_tool_by_id(
        &self,
        tenant_id: &str,
        client_id: Uuid,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let (client, sealed_cred) = self
            .store
            .get_mcp_client(tenant_id, client_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        self.invoke_remote_tool(
            tenant_id,
            &client,
            sealed_cred.as_ref(),
            tool_name,
            arguments,
        )
        .await
    }

    /// Invokes `tools/call` on a registered external MCP server by `client_name` (`ToolRef::Mcp.client`).
    ///
    /// # Errors
    /// Returns [`AppError`] if the client is missing/disabled, tool is disallowed, blocked by `NetGuard`, or upstream fails.
    pub async fn call_external_tool_by_name(
        &self,
        tenant_id: &str,
        client_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let (client, sealed_cred) = self
            .store
            .get_mcp_client_by_name(tenant_id, client_name)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        self.invoke_remote_tool(
            tenant_id,
            &client,
            sealed_cred.as_ref(),
            tool_name,
            arguments,
        )
        .await
    }

    async fn invoke_remote_tool(
        &self,
        tenant_id: &str,
        client: &McpClientRecord,
        sealed_cred: Option<&SealedCredential>,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        if !client.enabled {
            return Err(AppError::Conflict("CONFLICT"));
        }
        if !client.allowed_tools.is_empty() && !client.allowed_tools.iter().any(|t| t == tool_name)
        {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }

        let checked_url = self
            .url_guard
            .check_outbound_url(&client.url, client.allow_private_network)
            .await?;
        let extra_headers = self.build_auth_headers(tenant_id, sealed_cred)?;
        let http = self.client_for(client.allow_private_network);
        let session_id = handshake_initialize(http, &checked_url, &extra_headers).await?;

        let call_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments,
            }
        });
        let (_sid, resp) = post_mcp_message(
            http,
            &checked_url,
            &extra_headers,
            session_id.as_deref(),
            &call_req,
            Some(2),
        )
        .await?;
        extract_jsonrpc_result(&resp)
    }

    fn build_auth_headers(
        &self,
        tenant_id: &str,
        sealed_cred: Option<&SealedCredential>,
    ) -> Result<HeaderMap, AppError> {
        let mut map = HeaderMap::new();
        let (Some(cred), Some(kr)) = (sealed_cred, &self.keyring) else {
            return Ok(map);
        };
        let aad = CredentialKeyring::aad_for_credential(tenant_id, cred.id);
        let plain = kr.open_credential(&aad, cred)?;
        let secrets: BTreeMap<String, String> = serde_json::from_slice(plain.as_slice())
            .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))?;
        for (raw_name, raw_val) in secrets {
            let lower = raw_name.trim().to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "host" | "content-length" | "transfer-encoding"
            ) {
                continue;
            }
            if lower == "bearer_token" || lower == "token" {
                let bearer = format!("Bearer {}", raw_val.trim());
                let hval = HeaderValue::from_str(&bearer).map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid MCP token header: {err}"),
                    )
                })?;
                map.insert(reqwest::header::AUTHORIZATION, hval);
                continue;
            }
            let hname = HeaderName::from_bytes(raw_name.trim().as_bytes()).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid MCP header name: {err}"),
                )
            })?;
            let hval = HeaderValue::from_str(raw_val.trim()).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid MCP header value: {err}"),
                )
            })?;
            map.insert(hname, hval);
        }
        Ok(map)
    }
}

async fn handshake_initialize(
    http: &reqwest::Client,
    checked_url: &str,
    extra_headers: &HeaderMap,
) -> Result<Option<String>, AppError> {
    let proto_ver = serde_json::to_value(ProtocolVersion::LATEST)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| String::from("2025-03-26"));

    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": proto_ver,
            "capabilities": {},
            "clientInfo": {
                "name": "axpoint-mcp-client",
                "version": "0.1.0"
            }
        }
    });

    let (session_id, init_resp) =
        post_mcp_message(http, checked_url, extra_headers, None, &init_req, Some(1)).await?;
    let _ = extract_jsonrpc_result(&init_resp)?;

    let notify = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    if let Err(err) = post_mcp_message(
        http,
        checked_url,
        extra_headers,
        session_id.as_deref(),
        &notify,
        None,
    )
    .await
    {
        tracing::debug!(error = %err, "remote MCP notifications/initialized returned non-2xx");
    }

    Ok(session_id)
}

async fn post_mcp_message(
    http: &reqwest::Client,
    checked_url: &str,
    extra_headers: &HeaderMap,
    session_id: Option<&str>,
    payload: &serde_json::Value,
    expected_id: Option<i64>,
) -> Result<(Option<String>, serde_json::Value), AppError> {
    let mut req_builder = http
        .post(checked_url)
        .headers(extra_headers.clone())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(payload);

    if let Some(sid) = session_id {
        req_builder = req_builder.header("Mcp-Session-Id", sid);
    }

    let resp = req_builder
        .send()
        .await
        .map_err(|e| map_reqwest_error(&e))?;
    let status = resp.status();
    let returned_sid = resp
        .headers()
        .get(MCP_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| session_id.map(String::from));

    if !status.is_success() {
        return Err(AppError::Upstream(format!(
            "external MCP server returned HTTP {}",
            status.as_u16()
        )));
    }

    if expected_id.is_none() || status == reqwest::StatusCode::ACCEPTED {
        return Ok((returned_sid, serde_json::json!({ "result": {} })));
    }

    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let decoded = if content_type.contains("text/event-stream") {
        read_sse_jsonrpc_response(resp, expected_id).await?
    } else {
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| map_reqwest_error(&e))?
    };
    Ok((returned_sid, decoded))
}

async fn read_sse_jsonrpc_response(
    resp: reqwest::Response,
    expected_id: Option<i64>,
) -> Result<serde_json::Value, AppError> {
    let mut stream = resp.bytes_stream().eventsource();
    let mut last_json: Option<serde_json::Value> = None;

    while let Some(item) = stream.next().await {
        let event = item.map_err(|err| {
            AppError::Upstream(format!("failed to decode external MCP SSE stream: {err}"))
        })?;
        let trimmed = event.data.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if let Some(target_id) = expected_id
            && parsed.get("id").and_then(serde_json::Value::as_i64) == Some(target_id)
        {
            return Ok(parsed);
        }
        last_json = Some(parsed);
    }

    last_json.ok_or_else(|| {
        AppError::Upstream(String::from(
            "external MCP SSE stream ended without a JSON-RPC response",
        ))
    })
}

fn map_reqwest_error(err: &reqwest::Error) -> AppError {
    if err.is_timeout() {
        AppError::Timeout("UPSTREAM_TIMEOUT")
    } else {
        AppError::Upstream(format!("external MCP transport error: {err}"))
    }
}

fn extract_jsonrpc_result(resp: &serde_json::Value) -> Result<serde_json::Value, AppError> {
    if let Some(err_obj) = resp.get("error") {
        let msg = err_obj
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("remote MCP JSON-RPC error");
        return Err(AppError::Upstream(format!("external MCP error: {msg}")));
    }
    Ok(resp
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

fn parse_tools_list_result(resp: &serde_json::Value) -> Result<Vec<McpToolInfo>, AppError> {
    let result = extract_jsonrpc_result(resp)?;
    let Some(arr) = result.get("tools").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(name) = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let description = item
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned();
        let input_schema = item
            .get("inputSchema")
            .or_else(|| item.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
        out.push(McpToolInfo {
            name: String::from(name),
            description,
            input_schema,
        });
    }
    Ok(out)
}

fn validate_mcp_client_name(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    let Some(&first) = bytes.first() else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "MCP client name is empty",
        ));
    };
    if bytes.len() > 32
        || !first.is_ascii_lowercase()
        || !bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid MCP client name '{trimmed}': must match ^[a-z][a-z0-9_]{{0,31}}$"),
        ));
    }
    Ok(String::from(trimmed))
}
