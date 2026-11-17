//! `OpenAI`-compatible Server-Sent Events (`SSE`) streaming completion parser.

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::agent::ToolCallSpec;
use eventsource_stream::Eventsource as _;
use futures::{Stream, StreamExt as _};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::warn;
use uuid::Uuid;

use crate::tools::{TokenUsage, ToolChatTurn, parse_tool_arguments};

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments_raw: String,
}

#[derive(Debug, Deserialize)]
struct SseFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SseFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct SseMessageDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<SseToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct SseChoiceDelta {
    #[serde(default)]
    delta: SseMessageDelta,
}

#[derive(Debug, Deserialize)]
struct SseCompletionChunk {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<SseChoiceDelta>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Debug, Default)]
struct SseAccumulator {
    content: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    usage: TokenUsage,
    model_echo: Option<String>,
}

impl SseAccumulator {
    async fn apply_chunk(
        &mut self,
        chunk: SseCompletionChunk,
        token_tx: Option<&mpsc::Sender<String>>,
    ) {
        if let Some(model) = chunk.model.filter(|m| !m.trim().is_empty()) {
            self.model_echo = Some(model);
        }
        if let Some(usage) = chunk.usage {
            self.usage = usage;
        }
        let Some(choice) = chunk.choices.into_iter().next() else {
            return;
        };
        if let Some(delta_text) = choice.delta.content
            && !delta_text.is_empty()
        {
            self.content.push_str(&delta_text);
            if let Some(tx) = token_tx
                && tx.send(delta_text).await.is_err()
            {
                warn!("SSE token delta receiver closed before stream completion");
            }
        }
        if let Some(tc_deltas) = choice.delta.tool_calls {
            for tc in tc_deltas {
                let entry = self.tool_calls.entry(tc.index).or_default();
                if let Some(id_part) = tc.id
                    && !id_part.is_empty()
                {
                    if entry.id.is_empty() {
                        entry.id = id_part;
                    } else {
                        entry.id.push_str(&id_part);
                    }
                }
                if let Some(fn_delta) = tc.function {
                    if let Some(name_part) = fn_delta.name {
                        entry.name.push_str(&name_part);
                    }
                    if let Some(arg_part) = fn_delta.arguments {
                        entry.arguments_raw.push_str(&arg_part);
                    }
                }
            }
        }
    }

    fn finish(self) -> Result<ToolChatTurn, AppError> {
        let mut specs = Vec::with_capacity(self.tool_calls.len());
        for (idx, partial) in self.tool_calls {
            let name = partial.name.trim().to_owned();
            if name.is_empty() {
                return Err(AppError::Llm(format!(
                    "streaming tool call at index {idx} missing function name"
                )));
            }
            let id = if partial.id.trim().is_empty() {
                format!("call_{}_{idx}", Uuid::new_v4().simple())
            } else {
                partial.id
            };
            let arguments = parse_tool_arguments(&partial.arguments_raw)?;
            specs.push(ToolCallSpec {
                id,
                name,
                arguments,
            });
        }
        Ok(ToolChatTurn {
            content: self.content,
            tool_calls: specs,
            usage: self.usage,
            model_echo: self.model_echo,
        })
    }
}

/// Consumes an `OpenAI`-compatible `SSE` byte stream (`data: {...}` / `data: [DONE]`),
/// forwarding text deltas to `token_tx` and returning the assembled [`ToolChatTurn`].
///
/// # Errors
/// Returns [`AppError::Llm`] if the byte stream yields an I/O error, invalid UTF-8 `SSE` framing,
/// malformed chunk JSON, or unparseable accumulated `function.arguments` JSON.
pub async fn consume_sse_byte_stream<S, B, E>(
    byte_stream: S,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<ToolChatTurn, AppError>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut event_stream = Box::pin(byte_stream.eventsource());
    let mut acc = SseAccumulator::default();

    while let Some(item) = event_stream.next().await {
        let ev = item.map_err(|err| AppError::Llm(format!("SSE stream framing error: {err}")))?;
        let data = ev.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            break;
        }
        let chunk: SseCompletionChunk = serde_json::from_str(data)
            .map_err(|err| AppError::Llm(format!("invalid SSE completion chunk JSON: {err}")))?;
        acc.apply_chunk(chunk, token_tx.as_ref()).await;
    }

    acc.finish()
}

/// Consumes an `OpenAI`-compatible `reqwest::Response` `SSE` stream into a [`ToolChatTurn`],
/// emitting incremental assistant `content` chunks over `token_tx` if provided.
///
/// # Errors
/// Returns [`AppError::Llm`] if reading the HTTP body stream or parsing `SSE` JSON fails.
pub async fn consume_sse_response(
    resp: reqwest::Response,
    token_tx: Option<mpsc::Sender<String>>,
) -> Result<ToolChatTurn, AppError> {
    consume_sse_byte_stream(resp.bytes_stream(), token_tx).await
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn parses_streamed_content_and_chunked_tool_calls_until_done() -> Result<(), AppError> {
        let frames: Vec<Result<&'static [u8], Infallible>> = vec![
            Ok(b"data: {\"model\":\"qwen-plus\",\"choices\":[{\"delta\":{\"content\":\"\xE6\xAD\xA3\xE5\x9C\xA8\"}}]}\n\n"),
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"\xE6\x9F\xA5\xE8\xAF\xA2...\"}}]}\n\n"),
            Ok(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"ask_\",\"arguments\":\"{\\\"quest\"}}]}}]}\n\n"),
            Ok(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"search_knowledge\",\"arguments\":\"{\\\"query\\\":\\\"\xE8\xA1\xA5\xE8\xB4\xA7\\\"}\"}}]}}]}\n\n"),
            Ok(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"data\",\"arguments\":\"ion\\\":\\\"\xE9\x87\x91\xE9\xA2\x9D\\\"}\"}}]}}],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":35}}\n\n"),
            Ok(b"data: [DONE]\n\n"),
            Ok(b"data: {\"choices\":[{\"delta\":{\"content\":\"ignored_after_done\"}}]}\n\n"),
        ];

        let (tx, mut rx) = mpsc::channel(8);
        let turn = consume_sse_byte_stream(futures::stream::iter(frames), Some(tx)).await?;

        let mut collected = Vec::new();
        while let Some(tok) = rx.recv().await {
            collected.push(tok);
        }

        assert_eq!(collected, vec!["正在", "查询..."]);
        assert_eq!(turn.content, "正在查询...");
        assert_eq!(turn.model_echo.as_deref(), Some("qwen-plus"));
        assert_eq!(turn.usage.prompt_tokens, 120);
        assert_eq!(turn.usage.completion_tokens, 35);
        assert_eq!(turn.tool_calls.len(), 2);

        let tc0 = turn
            .tool_calls
            .first()
            .ok_or_else(|| AppError::Llm(String::from("missing tc0")))?;
        assert_eq!(tc0.id, "call_1");
        assert_eq!(tc0.name, "ask_data");
        assert_eq!(tc0.arguments, json!({"question": "金额"}));

        let tc1 = turn
            .tool_calls
            .get(1)
            .ok_or_else(|| AppError::Llm(String::from("missing tc1")))?;
        assert_eq!(tc1.id, "call_2");
        assert_eq!(tc1.name, "search_knowledge");
        assert_eq!(tc1.arguments, json!({"query": "补货"}));
        Ok(())
    }
}
