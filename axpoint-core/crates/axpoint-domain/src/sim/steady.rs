//! Steady-state causal propagation solver (`SCC` + damped Jacobi iteration, `09-STEP10` §10.4).

use std::collections::{HashMap, VecDeque};

use uuid::Uuid;

use crate::AppError;
use crate::enums::InterventionMode;
use crate::sim::action::normalize_actions_indexed;
use crate::sim::explain::{compute_hop_distances_indexed, compute_top_paths_indexed};
use crate::sim::graph::{
    EdgeSaturation, IndexedSimGraph, PropagationClamp, PropagationCurve, aggregate_kpis,
    apply_clamp, eval_edge_delta, merge_clamps,
};
use crate::sim::scc::{is_cyclic_component, tarjan_scc_topo};
use crate::sim::{
    Checkpoint, ImpactRow, NormalizedAction, SimGraph, SimParams, SimResult, SimWarning,
};

/// Active `(node, metric)` vertex inside a compiled [`ChannelSystem`].
#[derive(Debug, Clone)]
pub(crate) struct ChannelVertex {
    pub(crate) node_idx: usize,
    pub(crate) metric: String,
    pub(crate) before_val: Option<f64>,
    pub(crate) clamp: PropagationClamp,
    pub(crate) fixed_actions: Vec<ChannelFixedAction>,
}

/// Intervention bound to a [`ChannelVertex`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelFixedAction {
    pub(crate) mode: InterventionMode,
    pub(crate) value: f64,
    pub(crate) at_step: u32,
}

/// Directed propagation edge between active [`ChannelVertex`] entries.
#[derive(Debug, Clone)]
pub(crate) struct ChannelEdge {
    pub(crate) src_chan: usize,
    pub(crate) dst_chan: usize,
    pub(crate) mode: InterventionMode,
    pub(crate) gain: f64,
    pub(crate) delay_steps: u32,
    pub(crate) curve: PropagationCurve,
    pub(crate) saturation: EdgeSaturation,
}

/// Pre-compiled active channel system used by `steady`, `stepped`, and `uncertainty` solvers.
#[derive(Debug, Clone)]
pub(crate) struct ChannelSystem {
    pub(crate) vertices: Vec<ChannelVertex>,
    pub(crate) edges: Vec<ChannelEdge>,
    pub(crate) in_edges: Vec<Vec<usize>>,
    pub(crate) seeds: Vec<usize>,
    pub(crate) hop_distances: Vec<Option<u32>>,
}

/// Numerical output from solving a [`ChannelSystem`] (lightweight for Monte-Carlo reuse).
#[derive(Debug, Clone)]
pub(crate) struct SteadySolveOutput {
    pub(crate) after_vals: Vec<f64>,
    pub(crate) converged: bool,
    pub(crate) reason: Option<String>,
    pub(crate) iterations: u32,
    pub(crate) max_iter_warning_nodes: Vec<Uuid>,
    pub(crate) iter_snapshots: Vec<(u32, Vec<(usize, f64)>)>,
}

/// Validates solver parameters shared by `steady` and `stepped` solvers.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `damping`, `tolerance`, or `min_effect`
/// are non-finite or out of valid domain.
pub(crate) fn validate_common_params(params: &SimParams) -> Result<(), AppError> {
    if !params.damping.is_finite() || params.damping <= 0.0 || params.damping > 1.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "damping must be finite and in (0.0, 1.0]",
        ));
    }
    if !params.tolerance.is_finite() || params.tolerance <= 0.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "tolerance must be finite and > 0.0",
        ));
    }
    if !params.min_effect.is_finite() || params.min_effect < 0.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "min_effect must be finite and >= 0.0",
        ));
    }
    Ok(())
}

/// Solves steady-state causal propagation on `graph` under `actions` and `params` (`§10.4`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SIM_DIVERGENT`) when action/param
/// validation fails or a cyclic SCC diverges monotonically.
pub fn solve(
    graph: &SimGraph,
    actions: &[AxSimActionCompat],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    let indexed = IndexedSimGraph::from_sim_graph(graph)?;
    solve_indexed(&indexed, actions, params)
}

/// Type alias avoiding redundant imports while keeping the signature clear.
type AxSimActionCompat = crate::topology::AxSimAction;

/// Solves steady-state causal propagation on a pre-compiled [`IndexedSimGraph`] (`§10.4`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SIM_DIVERGENT`) when action/param
/// validation fails or a cyclic SCC diverges monotonically.
pub fn solve_indexed(
    indexed: &IndexedSimGraph,
    actions: &[AxSimActionCompat],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    validate_common_params(params)?;
    let normalized = normalize_actions_indexed(indexed, actions)?;
    if normalized.is_empty() {
        return Ok(empty_sim_result());
    }

    let system = build_channel_system(indexed, &normalized, params.max_hops);
    let adj = build_channel_adj(&system, false);
    let scc_graph = tarjan_scc_topo(system.vertices.len(), &adj);
    let edge_scales = vec![1.0_f64; system.edges.len()];

    let numeric = solve_steady_numeric(
        indexed,
        &system,
        &adj,
        &scc_graph,
        params,
        &edge_scales,
        true,
    )?;

    assemble_steady_result(indexed, &system, &numeric, params)
}

pub(crate) fn empty_sim_result() -> SimResult {
    SimResult {
        impacts: Vec::new(),
        affected_count: 0,
        kpis: Vec::new(),
        warnings: Vec::new(),
        converged: true,
        reason: None,
        iterations: 0,
        checkpoints: Vec::new(),
    }
}

pub(crate) fn build_channel_system(
    indexed: &IndexedSimGraph,
    actions: &[NormalizedAction],
    max_hops: Option<u32>,
) -> ChannelSystem {
    let mut vertices: Vec<ChannelVertex> = Vec::new();
    let mut chan_map: HashMap<(usize, String), usize> = HashMap::new();
    let mut seeds: Vec<usize> = Vec::new();
    let mut queue: VecDeque<(usize, u32)> = VecDeque::new();

    for act in actions {
        let Some(&node_idx) = indexed.node_index.get(&act.target_node_id) else {
            continue;
        };
        let chan_idx = get_or_insert_channel(
            indexed,
            &mut vertices,
            &mut chan_map,
            node_idx,
            &act.metric_key,
        );
        if let Some(v) = vertices.get_mut(chan_idx) {
            if v.fixed_actions.is_empty() {
                seeds.push(chan_idx);
                queue.push_back((chan_idx, 0));
            }
            v.fixed_actions.push(ChannelFixedAction {
                mode: act.mode,
                value: act.value,
                at_step: act.at_step,
            });
        }
    }

    let mut edges: Vec<ChannelEdge> = Vec::new();
    discover_reachable_channels(
        indexed,
        max_hops,
        &mut vertices,
        &mut chan_map,
        &mut edges,
        &mut queue,
    );

    // Apply `do`-operator: drop edges targeting vertices fixed at step 0 in steady mode,
    // and retain edges for stepped mode (stepped solver masks incoming edges dynamically by `at_step`).
    let mut in_edges = vec![Vec::new(); vertices.len()];
    for (ei, e) in edges.iter().enumerate() {
        if let Some(list) = in_edges.get_mut(e.dst_chan) {
            list.push(ei);
        }
    }

    let plain_adj = build_channel_adj_raw(vertices.len(), &edges, &vertices, false);
    let hop_distances = compute_hop_distances_indexed(vertices.len(), &seeds, &plain_adj);

    ChannelSystem {
        vertices,
        edges,
        in_edges,
        seeds,
        hop_distances,
    }
}

fn get_or_insert_channel(
    indexed: &IndexedSimGraph,
    vertices: &mut Vec<ChannelVertex>,
    chan_map: &mut HashMap<(usize, String), usize>,
    node_idx: usize,
    metric: &str,
) -> usize {
    let key = (node_idx, metric.to_string());
    if let Some(&idx) = chan_map.get(&key) {
        return idx;
    }
    let idx = vertices.len();
    let (before_val, clamp) = match indexed.nodes.get(node_idx) {
        Some(node) => (
            node.metrics.get(metric).copied(),
            node.metric_bounds.get(metric).cloned().unwrap_or_default(),
        ),
        None => (None, PropagationClamp::default()),
    };
    vertices.push(ChannelVertex {
        node_idx,
        metric: metric.to_string(),
        before_val,
        clamp,
        fixed_actions: Vec::new(),
    });
    chan_map.insert(key, idx);
    idx
}

fn discover_reachable_channels(
    indexed: &IndexedSimGraph,
    max_hops: Option<u32>,
    vertices: &mut Vec<ChannelVertex>,
    chan_map: &mut HashMap<(usize, String), usize>,
    edges: &mut Vec<ChannelEdge>,
    queue: &mut VecDeque<(usize, u32)>,
) {
    let mut expanded: HashMap<usize, u32> = HashMap::new();
    while let Some((src_chan, hop)) = queue.pop_front() {
        if max_hops.is_some_and(|limit| hop >= limit) {
            continue;
        }
        if expanded.get(&src_chan).is_some_and(|&prev| prev <= hop) {
            continue;
        }
        let first_expand = !expanded.contains_key(&src_chan);
        expanded.insert(src_chan, hop);
        if !first_expand {
            continue;
        }

        let Some(src_vert) = vertices.get(src_chan).cloned() else {
            continue;
        };
        let Some(out_indices) = indexed.out_edges.get(src_vert.node_idx) else {
            continue;
        };
        let next_hop = hop.saturating_add(1);

        for &edge_idx in out_indices {
            let Some(ce) = indexed.edges.get(edge_idx) else {
                continue;
            };
            let matches_src = ce
                .from_metric
                .as_deref()
                .is_none_or(|m| m == src_vert.metric);
            if !matches_src {
                continue;
            }
            let dst_metric = ce.to_metric.as_deref().unwrap_or(&src_vert.metric);
            let dst_node = ce.dst_node.index();
            let dst_chan = get_or_insert_channel(indexed, vertices, chan_map, dst_node, dst_metric);

            // Update target clamp with edge spec clamp when node did not override explicitly.
            if let Some(dst_v) = vertices.get_mut(dst_chan) {
                let has_explicit_node_bound = indexed
                    .nodes
                    .get(dst_node)
                    .is_some_and(|n| n.metric_bounds.contains_key(dst_metric));
                if !has_explicit_node_bound {
                    dst_v.clamp = merge_clamps(&dst_v.clamp, &ce.clamp);
                }
            }

            edges.push(ChannelEdge {
                src_chan,
                dst_chan,
                mode: ce.mode,
                gain: ce.gain,
                delay_steps: ce.delay_steps,
                curve: ce.curve.clone(),
                saturation: ce.saturation,
            });
            queue.push_back((dst_chan, next_hop));
        }
    }
}

pub(crate) fn build_channel_adj(system: &ChannelSystem, zero_lag_only: bool) -> Vec<Vec<usize>> {
    build_channel_adj_raw(
        system.vertices.len(),
        &system.edges,
        &system.vertices,
        zero_lag_only,
    )
}

fn build_channel_adj_raw(
    n: usize,
    edges: &[ChannelEdge],
    vertices: &[ChannelVertex],
    zero_lag_only: bool,
) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); n];
    for e in edges {
        if zero_lag_only && e.delay_steps != 0 {
            continue;
        }
        let dst_fixed = vertices
            .get(e.dst_chan)
            .is_some_and(|v| !v.fixed_actions.is_empty());
        if dst_fixed {
            continue;
        }
        if let Some(list) = adj.get_mut(e.src_chan) {
            list.push(e.dst_chan);
        }
    }
    adj
}

pub(crate) fn solve_steady_numeric(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    adj: &[Vec<usize>],
    scc_graph: &crate::sim::SccGraph,
    params: &SimParams,
    edge_scales: &[f64],
    record_snapshots: bool,
) -> Result<SteadySolveOutput, AppError> {
    let n = system.vertices.len();
    let comp_of = scc_graph.component_ids(n);
    let mut rel_x = vec![0.0_f64; n];
    let mut abs_x = vec![0.0_f64; n];
    let mut after_vals = vec![0.0_f64; n];
    let mut converged = true;
    let mut reason: Option<String> = None;
    let mut max_iters_used = 0_u32;
    let mut max_iter_warning_nodes = Vec::new();
    let mut snapshot_map: HashMap<u32, Vec<(usize, f64)>> = HashMap::new();

    for (cid, comp) in scc_graph.components.iter().enumerate() {
        if is_cyclic_component(comp, adj) {
            let ctx = CyclicSccCtx {
                cid,
                comp,
                comp_of: &comp_of,
                params,
                edge_scales,
                record_snapshots,
            };
            let outcome = solve_cyclic_scc(
                indexed,
                system,
                &ctx,
                &mut rel_x,
                &mut abs_x,
                &mut snapshot_map,
            )?;
            max_iters_used = max_iters_used.max(outcome.iters_used);
            if !outcome.converged {
                converged = false;
                reason = Some(String::from("MAX_ITER"));
                if let Some(node_id) = outcome.representative_node {
                    max_iter_warning_nodes.push(node_id);
                }
            }
        } else if let Some(&u) = comp.first() {
            solve_acyclic_vertex(system, u, edge_scales, &mut rel_x, &mut abs_x);
        }
    }

    for (u, vert) in system.vertices.iter().enumerate() {
        let rx = rel_x.get(u).copied().unwrap_or(0.0);
        let ax = abs_x.get(u).copied().unwrap_or(0.0);
        let after = compute_vertex_after_val(vert, rx, ax, true);
        if let Some(slot) = after_vals.get_mut(u) {
            *slot = after;
        }
    }

    let mut iter_snapshots: Vec<(u32, Vec<(usize, f64)>)> = snapshot_map.into_iter().collect();
    iter_snapshots.sort_by_key(|(iter, _)| *iter);

    Ok(SteadySolveOutput {
        after_vals,
        converged,
        reason,
        iterations: max_iters_used,
        max_iter_warning_nodes,
        iter_snapshots,
    })
}

struct CyclicSccCtx<'a> {
    cid: usize,
    comp: &'a [usize],
    comp_of: &'a [usize],
    params: &'a SimParams,
    edge_scales: &'a [f64],
    record_snapshots: bool,
}

struct CyclicSccOutcome {
    iters_used: u32,
    converged: bool,
    representative_node: Option<Uuid>,
}

struct CyclicPrep {
    ext_rel: Vec<f64>,
    ext_abs: Vec<f64>,
    int_edges: Vec<Vec<usize>>,
}

fn solve_acyclic_vertex(
    system: &ChannelSystem,
    u: usize,
    edge_scales: &[f64],
    rel_x: &mut [f64],
    abs_x: &mut [f64],
) {
    let Some(vert) = system.vertices.get(u) else {
        return;
    };
    if !vert.fixed_actions.is_empty() {
        let (rx, ax) = evaluate_fixed_actions(vert, None);
        set_pair(rel_x, abs_x, u, rx, ax);
        return;
    }

    let (rel_in, abs_in) = accumulate_incoming_all(system, u, edge_scales, rel_x, abs_x);
    let (rx, ax) = combine_channel_deltas(vert.before_val, rel_in, abs_in);
    set_pair(rel_x, abs_x, u, rx, ax);
}

fn prepare_cyclic_scc(
    system: &ChannelSystem,
    ctx: &CyclicSccCtx<'_>,
    rel_x: &[f64],
    abs_x: &[f64],
) -> CyclicPrep {
    let m = ctx.comp.len();
    let mut ext_rel = vec![0.0_f64; m];
    let mut ext_abs = vec![0.0_f64; m];
    let mut int_edges: Vec<Vec<usize>> = vec![Vec::new(); m];

    for (local_i, &u) in ctx.comp.iter().enumerate() {
        if let Some(in_list) = system.in_edges.get(u) {
            for &ei in in_list {
                let Some(e) = system.edges.get(ei) else {
                    continue;
                };
                if ctx.comp_of.get(e.src_chan).copied() == Some(ctx.cid) {
                    if let Some(slot) = int_edges.get_mut(local_i) {
                        slot.push(ei);
                    }
                } else {
                    let scale = ctx.edge_scales.get(ei).copied().unwrap_or(1.0);
                    accumulate_single_edge(
                        e,
                        scale,
                        rel_x,
                        abs_x,
                        ext_rel.get_mut(local_i),
                        ext_abs.get_mut(local_i),
                    );
                }
            }
        }
    }

    CyclicPrep {
        ext_rel,
        ext_abs,
        int_edges,
    }
}

fn jacobi_step_scc(
    system: &ChannelSystem,
    ctx: &CyclicSccCtx<'_>,
    prep: &CyclicPrep,
    cur_rel: &mut [f64],
    cur_abs: &mut [f64],
    rel_x: &mut [f64],
    abs_x: &mut [f64],
) -> Result<f64, AppError> {
    let m = ctx.comp.len();
    let mut next_rel = vec![0.0_f64; m];
    let mut next_abs = vec![0.0_f64; m];
    let alpha = ctx.params.damping;
    let mut max_diff = 0.0_f64;

    for (local_i, &u) in ctx.comp.iter().enumerate() {
        let mut r_sum = prep.ext_rel.get(local_i).copied().unwrap_or(0.0);
        let mut a_sum = prep.ext_abs.get(local_i).copied().unwrap_or(0.0);
        if let Some(e_list) = prep.int_edges.get(local_i) {
            for &ei in e_list {
                if let Some(e) = system.edges.get(ei) {
                    let scale = ctx.edge_scales.get(ei).copied().unwrap_or(1.0);
                    accumulate_single_edge(
                        e,
                        scale,
                        rel_x,
                        abs_x,
                        Some(&mut r_sum),
                        Some(&mut a_sum),
                    );
                }
            }
        }

        let before = system.vertices.get(u).and_then(|v| v.before_val);
        let (target_r, target_a) = combine_channel_deltas(before, r_sum, a_sum);
        let old_r = cur_rel.get(local_i).copied().unwrap_or(0.0);
        let old_a = cur_abs.get(local_i).copied().unwrap_or(0.0);
        let nr = (1.0 - alpha) * old_r + alpha * target_r;
        let na = (1.0 - alpha) * old_a + alpha * target_a;

        if !nr.is_finite() || !na.is_finite() {
            return Err(AppError::bad(
                "SIM_DIVERGENT",
                "cyclic propagation diverged to non-finite value (cycle gain product >= 1)",
            ));
        }

        max_diff = max_diff.max((nr - old_r).abs().max((na - old_a).abs()));
        if let Some(slot) = next_rel.get_mut(local_i) {
            *slot = nr;
        }
        if let Some(slot) = next_abs.get_mut(local_i) {
            *slot = na;
        }
    }

    for (local_i, &u) in ctx.comp.iter().enumerate() {
        let nr = next_rel.get(local_i).copied().unwrap_or(0.0);
        let na = next_abs.get(local_i).copied().unwrap_or(0.0);
        set_pair(cur_rel, cur_abs, local_i, nr, na);
        set_pair(rel_x, abs_x, u, nr, na);
    }

    Ok(max_diff)
}

fn solve_cyclic_scc(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    ctx: &CyclicSccCtx<'_>,
    rel_x: &mut [f64],
    abs_x: &mut [f64],
    snapshot_map: &mut HashMap<u32, Vec<(usize, f64)>>,
) -> Result<CyclicSccOutcome, AppError> {
    let m = ctx.comp.len();
    let prep = prepare_cyclic_scc(system, ctx, rel_x, abs_x);
    let mut cur_rel = vec![0.0_f64; m];
    let mut cur_abs = vec![0.0_f64; m];
    let max_iters = ctx.params.max_iterations;

    let mut prev_diff = f64::INFINITY;
    let mut inc_streak = 0_u32;
    let mut iters_used = 0_u32;
    let mut converged = false;

    for iter in 1..=max_iters {
        iters_used = iter;
        let max_diff =
            jacobi_step_scc(system, ctx, &prep, &mut cur_rel, &mut cur_abs, rel_x, abs_x)?;

        if ctx.record_snapshots && iter.is_power_of_two() {
            let entry = snapshot_map.entry(iter).or_default();
            for &u in ctx.comp {
                if let Some(vert) = system.vertices.get(u) {
                    let rx = rel_x.get(u).copied().unwrap_or(0.0);
                    let ax = abs_x.get(u).copied().unwrap_or(0.0);
                    entry.push((u, compute_vertex_after_val(vert, rx, ax, true)));
                }
            }
        }

        if iter > 1 && max_diff > prev_diff {
            inc_streak = inc_streak.saturating_add(1);
        } else {
            inc_streak = 0;
        }
        prev_diff = max_diff;

        if max_diff < ctx.params.tolerance {
            converged = true;
            break;
        }
    }

    if !converged && max_iters > 50 && inc_streak >= 50 {
        return Err(AppError::bad(
            "SIM_DIVERGENT",
            "simulation diverged monotonically over last 50 iterations (cycle gain product >= 1)",
        ));
    }

    let representative_node = if converged {
        None
    } else {
        ctx.comp
            .first()
            .and_then(|&u| system.vertices.get(u))
            .and_then(|v| indexed.nodes.get(v.node_idx))
            .map(|n| n.id)
    };

    Ok(CyclicSccOutcome {
        iters_used,
        converged,
        representative_node,
    })
}

pub(crate) fn evaluate_fixed_actions(vert: &ChannelVertex, step_limit: Option<u32>) -> (f64, f64) {
    let mut after_val = vert.before_val.unwrap_or(0.0);
    let mut has_any = false;
    for act in &vert.fixed_actions {
        if step_limit.is_some_and(|t| act.at_step > t) {
            continue;
        }
        has_any = true;
        match act.mode {
            InterventionMode::Ratio => {
                after_val *= 1.0 + act.value;
            }
            InterventionMode::Add => {
                after_val += act.value;
            }
            InterventionMode::Absolute => {
                after_val = act.value;
            }
        }
    }
    if !has_any {
        return (0.0, 0.0);
    }
    let clamped = apply_clamp(after_val, &vert.clamp);
    match vert.before_val {
        Some(before) => {
            let abs_d = clamped - before;
            let rel_d = if before.abs() > 1e-12 {
                abs_d / before
            } else {
                0.0
            };
            (rel_d, abs_d)
        }
        None => (0.0, clamped),
    }
}

fn accumulate_incoming_all(
    system: &ChannelSystem,
    u: usize,
    edge_scales: &[f64],
    rel_x: &[f64],
    abs_x: &[f64],
) -> (f64, f64) {
    let mut rel_in = 0.0_f64;
    let mut abs_in = 0.0_f64;
    if let Some(in_list) = system.in_edges.get(u) {
        for &ei in in_list {
            if let Some(e) = system.edges.get(ei) {
                let scale = edge_scales.get(ei).copied().unwrap_or(1.0);
                accumulate_single_edge(
                    e,
                    scale,
                    rel_x,
                    abs_x,
                    Some(&mut rel_in),
                    Some(&mut abs_in),
                );
            }
        }
    }
    (rel_in, abs_in)
}

pub(crate) fn accumulate_single_edge(
    e: &ChannelEdge,
    scale: f64,
    rel_x: &[f64],
    abs_x: &[f64],
    rel_out: Option<&mut f64>,
    abs_out: Option<&mut f64>,
) {
    match e.mode {
        InterventionMode::Ratio => {
            let src_val = rel_x.get(e.src_chan).copied().unwrap_or(0.0);
            let delta = eval_edge_delta(&e.curve, e.gain, e.saturation, src_val, scale);
            if let Some(out) = rel_out {
                *out += delta;
            }
        }
        InterventionMode::Add | InterventionMode::Absolute => {
            let src_val = abs_x.get(e.src_chan).copied().unwrap_or(0.0);
            let delta = eval_edge_delta(&e.curve, e.gain, e.saturation, src_val, scale);
            if let Some(out) = abs_out {
                *out += delta;
            }
        }
    }
}

pub(crate) fn combine_channel_deltas(before: Option<f64>, rel_in: f64, abs_in: f64) -> (f64, f64) {
    match before {
        Some(b) if b.abs() > 1e-12 => (rel_in + abs_in / b, b * rel_in + abs_in),
        Some(_) | None => (rel_in, abs_in),
    }
}

pub(crate) fn compute_vertex_after_val(
    vert: &ChannelVertex,
    _rel_x: f64,
    abs_x: f64,
    _include_all_steps: bool,
) -> f64 {
    let raw = match vert.before_val {
        Some(b) => b + abs_x,
        None => abs_x,
    };
    apply_clamp(raw, &vert.clamp)
}

pub(crate) fn set_pair(rel_x: &mut [f64], abs_x: &mut [f64], u: usize, rx: f64, ax: f64) {
    if let Some(slot) = rel_x.get_mut(u) {
        *slot = rx;
    }
    if let Some(slot) = abs_x.get_mut(u) {
        *slot = ax;
    }
}

pub(crate) fn compute_system_top_paths(system: &ChannelSystem) -> Vec<Vec<usize>> {
    let mut weighted_edges = Vec::with_capacity(system.edges.len());
    for e in &system.edges {
        if system
            .vertices
            .get(e.dst_chan)
            .is_some_and(|v| v.fixed_actions.is_empty())
        {
            weighted_edges.push((e.src_chan, e.dst_chan, e.gain));
        }
    }
    compute_top_paths_indexed(system.vertices.len(), &system.seeds, &weighted_edges)
}

pub(crate) fn collect_impacts_and_warnings(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    after_vals: &[f64],
    chan_paths: &[Vec<usize>],
    params: &SimParams,
    warnings: &mut Vec<SimWarning>,
) -> Result<Vec<(usize, ImpactRow)>, AppError> {
    let mut rows = Vec::new();
    for (u, vert) in system.vertices.iter().enumerate() {
        if indexed.nodes.get(vert.node_idx).is_none() {
            continue;
        }
        let after_val = after_vals.get(u).copied().unwrap_or(0.0);
        if !after_val.is_finite() {
            return Err(AppError::bad(
                "SIM_DIVERGENT",
                "simulation produced non-finite value",
            ));
        }

        if let Some(row) = build_single_impact_row(
            indexed,
            system,
            u,
            after_val,
            chan_paths.get(u),
            params.min_effect,
            warnings,
        ) {
            rows.push((u, row));
        }
    }
    Ok(rows)
}

fn build_single_impact_row(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    chan_idx: usize,
    after_val: f64,
    chan_path: Option<&Vec<usize>>,
    min_effect: f64,
    warnings: &mut Vec<SimWarning>,
) -> Option<ImpactRow> {
    let vert = system.vertices.get(chan_idx)?;
    let node = indexed.nodes.get(vert.node_idx)?;

    let Some(before) = vert.before_val else {
        let is_absolute = vert
            .fixed_actions
            .iter()
            .any(|a| a.mode == InterventionMode::Absolute);
        if is_absolute {
            return Some(ImpactRow {
                node_id: node.id,
                type_name: node.type_name.clone(),
                title: node.title.clone(),
                metric: vert.metric.clone(),
                before_val: None,
                after_val,
                delta: None,
                delta_ratio: None,
                p10: None,
                p90: None,
                hop_distance: 0,
                top_path: vec![node.id],
                series: Vec::new(),
            });
        }
        warnings.push(SimWarning {
            node_id: node.id,
            code: String::from("METRIC_MISSING"),
            message: format!("metric '{}' missing on node {}", vert.metric, node.id),
        });
        return None;
    };

    let delta = after_val - before;
    let rel_effect = if before.abs() > 1e-12 {
        delta.abs() / before.abs()
    } else {
        delta.abs()
    };
    if rel_effect < min_effect {
        return None;
    }

    let delta_ratio = if before.abs() > 1e-12 {
        Some(delta / before)
    } else {
        None
    };
    let hop_distance = system
        .hop_distances
        .get(chan_idx)
        .copied()
        .flatten()
        .unwrap_or(0);
    let top_path = map_channel_path_to_uuids(indexed, system, chan_path, node.id);

    Some(ImpactRow {
        node_id: node.id,
        type_name: node.type_name.clone(),
        title: node.title.clone(),
        metric: vert.metric.clone(),
        before_val: Some(before),
        after_val,
        delta: Some(delta),
        delta_ratio,
        p10: None,
        p90: None,
        hop_distance,
        top_path,
        series: Vec::new(),
    })
}

fn assemble_steady_result(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    numeric: &SteadySolveOutput,
    params: &SimParams,
) -> Result<SimResult, AppError> {
    let chan_paths = compute_system_top_paths(system);
    let mut warnings = Vec::new();
    for &node_id in &numeric.max_iter_warning_nodes {
        warnings.push(SimWarning {
            node_id,
            code: String::from("MAX_ITER"),
            message: format!(
                "cyclic SCC did not converge within {} iterations",
                params.max_iterations
            ),
        });
    }

    let indexed_rows = collect_impacts_and_warnings(
        indexed,
        system,
        &numeric.after_vals,
        &chan_paths,
        params,
        &mut warnings,
    )?;
    let mut impacts: Vec<ImpactRow> = indexed_rows.into_iter().map(|(_, row)| row).collect();
    sort_impacts_desc(&mut impacts);

    let affected_count = impacts.len();
    let kpis = aggregate_kpis(&impacts);
    let checkpoints = build_steady_checkpoints(indexed, system, numeric, &impacts);

    Ok(SimResult {
        impacts,
        affected_count,
        kpis,
        warnings,
        converged: numeric.converged,
        reason: numeric.reason.clone(),
        iterations: numeric.iterations,
        checkpoints,
    })
}

pub(crate) fn map_channel_path_to_uuids(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    path: Option<&Vec<usize>>,
    fallback_node_id: Uuid,
) -> Vec<Uuid> {
    let Some(chans) = path else {
        return vec![fallback_node_id];
    };
    let mut uuids = Vec::with_capacity(chans.len());
    for &c in chans {
        if let Some(vert) = system.vertices.get(c)
            && let Some(node) = indexed.nodes.get(vert.node_idx)
            && uuids.last().copied() != Some(node.id)
        {
            uuids.push(node.id);
        }
    }
    if uuids.is_empty() {
        uuids.push(fallback_node_id);
    }
    uuids
}

pub(crate) fn sort_impacts_desc(impacts: &mut [ImpactRow]) {
    impacts.sort_by(|a, b| {
        let abs_b = b.delta.map_or(b.after_val.abs(), f64::abs);
        let abs_a = a.delta.map_or(a.after_val.abs(), f64::abs);
        abs_b
            .total_cmp(&abs_a)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.metric.cmp(&b.metric))
    });
}

fn build_steady_checkpoints(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    numeric: &SteadySolveOutput,
    impacts: &[ImpactRow],
) -> Vec<Checkpoint> {
    let mut checkpoints = Vec::new();
    let mut prev_vals: HashMap<usize, f64> = HashMap::new();

    for (seq_idx, &(iter, ref pairs)) in numeric.iter_snapshots.iter().enumerate() {
        let mut node_map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for &(u, val) in pairs {
            let changed = prev_vals
                .get(&u)
                .is_none_or(|&prev| (val - prev).abs() > 1e-12);
            if !changed {
                continue;
            }
            if let Some(vert) = system.vertices.get(u)
                && vert.before_val.is_some()
                && let Some(node) = indexed.nodes.get(vert.node_idx)
            {
                prev_vals.insert(u, val);
                insert_checkpoint_metric(&mut node_map, node.id, &vert.metric, val);
            }
        }
        let seq = i32::try_from(seq_idx).unwrap_or(i32::MAX);
        checkpoints.push(Checkpoint {
            seq,
            label: format!("iter={iter}"),
            changes: serde_json::Value::Object(node_map),
        });
    }

    let mut final_map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for row in impacts {
        let changed = row
            .before_val
            .is_none_or(|b| (row.after_val - b).abs() > 1e-12);
        if changed {
            insert_checkpoint_metric(&mut final_map, row.node_id, &row.metric, row.after_val);
        }
    }
    if !final_map.is_empty() || checkpoints.is_empty() {
        let seq = i32::try_from(checkpoints.len()).unwrap_or(i32::MAX);
        checkpoints.push(Checkpoint {
            seq,
            label: String::from("steady"),
            changes: serde_json::Value::Object(final_map),
        });
    }

    checkpoints
}

pub(crate) fn insert_checkpoint_metric(
    root: &mut serde_json::Map<String, serde_json::Value>,
    node_id: Uuid,
    metric: &str,
    val: f64,
) {
    let key = node_id.to_string();
    let entry = root
        .entry(key)
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(obj) = entry.as_object_mut()
        && let Some(num) = serde_json::Number::from_f64(val)
    {
        obj.insert(metric.to_string(), serde_json::Value::Number(num));
    }
}
