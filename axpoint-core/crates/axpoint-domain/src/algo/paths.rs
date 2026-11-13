//! Weighted Dijkstra shortest path (`cost = 1.0 / weight.max(1e-9)`) and
//! unweighted level-synchronized bidirectional BFS shortest path.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

use petgraph::Direction;
use petgraph::visit::EdgeRef as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AlgoEdge, IndexedNodes, WeightedDiGraph};

/// Minimum positive weight denominator floor used in [`weighted_shortest_path`].
pub const MIN_EFFECTIVE_WEIGHT: f64 = 1e-9;

/// Shortest-path result returned by [`weighted_shortest_path`] and [`unweighted_shortest_path`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortestPathResult {
    /// Ordered node UUIDs from `from` to `to` (empty when unreachable within `max_depth`).
    pub path: Vec<Uuid>,
    /// Ordered directed edges traversed along `path`.
    pub edges: Vec<AlgoEdge>,
    /// Total path cost (`Some(hops)` when unweighted, `Some(sum(1.0 / weight))` when weighted, `None` if unreachable).
    pub total_cost: Option<f64>,
}

impl ShortestPathResult {
    /// Constructs an empty unreachable [`ShortestPathResult`].
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            path: Vec::new(),
            edges: Vec::new(),
            total_cost: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HeapState {
    cost: f64,
    hops: usize,
    node_rank: usize,
    state_id: usize,
}

impl PartialEq for HeapState {
    fn eq(&self, other: &Self) -> bool {
        self.cost.total_cmp(&other.cost) == Ordering::Equal
            && self.hops == other.hops
            && self.node_rank == other.node_rank
            && self.state_id == other.state_id
    }
}

impl Eq for HeapState {}

impl PartialOrd for HeapState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapState {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering so `BinaryHeap` acts as a min-heap on `(cost, hops, node_rank, state_id)`.
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.hops.cmp(&self.hops))
            .then_with(|| other.node_rank.cmp(&self.node_rank))
            .then_with(|| other.state_id.cmp(&self.state_id))
    }
}

#[derive(Debug, Clone, Copy)]
struct ParentRecord {
    node_rank: usize,
    parent_state: Option<usize>,
    edge: Option<AlgoEdge>,
}

/// Computes the minimum-cost directed path from `from` to `to` using Dijkstra's algorithm
/// with edge traversal cost `1.0 / weight.max(1e-9)`.
///
/// When `max_depth` is `Some(limit)`, only paths containing at most `limit` edges are
/// considered (using Pareto `(hops, cost)` dominance pruning so longer low-cost prefixes
/// never shadow valid hop-bounded paths). Returns [`ShortestPathResult::empty`] if
/// `from` or `to` does not exist or no directed path exists within `max_depth`.
#[must_use]
pub fn weighted_shortest_path(
    graph: &WeightedDiGraph,
    from: Uuid,
    to: Uuid,
    max_depth: Option<usize>,
) -> ShortestPathResult {
    let indexed = IndexedNodes::from_graph(graph);
    let (Some(&start_rank), Some(&goal_rank)) = (
        indexed.rank_by_uuid.get(&from),
        indexed.rank_by_uuid.get(&to),
    ) else {
        return ShortestPathResult::empty();
    };

    if start_rank == goal_rank {
        return ShortestPathResult {
            path: vec![from],
            edges: Vec::new(),
            total_cost: Some(0.0),
        };
    }

    if max_depth == Some(0) {
        return ShortestPathResult::empty();
    }

    let outgoing = build_weighted_outgoing(graph, &indexed);
    let mut records: Vec<ParentRecord> = vec![ParentRecord {
        node_rank: start_rank,
        parent_state: None,
        edge: None,
    }];
    let mut frontier_by_node: BTreeMap<usize, Vec<(usize, f64)>> = BTreeMap::new();
    frontier_by_node.insert(start_rank, vec![(0, 0.0)]);

    let mut heap = BinaryHeap::new();
    heap.push(HeapState {
        cost: 0.0,
        hops: 0,
        node_rank: start_rank,
        state_id: 0,
    });

    while let Some(curr) = heap.pop() {
        if curr.node_rank == goal_rank {
            return reconstruct_dijkstra_path(&indexed, &records, curr.state_id, curr.cost);
        }
        if max_depth.is_some_and(|limit| curr.hops >= limit) {
            continue;
        }

        let effective_curr_hops = if max_depth.is_some() { curr.hops } else { 0 };
        if frontier_by_node.get(&curr.node_rank).is_some_and(|list| {
            list.iter()
                .any(|&(h, c)| h <= effective_curr_hops && c < curr.cost - 1e-15)
        }) {
            continue;
        }

        let Some(adj) = outgoing.get(curr.node_rank) else {
            continue;
        };

        for &(next_rank, algo_edge, step_cost) in adj {
            let next_hops = curr.hops.saturating_add(1);
            if max_depth.is_some_and(|limit| next_hops > limit) {
                continue;
            }
            let next_cost = curr.cost + step_cost;
            let effective_hops = if max_depth.is_some() { next_hops } else { 0 };

            let dominated = frontier_by_node.get(&next_rank).is_some_and(|list| {
                list.iter()
                    .any(|&(h, c)| h <= effective_hops && c <= next_cost)
            });
            if dominated {
                continue;
            }

            let entry = frontier_by_node.entry(next_rank).or_default();
            entry.retain(|&(h, c)| !(effective_hops <= h && next_cost <= c));
            entry.push((effective_hops, next_cost));

            let next_state_id = records.len();
            records.push(ParentRecord {
                node_rank: next_rank,
                parent_state: Some(curr.state_id),
                edge: Some(algo_edge),
            });

            heap.push(HeapState {
                cost: next_cost,
                hops: next_hops,
                node_rank: next_rank,
                state_id: next_state_id,
            });
        }
    }

    ShortestPathResult::empty()
}

fn build_weighted_outgoing(
    graph: &WeightedDiGraph,
    indexed: &IndexedNodes,
) -> Vec<Vec<(usize, AlgoEdge, f64)>> {
    let mut outgoing = Vec::with_capacity(indexed.nodes.len());
    for &(src_uuid, src_idx) in &indexed.nodes {
        let mut adj = Vec::new();
        for edge in graph.edges_directed(src_idx, Direction::Outgoing) {
            let w = *edge.weight();
            if !w.is_finite() {
                continue;
            }
            if let Some(&dst_rank) = indexed.rank_by_idx.get(&edge.target())
                && let Some(&(dst_uuid, _)) = indexed.nodes.get(dst_rank)
            {
                let step_cost = 1.0 / w.max(MIN_EFFECTIVE_WEIGHT);
                let algo_edge = AlgoEdge {
                    src: src_uuid,
                    dst: dst_uuid,
                    weight: w,
                };
                adj.push((dst_rank, algo_edge, step_cost));
            }
        }
        adj.sort_by(|(ra, _, ca), (rb, _, cb)| ca.total_cmp(cb).then_with(|| ra.cmp(rb)));
        outgoing.push(adj);
    }
    outgoing
}

fn reconstruct_dijkstra_path(
    indexed: &IndexedNodes,
    records: &[ParentRecord],
    goal_state_id: usize,
    total_cost: f64,
) -> ShortestPathResult {
    let mut rev_nodes = Vec::new();
    let mut rev_edges = Vec::new();
    let mut cursor = Some(goal_state_id);

    while let Some(sid) = cursor {
        let Some(rec) = records.get(sid) else {
            return ShortestPathResult::empty();
        };
        if let Some(&(uuid, _)) = indexed.nodes.get(rec.node_rank) {
            rev_nodes.push(uuid);
        }
        if let Some(edge) = rec.edge {
            rev_edges.push(edge);
        }
        cursor = rec.parent_state;
    }

    rev_nodes.reverse();
    rev_edges.reverse();
    ShortestPathResult {
        path: rev_nodes,
        edges: rev_edges,
        total_cost: Some(total_cost),
    }
}

/// Computes the minimum-hop directed path from `from` to `to` within `max_depth` edges
/// using level-synchronized bidirectional BFS.
///
/// Forward expansion follows outgoing edges from `from`, while backward expansion follows
/// incoming edges into `to`. Returns [`ShortestPathResult::empty`] if `from` or `to` is
/// absent or unreachable within `max_depth` hops.
#[must_use]
pub fn unweighted_shortest_path(
    graph: &WeightedDiGraph,
    from: Uuid,
    to: Uuid,
    max_depth: usize,
) -> ShortestPathResult {
    let indexed = IndexedNodes::from_graph(graph);
    let (Some(&start_rank), Some(&goal_rank)) = (
        indexed.rank_by_uuid.get(&from),
        indexed.rank_by_uuid.get(&to),
    ) else {
        return ShortestPathResult::empty();
    };

    if start_rank == goal_rank {
        return ShortestPathResult {
            path: vec![from],
            edges: Vec::new(),
            total_cost: Some(0.0),
        };
    }

    if max_depth == 0 {
        return ShortestPathResult::empty();
    }

    let (outgoing, incoming) = build_bidirectional_adj(graph, &indexed);

    let mut fwd_dist: BTreeMap<usize, usize> = BTreeMap::from([(start_rank, 0)]);
    let mut fwd_prev: BTreeMap<usize, (usize, AlgoEdge)> = BTreeMap::new();
    let mut fwd_frontier = vec![start_rank];
    let mut fwd_steps: usize = 0;

    let mut bwd_dist: BTreeMap<usize, usize> = BTreeMap::from([(goal_rank, 0)]);
    let mut bwd_next: BTreeMap<usize, (usize, AlgoEdge)> = BTreeMap::new();
    let mut bwd_frontier = vec![goal_rank];
    let mut bwd_steps: usize = 0;

    while !fwd_frontier.is_empty()
        && !bwd_frontier.is_empty()
        && fwd_steps.saturating_add(bwd_steps) < max_depth
    {
        if fwd_frontier.len() <= bwd_frontier.len() {
            fwd_steps = fwd_steps.saturating_add(1);
            fwd_frontier = expand_bfs_level(
                &fwd_frontier,
                &outgoing,
                fwd_steps,
                &mut fwd_dist,
                &mut fwd_prev,
            );
        } else {
            bwd_steps = bwd_steps.saturating_add(1);
            bwd_frontier = expand_bfs_level(
                &bwd_frontier,
                &incoming,
                bwd_steps,
                &mut bwd_dist,
                &mut bwd_next,
            );
        }

        if let Some(meet_rank) = find_meeting_vertex(&fwd_dist, &bwd_dist, max_depth) {
            return reconstruct_bidirectional_path(&indexed, &fwd_prev, &bwd_next, meet_rank);
        }
    }

    ShortestPathResult::empty()
}

fn expand_bfs_level(
    frontier: &[usize],
    adj_list: &[Vec<(usize, AlgoEdge)>],
    step_num: usize,
    dist: &mut BTreeMap<usize, usize>,
    trace: &mut BTreeMap<usize, (usize, AlgoEdge)>,
) -> Vec<usize> {
    let mut next_frontier = Vec::new();
    for &curr_rank in frontier {
        if let Some(adj) = adj_list.get(curr_rank) {
            for &(nbr_rank, edge) in adj {
                if let std::collections::btree_map::Entry::Vacant(vacant) = dist.entry(nbr_rank) {
                    vacant.insert(step_num);
                    trace.insert(nbr_rank, (curr_rank, edge));
                    next_frontier.push(nbr_rank);
                }
            }
        }
    }
    next_frontier.sort_unstable();
    next_frontier
}

fn find_meeting_vertex(
    fwd_dist: &BTreeMap<usize, usize>,
    bwd_dist: &BTreeMap<usize, usize>,
    max_depth: usize,
) -> Option<usize> {
    let mut best_meet: Option<(usize, usize)> = None;
    for (&node_rank, &d_f) in fwd_dist {
        if let Some(&d_b) = bwd_dist.get(&node_rank) {
            let total_hops = d_f.saturating_add(d_b);
            if total_hops <= max_depth {
                match best_meet {
                    None => best_meet = Some((node_rank, total_hops)),
                    Some((best_r, best_h))
                        if total_hops < best_h || (total_hops == best_h && node_rank < best_r) =>
                    {
                        best_meet = Some((node_rank, total_hops));
                    }
                    Some(_) => {}
                }
            }
        }
    }
    best_meet.map(|(rank, _)| rank)
}

type AdjList = Vec<Vec<(usize, AlgoEdge)>>;

fn build_bidirectional_adj(graph: &WeightedDiGraph, indexed: &IndexedNodes) -> (AdjList, AdjList) {
    let n = indexed.nodes.len();
    let mut outgoing: AdjList = Vec::with_capacity(n);
    let mut incoming: AdjList = Vec::with_capacity(n);

    for &(node_uuid, node_idx) in &indexed.nodes {
        let mut out_edges = Vec::new();
        for edge in graph.edges_directed(node_idx, Direction::Outgoing) {
            let w = *edge.weight();
            if let Some(&dst_rank) = indexed.rank_by_idx.get(&edge.target())
                && let Some(&(dst_uuid, _)) = indexed.nodes.get(dst_rank)
            {
                out_edges.push((
                    dst_rank,
                    AlgoEdge {
                        src: node_uuid,
                        dst: dst_uuid,
                        weight: w,
                    },
                ));
            }
        }
        out_edges
            .sort_by(|(ra, ea), (rb, eb)| ra.cmp(rb).then_with(|| eb.weight.total_cmp(&ea.weight)));
        out_edges.dedup_by_key(|(r, _)| *r);
        outgoing.push(out_edges);

        let mut in_edges = Vec::new();
        for edge in graph.edges_directed(node_idx, Direction::Incoming) {
            let w = *edge.weight();
            if let Some(&src_rank) = indexed.rank_by_idx.get(&edge.source())
                && let Some(&(src_uuid, _)) = indexed.nodes.get(src_rank)
            {
                in_edges.push((
                    src_rank,
                    AlgoEdge {
                        src: src_uuid,
                        dst: node_uuid,
                        weight: w,
                    },
                ));
            }
        }
        in_edges
            .sort_by(|(ra, ea), (rb, eb)| ra.cmp(rb).then_with(|| eb.weight.total_cmp(&ea.weight)));
        in_edges.dedup_by_key(|(r, _)| *r);
        incoming.push(in_edges);
    }

    (outgoing, incoming)
}

fn reconstruct_bidirectional_path(
    indexed: &IndexedNodes,
    fwd_prev: &BTreeMap<usize, (usize, AlgoEdge)>,
    bwd_next: &BTreeMap<usize, (usize, AlgoEdge)>,
    meet_rank: usize,
) -> ShortestPathResult {
    let mut prefix_ranks = vec![meet_rank];
    let mut prefix_edges = Vec::new();
    let mut cursor = meet_rank;
    while let Some(&(parent_rank, edge)) = fwd_prev.get(&cursor) {
        prefix_ranks.push(parent_rank);
        prefix_edges.push(edge);
        cursor = parent_rank;
    }
    prefix_ranks.reverse();
    prefix_edges.reverse();

    let mut path_ranks = prefix_ranks;
    let mut edges = prefix_edges;

    let mut bwd_cursor = meet_rank;
    while let Some(&(next_rank, edge)) = bwd_next.get(&bwd_cursor) {
        path_ranks.push(next_rank);
        edges.push(edge);
        bwd_cursor = next_rank;
    }

    let mut path = Vec::with_capacity(path_ranks.len());
    for r in path_ranks {
        let Some(&(uuid, _)) = indexed.nodes.get(r) else {
            return ShortestPathResult::empty();
        };
        path.push(uuid);
    }

    let hops_f64 = u32::try_from(edges.len()).map_or(f64::INFINITY, f64::from);
    ShortestPathResult {
        path,
        edges,
        total_cost: Some(hops_f64),
    }
}
