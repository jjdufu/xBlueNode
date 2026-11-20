//! Real PostgreSQL RLS integration tests for `axpoint-db::sim_repo` (`SimRepository`).

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::{InterventionMode, OutboxTopic};
use axpoint_domain::graph::TraversalDirection;
use axpoint_domain::ports::{GraphStorePort as _, SimRepository as _};
use axpoint_domain::sim::{
    Checkpoint, CreateDecisionRequest, CreateOverlayLinkRequest, CreateOverlayNodeRequest,
    CreateScenarioRequest, DecisionChangeInput, DecisionStatus, ImpactListQuery, ImpactRow,
    PatchOverlayLinkRequest, PatchOverlayNodeRequest, SandboxScope, SandboxStatus, ScenarioStatus,
    SimMode, SimParams, SimResult,
};
use axpoint_domain::topology::{
    AxSimAction, CreateAxSandboxRequest, SimulateAxSandboxRequest, UpsertBlueNodeRequest,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
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
        .max_connections(6)
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

async fn setup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name)
         VALUES ($1, $2)
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM ax_sim_decisions WHERE tenant_id = $1",
        "DELETE FROM ax_sim_checkpoints WHERE tenant_id = $1",
        "DELETE FROM ax_sim_impacts WHERE tenant_id = $1",
        "DELETE FROM ax_sim_scenarios WHERE tenant_id = $1",
        "DELETE FROM ax_sim_overlay_links WHERE tenant_id = $1",
        "DELETE FROM ax_sim_overlay_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_sim_base_links WHERE tenant_id = $1",
        "DELETE FROM ax_sim_base_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_sandboxes WHERE tenant_id = $1",
        "DELETE FROM ax_approvals WHERE tenant_id = $1",
        "DELETE FROM ax_action_log WHERE tenant_id = $1",
        "DELETE FROM ax_outbox WHERE tenant_id = $1",
        "DELETE FROM sec_audit_logs WHERE tenant_id = $1",
        "DELETE FROM ax_jobs WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_edge_types WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
        "DELETE FROM xb_schema_versions WHERE tenant_id = $1",
        "DELETE FROM xb_schema_heads WHERE tenant_id = $1",
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

struct SeededSupplyGraph {
    factory: Uuid,
    warehouse: Uuid,
    order: Uuid,
    unrelated: Uuid,
    supplies_link: Uuid,
}

async fn seed_schema_types(owner_pool: &PgPool, tenant: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
         VALUES ($1, 'main', 'Main', TRUE)
         ON CONFLICT (tenant_id, slug) DO NOTHING",
    )
    .bind(tenant)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
         VALUES ($1, 'main', 3)
         ON CONFLICT (tenant_id, subgraph_slug) DO UPDATE SET current_version = 3",
    )
    .bind(tenant)
    .execute(owner_pool)
    .await?;

    for nt in ["Factory", "Warehouse", "Order"] {
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

    let supplies_prop = serde_json::json!({
        "rules": [{
            "src_metric": "output",
            "dst_metric": "inventory",
            "mode": "RATIO",
            "coefficient": 0.8,
            "lag_steps": 1
        }],
        "clamp": { "min": 0.0, "max": null }
    });
    sqlx::query(
        "INSERT INTO xb_edge_types (
             tenant_id, subgraph_slug, name, src_type_name, dst_type_name,
             impact_weight, propagation
         )
         VALUES ($1, 'main', 'SUPPLIES', 'Factory', 'Warehouse', 0.8, $2)
         ON CONFLICT (tenant_id, subgraph_slug, name, src_type_name, dst_type_name) DO UPDATE SET
             propagation = EXCLUDED.propagation",
    )
    .bind(tenant)
    .bind(supplies_prop)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_edge_types (
             tenant_id, subgraph_slug, name, src_type_name, dst_type_name, impact_weight
         )
         VALUES ($1, 'main', 'FULFILLS', 'Warehouse', 'Order', 0.6)
         ON CONFLICT (tenant_id, subgraph_slug, name, src_type_name, dst_type_name) DO NOTHING",
    )
    .bind(tenant)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn seed_supply_graph(
    owner_pool: &PgPool,
    store: &PgStore,
    tenant: &str,
) -> Result<SeededSupplyGraph, Box<dyn std::error::Error>> {
    seed_schema_types(owner_pool, tenant).await?;

    let order = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Order"),
                node_key: String::from("O-1"),
                title: String::from("Order 1"),
                metrics: BTreeMap::from([(String::from("fulfillment"), 1.0)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let warehouse = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("Warehouse 1"),
                metrics: BTreeMap::from([(String::from("inventory"), 500.0)]),
                link_to_node_id: Some(order.id),
                link_type: Some(String::from("FULFILLS")),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let factory = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Factory"),
                node_key: String::from("F-1"),
                title: String::from("Factory 1"),
                metrics: BTreeMap::from([(String::from("output"), 1000.0)]),
                link_to_node_id: Some(warehouse.id),
                link_type: Some(String::from("SUPPLIES")),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let unrelated = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Order"),
                node_key: String::from("O-UNRELATED"),
                title: String::from("Unrelated Order"),
                metrics: BTreeMap::from([(String::from("fulfillment"), 0.5)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let supplies_link: Uuid = sqlx::query_scalar(
        "SELECT id FROM xb_links
          WHERE tenant_id = $1 AND src_node_id = $2 AND dst_node_id = $3 AND link_type = 'SUPPLIES'",
    )
    .bind(tenant)
    .bind(factory.id)
    .bind(warehouse.id)
    .fetch_one(owner_pool)
    .await?;

    Ok(SeededSupplyGraph {
        factory: factory.id,
        warehouse: warehouse.id,
        order: order.id,
        unrelated: unrelated.id,
        supplies_link,
    })
}

fn sample_sim_result(seeded: &SeededSupplyGraph) -> SimResult {
    SimResult {
        impacts: vec![
            ImpactRow {
                node_id: seeded.factory,
                type_name: String::from("Factory"),
                title: String::from("Factory 1"),
                metric: String::from("output"),
                before_val: Some(1000.0),
                after_val: 700.0,
                delta: Some(-300.0),
                delta_ratio: Some(-0.3),
                p10: Some(680.0),
                p90: Some(720.0),
                hop_distance: 0,
                top_path: vec![seeded.factory],
                series: Vec::new(),
            },
            ImpactRow {
                node_id: seeded.warehouse,
                type_name: String::from("Warehouse"),
                title: String::from("Warehouse 1"),
                metric: String::from("inventory"),
                before_val: Some(500.0),
                after_val: 380.0,
                delta: Some(-120.0),
                delta_ratio: Some(-0.24),
                p10: None,
                p90: None,
                hop_distance: 1,
                top_path: vec![seeded.factory, seeded.warehouse],
                series: Vec::new(),
            },
            ImpactRow {
                node_id: seeded.order,
                type_name: String::from("Order"),
                title: String::from("Order 1"),
                metric: String::from("fulfillment"),
                before_val: Some(1.0),
                after_val: 0.85,
                delta: Some(-0.15),
                delta_ratio: Some(-0.15),
                p10: None,
                p90: None,
                hop_distance: 2,
                top_path: vec![seeded.factory, seeded.warehouse, seeded.order],
                series: Vec::new(),
            },
        ],
        affected_count: 3,
        kpis: Vec::new(),
        warnings: Vec::new(),
        converged: true,
        reason: None,
        iterations: 2,
        checkpoints: vec![
            Checkpoint {
                seq: 0,
                label: String::from("t=0"),
                changes: serde_json::json!({ seeded.factory.to_string(): { "output": 700.0 } }),
            },
            Checkpoint {
                seq: 1,
                label: String::from("t=1"),
                changes: serde_json::json!({ seeded.warehouse.to_string(): { "inventory": 380.0 } }),
            },
        ],
    }
}

#[tokio::test]
async fn sim_repo_scope_clone_limits_and_rls_isolation() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_sim_a");
    let tenant_b = unique_tenant("t_sim_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;

    let store = PgStore::new(app_pool);
    let seeded = seed_supply_graph(&owner_pool, &store, &tenant_a).await?;

    let over_depth = store
        .create_sandbox(
            &tenant_a,
            None,
            CreateAxSandboxRequest {
                name: String::from("bad-depth"),
                scope: Some(SandboxScope {
                    depth: 7,
                    ..SandboxScope::default()
                }),
            },
        )
        .await;
    assert!(matches!(
        over_depth,
        Err(AppError::BadRequest {
            code: "LIMIT_OUT_OF_RANGE",
            ..
        })
    ));

    let over_cap = store
        .create_sandbox(
            &tenant_a,
            None,
            CreateAxSandboxRequest {
                name: String::from("over-cap"),
                scope: Some(SandboxScope {
                    max_nodes: 2,
                    ..SandboxScope::default()
                }),
            },
        )
        .await;
    assert!(matches!(
        over_cap,
        Err(AppError::BadRequest {
            code: "LIMIT_OUT_OF_RANGE",
            ..
        })
    ));

    let sb_hop1 = store
        .create_sandbox(
            &tenant_a,
            None,
            CreateAxSandboxRequest {
                name: String::from("hop1-scope"),
                scope: Some(SandboxScope {
                    root_node_ids: vec![seeded.factory],
                    depth: 1,
                    direction: TraversalDirection::Out,
                    type_names: Vec::new(),
                    link_types: Vec::new(),
                    max_nodes: 50_000,
                }),
            },
        )
        .await?;
    assert_eq!(sb_hop1.status, SandboxStatus::Ready);
    assert_eq!(sb_hop1.node_count, 2);
    assert_eq!(sb_hop1.link_count, 1);
    assert_eq!(sb_hop1.schema_version, Some(3));
    assert!(sb_hop1.base_captured_at.is_some());

    let graph = store.load_graph(&tenant_a, sb_hop1.id).await?;
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.links.len(), 1);
    assert!(!graph.nodes.iter().any(|n| n.id == seeded.unrelated));
    let supplies_spec = graph
        .edge_specs
        .get("SUPPLIES")
        .ok_or("missing SUPPLIES edge_spec")?;
    assert_eq!(supplies_spec.rules.len(), 1);

    assert_eq!(store.list_sandboxes(&tenant_b).await?.len(), 0);
    assert!(store.get_sandbox(&tenant_b, sb_hop1.id).await?.is_none());
    assert!(matches!(
        store.load_graph(&tenant_b, sb_hop1.id).await,
        Err(AppError::NotFound("SANDBOX_NOT_FOUND"))
    ));
    assert!(!store.delete_sandbox(&tenant_b, sb_hop1.id).await?);

    assert_eq!(store.list_sandboxes(&tenant_a).await?.len(), 1);
    assert!(store.delete_sandbox(&tenant_a, sb_hop1.id).await?);
    assert!(store.get_sandbox(&tenant_a, sb_hop1.id).await?.is_none());

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

async fn verify_overlay_node_and_link_edits(
    store: &PgStore,
    tenant: &str,
    sb_id: Uuid,
    seeded: &SeededSupplyGraph,
    initial_hash: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    let patched_wh = store
        .patch_overlay_node(
            tenant,
            sb_id,
            seeded.warehouse,
            PatchOverlayNodeRequest {
                title: Some(String::from("Sandbox Warehouse")),
                metrics: Some(BTreeMap::from([(String::from("inventory"), 42.0)])),
                properties: None,
            },
        )
        .await?;
    assert!(patched_wh.is_modified);
    assert!(!patched_wh.is_new);
    assert_eq!(patched_wh.metrics.get("inventory").copied(), Some(42.0));

    let prod_wh = store
        .get_node(tenant, seeded.warehouse)
        .await?
        .ok_or("prod warehouse missing")?;
    assert_eq!(prod_wh.metrics.get("inventory").copied(), Some(500.0));
    assert_eq!(prod_wh.title, "Warehouse 1");

    let new_node = store
        .create_overlay_node(
            tenant,
            sb_id,
            CreateOverlayNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-NEW"),
                title: String::from("Virtual Backup Hub"),
                metrics: BTreeMap::from([(String::from("inventory"), 300.0)]),
                properties: serde_json::Map::new(),
            },
        )
        .await?;
    assert!(new_node.is_new);

    let new_link = store
        .create_overlay_link(
            tenant,
            sb_id,
            CreateOverlayLinkRequest {
                link_type: String::from("FULFILLS"),
                src_node_id: new_node.id,
                dst_node_id: seeded.order,
                weight: 0.9,
            },
        )
        .await?;
    assert!(new_link.is_new);

    let patched_link = store
        .patch_overlay_link(
            tenant,
            sb_id,
            seeded.supplies_link,
            PatchOverlayLinkRequest {
                link_type: None,
                weight: Some(0.95),
            },
        )
        .await?;
    assert!(patched_link.is_modified);
    assert!((patched_link.weight - 0.95).abs() < 1e-9);

    let view = store.get_sandbox_graph(tenant, sb_id).await?;
    assert_eq!(view.nodes.len(), 4);
    assert_eq!(view.links.len(), 3);
    assert_ne!(view.overlay_hash, initial_hash);

    Ok((new_node.id, new_link.id))
}

#[tokio::test]
async fn sim_repo_overlay_cow_isolation_refresh_and_quick_sim() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant = unique_tenant("t_sim_cow");
    setup_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let seeded = seed_supply_graph(&owner_pool, &store, &tenant).await?;

    let sb = store
        .create_sandbox(
            &tenant,
            None,
            CreateAxSandboxRequest {
                name: String::from("cow-sb"),
                scope: Some(SandboxScope {
                    root_node_ids: vec![seeded.factory],
                    depth: 3,
                    direction: TraversalDirection::Out,
                    ..SandboxScope::default()
                }),
            },
        )
        .await?;
    assert_eq!(sb.node_count, 3);
    assert_eq!(sb.link_count, 2);

    let initial_hash = store.compute_overlay_hash(&tenant, sb.id).await?;
    let (new_node_id, new_link_id) =
        verify_overlay_node_and_link_edits(&store, &tenant, sb.id, &seeded, &initial_hash).await?;

    let quick_res = store
        .quick_simulate_sandbox(
            &tenant,
            sb.id,
            SimulateAxSandboxRequest {
                actions: vec![AxSimAction {
                    target_node_id: seeded.factory,
                    metric_key: String::from("output"),
                    mode: None,
                    value: None,
                    delta_ratio: Some(-0.2),
                    at_step: 0,
                }],
                params: None,
            },
        )
        .await?;
    assert!(quick_res.converged);

    let bad_quick = store
        .quick_simulate_sandbox(
            &tenant,
            sb.id,
            SimulateAxSandboxRequest {
                actions: vec![AxSimAction {
                    target_node_id: seeded.factory,
                    metric_key: String::from("no_such_metric"),
                    mode: None,
                    value: None,
                    delta_ratio: Some(-0.2),
                    at_step: 0,
                }],
                params: None,
            },
        )
        .await;
    assert!(matches!(
        bad_quick,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));

    assert!(
        store
            .delete_overlay_link(&tenant, sb.id, new_link_id)
            .await?
    );
    assert!(
        store
            .delete_overlay_node(&tenant, sb.id, new_node_id)
            .await?
    );

    store.delete_node(&tenant, seeded.warehouse).await?;
    let refresh = store.refresh_sandbox_base(&tenant, sb.id).await?;
    assert!(refresh.orphaned_node_ids.contains(&seeded.warehouse));
    assert!(refresh.orphaned_link_ids.contains(&seeded.supplies_link));

    store.reset_overlay(&tenant, sb.id).await?;
    let reset_hash = store.compute_overlay_hash(&tenant, sb.id).await?;
    assert_eq!(reset_hash, initial_hash);

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}

async fn verify_scenario_impacts_and_staleness(
    store: &PgStore,
    tenant_a: &str,
    tenant_b: &str,
    sandbox_id: Uuid,
    scenario_id: Uuid,
    seeded: &SeededSupplyGraph,
) -> TestResult {
    let page1 = store
        .list_scenario_impacts(
            tenant_a,
            scenario_id,
            ImpactListQuery {
                limit: Some(2),
                ..ImpactListQuery::default()
            },
        )
        .await?;
    assert_eq!(page1.items.len(), 2);
    assert_eq!(page1.items.first().map(|r| r.node_id), Some(seeded.factory));
    assert!(page1.next_cursor.is_some());

    let page2 = store
        .list_scenario_impacts(
            tenant_a,
            scenario_id,
            ImpactListQuery {
                limit: Some(2),
                cursor: page1.next_cursor,
                ..ImpactListQuery::default()
            },
        )
        .await?;
    assert_eq!(page2.items.len(), 1);
    assert!(page2.next_cursor.is_none());

    let filtered = store
        .list_scenario_impacts(
            tenant_a,
            scenario_id,
            ImpactListQuery {
                type_name: Some(String::from("Order")),
                metric: Some(String::from("fulfillment")),
                ..ImpactListQuery::default()
            },
        )
        .await?;
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items.first().map(|r| r.hop_distance), Some(2));

    assert_eq!(
        store
            .list_all_scenario_impacts(tenant_a, scenario_id)
            .await?
            .len(),
        3
    );
    assert_eq!(
        store
            .list_scenario_checkpoints(tenant_a, scenario_id)
            .await?
            .len(),
        2
    );

    store
        .patch_overlay_node(
            tenant_a,
            sandbox_id,
            seeded.warehouse,
            PatchOverlayNodeRequest {
                metrics: Some(BTreeMap::from([(String::from("inventory"), 450.0)])),
                ..PatchOverlayNodeRequest::default()
            },
        )
        .await?;

    let scenarios_after_edit = store.list_scenarios(tenant_a, sandbox_id).await?;
    assert_eq!(scenarios_after_edit.len(), 1);
    assert_eq!(scenarios_after_edit.first().map(|s| s.is_stale), Some(true));

    assert!(store.get_scenario(tenant_b, scenario_id).await?.is_none());
    assert!(matches!(
        store.list_scenarios(tenant_b, sandbox_id).await,
        Err(AppError::NotFound("SANDBOX_NOT_FOUND"))
    ));
    Ok(())
}

#[tokio::test]
async fn sim_repo_scenarios_impacts_checkpoints_and_staleness() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_sim_sc_a");
    let tenant_b = unique_tenant("t_sim_sc_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;

    let store = PgStore::new(app_pool);
    let seeded = seed_supply_graph(&owner_pool, &store, &tenant_a).await?;
    let sb = store
        .create_sandbox(
            &tenant_a,
            None,
            CreateAxSandboxRequest {
                name: String::from("sc-sb"),
                scope: None,
            },
        )
        .await?;
    let hash_before = store.compute_overlay_hash(&tenant_a, sb.id).await?;

    let sc_req = CreateScenarioRequest {
        name: String::from("cut-30"),
        description: String::from("Factory output -30%"),
        actions: vec![AxSimAction {
            target_node_id: seeded.factory,
            metric_key: String::from("output"),
            mode: Some(InterventionMode::Ratio),
            value: Some(-0.3),
            delta_ratio: None,
            at_step: 0,
        }],
        params: SimParams {
            mode: SimMode::Stepped,
            horizon_steps: 2,
            ..SimParams::default()
        },
    };
    let sc = store
        .create_scenario_record(&tenant_a, sb.id, None, &hash_before, sc_req.clone())
        .await?;
    assert_eq!(sc.status, ScenarioStatus::Pending);
    assert!(!sc.is_stale);

    let dup = store
        .create_scenario_record(&tenant_a, sb.id, None, &hash_before, sc_req)
        .await;
    assert!(matches!(dup, Err(AppError::Conflict("NAME_TAKEN"))));

    store
        .mark_scenario_running(&tenant_a, sc.id, &hash_before)
        .await?;
    let saved = store
        .save_scenario_success(&tenant_a, sc.id, &hash_before, &sample_sim_result(&seeded))
        .await?;
    assert_eq!(saved.status, ScenarioStatus::Succeeded);
    assert!(!saved.is_stale);

    verify_scenario_impacts_and_staleness(&store, &tenant_a, &tenant_b, sb.id, sc.id, &seeded)
        .await?;

    let failed_sc = store
        .create_scenario_record(
            &tenant_a,
            sb.id,
            None,
            &hash_before,
            CreateScenarioRequest {
                name: String::from("fail-sc"),
                description: String::new(),
                actions: Vec::new(),
                params: SimParams::default(),
            },
        )
        .await?;
    store
        .save_scenario_failure(&tenant_a, failed_sc.id, "SIM_DIVERGENT")
        .await?;
    let failed_loaded = store
        .get_scenario(&tenant_a, failed_sc.id)
        .await?
        .ok_or("failed scenario missing")?;
    assert_eq!(failed_loaded.status, ScenarioStatus::Failed);
    assert_eq!(
        failed_loaded.error_message.as_deref(),
        Some("SIM_DIVERGENT")
    );
    assert!(store.delete_scenario(&tenant_a, failed_sc.id).await?);

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

async fn assert_writeback_side_effects(
    owner_pool: &PgPool,
    store: &PgStore,
    tenant_a: &str,
    factory_id: Uuid,
) -> TestResult {
    let prod_factory = store
        .get_node(tenant_a, factory_id)
        .await?
        .ok_or("prod factory missing")?;
    assert_eq!(prod_factory.metrics.get("output").copied(), Some(700.0));
    assert!(prod_factory.row_version >= 2);

    let series_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM xb_metric_series
          WHERE tenant_id = $1 AND node_id = $2 AND metric = 'output' AND source_tag = 'ACTION'",
    )
    .bind(tenant_a)
    .bind(factory_id)
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(series_cnt, 1);

    let action_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ax_action_log
          WHERE tenant_id = $1 AND source_kind = 'SIM_APPLY'
            AND action_kind = 'WRITE_NODE_METRIC' AND status = 'DONE'",
    )
    .bind(tenant_a)
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(action_cnt, 1);

    let outbox_topics: Vec<String> =
        sqlx::query_scalar("SELECT topic FROM ax_outbox WHERE tenant_id = $1 ORDER BY id ASC")
            .bind(tenant_a)
            .fetch_all(owner_pool)
            .await?;
    assert!(outbox_topics.contains(&String::from(OutboxTopic::SimApplied.as_str())));
    assert!(outbox_topics.contains(&String::from(OutboxTopic::NodeMetricsChanged.as_str())));

    let audit_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sec_audit_logs WHERE tenant_id = $1 AND action = 'SIM_APPLY'",
    )
    .bind(tenant_a)
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(audit_cnt, 1);
    Ok(())
}

async fn verify_conflict_and_rebase(
    store: &PgStore,
    tenant_a: &str,
    sc_id: Uuid,
    factory_id: Uuid,
) -> TestResult {
    let dec2 = store
        .create_decision(
            tenant_a,
            sc_id,
            None,
            CreateDecisionRequest {
                title: String::from("stale-apply"),
                changes: Some(vec![DecisionChangeInput {
                    node_id: factory_id,
                    metric: String::from("output"),
                    target_value: 650.0,
                }]),
                tolerance_ratio: 0.0,
                trigger_flow_id: None,
            },
        )
        .await?;
    let conflict = store
        .apply_or_reject_decision(tenant_a, dec2.id, None, true)
        .await?;
    assert_eq!(conflict.status, DecisionStatus::Conflict);
    assert_eq!(conflict.conflict_report.len(), 1);
    assert_eq!(
        conflict
            .conflict_report
            .first()
            .and_then(|c| c.current_value),
        Some(700.0)
    );

    let rebased = store.rebase_decision(tenant_a, dec2.id, None).await?;
    assert_eq!(rebased.status, DecisionStatus::PendingApproval);
    assert_eq!(rebased.conflict_report.len(), 0);
    assert_eq!(
        rebased.changes.first().and_then(|c| c.base_value),
        Some(700.0)
    );
    assert_ne!(rebased.approval_id, dec2.approval_id);

    let applied2 = store
        .apply_or_reject_decision(tenant_a, dec2.id, None, true)
        .await?;
    assert_eq!(applied2.status, DecisionStatus::Applied);
    let prod_after = store
        .get_node(tenant_a, factory_id)
        .await?
        .ok_or("prod factory missing after rebase")?;
    assert_eq!(prod_after.metrics.get("output").copied(), Some(650.0));
    Ok(())
}

async fn verify_decision_reject(store: &PgStore, tenant_a: &str, scenario_id: Uuid) -> TestResult {
    let dec_rej = store
        .create_decision(
            tenant_a,
            scenario_id,
            None,
            CreateDecisionRequest {
                title: String::from("reject-me"),
                changes: None,
                tolerance_ratio: 0.0,
                trigger_flow_id: None,
            },
        )
        .await?;
    assert_eq!(dec_rej.status, DecisionStatus::PendingApproval);
    assert!(dec_rej.approval_id.is_some());
    let rejected = store
        .apply_or_reject_decision(tenant_a, dec_rej.id, None, false)
        .await?;
    assert_eq!(rejected.status, DecisionStatus::Rejected);
    Ok(())
}

#[tokio::test]
async fn sim_repo_decisions_approval_conflict_rebase_and_writeback() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_sim_dec_a");
    let tenant_b = unique_tenant("t_sim_dec_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;

    let store = PgStore::new(app_pool);
    let seeded = seed_supply_graph(&owner_pool, &store, &tenant_a).await?;
    let sb = store
        .create_sandbox(
            &tenant_a,
            None,
            CreateAxSandboxRequest {
                name: String::from("dec-sb"),
                scope: None,
            },
        )
        .await?;
    let hash = store.compute_overlay_hash(&tenant_a, sb.id).await?;
    let sc = store
        .create_scenario_record(
            &tenant_a,
            sb.id,
            None,
            &hash,
            CreateScenarioRequest {
                name: String::from("cut-factory"),
                description: String::new(),
                actions: vec![AxSimAction {
                    target_node_id: seeded.factory,
                    metric_key: String::from("output"),
                    mode: None,
                    value: None,
                    delta_ratio: Some(-0.3),
                    at_step: 0,
                }],
                params: SimParams::default(),
            },
        )
        .await?;

    verify_decision_reject(&store, &tenant_a, sc.id).await?;

    let dec1 = store
        .create_decision(
            &tenant_a,
            sc.id,
            None,
            CreateDecisionRequest {
                title: String::from("apply-cut"),
                changes: None,
                tolerance_ratio: 0.0,
                trigger_flow_id: None,
            },
        )
        .await?;
    let apr1_id = dec1.approval_id.ok_or("missing approval_id")?;
    let apr_row = sqlx::query(
        "SELECT source_kind, source_ref, assignee_roles FROM ax_approvals WHERE id = $1",
    )
    .bind(apr1_id)
    .fetch_one(&owner_pool)
    .await?;
    assert_eq!(apr_row.get::<String, _>("source_kind"), "SIM_APPLY");
    assert_eq!(apr_row.get::<String, _>("source_ref"), dec1.id.to_string());
    assert_eq!(
        apr_row.get::<Vec<String>, _>("assignee_roles"),
        vec![String::from("ADMIN"), String::from("OPERATOR")]
    );

    assert!(store.get_decision(&tenant_b, dec1.id).await?.is_none());
    assert!(matches!(
        store
            .apply_or_reject_decision(&tenant_b, dec1.id, None, true)
            .await,
        Err(AppError::NotFound("DECISION_NOT_FOUND"))
    ));

    let applied = store
        .apply_or_reject_decision(&tenant_a, dec1.id, None, true)
        .await?;
    assert_eq!(applied.status, DecisionStatus::Applied);
    assert!(applied.applied_at.is_some());

    assert_writeback_side_effects(&owner_pool, &store, &tenant_a, seeded.factory).await?;
    verify_conflict_and_rebase(&store, &tenant_a, sc.id, seeded.factory).await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}
