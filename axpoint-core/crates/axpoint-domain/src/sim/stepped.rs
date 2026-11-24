//! Stepped temporal causal propagation solver with lag, scheduled/conditional events,
//! checkpoints, and `deadline_step` goal evaluation (`09-STEP10` §10.5 & `06-STEP28` §28.2).

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;

use crate::AppError;
use crate::enums::{InterventionMode, SimEventTriggerKind, SimThresholdOp};
use crate::sim::action::normalize_actions_indexed;
use crate::sim::deadline::{
    eval_threshold_op, evaluate_goal_constraints, validate_goal_constraints,
};
use crate::sim::graph::{IndexedSimGraph, aggregate_kpis, apply_clamp};
use crate::sim::scc::{SccGraph, is_cyclic_component, tarjan_scc_topo};
use crate::sim::steady::{
    ChannelFixedAction, ChannelSystem, ChannelVertex, accumulate_single_edge,
    build_channel_system_with_extra_seeds, collect_impacts_and_warnings, combine_channel_deltas,
    compute_system_top_paths, compute_vertex_after_val, empty_sim_result, evaluate_fixed_actions,
    insert_checkpoint_metric, set_pair, sort_impacts_desc, validate_common_params,
};
use crate::sim::{
    Checkpoint, DeadlineReport, FiredEventRecord, ImpactRow, NormalizedAction, SimEventSpec,
    SimGoalConstraint, SimGraph, SimParams, SimResult, SimWarning, StepSeriesPoint,
};
use crate::topology::AxSimAction;

/// Maximum `horizon_steps` permitted in `STEPPED` mode (`§10.3.3`).
pub const MAX_HORIZON_STEPS: u32 = 104;

/// Validated event ready for discrete-step evaluation.
#[derive(Debug, Clone)]
pub(crate) struct CompiledSimEvent {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) trigger_kind: SimEventTriggerKind,
    pub(crate) at_step: Option<u32>,
    pub(crate) watch_node_idx: Option<usize>,
    pub(crate) watch_metric: Option<String>,
    pub(crate) op: Option<SimThresholdOp>,
    pub(crate) threshold: Option<f64>,
    pub(crate) cooldown_steps: u32,
    pub(crate) max_fires: u32,
    pub(crate) actions: Vec<NormalizedAction>,
}

/// Numerical output from solving a [`ChannelSystem`] across `0..=horizon_steps`.
#[derive(Debug, Clone)]
pub(crate) struct SteppedSolveOutput {
    pub(crate) final_after_vals: Vec<f64>,
    pub(crate) step_after_vals: Vec<Vec<f64>>,
    pub(crate) fired_events: Vec<FiredEventRecord>,
    pub(crate) event_abs_channels: BTreeSet<(usize, u32)>,
    pub(crate) converged: bool,
    pub(crate) reason: Option<String>,
    pub(crate) max_iter_warning_nodes: Vec<Uuid>,
}

/// Bundle of numerical configuration passed into [`solve_stepped_numeric_with_events`].
pub(crate) struct SteppedNumericInput<'a> {
    pub(crate) indexed: &'a IndexedSimGraph,
    pub(crate) system: &'a ChannelSystem,
    pub(crate) chan_lookup: &'a HashMap<(usize, String), usize>,
    pub(crate) params: &'a SimParams,
    pub(crate) edge_scales: &'a [f64],
    pub(crate) events: &'a [CompiledSimEvent],
    pub(crate) record_series: bool,
}

/// Validates scheduled/conditional events and redline goal constraints against `graph` (`06-STEP28` §28.2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `INVALID_GEOMETRY`) if any event or
/// constraint is malformed.
pub fn validate_events_and_goals(
    graph: &SimGraph,
    scheduled_events: &[SimEventSpec],
    goal_constraints: &[SimGoalConstraint],
) -> Result<(), AppError> {
    let indexed = IndexedSimGraph::from_sim_graph(graph)?;
    let _compiled = compile_scheduled_events(&indexed, scheduled_events)?;
    validate_goal_constraints(&indexed, goal_constraints)
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
    solve_with_events(graph, actions, params, &[], &[])
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
    solve_indexed_with_events(indexed, actions, params, &[], &[])
}

/// Solves stepped causal propagation with scheduled/conditional events and `deadline_step` goal constraints (`06-STEP28` §28.2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`, `INVALID_GEOMETRY`, or `SIM_DIVERGENT`)
/// if validation fails or same-step cyclic propagation diverges.
pub fn solve_with_events(
    graph: &SimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
    scheduled_events: &[SimEventSpec],
    goal_constraints: &[SimGoalConstraint],
) -> Result<SimResult, AppError> {
    let indexed = IndexedSimGraph::from_sim_graph(graph)?;
    solve_indexed_with_events(
        &indexed,
        actions,
        params,
        scheduled_events,
        goal_constraints,
    )
}

/// Solves stepped causal propagation with events and goal constraints on a pre-compiled [`IndexedSimGraph`] (`06-STEP28` §28.2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`, `INVALID_GEOMETRY`, or `SIM_DIVERGENT`)
/// if validation fails or same-step cyclic propagation diverges.
pub fn solve_indexed_with_events(
    indexed: &IndexedSimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
    scheduled_events: &[SimEventSpec],
    goal_constraints: &[SimGoalConstraint],
) -> Result<SimResult, AppError> {
    validate_stepped_params(params)?;
    let normalized = normalize_actions_indexed(indexed, actions)?;
    let compiled_events = compile_scheduled_events(indexed, scheduled_events)?;
    validate_goal_constraints(indexed, goal_constraints)?;

    if normalized.is_empty() && compiled_events.is_empty() && goal_constraints.is_empty() {
        return Ok(empty_sim_result());
    }

    let extra_seeds = collect_event_seed_channels(indexed, &compiled_events);
    let mut system =
        build_channel_system_with_extra_seeds(indexed, &normalized, &extra_seeds, params.max_hops);
    let chan_lookup = build_chan_lookup(&system);
    let edge_scales = vec![1.0_f64; system.edges.len()];
    let numeric = solve_stepped_numeric_with_events(&SteppedNumericInput {
        indexed,
        system: &system,
        chan_lookup: &chan_lookup,
        params,
        edge_scales: &edge_scales,
        events: &compiled_events,
        record_series: true,
    })?;

    for &(chan_idx, at_step) in &numeric.event_abs_channels {
        if let Some(vert) = system.vertices.get_mut(chan_idx) {
            vert.fixed_actions.push(ChannelFixedAction {
                mode: InterventionMode::Absolute,
                value: 0.0,
                at_step,
            });
        }
    }

    let deadline_report = evaluate_goal_constraints(
        indexed,
        &system,
        &numeric.step_after_vals,
        params.horizon_steps,
        goal_constraints,
    )?;

    assemble_stepped_result(indexed, &system, &numeric, params, deadline_report)
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

pub(crate) fn compile_scheduled_events(
    indexed: &IndexedSimGraph,
    events: &[SimEventSpec],
) -> Result<Vec<CompiledSimEvent>, AppError> {
    let mut out = Vec::with_capacity(events.len());
    for ev in events {
        if ev.id.trim().is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "scheduled_event.id must not be empty",
            ));
        }
        let actions = normalize_actions_indexed(indexed, &ev.actions)?;
        let (watch_node_idx, watch_metric, op, threshold) = match ev.trigger_kind {
            SimEventTriggerKind::AtStep => {
                if ev.at_step.is_none() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!(
                            "event '{}' requires at_step when trigger_kind=AT_STEP",
                            ev.id
                        ),
                    ));
                }
                (None, None, None, None)
            }
            SimEventTriggerKind::MetricThreshold => {
                let wnid = ev.watch_node_id.ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("event '{}' requires watch_node_id", ev.id),
                    )
                })?;
                let Some(&w_idx) = indexed.node_index.get(&wnid) else {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!(
                            "event '{}' watch_node_id {wnid} not found in sandbox",
                            ev.id
                        ),
                    ));
                };
                let wm = ev
                    .watch_metric
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        AppError::bad(
                            "INVALID_ARGUMENT",
                            format!("event '{}' requires non-empty watch_metric", ev.id),
                        )
                    })?
                    .to_string();
                let op_val = ev.op.ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", format!("event '{}' requires op", ev.id))
                })?;
                let thresh = ev.threshold.ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("event '{}' requires threshold", ev.id),
                    )
                })?;
                if !thresh.is_finite() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("event '{}' threshold must be finite", ev.id),
                    ));
                }
                (Some(w_idx), Some(wm), Some(op_val), Some(thresh))
            }
        };

        out.push(CompiledSimEvent {
            id: ev.id.trim().to_string(),
            name: ev.name.trim().to_string(),
            trigger_kind: ev.trigger_kind,
            at_step: ev.at_step,
            watch_node_idx,
            watch_metric,
            op,
            threshold,
            cooldown_steps: ev.cooldown_steps,
            max_fires: if ev.max_fires == 0 { 1 } else { ev.max_fires },
            actions,
        });
    }
    Ok(out)
}

pub(crate) fn collect_event_seed_channels(
    indexed: &IndexedSimGraph,
    events: &[CompiledSimEvent],
) -> Vec<(usize, String)> {
    let mut seeds = Vec::new();
    for ev in events {
        for act in &ev.actions {
            if let Some(&n_idx) = indexed.node_index.get(&act.target_node_id) {
                let key = (n_idx, act.metric_key.clone());
                if !seeds.contains(&key) {
                    seeds.push(key);
                }
            }
        }
    }
    seeds
}

pub(crate) fn build_chan_lookup(system: &ChannelSystem) -> HashMap<(usize, String), usize> {
    system
        .vertices
        .iter()
        .enumerate()
        .map(|(u, v)| ((v.node_idx, v.metric.clone()), u))
        .collect()
}

#[derive(Default)]
struct EventRuntimeState {
    fire_count: u32,
    last_fired_step: Option<u32>,
}

pub(crate) fn solve_stepped_numeric_with_events(
    inp: &SteppedNumericInput<'_>,
) -> Result<SteppedSolveOutput, AppError> {
    let n = inp.system.vertices.len();
    let total_steps = usize::try_from(inp.params.horizon_steps)
        .unwrap_or(0)
        .saturating_add(1);
    let mut rel_hist: Vec<Vec<f64>> = Vec::with_capacity(total_steps);
    let mut abs_hist: Vec<Vec<f64>> = Vec::with_capacity(total_steps);
    let mut step_after_vals: Vec<Vec<f64>> = if inp.record_series {
        Vec::with_capacity(total_steps)
    } else {
        Vec::new()
    };
    let mut prev_step_vals = vec![0.0_f64; n];
    let mut event_states: Vec<EventRuntimeState> = (0..inp.events.len())
        .map(|_| EventRuntimeState::default())
        .collect();
    let mut active_event_actions: Vec<Vec<NormalizedAction>> = vec![Vec::new(); n];
    let mut fired_events: Vec<FiredEventRecord> = Vec::new();
    let mut event_abs_channels: BTreeSet<(usize, u32)> = BTreeSet::new();

    let activation_steps: BTreeSet<u32> = inp
        .system
        .vertices
        .iter()
        .flat_map(|v| v.fixed_actions.iter().map(|a| a.at_step))
        .collect();

    let mut cur_adj = build_zero_lag_adj_at(inp.system, &active_event_actions, 0);
    let mut cur_scc = tarjan_scc_topo(n, &cur_adj);
    let mut converged = true;
    let mut reason: Option<String> = None;
    let mut warn_nodes: Vec<Uuid> = Vec::new();
    let mut final_after_vals = vec![0.0_f64; n];

    for t in 0..=inp.params.horizon_steps {
        let abs_dirty = activate_due_events(
            inp,
            t,
            &prev_step_vals,
            &mut event_states,
            &mut active_event_actions,
            &mut fired_events,
            &mut event_abs_channels,
        );
        if (t > 0 && activation_steps.contains(&t)) || abs_dirty {
            cur_adj = build_zero_lag_adj_at(inp.system, &active_event_actions, t);
            cur_scc = tarjan_scc_topo(n, &cur_adj);
        }

        let (lag_rel, lag_abs) = compute_lagged_inputs(
            inp.system,
            &active_event_actions,
            t,
            inp.edge_scales,
            &rel_hist,
            &abs_hist,
        );
        let mut rel_t = vec![0.0_f64; n];
        let mut abs_t = vec![0.0_f64; n];

        let step_ctx = StepSolveCtx {
            step: t,
            adj: &cur_adj,
            scc_graph: &cur_scc,
            params: inp.params,
            edge_scales: inp.edge_scales,
            lag_rel: &lag_rel,
            lag_abs: &lag_abs,
            event_actions: &active_event_actions,
        };
        let outcome =
            solve_single_step(inp.indexed, inp.system, &step_ctx, &mut rel_t, &mut abs_t)?;
        if !outcome.converged {
            converged = false;
            reason = Some(String::from("MAX_ITER"));
            for nid in outcome.warn_nodes {
                if !warn_nodes.contains(&nid) {
                    warn_nodes.push(nid);
                }
            }
        }

        let need_vals =
            inp.record_series || !inp.events.is_empty() || t == inp.params.horizon_steps;
        if need_vals {
            let after_t = compute_step_after_vals(inp.system, &rel_t, &abs_t);
            if t == inp.params.horizon_steps {
                final_after_vals.clone_from(&after_t);
            }
            prev_step_vals.clone_from(&after_t);
            if inp.record_series {
                step_after_vals.push(after_t);
            }
        }
        rel_hist.push(rel_t);
        abs_hist.push(abs_t);
    }

    Ok(SteppedSolveOutput {
        final_after_vals,
        step_after_vals,
        fired_events,
        event_abs_channels,
        converged,
        reason,
        max_iter_warning_nodes: warn_nodes,
    })
}

fn activate_due_events(
    inp: &SteppedNumericInput<'_>,
    step: u32,
    prev_step_vals: &[f64],
    states: &mut [EventRuntimeState],
    active_event_actions: &mut [Vec<NormalizedAction>],
    fired_events: &mut Vec<FiredEventRecord>,
    event_abs_channels: &mut BTreeSet<(usize, u32)>,
) -> bool {
    let mut abs_dirty = false;
    for (idx, ev) in inp.events.iter().enumerate() {
        let (fired, trigger_value) = match ev.trigger_kind {
            SimEventTriggerKind::AtStep => (ev.at_step == Some(step), None),
            SimEventTriggerKind::MetricThreshold => {
                let tv = eval_conditional_trigger(inp, ev, step, prev_step_vals, states.get(idx));
                (tv.is_some(), tv)
            }
        };

        if !fired {
            continue;
        }
        if let Some(st) = states.get_mut(idx) {
            st.fire_count = st.fire_count.saturating_add(1);
            st.last_fired_step = Some(step);
        }
        for act in &ev.actions {
            if let Some(&n_idx) = inp.indexed.node_index.get(&act.target_node_id)
                && let Some(&chan_idx) = inp.chan_lookup.get(&(n_idx, act.metric_key.clone()))
                && let Some(slot) = active_event_actions.get_mut(chan_idx)
            {
                if act.mode == InterventionMode::Absolute {
                    abs_dirty = true;
                    event_abs_channels.insert((chan_idx, step));
                }
                slot.push(NormalizedAction {
                    at_step: step,
                    ..act.clone()
                });
            }
        }
        fired_events.push(FiredEventRecord {
            event_id: ev.id.clone(),
            event_name: ev.name.clone(),
            trigger_kind: ev.trigger_kind,
            step,
            trigger_value,
            applied_actions: ev.actions.len(),
        });
    }
    abs_dirty
}

fn eval_conditional_trigger(
    inp: &SteppedNumericInput<'_>,
    ev: &CompiledSimEvent,
    step: u32,
    prev_step_vals: &[f64],
    state: Option<&EventRuntimeState>,
) -> Option<f64> {
    if step == 0 {
        return None;
    }
    let st = state?;
    if st.fire_count >= ev.max_fires {
        return None;
    }
    if st
        .last_fired_step
        .is_some_and(|prev| step.saturating_sub(prev) < ev.cooldown_steps.max(1))
    {
        return None;
    }
    let w_node_idx = ev.watch_node_idx?;
    let w_metric = ev.watch_metric.as_ref()?;
    let op = ev.op?;
    let threshold = ev.threshold?;

    let prev_val = match inp.chan_lookup.get(&(w_node_idx, w_metric.clone())) {
        Some(&chan_idx) => prev_step_vals.get(chan_idx).copied(),
        None => inp
            .indexed
            .nodes
            .get(w_node_idx)
            .and_then(|n| n.metrics.get(w_metric))
            .copied(),
    }?;

    eval_threshold_op(prev_val, op, threshold).then_some(prev_val)
}

struct StepSolveCtx<'a> {
    step: u32,
    adj: &'a [Vec<usize>],
    scc_graph: &'a SccGraph,
    params: &'a SimParams,
    edge_scales: &'a [f64],
    lag_rel: &'a [f64],
    lag_abs: &'a [f64],
    event_actions: &'a [Vec<NormalizedAction>],
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

fn has_event_absolute(event_actions: &[Vec<NormalizedAction>], u: usize) -> bool {
    event_actions
        .get(u)
        .is_some_and(|acts| acts.iter().any(|a| a.mode == InterventionMode::Absolute))
}

fn is_masked_at(
    system: &ChannelSystem,
    event_actions: &[Vec<NormalizedAction>],
    u: usize,
    step: u32,
) -> bool {
    is_fixed_at(system, u, step) || has_event_absolute(event_actions, u)
}

fn build_zero_lag_adj_at(
    system: &ChannelSystem,
    event_actions: &[Vec<NormalizedAction>],
    step: u32,
) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); system.vertices.len()];
    for e in &system.edges {
        if e.delay_steps != 0 || is_masked_at(system, event_actions, e.dst_chan, step) {
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
    event_actions: &[Vec<NormalizedAction>],
    step: u32,
    edge_scales: &[f64],
    rel_hist: &[Vec<f64>],
    abs_hist: &[Vec<f64>],
) -> (Vec<f64>, Vec<f64>) {
    let n = system.vertices.len();
    let mut lag_rel = vec![0.0_f64; n];
    let mut lag_abs = vec![0.0_f64; n];

    for (u, in_list) in system.in_edges.iter().enumerate() {
        if is_masked_at(system, event_actions, u, step) {
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

fn eval_event_exogenous_delta(acts: &[NormalizedAction]) -> (f64, f64) {
    let mut rel_mult = 1.0_f64;
    let mut abs_sum = 0.0_f64;
    for act in acts {
        match act.mode {
            InterventionMode::Ratio => {
                rel_mult *= 1.0 + act.value;
            }
            InterventionMode::Add => {
                abs_sum += act.value;
            }
            InterventionMode::Absolute => {}
        }
    }
    (rel_mult - 1.0, abs_sum)
}

fn eval_event_absolute_override(vert: &ChannelVertex, acts: &[NormalizedAction]) -> (f64, f64) {
    let mut after_val = vert.before_val.unwrap_or(0.0);
    for act in acts {
        match act.mode {
            InterventionMode::Ratio => after_val *= 1.0 + act.value,
            InterventionMode::Add => after_val += act.value,
            InterventionMode::Absolute => after_val = act.value,
        }
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

fn combine_with_vertex_clamp(vert: &ChannelVertex, r_sum: f64, a_sum: f64) -> (f64, f64) {
    let (rx, ax) = combine_channel_deltas(vert.before_val, r_sum, a_sum);
    let Some(before) = vert.before_val else {
        return (rx, apply_clamp(ax, &vert.clamp));
    };
    let clamped = apply_clamp(before + ax, &vert.clamp);
    let clamped_abs = clamped - before;
    let clamped_rel = if before.abs() > 1e-12 {
        clamped_abs / before
    } else {
        rx
    };
    (clamped_rel, clamped_abs)
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
    if has_event_absolute(ctx.event_actions, u)
        && let Some(ev_acts) = ctx.event_actions.get(u)
    {
        let (rx, ax) = eval_event_absolute_override(vert, ev_acts);
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

    let (rx, ax) = if let Some(ev_acts) = ctx.event_actions.get(u)
        && !ev_acts.is_empty()
    {
        let (ev_r, ev_a) = eval_event_exogenous_delta(ev_acts);
        combine_with_vertex_clamp(vert, r_sum + ev_r, a_sum + ev_a)
    } else {
        combine_channel_deltas(vert.before_val, r_sum, a_sum)
    };
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
        let mut lr = ctx.lag_rel.get(u).copied().unwrap_or(0.0);
        let mut la = ctx.lag_abs.get(u).copied().unwrap_or(0.0);
        if let Some(ev_acts) = ctx.event_actions.get(u)
            && !ev_acts.is_empty()
        {
            let (ev_r, ev_a) = eval_event_exogenous_delta(ev_acts);
            lr += ev_r;
            la += ev_a;
        }
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
    deadline_report: Option<DeadlineReport>,
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
        fired_events: numeric.fired_events.clone(),
        deadline_report,
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
                    .any(|a| a.mode == InterventionMode::Absolute && a.at_step <= t);
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
