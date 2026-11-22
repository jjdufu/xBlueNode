//! Throttled IM progress-card hints (`>= 800ms`) and Markdown-safe message chunking (`07-STEP21` §21.2.3).

use std::time::{Duration, Instant};

use axpoint_domain::agent::AgentEvent;
use axpoint_domain::im::ImOutboundMessage;

pub use axpoint_domain::im::{
    IM_MARKDOWN_MAX_CHUNK_BYTES, IM_PROGRESS_THROTTLE_MS, split_markdown_chunks,
};

/// Rate-limits intermediate [`AgentEvent`] progress hints to at least `IM_PROGRESS_THROTTLE_MS` (`800ms`)
/// between consecutive updates on the same IM turn (`07-STEP21` §21.2.3).
#[derive(Debug)]
pub struct ProgressThrottler {
    min_interval: Duration,
    last_emitted_at: Option<Instant>,
    last_hint: String,
}

impl Default for ProgressThrottler {
    fn default() -> Self {
        Self::new(Duration::from_millis(IM_PROGRESS_THROTTLE_MS))
    }
}

impl ProgressThrottler {
    /// Creates a new [`ProgressThrottler`] enforcing `min_interval` between emitted hints.
    #[must_use]
    pub const fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_emitted_at: None,
            last_hint: String::new(),
        }
    }

    /// Evaluates `event` at `now`, returning a non-final [`ImOutboundMessage`] when a distinct
    /// progress hint is ready to emit without violating `min_interval`.
    pub fn on_event(
        &mut self,
        event: &AgentEvent,
        chat_id: &str,
        reply_token: Option<&str>,
        stream_id: Option<&str>,
        now: Instant,
    ) -> Option<ImOutboundMessage> {
        let hint = progress_hint_for_event(event)?;
        let is_approval = matches!(event, AgentEvent::ApprovalRequired { .. });
        if !is_approval
            && let Some(prev) = self.last_emitted_at
            && now.saturating_duration_since(prev) < self.min_interval
        {
            return None;
        }
        if self.last_hint == hint && !is_approval {
            return None;
        }
        self.last_emitted_at = Some(now);
        self.last_hint = String::from(hint);
        Some(ImOutboundMessage {
            chat_id: String::from(chat_id),
            text: String::from(hint),
            reply_token: reply_token.map(String::from),
            stream_id: stream_id.map(String::from),
            is_final: false,
        })
    }
}

/// Maps an [`AgentEvent`] to its terse status indicator (`07-STEP21` §21.2.3).
#[must_use]
pub fn progress_hint_for_event(event: &AgentEvent) -> Option<&'static str> {
    match event {
        AgentEvent::ToolCall { name, .. } => match name.as_str() {
            "search_knowledge" | "search_nodes" | "get_node" | "graph_neighbors" => {
                Some("检索知识库…")
            }
            "ask_data" | "ask_analytics" | "run_sql" | "query_sql" | "query_metric"
            | "query_objects" | "node_timeseries" | "node_metric_series" => Some("执行受控问数…"),
            "run_simulation" => Some("执行推演计算…"),
            _ => Some("执行工具调用…"),
        },
        AgentEvent::ApprovalRequired { .. } => Some("等待人工审批…"),
        AgentEvent::Token { .. }
        | AgentEvent::ToolResult { .. }
        | AgentEvent::Final { .. }
        | AgentEvent::Error { .. } => None,
    }
}
