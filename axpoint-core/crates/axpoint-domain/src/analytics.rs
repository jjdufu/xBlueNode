//! Domain DTOs, semantic layer models, pivot specs, NL2SQL types, and transform contracts for `AxAnalytics` (`05-STEP04`).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::axhub::{ColumnMeta, Dataset};
use crate::enums::{
    AggFn, ConnectorType, DataType, GroundingMode, Permission, PiiCategory, RollupGrain, WidgetKind,
};

/// Principal context passed into [`crate::ports::AnalyticsQueryPort`] methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsPrincipal {
    /// Authenticated user UUID.
    pub user_id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Login username.
    pub username: String,
    /// Effective permission set.
    pub permissions: BTreeSet<Permission>,
}

impl AnalyticsPrincipal {
    /// Returns `true` if this principal holds `perm`.
    #[must_use]
    pub fn has_permission(&self, perm: Permission) -> bool {
        self.permissions.contains(&perm)
    }
}

/// Dataset entry inside `GET /api/analytics/catalog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDatasetNode {
    /// Dataset UUID.
    pub id: Uuid,
    /// Dataset SQL identifier name.
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Underlying `DuckLake` table name.
    pub lake_table: String,
    /// Current dataset schema version.
    pub schema_version: i32,
    /// Latest committed row count.
    pub row_count: i64,
    /// Column metadata entries.
    pub columns: Vec<ColumnMeta>,
}

/// Platform read-only federation view entry (`ax.nodes`, `ax.links`, `ax.metric_points`, `ax.metric_rollups`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformViewNode {
    /// Two-part view name, e.g. `ax.nodes`.
    pub name: String,
    /// View description.
    pub description: String,
    /// Projected columns.
    pub columns: Vec<QueryColumn>,
}

/// Live-query relational connection alias (`live_<name>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveConnectionNode {
    /// Connection UUID.
    pub id: Uuid,
    /// Connection name.
    pub name: String,
    /// `DuckDB` attached catalog alias (`live_<name>`).
    pub alias: String,
    /// Connector type (`POSTGRES`, `MYSQL`, or `SQLITE`).
    pub connector_type: ConnectorType,
}

/// Response payload for `GET /api/analytics/catalog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsCatalogResponse {
    /// Ready datasets in `DuckLake`.
    pub datasets: Vec<CatalogDatasetNode>,
    /// Federated `ax.*` platform views.
    pub platform_views: Vec<PlatformViewNode>,
    /// Live-attached relational connections.
    pub live_connections: Vec<LiveConnectionNode>,
}

/// Request body for `POST /api/analytics/sql/run` and `POST /api/analytics/sql/explain`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    /// User or compiled `DuckDB` SQL query.
    pub sql: String,
    /// Named `:param` bindings.
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    /// Maximum rows to return (`default = 10_000`, `cap = 100_000`).
    #[serde(default)]
    pub max_rows: Option<u32>,
    /// Query timeout in milliseconds (`default = 30_000`, `cap = 120_000`).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Whether `live_<conn>.*` references are permitted.
    #[serde(default)]
    pub allow_live: bool,
    /// Whether unmasked PII values are requested (requires `PII_RAW_READ`).
    #[serde(default)]
    pub reveal_pii: bool,
    /// When `true`, multi-dataset `SELECT` queries must join exclusively along published ontology edges (`02-STEP16` §16.3.3).
    #[serde(default)]
    pub strict_ontology_join: bool,
}

/// Column descriptor in an `AxAnalytics` result set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryColumn {
    /// Output column name.
    pub name: String,
    /// Logical data type.
    pub data_type: DataType,
    /// Propagated PII classification (`NONE` when non-sensitive).
    pub pii_category: PiiCategory,
    /// `true` if dynamic PII masking or redaction was applied.
    pub masked: bool,
}

/// Response payload for `POST /api/analytics/sql/run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Output column metadata.
    pub columns: Vec<QueryColumn>,
    /// Row records serialized per `A1` §7.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Number of returned rows.
    pub row_count: usize,
    /// `true` if more than `max_rows` matched and output was truncated.
    pub truncated: bool,
    /// Guarded and rewritten SQL executed against `DuckDB`.
    pub sql: String,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Response payload for `POST /api/analytics/sql/explain`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResult {
    /// Guarded and rewritten SQL.
    pub sql: String,
    /// `DuckDB` `EXPLAIN` plan text.
    pub plan: String,
    /// Dataset UUIDs referenced by the query.
    pub referenced_datasets: Vec<Uuid>,
    /// Whether any `ax.*` platform view is referenced.
    pub uses_platform_views: bool,
}

/// Row or column dimension reference in a [`PivotSpec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRef {
    /// Source column identifier.
    pub field: String,
    /// Optional custom header label.
    #[serde(default)]
    pub label: Option<String>,
}

/// Measure aggregation specification in a [`PivotSpec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueSpec {
    /// Source column identifier.
    pub field: String,
    /// Aggregation function (`SUM`, `AVG`, `COUNT`, `COUNT_DISTINCT`, `MIN`, `MAX`, `MEDIAN`, `LAST`).
    pub agg: AggFn,
    /// Optional output alias.
    #[serde(default)]
    pub alias: Option<String>,
    /// Optional numeric format string (e.g. `"0,0.00"`).
    #[serde(default)]
    pub format: Option<String>,
}

/// Filter comparison operator for [`FilterSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOp {
    /// `=`
    #[serde(alias = "EQ", alias = "eq")]
    Eq,
    /// `<>`
    #[serde(alias = "NE", alias = "ne")]
    Ne,
    /// `>`
    #[serde(alias = "GT", alias = "gt")]
    Gt,
    /// `>=`
    #[serde(alias = "GTE", alias = "gte")]
    Gte,
    /// `<`
    #[serde(alias = "LT", alias = "lt")]
    Lt,
    /// `<=`
    #[serde(alias = "LTE", alias = "lte")]
    Lte,
    /// `IN (...)`
    #[serde(alias = "IN", alias = "in")]
    In,
    /// `NOT IN (...)`
    #[serde(alias = "NOT_IN", alias = "not_in")]
    NotIn,
    /// `BETWEEN low AND high`
    #[serde(alias = "BETWEEN", alias = "between")]
    Between,
    /// `LIKE pattern`
    #[serde(alias = "LIKE", alias = "like")]
    Like,
    /// `IS NULL`
    #[serde(alias = "IS_NULL", alias = "is_null")]
    IsNull,
    /// `IS NOT NULL`
    #[serde(alias = "IS_NOT_NULL", alias = "is_not_null")]
    IsNotNull,
}

/// Structured filter clause in [`PivotSpec`], [`MetricQuery`], or [`AskContext`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterSpec {
    /// Column name.
    pub field: String,
    /// Filter operator.
    pub op: FilterOp,
    /// Scalar or array value (ignored for `IsNull` / `IsNotNull`).
    #[serde(default)]
    pub value: serde_json::Value,
}

/// Time bucketing directive in a [`PivotSpec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeBucket {
    /// Date or timestamp column name.
    pub field: String,
    /// Time truncation granularity.
    pub grain: RollupGrain,
}

/// Sort direction in a [`SortSpec`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDir {
    /// Ascending order.
    #[default]
    #[serde(rename = "ASC", alias = "Asc", alias = "asc")]
    Asc,
    /// Descending order.
    #[serde(rename = "DESC", alias = "Desc", alias = "desc")]
    Desc,
}

/// Sort specification in a [`PivotSpec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SortSpec {
    /// Target field or measure alias.
    pub field: String,
    /// Sort direction.
    #[serde(default)]
    pub dir: SortDir,
}

const fn default_pivot_row_limit() -> u32 {
    1_000
}

const fn default_pivot_col_limit() -> u32 {
    50
}

/// Request body for `POST /api/analytics/pivot` (`05-STEP04` §04.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PivotSpec {
    /// Dataset name or federated platform view (e.g. `ax.metric_rollups`).
    pub dataset: String,
    /// Row dimension fields (`<= 5`).
    #[serde(default)]
    pub rows: Vec<FieldRef>,
    /// Column dimension fields (`<= 3`).
    #[serde(default)]
    pub columns: Vec<FieldRef>,
    /// Aggregated value measures (`1..=10`).
    pub values: Vec<ValueSpec>,
    /// Structured filter predicates.
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    /// Optional `date_trunc` bucket applied to a temporal column.
    #[serde(default)]
    pub time_bucket: Option<TimeBucket>,
    /// Optional ordering clauses.
    #[serde(default)]
    pub sort: Vec<SortSpec>,
    /// Emit subtotal rows via `GROUPING SETS`.
    #[serde(default)]
    pub subtotals: bool,
    /// Emit a grand total row `()`.
    #[serde(default)]
    pub grand_total: bool,
    /// Row limit (`1..=10_000`).
    #[serde(default = "default_pivot_row_limit")]
    pub row_limit: u32,
    /// Maximum distinct column combinations (`1..=200`).
    #[serde(default = "default_pivot_col_limit")]
    pub column_limit: u32,
}

/// Classification of each matrix row in [`PivotResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PivotRowKind {
    /// Leaf detail row.
    Data,
    /// Subtotal group row.
    Subtotal,
    /// Overall grand total row.
    GrandTotal,
}

/// Cross-tabulated response payload for `POST /api/analytics/pivot`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PivotResult {
    /// Row dimension labels per matrix row.
    pub row_headers: Vec<Vec<String>>,
    /// Column dimension + measure labels per matrix column.
    pub column_headers: Vec<Vec<String>>,
    /// Formatted cell values (`row_headers.len() x column_headers.len()`).
    pub cells: Vec<Vec<String>>,
    /// Row kind (`DATA`, `SUBTOTAL`, `GRAND_TOTAL`) aligned with `row_headers`.
    pub row_kinds: Vec<PivotRowKind>,
    /// `true` when non-mergeable measures (`AVG`, `MEDIAN`, `COUNT_DISTINCT`) exceeded `column_limit`.
    pub columns_truncated: bool,
    /// Guarded SQL executed for this pivot.
    pub sql: String,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Allowed dimension descriptor on a semantic [`MetricDef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricDimension {
    /// Dimension logical key.
    pub name: String,
    /// Physical column name in the metric's dataset.
    pub column: String,
    /// Display label.
    #[serde(default)]
    pub display_name: String,
}

/// Semantic metric definition stored in `an_metrics` (`05-STEP04` §04.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDef {
    /// Metric UUID.
    pub id: Uuid,
    /// Unique machine name (`^[a-z][a-z0-9_]{0,62}$`).
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Business definition description.
    pub description: String,
    /// Backing dataset UUID.
    pub dataset_id: Uuid,
    /// Aggregate SQL expression (e.g. `SUM(amount)`).
    pub expression: String,
    /// Optional boolean SQL filter predicate (e.g. `status = 'PAID'`).
    pub filter_sql: String,
    /// Optional date/timestamp column for time-series queries.
    pub time_column: Option<String>,
    /// Default time bucket granularity.
    pub default_grain: RollupGrain,
    /// Declared sliceable dimensions.
    pub dimensions: Vec<MetricDimension>,
    /// Display unit (e.g. `"CNY"`, `"%"`).
    pub unit: String,
    /// Display format pattern (e.g. `"0,0.00"`).
    pub format: String,
    /// Synonym phrases for NL2SQL schema linking.
    pub synonyms: Vec<String>,
    /// Official governance certification flag (requires `STUDIO_PUBLISH`).
    pub certified: bool,
    /// Dataset `schema_version` captured when defined/updated.
    pub schema_version: i32,
    /// `true` if a referenced column disappeared in the backing dataset's current schema.
    pub broken: bool,
    /// Optional creator user UUID.
    pub owner_user_id: Option<Uuid>,
    /// Monotonic metric definition version.
    pub version: i32,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

const fn default_metric_grain() -> RollupGrain {
    RollupGrain::Day
}

/// Request body for `POST /api/analytics/metrics`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMetricRequest {
    /// Unique machine name (`^[a-z][a-z0-9_]{0,62}$`).
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Backing dataset UUID.
    pub dataset_id: Uuid,
    /// Validated aggregate SQL expression.
    pub expression: String,
    /// Optional boolean SQL filter expression.
    #[serde(default)]
    pub filter_sql: String,
    /// Optional `DATE` or `TIMESTAMP` column name.
    #[serde(default)]
    pub time_column: Option<String>,
    /// Default time grain (`DAY` if omitted).
    #[serde(default = "default_metric_grain")]
    pub default_grain: RollupGrain,
    /// Allowed dimension definitions.
    #[serde(default)]
    pub dimensions: Vec<MetricDimension>,
    /// Unit label.
    #[serde(default)]
    pub unit: String,
    /// Numeric format string.
    #[serde(default)]
    pub format: String,
    /// Synonym phrases.
    #[serde(default)]
    pub synonyms: Vec<String>,
    /// Certified flag (only honored when caller holds `STUDIO_PUBLISH`).
    #[serde(default)]
    pub certified: bool,
}

/// Request body for `PATCH /api/analytics/metrics/{name}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateMetricRequest {
    /// Updated display name.
    pub display_name: Option<String>,
    /// Updated description.
    pub description: Option<String>,
    /// Updated aggregate SQL expression.
    pub expression: Option<String>,
    /// Updated boolean filter SQL.
    pub filter_sql: Option<String>,
    /// Updated time column (`Some(None)` clears, `Some(Some(col))` sets).
    pub time_column: Option<Option<String>>,
    /// Updated default grain.
    pub default_grain: Option<RollupGrain>,
    /// Updated dimensions list.
    pub dimensions: Option<Vec<MetricDimension>>,
    /// Updated unit.
    pub unit: Option<String>,
    /// Updated format string.
    pub format: Option<String>,
    /// Updated synonyms.
    pub synonyms: Option<Vec<String>>,
    /// Updated certification flag (requires `STUDIO_PUBLISH`).
    pub certified: Option<bool>,
}

/// Half-open `[from, to)` UTC interval for [`MetricQuery`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricTimeRange {
    /// Inclusive start timestamp.
    pub from: DateTime<Utc>,
    /// Exclusive end timestamp.
    pub to: DateTime<Utc>,
}

/// Period-over-period comparison mode in [`MetricQuery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetricCompareMode {
    /// Shift `[from, to)` backward by `(to - from)`.
    PreviousPeriod,
    /// Shift `[from, to)` backward by 1 calendar year.
    SamePeriodLastYear,
}

/// Request body for `POST /api/analytics/metrics/{name}/query`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricQuery {
    /// Dimension names or column names to group by.
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Additional ad-hoc filter predicates.
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    /// Optional `[from, to)` time window.
    #[serde(default)]
    pub time_range: Option<MetricTimeRange>,
    /// Optional time bucket grain override.
    #[serde(default)]
    pub grain: Option<RollupGrain>,
    /// Optional period-over-period comparison mode.
    #[serde(default)]
    pub compare: Option<MetricCompareMode>,
}

/// Request body for `POST /api/analytics/metrics/query-multi`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiMetricQuery {
    /// Metric names to evaluate together (`>= 1`).
    pub metric_names: Vec<String>,
    /// Shared grouping dimensions.
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Shared filter predicates.
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    /// Optional shared `[from, to)` time window.
    #[serde(default)]
    pub time_range: Option<MetricTimeRange>,
    /// Optional time bucket grain override.
    #[serde(default)]
    pub grain: Option<RollupGrain>,
    /// Optional period-over-period comparison mode.
    #[serde(default)]
    pub compare: Option<MetricCompareMode>,
}

/// Response payload for `POST /api/analytics/metrics/{name}/query` and `query-multi`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricQueryResult {
    /// Metric name(s) evaluated.
    pub metrics: Vec<String>,
    /// Output column descriptors.
    pub columns: Vec<QueryColumn>,
    /// Result rows (containing `value` for single-metric queries, plus `period`, dimensions, and optional `compare_value`/`delta`/`delta_ratio`).
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Guarded SQL statement(s) executed.
    pub sql: String,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Kind of saved query in `an_saved_queries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SavedQueryKind {
    /// Raw parameterized SQL query (`spec = {sql}`).
    Sql,
    /// Cross-tabulated [`PivotSpec`].
    Pivot,
    /// Semantic [`MetricQuery`] (`spec = {metric_name, query}`).
    Metric,
    /// Saved NL2SQL question + SQL (`spec = {question, sql}`).
    Ask,
}

/// Parameter declaration on an [`SavedQuery`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryParamSpec {
    /// Parameter identifier referenced as `:name` in SQL.
    pub name: String,
    /// Expected logical data type.
    pub data_type: DataType,
    /// Optional default value when caller omits `:name`.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// Saved query record stored in `an_saved_queries`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQuery {
    /// Saved query UUID.
    pub id: Uuid,
    /// Unique query name within tenant.
    pub name: String,
    /// Saved query kind.
    pub kind: SavedQueryKind,
    /// Kind-specific specification JSON.
    pub spec: serde_json::Value,
    /// Declared parameters.
    pub parameters: Vec<QueryParamSpec>,
    /// Creator user UUID.
    pub owner_user_id: Option<Uuid>,
    /// Whether visible to all users in the tenant.
    pub shared: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Request body for `POST /api/analytics/saved-queries`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSavedQueryRequest {
    /// Unique query name within tenant.
    pub name: String,
    /// Query kind.
    pub kind: SavedQueryKind,
    /// Query specification JSON.
    pub spec: serde_json::Value,
    /// Optional declared parameters.
    #[serde(default)]
    pub parameters: Vec<QueryParamSpec>,
    /// Share across tenant (`false` by default).
    #[serde(default)]
    pub shared: bool,
}

/// Request body for `PATCH /api/analytics/saved-queries/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSavedQueryRequest {
    /// Optional new name.
    pub name: Option<String>,
    /// Optional updated spec JSON.
    pub spec: Option<serde_json::Value>,
    /// Optional updated parameter declarations.
    pub parameters: Option<Vec<QueryParamSpec>>,
    /// Optional updated shared flag.
    pub shared: Option<bool>,
}

/// Request body for `POST /api/analytics/saved-queries/{id}/run`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSavedQueryRequest {
    /// Runtime `:name` parameter values.
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
}

/// Execution origin kind recorded in `an_query_history`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QueryHistoryKind {
    /// Ad-hoc or saved SQL execution.
    Sql,
    /// Pivot execution.
    Pivot,
    /// Semantic metric execution.
    Metric,
    /// NL2SQL execution.
    Ask,
    /// Transform preview execution.
    Transform,
}

impl QueryHistoryKind {
    /// Canonical database string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sql => "SQL",
            Self::Pivot => "PIVOT",
            Self::Metric => "METRIC",
            Self::Ask => "ASK",
            Self::Transform => "TRANSFORM",
        }
    }
}

/// Terminal outcome recorded in `an_query_history`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QueryHistoryStatus {
    /// Query completed cleanly.
    Succeeded,
    /// Runtime execution or repair failed.
    Failed,
    /// Blocked by `sql_guard`.
    Rejected,
    /// Interrupted due to timeout.
    Timeout,
}

impl QueryHistoryStatus {
    /// Canonical database string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Rejected => "REJECTED",
            Self::Timeout => "TIMEOUT",
        }
    }
}

/// Input payload to append a row into `an_query_history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewQueryHistoryEntry {
    /// Executing user UUID.
    pub user_id: Option<Uuid>,
    /// Query category.
    pub kind: QueryHistoryKind,
    /// SQL text (truncated to 4,000 chars before persistence).
    pub sql_text: String,
    /// Execution status.
    pub status: QueryHistoryStatus,
    /// Returned row count, if succeeded.
    pub row_count: Option<i64>,
    /// Duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Canonical error code when failed/rejected/timed out.
    pub error_code: Option<String>,
}

/// Persisted row returned by `GET /api/analytics/history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    /// Monotonic history ID.
    pub id: i64,
    /// Executing user UUID.
    pub user_id: Option<Uuid>,
    /// Query category.
    pub kind: QueryHistoryKind,
    /// Executed SQL text.
    pub sql_text: String,
    /// Execution status.
    pub status: QueryHistoryStatus,
    /// Returned row count.
    pub row_count: Option<i64>,
    /// Duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Optional error code.
    pub error_code: Option<String>,
    /// Execution timestamp.
    pub created_at: DateTime<Utc>,
}

/// Query parameters for `GET /api/analytics/history`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryHistoryListQuery {
    /// Optional filter by kind.
    pub kind: Option<QueryHistoryKind>,
    /// Optional filter by status.
    pub status: Option<QueryHistoryStatus>,
    /// Maximum items (`1..=500`, default `50`).
    pub limit: Option<u32>,
    /// Pagination cursor (`id` < cursor).
    pub cursor: Option<String>,
}

/// Intent routed by `nl2sql::intent` and `/api/chat/query` (`05-STEP04` §04.6.2, §04.8).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AskIntent {
    /// Document / hypergraph RAG retrieval.
    #[default]
    Doc,
    /// Structured data / NL2SQL analytical query.
    Data,
    /// Combined hypergraph RAG + structured NL2SQL synthesis.
    Hybrid,
    /// Conversational greeting / non-data chitchat.
    Chitchat,
}

impl AskIntent {
    /// Canonical database string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Doc => "DOC",
            Self::Data => "DATA",
            Self::Hybrid => "HYBRID",
            Self::Chitchat => "CHITCHAT",
        }
    }
}

/// Explicit routing mode on `POST /api/chat/query` (`05-STEP04` §04.8).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChatQueryMode {
    /// Automatic intent classification (`DOC` / `DATA` / `HYBRID` / `CHITCHAT`).
    #[default]
    Auto,
    /// Force document hypergraph RAG pipeline.
    Doc,
    /// Force `AxAnalytics` NL2SQL pipeline.
    Data,
}

/// User thumbs feedback on an NL2SQL turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AskFeedbackKind {
    /// Positive feedback (eligible as few-shot example).
    Up,
    /// Negative feedback.
    Down,
}

impl AskFeedbackKind {
    /// Canonical database string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
        }
    }
}

/// Explicit session-pinned retrieval and analytical scope (`06-STEP20` §20.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatQueryScope {
    /// Restrict structured SQL / metric candidates to these `ds_datasets.id` UUIDs.
    #[serde(default)]
    pub dataset_ids: Vec<Uuid>,
    /// Restrict hypergraph document recall to these `hg_docs.id` UUIDs.
    #[serde(default)]
    pub doc_ids: Vec<Uuid>,
    /// Optional document title prefix filter (`hg_docs.title LIKE prefix || '%'`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_title_prefix: Option<String>,
    /// Optional business subgraph slug bounding `NodeTypeSpec`, `xb_nodes`, and ontology routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph_slug: Option<String>,
    /// Optional published capability endpoint slugs (`ax_published_endpoints.slug`) pinned for hybrid invocation.
    #[serde(default)]
    pub endpoint_slugs: Vec<String>,
}

impl ChatQueryScope {
    /// Returns `true` when no scope constraint is active after trimming string fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let prefix_empty = self
            .doc_title_prefix
            .as_deref()
            .is_none_or(|s| s.trim().is_empty());
        let subgraph_empty = self
            .subgraph_slug
            .as_deref()
            .is_none_or(|s| s.trim().is_empty());
        let endpoints_empty = self.endpoint_slugs.iter().all(|s| s.trim().is_empty());
        self.dataset_ids.is_empty()
            && self.doc_ids.is_empty()
            && prefix_empty
            && subgraph_empty
            && endpoints_empty
    }

    /// Trims string fields, deduplicates UUID and slug lists while preserving order, and returns `None` if empty.
    #[must_use]
    pub fn normalized(&self) -> Option<Self> {
        let mut seen_ds = BTreeSet::new();
        let dataset_ids: Vec<Uuid> = self
            .dataset_ids
            .iter()
            .copied()
            .filter(|id| seen_ds.insert(*id))
            .collect();

        let mut seen_docs = BTreeSet::new();
        let doc_ids: Vec<Uuid> = self
            .doc_ids
            .iter()
            .copied()
            .filter(|id| seen_docs.insert(*id))
            .collect();

        let doc_title_prefix = self
            .doc_title_prefix
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        let subgraph_slug = self
            .subgraph_slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        let mut seen_eps = BTreeSet::new();
        let endpoint_slugs: Vec<String> = self
            .endpoint_slugs
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter(|s| seen_eps.insert(*s))
            .map(ToOwned::to_owned)
            .collect();

        let cleaned = Self {
            dataset_ids,
            doc_ids,
            doc_title_prefix,
            subgraph_slug,
            endpoint_slugs,
        };
        (!cleaned.is_empty()).then_some(cleaned)
    }
}

/// Optional dashboard/dataset scoping context on `POST /api/analytics/ask`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskContext {
    /// Restrict candidate schema linking to these dataset UUIDs.
    #[serde(default)]
    pub dataset_ids: Vec<Uuid>,
    /// Restrict hypergraph document recall to these `hg_docs.id` UUIDs (`06-STEP20` §20.2.1).
    #[serde(default)]
    pub doc_ids: Vec<Uuid>,
    /// Optional document title prefix filter (`06-STEP20` §20.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_title_prefix: Option<String>,
    /// Optional business subgraph slug (`06-STEP20` §20.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph_slug: Option<String>,
    /// Optional pinned published capability endpoint slugs (`06-STEP20` §20.2.1).
    #[serde(default)]
    pub endpoint_slugs: Vec<String>,
    /// Additional dashboard filter constraints injected into prompt.
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    /// Active drill-down breadcrumb path.
    #[serde(default)]
    pub drill_path: Vec<String>,
    /// Originating dashboard UUID (Step 11).
    #[serde(default)]
    pub dashboard_id: Option<Uuid>,
}

/// Request body for `POST /api/analytics/ask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    /// Natural-language analytical question.
    pub question: String,
    /// Optional multi-turn conversation session UUID (generated if omitted).
    #[serde(default)]
    pub session_id: Option<Uuid>,
    /// Optional scoping and filter context.
    #[serde(default)]
    pub context: Option<AskContext>,
}

/// Recommended chart configuration returned by NL2SQL summarization (`§04.6.6`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartSuggestion {
    /// Recommended widget visualization kind (`LINE_CHART`, `BAR_CHART`, `PIE_CHART`, `KPI_CARD`, `TABLE`).
    pub kind: WidgetKind,
    /// Recommended X-axis or category column.
    #[serde(default)]
    pub x_field: Option<String>,
    /// Recommended numeric Y-axis / value columns.
    #[serde(default)]
    pub y_fields: Vec<String>,
    /// Optional chart title.
    #[serde(default)]
    pub title: Option<String>,
}

/// Schema-linked objects recorded on an `an_ask_turns` row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LinkedObjects {
    /// Linked dataset names.
    #[serde(default)]
    pub datasets: Vec<String>,
    /// Linked `dataset.column` identifiers.
    #[serde(default)]
    pub columns: Vec<String>,
    /// Linked semantic metric names.
    #[serde(default)]
    pub metrics: Vec<String>,
    /// Matched `BlueNode` keys (`type_name:node_key`).
    #[serde(default)]
    pub nodes: Vec<String>,
}

/// Document paragraph citation surfaced in `DOC` and `HYBRID` answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskCitation {
    /// Paragraph UUID.
    pub para_id: Uuid,
    /// Document UUID.
    pub doc_id: Uuid,
    /// Document title.
    pub doc_title: String,
    /// Excerpt snippet.
    pub snippet: String,
}

/// Summary of a linked `BlueNode` surfaced in `/api/chat/query` (`§04.8`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedNodeSummary {
    /// Business node UUID.
    pub id: Uuid,
    /// Business node type name.
    pub type_name: String,
    /// Business node title.
    pub title: String,
    /// Latest node metric map.
    pub metrics: serde_json::Value,
}

/// Response payload for `POST /api/analytics/ask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResponse {
    /// Persisted `an_ask_turns.id`.
    pub turn_id: Uuid,
    /// Session UUID.
    pub session_id: Uuid,
    /// Classified intent.
    pub intent: AskIntent,
    /// Execution status (`SUCCEEDED` or `FAILED`).
    pub status: QueryHistoryStatus,
    /// Generated and guarded SQL (`""` when `need_clarification`, `CHITCHAT`, or `DOC`).
    pub sql: String,
    /// Result columns.
    pub columns: Vec<QueryColumn>,
    /// Result rows (`<= 1_000`, PII-masked).
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Whether result rows were truncated at `1_000`.
    pub truncated: bool,
    /// Verified natural-language answer or clarification prompt.
    pub answer: String,
    /// Recommended chart visualization.
    pub chart: Option<ChartSuggestion>,
    /// `true` when the LLM requested user clarification instead of emitting SQL.
    pub need_clarification: bool,
    /// Model explanation or clarification message.
    pub explanation: String,
    /// Schema-linked datasets, columns, metrics, and nodes.
    pub linked_objects: LinkedObjects,
    /// Document citations (when `DOC` or `HYBRID`).
    pub citations: Vec<AskCitation>,
    /// Execution and verification path taken (`02-STEP16` §16.2.1).
    #[serde(default)]
    pub grounding_mode: GroundingMode,
    /// Count of verified in-table numeric values in `answer` (`02-STEP16` §16.2.1).
    #[serde(default)]
    pub verified_numbers: u32,
    /// Count of verified in-bounds `[文N]` citations in `answer` (`02-STEP16` §16.2.1).
    #[serde(default)]
    pub verified_citations: u32,
    /// `true` when zero actionable evidence was recalled and free-form synthesis was skipped (`02-STEP16` §16.2.1).
    #[serde(default)]
    pub abstained: bool,
    /// Structured slot payload compiled via Stage 0 ontology routing (`02-STEP16` §16.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_payload: Option<serde_json::Value>,
}

/// Persisted conversation turn returned by `GET /api/analytics/ask/sessions/{session_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskTurnRecord {
    /// Turn UUID.
    pub id: Uuid,
    /// Session UUID.
    pub session_id: Uuid,
    /// Asking user UUID.
    pub user_id: Option<Uuid>,
    /// Original user question.
    pub question: String,
    /// Routed intent.
    pub intent: AskIntent,
    /// Linked schema objects.
    pub linked_objects: LinkedObjects,
    /// Executed SQL text, if any.
    pub sql_text: Option<String>,
    /// Result preview JSON (`{columns, rows, truncated, chart}`).
    pub result_preview: Option<serde_json::Value>,
    /// Synthesized answer.
    pub answer: String,
    /// Document citations.
    pub citations: Vec<AskCitation>,
    /// User feedback (`UP` / `DOWN`).
    pub feedback: Option<AskFeedbackKind>,
    /// User-supplied corrected SQL (validated via `sql_guard`).
    pub corrected_sql: Option<String>,
    /// Model identifier used.
    pub model_name: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Execution and verification path taken (`02-STEP16` §16.1).
    #[serde(default)]
    pub grounding_mode: GroundingMode,
    /// Count of verified in-table numeric values in `answer`.
    #[serde(default)]
    pub verified_numbers: u32,
    /// Count of verified in-bounds `[文N]` citations in `answer`.
    #[serde(default)]
    pub verified_citations: u32,
    /// Whether the turn deterministically abstained due to zero recalled evidence.
    #[serde(default)]
    pub abstained: bool,
    /// Structured slot payload compiled via Stage 0 ontology routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_payload: Option<serde_json::Value>,
}

/// Input payload to persist a new row in `an_ask_turns`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAskTurnInput {
    /// Session UUID.
    pub session_id: Uuid,
    /// Asking user UUID.
    pub user_id: Option<Uuid>,
    /// User question.
    pub question: String,
    /// Routed intent.
    pub intent: AskIntent,
    /// Linked schema objects.
    pub linked_objects: LinkedObjects,
    /// Executed SQL text, if any.
    pub sql_text: Option<String>,
    /// Result preview JSON.
    pub result_preview: Option<serde_json::Value>,
    /// Synthesized answer.
    pub answer: String,
    /// Document citations.
    pub citations: Vec<AskCitation>,
    /// Model identifier.
    pub model_name: String,
    /// Execution and verification path taken (`02-STEP16` §16.1).
    #[serde(default)]
    pub grounding_mode: GroundingMode,
    /// Count of verified in-table numeric values in `answer`.
    #[serde(default)]
    pub verified_numbers: u32,
    /// Count of verified in-bounds `[文N]` citations in `answer`.
    #[serde(default)]
    pub verified_citations: u32,
    /// Whether the turn deterministically abstained due to zero recalled evidence.
    #[serde(default)]
    pub abstained: bool,
    /// Structured slot payload compiled via Stage 0 ontology routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_payload: Option<serde_json::Value>,
}

/// Declared legal join edge between two physical datasets derived from published `xBlueStudio` mappings (`02-STEP16` §16.3.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AllowedDatasetJoinEdge {
    /// Left dataset SQL identifier (lowercased).
    pub left_dataset: String,
    /// Left dataset column identifier (lowercased).
    pub left_column: String,
    /// Right dataset SQL identifier (lowercased).
    pub right_dataset: String,
    /// Right dataset column identifier (lowercased).
    pub right_column: String,
    /// Originating ontology `EdgeTypeSpec.name` or `"SAME_ENTITY:<NodeType>"`.
    pub relationship_label: String,
}

impl AllowedDatasetJoinEdge {
    /// Returns `true` if `(ds_a, col_a)` and `(ds_b, col_b)` match this edge in either direction (case-insensitive).
    #[must_use]
    pub fn matches_undirected(&self, ds_a: &str, col_a: &str, ds_b: &str, col_b: &str) -> bool {
        let forward = self.left_dataset.eq_ignore_ascii_case(ds_a)
            && self.left_column.eq_ignore_ascii_case(col_a)
            && self.right_dataset.eq_ignore_ascii_case(ds_b)
            && self.right_column.eq_ignore_ascii_case(col_b);
        let reverse = self.left_dataset.eq_ignore_ascii_case(ds_b)
            && self.left_column.eq_ignore_ascii_case(col_b)
            && self.right_dataset.eq_ignore_ascii_case(ds_a)
            && self.right_column.eq_ignore_ascii_case(col_a);
        forward || reverse
    }
}

/// Structured filter item emitted by full `JsonSchemaSpec::ontology_slot_extraction` (`02-STEP16` §16.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologySlotFilter {
    /// Column or property identifier.
    pub field: String,
    /// Comparison operator (`EQ`, `NE`, `GT`, `GTE`, `LT`, `LTE`, `IN`, `NOT_IN`, `LIKE`, `IS_NULL`, `IS_NOT_NULL`).
    pub op: String,
    /// Scalar or comma-separated literal text.
    pub value_text: String,
}

/// Stage 0 structured slot draft decoded from either full cloud JSON Schema or compact local-7B JSON Schema (`02-STEP16` §16.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologySlotDraft {
    /// Selected route (`SINGLE_METRIC` | `MULTI_METRIC` | `DOMAIN_OBJECT` | `CLARIFY` | `FALLBACK_SQL`).
    pub route: String,
    /// Matched `MetricDef.name` identifiers.
    #[serde(default)]
    pub metric_names: Vec<String>,
    /// Matched `NodeTypeSpec.name` (normalized from `null` or `""` to `None`).
    #[serde(default)]
    pub target_node_type: Option<String>,
    /// Requested group-by dimension names.
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Structured filter items (from full schema).
    #[serde(default)]
    pub filters: Vec<OntologySlotFilter>,
    /// Compact filter expressions (from local-7B schema, e.g. `"region = '华东'"`).
    #[serde(default)]
    pub filter_exprs: Vec<String>,
    /// Inclusive lower ISO-8601 timestamp bound, if any.
    #[serde(default)]
    pub time_from: Option<String>,
    /// Exclusive upper ISO-8601 timestamp bound, if any.
    #[serde(default)]
    pub time_to: Option<String>,
    /// Optional time rollup grain (`MINUTE` | `HOUR` | `DAY` | `MONTH`).
    #[serde(default)]
    pub grain: Option<RollupGrain>,
    /// Optional period comparison (`PREVIOUS_PERIOD` | `SAME_PERIOD_LAST_YEAR`).
    #[serde(default)]
    pub compare: Option<MetricCompareMode>,
    /// Incident `EdgeTypeSpec.name` links requested for `DOMAIN_OBJECT` queries.
    #[serde(default)]
    pub include_links: Vec<String>,
    /// Clarification prompt when `route == "CLARIFY"`.
    #[serde(default)]
    pub clarification: String,
}

impl OntologySlotDraft {
    /// Normalizes a `serde_json::Value` conforming to either full or local-7B compact `axpoint_ontology_slots` into [`Self`].
    ///
    /// # Errors
    /// Returns [`crate::AppError::BadRequest`] if required fields have incompatible types or invalid enum values.
    pub fn from_structured_value(val: &serde_json::Value) -> Result<Self, crate::AppError> {
        let obj = val.as_object().ok_or_else(|| {
            crate::AppError::bad(
                "INVALID_ARGUMENT",
                "ontology slot payload must be a JSON object",
            )
        })?;
        let route = obj
            .get("route")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("FALLBACK_SQL")
            .trim()
            .to_owned();
        let metric_names = parse_str_array(obj.get("metric_names"));
        let target_node_type = parse_non_empty_opt_str(obj.get("target_node_type"));
        let dimensions = parse_str_array(obj.get("dimensions"));
        let filters = obj
            .get("filters")
            .cloned()
            .map(serde_json::from_value::<Vec<OntologySlotFilter>>)
            .transpose()
            .map_err(|e| {
                crate::AppError::bad("INVALID_ARGUMENT", format!("invalid slot filters: {e}"))
            })?
            .unwrap_or_default();
        let filter_exprs = parse_str_array(obj.get("filter_exprs"));
        let time_from = parse_non_empty_opt_str(obj.get("time_from"));
        let time_to = parse_non_empty_opt_str(obj.get("time_to"));
        let grain = match parse_non_empty_opt_str(obj.get("grain")) {
            Some(g) => Some(g.parse::<RollupGrain>().map_err(|_| {
                crate::AppError::bad("INVALID_ARGUMENT", format!("invalid slot grain '{g}'"))
            })?),
            None => None,
        };
        let compare = match parse_non_empty_opt_str(obj.get("compare")) {
            Some(c) => Some(
                serde_json::from_value::<MetricCompareMode>(serde_json::Value::String(c)).map_err(
                    |e| {
                        crate::AppError::bad(
                            "INVALID_ARGUMENT",
                            format!("invalid slot compare: {e}"),
                        )
                    },
                )?,
            ),
            None => None,
        };
        let include_links = parse_str_array(obj.get("include_links"));
        let clarification = obj
            .get("clarification")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();

        Ok(Self {
            route,
            metric_names,
            target_node_type,
            dimensions,
            filters,
            filter_exprs,
            time_from,
            time_to,
            grain,
            compare,
            include_links,
            clarification,
        })
    }
}

fn parse_non_empty_opt_str(val: Option<&serde_json::Value>) -> Option<String> {
    val.and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_str_array(val: Option<&serde_json::Value>) -> Vec<String> {
    val.and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
}

/// Evaluation track in `an_grounding_eval_runs` (`0016_grounded_ask_eval.sql`, `02-STEP16` §16.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GroundingEvalTrack {
    /// Cloud frontier/standard model replay track (`hallucination_cloud_50.jsonl`).
    Cloud,
    /// Local 7B compact & noisy model replay track (`hallucination_local7b_50.jsonl`).
    Local7b,
}

impl GroundingEvalTrack {
    /// Canonical database wire string (`CLOUD` | `LOCAL_7B`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cloud => "CLOUD",
            Self::Local7b => "LOCAL_7B",
        }
    }
}

/// Persisted benchmark run record in `an_grounding_eval_runs` (`02-STEP16` §16.1, §16.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingEvalRunRecord {
    /// Evaluation run UUID.
    pub id: Uuid,
    /// Suite identifier (e.g. `"hallucination_50"`).
    pub suite_name: String,
    /// Replay track (`CLOUD` | `LOCAL_7B`).
    pub track: GroundingEvalTrack,
    /// Model profile identifier.
    pub model_name: String,
    /// Total benchmark cases evaluated.
    pub total_cases: u32,
    /// Cases whose hallucination vector was deterministically intercepted.
    pub intercepted_cases: u32,
    /// Pass rate in `[0.0, 1.0]`.
    pub pass_rate: f64,
    /// Per-category pass breakdown JSON (`CAT1_METRIC_SLOT`..`CAT5_ZERO_EVIDENCE`).
    pub category_breakdown: serde_json::Value,
    /// Failure details array (empty when `pass_rate == 1.0`).
    pub failures: serde_json::Value,
    /// Suite execution duration in milliseconds.
    pub duration_ms: u64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Input payload to persist a new row in `an_grounding_eval_runs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewGroundingEvalRunInput {
    /// Suite identifier.
    pub suite_name: String,
    /// Replay track (`CLOUD` | `LOCAL_7B`).
    pub track: GroundingEvalTrack,
    /// Model profile identifier.
    pub model_name: String,
    /// Total benchmark cases evaluated.
    pub total_cases: u32,
    /// Intercepted benchmark cases.
    pub intercepted_cases: u32,
    /// Pass rate in `[0.0, 1.0]`.
    pub pass_rate: f64,
    /// Per-category pass breakdown JSON.
    pub category_breakdown: serde_json::Value,
    /// Failure details array.
    pub failures: serde_json::Value,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Response payload for `POST /api/analytics/grounding-eval/run` (`02-STEP16` §16.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingEvalRunResponse {
    /// Persisted `CLOUD` and `LOCAL_7B` evaluation run records.
    pub runs: Vec<GroundingEvalRunRecord>,
}

/// Request body for `POST /api/analytics/ask/{turn_id}/feedback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskFeedbackRequest {
    /// Thumbs `UP` or `DOWN`.
    pub feedback: AskFeedbackKind,
    /// Optional user-corrected SQL (validated through `sql_guard` before saving).
    #[serde(default)]
    pub corrected_sql: Option<String>,
}

/// Candidate column recalled during NL2SQL schema linking (`§04.6.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedColumnHit {
    /// Parent dataset UUID.
    pub dataset_id: Uuid,
    /// Parent dataset SQL name.
    pub dataset_name: String,
    /// Column name.
    pub column_name: String,
    /// Logical data type.
    pub data_type: DataType,
    /// Column description.
    pub description: String,
    /// Linked business glossary term.
    pub business_term: String,
    /// Measurement unit.
    pub unit: String,
    /// PII classification.
    pub pii_category: PiiCategory,
    /// Up to 3 masked sample values.
    pub sample_values: Vec<String>,
    /// Normalized lexical/synonym score in `[0.0, 1.0]`.
    pub fts_score: f64,
    /// Cosine vector similarity score in `[0.0, 1.0]`.
    pub vec_score: f64,
}

/// Candidate semantic metric recalled during NL2SQL schema linking (`§04.6.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedMetricHit {
    /// Metric definition.
    pub metric: MetricDef,
    /// Normalized FTS rank score in `[0.0, 1.0]`.
    pub fts_score: f64,
    /// Cosine vector similarity score in `[0.0, 1.0]`.
    pub vec_score: f64,
}

/// Combined schema-linking recall bundle returned by `AnalyticsRepository::recall_schema_candidates`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaRecallBundle {
    /// Candidate columns across datasets.
    pub columns: Vec<LinkedColumnHit>,
    /// Candidate semantic metrics.
    pub metrics: Vec<LinkedMetricHit>,
    /// Matched business node `(type_name, node_key, title)` tuples.
    pub matched_nodes: Vec<(String, String, String)>,
}

/// Positive-feedback or user-corrected few-shot example from `an_ask_turns`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FewShotTurn {
    /// Historical question.
    pub question: String,
    /// Verified or corrected SQL.
    pub sql_text: String,
}

/// Lifecycle state of an AI data transformation draft (`an_transform_drafts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransformDraftStatus {
    /// Editable draft awaiting confirmation.
    Draft,
    /// Applied and materialized into a `DERIVED_VIEW` dataset.
    Applied,
    /// Discarded by user.
    Discarded,
}

impl TransformDraftStatus {
    /// Canonical database string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Applied => "APPLIED",
            Self::Discarded => "DISCARDED",
        }
    }
}

/// Preview snapshot stored on an [`TransformDraft`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransformPreview {
    /// Output columns of the transformation SQL.
    #[serde(default)]
    pub columns: Vec<QueryColumn>,
    /// Up to 100 PII-masked preview rows.
    #[serde(default)]
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Estimated or previewed row count.
    #[serde(default)]
    pub row_count_estimate: i64,
    /// Columns added relative to source datasets.
    #[serde(default)]
    pub added_columns: Vec<String>,
    /// Source columns omitted from transformation output.
    #[serde(default)]
    pub removed_columns: Vec<String>,
}

/// AI data transformation draft stored in `an_transform_drafts` (`§04.7`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformDraft {
    /// Draft UUID.
    pub id: Uuid,
    /// Author user UUID.
    pub user_id: Option<Uuid>,
    /// Natural-language transformation instruction.
    pub instruction: String,
    /// Input dataset UUIDs.
    pub source_dataset_ids: Vec<Uuid>,
    /// User-facing `SELECT` SQL (before `lake."tenant_<id>"` rewrite).
    pub sql_text: String,
    /// Dry-run preview (`<= 100` rows + column diff).
    pub preview: TransformPreview,
    /// Target `DERIVED_VIEW` dataset name.
    pub target_dataset_name: String,
    /// Draft status.
    pub status: TransformDraftStatus,
    /// Created dataset UUID once applied.
    pub dataset_id: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Persistence input to insert a row into `an_transform_drafts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTransformDraftInput {
    /// Author user UUID.
    pub user_id: Option<Uuid>,
    /// Natural-language instruction.
    pub instruction: String,
    /// Source dataset UUIDs.
    pub source_dataset_ids: Vec<Uuid>,
    /// Generated `SELECT` SQL.
    pub sql_text: String,
    /// Dry-run preview.
    pub preview: TransformPreview,
    /// Target dataset name (`^[a-z][a-z0-9_]{0,62}$`).
    pub target_dataset_name: String,
}

/// Request body for `POST /api/analytics/transform/draft`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTransformDraftRequest {
    /// Natural-language cleaning / join / aggregation instruction.
    pub instruction: String,
    /// Source dataset UUIDs or names (UUIDs per `§04.7`).
    pub source_datasets: Vec<Uuid>,
    /// Target `DERIVED_VIEW` dataset machine name (`^[a-z][a-z0-9_]{0,62}$`).
    pub target_dataset_name: String,
}

/// Request body for `PATCH /api/analytics/transform/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTransformDraftRequest {
    /// Edited `SELECT` SQL text to re-validate and re-preview.
    pub sql_text: String,
}

/// Response payload for `POST /api/analytics/transform/{id}/apply`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyTransformResponse {
    /// Updated transform draft (`status = APPLIED`).
    pub draft: TransformDraft,
    /// Newly created `DERIVED_VIEW` dataset.
    pub dataset: Dataset,
    /// Enqueued `SYNC_DATASET` job UUID.
    pub job_id: Uuid,
}

/// Internal Sidecar request for `POST /lake/materialize` (`§04.7`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializeRequest {
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Backing `DERIVED_VIEW` dataset UUID.
    pub dataset_id: String,
    /// Target `DuckLake` table name.
    pub lake_table: String,
    /// Rewritten `DuckDB` `SELECT` query referencing only `lake."tenant_<id>".*`.
    pub sql: String,
    /// Idempotent sync batch UUID string.
    pub batch_id: String,
}

/// Deterministic `UUIDv5` namespace helper for `DS_COLUMN` embeddings (`UUIDv5(dataset_id, column_name)` per `§04.6.3`).
#[must_use]
pub fn ds_column_embedding_id(dataset_id: Uuid, column_name: &str) -> Uuid {
    Uuid::new_v5(&dataset_id, column_name.as_bytes())
}

/// Declarative join-free ontology query request (`POST /api/analytics/axql/compile` and `POST /api/analytics/axql/run`, `05-STEP27` §27.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxqlQueryRequest {
    /// `AxQL` statement (`FROM OBJECT(...)`, `FROM MATCH (...)`, `SUM_BY_ENTITY(...)`, `MATCH_KNOWLEDGE(...)`).
    pub axql: String,
    /// Named `$param` / `:param` values substituted into parameter placeholders.
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    /// Optional row limit (`1..=50_000`, default `1_000`).
    #[serde(default)]
    pub max_rows: Option<u32>,
    /// Optional timeout in milliseconds (`100..=60_000`, default `15_000`).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Explicit PII reveal request (`403` unless authorized).
    #[serde(default)]
    pub reveal_pii: bool,
    /// Target ontology subgraph slug (`"main"` default).
    #[serde(default)]
    pub subgraph_slug: Option<String>,
}

/// Deterministic compilation plan produced by `axpoint-analytics::axql` (`POST /api/analytics/axql/compile`, `05-STEP27` §27.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxqlCompilePlan {
    /// Guarded `DuckDB` SQL statement after cross-tier `OBJECT(...)` folding, `MATCH` path expansion, and `SUM_BY_ENTITY` rewriting.
    pub compiled_sql: String,
    /// Referenced `NodeTypeSpec.name` identifiers verified against `SchemaDoc`.
    #[serde(default)]
    pub referenced_node_types: Vec<String>,
    /// Referenced `EdgeTypeSpec.name` identifiers verified against `SchemaDoc`.
    #[serde(default)]
    pub referenced_edge_types: Vec<String>,
    /// Underlying `ds_datasets.name` tables or `ax.*` platform views bound in the compiled plan.
    #[serde(default)]
    pub referenced_datasets: Vec<String>,
    /// Aliases protected by `SUM_BY_ENTITY` / `AVG_BY_ENTITY` entity-key deduplication.
    #[serde(default)]
    pub dedup_entities: Vec<String>,
    /// Count of `xb_nodes.id` candidates recalled via `MATCH_KNOWLEDGE(...)` hypergraph bridge.
    #[serde(default)]
    pub knowledge_recall_nodes: usize,
    /// `true` when 1:1 pre-aggregated CTE folding or `*_BY_ENTITY` deduplication was applied to prevent fanout inflation.
    #[serde(default)]
    pub fanout_protected: bool,
}

/// Combined compilation plan and masked tabular execution result returned by `POST /api/analytics/axql/run` (`05-STEP27` §27.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxqlQueryResponse {
    /// Compilation plan metadata (`compiled_sql`, referenced ontology elements, fanout protection).
    pub plan: AxqlCompilePlan,
    /// Guarded `DuckDB` tabular result (flattened so `.columns` and `.rows` are accessible at top level as well as `.plan`).
    #[serde(flatten)]
    pub result: QueryResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_request_rejects_tenant_id_override() {
        let raw = r#"{"tenant_id":"other","sql":"SELECT 1"}"#;
        assert!(serde_json::from_str::<QueryRequest>(raw).is_err());
    }

    #[test]
    fn filter_op_accepts_both_pascal_and_upper_case() {
        let f1: FilterSpec = serde_json::from_str(r#"{"field":"status","op":"Eq","value":"PAID"}"#)
            .ok()
            .unwrap_or(FilterSpec {
                field: String::new(),
                op: FilterOp::IsNull,
                value: serde_json::Value::Null,
            });
        let f2: FilterSpec = serde_json::from_str(r#"{"field":"status","op":"EQ","value":"PAID"}"#)
            .ok()
            .unwrap_or(FilterSpec {
                field: String::new(),
                op: FilterOp::IsNull,
                value: serde_json::Value::Null,
            });
        assert_eq!(f1.op, FilterOp::Eq);
        assert_eq!(f2.op, FilterOp::Eq);
    }

    #[test]
    fn ds_column_embedding_id_is_deterministic() {
        let ds_id = Uuid::nil();
        let a = ds_column_embedding_id(ds_id, "amount");
        let b = ds_column_embedding_id(ds_id, "amount");
        let c = ds_column_embedding_id(ds_id, "status");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn chat_query_scope_normalizes_and_deduplicates() {
        let empty = ChatQueryScope {
            doc_title_prefix: Some(String::from("   ")),
            subgraph_slug: Some(String::new()),
            endpoint_slugs: vec![String::from("  ")],
            ..ChatQueryScope::default()
        };
        assert!(empty.is_empty());
        assert_eq!(empty.normalized(), None);

        let ds = Uuid::new_v4();
        let scoped = ChatQueryScope {
            dataset_ids: vec![ds, ds],
            doc_ids: Vec::new(),
            doc_title_prefix: Some(String::from("  SOP-2026- ")),
            subgraph_slug: Some(String::from(" main ")),
            endpoint_slugs: vec![String::from("order_lookup"), String::from(" order_lookup ")],
        };
        let norm = scoped.normalized();
        assert_eq!(
            norm,
            Some(ChatQueryScope {
                dataset_ids: vec![ds],
                doc_ids: Vec::new(),
                doc_title_prefix: Some(String::from("SOP-2026-")),
                subgraph_slug: Some(String::from("main")),
                endpoint_slugs: vec![String::from("order_lookup")],
            })
        );
    }
}
