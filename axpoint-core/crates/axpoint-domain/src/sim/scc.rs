//! Iterative Tarjan strongly connected component (`SCC`) condensation in causal topological order (`09-STEP10` §10.4).

/// Condensed DAG of strongly connected components in causal topological order (sources first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SccGraph {
    /// Components ordered topologically; each component holds vertex indices in `0..n`.
    pub components: Vec<Vec<usize>>,
}

impl SccGraph {
    /// Builds a vertex-to-component-index lookup table of length `n`.
    #[must_use]
    pub fn component_ids(&self, n: usize) -> Vec<usize> {
        let mut comp_of = vec![usize::MAX; n];
        for (cid, comp) in self.components.iter().enumerate() {
            for &v in comp {
                if let Some(slot) = comp_of.get_mut(v) {
                    *slot = cid;
                }
            }
        }
        comp_of
    }
}

/// Returns `true` if `comp` contains a cycle (either multiple vertices or a self-loop).
#[must_use]
pub fn is_cyclic_component(comp: &[usize], adj: &[Vec<usize>]) -> bool {
    if comp.len() > 1 {
        return true;
    }
    if let Some(&u) = comp.first()
        && let Some(neighbors) = adj.get(u)
    {
        return neighbors.contains(&u);
    }
    false
}

/// Computes Tarjan SCCs for `n` vertices and directed edges `adj[u] -> v` in causal topological order.
///
/// Uses an explicit call stack so deep chains (`50_000+` nodes) never overflow the thread stack.
#[must_use]
pub fn tarjan_scc_topo(n: usize, adj: &[Vec<usize>]) -> SccGraph {
    let mut state = TarjanState::new(n);

    for start in 0..n {
        if state.indices.get(start).copied() == Some(usize::MAX) {
            state.push_vertex(start);
            state.drain_dfs(adj);
        }
    }

    state.components.reverse();
    SccGraph {
        components: state.components,
    }
}

struct TarjanState {
    indices: Vec<usize>,
    lowlink: Vec<usize>,
    on_stack: Vec<bool>,
    tarjan_stack: Vec<usize>,
    call_stack: Vec<(usize, usize)>,
    next_index: usize,
    components: Vec<Vec<usize>>,
}

impl TarjanState {
    fn new(n: usize) -> Self {
        Self {
            indices: vec![usize::MAX; n],
            lowlink: vec![0; n],
            on_stack: vec![false; n],
            tarjan_stack: Vec::new(),
            call_stack: Vec::new(),
            next_index: 0,
            components: Vec::new(),
        }
    }

    fn push_vertex(&mut self, v: usize) {
        let idx = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        if let Some(slot) = self.indices.get_mut(v) {
            *slot = idx;
        }
        if let Some(slot) = self.lowlink.get_mut(v) {
            *slot = idx;
        }
        if let Some(slot) = self.on_stack.get_mut(v) {
            *slot = true;
        }
        self.tarjan_stack.push(v);
        self.call_stack.push((v, 0));
    }

    fn drain_dfs(&mut self, adj: &[Vec<usize>]) {
        while let Some(&(u, edge_pos)) = self.call_stack.last() {
            let next_neighbor = adj.get(u).and_then(|nbrs| nbrs.get(edge_pos).copied());
            if let Some(v) = next_neighbor {
                if let Some(frame) = self.call_stack.last_mut() {
                    frame.1 = frame.1.saturating_add(1);
                }
                self.visit_edge(u, v);
            } else {
                self.finish_vertex(u);
            }
        }
    }

    fn visit_edge(&mut self, u: usize, v: usize) {
        match self.indices.get(v).copied() {
            Some(usize::MAX) => {
                self.push_vertex(v);
            }
            Some(v_idx) if self.on_stack.get(v).copied() == Some(true) => {
                if let Some(low_u) = self.lowlink.get_mut(u) {
                    *low_u = (*low_u).min(v_idx);
                }
            }
            _ => {}
        }
    }

    fn finish_vertex(&mut self, u: usize) {
        self.call_stack.pop();
        if let Some(&(parent, _)) = self.call_stack.last()
            && let Some(low_u) = self.lowlink.get(u).copied()
            && let Some(low_p) = self.lowlink.get_mut(parent)
        {
            *low_p = (*low_p).min(low_u);
        }

        if self.lowlink.get(u) == self.indices.get(u) {
            let mut comp = Vec::new();
            while let Some(w) = self.tarjan_stack.pop() {
                if let Some(flag) = self.on_stack.get_mut(w) {
                    *flag = false;
                }
                comp.push(w);
                if w == u {
                    break;
                }
            }
            self.components.push(comp);
        }
    }
}
