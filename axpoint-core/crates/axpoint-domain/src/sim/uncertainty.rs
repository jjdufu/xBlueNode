//! Monte-Carlo edge-weight uncertainty sampling with reproducible `ChaCha8Rng` and `rayon` (`09-STEP10` §10.6).

use crate::AppError;
use crate::sim::{SimGraph, SimParams, SimResult};
use crate::topology::AxSimAction;

/// Runs deterministic solve plus optional Monte-Carlo `P10` / `P90` uncertainty quantification (`§10.6`).
///
/// # Errors
/// Propagates any validation or divergence error from [`crate::sim::steady::solve`] or [`crate::sim::stepped::solve`].
pub fn solve(
    graph: &SimGraph,
    actions: &[AxSimAction],
    params: &SimParams,
) -> Result<SimResult, AppError> {
    crate::sim::steady::solve(graph, actions, params)
}
