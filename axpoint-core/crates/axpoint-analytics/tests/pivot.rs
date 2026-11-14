//! Integration tests for `axpoint_analytics::pivot` (`05-STEP04` §04.4).

use axpoint_analytics::pivot::{
    PivotExecuteOptions, compile_pivot, execute_pivot, execute_pivot_conn,
};
use axpoint_analytics::{
    AnalyticsConfig, CatalogDatasetEntry, FieldRef, FilterOp, FilterSpec, PivotRowKind, PivotSpec,
    SessionPool, SortDir, SortSpec, TenantCatalog, TimeBucket, ValueSpec,
};
use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{AggFn, DataType, PiiCategory, RollupGrain};
use serde_json::json;
use uuid::Uuid;

fn col_meta(
    dataset_id: Uuid,
    name: &str,
    data_type: DataType,
    pii_category: PiiCategory,
    ordinal: i32,
) -> ColumnMeta {
    ColumnMeta {
        dataset_id,
        column_name: String::from(name),
        data_type,
        pii_category,
        pii_confirmed: pii_category != PiiCategory::None,
        description: String::new(),
        business_term: String::new(),
        unit: String::new(),
        ordinal,
    }
}

fn build_sales_fixture() -> Result<(duckdb::Connection, TenantCatalog), AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open duckdb failed: {e}")))?;
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_default\";
         CREATE TABLE lake.\"tenant_default\".\"sales\" (
             region VARCHAR,
             city VARCHAR,
             month VARCHAR,
             order_date DATE,
             status VARCHAR,
             phone VARCHAR,
             amount DOUBLE
         );
         INSERT INTO lake.\"tenant_default\".\"sales\" VALUES
             -- 华东 / 上海 across 4 months (m1..m4)
             ('华东', '上海', '2026-01', DATE '2026-01-10', 'PAID', '13812345678', 100.0),
             ('华东', '上海', '2026-01', DATE '2026-01-20', 'PAID', '13812345678',  50.0),
             ('华东', '上海', '2026-02', DATE '2026-02-05', 'PAID', '13812345678', 200.0),
             ('华东', '上海', '2026-03', DATE '2026-03-11', 'PAID', '13812345678', 300.0),
             ('华东', '上海', '2026-04', DATE '2026-04-09', 'PAID', '13812345678', 400.0),
             -- 华东 / 杭州 across 4 months
             ('华东', '杭州', '2026-01', DATE '2026-01-15', 'PAID', '13987654321',  80.0),
             ('华东', '杭州', '2026-02', DATE '2026-02-18', 'PAID', '13987654321', 120.0),
             ('华东', '杭州', '2026-03', DATE '2026-03-22', 'PAID', '13987654321', 160.0),
             ('华东', '杭州', '2026-04', DATE '2026-04-25', 'PAID', '13987654321', 240.0),
             -- 华北 / 北京 across 4 months + one PENDING row filtered out when status='PAID'
             ('华北', '北京', '2026-01', DATE '2026-01-08', 'PAID', '13700001111',  60.0),
             ('华北', '北京', '2026-02', DATE '2026-02-12', 'PAID', '13700001111',  90.0),
             ('华北', '北京', '2026-02', DATE '2026-02-28', 'PAID', '13700001111',  30.0),
             ('华北', '北京', '2026-03', DATE '2026-03-14', 'PAID', '13700001111', 150.0),
             ('华北', '北京', '2026-04', DATE '2026-04-19', 'PAID', '13700001111', 210.0),
             ('华北', '北京', '2026-01', DATE '2026-01-29', 'PENDING', '13700001111', 999.0);",
    )
    .map_err(|e| AppError::Database(format!("seed sales failed: {e}")))?;

    let ds_id = Uuid::new_v4();
    let mut ds = CatalogDatasetEntry::simple(ds_id, "sales", "sales");
    for (ord, (col, dt, pii)) in [
        ("region", DataType::String, PiiCategory::None),
        ("city", DataType::String, PiiCategory::None),
        ("month", DataType::String, PiiCategory::None),
        ("order_date", DataType::Date, PiiCategory::None),
        ("status", DataType::String, PiiCategory::None),
        ("phone", DataType::String, PiiCategory::Phone),
        ("amount", DataType::Float64, PiiCategory::None),
    ]
    .into_iter()
    .enumerate()
    {
        let ord_i32 = i32::try_from(ord).unwrap_or(0);
        ds.columns
            .insert(String::from(col), col_meta(ds_id, col, dt, pii, ord_i32));
    }

    let catalog = TenantCatalog::new("default").with_dataset(ds);
    Ok((conn, catalog))
}

#[test]
fn pivot_3x4_matrix_verifies_every_cell_subtotals_and_grand_total() -> Result<(), AppError> {
    let (conn, catalog) = build_sales_fixture()?;

    let spec = PivotSpec {
        dataset: String::from("sales"),
        rows: vec![
            FieldRef {
                field: String::from("region"),
                label: None,
            },
            FieldRef {
                field: String::from("city"),
                label: None,
            },
        ],
        columns: vec![FieldRef {
            field: String::from("month"),
            label: None,
        }],
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Sum,
            alias: Some(String::from("金额")),
            format: Some(String::from("0.00")),
        }],
        filters: vec![FilterSpec {
            field: String::from("status"),
            op: FilterOp::Eq,
            value: json!("PAID"),
        }],
        time_bucket: None,
        sort: Vec::new(),
        subtotals: true,
        grand_total: true,
        row_limit: 1_000,
        column_limit: 50,
    };

    let res = execute_pivot_conn(&conn, &catalog, &spec, PivotExecuteOptions::default())?;
    assert!(!res.columns_truncated);
    assert!(res.sql.contains("lake."));

    assert_eq!(
        res.column_headers,
        vec![
            vec!["2026-01", "金额"],
            vec!["2026-02", "金额"],
            vec!["2026-03", "金额"],
            vec!["2026-04", "金额"],
        ]
    );

    // UTF-8 lexicographical order: '华东' (U+534E U+4E1C) < '华北' (U+534E U+5317),
    // '上海' (U+4E0A) < '杭州' (U+676D).
    assert_eq!(
        res.row_headers,
        vec![
            vec!["华东", "上海"],
            vec!["华东", "杭州"],
            vec!["华东", "小计"],
            vec!["华北", "北京"],
            vec!["华北", "小计"],
            vec!["总计", ""],
        ]
    );

    assert_eq!(
        res.row_kinds,
        vec![
            PivotRowKind::Data,
            PivotRowKind::Data,
            PivotRowKind::Subtotal,
            PivotRowKind::Data,
            PivotRowKind::Subtotal,
            PivotRowKind::GrandTotal,
        ]
    );

    assert_eq!(
        res.cells,
        vec![
            // 华东 / 上海: 100+50=150, 200, 300, 400
            vec!["150.00", "200.00", "300.00", "400.00"],
            // 华东 / 杭州: 80, 120, 160, 240
            vec!["80.00", "120.00", "160.00", "240.00"],
            // 华东 / 小计: 230, 320, 460, 640
            vec!["230.00", "320.00", "460.00", "640.00"],
            // 华北 / 北京: 60, 90+30=120, 150, 210
            vec!["60.00", "120.00", "150.00", "210.00"],
            // 华北 / 小计: 60, 120, 150, 210
            vec!["60.00", "120.00", "150.00", "210.00"],
            // 总计: 290, 440, 610, 850
            vec!["290.00", "440.00", "610.00", "850.00"],
        ]
    );

    Ok(())
}

#[test]
fn pivot_single_row_dim_no_col_dim_emits_grand_total_matching_step04_gate() -> Result<(), AppError>
{
    let (conn, catalog) = build_sales_fixture()?;

    // Exact shape used by step04.sh line 23:
    // rows:[{field:"status"}], columns:[], subtotals:true, grand_total:true
    let spec = PivotSpec {
        dataset: String::from("sales"),
        rows: vec![FieldRef {
            field: String::from("status"),
            label: None,
        }],
        columns: Vec::new(),
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Sum,
            alias: Some(String::from("total_amount")),
            format: Some(String::from("0.00")),
        }],
        filters: Vec::new(),
        time_bucket: None,
        sort: Vec::new(),
        subtotals: true,
        grand_total: true,
        row_limit: 1_000,
        column_limit: 50,
    };

    let res = execute_pivot_conn(&conn, &catalog, &spec, PivotExecuteOptions::default())?;
    assert_eq!(res.column_headers, vec![vec!["total_amount"]]);
    assert_eq!(
        res.row_headers,
        vec![vec!["PAID"], vec!["PENDING"], vec!["总计"]]
    );
    assert_eq!(
        res.row_kinds,
        vec![
            PivotRowKind::Data,
            PivotRowKind::Data,
            PivotRowKind::GrandTotal,
        ]
    );
    // PAID total = 290 + 440 + 610 + 850 = 2190.00; PENDING = 999.00; GRAND_TOTAL = 3189.00
    assert_eq!(
        res.cells,
        vec![vec!["2190.00"], vec!["999.00"], vec!["3189.00"]]
    );

    let serialized = serde_json::to_value(&res).map_err(|e| AppError::Database(e.to_string()))?;
    let kinds_json = serialized
        .get("row_kinds")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| AppError::bad("TEST_FAIL", "missing row_kinds"))?;
    assert!(kinds_json.contains(&json!("GRAND_TOTAL")));

    Ok(())
}

#[test]
fn pivot_column_limit_merges_sum_into_other_but_truncates_avg_without_false_merge()
-> Result<(), AppError> {
    let (conn, catalog) = build_sales_fixture()?;

    // Paid rows per month:
    // 2026-01: 4 rows (上海 x2, 杭州 x1, 北京 x1)
    // 2026-02: 4 rows (上海 x1, 杭州 x1, 北京 x2)
    // 2026-03: 3 rows
    // 2026-04: 3 rows
    // With column_limit = 2, top-2 months are 2026-01 and 2026-02; overflow is 2026-03 + 2026-04.
    let sum_spec = PivotSpec {
        dataset: String::from("sales"),
        rows: vec![FieldRef {
            field: String::from("region"),
            label: None,
        }],
        columns: vec![FieldRef {
            field: String::from("month"),
            label: None,
        }],
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Sum,
            alias: Some(String::from("金额")),
            format: Some(String::from("0.00")),
        }],
        filters: vec![FilterSpec {
            field: String::from("status"),
            op: FilterOp::Eq,
            value: json!("PAID"),
        }],
        time_bucket: None,
        sort: Vec::new(),
        subtotals: false,
        grand_total: true,
        row_limit: 100,
        column_limit: 2,
    };

    let sum_res = execute_pivot_conn(&conn, &catalog, &sum_spec, PivotExecuteOptions::default())?;
    assert!(sum_res.columns_truncated);
    assert_eq!(
        sum_res.column_headers,
        vec![
            vec!["2026-01", "金额"],
            vec!["2026-02", "金额"],
            vec!["其他", "金额"],
        ]
    );
    // 华东: 2026-03 (460) + 2026-04 (640) = 1100.00
    // 华北: 2026-03 (150) + 2026-04 (210) = 360.00
    // 总计: 610 + 850 = 1460.00
    assert_eq!(
        sum_res.cells,
        vec![
            vec!["230.00", "320.00", "1100.00"],
            vec!["60.00", "120.00", "360.00"],
            vec!["290.00", "440.00", "1460.00"],
        ]
    );

    // AVG is non-mergeable: must NOT emit an "其他" column and MUST set columns_truncated = true.
    let avg_spec = PivotSpec {
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Avg,
            alias: Some(String::from("均价")),
            format: Some(String::from("0.00")),
        }],
        ..sum_spec
    };
    let avg_res = execute_pivot_conn(&conn, &catalog, &avg_spec, PivotExecuteOptions::default())?;
    assert!(avg_res.columns_truncated);
    assert_eq!(
        avg_res.column_headers,
        vec![vec!["2026-01", "均价"], vec!["2026-02", "均价"]]
    );
    // 华东 2026-01: (100+50+80)/3 = 76.67; 2026-02: (200+120)/2 = 160.00
    // 华北 2026-01: 60/1 = 60.00; 2026-02: (90+30)/2 = 60.00
    // 总计 2026-01: 290/4 = 72.50; 2026-02: 440/4 = 110.00
    assert_eq!(
        avg_res.cells,
        vec![
            vec!["76.67", "160.00"],
            vec!["60.00", "60.00"],
            vec!["72.50", "110.00"],
        ]
    );

    Ok(())
}

#[test]
fn pivot_grouping_bits_distinguish_real_null_from_subtotals() -> Result<(), AppError> {
    let (conn, catalog) = build_sales_fixture()?;
    conn.execute_batch(
        "INSERT INTO lake.\"tenant_default\".\"sales\" VALUES
         ('华东', NULL, '2026-01', DATE '2026-01-25', 'PAID', '13511112222', 10.0);",
    )
    .map_err(|e| AppError::Database(e.to_string()))?;

    let spec = PivotSpec {
        dataset: String::from("sales"),
        rows: vec![
            FieldRef {
                field: String::from("region"),
                label: None,
            },
            FieldRef {
                field: String::from("city"),
                label: None,
            },
        ],
        columns: Vec::new(),
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Sum,
            alias: None,
            format: Some(String::from("0.00")),
        }],
        filters: vec![FilterSpec {
            field: String::from("region"),
            op: FilterOp::Eq,
            value: json!("华东"),
        }],
        time_bucket: None,
        sort: Vec::new(),
        subtotals: true,
        grand_total: false,
        row_limit: 100,
        column_limit: 50,
    };

    let res = execute_pivot_conn(&conn, &catalog, &spec, PivotExecuteOptions::default())?;
    assert_eq!(
        res.row_headers,
        vec![
            vec!["华东", "上海"],
            vec!["华东", "杭州"],
            vec!["华东", ""],
            vec!["华东", "小计"],
        ]
    );
    assert_eq!(
        res.row_kinds,
        vec![
            PivotRowKind::Data,
            PivotRowKind::Data,
            PivotRowKind::Data,
            PivotRowKind::Subtotal,
        ]
    );
    assert_eq!(
        res.cells,
        vec![
            vec!["1050.00"],
            vec!["600.00"],
            vec!["10.00"],
            vec!["1660.00"],
        ]
    );
    Ok(())
}

#[test]
fn pivot_masks_pii_dimensions_unless_reveal_pii_permitted() -> Result<(), AppError> {
    let (conn, catalog) = build_sales_fixture()?;
    let pii_spec = PivotSpec {
        dataset: String::from("sales"),
        rows: vec![FieldRef {
            field: String::from("phone"),
            label: None,
        }],
        columns: Vec::new(),
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Count,
            alias: None,
            format: None,
        }],
        filters: vec![FilterSpec {
            field: String::from("city"),
            op: FilterOp::Eq,
            value: json!("杭州"),
        }],
        time_bucket: None,
        sort: Vec::new(),
        subtotals: false,
        grand_total: false,
        row_limit: 10,
        column_limit: 10,
    };
    let masked_res =
        execute_pivot_conn(&conn, &catalog, &pii_spec, PivotExecuteOptions::default())?;
    assert_eq!(masked_res.row_headers, vec![vec!["139****4321"]]);

    let denied = execute_pivot_conn(
        &conn,
        &catalog,
        &pii_spec,
        PivotExecuteOptions {
            timeout_ms: None,
            reveal_pii: true,
            has_pii_raw_read: false,
        },
    );
    assert!(matches!(
        denied,
        Err(AppError::Forbidden("PERMISSION_DENIED"))
    ));

    let unmasked_res = execute_pivot_conn(
        &conn,
        &catalog,
        &pii_spec,
        PivotExecuteOptions {
            timeout_ms: None,
            reveal_pii: true,
            has_pii_raw_read: true,
        },
    )?;
    assert_eq!(unmasked_res.row_headers, vec![vec!["13987654321"]]);
    Ok(())
}

#[tokio::test]
async fn pivot_executes_via_session_pool_with_time_bucket() -> Result<(), AppError> {
    let dir = std::env::temp_dir().join(format!("axpoint-pivot-test-{}", Uuid::new_v4()));
    let lake_dir = dir.join("lake");
    std::fs::create_dir_all(&lake_dir).map_err(|e| AppError::Config(e.to_string()))?;
    let catalog_file = dir.join("lake_catalog.ducklake");

    {
        let seed =
            duckdb::Connection::open_in_memory().map_err(|e| AppError::Config(e.to_string()))?;
        let sql = format!(
            "LOAD ducklake;
             ATTACH 'ducklake:{}' AS lake_seed (DATA_PATH '{}/');
             CREATE SCHEMA IF NOT EXISTS lake_seed.\"tenant_default\";
             CREATE TABLE lake_seed.\"tenant_default\".\"events\" AS
                 SELECT DATE '2026-01-10' AS dt, 'A' AS grp, 1200.5 AS v
                 UNION ALL
                 SELECT DATE '2026-02-15' AS dt, 'A' AS grp, 800.0 AS v;
             DETACH lake_seed;",
            catalog_file.to_string_lossy(),
            lake_dir.to_string_lossy()
        );
        seed.execute_batch(&sql)
            .map_err(|e| AppError::Database(e.to_string()))?;
    }

    let ds_id = Uuid::new_v4();
    let mut ds = CatalogDatasetEntry::simple(ds_id, "events", "events");
    ds.columns.insert(
        String::from("dt"),
        col_meta(ds_id, "dt", DataType::Date, PiiCategory::None, 0),
    );
    ds.columns.insert(
        String::from("grp"),
        col_meta(ds_id, "grp", DataType::String, PiiCategory::None, 1),
    );
    ds.columns.insert(
        String::from("v"),
        col_meta(ds_id, "v", DataType::Float64, PiiCategory::None, 2),
    );
    let catalog = TenantCatalog::new("default").with_dataset(ds);

    let pool = SessionPool::new(AnalyticsConfig {
        data_root: dir.clone(),
        lake_catalog_dsn: None,
        app_database_url: None,
        tenant_cache_capacity: 2,
        memory_limit: String::from("256MB"),
        threads: 2,
        keyring: None,
    });
    let session = pool
        .get_or_try_init("default", catalog.clone(), &[])
        .await?;

    let spec = PivotSpec {
        dataset: String::from("events"),
        rows: vec![FieldRef {
            field: String::from("dt"),
            label: Some(String::from("月份")),
        }],
        columns: Vec::new(),
        values: vec![ValueSpec {
            field: String::from("v"),
            agg: AggFn::Sum,
            alias: Some(String::from("合计")),
            format: Some(String::from("0,0.00")),
        }],
        filters: vec![FilterSpec {
            field: String::from("grp"),
            op: FilterOp::In,
            value: json!(["A", "B"]),
        }],
        time_bucket: Some(TimeBucket {
            field: String::from("dt"),
            grain: RollupGrain::Month,
        }),
        sort: vec![SortSpec {
            field: String::from("dt"),
            dir: SortDir::Desc,
        }],
        subtotals: false,
        grand_total: true,
        row_limit: 100,
        column_limit: 50,
    };

    let res = execute_pivot(&session, &catalog, &spec, PivotExecuteOptions::default()).await?;
    assert_eq!(
        res.row_kinds,
        vec![
            PivotRowKind::Data,
            PivotRowKind::Data,
            PivotRowKind::GrandTotal
        ]
    );
    assert_eq!(
        res.cells,
        vec![vec!["800.00"], vec!["1,200.50"], vec!["2,000.50"]]
    );
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

#[test]
fn pivot_validates_dimension_bounds_and_missing_dataset() -> Result<(), AppError> {
    let (_conn, catalog) = build_sales_fixture()?;
    let base = PivotSpec {
        dataset: String::from("sales"),
        rows: (0..6)
            .map(|_| FieldRef {
                field: String::from("region"),
                label: None,
            })
            .collect(),
        columns: Vec::new(),
        values: vec![ValueSpec {
            field: String::from("amount"),
            agg: AggFn::Sum,
            alias: None,
            format: None,
        }],
        filters: Vec::new(),
        time_bucket: None,
        sort: Vec::new(),
        subtotals: false,
        grand_total: false,
        row_limit: 100,
        column_limit: 50,
    };
    assert_eq!(
        compile_pivot(&base, &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("INVALID_ARGUMENT")
    );

    let missing_ds = PivotSpec {
        dataset: String::from("nonexistent"),
        rows: Vec::new(),
        ..base
    };
    assert_eq!(
        compile_pivot(&missing_ds, &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("DATASET_NOT_FOUND")
    );
    Ok(())
}
