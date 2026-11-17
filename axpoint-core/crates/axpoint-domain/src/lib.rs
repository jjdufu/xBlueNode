//! Domain types, error definitions, and five-way RRF fusion logic for `AxPoint`.
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub mod agent;
pub mod algo;
pub mod analytics;
pub mod axhub;
pub mod enums;
pub mod expr;
pub mod flow;
pub mod graph;
pub mod platform;
pub mod ports;
pub mod rollup;
pub mod rules;
pub mod studio;
pub mod topology;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Unified application error type across `AxPoint` crates.
///
/// Error codes follow `docs/blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md` §5.
#[derive(Debug, Error)]
pub enum AppError {
    /// Client sent invalid parameters (`HTTP 400`).
    #[error("{code}: {message}")]
    BadRequest {
        /// Stable machine-readable error code, e.g., `INVALID_ARGUMENT`.
        code: &'static str,
        /// Human-readable diagnostic message.
        message: String,
    },

    /// Missing or invalid authentication credentials (`HTTP 401`).
    #[error("{0}")]
    Unauthorized(&'static str),

    /// Permission or policy check denied (`HTTP 403`).
    #[error("{0}")]
    Forbidden(&'static str),

    /// Requested resource does not exist or belongs to another tenant (`HTTP 404`).
    #[error("{0}")]
    NotFound(&'static str),

    /// Uniqueness, version, or state conflict (`HTTP 409`).
    #[error("{0}")]
    Conflict(&'static str),

    /// Request body exceeds limit or archive compression ratio unsafe (`HTTP 413`).
    #[error("{0}")]
    PayloadTooLarge(&'static str),

    /// Unsupported media or file format (`HTTP 415`).
    #[error("{0}")]
    Unsupported(&'static str),

    /// Semantically unprocessable payload (`HTTP 422`).
    #[error("{code}: {message}")]
    Unprocessable {
        /// Stable machine-readable error code, e.g., `PARSE_FAILED`.
        code: &'static str,
        /// Human-readable diagnostic message.
        message: String,
    },

    /// Rate limit exceeded (`HTTP 429`).
    #[error("rate limited")]
    RateLimited,

    /// Query or upstream call timed out (`HTTP 504`).
    #[error("{0}")]
    Timeout(&'static str),

    /// Configuration or environment variable error (`HTTP 500`).
    #[error("configuration error: {0}")]
    Config(String),

    /// Database persistence or query error (`HTTP 500`).
    #[error("database error: {0}")]
    Database(String),

    /// External LLM or embedding service error (`HTTP 502`).
    #[error("llm error: {0}")]
    Llm(String),

    /// Document parser sidecar error (`HTTP 502`).
    #[error("parser error: {0}")]
    Parser(String),

    /// Other upstream connector, webhook, or MCP error (`HTTP 502`).
    #[error("upstream error: {0}")]
    Upstream(String),
}

impl AppError {
    /// Constructs [`AppError::BadRequest`] with a canonical error code and message.
    #[must_use]
    pub fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self::BadRequest {
            code,
            message: message.into(),
        }
    }

    /// Constructs [`AppError::Unprocessable`] with a canonical error code and message.
    #[must_use]
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::Unprocessable {
            code,
            message: message.into(),
        }
    }

    /// Returns the canonical `error_code` string per `A1` §5.
    #[must_use]
    pub const fn error_code(&self) -> &'static str {
        match self {
            Self::BadRequest { code, .. }
            | Self::Unprocessable { code, .. }
            | Self::Unauthorized(code)
            | Self::Forbidden(code)
            | Self::NotFound(code)
            | Self::Conflict(code)
            | Self::PayloadTooLarge(code)
            | Self::Unsupported(code)
            | Self::Timeout(code) => code,
            Self::RateLimited => "RATE_LIMITED",
            Self::Config(_) | Self::Database(_) => "INTERNAL",
            Self::Llm(_) => "UPSTREAM_LLM",
            Self::Parser(_) => "UPSTREAM_PARSER",
            Self::Upstream(_) => "UPSTREAM_CONNECTOR",
        }
    }

    /// Returns the public-safe client message (redacting internal 500 details per `01-STEP00` §00.6.1).
    #[must_use]
    pub fn client_message(&self) -> String {
        match self {
            Self::Config(_) | Self::Database(_) => String::from("internal error"),
            Self::BadRequest { message, .. } | Self::Unprocessable { message, .. } => {
                message.clone()
            }
            other => other.to_string(),
        }
    }
}

/// Recall channel identifier in the five-way hypergraph retrieval engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallChannel {
    /// Dense semantic vector similarity via `pgvector`.
    Vector,
    /// Lexical full-text / BM25 keyword matching.
    Bm25,
    /// Cross-document entity-bridge association recall.
    EntityLink,
    /// Adjacent paragraph (`NEXT_PARA`) context expansion.
    Context,
    /// Multi-hop (`RELATES`) graph path reasoning recall.
    MultiHop,
}

/// A single paragraph candidate returned by one retrieval channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCandidate {
    /// Paragraph identifier.
    pub para_id: Uuid,
    /// Parent document identifier.
    pub doc_id: Uuid,
    /// Document title.
    pub doc_title: String,
    /// Paragraph text content.
    pub content: String,
    /// Originating recall channel.
    pub channel: RecallChannel,
    /// 1-based rank within the originating channel.
    pub rank: u32,
    /// Optional reasoning path or matched entity trace.
    pub trace: Option<String>,
}

/// Fused retrieval hit returned after Reciprocal Rank Fusion (RRF).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusedHit {
    /// Paragraph identifier.
    pub para_id: Uuid,
    /// Parent document identifier.
    pub doc_id: Uuid,
    /// Document title.
    pub doc_title: String,
    /// Paragraph text content.
    pub content: String,
    /// Combined RRF score.
    pub rrf_score: f64,
    /// All channels that surfaced this paragraph.
    pub channels: Vec<RecallChannel>,
    /// Reasoning traces collected from contributing channels.
    pub traces: Vec<String>,
}

/// Extracted entity-relation-entity triple from a paragraph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedTriple {
    /// Source entity name.
    pub subject: String,
    /// Source entity type (e.g., `CONCEPT`, `SYSTEM`, `ORG`).
    pub subject_type: String,
    /// Relation predicate label.
    pub predicate: String,
    /// Target entity name.
    pub object: String,
    /// Target entity type.
    pub object_type: String,
    /// Supporting fact sentence.
    pub fact: String,
    /// Extraction confidence in `[0.0, 1.0]`.
    pub confidence: f64,
}

/// Document ingestion request payload (tenant resolved strictly from `AuthPrincipal`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestTextRequest {
    /// Document title.
    pub title: String,
    /// Original file or source name.
    pub source_name: String,
    /// Markdown or plain text content.
    pub content: String,
}

/// Document ingestion summary response.
#[derive(Debug, Clone, Serialize)]
pub struct IngestResponse {
    /// Created document UUID.
    pub doc_id: Uuid,
    /// Document title.
    pub title: String,
    /// Number of stored paragraphs.
    pub para_count: usize,
    /// Number of upserted entities.
    pub entity_count: usize,
    /// Number of created hypergraph edges.
    pub edge_count: usize,
    /// Embedding model used.
    pub embedding_model: String,
    /// Embedding vector dimension stored.
    pub embedding_dim: usize,
}

/// Chat / RAG / Hybrid query request payload (`05-STEP04` §04.8; tenant resolved strictly from `AuthPrincipal`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatQueryRequest {
    /// User natural-language question.
    pub query: String,
    /// Maximum fused chunks to include in synthesis.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Optional multi-turn conversation session UUID (`05-STEP04` §04.8).
    #[serde(default)]
    pub session_id: Option<Uuid>,
    /// Explicit intent routing mode (`AUTO` | `DOC` | `DATA`, default `AUTO`).
    #[serde(default)]
    pub mode: analytics::ChatQueryMode,
}

/// Chat / RAG / Hybrid query response with five-way provenance and structured NL2SQL results (`05-STEP04` §04.8).
#[derive(Debug, Clone, Serialize)]
pub struct ChatQueryResponse {
    /// Synthesized answer from the LLM.
    pub answer: String,
    /// Fused evidence hits ordered by RRF score.
    pub hits: Vec<FusedHit>,
    /// Extracted seed entities from the query.
    pub seed_entities: Vec<String>,
    /// Elapsed milliseconds for retrieval and synthesis.
    pub elapsed_ms: u128,
    /// Classified intent (`DOC` | `DATA` | `HYBRID` | `CHITCHAT`).
    pub intent: analytics::AskIntent,
    /// Active session UUID when turn tracking is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    /// Persisted `an_ask_turns.id` when `DATA` or `HYBRID` executed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Uuid>,
    /// Paragraph citations (`[文n]`).
    #[serde(default)]
    pub citations: Vec<analytics::AskCitation>,
    /// Executed `DuckDB` SQL (`DATA` or `HYBRID`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Structured result columns (`DATA` or `HYBRID`).
    #[serde(default)]
    pub columns: Vec<analytics::QueryColumn>,
    /// Structured result rows (`DATA` or `HYBRID`).
    #[serde(default)]
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Recommended chart specification (`DATA` or `HYBRID`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart: Option<analytics::ChartSuggestion>,
    /// Linked `BlueNode` records associated with recalled hypergraph entities (`§04.8`).
    #[serde(default)]
    pub related_nodes: Vec<analytics::RelatedNodeSummary>,
}

/// System-wide hypergraph statistics for the `xBlueNode` dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct SystemStats {
    /// Total documents.
    pub doc_count: i64,
    /// Total paragraphs.
    pub para_count: i64,
    /// Total distinct entities.
    pub entity_count: i64,
    /// Total hypergraph edges.
    pub edge_count: i64,
    /// Total stored embedding vectors.
    pub embedding_count: i64,
    /// Distinct embedding dimensions currently stored.
    pub embedding_dims: Vec<i32>,
    /// Server timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Visual node for the `xBlueNode` topology explorer.
#[derive(Debug, Clone, Serialize)]
pub struct TopologyNode {
    /// Node UUID.
    pub id: Uuid,
    /// Node category (`DOC`, `PARA`, or `ENTITY`).
    pub node_type: String,
    /// Display label.
    pub label: String,
    /// Occurrence count or density metric.
    pub weight: i64,
}

/// Visual edge for the `xBlueNode` topology explorer.
#[derive(Debug, Clone, Serialize)]
pub struct TopologyEdge {
    /// Edge UUID.
    pub id: Uuid,
    /// Source node UUID.
    pub source: Uuid,
    /// Target node UUID.
    pub target: Uuid,
    /// Edge relation type.
    pub rel_type: String,
    /// Optional supporting fact.
    pub fact: String,
}

/// Graph topology response payload.
#[derive(Debug, Clone, Serialize)]
pub struct TopologyGraph {
    /// Graph nodes.
    pub nodes: Vec<TopologyNode>,
    /// Graph edges.
    pub edges: Vec<TopologyEdge>,
}

const fn default_top_k() -> usize {
    6
}

/// Splits Markdown/text into retrieval-friendly paragraphs, merging tiny fragments.
#[must_use]
pub fn split_paragraphs(markdown: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for raw_block in markdown.split("\n\n") {
        let block = raw_block.trim();
        if block.is_empty() {
            continue;
        }
        if !current.is_empty() && current.len().saturating_add(block.len()) > max_chars {
            chunks.push(current.clone());
            current.clear();
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(block);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Fuses multi-channel candidates using Reciprocal Rank Fusion (RRF, `k = 60`).
#[must_use]
pub fn rrf_fuse(candidates: Vec<ChannelCandidate>, top_k: usize) -> Vec<FusedHit> {
    const RRF_K: f64 = 60.0;
    let mut map: HashMap<Uuid, FusedHit> = HashMap::new();

    for cand in candidates {
        let rank_f64 = f64::from(cand.rank.max(1));
        let delta = 1.0 / (RRF_K + rank_f64);

        let entry = map.entry(cand.para_id).or_insert_with(|| FusedHit {
            para_id: cand.para_id,
            doc_id: cand.doc_id,
            doc_title: cand.doc_title.clone(),
            content: cand.content.clone(),
            rrf_score: 0.0,
            channels: Vec::new(),
            traces: Vec::new(),
        });

        entry.rrf_score += delta;
        if !entry.channels.contains(&cand.channel) {
            entry.channels.push(cand.channel);
        }
        if let Some(trace) = cand.trace
            && !trace.is_empty()
            && !entry.traces.contains(&trace)
        {
            entry.traces.push(trace);
        }
    }

    let mut hits: Vec<FusedHit> = map.into_values().collect();
    hits.sort_by(|a, b| b.rrf_score.total_cmp(&a.rrf_score));
    hits.truncate(top_k);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrf_fuse_prioritizes_multi_channel_hits() {
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        let d1 = Uuid::new_v4();

        let candidates = vec![
            ChannelCandidate {
                para_id: p1,
                doc_id: d1,
                doc_title: String::from("Doc"),
                content: String::from("Para 1"),
                channel: RecallChannel::Vector,
                rank: 2,
                trace: Some(String::from("vec:0.85")),
            },
            ChannelCandidate {
                para_id: p1,
                doc_id: d1,
                doc_title: String::from("Doc"),
                content: String::from("Para 1"),
                channel: RecallChannel::Bm25,
                rank: 1,
                trace: Some(String::from("bm25:match")),
            },
            ChannelCandidate {
                para_id: p2,
                doc_id: d1,
                doc_title: String::from("Doc"),
                content: String::from("Para 2"),
                channel: RecallChannel::Vector,
                rank: 1,
                trace: None,
            },
        ];

        let fused = rrf_fuse(candidates, 5);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused.first().map(|h| h.para_id), Some(p1));
        assert_eq!(fused.first().map(|h| h.channels.len()), Some(2));
    }

    #[test]
    fn test_split_paragraphs_merges_short_blocks() {
        let md = "Title\n\nShort line.\n\nAnother short line.";
        let chunks = split_paragraphs(md, 500);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn ingest_request_rejects_tenant_id_field() {
        let raw = r#"{"tenant_id":"x","title":"t","source_name":"t.md","content":"c"}"#;
        let res = serde_json::from_str::<IngestTextRequest>(raw);
        assert!(res.is_err());
    }
}
