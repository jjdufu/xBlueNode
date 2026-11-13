//! `AxAnalytics` OLAP session pool, SQL AST guard, pivot engine, semantic layer, NL2SQL, and AI transforms (`05-STEP04`).

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;

pub use arrow;
use axpoint_domain::AppError;
pub use axpoint_domain::analytics::{
    AnalyticsCatalogResponse, AnalyticsPrincipal, ApplyTransformResponse, AskContext,
    AskFeedbackKind, AskFeedbackRequest, AskIntent, AskRequest, AskResponse, AskTurnRecord,
    CatalogDatasetNode, ChartSuggestion, CreateMetricRequest, CreateSavedQueryRequest,
    CreateTransformDraftRequest, ExplainResult, FewShotTurn, FieldRef, FilterOp, FilterSpec,
    LinkedColumnHit, LinkedMetricHit, LinkedObjects, LiveConnectionNode, MaterializeRequest,
    MetricCompareMode, MetricDef, MetricDimension, MetricQuery, MetricQueryResult, MetricTimeRange,
    MultiMetricQuery, NewAskTurnInput, NewQueryHistoryEntry, NewTransformDraftInput, PivotResult,
    PivotRowKind, PivotSpec, PlatformViewNode, QueryColumn, QueryHistoryEntry, QueryHistoryKind,
    QueryHistoryListQuery, QueryHistoryStatus, QueryParamSpec, QueryRequest, QueryResult,
    RunSavedQueryRequest, SavedQuery, SavedQueryKind, SchemaRecallBundle, SortDir, SortSpec,
    TimeBucket, TransformDraft, TransformDraftStatus, TransformPreview, UpdateMetricRequest,
    UpdateSavedQueryRequest, UpdateTransformDraftRequest, ValueSpec, ds_column_embedding_id,
};
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::ConnectorType;
use axpoint_security::Keyring;
use chrono::Utc;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

/// Runtime configuration for `AxAnalytics` `SessionPool` (`05-STEP04` §04.2).
#[derive(Debug, Clone)]
pub struct AnalyticsConfig {
    /// Root data directory (`<data_root>/lake/`).
    pub data_root: PathBuf,
    /// Optional `DuckLake` PostgreSQL catalog DSN (`AXPOINT_LAKE_CATALOG_DSN`).
    pub lake_catalog_dsn: Option<String>,
    /// Optional platform PostgreSQL `axpoint_app` DSN (`DATABASE_URL`).
    pub app_database_url: Option<String>,
    /// Maximum cached tenant `DuckDB` instances (`AXPOINT_DUCKDB_TENANT_CACHE`, default `32`).
    pub tenant_cache_capacity: usize,
    /// Per-instance `DuckDB` memory limit (`AXPOINT_DUCKDB_MEMORY`, default `"1GB"`).
    pub memory_limit: String,
    /// Per-instance `DuckDB` worker thread count (default `4`).
    pub threads: u32,
    /// Optional keyring used to unseal `live_query = true` connection secrets.
    pub keyring: Option<Keyring>,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            data_root: PathBuf::from("/var/lib/axpoint"),
            lake_catalog_dsn: None,
            app_database_url: None,
            tenant_cache_capacity: 32,
            memory_limit: String::from("1GB"),
            threads: 4,
            keyring: None,
        }
    }
}

/// Dataset entry cached inside [`TenantCatalog`] for `sql_guard` resolution and PII masking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDatasetEntry {
    /// Dataset UUID.
    pub id: Uuid,
    /// Dataset SQL name.
    pub name: String,
    /// Backing `DuckLake` table name.
    pub lake_table: String,
    /// Current schema version.
    pub schema_version: i32,
    /// Column metadata indexed by lowercase column name.
    pub columns: BTreeMap<String, ColumnMeta>,
}

/// Live-attached relational connection metadata cached in [`TenantCatalog`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogLiveEntry {
    /// Connection UUID.
    pub id: Uuid,
    /// Connection name (`live_<name>`).
    pub name: String,
    /// Connector type (`POSTGRES`, `MYSQL`, `SQLITE`).
    pub connector_type: ConnectorType,
}

/// Per-tenant catalog snapshot consumed by `sql_guard`, `pivot`, `semantic`, and `result` (`§04.2`, `§04.3`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TenantCatalog {
    /// Active tenant identifier (`lake."tenant_<id>"`).
    pub tenant_id: String,
    /// Datasets keyed by lowercase dataset SQL name.
    pub datasets: BTreeMap<String, CatalogDatasetEntry>,
    /// Live relational connections keyed by lowercase alias (`live_<name>`).
    pub live_connections: BTreeMap<String, CatalogLiveEntry>,
}

/// Options passed into `sql_guard::guard` (`05-STEP04` §04.3).
#[derive(Debug, Clone)]
pub struct GuardOptions {
    /// Maximum rows before truncation (`default = 10_000`, `cap = 100_000`).
    pub max_rows: u32,
    /// Whether `live_<conn>.<schema>.<table>` references are allowed.
    pub allow_live: bool,
    /// Named `:param` values substituted as typed SQL literals.
    pub params: BTreeMap<String, serde_json::Value>,
}

impl Default for GuardOptions {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            allow_live: false,
            params: BTreeMap::new(),
        }
    }
}

/// Validated and rewritten SQL output produced by `sql_guard::guard` (`05-STEP04` §04.3).
#[derive(Debug, Clone)]
pub struct GuardedSql {
    /// Rewritten SQL wrapped with `SELECT * FROM (...) AS q LIMIT <max_rows + 1>`.
    pub sql: String,
    /// Referenced dataset UUIDs.
    pub referenced_datasets: Vec<Uuid>,
    /// Referenced `(dataset_id, column_name)` pairs.
    pub referenced_columns: Vec<(Uuid, String)>,
    /// Whether any `ax.*` platform view was referenced.
    pub uses_platform_views: bool,
}

/// Verifies bundled `DuckDB`, `DuckDbDialect` parser, and LRU cache readiness (replaced/expanded in Stage 2).
///
/// # Errors
/// Returns [`AppError::Config`] or [`AppError::BadRequest`] if bundled `DuckDB` or `sqlparser` fails.
pub async fn probe_analytics_runtime() -> Result<String, AppError> {
    let dialect = DuckDbDialect {};
    let statements = Parser::parse_sql(&dialect, "SELECT 1 AS ok")
        .map_err(|err| AppError::bad("SQL_REJECTED", format!("sql parse failed: {err}")))?;
    if statements.len() != 1 {
        return Err(AppError::bad("SQL_REJECTED", "expected single statement"));
    }

    let cap = NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN);
    let cache: Mutex<LruCache<String, i64>> = Mutex::new(LruCache::new(cap));
    {
        let mut guard = cache.lock().await;
        guard.put(String::from("probe"), Utc::now().timestamp());
    }

    let version = tokio::task::spawn_blocking(|| -> Result<String, AppError> {
        let conn = duckdb::Connection::open_in_memory()
            .map_err(|err| AppError::Config(format!("duckdb open_in_memory failed: {err}")))?;
        let ver: String = conn
            .query_row("SELECT version()", [], |row| row.get(0))
            .map_err(|err| AppError::Config(format!("duckdb version query failed: {err}")))?;
        Ok(ver)
    })
    .await
    .map_err(|err| AppError::Config(format!("duckdb task join failed: {err}")))??;

    debug!(duckdb_version = %version, "axpoint-analytics runtime probe succeeded");
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bundled_duckdb_and_sqlparser_probe_succeeds() {
        let res = probe_analytics_runtime().await;
        assert!(res.is_ok());
    }
}
