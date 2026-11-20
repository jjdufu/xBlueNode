//! Hop-distance BFS, maximum-product `top_path` attribution, and human-readable path explanation (`09-STEP10` §10.4).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sim::{ImpactRow, SimGraph};

/// Single step along a dominant causal propagation path returned by [`explain_impact_path`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathStepDetail {
    /// Node UUID at this hop.
    pub node_id: Uuid,
    /// Human-readable node title (or UUID string if missing from `graph`).
    pub node_title: String,
    /// Incoming link type from the previous hop (`None` for the intervention seed).
    pub edge_type: Option<String>,
    /// Effective propagation weight/gain on the incoming edge (`None` for the seed).
    pub weight: Option<f64>,
    /// Cumulative product `Π |weight|` from the seed up to this node.
    pub cumulative_gain: f64,
}

/// Human-readable explanation of an [`ImpactRow`]'s dominant causal path and gain contribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactPathExplanation {
    /// Target impacted node UUID.
    pub target_node_id: Uuid,
    /// Impacted metric key.
    pub metric: String,
    /// Shortest hop distance from any intervention seed.
    pub hop_distance: u32,
    /// Total cumulative gain `Π |weight|` along `steps`.
    pub total_gain: f64,
    /// Ordered hops from the seed node to `target_node_id`.
    pub steps: Vec<PathStepDetail>,
    /// Concise human-readable narrative describing the propagation path.
    pub summary: String,
}

/// Computes shortest hop distances from intervention `seeds` across `(src, dst)` edges.
#[must_use]
pub fn compute_hop_distances(seeds: &[Uuid], edges: &[(Uuid, Uuid)]) -> HashMap<Uuid, u32> {
    let mut adj: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for &(src, dst) in edges {
        adj.entry(src).or_default().push(dst);
    }

    let mut dist: HashMap<Uuid, u32> = HashMap::new();
    let mut queue: VecDeque<(Uuid, u32)> = VecDeque::new();

    for &seed in seeds {
        if let std::collections::hash_map::Entry::Vacant(e) = dist.entry(seed) {
            e.insert(0);
            queue.push_back((seed, 0));
        }
    }

    while let Some((u, d)) = queue.pop_front() {
        let next_d = d.saturating_add(1);
        if let Some(neighbors) = adj.get(&u) {
            for &v in neighbors {
                if let std::collections::hash_map::Entry::Vacant(e) = dist.entry(v) {
                    e.insert(next_d);
                    queue.push_back((v, next_d));
                }
            }
        }
    }

    dist
}

/// Computes dominant propagation paths `[seed, ..., target]` maximizing `Π |weight|`.
#[must_use]
pub fn compute_top_paths(
    seeds: &[Uuid],
    weighted_edges: &[(Uuid, Uuid, f64)],
) -> HashMap<Uuid, Vec<Uuid>> {
    let mut ids: Vec<Uuid> = Vec::new();
    let mut id_to_idx: HashMap<Uuid, usize> = HashMap::new();

    let mut register = |id: Uuid| -> usize {
        if let Some(&idx) = id_to_idx.get(&id) {
            idx
        } else {
            let idx = ids.len();
            ids.push(id);
            id_to_idx.insert(id, idx);
            idx
        }
    };

    let mut seed_indices = Vec::with_capacity(seeds.len());
    for &s in seeds {
        seed_indices.push(register(s));
    }

    let mut indexed_edges = Vec::with_capacity(weighted_edges.len());
    for &(src, dst, weight) in weighted_edges {
        let u = register(src);
        let v = register(dst);
        indexed_edges.push((u, v, weight));
    }

    let paths = compute_top_paths_indexed(ids.len(), &seed_indices, &indexed_edges);
    let mut out = HashMap::new();
    for (idx, path) in paths.into_iter().enumerate() {
        if !path.is_empty()
            && let Some(&node_id) = ids.get(idx)
        {
            let uuid_path: Vec<Uuid> = path
                .into_iter()
                .filter_map(|i| ids.get(i).copied())
                .collect();
            out.insert(node_id, uuid_path);
        }
    }
    out
}

/// Computes shortest hop distances on compact `0..n` vertex indices.
#[must_use]
pub fn compute_hop_distances_indexed(
    n: usize,
    seeds: &[usize],
    adj: &[Vec<usize>],
) -> Vec<Option<u32>> {
    let mut dist = vec![None; n];
    let mut queue = VecDeque::new();

    for &seed in seeds {
        if let Some(slot) = dist.get_mut(seed)
            && slot.is_none()
        {
            *slot = Some(0);
            queue.push_back((seed, 0_u32));
        }
    }

    while let Some((u, d)) = queue.pop_front() {
        let next_d = d.saturating_add(1);
        if let Some(neighbors) = adj.get(u) {
            for &v in neighbors {
                if let Some(slot) = dist.get_mut(v)
                    && slot.is_none()
                {
                    *slot = Some(next_d);
                    queue.push_back((v, next_d));
                }
            }
        }
    }

    dist
}

/// Computes dominant paths maximizing `Π |weight|` on compact `0..n` vertex indices.
#[must_use]
pub fn compute_top_paths_indexed(
    n: usize,
    seeds: &[usize],
    weighted_edges: &[(usize, usize, f64)],
) -> Vec<Vec<usize>> {
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(u, v, w) in weighted_edges {
        let mag = w.abs();
        if mag > 0.0
            && mag.is_finite()
            && let Some(list) = adj.get_mut(u)
        {
            list.push((v, mag));
        }
    }

    let mut best_prod = vec![0.0_f64; n];
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut heap = BinaryHeap::new();

    for &s in seeds {
        if let Some(slot) = best_prod.get_mut(s)
            && *slot < 1.0
        {
            *slot = 1.0;
            if let Some(p_slot) = parent.get_mut(s) {
                *p_slot = None;
            }
            heap.push(PathCandidate { prod: 1.0, node: s });
        }
    }

    relax_max_product_heap(&adj, &mut best_prod, &mut parent, &mut heap);
    reconstruct_all_paths(n, &best_prod, &parent)
}

/// Formats a human-readable [`ImpactPathExplanation`] for an [`ImpactRow`] against `graph`.
#[must_use]
pub fn explain_impact_path(graph: &SimGraph, row: &ImpactRow) -> ImpactPathExplanation {
    let node_titles: HashMap<Uuid, &str> = graph
        .nodes
        .iter()
        .map(|n| (n.id, n.title.as_str()))
        .collect();

    let mut steps = Vec::with_capacity(row.top_path.len());
    let mut cumulative_gain = 1.0_f64;
    let mut prev_id: Option<Uuid> = None;

    for &nid in &row.top_path {
        let title = node_titles
            .get(&nid)
            .map_or_else(|| nid.to_string(), |t| (*t).to_string());
        let (edge_type, weight) = if let Some(src) = prev_id {
            find_strongest_link_meta(graph, src, nid)
        } else {
            (None, None)
        };
        if let Some(w) = weight {
            cumulative_gain *= w.abs();
        }
        steps.push(PathStepDetail {
            node_id: nid,
            node_title: title,
            edge_type,
            weight,
            cumulative_gain,
        });
        prev_id = Some(nid);
    }

    let summary = format_path_summary(row, &steps, cumulative_gain);

    ImpactPathExplanation {
        target_node_id: row.node_id,
        metric: row.metric.clone(),
        hop_distance: row.hop_distance,
        total_gain: if steps.is_empty() {
            0.0
        } else {
            cumulative_gain
        },
        steps,
        summary,
    }
}

fn find_strongest_link_meta(
    graph: &SimGraph,
    src: Uuid,
    dst: Uuid,
) -> (Option<String>, Option<f64>) {
    let mut best: Option<(&str, f64)> = None;
    for link in &graph.links {
        if link.src_node_id == src && link.dst_node_id == dst {
            let candidate_w = graph
                .edge_specs
                .get(&link.link_type)
                .and_then(|spec| spec.rules.first())
                .and_then(|r| r.coefficient)
                .unwrap_or(link.weight);
            let replace = best.is_none_or(|(_, bw)| candidate_w.abs() > bw.abs());
            if replace {
                best = Some((link.link_type.as_str(), candidate_w));
            }
        }
    }
    match best {
        Some((lt, w)) => (Some(lt.to_string()), Some(w)),
        None => (None, None),
    }
}

fn format_path_summary(row: &ImpactRow, steps: &[PathStepDetail], total_gain: f64) -> String {
    if steps.is_empty() {
        return format!("{} ({}) has no recorded causal path", row.title, row.metric);
    }
    let mut parts = Vec::with_capacity(steps.len().saturating_mul(2).saturating_add(1));
    for (idx, step) in steps.iter().enumerate() {
        if idx > 0 {
            let rel = step.edge_type.as_deref().unwrap_or("LINK");
            let weight = step.weight.unwrap_or(1.0);
            parts.push(format!(" --({rel} x{weight:.3})--> "));
        }
        parts.push(step.node_title.clone());
    }
    parts.push(format!(
        " [metric={}, hops={}, total_gain={total_gain:.4}]",
        row.metric, row.hop_distance
    ));
    parts.concat()
}

#[derive(Debug, Clone, Copy)]
struct PathCandidate {
    prod: f64,
    node: usize,
}

impl PartialEq for PathCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.prod.total_cmp(&other.prod) == Ordering::Equal
    }
}

impl Eq for PathCandidate {}

impl PartialOrd for PathCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.prod
            .total_cmp(&other.prod)
            .then_with(|| other.node.cmp(&self.node))
    }
}

fn relax_max_product_heap(
    adj: &[Vec<(usize, f64)>],
    best_prod: &mut [f64],
    parent: &mut [Option<usize>],
    heap: &mut BinaryHeap<PathCandidate>,
) {
    while let Some(PathCandidate { prod, node: u }) = heap.pop() {
        let current_best = best_prod.get(u).copied().unwrap_or(0.0);
        if prod < current_best {
            continue;
        }
        let Some(neighbors) = adj.get(u) else {
            continue;
        };
        for &(v, w) in neighbors {
            let cand = prod * w;
            let dst_best = best_prod.get(v).copied().unwrap_or(0.0);
            if cand > dst_best && (w <= 1.0 || !is_ancestor(parent, u, v)) {
                if let Some(b_slot) = best_prod.get_mut(v) {
                    *b_slot = cand;
                }
                if let Some(p_slot) = parent.get_mut(v) {
                    *p_slot = Some(u);
                }
                heap.push(PathCandidate {
                    prod: cand,
                    node: v,
                });
            }
        }
    }
}

fn is_ancestor(parent: &[Option<usize>], start: usize, target: usize) -> bool {
    let mut curr = Some(start);
    let mut steps = 0_usize;
    let limit = parent.len();
    while let Some(u) = curr {
        if u == target {
            return true;
        }
        if steps >= limit {
            return true;
        }
        steps = steps.saturating_add(1);
        curr = parent.get(u).copied().flatten();
    }
    false
}

fn reconstruct_all_paths(n: usize, best_prod: &[f64], parent: &[Option<usize>]) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); n];
    for v in 0..n {
        if best_prod.get(v).copied().unwrap_or(0.0) <= 0.0 {
            continue;
        }
        let mut path = Vec::new();
        let mut curr = Some(v);
        let mut guard = 0_usize;
        while let Some(u) = curr {
            path.push(u);
            guard = guard.saturating_add(1);
            if guard > n {
                break;
            }
            curr = parent.get(u).copied().flatten();
        }
        path.reverse();
        if let Some(slot) = out.get_mut(v) {
            *slot = path;
        }
    }
    out
}
