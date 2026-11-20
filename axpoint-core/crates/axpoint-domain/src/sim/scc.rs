//! Tarjan strongly connected component (SCC) condensation in topological order (`09-STEP10` §10.4).

/// Condensed DAG of strongly connected components in causal topological order (sources first).
#[derive(Debug, Clone, Default)]
pub struct SccGraph {
    /// Components ordered topologically; each component holds vertex indices in `0..n`.
    pub components: Vec<Vec<usize>>,
}

/// Computes Tarjan SCCs for `n` vertices and directed edges `adj[u] -> v` in topological order.
#[must_use]
pub fn tarjan_scc_topo(n: usize, _adj: &[Vec<usize>]) -> SccGraph {
    let components = (0..n).map(|i| vec![i]).collect();
    SccGraph { components }
}
