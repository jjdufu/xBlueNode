//! Monte-Carlo edge-weight uncertainty sampling with reproducible `ChaCha8Rng` and `rayon` (`09-STEP10` §10.6 & `06-STEP28` §28.2).

use std::collections::HashMap;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use uuid::Uuid;

use crate::AppError;
use crate::sim::action::normalize_actions_indexed;
use crate::sim::deadline::evaluate_goal_constraints;
use crate::sim::graph::IndexedSimGraph;
use crate::sim::scc::tarjan_scc_topo;
use crate::sim::steady::{
    build_channel_adj, build_channel_system_with_extra_seeds, solve_steady_numeric,
};
use crate::sim::stepped::{
    SteppedNumericInput, build_chan_lookup, collect_event_seed_channels, compile_scheduled_events,
    solve_stepped_numeric_with_events,
};
use crate::sim::{
    SimEventSpec, SimGoalConstraint, SimGraph, SimMode, SimParams, SimResult, UncertaintyConfig,
};
use crate::topology::AxSimAction;

/// Maximum Monte-Carlo sample count permitted per simulation (`§10.3.3`).
pub const MAX_UNCERTAINTY_SAMPLES: u32 = 500;

/// Runs deterministic solve plus optional Monte-Carlo `P10` / `P90` uncertainty quantification (`§10.6`).
///
/// # Errors
/// Propagates any validation or divergence error from [`crate::sim::steady::solve`] or
/// [`crate::sim::stepped::solve`], or returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if
/// `uncertainty` parameters are invalid.
pub fn solve(
    graph: &SimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    solve_with_events(graph, actions, params, &[], &[])
}

/// Runs deterministic solve plus optional Monte-Carlo `P10` / `P90` on a pre-compiled [`IndexedSimGraph`].
///
/// # Errors
/// Propagates any validation or divergence error from [`crate::sim::steady::solve_indexed`] or
/// [`crate::sim::stepped::solve_indexed`].
pub fn solve_indexed(
    indexed: &IndexedSimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    solve_indexed_with_events(indexed, actions, params, &[], &[])
}

/// Runs deterministic solve with scheduled/conditional events and `deadline_step` goal constraints,
/// plus optional Monte-Carlo `P10` / `P90` uncertainty quantification (`06-STEP28` §28.2).
///
/// # Errors
/// Propagates any validation, geometry, or divergence error from the underlying solver.
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

/// Runs deterministic solve with events and goal constraints plus optional Monte-Carlo sampling on [`IndexedSimGraph`].
///
/// # Errors
/// Propagates any validation, geometry, or divergence error from the underlying solver.
pub fn solve_indexed_with_events(
    indexed: &IndexedSimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
    scheduled_events: &[SimEventSpec],
    goal_constraints: &[SimGoalConstraint],
) -> Result<SimResult, AppError> {
    let use_stepped = params.mode == SimMode::Stepped || !scheduled_events.is_empty();
    let mut det_result = if use_stepped {
        crate::sim::stepped::solve_indexed_with_events(
            indexed,
            actions,
            params,
            scheduled_events,
            goal_constraints,
        )?
    } else {
        let mut res = crate::sim::steady::solve_indexed(indexed, actions, params)?;
        if !goal_constraints.is_empty() {
            res.deadline_report =
                evaluate_steady_goals(indexed, actions, params, goal_constraints)?;
        }
        res
    };

    let Some(uc) = &params.uncertainty else {
        return Ok(det_result);
    };
    validate_uncertainty_config(uc)?;

    if det_result.impacts.is_empty() {
        return Ok(det_result);
    }

    run_monte_carlo_band(
        indexed,
        actions,
        params,
        scheduled_events,
        use_stepped,
        uc,
        &mut det_result,
    )?;
    Ok(det_result)
}

fn evaluate_steady_goals(
    indexed: &IndexedSimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
    goal_constraints: &[SimGoalConstraint],
) -> Result<Option<crate::sim::DeadlineReport>, AppError> {
    let normalized = normalize_actions_indexed(indexed, actions)?;
    let system = build_channel_system_with_extra_seeds(indexed, &normalized, &[], params.max_hops);
    if system.vertices.is_empty() {
        return evaluate_goal_constraints(indexed, &system, &[Vec::new()], 0, goal_constraints);
    }
    let adj = build_channel_adj(&system, false);
    let scc = tarjan_scc_topo(system.vertices.len(), &adj);
    let edge_scales = vec![1.0_f64; system.edges.len()];
    let out = solve_steady_numeric(indexed, &system, &adj, &scc, params, &edge_scales, false)?;
    evaluate_goal_constraints(indexed, &system, &[out.after_vals], 0, goal_constraints)
}

fn run_monte_carlo_band(
    indexed: &IndexedSimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
    scheduled_events: &[SimEventSpec],
    use_stepped: bool,
    uc: &UncertaintyConfig,
    det_result: &mut SimResult,
) -> Result<(), AppError> {
    let normalized = normalize_actions_indexed(indexed, actions)?;
    let compiled_events = compile_scheduled_events(indexed, scheduled_events)?;
    let extra_seeds = collect_event_seed_channels(indexed, &compiled_events);
    let system =
        build_channel_system_with_extra_seeds(indexed, &normalized, &extra_seeds, params.max_hops);
    let chan_lookup = build_chan_lookup(&system);
    let steady_adj = (!use_stepped).then(|| build_channel_adj(&system, false));
    let steady_scc = steady_adj
        .as_ref()
        .map(|adj| tarjan_scc_topo(system.vertices.len(), adj));

    let mut master_rng = ChaCha8Rng::seed_from_u64(uc.seed);
    let sample_seeds: Vec<u64> = (0..uc.samples).map(|_| master_rng.random()).collect();

    let draws: Vec<Vec<f64>> = sample_seeds
        .into_par_iter()
        .map(|seed| {
            let edge_scales = sample_edge_scales(seed, system.edges.len(), uc.coefficient_cv);
            if use_stepped {
                let out = solve_stepped_numeric_with_events(&SteppedNumericInput {
                    indexed,
                    system: &system,
                    chan_lookup: &chan_lookup,
                    params,
                    edge_scales: &edge_scales,
                    events: &compiled_events,
                    record_series: false,
                })?;
                Ok(out.final_after_vals)
            } else {
                let (Some(adj), Some(scc)) = (steady_adj.as_ref(), steady_scc.as_ref()) else {
                    return Ok(Vec::new());
                };
                let out =
                    solve_steady_numeric(indexed, &system, adj, scc, params, &edge_scales, false)?;
                Ok(out.after_vals)
            }
        })
        .collect::<Result<Vec<Vec<f64>>, AppError>>()?;

    annotate_percentiles(indexed, &system, &draws, det_result)
}

fn validate_uncertainty_config(uc: &UncertaintyConfig) -> Result<(), AppError> {
    if uc.samples == 0 || uc.samples > MAX_UNCERTAINTY_SAMPLES {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("uncertainty.samples must be in 1..={MAX_UNCERTAINTY_SAMPLES}"),
        ));
    }
    if !uc.coefficient_cv.is_finite() || uc.coefficient_cv < 0.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "uncertainty.coefficient_cv must be finite and >= 0.0",
        ));
    }
    Ok(())
}

fn sample_edge_scales(seed: u64, edge_count: usize, cv: f64) -> Vec<f64> {
    if cv <= 0.0 {
        return vec![1.0_f64; edge_count];
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut spare: Option<f64> = None;
    let mut scales = Vec::with_capacity(edge_count);
    for _ in 0..edge_count {
        let z = next_standard_normal(&mut rng, &mut spare);
        scales.push((1.0 + cv * z).max(0.0));
    }
    scales
}

fn next_standard_normal(rng: &mut ChaCha8Rng, spare: &mut Option<f64>) -> f64 {
    if let Some(z) = spare.take() {
        return z;
    }
    let u1_raw: f64 = rng.random();
    let u1 = (1.0 - u1_raw).max(f64::MIN_POSITIVE);
    let u2: f64 = rng.random();
    let mag = (-2.0 * u1.ln()).sqrt();
    let angle = std::f64::consts::TAU * u2;
    *spare = Some(mag * angle.sin());
    mag * angle.cos()
}

struct QuantileWeights {
    lo: usize,
    hi: usize,
    frac: f64,
}

impl QuantileWeights {
    fn for_len(len: usize, p: f64) -> Self {
        if len <= 1 {
            return Self {
                lo: 0,
                hi: 0,
                frac: 0.0,
            };
        }
        let max_idx_u32 = u32::try_from(len.saturating_sub(1)).unwrap_or(u32::MAX);
        let pos = p * f64::from(max_idx_u32);
        let mut lo = 0_usize;
        while lo.saturating_add(1) < len
            && f64::from(u32::try_from(lo.saturating_add(1)).unwrap_or(u32::MAX)) <= pos
        {
            lo = lo.saturating_add(1);
        }
        let hi = lo.saturating_add(1).min(len.saturating_sub(1));
        let frac = (pos - f64::from(u32::try_from(lo).unwrap_or(u32::MAX))).clamp(0.0, 1.0);
        Self { lo, hi, frac }
    }

    fn eval(&self, sorted: &[f64]) -> Option<f64> {
        let v_lo = sorted.get(self.lo).copied()?;
        let v_hi = sorted.get(self.hi).copied()?;
        Some(v_lo + self.frac * (v_hi - v_lo))
    }
}

fn annotate_percentiles(
    indexed: &IndexedSimGraph,
    system: &crate::sim::steady::ChannelSystem,
    draws: &[Vec<f64>],
    det_result: &mut SimResult,
) -> Result<(), AppError> {
    let mut chan_lookup: HashMap<(Uuid, &str), usize> =
        HashMap::with_capacity(system.vertices.len());
    for (u, vert) in system.vertices.iter().enumerate() {
        if let Some(node) = indexed.nodes.get(vert.node_idx) {
            chan_lookup.insert((node.id, vert.metric.as_str()), u);
        }
    }

    let sample_count = draws.len();
    let w10 = QuantileWeights::for_len(sample_count, 0.10);
    let w90 = QuantileWeights::for_len(sample_count, 0.90);
    let mut scratch = Vec::with_capacity(sample_count);

    for row in &mut det_result.impacts {
        let Some(&chan_idx) = chan_lookup.get(&(row.node_id, row.metric.as_str())) else {
            continue;
        };
        scratch.clear();
        for draw in draws {
            if let Some(&val) = draw.get(chan_idx) {
                if !val.is_finite() {
                    return Err(AppError::bad(
                        "SIM_DIVERGENT",
                        "uncertainty sample produced non-finite value",
                    ));
                }
                scratch.push(val);
            }
        }
        scratch.sort_by(f64::total_cmp);
        row.p10 = w10.eval(&scratch);
        row.p90 = w90.eval(&scratch);
    }

    Ok(())
}
