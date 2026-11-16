//! Directed acyclic graph adjacency, ancestor discovery, and Kahn cycle detection (`07-STEP07-08` §07.2).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use axpoint_domain::flow::{FlowEdge, FlowGraph, FlowNode};

/// Builds a borrowed lookup map from `node.id` to `&FlowNode`.
///
/// If duplicate IDs exist in an unvalidated draft, the first occurrence is retained.
#[must_use]
pub fn node_map(graph: &FlowGraph) -> HashMap<&str, &FlowNode> {
    let mut map = HashMap::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        map.entry(node.id.as_str()).or_insert(node);
    }
    map
}

/// Returns all incoming [`FlowEdge`]s targeting `node_id` (`edge.to == node_id`), in declaration order.
#[must_use]
pub fn predecessors<'a>(graph: &'a FlowGraph, node_id: &str) -> Vec<&'a FlowEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.to == node_id)
        .collect()
}

/// Returns all outgoing [`FlowEdge`]s originating at `node_id` (`edge.from == node_id`), in declaration order.
#[must_use]
pub fn successors<'a>(graph: &'a FlowGraph, node_id: &str) -> Vec<&'a FlowEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from == node_id)
        .collect()
}

/// Computes the transitive set of strict ancestor node IDs able to reach `node_id`.
///
/// Cycle-safe even on unvalidated cyclic drafts; `node_id` itself is excluded from the returned set.
#[must_use]
pub fn ancestors(graph: &FlowGraph, node_id: &str) -> BTreeSet<String> {
    let known: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut incoming: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if known.contains(edge.from.as_str()) && known.contains(edge.to.as_str()) {
            incoming
                .entry(edge.to.as_str())
                .or_default()
                .push(edge.from.as_str());
        }
    }

    let mut result = BTreeSet::new();
    let mut visited = HashSet::new();
    visited.insert(node_id);
    let mut queue = VecDeque::new();
    queue.push_back(node_id);

    while let Some(curr) = queue.pop_front() {
        if let Some(preds) = incoming.get(curr) {
            for &pred in preds {
                if pred != node_id {
                    result.insert(String::from(pred));
                }
                if visited.insert(pred) {
                    queue.push_back(pred);
                }
            }
        }
    }

    result
}

/// Computes all nodes reachable downstream from `seed_node_ids` (excluding the seeds unless re-entered).
#[must_use]
pub fn strict_descendants(graph: &FlowGraph, seed_node_ids: &[String]) -> BTreeSet<String> {
    let known: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if known.contains(edge.from.as_str()) && known.contains(edge.to.as_str()) {
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }
    }

    let mut result = BTreeSet::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    for seed in seed_node_ids {
        visited.insert(seed.as_str());
        queue.push_back(seed.as_str());
    }

    while let Some(curr) = queue.pop_front() {
        if let Some(succs) = outgoing.get(curr) {
            for &succ in succs {
                result.insert(String::from(succ));
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }

    result
}

/// Returns root [`FlowNode`]s (zero incoming edges from known nodes), preserving `graph.nodes` order.
#[must_use]
pub fn root_nodes(graph: &FlowGraph) -> Vec<&FlowNode> {
    let known: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut has_incoming = HashSet::new();
    for edge in &graph.edges {
        if known.contains(edge.from.as_str()) && known.contains(edge.to.as_str()) {
            has_incoming.insert(edge.to.as_str());
        }
    }
    graph
        .nodes
        .iter()
        .filter(|node| !has_incoming.contains(node.id.as_str()))
        .collect()
}

/// Returns terminal leaf [`FlowNode`]s (zero outgoing edges to known nodes), preserving `graph.nodes` order.
#[must_use]
pub fn leaf_nodes(graph: &FlowGraph) -> Vec<&FlowNode> {
    let known: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut has_outgoing = HashSet::new();
    for edge in &graph.edges {
        if known.contains(edge.from.as_str()) && known.contains(edge.to.as_str()) {
            has_outgoing.insert(edge.from.as_str());
        }
    }
    graph
        .nodes
        .iter()
        .filter(|node| !has_outgoing.contains(node.id.as_str()))
        .collect()
}

/// Computes a deterministic Kahn topological ordering of unique known node IDs,
/// or extracts a closed cycle path `["a", "b", "a"]` if a directed cycle exists (`07-STEP07-08` §07.2 rule 3).
///
/// Self-loops (`from == to`) and edges referencing unknown endpoints are ignored here because
/// static validation rule 2 reports them separately as `INVALID_ARGUMENT`.
///
/// # Errors
/// Returns `Err(cycle_path)` containing a closed directed cycle `["u", ..., "u"]` when `graph` contains a cycle.
pub fn topological_order_or_cycle(graph: &FlowGraph) -> Result<Vec<String>, Vec<String>> {
    let mut unique_order: Vec<&str> = Vec::with_capacity(graph.nodes.len());
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    for node in &graph.nodes {
        let id = node.id.as_str();
        if !in_degree.contains_key(id) {
            unique_order.push(id);
            in_degree.insert(id, 0);
        }
    }

    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut rev_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut seen_edges: HashSet<(&str, &str)> = HashSet::new();

    for edge in &graph.edges {
        let from = edge.from.as_str();
        let to = edge.to.as_str();
        if from == to || !in_degree.contains_key(from) || !in_degree.contains_key(to) {
            continue;
        }
        if seen_edges.insert((from, to)) {
            adj.entry(from).or_default().push(to);
            rev_adj.entry(to).or_default().push(from);
            if let Some(deg) = in_degree.get_mut(to) {
                *deg = deg.saturating_add(1);
            }
        }
    }

    let mut ready = VecDeque::new();
    for &id in &unique_order {
        if in_degree.get(id).copied() == Some(0) {
            ready.push_back(id);
        }
    }

    let mut sorted = Vec::with_capacity(unique_order.len());
    let mut settled = HashSet::with_capacity(unique_order.len());

    while let Some(u) = ready.pop_front() {
        sorted.push(String::from(u));
        settled.insert(u);
        if let Some(neighbors) = adj.get(u) {
            for &v in neighbors {
                if let Some(deg) = in_degree.get_mut(v) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.push_back(v);
                    }
                }
            }
        }
    }

    if sorted.len() == unique_order.len() {
        return Ok(sorted);
    }

    Err(extract_cycle_from_remainder(
        &unique_order,
        &settled,
        &rev_adj,
    ))
}

fn extract_cycle_from_remainder(
    unique_order: &[&str],
    settled: &HashSet<&str>,
    rev_adj: &HashMap<&str, Vec<&str>>,
) -> Vec<String> {
    let Some(&start) = unique_order.iter().find(|&&id| !settled.contains(id)) else {
        return Vec::new();
    };

    // Every unsettled node in Kahn's remainder has >= 1 unsettled predecessor,
    // so walking backwards along `rev_adj` must revisit a vertex within `<= V` steps.
    let mut rev_walk: Vec<&str> = vec![start];
    let mut pos_by_node: HashMap<&str, usize> = HashMap::from([(start, 0)]);
    let mut curr = start;

    while let Some(preds) = rev_adj.get(curr) {
        let Some(&pred) = preds.iter().find(|&&p| !settled.contains(p)) else {
            break;
        };
        if let Some(&loop_idx) = pos_by_node.get(pred) {
            let mut cycle: Vec<&str> = rev_walk.iter().skip(loop_idx).copied().collect();
            cycle.reverse();
            let rotated = rotate_cycle_to_canonical(&cycle, unique_order);
            return rotated;
        }
        pos_by_node.insert(pred, rev_walk.len());
        rev_walk.push(pred);
        curr = pred;
    }

    vec![String::from(start), String::from(start)]
}

fn rotate_cycle_to_canonical(cycle: &[&str], unique_order: &[&str]) -> Vec<String> {
    if cycle.is_empty() {
        return Vec::new();
    }
    let rank_of: HashMap<&str, usize> = unique_order
        .iter()
        .enumerate()
        .map(|(idx, &id)| (id, idx))
        .collect();

    let best_idx = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, id)| rank_of.get(**id).copied().unwrap_or(usize::MAX))
        .map_or(0, |(idx, _)| idx);

    let mut out = Vec::with_capacity(cycle.len().saturating_add(1));
    for &id in cycle.iter().skip(best_idx) {
        out.push(String::from(id));
    }
    for &id in cycle.iter().take(best_idx) {
        out.push(String::from(id));
    }
    if let Some(first) = out.first().cloned() {
        out.push(first);
    }
    out
}
