//! Goal redline invariant evaluation and reverse-lag `deadline_step` solver (`06-STEP28` §28.2.2).

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};

use uuid::Uuid;

use crate::AppError;
use crate::enums::{DeadlineWindowStatus, SimThresholdOp};
use crate::geo_ops::{matches_spatial_filter, spatial_filter_bbox};
use crate::sim::graph::{IndexedSimGraph, IndexedSimNode};
use crate::sim::steady::ChannelSystem;
use crate::sim::{DeadlineDiagnostic, DeadlineReport, SimGoalConstraint};

/// Evaluates a scalar metric value against a [`SimThresholdOp`] threshold (`LT | LTE | GT | GTE`).
#[must_use]
pub fn eval_threshold_op(val: f64, op: SimThresholdOp, threshold: f64) -> bool {
    match op {
        SimThresholdOp::Lt => val < threshold,
        SimThresholdOp::Lte => val <= threshold,
        SimThresholdOp::Gt => val > threshold,
        SimThresholdOp::Gte => val >= threshold,
    }
}

/// Validates [`SimGoalConstraint`] specifications against an [`IndexedSimGraph`] (`06-STEP28` §28.2.2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `INVALID_GEOMETRY`) if constraint fields,
/// target nodes, or spatial filters are malformed.
pub fn validate_goal_constraints(
    indexed: &IndexedSimGraph,
    constraints: &[SimGoalConstraint],
) -> Result<(), AppError> {
    for c in constraints {
        if c.id.trim().is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "goal_constraint.id must not be empty",
            ));
        }
        if c.metric_key.trim().is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("goal_constraint '{}' metric_key must not be empty", c.id),
            ));
        }
        if !c.threshold.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("goal_constraint '{}' threshold must be finite", c.id),
            ));
        }
        if let Some(tid) = c.target_node_id
            && !indexed.node_index.contains_key(&tid)
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "goal_constraint '{}' target_node_id {tid} not found in sandbox",
                    c.id
                ),
            ));
        }
        if let Some(ref sf) = c.spatial_filter {
            let _bbox = spatial_filter_bbox(sf)?;
        }
    }
    Ok(())
}

pub(crate) fn evaluate_goal_constraints(
    indexed: &IndexedSimGraph,
    system: &ChannelSystem,
    step_after_vals: &[Vec<f64>],
    horizon_steps: u32,
    constraints: &[SimGoalConstraint],
) -> Result<Option<DeadlineReport>, AppError> {
    if constraints.is_empty() {
        return Ok(None);
    }
    validate_goal_constraints(indexed, constraints)?;

    let mut chan_lookup: HashMap<(usize, &str), usize> =
        HashMap::with_capacity(system.vertices.len());
    for (u, vert) in system.vertices.iter().enumerate() {
        chan_lookup.insert((vert.node_idx, vert.metric.as_str()), u);
    }

    let in_compiled_edges = build_reverse_edge_index(indexed);
    let mut items = Vec::with_capacity(constraints.len());

    for c in constraints {
        let diag = evaluate_single_constraint(
            indexed,
            &chan_lookup,
            &in_compiled_edges,
            step_after_vals,
            horizon_steps,
            c,
        )?;
        items.push(diag);
    }

    Ok(Some(aggregate_deadline_report(items)))
}

struct BreachHit {
    step: u32,
    node_idx: usize,
    node_id: Uuid,
    value: f64,
}

fn build_reverse_edge_index(indexed: &IndexedSimGraph) -> Vec<Vec<usize>> {
    let mut in_edges = vec![Vec::new(); indexed.nodes.len()];
    for (ei, ce) in indexed.edges.iter().enumerate() {
        if let Some(slot) = in_edges.get_mut(ce.dst_node.index()) {
            slot.push(ei);
        }
    }
    in_edges
}

fn evaluate_single_constraint(
    indexed: &IndexedSimGraph,
    chan_lookup: &HashMap<(usize, &str), usize>,
    in_compiled_edges: &[Vec<usize>],
    step_after_vals: &[Vec<f64>],
    horizon_steps: u32,
    c: &SimGoalConstraint,
) -> Result<DeadlineDiagnostic, AppError> {
    let candidates = select_constraint_nodes(indexed, c)?;
    let metric = c.metric_key.trim();

    let Some(hit) = find_earliest_breach(
        &candidates,
        chan_lookup,
        step_after_vals,
        horizon_steps,
        metric,
        c.op,
        c.threshold,
    ) else {
        return Ok(DeadlineDiagnostic {
            constraint_id: c.id.clone(),
            constraint_name: c.name.clone(),
            breached: false,
            breach_node_id: None,
            breach_step: None,
            breach_value: None,
            min_lag_steps: None,
            deadline_step: None,
            status: DeadlineWindowStatus::Satisfied,
            recommended_sources: Vec::new(),
        });
    };

    let (min_lag_steps, recommended_sources) = compute_reverse_min_lag(
        indexed,
        in_compiled_edges,
        hit.node_idx,
        metric,
        &c.controllable_node_ids,
    );

    let (deadline_step, status) = match min_lag_steps {
        Some(l_min) => {
            let dl_i64 = i64::from(hit.step) - i64::from(l_min) - 1;
            let dl = i32::try_from(dl_i64).unwrap_or(i32::MIN);
            let st = if dl >= 0 {
                DeadlineWindowStatus::ActionableWindow
            } else {
                DeadlineWindowStatus::ImmediateBreachUnavoidableByLag
            };
            (Some(dl), st)
        }
        None => (None, DeadlineWindowStatus::ImmediateBreachUnavoidableByLag),
    };

    Ok(DeadlineDiagnostic {
        constraint_id: c.id.clone(),
        constraint_name: c.name.clone(),
        breached: true,
        breach_node_id: Some(hit.node_id),
        breach_step: Some(hit.step),
        breach_value: Some(hit.value),
        min_lag_steps,
        deadline_step,
        status,
        recommended_sources,
    })
}

fn select_constraint_nodes<'a>(
    indexed: &'a IndexedSimGraph,
    c: &SimGoalConstraint,
) -> Result<Vec<&'a IndexedSimNode>, AppError> {
    let type_filter = c
        .target_type_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut out = Vec::new();
    for node in &indexed.nodes {
        if c.target_node_id.is_some_and(|tid| node.id != tid) {
            continue;
        }
        if type_filter.is_some_and(|tf| node.type_name != tf) {
            continue;
        }
        if let Some(ref sf) = c.spatial_filter
            && !matches_spatial_filter(node.geo, node.geo_shape.as_ref(), sf)?
        {
            continue;
        }
        out.push(node);
    }
    out.sort_by_key(|n| n.id);
    Ok(out)
}

fn find_earliest_breach(
    candidates: &[&IndexedSimNode],
    chan_lookup: &HashMap<(usize, &str), usize>,
    step_after_vals: &[Vec<f64>],
    horizon_steps: u32,
    metric: &str,
    op: SimThresholdOp,
    threshold: f64,
) -> Option<BreachHit> {
    for t in 0..=horizon_steps {
        let step_idx = usize::try_from(t).unwrap_or(usize::MAX);
        for &node in candidates {
            let n_idx = node.idx.index();
            let val_opt = match chan_lookup.get(&(n_idx, metric)) {
                Some(&chan_idx) => step_after_vals
                    .get(step_idx)
                    .and_then(|row| row.get(chan_idx))
                    .copied()
                    .or_else(|| node.metrics.get(metric).copied()),
                None => node.metrics.get(metric).copied(),
            };
            if let Some(value) = val_opt
                && !eval_threshold_op(value, op, threshold)
            {
                return Some(BreachHit {
                    step: t,
                    node_idx: n_idx,
                    node_id: node.id,
                    value,
                });
            }
        }
    }
    None
}

fn compute_reverse_min_lag(
    indexed: &IndexedSimGraph,
    in_compiled_edges: &[Vec<usize>],
    breach_node_idx: usize,
    breach_metric: &str,
    controllable_node_ids: &[Uuid],
) -> (Option<u32>, Vec<Uuid>) {
    let mut dist: HashMap<(usize, String), u32> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<(u32, usize, String)>> = BinaryHeap::new();

    let start_key = (breach_node_idx, String::from(breach_metric));
    dist.insert(start_key.clone(), 0);
    heap.push(Reverse((0, start_key.0, start_key.1)));

    while let Some(Reverse((d, cur_node, cur_m))) = heap.pop() {
        if dist
            .get(&(cur_node, cur_m.clone()))
            .is_some_and(|&best| d > best)
        {
            continue;
        }
        let Some(incoming) = in_compiled_edges.get(cur_node) else {
            continue;
        };
        for &ei in incoming {
            let Some(ce) = indexed.edges.get(ei) else {
                continue;
            };
            if ce.gain.abs() <= 1e-12 {
                continue;
            }
            if ce.to_metric.as_deref().is_some_and(|tm| tm != cur_m) {
                continue;
            }
            let src_m = ce
                .from_metric
                .as_deref()
                .unwrap_or(cur_m.as_str())
                .to_string();
            let src_n = ce.src_node.index();
            let next_d = d.saturating_add(ce.delay_steps);
            let key = (src_n, src_m.clone());
            if dist.get(&key).is_none_or(|&prev| next_d < prev) {
                dist.insert(key, next_d);
                heap.push(Reverse((next_d, src_n, src_m)));
            }
        }
    }

    let mut min_by_node: HashMap<usize, u32> = HashMap::new();
    for ((n_idx, _), lag) in dist {
        min_by_node
            .entry(n_idx)
            .and_modify(|cur| *cur = (*cur).min(lag))
            .or_insert(lag);
    }

    resolve_controllable_sources(
        indexed,
        breach_node_idx,
        &min_by_node,
        controllable_node_ids,
    )
}

fn resolve_controllable_sources(
    indexed: &IndexedSimGraph,
    breach_node_idx: usize,
    min_by_node: &HashMap<usize, u32>,
    controllable_node_ids: &[Uuid],
) -> (Option<u32>, Vec<Uuid>) {
    if !controllable_node_ids.is_empty() {
        let mut best_lag: Option<u32> = None;
        for ctrl_id in controllable_node_ids {
            if let Some(&n_idx) = indexed.node_index.get(ctrl_id)
                && let Some(&lag) = min_by_node.get(&n_idx)
            {
                best_lag = Some(best_lag.map_or(lag, |cur| cur.min(lag)));
            }
        }
        let Some(l_min) = best_lag else {
            return (None, Vec::new());
        };
        let mut seen = BTreeSet::new();
        let mut sources = Vec::new();
        for &ctrl_id in controllable_node_ids {
            if let Some(&n_idx) = indexed.node_index.get(&ctrl_id)
                && min_by_node.get(&n_idx) == Some(&l_min)
                && seen.insert(ctrl_id)
            {
                sources.push(ctrl_id);
            }
        }
        return (Some(l_min), sources);
    }

    let best_lag = min_by_node
        .iter()
        .filter(|(n_idx, _)| **n_idx != breach_node_idx)
        .map(|(_, &lag)| lag)
        .min();
    let Some(l_min) = best_lag else {
        return (None, Vec::new());
    };

    let mut sources: Vec<Uuid> = min_by_node
        .iter()
        .filter(|(n_idx, lag)| **n_idx != breach_node_idx && **lag == l_min)
        .filter_map(|(&n_idx, _)| indexed.nodes.get(n_idx).map(|n| n.id))
        .collect();
    sources.sort();
    (Some(l_min), sources)
}

fn aggregate_deadline_report(items: Vec<DeadlineDiagnostic>) -> DeadlineReport {
    let all_satisfied = items.iter().all(|d| !d.breached);
    let earliest_breach_step = items.iter().filter_map(|d| d.breach_step).min();
    let earliest_deadline_step = items.iter().filter_map(|d| d.deadline_step).min();
    let status = if items
        .iter()
        .any(|d| d.status == DeadlineWindowStatus::ImmediateBreachUnavoidableByLag)
    {
        DeadlineWindowStatus::ImmediateBreachUnavoidableByLag
    } else if items
        .iter()
        .any(|d| d.status == DeadlineWindowStatus::ActionableWindow)
    {
        DeadlineWindowStatus::ActionableWindow
    } else {
        DeadlineWindowStatus::Satisfied
    };

    DeadlineReport {
        all_satisfied,
        earliest_breach_step,
        earliest_deadline_step,
        status,
        items,
    }
}
