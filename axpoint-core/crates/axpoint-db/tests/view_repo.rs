//! Real PostgreSQL + Row-Level Security (`RLS`) integration tests for `axpoint-db::view_repo` (`ViewRepository`).

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::{AssetKind, DataType, PiiCategory, WidgetKind};
use axpoint_domain::flow::{ActionKind, ActionLogStatus, ActionSourceKind};
use axpoint_domain::graph::{GeoPoint, UpsertLinkRequest};
use axpoint_domain::ports::{GraphStorePort as _, ViewRepository as _};
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::studio::{NodeTypeSpec, PropertyKind, PropertySpec, SchemaDoc};
use axpoint_domain::topology::UpsertBlueNodeRequest;
use axpoint_domain::view::{
    CanvasConfig, CreateAssetParams, CreateDashboardRequest, CreateWidgetRecordParams,
    DashboardStatus, DashboardTheme, FilterDef, FilterKind, LayoutUpdateItem,
    PutFloorPlacementItem, PutScene3dBindingItem, RefreshMode, UpdateDashboardRequest,
    UpdateWidgetRecordParams, ViewWriteNodeParams, WidgetDataSource, WidgetDeps, WidgetPosition,
    WidgetRecord, WriteNodeTargetKind,
};
use serde_json::json;
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
        "DELETE FROM xv_share_links WHERE tenant_id = $1",
        "DELETE FROM xv_export_jobs WHERE tenant_id = $1",
        "DELETE FROM xv_export_templates WHERE tenant_id = $1",
        "DELETE FROM xv_scene3d_bindings WHERE tenant_id = $1",
        "DELETE FROM xv_floor_placements WHERE tenant_id = $1",
        "DELETE FROM xv_widgets WHERE tenant_id = $1",
        "DELETE FROM xv_dashboards WHERE tenant_id = $1",
        "DELETE FROM xv_assets WHERE tenant_id = $1",
        "DELETE FROM ax_alerts WHERE tenant_id = $1",
        "DELETE FROM ax_rules WHERE tenant_id = $1",
        "DELETE FROM ax_action_log WHERE tenant_id = $1",
        "DELETE FROM ax_outbox WHERE tenant_id = $1",
        "DELETE FROM ax_jobs WHERE tenant_id = $1",
        "DELETE FROM ax_retention_policies WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
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

fn warehouse_schema_doc() -> SchemaDoc {
    SchemaDoc {
        node_types: vec![NodeTypeSpec {
            id: None,
            name: String::from("Warehouse"),
            display_name: String::from("仓库"),
            description: String::new(),
            icon: String::from("box"),
            color: String::from("#2563eb"),
            title_template: String::from("{node_key}"),
            properties: vec![
                PropertySpec {
                    name: String::from("manager"),
                    display_name: String::from("负责人"),
                    description: String::new(),
                    unit: None,
                    data_type: DataType::String,
                    kind: PropertyKind::Attribute,
                    required: false,
                    pii_category: PiiCategory::None,
                    allowed_values: None,
                    maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                },
                PropertySpec {
                    name: String::from("status"),
                    display_name: String::from("状态"),
                    description: String::new(),
                    unit: None,
                    data_type: DataType::String,
                    kind: PropertyKind::Attribute,
                    required: true,
                    pii_category: PiiCategory::None,
                    allowed_values: Some(vec![String::from("ACTIVE"), String::from("MAINTENANCE")]),
                    maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                },
                PropertySpec {
                    name: String::from("inventory"),
                    display_name: String::from("库存"),
                    description: String::new(),
                    unit: Some(String::from("件")),
                    data_type: DataType::Float64,
                    kind: PropertyKind::Metric,
                    required: false,
                    pii_category: PiiCategory::None,
                    allowed_values: None,
                    maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                },
            ],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 120.0,
            pos_y: 120.0,
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            semantic_layer_tier: None,
            domain_group: None,
        }],
        edge_types: Vec::new(),
        mappings: Vec::new(),
    }
}

async fn seed_warehouse_schema(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
         VALUES ($1, 'main', 'Main', TRUE)
         ON CONFLICT (tenant_id, slug) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_node_types (tenant_id, subgraph_slug, name, display_name)
         VALUES ($1, 'main', 'Warehouse', '仓库')
         ON CONFLICT (tenant_id, subgraph_slug, name) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_edge_types (tenant_id, subgraph_slug, name, src_type_name, dst_type_name)
         VALUES ($1, 'main', 'SUPPLIES', 'Warehouse', 'Warehouse')
         ON CONFLICT (tenant_id, subgraph_slug, name, src_type_name, dst_type_name) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    let doc_json = serde_json::to_value(warehouse_schema_doc())?;
    sqlx::query(
        "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
         VALUES ($1, 'main', 1)
         ON CONFLICT (tenant_id, subgraph_slug) DO UPDATE SET current_version = 1",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_schema_versions (tenant_id, subgraph_slug, version, schema_doc, change_summary)
         VALUES ($1, 'main', 1, $2, 'test seed')
         ON CONFLICT (tenant_id, subgraph_slug, version) DO UPDATE SET schema_doc = EXCLUDED.schema_doc",
    )
    .bind(tenant_id)
    .bind(doc_json)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn seed_open_alert(
    owner_pool: &PgPool,
    tenant_id: &str,
    node_id: Uuid,
    dedup_key: &str,
    status: &str,
) -> TestResult {
    let rule_id: Uuid = sqlx::query_scalar(
        "INSERT INTO ax_rules (
             tenant_id, name, scope_kind, target, condition, severity, title_template
         )
         VALUES ($1, $2, 'METRIC', '{\"type_name\":\"Warehouse\",\"metric\":\"inventory\"}'::jsonb,
                 '{\"kind\":\"THRESHOLD\",\"op\":\"LT\",\"value\":100}'::jsonb, 'WARN', 'Low stock')
         ON CONFLICT (tenant_id, name) DO UPDATE SET updated_at = NOW()
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(format!("rule-{dedup_key}"))
    .fetch_one(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO ax_alerts (
             tenant_id, rule_id, dedup_key, severity, status, title, node_id
         )
         VALUES ($1, $2, $3, 'WARN', $4, 'Low inventory', $5)",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .bind(dedup_key)
    .bind(status)
    .bind(node_id)
    .execute(owner_pool)
    .await?;
    Ok(())
}

fn static_widget_params(
    dashboard_id: Uuid,
    widget_kind: WidgetKind,
    title: &str,
    position: WidgetPosition,
    deps: WidgetDeps,
) -> CreateWidgetRecordParams {
    CreateWidgetRecordParams {
        dashboard_id,
        widget_kind,
        title: String::from(title),
        position,
        data_source: WidgetDataSource::Static {
            columns: Vec::new(),
            rows: Vec::new(),
        },
        encoding: json!({}),
        options: json!({}),
        drill: Vec::new(),
        refresh_mode: RefreshMode::Live,
        refresh_seconds: 60,
        deps,
    }
}

async fn verify_dashboard_slug_and_draft_creation(
    store: &PgStore,
    tenant_a: &str,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    assert!(matches!(
        store
            .create_dashboard(
                tenant_a,
                None,
                CreateDashboardRequest {
                    name: String::from("Bad"),
                    slug: String::from("Bad_Slug!"),
                    description: String::new(),
                    filters: Vec::new(),
                    theme: DashboardTheme::Light,
                    canvas: CanvasConfig::default(),
                },
            )
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));

    let created = store
        .create_dashboard(
            tenant_a,
            None,
            CreateDashboardRequest {
                name: String::from("Supply Overview"),
                slug: String::from("supply-overview"),
                description: String::from("Main twin"),
                filters: Vec::new(),
                theme: DashboardTheme::Dark,
                canvas: CanvasConfig::default(),
            },
        )
        .await?;
    assert_eq!(created.dashboard.status, DashboardStatus::Draft);
    assert_eq!(created.dashboard.version, 1);

    assert!(matches!(
        store
            .create_dashboard(
                tenant_a,
                None,
                CreateDashboardRequest {
                    name: String::from("Dup"),
                    slug: String::from("supply-overview"),
                    description: String::new(),
                    filters: Vec::new(),
                    theme: DashboardTheme::Light,
                    canvas: CanvasConfig::default(),
                },
            )
            .await,
        Err(AppError::Conflict("NAME_TAKEN"))
    ));

    assert!(
        store
            .get_dashboard(tenant_a, created.dashboard.id, true)
            .await?
            .is_none()
    );
    assert!(
        store
            .get_dashboard_by_slug(tenant_a, "supply-overview", true)
            .await?
            .is_none()
    );
    assert!(
        store
            .list_dashboards(tenant_a, true, SimplePageQuery::default())
            .await?
            .items
            .is_empty()
    );
    Ok(created.dashboard.id)
}

async fn verify_dashboard_validation_and_publish(
    store: &PgStore,
    tenant_a: &str,
) -> Result<(Uuid, WidgetRecord), Box<dyn std::error::Error>> {
    let dashboard_id = verify_dashboard_slug_and_draft_creation(store, tenant_a).await?;
    let w1 = store
        .create_widget(
            tenant_a,
            static_widget_params(
                dashboard_id,
                WidgetKind::KpiCard,
                "Paid Amount",
                WidgetPosition {
                    x: 0,
                    y: 0,
                    w: 6,
                    h: 4,
                },
                WidgetDeps::default(),
            ),
        )
        .await?;

    let mut field_map = BTreeMap::new();
    field_map.insert(w1.id.to_string(), String::from("status"));
    let updated = store
        .update_dashboard(
            tenant_a,
            dashboard_id,
            UpdateDashboardRequest {
                name: Some(String::from("Supply Overview V2")),
                filters: Some(vec![FilterDef {
                    key: String::from("status"),
                    label: String::from("Status"),
                    kind: FilterKind::Select,
                    field_map,
                    options_source: None,
                    default: None,
                }]),
                ..UpdateDashboardRequest::default()
            },
        )
        .await?;
    assert_eq!(updated.name, "Supply Overview V2");

    let published = store.publish_dashboard(tenant_a, dashboard_id).await?;
    assert_eq!(published.status, DashboardStatus::Published);
    assert_eq!(published.version, 2);

    let by_slug = store
        .get_dashboard_by_slug(tenant_a, "supply-overview", true)
        .await?
        .ok_or("published dashboard missing by slug")?;
    assert_eq!(by_slug.widgets.len(), 1);
    Ok((dashboard_id, w1))
}

async fn verify_duplicate_ai_and_pagination(
    store: &PgStore,
    tenant_a: &str,
    tenant_b: &str,
    dashboard_id: Uuid,
    original_widget_id: Uuid,
) -> TestResult {
    let dup = store
        .duplicate_dashboard(
            tenant_a,
            dashboard_id,
            Some(String::from("Supply Copy")),
            Some(String::from("supply-copy")),
            None,
        )
        .await?;
    assert_eq!(dup.dashboard.status, DashboardStatus::Draft);
    assert_eq!(dup.dashboard.version, 1);
    let cloned_w = dup.widgets.first().ok_or("cloned widget missing")?;
    assert_ne!(cloned_w.id, original_widget_id);
    let cloned_filter = dup
        .dashboard
        .filters
        .first()
        .ok_or("cloned filter missing")?;
    assert_eq!(
        cloned_filter
            .field_map
            .get(&cloned_w.id.to_string())
            .map(String::as_str),
        Some("status")
    );

    let ai = store
        .create_ai_dashboard(
            tenant_a,
            None,
            CreateDashboardRequest {
                name: String::from("AI Cockpit"),
                slug: String::from("ai-cockpit"),
                description: String::from("Generated"),
                filters: Vec::new(),
                theme: DashboardTheme::Screen,
                canvas: CanvasConfig::default(),
            },
            vec![static_widget_params(
                Uuid::nil(),
                WidgetKind::AlertList,
                "Alerts",
                WidgetPosition {
                    x: 0,
                    y: 0,
                    w: 12,
                    h: 6,
                },
                WidgetDeps {
                    alerts: true,
                    ..WidgetDeps::default()
                },
            )],
        )
        .await?;
    assert_eq!(ai.dashboard.status, DashboardStatus::Draft);
    assert_eq!(ai.widgets.len(), 1);

    let page1 = store
        .list_dashboards(
            tenant_a,
            false,
            SimplePageQuery {
                limit: Some(2),
                cursor: None,
            },
        )
        .await?;
    assert_eq!(page1.items.len(), 2);
    let page2 = store
        .list_dashboards(
            tenant_a,
            false,
            SimplePageQuery {
                limit: Some(2),
                cursor: page1.next_cursor,
            },
        )
        .await?;
    assert_eq!(page2.items.len(), 1);
    assert!(page2.next_cursor.is_none());

    assert!(
        store
            .get_dashboard(tenant_b, dashboard_id, false)
            .await?
            .is_none()
    );
    assert!(!store.delete_dashboard(tenant_b, dashboard_id).await?);
    assert!(store.delete_dashboard(tenant_a, ai.dashboard.id).await?);
    Ok(())
}

#[tokio::test]
async fn view_repo_dashboard_crud_publish_duplicate_and_ai_generate() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_dash_a");
    let tenant_b = unique_tenant("t_view_dash_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;
    let store = PgStore::new(app_pool);

    let (dashboard_id, w1) = verify_dashboard_validation_and_publish(&store, &tenant_a).await?;
    verify_duplicate_ai_and_pagination(&store, &tenant_a, &tenant_b, dashboard_id, w1.id).await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

#[tokio::test]
async fn view_repo_widgets_layout_and_gin_deps_lookup() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_wid_a");
    let tenant_b = unique_tenant("t_view_wid_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;
    let store = PgStore::new(app_pool);

    let db = store
        .create_dashboard(
            &tenant_a,
            None,
            CreateDashboardRequest {
                name: String::from("Deps Board"),
                slug: String::from("deps-board"),
                description: String::new(),
                filters: Vec::new(),
                theme: DashboardTheme::Light,
                canvas: CanvasConfig::default(),
            },
        )
        .await?;

    let ds1 = Uuid::new_v4();
    let sc1 = Uuid::new_v4();
    let w_ds = store
        .create_widget(
            &tenant_a,
            static_widget_params(
                db.dashboard.id,
                WidgetKind::BarChart,
                "Orders",
                WidgetPosition {
                    x: 0,
                    y: 0,
                    w: 12,
                    h: 6,
                },
                WidgetDeps {
                    dataset_ids: vec![ds1],
                    ..WidgetDeps::default()
                },
            ),
        )
        .await?;
    let w_node = store
        .create_widget(
            &tenant_a,
            static_widget_params(
                db.dashboard.id,
                WidgetKind::Scene3d,
                "3D Twin",
                WidgetPosition {
                    x: 12,
                    y: 0,
                    w: 12,
                    h: 6,
                },
                WidgetDeps {
                    type_names: vec![String::from("Warehouse")],
                    metrics: vec![String::from("inventory")],
                    alerts: true,
                    scenario_ids: vec![sc1],
                    ..WidgetDeps::default()
                },
            ),
        )
        .await?;

    verify_widget_updates_and_deps(
        &store,
        &tenant_a,
        &tenant_b,
        db.dashboard.id,
        &w_ds,
        &w_node,
    )
    .await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

async fn verify_widget_updates_and_deps(
    store: &PgStore,
    tenant_a: &str,
    tenant_b: &str,
    dashboard_id: Uuid,
    w_ds: &WidgetRecord,
    w_node: &WidgetRecord,
) -> TestResult {
    let updated = store
        .update_widget(
            tenant_a,
            w_ds.id,
            UpdateWidgetRecordParams {
                title: Some(String::from("Orders Updated")),
                refresh_seconds: Some(30),
                ..UpdateWidgetRecordParams::default()
            },
        )
        .await?;
    assert_eq!(updated.title, "Orders Updated");
    assert_eq!(updated.refresh_seconds, 30);

    let reordered = store
        .update_dashboard_layout(
            tenant_a,
            dashboard_id,
            &[
                LayoutUpdateItem {
                    id: w_node.id,
                    position: WidgetPosition {
                        x: 0,
                        y: 0,
                        w: 12,
                        h: 6,
                    },
                },
                LayoutUpdateItem {
                    id: w_ds.id,
                    position: WidgetPosition {
                        x: 0,
                        y: 6,
                        w: 12,
                        h: 6,
                    },
                },
            ],
        )
        .await?;
    assert_eq!(reordered.first().map(|w| w.id), Some(w_node.id));
    assert_eq!(reordered.get(1).map(|w| w.id), Some(w_ds.id));

    let by_ds = store
        .find_widgets_by_deps(
            tenant_a,
            &WidgetDeps {
                dataset_ids: w_ds.deps.dataset_ids.clone(),
                ..WidgetDeps::default()
            },
        )
        .await?;
    assert_eq!(by_ds.len(), 1);
    assert_eq!(by_ds.first().map(|r| r.widget_id), Some(w_ds.id));

    let by_alert_metric = store
        .find_widgets_by_deps(
            tenant_a,
            &WidgetDeps {
                metrics: vec![String::from("inventory")],
                alerts: true,
                ..WidgetDeps::default()
            },
        )
        .await?;
    assert_eq!(by_alert_metric.len(), 1);
    assert_eq!(
        by_alert_metric.first().map(|r| r.widget_id),
        Some(w_node.id)
    );

    let tenant_b_deps = store
        .find_widgets_by_deps(
            tenant_b,
            &WidgetDeps {
                dataset_ids: w_ds.deps.dataset_ids.clone(),
                alerts: true,
                ..WidgetDeps::default()
            },
        )
        .await?;
    assert_eq!(tenant_b_deps, []);
    assert!(store.get_widget(tenant_b, w_ds.id).await?.is_none());
    assert!(store.delete_widget(tenant_a, w_ds.id).await?);
    Ok(())
}

async fn create_test_spatial_assets(
    store: &PgStore,
    tenant_a: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    let floor_asset = store
        .create_asset(
            tenant_a,
            CreateAssetParams {
                id: Uuid::new_v4(),
                asset_kind: AssetKind::FloorPlanImage,
                name: String::from("floor-1.png"),
                file_path: String::from("assets/t/floor-1.png"),
                mime_type: String::from("image/png"),
                sha256: String::from("abcd"),
                width_px: Some(1200),
                height_px: Some(800),
            },
        )
        .await?;
    let glb_asset = store
        .create_asset(
            tenant_a,
            CreateAssetParams {
                id: Uuid::new_v4(),
                asset_kind: AssetKind::Model3d,
                name: String::from("warehouse_twin.glb"),
                file_path: String::from("assets/t/warehouse_twin.glb"),
                mime_type: String::from("model/gltf-binary"),
                sha256: String::from("ef01"),
                width_px: None,
                height_px: None,
            },
        )
        .await?;
    let models_only = store
        .list_assets(
            tenant_a,
            Some(AssetKind::Model3d),
            SimplePageQuery::default(),
        )
        .await?;
    assert_eq!(models_only.items.len(), 1);
    Ok((floor_asset.id, glb_asset.id))
}

#[tokio::test]
async fn view_repo_assets_spatial_bindings_and_queries() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_sp_a");
    let tenant_b = unique_tenant("t_view_sp_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;
    seed_warehouse_schema(&owner_pool, &tenant_a).await?;
    let store = PgStore::new(app_pool);

    let mut metrics = BTreeMap::new();
    metrics.insert(String::from("inventory"), 420.5);
    let node_w1 = store
        .upsert_blue_node(
            &tenant_a,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("East Warehouse"),
                metrics,
                geo: Some(GeoPoint {
                    lng: 121.4737,
                    lat: 31.2304,
                }),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    seed_open_alert(&owner_pool, &tenant_a, node_w1.id, "k-open", "OPEN").await?;
    seed_open_alert(&owner_pool, &tenant_a, node_w1.id, "k-ack", "ACK").await?;
    seed_open_alert(&owner_pool, &tenant_a, node_w1.id, "k-res", "RESOLVED").await?;

    let (floor_id, glb_id) = create_test_spatial_assets(&store, &tenant_a).await?;
    verify_spatial_bindings_and_rows(&store, &tenant_a, &tenant_b, node_w1.id, floor_id, glb_id)
        .await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

async fn verify_spatial_bindings_and_rows(
    store: &PgStore,
    tenant_a: &str,
    tenant_b: &str,
    node_id: Uuid,
    floor_id: Uuid,
    glb_id: Uuid,
) -> TestResult {
    assert!(matches!(
        store
            .replace_floor_placements(
                tenant_a,
                floor_id,
                &[PutFloorPlacementItem {
                    node_id,
                    x_ratio: 1.5,
                    y_ratio: 0.5,
                }],
            )
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));

    let placements = store
        .replace_floor_placements(
            tenant_a,
            floor_id,
            &[PutFloorPlacementItem {
                node_id,
                x_ratio: 0.25,
                y_ratio: 0.75,
            }],
        )
        .await?;
    assert_eq!(placements.len(), 1);
    assert_eq!(
        store.list_floor_placements(tenant_a, floor_id).await?.len(),
        1
    );

    let bindings = store
        .replace_scene3d_bindings(
            tenant_a,
            glb_id,
            &[PutScene3dBindingItem {
                node_id,
                mesh_name: String::from("Rack_A"),
                pos_x: 1.0,
                pos_y: 2.0,
                pos_z: 3.0,
                scale: 1.5,
            }],
        )
        .await?;
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        store.list_scene3d_bindings(tenant_a, glb_id).await?.len(),
        1
    );

    let geo_rows = store
        .query_nodes_geo(tenant_a, &[String::from("Warehouse")], Some("inventory"))
        .await?;
    assert_eq!(geo_rows.first().and_then(|r| r.metric_value), Some(420.5));
    assert_eq!(geo_rows.first().map(|r| r.open_alerts), Some(2));

    let fp_rows = store
        .query_floor_plan_rows(tenant_a, floor_id, Some("inventory"))
        .await?;
    assert_eq!(fp_rows.first().and_then(|r| r.metric_value), Some(420.5));
    assert_eq!(fp_rows.first().map(|r| r.open_alerts), Some(2));

    let s3d_rows = store
        .query_scene3d_rows(tenant_a, glb_id, Some("inventory"))
        .await?;
    let s3d_row = s3d_rows.first().ok_or("missing scene3d row")?;
    assert_eq!(s3d_row.mesh_name, "Rack_A");
    assert_eq!(s3d_row.metric_value, Some(420.5));
    assert_eq!(s3d_row.open_alerts, 2);

    assert!(store.get_asset(tenant_b, glb_id).await?.is_none());
    assert!(
        store
            .query_scene3d_rows(tenant_b, glb_id, Some("inventory"))
            .await?
            .is_empty()
    );
    assert!(store.delete_asset(tenant_a, glb_id).await?.is_some());
    Ok(())
}

async fn verify_write_node_validations(
    store: &PgStore,
    tenant_a: &str,
    widget_id: Uuid,
    node_id: Uuid,
) -> TestResult {
    assert!(matches!(
        store
            .execute_view_write_node(
                tenant_a,
                ViewWriteNodeParams {
                    widget_id,
                    node_id,
                    target_kind: WriteNodeTargetKind::Metric,
                    key: String::from("inventory"),
                    value: json!(600),
                    idempotency_key: String::from("idem-abac-deny"),
                    caller_dept_id: String::from("finance"),
                    numeric_value: Some(600.0),
                },
            )
            .await,
        Err(AppError::Forbidden("PERMISSION_DENIED"))
    ));

    assert!(matches!(
        store
            .execute_view_write_node(
                tenant_a,
                ViewWriteNodeParams {
                    widget_id,
                    node_id,
                    target_kind: WriteNodeTargetKind::Property,
                    key: String::from("inventory"),
                    value: json!(600),
                    idempotency_key: String::from("idem-wrong-kind"),
                    caller_dept_id: String::from("supply"),
                    numeric_value: None,
                },
            )
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));

    assert!(matches!(
        store
            .execute_view_write_node(
                tenant_a,
                ViewWriteNodeParams {
                    widget_id,
                    node_id,
                    target_kind: WriteNodeTargetKind::Property,
                    key: String::from("status"),
                    value: json!("UNKNOWN_STATE"),
                    idempotency_key: String::from("idem-bad-enum"),
                    caller_dept_id: String::from("supply"),
                    numeric_value: None,
                },
            )
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));
    Ok(())
}

async fn verify_write_node_commit_and_replay(
    store: &PgStore,
    owner_pool: &PgPool,
    tenant_a: &str,
    tenant_b: &str,
    widget_id: Uuid,
    node_id: Uuid,
) -> TestResult {
    let prop_res = store
        .execute_view_write_node(
            tenant_a,
            ViewWriteNodeParams {
                widget_id,
                node_id,
                target_kind: WriteNodeTargetKind::Property,
                key: String::from("manager"),
                value: json!("Alice"),
                idempotency_key: String::from("idem-prop-1"),
                caller_dept_id: String::from("supply"),
                numeric_value: None,
            },
        )
        .await?;
    assert!(!prop_res.idempotency_replay);

    let met_res = store
        .execute_view_write_node(
            tenant_a,
            ViewWriteNodeParams {
                widget_id,
                node_id,
                target_kind: WriteNodeTargetKind::Metric,
                key: String::from("inventory"),
                value: json!(620),
                idempotency_key: String::from("idem-met-1"),
                caller_dept_id: String::from("supply"),
                numeric_value: Some(620.0),
            },
        )
        .await?;
    assert!(!met_res.idempotency_replay);

    let replay = store
        .execute_view_write_node(
            tenant_a,
            ViewWriteNodeParams {
                widget_id,
                node_id,
                target_kind: WriteNodeTargetKind::Metric,
                key: String::from("inventory"),
                value: json!(999),
                idempotency_key: String::from("idem-met-1"),
                caller_dept_id: String::from("supply"),
                numeric_value: Some(999.0),
            },
        )
        .await?;
    assert!(replay.idempotency_replay);
    assert_eq!(replay.action_log_id, met_res.action_log_id);

    let reloaded = store
        .get_node(tenant_a, node_id)
        .await?
        .ok_or("node missing after writeback")?;
    assert_eq!(
        reloaded.properties.get("manager").and_then(|v| v.as_str()),
        Some("Alice")
    );
    assert_eq!(reloaded.metrics.get("inventory").copied(), Some(620.0));

    verify_write_node_logs_outbox_and_rls(store, owner_pool, tenant_a, tenant_b, widget_id, node_id)
        .await
}

async fn verify_write_node_logs_outbox_and_rls(
    store: &PgStore,
    owner_pool: &PgPool,
    tenant_a: &str,
    tenant_b: &str,
    widget_id: Uuid,
    node_id: Uuid,
) -> TestResult {
    let series_val: f64 = sqlx::query_scalar(
        "SELECT value FROM xb_metric_series
          WHERE tenant_id = $1 AND node_id = $2 AND metric = 'inventory' AND source_tag = 'ACTION'",
    )
    .bind(tenant_a)
    .bind(node_id)
    .fetch_one(owner_pool)
    .await?;
    assert!((series_val - 620.0).abs() < f64::EPSILON);

    let logs = store
        .list_widget_action_logs(tenant_a, widget_id, 10)
        .await?;
    assert_eq!(logs.len(), 2);
    let latest = logs.first().ok_or("missing latest action log")?;
    assert_eq!(latest.source_kind, ActionSourceKind::View);
    assert_eq!(latest.action_kind, ActionKind::WriteNodeMetric);
    assert_eq!(latest.status, ActionLogStatus::Done);

    let outbox_topics: Vec<String> =
        sqlx::query("SELECT topic FROM ax_outbox WHERE tenant_id = $1 ORDER BY id ASC")
            .bind(tenant_a)
            .fetch_all(owner_pool)
            .await?
            .into_iter()
            .map(|r| r.get("topic"))
            .collect();
    assert!(outbox_topics.contains(&String::from("node.upserted")));
    assert!(outbox_topics.contains(&String::from("node.metrics_changed")));

    assert!(matches!(
        store
            .execute_view_write_node(
                tenant_b,
                ViewWriteNodeParams {
                    widget_id,
                    node_id,
                    target_kind: WriteNodeTargetKind::Metric,
                    key: String::from("inventory"),
                    value: json!(700),
                    idempotency_key: String::from("idem-cross-tenant"),
                    caller_dept_id: String::new(),
                    numeric_value: Some(700.0),
                },
            )
            .await,
        Err(AppError::NotFound("NODE_NOT_FOUND"))
    ));
    assert!(
        store
            .list_widget_action_logs(tenant_b, widget_id, 10)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn view_repo_atomic_write_node_idempotency_and_outbox() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_wr_a");
    let tenant_b = unique_tenant("t_view_wr_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;
    seed_warehouse_schema(&owner_pool, &tenant_a).await?;
    let store = PgStore::new(app_pool);

    let mut props = serde_json::Map::new();
    props.insert(String::from("dept_id"), json!("supply"));
    props.insert(String::from("status"), json!("ACTIVE"));
    let mut metrics = BTreeMap::new();
    metrics.insert(String::from("inventory"), 500.0);

    let node = store
        .upsert_blue_node(
            &tenant_a,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("East Warehouse"),
                properties: props,
                metrics,
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let widget_id = Uuid::new_v4();

    verify_write_node_validations(&store, &tenant_a, widget_id, node.id).await?;
    verify_write_node_commit_and_replay(
        &store,
        &owner_pool,
        &tenant_a,
        &tenant_b,
        widget_id,
        node.id,
    )
    .await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

async fn assert_bounds_and_canvas_rejected(
    store: &PgStore,
    tenant_a: &str,
    dashboard_id: Uuid,
) -> TestResult {
    assert!(matches!(
        store
            .create_dashboard(
                tenant_a,
                None,
                CreateDashboardRequest {
                    name: String::from("Zero Cols"),
                    slug: String::from("zero-cols"),
                    description: String::new(),
                    filters: Vec::new(),
                    theme: DashboardTheme::Light,
                    canvas: CanvasConfig {
                        cols: 0,
                        ..CanvasConfig::default()
                    },
                },
            )
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));

    assert!(matches!(
        store
            .create_widget(
                tenant_a,
                static_widget_params(
                    dashboard_id,
                    WidgetKind::KpiCard,
                    "Overflow X+W",
                    WidgetPosition {
                        x: 20,
                        y: 0,
                        w: 6,
                        h: 4,
                    },
                    WidgetDeps::default(),
                ),
            )
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));
    Ok(())
}

async fn assert_asset_kind_and_widget_lookup(
    store: &PgStore,
    tenant_a: &str,
    dashboard_id: Uuid,
    floor_id: Uuid,
    glb_id: Uuid,
) -> TestResult {
    assert!(matches!(
        store
            .replace_floor_placements(tenant_a, Uuid::new_v4(), &[])
            .await,
        Err(AppError::NotFound("ASSET_NOT_FOUND"))
    ));
    assert!(matches!(
        store.replace_floor_placements(tenant_a, glb_id, &[]).await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));
    assert!(matches!(
        store
            .replace_scene3d_bindings(tenant_a, floor_id, &[])
            .await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));

    let fp_widget = store
        .create_widget(
            tenant_a,
            CreateWidgetRecordParams {
                dashboard_id,
                widget_kind: WidgetKind::FloorPlan,
                title: String::from("Floor View"),
                position: WidgetPosition {
                    x: 0,
                    y: 0,
                    w: 12,
                    h: 6,
                },
                data_source: WidgetDataSource::FloorPlan {
                    asset_id: floor_id,
                    color_metric: Some(String::from("inventory")),
                },
                encoding: json!({}),
                options: json!({}),
                drill: Vec::new(),
                refresh_mode: RefreshMode::Live,
                refresh_seconds: 60,
                deps: WidgetDeps::default(),
            },
        )
        .await?;
    let s3d_widget = store
        .create_widget(
            tenant_a,
            CreateWidgetRecordParams {
                dashboard_id,
                widget_kind: WidgetKind::Scene3d,
                title: String::from("3D Scene"),
                position: WidgetPosition {
                    x: 12,
                    y: 0,
                    w: 12,
                    h: 6,
                },
                data_source: WidgetDataSource::Scene3d {
                    asset_id: glb_id,
                    color_metric: None,
                },
                encoding: json!({}),
                options: json!({}),
                drill: Vec::new(),
                refresh_mode: RefreshMode::Live,
                refresh_seconds: 60,
                deps: WidgetDeps::default(),
            },
        )
        .await?;

    let by_floor = store.list_widgets_by_asset(tenant_a, floor_id).await?;
    assert_eq!(by_floor.len(), 1);
    assert_eq!(by_floor.first().map(|w| w.id), Some(fp_widget.id));

    let by_glb = store.list_widgets_by_asset(tenant_a, glb_id).await?;
    assert_eq!(by_glb.len(), 1);
    assert_eq!(by_glb.first().map(|w| w.id), Some(s3d_widget.id));
    Ok(())
}

#[tokio::test]
async fn view_repo_bounds_canvas_asset_kind_and_widgets_by_asset() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_bnd_a");
    setup_tenant(&owner_pool, &tenant_a).await?;
    let store = PgStore::new(app_pool);

    let board = store
        .create_dashboard(
            &tenant_a,
            None,
            CreateDashboardRequest {
                name: String::from("Bounds Board"),
                slug: String::from("bounds-board"),
                description: String::new(),
                filters: Vec::new(),
                theme: DashboardTheme::Light,
                canvas: CanvasConfig::default(),
            },
        )
        .await?;
    assert_bounds_and_canvas_rejected(&store, &tenant_a, board.dashboard.id).await?;
    let (floor_id, glb_id) = create_test_spatial_assets(&store, &tenant_a).await?;
    assert_asset_kind_and_widget_lookup(&store, &tenant_a, board.dashboard.id, floor_id, glb_id)
        .await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    Ok(())
}

async fn seed_two_geo_warehouses(
    store: &PgStore,
    tenant_a: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    let mut props = serde_json::Map::new();
    props.insert(String::from("dept_id"), json!("supply"));
    props.insert(String::from("status"), json!("ACTIVE"));
    let mut metrics = BTreeMap::new();
    metrics.insert(String::from("inventory"), 500.0);

    let w1 = store
        .upsert_blue_node(
            tenant_a,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-SH"),
                title: String::from("Shanghai Hub"),
                properties: props.clone(),
                metrics: metrics.clone(),
                geo: Some(GeoPoint {
                    lng: 121.4737,
                    lat: 31.2304,
                }),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let w2 = store
        .upsert_blue_node(
            tenant_a,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-SZ"),
                title: String::from("Shenzhen Hub"),
                properties: props,
                metrics,
                geo: Some(GeoPoint {
                    lng: 114.0579,
                    lat: 22.5431,
                }),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    Ok((w1.id, w2.id))
}

async fn assert_concurrent_and_cross_widget_idempotency(
    store: &PgStore,
    tenant_a: &str,
    node_id: Uuid,
) -> TestResult {
    let widget_a = Uuid::new_v4();
    let widget_b = Uuid::new_v4();
    let params_a = ViewWriteNodeParams {
        widget_id: widget_a,
        node_id,
        target_kind: WriteNodeTargetKind::Metric,
        key: String::from("inventory"),
        value: json!(750),
        idempotency_key: String::from("idem-race-key"),
        caller_dept_id: String::from("supply"),
        numeric_value: Some(750.0),
    };
    let (res_1, res_2) = tokio::join!(
        store.execute_view_write_node(tenant_a, params_a.clone()),
        store.execute_view_write_node(tenant_a, params_a),
    );
    let out_1 = res_1?;
    let out_2 = res_2?;
    assert_eq!(out_1.action_log_id, out_2.action_log_id);
    assert_ne!(out_1.idempotency_replay, out_2.idempotency_replay);

    assert!(matches!(
        store
            .execute_view_write_node(
                tenant_a,
                ViewWriteNodeParams {
                    widget_id: widget_b,
                    node_id,
                    target_kind: WriteNodeTargetKind::Metric,
                    key: String::from("inventory"),
                    value: json!(800),
                    idempotency_key: String::from("idem-race-key"),
                    caller_dept_id: String::from("supply"),
                    numeric_value: Some(800.0),
                },
            )
            .await,
        Err(AppError::Conflict("CONFLICT"))
    ));
    Ok(())
}

#[tokio::test]
async fn view_repo_geo_links_and_concurrent_write_idempotency() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_gl_a");
    setup_tenant(&owner_pool, &tenant_a).await?;
    seed_warehouse_schema(&owner_pool, &tenant_a).await?;
    let store = PgStore::new(app_pool);

    let (w1_id, w2_id) = seed_two_geo_warehouses(&store, &tenant_a).await?;
    let link = store
        .upsert_link(
            &tenant_a,
            UpsertLinkRequest {
                link_type: String::from("SUPPLIES"),
                src_node_id: w1_id,
                dst_node_id: w2_id,
                weight: Some(0.85),
                properties: serde_json::Map::new(),
                valid_from: None,
                valid_to: None,
            },
        )
        .await?;

    let matched = store
        .query_nodes_geo_links(&tenant_a, &[w1_id, w2_id], &[String::from("SUPPLIES")])
        .await?;
    assert_eq!(matched.len(), 1);
    assert_eq!(matched.first().map(|r| r.link_id), Some(link.id));

    let filtered_out = store
        .query_nodes_geo_links(&tenant_a, &[w1_id, w2_id], &[String::from("TRANSPORTS")])
        .await?;
    assert!(filtered_out.is_empty());

    assert_concurrent_and_cross_widget_idempotency(&store, &tenant_a, w1_id).await?;
    cleanup_tenant(&owner_pool, &tenant_a).await?;
    Ok(())
}

#[tokio::test]
async fn view_repo_export_jobs_templates_and_retention() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_exp_a");
    let tenant_b = unique_tenant("t_view_exp_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;
    let store = PgStore::new(app_pool);

    assert_export_template_crud(&store, &tenant_a, &tenant_b).await?;
    assert_export_job_lifecycle_and_retention(&store, &tenant_a, &tenant_b).await?;

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

async fn assert_export_template_crud(
    store: &PgStore,
    tenant_a: &str,
    tenant_b: &str,
) -> TestResult {
    use axpoint_domain::enums::ExportFormat;
    use axpoint_domain::export::CreateExportTemplateParams;

    let tpl_id = Uuid::new_v4();
    let created = store
        .create_export_template(
            tenant_a,
            CreateExportTemplateParams {
                id: tpl_id,
                name: String::from("周报模板"),
                export_format: ExportFormat::Docx,
                file_path: format!("export_templates/{tenant_a}/{tpl_id}.docx"),
                sha256: "a".repeat(64),
                placeholders: vec![String::from("title"), String::from("sections")],
                created_by: Some(Uuid::nil()),
            },
        )
        .await?;
    assert_eq!(created.id, tpl_id);
    assert_eq!(created.placeholders.len(), 2);

    let dup = store
        .create_export_template(
            tenant_a,
            CreateExportTemplateParams {
                id: Uuid::new_v4(),
                name: String::from("周报模板"),
                export_format: ExportFormat::Docx,
                file_path: String::from("other.docx"),
                sha256: "b".repeat(64),
                placeholders: Vec::new(),
                created_by: None,
            },
        )
        .await;
    assert!(matches!(dup, Err(AppError::Conflict("CONFLICT"))));

    assert!(store.get_export_template(tenant_b, tpl_id).await?.is_none());
    let listed = store
        .list_export_templates(tenant_a, SimplePageQuery::default())
        .await?;
    assert_eq!(listed.items.len(), 1);
    assert!(
        store
            .delete_export_template(tenant_a, tpl_id)
            .await?
            .is_some()
    );
    Ok(())
}

async fn assert_export_job_lifecycle_and_retention(
    store: &PgStore,
    tenant_a: &str,
    tenant_b: &str,
) -> TestResult {
    use axpoint_domain::enums::{ExportFormat, JobKind, JobStatus};
    use axpoint_domain::export::{
        CompleteExportJobParams, CreateExportJobParams, ExportListQuery, ExportOptions,
        ExportSource,
    };
    use axpoint_domain::ports::JobQueuePort as _;

    assert_eq!(store.get_export_keep_days(tenant_a).await?, 30);

    let job = store
        .create_export_job(
            tenant_a,
            CreateExportJobParams {
                export_format: ExportFormat::Pdf,
                source: ExportSource::Dashboard {
                    dashboard_id: Uuid::new_v4(),
                    filters: BTreeMap::new(),
                },
                options: ExportOptions::default(),
                template_id: None,
                created_by: Uuid::nil(),
            },
        )
        .await?;
    assert_eq!(job.status, JobStatus::Pending);
    assert_eq!(job.created_by, Uuid::nil());
    let qjob_id = job.job_id.ok_or("missing backing ax_jobs.id")?;
    let qjob = store
        .get_job(tenant_a, qjob_id)
        .await?
        .ok_or("backing ax_job not found")?;
    assert_eq!(qjob.kind, JobKind::Export);

    let running = store
        .mark_export_job_running(tenant_a, job.id)
        .await?
        .ok_or("mark_export_job_running returned None")?;
    assert_eq!(running.status, JobStatus::Running);

    let expired_ts = chrono::Utc::now() - chrono::Duration::minutes(5);
    let completed = store
        .complete_export_job(
            tenant_a,
            job.id,
            CompleteExportJobParams {
                file_name: String::from("report.pdf"),
                file_path: format!("exports/{tenant_a}/{}.pdf", job.id),
                file_size: 2048,
                sha256: "c".repeat(64),
                expires_at: expired_ts,
            },
        )
        .await?;
    assert_eq!(completed.status, JobStatus::Succeeded);
    assert!(store.get_export_job(tenant_b, job.id).await?.is_none());

    let listed = store
        .list_export_jobs(tenant_a, Some(Uuid::nil()), ExportListQuery::default())
        .await?;
    assert_eq!(listed.items.len(), 1);

    let _stats = store.run_retention_cleanup(tenant_a).await?;
    let after_ret = store
        .get_export_job(tenant_a, job.id)
        .await?
        .ok_or("job missing after retention")?;
    assert_eq!(after_ret.status, JobStatus::Expired);
    assert!(after_ret.file_path.is_none());
    Ok(())
}

#[tokio::test]
async fn view_repo_share_links_resolve_max_views_and_revocation() -> TestResult {
    use axpoint_domain::export::{CreateShareLinkParams, ShareListQuery, ShareTargetKind};

    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_view_sh_a");
    setup_tenant(&owner_pool, &tenant_a).await?;
    let store = PgStore::new(app_pool);

    let target_id = Uuid::new_v4();
    let token_hash = format!("hash_{}", Uuid::new_v4().simple());
    let link = store
        .create_share_link(
            &tenant_a,
            CreateShareLinkParams {
                token_prefix: String::from("abcd1234"),
                token_hash: token_hash.clone(),
                target_kind: ShareTargetKind::Dashboard,
                target_id,
                locked_filters: BTreeMap::from([(String::from("status"), json!("PAID"))]),
                allow_filters: false,
                allow_export: true,
                password_hash: None,
                expires_at: chrono::Utc::now() + chrono::Duration::hours(2),
                max_views: Some(2),
                created_by: Uuid::nil(),
            },
        )
        .await?;
    assert_eq!(link.max_views, Some(2));
    assert_eq!(link.view_count, 0);

    let resolved = store
        .resolve_share_by_hash(&token_hash)
        .await?
        .ok_or("expected active share to resolve")?;
    assert_eq!(resolved.share_id, link.id);
    assert_eq!(resolved.token_prefix, "abcd1234");

    assert!(store.increment_share_view_count(&tenant_a, link.id).await?);
    assert!(store.increment_share_view_count(&tenant_a, link.id).await?);
    assert!(!store.increment_share_view_count(&tenant_a, link.id).await?);
    assert!(store.resolve_share_by_hash(&token_hash).await?.is_none());

    let token_hash_2 = format!("hash_{}", Uuid::new_v4().simple());
    let link2 = store
        .create_share_link(
            &tenant_a,
            CreateShareLinkParams {
                token_prefix: String::from("efgh5678"),
                token_hash: token_hash_2.clone(),
                target_kind: ShareTargetKind::Dashboard,
                target_id,
                locked_filters: BTreeMap::new(),
                allow_filters: true,
                allow_export: false,
                password_hash: Some(String::from("$argon2id$v=19$test")),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                max_views: None,
                created_by: Uuid::nil(),
            },
        )
        .await?;
    assert!(link2.has_password);
    let listed = store
        .list_share_links(
            &tenant_a,
            ShareListQuery {
                target_id: Some(target_id),
                ..ShareListQuery::default()
            },
        )
        .await?;
    assert_eq!(listed.items.len(), 2);

    assert!(store.revoke_share_link(&tenant_a, link2.id).await?);
    assert!(!store.revoke_share_link(&tenant_a, link2.id).await?);
    assert!(store.resolve_share_by_hash(&token_hash_2).await?.is_none());

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    Ok(())
}
