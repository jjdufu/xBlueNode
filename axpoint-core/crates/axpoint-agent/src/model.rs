//! Decoupled tool-calling model port (`AgentModelPort`) and offline `JSONL` cassette adapter (`08-STEP09` §09.2 & §09.5).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use axpoint_domain::AppError;
use axpoint_domain::agent::ToolCallSpec;
use axpoint_domain::enums::Purpose;
use axpoint_domain::platform::BoxFuture;
use axpoint_llm::sha256_hex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Single chat message passed to [`AgentModelPort::complete_turn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ModelMessage {
    /// System prompt or step-exhaustion instruction.
    System {
        /// Instruction text.
        content: String,
    },
    /// End-user or coordinator task message.
    User {
        /// Message text.
        content: String,
    },
    /// Assistant message containing text and/or requested tool calls.
    Assistant {
        /// Text content (may be empty when tool calls are emitted).
        #[serde(default)]
        content: String,
        /// Tool invocations requested by the model.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCallSpec>,
    },
    /// Tool execution output linked to a preceding `ToolCallSpec.id`.
    Tool {
        /// Originating tool call identifier.
        tool_call_id: String,
        /// Serialized (and possibly truncated) tool output text.
        content: String,
    },
}

/// Tool definition advertised to the model during tool-calling turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelToolDef {
    /// Unique function/tool name matching `^[a-zA-Z0-9_-]{1,64}$`.
    pub name: String,
    /// Human-readable description guiding model tool selection.
    pub description: String,
    /// JSON Schema object describing accepted tool arguments.
    pub parameters: serde_json::Value,
}

/// Model completion request dispatched by `AgentRuntime` through [`AgentModelPort`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelRequest {
    /// Active tenant identifier (for provider routing and usage accounting).
    pub tenant_id: String,
    /// Workload purpose (`Purpose::Agent`, `Purpose::Extract`, etc.).
    pub purpose: Purpose,
    /// Optional explicit `ax_llm_providers.id` pinned by `AgentSpec`.
    pub provider_id: Option<Uuid>,
    /// Sampling temperature (`0.0..=2.0`).
    pub temperature: f32,
    /// Ordered conversation context messages.
    pub messages: Vec<ModelMessage>,
    /// Visible tools permitted for the current caller and step.
    pub tools: Vec<ModelToolDef>,
    /// Optional final structured output JSON Schema (`response_format`).
    pub output_schema: Option<serde_json::Value>,
}

/// Model completion response returned by [`AgentModelPort::complete_turn`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelTurn {
    /// Assistant text content.
    #[serde(default)]
    pub content: String,
    /// Tool invocations requested by the model (empty when final text is produced).
    #[serde(default)]
    pub tool_calls: Vec<ToolCallSpec>,
    /// Prompt tokens consumed on this turn.
    #[serde(default)]
    pub prompt_tokens: i64,
    /// Completion tokens generated on this turn.
    #[serde(default)]
    pub completion_tokens: i64,
}

/// Object-safe model completion port decoupling `axpoint-agent` from `axpoint-llm::router::LlmRouter`.
pub trait AgentModelPort: Send + Sync {
    /// Executes a single chat/tool-calling completion turn, optionally streaming text deltas into `token_tx`.
    fn complete_turn(
        &self,
        req: AgentModelRequest,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> BoxFuture<'_, AgentModelTurn>;
}

/// Computes a deterministic `SHA-256` hex digest for an [`AgentModelRequest`]'s `(purpose, messages, tool_names)`.
#[must_use]
pub fn compute_agent_request_sha256(req: &AgentModelRequest) -> String {
    let tool_names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    let canonical = serde_json::json!({
        "purpose": req.purpose,
        "messages": &req.messages,
        "tools": tool_names,
    });
    sha256_hex(&canonical.to_string())
}

/// Single `JSONL` record consumed by [`CassetteModelAdapter`] for deterministic offline agent testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCassetteEntry {
    /// Exact `SHA-256` digest of `(purpose, messages, tool_names)` via [`compute_agent_request_sha256`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_sha256: Option<String>,
    /// Optional list of substrings that must ALL appear in the serialized request messages/tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_all: Vec<String>,
    /// Optional single substring fallback matched against the serialized request messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_substring: Option<String>,
    /// Optional requirement on the role of the latest message (`"user"`, `"tool"`, `"system"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_role: Option<String>,
    /// Recorded structured [`AgentModelTurn`] response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<AgentModelTurn>,
    /// Shorthand assistant text response when `turn` is omitted (compatible with `axpoint-llm` cassettes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
}

impl AgentCassetteEntry {
    /// Resolves this entry into an [`AgentModelTurn`].
    #[must_use]
    pub fn into_turn(self) -> AgentModelTurn {
        if let Some(turn) = self.turn {
            turn
        } else {
            AgentModelTurn {
                content: self.response.unwrap_or_default(),
                tool_calls: Vec::new(),
                prompt_tokens: 16,
                completion_tokens: 12,
            }
        }
    }
}

#[derive(Debug, Default)]
struct CassetteMatcherState {
    by_hash: HashMap<String, AgentModelTurn>,
    rules: Vec<AgentCassetteEntry>,
    recorded_requests: Vec<AgentModelRequest>,
}

/// Deterministic offline [`AgentModelPort`] replaying recorded tool calls and assistant answers without network I/O.
#[derive(Debug, Clone, Default)]
pub struct CassetteModelAdapter {
    state: Arc<Mutex<CassetteMatcherState>>,
}

impl CassetteModelAdapter {
    /// Creates an empty [`CassetteModelAdapter`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a [`CassetteModelAdapter`] from an in-memory sequence of [`AgentCassetteEntry`] items.
    #[must_use]
    pub fn from_entries<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = AgentCassetteEntry>,
    {
        let adapter = Self::new();
        for entry in entries {
            adapter.insert_entry(entry);
        }
        adapter
    }

    /// Loads a [`CassetteModelAdapter`] from a `.jsonl` file on disk.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if the file cannot be read or contains malformed JSON lines.
    pub fn from_jsonl_path(path: &Path) -> Result<Self, AppError> {
        let raw = fs::read_to_string(path).map_err(|err| {
            AppError::Llm(format!(
                "failed to read agent cassette {}: {err}",
                path.display()
            ))
        })?;
        Self::from_jsonl_str(&raw)
    }

    /// Parses a [`CassetteModelAdapter`] from a `JSONL` string.
    ///
    /// # Errors
    /// Returns [`AppError::Llm`] if any non-comment line fails JSON deserialization.
    pub fn from_jsonl_str(raw: &str) -> Result<Self, AppError> {
        let adapter = Self::new();
        for (idx, raw_line) in raw.lines().enumerate() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let line_no = idx.saturating_add(1);
            let entry: AgentCassetteEntry = serde_json::from_str(trimmed).map_err(|err| {
                AppError::Llm(format!(
                    "invalid agent cassette JSON at line {line_no}: {err}"
                ))
            })?;
            adapter.insert_entry(entry);
        }
        Ok(adapter)
    }

    /// Inserts a single [`AgentCassetteEntry`] into this adapter.
    pub fn insert_entry(&self, entry: AgentCassetteEntry) {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(hash) = entry
            .request_sha256
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            guard
                .by_hash
                .insert(hash.to_ascii_lowercase(), entry.clone().into_turn());
        }
        if !entry.match_all.is_empty() || entry.match_substring.is_some() {
            guard.rules.push(entry);
        }
    }

    /// Returns a snapshot of all [`AgentModelRequest`]s observed by this adapter (useful for test assertions).
    #[must_use]
    pub fn recorded_requests(&self) -> Vec<AgentModelRequest> {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.recorded_requests.clone()
    }

    fn resolve_turn(&self, req: &AgentModelRequest) -> Result<AgentModelTurn, AppError> {
        let req_hash = compute_agent_request_sha256(req);
        let serialized_haystack = serde_json::json!({
            "messages": &req.messages,
            "tools": req.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
        })
        .to_string();
        let last_role_str = req.messages.last().map(message_role_label);

        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.recorded_requests.push(req.clone());

        if let Some(turn) = guard.by_hash.get(&req_hash) {
            return Ok(turn.clone());
        }

        for rule in &guard.rules {
            if let Some(expected_role) = rule.last_role.as_deref()
                && last_role_str != Some(expected_role)
            {
                continue;
            }
            if !rule.match_all.is_empty() {
                let all_match = rule
                    .match_all
                    .iter()
                    .all(|pat| serialized_haystack.contains(pat.as_str()));
                if all_match {
                    return Ok(rule.clone().into_turn());
                }
            }
        }

        for rule in &guard.rules {
            if let Some(expected_role) = rule.last_role.as_deref()
                && last_role_str != Some(expected_role)
            {
                continue;
            }
            if let Some(sub) = rule.match_substring.as_deref()
                && serialized_haystack.contains(sub)
            {
                return Ok(rule.clone().into_turn());
            }
        }

        Err(AppError::Llm(format!(
            "agent cassette miss for request_sha256={req_hash}"
        )))
    }
}

const fn message_role_label(msg: &ModelMessage) -> &'static str {
    match msg {
        ModelMessage::System { .. } => "system",
        ModelMessage::User { .. } => "user",
        ModelMessage::Assistant { .. } => "assistant",
        ModelMessage::Tool { .. } => "tool",
    }
}

impl AgentModelPort for CassetteModelAdapter {
    fn complete_turn(
        &self,
        req: AgentModelRequest,
        token_tx: Option<mpsc::Sender<String>>,
    ) -> BoxFuture<'_, AgentModelTurn> {
        Box::pin(async move {
            let turn = self.resolve_turn(&req)?;
            if let Some(tx) = token_tx
                && !turn.content.is_empty()
            {
                // Stream deterministic chunks so SSE token delta consumers exercise multi-chunk assembly.
                for chunk in split_token_chunks(&turn.content, 24) {
                    if tx.send(chunk).await.is_err() {
                        break;
                    }
                }
            }
            Ok(turn)
        })
    }
}

fn split_token_chunks(content: &str, max_chars: usize) -> Vec<String> {
    let step = max_chars.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0_usize;
    for ch in content.chars() {
        current.push(ch);
        count = count.saturating_add(1);
        if count >= step {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use axpoint_domain::AppError;
    use axpoint_domain::agent::ToolCallSpec;
    use axpoint_domain::enums::Purpose;
    use tokio::sync::mpsc;

    use super::{
        AgentModelPort, AgentModelRequest, AgentModelTurn, CassetteModelAdapter, ModelMessage,
    };

    #[tokio::test]
    async fn cassette_adapter_matches_rules_and_streams_tokens() -> Result<(), AppError> {
        let jsonl = r#"
{"match_all":["库存告警"],"last_role":"user","turn":{"content":"","tool_calls":[{"id":"call_1","name":"ack_alert","arguments":{"alert_id":"00000000-0000-0000-0000-000000000001"}}],"prompt_tokens":20,"completion_tokens":10}}
{"match_all":["库存告警"],"last_role":"tool","turn":{"content":"告警已确认完毕，原因为安全库存低于阈值。","tool_calls":[],"prompt_tokens":35,"completion_tokens":18}}
"#;
        let adapter = CassetteModelAdapter::from_jsonl_str(jsonl)?;

        let req1 = AgentModelRequest {
            tenant_id: String::from("default"),
            purpose: Purpose::Agent,
            provider_id: None,
            temperature: 0.2,
            messages: vec![ModelMessage::User {
                content: String::from("请处理库存告警"),
            }],
            tools: Vec::new(),
            output_schema: None,
        };
        let turn1 = adapter.complete_turn(req1, None).await?;
        assert_eq!(turn1.tool_calls.len(), 1);
        assert_eq!(
            turn1.tool_calls.first().map(|c| c.name.as_str()),
            Some("ack_alert")
        );

        let (tx, mut rx) = mpsc::channel(8);
        let req2 = AgentModelRequest {
            tenant_id: String::from("default"),
            purpose: Purpose::Agent,
            provider_id: None,
            temperature: 0.2,
            messages: vec![
                ModelMessage::User {
                    content: String::from("请处理库存告警"),
                },
                ModelMessage::Assistant {
                    content: String::new(),
                    tool_calls: vec![ToolCallSpec {
                        id: String::from("call_1"),
                        name: String::from("ack_alert"),
                        arguments: serde_json::json!({}),
                    }],
                },
                ModelMessage::Tool {
                    tool_call_id: String::from("call_1"),
                    content: String::from("{\"status\":\"ACK\"}"),
                },
            ],
            tools: Vec::new(),
            output_schema: None,
        };
        let turn2: AgentModelTurn = adapter.complete_turn(req2, Some(tx)).await?;
        let mut streamed = String::new();
        while let Some(delta) = rx.recv().await {
            streamed.push_str(&delta);
        }
        assert_eq!(streamed, turn2.content);
        assert!(turn2.content.contains("告警已确认"));
        assert_eq!(adapter.recorded_requests().len(), 2);
        Ok(())
    }
}
