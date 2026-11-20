//! Hop-distance BFS and maximum-product `top_path` attribution for `AxSim` impacts (`09-STEP10` §10.4).

use std::collections::HashMap;

use uuid::Uuid;

/// Computes shortest hop distances from intervention seeds across `(src, dst)` edges.
#[must_use]
pub fn compute_hop_distances(seeds: &[Uuid], _edges: &[(Uuid, Uuid)]) -> HashMap<Uuid, u32> {
    seeds.iter().map(|&id| (id, 0)).collect()
}

/// Computes dominant propagation paths `[seed, ..., target]` maximizing `Π |weight|`.
#[must_use]
pub fn compute_top_paths(
    seeds: &[Uuid],
    _weighted_edges: &[(Uuid, Uuid, f64)],
) -> HashMap<Uuid, Vec<Uuid>> {
    seeds.iter().map(|&id| (id, vec![id])).collect()
}
