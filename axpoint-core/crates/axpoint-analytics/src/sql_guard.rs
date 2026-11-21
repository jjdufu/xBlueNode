//! AST-based `DuckDB` SQL security guard, identifier rewriter, parameter binder,
//! and output column lineage tracker (`05-STEP04` §04.3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{ColumnMeta, Dataset};
use axpoint_domain::enums::{DataType, MaskRule, PiiCategory};
use axpoint_domain::guard::{DatasetPolicySpec, PolicyContext};
use serde::{Deserialize, Serialize};
use sqlparser::ast::{
    ExcludeSelectItem, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments, Ident,
    IdentWithAlias, InTarget, ObjectName, ObjectNamePart, Query, RenameSelectItem, Select,
    SelectItem, SelectItemQualifiedWildcardKind, SetExpr, SetQuantifier, Statement, TableAlias,
    TableAliasColumnDef, TableFactor, TableWithJoins, Value, ValueWithSpan, Visit, VisitMut,
    Visitor, VisitorMut, WildcardAdditionalOptions, With,
};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use uuid::Uuid;

/// Default maximum rows returned by an ad-hoc analytical query (`05-STEP04` §04.3).
pub const DEFAULT_MAX_ROWS: u32 = 10_000;

/// Hard upper bound on `max_rows` for `POST /api/analytics/sql/run` (`05-STEP04` §04.3).
pub const HARD_MAX_ROWS: u32 = 100_000;

/// Default row cap for `guard_and_rewrite` and NL2SQL / pivot result caps.
pub const MAX_RESULT_ROWS: u32 = 10_000;

/// Maximum SQL byte length accepted before returning `PAYLOAD_TOO_LARGE` (`64 KiB`).
pub const MAX_SQL_BYTES: usize = 65_536;

const ALLOWED_PLATFORM_VIEWS: &[&str] = &[
    "nodes",
    "links",
    "metric_points",
    "metric_series",
    "metric_rollups",
];

const ALLOWED_TABLE_FUNCTIONS: &[&str] = &["range", "generate_series", "unnest"];

const FORBIDDEN_SCALAR_FUNCTIONS: &[&str] = &[
    "getenv",
    "current_setting",
    "current_database",
    "current_catalog",
    "current_schemas",
    "current_user",
    "session_user",
    "user",
    "version",
    "txid_current",
    "read_text",
    "read_blob",
    "read_csv",
    "read_csv_auto",
    "read_parquet",
    "read_json",
    "read_json_auto",
    "read_json_objects",
    "read_ndjson",
    "read_ndjson_auto",
    "read_ndjson_objects",
    "read_xlsx",
    "parquet_metadata",
    "parquet_schema",
    "parquet_file_metadata",
    "parquet_kv_metadata",
    "csv_sniffer",
    "glob",
    "query",
    "query_table",
    "load_extension",
    "install_extension",
    "load_aws_credentials",
    "sql_auto_complete",
    "writefile",
    "editdist3",
    "shell",
    "system",
    "http_get",
    "http_post",
];

const FORBIDDEN_FUNC_PREFIXES: &[&str] = &[
    "pg_",
    "duckdb_",
    "pragma_",
    "ducklake_",
    "postgres_",
    "sqlite_",
    "mysql_",
    "iceberg_",
    "delta_",
    "azure_",
    "s3_",
    "__internal",
];

const FORBIDDEN_BARE_RELATIONS: &[&str] = &[
    "sqlite_master",
    "sqlite_schema",
    "sqlite_temp_master",
    "sqlite_temp_schema",
];

/// Catalog metadata for a single tenant dataset exposed to `AxAnalytics`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDatasetEntry {
    /// Dataset UUID.
    pub id: Uuid,
    /// Dataset SQL identifier (`^[a-z][a-z0-9_]{0,62}$`).
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Physical table name in `lake."tenant_<id>"`.
    pub lake_table: String,
    /// Monotonic dataset schema version.
    pub schema_version: i32,
    /// Committed row count.
    pub row_count: i64,
    /// Lowercased column name -> [`ColumnMeta`].
    pub columns: BTreeMap<String, ColumnMeta>,
}

impl CatalogDatasetEntry {
    /// Builds a minimal [`CatalogDatasetEntry`] for tests or lightweight catalogs.
    #[must_use]
    pub fn simple(id: Uuid, name: impl Into<String>, lake_table: impl Into<String>) -> Self {
        let name_str = name.into();
        Self {
            id,
            display_name: name_str.clone(),
            name: name_str,
            lake_table: lake_table.into(),
            schema_version: 1,
            row_count: 0,
            columns: BTreeMap::new(),
        }
    }

    /// Builds a [`CatalogDatasetEntry`] from a domain [`Dataset`] and its [`ColumnMeta`] list.
    #[must_use]
    pub fn from_dataset(dataset: &Dataset, column_metas: &[ColumnMeta]) -> Self {
        let mut columns = BTreeMap::new();
        for prof in &dataset.schema_json {
            let key = prof.name.to_ascii_lowercase();
            columns.insert(
                key,
                ColumnMeta {
                    dataset_id: dataset.id,
                    column_name: prof.name.clone(),
                    data_type: prof.data_type,
                    pii_category: PiiCategory::None,
                    pii_confirmed: false,
                    description: prof.source_comment.clone(),
                    business_term: String::new(),
                    unit: String::new(),
                    ordinal: prof.ordinal,
                },
            );
        }
        for meta in column_metas {
            columns.insert(meta.column_name.to_ascii_lowercase(), meta.clone());
        }
        Self {
            id: dataset.id,
            name: dataset.name.clone(),
            display_name: dataset.display_name.clone(),
            lake_table: dataset.lake_table.clone(),
            schema_version: dataset.schema_version,
            row_count: dataset.row_count,
            columns,
        }
    }
}

/// Per-tenant schema catalog consumed by `sql_guard`, `pivot`, `semantic`, and `result`.
#[derive(Debug, Clone, Default)]
pub struct TenantCatalog {
    /// Tenant identifier (`^[a-z0-9_-]{1,64}$`).
    pub tenant_id: String,
    /// Lowercased dataset name -> [`CatalogDatasetEntry`].
    pub datasets: HashMap<String, CatalogDatasetEntry>,
    /// Lowercased live connection names (without `live_` prefix).
    pub live_connections: HashSet<String>,
    /// Whether the executing principal holds `Permission::DataLiveQuery`.
    pub has_live_permission: bool,
}

impl TenantCatalog {
    /// Creates an empty [`TenantCatalog`] for `tenant_id`.
    #[must_use]
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            datasets: HashMap::new(),
            live_connections: HashSet::new(),
            has_live_permission: false,
        }
    }

    /// Inserts a [`CatalogDatasetEntry`] into this catalog.
    pub fn insert_dataset(&mut self, entry: CatalogDatasetEntry) {
        self.datasets.insert(entry.name.to_ascii_lowercase(), entry);
    }

    /// Builder helper adding a [`CatalogDatasetEntry`].
    #[must_use]
    pub fn with_dataset(mut self, entry: CatalogDatasetEntry) -> Self {
        self.insert_dataset(entry);
        self
    }

    /// Builder helper adding a dataset by `(name, lake_table)` with a deterministic `UUIDv5`.
    #[must_use]
    pub fn with_simple_dataset(
        mut self,
        name: impl Into<String>,
        lake_table: impl Into<String>,
    ) -> Self {
        let name_str = name.into();
        let id = Uuid::new_v5(&Uuid::NAMESPACE_OID, name_str.as_bytes());
        self.insert_dataset(CatalogDatasetEntry::simple(id, name_str, lake_table));
        self
    }

    /// Registers a live connection name (stripping an optional `live_` prefix).
    pub fn insert_live_connection(&mut self, name: impl AsRef<str>) {
        let lower = name.as_ref().trim().to_ascii_lowercase();
        let normalized = lower.strip_prefix("live_").unwrap_or(&lower);
        if !normalized.is_empty() {
            self.live_connections.insert(String::from(normalized));
        }
    }

    /// Builder helper registering a live connection name.
    #[must_use]
    pub fn with_live_connection(mut self, name: impl AsRef<str>) -> Self {
        self.insert_live_connection(name);
        self
    }

    /// Builder helper setting `has_live_permission`.
    #[must_use]
    pub const fn with_live_permission(mut self, allowed: bool) -> Self {
        self.has_live_permission = allowed;
        self
    }

    /// Looks up a dataset entry case-insensitively by SQL name.
    #[must_use]
    pub fn dataset(&self, name: &str) -> Option<&CatalogDatasetEntry> {
        self.datasets.get(&name.trim().to_ascii_lowercase())
    }

    /// Looks up a dataset entry by UUID.
    #[must_use]
    pub fn dataset_by_id(&self, id: Uuid) -> Option<&CatalogDatasetEntry> {
        self.datasets.values().find(|d| d.id == id)
    }

    /// Checks if `conn_name` (without `live_`) is registered for live federated queries.
    #[must_use]
    pub fn has_live_connection(&self, conn_name: &str) -> bool {
        let lower = conn_name.trim().to_ascii_lowercase();
        let normalized = lower.strip_prefix("live_").unwrap_or(&lower);
        self.live_connections.contains(normalized)
    }

    /// Looks up [`ColumnMeta`] for `(dataset_name, column_name)` case-insensitively.
    #[must_use]
    pub fn column_meta(&self, dataset_name: &str, column_name: &str) -> Option<&ColumnMeta> {
        self.dataset(dataset_name)?
            .columns
            .get(&column_name.trim().to_ascii_lowercase())
    }

    /// Resolves Logical [`DataType`] for a dataset or platform view column (`None` if unknown).
    #[must_use]
    pub fn resolve_column_type(&self, relation: &str, column_name: &str) -> Option<DataType> {
        let rel_lower = relation.trim().to_ascii_lowercase();
        let col_lower = column_name.trim().to_ascii_lowercase();
        if let Some(view_name) = rel_lower.strip_prefix("ax.") {
            return platform_view_column_type(view_name, &col_lower);
        }
        self.column_meta(&rel_lower, &col_lower)
            .map(|m| m.data_type)
    }
}

/// Options controlling [`guard`] execution (`05-STEP04` §04.3, `11-STEP13` §13.6.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardOptions {
    /// Maximum rows before truncation (`default = 10_000`, `cap = 100_000`).
    pub max_rows: u32,
    /// Whether `live_<conn>.<schema>.<table>` references are allowed.
    pub allow_live: bool,
    /// Named `:param` values substituted as validated SQL literals.
    pub params: BTreeMap<String, serde_json::Value>,
    /// Optional compiled `AxGuard` row/column security context (`11-STEP13` §13.6.3).
    #[serde(default)]
    pub policy: Option<PolicyContext>,
}

impl Default for GuardOptions {
    fn default() -> Self {
        Self {
            max_rows: DEFAULT_MAX_ROWS,
            allow_live: false,
            params: BTreeMap::new(),
            policy: None,
        }
    }
}

impl GuardOptions {
    /// Builder helper attaching an optional [`PolicyContext`].
    #[must_use]
    pub fn with_policy(mut self, policy: Option<PolicyContext>) -> Self {
        self.policy = policy;
        self
    }
}

/// Origin classification of a projected output column for dynamic PII masking (`05-STEP04` §04.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputColumnOrigin {
    /// Wildcard projection (`SELECT *` or `SELECT t.*`) resolved dynamically by column name.
    Wildcard,
    /// Direct projection of a physical column (`SELECT phone` or `SELECT o.phone AS contact`).
    Direct {
        /// Backing dataset UUID.
        dataset_id: Uuid,
        /// Physical column name.
        column_name: String,
        /// Output column alias or column name.
        output_name: String,
        /// Column PII classification.
        pii_category: PiiCategory,
    },
    /// Expression derived from one or more columns, at least one of which is PII (`REDACT` by default).
    DerivedFromPii {
        /// Output column alias (if explicit).
        output_name: Option<String>,
        /// First detected non-`NONE` PII category contributing to the expression.
        pii_category: PiiCategory,
    },
    /// Non-sensitive expression or literal.
    NonSensitive {
        /// Output column alias (if explicit).
        output_name: Option<String>,
    },
}

/// Validated and rewritten SQL output produced by [`guard`] (`05-STEP04` §04.3).
#[derive(Debug, Clone)]
pub struct GuardedSql {
    /// Rewritten SQL (wrapped in `SELECT * FROM (...) AS q LIMIT <max_rows + 1>` for queries).
    pub sql: String,
    /// Effective `max_rows` cap applied (`rows.len() > effective_max_rows` indicates truncation).
    pub effective_max_rows: u32,
    /// Referenced dataset UUIDs in deterministic encounter order.
    pub referenced_datasets: Vec<Uuid>,
    /// Referenced lowercased dataset names.
    pub referenced_dataset_names: Vec<String>,
    /// Directly referenced `(dataset_id, column_name)` pairs tracked from outer projections.
    pub referenced_columns: Vec<(Uuid, String)>,
    /// Ordered lineage descriptors for the outer query projection (empty when outer query is `VALUES`).
    pub output_lineage: Vec<OutputColumnOrigin>,
    /// `true` if any `ax.*` platform federation view is referenced.
    pub uses_platform_views: bool,
    /// Referenced `live_<conn>` connection names (without `live_` prefix).
    pub referenced_live_connections: Vec<String>,
    /// `true` when SQL-level `PolicyContext` column/row governance was evaluated during rewrite.
    pub sql_policy_applied: bool,
    /// Lowercased column names masked directly inside `DuckDB` SQL via `ax_mask_*` UDFs.
    pub sql_masked_columns: HashSet<String>,
}

/// Positional-parameter rewritten SQL output produced by [`guard_and_rewrite`].
#[derive(Debug, Clone)]
pub struct GuardedQuery {
    /// Rewritten SQL with `:name` replaced by `?` and wrapped with outer `LIMIT`.
    pub rewritten_sql: String,
    /// Ordered parameter names corresponding to each `?` placeholder.
    pub param_order: Vec<String>,
    /// Effective row limit before `+ 1` truncation probe.
    pub effective_limit: u32,
    /// Referenced dataset UUIDs.
    pub referenced_datasets: Vec<Uuid>,
    /// Referenced lowercased dataset names.
    pub referenced_dataset_names: Vec<String>,
    /// Ordered lineage descriptors for the outer `SELECT` list.
    pub output_lineage: Vec<OutputColumnOrigin>,
    /// `true` if any `ax.*` platform view is referenced.
    pub uses_platform_views: bool,
    /// Referenced `live_<conn>` connection names (without `live_` prefix).
    pub referenced_live_connections: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParamBindMode<'a> {
    Literal(&'a BTreeMap<String, serde_json::Value>),
    PositionalPlaceholder,
    ForbidParams,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuardTarget {
    InteractiveQuery { allow_live: bool },
    MaterializeView,
}

/// Validates and rewrites `sql` according to all 8 rules of `05-STEP04` §04.3.
///
/// # Errors
/// - Returns `AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE")` if `sql.len() > 65_536`.
/// - Returns `AppError::Forbidden("PERMISSION_DENIED")` if a `live_*` table is referenced with
///   `opts.allow_live = true` while `catalog.has_live_permission = false`.
/// - Returns `AppError::Forbidden("POLICY_DENIED")` if any referenced dataset appears in
///   `opts.policy.denied_datasets`.
/// - Returns `AppError::BadRequest { code: "SQL_REJECTED", .. }` on any syntax, statement,
///   relation, function, parameter, or post-rewrite verification violation.
pub fn guard(
    sql: &str,
    catalog: &TenantCatalog,
    opts: &GuardOptions,
) -> Result<GuardedSql, AppError> {
    guard_with_ceiling(sql, catalog, opts, HARD_MAX_ROWS)
}

/// Validates and rewrites `sql` with an explicit upper `max_rows` ceiling (up to
/// `EXPORT_SAVED_QUERY_MAX_ROWS = 1_000_000` for internal `CSV`/`XLSX` export jobs, `10-STEP11-12` §12.2).
///
/// # Errors
/// Returns `PAYLOAD_TOO_LARGE`, `PERMISSION_DENIED`, `POLICY_DENIED`, or `SQL_REJECTED` on any guard violation.
pub fn guard_with_ceiling(
    sql: &str,
    catalog: &TenantCatalog,
    opts: &GuardOptions,
    max_rows_ceiling: u32,
) -> Result<GuardedSql, AppError> {
    let bounded_ceiling =
        max_rows_ceiling.clamp(1, axpoint_domain::export::EXPORT_SAVED_QUERY_MAX_ROWS);
    let effective_max_rows = if opts.max_rows == 0 {
        DEFAULT_MAX_ROWS.min(bounded_ceiling)
    } else {
        opts.max_rows.min(bounded_ceiling)
    };
    let mut query = parse_single_select_query(sql, &catalog.tenant_id)?;
    let output_lineage = analyze_outer_lineage(&query, catalog);

    let mut visitor = GuardVisitor::new(
        catalog,
        GuardTarget::InteractiveQuery {
            allow_live: opts.allow_live,
        },
        ParamBindMode::Literal(&opts.params),
        opts.policy.as_ref(),
    );
    if let ControlFlow::Break(err) = VisitMut::visit(&mut query, &mut visitor) {
        return Err(*err);
    }

    let fetch_limit = u64::from(effective_max_rows).saturating_add(1);
    let wrapped_sql = format!("SELECT * FROM ({query}) AS q LIMIT {fetch_limit}");

    verify_rewritten_sql(
        &wrapped_sql,
        catalog,
        GuardTarget::InteractiveQuery {
            allow_live: opts.allow_live,
        },
        false,
    )?;

    let referenced_columns = collect_direct_columns(&output_lineage);
    Ok(GuardedSql {
        sql: wrapped_sql,
        effective_max_rows,
        referenced_datasets: visitor.referenced_dataset_ids,
        referenced_dataset_names: visitor.referenced_dataset_names,
        referenced_columns,
        output_lineage,
        uses_platform_views: visitor.uses_platform_views,
        referenced_live_connections: visitor.referenced_live_connections,
        sql_policy_applied: visitor.sql_policy_applied,
        sql_masked_columns: visitor.sql_masked_columns,
    })
}

/// Validates and rewrites `sql`, converting `:name` placeholders into positional `?` markers
/// and returning their encounter order in [`GuardedQuery::param_order`].
///
/// # Errors
/// Returns `PAYLOAD_TOO_LARGE`, `PERMISSION_DENIED`, or `SQL_REJECTED` on any guard violation.
pub fn guard_and_rewrite(
    sql: &str,
    catalog: &TenantCatalog,
    limit: u32,
) -> Result<GuardedQuery, AppError> {
    let effective_limit = if limit == 0 {
        1_000
    } else {
        limit.min(MAX_RESULT_ROWS)
    };
    let mut query = parse_single_select_query(sql, &catalog.tenant_id)?;
    let output_lineage = analyze_outer_lineage(&query, catalog);

    let mut visitor = GuardVisitor::new(
        catalog,
        GuardTarget::InteractiveQuery {
            allow_live: catalog.has_live_permission,
        },
        ParamBindMode::PositionalPlaceholder,
        None,
    );
    if let ControlFlow::Break(err) = VisitMut::visit(&mut query, &mut visitor) {
        return Err(*err);
    }

    let fetch_limit = u64::from(effective_limit).saturating_add(1);
    let wrapped_sql = format!("SELECT * FROM ({query}) AS q LIMIT {fetch_limit}");

    verify_rewritten_sql(
        &wrapped_sql,
        catalog,
        GuardTarget::InteractiveQuery {
            allow_live: catalog.has_live_permission,
        },
        true,
    )?;

    Ok(GuardedQuery {
        rewritten_sql: wrapped_sql,
        param_order: visitor.param_order,
        effective_limit,
        referenced_datasets: visitor.referenced_dataset_ids,
        referenced_dataset_names: visitor.referenced_dataset_names,
        output_lineage,
        uses_platform_views: visitor.uses_platform_views,
        referenced_live_connections: visitor.referenced_live_connections,
    })
}

/// Validates and rewrites `sql` for `DERIVED_VIEW` materialization (`POST /lake/materialize`).
///
/// Rejects `ax.*` platform views, `live_*` federated connections, and unbound `:param` placeholders,
/// and omits the outer `LIMIT` wrapper so the full derived dataset is materialized into `DuckLake`.
///
/// # Errors
/// Returns `PAYLOAD_TOO_LARGE` or `SQL_REJECTED` on any guard violation.
pub fn guard_for_materialize(sql: &str, catalog: &TenantCatalog) -> Result<GuardedSql, AppError> {
    let mut query = parse_single_select_query(sql, &catalog.tenant_id)?;
    let output_lineage = analyze_outer_lineage(&query, catalog);

    let mut visitor = GuardVisitor::new(
        catalog,
        GuardTarget::MaterializeView,
        ParamBindMode::ForbidParams,
        None,
    );
    if let ControlFlow::Break(err) = VisitMut::visit(&mut query, &mut visitor) {
        return Err(*err);
    }

    let clean_sql = query.to_string();
    verify_rewritten_sql(&clean_sql, catalog, GuardTarget::MaterializeView, false)?;

    let referenced_columns = collect_direct_columns(&output_lineage);
    Ok(GuardedSql {
        sql: clean_sql,
        effective_max_rows: 0,
        referenced_datasets: visitor.referenced_dataset_ids,
        referenced_dataset_names: visitor.referenced_dataset_names,
        referenced_columns,
        output_lineage,
        uses_platform_views: false,
        referenced_live_connections: Vec::new(),
        sql_policy_applied: false,
        sql_masked_columns: HashSet::new(),
    })
}

/// Converts a JSON value into a safe `sqlparser` [`Expr`] literal.
///
/// # Errors
/// Returns `SQL_REJECTED` if `val` contains non-finite floats or nested objects/arrays that cannot
/// be represented as scalar or flat list SQL literals.
pub fn json_value_to_sql_expr(val: &serde_json::Value) -> Result<Expr, AppError> {
    match val {
        serde_json::Value::Null => Ok(Expr::Value(ValueWithSpan::from(Value::Null))),
        serde_json::Value::Bool(b) => Ok(Expr::Value(ValueWithSpan::from(Value::Boolean(*b)))),
        serde_json::Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                return Ok(number_to_ast_expr(&i.to_string()));
            }
            if let Some(u) = num.as_u64() {
                return Ok(Expr::Value(ValueWithSpan::from(Value::Number(
                    u.to_string(),
                    false,
                ))));
            }
            let Some(f) = num.as_f64() else {
                return Err(rejected("numeric parameter out of range"));
            };
            if !f.is_finite() {
                return Err(rejected("non-finite floating point parameter is forbidden"));
            }
            let formatted = f.to_string();
            Ok(number_to_ast_expr(&formatted))
        }
        serde_json::Value::String(s) => {
            if s.contains('\0') {
                return Err(rejected("NUL byte in string parameter is forbidden"));
            }
            Ok(Expr::Value(ValueWithSpan::from(Value::SingleQuotedString(
                s.clone(),
            ))))
        }
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return Err(rejected("array SQL parameter cannot be empty"));
            }
            let mut exprs = Vec::with_capacity(items.len());
            for item in items {
                if item.is_array() || item.is_object() {
                    return Err(rejected(
                        "nested arrays or objects are forbidden in SQL parameters",
                    ));
                }
                exprs.push(json_value_to_sql_expr(item)?);
            }
            Ok(Expr::Tuple(exprs))
        }
        serde_json::Value::Object(_) => {
            Err(rejected("JSON object parameter cannot be bound directly"))
        }
    }
}

fn number_to_ast_expr(text: &str) -> Expr {
    if let Some(unsigned) = text.strip_prefix('-') {
        Expr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Minus,
            expr: Box::new(Expr::Value(ValueWithSpan::from(Value::Number(
                String::from(unsigned),
                false,
            )))),
        }
    } else {
        Expr::Value(ValueWithSpan::from(Value::Number(
            String::from(text),
            false,
        )))
    }
}

fn rejected(reason: impl Into<String>) -> AppError {
    AppError::bad("SQL_REJECTED", reason.into())
}

fn is_valid_tenant_id(tenant_id: &str) -> bool {
    let bytes = tenant_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn is_valid_param_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Checks for multiple top-level semicolons outside quotes/comments (e.g. `SELECT 1;;`).
fn has_extra_unquoted_semicolons(sql: &str) -> bool {
    let mut chars = sql.chars().peekable();
    let mut semicolon_count: usize = 0;
    let mut token_after_semicolon = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                if semicolon_count > 0 {
                    token_after_semicolon = true;
                }
                while let Some(c) = chars.next() {
                    if c == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '"' => {
                if semicolon_count > 0 {
                    token_after_semicolon = true;
                }
                while let Some(c) = chars.next() {
                    if c == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut depth: usize = 1;
                while let Some(c) = chars.next() {
                    if c == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        depth = depth.saturating_add(1);
                    } else if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            ';' => {
                semicolon_count = semicolon_count.saturating_add(1);
                if semicolon_count > 1 {
                    return true;
                }
            }
            c if !c.is_whitespace() && semicolon_count > 0 => {
                token_after_semicolon = true;
            }
            _ => {}
        }
    }

    token_after_semicolon
}

fn parse_single_select_query(sql: &str, tenant_id: &str) -> Result<Box<Query>, AppError> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE"));
    }
    if !is_valid_tenant_id(tenant_id) {
        return Err(rejected(format!("invalid tenant identifier '{tenant_id}'")));
    }
    if sql.contains('\0') {
        return Err(rejected("SQL contains forbidden NUL byte"));
    }
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err(rejected("SQL query cannot be empty"));
    }
    if has_extra_unquoted_semicolons(trimmed) {
        return Err(rejected(
            "multiple SQL statements or extra semicolons are forbidden",
        ));
    }

    let stmts = Parser::parse_sql(&DuckDbDialect {}, trimmed)
        .map_err(|err| rejected(format!("SQL parse error: {err}")))?;
    let mut iter = stmts.into_iter();
    let Some(stmt) = iter.next() else {
        return Err(rejected("SQL query contains no statement"));
    };
    if iter.next().is_some() {
        return Err(rejected("only a single SELECT query is allowed"));
    }

    match stmt {
        Statement::Query(query) => Ok(query),
        other => Err(rejected(format!(
            "statement kind '{}' is forbidden; only SELECT queries are allowed",
            statement_label(&other)
        ))),
    }
}

const fn statement_label(stmt: &Statement) -> &'static str {
    match stmt {
        Statement::Query(_) => "QUERY",
        Statement::Insert(_) => "INSERT",
        Statement::Update { .. } => "UPDATE",
        Statement::Delete(_) => "DELETE",
        Statement::Merge { .. } => "MERGE",
        Statement::CreateTable(_) => "CREATE TABLE",
        Statement::CreateView { .. } => "CREATE VIEW",
        Statement::Drop { .. } => "DROP",
        Statement::AlterTable { .. } => "ALTER TABLE",
        Statement::Copy { .. } => "COPY",
        Statement::AttachDatabase { .. } | Statement::AttachDuckDBDatabase { .. } => "ATTACH",
        Statement::DetachDuckDBDatabase { .. } => "DETACH",
        Statement::Install { .. } => "INSTALL",
        Statement::Load { .. } => "LOAD",
        Statement::Set(_) => "SET",
        Statement::Pragma { .. } => "PRAGMA",
        Statement::Call(_) => "CALL",
        Statement::Explain { .. } | Statement::ExplainTable { .. } => "EXPLAIN",
        _ => "NON_QUERY_STATEMENT",
    }
}

fn is_forbidden_function_name(lower_name: &str) -> bool {
    if FORBIDDEN_SCALAR_FUNCTIONS.contains(&lower_name) {
        return true;
    }
    if lower_name.ends_with("_scan") || lower_name.ends_with("_scan_pushdown") {
        return true;
    }
    FORBIDDEN_FUNC_PREFIXES
        .iter()
        .any(|prefix| lower_name.starts_with(prefix))
}

fn extract_ident_parts(name: &ObjectName) -> Result<Vec<&Ident>, AppError> {
    let mut out = Vec::with_capacity(name.0.len());
    for part in &name.0 {
        let Some(ident) = part.as_ident() else {
            return Err(rejected("dynamic identifier functions are forbidden"));
        };
        if ident.value.trim().is_empty() || ident.value.contains('\0') {
            return Err(rejected("empty or invalid identifier part"));
        }
        out.push(ident);
    }
    if out.is_empty() {
        return Err(rejected("empty object name"));
    }
    Ok(out)
}

fn check_allowed_table_function_name(name: &ObjectName) -> Result<(), AppError> {
    let parts = extract_ident_parts(name)?;
    let [only] = parts.as_slice() else {
        return Err(rejected(format!(
            "schema-qualified table function '{name}' is forbidden"
        )));
    };
    let lower = only.value.to_ascii_lowercase();
    if !ALLOWED_TABLE_FUNCTIONS.contains(&lower.as_str()) {
        return Err(rejected(format!(
            "table function '{lower}' is forbidden; only range, generate_series, unnest are allowed"
        )));
    }
    Ok(())
}

enum PendingTableRewrite {
    None,
    Subquery {
        subquery_sql: String,
        alias: TableAlias,
    },
}

const fn mask_rule_udf_name(rule: MaskRule) -> Option<&'static str> {
    match rule {
        MaskRule::None => None,
        MaskRule::MaskPhone => Some("ax_mask_phone"),
        MaskRule::MaskIdCard => Some("ax_mask_id_card"),
        MaskRule::MaskEmail => Some("ax_mask_email"),
        MaskRule::MaskBankCard => Some("ax_mask_bank_card"),
        MaskRule::MaskName => Some("ax_mask_name"),
        MaskRule::MaskAddress => Some("ax_mask_address"),
        MaskRule::HmacHash => Some("ax_mask_hmac_hash"),
        MaskRule::Redact => Some("ax_mask_redact"),
    }
}

fn quote_policy_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn is_dataset_denied(
    policy: Option<&PolicyContext>,
    raw_name: &str,
    lower_name: &str,
    dataset_id: Option<Uuid>,
) -> bool {
    let Some(pol) = policy else {
        return false;
    };
    if pol.denied_datasets.contains(raw_name) || pol.denied_datasets.contains(lower_name) {
        return true;
    }
    if let Some(id) = dataset_id
        && pol.denied_datasets.contains(&id.to_string())
    {
        return true;
    }
    pol.denied_datasets
        .iter()
        .any(|d| d.eq_ignore_ascii_case(lower_name))
}

fn lookup_dataset_policy<'a>(
    pol: &'a PolicyContext,
    ds: &CatalogDatasetEntry,
    lower_name: &str,
) -> Option<&'a DatasetPolicySpec> {
    pol.datasets
        .get(&ds.id.to_string())
        .or_else(|| pol.datasets.get(lower_name))
        .or_else(|| pol.datasets.get(&ds.name))
}

fn build_node_types_case_filter(pol: &PolicyContext) -> Option<String> {
    let mut branches = Vec::new();
    for (type_name, spec) in &pol.node_types {
        if let Some(ref filter) = spec.duckdb_row_filter {
            let escaped = type_name.replace('\'', "''");
            branches.push(format!("WHEN '{escaped}' THEN ({filter})"));
        }
    }
    if branches.is_empty() {
        None
    } else {
        Some(format!(
            "CASE \"type_name\" {} ELSE TRUE END",
            branches.join(" ")
        ))
    }
}

fn parse_policy_subquery(subquery_sql: &str) -> Result<Box<Query>, AppError> {
    let mut stmts = Parser::parse_sql(&DuckDbDialect {}, subquery_sql).map_err(|err| {
        rejected(format!(
            "failed to parse compiled row/column policy subquery: {err}"
        ))
    })?;
    if stmts.len() != 1 {
        return Err(rejected(
            "compiled row/column policy subquery produced multiple statements",
        ));
    }
    match stmts.pop() {
        Some(Statement::Query(q)) => Ok(q),
        _ => Err(rejected(
            "compiled row/column policy subquery must be a SELECT query",
        )),
    }
}

struct GuardVisitor<'a> {
    catalog: &'a TenantCatalog,
    target: GuardTarget,
    param_mode: ParamBindMode<'a>,
    policy: Option<&'a PolicyContext>,
    cte_stack: Vec<BTreeSet<String>>,
    saved_with_stack: Vec<Option<With>>,
    pending_rewrites: Vec<PendingTableRewrite>,
    in_allowed_table_func_expr: usize,
    param_order: Vec<String>,
    referenced_dataset_ids: Vec<Uuid>,
    referenced_dataset_set: HashSet<Uuid>,
    referenced_dataset_names: Vec<String>,
    uses_platform_views: bool,
    referenced_live_connections: Vec<String>,
    referenced_live_set: HashSet<String>,
    sql_policy_applied: bool,
    sql_masked_columns: HashSet<String>,
}

impl<'a> GuardVisitor<'a> {
    fn new(
        catalog: &'a TenantCatalog,
        target: GuardTarget,
        param_mode: ParamBindMode<'a>,
        policy: Option<&'a PolicyContext>,
    ) -> Self {
        Self {
            catalog,
            target,
            param_mode,
            policy,
            cte_stack: vec![BTreeSet::new()],
            saved_with_stack: Vec::new(),
            pending_rewrites: Vec::new(),
            in_allowed_table_func_expr: 0,
            param_order: Vec::new(),
            referenced_dataset_ids: Vec::new(),
            referenced_dataset_set: HashSet::new(),
            referenced_dataset_names: Vec::new(),
            uses_platform_views: false,
            referenced_live_connections: Vec::new(),
            referenced_live_set: HashSet::new(),
            sql_policy_applied: false,
            sql_masked_columns: HashSet::new(),
        }
    }

    fn current_ctes(&self) -> BTreeSet<String> {
        self.cte_stack.last().cloned().unwrap_or_default()
    }

    fn record_dataset(&mut self, entry: &CatalogDatasetEntry) {
        if self.referenced_dataset_set.insert(entry.id) {
            self.referenced_dataset_ids.push(entry.id);
            self.referenced_dataset_names
                .push(entry.name.to_ascii_lowercase());
        }
    }

    fn record_live_connection(&mut self, conn_name: &str) {
        if self.referenced_live_set.insert(String::from(conn_name)) {
            self.referenced_live_connections
                .push(String::from(conn_name));
        }
    }

    fn rewrite_dataset_relation(
        &mut self,
        raw_ident: String,
        name: &mut ObjectName,
        alias: &mut Option<TableAlias>,
    ) -> Result<PendingTableRewrite, AppError> {
        let lower = raw_ident.to_ascii_lowercase();
        if self.current_ctes().contains(&lower) {
            return Ok(PendingTableRewrite::None);
        }
        if FORBIDDEN_BARE_RELATIONS.contains(&lower.as_str())
            || lower.starts_with("duckdb_")
            || lower.starts_with("pragma_")
        {
            return Err(rejected(format!("system relation '{lower}' is forbidden")));
        }
        if is_dataset_denied(self.policy, &raw_ident, &lower, None) {
            return Err(AppError::Forbidden("POLICY_DENIED"));
        }
        let Some(ds) = self.catalog.dataset(&lower) else {
            return Err(rejected(format!(
                "unknown or unauthorized dataset '{raw_ident}'"
            )));
        };
        let ds_clone = ds.clone();
        if is_dataset_denied(self.policy, &ds_clone.name, &lower, Some(ds_clone.id)) {
            return Err(AppError::Forbidden("POLICY_DENIED"));
        }
        self.record_dataset(&ds_clone);
        let default_alias = TableAlias {
            explicit: true,
            name: Ident::with_quote('"', raw_ident),
            columns: Vec::new(),
            at: None,
        };
        let effective_alias = alias.clone().unwrap_or_else(|| default_alias.clone());
        if alias.is_none() {
            *alias = Some(default_alias);
        }
        let tenant_schema = format!("tenant_{}", self.catalog.tenant_id);
        *name = ObjectName::from(vec![
            Ident::new("lake"),
            Ident::with_quote('"', tenant_schema.clone()),
            Ident::with_quote('"', ds_clone.lake_table.clone()),
        ]);
        Ok(self.build_dataset_subquery(&ds_clone, &lower, &tenant_schema, effective_alias))
    }

    fn build_dataset_subquery(
        &mut self,
        ds: &CatalogDatasetEntry,
        lower_name: &str,
        tenant_schema: &str,
        effective_alias: TableAlias,
    ) -> PendingTableRewrite {
        let Some(pol) = self.policy else {
            return PendingTableRewrite::None;
        };
        let Some(spec) = lookup_dataset_policy(pol, ds, lower_name) else {
            return PendingTableRewrite::None;
        };
        self.sql_policy_applied = true;

        let row_filter = if pol.bypass_row_policies {
            None
        } else {
            spec.duckdb_row_filter.as_deref()
        };

        let mut replace_clauses = Vec::new();
        for (col_key, &rule) in &spec.column_masks {
            let Some(udf_name) = mask_rule_udf_name(rule) else {
                continue;
            };
            let canonical_col = if ds.columns.is_empty() {
                Some(col_key.clone())
            } else {
                ds.columns
                    .get(&col_key.to_ascii_lowercase())
                    .map(|m| m.column_name.clone())
            };
            if let Some(col_name) = canonical_col {
                self.sql_masked_columns
                    .insert(col_name.to_ascii_lowercase());
                let qcol = quote_policy_ident(&col_name);
                replace_clauses.push(format!("{udf_name}(CAST({qcol} AS VARCHAR)) AS {qcol}"));
            }
        }

        if row_filter.is_none() && replace_clauses.is_empty() {
            return PendingTableRewrite::None;
        }

        let proj = if replace_clauses.is_empty() {
            String::from("*")
        } else {
            format!("* REPLACE ({})", replace_clauses.join(", "))
        };
        let lake_rel = format!(
            "lake.{}.{}",
            quote_policy_ident(tenant_schema),
            quote_policy_ident(&ds.lake_table)
        );
        let where_part = row_filter.map_or_else(String::new, |f| format!(" WHERE ({f})"));
        PendingTableRewrite::Subquery {
            subquery_sql: format!("SELECT {proj} FROM {lake_rel}{where_part}"),
            alias: effective_alias,
        }
    }

    fn rewrite_platform_view_relation(
        &mut self,
        view_name: String,
        name: &mut ObjectName,
        alias: Option<&TableAlias>,
    ) -> Result<PendingTableRewrite, AppError> {
        if self.target == GuardTarget::MaterializeView {
            return Err(rejected(
                "platform views ax.* are not allowed in materialized derived views",
            ));
        }
        self.uses_platform_views = true;
        *name = ObjectName::from(vec![Ident::new("ax"), Ident::new(view_name.clone())]);

        let Some(pol) = self.policy else {
            return Ok(PendingTableRewrite::None);
        };
        if pol.bypass_row_policies {
            return Ok(PendingTableRewrite::None);
        }
        let Some(case_sql) = build_node_types_case_filter(pol) else {
            return Ok(PendingTableRewrite::None);
        };
        self.sql_policy_applied = true;

        let sub_sql = match view_name.as_str() {
            "nodes" => format!("SELECT * FROM ax.nodes WHERE {case_sql}"),
            "links" => format!(
                "SELECT * FROM ax.links WHERE \"src_node_id\" IN (SELECT \"id\" FROM ax.nodes WHERE {case_sql}) AND \"dst_node_id\" IN (SELECT \"id\" FROM ax.nodes WHERE {case_sql})"
            ),
            _ => format!(
                "SELECT * FROM ax.{view_name} WHERE \"node_id\" IN (SELECT \"id\" FROM ax.nodes WHERE {case_sql})"
            ),
        };
        let effective_alias = alias.cloned().unwrap_or_else(|| TableAlias {
            explicit: true,
            name: Ident::with_quote('"', view_name),
            columns: Vec::new(),
            at: None,
        });
        Ok(PendingTableRewrite::Subquery {
            subquery_sql: sub_sql,
            alias: effective_alias,
        })
    }

    fn rewrite_table_relation(
        &mut self,
        name: &mut ObjectName,
        alias: &mut Option<TableAlias>,
    ) -> Result<PendingTableRewrite, AppError> {
        let parts = extract_ident_parts(name)?;
        match parts.as_slice() {
            [single] => self.rewrite_dataset_relation(single.value.clone(), name, alias),
            [first, second] => {
                let s0 = first.value.to_ascii_lowercase();
                let s1 = second.value.to_ascii_lowercase();
                if s0 == "ax" && ALLOWED_PLATFORM_VIEWS.contains(&s1.as_str()) {
                    return self.rewrite_platform_view_relation(s1, name, alias.as_ref());
                }
                Err(rejected(format!(
                    "unauthorized relation '{s0}.{s1}'; only registered datasets or ax.* views are allowed"
                )))
            }
            [first, second, third] => {
                let s0 = first.value.to_ascii_lowercase();
                let s1 = second.value.to_ascii_lowercase();
                let s2 = third.value.to_ascii_lowercase();
                let Some(conn_name) = s0.strip_prefix("live_") else {
                    return Err(rejected(format!(
                        "unauthorized 3-part relation '{s0}.{s1}.{s2}'"
                    )));
                };
                let GuardTarget::InteractiveQuery { allow_live } = self.target else {
                    return Err(rejected(
                        "live connections are not allowed in materialized derived views",
                    ));
                };
                if !allow_live {
                    return Err(rejected(format!(
                        "live connection '{s0}' requires allow_live=true"
                    )));
                }
                if !self.catalog.has_live_permission {
                    return Err(AppError::Forbidden("PERMISSION_DENIED"));
                }
                if conn_name.is_empty() || !self.catalog.has_live_connection(conn_name) {
                    return Err(rejected(format!("unknown live connection '{s0}'")));
                }
                if matches!(
                    s1.as_str(),
                    "information_schema" | "pg_catalog" | "mysql" | "performance_schema" | "sys"
                ) || s2.starts_with("pg_")
                    || s2.starts_with("sqlite_")
                {
                    return Err(rejected(format!(
                        "system catalog relation '{s0}.{s1}.{s2}' is forbidden"
                    )));
                }
                self.record_live_connection(conn_name);
                Ok(PendingTableRewrite::None)
            }
            _ => Err(rejected(format!("invalid multi-part relation '{name}'"))),
        }
    }

    fn validate_scalar_or_agg_function(&self, func: &Function) -> Result<(), AppError> {
        if func.uses_odbc_syntax {
            return Err(rejected("ODBC function call syntax is forbidden"));
        }
        if !matches!(func.parameters, FunctionArguments::None) {
            return Err(rejected("parametric function syntax is forbidden"));
        }
        let parts = extract_ident_parts(&func.name)?;
        let [only] = parts.as_slice() else {
            return Err(rejected(format!(
                "schema-qualified function '{}' is forbidden",
                func.name
            )));
        };
        let lower = only.value.to_ascii_lowercase();
        if self.in_allowed_table_func_expr > 0 && ALLOWED_TABLE_FUNCTIONS.contains(&lower.as_str())
        {
            return Ok(());
        }
        if is_forbidden_function_name(&lower) {
            return Err(rejected(format!("function '{lower}' is forbidden")));
        }
        Ok(())
    }

    fn bind_placeholder(&mut self, raw: &str) -> Result<Expr, AppError> {
        let Some(param_name) = raw.strip_prefix(':') else {
            return Err(rejected(format!(
                "positional or non-colon placeholder '{raw}' is forbidden; use :name"
            )));
        };
        if !is_valid_param_ident(param_name) {
            return Err(rejected(format!(
                "invalid SQL parameter name ':{param_name}'"
            )));
        }
        match self.param_mode {
            ParamBindMode::Literal(params) => {
                let Some(val) = params.get(param_name) else {
                    return Err(rejected(format!("missing SQL parameter ':{param_name}'")));
                };
                json_value_to_sql_expr(val)
            }
            ParamBindMode::PositionalPlaceholder => {
                self.param_order.push(String::from(param_name));
                Ok(Expr::Value(ValueWithSpan::from(Value::Placeholder(
                    String::from("?"),
                ))))
            }
            ParamBindMode::ForbidParams => Err(rejected(format!(
                "runtime parameter ':{param_name}' is not allowed in materialized transform SQL"
            ))),
        }
    }
}

fn break_err(err: AppError) -> ControlFlow<Box<AppError>> {
    ControlFlow::Break(Box::new(err))
}

impl VisitorMut for GuardVisitor<'_> {
    type Break = Box<AppError>;

    fn pre_visit_statement(&mut self, stmt: &mut Statement) -> ControlFlow<Self::Break> {
        if !matches!(stmt, Statement::Query(_)) {
            return break_err(rejected(format!(
                "nested statement '{}' is forbidden",
                statement_label(stmt)
            )));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &mut Query) -> ControlFlow<Self::Break> {
        if !query.locks.is_empty() {
            return break_err(rejected("SELECT ... FOR UPDATE/SHARE locking is forbidden"));
        }
        if query.settings.is_some() {
            return break_err(rejected("SELECT ... SETTINGS clause is forbidden"));
        }
        if query.format_clause.is_some() {
            return break_err(rejected("SELECT ... FORMAT clause is forbidden"));
        }
        if query.for_clause.is_some() {
            return break_err(rejected("SELECT ... FOR XML/JSON clause is forbidden"));
        }
        if !query.pipe_operators.is_empty() {
            return break_err(rejected("pipe operators are forbidden"));
        }
        if !matches!(
            query.body.as_ref(),
            SetExpr::Select(_)
                | SetExpr::Query(_)
                | SetExpr::SetOperation { .. }
                | SetExpr::Values(_)
        ) {
            return break_err(rejected("DML or TABLE query body is forbidden"));
        }

        let mut scope = self.current_ctes();
        let taken_with = query.with.take();
        let rewritten_with = if let Some(mut with_clause) = taken_with {
            if with_clause.recursive {
                for cte in &with_clause.cte_tables {
                    scope.insert(cte.alias.name.value.to_ascii_lowercase());
                }
            }
            for cte in &mut with_clause.cte_tables {
                if cte.from.is_some() {
                    return break_err(rejected("CTE FROM clause is forbidden"));
                }
                let cte_name = cte.alias.name.value.trim().to_ascii_lowercase();
                if cte_name.is_empty() {
                    return break_err(rejected("CTE name cannot be empty"));
                }
                self.cte_stack.push(scope.clone());
                let res = VisitMut::visit(&mut cte.query, self);
                self.cte_stack.pop();
                res?;
                scope.insert(cte_name);
            }
            Some(with_clause)
        } else {
            None
        };

        self.cte_stack.push(scope);
        self.saved_with_stack.push(rewritten_with);
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, query: &mut Query) -> ControlFlow<Self::Break> {
        self.cte_stack.pop();
        if let Some(restored) = self.saved_with_stack.pop() {
            query.with = restored;
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_select(&mut self, select: &mut Select) -> ControlFlow<Self::Break> {
        if select.into.is_some() {
            return break_err(rejected("SELECT ... INTO is forbidden"));
        }
        if !select.lateral_views.is_empty() {
            return break_err(rejected("LATERAL VIEW is forbidden"));
        }
        if !select.connect_by.is_empty() {
            return break_err(rejected("CONNECT BY is forbidden"));
        }
        if select.prewhere.is_some() {
            return break_err(rejected("PREWHERE is forbidden"));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, tf: &mut TableFactor) -> ControlFlow<Self::Break> {
        let mut pending = PendingTableRewrite::None;
        match tf {
            TableFactor::Table {
                name,
                alias,
                args,
                with_hints,
                version,
                partitions,
                json_path,
                sample,
                index_directives,
                inheritance,
                ..
            } => {
                if !with_hints.is_empty()
                    || version.is_some()
                    || !partitions.is_empty()
                    || json_path.is_some()
                    || sample.is_some()
                    || !index_directives.is_empty()
                    || inheritance.is_some()
                {
                    return break_err(rejected(
                        "table hints, time-travel versions, samples, and partitions are forbidden",
                    ));
                }
                if let Some(tf_args) = args {
                    if tf_args.settings.is_some() {
                        return break_err(rejected("table function SETTINGS are forbidden"));
                    }
                    if let Err(err) = check_allowed_table_function_name(name) {
                        return break_err(err);
                    }
                } else {
                    match self.rewrite_table_relation(name, alias) {
                        Ok(p) => pending = p,
                        Err(err) => return break_err(err),
                    }
                }
            }
            TableFactor::Derived { sample, .. } => {
                if sample.is_some() {
                    return break_err(rejected("TABLESAMPLE on subquery is forbidden"));
                }
            }
            TableFactor::NestedJoin { .. } | TableFactor::UNNEST { .. } => {}
            TableFactor::TableFunction { expr, .. } => {
                let Expr::Function(func) = expr else {
                    return break_err(rejected("unsupported TABLE(...) expression"));
                };
                if let Err(err) = check_allowed_table_function_name(&func.name) {
                    return break_err(err);
                }
                self.in_allowed_table_func_expr = self.in_allowed_table_func_expr.saturating_add(1);
            }
            TableFactor::Function { name, .. } => {
                if let Err(err) = check_allowed_table_function_name(name) {
                    return break_err(err);
                }
            }
            _ => {
                return break_err(rejected("unsupported FROM table factor construct"));
            }
        }
        self.pending_rewrites.push(pending);
        ControlFlow::Continue(())
    }

    fn post_visit_table_factor(&mut self, tf: &mut TableFactor) -> ControlFlow<Self::Break> {
        if matches!(tf, TableFactor::TableFunction { .. }) {
            self.in_allowed_table_func_expr = self.in_allowed_table_func_expr.saturating_sub(1);
        }
        if let Some(PendingTableRewrite::Subquery {
            subquery_sql,
            alias,
        }) = self.pending_rewrites.pop()
        {
            match parse_policy_subquery(&subquery_sql) {
                Ok(subquery) => {
                    *tf = TableFactor::Derived {
                        lateral: false,
                        subquery,
                        alias: Some(alias),
                        sample: None,
                    };
                }
                Err(err) => return break_err(err),
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        match expr {
            Expr::Function(func) => {
                if let Err(err) = self.validate_scalar_or_agg_function(func) {
                    return break_err(err);
                }
            }
            Expr::In {
                target: InTarget::Table { .. },
                ..
            } => {
                return break_err(rejected("IN <table> shorthand is forbidden"));
            }
            Expr::Value(val_span) => {
                if let Value::Placeholder(raw) = &val_span.value {
                    let raw_copy = raw.clone();
                    match self.bind_placeholder(&raw_copy) {
                        Ok(replacement) => *expr = replacement,
                        Err(err) => return break_err(err),
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        if let Expr::In {
            target: InTarget::List(list),
            ..
        } = expr
        {
            let mut flattened = Vec::with_capacity(list.len());
            for item in std::mem::take(list) {
                if let Expr::Tuple(elems) = item {
                    flattened.extend(elems);
                } else {
                    flattened.push(item);
                }
            }
            *list = flattened;
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_value(&mut self, value: &mut ValueWithSpan) -> ControlFlow<Self::Break> {
        // Catch placeholders outside Expr::Value (e.g. LIMIT :n or OFFSET :n).
        if let Value::Placeholder(raw) = &value.value {
            let raw_copy = raw.clone();
            if raw_copy != "?" {
                match self.param_mode {
                    ParamBindMode::Literal(_) | ParamBindMode::PositionalPlaceholder => {}
                    ParamBindMode::ForbidParams => {
                        return break_err(rejected(
                            "placeholders are forbidden in materialized derived views",
                        ));
                    }
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn post_visit_value(&mut self, value: &mut ValueWithSpan) -> ControlFlow<Self::Break> {
        // If a placeholder survived outside an Expr::Value node (e.g. LIMIT :n), bind scalar value here.
        if let Value::Placeholder(raw) = &value.value
            && raw != "?"
        {
            let raw_copy = raw.clone();
            match self.bind_placeholder(&raw_copy) {
                Ok(Expr::Value(new_val)) => *value = new_val,
                Ok(_) => {
                    return break_err(rejected(
                        "non-scalar or signed parameter is not valid in LIMIT/OFFSET position",
                    ));
                }
                Err(err) => return break_err(err),
            }
        }
        ControlFlow::Continue(())
    }
}

/// Rule 8 (`05-STEP04` §04.3): Re-parses the rewritten SQL and verifies Rules 1..4 still hold.
fn verify_rewritten_sql(
    rewritten_sql: &str,
    catalog: &TenantCatalog,
    target: GuardTarget,
    allow_positional_qmark: bool,
) -> Result<(), AppError> {
    let stmts = Parser::parse_sql(&DuckDbDialect {}, rewritten_sql)
        .map_err(|err| rejected(format!("post-rewrite SQL verification parse failed: {err}")))?;
    if stmts.len() != 1 {
        return Err(rejected(
            "post-rewrite SQL verification failed: multiple statements",
        ));
    }
    let Some(Statement::Query(query)) = stmts.first() else {
        return Err(rejected(
            "post-rewrite SQL verification failed: non-query statement",
        ));
    };

    let expected_schema = format!("tenant_{}", catalog.tenant_id);
    let valid_lake_tables: HashSet<&str> = catalog
        .datasets
        .values()
        .map(|d| d.lake_table.as_str())
        .collect();

    let mut verifier = PostRewriteVerifier {
        catalog,
        expected_schema: &expected_schema,
        valid_lake_tables: &valid_lake_tables,
        target,
        allow_positional_qmark,
        cte_stack: vec![BTreeSet::new()],
        in_allowed_table_func_expr: 0,
    };
    if let ControlFlow::Break(err) = query.visit(&mut verifier) {
        return Err(*err);
    }
    Ok(())
}

struct PostRewriteVerifier<'a> {
    catalog: &'a TenantCatalog,
    expected_schema: &'a str,
    valid_lake_tables: &'a HashSet<&'a str>,
    target: GuardTarget,
    allow_positional_qmark: bool,
    cte_stack: Vec<BTreeSet<String>>,
    in_allowed_table_func_expr: usize,
}

impl PostRewriteVerifier<'_> {
    fn current_ctes(&self) -> BTreeSet<String> {
        self.cte_stack.last().cloned().unwrap_or_default()
    }
}

impl Visitor for PostRewriteVerifier<'_> {
    type Break = Box<AppError>;

    fn pre_visit_statement(&mut self, stmt: &Statement) -> ControlFlow<Self::Break> {
        if !matches!(stmt, Statement::Query(_)) {
            return break_err(rejected("post-rewrite check: non-query statement"));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<Self::Break> {
        let mut scope = self.current_ctes();
        if let Some(ref with_clause) = q.with {
            for cte in &with_clause.cte_tables {
                scope.insert(cte.alias.name.value.to_ascii_lowercase());
            }
        }
        self.cte_stack.push(scope);
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _q: &Query) -> ControlFlow<Self::Break> {
        self.cte_stack.pop();
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, tf: &TableFactor) -> ControlFlow<Self::Break> {
        match tf {
            TableFactor::Table { name, args, .. } => {
                if args.is_some() {
                    if let Err(err) = check_allowed_table_function_name(name) {
                        return break_err(err);
                    }
                    return ControlFlow::Continue(());
                }
                let parts = match extract_ident_parts(name) {
                    Ok(p) => p,
                    Err(err) => return break_err(err),
                };
                match parts.as_slice() {
                    [only] => {
                        let lower = only.value.to_ascii_lowercase();
                        if !self.current_ctes().contains(&lower) {
                            return break_err(rejected(format!(
                                "post-rewrite check: unrewritten bare relation '{lower}'"
                            )));
                        }
                    }
                    [p0, p1] => {
                        let s0 = p0.value.to_ascii_lowercase();
                        let s1 = p1.value.to_ascii_lowercase();
                        if s0 != "ax"
                            || !ALLOWED_PLATFORM_VIEWS.contains(&s1.as_str())
                            || self.target == GuardTarget::MaterializeView
                        {
                            return break_err(rejected(format!(
                                "post-rewrite check: unauthorized 2-part relation '{s0}.{s1}'"
                            )));
                        }
                    }
                    [p0, p1, p2] => {
                        let s0 = p0.value.to_ascii_lowercase();
                        if s0 == "lake" {
                            if p1.value != self.expected_schema
                                || !self.valid_lake_tables.contains(p2.value.as_str())
                            {
                                return break_err(rejected(
                                    "post-rewrite check: cross-tenant or unregistered lake table",
                                ));
                            }
                        } else if let Some(conn_name) = s0.strip_prefix("live_") {
                            let GuardTarget::InteractiveQuery { allow_live } = self.target else {
                                return break_err(rejected(
                                    "post-rewrite check: live relation in materialize",
                                ));
                            };
                            if !allow_live
                                || !self.catalog.has_live_permission
                                || !self.catalog.has_live_connection(conn_name)
                            {
                                return break_err(rejected(
                                    "post-rewrite check: unauthorized live relation",
                                ));
                            }
                        } else {
                            return break_err(rejected(
                                "post-rewrite check: unauthorized 3-part relation",
                            ));
                        }
                    }
                    _ => return break_err(rejected("post-rewrite check: invalid relation parts")),
                }
            }
            TableFactor::TableFunction { expr, .. } => {
                let Expr::Function(func) = expr else {
                    return break_err(rejected("post-rewrite check: invalid TABLE function"));
                };
                if let Err(err) = check_allowed_table_function_name(&func.name) {
                    return break_err(err);
                }
                self.in_allowed_table_func_expr = self.in_allowed_table_func_expr.saturating_add(1);
            }
            TableFactor::Function { name, .. } => {
                if let Err(err) = check_allowed_table_function_name(name) {
                    return break_err(err);
                }
            }
            TableFactor::Derived { .. }
            | TableFactor::NestedJoin { .. }
            | TableFactor::UNNEST { .. } => {}
            _ => return break_err(rejected("post-rewrite check: forbidden table factor")),
        }
        ControlFlow::Continue(())
    }

    fn post_visit_table_factor(&mut self, tf: &TableFactor) -> ControlFlow<Self::Break> {
        if matches!(tf, TableFactor::TableFunction { .. }) {
            self.in_allowed_table_func_expr = self.in_allowed_table_func_expr.saturating_sub(1);
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(func) = expr {
            let Ok(parts) = extract_ident_parts(&func.name) else {
                return break_err(rejected("post-rewrite check: invalid function name"));
            };
            let [only] = parts.as_slice() else {
                return break_err(rejected("post-rewrite check: qualified function forbidden"));
            };
            let lower = only.value.to_ascii_lowercase();
            let allowed_tf = self.in_allowed_table_func_expr > 0
                && ALLOWED_TABLE_FUNCTIONS.contains(&lower.as_str());
            if !allowed_tf && is_forbidden_function_name(&lower) {
                return break_err(rejected(format!(
                    "post-rewrite check: forbidden function '{lower}'"
                )));
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_value(&mut self, value: &ValueWithSpan) -> ControlFlow<Self::Break> {
        if let Value::Placeholder(p) = &value.value
            && !(self.allow_positional_qmark && p == "?")
        {
            return break_err(rejected(format!(
                "post-rewrite check: unbound placeholder '{p}'"
            )));
        }
        ControlFlow::Continue(())
    }
}

const MAX_LINEAGE_DEPTH: usize = 64;

#[derive(Debug, Clone, Default)]
struct QueryLineage<'a> {
    columns: Vec<OutputColumnOrigin>,
    wildcard_datasets: Vec<&'a CatalogDatasetEntry>,
}

#[derive(Debug, Clone)]
enum RelationKind<'a> {
    Dataset(&'a CatalogDatasetEntry),
    Derived(QueryLineage<'a>),
    Opaque,
}

#[derive(Debug, Clone)]
struct RelationEntry<'a> {
    alias: Option<String>,
    kind: RelationKind<'a>,
}

#[derive(Debug, Clone)]
enum ResolvedColumn {
    Direct {
        dataset_id: Uuid,
        column_name: String,
        pii_category: PiiCategory,
    },
    DerivedPii {
        pii_category: PiiCategory,
    },
    NonSensitive,
}

impl ResolvedColumn {
    const fn pii_category(&self) -> Option<PiiCategory> {
        match self {
            Self::Direct {
                pii_category: PiiCategory::None,
                ..
            }
            | Self::DerivedPii {
                pii_category: PiiCategory::None,
            }
            | Self::NonSensitive => None,
            Self::Direct { pii_category, .. } | Self::DerivedPii { pii_category } => {
                Some(*pii_category)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum OriginSensitivity {
    DirectPii {
        dataset_id: Uuid,
        column_name: String,
        pii_category: PiiCategory,
    },
    DerivedPii(PiiCategory),
    Wildcard,
    Clean,
}

#[derive(Clone, Copy)]
struct LineageCtx<'a, 'b> {
    catalog: &'a TenantCatalog,
    cte_scope: &'b HashMap<String, QueryLineage<'a>>,
    outer_scopes: &'b [Vec<RelationEntry<'a>>],
    depth: usize,
}

/// Analyzes the outer query projection before rewriting so `result.rs` knows whether each output
/// column is a direct PII column (`MaskRule` by category) or a derived expression touching a PII
/// column (`MaskRule::Redact`), recursively resolving subqueries, CTEs, and `UNION` branches.
fn analyze_outer_lineage(query: &Query, catalog: &TenantCatalog) -> Vec<OutputColumnOrigin> {
    let empty_ctes = HashMap::new();
    let ctx = LineageCtx {
        catalog,
        cte_scope: &empty_ctes,
        outer_scopes: &[],
        depth: 0,
    };
    analyze_query_lineage(query, ctx).columns
}

fn analyze_query_lineage<'a>(query: &Query, ctx: LineageCtx<'a, '_>) -> QueryLineage<'a> {
    if ctx.depth > MAX_LINEAGE_DEPTH {
        return QueryLineage::default();
    }
    let Some(ref with_clause) = query.with else {
        return analyze_set_expr_lineage(&query.body, ctx);
    };
    let local_ctes = build_with_cte_scope(with_clause, ctx);
    let body_ctx = LineageCtx {
        cte_scope: &local_ctes,
        ..ctx
    };
    analyze_set_expr_lineage(&query.body, body_ctx)
}

fn build_with_cte_scope<'a>(
    with_clause: &With,
    ctx: LineageCtx<'a, '_>,
) -> HashMap<String, QueryLineage<'a>> {
    let mut local_ctes = ctx.cte_scope.clone();
    if with_clause.recursive {
        for cte in &with_clause.cte_tables {
            let cte_name = cte.alias.name.value.trim().to_ascii_lowercase();
            if !cte_name.is_empty() {
                local_ctes.insert(cte_name, QueryLineage::default());
            }
        }
    }
    let passes: usize = if with_clause.recursive { 2 } else { 1 };
    for _ in 0..passes {
        for cte in &with_clause.cte_tables {
            let cte_name = cte.alias.name.value.trim().to_ascii_lowercase();
            if cte_name.is_empty() {
                continue;
            }
            let cte_ctx = LineageCtx {
                catalog: ctx.catalog,
                cte_scope: &local_ctes,
                outer_scopes: ctx.outer_scopes,
                depth: ctx.depth.saturating_add(1),
            };
            let mut cte_lineage = analyze_query_lineage(&cte.query, cte_ctx);
            apply_table_column_aliases(&mut cte_lineage, &cte.alias.columns);
            local_ctes.insert(cte_name, cte_lineage);
        }
    }
    local_ctes
}

fn analyze_set_expr_lineage<'a>(set_expr: &SetExpr, ctx: LineageCtx<'a, '_>) -> QueryLineage<'a> {
    if ctx.depth > MAX_LINEAGE_DEPTH {
        return QueryLineage::default();
    }
    match set_expr {
        SetExpr::Select(select) => analyze_select_lineage(select, ctx),
        SetExpr::Query(inner_query) => {
            let next_ctx = LineageCtx {
                depth: ctx.depth.saturating_add(1),
                ..ctx
            };
            analyze_query_lineage(inner_query, next_ctx)
        }
        SetExpr::SetOperation {
            left,
            set_quantifier,
            right,
            ..
        } => {
            let next_ctx = LineageCtx {
                depth: ctx.depth.saturating_add(1),
                ..ctx
            };
            let left_lineage = analyze_set_expr_lineage(left, next_ctx);
            let right_lineage = analyze_set_expr_lineage(right, next_ctx);
            merge_set_operation_lineage(left_lineage, right_lineage, *set_quantifier)
        }
        _ => QueryLineage::default(),
    }
}

fn analyze_select_lineage<'a>(select: &Select, ctx: LineageCtx<'a, '_>) -> QueryLineage<'a> {
    let relations = collect_select_relations(select, ctx);
    let mut out = QueryLineage {
        columns: Vec::with_capacity(select.projection.len()),
        wildcard_datasets: Vec::new(),
    };

    for item in &select.projection {
        match item {
            SelectItem::Wildcard(opts) => {
                expand_unqualified_wildcard(opts, &relations, ctx, &mut out);
            }
            SelectItem::QualifiedWildcard(kind, opts) => {
                expand_qualified_wildcard(kind, opts, &relations, ctx, &mut out);
            }
            SelectItem::UnnamedExpr(expr) => {
                out.columns
                    .push(classify_projected_expr(expr, None, &relations, ctx));
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                out.columns.push(classify_projected_expr(
                    expr,
                    Some(alias.value.clone()),
                    &relations,
                    ctx,
                ));
            }
            SelectItem::ExprWithAliases { expr, aliases } => {
                let first_alias = aliases.first().map(|a| a.value.clone());
                out.columns
                    .push(classify_projected_expr(expr, first_alias, &relations, ctx));
            }
        }
    }

    out
}

fn collect_select_relations<'a>(
    select: &Select,
    ctx: LineageCtx<'a, '_>,
) -> Vec<RelationEntry<'a>> {
    let mut relations = Vec::new();
    for twj in &select.from {
        collect_table_with_joins(twj, ctx, &mut relations);
    }
    relations
}

fn collect_table_with_joins<'a>(
    twj: &TableWithJoins,
    ctx: LineageCtx<'a, '_>,
    relations: &mut Vec<RelationEntry<'a>>,
) {
    collect_table_factor(&twj.relation, ctx, relations);
    for join in &twj.joins {
        collect_table_factor(&join.relation, ctx, relations);
    }
}

fn collect_table_factor<'a>(
    tf: &TableFactor,
    ctx: LineageCtx<'a, '_>,
    relations: &mut Vec<RelationEntry<'a>>,
) {
    match tf {
        TableFactor::Table {
            name,
            alias,
            args: None,
            ..
        } => {
            collect_named_table_relation(name, alias.as_ref(), ctx, relations);
        }
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            collect_derived_relation(subquery, alias.as_ref(), ctx, relations);
        }
        TableFactor::NestedJoin {
            table_with_joins,
            alias,
            ..
        } => {
            if let Some(tbl_alias) = alias.as_ref() {
                let mut nested_rels = Vec::new();
                collect_table_with_joins(table_with_joins, ctx, &mut nested_rels);
                let mut nested_ql =
                    expand_relations_into_lineage(&nested_rels, !tbl_alias.columns.is_empty());
                apply_table_column_aliases(&mut nested_ql, &tbl_alias.columns);
                relations.push(RelationEntry {
                    alias: Some(tbl_alias.name.value.to_ascii_lowercase()),
                    kind: RelationKind::Derived(nested_ql),
                });
            } else {
                collect_table_with_joins(table_with_joins, ctx, relations);
            }
        }
        TableFactor::UNNEST { alias, .. }
        | TableFactor::TableFunction { alias, .. }
        | TableFactor::Function { alias, .. }
        | TableFactor::Table {
            alias,
            args: Some(_),
            ..
        } => {
            collect_function_relation(tf, alias.as_ref(), ctx, relations);
        }
        _ => {}
    }
}

fn collect_derived_relation<'a>(
    subquery: &Query,
    alias: Option<&TableAlias>,
    ctx: LineageCtx<'a, '_>,
    relations: &mut Vec<RelationEntry<'a>>,
) {
    let mut sub_outer = ctx.outer_scopes.to_vec();
    if !relations.is_empty() {
        sub_outer.push(relations.clone());
    }
    let sub_ctx = LineageCtx {
        catalog: ctx.catalog,
        cte_scope: ctx.cte_scope,
        outer_scopes: &sub_outer,
        depth: ctx.depth.saturating_add(1),
    };
    let mut sub_ql = analyze_query_lineage(subquery, sub_ctx);
    if let Some(tbl_alias) = alias {
        apply_table_column_aliases(&mut sub_ql, &tbl_alias.columns);
    }
    relations.push(RelationEntry {
        alias: alias.map(|a| a.name.value.to_ascii_lowercase()),
        kind: RelationKind::Derived(sub_ql),
    });
}

fn collect_function_relation<'a>(
    tf: &TableFactor,
    alias: Option<&TableAlias>,
    ctx: LineageCtx<'a, '_>,
    relations: &mut Vec<RelationEntry<'a>>,
) {
    let alias_key = alias.map(|a| a.name.value.to_ascii_lowercase());
    if let Some(pii) = find_any_pii_in_ast(tf, relations, ctx) {
        let mut fn_ql = QueryLineage {
            columns: vec![OutputColumnOrigin::DerivedFromPii {
                output_name: alias.map(|a| a.name.value.clone()),
                pii_category: pii,
            }],
            wildcard_datasets: Vec::new(),
        };
        if let Some(tbl_alias) = alias {
            apply_table_column_aliases(&mut fn_ql, &tbl_alias.columns);
        }
        relations.push(RelationEntry {
            alias: alias_key,
            kind: RelationKind::Derived(fn_ql),
        });
    } else {
        relations.push(RelationEntry {
            alias: alias_key,
            kind: RelationKind::Opaque,
        });
    }
}

fn collect_named_table_relation<'a>(
    name: &ObjectName,
    alias: Option<&TableAlias>,
    ctx: LineageCtx<'a, '_>,
    relations: &mut Vec<RelationEntry<'a>>,
) {
    if let [ObjectNamePart::Identifier(ident)] = name.0.as_slice() {
        let lower = ident.value.to_ascii_lowercase();
        if let Some(cte_ql) = ctx.cte_scope.get(&lower) {
            let mut local_ql = cte_ql.clone();
            if let Some(tbl_alias) = alias {
                apply_table_column_aliases(&mut local_ql, &tbl_alias.columns);
            }
            let key = alias.map_or(lower, |a| a.name.value.to_ascii_lowercase());
            relations.push(RelationEntry {
                alias: Some(key),
                kind: RelationKind::Derived(local_ql),
            });
            return;
        }
        if let Some(ds) = ctx.catalog.dataset(&lower) {
            let key = alias.map_or(lower, |a| a.name.value.to_ascii_lowercase());
            if let Some(tbl_alias) = alias
                && !tbl_alias.columns.is_empty()
            {
                let mut ds_ql = QueryLineage {
                    columns: dataset_ordered_origins(ds),
                    wildcard_datasets: Vec::new(),
                };
                apply_table_column_aliases(&mut ds_ql, &tbl_alias.columns);
                relations.push(RelationEntry {
                    alias: Some(key),
                    kind: RelationKind::Derived(ds_ql),
                });
            } else {
                relations.push(RelationEntry {
                    alias: Some(key),
                    kind: RelationKind::Dataset(ds),
                });
            }
            return;
        }
    }
    let fallback_alias = alias
        .map(|a| a.name.value.to_ascii_lowercase())
        .or_else(|| {
            name.0
                .last()
                .and_then(ObjectNamePart::as_ident)
                .map(|i| i.value.to_ascii_lowercase())
        });
    relations.push(RelationEntry {
        alias: fallback_alias,
        kind: RelationKind::Opaque,
    });
}

const fn wildcard_opts_have_modifiers(opts: &WildcardAdditionalOptions) -> bool {
    opts.opt_exclude.is_some()
        || opts.opt_except.is_some()
        || opts.opt_replace.is_some()
        || opts.opt_rename.is_some()
}

fn push_origin_dedup_wildcard(out: &mut Vec<OutputColumnOrigin>, origin: OutputColumnOrigin) {
    if matches!(origin, OutputColumnOrigin::Wildcard)
        && matches!(out.last(), Some(OutputColumnOrigin::Wildcard))
    {
        return;
    }
    out.push(origin);
}

fn expand_single_relation<'a>(
    rel: &RelationEntry<'a>,
    force_expand_dataset: bool,
    out: &mut QueryLineage<'a>,
) {
    match &rel.kind {
        RelationKind::Derived(ql) => {
            for col in &ql.columns {
                push_origin_dedup_wildcard(&mut out.columns, col.clone());
            }
            append_unique_datasets(&mut out.wildcard_datasets, &ql.wildcard_datasets);
        }
        RelationKind::Dataset(ds) => {
            if force_expand_dataset && !ds.columns.is_empty() {
                for col in dataset_ordered_origins(ds) {
                    out.columns.push(col);
                }
            } else {
                push_origin_dedup_wildcard(&mut out.columns, OutputColumnOrigin::Wildcard);
                append_unique_datasets(&mut out.wildcard_datasets, &[*ds]);
            }
        }
        RelationKind::Opaque => {
            push_origin_dedup_wildcard(&mut out.columns, OutputColumnOrigin::Wildcard);
        }
    }
}

fn expand_relations_into_lineage<'a>(
    relations: &[RelationEntry<'a>],
    force_expand_dataset: bool,
) -> QueryLineage<'a> {
    let mut out = QueryLineage::default();
    if relations.is_empty() {
        out.columns.push(OutputColumnOrigin::Wildcard);
        return out;
    }
    for rel in relations {
        expand_single_relation(rel, force_expand_dataset, &mut out);
    }
    out
}

fn expand_unqualified_wildcard<'a>(
    opts: &WildcardAdditionalOptions,
    relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
    out: &mut QueryLineage<'a>,
) {
    let has_mods = wildcard_opts_have_modifiers(opts);
    let mut expanded = expand_relations_into_lineage(relations, has_mods);
    if has_mods {
        apply_wildcard_options(&mut expanded.columns, opts, relations, ctx);
    }
    for col in expanded.columns {
        push_origin_dedup_wildcard(&mut out.columns, col);
    }
    append_unique_datasets(&mut out.wildcard_datasets, &expanded.wildcard_datasets);
}

fn expand_qualified_wildcard<'a>(
    kind: &SelectItemQualifiedWildcardKind,
    opts: &WildcardAdditionalOptions,
    relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
    out: &mut QueryLineage<'a>,
) {
    match kind {
        SelectItemQualifiedWildcardKind::ObjectName(obj_name) => {
            let has_mods = wildcard_opts_have_modifiers(opts);
            let mut matched_any = false;
            let mut expanded = QueryLineage::default();
            if let [ObjectNamePart::Identifier(tbl_ident)] = obj_name.0.as_slice() {
                let tbl_lower = tbl_ident.value.to_ascii_lowercase();
                for rel in relations {
                    if rel.alias.as_deref() == Some(tbl_lower.as_str()) {
                        matched_any = true;
                        expand_single_relation(rel, has_mods, &mut expanded);
                    }
                }
            }
            if !matched_any {
                push_origin_dedup_wildcard(&mut out.columns, OutputColumnOrigin::Wildcard);
                return;
            }
            if has_mods {
                apply_wildcard_options(&mut expanded.columns, opts, relations, ctx);
            }
            for col in expanded.columns {
                push_origin_dedup_wildcard(&mut out.columns, col);
            }
            append_unique_datasets(&mut out.wildcard_datasets, &expanded.wildcard_datasets);
        }
        SelectItemQualifiedWildcardKind::Expr(expr) => {
            if let Some(pii) = find_any_pii_in_ast(expr, relations, ctx) {
                out.columns.push(OutputColumnOrigin::DerivedFromPii {
                    output_name: None,
                    pii_category: pii,
                });
            } else {
                push_origin_dedup_wildcard(&mut out.columns, OutputColumnOrigin::Wildcard);
            }
        }
    }
}

fn collect_excluded_wildcard_cols(opts: &WildcardAdditionalOptions) -> HashSet<String> {
    let mut excluded = HashSet::new();
    if let Some(excl) = &opts.opt_exclude {
        match excl {
            ExcludeSelectItem::Single(obj) => {
                if let Some(ident) = obj.0.last().and_then(ObjectNamePart::as_ident) {
                    excluded.insert(ident.value.to_ascii_lowercase());
                }
            }
            ExcludeSelectItem::Multiple(objs) => {
                for obj in objs {
                    if let Some(ident) = obj.0.last().and_then(ObjectNamePart::as_ident) {
                        excluded.insert(ident.value.to_ascii_lowercase());
                    }
                }
            }
        }
    }
    if let Some(exc) = &opts.opt_except {
        excluded.insert(exc.first_element.value.to_ascii_lowercase());
        for ident in &exc.additional_elements {
            excluded.insert(ident.value.to_ascii_lowercase());
        }
    }
    excluded
}

fn apply_wildcard_options<'a>(
    cols: &mut Vec<OutputColumnOrigin>,
    opts: &WildcardAdditionalOptions,
    relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
) {
    let excluded = collect_excluded_wildcard_cols(opts);
    if !excluded.is_empty()
        && !cols
            .iter()
            .any(|c| matches!(c, OutputColumnOrigin::Wildcard))
    {
        cols.retain(|c| {
            origin_output_name(c).is_none_or(|name| !excluded.contains(&name.to_ascii_lowercase()))
        });
    }
    if let Some(rep) = &opts.opt_replace {
        for elem in &rep.items {
            let new_origin = classify_projected_expr(
                &elem.expr,
                Some(elem.column_name.value.clone()),
                relations,
                ctx,
            );
            let target = elem.column_name.value.to_ascii_lowercase();
            if let Some(slot) = cols
                .iter_mut()
                .find(|c| origin_output_name(c).is_some_and(|n| n.eq_ignore_ascii_case(&target)))
            {
                *slot = new_origin;
            }
        }
    }
    if let Some(ren) = &opts.opt_rename {
        let pairs: &[IdentWithAlias] = match ren {
            RenameSelectItem::Single(one) => std::slice::from_ref(one),
            RenameSelectItem::Multiple(many) => many.as_slice(),
        };
        for pair in pairs {
            let target = pair.ident.value.to_ascii_lowercase();
            if let Some(slot) = cols
                .iter_mut()
                .find(|c| origin_output_name(c).is_some_and(|n| n.eq_ignore_ascii_case(&target)))
            {
                set_origin_output_name(slot, pair.alias.value.clone());
            }
        }
    }
}

fn origin_output_name(origin: &OutputColumnOrigin) -> Option<&str> {
    match origin {
        OutputColumnOrigin::Direct { output_name, .. } => Some(output_name.as_str()),
        OutputColumnOrigin::DerivedFromPii { output_name, .. }
        | OutputColumnOrigin::NonSensitive { output_name } => output_name.as_deref(),
        OutputColumnOrigin::Wildcard => None,
    }
}

fn set_origin_output_name(origin: &mut OutputColumnOrigin, new_name: String) {
    match origin {
        OutputColumnOrigin::Direct { output_name, .. } => {
            *output_name = new_name;
        }
        OutputColumnOrigin::DerivedFromPii { output_name, .. }
        | OutputColumnOrigin::NonSensitive { output_name } => {
            *output_name = Some(new_name);
        }
        OutputColumnOrigin::Wildcard => {}
    }
}

fn dataset_ordered_origins(ds: &CatalogDatasetEntry) -> Vec<OutputColumnOrigin> {
    if ds.columns.is_empty() {
        return vec![OutputColumnOrigin::Wildcard];
    }
    let mut metas: Vec<&ColumnMeta> = ds.columns.values().collect();
    metas.sort_by(|a, b| {
        a.ordinal
            .cmp(&b.ordinal)
            .then_with(|| a.column_name.cmp(&b.column_name))
    });
    metas
        .into_iter()
        .map(|meta| OutputColumnOrigin::Direct {
            dataset_id: ds.id,
            column_name: meta.column_name.clone(),
            output_name: meta.column_name.clone(),
            pii_category: meta.pii_category,
        })
        .collect()
}

fn apply_table_column_aliases(ql: &mut QueryLineage<'_>, col_defs: &[TableAliasColumnDef]) {
    if col_defs.is_empty() {
        return;
    }
    if ql.columns.len() == 1
        && matches!(ql.columns.first(), Some(OutputColumnOrigin::Wildcard))
        && let [single_ds] = ql.wildcard_datasets.as_slice()
        && !single_ds.columns.is_empty()
    {
        ql.columns = dataset_ordered_origins(single_ds);
        ql.wildcard_datasets.clear();
    }
    for (origin, col_def) in ql.columns.iter_mut().zip(col_defs) {
        set_origin_output_name(origin, col_def.name.value.clone());
    }
}

fn append_unique_datasets<'a>(
    dst: &mut Vec<&'a CatalogDatasetEntry>,
    src: &[&'a CatalogDatasetEntry],
) {
    for ds in src {
        if !dst.iter().any(|existing| existing.id == ds.id) {
            dst.push(*ds);
        }
    }
}

fn first_pii_in_datasets(datasets: &[&CatalogDatasetEntry]) -> Option<PiiCategory> {
    for ds in datasets {
        for meta in ds.columns.values() {
            if meta.pii_category != PiiCategory::None {
                return Some(meta.pii_category);
            }
        }
    }
    None
}

fn classify_origin_sensitivity(origin: &OutputColumnOrigin) -> OriginSensitivity {
    match origin {
        OutputColumnOrigin::Direct {
            dataset_id,
            column_name,
            pii_category,
            ..
        } if *pii_category != PiiCategory::None => OriginSensitivity::DirectPii {
            dataset_id: *dataset_id,
            column_name: column_name.clone(),
            pii_category: *pii_category,
        },
        OutputColumnOrigin::DerivedFromPii { pii_category, .. }
            if *pii_category != PiiCategory::None =>
        {
            OriginSensitivity::DerivedPii(*pii_category)
        }
        OutputColumnOrigin::Wildcard => OriginSensitivity::Wildcard,
        OutputColumnOrigin::Direct { .. }
        | OutputColumnOrigin::DerivedFromPii { .. }
        | OutputColumnOrigin::NonSensitive { .. } => OriginSensitivity::Clean,
    }
}

fn build_direct_origin(
    dataset_id: Uuid,
    column_name: String,
    left_name: Option<String>,
    pii_category: PiiCategory,
) -> OutputColumnOrigin {
    let output_name = left_name.unwrap_or_else(|| column_name.clone());
    OutputColumnOrigin::Direct {
        dataset_id,
        column_name,
        output_name,
        pii_category,
    }
}

fn merge_two_origins(left: &OutputColumnOrigin, right: &OutputColumnOrigin) -> OutputColumnOrigin {
    let left_name = origin_output_name(left).map(String::from);
    match (
        classify_origin_sensitivity(left),
        classify_origin_sensitivity(right),
    ) {
        (OriginSensitivity::Clean, OriginSensitivity::Clean) => left.clone(),
        (OriginSensitivity::DerivedPii(left_pii), _)
        | (
            OriginSensitivity::DirectPii {
                pii_category: left_pii,
                ..
            },
            OriginSensitivity::DerivedPii(_),
        ) => OutputColumnOrigin::DerivedFromPii {
            output_name: left_name,
            pii_category: left_pii,
        },
        (_, OriginSensitivity::DerivedPii(right_pii)) => OutputColumnOrigin::DerivedFromPii {
            output_name: left_name,
            pii_category: right_pii,
        },
        (
            OriginSensitivity::DirectPii {
                dataset_id,
                column_name,
                pii_category,
            },
            OriginSensitivity::Clean | OriginSensitivity::Wildcard,
        ) => build_direct_origin(dataset_id, column_name, left_name, pii_category),
        (
            OriginSensitivity::Clean | OriginSensitivity::Wildcard,
            OriginSensitivity::DirectPii {
                dataset_id,
                column_name,
                pii_category,
            },
        ) => {
            if left_name.is_some() {
                build_direct_origin(dataset_id, column_name, left_name, pii_category)
            } else {
                OutputColumnOrigin::DerivedFromPii {
                    output_name: None,
                    pii_category,
                }
            }
        }
        (
            OriginSensitivity::DirectPii {
                dataset_id,
                column_name,
                pii_category: left_pii,
            },
            OriginSensitivity::DirectPii {
                pii_category: right_pii,
                ..
            },
        ) => {
            if left_pii == right_pii {
                build_direct_origin(dataset_id, column_name, left_name, left_pii)
            } else {
                OutputColumnOrigin::DerivedFromPii {
                    output_name: left_name,
                    pii_category: left_pii,
                }
            }
        }
        (OriginSensitivity::Wildcard, OriginSensitivity::Clean | OriginSensitivity::Wildcard)
        | (OriginSensitivity::Clean, OriginSensitivity::Wildcard) => OutputColumnOrigin::Wildcard,
    }
}

fn try_expand_wildcard_datasets(
    cols: &[OutputColumnOrigin],
    wildcard_datasets: &[&CatalogDatasetEntry],
    target_len: usize,
) -> Option<Vec<OutputColumnOrigin>> {
    if cols.len() != 1 || !matches!(cols.first(), Some(OutputColumnOrigin::Wildcard)) {
        return None;
    }
    if wildcard_datasets.is_empty() || wildcard_datasets.iter().any(|ds| ds.columns.is_empty()) {
        return None;
    }
    let expanded: Vec<OutputColumnOrigin> = wildcard_datasets
        .iter()
        .copied()
        .flat_map(dataset_ordered_origins)
        .collect();
    (expanded.len() == target_len).then_some(expanded)
}

fn upgrade_clean_columns_to_derived(
    cols: &[OutputColumnOrigin],
    pii_category: PiiCategory,
) -> Vec<OutputColumnOrigin> {
    cols.iter()
        .map(|col| match classify_origin_sensitivity(col) {
            OriginSensitivity::Clean => OutputColumnOrigin::DerivedFromPii {
                output_name: origin_output_name(col).map(String::from),
                pii_category,
            },
            OriginSensitivity::DirectPii { .. }
            | OriginSensitivity::DerivedPii(_)
            | OriginSensitivity::Wildcard => col.clone(),
        })
        .collect()
}

fn merge_set_operation_lineage<'a>(
    left_ql: QueryLineage<'a>,
    right_ql: QueryLineage<'a>,
    quantifier: SetQuantifier,
) -> QueryLineage<'a> {
    let is_by_name = matches!(
        quantifier,
        SetQuantifier::ByName | SetQuantifier::AllByName | SetQuantifier::DistinctByName
    );
    if is_by_name {
        let mut wildcard_datasets = left_ql.wildcard_datasets;
        append_unique_datasets(&mut wildcard_datasets, &right_ql.wildcard_datasets);
        let columns = merge_columns_by_name(left_ql.columns, right_ql.columns);
        return QueryLineage {
            columns,
            wildcard_datasets,
        };
    }

    let left_has_wild = left_ql
        .columns
        .iter()
        .any(|c| matches!(c, OutputColumnOrigin::Wildcard));
    let right_has_wild = right_ql
        .columns
        .iter()
        .any(|c| matches!(c, OutputColumnOrigin::Wildcard));

    let left_cols = if left_has_wild && !right_has_wild {
        try_expand_wildcard_datasets(
            &left_ql.columns,
            &left_ql.wildcard_datasets,
            right_ql.columns.len(),
        )
        .unwrap_or(left_ql.columns)
    } else {
        left_ql.columns
    };
    let right_cols = if right_has_wild && !left_has_wild {
        try_expand_wildcard_datasets(
            &right_ql.columns,
            &right_ql.wildcard_datasets,
            left_cols.len(),
        )
        .unwrap_or(right_ql.columns)
    } else {
        right_ql.columns
    };

    let right_wild_pii = first_pii_in_datasets(&right_ql.wildcard_datasets);
    let left_wild_pii = first_pii_in_datasets(&left_ql.wildcard_datasets);
    let mut wildcard_datasets = left_ql.wildcard_datasets;
    append_unique_datasets(&mut wildcard_datasets, &right_ql.wildcard_datasets);

    let columns = merge_columns_positional(&left_cols, &right_cols, left_wild_pii, right_wild_pii);
    QueryLineage {
        columns,
        wildcard_datasets,
    }
}

fn merge_columns_positional(
    left: &[OutputColumnOrigin],
    right: &[OutputColumnOrigin],
    left_wild_pii: Option<PiiCategory>,
    right_wild_pii: Option<PiiCategory>,
) -> Vec<OutputColumnOrigin> {
    let left_wild = left
        .iter()
        .any(|c| matches!(c, OutputColumnOrigin::Wildcard));
    let right_wild = right
        .iter()
        .any(|c| matches!(c, OutputColumnOrigin::Wildcard));

    if left_wild || right_wild {
        let same_shape = left.len() == right.len()
            && left.iter().zip(right).all(|(l, r)| {
                matches!(l, OutputColumnOrigin::Wildcard)
                    == matches!(r, OutputColumnOrigin::Wildcard)
            });
        if same_shape {
            return left
                .iter()
                .zip(right)
                .map(|(l, r)| merge_two_origins(l, r))
                .collect();
        }
        if !left_wild {
            if let Some(pii) = right_wild_pii {
                return upgrade_clean_columns_to_derived(left, pii);
            }
            return left.to_vec();
        }
        if !right_wild && left_wild_pii.is_none() {
            return right.to_vec();
        }
        return vec![OutputColumnOrigin::Wildcard];
    }

    let max_len = left.len().max(right.len());
    let mut out = Vec::with_capacity(max_len);
    for idx in 0..max_len {
        match (left.get(idx), right.get(idx)) {
            (Some(left_item), Some(right_item)) => {
                out.push(merge_two_origins(left_item, right_item));
            }
            (Some(left_item), None) => out.push(left_item.clone()),
            (None, Some(right_item)) => out.push(right_item.clone()),
            (None, None) => {}
        }
    }
    out
}

fn merge_columns_by_name(
    left: Vec<OutputColumnOrigin>,
    right: Vec<OutputColumnOrigin>,
) -> Vec<OutputColumnOrigin> {
    let mut out = left;
    for right_item in right {
        if matches!(right_item, OutputColumnOrigin::Wildcard) {
            if !out
                .iter()
                .any(|c| matches!(c, OutputColumnOrigin::Wildcard))
            {
                out.push(OutputColumnOrigin::Wildcard);
            }
            continue;
        }
        if let Some(right_name) = origin_output_name(&right_item)
            && let Some(existing) = out.iter_mut().find(|left_item| {
                origin_output_name(left_item).is_some_and(|ln| ln.eq_ignore_ascii_case(right_name))
            })
        {
            *existing = merge_two_origins(existing, &right_item);
        } else {
            out.push(right_item);
        }
    }
    out
}

fn classify_projected_expr<'a>(
    expr: &Expr,
    explicit_alias: Option<String>,
    relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
) -> OutputColumnOrigin {
    let mut unwrapped = expr;
    while let Expr::Nested(inner) = unwrapped {
        unwrapped = inner;
    }

    match unwrapped {
        Expr::Identifier(ident) => {
            let col_lower = ident.value.to_ascii_lowercase();
            match resolve_unqualified(&col_lower, relations, ctx.outer_scopes) {
                Some(ResolvedColumn::Direct {
                    dataset_id,
                    column_name,
                    pii_category,
                }) => OutputColumnOrigin::Direct {
                    dataset_id,
                    column_name,
                    output_name: explicit_alias.unwrap_or_else(|| ident.value.clone()),
                    pii_category,
                },
                Some(ResolvedColumn::DerivedPii { pii_category }) => {
                    OutputColumnOrigin::DerivedFromPii {
                        output_name: explicit_alias.or_else(|| Some(ident.value.clone())),
                        pii_category,
                    }
                }
                Some(ResolvedColumn::NonSensitive) => OutputColumnOrigin::NonSensitive {
                    output_name: explicit_alias.or_else(|| Some(ident.value.clone())),
                },
                None => {
                    let out_name = explicit_alias.or_else(|| Some(ident.value.clone()));
                    if let Some(pii) = relation_any_pii(&col_lower, relations, ctx.outer_scopes) {
                        OutputColumnOrigin::DerivedFromPii {
                            output_name: out_name,
                            pii_category: pii,
                        }
                    } else {
                        OutputColumnOrigin::NonSensitive {
                            output_name: out_name,
                        }
                    }
                }
            }
        }
        Expr::CompoundIdentifier(parts) => {
            if let [tbl_ident, col_ident] = parts.as_slice() {
                let tbl_lower = tbl_ident.value.to_ascii_lowercase();
                let col_lower = col_ident.value.to_ascii_lowercase();
                match resolve_qualified(&tbl_lower, &col_lower, relations, ctx.outer_scopes) {
                    Some(ResolvedColumn::Direct {
                        dataset_id,
                        column_name,
                        pii_category,
                    }) => {
                        return OutputColumnOrigin::Direct {
                            dataset_id,
                            column_name,
                            output_name: explicit_alias.unwrap_or_else(|| col_ident.value.clone()),
                            pii_category,
                        };
                    }
                    Some(ResolvedColumn::DerivedPii { pii_category }) => {
                        return OutputColumnOrigin::DerivedFromPii {
                            output_name: explicit_alias.or_else(|| Some(col_ident.value.clone())),
                            pii_category,
                        };
                    }
                    Some(ResolvedColumn::NonSensitive) => {
                        return OutputColumnOrigin::NonSensitive {
                            output_name: explicit_alias.or_else(|| Some(col_ident.value.clone())),
                        };
                    }
                    None => {}
                }
            }
            classify_derived_or_clean_expr(expr, explicit_alias, relations, ctx)
        }
        _ => classify_derived_or_clean_expr(expr, explicit_alias, relations, ctx),
    }
}

fn classify_derived_or_clean_expr<'a>(
    expr: &Expr,
    explicit_alias: Option<String>,
    relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
) -> OutputColumnOrigin {
    if let Some(pii) = find_any_pii_in_ast(expr, relations, ctx) {
        OutputColumnOrigin::DerivedFromPii {
            output_name: explicit_alias,
            pii_category: pii,
        }
    } else {
        OutputColumnOrigin::NonSensitive {
            output_name: explicit_alias,
        }
    }
}

fn resolve_unqualified_col<'a>(
    col_lower: &str,
    base_datasets: &[&'a CatalogDatasetEntry],
) -> Option<(&'a CatalogDatasetEntry, &'a ColumnMeta)> {
    let mut found: Option<(&'a CatalogDatasetEntry, &'a ColumnMeta)> = None;
    for ds in base_datasets {
        if let Some(meta) = ds.columns.get(col_lower) {
            if meta.pii_category != PiiCategory::None {
                return Some((*ds, meta));
            }
            if found.is_none() {
                found = Some((*ds, meta));
            }
        }
    }
    found
}

fn resolve_in_derived(col_lower: &str, ql: &QueryLineage<'_>) -> Option<ResolvedColumn> {
    let mut fallback: Option<ResolvedColumn> = None;
    for item in &ql.columns {
        let Some(name) = origin_output_name(item) else {
            continue;
        };
        if !name.eq_ignore_ascii_case(col_lower) {
            continue;
        }
        match item {
            OutputColumnOrigin::Direct {
                dataset_id,
                column_name,
                pii_category,
                ..
            } => {
                let candidate = ResolvedColumn::Direct {
                    dataset_id: *dataset_id,
                    column_name: column_name.clone(),
                    pii_category: *pii_category,
                };
                if *pii_category != PiiCategory::None {
                    return Some(candidate);
                }
                if fallback.is_none() {
                    fallback = Some(candidate);
                }
            }
            OutputColumnOrigin::DerivedFromPii { pii_category, .. } => {
                if *pii_category != PiiCategory::None {
                    return Some(ResolvedColumn::DerivedPii {
                        pii_category: *pii_category,
                    });
                }
                if fallback.is_none() {
                    fallback = Some(ResolvedColumn::NonSensitive);
                }
            }
            OutputColumnOrigin::NonSensitive { .. } => {
                if fallback.is_none() {
                    fallback = Some(ResolvedColumn::NonSensitive);
                }
            }
            OutputColumnOrigin::Wildcard => {}
        }
    }

    if let Some((ds, meta)) = resolve_unqualified_col(col_lower, &ql.wildcard_datasets) {
        let wild_col = ResolvedColumn::Direct {
            dataset_id: ds.id,
            column_name: meta.column_name.clone(),
            pii_category: meta.pii_category,
        };
        if meta.pii_category != PiiCategory::None || fallback.is_none() {
            return Some(wild_col);
        }
    }

    fallback
}

fn resolve_in_relation(col_lower: &str, rel: &RelationEntry<'_>) -> Option<ResolvedColumn> {
    match &rel.kind {
        RelationKind::Dataset(ds) => ds
            .columns
            .get(col_lower)
            .map(|meta| ResolvedColumn::Direct {
                dataset_id: ds.id,
                column_name: meta.column_name.clone(),
                pii_category: meta.pii_category,
            }),
        RelationKind::Derived(ql) => resolve_in_derived(col_lower, ql),
        RelationKind::Opaque => None,
    }
}

fn resolve_unqualified_in_slice(
    col_lower: &str,
    relations: &[RelationEntry<'_>],
) -> Option<ResolvedColumn> {
    let mut fallback: Option<ResolvedColumn> = None;
    for rel in relations {
        if let Some(resolved) = resolve_in_relation(col_lower, rel) {
            if resolved.pii_category().is_some() {
                return Some(resolved);
            }
            if fallback.is_none() {
                fallback = Some(resolved);
            }
        }
    }
    fallback
}

fn resolve_unqualified(
    col_lower: &str,
    relations: &[RelationEntry<'_>],
    outer_scopes: &[Vec<RelationEntry<'_>>],
) -> Option<ResolvedColumn> {
    if let Some(local) = resolve_unqualified_in_slice(col_lower, relations) {
        return Some(local);
    }
    for scope in outer_scopes.iter().rev() {
        if let Some(outer) = resolve_unqualified_in_slice(col_lower, scope) {
            return Some(outer);
        }
    }
    None
}

fn resolve_qualified_in_slice(
    tbl_lower: &str,
    col_lower: &str,
    relations: &[RelationEntry<'_>],
) -> Option<(bool, Option<ResolvedColumn>)> {
    let mut matched_table = false;
    let mut fallback: Option<ResolvedColumn> = None;
    for rel in relations {
        if rel.alias.as_deref() == Some(tbl_lower) {
            matched_table = true;
            if let Some(resolved) = resolve_in_relation(col_lower, rel) {
                if resolved.pii_category().is_some() {
                    return Some((true, Some(resolved)));
                }
                if fallback.is_none() {
                    fallback = Some(resolved);
                }
            }
        }
    }
    matched_table.then_some((true, fallback))
}

fn resolve_qualified(
    tbl_lower: &str,
    col_lower: &str,
    relations: &[RelationEntry<'_>],
    outer_scopes: &[Vec<RelationEntry<'_>>],
) -> Option<ResolvedColumn> {
    if let Some((_, resolved)) = resolve_qualified_in_slice(tbl_lower, col_lower, relations) {
        return resolved;
    }
    for scope in outer_scopes.iter().rev() {
        if let Some((_, resolved)) = resolve_qualified_in_slice(tbl_lower, col_lower, scope) {
            return resolved;
        }
    }
    None
}

fn query_lineage_any_pii(ql: &QueryLineage<'_>) -> Option<PiiCategory> {
    for col in &ql.columns {
        match col {
            OutputColumnOrigin::Direct { pii_category, .. }
            | OutputColumnOrigin::DerivedFromPii { pii_category, .. }
                if *pii_category != PiiCategory::None =>
            {
                return Some(*pii_category);
            }
            _ => {}
        }
    }
    first_pii_in_datasets(&ql.wildcard_datasets)
}

fn relation_entry_any_pii(rel: &RelationEntry<'_>) -> Option<PiiCategory> {
    match &rel.kind {
        RelationKind::Dataset(ds) => first_pii_in_datasets(&[*ds]),
        RelationKind::Derived(ql) => query_lineage_any_pii(ql),
        RelationKind::Opaque => None,
    }
}

fn relation_any_pii(
    tbl_lower: &str,
    relations: &[RelationEntry<'_>],
    outer_scopes: &[Vec<RelationEntry<'_>>],
) -> Option<PiiCategory> {
    for rel in relations {
        if rel.alias.as_deref() == Some(tbl_lower)
            && let Some(pii) = relation_entry_any_pii(rel)
        {
            return Some(pii);
        }
    }
    for scope in outer_scopes.iter().rev() {
        for rel in scope {
            if rel.alias.as_deref() == Some(tbl_lower)
                && let Some(pii) = relation_entry_any_pii(rel)
            {
                return Some(pii);
            }
        }
    }
    None
}

struct ExprPiiFinder<'a, 'b> {
    relations: &'b [RelationEntry<'a>],
    ctx: LineageCtx<'a, 'b>,
    subquery_depth: usize,
    in_exists_head: bool,
    detected_pii: Option<PiiCategory>,
}

impl Visitor for ExprPiiFinder<'_, '_> {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if self.subquery_depth == 0 {
            let is_exists = std::mem::replace(&mut self.in_exists_head, false);
            if let Some(pii) = find_any_pii_in_subquery(query, self.relations, self.ctx, !is_exists)
            {
                self.detected_pii = Some(pii);
                return ControlFlow::Break(());
            }
        }
        self.subquery_depth = self.subquery_depth.saturating_add(1);
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        self.subquery_depth = self.subquery_depth.saturating_sub(1);
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<Self::Break> {
        if self.subquery_depth > 0 {
            return ControlFlow::Continue(());
        }
        match e {
            Expr::Exists { .. } => {
                self.in_exists_head = true;
            }
            Expr::Identifier(ident) => {
                let col_lower = ident.value.to_ascii_lowercase();
                if let Some(resolved) =
                    resolve_unqualified(&col_lower, self.relations, self.ctx.outer_scopes)
                {
                    if let Some(pii) = resolved.pii_category() {
                        self.detected_pii = Some(pii);
                        return ControlFlow::Break(());
                    }
                } else if let Some(pii) =
                    relation_any_pii(&col_lower, self.relations, self.ctx.outer_scopes)
                {
                    self.detected_pii = Some(pii);
                    return ControlFlow::Break(());
                }
            }
            Expr::CompoundIdentifier(parts) => {
                if let [tbl, col, ..] = parts.as_slice() {
                    let t_low = tbl.value.to_ascii_lowercase();
                    let c_low = col.value.to_ascii_lowercase();
                    if let Some(resolved) =
                        resolve_qualified(&t_low, &c_low, self.relations, self.ctx.outer_scopes)
                    {
                        if let Some(pii) = resolved.pii_category() {
                            self.detected_pii = Some(pii);
                            return ControlFlow::Break(());
                        }
                    } else if let Some(resolved) =
                        resolve_unqualified(&t_low, self.relations, self.ctx.outer_scopes)
                        && let Some(pii) = resolved.pii_category()
                    {
                        self.detected_pii = Some(pii);
                        return ControlFlow::Break(());
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

fn find_any_pii_in_ast<'a, N: Visit>(
    node: &N,
    relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
) -> Option<PiiCategory> {
    let mut finder = ExprPiiFinder {
        relations,
        ctx,
        subquery_depth: 0,
        in_exists_head: false,
        detected_pii: None,
    };
    let _visit_flow = node.visit(&mut finder);
    finder.detected_pii
}

fn find_any_pii_in_subquery<'a>(
    subquery: &Query,
    current_relations: &[RelationEntry<'a>],
    ctx: LineageCtx<'a, '_>,
    include_wildcard_datasets: bool,
) -> Option<PiiCategory> {
    if ctx.depth > MAX_LINEAGE_DEPTH {
        return None;
    }
    let mut sub_outer = ctx.outer_scopes.to_vec();
    if !current_relations.is_empty() {
        sub_outer.push(current_relations.to_vec());
    }
    let sub_ctx = LineageCtx {
        catalog: ctx.catalog,
        cte_scope: ctx.cte_scope,
        outer_scopes: &sub_outer,
        depth: ctx.depth.saturating_add(1),
    };
    let sub_ql = analyze_query_lineage(subquery, sub_ctx);
    for col in &sub_ql.columns {
        match col {
            OutputColumnOrigin::Direct { pii_category, .. }
            | OutputColumnOrigin::DerivedFromPii { pii_category, .. }
                if *pii_category != PiiCategory::None =>
            {
                return Some(*pii_category);
            }
            _ => {}
        }
    }
    if include_wildcard_datasets && let Some(pii) = first_pii_in_datasets(&sub_ql.wildcard_datasets)
    {
        return Some(pii);
    }
    find_any_pii_in_query_predicates(subquery, sub_ctx)
}

fn find_any_pii_in_query_predicates(query: &Query, ctx: LineageCtx<'_, '_>) -> Option<PiiCategory> {
    if ctx.depth > MAX_LINEAGE_DEPTH {
        return None;
    }
    let Some(ref with_clause) = query.with else {
        return find_any_pii_in_set_expr_predicates(&query.body, ctx);
    };
    let local_ctes = build_with_cte_scope(with_clause, ctx);
    for cte in &with_clause.cte_tables {
        let cte_ctx = LineageCtx {
            cte_scope: &local_ctes,
            depth: ctx.depth.saturating_add(1),
            ..ctx
        };
        if let Some(pii) = find_any_pii_in_query_predicates(&cte.query, cte_ctx) {
            return Some(pii);
        }
    }
    let body_ctx = LineageCtx {
        cte_scope: &local_ctes,
        ..ctx
    };
    find_any_pii_in_set_expr_predicates(&query.body, body_ctx)
}

fn find_any_pii_in_set_expr_predicates(
    set_expr: &SetExpr,
    ctx: LineageCtx<'_, '_>,
) -> Option<PiiCategory> {
    if ctx.depth > MAX_LINEAGE_DEPTH {
        return None;
    }
    match set_expr {
        SetExpr::Select(select) => {
            let relations = collect_select_relations(select, ctx);
            for pred in [&select.selection, &select.having, &select.qualify]
                .into_iter()
                .flatten()
            {
                if let Some(pii) = find_any_pii_in_ast(pred, &relations, ctx) {
                    return Some(pii);
                }
            }
            None
        }
        SetExpr::Query(inner) => {
            let next_ctx = LineageCtx {
                depth: ctx.depth.saturating_add(1),
                ..ctx
            };
            find_any_pii_in_query_predicates(inner, next_ctx)
        }
        SetExpr::SetOperation { left, right, .. } => {
            let next_ctx = LineageCtx {
                depth: ctx.depth.saturating_add(1),
                ..ctx
            };
            find_any_pii_in_set_expr_predicates(left, next_ctx)
                .or_else(|| find_any_pii_in_set_expr_predicates(right, next_ctx))
        }
        _ => None,
    }
}

fn collect_direct_columns(lineage: &[OutputColumnOrigin]) -> Vec<(Uuid, String)> {
    let mut out = Vec::new();
    for item in lineage {
        if let OutputColumnOrigin::Direct {
            dataset_id,
            column_name,
            ..
        } = item
        {
            out.push((*dataset_id, column_name.clone()));
        }
    }
    out
}

fn platform_view_column_type(view_name: &str, col_lower: &str) -> Option<DataType> {
    match (view_name, col_lower) {
        ("nodes", "id" | "type_name" | "node_key" | "title")
        | ("links", "id" | "link_type" | "src_node_id" | "dst_node_id")
        | ("metric_points" | "metric_series", "node_id" | "metric" | "value_text" | "source_tag")
        | ("metric_rollups", "node_id" | "metric" | "grain") => Some(DataType::String),
        ("nodes", "properties" | "metrics") | ("links", "properties") => Some(DataType::Json),
        ("nodes", "geo_lng" | "geo_lat")
        | ("links", "weight")
        | ("metric_points" | "metric_series", "value")
        | ("metric_rollups", "sum_v" | "min_v" | "max_v" | "last_v") => Some(DataType::Float64),
        ("nodes", "updated_at")
        | ("metric_points" | "metric_series", "ts")
        | ("metric_rollups", "bucket_start") => Some(DataType::Timestamp),
        ("metric_rollups", "cnt") => Some(DataType::Int64),
        _ => None,
    }
}

struct ExprInspector {
    forbid_subqueries: bool,
    columns: BTreeSet<String>,
    functions: Vec<String>,
}

impl Visitor for ExprInspector {
    type Break = AppError;

    fn pre_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        if self.forbid_subqueries {
            return ControlFlow::Break(AppError::bad(
                "INVALID_ARGUMENT",
                "subqueries are not allowed in metric expressions or filters",
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<Self::Break> {
        match e {
            Expr::Subquery(_) | Expr::Exists { .. } if self.forbid_subqueries => {
                return ControlFlow::Break(AppError::bad(
                    "INVALID_ARGUMENT",
                    "subqueries are not allowed in metric expressions or filters",
                ));
            }
            Expr::In {
                target: InTarget::Subquery(_) | InTarget::Table { .. },
                ..
            } if self.forbid_subqueries => {
                return ControlFlow::Break(AppError::bad(
                    "INVALID_ARGUMENT",
                    "IN subqueries are not allowed in metric expressions or filters",
                ));
            }
            Expr::Function(func) => {
                let parts = match extract_ident_parts(&func.name) {
                    Ok(p) => p,
                    Err(err) => return ControlFlow::Break(err),
                };
                let [only] = parts.as_slice() else {
                    return ControlFlow::Break(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("schema-qualified function '{}' is forbidden", func.name),
                    ));
                };
                let lower = only.value.to_ascii_lowercase();
                if is_forbidden_function_name(&lower) {
                    return ControlFlow::Break(rejected(format!(
                        "function '{lower}' is forbidden"
                    )));
                }
                self.functions.push(lower);
                if let FunctionArguments::List(ref arg_list) = func.args {
                    for arg in &arg_list.args {
                        if let FunctionArg::Named {
                            arg: FunctionArgExpr::Expr(_),
                            ..
                        }
                        | FunctionArg::ExprNamed {
                            arg: FunctionArgExpr::Expr(_),
                            ..
                        }
                        | FunctionArg::Unnamed(FunctionArgExpr::Expr(_)) = arg
                        {
                        }
                    }
                }
            }
            Expr::Identifier(ident) => {
                self.columns.insert(ident.value.to_ascii_lowercase());
            }
            Expr::CompoundIdentifier(parts) => {
                if let Some(last) = parts.last() {
                    self.columns.insert(last.value.to_ascii_lowercase());
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

fn parse_single_sql_expr(trimmed: &str) -> Result<Expr, AppError> {
    let mut stmts =
        Parser::parse_sql(&DuckDbDialect {}, &format!("SELECT {trimmed}")).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid SQL expression: {err}"))
        })?;
    if stmts.len() != 1 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "SQL expression must not contain multiple statements",
        ));
    }
    let Some(Statement::Query(q)) = stmts.pop() else {
        return Err(AppError::bad("INVALID_ARGUMENT", "expected expression"));
    };
    let SetExpr::Select(mut sel) = *q.body else {
        return Err(AppError::bad("INVALID_ARGUMENT", "expected expression"));
    };
    if !sel.from.is_empty()
        || sel.selection.is_some()
        || sel.having.is_some()
        || sel.projection.len() != 1
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "expected a single scalar/aggregate SQL expression",
        ));
    }
    match sel.projection.pop() {
        Some(SelectItem::UnnamedExpr(e)) => Ok(e),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            "SQL expression cannot contain wildcards or aliases",
        )),
    }
}

/// Extracts all column references inside an arbitrary SQL expression (used by `semantic.rs`).
///
/// Rejects subqueries (`Expr::Subquery`, `Expr::InSubquery`, `Expr::Exists`) when
/// `forbid_subqueries` is `true`.
///
/// # Errors
/// Returns `INVALID_ARGUMENT` or `SQL_REJECTED` if subqueries or forbidden functions appear.
pub fn inspect_sql_expression(
    expr_sql: &str,
    forbid_subqueries: bool,
) -> Result<(Expr, BTreeSet<String>, Vec<String>), AppError> {
    let trimmed = expr_sql.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "SQL expression cannot be empty",
        ));
    }
    let expr = parse_single_sql_expr(trimmed)?;
    let mut inspector = ExprInspector {
        forbid_subqueries,
        columns: BTreeSet::new(),
        functions: Vec::new(),
    };
    if let ControlFlow::Break(err) = expr.visit(&mut inspector) {
        return Err(err);
    }
    Ok((expr, inspector.columns, inspector.functions))
}
