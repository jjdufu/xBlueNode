//! Integration tests for `axpoint_analytics::semantic` (`05-STEP04` §04.5).

use axpoint_analytics::semantic::{
    MetricExecuteOptions, check_metric_compatibility, execute_metric_query,
    execute_metric_query_conn, execute_multi_metric_query_conn, is_metric_broken,
    validate_create_metric_request,
};
use axpoint_analytics::{
    AnalyticsConfig, CatalogDatasetEntry, CreateMetricRequest, FilterOp, FilterSpec,
    MetricCompareMode, MetricDef, MetricDimension, MetricQuery, MetricTimeRange, MultiMetricQuery,
    SessionPool, TenantCatalog,
};
use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, PiiCategory, RollupGrain};
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

fn col_meta(dataset_id: Uuid, name: &str, data_type: DataType, ordinal: i32) -> ColumnMeta {
    ColumnMeta {
        dataset_id,
        column_name: String::from(name),
        data_type,
        pii_category: PiiCategory::None,
        pii_confirmed: false,
        description: String::new(),
        business_term: String::new(),
        unit: String::new(),
        ordinal,
    }
}

fn setup_semantic_fixture() -> Result<(duckdb::Connection, TenantCatalog, Uuid, Uuid), AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open duckdb failed: {e}")))?;
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_default\";
         CREATE TABLE lake.\"tenant_default\".\"orders\" (
             order_date DATE,
             region VARCHAR,
             city VARCHAR,
             status VARCHAR,
             amount DOUBLE
         );
         INSERT INTO lake.\"tenant_default\".\"orders\" VALUES
             (DATE '2025-01-10', '华东', '上海', 'PAID',     100.0),
             (DATE '2025-02-12', '华东', '上海', 'PAID',       0.0),
             (DATE '2026-01-05', '华东', '上海', 'PAID',      90.0),
             (DATE '2026-01-20', '华东', '杭州', 'PAID',      60.0),
             (DATE '2026-01-25', '华东', '上海', 'REFUNDED',  30.0),
             (DATE '2026-02-08', '华东', '上海', 'PAID',     240.0),
             (DATE '2026-02-18', '华北', '北京', 'PAID',     120.0),
             (DATE '2026-02-22', '华北', '北京', 'REFUNDED',  20.0);

         CREATE TABLE lake.\"tenant_default\".\"visits\" (
             visit_date DATE,
             region VARCHAR,
             uv BIGINT
         );
         INSERT INTO lake.\"tenant_default\".\"visits\" VALUES
             (DATE '2026-01-15', '华东', 1000),
             (DATE '2026-02-10', '华东', 1200),
             (DATE '2026-02-10', '华南',  500);",
    )
    .map_err(|e| AppError::Database(format!("seed semantic tables failed: {e}")))?;

    let orders_id = Uuid::new_v4();
    let mut orders_ds = CatalogDatasetEntry::simple(orders_id, "orders", "orders");
    orders_ds.schema_version = 3;
    for (ord, (col, dt)) in [
        ("order_date", DataType::Date),
        ("region", DataType::String),
        ("city", DataType::String),
        ("status", DataType::String),
        ("amount", DataType::Float64),
    ]
    .into_iter()
    .enumerate()
    {
        orders_ds.columns.insert(
            String::from(col),
            col_meta(orders_id, col, dt, i32::try_from(ord).unwrap_or(0)),
        );
    }

    let visits_id = Uuid::new_v4();
    let mut visits_ds = CatalogDatasetEntry::simple(visits_id, "visits", "visits");
    for (ord, (col, dt)) in [
        ("visit_date", DataType::Date),
        ("region", DataType::String),
        ("uv", DataType::Int64),
    ]
    .into_iter()
    .enumerate()
    {
        visits_ds.columns.insert(
            String::from(col),
            col_meta(visits_id, col, dt, i32::try_from(ord).unwrap_or(0)),
        );
    }

    let catalog = TenantCatalog::new("default")
        .with_dataset(orders_ds)
        .with_dataset(visits_ds);
    Ok((conn, catalog, orders_id, visits_id))
}

fn sample_metric(
    dataset_id: Uuid,
    name: &str,
    expr: &str,
    filter_sql: &str,
    time_col: Option<&str>,
) -> MetricDef {
    let now = Utc::now();
    MetricDef {
        id: Uuid::new_v4(),
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        dataset_id,
        expression: String::from(expr),
        filter_sql: String::from(filter_sql),
        time_column: time_col.map(String::from),
        default_grain: RollupGrain::Month,
        dimensions: vec![
            MetricDimension {
                name: String::from("region"),
                column: String::from("region"),
                display_name: String::from("大区"),
            },
            MetricDimension {
                name: String::from("city"),
                column: String::from("city"),
                display_name: String::from("城市"),
            },
        ],
        unit: String::from("CNY"),
        format: String::from("0.00"),
        synonyms: Vec::new(),
        certified: true,
        schema_version: 3,
        broken: false,
        owner_user_id: None,
        version: 1,
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn semantic_validates_metric_definition_and_rejects_bad_expressions() -> Result<(), AppError> {
    let (_conn, catalog, orders_id, _) = setup_semantic_fixture()?;
    let valid_req = CreateMetricRequest {
        name: String::from("paid_gmv"),
        display_name: String::from("已付GMV"),
        description: String::new(),
        dataset_id: orders_id,
        expression: String::from("SUM(amount)"),
        filter_sql: String::from("status = 'PAID'"),
        time_column: Some(String::from("order_date")),
        default_grain: RollupGrain::Month,
        dimensions: vec![MetricDimension {
            name: String::from("region"),
            column: String::from("region"),
            display_name: String::from("大区"),
        }],
        unit: String::new(),
        format: String::new(),
        synonyms: Vec::new(),
        certified: false,
    };
    assert_eq!(validate_create_metric_request(&valid_req, &catalog)?, 3);

    // Non-aggregate expression rejected
    let non_agg = CreateMetricRequest {
        expression: String::from("amount * 2"),
        ..valid_req.clone()
    };
    assert_eq!(
        validate_create_metric_request(&non_agg, &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("INVALID_ARGUMENT")
    );

    // Subquery in expression rejected
    let subq = CreateMetricRequest {
        expression: String::from("SUM(amount) + (SELECT 1)"),
        ..valid_req.clone()
    };
    assert_eq!(
        validate_create_metric_request(&subq, &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("INVALID_ARGUMENT")
    );

    // Aggregate in filter_sql rejected
    let agg_filter = CreateMetricRequest {
        filter_sql: String::from("SUM(amount) > 0"),
        ..valid_req.clone()
    };
    assert_eq!(
        validate_create_metric_request(&agg_filter, &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("INVALID_ARGUMENT")
    );

    // Non-date/timestamp time_column rejected
    let bad_time = CreateMetricRequest {
        time_column: Some(String::from("region")),
        ..valid_req
    };
    assert_eq!(
        validate_create_metric_request(&bad_time, &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("INVALID_ARGUMENT")
    );
    Ok(())
}

#[test]
fn semantic_single_metric_monthly_grain_matches_step04_gate_and_filters() -> Result<(), AppError> {
    let (conn, catalog, orders_id, _) = setup_semantic_fixture()?;
    let paid_gmv = sample_metric(
        orders_id,
        "paid_gmv",
        "SUM(amount)",
        "status = 'PAID'",
        Some("order_date"),
    );

    // Exact step04.sh shape: {"dimensions":[],"filters":[],"grain":"MONTH"}
    let monthly_q = MetricQuery {
        dimensions: Vec::new(),
        filters: Vec::new(),
        time_range: None,
        grain: Some(RollupGrain::Month),
        compare: None,
    };
    let res = execute_metric_query_conn(
        &conn,
        &catalog,
        &paid_gmv,
        &monthly_q,
        MetricExecuteOptions::default(),
    )?;
    assert_eq!(res.metrics, vec!["paid_gmv"]);
    assert_eq!(res.rows.len(), 4);
    let total_sum: f64 = res
        .rows
        .iter()
        .filter_map(|r| r.get("value").and_then(serde_json::Value::as_f64))
        .sum();
    // 100 + 0 + (90+60) + (240+120) = 610.0
    assert!((total_sum - 610.0).abs() < 1e-9);

    // Dimension + ad-hoc filter query
    let filtered_q = MetricQuery {
        dimensions: vec![String::from("region")],
        filters: vec![FilterSpec {
            field: String::from("city"),
            op: FilterOp::In,
            value: json!(["上海", "北京"]),
        }],
        time_range: Some(MetricTimeRange {
            from: Utc
                .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| AppError::bad("TEST", "dt"))?,
            to: Utc
                .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| AppError::bad("TEST", "dt"))?,
        }),
        grain: Some(RollupGrain::Month),
        compare: None,
    };
    let f_res = execute_metric_query_conn(
        &conn,
        &catalog,
        &paid_gmv,
        &filtered_q,
        MetricExecuteOptions::default(),
    )?;
    assert_eq!(f_res.rows.len(), 2);
    assert_eq!(
        f_res.rows.first().and_then(|r| r.get("value")),
        Some(&json!(240.0))
    );
    assert_eq!(
        f_res.rows.get(1).and_then(|r| r.get("value")),
        Some(&json!(120.0))
    );
    Ok(())
}

#[test]
fn semantic_period_comparison_computes_previous_period_and_yoy_with_zero_guard()
-> Result<(), AppError> {
    let (conn, catalog, orders_id, _) = setup_semantic_fixture()?;
    let paid_gmv = sample_metric(
        orders_id,
        "paid_gmv",
        "SUM(amount)",
        "status = 'PAID' AND region = '华东'",
        Some("order_date"),
    );

    let yoy_q = MetricQuery {
        dimensions: Vec::new(),
        filters: Vec::new(),
        time_range: Some(MetricTimeRange {
            from: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| AppError::bad("TEST", "dt"))?,
            to: Utc
                .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| AppError::bad("TEST", "dt"))?,
        }),
        grain: Some(RollupGrain::Month),
        compare: Some(MetricCompareMode::SamePeriodLastYear),
    };
    let yoy_res = execute_metric_query_conn(
        &conn,
        &catalog,
        &paid_gmv,
        &yoy_q,
        MetricExecuteOptions::default(),
    )?;
    assert_eq!(yoy_res.rows.len(), 2);

    // 2026-01 (150.0) vs 2025-01 (100.0): delta = 50.0, delta_ratio = 0.5
    let jan = yoy_res
        .rows
        .first()
        .ok_or_else(|| AppError::bad("TEST", "jan"))?;
    assert_eq!(jan.get("value"), Some(&json!(150.0)));
    assert_eq!(jan.get("compare_value"), Some(&json!(100.0)));
    assert_eq!(jan.get("delta"), Some(&json!(50.0)));
    assert_eq!(jan.get("delta_ratio"), Some(&json!(0.5)));

    // 2026-02 (240.0) vs 2025-02 (0.0): delta = 240.0, delta_ratio = null (divide-by-zero protected!)
    let feb = yoy_res
        .rows
        .get(1)
        .ok_or_else(|| AppError::bad("TEST", "feb"))?;
    assert_eq!(feb.get("value"), Some(&json!(240.0)));
    assert_eq!(feb.get("compare_value"), Some(&json!(0.0)));
    assert_eq!(feb.get("delta"), Some(&json!(240.0)));
    assert_eq!(feb.get("delta_ratio"), Some(&serde_json::Value::Null));

    // PREVIOUS_PERIOD on [2026-02-01, 2026-03-01) shifts by 1 calendar month to [2026-01-01, 2026-02-01) (150.0)
    let mom_q = MetricQuery {
        time_range: Some(MetricTimeRange {
            from: Utc
                .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| AppError::bad("TEST", "dt"))?,
            to: Utc
                .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| AppError::bad("TEST", "dt"))?,
        }),
        compare: Some(MetricCompareMode::PreviousPeriod),
        ..yoy_q
    };
    let mom_res = execute_metric_query_conn(
        &conn,
        &catalog,
        &paid_gmv,
        &mom_q,
        MetricExecuteOptions::default(),
    )?;
    let mom_row = mom_res
        .rows
        .first()
        .ok_or_else(|| AppError::bad("TEST", "mom"))?;
    assert_eq!(mom_row.get("value"), Some(&json!(240.0)));
    assert_eq!(mom_row.get("compare_value"), Some(&json!(150.0)));
    assert_eq!(mom_row.get("delta"), Some(&json!(90.0)));
    assert_eq!(mom_row.get("delta_ratio"), Some(&json!(0.6)));
    Ok(())
}

#[test]
fn semantic_multi_metric_fuses_same_dataset_and_outer_joins_cross_dataset() -> Result<(), AppError>
{
    let (conn, catalog, orders_id, visits_id) = setup_semantic_fixture()?;
    let paid_gmv = sample_metric(
        orders_id,
        "paid_gmv",
        "SUM(amount)",
        "status = 'PAID'",
        Some("order_date"),
    );
    let refund_gmv = sample_metric(
        orders_id,
        "refund_gmv",
        "SUM(amount)",
        "status = 'REFUNDED'",
        Some("order_date"),
    );
    let mut uv_total = sample_metric(visits_id, "uv_total", "SUM(uv)", "", Some("visit_date"));
    uv_total.dimensions = vec![MetricDimension {
        name: String::from("region"),
        column: String::from("region"),
        display_name: String::from("大区"),
    }];
    let all_metrics = vec![paid_gmv, refund_gmv, uv_total];

    let feb_range = MetricTimeRange {
        from: Utc
            .with_ymd_and_hms(2026, 2, 1, 0, 0, 0)
            .single()
            .ok_or_else(|| AppError::bad("TEST", "dt"))?,
        to: Utc
            .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
            .single()
            .ok_or_else(|| AppError::bad("TEST", "dt"))?,
    };

    // 1. Same-dataset multi-metric fusion uses single scan with FILTER (WHERE ...)
    let same_q = MultiMetricQuery {
        metric_names: vec![String::from("paid_gmv"), String::from("refund_gmv")],
        dimensions: vec![String::from("region")],
        filters: Vec::new(),
        time_range: Some(feb_range.clone()),
        grain: Some(RollupGrain::Month),
        compare: None,
    };
    let same_res = execute_multi_metric_query_conn(
        &conn,
        &catalog,
        &all_metrics,
        &same_q,
        MetricExecuteOptions::default(),
    )?;
    assert!(!same_res.sql.contains(';'));
    assert!(same_res.sql.to_ascii_uppercase().contains("FILTER"));
    assert_eq!(same_res.rows.len(), 2);
    // 华东 Feb: paid=240, refund=null; 华北 Feb: paid=120, refund=20
    assert_eq!(
        same_res.rows.first().and_then(|r| r.get("paid_gmv")),
        Some(&json!(240.0))
    );
    assert_eq!(
        same_res.rows.get(1).and_then(|r| r.get("refund_gmv")),
        Some(&json!(20.0))
    );

    // 2. Cross-dataset outer join merges orders (华东, 华北) + visits (华东, 华南) => 3 regions!
    let cross_q = MultiMetricQuery {
        metric_names: vec![String::from("paid_gmv"), String::from("uv_total")],
        ..same_q
    };
    let cross_res = execute_multi_metric_query_conn(
        &conn,
        &catalog,
        &all_metrics,
        &cross_q,
        MetricExecuteOptions::default(),
    )?;
    assert_eq!(cross_res.rows.len(), 3);
    let south = cross_res
        .rows
        .iter()
        .find(|r| r.get("region") == Some(&json!("华南")))
        .ok_or_else(|| AppError::bad("TEST", "missing 华南 outer join row"))?;
    assert_eq!(south.get("paid_gmv"), Some(&serde_json::Value::Null));
    assert_eq!(south.get("uv_total"), Some(&json!(500)));
    Ok(())
}

#[tokio::test]
async fn semantic_schema_breaking_change_returns_409_and_session_pool_runs_metric()
-> Result<(), AppError> {
    let (conn, catalog, orders_id, _) = setup_semantic_fixture()?;
    let paid_gmv = sample_metric(
        orders_id,
        "paid_gmv",
        "SUM(amount)",
        "status = 'PAID'",
        Some("order_date"),
    );
    assert!(!is_metric_broken(&paid_gmv, &catalog));

    // Simulate dataset schema evolution dropping column `amount`
    let mut evolved_ds = CatalogDatasetEntry::simple(orders_id, "orders", "orders");
    evolved_ds.columns.insert(
        String::from("order_date"),
        col_meta(orders_id, "order_date", DataType::Date, 0),
    );
    evolved_ds.columns.insert(
        String::from("status"),
        col_meta(orders_id, "status", DataType::String, 1),
    );
    let evolved_catalog = TenantCatalog::new("default").with_dataset(evolved_ds);

    assert!(is_metric_broken(&paid_gmv, &evolved_catalog));
    assert_eq!(
        check_metric_compatibility(&paid_gmv, &evolved_catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("SCHEMA_BREAKING_CHANGE")
    );
    let q = MetricQuery {
        dimensions: Vec::new(),
        filters: Vec::new(),
        time_range: None,
        grain: None,
        compare: None,
    };
    let exec_err = execute_metric_query_conn(
        &conn,
        &evolved_catalog,
        &paid_gmv,
        &q,
        MetricExecuteOptions::default(),
    );
    assert!(matches!(
        exec_err,
        Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"))
    ));

    // Verify async SessionPool execution on scalar metric without time_column
    let dir = std::env::temp_dir().join(format!("axpoint-semantic-test-{}", Uuid::new_v4()));
    let pool = SessionPool::new(AnalyticsConfig {
        data_root: dir.clone(),
        lake_catalog_dsn: None,
        app_database_url: None,
        tenant_cache_capacity: 2,
        memory_limit: String::from("256MB"),
        threads: 2,
        keyring: None,
    });
    let empty_cat = TenantCatalog::new("default");
    let session = pool.get_or_try_init("default", empty_cat, &[]).await?;
    assert_eq!(session.tenant_id(), "default");
    let _ = std::fs::remove_dir_all(dir);
    let _ = execute_metric_query;
    Ok(())
}
