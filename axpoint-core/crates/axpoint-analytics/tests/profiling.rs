//! Integration tests for `TenantDb::run_internal`, `SessionPool::run_internal`,
//! deterministic `DuckDB` dataset profiling (`06-STEP05-06` §05.5.1), and dry-run mapping preview (§05.7).

use std::collections::BTreeMap;

use axpoint_analytics::{
    AnalyticsConfig, SessionPool, TenantDb, format_tenant_lake_table, preview_mapping_rows,
    preview_mapping_rows_conn, profile_datasets, profile_datasets_conn,
};
use axpoint_domain::AppError;
use axpoint_domain::axhub::{Dataset, DatasetStatus, SchemaPolicy};
use axpoint_domain::enums::{Cardinality, DataType, PiiCategory, SourceKind, SyncMode};
use axpoint_domain::graph::GeoPoint;
use axpoint_domain::studio::{
    ColumnRoleTag, LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode, MappingSpec,
    NodeTypeSpec, OnSourceDelete, PropertyKind, PropertySpec,
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

fn make_dataset(name: &str, display_name: &str, tenant_id: &str) -> Dataset {
    let now = Utc::now();
    Dataset {
        id: Uuid::new_v4(),
        connection_id: None,
        name: String::from(name),
        display_name: String::from(display_name),
        description: String::new(),
        source_kind: SourceKind::FileTable,
        source_ref: json!({}),
        sync_mode: SyncMode::Snapshot,
        primary_key: Vec::new(),
        cursor_column: None,
        cursor_value: None,
        delete_column: None,
        schema_policy: SchemaPolicy::Additive,
        lake_schema: format!("tenant_{tenant_id}"),
        lake_table: String::from(name),
        schema_json: Vec::new(),
        schema_version: 1,
        row_count: 0,
        last_snapshot_id: None,
        last_synced_at: None,
        schedule_cron: None,
        tags: Vec::new(),
        owner_user_id: None,
        status: DatasetStatus::Ready,
        created_at: now,
        updated_at: now,
    }
}

fn make_prop(
    name: &str,
    data_type: DataType,
    kind: PropertyKind,
    required: bool,
    allowed_values: Option<Vec<String>>,
) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        data_type,
        kind,
        required,
        unit: None,
        pii_category: PiiCategory::None,
        allowed_values,
        description: String::new(),
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
    }
}

fn seed_supply_chain_in_memory() -> Result<TenantDb, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open in-memory duckdb: {e}")))?;

    conn.execute_batch(
        "CREATE TABLE demo_factories (
            title VARCHAR,
            factory_id VARCHAR,
            region VARCHAR,
            daily_output DOUBLE,
            established_at TIMESTAMPTZ
        );
        INSERT INTO demo_factories VALUES
            ('苏州一号工厂', 'FAC-01', 'EAST', 1200.0, TIMESTAMPTZ '2024-03-15 08:00:00+08'),
            ('深圳二号工厂', 'FAC-02', 'SOUTH', 1500.0, TIMESTAMPTZ '2024-06-20 09:30:00+08'),
            ('天津三号工厂', 'FAC-03', 'NORTH', 950.0, TIMESTAMPTZ '2025-01-10 10:15:00+08');

        CREATE TABLE demo_warehouses (
            warehouse_name VARCHAR,
            warehouse_code VARCHAR,
            factory_id VARCHAR,
            tier VARCHAR,
            inventory DOUBLE,
            lng DOUBLE,
            lat DOUBLE,
            updated_at TIMESTAMPTZ
        );
        INSERT INTO demo_warehouses VALUES
            ('华东中心仓', 'WH-01', 'FAC-01', 'CORE', 5000.0, 121.4737, 31.2304, TIMESTAMPTZ '2026-01-05 12:00:00+00'),
            ('华南枢纽仓', 'WH-02', 'FAC-02', 'CORE', 4200.0, 114.0579, 22.5431, TIMESTAMPTZ '2026-01-06 12:00:00+00'),
            ('华北区域仓', 'WH-03', 'FAC-03', 'REGIONAL', 3100.0, 117.2009, 39.0842, TIMESTAMPTZ '2026-01-07 12:00:00+00');

        CREATE TABLE demo_orders (
            order_id VARCHAR,
            warehouse_code VARCHAR,
            customer_name VARCHAR,
            status VARCHAR,
            priority_rank INTEGER,
            amount VARCHAR,
            lng DOUBLE,
            lat DOUBLE,
            ordered_at TIMESTAMPTZ
        );
        INSERT INTO demo_orders VALUES
            ('ORD-1001', 'WH-01', '阿尔法制造', 'PAID', 1, '1250.50', 121.5000, 31.2400, TIMESTAMPTZ '2026-02-01 09:00:00+00'),
            ('ORD-1002', 'WH-01', '贝塔电子', 'PAID', 2, '880.00', 121.4800, 31.2200, TIMESTAMPTZ '2026-02-01 10:00:00+00'),
            ('ORD-1003', 'WH-02', '伽马精密', 'OPEN', 1, '3400.25', 114.0600, 22.5500, TIMESTAMPTZ '2026-02-02 11:00:00+00'),
            ('ORD-1004', 'WH-02', '德尔塔科技', 'PAID', 3, '620.00', NULL, NULL, TIMESTAMPTZ '2026-02-02 15:30:00+00'),
            ('ORD-1005', 'WH-03', '艾普西隆光学', 'SHIPPED', 2, '1999.00', 117.2100, 39.0900, TIMESTAMPTZ '2026-02-03 08:45:00+00'),
            ('ORD-1006', 'WH-01', '泽塔汽车', 'OPEN', 1, '450.75', 121.4900, 31.2350, TIMESTAMPTZ '2026-02-03 14:20:00+00'),
            ('ORD-1007', 'WH-02', '伊塔半导体', 'PAID', 2, '5120.00', 114.0700, 22.5600, TIMESTAMPTZ '2026-02-04 09:10:00+00'),
            ('ORD-1008', 'WH-03', '西塔新材', 'PAID', 1, '760.00', 117.2200, 39.0800, TIMESTAMPTZ '2026-02-04 16:00:00+00'),
            ('ORD-1009', 'WH-01', '约塔机电', 'CANCELLED', 3, '300.00', 121.4700, 31.2300, TIMESTAMPTZ '2026-02-05 11:15:00+00'),
            ('ORD-1010', NULL, '卡帕能源', 'OPEN', 2, '910.00', 120.1551, 30.2741, TIMESTAMPTZ '2026-02-05 17:00:00+00');"
    )
    .map_err(|e| AppError::Database(format!("seed supply chain tables: {e}")))?;

    TenantDb::from_connection("default", conn)
}

#[test]
fn run_internal_returns_accurate_rows_truncation_and_column_types() -> Result<(), AppError> {
    let db = seed_supply_chain_in_memory()?;

    let qr = db.run_internal(
        "SELECT factory_id, daily_output, established_at FROM demo_factories ORDER BY factory_id",
        2,
    )?;
    assert_eq!(qr.row_count, 2);
    assert!(qr.truncated);
    assert_eq!(qr.columns.len(), 3);

    let [c0, c1, c2] = qr.columns.as_slice() else {
        return Err(AppError::bad("TEST_FAIL", "expected 3 columns"));
    };
    assert_eq!(c0.name, "factory_id");
    assert_eq!(c0.data_type, DataType::String);
    assert_eq!(c1.name, "daily_output");
    assert_eq!(c1.data_type, DataType::Float64);
    assert_eq!(c2.name, "established_at");
    assert_eq!(c2.data_type, DataType::Timestamp);

    let full = db.run_internal("SELECT * FROM demo_factories", 10)?;
    assert_eq!(full.row_count, 3);
    assert!(!full.truncated);

    assert_eq!(
        format_tenant_lake_table("acme-corp", "demo_orders")?,
        "lake.\"tenant_acme_corp\".\"demo_orders\""
    );
    Ok(())
}

#[test]
fn profile_datasets_conn_infers_pk_priority_roles_timestamptz_and_fks() -> Result<(), AppError> {
    let db = seed_supply_chain_in_memory()?;

    let ds_factories = make_dataset("demo_factories", "工厂台账", "default");
    let ds_warehouses = make_dataset("demo_warehouses", "仓库台账", "default");
    let ds_orders = make_dataset("demo_orders", "销售订单", "default");

    let bundle = profile_datasets_conn(&db, "default", &[ds_factories, ds_warehouses, ds_orders])?;
    assert_eq!(bundle.datasets.len(), 3);

    // 1. Verify PK priority ordering: even though `title` / `warehouse_name` are placed BEFORE
    // `factory_id` / `warehouse_code` and also unique in the 3-row tables, `*_id` and `*_code`
    // matching the entity stem must rank first!
    let Some(fac_prof) = bundle
        .datasets
        .iter()
        .find(|d| d.dataset_name == "demo_factories")
    else {
        return Err(AppError::bad("TEST_FAIL", "missing demo_factories profile"));
    };
    assert_eq!(fac_prof.row_count, 3);
    assert_eq!(
        fac_prof.pk_candidates.first().map(String::as_str),
        Some("factory_id")
    );

    // Check TIMESTAMPTZ column profiled cleanly without `icu` extension
    let Some(est_col) = fac_prof.columns.iter().find(|c| c.name == "established_at") else {
        return Err(AppError::bad("TEST_FAIL", "missing established_at column"));
    };
    assert_eq!(est_col.data_type, DataType::Timestamp);
    assert!(est_col.roles.contains(&ColumnRoleTag::Temporal));
    assert_eq!(est_col.sample_values.len(), 3);

    let Some(wh_prof) = bundle
        .datasets
        .iter()
        .find(|d| d.dataset_name == "demo_warehouses")
    else {
        return Err(AppError::bad(
            "TEST_FAIL",
            "missing demo_warehouses profile",
        ));
    };
    assert_eq!(
        wh_prof.pk_candidates.first().map(String::as_str),
        Some("warehouse_code")
    );
    let Some(inv_col) = wh_prof.columns.iter().find(|c| c.name == "inventory") else {
        return Err(AppError::bad("TEST_FAIL", "missing inventory column"));
    };
    assert!(inv_col.roles.contains(&ColumnRoleTag::NumericMetric));
    let Some(tier_col) = wh_prof.columns.iter().find(|c| c.name == "tier") else {
        return Err(AppError::bad("TEST_FAIL", "missing tier column"));
    };
    assert!(tier_col.roles.contains(&ColumnRoleTag::Categorical));

    let Some(ord_prof) = bundle
        .datasets
        .iter()
        .find(|d| d.dataset_name == "demo_orders")
    else {
        return Err(AppError::bad("TEST_FAIL", "missing demo_orders profile"));
    };
    assert_eq!(ord_prof.row_count, 10);
    assert_eq!(
        ord_prof.pk_candidates.first().map(String::as_str),
        Some("order_id")
    );
    let Some(status_col) = ord_prof.columns.iter().find(|c| c.name == "status") else {
        return Err(AppError::bad("TEST_FAIL", "missing status column"));
    };
    assert!(status_col.roles.contains(&ColumnRoleTag::Categorical));
    assert!(status_col.sample_values.len() <= 8);

    // Check nullable `warehouse_code` on `demo_orders` (1 null out of 10 -> 0.1)
    let Some(wh_fk_col) = ord_prof.columns.iter().find(|c| c.name == "warehouse_code") else {
        return Err(AppError::bad("TEST_FAIL", "missing warehouse_code column"));
    };
    assert!((wh_fk_col.null_rate - 0.1).abs() < 1e-6);

    // 2. Verify FK inference:
    // - demo_orders.warehouse_code -> demo_warehouses.warehouse_code (3 distinct over 9 non-null rows -> MANY_TO_ONE, overlap_ratio = 1.0 >= 0.9)
    let Some(ord_to_wh) = bundle.fk_candidates.iter().find(|fk| {
        fk.src_dataset == "demo_orders"
            && fk.src_column == "warehouse_code"
            && fk.dst_dataset == "demo_warehouses"
    }) else {
        return Err(AppError::bad(
            "TEST_FAIL",
            format!(
                "missing FK demo_orders -> demo_warehouses, got: {:?}",
                bundle.fk_candidates
            ),
        ));
    };
    assert_eq!(ord_to_wh.dst_pk_column, "warehouse_code");
    assert!(ord_to_wh.overlap_ratio >= 0.9);
    assert_eq!(ord_to_wh.cardinality, Cardinality::ManyToOne);

    // - demo_warehouses.factory_id -> demo_factories.factory_id (3 distinct over 3 non-null rows -> ONE_TO_ONE, overlap_ratio = 1.0 >= 0.9)
    let Some(wh_to_fac) = bundle.fk_candidates.iter().find(|fk| {
        fk.src_dataset == "demo_warehouses"
            && fk.src_column == "factory_id"
            && fk.dst_dataset == "demo_factories"
    }) else {
        return Err(AppError::bad(
            "TEST_FAIL",
            format!(
                "missing FK demo_warehouses -> demo_factories, got: {:?}",
                bundle.fk_candidates
            ),
        ));
    };
    assert_eq!(wh_to_fac.dst_pk_column, "factory_id");
    assert!(wh_to_fac.overlap_ratio >= 0.9);
    assert_eq!(wh_to_fac.cardinality, Cardinality::OneToOne);

    Ok(())
}

fn seed_dirty_orders_db() -> Result<TenantDb, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open in-memory duckdb: {e}")))?;

    conn.execute_batch(
        "CREATE TABLE dirty_orders (
            order_id VARCHAR,
            warehouse_code VARCHAR,
            customer_name VARCHAR,
            status VARCHAR,
            priority_rank VARCHAR,
            amount VARCHAR,
            lng DOUBLE,
            lat DOUBLE,
            ordered_at TIMESTAMPTZ
        );
        INSERT INTO dirty_orders VALUES
            ('ORD-2001', 'WH-01', '华东机电', 'PAID', '1', '1580.50', 121.4737, 31.2304, TIMESTAMPTZ '2026-03-01 08:00:00+00'),
            ('ORD-2002', NULL, '华南电子', 'OPEN', '2', '640.00', NULL, NULL, TIMESTAMPTZ '2026-03-01 09:00:00+00'),
            (NULL, 'WH-01', '缺失主键行', 'PAID', '1', '100.00', 121.0, 31.0, TIMESTAMPTZ '2026-03-01 10:00:00+00'),
            ('   ', 'WH-01', '空白主键行', 'PAID', '1', '100.00', 121.0, 31.0, TIMESTAMPTZ '2026-03-01 10:00:00+00'),
            ('ORD-BAD-NUM', 'WH-02', '非法金额行', 'PAID', '1', 'NOT_A_NUMBER', 114.0, 22.5, TIMESTAMPTZ '2026-03-01 11:00:00+00'),
            ('ORD-BAD-REQ', 'WH-02', NULL, 'PAID', '1', '200.00', 114.0, 22.5, TIMESTAMPTZ '2026-03-01 12:00:00+00'),
            ('ORD-BAD-GEO', 'WH-02', '越界经纬度', 'PAID', '1', '200.00', 199.9, 22.5, TIMESTAMPTZ '2026-03-01 12:30:00+00');"
    )
    .map_err(|e| AppError::Database(format!("seed dirty_orders: {e}")))?;

    for idx in 0..20_u32 {
        let sql = format!(
            "INSERT INTO dirty_orders VALUES ('ORD-NAN-{idx}', 'WH-01', '脏指标样本', 'PAID', '1', 'NaN', 121.0, 31.0, TIMESTAMPTZ '2026-03-02 00:00:00+00');"
        );
        conn.execute_batch(&sql)
            .map_err(|e| AppError::Database(format!("seed nan row: {e}")))?;
    }

    TenantDb::from_connection("default", conn)
}

fn build_sales_order_mapping_fixture() -> (NodeTypeSpec, MappingSpec) {
    let node_type = NodeTypeSpec {
        id: None,
        name: String::from("SalesOrder"),
        display_name: String::from("销售订单"),
        description: String::new(),
        icon: String::from("receipt"),
        color: String::from("#2563EB"),
        title_template: String::from("订单 {node_key}"),
        properties: vec![
            make_prop(
                "customer_name",
                DataType::String,
                PropertyKind::Attribute,
                true,
                None,
            ),
            make_prop(
                "status",
                DataType::String,
                PropertyKind::Attribute,
                true,
                Some(vec![String::from("PAID"), String::from("OPEN")]),
            ),
            make_prop(
                "priority_rank",
                DataType::Int64,
                PropertyKind::Attribute,
                false,
                None,
            ),
            make_prop(
                "amount",
                DataType::Float64,
                PropertyKind::Metric,
                true,
                None,
            ),
        ],
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: true,
        pos_x: 0.0,
        pos_y: 0.0,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        semantic_layer_tier: None,
        domain_group: None,
        actions: Vec::new(),
    };

    let mut attributes = BTreeMap::new();
    attributes.insert(String::from("customer_name"), String::from("customer_name"));
    attributes.insert(String::from("status"), String::from("status"));
    attributes.insert(String::from("priority_rank"), String::from("priority_rank"));

    let mut metrics = BTreeMap::new();
    metrics.insert(String::from("amount"), String::from("amount"));

    let spec = MappingSpec {
        name: String::from("orders_to_sales_order"),
        dataset: String::from("dirty_orders"),
        node_type: String::from("SalesOrder"),
        key_column: String::from("order_id"),
        title: String::from("订单 {order_id} ({customer_name})"),
        filter: None,
        attributes,
        metrics,
        metric_time_column: Some(String::from("ordered_at")),
        geo: MappingGeoSpec {
            lng_column: Some(String::from("lng")),
            lat_column: Some(String::from("lat")),
        },
        links: vec![MappingLinkSpec {
            link_type: String::from("FULFILLS"),
            direction: LinkDirection::In,
            target_type: String::from("Warehouse"),
            target_key_column: String::from("warehouse_code"),
            weight_column: None,
            create_missing_target: true,
        }],
        on_source_delete: OnSourceDelete::Keep,
        mode: MappingMode::Incremental,
    };
    (node_type, spec)
}

#[test]
fn preview_mapping_rows_conn_coerces_valid_rows_and_caps_reject_samples_at_20()
-> Result<(), AppError> {
    let db = seed_dirty_orders_db()?;
    let dataset = make_dataset("dirty_orders", "测试订单", "default");
    let (node_type, spec) = build_sales_order_mapping_fixture();

    let mapping_id = Uuid::new_v4();
    let preview =
        preview_mapping_rows_conn(&db, "default", mapping_id, &dataset, &node_type, &spec, 50)?;

    assert_eq!(preview.mapping_id, mapping_id);
    assert_eq!(preview.sampled_rows, 27);
    assert_eq!(preview.items.len(), 2);
    assert_eq!(preview.rows_rejected, 25);
    assert_eq!(preview.reject_samples.len(), 20);

    let [first, second] = preview.items.as_slice() else {
        return Err(AppError::bad("TEST_FAIL", "expected 2 valid preview items"));
    };
    assert_eq!(first.node_key, "ORD-2001");
    assert_eq!(first.title, "订单 ORD-2001 (华东机电)");
    assert_eq!(
        first.attributes.get("customer_name"),
        Some(&json!("华东机电"))
    );
    assert_eq!(first.attributes.get("status"), Some(&json!("PAID")));
    assert_eq!(first.attributes.get("priority_rank"), Some(&json!(1)));
    let Some(&amt1) = first.metrics.get("amount") else {
        return Err(AppError::bad("TEST_FAIL", "missing amount metric"));
    };
    assert!((amt1 - 1580.50).abs() < 1e-6);
    assert_eq!(
        first.geo,
        Some(GeoPoint {
            lng: 121.4737,
            lat: 31.2304
        })
    );
    assert_eq!(first.links.len(), 1);
    assert_eq!(
        first.links.first(),
        Some(&json!({
            "link_type": "FULFILLS",
            "direction": "IN",
            "target_type": "Warehouse",
            "target_key": "WH-01",
            "create_missing_target": true
        }))
    );

    assert_eq!(second.node_key, "ORD-2002");
    assert_eq!(second.title, "订单 ORD-2002 (华南电子)");
    assert_eq!(second.geo, None);
    assert_eq!(second.links.len(), 0);

    Ok(())
}

fn seed_ducklake_root() -> Result<std::path::PathBuf, AppError> {
    let root = std::env::temp_dir().join(format!("axpoint-profiling-lake-{}", Uuid::new_v4()));
    let lake_dir = root.join("lake");
    std::fs::create_dir_all(&lake_dir).map_err(|e| AppError::Config(e.to_string()))?;
    let catalog_path = root.join("lake_catalog.ducklake");

    let seed_conn =
        duckdb::Connection::open_in_memory().map_err(|e| AppError::Config(e.to_string()))?;
    let batch = format!(
        "LOAD ducklake;
         ATTACH 'ducklake:{}' AS lake_seed (DATA_PATH '{}/');
         CREATE SCHEMA IF NOT EXISTS lake_seed.\"tenant_default\";
         CREATE TABLE lake_seed.\"tenant_default\".\"demo_warehouses\" AS
             SELECT 'WH-01' AS warehouse_code, '上海仓' AS title, 900.0 AS inventory, TIMESTAMPTZ '2026-03-01 00:00:00+00' AS updated_at
             UNION ALL
             SELECT 'WH-02' AS warehouse_code, '深圳仓' AS title, 600.0 AS inventory, TIMESTAMPTZ '2026-03-02 00:00:00+00' AS updated_at;
         CREATE TABLE lake_seed.\"tenant_default\".\"demo_orders\" AS
             SELECT 'O-1' AS order_id, 'WH-01' AS warehouse_code, 'PAID' AS status, 300.0 AS amount
             UNION ALL
             SELECT 'O-2' AS order_id, 'WH-02' AS warehouse_code, 'PAID' AS status, 500.0 AS amount
             UNION ALL
             SELECT 'O-3' AS order_id, 'WH-01' AS warehouse_code, 'TEST' AS status, 999.0 AS amount;
         DETACH lake_seed;",
        catalog_path.to_string_lossy(),
        lake_dir.to_string_lossy()
    );
    seed_conn
        .execute_batch(&batch)
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(root)
}

#[tokio::test]
async fn session_pool_ducklake_profile_and_preview_end_to_end() -> Result<(), AppError> {
    let root = seed_ducklake_root()?;
    let pool = SessionPool::new(AnalyticsConfig {
        data_root: root.clone(),
        lake_catalog_dsn: None,
        app_database_url: None,
        tenant_cache_capacity: 2,
        memory_limit: String::from("256MB"),
        threads: 2,
        keyring: None,
    });

    let qr = pool
        .run_internal(
            "default",
            "SELECT COUNT(*) AS cnt FROM lake.\"tenant_default\".\"demo_orders\"",
            10,
        )
        .await?;
    assert_eq!(qr.rows.first().and_then(|r| r.get("cnt")), Some(&json!(3)));

    let ds_wh = make_dataset("demo_warehouses", "仓库", "default");
    let ds_ord = make_dataset("demo_orders", "订单", "default");
    let bundle = profile_datasets(&pool, "default", &[ds_wh, ds_ord.clone()]).await?;
    assert_eq!(bundle.datasets.len(), 2);
    assert_eq!(bundle.fk_candidates.len(), 1);
    assert_eq!(
        bundle.fk_candidates.first().map(|f| f.cardinality),
        Some(Cardinality::ManyToOne)
    );

    let node_type = NodeTypeSpec {
        id: None,
        name: String::from("Order"),
        display_name: String::from("订单"),
        description: String::new(),
        icon: String::new(),
        color: String::new(),
        title_template: String::from("{node_key}"),
        properties: vec![
            make_prop(
                "status",
                DataType::String,
                PropertyKind::Attribute,
                true,
                None,
            ),
            make_prop(
                "amount",
                DataType::Float64,
                PropertyKind::Metric,
                true,
                None,
            ),
        ],
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: true,
        pos_x: 0.0,
        pos_y: 0.0,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        semantic_layer_tier: None,
        domain_group: None,
        actions: Vec::new(),
    };
    let mut attrs = BTreeMap::new();
    attrs.insert(String::from("status"), String::from("status"));
    let mut mets = BTreeMap::new();
    mets.insert(String::from("amount"), String::from("amount"));

    let spec = MappingSpec {
        name: String::from("map_orders"),
        dataset: String::from("demo_orders"),
        node_type: String::from("Order"),
        key_column: String::from("order_id"),
        title: String::from("订单 {order_id}"),
        filter: Some(String::from("status != 'TEST'")),
        attributes: attrs,
        metrics: mets,
        metric_time_column: None,
        geo: MappingGeoSpec::default(),
        links: Vec::new(),
        on_source_delete: OnSourceDelete::Keep,
        mode: MappingMode::Incremental,
    };

    let preview = preview_mapping_rows(
        &pool,
        "default",
        Uuid::new_v4(),
        &ds_ord,
        &node_type,
        &spec,
        50,
    )
    .await?;
    assert_eq!(preview.sampled_rows, 2);
    assert_eq!(preview.items.len(), 2);
    assert_eq!(preview.rows_rejected, 0);

    std::fs::remove_dir_all(root).map_err(|e| AppError::Config(e.to_string()))?;
    Ok(())
}
