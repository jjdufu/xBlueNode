//! Real PostgreSQL + Row-Level Security (`RLS`) integration tests for Step 25 `xv_charts` CRUD,
//! `CanvasConfig.compact_mode` round-trip, and `STANDARD_FIVE_ZONE` non-overlapping `pin` (`03-STEP25` §25.1–§25.2).

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::{
    CockpitZoneLayout, GridCompactMode, SemanticZoneKind, Sensitivity, WidgetKind,
};
use axpoint_domain::ports::ViewRepository as _;
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::view::{
    CanvasConfig, CreateDashboardRequest, CreateWidgetRecordParams, DashboardDetail,
    DashboardTheme, FiveZonePresetContext, PinChartToDashboardRequest, RefreshMode,
    SaveChartRequest, UpdateDashboardRequest, UpdateSavedChartRequest, WidgetDataSource,
    WidgetDeps, WidgetPosition, generate_five_zone_preset,
};
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
        "DELETE FROM xv_charts WHERE tenant_id = $1",
        "DELETE FROM xv_widgets WHERE tenant_id = $1",
        "DELETE FROM xv_dashboards WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

fn rects_overlap(a: &WidgetPosition, b: &WidgetPosition) -> bool {
    let a_right = a.x.saturating_add(a.w);
    let b_right = b.x.saturating_add(b.w);
    let a_bottom = a.y.saturating_add(a.h);
    let b_bottom = b.y.saturating_add(b.h);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

async fn seed_five_zone_dashboard(
    store: &PgStore,
    tenant: &str,
    slug: &str,
) -> Result<DashboardDetail, AppError> {
    let preset_slots: Vec<CreateWidgetRecordParams> =
        generate_five_zone_preset(&FiveZonePresetContext::default())
            .into_iter()
            .map(|slot| CreateWidgetRecordParams {
                dashboard_id: Uuid::nil(),
                widget_kind: slot.widget_kind,
                title: slot.title,
                position: slot.position,
                data_source: slot.data_source,
                encoding: slot.encoding,
                options: slot.options,
                drill: Vec::new(),
                refresh_mode: RefreshMode::Live,
                refresh_seconds: 60,
                deps: WidgetDeps::default(),
            })
            .collect();

    store
        .create_ai_dashboard(
            tenant,
            None,
            CreateDashboardRequest {
                name: String::from("五区联动看板"),
                slug: String::from(slug),
                description: String::new(),
                filters: Vec::new(),
                theme: DashboardTheme::Light,
                canvas: CanvasConfig::standard_five_zone(),
                zone_layout: Some(CockpitZoneLayout::StandardFiveZone),
            },
            preset_slots,
        )
        .await
}

#[tokio::test]
async fn xv_charts_crud_and_cross_tenant_rls_zero_visibility() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool.clone());
    let tenant_a = unique_tenant("step25_a");
    let tenant_b = unique_tenant("step25_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;

    let created = store
        .create_saved_chart(
            &tenant_a,
            Some(Uuid::new_v4()),
            "supply_dept",
            SaveChartRequest {
                name: String::from("各状态订单金额分布"),
                description: String::from("按履约状态聚合金额"),
                widget_kind: WidgetKind::BarChart,
                data_source: WidgetDataSource::Sql {
                    sql: String::from(
                        "SELECT status, SUM(amount) AS total_amt FROM demo_orders GROUP BY status",
                    ),
                },
                encoding: json!({ "x": "status", "y": ["total_amt"] }),
                options: json!({ "unit": "元", "show_mark_line": true }),
                sensitivity: Sensitivity::Internal,
                shared: true,
            },
        )
        .await?;
    assert_eq!(created.tenant_id, tenant_a);
    assert_eq!(created.owner_dept, "supply_dept");

    let dup = store
        .create_saved_chart(
            &tenant_a,
            None,
            "default",
            SaveChartRequest {
                name: String::from("各状态订单金额分布"),
                description: String::new(),
                widget_kind: WidgetKind::LineChart,
                data_source: WidgetDataSource::default(),
                encoding: json!({}),
                options: json!({}),
                sensitivity: Sensitivity::Internal,
                shared: true,
            },
        )
        .await;
    assert!(matches!(dup, Err(AppError::Conflict("CONFLICT"))));

    assert!(
        store
            .get_saved_chart(&tenant_b, created.id)
            .await?
            .is_none()
    );
    let b_list = store
        .list_saved_charts(&tenant_b, Uuid::nil(), true, SimplePageQuery::default())
        .await?;
    assert!(b_list.items.is_empty());

    let mut rls_tx = app_pool.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(&tenant_b)
        .execute(&mut *rls_tx)
        .await?;
    let leaked_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM xv_charts WHERE id = $1")
        .bind(created.id)
        .fetch_one(&mut *rls_tx)
        .await?;
    rls_tx.commit().await?;
    assert_eq!(leaked_cnt, 0);

    let updated = store
        .update_saved_chart(
            &tenant_a,
            created.id,
            UpdateSavedChartRequest {
                name: Some(String::from("各状态订单金额趋势")),
                widget_kind: Some(WidgetKind::LineChart),
                options: Some(json!({ "unit": "万元", "mark_line_value": 5000 })),
                ..UpdateSavedChartRequest::default()
            },
        )
        .await?
        .ok_or("expected updated chart")?;
    assert_eq!(updated.name, "各状态订单金额趋势");
    assert_eq!(updated.widget_kind, WidgetKind::LineChart);

    assert!(!store.delete_saved_chart(&tenant_b, created.id).await?);
    assert!(store.delete_saved_chart(&tenant_a, created.id).await?);
    assert!(
        store
            .get_saved_chart(&tenant_a, created.id)
            .await?
            .is_none()
    );

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

fn assert_no_widget_overlaps(detail: &DashboardDetail) {
    for (idx, a) in detail.widgets.iter().enumerate() {
        for b in detail.widgets.iter().skip(idx.saturating_add(1)) {
            assert!(
                !rects_overlap(&a.position, &b.position),
                "widgets {} {:?} and {} {:?} must not overlap",
                a.title,
                a.position,
                b.title,
                b.position
            );
        }
    }
}

#[tokio::test]
async fn pin_chart_to_standard_five_zone_insight_without_overlaps() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let tenant = unique_tenant("step25_pin");
    setup_tenant(&owner_pool, &tenant).await?;

    let chart = store
        .create_saved_chart(
            &tenant,
            None,
            "default",
            SaveChartRequest {
                name: String::from("履约状态金额分布"),
                description: String::new(),
                widget_kind: WidgetKind::BarChart,
                data_source: WidgetDataSource::Sql {
                    sql: String::from("SELECT status, amount FROM demo_orders"),
                },
                encoding: json!({ "x": "status", "y": ["amount"] }),
                options: json!({ "unit": "元" }),
                sensitivity: Sensitivity::Internal,
                shared: true,
            },
        )
        .await?;

    let created_dash = seed_five_zone_dashboard(&store, &tenant, "step25-five-zone").await?;
    if let Some(second_insight) = created_dash
        .widgets
        .iter()
        .find(|w| w.position.x == 7 && w.position.y == 9)
    {
        assert!(store.delete_widget(&tenant, second_insight.id).await?);
    }

    let first_pin = store
        .pin_saved_chart_to_dashboard(
            &tenant,
            chart.id,
            PinChartToDashboardRequest {
                dashboard_id: created_dash.dashboard.id,
                target_zone: Some(SemanticZoneKind::Insight),
                width: None,
                height: None,
            },
            WidgetDeps::default(),
        )
        .await?;
    assert_eq!(first_pin.semantic_zone, Some(SemanticZoneKind::Insight));
    assert_eq!(first_pin.deep_link, "/view/step25-five-zone");
    assert_eq!(
        first_pin.widget.position,
        WidgetPosition {
            x: 7,
            y: 9,
            w: 5,
            h: 4,
        }
    );

    let second_pin = store
        .pin_saved_chart_to_dashboard(
            &tenant,
            chart.id,
            PinChartToDashboardRequest {
                dashboard_id: created_dash.dashboard.id,
                target_zone: Some(SemanticZoneKind::Insight),
                width: None,
                height: Some(4),
            },
            WidgetDeps::default(),
        )
        .await?;
    assert_eq!(second_pin.semantic_zone, Some(SemanticZoneKind::Insight));
    assert_eq!(second_pin.widget.position.x, 7);
    assert!(second_pin.widget.position.x + second_pin.widget.position.w <= 12);

    let reloaded = store
        .get_dashboard(&tenant, created_dash.dashboard.id, false)
        .await?
        .ok_or("expected reloaded dashboard")?;
    assert_no_widget_overlaps(&reloaded);

    let mut next_canvas = reloaded.dashboard.canvas.clone();
    next_canvas.compact_mode = GridCompactMode::FreeFloat;
    let updated_dash = store
        .update_dashboard(
            &tenant,
            created_dash.dashboard.id,
            UpdateDashboardRequest {
                canvas: Some(next_canvas),
                ..UpdateDashboardRequest::default()
            },
        )
        .await?;
    assert_eq!(updated_dash.canvas.compact_mode, GridCompactMode::FreeFloat);

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}
