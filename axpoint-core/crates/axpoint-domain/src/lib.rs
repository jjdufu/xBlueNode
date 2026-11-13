//! Domain types, error definitions, and five-way RRF fusion logic for `AxPoint`.

pub mod ports;
pub mod topology;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Unified application error type across `AxPoint` crates.
#[derive(Debug, Error)]
pub enum AppError {
    /// Configuration or environment variable error.
    #[error("configuration error: {0}")]
    Config(String),

    /// Database persistence or query error.
    #[error("database error: {0}")]
    Database(String),

    /// External LLM or embedding service error.
    #[error("llm error: {0}")]
    Llm(String),

    /// Document parser sidecar error.
    #[error("parser error: {0}")]
    Parser(String),

    /// Invalid client input.
    #[error("bad request: {0}")]
    BadRequest(String),
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

/// Document ingestion request payload.
#[derive(Debug, Clone, Deserialize)]
pub struct IngestTextRequest {
    /// Tenant or subgraph isolation identifier.
    #[serde(default = "default_tenant")]
    pub tenant_id: String,
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

/// Chat / RAG query request payload.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatQueryRequest {
    /// Tenant or subgraph isolation identifier.
    #[serde(default = "default_tenant")]
    pub tenant_id: String,
    /// User natural-language question.
    pub query: String,
    /// Maximum fused chunks to include in synthesis.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

/// Chat / RAG query response with full five-way provenance.
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

fn default_tenant() -> String {
    String::from("default")
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
}
