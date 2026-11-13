//! Deterministic asynchronous label propagation community detection.

use std::collections::BTreeMap;

use petgraph::Direction;
use petgraph::visit::EdgeRef as _;
use serde::{Deserialize, Serialize};

use super::{
    IndexedNodes, NodeAlgoOutput, WeightedDiGraph, extract_params_object, parse_optional_u64,
    parse_optional_usize, sort_by_community_size_desc, usize_to_i64,
};
use crate::AppError;

/// Upper bound on label propagation iterations.
pub const MAX_LABEL_PROPAGATION_ITER: usize = 1_000;

/// Configuration parameters for deterministic asynchronous label propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelPropagationParams {
    /// Maximum propagation rounds in `1..=1_000` (default `30`).
    pub max_iter: usize,
    /// Deterministic `SplitMix64` shuffle seed (default `42`).
    pub seed: u64,
}

impl Default for LabelPropagationParams {
    fn default() -> Self {
        Self {
            max_iter: 30,
            seed: 42,
        }
    }
}

impl LabelPropagationParams {
    /// Validates `max_iter` bounds.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `max_iter` is outside `1..=1_000`.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.max_iter == 0 || self.max_iter > MAX_LABEL_PROPAGATION_ITER {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "max_iter must be in 1..={MAX_LABEL_PROPAGATION_ITER}, got {}",
                    self.max_iter
                ),
            ));
        }
        Ok(())
    }

    /// Parses [`LabelPropagationParams`] from a JSON object value.
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
            max_iter: parse_optional_usize(map, "max_iter")?.unwrap_or(defaults.max_iter),
            seed: parse_optional_u64(map, "seed")?.unwrap_or(defaults.seed),
        };
        out.validate()?;
        Ok(out)
    }
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

fn shuffle_order(order: &mut [usize], seed: u64) {
    let mut rng = SplitMix64::new(seed);
    let len = order.len();
    for rev_offset in 0..len {
        let i = len.saturating_sub(1).saturating_sub(rev_offset);
        if i == 0 {
            break;
        }
        let bound = u64::try_from(i.saturating_add(1)).unwrap_or(1);
        let chosen = rng.next_u64() % bound;
        let j = usize::try_from(chosen).unwrap_or(0);
        if i != j
            && let Ok([slot_i, slot_j]) = order.get_disjoint_mut([i, j])
        {
            std::mem::swap(slot_i, slot_j);
        }
    }
}

/// Executes deterministic asynchronous label propagation community detection.
///
/// Each node initially receives a unique community label `0..N` ordered by ascending
/// node [`uuid::Uuid`]. In round `r`, node visit order is shuffled deterministically
/// via `SplitMix64(seed ^ r)` and Fisher-Yates swaps. Neighbor community weights sum
/// incident positive edge weights in both directions; ties strictly select the smallest
/// community label ID for cross-platform determinism, exiting early when a full round
/// produces zero label updates.
///
/// # Errors
/// Returns [`AppError::BadRequest`] if `params` is invalid or node count overflows `i64`.
pub fn label_propagation(
    graph: &WeightedDiGraph,
    params: LabelPropagationParams,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    params.validate()?;

    let indexed = IndexedNodes::from_graph(graph);
    let n = indexed.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut neighbors: Vec<Vec<(usize, f64)>> = Vec::with_capacity(n);
    for (u_rank, &(_, u_idx)) in indexed.nodes.iter().enumerate() {
        let mut acc: BTreeMap<usize, f64> = BTreeMap::new();

        for edge in graph.edges_directed(u_idx, Direction::Outgoing) {
            let w = *edge.weight();
            if w.is_finite()
                && w > 0.0
                && let Some(&v_rank) = indexed.rank_by_idx.get(&edge.target())
                && v_rank != u_rank
            {
                let entry = acc.entry(v_rank).or_insert(0.0);
                *entry += w;
            }
        }

        for edge in graph.edges_directed(u_idx, Direction::Incoming) {
            let w = *edge.weight();
            if w.is_finite()
                && w > 0.0
                && let Some(&v_rank) = indexed.rank_by_idx.get(&edge.source())
                && v_rank != u_rank
            {
                let entry = acc.entry(v_rank).or_insert(0.0);
                *entry += w;
            }
        }

        neighbors.push(acc.into_iter().collect());
    }

    let mut labels = Vec::with_capacity(n);
    for rank in 0..n {
        labels.push(usize_to_i64(rank)?);
    }

    for round in 0..params.max_iter {
        let round_u64 = u64::try_from(round).unwrap_or(u64::MAX);
        let mut order: Vec<usize> = (0..n).collect();
        shuffle_order(&mut order, params.seed ^ round_u64);

        let mut changed = false;
        for u_rank in order {
            let Some(nbrs) = neighbors.get(u_rank) else {
                continue;
            };
            if nbrs.is_empty() {
                continue;
            }

            let mut scores_by_label: BTreeMap<i64, f64> = BTreeMap::new();
            for &(v_rank, weight) in nbrs {
                if let Some(&lbl) = labels.get(v_rank) {
                    let entry = scores_by_label.entry(lbl).or_insert(0.0);
                    *entry += weight;
                }
            }

            // `BTreeMap` iterates labels in strictly ascending order; requiring
            // `score > best_score + 1e-12` guarantees exact and floating-point ties
            // deterministically select the smallest community label ID.
            let mut best: Option<(i64, f64)> = None;
            for (lbl, score) in scores_by_label {
                match best {
                    None => best = Some((lbl, score)),
                    Some((_, best_score)) if score > best_score + 1e-12 => {
                        best = Some((lbl, score));
                    }
                    Some(_) => {}
                }
            }

            if let (Some((best_lbl, _)), Some(slot)) = (best, labels.get_mut(u_rank))
                && *slot != best_lbl
            {
                *slot = best_lbl;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    let mut outputs = Vec::with_capacity(n);
    for (&(node_id, _), &comm) in indexed.nodes.iter().zip(&labels) {
        outputs.push(NodeAlgoOutput {
            node_id,
            score: None,
            community: Some(comm),
        });
    }
    sort_by_community_size_desc(&mut outputs);
    Ok(outputs)
}
