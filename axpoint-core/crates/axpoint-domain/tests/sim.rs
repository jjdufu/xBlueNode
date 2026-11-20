//! Comprehensive unit, analytical-reference, and benchmark tests for `AxSim` (`09-STEP10` §10.3–§10.6, §10.11).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::Instant;

use axpoint_domain::AppError;
use axpoint_domain::enums::InterventionMode;
use axpoint_domain::sim::graph::{
    CompiledEdge, EdgeSaturation, IndexedSimGraph, NodeIdx, PropagationClamp, PropagationCurve,
    PropagationRule, PropagationSpec, SimGraph, SimLink, SimNode,
};
use axpoint_domain::sim::{
    SimMode, SimParams, SimResult, UncertaintyConfig, explain_impact_path, normalize_actions,
    steady, stepped, tarjan_scc_topo, uncertainty,
};
use axpoint_domain::topology::AxSimAction;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use uuid::Uuid;

fn det_id(idx: u128) -> Uuid {
    Uuid::from_u128(idx.saturating_add(1))
}

fn make_node(idx: u128, type_name: &str, title: &str, metrics: &[(&str, f64)]) -> SimNode {
    let mut map = BTreeMap::new();
    for &(k, v) in metrics {
        map.insert(k.to_string(), v);
    }
    SimNode {
        id: det_id(idx),
        type_name: type_name.to_string(),
        node_key: format!("KEY-{idx}"),
        title: title.to_string(),
        metrics: map,
        properties: serde_json::Map::new(),
        row_version: 1,
    }
}

fn make_link(idx: u128, link_type: &str, src: Uuid, dst: Uuid, weight: f64) -> SimLink {
    SimLink {
        id: det_id(10_000_u128.saturating_add(idx)),
        link_type: link_type.to_string(),
        src_node_id: src,
        dst_node_id: dst,
        weight,
    }
}

fn ratio_action(target: Uuid, metric: &str, value: f64) -> AxSimAction {
    AxSimAction {
        target_node_id: target,
        metric_key: metric.to_string(),
        mode: Some(InterventionMode::Ratio),
        value: Some(value),
        delta_ratio: None,
        at_step: 0,
    }
}

fn find_impact(res: &SimResult, node_id: Uuid, metric: &str) -> Result<f64, AppError> {
    res.impacts
        .iter()
        .find(|r| r.node_id == node_id && r.metric == metric)
        .map(|r| r.after_val)
        .ok_or_else(|| AppError::bad("TEST_MISSING", format!("missing impact {node_id}/{metric}")))
}

#[test]
fn sim_steady_chain_exact() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("output", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("output", 100.0)]);
    let c = make_node(3, "Order", "C", &[("output", 100.0)]);
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone()],
        links: vec![
            make_link(1, "FLOW", a.id, b.id, 0.5),
            make_link(2, "FLOW", b.id, c.id, 0.4),
        ],
        edge_specs: BTreeMap::new(),
    };

    let res = steady::solve(
        &graph,
        &[ratio_action(a.id, "output", -0.5)],
        &SimParams::default(),
    )?;
    assert!(res.converged);
    assert!((find_impact(&res, a.id, "output")? - 50.0).abs() < 1e-9);
    assert!((find_impact(&res, b.id, "output")? - 75.0).abs() < 1e-9);
    assert!((find_impact(&res, c.id, "output")? - 90.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn sim_steady_diamond_sums_paths() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let c = make_node(3, "Warehouse", "C", &[("m", 100.0)]);
    let d = make_node(4, "Order", "D", &[("m", 100.0)]);
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone(), d.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.5),
            make_link(2, "REL", a.id, c.id, 0.6),
            make_link(3, "REL", b.id, d.id, 0.4),
            make_link(4, "REL", c.id, d.id, 0.5),
        ],
        edge_specs: BTreeMap::new(),
    };

    let res = steady::solve(
        &graph,
        &[ratio_action(a.id, "m", -0.5)],
        &SimParams::default(),
    )?;
    // B = -25%, C = -30%, D = (-0.25 * 0.4) + (-0.30 * 0.5) = -0.10 - 0.15 = -0.25 -> 75.0
    assert!((find_impact(&res, d.id, "m")? - 75.0).abs() < 1e-9);
    assert_eq!(res.kpis.len(), 3);
    Ok(())
}

#[test]
fn sim_steady_two_cycle_do_operator_cuts_incoming() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.5),
            make_link(2, "REL", b.id, a.id, 0.5),
        ],
        edge_specs: BTreeMap::new(),
    };

    let res = steady::solve(
        &graph,
        &[ratio_action(a.id, "m", -0.5)],
        &SimParams::default(),
    )?;
    // A is intervened (B->A is cut), so B = -25% exact and A remains -50%.
    assert!((find_impact(&res, a.id, "m")? - 50.0).abs() < 1e-9);
    assert!((find_impact(&res, b.id, "m")? - 75.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn sim_steady_three_cycle_matches_analytical_inverse() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let c = make_node(3, "Warehouse", "C", &[("m", 100.0)]);
    let d = make_node(4, "Order", "D", &[("m", 100.0)]);
    // Cycle B->C->D->B with weights 0.5, 0.5, 0.5 (product = 0.125), external A->B with weight 0.8.
    // Input c_B = -0.5 * 0.8 = -0.4.
    // Analytical: x_B = c_B / (1 - 0.125) = -0.4 / 0.875 = -0.45714285714285713
    // x_C = 0.5 * x_B, x_D = 0.25 * x_B.
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone(), d.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.8),
            make_link(2, "REL", b.id, c.id, 0.5),
            make_link(3, "REL", c.id, d.id, 0.5),
            make_link(4, "REL", d.id, b.id, 0.5),
        ],
        edge_specs: BTreeMap::new(),
    };

    let res = steady::solve(
        &graph,
        &[ratio_action(a.id, "m", -0.5)],
        &SimParams::default(),
    )?;
    assert!(res.converged);
    let xb = -0.4_f64 / 0.875_f64;
    let expected_b = 100.0 * (1.0 + xb);
    let expected_c = 100.0 * (1.0 + 0.5 * xb);
    let expected_d = 100.0 * (1.0 + 0.25 * xb);
    assert!((find_impact(&res, b.id, "m")? - expected_b).abs() < 1e-8);
    assert!((find_impact(&res, c.id, "m")? - expected_c).abs() < 1e-8);
    assert!((find_impact(&res, d.id, "m")? - expected_d).abs() < 1e-8);
    Ok(())
}

#[test]
fn sim_steady_divergent_cycle_returns_sim_divergent() {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let c = make_node(3, "Warehouse", "C", &[("m", 100.0)]);
    let d = make_node(4, "Order", "D", &[("m", 100.0)]);
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone(), d.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.5),
            make_link(2, "REL", b.id, c.id, 1.2),
            make_link(3, "REL", c.id, d.id, 1.0),
            make_link(4, "REL", d.id, b.id, 1.0),
        ],
        edge_specs: BTreeMap::new(),
    };

    let err = steady::solve(
        &graph,
        &[ratio_action(a.id, "m", 0.2)],
        &SimParams::default(),
    );
    assert!(err.is_err());
    if let Err(e) = err {
        assert_eq!(e.error_code(), "SIM_DIVERGENT");
    }
}

#[test]
fn sim_steady_max_iter_emits_warning_and_non_converged() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let c = make_node(3, "Warehouse", "C", &[("m", 100.0)]);
    // Cycle product = 0.99 (converges slowly), capped at max_iterations = 10 -> MAX_ITER.
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.5),
            make_link(2, "REL", b.id, c.id, 0.99),
            make_link(3, "REL", c.id, b.id, 1.0),
        ],
        edge_specs: BTreeMap::new(),
    };

    let params = SimParams {
        max_iterations: 10,
        ..SimParams::default()
    };
    let res = steady::solve(&graph, &[ratio_action(a.id, "m", 0.5)], &params)?;
    assert!(!res.converged);
    assert_eq!(res.reason.as_deref(), Some("MAX_ITER"));
    assert!(res.warnings.iter().any(|w| w.code == "MAX_ITER"));
    Ok(())
}

#[test]
fn sim_action_modes_validation_and_shorthand_equivalence() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("output", 200.0)]);
    let b = make_node(2, "Warehouse", "B", &[("inventory", 100.0)]);
    let c = make_node(3, "Order", "C", &[("fulfillment", 1.0)]);

    let mut edge_specs = BTreeMap::new();
    edge_specs.insert(
        String::from("SUPPLIES"),
        PropagationSpec {
            rules: vec![PropagationRule {
                src_metric: String::from("output"),
                dst_metric: String::from("inventory"),
                mode: InterventionMode::Ratio,
                coefficient: None,
                lag_steps: 0,
            }],
            clamp: PropagationClamp::default(),
        },
    );
    edge_specs.insert(
        String::from("FULFILLS"),
        PropagationSpec {
            rules: vec![PropagationRule {
                src_metric: String::from("inventory"),
                dst_metric: String::from("fulfillment"),
                mode: InterventionMode::Ratio,
                coefficient: Some(0.5),
                lag_steps: 0,
            }],
            clamp: PropagationClamp::default(),
        },
    );

    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone()],
        links: vec![
            make_link(1, "SUPPLIES", a.id, b.id, 0.8),
            make_link(2, "FULFILLS", b.id, c.id, 1.0),
        ],
        edge_specs,
    };

    let shorthand = AxSimAction {
        target_node_id: a.id,
        metric_key: String::from("output"),
        mode: None,
        value: None,
        delta_ratio: Some(-0.3),
        at_step: 0,
    };
    let explicit = ratio_action(a.id, "output", -0.3);

    let r1 = steady::solve(&graph, &[shorthand], &SimParams::default())?;
    let r2 = steady::solve(&graph, &[explicit], &SimParams::default())?;
    assert_eq!(r1.impacts, r2.impacts);
    assert!((find_impact(&r1, c.id, "fulfillment")? - 0.88).abs() < 1e-9);

    // Reject missing metric on RATIO/ADD.
    let missing_ratio = steady::solve(
        &graph,
        &[ratio_action(a.id, "no_such", -0.2)],
        &SimParams::default(),
    );
    assert!(missing_ratio.is_err());

    // Reject conflicting delta_ratio + mode.
    let conflict = normalize_actions(
        &graph,
        &[AxSimAction {
            target_node_id: a.id,
            metric_key: String::from("output"),
            mode: Some(InterventionMode::Ratio),
            value: Some(-0.1),
            delta_ratio: Some(-0.1),
            at_step: 0,
        }],
    );
    assert!(conflict.is_err());

    // Allow ABSOLUTE on missing metric with before_val = None, delta = None, delta_ratio = None.
    let abs_missing = AxSimAction {
        target_node_id: a.id,
        metric_key: String::from("new_kpi"),
        mode: Some(InterventionMode::Absolute),
        value: Some(42.5),
        delta_ratio: None,
        at_step: 0,
    };
    let r_abs = steady::solve(&graph, &[abs_missing], &SimParams::default())?;
    let row = r_abs
        .impacts
        .iter()
        .find(|r| r.node_id == a.id && r.metric == "new_kpi")
        .ok_or_else(|| AppError::bad("TEST", "missing ABSOLUTE row"))?;
    assert_eq!(row.before_val, None);
    assert_eq!(row.delta, None);
    assert_eq!(row.delta_ratio, None);
    assert!((row.after_val - 42.5).abs() < 1e-9);

    // Test ADD mode propagating through RATIO downstream.
    let add_act = AxSimAction {
        target_node_id: a.id,
        metric_key: String::from("output"),
        mode: Some(InterventionMode::Add),
        value: Some(-60.0), // -30% of 200.0
        delta_ratio: None,
        at_step: 0,
    };
    let r_add = steady::solve(&graph, &[add_act], &SimParams::default())?;
    assert!((find_impact(&r_add, c.id, "fulfillment")? - 0.88).abs() < 1e-9);
    Ok(())
}

#[test]
fn sim_missing_intermediate_metric_passes_through_with_warning() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[]); // missing 'm'
    let c = make_node(3, "Order", "C", &[("m", 100.0)]);
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.5),
            make_link(2, "REL", b.id, c.id, 0.4),
        ],
        edge_specs: BTreeMap::new(),
    };

    let res = steady::solve(
        &graph,
        &[ratio_action(a.id, "m", -0.5)],
        &SimParams::default(),
    )?;
    assert!(
        res.warnings
            .iter()
            .any(|w| w.node_id == b.id && w.code == "METRIC_MISSING")
    );
    assert!(!res.impacts.iter().any(|r| r.node_id == b.id));
    assert!((find_impact(&res, c.id, "m")? - 90.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn sim_curves_saturation_and_bounds() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let c = make_node(3, "Order", "C", &[("m", 100.0)]);
    let d = make_node(4, "Order", "D", &[("m", 10.0)]);

    let base_graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone(), d.clone()],
        links: Vec::new(),
        edge_specs: BTreeMap::new(),
    };
    let mut indexed = IndexedSimGraph::from_sim_graph(&base_graph)?;
    indexed.set_metric_bounds(
        d.id,
        "m",
        PropagationClamp {
            min: Some(5.0),
            max: Some(15.0),
        },
    )?;

    // Threshold edge A->B (threshold 0.3, gain 0.5)
    indexed.add_custom_edge(CompiledEdge {
        src_node: NodeIdx(0),
        dst_node: NodeIdx(1),
        edge_type: String::from("THRESH"),
        from_metric: Some(String::from("m")),
        to_metric: Some(String::from("m")),
        mode: InterventionMode::Ratio,
        gain: 0.5,
        weight: 0.5,
        delay_steps: 0,
        curve: PropagationCurve::Threshold { threshold: 0.3 },
        saturation: EdgeSaturation::default(),
        clamp: PropagationClamp::default(),
    })?;
    // StepFunction edge A->C: steps [(0.0, 0.0), (0.2, 0.1), (0.4, 0.25)], saturated at max 0.2
    indexed.add_custom_edge(CompiledEdge {
        src_node: NodeIdx(0),
        dst_node: NodeIdx(2),
        edge_type: String::from("STEP"),
        from_metric: Some(String::from("m")),
        to_metric: Some(String::from("m")),
        mode: InterventionMode::Ratio,
        gain: 1.0,
        weight: 1.0,
        delay_steps: 0,
        curve: PropagationCurve::StepFunction {
            steps: vec![(0.0, 0.0), (0.2, 0.1), (0.4, 0.25)],
        },
        saturation: EdgeSaturation {
            min: Some(-0.2),
            max: Some(0.2),
        },
        clamp: PropagationClamp::default(),
    })?;
    // Linear edge A->D with gain 2.0 (would go to 10*(1+1)=20, clamped to max 15.0)
    indexed.add_custom_edge(CompiledEdge {
        src_node: NodeIdx(0),
        dst_node: NodeIdx(3),
        edge_type: String::from("LIN"),
        from_metric: Some(String::from("m")),
        to_metric: Some(String::from("m")),
        mode: InterventionMode::Ratio,
        gain: 2.0,
        weight: 2.0,
        delay_steps: 0,
        curve: PropagationCurve::Linear,
        saturation: EdgeSaturation::default(),
        clamp: PropagationClamp::default(),
    })?;

    // Below threshold 0.2 < 0.3 -> B unaffected, C gets 0.1 -> 110, D = 10*(1+0.4)=14
    let r_low = steady::solve_indexed(
        &indexed,
        &[ratio_action(a.id, "m", 0.2)],
        &SimParams::default(),
    )?;
    assert!(!r_low.impacts.iter().any(|r| r.node_id == b.id));
    assert!((find_impact(&r_low, c.id, "m")? - 110.0).abs() < 1e-9);
    assert!((find_impact(&r_low, d.id, "m")? - 14.0).abs() < 1e-9);

    // Above threshold 0.5 >= 0.3 -> B = 125, C saturated at 0.2 -> 120, D clamped at 15.0
    let r_high = steady::solve_indexed(
        &indexed,
        &[ratio_action(a.id, "m", 0.5)],
        &SimParams::default(),
    )?;
    assert!((find_impact(&r_high, b.id, "m")? - 125.0).abs() < 1e-9);
    assert!((find_impact(&r_high, c.id, "m")? - 120.0).abs() < 1e-9);
    assert!((find_impact(&r_high, d.id, "m")? - 15.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn sim_top_path_max_product_and_explain() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "Factory-A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "Hub-B", &[("m", 100.0)]);
    let c = make_node(3, "Warehouse", "Hub-C", &[("m", 100.0)]);
    let d = make_node(4, "Order", "Order-D", &[("m", 100.0)]);

    // Direct 1-hop A->D has weight 0.1; 2-hop A->B->D has 0.8*0.5 = 0.4 > 0.1; A->C->D has 0.3*0.3=0.09
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone(), d.clone()],
        links: vec![
            make_link(1, "DIRECT", a.id, d.id, 0.1),
            make_link(2, "SUPPLIES", a.id, b.id, 0.8),
            make_link(3, "FULFILLS", b.id, d.id, 0.5),
            make_link(4, "SUPPLIES", a.id, c.id, 0.3),
            make_link(5, "FULFILLS", c.id, d.id, 0.3),
        ],
        edge_specs: BTreeMap::new(),
    };

    let res = steady::solve(
        &graph,
        &[ratio_action(a.id, "m", -0.5)],
        &SimParams::default(),
    )?;
    let row_d = res
        .impacts
        .iter()
        .find(|r| r.node_id == d.id)
        .ok_or_else(|| AppError::bad("TEST", "missing D"))?;
    assert_eq!(row_d.hop_distance, 1);
    assert_eq!(row_d.top_path, vec![a.id, b.id, d.id]);

    let explanation = explain_impact_path(&graph, row_d);
    assert_eq!(explanation.steps.len(), 3);
    assert!((explanation.total_gain - 0.4).abs() < 1e-9);
    assert!(explanation.summary.contains("Factory-A"));
    assert!(explanation.summary.contains("Order-D"));
    Ok(())
}

#[test]
fn sim_stepped_lag_at_step_checkpoints_and_top50_series() -> Result<(), AppError> {
    // 65-node chain where node 0 -> 1 -> 2 ... -> 64, lag=1 on 0->1, lag=0 on rest, at_step=1.
    let mut nodes = Vec::with_capacity(65);
    let mut links = Vec::with_capacity(64);
    for i in 0..65_u128 {
        nodes.push(make_node(
            i + 1,
            "Node",
            &format!("N-{i}"),
            &[(
                "val",
                1000.0 / (f64::from(u32::try_from(i).unwrap_or(0)) + 1.0),
            )],
        ));
    }
    let mut edge_specs = BTreeMap::new();
    edge_specs.insert(
        String::from("LAGGED"),
        PropagationSpec {
            rules: vec![PropagationRule {
                src_metric: String::from("val"),
                dst_metric: String::from("val"),
                mode: InterventionMode::Ratio,
                coefficient: Some(0.9),
                lag_steps: 1,
            }],
            clamp: PropagationClamp::default(),
        },
    );

    for i in 0..64_usize {
        let lt = if i == 0 { "LAGGED" } else { "INSTANT" };
        if let (Some(src), Some(dst)) = (nodes.get(i), nodes.get(i + 1)) {
            links.push(make_link(
                u128::try_from(i + 1).unwrap_or(1),
                lt,
                src.id,
                dst.id,
                0.98,
            ));
        }
    }

    let graph = SimGraph {
        nodes: nodes.clone(),
        links,
        edge_specs,
    };
    let action = AxSimAction {
        target_node_id: nodes.first().map_or_else(Uuid::nil, |n| n.id),
        metric_key: String::from("val"),
        mode: Some(InterventionMode::Ratio),
        value: Some(-0.5),
        delta_ratio: None,
        at_step: 1,
    };
    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 6,
        checkpoint_every: 1,
        ..SimParams::default()
    };

    let res = stepped::solve(&graph, &[action], &params)?;
    assert_eq!(res.checkpoints.len(), 7);
    assert_eq!(res.affected_count, 65);

    // At t=0: action not active; at t=1: N-0 drops to 500, N-1 still 500; at t=2: N-1 drops by 0.9*(-0.5)=-0.45 -> 275.
    let n0_id = nodes.first().map_or_else(Uuid::nil, |n| n.id);
    let n1_id = nodes.get(1).map_or_else(Uuid::nil, |n| n.id);
    let n0_series = &res
        .impacts
        .iter()
        .find(|r| r.node_id == n0_id)
        .ok_or_else(|| AppError::bad("TEST", "missing N-0"))?
        .series;
    assert_eq!(n0_series.len(), 7);
    assert!((n0_series.first().map_or(0.0, |p| p.value) - 1000.0).abs() < 1e-9);
    assert!((n0_series.get(1).map_or(0.0, |p| p.value) - 500.0).abs() < 1e-9);

    let n1_series = &res
        .impacts
        .iter()
        .find(|r| r.node_id == n1_id)
        .ok_or_else(|| AppError::bad("TEST", "missing N-1"))?
        .series;
    assert!((n1_series.get(1).map_or(0.0, |p| p.value) - 500.0).abs() < 1e-9);
    assert!((n1_series.get(2).map_or(0.0, |p| p.value) - 275.0).abs() < 1e-9);

    // Top 50 have series, remaining 15 do not.
    let with_series = res.impacts.iter().filter(|r| !r.series.is_empty()).count();
    assert_eq!(with_series, 50);
    Ok(())
}

#[test]
fn sim_uncertainty_reproducible_and_percentile_ordered() -> Result<(), AppError> {
    let a = make_node(1, "Factory", "A", &[("m", 100.0)]);
    let b = make_node(2, "Warehouse", "B", &[("m", 100.0)]);
    let c = make_node(3, "Order", "C", &[("m", 100.0)]);
    let graph = SimGraph {
        nodes: vec![a.clone(), b.clone(), c.clone()],
        links: vec![
            make_link(1, "REL", a.id, b.id, 0.6),
            make_link(2, "REL", b.id, c.id, 0.5),
        ],
        edge_specs: BTreeMap::new(),
    };

    let params = SimParams {
        uncertainty: Some(UncertaintyConfig {
            samples: 100,
            coefficient_cv: 0.25,
            seed: 42,
        }),
        ..SimParams::default()
    };

    let r1 = uncertainty::solve(&graph, &[ratio_action(a.id, "m", -0.4)], &params)?;
    let r2 = uncertainty::solve(&graph, &[ratio_action(a.id, "m", -0.4)], &params)?;
    assert_eq!(r1.impacts, r2.impacts);

    let c_row = r1
        .impacts
        .iter()
        .find(|r| r.node_id == c.id)
        .ok_or_else(|| AppError::bad("TEST", "missing C"))?;
    let p10 = c_row
        .p10
        .ok_or_else(|| AppError::bad("TEST", "missing p10"))?;
    let p90 = c_row
        .p90
        .ok_or_else(|| AppError::bad("TEST", "missing p90"))?;
    assert!(p10 < c_row.after_val && c_row.after_val < p90);
    Ok(())
}

#[test]
fn sim_scc_iterative_deep_chain_no_stack_overflow() {
    let n = 20_000_usize;
    let mut adj = vec![Vec::new(); n];
    for (u, list) in adj.iter_mut().enumerate().take(n - 1) {
        list.push(u + 1);
    }
    let scc = tarjan_scc_topo(n, &adj);
    assert_eq!(scc.components.len(), n);
    assert_eq!(
        scc.components.first().and_then(|c| c.first()).copied(),
        Some(0)
    );
    assert_eq!(
        scc.components.last().and_then(|c| c.first()).copied(),
        Some(n - 1)
    );
}

#[test]
fn sim_1k_node_cyclic_graph_solves_fast() -> Result<(), AppError> {
    let graph = generate_benchmark_graph(1_000, 4_000, 10, 42);
    let seed_id = graph
        .nodes
        .first()
        .map(|n| n.id)
        .ok_or_else(|| AppError::bad("TEST", "empty graph"))?;
    let t0 = Instant::now();
    let res = steady::solve(
        &graph,
        &[ratio_action(seed_id, "m", -0.3)],
        &SimParams::default(),
    )?;
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert!(res.converged);
    assert!(res.affected_count > 100);
    assert!(elapsed_ms < 500.0, "1k solve took {elapsed_ms:.1} ms");
    Ok(())
}

fn generate_benchmark_graph(
    num_nodes: usize,
    num_links: usize,
    num_cycles: usize,
    seed: u64,
) -> SimGraph {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut nodes = Vec::with_capacity(num_nodes);
    for i in 0..num_nodes {
        let type_name = match i % 3 {
            0 => "Factory",
            1 => "Warehouse",
            _ => "Order",
        };
        let mut metrics = BTreeMap::new();
        metrics.insert(String::from("m"), 100.0);
        nodes.push(SimNode {
            id: det_id(u128::try_from(i).unwrap_or(0)),
            type_name: type_name.to_string(),
            node_key: format!("N-{i}"),
            title: format!("Node {i}"),
            metrics,
            properties: serde_json::Map::new(),
            row_version: 1,
        });
    }

    let mut links = Vec::with_capacity(num_links);
    let mut link_seq = 0_u128;
    let mut cycle_of = vec![None; num_nodes];

    // Seed backbone from node 0 into each cycle so all 100 cycles (size 10..=50) are reachable.
    let mut cursor = 1_usize;
    for c in 0..num_cycles {
        let cycle_len = 10_usize.saturating_add(c % 41); // 10..=50
        if cursor.saturating_add(cycle_len) >= num_nodes {
            break;
        }
        let start = cursor;
        if let (Some(src), Some(dst)) = (nodes.first(), nodes.get(start)) {
            links.push(make_link(link_seq, "FLOW", src.id, dst.id, 0.3));
            link_seq = link_seq.saturating_add(1);
        }
        for offset in 0..cycle_len {
            let u = start + offset;
            if let Some(slot) = cycle_of.get_mut(u) {
                *slot = Some(c);
            }
            let v = if offset + 1 == cycle_len {
                start
            } else {
                u + 1
            };
            // Product <= 0.5^10 < 0.001, guaranteed stable under CV=0.2 Monte-Carlo draws.
            if let (Some(src), Some(dst)) = (nodes.get(u), nodes.get(v)) {
                links.push(make_link(link_seq, "FLOW", src.id, dst.id, 0.5));
                link_seq = link_seq.saturating_add(1);
            }
        }
        cursor = cursor.saturating_add(cycle_len);
    }

    // Fill remaining edges as forward DAG edges across distinct components/cycles.
    while links.len() < num_links && num_nodes > 2 {
        let a = rng.random_range(0..(num_nodes - 1));
        let b = rng.random_range((a + 1)..num_nodes);
        let same_cycle = cycle_of
            .get(a)
            .copied()
            .flatten()
            .is_some_and(|ca| cycle_of.get(b).copied().flatten() == Some(ca));
        if same_cycle {
            continue;
        }
        let weight = rng.random_range(0.05..0.25);
        if let (Some(src), Some(dst)) = (nodes.get(a), nodes.get(b)) {
            links.push(make_link(link_seq, "FLOW", src.id, dst.id, weight));
            link_seq = link_seq.saturating_add(1);
        }
    }

    SimGraph {
        nodes,
        links,
        edge_specs: BTreeMap::new(),
    }
}

fn percentile_ms(samples: &[f64], pct: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let len = sorted.len();
    if len == 0 {
        return 0.0;
    }
    let max_idx = u32::try_from(len.saturating_sub(1)).unwrap_or(0);
    let pos = (pct / 100.0) * f64::from(max_idx);
    let mut idx = 0_usize;
    while idx.saturating_add(1) < len
        && f64::from(u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX)) <= pos + 0.5
    {
        idx = idx.saturating_add(1);
    }
    sorted.get(idx).copied().unwrap_or(0.0)
}

#[test]
#[ignore = "50k-node / 200k-link performance benchmark executed via scripts/bench/sim-bench.sh"]
fn bench_sim_50k_nodes_200k_links_100_cycles() -> Result<(), AppError> {
    let iters: usize = std::env::var("AXSIM_BENCH_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let graph = generate_benchmark_graph(50_000, 200_000, 100, 2026);
    let seed_id = graph
        .nodes
        .first()
        .map(|n| n.id)
        .ok_or_else(|| AppError::bad("BENCH", "empty graph"))?;
    let actions = [ratio_action(seed_id, "m", -0.3)];

    // Warmup
    let warm = steady::solve(&graph, &actions, &SimParams::default())?;
    assert!(warm.converged);

    let mut steady_ms = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        let res = steady::solve(&graph, &actions, &SimParams::default())?;
        assert!(res.converged);
        steady_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let uc_params = SimParams {
        uncertainty: Some(UncertaintyConfig {
            samples: 200,
            coefficient_cv: 0.2,
            seed: 42,
        }),
        ..SimParams::default()
    };
    let mut uc_ms = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        let res = uncertainty::solve(&graph, &actions, &uc_params)?;
        assert!(res.converged);
        uc_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let steady_p50 = percentile_ms(&steady_ms, 50.0);
    let steady_p95 = percentile_ms(&steady_ms, 95.0);
    let uc_p50 = percentile_ms(&uc_ms, 50.0);
    let uc_p95 = percentile_ms(&uc_ms, 95.0);

    let report = serde_json::json!({
        "nodes": 50_000,
        "links": 200_000,
        "cycles": 100,
        "iterations": iters,
        "steady": { "p50_ms": steady_p50, "p95_ms": steady_p95, "limit_p95_ms": 2000.0 },
        "uncertainty_200": { "p50_ms": uc_p50, "p95_ms": uc_p95, "limit_p95_ms": 20000.0 },
    });

    if let Ok(out_path) = std::env::var("AXSIM_BENCH_OUT") {
        let body = serde_json::to_string_pretty(&report)
            .map_err(|e| AppError::Config(format!("serialize bench report failed: {e}")))?;
        std::fs::write(out_path, body)
            .map_err(|e| AppError::Config(format!("write bench report failed: {e}")))?;
    }
    let summary = serde_json::to_string_pretty(&report)
        .map_err(|e| AppError::Config(format!("serialize bench report failed: {e}")))?;
    writeln!(std::io::stdout(), "{summary}")
        .map_err(|e| AppError::Config(format!("write stdout failed: {e}")))?;

    assert!(
        steady_p95 <= 2000.0,
        "steady P95 {steady_p95:.2} ms exceeded 2000 ms"
    );
    assert!(
        uc_p95 <= 20000.0,
        "uncertainty-200 P95 {uc_p95:.2} ms exceeded 20000 ms"
    );
    Ok(())
}
