//! `AxAnalytics` OLAP session pool, SQL AST guard, pivot engine, semantic layer, NL2SQL, and AI transforms (`05-STEP04`).
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub mod grounding_eval;
pub mod nl2sql;
pub(crate) mod ontology_join;
pub mod pivot;
pub mod profiling;
pub mod result;
pub mod semantic;
pub mod session;
pub mod sql_guard;
pub mod transform;

use std::num::NonZeroUsize;
use std::path::PathBuf;

pub use arrow;
use axpoint_domain::AppError;
pub use axpoint_domain::analytics::{
    AllowedDatasetJoinEdge, AnalyticsCatalogResponse, AnalyticsPrincipal, ApplyTransformResponse,
    AskContext, AskFeedbackKind, AskFeedbackRequest, AskIntent, AskRequest, AskResponse,
    AskTurnRecord, CatalogDatasetNode, ChartSuggestion, CreateMetricRequest,
    CreateSavedQueryRequest, CreateTransformDraftRequest, ExplainResult, FewShotTurn, FieldRef,
    FilterOp, FilterSpec, GroundingEvalRunRecord, GroundingEvalRunResponse, GroundingEvalTrack,
    LinkedColumnHit, LinkedMetricHit, LinkedObjects, LiveConnectionNode, MaterializeRequest,
    MetricCompareMode, MetricDef, MetricDimension, MetricQuery, MetricQueryResult, MetricTimeRange,
    MultiMetricQuery, NewAskTurnInput, NewGroundingEvalRunInput, NewQueryHistoryEntry,
    NewTransformDraftInput, OntologySlotDraft, OntologySlotFilter, PivotResult, PivotRowKind,
    PivotSpec, PlatformViewNode, QueryColumn, QueryHistoryEntry, QueryHistoryKind,
    QueryHistoryListQuery, QueryHistoryStatus, QueryParamSpec, QueryRequest, QueryResult,
    RunSavedQueryRequest, SavedQuery, SavedQueryKind, SchemaRecallBundle, SortDir, SortSpec,
    TimeBucket, TransformDraft, TransformDraftStatus, TransformPreview, UpdateMetricRequest,
    UpdateSavedQueryRequest, UpdateTransformDraftRequest, ValueSpec, ds_column_embedding_id,
};
use axpoint_domain::enums::ConnectorType;
use axpoint_security::Keyring;
use chrono::Utc;
pub use duckdb;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

pub use grounding_eval::{
    GroundedDocSynthesis, GroundedHybridSynthesis, HybridGroundingInput, MIN_DOC_VECTOR_COSINE_SIM,
    candidate_has_substantive_term, filter_noise_recall_candidates, run_grounding_eval_suite,
    run_grounding_eval_track, sanitize_and_ground_doc_answer, sanitize_and_ground_hybrid_answer,
};
pub use nl2sql::schema_link::compile_ontology_join_edges;
pub use nl2sql::{
    AskPipelineInput, AskPipelineOutput, MAX_REPAIR_ROUNDS, MAX_SUMMARY_ROWS, NL2SQL_MAX_ROWS,
    run_ask_pipeline, run_ask_pipeline_conn, run_ask_pipeline_with_policy, validate_corrected_sql,
};
pub use pivot::{
    CompiledPivot, DEFAULT_PIVOT_COLUMN_LIMIT, DEFAULT_PIVOT_ROW_LIMIT, MAX_PIVOT_COL_DIMS,
    MAX_PIVOT_COLUMN_LIMIT, MAX_PIVOT_ROW_DIMS, MAX_PIVOT_ROW_LIMIT, MAX_PIVOT_VALUES,
    PivotExecuteOptions, PivotRunOptions, compile_pivot, compile_pivot_with_policy, execute_pivot,
    execute_pivot_conn,
};
pub use profiling::{
    CATEGORICAL_MAX_DISTINCT, FK_MIN_OVERLAP_RATIO, MAX_MAPPING_PREVIEW_ROWS,
    MAX_PROFILE_SAMPLE_ROWS, MAX_PROFILE_SAMPLE_VALUES, MAX_REJECT_SAMPLES,
    ONE_TO_ONE_MIN_UNIQUE_RATIO, format_tenant_lake_table, preview_mapping_rows,
    preview_mapping_rows_conn, profile_datasets, profile_datasets_conn,
};
pub use result::{
    ResultSerializeOptions, SerializedQueryOutput, batches_to_query_result, map_arrow_data_type,
    mask_pii_str,
};
pub use semantic::{
    CompiledMetricQuery, MAX_MULTI_METRICS, MetricExecuteOptions, SemanticQueryOptions,
    check_metric_compatibility, compile_metric_query, compile_metric_query_with_policy,
    execute_metric_query, execute_metric_query_conn, execute_multi_metric_query,
    execute_multi_metric_query_conn, is_aggregate_function_name, is_metric_broken,
    is_metric_broken_for_dataset, validate_create_metric_request, validate_metric_def,
};
pub use session::{
    AnalyticsService, DEFAULT_QUERY_TIMEOUT_MS, LiveConnectionSpec, MAX_QUERY_TIMEOUT_MS,
    PolicyQueryAccess, SessionPool, TenantDb, TenantSession, postgres_dsn_to_libpq,
    register_mask_udfs,
};
pub use sql_guard::{
    CatalogDatasetEntry, DEFAULT_MAX_ROWS, GuardOptions, GuardedQuery, GuardedSql, HARD_MAX_ROWS,
    MAX_RESULT_ROWS, MAX_SQL_BYTES, OutputColumnOrigin, TenantCatalog, guard, guard_and_rewrite,
    guard_for_materialize, guard_with_ceiling, inspect_sql_expression, json_value_to_sql_expr,
};
pub use transform::{
    TRANSFORM_PREVIEW_MAX_ROWS, TRANSFORM_SYSTEM_PROMPT, ValidatedTransformPlan,
    ensure_no_dependency_cycle, generate_transform_sql, preview_transform_sql,
    preview_transform_sql_conn, topological_downstream_dependents, validate_target_dataset_name,
};

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

/// Live-attached relational connection metadata cached for `SessionPool`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogLiveEntry {
    /// Connection UUID.
    pub id: Uuid,
    /// Connection name (`live_<name>`).
    pub name: String,
    /// Connector type (`POSTGRES`, `MYSQL`, `SQLITE`).
    pub connector_type: ConnectorType,
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

/// Installs the required `DuckDB` offline extensions (`ducklake`, `postgres`, `mysql_scanner`,
/// `sqlite_scanner`) into `$HOME/.duckdb/extensions/` during container image build (`05-STEP04` §04.1).
///
/// # Errors
/// Returns [`AppError::Config`] if opening in-memory `DuckDB` or executing `INSTALL` fails.
pub fn install_duckdb_extensions() -> Result<(), AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|err| AppError::Config(format!("duckdb open_in_memory failed: {err}")))?;
    conn.execute_batch(
        "INSTALL ducklake; INSTALL postgres; INSTALL mysql_scanner; INSTALL sqlite_scanner;",
    )
    .map_err(|err| AppError::Config(format!("duckdb extension install failed: {err}")))?;
    Ok(())
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
