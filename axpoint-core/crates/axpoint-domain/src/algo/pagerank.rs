//! Damped power-iteration `PageRank` (with exact dangling-node redistribution)
//! and normalized in/out/bidirectional degree centrality.

use std::collections::BTreeMap;

use petgraph::Direction;
use petgraph::visit::EdgeRef as _;
use serde::{Deserialize, Serialize};

use super::{
    IndexedNodes, NodeAlgoOutput, WeightedDiGraph, extract_params_object, parse_optional_bool,
    parse_optional_f64, parse_optional_str, parse_optional_usize, sort_by_score_desc, usize_to_f64,
};
use crate::AppError;
use crate::graph::TraversalDirection;

/// Upper bound on `PageRank` power-iteration steps.
pub const MAX_PAGERANK_ITER: usize = 500;

/// Configuration parameters for damped power-iteration `PageRank`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageRankParams {
    /// Damping factor strictly inside `(0.0, 1.0)` (default `0.85`).
    pub damping: f64,
    /// Maximum power-iteration rounds in `1..=500` (default `50`).
    pub max_iter: usize,
    /// L1 residual convergence threshold `> 0.0` (default `1e-6`).
    pub tol: f64,
}

impl Default for PageRankParams {
    fn default() -> Self {
        Self {
            damping: 0.85,
            max_iter: 50,
            tol: 1e-6,
        }
    }
}

impl PageRankParams {
    /// Validates `damping`, `max_iter`, and `tol` bounds.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if any parameter is out of range.
    pub fn validate(&self) -> Result<(), AppError> {
        if !self.damping.is_finite() || self.damping <= 0.0 || self.damping >= 1.0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("damping must be in (0.0, 1.0), got {}", self.damping),
            ));
        }
        if self.max_iter == 0 || self.max_iter > MAX_PAGERANK_ITER {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "max_iter must be in 1..={MAX_PAGERANK_ITER}, got {}",
                    self.max_iter
                ),
            ));
        }
        if !self.tol.is_finite() || self.tol <= 0.0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("tol must be finite and > 0.0, got {}", self.tol),
            ));
        }
        Ok(())
    }

    /// Parses [`PageRankParams`] from a JSON object value, applying defaults for omitted keys.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) on type mismatch or out-of-range values.
    pub fn from_json(params: &serde_json::Value) -> Result<Self, AppError> {
        let map = extract_params_object(params)?;
        Self::from_json_map(map)
    }

    pub(super) fn from_json_map(
        map: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Self, AppError> {
        let defaults = Self::default();
        let out = Self {
            damping: parse_optional_f64(map, "damping")?.unwrap_or(defaults.damping),
            max_iter: parse_optional_usize(map, "max_iter")?.unwrap_or(defaults.max_iter),
            tol: parse_optional_f64(map, "tol")?.unwrap_or(defaults.tol),
        };
        out.validate()?;
        Ok(out)
    }
}

/// Computes damped weighted `PageRank` via power iteration.
///
/// Outgoing positive edge weights are normalized per source node. Dangling nodes
/// (nodes with no outgoing edges of positive finite weight) redistribute 100% of
/// their rank mass uniformly across all `N` nodes in the graph. Returns an empty
/// vector for an empty graph; otherwise probabilities sum to `1.0`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] if `params` fails validation or node count overflows `u32`.
pub fn pagerank(
    graph: &WeightedDiGraph,
    params: PageRankParams,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    params.validate()?;

    let indexed = IndexedNodes::from_graph(graph);
    let n = indexed.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let n_f64 = usize_to_f64(n)?;
    let mut outgoing: Vec<Vec<(usize, f64)>> = Vec::with_capacity(n);

    for &(_, node_idx) in &indexed.nodes {
        let mut by_target: BTreeMap<usize, f64> = BTreeMap::new();
        let mut positive_sum = 0.0_f64;

        for edge in graph.edges_directed(node_idx, Direction::Outgoing) {
            let w = *edge.weight();
            if w.is_finite()
                && w > 0.0
                && let Some(&dst_rank) = indexed.rank_by_idx.get(&edge.target())
            {
                let entry = by_target.entry(dst_rank).or_insert(0.0);
                *entry += w;
                positive_sum += w;
            }
        }

        if positive_sum > 0.0 && positive_sum.is_finite() {
            let normalized = by_target
                .into_iter()
                .map(|(dst_rank, w)| (dst_rank, w / positive_sum))
                .collect();
            outgoing.push(normalized);
        } else {
            // Dangling node (zero outgoing edges or all outgoing weights <= 0):
            // mass is redistributed uniformly across all N nodes during iteration.
            outgoing.push(Vec::new());
        }
    }

    let uniform = 1.0 / n_f64;
    let mut rank = vec![uniform; n];

    for _ in 0..params.max_iter {
        let dangling_mass: f64 = rank
            .iter()
            .zip(&outgoing)
            .filter_map(|(&r, trans)| if trans.is_empty() { Some(r) } else { None })
            .sum();

        let base = ((1.0 - params.damping) + params.damping * dangling_mass) / n_f64;
        let mut next_rank = vec![base; n];

        for (&r_u, trans) in rank.iter().zip(&outgoing) {
            let scaled = params.damping * r_u;
            for &(dst_rank, prob) in trans {
                if let Some(slot) = next_rank.get_mut(dst_rank) {
                    *slot += scaled * prob;
                }
            }
        }

        let sum_next: f64 = next_rank.iter().copied().sum();
        if sum_next > 0.0 && sum_next.is_finite() {
            for val in &mut next_rank {
                *val /= sum_next;
            }
        }

        let l1_residual: f64 = next_rank
            .iter()
            .zip(&rank)
            .map(|(&next_v, &prev_v)| (next_v - prev_v).abs())
            .sum();

        rank = next_rank;
        if l1_residual < params.tol {
            break;
        }
    }

    let final_sum: f64 = rank.iter().copied().sum();
    if final_sum > 0.0 && final_sum.is_finite() {
        for val in &mut rank {
            *val /= final_sum;
        }
    }

    let mut outputs = Vec::with_capacity(n);
    for (&(node_id, _), &score) in indexed.nodes.iter().zip(&rank) {
        outputs.push(NodeAlgoOutput {
            node_id,
            score: Some(score),
            community: None,
        });
    }
    sort_by_score_desc(&mut outputs);
    Ok(outputs)
}

/// Configuration parameters for normalized degree centrality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DegreeParams {
    /// Edge direction (`IN`, `OUT`, or `BOTH`; defaults to `BOTH`).
    pub direction: TraversalDirection,
    /// When `true`, sums positive finite edge weights instead of counting edges.
    pub weighted: bool,
}

impl DegreeParams {
    /// Parses [`DegreeParams`] from a JSON object value.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `direction` or `weighted` is invalid.
    pub fn from_json(params: &serde_json::Value) -> Result<Self, AppError> {
        let map = extract_params_object(params)?;
        Self::from_json_map(map)
    }

    pub(super) fn from_json_map(
        map: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Self, AppError> {
        let direction = match parse_optional_str(map, "direction")? {
            None => TraversalDirection::Both,
            Some(raw) => match raw.trim().to_ascii_uppercase().as_str() {
                "IN" => TraversalDirection::In,
                "OUT" => TraversalDirection::Out,
                "BOTH" => TraversalDirection::Both,
                _ => {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("params.direction must be IN, OUT, or BOTH, got '{raw}'"),
                    ));
                }
            },
        };
        let weighted = parse_optional_bool(map, "weighted")?.unwrap_or(false);
        Ok(Self {
            direction,
            weighted,
        })
    }
}

/// Computes normalized degree centrality (`degree / (N - 1)` for `N > 1`, `0.0` for `N == 1`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if node count overflows `u32`.
pub fn degree_centrality(
    graph: &WeightedDiGraph,
    params: DegreeParams,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    let indexed = IndexedNodes::from_graph(graph);
    let n = indexed.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let denom = if n > 1 {
        Some(usize_to_f64(n.saturating_sub(1))?)
    } else {
        None
    };

    let edge_contrib = |w: f64| -> f64 {
        if params.weighted {
            if w.is_finite() && w > 0.0 { w } else { 0.0 }
        } else {
            1.0
        }
    };

    let mut outputs = Vec::with_capacity(n);
    for &(node_id, node_idx) in &indexed.nodes {
        let in_deg: f64 = graph
            .edges_directed(node_idx, Direction::Incoming)
            .map(|e| edge_contrib(*e.weight()))
            .sum();
        let out_deg: f64 = graph
            .edges_directed(node_idx, Direction::Outgoing)
            .map(|e| edge_contrib(*e.weight()))
            .sum();

        let raw = match params.direction {
            TraversalDirection::In => in_deg,
            TraversalDirection::Out => out_deg,
            TraversalDirection::Both => in_deg + out_deg,
        };

        let score = match denom {
            Some(d) => raw / d,
            None => 0.0,
        };

        outputs.push(NodeAlgoOutput {
            node_id,
            score: Some(score),
            community: None,
        });
    }

    sort_by_score_desc(&mut outputs);
    Ok(outputs)
}
