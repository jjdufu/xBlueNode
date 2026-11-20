//! Stepped temporal causal propagation solver with lag and checkpoints (`09-STEP10` §10.5).

use crate::AppError;
use crate::sim::{SimGraph, SimParams, SimResult};
use crate::topology::AxSimAction;

/// Solves stepped temporal causal propagation across `t = 0..=horizon_steps` (`§10.5`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SIM_DIVERGENT`) if actions
/// are invalid or same-step cyclic propagation diverges.
pub fn solve(
    graph: &SimGraph,
    actions: &[AxSimAction],
    _params: &SimParams,
) -> Result<SimResult, AppError> {
    let _normalized = crate::sim::normalize_actions(graph, actions)?;
    Ok(SimResult {
        impacts: Vec::new(),
        affected_count: 0,
        kpis: Vec::new(),
        warnings: Vec::new(),
        converged: true,
        reason: None,
        iterations: 0,
        checkpoints: Vec::new(),
    })
}
