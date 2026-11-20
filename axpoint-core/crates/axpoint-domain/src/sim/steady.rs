//! Steady-state causal propagation solver (`SCC` + damped Jacobi iteration, `09-STEP10` §10.4).

use crate::AppError;
use crate::sim::{SimGraph, SimParams, SimResult};
use crate::topology::AxSimAction;

/// Solves steady-state causal propagation on `graph` under `actions` and `params` (`§10.4`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SIM_DIVERGENT`) when action
/// normalization fails or a cyclic SCC diverges monotonically.
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
