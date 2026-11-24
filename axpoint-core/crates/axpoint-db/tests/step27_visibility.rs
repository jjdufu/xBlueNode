//! Real PostgreSQL RLS integration tests for Step 27 state-driven row policies (`sec_row_policies`
//! `policy_mode` + TTL) and `SimVisibilityPort` governed preflight, sandbox cloning, and graph reads.

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_domain::enums::{MaskRule, RowPolicyMode};
use axpoint_domain::guard::{
    NodeTypePolicySpec, PolicyContext, PolicyTargetKind, SaveRowPolicyRequest, SubjectSelector,
};
use axpoint_domain::ports::{GraphStorePort as _, GuardRepository as _, SimVisibilityPort as _};
use axpoint_domain::sim::SandboxScope;
use axpoint_domain::topology::{CreateAxSandboxRequest, UpsertBlueNodeRequest};
use serde_json::json;
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

fn unique_tenant() -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("s27_{short}")
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

    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
         VALUES ($1, 'main', 'Main', TRUE)
         ON CONFLICT (tenant_id, slug) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
         VALUES ($1, 'main', 1)
         ON CONFLICT (tenant_id, subgraph_slug) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_node_types (tenant_id, subgraph_slug, name, display_name)
         VALUES ($1, 'main', 'Step27Warehouse', 'Step27Warehouse')
         ON CONFLICT (tenant_id, subgraph_slug, name) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM ax_sim_overlay_links WHERE tenant_id = $1",
        "DELETE FROM ax_sim_overlay_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_sim_base_links WHERE tenant_id = $1",
        "DELETE FROM ax_sim_base_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_sandboxes WHERE tenant_id = $1",
        "DELETE FROM sec_row_policies WHERE tenant_id = $1",
        "DELETE FROM ax_outbox WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
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

#[tokio::test]
async fn step27_row_policy_mode_and_ttl_persistence() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_id = unique_tenant();
    setup_tenant(&owner_pool, &tenant_id).await?;
    let store = PgStore::new(app_pool);

    let created_rp = store
        .create_row_policy(
            &tenant_id,
            SaveRowPolicyRequest {
                name: String::from("low_inv_ttl_policy"),
                target_kind: PolicyTargetKind::NodeType,
                target_ref: String::from("Step27Warehouse"),
                applies_to: SubjectSelector::default(),
                predicate: String::from("metrics.inventory < 100"),
                enabled: true,
                policy_mode: RowPolicyMode::StateDriven,
                ttl_timestamp_field: Some(String::from("updated_at")),
                ttl_max_age_hours: Some(72),
            },
        )
        .await?;
    assert_eq!(created_rp.policy_mode, RowPolicyMode::StateDriven);
    assert_eq!(
        created_rp.ttl_timestamp_field.as_deref(),
        Some("updated_at")
    );
    assert_eq!(created_rp.ttl_max_age_hours, Some(72));

    let listed = store.list_row_policies(&tenant_id).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed.first().map(|r| r.policy_mode),
        Some(RowPolicyMode::StateDriven)
    );

    cleanup_tenant(&owner_pool, &tenant_id).await?;
    Ok(())
}

async fn seed_two_warehouses(store: &PgStore, tenant_id: &str) -> TestResult {
    store
        .upsert_node(
            tenant_id,
            UpsertBlueNodeRequest {
                type_name: String::from("Step27Warehouse"),
                node_key: String::from("W-HIGH"),
                title: String::from("High Warehouse"),
                properties: json!({"phone": "13811112222"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                metrics: BTreeMap::from([(String::from("inventory"), 500.0)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    store
        .upsert_node(
            tenant_id,
            UpsertBlueNodeRequest {
                type_name: String::from("Step27Warehouse"),
                node_key: String::from("W-LOW"),
                title: String::from("Low Warehouse"),
                properties: json!({"phone": "13933334444"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                metrics: BTreeMap::from([(String::from("inventory"), 40.0)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn step27_governed_sim_visibility_filters_and_masks() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_id = unique_tenant();
    setup_tenant(&owner_pool, &tenant_id).await?;
    let store = PgStore::new(app_pool);
    seed_two_warehouses(&store, &tenant_id).await?;

    let mut policy = PolicyContext::default();
    policy.node_types.insert(
        String::from("Step27Warehouse"),
        NodeTypePolicySpec {
            duckdb_row_filter: None,
            pg_row_filter: Some(String::from(
                "((metrics->>'inventory')::double precision) < 100",
            )),
            property_masks: BTreeMap::from([(String::from("phone"), MaskRule::Redact)]),
        },
    );

    let scope = SandboxScope {
        type_names: vec![String::from("Step27Warehouse")],
        ..SandboxScope::default()
    };
    let preflight = store
        .preview_scope_slice_governed(&tenant_id, "main", &scope, &policy)
        .await?;
    assert_eq!(
        preflight
            .node_counts_by_type
            .get("Step27Warehouse")
            .copied(),
        Some(1)
    );

    let sb = store
        .create_sandbox_governed(
            &tenant_id,
            None,
            CreateAxSandboxRequest {
                name: String::from("s27-gov-sb"),
                scope: Some(scope),
            },
            &policy,
        )
        .await?;
    assert_eq!(sb.node_count, 1);

    let graph = store
        .get_sandbox_graph_governed(&tenant_id, sb.id, &policy)
        .await?;
    assert_eq!(graph.nodes.len(), 1);
    assert_eq!(
        graph.nodes.first().map(|n| n.node_key.as_str()),
        Some("W-LOW")
    );
    assert_eq!(
        graph
            .nodes
            .first()
            .and_then(|n| n.properties.get("phone"))
            .and_then(serde_json::Value::as_str),
        Some("[REDACTED]")
    );

    cleanup_tenant(&owner_pool, &tenant_id).await?;
    Ok(())
}
