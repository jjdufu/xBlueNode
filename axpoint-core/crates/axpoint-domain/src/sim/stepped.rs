//! Stepped temporal causal propagation solver with lag and checkpoints (`09-STEP10` §10.5).

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::AppError;
use crate::sim::action::normalize_actions_indexed;
use crate::sim::graph::{IndexedSimGraph, aggregate_kpis};
use crate::sim::scc::{SccGraph, is_cyclic_component, tarjan_scc_topo};
use crate::sim::steady::{
    ChannelSystem, accumulate_single_edge, build_channel_system, collect_impacts_and_warnings,
    combine_channel_deltas, compute_system_top_paths, compute_vertex_after_val, empty_sim_result,
    evaluate_fixed_actions, insert_checkpoint_metric, set_pair, sort_impacts_desc,
    validate_common_params,
};
use crate::sim::{
    Checkpoint, ImpactRow, SimGraph, SimParams, SimResult, SimWarning, StepSeriesPoint,
};
use crate::topology::AxSimAction;

/// Maximum `horizon_steps` permitted in `STEPPED` mode (`§10.3.3`).
pub const MAX_HORIZON_STEPS: u32 = 104;

/// Numerical output from solving a [`ChannelSystem`] across `0..=horizon_steps`.
#[derive(Debug, Clone)]
pub(crate) struct SteppedSolveOutput {
    pub(crate) final_after_vals: Vec<f64>,
    pub(crate) step_after_vals: Vec<Vec<f64>>,
    pub(crate) converged: bool,
    pub(crate) reason: Option<String>,
    pub(crate) max_iter_warning_nodes: Vec<Uuid>,
}

/// Solves stepped temporal causal propagation across `t = 0..=horizon_steps` (`§10.5`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SIM_DIVERGENT`) if actions/params
/// are invalid or same-step cyclic propagation diverges.
pub fn solve(
    graph: &SimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    let indexed = IndexedSimGraph::from_sim_graph(graph)?;
    solve_indexed(&indexed, actions, params)
}

/// Solves stepped temporal causal propagation on a pre-compiled [`IndexedSimGraph`] (`§10.5`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SIM_DIVERGENT`) if actions/params
/// are invalid or same-step cyclic propagation diverges.
pub fn solve_indexed(
    indexed: &IndexedSimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    validate_stepped_params(params)?;
    let normalized = normalize_actions_indexed(indexed, actions)?;
    if normalized.is_empty() {
        return Ok(empty_sim_result());
    }

    let system = build_channel_system(indexed, &normalized, params.max_hops);
    let edge_scales = vec![1.0_f64; system.edges.len()];
    let numeric = solve_stepped_numeric(indexed, &system, params, &edge_scales, true)?;

    assemble_stepped_result(indexed, &system, &numeric, params)
}

pub(crate) fn validate_stepped_params(params: &SimParams) -> Result<(), AppError> {
    validate_common_params(params)?;
    if params.horizon_steps == 0 || params.horizon_steps > MAX_HORIZON_STEPS {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("horizon_steps must be in 1..={MAX_HORIZON_STEPS}"),
        ));
    }
    Ok(())
}

pub(crate) fn solve_stepped_numeric(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    params: &SimParams,
    edge_scales: &[f64],
    record_series: bool,
) -> Result<SteppedSolveOutput, AppError> {
    let n = system.vertices.len();
    let total_steps = usize::try_from(params.horizon_steps)
        .unwrap_or(0)
        .saturating_add(1);
    let mut rel_hist: Vec<Vec<f64>> = Vec::with_capacity(total_steps);
    let mut abs_hist: Vec<Vec<f64>> = Vec::with_capacity(total_steps);
    let mut step_after_vals: Vec<Vec<f64>> = if record_series {
        Vec::with_capacity(total_steps)
    } else {
        Vec::new()
    };

    let activation_steps: BTreeSet<u32> = system
        .vertices
        .iter()
        .flat_map(|v| v.fixed_actions.iter().map(|a| a.at_step))
        .collect();

    let mut cur_adj = build_zero_lag_adj_at(system, 0);
    let mut cur_scc = tarjan_scc_topo(n, &cur_adj);
    let mut converged = true;
    let mut reason: Option<String> = None;
    let mut warn_nodes: Vec<Uuid> = Vec::new();
    let mut final_after_vals = vec![0.0_f64; n];

    for t in 0..=params.horizon_steps {
        if t > 0 && activation_steps.contains(&t) {
            cur_adj = build_zero_lag_adj_at(system, t);
            cur_scc = tarjan_scc_topo(n, &cur_adj);
        }

        let (lag_rel, lag_abs) =
            compute_lagged_inputs(system, t, edge_scales, &rel_hist, &abs_hist);
        let mut rel_t = vec![0.0_f64; n];
        let mut abs_t = vec![0.0_f64; n];

        let step_ctx = StepSolveCtx {
            step: t,
            adj: &cur_adj,
            scc_graph: &cur_scc,
            params,
            edge_scales,
            lag_rel: &lag_rel,
            lag_abs: &lag_abs,
        };
        let outcome = solve_single_step(indexed, system, &step_ctx, &mut rel_t, &mut abs_t)?;
        if !outcome.converged {
            converged = false;
            reason = Some(String::from("MAX_ITER"));
            for nid in outcome.warn_nodes {
                if !warn_nodes.contains(&nid) {
                    warn_nodes.push(nid);
                }
            }
        }

        if record_series || t == params.horizon_steps {
            let after_t = compute_step_after_vals(system, &rel_t, &abs_t);
            if t == params.horizon_steps {
                final_after_vals.clone_from(&after_t);
            }
            if record_series {
                step_after_vals.push(after_t);
            }
        }
        rel_hist.push(rel_t);
        abs_hist.push(abs_t);
    }

    Ok(SteppedSolveOutput {
        final_after_vals,
        step_after_vals,
        converged,
        reason,
        max_iter_warning_nodes: warn_nodes,
    })
}

struct StepSolveCtx<'a> {
    step: u32,
    adj: &'a [Vec<usize>],
    scc_graph: &'a SccGraph,
    params: &'a SimParams,
    edge_scales: &'a [f64],
    lag_rel: &'a [f64],
    lag_abs: &'a [f64],
}

struct StepOutcome {
    converged: bool,
    warn_nodes: Vec<Uuid>,
}

struct StepCyclicPrep {
    ext_rel: Vec<f64>,
    ext_abs: Vec<f64>,
    int_edges: Vec<Vec<usize>>,
}

fn is_fixed_at(system: &ChannelSystem, u: usize, step: u32) -> bool {
    system
        .vertices
        .get(u)
        .is_some_and(|v| v.fixed_actions.iter().any(|a| a.at_step <= step))
}

fn build_zero_lag_adj_at(system: &ChannelSystem, step: u32) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); system.vertices.len()];
    for e in &system.edges {
        if e.delay_steps != 0 || is_fixed_at(system, e.dst_chan, step) {
            continue;
        }
        if let Some(list) = adj.get_mut(e.src_chan) {
            list.push(e.dst_chan);
        }
    }
    adj
}

fn compute_lagged_inputs(
    system: &ChannelSystem,
    step: u32,
    edge_scales: &[f64],
    rel_hist: &[Vec<f64>],
    abs_hist: &[Vec<f64>],
) -> (Vec<f64>, Vec<f64>) {
    let n = system.vertices.len();
    let mut lag_rel = vec![0.0_f64; n];
    let mut lag_abs = vec![0.0_f64; n];

    for (u, in_list) in system.in_edges.iter().enumerate() {
        if is_fixed_at(system, u, step) {
            continue;
        }
        let mut r_sum = 0.0_f64;
        let mut a_sum = 0.0_f64;
        for &ei in in_list {
            let Some(e) = system.edges.get(ei) else {
                continue;
            };
            if e.delay_steps == 0 || step < e.delay_steps {
                continue;
            }
            let src_step = usize::try_from(step - e.delay_steps).unwrap_or(usize::MAX);
            let Some(prev_rel) = rel_hist.get(src_step) else {
                continue;
            };
            let Some(prev_abs) = abs_hist.get(src_step) else {
                continue;
            };
            let scale = edge_scales.get(ei).copied().unwrap_or(1.0);
            accumulate_single_edge(
                e,
                scale,
                prev_rel,
                prev_abs,
                Some(&mut r_sum),
                Some(&mut a_sum),
            );
        }
        set_pair(&mut lag_rel, &mut lag_abs, u, r_sum, a_sum);
    }

    (lag_rel, lag_abs)
}

fn solve_single_step(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    ctx: &StepSolveCtx<'_>,
    rel_t: &mut [f64],
    abs_t: &mut [f64],
) -> Result<StepOutcome, AppError> {
    let n = system.vertices.len();
    let comp_of = ctx.scc_graph.component_ids(n);
    let mut converged = true;
    let mut warn_nodes = Vec::new();

    for (cid, comp) in ctx.scc_graph.components.iter().enumerate() {
        if is_cyclic_component(comp, ctx.adj) {
            let ok = solve_step_cyclic_scc(system, ctx, cid, comp, &comp_of, rel_t, abs_t)?;
            if !ok {
                converged = false;
                if let Some(nid) = comp
                    .first()
                    .and_then(|&u| system.vertices.get(u))
                    .and_then(|v| indexed.nodes.get(v.node_idx))
                    .map(|node| node.id)
                {
                    warn_nodes.push(nid);
                }
            }
        } else if let Some(&u) = comp.first() {
            solve_step_acyclic_vertex(system, ctx, u, rel_t, abs_t);
        }
    }

    Ok(StepOutcome {
        converged,
        warn_nodes,
    })
}

fn solve_step_acyclic_vertex(
    system: &ChannelSystem,
    ctx: &StepSolveCtx<'_>,
    u: usize,
    rel_t: &mut [f64],
    abs_t: &mut [f64],
) {
    let Some(vert) = system.vertices.get(u) else {
        return;
    };
    if is_fixed_at(system, u, ctx.step) {
        let (rx, ax) = evaluate_fixed_actions(vert, Some(ctx.step));
        set_pair(rel_t, abs_t, u, rx, ax);
        return;
    }

    let mut r_sum = ctx.lag_rel.get(u).copied().unwrap_or(0.0);
    let mut a_sum = ctx.lag_abs.get(u).copied().unwrap_or(0.0);
    if let Some(in_list) = system.in_edges.get(u) {
        for &ei in in_list {
            if let Some(e) = system.edges.get(ei)
                && e.delay_steps == 0
            {
                let scale = ctx.edge_scales.get(ei).copied().unwrap_or(1.0);
                accumulate_single_edge(e, scale, rel_t, abs_t, Some(&mut r_sum), Some(&mut a_sum));
            }
        }
    }

    let (rx, ax) = combine_channel_deltas(vert.before_val, r_sum, a_sum);
    set_pair(rel_t, abs_t, u, rx, ax);
}

fn solve_step_cyclic_scc(
    system: &ChannelSystem,
    ctx: &StepSolveCtx<'_>,
    cid: usize,
    comp: &[usize],
    comp_of: &[usize],
    rel_t: &mut [f64],
    abs_t: &mut [f64],
) -> Result<bool, AppError> {
    let prep = prepare_step_cyclic(system, ctx, cid, comp, comp_of, rel_t, abs_t);
    let m = comp.len();
    let mut cur_rel = vec![0.0_f64; m];
    let mut cur_abs = vec![0.0_f64; m];
    let mut prev_diff = f64::INFINITY;
    let mut inc_streak = 0_u32;

    for iter in 1..=ctx.params.max_iterations {
        let mut state = StepJacobiState {
            cur_rel: &mut cur_rel,
            cur_abs: &mut cur_abs,
            rel_t,
            abs_t,
        };
        let max_diff = jacobi_step_for_t(system, ctx, comp, &prep, &mut state)?;
        if iter > 1 && max_diff > prev_diff {
            inc_streak = inc_streak.saturating_add(1);
        } else {
            inc_streak = 0;
        }
        prev_diff = max_diff;
        if max_diff < ctx.params.tolerance {
            return Ok(true);
        }
    }

    if ctx.params.max_iterations > 50 && inc_streak >= 50 {
        return Err(AppError::bad(
            "SIM_DIVERGENT",
            "stepped same-step cyclic propagation diverged monotonically (cycle gain product >= 1)",
        ));
    }
    Ok(false)
}

fn prepare_step_cyclic(
    system: &ChannelSystem,
    ctx: &StepSolveCtx<'_>,
    cid: usize,
    comp: &[usize],
    comp_of: &[usize],
    rel_t: &[f64],
    abs_t: &[f64],
) -> StepCyclicPrep {
    let m = comp.len();
    let mut ext_rel = vec![0.0_f64; m];
    let mut ext_abs = vec![0.0_f64; m];
    let mut int_edges: Vec<Vec<usize>> = vec![Vec::new(); m];

    for (local_i, &u) in comp.iter().enumerate() {
        let lr = ctx.lag_rel.get(u).copied().unwrap_or(0.0);
        let la = ctx.lag_abs.get(u).copied().unwrap_or(0.0);
        set_pair(&mut ext_rel, &mut ext_abs, local_i, lr, la);
        if let Some(in_list) = system.in_edges.get(u) {
            for &ei in in_list {
                let Some(e) = system.edges.get(ei) else {
                    continue;
                };
                if e.delay_steps != 0 {
                    continue;
                }
                if comp_of.get(e.src_chan).copied() == Some(cid) {
                    if let Some(slot) = int_edges.get_mut(local_i) {
                        slot.push(ei);
                    }
                } else {
                    let scale = ctx.edge_scales.get(ei).copied().unwrap_or(1.0);
                    accumulate_single_edge(
                        e,
                        scale,
                        rel_t,
                        abs_t,
                        ext_rel.get_mut(local_i),
                        ext_abs.get_mut(local_i),
                    );
                }
            }
        }
    }

    StepCyclicPrep {
        ext_rel,
        ext_abs,
        int_edges,
    }
}

struct StepJacobiState<'a> {
    cur_rel: &'a mut [f64],
    cur_abs: &'a mut [f64],
    rel_t: &'a mut [f64],
    abs_t: &'a mut [f64],
}

fn jacobi_step_for_t(
    system: &ChannelSystem,
    ctx: &StepSolveCtx<'_>,
    comp: &[usize],
    prep: &StepCyclicPrep,
    state: &mut StepJacobiState<'_>,
) -> Result<f64, AppError> {
    let m = comp.len();
    let mut next_rel = vec![0.0_f64; m];
    let mut next_abs = vec![0.0_f64; m];
    let alpha = ctx.params.damping;
    let mut max_diff = 0.0_f64;

    for (local_i, &u) in comp.iter().enumerate() {
        let mut r_sum = prep.ext_rel.get(local_i).copied().unwrap_or(0.0);
        let mut a_sum = prep.ext_abs.get(local_i).copied().unwrap_or(0.0);
        if let Some(e_list) = prep.int_edges.get(local_i) {
            for &ei in e_list {
                if let Some(e) = system.edges.get(ei) {
                    let scale = ctx.edge_scales.get(ei).copied().unwrap_or(1.0);
                    accumulate_single_edge(
                        e,
                        scale,
                        state.rel_t,
                        state.abs_t,
                        Some(&mut r_sum),
                        Some(&mut a_sum),
                    );
                }
            }
        }

        let before = system.vertices.get(u).and_then(|v| v.before_val);
        let (target_r, target_a) = combine_channel_deltas(before, r_sum, a_sum);
        let old_r = state.cur_rel.get(local_i).copied().unwrap_or(0.0);
        let old_a = state.cur_abs.get(local_i).copied().unwrap_or(0.0);
        let nr = (1.0 - alpha) * old_r + alpha * target_r;
        let na = (1.0 - alpha) * old_a + alpha * target_a;

        if !nr.is_finite() || !na.is_finite() {
            return Err(AppError::bad(
                "SIM_DIVERGENT",
                "stepped same-step cycle diverged to non-finite value (cycle gain product >= 1)",
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

    for (local_i, &u) in comp.iter().enumerate() {
        let nr = next_rel.get(local_i).copied().unwrap_or(0.0);
        let na = next_abs.get(local_i).copied().unwrap_or(0.0);
        set_pair(state.cur_rel, state.cur_abs, local_i, nr, na);
        set_pair(state.rel_t, state.abs_t, u, nr, na);
    }

    Ok(max_diff)
}

fn compute_step_after_vals(system: &ChannelSystem, rel_t: &[f64], abs_t: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0_f64; system.vertices.len()];
    for (u, vert) in system.vertices.iter().enumerate() {
        let rx = rel_t.get(u).copied().unwrap_or(0.0);
        let ax = abs_t.get(u).copied().unwrap_or(0.0);
        if let Some(slot) = out.get_mut(u) {
            *slot = compute_vertex_after_val(vert, rx, ax, false);
        }
    }
    out
}

fn assemble_stepped_result(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    numeric: &SteppedSolveOutput,
    params: &SimParams,
) -> Result<SimResult, AppError> {
    let chan_paths = compute_system_top_paths(system);
    let mut warnings = Vec::new();
    for &node_id in &numeric.max_iter_warning_nodes {
        warnings.push(SimWarning {
            node_id,
            code: String::from("MAX_ITER"),
            message: format!(
                "stepped same-step cyclic SCC did not converge within {} iterations",
                params.max_iterations
            ),
        });
    }

    let mut indexed_rows = collect_impacts_and_warnings(
        indexed,
        system,
        &numeric.final_after_vals,
        &chan_paths,
        params,
        &mut warnings,
    )?;

    indexed_rows.sort_by(|(_, a), (_, b)| {
        let abs_b = b.delta.map_or(b.after_val.abs(), f64::abs);
        let abs_a = a.delta.map_or(a.after_val.abs(), f64::abs);
        abs_b
            .total_cmp(&abs_a)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.metric.cmp(&b.metric))
    });

    for (u, row) in indexed_rows.iter_mut().take(50) {
        let mut series = Vec::with_capacity(numeric.step_after_vals.len());
        for (step_idx, step_vals) in numeric.step_after_vals.iter().enumerate() {
            if let Some(&val) = step_vals.get(*u) {
                let step = u32::try_from(step_idx).unwrap_or(u32::MAX);
                series.push(StepSeriesPoint { step, value: val });
            }
        }
        row.series = series;
    }

    let mut impacts: Vec<ImpactRow> = indexed_rows.into_iter().map(|(_, row)| row).collect();
    sort_impacts_desc(&mut impacts);

    let affected_count = impacts.len();
    let kpis = aggregate_kpis(&impacts);
    let checkpoints = build_stepped_checkpoints(indexed, system, &numeric.step_after_vals, params);

    Ok(SimResult {
        impacts,
        affected_count,
        kpis,
        warnings,
        converged: numeric.converged,
        reason: numeric.reason.clone(),
        iterations: params.horizon_steps,
        checkpoints,
        fired_events: Vec::new(),
        deadline_report: None,
    })
}

fn build_stepped_checkpoints(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    step_after_vals: &[Vec<f64>],
    params: &SimParams,
) -> Vec<Checkpoint> {
    let stride = params.checkpoint_every.max(1);
    let mut prev_vals: Vec<Option<f64>> = system.vertices.iter().map(|v| v.before_val).collect();
    let mut checkpoints = Vec::new();

    for (step_idx, vals) in step_after_vals.iter().enumerate() {
        let t = u32::try_from(step_idx).unwrap_or(u32::MAX);
        if t % stride != 0 && t != params.horizon_steps {
            continue;
        }
        let mut node_map = serde_json::Map::new();
        for (u, &val) in vals.iter().enumerate() {
            let Some(vert) = system.vertices.get(u) else {
                continue;
            };
            let is_active_missing = vert.before_val.is_none()
                && !vert
                    .fixed_actions
                    .iter()
                    .any(|a| a.mode == crate::enums::InterventionMode::Absolute && a.at_step <= t);
            if is_active_missing {
                continue;
            }
            let prev = prev_vals.get(u).copied().flatten();
            let changed = prev.is_none_or(|p| (val - p).abs() > 1e-12);
            if changed {
                if let Some(slot) = prev_vals.get_mut(u) {
                    *slot = Some(val);
                }
                if let Some(node) = indexed.nodes.get(vert.node_idx) {
                    insert_checkpoint_metric(&mut node_map, node.id, &vert.metric, val);
                }
            }
        }
        checkpoints.push(Checkpoint {
            seq: i32::try_from(t).unwrap_or(i32::MAX),
            label: format!("t={t}"),
            changes: serde_json::Value::Object(node_map),
        });
    }

    checkpoints
}
