//! Deterministic zero-temperature sampling lock, constrained decoding wire adapters,
//! local `7B–32B` context budget trimming, `jsonschema` post-validation, and single-round
//! schema downgrade/repair helpers (`01-STEP15` §15.3.2–§15.3.3).

use axpoint_domain::AppError;
use axpoint_domain::agent::LlmProviderRecord;
use axpoint_domain::enums::{Purpose, StructuredOutputMode};
use axpoint_domain::json_schema::{JsonSchemaSpec, strip_markdown_json_fence};
use reqwest::StatusCode;

use crate::tools::{ToolChatMessage, WireJsonSchemaPayload, WireResponseFormat};

/// Default fixed sampling seed injected on deterministic structured workloads when
/// `provider.structured_caps.deterministic_seed` is `None`.
pub const DEFAULT_DETERMINISTIC_SEED: i64 = 42;

/// Minimum input token budget floor retained when trimming conversation history for local models.
const MIN_CONTEXT_INPUT_BUDGET: i32 = 1_024;

/// Maximum `jsonschema` validation error paths included in a repair prompt or `422` diagnostic.
const MAX_REPORTED_SCHEMA_ERRORS: usize = 3;

/// Returns `true` when `(purpose, has_schema)` requires deterministic `temperature = 0.0` sampling.
#[must_use]
pub const fn is_deterministic_workload(purpose: Purpose, has_schema: bool) -> bool {
    matches!(
        purpose,
        Purpose::Nl2Sql | Purpose::Extract | Purpose::Summarize
    ) || has_schema
}

/// Resolves outbound `(temperature, seed)` for `provider`, physically clamping `temperature` to
/// `0.0_f32` on deterministic purposes (`NL2SQL`, `EXTRACT`, `SUMMARIZE`) or whenever `has_schema`
/// is `true` (`01-STEP15` §15.3.2).
#[must_use]
pub fn resolve_effective_sampling(
    purpose: Purpose,
    provider: &LlmProviderRecord,
    req_temp: Option<f32>,
    has_schema: bool,
) -> (f32, Option<i64>) {
    let raw_temp = req_temp.unwrap_or(provider.temperature);
    if is_deterministic_workload(purpose, has_schema) {
        if raw_temp > f32::EPSILON {
            metrics::counter!(
                "axpoint_llm_temperature_clamped_total",
                "purpose" => purpose.as_str()
            )
            .increment(1);
        }
        let seed = provider.structured_caps.deterministic_seed.or_else(|| {
            (provider.structured_caps.structured_mode != StructuredOutputMode::None)
                .then_some(DEFAULT_DETERMINISTIC_SEED)
        });
        (0.0_f32, seed)
    } else {
        (raw_temp, provider.structured_caps.deterministic_seed)
    }
}

/// Resolves outbound `temperature` for the fallback environment `default_llm`, clamping to `0.0_f32`
/// on deterministic workloads.
#[must_use]
pub fn resolve_default_llm_temperature(
    purpose: Purpose,
    req_temp: Option<f32>,
    has_schema: bool,
) -> f32 {
    let raw_temp = req_temp.unwrap_or(0.2_f32);
    if is_deterministic_workload(purpose, has_schema) {
        if raw_temp > f32::EPSILON {
            metrics::counter!(
                "axpoint_llm_temperature_clamped_total",
                "purpose" => purpose.as_str()
            )
            .increment(1);
        }
        0.0_f32
    } else {
        raw_temp
    }
}

/// Builds the outbound [`WireResponseFormat`] for `provider` and `schema`, selecting
/// `local_compact_schema` automatically when `provider.structured_caps.is_local_model` is `true`.
#[must_use]
pub fn build_wire_response_format(
    provider: &LlmProviderRecord,
    schema: &JsonSchemaSpec,
) -> Option<WireResponseFormat> {
    match provider.structured_caps.structured_mode {
        StructuredOutputMode::JsonSchema => {
            let effective = schema
                .effective_schema(provider.structured_caps.is_local_model)
                .clone();
            let is_strict = is_strict_draft2020_12_schema(&effective);
            Some(WireResponseFormat::JsonSchema {
                json_schema: WireJsonSchemaPayload {
                    name: schema.name.clone(),
                    description: schema.description.clone(),
                    strict: is_strict,
                    schema: effective,
                },
            })
        }
        StructuredOutputMode::JsonObject => Some(WireResponseFormat::JsonObject),
        StructuredOutputMode::None => None,
    }
}

/// Returns `true` when `root` satisfies strict Draft 2020-12 grammar constraints
/// (`additionalProperties == false` and full `required` coverage on every `"object"` schema node).
#[must_use]
pub fn is_strict_draft2020_12_schema(root: &serde_json::Value) -> bool {
    let Some(obj) = root.as_object() else {
        return false;
    };
    obj.get("type").and_then(serde_json::Value::as_str) == Some("object")
        && is_strict_schema_node(root)
}

fn is_strict_schema_node(node: &serde_json::Value) -> bool {
    let Some(obj) = node.as_object() else {
        return true;
    };
    if obj.get("type").and_then(serde_json::Value::as_str) == Some("object")
        && !is_strict_object_map(obj)
    {
        return false;
    }
    if let Some(items) = obj.get("items")
        && !is_strict_schema_node(items)
    {
        return false;
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(arr) = obj.get(key).and_then(serde_json::Value::as_array)
            && !arr.iter().all(is_strict_schema_node)
        {
            return false;
        }
    }
    for defs_key in ["$defs", "definitions"] {
        if let Some(defs) = obj.get(defs_key).and_then(serde_json::Value::as_object)
            && !defs.values().all(is_strict_schema_node)
        {
            return false;
        }
    }
    true
}

fn is_strict_object_map(obj: &serde_json::Map<String, serde_json::Value>) -> bool {
    if obj.get("additionalProperties") != Some(&serde_json::Value::Bool(false)) {
        return false;
    }
    let Some(props) = obj.get("properties").and_then(serde_json::Value::as_object) else {
        return false;
    };
    let Some(req_arr) = obj.get("required").and_then(serde_json::Value::as_array) else {
        return false;
    };
    let all_required = props
        .keys()
        .all(|k| req_arr.iter().any(|v| v.as_str() == Some(k.as_str())));
    all_required && props.values().all(is_strict_schema_node)
}

/// Injects a compact inline JSON Schema skeleton into the leading `"system"` prompt when operating
/// in [`StructuredOutputMode::JsonObject`] or [`StructuredOutputMode::None`] mode (`01-STEP15` §15.3).
#[must_use]
pub fn augment_messages_with_schema_skeleton(
    messages: &[ToolChatMessage],
    mode: StructuredOutputMode,
    effective_schema: &serde_json::Value,
) -> Vec<ToolChatMessage> {
    if mode == StructuredOutputMode::JsonSchema {
        return messages.to_vec();
    }
    let compact_schema =
        serde_json::to_string(effective_schema).unwrap_or_else(|_| String::from("{}"));
    let directive = format!(
        "Output ONLY a single valid JSON object conforming strictly to this JSON Schema (no Markdown code fences, no extra text):\n{compact_schema}"
    );

    let mut out = Vec::with_capacity(messages.len().saturating_add(1));
    let mut injected = false;
    for msg in messages {
        if !injected && msg.role.eq_ignore_ascii_case("system") {
            let merged = if msg.content.trim().is_empty() {
                directive.clone()
            } else {
                format!("{}\n\n{directive}", msg.content)
            };
            out.push(ToolChatMessage::system(merged));
            injected = true;
        } else {
            out.push(msg.clone());
        }
    }
    if !injected {
        out.insert(0, ToolChatMessage::system(directive));
    }
    out
}

/// Estimates the token footprint of a [`ToolChatMessage`] using the conservative heuristic
/// `tokens ≈ utf8_chars / 3 + 16` (`01-STEP15` §15.3.3).
#[must_use]
pub fn estimate_message_tokens(msg: &ToolChatMessage) -> usize {
    let mut chars = msg
        .role
        .chars()
        .count()
        .saturating_add(msg.content.chars().count());
    if let Some(ref tc_id) = msg.tool_call_id {
        chars = chars.saturating_add(tc_id.chars().count());
    }
    for tc in &msg.tool_calls {
        chars = chars.saturating_add(tc.id.chars().count());
        chars = chars.saturating_add(tc.name.chars().count());
        let arg_len = serde_json::to_string(&tc.arguments).map_or(2, |s| s.chars().count());
        chars = chars.saturating_add(arg_len);
    }
    (chars / 3).saturating_add(16)
}

/// Trims intermediate historical turns when `is_local_model` context exceeds
/// `max_context_tokens - eff_max_tokens`, preserving leading `"system"` messages and the latest
/// active turn tail without leaving orphan `"tool"` messages (`01-STEP15` §15.3.3).
#[must_use]
pub fn trim_messages_to_context_budget(
    messages: &[ToolChatMessage],
    max_context_tokens: i32,
    eff_max_tokens: i32,
) -> Vec<ToolChatMessage> {
    let raw_budget = max_context_tokens
        .saturating_sub(eff_max_tokens)
        .max(MIN_CONTEXT_INPUT_BUDGET);
    let budget = usize::try_from(raw_budget).unwrap_or(1_024);

    let total: usize = messages.iter().map(estimate_message_tokens).sum();
    if total <= budget || messages.len() <= 2 {
        return messages.to_vec();
    }

    let sys_end = messages
        .iter()
        .take_while(|m| m.role.eq_ignore_ascii_case("system"))
        .count();
    let head = messages.get(..sys_end).unwrap_or(&[]);
    let body = messages.get(sys_end..).unwrap_or(&[]);
    if body.len() <= 1 {
        return messages.to_vec();
    }

    let groups = group_atomic_conversation_units(body);
    if groups.len() <= 1 {
        return messages.to_vec();
    }

    let head_tokens: usize = head.iter().map(estimate_message_tokens).sum();
    let mut kept_groups: Vec<&[ToolChatMessage]> = Vec::new();
    let mut used = head_tokens;

    for (rev_idx, grp) in groups.iter().rev().enumerate() {
        let grp_tokens: usize = grp.iter().map(estimate_message_tokens).sum();
        if rev_idx == 0 || used.saturating_add(grp_tokens) <= budget {
            kept_groups.push(grp);
            used = used.saturating_add(grp_tokens);
        } else {
            break;
        }
    }
    kept_groups.reverse();

    let mut trimmed = Vec::with_capacity(messages.len());
    trimmed.extend_from_slice(head);
    for grp in kept_groups {
        trimmed.extend_from_slice(grp);
    }
    trimmed
}

fn group_atomic_conversation_units(body: &[ToolChatMessage]) -> Vec<&[ToolChatMessage]> {
    let mut groups = Vec::new();
    let mut idx = 0_usize;
    while idx < body.len() {
        let Some(curr) = body.get(idx) else {
            break;
        };
        let mut end = idx.saturating_add(1);
        if curr.role.eq_ignore_ascii_case("assistant") && !curr.tool_calls.is_empty() {
            while let Some(next) = body.get(end) {
                if next.role.eq_ignore_ascii_case("tool") {
                    end = end.saturating_add(1);
                } else {
                    break;
                }
            }
        }
        if let Some(slice) = body.get(idx..end) {
            groups.push(slice);
        }
        idx = end;
    }
    groups
}

/// Strips optional Markdown JSON code fences, parses `raw_text` into a [`serde_json::Value`],
/// and validates it against `target_schema` via [`jsonschema::validator_for`] (`01-STEP15` §15.3.3).
///
/// # Errors
/// Returns up to `3` formatted `"/path: message"` diagnostic strings when JSON parsing or
/// Draft 2020-12 schema validation fails.
pub fn validate_with_jsonschema(
    raw_text: &str,
    target_schema: &serde_json::Value,
) -> Result<serde_json::Value, Vec<String>> {
    let cleaned = strip_markdown_json_fence(raw_text);
    let parsed: serde_json::Value = serde_json::from_str(cleaned)
        .map_err(|err| vec![format!("/: invalid JSON syntax: {err}")])?;

    let validator = jsonschema::validator_for(target_schema)
        .map_err(|err| vec![format!("/: invalid target JSON Schema: {err}")])?;

    let errors: Vec<String> = validator
        .iter_errors(&parsed)
        .take(MAX_REPORTED_SCHEMA_ERRORS)
        .map(|err| {
            let path = err.instance_path().to_string();
            let display_path = if path.is_empty() { "/" } else { path.as_str() };
            format!("{display_path}: {err}")
        })
        .collect();

    if errors.is_empty() {
        Ok(parsed)
    } else {
        Err(errors)
    }
}

/// Returns `true` when an upstream `HTTP 400 Bad Request` indicates that `response_format` /
/// `json_schema` / `guided_json` grammar decoding is unsupported by the model runtime
/// (Downgrade Branch A, `01-STEP15` §15.3.3).
#[must_use]
pub fn is_upstream_schema_unsupported_400(status: StatusCode, body: &str) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("response_format")
        || lower.contains("json_schema")
        || lower.contains("guided_json")
        || lower.contains("xgrammar")
        || lower.contains("invalid_json_schema")
        || lower.contains("invalid schema")
        || lower.contains("additionalproperties")
}

/// Builds the single-round `user` repair prompt listing exact `jsonschema` violation paths
/// (Downgrade Branch B, `01-STEP15` §15.3.3).
#[must_use]
pub fn build_schema_repair_message(
    schema_name: &str,
    errors: &[String],
    target_schema: &serde_json::Value,
) -> ToolChatMessage {
    let joined = errors.join("; ");
    let compact = serde_json::to_string(target_schema).unwrap_or_else(|_| String::from("{}"));
    ToolChatMessage::user(format!(
        "Your previous JSON output failed `{schema_name}` schema validation ({joined}). \
         Fix all listed fields and output ONLY a valid JSON object matching this schema (no Markdown code fences): {compact}"
    ))
}

/// Constructs the terminal `HTTP 422` (`STRUCTURED_OUTPUT_INVALID`) [`AppError`] when post-repair
/// validation still fails.
#[must_use]
pub fn unprocessable_structured_error(errors: &[String]) -> AppError {
    AppError::unprocessable("STRUCTURED_OUTPUT_INVALID", errors.join("; "))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, PoisonError};

    use axpoint_domain::agent::{LlmProviderFlags, LlmStructuredCaps, LlmUsageDelta, ToolCallSpec};
    use axpoint_domain::json_schema::TenantOntologyEnums;
    use axpoint_domain::platform::BoxFuture;
    use axpoint_domain::ports::LlmProviderStore;
    use chrono::{TimeZone as _, Utc};
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use uuid::Uuid;

    use super::*;
    use crate::LlmConfig;
    use crate::router::{LlmRouter, RouterTurnRequest};

    #[derive(Default)]
    struct StructuredTestStore {
        providers: Mutex<HashMap<String, Vec<LlmProviderRecord>>>,
        usages: Mutex<Vec<LlmUsageDelta>>,
    }

    impl StructuredTestStore {
        fn set_providers(&self, tenant_id: &str, items: Vec<LlmProviderRecord>) {
            let mut guard = self
                .providers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.insert(String::from(tenant_id), items);
        }

        fn usage_snapshot(&self) -> Vec<LlmUsageDelta> {
            self.usages
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl LlmProviderStore for StructuredTestStore {
        fn list_tenant_providers<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> BoxFuture<'a, Vec<LlmProviderRecord>> {
            Box::pin(async move {
                let guard = self
                    .providers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                Ok(guard.get(tenant_id).cloned().unwrap_or_default())
            })
        }

        fn record_usage<'a>(
            &'a self,
            _tenant_id: &'a str,
            delta: LlmUsageDelta,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                let mut guard = self.usages.lock().unwrap_or_else(PoisonError::into_inner);
                guard.push(delta);
                Ok(())
            })
        }
    }

    async fn read_http_json_body(sock: &mut TcpStream) -> Result<Value, AppError> {
        let mut buf = Vec::with_capacity(8_192);
        let mut chunk = [0_u8; 4_096];
        let mut expected_total: Option<usize> = None;

        for _ in 0..32 {
            let n = sock
                .read(&mut chunk)
                .await
                .map_err(|e| AppError::Llm(e.to_string()))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));

            if expected_total.is_none()
                && let Some(hdr_end) = find_header_end(&buf)
            {
                let hdr_str = String::from_utf8_lossy(buf.get(..hdr_end).unwrap_or(&[]));
                let content_len = parse_content_length(&hdr_str);
                expected_total = Some(hdr_end.saturating_add(4).saturating_add(content_len));
            }
            if let Some(total) = expected_total
                && buf.len() >= total
            {
                break;
            }
        }

        let body_start = find_header_end(&buf).map_or(0, |idx| idx.saturating_add(4));
        let body_bytes = buf.get(body_start..).unwrap_or(&[]);
        serde_json::from_slice(body_bytes).map_err(|e| AppError::Llm(e.to_string()))
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn parse_content_length(headers: &str) -> usize {
        for line in headers.lines() {
            if let Some((k, v)) = line.split_once(':')
                && k.trim().eq_ignore_ascii_case("content-length")
                && let Ok(parsed) = v.trim().parse::<usize>()
            {
                return parsed;
            }
        }
        0
    }

    async fn write_http_response(sock: &mut TcpStream, status: u16, body: &str) {
        let status_line = match status {
            200 => "HTTP/1.1 200 OK",
            400 => "HTTP/1.1 400 Bad Request",
            _ => "HTTP/1.1 500 Internal Server Error",
        };
        let raw = format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ok = sock.write_all(raw.as_bytes()).await.is_ok();
    }

    fn sample_structured_provider(
        base_url: String,
        temperature: f32,
        caps: LlmStructuredCaps,
    ) -> LlmProviderRecord {
        LlmProviderRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from("t_step15"),
            name: String::from("vllm-structured"),
            base_url,
            model: String::from("qwen2.5-14b-instruct"),
            purposes: vec![Purpose::Nl2Sql, Purpose::Extract, Purpose::Chat],
            has_api_key: false,
            credential_id: None,
            priority: 10,
            temperature,
            max_tokens: 1_024,
            timeout_s: 10,
            rpm_limit: None,
            embedding_dim: None,
            flags: LlmProviderFlags {
                supports_tools: true,
                allow_private_network: true,
                allow_pii: false,
            },
            structured_caps: caps,
            owner_user_id: None,
            is_readonly: false,
            enabled: true,
            created_at: Utc
                .with_ymd_and_hms(2026, 11, 22, 0, 0, 0)
                .single()
                .unwrap_or_else(Utc::now),
        }
    }

    fn dead_llm_config() -> LlmConfig {
        LlmConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("dead"),
        }
    }

    #[tokio::test]
    async fn wire_payload_carries_strict_json_schema_and_clamps_temp_zero() -> Result<(), AppError>
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_bg = Arc::clone(&captured);

        tokio::spawn(async move {
            let replies = [
                r#"{"sql":"SELECT SUM(amount) FROM demo_orders","explanation":"汇总订单金额"}"#,
                r#"{"status":"OK"}"#,
            ];
            for reply_content in replies {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                if let Ok(req_json) = read_http_json_body(&mut sock).await {
                    captured_bg
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(req_json);
                }
                let body = json!({
                    "id": "cmpl-strict",
                    "model": "qwen2.5-14b-instruct",
                    "choices": [{ "message": { "role": "assistant", "content": reply_content } }],
                    "usage": { "prompt_tokens": 20, "completion_tokens": 10 }
                })
                .to_string();
                write_http_response(&mut sock, 200, &body).await;
            }
        });

        let provider = sample_structured_provider(
            format!("http://{addr}/v1"),
            0.85,
            LlmStructuredCaps {
                structured_mode: StructuredOutputMode::JsonSchema,
                deterministic_seed: Some(77),
                is_local_model: false,
                max_context_tokens: 32_768,
            },
        );
        let store = Arc::new(StructuredTestStore::default());
        store.set_providers("t_step15", vec![provider]);

        let router = LlmRouter::new(dead_llm_config())
            .with_store(store)
            .with_cassette(None);

        let nl2sql_req = RouterTurnRequest::new(
            Purpose::Nl2Sql,
            vec![
                ToolChatMessage::system("Generate SQL"),
                ToolChatMessage::user("总金额是多少"),
            ],
        )
        .with_temperature(0.65)
        .with_response_schema(JsonSchemaSpec::nl2sql_draft());

        let nl2sql_turn = router.complete_tool_turn("t_step15", nl2sql_req).await?;
        assert!(nl2sql_turn.content.contains("SELECT SUM(amount)"));

        let extract_req = RouterTurnRequest::new(
            Purpose::Extract,
            vec![ToolChatMessage::user("Probe structured")],
        )
        .with_response_schema(JsonSchemaSpec::connectivity_probe());

        let extract_turn = router.complete_tool_turn("t_step15", extract_req).await?;
        assert_eq!(extract_turn.content, r#"{"status":"OK"}"#);

        let wire_reqs = captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        assert_eq!(wire_reqs.len(), 2);

        for req_val in &wire_reqs {
            assert_eq!(req_val.get("temperature"), Some(&json!(0.0)));
            assert_eq!(req_val.get("seed"), Some(&json!(77)));
            assert_eq!(
                req_val.pointer("/response_format/type"),
                Some(&json!("json_schema"))
            );
            assert_eq!(
                req_val.pointer("/response_format/json_schema/strict"),
                Some(&json!(true))
            );
        }
        Ok(())
    }

    async fn spawn_local_fallback_repair_stub(
        captured: Arc<Mutex<Vec<Value>>>,
    ) -> Result<std::net::SocketAddr, AppError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AppError::Llm(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| AppError::Llm(e.to_string()))?;

        tokio::spawn(async move {
            if let Ok((mut s1, _)) = listener.accept().await {
                if let Ok(j1) = read_http_json_body(&mut s1).await {
                    captured
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(j1);
                }
                write_http_response(
                    &mut s1,
                    400,
                    r#"{"error":{"message":"response_format json_schema is not supported"}}"#,
                )
                .await;
            }
            if let Ok((mut s2, _)) = listener.accept().await {
                if let Ok(j2) = read_http_json_body(&mut s2).await {
                    captured
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(j2);
                }
                let bad_slot = "```json\n{\"route\":\"SINGLE_METRIC\",\"metric_names\":[\"paid_amount\"],\"target_node_type\":\"\",\"dimensions\":[],\"filter_exprs\":[],\"time_from\":\"\",\"time_to\":\"\",\"grain\":\"WEEKLY\",\"compare\":\"\",\"include_links\":[],\"clarification\":\"\"}\n```";
                let body = json!({
                    "id": "cmpl-2",
                    "model": "qwen2.5-7b-local",
                    "choices": [{ "message": { "role": "assistant", "content": bad_slot } }],
                    "usage": { "prompt_tokens": 18, "completion_tokens": 22 }
                })
                .to_string();
                write_http_response(&mut s2, 200, &body).await;
            }
            if let Ok((mut s3, _)) = listener.accept().await {
                if let Ok(j3) = read_http_json_body(&mut s3).await {
                    captured
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(j3);
                }
                let good_slot = json!({
                    "route": "SINGLE_METRIC",
                    "metric_names": ["paid_amount"],
                    "target_node_type": "Order",
                    "dimensions": [],
                    "filter_exprs": [],
                    "time_from": "2026-01-01",
                    "time_to": "2026-01-31",
                    "grain": "DAY",
                    "compare": "",
                    "include_links": [],
                    "clarification": ""
                })
                .to_string();
                let body = json!({
                    "id": "cmpl-3",
                    "model": "qwen2.5-7b-local",
                    "choices": [{ "message": { "role": "assistant", "content": good_slot } }],
                    "usage": { "prompt_tokens": 30, "completion_tokens": 24 }
                })
                .to_string();
                write_http_response(&mut s3, 200, &body).await;
            }
        });
        Ok(addr)
    }

    #[tokio::test]
    async fn local_small_model_falls_back_on_400_and_repairs_invalid_slot() -> Result<(), AppError>
    {
        let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_local_fallback_repair_stub(Arc::clone(&captured)).await?;

        let provider = sample_structured_provider(
            format!("http://{addr}/v1"),
            0.7,
            LlmStructuredCaps {
                structured_mode: StructuredOutputMode::JsonSchema,
                deterministic_seed: None,
                is_local_model: true,
                max_context_tokens: 16_384,
            },
        );
        let store = Arc::new(StructuredTestStore::default());
        store.set_providers("t_step15", vec![provider]);
        let store_dyn: Arc<dyn LlmProviderStore> = store.clone();

        let router = LlmRouter::new(dead_llm_config())
            .with_store(store_dyn)
            .with_cassette(None)
            .with_circuit_breaker(1, std::time::Duration::from_secs(60));

        let enums = TenantOntologyEnums {
            node_types: vec![String::from("Order")],
            edge_types: Vec::new(),
            metric_names: vec![String::from("paid_amount")],
            metric_dimensions: std::collections::BTreeMap::new(),
            dataset_names: vec![String::from("demo_orders")],
        };
        let slot_schema = JsonSchemaSpec::ontology_slot_extraction(&enums);

        let req = RouterTurnRequest::new(
            Purpose::Extract,
            vec![
                ToolChatMessage::system("Extract ontology slots."),
                ToolChatMessage::user("按天统计已支付订单金额"),
            ],
        )
        .with_response_schema(slot_schema);

        let turn = router.complete_tool_turn("t_step15", req).await?;
        let parsed: Value =
            serde_json::from_str(&turn.content).map_err(|e| AppError::Llm(e.to_string()))?;
        assert_eq!(parsed.get("grain"), Some(&json!("DAY")));
        assert_eq!(parsed.get("target_node_type"), Some(&json!("Order")));

        let wire_reqs = captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        assert_eq!(wire_reqs.len(), 3);

        // Attempt 1 used local_compact_schema (`filter_exprs`) + default seed 42 + temp 0.0
        let first = wire_reqs
            .first()
            .ok_or_else(|| AppError::Llm(String::from("missing req 1")))?;
        assert_eq!(
            first.pointer("/response_format/type"),
            Some(&json!("json_schema"))
        );
        assert!(
            first
                .pointer("/response_format/json_schema/schema/properties/filter_exprs")
                .is_some()
        );
        assert_eq!(first.get("seed"), Some(&json!(42)));
        assert_eq!(first.get("temperature"), Some(&json!(0.0)));

        // Attempt 2 downgraded to json_object
        let second = wire_reqs
            .get(1)
            .ok_or_else(|| AppError::Llm(String::from("missing req 2")))?;
        assert_eq!(
            second.pointer("/response_format/type"),
            Some(&json!("json_object"))
        );
        assert_eq!(second.get("temperature"), Some(&json!(0.0)));

        // Attempt 3 carried exact `/grain` repair prompt at temperature 0.0
        let third = wire_reqs
            .get(2)
            .ok_or_else(|| AppError::Llm(String::from("missing req 3")))?;
        let third_raw = third.to_string();
        assert!(third_raw.contains("/grain"));
        assert_eq!(third.get("temperature"), Some(&json!(0.0)));

        // Recorded 1 succeeded request with 0 errors (400 downgrade did not trip breaker)
        let usages = store.usage_snapshot();
        assert_eq!(usages.len(), 1);
        assert_eq!(usages.first().map(|u| u.errors), Some(0));
        Ok(())
    }

    #[test]
    fn context_budget_trimming_preserves_system_and_recent_tail() {
        let long_blob = "x".repeat(2_400);
        let messages = vec![
            ToolChatMessage::system("System ontology instructions"),
            ToolChatMessage::user(format!("Old turn 1 {long_blob}")),
            ToolChatMessage::assistant_with_tools(
                "",
                vec![ToolCallSpec {
                    id: String::from("call_old"),
                    name: String::from("ask_data"),
                    arguments: json!({"q": long_blob}),
                }],
            ),
            ToolChatMessage::tool_result("call_old", format!("Old tool output {long_blob}")),
            ToolChatMessage::user("Latest user question: 已支付订单总金额是多少？"),
        ];

        let trimmed = trim_messages_to_context_budget(&messages, 2_048, 800);
        assert!(trimmed.len() < messages.len());
        assert_eq!(trimmed.first().map(|m| m.role.as_str()), Some("system"));
        assert_eq!(
            trimmed.last().map(|m| m.content.as_str()),
            Some("Latest user question: 已支付订单总金额是多少？")
        );
        // Ensure no orphan `tool` message without its preceding `assistant` `tool_calls`
        for (idx, msg) in trimmed.iter().enumerate() {
            if msg.role == "tool" {
                let prev = idx.checked_sub(1).and_then(|p| trimmed.get(p));
                assert!(prev.is_some_and(|m| m.role == "assistant" || m.role == "tool"));
            }
        }

        let errs = validate_with_jsonschema(
            r#"{"status":"INVALID"}"#,
            &JsonSchemaSpec::connectivity_probe().schema,
        );
        assert!(errs.is_err());
        if let Err(paths) = errs {
            let app_err = unprocessable_structured_error(&paths);
            assert_eq!(app_err.error_code(), "STRUCTURED_OUTPUT_INVALID");
        }
    }

    #[test]
    fn flexible_schema_sets_strict_false_and_400_detector_matches_invalid_schema() {
        let provider = sample_structured_provider(
            String::from("http://127.0.0.1:9/v1"),
            0.2,
            LlmStructuredCaps {
                structured_mode: StructuredOutputMode::JsonSchema,
                deterministic_seed: Some(42),
                is_local_model: false,
                max_context_tokens: 32_768,
            },
        );

        let strict_rf =
            build_wire_response_format(&provider, &JsonSchemaSpec::connectivity_probe());
        assert!(matches!(
            strict_rf,
            Some(WireResponseFormat::JsonSchema { json_schema }) if json_schema.strict
        ));

        let flexible_spec = JsonSchemaSpec::new_flexible(
            "axagent_output",
            "Flexible agent output",
            json!({ "type": "object" }),
        );
        let flex_rf = build_wire_response_format(&provider, &flexible_spec);
        assert!(matches!(
            flex_rf,
            Some(WireResponseFormat::JsonSchema { json_schema }) if !json_schema.strict
        ));

        assert!(is_upstream_schema_unsupported_400(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"invalid_json_schema","message":"additionalProperties is required"}}"#
        ));
        assert!(is_upstream_schema_unsupported_400(
            StatusCode::BAD_REQUEST,
            "Invalid schema for response_format"
        ));
    }
}
