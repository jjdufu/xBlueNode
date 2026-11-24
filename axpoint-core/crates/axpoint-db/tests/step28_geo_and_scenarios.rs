//! Step 28 PostgreSQL RLS integration tests for `geo_shape` persistence, Stage 1 + Stage 2 spatial
//! filtering, governed convex hull point collection, and `AxSim` 2.0 scenario event/deadline persistence.

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::{
    DeadlineWindowStatus, GeoPredicateOp, InterventionMode, SimEventTriggerKind, SimThresholdOp,
};
use axpoint_domain::geo_ops::{GeoShapeDto, compute_convex_hull};
use axpoint_domain::graph::{GeoPoint, NodeListQuery};
use axpoint_domain::ports::{GraphStorePort as _, SimRepository as _};
use axpoint_domain::sim::{
    CreateScenarioRequest, PatchOverlayNodeRequest, SimEventSpec, SimGoalConstraint, SimMode,
    SimParams, uncertainty,
};
use axpoint_domain::topology::{
    AxSimAction, CreateAxSandboxRequest, SimulateAxSandboxRequest, UpsertBlueNodeRequest,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let require_db = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let owner_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));

    let app_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };
    let owner_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };
    Ok(Some((app_pool, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("{prefix}_{short}")
}

async fn setup_step28_schema(owner_pool: &PgPool, tenant: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name)
         VALUES ($1, $1)
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
         VALUES ($1, 'main', 'Main', TRUE)
         ON CONFLICT (tenant_id, slug) DO NOTHING",
    )
    .bind(tenant)
    .execute(owner_pool)
    .await?;

    for nt in ["Step28Plant", "Step28Hub"] {
        sqlx::query(
            "INSERT INTO xb_node_types (tenant_id, subgraph_slug, name, display_name)
             VALUES ($1, 'main', $2, $2)
             ON CONFLICT (tenant_id, subgraph_slug, name) DO NOTHING",
        )
        .bind(tenant)
        .bind(nt)
        .execute(owner_pool)
        .await?;
    }

    let prop = serde_json::json!({
        "rules": [{
            "from_metric": "output",
            "to_metric": "inventory",
            "coefficient": 1.0,
            "lag_steps": 3
        }]
    });
    sqlx::query(
        "INSERT INTO xb_edge_types (
             tenant_id, subgraph_slug, name, src_type_name, dst_type_name,
             impact_weight, propagation
         )
         VALUES ($1, 'main', 'STEP28_SUPPLIES', 'Step28Plant', 'Step28Hub', 1.0, $2)
         ON CONFLICT (tenant_id, subgraph_slug, name, src_type_name, dst_type_name)
         DO UPDATE SET propagation = EXCLUDED.propagation",
    )
    .bind(tenant)
    .bind(prop)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM ax_sim_checkpoints WHERE tenant_id = $1",
        "DELETE FROM ax_sim_impacts WHERE tenant_id = $1",
        "DELETE FROM ax_sim_scenarios WHERE tenant_id = $1",
        "DELETE FROM ax_sim_overlay_links WHERE tenant_id = $1",
        "DELETE FROM ax_sim_overlay_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_sim_base_links WHERE tenant_id = $1",
        "DELETE FROM ax_sim_base_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_sandboxes WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_edge_types WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
        "DELETE FROM xb_subgraphs WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

async fn seed_step28_hubs(
    store: &PgStore,
    tenant: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    let sh_geofence = GeoShapeDto::Polygon(vec![vec![
        [121.30, 31.10],
        [121.65, 31.10],
        [121.65, 31.35],
        [121.30, 31.35],
        [121.30, 31.10],
    ]]);

    let hubs = [
        ("H-28-SH", "Shanghai Hub", 121.47, 31.23, Some(sh_geofence)),
        ("H-28-SUZ", "Suzhou Hub", 120.62, 31.30, None),
        ("H-28-HZ", "Hangzhou Hub", 120.15, 30.28, None),
        ("H-28-SZ", "Shenzhen Hub", 114.06, 22.54, None),
    ];
    let mut sh_id = Uuid::nil();
    for (key, title, lng, lat, shape) in hubs {
        let mut metrics = BTreeMap::new();
        metrics.insert(String::from("inventory"), 300.0);
        let node = store
            .upsert_node(
                tenant,
                UpsertBlueNodeRequest {
                    type_name: String::from("Step28Hub"),
                    node_key: String::from(key),
                    title: String::from(title),
                    metrics,
                    geo: Some(GeoPoint { lng, lat }),
                    geo_shape: shape,
                    ..UpsertBlueNodeRequest::default()
                },
            )
            .await?;
        if key == "H-28-SH" {
            sh_id = node.id;
        }
    }

    let mut plant_metrics = BTreeMap::new();
    plant_metrics.insert(String::from("output"), 300.0);
    let plant = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Step28Plant"),
                node_key: String::from("P-28-KS"),
                title: String::from("Kunshan Plant"),
                metrics: plant_metrics,
                geo: Some(GeoPoint {
                    lng: 120.98,
                    lat: 31.38,
                }),
                link_to_node_id: Some(sh_id),
                link_type: Some(String::from("STEP28_SUPPLIES")),
                link_weight: Some(1.0),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    Ok((plant.id, sh_id))
}

#[tokio::test]
async fn step28_spatial_queries_and_convex_hull_db() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant = unique_tenant("t_step28_geo");
    setup_step28_schema(&owner_pool, &tenant).await?;
    let (_plant_id, sh_id) = seed_step28_hubs(&store, &tenant).await?;

    let loaded_sh = store
        .get_node(&tenant, sh_id)
        .await?
        .ok_or("missing Shanghai Hub")?;
    assert!(matches!(loaded_sh.geo_shape, Some(GeoShapeDto::Polygon(_))));

    let poly_json = r#"{"type":"Polygon","coordinates":[[[119.5,29.8],[122.2,29.8],[122.2,32.0],[119.5,32.0],[119.5,29.8]]]}"#;
    let listed = store
        .list_nodes_with_row_policies(
            &tenant,
            NodeListQuery {
                type_name: Some(String::from("Step28Hub")),
                geo_op: Some(GeoPredicateOp::GeoWithinPolygon),
                polygon: Some(String::from(poly_json)),
                ..NodeListQuery::default()
            },
            None,
        )
        .await?;
    let mut keys: Vec<String> = listed.items.into_iter().map(|n| n.node_key).collect();
    keys.sort();
    assert_eq!(keys, vec!["H-28-HZ", "H-28-SH", "H-28-SUZ"]);

    let bad_res = store
        .list_nodes_with_row_policies(
            &tenant,
            NodeListQuery {
                type_name: Some(String::from("Step28Hub")),
                geo_op: Some(GeoPredicateOp::GeoWithinPolygon),
                polygon: Some(String::from(
                    r#"{"type":"Polygon","coordinates":[[[120.0,30.0],[121.0,31.0]]]}"#,
                )),
                ..NodeListQuery::default()
            },
            None,
        )
        .await;
    assert!(matches!(
        bad_res,
        Err(AppError::BadRequest {
            code: "INVALID_GEOMETRY",
            ..
        })
    ));

    let pts = store
        .collect_governed_geo_points(&tenant, "main", Some("Step28Hub"), &[], None)
        .await?;
    let hull = compute_convex_hull(&pts)?;
    assert!(hull.point_count >= 4);
    assert!(hull.area_sq_deg > 0.0);
    assert!(matches!(hull.hull, Some(GeoShapeDto::Polygon(_))));

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}

fn build_step28_events_and_goals(
    plant_id: Uuid,
    sh_id: Uuid,
) -> (Vec<SimEventSpec>, Vec<SimGoalConstraint>, SimParams) {
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
    let params = SimParams {
        mode: SimMode::Stepped,
        horizon_steps: 12,
        ..SimParams::default()
    };
    (events, goals, params)
}

#[tokio::test]
async fn step28_scenario_events_deadline_and_sandbox_geo_persistence() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant = unique_tenant("t_step28_sim");
    setup_step28_schema(&owner_pool, &tenant).await?;
    let (plant_id, sh_id) = seed_step28_hubs(&store, &tenant).await?;

    let sb = store
        .create_sandbox(
            &tenant,
            None,
            CreateAxSandboxRequest {
                name: String::from("Step28 Sandbox"),
                scope: None,
            },
        )
        .await?;

    let graph_before = store.load_graph(&tenant, sb.id).await?;
    let sh_sim = graph_before
        .nodes
        .iter()
        .find(|n| n.id == sh_id)
        .ok_or("missing sh_id in SimGraph")?;
    assert!(sh_sim.geo.is_some() && sh_sim.geo_shape.is_some());

    let patched = store
        .patch_overlay_node(
            &tenant,
            sb.id,
            sh_id,
            PatchOverlayNodeRequest {
                title: None,
                metrics: None,
                properties: None,
                geo: Some(GeoPoint {
                    lng: 121.48,
                    lat: 31.24,
                }),
                geo_shape: None,
            },
        )
        .await?;
    assert_eq!(
        patched.geo,
        Some(GeoPoint {
            lng: 121.48,
            lat: 31.24
        })
    );

    let (events, goals, params) = build_step28_events_and_goals(plant_id, sh_id);
    let quick = store
        .quick_simulate_sandbox(
            &tenant,
            sb.id,
            SimulateAxSandboxRequest {
                actions: Vec::new(),
                scheduled_events: events.clone(),
                goal_constraints: goals.clone(),
                params: Some(params.clone()),
            },
        )
        .await?;
    assert_eq!(quick.fired_events.len(), 2);
    assert_eq!(
        quick
            .deadline_report
            .as_ref()
            .and_then(|r| r.earliest_deadline_step),
        Some(3)
    );

    let hash = store.compute_overlay_hash(&tenant, sb.id).await?;
    let created = store
        .create_scenario_record(
            &tenant,
            sb.id,
            None,
            &hash,
            CreateScenarioRequest {
                name: String::from("step28-persist"),
                description: String::new(),
                actions: Vec::new(),
                scheduled_events: events.clone(),
                goal_constraints: goals.clone(),
                params: params.clone(),
            },
        )
        .await?;
    let sim_graph = store.load_graph(&tenant, sb.id).await?;
    let sim_res = uncertainty::solve_with_events(&sim_graph, &[], &params, &events, &goals)?;
    let saved = store
        .save_scenario_success(&tenant, created.id, &hash, &sim_res)
        .await?;
    assert_eq!(saved.scheduled_events.len(), 2);
    assert_eq!(saved.goal_constraints.len(), 1);
    assert_eq!(saved.fired_events.len(), 2);
    let report = saved.deadline_report.ok_or("missing deadline_report")?;
    assert_eq!(report.earliest_breach_step, Some(7));
    assert_eq!(report.earliest_deadline_step, Some(3));
    assert_eq!(report.status, DeadlineWindowStatus::ActionableWindow);

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}
