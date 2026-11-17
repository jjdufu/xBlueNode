//! `OpenAI`-compatible function/tool-calling wire types, transcript builders, and response converters.

use axpoint_domain::AppError;
use axpoint_domain::agent::ToolCallSpec;
use axpoint_domain::enums::Purpose;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `OpenAI`-compatible function schema nested inside [`ToolDefinition`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFunctionDef {
    /// Function identifier (`^[a-zA-Z0-9_-]{1,64}$`).
    pub name: String,
    /// Human-readable description guiding model tool selection.
    pub description: String,
    /// JSON Schema object describing accepted arguments.
    pub parameters: serde_json::Value,
}

/// `OpenAI`-compatible tool definition passed in `chat/completions` requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool discriminator (always `"function"`).
    pub r#type: String,
    /// Function metadata and parameter schema.
    pub function: ToolFunctionDef,
}

impl ToolDefinition {
    /// Constructs a `"function"` [`ToolDefinition`].
    #[must_use]
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            r#type: String::from("function"),
            function: ToolFunctionDef {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// Conversation message supporting `system`, `user`, `assistant` (with optional `tool_calls`),
/// and `tool` (with `tool_call_id` + `content`) roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolChatMessage {
    /// Message role (`"system"`, `"user"`, `"assistant"`, or `"tool"`).
    pub role: String,
    /// Text content (or serialized tool output when `role == "tool"`).
    #[serde(default)]
    pub content: String,
    /// Structured tool calls emitted by an `"assistant"` turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
    /// Correlated [`ToolCallSpec::id`] when `role == "tool"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ToolChatMessage {
    /// Constructs a `"system"` prompt message.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: String::from("system"),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Constructs a `"user"` prompt message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: String::from("user"),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Constructs a plain `"assistant"` text message without tool calls.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: String::from("assistant"),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Constructs an `"assistant"` message carrying one or more [`ToolCallSpec`] items.
    #[must_use]
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCallSpec>) -> Self {
        Self {
            role: String::from("assistant"),
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// Constructs a `"tool"` result message correlated to `tool_call_id`.
    #[must_use]
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: String::from("tool"),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// Converts this domain message into an `OpenAI` wire-format message object.
    #[must_use]
    pub fn to_wire(&self) -> WireRequestMessage {
        let wire_calls: Vec<WireToolCall> =
            self.tool_calls.iter().map(tool_call_spec_to_wire).collect();
        let content = if self.content.is_empty() && !wire_calls.is_empty() {
            None
        } else {
            Some(self.content.clone())
        };
        WireRequestMessage {
            role: self.role.clone(),
            content,
            tool_calls: wire_calls,
            tool_call_id: self.tool_call_id.clone(),
        }
    }
}

/// Token consumption reported by an `OpenAI`-compatible completion or final SSE chunk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of input/prompt tokens billed.
    #[serde(default)]
    pub prompt_tokens: i64,
    /// Number of output/completion tokens generated.
    #[serde(default)]
    pub completion_tokens: i64,
}

/// Assembled assistant generation turn (content, parsed tool calls, usage, and model echo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolChatTurn {
    /// Assistant text content (may be empty when only `tool_calls` are emitted).
    pub content: String,
    /// Parsed tool calls requested by the model.
    pub tool_calls: Vec<ToolCallSpec>,
    /// Prompt and completion token usage counters.
    pub usage: TokenUsage,
    /// Upstream echoed `model` identifier, if returned by the provider.
    pub model_echo: Option<String>,
}

/// `OpenAI` wire-format `function` payload inside a `tool_calls` element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFunctionCall {
    /// Invoked function name.
    pub name: String,
    /// Raw JSON-encoded arguments string.
    #[serde(default)]
    pub arguments: String,
}

/// `OpenAI` wire-format item inside `message.tool_calls`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireToolCall {
    /// Tool call identifier assigned by the model/provider.
    #[serde(default)]
    pub id: String,
    /// Tool call type (defaults to `"function"`).
    #[serde(default = "default_function_type")]
    pub r#type: String,
    /// Function name and serialized arguments.
    pub function: WireFunctionCall,
}

fn default_function_type() -> String {
    String::from("function")
}

/// `OpenAI` wire-format message serialized in outbound `chat/completions` requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireRequestMessage {
    /// Role (`"system"`, `"user"`, `"assistant"`, or `"tool"`).
    pub role: String,
    /// Text content (`null` when an assistant message contains only `tool_calls`).
    pub content: Option<String>,
    /// Outbound assistant tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<WireToolCall>,
    /// Correlated tool call ID when `role == "tool"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// `OpenAI` wire-format `stream_options` object requesting final chunk token usage.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct WireStreamOptions {
    /// Whether the final SSE chunk before `[DONE]` includes a `usage` object.
    pub include_usage: bool,
}

/// Outbound `OpenAI` `POST /chat/completions` JSON request body.
#[derive(Debug, Clone, Serialize)]
pub struct WireChatCompletionRequest {
    /// Target model identifier.
    pub model: String,
    /// Conversation message list.
    pub messages: Vec<WireRequestMessage>,
    /// Sampling temperature.
    pub temperature: f32,
    /// Optional maximum completion tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    /// Optional function tools exposed to the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// Enable SSE streaming when `Some(true)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Optional stream options (`include_usage: true` when streaming).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<WireStreamOptions>,
}

/// `OpenAI` wire-format `choices[i].message` in non-streaming `chat/completions` responses.
#[derive(Debug, Clone, Deserialize)]
pub struct WireResponseMessage {
    /// Assistant text content (`null` when omitted or tool-only).
    #[serde(default)]
    pub content: Option<String>,
    /// Optional list of tool calls requested by the assistant.
    #[serde(default)]
    pub tool_calls: Option<Vec<WireToolCall>>,
}

/// `OpenAI` wire-format `choices[i]` item in non-streaming `chat/completions` responses.
#[derive(Debug, Clone, Deserialize)]
pub struct WireResponseChoice {
    /// Assistant response message.
    pub message: WireResponseMessage,
}

/// Inbound `OpenAI` non-streaming `POST /chat/completions` JSON response body.
#[derive(Debug, Clone, Deserialize)]
pub struct WireChatCompletionResponse {
    /// Echoed model identifier.
    #[serde(default)]
    pub model: Option<String>,
    /// Completion choices.
    #[serde(default)]
    pub choices: Vec<WireResponseChoice>,
    /// Optional token usage summary.
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

/// Converts a domain [`ToolCallSpec`] into an `OpenAI` [`WireToolCall`].
#[must_use]
pub fn tool_call_spec_to_wire(spec: &ToolCallSpec) -> WireToolCall {
    let arguments = serde_json::to_string(&spec.arguments).unwrap_or_else(|_| String::from("{}"));
    WireToolCall {
        id: spec.id.clone(),
        r#type: String::from("function"),
        function: WireFunctionCall {
            name: spec.name.clone(),
            arguments,
        },
    }
}

/// Parses a raw `OpenAI` `function.arguments` JSON string into a [`serde_json::Value`].
///
/// Empty or whitespace-only strings normalize to `{}`.
///
/// # Errors
/// Returns [`AppError::Llm`] if `raw` is non-empty and not valid JSON.
pub fn parse_tool_arguments(raw: &str) -> Result<serde_json::Value, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(trimmed)
        .map_err(|err| AppError::Llm(format!("invalid tool call arguments JSON: {err}")))
}

/// Converts `OpenAI` `choices[0].message.tool_calls` items into domain [`ToolCallSpec`] values.
///
/// Synthesizes a fallback call ID if an upstream provider omits `id`.
///
/// # Errors
/// Returns [`AppError::Llm`] if any tool call has an empty function name or malformed JSON arguments.
pub fn convert_wire_tool_calls(
    wire_calls: Vec<WireToolCall>,
) -> Result<Vec<ToolCallSpec>, AppError> {
    let mut specs = Vec::with_capacity(wire_calls.len());
    for (idx, wc) in wire_calls.into_iter().enumerate() {
        let fn_name = wc.function.name.trim().to_owned();
        if fn_name.is_empty() {
            return Err(AppError::Llm(format!(
                "tool call at index {idx} has empty function name"
            )));
        }
        let call_id = if wc.id.trim().is_empty() {
            format!("call_{}_{idx}", Uuid::new_v4().simple())
        } else {
            wc.id
        };
        let arguments = parse_tool_arguments(&wc.function.arguments)?;
        specs.push(ToolCallSpec {
            id: call_id,
            name: fn_name,
            arguments,
        });
    }
    Ok(specs)
}

/// Converts a non-streaming [`WireChatCompletionResponse`] into a [`ToolChatTurn`].
///
/// # Errors
/// Returns [`AppError::Llm`] if `choices` is empty or any `tool_calls` entry contains invalid JSON.
pub fn parse_chat_completion_response(
    resp: WireChatCompletionResponse,
) -> Result<ToolChatTurn, AppError> {
    let first = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Llm(String::from("chat API returned zero choices")))?;
    let content = first.message.content.unwrap_or_default();
    let raw_calls = first.message.tool_calls.unwrap_or_default();
    let tool_calls = convert_wire_tool_calls(raw_calls)?;
    let model_echo = resp.model.filter(|m| !m.trim().is_empty());
    Ok(ToolChatTurn {
        content,
        tool_calls,
        usage: resp.usage.unwrap_or_default(),
        model_echo,
    })
}

/// Builds a deterministic canonical request transcript from `(purpose, messages, tools)`
/// for offline [`crate::Cassette`] SHA-256 hashing and `match_all` / `exclude_any` matching.
#[must_use]
pub fn canonical_request_transcript(
    purpose: Purpose,
    messages: &[ToolChatMessage],
    tools: &[ToolDefinition],
) -> String {
    let mut out = format!("PURPOSE:{purpose}\n");
    if !tools.is_empty() {
        let mut names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
        names.sort_unstable();
        out.push_str("TOOLS:");
        out.push_str(&names.join(","));
        out.push('\n');
    }
    for msg in messages {
        let role = msg.role.trim().to_ascii_lowercase();
        out.push_str("ROLE:");
        out.push_str(&role);
        if let Some(tc_id) = &msg.tool_call_id {
            out.push_str(" tool_call_id=");
            out.push_str(tc_id);
        }
        out.push('\n');
        if !msg.content.is_empty() {
            out.push_str(&msg.content);
            out.push('\n');
        }
        for tc in &msg.tool_calls {
            let args = serde_json::to_string(&tc.arguments).unwrap_or_else(|_| String::from("{}"));
            out.push_str("TOOL_CALL:");
            out.push_str(&tc.id);
            out.push(':');
            out.push_str(&tc.name);
            out.push(':');
            out.push_str(&args);
            out.push('\n');
        }
        out.push_str("---\n");
    }
    out
}

/// Extracts `(system_prompt, last_user_prompt)` from a slice of [`ToolChatMessage`] items.
#[must_use]
pub fn extract_system_and_last_user(messages: &[ToolChatMessage]) -> (String, String) {
    let system_prompt = messages
        .iter()
        .find(|m| m.role.eq_ignore_ascii_case("system"))
        .map_or_else(String::new, |m| m.content.clone());
    let last_user_prompt = messages
        .iter()
        .rev()
        .find(|m| m.role.eq_ignore_ascii_case("user"))
        .map_or_else(String::new, |m| m.content.clone());
    (system_prompt, last_user_prompt)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn converts_openai_wire_tool_calls_and_roundtrips_transcript() -> Result<(), AppError> {
        let wire = WireChatCompletionResponse {
            model: Some(String::from("gpt-4o-mini")),
            choices: vec![WireResponseChoice {
                message: WireResponseMessage {
                    content: None,
                    tool_calls: Some(vec![
                        WireToolCall {
                            id: String::from("call_abc"),
                            r#type: String::from("function"),
                            function: WireFunctionCall {
                                name: String::from("ask_data"),
                                arguments: String::from(r#"{"question":"已支付订单总金额"}"#),
                            },
                        },
                        WireToolCall {
                            id: String::new(),
                            r#type: String::from("function"),
                            function: WireFunctionCall {
                                name: String::from("list_datasets"),
                                arguments: String::new(),
                            },
                        },
                    ]),
                },
            }],
            usage: Some(TokenUsage {
                prompt_tokens: 42,
                completion_tokens: 18,
            }),
        };

        let turn = parse_chat_completion_response(wire)?;
        assert_eq!(turn.content, "");
        assert_eq!(turn.model_echo.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(turn.usage.prompt_tokens, 42);
        assert_eq!(turn.usage.completion_tokens, 18);
        assert_eq!(turn.tool_calls.len(), 2);

        let first = turn
            .tool_calls
            .first()
            .ok_or_else(|| AppError::Llm(String::from("missing first call")))?;
        assert_eq!(first.id, "call_abc");
        assert_eq!(first.name, "ask_data");
        assert_eq!(first.arguments, json!({"question": "已支付订单总金额"}));

        let second = turn
            .tool_calls
            .get(1)
            .ok_or_else(|| AppError::Llm(String::from("missing second call")))?;
        assert!(second.id.starts_with("call_"));
        assert_eq!(second.name, "list_datasets");
        assert_eq!(second.arguments, json!({}));

        let msgs = vec![
            ToolChatMessage::system("You are ops_supervisor."),
            ToolChatMessage::user("查询已支付金额"),
            ToolChatMessage::assistant_with_tools("", vec![first.clone()]),
            ToolChatMessage::tool_result("call_abc", r#"{"rows":[{"paid_amount":33239.25}]}"#),
        ];
        let tools = vec![ToolDefinition::function(
            "ask_data",
            "Ask data",
            json!({"type":"object"}),
        )];
        let transcript = canonical_request_transcript(Purpose::Agent, &msgs, &tools);
        assert!(transcript.contains("PURPOSE:AGENT"));
        assert!(transcript.contains("TOOLS:ask_data"));
        assert!(transcript.contains("ROLE:tool tool_call_id=call_abc"));
        Ok(())
    }
}
