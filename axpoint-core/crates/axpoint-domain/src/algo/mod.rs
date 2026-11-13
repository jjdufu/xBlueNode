//! Pure `petgraph`-backed graph algorithms (`PageRank`, degree centrality,
//! deterministic label propagation, connected components, and shortest paths).

pub mod components;
pub mod label_propagation;
pub mod pagerank;
pub mod paths;

use std::collections::{BTreeMap, BTreeSet};

use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::GraphAlgo;

pub use self::components::{strongly_connected_components, weakly_connected_components};
pub use self::label_propagation::{LabelPropagationParams, label_propagation};
pub use self::pagerank::{
    DegreeParams, MAX_PAGERANK_ITER, PageRankParams, degree_centrality, pagerank,
};
pub use self::paths::{ShortestPathResult, unweighted_shortest_path, weighted_shortest_path};

/// Canonical directed weighted business graph (`node = Uuid`, `edge = weight`).
pub type WeightedDiGraph = DiGraph<Uuid, f64>;

/// Directed weighted edge input for pure graph algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlgoEdge {
    /// Source node identifier.
    pub src: Uuid,
    /// Destination node identifier.
    pub dst: Uuid,
    /// Edge weight (positive causal or association strength).
    pub weight: f64,
}

/// Per-node output produced by [`execute_algorithm`] and individual graph algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NodeAlgoOutput {
    /// Business node identifier.
    pub node_id: Uuid,
    /// Continuous centrality or `PageRank` score, when produced by the algorithm.
    pub score: Option<f64>,
    /// Deterministic community or connected-component identifier, when produced by the algorithm.
    pub community: Option<i64>,
}

/// Constructs a canonical [`WeightedDiGraph`] from node UUIDs and directed edges.
///
/// Nodes are deduplicated and inserted in ascending [`Uuid`] order so node indices
/// correspond deterministically to sorted [`Uuid`] ranks. Edges whose endpoints are
/// not present in `nodes` are ignored.
#[must_use]
pub fn build_weighted_digraph(nodes: &[Uuid], edges: &[AlgoEdge]) -> WeightedDiGraph {
    let unique_nodes: BTreeSet<Uuid> = nodes.iter().copied().collect();
    let mut graph = WeightedDiGraph::with_capacity(unique_nodes.len(), edges.len());
    let mut index_by_uuid: BTreeMap<Uuid, NodeIndex> = BTreeMap::new();

    for node_id in unique_nodes {
        let idx = graph.add_node(node_id);
        index_by_uuid.insert(node_id, idx);
    }

    for edge in edges {
        if let (Some(&src_idx), Some(&dst_idx)) =
            (index_by_uuid.get(&edge.src), index_by_uuid.get(&edge.dst))
        {
            graph.add_edge(src_idx, dst_idx, edge.weight);
        }
    }

    graph
}

/// Executes a [`GraphAlgo`] over raw `nodes` and `edges` with JSON `params`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `LIMIT_OUT_OF_RANGE`) if
/// `params` is malformed or out of valid bounds.
pub fn execute_algorithm(
    algo: GraphAlgo,
    nodes: &[Uuid],
    edges: &[AlgoEdge],
    params: &serde_json::Value,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    let graph = build_weighted_digraph(nodes, edges);
    execute_algorithm_on_graph(algo, &graph, params)
}

/// Executes a [`GraphAlgo`] on a pre-built [`WeightedDiGraph`] with JSON `params`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `LIMIT_OUT_OF_RANGE`) if
/// `params` is malformed or out of valid bounds.
pub fn execute_algorithm_on_graph(
    algo: GraphAlgo,
    graph: &WeightedDiGraph,
    params: &serde_json::Value,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    let params_map = extract_params_object(params)?;
    match algo {
        GraphAlgo::Pagerank => {
            let pr_params = PageRankParams::from_json_map(params_map)?;
            pagerank(graph, pr_params)
        }
        GraphAlgo::Degree => {
            let deg_params = DegreeParams::from_json_map(params_map)?;
            degree_centrality(graph, deg_params)
        }
        GraphAlgo::LabelPropagation => {
            let lpa_params = LabelPropagationParams::from_json_map(params_map)?;
            label_propagation(graph, lpa_params)
        }
        GraphAlgo::StronglyConnected => strongly_connected_components(graph),
        GraphAlgo::WeaklyConnected => weakly_connected_components(graph),
        GraphAlgo::ConnectedComponents => {
            let weak = parse_optional_bool(params_map, "weak")?.unwrap_or(false);
            if weak {
                weakly_connected_components(graph)
            } else {
                strongly_connected_components(graph)
            }
        }
    }
}

pub(super) struct IndexedNodes {
    pub(super) nodes: Vec<(Uuid, NodeIndex)>,
    pub(super) rank_by_idx: BTreeMap<NodeIndex, usize>,
    pub(super) rank_by_uuid: BTreeMap<Uuid, usize>,
}

impl IndexedNodes {
    pub(super) fn from_graph(graph: &WeightedDiGraph) -> Self {
        let mut nodes = Vec::with_capacity(graph.node_count());
        for idx in graph.node_indices() {
            if let Some(&id) = graph.node_weight(idx) {
                nodes.push((id, idx));
            }
        }
        nodes.sort_by(|(id_a, idx_a), (id_b, idx_b)| {
            id_a.cmp(id_b)
                .then_with(|| idx_a.index().cmp(&idx_b.index()))
        });

        let mut rank_by_idx = BTreeMap::new();
        let mut rank_by_uuid = BTreeMap::new();
        for (rank, &(id, idx)) in nodes.iter().enumerate() {
            rank_by_idx.insert(idx, rank);
            rank_by_uuid.entry(id).or_insert(rank);
        }

        Self {
            nodes,
            rank_by_idx,
            rank_by_uuid,
        }
    }
}

pub(super) fn usize_to_f64(value: usize) -> Result<f64, AppError> {
    let count = u32::try_from(value).map_err(|_| {
        AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("count {value} exceeds u32::MAX"),
        )
    })?;
    Ok(f64::from(count))
}

pub(super) fn usize_to_i64(value: usize) -> Result<i64, AppError> {
    i64::try_from(value).map_err(|_| {
        AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("index {value} exceeds i64::MAX"),
        )
    })
}

pub(super) fn sort_by_score_desc(outputs: &mut [NodeAlgoOutput]) {
    outputs.sort_by(|a, b| {
        let sa = a.score.unwrap_or(f64::NEG_INFINITY);
        let sb = b.score.unwrap_or(f64::NEG_INFINITY);
        sb.total_cmp(&sa).then_with(|| a.node_id.cmp(&b.node_id))
    });
}

pub(super) fn sort_by_community_size_desc(outputs: &mut [NodeAlgoOutput]) {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for item in outputs.iter() {
        if let Some(comm) = item.community {
            let entry = counts.entry(comm).or_insert(0);
            *entry = entry.saturating_add(1);
        }
    }
    outputs.sort_by(|a, b| {
        let ca = a.community.unwrap_or(i64::MAX);
        let cb = b.community.unwrap_or(i64::MAX);
        let size_a = counts.get(&ca).copied().unwrap_or(0);
        let size_b = counts.get(&cb).copied().unwrap_or(0);
        size_b
            .cmp(&size_a)
            .then_with(|| ca.cmp(&cb))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
}

pub(super) fn extract_params_object(
    params: &serde_json::Value,
) -> Result<Option<&serde_json::Map<String, serde_json::Value>>, AppError> {
    match params {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(map) => Ok(Some(map)),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            "algorithm params must be a JSON object",
        )),
    }
}

pub(super) fn parse_optional_f64(
    map: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<Option<f64>, AppError> {
    let Some(val) = map.and_then(|m| m.get(key)) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_f64().map(Some).ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("params.{key} must be a finite number"),
        )
    })
}

pub(super) fn parse_optional_u64(
    map: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<Option<u64>, AppError> {
    let Some(val) = map.and_then(|m| m.get(key)) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_u64().map(Some).ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("params.{key} must be a non-negative integer"),
        )
    })
}

pub(super) fn parse_optional_usize(
    map: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<Option<usize>, AppError> {
    let Some(raw) = parse_optional_u64(map, key)? else {
        return Ok(None);
    };
    let parsed = usize::try_from(raw).map_err(|_| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("params.{key} exceeds platform usize"),
        )
    })?;
    Ok(Some(parsed))
}

pub(super) fn parse_optional_bool(
    map: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<Option<bool>, AppError> {
    let Some(val) = map.and_then(|m| m.get(key)) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_bool().map(Some).ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("params.{key} must be a boolean"),
        )
    })
}

pub(super) fn parse_optional_str<'a>(
    map: Option<&'a serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<Option<&'a str>, AppError> {
    let Some(val) = map.and_then(|m| m.get(key)) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    val.as_str()
        .map(Some)
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("params.{key} must be a string")))
}
