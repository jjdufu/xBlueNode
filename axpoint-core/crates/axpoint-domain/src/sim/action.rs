//! Normalization and validation of `AxSimAction` interventions (`09-STEP10` §10.3.1).

use uuid::Uuid;

use crate::AppError;
use crate::enums::InterventionMode;
use crate::sim::SimGraph;
use crate::topology::AxSimAction;

/// Validated and canonicalized intervention ready for `steady::solve` / `stepped::solve`.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedAction {
    /// Target node UUID.
    pub target_node_id: Uuid,
    /// Target metric key.
    pub metric_key: String,
    /// Canonical mode (`RATIO`, `ADD`, or `ABSOLUTE`).
    pub mode: InterventionMode,
    /// Numeric perturbation value.
    pub value: f64,
    /// Time step at which the intervention activates (`0` in steady-state).
    pub at_step: u32,
}

/// Normalizes and validates raw [`AxSimAction`] entries against `graph` (`§10.3.1`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `delta_ratio` and `mode/value`
/// conflict, required fields are missing, `RATIO <= -1.0`, or a non-`ABSOLUTE` target is
/// missing the specified metric.
pub fn normalize_actions(
    graph: &SimGraph,
    actions: &[AxSimAction],
) -> Result<Vec<NormalizedAction>, AppError> {
    let mut out = Vec::with_capacity(actions.len());
    for act in actions {
        let (mode, value) = match (act.delta_ratio, act.mode, act.value) {
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "cannot specify both delta_ratio and mode/value",
                ));
            }
            (Some(dr), None, None) => (InterventionMode::Ratio, dr),
            (None, Some(m), Some(v)) => (m, v),
            (None, Some(_), None) => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "value is required when mode is specified",
                ));
            }
            (None, None, _) => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "either delta_ratio or mode + value must be specified",
                ));
            }
        };

        if !value.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "intervention value must be finite",
            ));
        }
        if mode == InterventionMode::Ratio && value <= -1.0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "RATIO intervention value must be > -1.0",
            ));
        }

        let node = graph
            .nodes
            .iter()
            .find(|n| n.id == act.target_node_id)
            .ok_or_else(|| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("target node {} not found in sandbox", act.target_node_id),
                )
            })?;

        if mode != InterventionMode::Absolute && !node.metrics.contains_key(&act.metric_key) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "metric missing on target node {}: {}",
                    act.target_node_id, act.metric_key
                ),
            ));
        }

        out.push(NormalizedAction {
            target_node_id: act.target_node_id,
            metric_key: act.metric_key.clone(),
            mode,
            value,
            at_step: act.at_step,
        });
    }
    Ok(out)
}
