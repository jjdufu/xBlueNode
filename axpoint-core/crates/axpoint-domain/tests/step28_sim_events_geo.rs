//! Step 28 domain unit tests for `AxSim` 2.0 event queue, `deadline_step` reverse-lag solver,
//! and pure-Rust `geo` spatial geometry kernel (`06-STEP28`).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::enums::{
    DeadlineWindowStatus, GeoPredicateOp, InterventionMode, SimEventTriggerKind, SimThresholdOp,
};
use axpoint_domain::geo_ops::{
    GeoShapeDto, SpatialFilterSpec, compute_convex_hull, haversine_distance_meters,
    matches_spatial_filter, shape_bbox, spatial_filter_bbox, validate_geo_point,
    validate_geo_shape,
};
use axpoint_domain::graph::GeoPoint;
use axpoint_domain::sim::graph::{PropagationSpec, SimGraph, SimLink, SimNode};
use axpoint_domain::sim::{
    SimEventSpec, SimGoalConstraint, SimMode, SimParams, stepped, uncertainty,
};
use axpoint_domain::topology::AxSimAction;
use uuid::Uuid;

fn det_id(idx: u128) -> Uuid {
    Uuid::from_u128(idx.saturating_add(100))
}

fn make_geo_node(
    idx: u128,
    type_name: &str,
    title: &str,
    metrics: &[(&str, f64)],
    geo: Option<GeoPoint>,
) -> SimNode {
    let mut map = BTreeMap::new();
    for &(k, v) in metrics {
        map.insert(String::from(k), v);
    }
    SimNode {
        id: det_id(idx),
        type_name: String::from(type_name),
        node_key: format!("N-{idx}"),
        title: String::from(title),
        metrics: map,
        properties: serde_json::Map::new(),
        geo,
        geo_shape: None,
        row_version: 1,
    }
}

fn expect_invalid_geometry<T: std::fmt::Debug>(res: Result<T, AppError>) -> Result<(), AppError> {
    match res {
        Err(AppError::BadRequest {
            code: "INVALID_GEOMETRY",
            ..
        }) => Ok(()),
        other => Err(AppError::bad(
            "TEST_ASSERTION",
            format!("expected INVALID_GEOMETRY, got {other:?}"),
        )),
    }
}

fn build_gate_events_and_goals(
    plant_id: Uuid,
    sh_id: Uuid,
) -> (Vec<SimEventSpec>, Vec<SimGoalConstraint>) {
    let events = vec![
        SimEventSpec {
            id: String::from("ev_cut_step4"),
            name: String::from("Step-4 Supply Cut"),
            trigger_kind: SimEventTriggerKind::AtStep,
            at_step: Some(4),
            watch_node_id: None,
            watch_metric: None,
            op: None,
            threshold: None,
            cooldown_steps: 1,
            max_fires: 1,
            actions: vec![AxSimAction {
                target_node_id: plant_id,
                metric_key: String::from("output"),
                mode: Some(InterventionMode::Add),
                value: Some(-150.0),
                delta_ratio: None,
                at_step: 0,
            }],
        },
        SimEventSpec {
            id: String::from("ev_restock_under_200"),
            name: String::from("Emergency Restock < 200"),
            trigger_kind: SimEventTriggerKind::MetricThreshold,
            at_step: None,
            watch_node_id: Some(sh_id),
            watch_metric: Some(String::from("inventory")),
            op: Some(SimThresholdOp::Lt),
            threshold: Some(200.0),
            cooldown_steps: 3,
            max_fires: 1,
            actions: vec![AxSimAction {
                target_node_id: sh_id,
                metric_key: String::from("inventory"),
                mode: Some(InterventionMode::Add),
                value: Some(80.0),
                delta_ratio: None,
                at_step: 0,
            }],
        },
    ];
    let goals = vec![SimGoalConstraint {
        id: String::from("goal_sh_inv_200"),
        name: String::from("Shanghai Inventory >= 200"),
        target_node_id: Some(sh_id),
        target_type_name: None,
        metric_key: String::from("inventory"),
        op: SimThresholdOp::Gte,
        threshold: 200.0,
        spatial_filter: None,
        controllable_node_ids: vec![plant_id],
    }];
    (events, goals)
}

#[test]
fn step28_gate_exact_trace_at_step_threshold_and_deadline() -> Result<(), AppError> {
    let plant = make_geo_node(
        1,
        "Step28Plant",
        "Kunshan Plant",
        &[("output", 300.0)],
        None,
    );
    let hub_sh = make_geo_node(
        2,
        "Step28Hub",
        "Shanghai Hub",
        &[("inventory", 300.0)],
        Some(GeoPoint {
            lng: 121.47,
            lat: 31.23,
        }),
    );

    let spec: PropagationSpec = serde_json::from_value(serde_json::json!({
        "rules": [{
            "from_metric": "output",
            "to_metric": "inventory",
            "coefficient": 1.0,
            "lag_steps": 3
        }]
    }))
    .map_err(|err| AppError::bad("TEST_PARSE", err.to_string()))?;

    let mut edge_specs = BTreeMap::new();
    edge_specs.insert(String::from("STEP28_SUPPLIES"), spec);

    let graph = SimGraph {
        nodes: vec![plant.clone(), hub_sh.clone()],
        links: vec![SimLink {
            id: det_id(10),
            link_type: String::from("STEP28_SUPPLIES"),
            src_node_id: plant.id,
            dst_node_id: hub_sh.id,
            weight: 1.0,
        }],
        edge_specs,
    };

    let (events, goals) = build_gate_events_and_goals(plant.id, hub_sh.id);
    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 12,
        ..SimParams::default()
    };

    let res = uncertainty::solve_with_events(&graph, &[], &params, &events, &goals)?;
    assert_eq!(res.fired_events.len(), 2);

    let ev_cut = res
        .fired_events
        .iter()
        .find(|e| e.event_id == "ev_cut_step4")
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing ev_cut_step4"))?;
    assert_eq!(ev_cut.step, 4);
    assert_eq!(ev_cut.trigger_kind, SimEventTriggerKind::AtStep);

    let ev_restock = res
        .fired_events
        .iter()
        .find(|e| e.event_id == "ev_restock_under_200")
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing ev_restock_under_200"))?;
    assert_eq!(ev_restock.step, 8);
    assert_eq!(
        ev_restock.trigger_kind,
        SimEventTriggerKind::MetricThreshold
    );
    assert_eq!(ev_restock.trigger_value, Some(150.0));

    let report = res
        .deadline_report
        .as_ref()
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing deadline_report"))?;
    assert_eq!(report.earliest_breach_step, Some(7));
    assert_eq!(report.earliest_deadline_step, Some(3));
    assert_eq!(report.status, DeadlineWindowStatus::ActionableWindow);

    let sh_impact = res
        .impacts
        .iter()
        .find(|r| r.node_id == hub_sh.id && r.metric == "inventory")
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing hub_sh inventory impact"))?;
    assert!((sh_impact.after_val - 230.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn step28_metric_threshold_cooldown_and_max_fires() -> Result<(), AppError> {
    let tank = make_geo_node(11, "Tank", "Buffer Tank", &[("level", 100.0)], None);
    let graph = SimGraph {
        nodes: vec![tank.clone()],
        links: Vec::new(),
        edge_specs: BTreeMap::new(),
    };

    let events = vec![
        SimEventSpec {
            id: String::from("ev_drop"),
            name: String::from("Step-1 Drain"),
            trigger_kind: SimEventTriggerKind::AtStep,
            at_step: Some(1),
            watch_node_id: None,
            watch_metric: None,
            op: None,
            threshold: None,
            cooldown_steps: 1,
            max_fires: 1,
            actions: vec![AxSimAction {
                target_node_id: tank.id,
                metric_key: String::from("level"),
                mode: Some(InterventionMode::Add),
                value: Some(-60.0),
                delta_ratio: None,
                at_step: 0,
            }],
        },
        SimEventSpec {
            id: String::from("ev_pump"),
            name: String::from("Top-up +10 when < 80"),
            trigger_kind: SimEventTriggerKind::MetricThreshold,
            at_step: None,
            watch_node_id: Some(tank.id),
            watch_metric: Some(String::from("level")),
            op: Some(SimThresholdOp::Lt),
            threshold: Some(80.0),
            cooldown_steps: 3,
            max_fires: 2,
            actions: vec![AxSimAction {
                target_node_id: tank.id,
                metric_key: String::from("level"),
                mode: Some(InterventionMode::Add),
                value: Some(10.0),
                delta_ratio: None,
                at_step: 0,
            }],
        },
    ];

    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 10,
        ..SimParams::default()
    };
    let res = stepped::solve_with_events(&graph, &[], &params, &events, &[])?;
    let pump_steps: Vec<u32> = res
        .fired_events
        .iter()
        .filter(|e| e.event_id == "ev_pump")
        .map(|e| e.step)
        .collect();
    assert_eq!(pump_steps, vec![2, 5]);
    Ok(())
}

fn build_spatial_lag_graph() -> Result<(SimGraph, Uuid, Uuid), AppError> {
    let plant = make_geo_node(21, "Plant", "Plant", &[("output", 100.0)], None);
    let hub_in = make_geo_node(
        22,
        "Hub",
        "Inside Hub",
        &[("inventory", 180.0)],
        Some(GeoPoint {
            lng: 121.47,
            lat: 31.23,
        }),
    );
    let hub_out = make_geo_node(
        23,
        "Hub",
        "Outside Hub",
        &[("inventory", 50.0)],
        Some(GeoPoint {
            lng: 114.06,
            lat: 22.54,
        }),
    );

    let spec: PropagationSpec = serde_json::from_value(serde_json::json!({
        "rules": [{
            "src_metric": "output",
            "dst_metric": "inventory",
            "mode": "ADD",
            "coefficient": 1.0,
            "lag_steps": 4
        }]
    }))
    .map_err(|err| AppError::bad("TEST_PARSE", err.to_string()))?;
    let mut edge_specs = BTreeMap::new();
    edge_specs.insert(String::from("FEEDS"), spec);

    let graph = SimGraph {
        nodes: vec![plant.clone(), hub_in.clone(), hub_out],
        links: vec![SimLink {
            id: det_id(29),
            link_type: String::from("FEEDS"),
            src_node_id: plant.id,
            dst_node_id: hub_in.id,
            weight: 1.0,
        }],
        edge_specs,
    };
    Ok((graph, plant.id, hub_in.id))
}

#[test]
fn step28_deadline_immediate_breach_and_spatial_goal() -> Result<(), AppError> {
    let (graph, plant_id, hub_in_id) = build_spatial_lag_graph()?;
    let yrd_poly = GeoShapeDto::Polygon(vec![vec![
        [119.5, 29.8],
        [122.2, 29.8],
        [122.2, 32.0],
        [119.5, 32.0],
        [119.5, 29.8],
    ]]);

    let events = vec![SimEventSpec {
        id: String::from("ev_early_dip"),
        name: String::from("Step-2 Dip"),
        trigger_kind: SimEventTriggerKind::AtStep,
        at_step: Some(2),
        watch_node_id: None,
        watch_metric: None,
        op: None,
        threshold: None,
        cooldown_steps: 1,
        max_fires: 1,
        actions: vec![AxSimAction {
            target_node_id: hub_in_id,
            metric_key: String::from("inventory"),
            mode: Some(InterventionMode::Add),
            value: Some(-40.0),
            delta_ratio: None,
            at_step: 0,
        }],
    }];

    let goals = vec![SimGoalConstraint {
        id: String::from("goal_yrd_150"),
        name: String::from("YRD Hub >= 150"),
        target_node_id: None,
        target_type_name: Some(String::from("Hub")),
        metric_key: String::from("inventory"),
        op: SimThresholdOp::Gte,
        threshold: 150.0,
        spatial_filter: Some(SpatialFilterSpec {
            op: GeoPredicateOp::GeoWithinPolygon,
            bbox: None,
            polygon: Some(yrd_poly),
            center: None,
            radius_meters: None,
        }),
        controllable_node_ids: vec![plant_id],
    }];

    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 8,
        ..SimParams::default()
    };
    let res = stepped::solve_with_events(&graph, &[], &params, &events, &goals)?;
    let report = res
        .deadline_report
        .as_ref()
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing deadline_report"))?;
    assert_eq!(report.earliest_breach_step, Some(2));
    assert_eq!(report.earliest_deadline_step, Some(-3));
    assert_eq!(
        report.status,
        DeadlineWindowStatus::ImmediateBreachUnavoidableByLag
    );
    Ok(())
}

#[test]
fn step28_geo_polygon_within_hole_and_boundary() -> Result<(), AppError> {
    let poly_with_hole = GeoShapeDto::Polygon(vec![
        vec![
            [120.0, 30.0],
            [122.0, 30.0],
            [122.0, 32.0],
            [120.0, 32.0],
            [120.0, 30.0],
        ],
        vec![
            [120.8, 30.8],
            [121.2, 30.8],
            [121.2, 31.2],
            [120.8, 31.2],
            [120.8, 30.8],
        ],
    ]);
    let poly_filter = SpatialFilterSpec {
        op: GeoPredicateOp::GeoWithinPolygon,
        bbox: None,
        polygon: Some(poly_with_hole.clone()),
        center: None,
        radius_meters: None,
    };

    assert!(matches_spatial_filter(
        Some(GeoPoint {
            lng: 120.4,
            lat: 30.4
        }),
        None,
        &poly_filter,
    )?);
    assert!(matches_spatial_filter(
        Some(GeoPoint {
            lng: 120.0,
            lat: 30.0
        }),
        None,
        &poly_filter,
    )?);
    assert!(!matches_spatial_filter(
        Some(GeoPoint {
            lng: 121.0,
            lat: 31.0
        }),
        None,
        &poly_filter,
    )?);
    assert!(!matches_spatial_filter(
        Some(GeoPoint {
            lng: 114.06,
            lat: 22.54
        }),
        None,
        &poly_filter,
    )?);

    let bbox = shape_bbox(&poly_with_hole)?;
    assert_eq!(bbox, [120.0, 30.0, 122.0, 32.0]);
    Ok(())
}

#[test]
fn step28_geo_dwithin_and_convex_hull() -> Result<(), AppError> {
    let sh = GeoPoint {
        lng: 121.47,
        lat: 31.23,
    };
    let suz = GeoPoint {
        lng: 120.62,
        lat: 31.30,
    };
    let dist_m = haversine_distance_meters(sh, suz)?;
    assert!(dist_m > 75_000.0 && dist_m < 90_000.0);

    let dwithin_filter = SpatialFilterSpec {
        op: GeoPredicateOp::GeoDwithinMeters,
        bbox: None,
        polygon: None,
        center: Some(sh),
        radius_meters: Some(100_000.0),
    };
    let pre_bbox = spatial_filter_bbox(&dwithin_filter)?;
    assert!(suz.lng >= pre_bbox[0] && suz.lng <= pre_bbox[2]);
    assert!(matches_spatial_filter(Some(suz), None, &dwithin_filter)?);
    assert!(!matches_spatial_filter(
        Some(GeoPoint {
            lng: 114.06,
            lat: 22.54
        }),
        None,
        &dwithin_filter,
    )?);

    let hull_resp = compute_convex_hull(&[
        sh,
        suz,
        GeoPoint {
            lng: 120.15,
            lat: 30.28,
        },
        GeoPoint {
            lng: 114.06,
            lat: 22.54,
        },
    ])?;
    assert_eq!(hull_resp.point_count, 4);
    assert!(hull_resp.area_sq_deg > 0.0);
    assert!(matches!(hull_resp.hull, Some(GeoShapeDto::Polygon(_))));

    let line_hull = compute_convex_hull(&[
        GeoPoint {
            lng: 120.0,
            lat: 30.0,
        },
        GeoPoint {
            lng: 121.0,
            lat: 31.0,
        },
        GeoPoint {
            lng: 122.0,
            lat: 32.0,
        },
    ])?;
    assert!(matches!(line_hull.hull, Some(GeoShapeDto::LineString(_))));
    let pt_hull = compute_convex_hull(&[sh])?;
    assert!(matches!(pt_hull.hull, Some(GeoShapeDto::Point(_))));
    let empty_hull = compute_convex_hull(&[])?;
    assert!(empty_hull.hull.is_none());

    Ok(())
}

#[test]
fn step28_geo_validation_rejects_invalid_geometries() -> Result<(), AppError> {
    expect_invalid_geometry(validate_geo_point(GeoPoint {
        lng: f64::NAN,
        lat: 30.0,
    }))?;
    expect_invalid_geometry(validate_geo_point(GeoPoint {
        lng: 120.0,
        lat: f64::INFINITY,
    }))?;
    expect_invalid_geometry(validate_geo_point(GeoPoint {
        lng: 181.0,
        lat: 30.0,
    }))?;
    expect_invalid_geometry(validate_geo_point(GeoPoint {
        lng: 120.0,
        lat: -91.0,
    }))?;

    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::LineString(vec![[
        120.0, 30.0,
    ]])))?;

    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::Polygon(vec![vec![
        [120.0, 30.0],
        [121.0, 30.0],
        [120.0, 30.0],
    ]])))?;

    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::Polygon(vec![vec![
        [120.0, 30.0],
        [121.0, 30.0],
        [121.0, 31.0],
        [120.0, 31.0],
    ]])))?;

    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::Polygon(vec![vec![
        [120.0, 30.0],
        [121.0, 31.0],
        [122.0, 32.0],
        [120.0, 30.0],
    ]])))?;

    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::Polygon(vec![vec![
        [120.0, 30.0],
        [122.0, 32.0],
        [122.0, 30.0],
        [120.0, 32.0],
        [120.0, 30.0],
    ]])))?;

    Ok(())
}

#[test]
fn step28_zero_radius_and_zero_net_area_polygon_rejected() -> Result<(), AppError> {
    let center = GeoPoint {
        lng: 121.47,
        lat: 31.23,
    };
    for bad_radius in [0.0, -10.0, f64::NAN] {
        let spec = SpatialFilterSpec {
            op: GeoPredicateOp::GeoDwithinMeters,
            bbox: None,
            polygon: None,
            center: Some(center),
            radius_meters: Some(bad_radius),
        };
        expect_invalid_geometry(spatial_filter_bbox(&spec))?;
        expect_invalid_geometry(matches_spatial_filter(Some(center), None, &spec))?;
    }

    // Polygon([ring, ring]) with identical exterior & interior hole -> 0 net area (same & opposite winding)
    let outer_ring = vec![
        [120.0, 30.0],
        [122.0, 30.0],
        [122.0, 32.0],
        [120.0, 32.0],
        [120.0, 30.0],
    ];
    let mut reversed_ring = outer_ring.clone();
    reversed_ring.reverse();
    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::Polygon(vec![
        outer_ring.clone(),
        outer_ring.clone(),
    ])))?;
    expect_invalid_geometry(validate_geo_shape(&GeoShapeDto::Polygon(vec![
        outer_ring,
        reversed_ring,
    ])))?;

    Ok(())
}

#[test]
fn step28_manual_action_and_at_step_event_compose_on_same_channel() -> Result<(), AppError> {
    let plant = make_geo_node(
        41,
        "Step28Plant",
        "Kunshan Plant",
        &[("output", 300.0)],
        None,
    );
    let graph = SimGraph {
        nodes: vec![plant.clone()],
        links: Vec::new(),
        edge_specs: BTreeMap::new(),
    };
    let manual_actions = vec![AxSimAction {
        target_node_id: plant.id,
        metric_key: String::from("output"),
        mode: Some(InterventionMode::Add),
        value: Some(-100.0),
        delta_ratio: None,
        at_step: 0,
    }];
    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 6,
        ..SimParams::default()
    };
    let recover_events = vec![SimEventSpec {
        id: String::from("ev_recover_step4"),
        name: String::from("Step-4 Recovery"),
        trigger_kind: SimEventTriggerKind::AtStep,
        at_step: Some(4),
        watch_node_id: None,
        watch_metric: None,
        op: None,
        threshold: None,
        cooldown_steps: 1,
        max_fires: 1,
        actions: vec![AxSimAction {
            target_node_id: plant.id,
            metric_key: String::from("output"),
            mode: Some(InterventionMode::Add),
            value: Some(80.0),
            delta_ratio: None,
            at_step: 0,
        }],
    }];

    let res = stepped::solve_with_events(&graph, &manual_actions, &params, &recover_events, &[])?;
    let imp = res
        .impacts
        .iter()
        .find(|r| r.node_id == plant.id && r.metric == "output")
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing output impact"))?;
    assert!((imp.after_val - 280.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn step28_manual_action_threshold_and_at_step_compose_chronologically() -> Result<(), AppError> {
    let plant = make_geo_node(
        42,
        "Step28Plant",
        "Kunshan Plant",
        &[("output", 300.0)],
        None,
    );
    let graph = SimGraph {
        nodes: vec![plant.clone()],
        links: Vec::new(),
        edge_specs: BTreeMap::new(),
    };
    let manual_actions = vec![AxSimAction {
        target_node_id: plant.id,
        metric_key: String::from("output"),
        mode: Some(InterventionMode::Add),
        value: Some(-100.0),
        delta_ratio: None,
        at_step: 0,
    }];
    let events = vec![
        SimEventSpec {
            id: String::from("ev_thresh_boost"),
            name: String::from("Low Output Boost"),
            trigger_kind: SimEventTriggerKind::MetricThreshold,
            at_step: None,
            watch_node_id: Some(plant.id),
            watch_metric: Some(String::from("output")),
            op: Some(SimThresholdOp::Lt),
            threshold: Some(220.0),
            cooldown_steps: 2,
            max_fires: 1,
            actions: vec![AxSimAction {
                target_node_id: plant.id,
                metric_key: String::from("output"),
                mode: Some(InterventionMode::Add),
                value: Some(30.0),
                delta_ratio: None,
                at_step: 0,
            }],
        },
        SimEventSpec {
            id: String::from("ev_recover_step4"),
            name: String::from("Step-4 Recovery"),
            trigger_kind: SimEventTriggerKind::AtStep,
            at_step: Some(4),
            watch_node_id: None,
            watch_metric: None,
            op: None,
            threshold: None,
            cooldown_steps: 1,
            max_fires: 1,
            actions: vec![AxSimAction {
                target_node_id: plant.id,
                metric_key: String::from("output"),
                mode: Some(InterventionMode::Add),
                value: Some(80.0),
                delta_ratio: None,
                at_step: 0,
            }],
        },
    ];
    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 6,
        ..SimParams::default()
    };

    let res = stepped::solve_with_events(&graph, &manual_actions, &params, &events, &[])?;
    assert_eq!(res.fired_events.len(), 2);
    let imp = res
        .impacts
        .iter()
        .find(|r| r.node_id == plant.id && r.metric == "output")
        .ok_or_else(|| AppError::bad("TEST_MISSING", "missing combined output impact"))?;
    assert!((imp.after_val - 310.0).abs() < 1e-9);
    Ok(())
}
