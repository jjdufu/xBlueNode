//! Comprehensive table-driven AST security, identifier rewriting, parameter binding,
//! and PII lineage test suite for `axpoint_analytics::sql_guard` (`05-STEP04` §04.3).

use std::collections::BTreeMap;
use std::sync::Arc;

use axpoint_analytics::arrow::array::{RecordBatch, StringArray};
use axpoint_analytics::arrow::datatypes::{DataType as ArrowDataType, Field, Schema};
use axpoint_analytics::{
    CatalogDatasetEntry, DEFAULT_MAX_ROWS, GuardOptions, HARD_MAX_ROWS, MAX_SQL_BYTES,
    OutputColumnOrigin, ResultSerializeOptions, TenantCatalog, batches_to_query_result, guard,
    guard_and_rewrite, guard_for_materialize,
};
use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, PiiCategory};
use serde_json::json;
use uuid::Uuid;

fn make_col(
    dataset_id: Uuid,
    name: &str,
    data_type: DataType,
    pii: PiiCategory,
    ordinal: i32,
) -> ColumnMeta {
    ColumnMeta {
        dataset_id,
        column_name: String::from(name),
        data_type,
        pii_category: pii,
        pii_confirmed: pii != PiiCategory::None,
        description: String::new(),
        business_term: String::new(),
        unit: String::new(),
        ordinal,
    }
}

fn demo_catalog() -> TenantCatalog {
    let orders_id = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
    let customers_id = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

    let mut orders = CatalogDatasetEntry::simple(orders_id, "demo_orders", "demo_orders");
    for (idx, (col, dt, pii)) in [
        ("id", DataType::Int64, PiiCategory::None),
        ("customer_id", DataType::Int64, PiiCategory::None),
        ("status", DataType::String, PiiCategory::None),
        ("amount", DataType::Decimal, PiiCategory::None),
        ("phone", DataType::String, PiiCategory::Phone),
        ("email", DataType::String, PiiCategory::Email),
        ("created_at", DataType::Timestamp, PiiCategory::None),
    ]
    .into_iter()
    .enumerate()
    {
        let ord = i32::try_from(idx).unwrap_or(0);
        orders
            .columns
            .insert(String::from(col), make_col(orders_id, col, dt, pii, ord));
    }

    let mut customers = CatalogDatasetEntry::simple(customers_id, "customers", "customers_lake");
    for (idx, (col, dt, pii)) in [
        ("id", DataType::Int64, PiiCategory::None),
        ("full_name", DataType::String, PiiCategory::Name),
        ("id_card", DataType::String, PiiCategory::IdCard),
        ("region", DataType::String, PiiCategory::None),
    ]
    .into_iter()
    .enumerate()
    {
        let ord = i32::try_from(idx).unwrap_or(0);
        customers
            .columns
            .insert(String::from(col), make_col(customers_id, col, dt, pii, ord));
    }

    TenantCatalog::new("default")
        .with_dataset(orders)
        .with_dataset(customers)
        .with_live_connection("erp")
}

fn assert_sql_rejected(sql: &str, catalog: &TenantCatalog, opts: &GuardOptions) {
    let res = guard(sql, catalog, opts);
    assert!(
        matches!(&res, Err(err) if err.error_code() == "SQL_REJECTED"),
        "expected SQL_REJECTED for SQL `{sql}`, got: {res:?}"
    );
}

const REJECTED_CASES: &[(&str, &str)] = &[
    // 1. Empty & multi-statement attacks
    ("empty", ""),
    ("whitespace-only", "   \n\t  "),
    ("multi-drop", "SELECT 1; DROP TABLE demo_orders"),
    ("multi-select", "SELECT 1; SELECT 2"),
    ("multi-double-semicolon", "SELECT 1;;"),
    (
        "multi-comment-injected",
        "SELECT 1; /* comment */ COPY demo_orders TO '/tmp/x'",
    ),
    // 2. Non-SELECT DDL/DML/Admin statements (with case, comment, quote variants)
    ("copy-out", "COPY demo_orders TO '/tmp/x.csv'"),
    (
        "copy-lower",
        "/* export */ copy \"demo_orders\" to '/tmp/x.parquet'",
    ),
    ("copy-from", "COPY demo_orders FROM '/etc/passwd'"),
    ("insert", "INSERT INTO demo_orders VALUES (1)"),
    (
        "insert-select",
        "insert into \"demo_orders\" select * from demo_orders",
    ),
    ("update", "UPDATE demo_orders SET status = 'PAID'"),
    ("delete", "DELETE FROM demo_orders WHERE 1 = 1"),
    ("create-table", "CREATE TABLE hacked AS SELECT 1"),
    ("drop-table", "DROP TABLE demo_orders"),
    ("alter-table", "ALTER TABLE demo_orders ADD COLUMN x INT"),
    ("attach-mem", "ATTACH ':memory:' AS x"),
    (
        "attach-pg",
        "/* bypass */ attach 'dbname=xbluenode' as p (TYPE postgres)",
    ),
    ("detach-lake", "DETACH lake"),
    ("set-ext", "SET enable_external_access = true"),
    ("set-lower", "set memory_limit = '99GB'"),
    ("reset-ext", "RESET enable_external_access"),
    ("pragma", "PRAGMA database_list"),
    ("pragma-version", "pragma version"),
    ("install-ext", "INSTALL httpfs"),
    ("load-ext", "LOAD httpfs"),
    ("call-proc", "CALL pragma_database_list()"),
    ("export-db", "EXPORT DATABASE '/tmp/dump'"),
    ("import-db", "IMPORT DATABASE '/tmp/dump'"),
    ("use-db", "USE lake"),
    ("vacuum", "VACUUM"),
    ("checkpoint", "CHECKPOINT"),
    // 3. File-reading & external table functions (case, quotes, comments, subquery, CTE, UNION)
    ("read-csv-passwd", "SELECT * FROM read_csv('/etc/passwd')"),
    (
        "read-csv-upper",
        "SELECT * FROM READ_CSV_AUTO('/etc/passwd')",
    ),
    (
        "read-csv-quoted",
        "SELECT * FROM \"read_csv\"('/etc/passwd')",
    ),
    (
        "read-parquet-subquery",
        "SELECT * FROM (SELECT * FROM read_parquet('/var/lib/axpoint/lake/a.parquet')) sub",
    ),
    (
        "read-json-cte",
        "WITH f AS (SELECT * FROM read_json('/tmp/a.json')) SELECT * FROM f",
    ),
    (
        "glob-union",
        "SELECT 'a' AS p UNION ALL SELECT * FROM glob('/etc/*')",
    ),
    (
        "parquet-scan",
        "SELECT * FROM parquet_scan('/tmp/a.parquet')",
    ),
    (
        "sqlite-scan",
        "SELECT * FROM sqlite_scan('/tmp/a.db', 'users')",
    ),
    (
        "postgres-scan",
        "SELECT * FROM postgres_scan('host=127.0.0.1', 'public', 'sec_users')",
    ),
    (
        "postgres-query",
        "SELECT * FROM postgres_query('ax_pg', 'SELECT * FROM sec_users')",
    ),
    (
        "ducklake-snapshots",
        "SELECT * FROM ducklake_snapshots('lake')",
    ),
    ("iceberg-scan", "SELECT * FROM iceberg_scan('/tmp/ice')"),
    ("delta-scan", "SELECT * FROM delta_scan('/tmp/delta')"),
    ("query-fn", "SELECT * FROM query('SELECT 1')"),
    ("query-table-fn", "SELECT * FROM query_table('demo_orders')"),
    // 4. Forbidden scalar / system / environment / internal functions
    ("getenv-home", "SELECT getenv('HOME')"),
    (
        "getenv-upper-comment",
        "SELECT /* leak */ GETENV('AXPOINT_DEV_TOKEN')",
    ),
    (
        "getenv-quoted",
        "SELECT \"getenv\"('PATH') FROM demo_orders",
    ),
    (
        "getenv-subquery",
        "SELECT status FROM demo_orders WHERE status = (SELECT getenv('USER'))",
    ),
    (
        "getenv-cte",
        "WITH leak AS (SELECT getenv('HOME') AS h) SELECT h FROM leak",
    ),
    (
        "getenv-union",
        "SELECT status FROM demo_orders UNION SELECT getenv('HOME')",
    ),
    (
        "current-setting",
        "SELECT current_setting('enable_external_access')",
    ),
    (
        "current-setting-upper",
        "SELECT CURRENT_SETTING('memory_limit')",
    ),
    ("read-text", "SELECT read_text('/etc/hostname')"),
    ("read-blob", "SELECT read_blob('/etc/shadow')"),
    ("duckdb-settings-table", "SELECT * FROM duckdb_settings()"),
    ("duckdb-settings-scalar", "SELECT duckdb_Memory()"),
    ("duckdb-secrets", "SELECT * FROM duckdb_secrets()"),
    ("pg-sleep", "SELECT pg_sleep(10)"),
    ("pg-read-file", "SELECT pg_read_file('/etc/passwd')"),
    ("pg-typeof", "SELECT pg_typeof(1)"),
    ("pragma-func", "SELECT * FROM pragma_database_list()"),
    ("load-extension-fn", "SELECT load_extension('httpfs')"),
    (
        "sql-auto-complete",
        "SELECT * FROM sql_auto_complete('SELECT ')",
    ),
    ("internal-prefix", "SELECT __internal_compress('abc')"),
    ("internal-upper", "SELECT \"__INTERNAL_PEEK\"(1)"),
    ("qualified-func", "SELECT main.getenv('HOME')"),
    // 5. Cross-tenant lake & direct lake schema access
    (
        "cross-tenant-lake",
        "SELECT * FROM lake.\"tenant_other\".orders",
    ),
    (
        "cross-tenant-unquoted",
        "SELECT * FROM lake.tenant_other.demo_orders",
    ),
    (
        "cross-tenant-subquery",
        "SELECT * FROM (SELECT * FROM lake.\"tenant_b\".demo_orders) t",
    ),
    (
        "cross-tenant-cte",
        "WITH x AS (SELECT * FROM lake.\"tenant_other\".orders) SELECT * FROM x",
    ),
    (
        "cross-tenant-union",
        "SELECT id FROM demo_orders UNION ALL SELECT id FROM lake.\"tenant_other\".demo_orders",
    ),
    (
        "direct-own-lake-bypass",
        "SELECT * FROM lake.\"tenant_default\".\"demo_orders\"",
    ),
    // 6. Bypassing platform views to query ax_pg or internal catalogs
    ("ax-pg-sec-users", "SELECT * FROM ax_pg.public.sec_users"),
    (
        "ax-pg-quoted",
        "SELECT * FROM \"ax_pg\".\"public\".\"sec_api_tokens\"",
    ),
    ("ax-pg-nodes", "SELECT * FROM ax_pg.public.xb_nodes"),
    ("info-schema", "SELECT * FROM information_schema.tables"),
    ("pg-catalog", "SELECT * FROM pg_catalog.pg_tables"),
    ("system-catalog", "SELECT * FROM system.main.duckdb_tables"),
    ("main-schema", "SELECT * FROM main.demo_orders"),
    ("sqlite-master", "SELECT * FROM sqlite_master"),
    // 7. Unknown dataset & unknown platform views
    ("unknown-bare-table", "SELECT * FROM unknown_dataset"),
    ("unknown-ax-secret", "SELECT * FROM ax.secret"),
    ("unknown-ax-users", "SELECT * FROM ax.sec_users"),
    ("unknown-ax-upper", "SELECT * FROM AX.\"secret\""),
];

#[test]
fn guard_table_driven_rejection_suite_covers_all_security_rules() {
    let catalog = demo_catalog();
    let opts = GuardOptions::default();

    assert!(
        REJECTED_CASES.len() >= 65,
        "expected >= 60 table-driven rejection cases"
    );

    for (name, sql) in REJECTED_CASES {
        let res = guard(sql, &catalog, &opts);
        assert!(
            matches!(&res, Err(err) if err.error_code() == "SQL_REJECTED"),
            "case `{name}` (`{sql}`) should be SQL_REJECTED, got: {res:?}"
        );
    }
}

#[test]
fn guard_bare_dataset_rewrites_to_tenant_lake_schema_and_wraps_limit() -> Result<(), AppError> {
    let catalog = demo_catalog();
    let res = guard(
        "SELECT count(*) AS n FROM demo_orders;",
        &catalog,
        &GuardOptions::default(),
    )?;

    assert_eq!(
        res.sql,
        "SELECT * FROM (SELECT count(*) AS n FROM lake.\"tenant_default\".\"demo_orders\" AS \"demo_orders\") AS q LIMIT 10001"
    );
    assert_eq!(res.effective_max_rows, DEFAULT_MAX_ROWS);
    assert_eq!(
        res.referenced_dataset_names,
        vec![String::from("demo_orders")]
    );
    assert!(!res.uses_platform_views);

    // Join with distinct physical lake_table name + max_rows cap clamping at HARD_MAX_ROWS (100_000)
    let join_opts = GuardOptions {
        max_rows: 500_000,
        allow_live: false,
        params: BTreeMap::new(),
    };
    let join_res = guard(
        "SELECT o.id, c.region FROM demo_orders o JOIN customers c ON o.customer_id = c.id",
        &catalog,
        &join_opts,
    )?;
    assert_eq!(join_res.effective_max_rows, HARD_MAX_ROWS);
    assert!(
        join_res
            .sql
            .contains("lake.\"tenant_default\".\"demo_orders\"")
    );
    assert!(
        join_res
            .sql
            .contains("lake.\"tenant_default\".\"customers_lake\"")
    );
    assert!(join_res.sql.ends_with("LIMIT 100001"));
    assert_eq!(join_res.referenced_datasets.len(), 2);

    Ok(())
}

#[test]
fn guard_whitelisted_table_functions_are_permitted_and_wrapped() -> Result<(), AppError> {
    let catalog = demo_catalog();
    let opts = GuardOptions {
        max_rows: 10,
        allow_live: false,
        params: BTreeMap::new(),
    };

    for sql in [
        "SELECT * FROM range(20)",
        "SELECT * FROM generate_series(1, 20)",
        "SELECT * FROM unnest([10, 20, 30])",
    ] {
        let guarded = guard(sql, &catalog, &opts)?;
        assert_eq!(guarded.effective_max_rows, 10);
        assert!(
            guarded.sql.ends_with("AS q LIMIT 11"),
            "unexpected rewritten SQL: {}",
            guarded.sql
        );
    }

    Ok(())
}

#[test]
fn guard_cte_names_are_preserved_and_shadow_datasets_correctly() -> Result<(), AppError> {
    let catalog = demo_catalog();
    let opts = GuardOptions::default();

    // Standard CTE referencing a dataset
    let res = guard(
        "WITH tmp AS (SELECT id, amount FROM demo_orders WHERE status = 'PAID') SELECT count(*) FROM tmp",
        &catalog,
        &opts,
    )?;
    assert!(
        res.sql
            .contains("FROM lake.\"tenant_default\".\"demo_orders\"")
    );
    assert!(res.sql.contains("SELECT count(*) FROM tmp"));
    assert!(!res.sql.contains("\"tmp\""));

    // CTE shadowing an existing dataset name (`demo_orders`) must NOT rewrite outer CTE reference
    let shadowed = guard(
        "WITH demo_orders AS (SELECT 42 AS id) SELECT id FROM demo_orders",
        &catalog,
        &opts,
    )?;
    assert!(
        !shadowed.sql.contains("lake.\"tenant_default\""),
        "CTE shadowing dataset should not rewrite to lake: {}",
        shadowed.sql
    );
    assert_eq!(shadowed.referenced_datasets, Vec::<Uuid>::new());

    Ok(())
}

#[test]
fn guard_platform_views_are_allowed_and_flagged_while_materialize_forbids_them()
-> Result<(), AppError> {
    let catalog = demo_catalog();
    let opts = GuardOptions::default();

    for view in [
        "ax.nodes",
        "ax.links",
        "ax.metric_points",
        "ax.metric_series",
        "ax.metric_rollups",
    ] {
        let sql = format!("SELECT * FROM {view}");
        let guarded = guard(&sql, &catalog, &opts)?;
        assert!(
            guarded.uses_platform_views,
            "expected uses_platform_views=true for {view}"
        );
        assert!(guarded.sql.contains("ax."));

        let mat = guard_for_materialize(&sql, &catalog);
        assert!(
            matches!(&mat, Err(err) if err.error_code() == "SQL_REJECTED"),
            "materialize must reject platform view {view}, got {mat:?}"
        );
    }

    assert_sql_rejected("SELECT * FROM ax.secret", &catalog, &opts);
    Ok(())
}

#[test]
fn guard_live_federated_connection_enforces_allow_live_permission_and_registration()
-> Result<(), AppError> {
    let base_catalog = demo_catalog(); // has_live_permission = false, live_connections = {"erp"}

    // 1. allow_live = false -> SQL_REJECTED
    let opts_disallowed = GuardOptions {
        max_rows: 100,
        allow_live: false,
        params: BTreeMap::new(),
    };
    assert_sql_rejected(
        "SELECT * FROM live_erp.public.orders",
        &base_catalog,
        &opts_disallowed,
    );

    // 2. allow_live = true, has_live_permission = false -> 403 PERMISSION_DENIED
    let opts_live = GuardOptions {
        max_rows: 100,
        allow_live: true,
        params: BTreeMap::new(),
    };
    let denied = guard(
        "SELECT * FROM live_erp.public.orders",
        &base_catalog,
        &opts_live,
    );
    assert!(
        matches!(&denied, Err(AppError::Forbidden("PERMISSION_DENIED"))),
        "expected PERMISSION_DENIED when has_live_permission=false, got: {denied:?}"
    );

    // 3. allow_live = true, has_live_permission = true, registered -> allowed
    let permitted_catalog = base_catalog.with_live_permission(true);
    let ok = guard(
        "SELECT id, total FROM live_erp.public.orders",
        &permitted_catalog,
        &opts_live,
    )?;
    assert_eq!(ok.referenced_live_connections, vec![String::from("erp")]);
    assert!(ok.sql.contains("live_erp.public.orders"));
    assert!(ok.sql.ends_with("LIMIT 101"));

    // 4. Unregistered live connection -> SQL_REJECTED even with permission
    assert_sql_rejected(
        "SELECT * FROM live_crm.public.contacts",
        &permitted_catalog,
        &opts_live,
    );

    Ok(())
}

#[test]
fn guard_named_parameters_payload_size_and_output_pii_lineage_work_accurately()
-> Result<(), AppError> {
    let catalog = demo_catalog();

    // Named parameters substituted safely with quote escaping
    let mut params = BTreeMap::new();
    params.insert(String::from("status"), json!("O'Reilly"));
    params.insert(String::from("min_amt"), json!(-12.5));
    params.insert(String::from("active"), json!(true));
    params.insert(String::from("ids"), json!([1, 2, 3]));

    let opts = GuardOptions {
        max_rows: 50,
        allow_live: false,
        params,
    };
    let bound = guard(
        "SELECT id FROM demo_orders WHERE status = :status AND amount > :min_amt AND :active AND id IN (:ids)",
        &catalog,
        &opts,
    )?;
    assert!(bound.sql.contains("'O''Reilly'"), "sql was: {}", bound.sql);
    assert!(bound.sql.contains("-12.5"), "sql was: {}", bound.sql);
    assert!(bound.sql.contains("true"), "sql was: {}", bound.sql);
    assert!(bound.sql.contains("IN (1, 2, 3)"), "sql was: {}", bound.sql);

    // Missing named parameter -> SQL_REJECTED
    assert_sql_rejected(
        "SELECT id FROM demo_orders WHERE status = :missing_param",
        &catalog,
        &opts,
    );

    // Positional rewrite helper (`guard_and_rewrite`)
    let pos = guard_and_rewrite(
        "SELECT id FROM demo_orders WHERE status = :status AND amount >= :min_amt",
        &catalog,
        25,
    )?;
    assert_eq!(
        pos.param_order,
        vec![String::from("status"), String::from("min_amt")]
    );
    assert!(pos.rewritten_sql.contains("status = ?"));
    assert!(pos.rewritten_sql.ends_with("LIMIT 26"));

    // SQL exceeding 64 KiB -> 413 PAYLOAD_TOO_LARGE
    let padding = " ".repeat(MAX_SQL_BYTES);
    let oversized_sql = format!("SELECT 1{padding}");
    assert!(oversized_sql.len() > MAX_SQL_BYTES);
    let too_large = guard(&oversized_sql, &catalog, &GuardOptions::default());
    assert!(
        matches!(
            &too_large,
            Err(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE"))
        ),
        "expected PAYLOAD_TOO_LARGE, got: {too_large:?}"
    );

    // Output column PII lineage: Direct PII, DerivedFromPii, Direct non-PII, NonSensitive literal, and Wildcard
    let lineage_res = guard(
        "SELECT o.phone AS contact_phone, substring(o.email, 1, 4) AS email_prefix, o.status, o.amount * 2 AS dbl_amt FROM demo_orders o",
        &catalog,
        &GuardOptions::default(),
    )?;
    let [col0, col1, col2, col3] = lineage_res.output_lineage.as_slice() else {
        return Err(AppError::bad("TEST_FAIL", "expected 4 lineage columns"));
    };
    assert!(matches!(
        col0,
        OutputColumnOrigin::Direct {
            column_name,
            output_name,
            pii_category: PiiCategory::Phone,
            ..
        } if column_name == "phone" && output_name == "contact_phone"
    ));
    assert!(matches!(
        col1,
        OutputColumnOrigin::DerivedFromPii {
            output_name: Some(alias),
            pii_category: PiiCategory::Email,
        } if alias == "email_prefix"
    ));
    assert!(matches!(
        col2,
        OutputColumnOrigin::Direct {
            column_name,
            pii_category: PiiCategory::None,
            ..
        } if column_name == "status"
    ));
    assert!(matches!(
        col3,
        OutputColumnOrigin::NonSensitive {
            output_name: Some(alias),
        } if alias == "dbl_amt"
    ));

    // Wildcard lineage
    let wild = guard(
        "SELECT * FROM demo_orders",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(wild.output_lineage, vec![OutputColumnOrigin::Wildcard]);

    Ok(())
}

#[test]
fn guard_derived_subquery_wrapper_preserves_direct_and_derived_pii_lineage() -> Result<(), AppError>
{
    let catalog = demo_catalog();
    let orders_id = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);

    // 1. Subquery filter wrapper over aliased direct PII column
    let direct_wrapped = guard(
        "SELECT * FROM (SELECT phone AS contact_num FROM demo_orders) AS _w WHERE status = 'PAID'",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        direct_wrapped.output_lineage,
        vec![OutputColumnOrigin::Direct {
            dataset_id: orders_id,
            column_name: String::from("phone"),
            output_name: String::from("contact_num"),
            pii_category: PiiCategory::Phone,
        }]
    );
    assert_eq!(
        direct_wrapped.referenced_columns,
        vec![(orders_id, String::from("phone"))]
    );

    // 2. Subquery filter wrapper over derived PII expression -> DerivedFromPii -> [REDACTED]
    let derived_wrapped = guard(
        "SELECT * FROM (SELECT substring(email, 1, 4) AS em FROM demo_orders) AS _w WHERE status = 'PAID'",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        derived_wrapped.output_lineage,
        vec![OutputColumnOrigin::DerivedFromPii {
            output_name: Some(String::from("em")),
            pii_category: PiiCategory::Email,
        }]
    );
    let schema = Arc::new(Schema::new(vec![Field::new(
        "em",
        ArrowDataType::Utf8,
        true,
    )]));
    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![Arc::new(StringArray::from(vec![Some("alic")]))],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    let serialized = batches_to_query_result(
        &schema,
        &[batch],
        ResultSerializeOptions {
            effective_max_rows: 100,
            sql: derived_wrapped.sql.clone(),
            duration_ms: 1,
            reveal_pii: false,
            has_pii_raw_read: false,
            catalog: &catalog,
            guarded: &derived_wrapped,
        },
    )?;
    let Some(first_row) = serialized.result.rows.first() else {
        return Err(AppError::bad("TEST_FAIL", "expected row"));
    };
    assert_eq!(first_row.get("em"), Some(&json!("[REDACTED]")));

    Ok(())
}

#[test]
fn guard_subquery_qualified_wildcard_and_drill_exprs_preserve_pii_lineage() -> Result<(), AppError>
{
    let catalog = demo_catalog();
    let orders_id = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);

    let qwild = guard(
        "SELECT _w.* FROM (SELECT phone AS contact_num FROM demo_orders) AS _w",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        qwild.output_lineage,
        vec![OutputColumnOrigin::Direct {
            dataset_id: orders_id,
            column_name: String::from("phone"),
            output_name: String::from("contact_num"),
            pii_category: PiiCategory::Phone,
        }]
    );

    let drill = guard(
        "SELECT _w.contact_num, upper(_w.contact_num) AS loud_num, SUM(amount) AS amount FROM (SELECT phone AS contact_num, amount, status FROM demo_orders) AS _w WHERE status = 'PAID' GROUP BY _w.contact_num",
        &catalog,
        &GuardOptions::default(),
    )?;
    let [d0, d1, d2] = drill.output_lineage.as_slice() else {
        return Err(AppError::bad("TEST_FAIL", "expected 3 drill columns"));
    };
    assert_eq!(
        d0,
        &OutputColumnOrigin::Direct {
            dataset_id: orders_id,
            column_name: String::from("phone"),
            output_name: String::from("contact_num"),
            pii_category: PiiCategory::Phone,
        }
    );
    assert_eq!(
        d1,
        &OutputColumnOrigin::DerivedFromPii {
            output_name: Some(String::from("loud_num")),
            pii_category: PiiCategory::Phone,
        }
    );
    assert_eq!(
        d2,
        &OutputColumnOrigin::NonSensitive {
            output_name: Some(String::from("amount")),
        }
    );

    Ok(())
}

#[test]
fn guard_nested_subqueries_union_and_cte_propagate_pii_lineage() -> Result<(), AppError> {
    let catalog = demo_catalog();
    let orders_id = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);

    // 1. CTE explicit projection and CTE wildcard projection
    let cte_direct = guard(
        "WITH sub AS (SELECT phone AS p FROM demo_orders) SELECT p FROM sub",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        cte_direct.output_lineage,
        vec![OutputColumnOrigin::Direct {
            dataset_id: orders_id,
            column_name: String::from("phone"),
            output_name: String::from("p"),
            pii_category: PiiCategory::Phone,
        }]
    );

    let cte_star = guard(
        "WITH sub AS (SELECT phone AS p, substring(email, 1, 4) AS em FROM demo_orders) SELECT * FROM sub",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        cte_star.output_lineage,
        vec![
            OutputColumnOrigin::Direct {
                dataset_id: orders_id,
                column_name: String::from("phone"),
                output_name: String::from("p"),
                pii_category: PiiCategory::Phone,
            },
            OutputColumnOrigin::DerivedFromPii {
                output_name: Some(String::from("em")),
                pii_category: PiiCategory::Email,
            },
        ]
    );

    // 2. CTE wildcard over physical dataset followed by outer alias + derived PII
    let cte_wild_alias = guard(
        "WITH base AS (SELECT * FROM demo_orders) SELECT phone AS contact_num, substring(email, 1, 3) AS em FROM base",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        cte_wild_alias.output_lineage,
        vec![
            OutputColumnOrigin::Direct {
                dataset_id: orders_id,
                column_name: String::from("phone"),
                output_name: String::from("contact_num"),
                pii_category: PiiCategory::Phone,
            },
            OutputColumnOrigin::DerivedFromPii {
                output_name: Some(String::from("em")),
                pii_category: PiiCategory::Email,
            },
        ]
    );

    // 3. Multi-level nested subquery with inner UNION merging Direct PII + NonSensitive and Direct PII + DerivedFromPii
    let union_nested = guard(
        "SELECT * FROM (SELECT * FROM (SELECT phone AS contact_num, email AS msg FROM demo_orders UNION ALL SELECT 'N/A' AS contact_num, substring(email, 1, 4) AS msg FROM demo_orders) AS u) AS _w",
        &catalog,
        &GuardOptions::default(),
    )?;
    assert_eq!(
        union_nested.output_lineage,
        vec![
            OutputColumnOrigin::Direct {
                dataset_id: orders_id,
                column_name: String::from("phone"),
                output_name: String::from("contact_num"),
                pii_category: PiiCategory::Phone,
            },
            OutputColumnOrigin::DerivedFromPii {
                output_name: Some(String::from("msg")),
                pii_category: PiiCategory::Email,
            },
        ]
    );

    Ok(())
}
