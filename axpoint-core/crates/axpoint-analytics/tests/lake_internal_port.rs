//! Integration tests for `SessionPool`'s `LakeInternalQueryPort` implementation against a local `DuckLake` catalog (`06-STEP05-06` §06.2.1).

use std::path::{Path, PathBuf};

use axpoint_analytics::{AnalyticsConfig, SessionPool};
use axpoint_domain::AppError;
use axpoint_domain::analytics::QueryResult;
use axpoint_domain::ports::LakeInternalQueryPort;
use serde_json::json;
use uuid::Uuid;

struct TempDuckLake {
    data_root: PathBuf,
    catalog_file: PathBuf,
    lake_dir: PathBuf,
}

impl TempDuckLake {
    fn new(tag: &str) -> Result<Self, AppError> {
        let data_root =
            std::env::temp_dir().join(format!("axpoint-lake-port-{tag}-{}", Uuid::new_v4()));
        let raw_lake_dir = data_root.join("lake");
        std::fs::create_dir_all(&raw_lake_dir)
            .map_err(|err| AppError::Config(format!("create temp lake dir failed: {err}")))?;
        let canon_root = canonical_or_owned(&data_root);
        let lake_dir = canonical_or_owned(&raw_lake_dir);
        let catalog_file = canon_root.join("lake_catalog.ducklake");
        Ok(Self {
            data_root,
            catalog_file,
            lake_dir,
        })
    }

    fn analytics_config(&self) -> AnalyticsConfig {
        AnalyticsConfig {
            data_root: self.data_root.clone(),
            lake_catalog_dsn: None,
            app_database_url: None,
            tenant_cache_capacity: 4,
            memory_limit: String::from("256MB"),
            threads: 2,
            keyring: None,
        }
    }

    fn execute_writer_batch(&self, body_sql: &str) -> Result<(), AppError> {
        let conn = duckdb::Connection::open_in_memory()
            .map_err(|err| AppError::Config(format!("open writer duckdb failed: {err}")))?;
        let escaped_catalog = sql_escape_single_quoted(&self.catalog_file.to_string_lossy());
        let escaped_data = sql_escape_single_quoted(&self.lake_dir.to_string_lossy());
        let full_sql = format!(
            "LOAD ducklake;
             ATTACH 'ducklake:{escaped_catalog}' AS lake_rw (DATA_PATH '{escaped_data}/');
             {body_sql}
             DETACH lake_rw;"
        );
        conn.execute_batch(&full_sql)
            .map_err(|err| AppError::Database(format!("ducklake writer batch failed: {err}")))
    }
}

impl Drop for TempDuckLake {
    fn drop(&mut self) {
        let _cleanup_ok = std::fs::remove_dir_all(&self.data_root).is_ok();
    }
}

fn canonical_or_owned(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn sql_escape_single_quoted(raw: &str) -> String {
    raw.replace('\'', "''")
}

async fn read_current_snapshot<L: LakeInternalQueryPort>(
    lake: &L,
    tenant_id: &str,
) -> Result<i64, AppError> {
    let qr = lake
        .query_internal(
            tenant_id,
            "SELECT id FROM ducklake_current_snapshot('lake')",
            1,
        )
        .await?;
    extract_i64_cell(&qr, 0, "id")
}

fn extract_i64_cell(qr: &QueryResult, row_idx: usize, col: &str) -> Result<i64, AppError> {
    let row = qr.rows.get(row_idx).ok_or_else(|| {
        AppError::bad(
            "TEST_ASSERT",
            format!("missing row {row_idx} in query result"),
        )
    })?;
    let val = row.get(col).ok_or_else(|| {
        AppError::bad(
            "TEST_ASSERT",
            format!("missing column '{col}' in row {row_idx}"),
        )
    })?;
    if let Some(n) = val.as_i64() {
        return Ok(n);
    }
    if let Some(s) = val.as_str() {
        return s.parse::<i64>().map_err(|err| {
            AppError::bad(
                "TEST_ASSERT",
                format!("invalid i64 string '{s}' in '{col}': {err}"),
            )
        });
    }
    Err(AppError::bad(
        "TEST_ASSERT",
        format!("column '{col}' is not an integer: {val}"),
    ))
}

fn dedup_orders_changes_sql(from_snap: i64, to_snap: i64) -> String {
    format!(
        "SELECT snapshot_id, change_type, order_id, status, amount
         FROM (
             SELECT
                 snapshot_id,
                 change_type,
                 rowid,
                 order_id,
                 status,
                 amount,
                 ROW_NUMBER() OVER (
                     PARTITION BY order_id
                     ORDER BY
                         snapshot_id DESC,
                         CASE WHEN change_type = 'delete' THEN 0 ELSE 1 END DESC,
                         rowid DESC
                 ) AS rn
             FROM ducklake_table_changes('lake', 'tenant_default', 'orders', {from_snap}, {to_snap})
         ) deduped
         WHERE rn = 1
         ORDER BY order_id ASC"
    )
}

async fn verify_initial_snapshot_and_chunked_full<L: LakeInternalQueryPort>(
    lake: &L,
) -> Result<i64, AppError> {
    let snap1 = read_current_snapshot(lake, "default").await?;
    assert!(snap1 >= 1, "expected initial snapshot >= 1, got {snap1}");

    let truncated_probe = lake
        .query_internal(
            "default",
            "SELECT order_id, status, amount FROM lake.\"tenant_default\".\"orders\" ORDER BY order_id ASC",
            2,
        )
        .await?;
    assert!(truncated_probe.truncated);
    assert_eq!(truncated_probe.row_count, 2);

    let chunk_size = 2_usize;
    let mut offset = 0_usize;
    let mut all_orders = Vec::new();
    loop {
        let chunk_sql = format!(
            "SELECT order_id, status, amount
             FROM lake.\"tenant_default\".\"orders\"
             ORDER BY order_id ASC
             LIMIT {chunk_size} OFFSET {offset}"
        );
        let page = lake
            .query_internal("default", &chunk_sql, chunk_size)
            .await?;
        assert!(!page.truncated);
        let fetched = page.rows.len();
        all_orders.extend(page.rows);
        if fetched < chunk_size {
            break;
        }
        offset = offset.saturating_add(fetched);
    }
    assert_eq!(all_orders.len(), 4);
    assert_eq!(
        all_orders.first().and_then(|r| r.get("order_id")),
        Some(&json!(1))
    );
    assert_eq!(
        all_orders.first().and_then(|r| r.get("status")),
        Some(&json!("OPEN"))
    );
    assert_eq!(
        all_orders.first().and_then(|r| r.get("amount")),
        Some(&json!(100.0))
    );
    assert_eq!(
        all_orders.get(3).and_then(|r| r.get("order_id")),
        Some(&json!(5))
    );
    Ok(snap1)
}

async fn verify_upsert_changes_and_dedup<L: LakeInternalQueryPort>(
    env: &TempDuckLake,
    lake: &L,
    snap1: i64,
) -> Result<i64, AppError> {
    env.execute_writer_batch(
        "BEGIN;
         DELETE FROM lake_rw.\"tenant_default\".\"orders\" WHERE order_id = 1;
         INSERT INTO lake_rw.\"tenant_default\".\"orders\" VALUES
             (1, 'SHIPPED', 250.0),
             (3, 'OPEN', 90.0);
         COMMIT;",
    )?;

    assert!(lake.invalidate_tenant_session("default").await);
    assert!(!lake.invalidate_tenant_session("default").await);

    let snap2 = read_current_snapshot(lake, "default").await?;
    assert!(
        snap2 > snap1,
        "expected snap2 ({snap2}) > snap1 ({snap1}) after UPSERT"
    );

    let from_snap = snap1.saturating_add(1);
    let raw_changes_sql = format!(
        "SELECT snapshot_id, change_type, rowid, order_id, status, amount
         FROM ducklake_table_changes('lake', 'tenant_default', 'orders', {from_snap}, {snap2})
         ORDER BY order_id ASC, snapshot_id ASC"
    );
    let raw_changes = lake.query_internal("default", &raw_changes_sql, 50).await?;
    assert!(
        raw_changes.row_count >= 2,
        "expected raw change feed rows for order_id 1 and 3, got {}",
        raw_changes.row_count
    );

    let dedup_sql = dedup_orders_changes_sql(from_snap, snap2);
    let dedup_upsert = lake.query_internal("default", &dedup_sql, 50).await?;
    assert_eq!(dedup_upsert.row_count, 2);

    let row_ord_1 = dedup_upsert
        .rows
        .first()
        .ok_or_else(|| AppError::bad("TEST_ASSERT", "missing deduplicated row for order_id = 1"))?;
    assert_eq!(row_ord_1.get("order_id"), Some(&json!(1)));
    assert_eq!(row_ord_1.get("status"), Some(&json!("SHIPPED")));
    assert_eq!(row_ord_1.get("amount"), Some(&json!(250.0)));
    assert_ne!(row_ord_1.get("change_type"), Some(&json!("delete")));

    let row_ord_3 = dedup_upsert
        .rows
        .get(1)
        .ok_or_else(|| AppError::bad("TEST_ASSERT", "missing deduplicated row for order_id = 3"))?;
    assert_eq!(row_ord_3.get("order_id"), Some(&json!(3)));
    assert_eq!(row_ord_3.get("status"), Some(&json!("OPEN")));
    assert_eq!(row_ord_3.get("amount"), Some(&json!(90.0)));
    assert_eq!(row_ord_3.get("change_type"), Some(&json!("insert")));
    Ok(snap2)
}

async fn verify_delete_changes<L: LakeInternalQueryPort>(
    env: &TempDuckLake,
    lake: &L,
    snap2: i64,
) -> Result<(), AppError> {
    env.execute_writer_batch(
        "BEGIN;
         DELETE FROM lake_rw.\"tenant_default\".\"orders\" WHERE order_id = 2;
         COMMIT;",
    )?;

    assert!(lake.invalidate_tenant_session("default").await);

    let snap3 = read_current_snapshot(lake, "default").await?;
    assert!(
        snap3 > snap2,
        "expected snap3 ({snap3}) > snap2 ({snap2}) after DELETE"
    );

    let from_snap = snap2.saturating_add(1);
    let delete_changes_sql = dedup_orders_changes_sql(from_snap, snap3);
    let delete_changes = lake
        .query_internal("default", &delete_changes_sql, 50)
        .await?;
    assert_eq!(delete_changes.row_count, 1);

    let del_row = delete_changes.rows.first().ok_or_else(|| {
        AppError::bad("TEST_ASSERT", "missing delete change row for order_id = 2")
    })?;
    assert_eq!(del_row.get("order_id"), Some(&json!(2)));
    assert_eq!(del_row.get("change_type"), Some(&json!("delete")));
    Ok(())
}

async fn verify_port_lifecycle<L: LakeInternalQueryPort>(
    env: &TempDuckLake,
    lake: &L,
) -> Result<(), AppError> {
    let snap1 = verify_initial_snapshot_and_chunked_full(lake).await?;
    let snap2 = verify_upsert_changes_and_dedup(env, lake, snap1).await?;
    verify_delete_changes(env, lake, snap2).await
}

#[tokio::test]
async fn session_pool_lake_internal_port_full_and_incremental_changes() -> Result<(), AppError> {
    let env = TempDuckLake::new("orders")?;
    env.execute_writer_batch(
        "CREATE SCHEMA IF NOT EXISTS lake_rw.\"tenant_default\";
         CREATE TABLE lake_rw.\"tenant_default\".\"orders\" (
             order_id INT,
             status VARCHAR,
             amount DOUBLE
         );
         INSERT INTO lake_rw.\"tenant_default\".\"orders\" VALUES
             (1, 'OPEN', 100.0),
             (2, 'PAID', 200.0),
             (4, 'OPEN', 40.0),
             (5, 'PAID', 55.5);",
    )?;

    let pool = SessionPool::new(env.analytics_config());
    verify_port_lifecycle(&env, &pool).await
}

#[tokio::test]
async fn lake_internal_port_error_and_tenant_isolation_guards() -> Result<(), AppError> {
    let env = TempDuckLake::new("guards")?;
    env.execute_writer_batch("CREATE SCHEMA IF NOT EXISTS lake_rw.\"tenant_alpha\";")?;

    let pool = SessionPool::new(env.analytics_config());
    verify_port_guards(&pool).await
}

async fn verify_port_guards<L: LakeInternalQueryPort>(lake: &L) -> Result<(), AppError> {
    let bad_tenant = lake.query_internal("../bad", "SELECT 1 AS ok", 10).await;
    assert!(matches!(bad_tenant, Err(AppError::BadRequest { .. })));

    let empty_sql = lake.query_internal("alpha", "   ", 10).await;
    assert!(matches!(empty_sql, Err(AppError::BadRequest { .. })));

    let nul_sql = lake.query_internal("alpha", "SELECT 1\0", 10).await;
    assert!(matches!(nul_sql, Err(AppError::BadRequest { .. })));

    let ok_alpha = lake.query_internal("alpha", "SELECT 42 AS v", 10).await?;
    assert_eq!(
        ok_alpha.rows.first().and_then(|r| r.get("v")),
        Some(&json!(42))
    );

    let ok_beta = lake.query_internal("beta", "SELECT 7 AS v", 10).await?;
    assert_eq!(
        ok_beta.rows.first().and_then(|r| r.get("v")),
        Some(&json!(7))
    );

    // Invalidating beta must not evict alpha's cached session.
    assert!(lake.invalidate_tenant_session("beta").await);
    assert!(!lake.invalidate_tenant_session("beta").await);
    assert!(lake.invalidate_tenant_session("alpha").await);

    Ok(())
}
