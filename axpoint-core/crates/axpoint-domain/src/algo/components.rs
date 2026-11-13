//! Directed strongly connected components (`kosaraju_scc`) and undirected
//! weakly connected components (`UnionFind`) with deterministic community IDs.

use std::collections::BTreeMap;

use petgraph::algo::kosaraju_scc;
use petgraph::unionfind::UnionFind;
use petgraph::visit::EdgeRef as _;

use super::{
    IndexedNodes, NodeAlgoOutput, WeightedDiGraph, sort_by_community_size_desc, usize_to_i64,
};
use crate::AppError;

/// Computes directed strongly connected components (SCC) via `Kosaraju`'s algorithm.
///
/// Each component is assigned a deterministic `i64` identifier equal to the minimum
/// [`uuid::Uuid`]-sorted rank (`0..N`) among all nodes belonging to that component.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if node count overflows `i64`.
pub fn strongly_connected_components(
    graph: &WeightedDiGraph,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    let indexed = IndexedNodes::from_graph(graph);
    let n = indexed.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let sccs = kosaraju_scc(graph);
    let mut comm_by_rank: BTreeMap<usize, i64> = BTreeMap::new();

    for comp in sccs {
        let min_rank = comp
            .iter()
            .filter_map(|nx| indexed.rank_by_idx.get(nx).copied())
            .min();

        if let Some(min_r) = min_rank {
            let comm_id = usize_to_i64(min_r)?;
            for nx in comp {
                if let Some(&r) = indexed.rank_by_idx.get(&nx) {
                    comm_by_rank.insert(r, comm_id);
                }
            }
        }
    }

    let mut outputs = Vec::with_capacity(n);
    for (rank, &(node_id, _)) in indexed.nodes.iter().enumerate() {
        let fallback = usize_to_i64(rank)?;
        let comm = comm_by_rank.get(&rank).copied().unwrap_or(fallback);
        outputs.push(NodeAlgoOutput {
            node_id,
            score: None,
            community: Some(comm),
        });
    }

    sort_by_community_size_desc(&mut outputs);
    Ok(outputs)
}

/// Computes undirected weakly connected components (WCC) via [`UnionFind`], ignoring edge orientation.
///
/// Each component is assigned a deterministic `i64` identifier equal to the minimum
/// [`uuid::Uuid`]-sorted rank (`0..N`) among all nodes belonging to that component.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if node count overflows `i64`.
pub fn weakly_connected_components(
    graph: &WeightedDiGraph,
) -> Result<Vec<NodeAlgoOutput>, AppError> {
    let indexed = IndexedNodes::from_graph(graph);
    let n = indexed.nodes.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut uf = UnionFind::<usize>::new(n);
    for edge in graph.edge_references() {
        if let (Some(&src_r), Some(&dst_r)) = (
            indexed.rank_by_idx.get(&edge.source()),
            indexed.rank_by_idx.get(&edge.target()),
        ) {
            uf.union(src_r, dst_r);
        }
    }

    let mut min_rank_by_rep: BTreeMap<usize, usize> = BTreeMap::new();
    for rank in 0..n {
        let rep = uf.find(rank);
        min_rank_by_rep
            .entry(rep)
            .and_modify(|curr| {
                if rank < *curr {
                    *curr = rank;
                }
            })
            .or_insert(rank);
    }

    let mut outputs = Vec::with_capacity(n);
    for (rank, &(node_id, _)) in indexed.nodes.iter().enumerate() {
        let rep = uf.find(rank);
        let min_r = min_rank_by_rep.get(&rep).copied().unwrap_or(rank);
        let comm = usize_to_i64(min_r)?;
        outputs.push(NodeAlgoOutput {
            node_id,
            score: None,
            community: Some(comm),
        });
    }

    sort_by_community_size_desc(&mut outputs);
    Ok(outputs)
}
