//! Monte-Carlo edge-weight uncertainty sampling with reproducible `ChaCha8Rng` and `rayon` (`09-STEP10` §10.6).

use std::collections::HashMap;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use uuid::Uuid;

use crate::AppError;
use crate::sim::action::normalize_actions_indexed;
use crate::sim::graph::IndexedSimGraph;
use crate::sim::scc::tarjan_scc_topo;
use crate::sim::steady::{build_channel_adj, build_channel_system, solve_steady_numeric};
use crate::sim::stepped::solve_stepped_numeric;
use crate::sim::{SimGraph, SimMode, SimParams, SimResult, UncertaintyConfig};
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
    let indexed = IndexedSimGraph::from_sim_graph(graph)?;
    solve_indexed(&indexed, actions, params)
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
    let mut det_result = match params.mode {
        SimMode::Steady => crate::sim::steady::solve_indexed(indexed, actions, params)?,
        SimMode::Stepped => crate::sim::stepped::solve_indexed(indexed, actions, params)?,
    };

    let Some(uc) = &params.uncertainty else {
        return Ok(det_result);
    };
    validate_uncertainty_config(uc)?;

    if det_result.impacts.is_empty() {
        return Ok(det_result);
    }

    let normalized = normalize_actions_indexed(indexed, actions)?;
    let system = build_channel_system(indexed, &normalized, params.max_hops);
    let steady_adj = if params.mode == SimMode::Steady {
        Some(build_channel_adj(&system, false))
    } else {
        None
    };
    let steady_scc = steady_adj
        .as_ref()
        .map(|adj| tarjan_scc_topo(system.vertices.len(), adj));

    let mut master_rng = ChaCha8Rng::seed_from_u64(uc.seed);
    let sample_seeds: Vec<u64> = (0..uc.samples).map(|_| master_rng.random()).collect();

    let draws: Vec<Vec<f64>> = sample_seeds
        .into_par_iter()
        .map(|seed| {
            let edge_scales = sample_edge_scales(seed, system.edges.len(), uc.coefficient_cv);
            match params.mode {
                SimMode::Steady => {
                    let Some(adj) = steady_adj.as_ref() else {
                        return Ok(Vec::new());
                    };
                    let Some(scc) = steady_scc.as_ref() else {
                        return Ok(Vec::new());
                    };
                    let out = solve_steady_numeric(
                        indexed,
                        &system,
                        adj,
                        scc,
                        params,
                        &edge_scales,
                        false,
                    )?;
                    Ok(out.after_vals)
                }
                SimMode::Stepped => {
                    let out = solve_stepped_numeric(indexed, &system, params, &edge_scales, false)?;
                    Ok(out.final_after_vals)
                }
            }
        })
        .collect::<Result<Vec<Vec<f64>>, AppError>>()?;

    annotate_percentiles(indexed, &system, &draws, &mut det_result)?;
    Ok(det_result)
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
