//! AST-based `DuckDB` SQL security guard, identifier rewriter, parameter binder,
//! and output column lineage tracker (`05-STEP04` §04.3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{ColumnMeta, Dataset};
use axpoint_domain::enums::{DataType, PiiCategory};
use serde::{Deserialize, Serialize};
use sqlparser::ast::{
    Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments, Ident, InTarget, ObjectName,
    ObjectNamePart, Query, Select, SelectItem, SetExpr, Statement, TableAlias, TableFactor,
    TableWithJoins, Value, ValueWithSpan, Visit, VisitMut, Visitor, VisitorMut, With,
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

/// Options controlling [`guard`] execution (`05-STEP04` §04.3).
#[derive(Debug, Clone)]
pub struct GuardOptions {
    /// Maximum rows before truncation (`default = 10_000`, `cap = 100_000`).
    pub max_rows: u32,
    /// Whether `live_<conn>.<schema>.<table>` references are allowed.
    pub allow_live: bool,
    /// Named `:param` values substituted as validated SQL literals.
    pub params: BTreeMap<String, serde_json::Value>,
}

impl Default for GuardOptions {
    fn default() -> Self {
        Self {
            max_rows: DEFAULT_MAX_ROWS,
            allow_live: false,
            params: BTreeMap::new(),
        }
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
    /// Ordered lineage descriptors for the outer `SELECT` list (empty when outer query is `UNION`/`VALUES`).
    pub output_lineage: Vec<OutputColumnOrigin>,
    /// `true` if any `ax.*` platform federation view is referenced.
    pub uses_platform_views: bool,
    /// Referenced `live_<conn>` connection names (without `live_` prefix).
    pub referenced_live_connections: Vec<String>,
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
/// - Returns `AppError::BadRequest { code: "SQL_REJECTED", .. }` on any syntax, statement,
///   relation, function, parameter, or post-rewrite verification violation.
pub fn guard(
    sql: &str,
    catalog: &TenantCatalog,
    opts: &GuardOptions,
) -> Result<GuardedSql, AppError> {
    let effective_max_rows = if opts.max_rows == 0 {
        DEFAULT_MAX_ROWS
    } else {
        opts.max_rows.min(HARD_MAX_ROWS)
    };
    let mut query = parse_single_select_query(sql, &catalog.tenant_id)?;
    let output_lineage = analyze_outer_lineage(&query, catalog);

    let mut visitor = GuardVisitor::new(
        catalog,
        GuardTarget::InteractiveQuery {
            allow_live: opts.allow_live,
        },
        ParamBindMode::Literal(&opts.params),
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

struct GuardVisitor<'a> {
    catalog: &'a TenantCatalog,
    target: GuardTarget,
    param_mode: ParamBindMode<'a>,
    cte_stack: Vec<BTreeSet<String>>,
    saved_with_stack: Vec<Option<With>>,
    in_allowed_table_func_expr: usize,
    param_order: Vec<String>,
    referenced_dataset_ids: Vec<Uuid>,
    referenced_dataset_set: HashSet<Uuid>,
    referenced_dataset_names: Vec<String>,
    uses_platform_views: bool,
    referenced_live_connections: Vec<String>,
    referenced_live_set: HashSet<String>,
}

impl<'a> GuardVisitor<'a> {
    fn new(catalog: &'a TenantCatalog, target: GuardTarget, param_mode: ParamBindMode<'a>) -> Self {
        Self {
            catalog,
            target,
            param_mode,
            cte_stack: vec![BTreeSet::new()],
            saved_with_stack: Vec::new(),
            in_allowed_table_func_expr: 0,
            param_order: Vec::new(),
            referenced_dataset_ids: Vec::new(),
            referenced_dataset_set: HashSet::new(),
            referenced_dataset_names: Vec::new(),
            uses_platform_views: false,
            referenced_live_connections: Vec::new(),
            referenced_live_set: HashSet::new(),
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

    fn rewrite_table_relation(
        &mut self,
        name: &mut ObjectName,
        alias: &mut Option<TableAlias>,
    ) -> Result<(), AppError> {
        let parts = extract_ident_parts(name)?;
        match parts.as_slice() {
            [single] => {
                let raw_ident = single.value.clone();
                let lower = raw_ident.to_ascii_lowercase();
                if self.current_ctes().contains(&lower) {
                    return Ok(());
                }
                if FORBIDDEN_BARE_RELATIONS.contains(&lower.as_str())
                    || lower.starts_with("duckdb_")
                    || lower.starts_with("pragma_")
                {
                    return Err(rejected(format!("system relation '{lower}' is forbidden")));
                }
                let Some(ds) = self.catalog.dataset(&lower) else {
                    return Err(rejected(format!(
                        "unknown or unauthorized dataset '{raw_ident}'"
                    )));
                };
                let ds_clone = ds.clone();
                self.record_dataset(&ds_clone);
                if alias.is_none() {
                    *alias = Some(TableAlias {
                        explicit: true,
                        name: Ident::with_quote('"', raw_ident),
                        columns: Vec::new(),
                        at: None,
                    });
                }
                let tenant_schema = format!("tenant_{}", self.catalog.tenant_id);
                *name = ObjectName::from(vec![
                    Ident::new("lake"),
                    Ident::with_quote('"', tenant_schema),
                    Ident::with_quote('"', ds_clone.lake_table),
                ]);
                Ok(())
            }
            [first, second] => {
                let s0 = first.value.to_ascii_lowercase();
                let s1 = second.value.to_ascii_lowercase();
                if s0 == "ax" && ALLOWED_PLATFORM_VIEWS.contains(&s1.as_str()) {
                    if self.target == GuardTarget::MaterializeView {
                        return Err(rejected(
                            "platform views ax.* are not allowed in materialized derived views",
                        ));
                    }
                    self.uses_platform_views = true;
                    *name = ObjectName::from(vec![Ident::new("ax"), Ident::new(s1)]);
                    return Ok(());
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
                Ok(())
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
                } else if let Err(err) = self.rewrite_table_relation(name, alias) {
                    return break_err(err);
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
        ControlFlow::Continue(())
    }

    fn post_visit_table_factor(&mut self, tf: &mut TableFactor) -> ControlFlow<Self::Break> {
        if matches!(tf, TableFactor::TableFunction { .. }) {
            self.in_allowed_table_func_expr = self.in_allowed_table_func_expr.saturating_sub(1);
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
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        if let Expr::Value(val_span) = expr
            && let Value::Placeholder(raw) = &val_span.value
        {
            let raw_copy = raw.clone();
            match self.bind_placeholder(&raw_copy) {
                Ok(replacement) => *expr = replacement,
                Err(err) => return break_err(err),
            }
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
    if let ControlFlow::Break(err) = verify_query_recursive(query, &mut verifier) {
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

fn verify_query_recursive(
    query: &Query,
    verifier: &mut PostRewriteVerifier<'_>,
) -> ControlFlow<Box<AppError>> {
    let mut scope = verifier.current_ctes();
    if let Some(ref with_clause) = query.with {
        if with_clause.recursive {
            for cte in &with_clause.cte_tables {
                scope.insert(cte.alias.name.value.to_ascii_lowercase());
            }
        }
        for cte in &with_clause.cte_tables {
            verifier.cte_stack.push(scope.clone());
            let res = verify_query_recursive(&cte.query, verifier);
            verifier.cte_stack.pop();
            res?;
            scope.insert(cte.alias.name.value.to_ascii_lowercase());
        }
    }
    verifier.cte_stack.push(scope);
    let mut res = query.body.visit(verifier);
    if res.is_continue()
        && let Some(ref ob) = query.order_by
    {
        res = ob.visit(verifier);
    }
    if res.is_continue()
        && let Some(ref lc) = query.limit_clause
    {
        res = lc.visit(verifier);
    }
    verifier.cte_stack.pop();
    res
}

impl Visitor for PostRewriteVerifier<'_> {
    type Break = Box<AppError>;

    fn pre_visit_statement(&mut self, stmt: &Statement) -> ControlFlow<Self::Break> {
        if !matches!(stmt, Statement::Query(_)) {
            return break_err(rejected("post-rewrite check: non-query statement"));
        }
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

/// Analyzes the outer `SELECT` projection list before rewriting so `result.rs` knows whether
/// each output column is a direct PII column (`MaskRule` by category) or a derived expression
/// touching a PII column (`MaskRule::Redact`).
fn analyze_outer_lineage(query: &Query, catalog: &TenantCatalog) -> Vec<OutputColumnOrigin> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Vec::new();
    };
    let cte_names: BTreeSet<String> = query.with.as_ref().map_or_else(BTreeSet::new, |w| {
        w.cte_tables
            .iter()
            .map(|c| c.alias.name.value.to_ascii_lowercase())
            .collect()
    });

    let mut alias_to_dataset: HashMap<String, &CatalogDatasetEntry> = HashMap::new();
    let mut base_datasets: Vec<&CatalogDatasetEntry> = Vec::new();
    for twj in &select.from {
        collect_from_datasets(
            &twj.relation,
            &cte_names,
            catalog,
            &mut alias_to_dataset,
            &mut base_datasets,
        );
        for join in &twj.joins {
            collect_from_datasets(
                &join.relation,
                &cte_names,
                catalog,
                &mut alias_to_dataset,
                &mut base_datasets,
            );
        }
    }

    let mut out = Vec::with_capacity(select.projection.len());
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _) => {
                out.push(OutputColumnOrigin::Wildcard);
            }
            SelectItem::UnnamedExpr(expr) => {
                out.push(classify_projected_expr(
                    expr,
                    None,
                    &alias_to_dataset,
                    &base_datasets,
                ));
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                out.push(classify_projected_expr(
                    expr,
                    Some(alias.value.clone()),
                    &alias_to_dataset,
                    &base_datasets,
                ));
            }
            SelectItem::ExprWithAliases { expr, aliases } => {
                let first_alias = aliases.first().map(|a| a.value.clone());
                out.push(classify_projected_expr(
                    expr,
                    first_alias,
                    &alias_to_dataset,
                    &base_datasets,
                ));
            }
        }
    }
    out
}

fn collect_from_datasets<'a>(
    tf: &TableFactor,
    cte_names: &BTreeSet<String>,
    catalog: &'a TenantCatalog,
    alias_map: &mut HashMap<String, &'a CatalogDatasetEntry>,
    base_list: &mut Vec<&'a CatalogDatasetEntry>,
) {
    match tf {
        TableFactor::Table {
            name,
            alias,
            args: None,
            ..
        } => {
            if let [ObjectNamePart::Identifier(ident)] = name.0.as_slice() {
                let lower = ident.value.to_ascii_lowercase();
                if !cte_names.contains(&lower)
                    && let Some(ds) = catalog.dataset(&lower)
                {
                    let key = alias
                        .as_ref()
                        .map_or(lower, |a| a.name.value.to_ascii_lowercase());
                    alias_map.insert(key, ds);
                    base_list.push(ds);
                }
            }
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => {
            collect_table_with_joins(table_with_joins, cte_names, catalog, alias_map, base_list);
        }
        _ => {}
    }
}

fn collect_table_with_joins<'a>(
    twj: &TableWithJoins,
    cte_names: &BTreeSet<String>,
    catalog: &'a TenantCatalog,
    alias_map: &mut HashMap<String, &'a CatalogDatasetEntry>,
    base_list: &mut Vec<&'a CatalogDatasetEntry>,
) {
    collect_from_datasets(&twj.relation, cte_names, catalog, alias_map, base_list);
    for join in &twj.joins {
        collect_from_datasets(&join.relation, cte_names, catalog, alias_map, base_list);
    }
}

fn classify_projected_expr(
    expr: &Expr,
    explicit_alias: Option<String>,
    alias_to_dataset: &HashMap<String, &CatalogDatasetEntry>,
    base_datasets: &[&CatalogDatasetEntry],
) -> OutputColumnOrigin {
    match expr {
        Expr::Identifier(ident) => {
            let col_lower = ident.value.to_ascii_lowercase();
            if let Some((ds, meta)) = resolve_unqualified_col(&col_lower, base_datasets) {
                return OutputColumnOrigin::Direct {
                    dataset_id: ds.id,
                    column_name: meta.column_name.clone(),
                    output_name: explicit_alias.unwrap_or_else(|| ident.value.clone()),
                    pii_category: meta.pii_category,
                };
            }
            OutputColumnOrigin::NonSensitive {
                output_name: explicit_alias.or_else(|| Some(ident.value.clone())),
            }
        }
        Expr::CompoundIdentifier(parts) => {
            if let [tbl_ident, col_ident] = parts.as_slice() {
                let tbl_lower = tbl_ident.value.to_ascii_lowercase();
                let col_lower = col_ident.value.to_ascii_lowercase();
                if let Some(ds) = alias_to_dataset.get(&tbl_lower)
                    && let Some(meta) = ds.columns.get(&col_lower)
                {
                    return OutputColumnOrigin::Direct {
                        dataset_id: ds.id,
                        column_name: meta.column_name.clone(),
                        output_name: explicit_alias.unwrap_or_else(|| col_ident.value.clone()),
                        pii_category: meta.pii_category,
                    };
                }
            }
            if let Some(pii) = find_any_pii_in_expr(expr, alias_to_dataset, base_datasets) {
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
        _ => {
            if let Some(pii) = find_any_pii_in_expr(expr, alias_to_dataset, base_datasets) {
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

fn find_any_pii_in_expr(
    expr: &Expr,
    alias_to_dataset: &HashMap<String, &CatalogDatasetEntry>,
    base_datasets: &[&CatalogDatasetEntry],
) -> Option<PiiCategory> {
    struct ExprColCollector<'a> {
        alias_to_dataset: &'a HashMap<String, &'a CatalogDatasetEntry>,
        base_datasets: &'a [&'a CatalogDatasetEntry],
        detected_pii: Option<PiiCategory>,
    }

    impl Visitor for ExprColCollector<'_> {
        type Break = ();

        fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<Self::Break> {
            match e {
                Expr::Identifier(ident) => {
                    let col_lower = ident.value.to_ascii_lowercase();
                    if let Some((_, meta)) = resolve_unqualified_col(&col_lower, self.base_datasets)
                        && meta.pii_category != PiiCategory::None
                    {
                        self.detected_pii = Some(meta.pii_category);
                        return ControlFlow::Break(());
                    }
                }
                Expr::CompoundIdentifier(parts) => {
                    if let [tbl, col] = parts.as_slice() {
                        let t_low = tbl.value.to_ascii_lowercase();
                        let c_low = col.value.to_ascii_lowercase();
                        if let Some(ds) = self.alias_to_dataset.get(&t_low)
                            && let Some(meta) = ds.columns.get(&c_low)
                            && meta.pii_category != PiiCategory::None
                        {
                            self.detected_pii = Some(meta.pii_category);
                            return ControlFlow::Break(());
                        }
                    }
                }
                _ => {}
            }
            ControlFlow::Continue(())
        }
    }

    let mut collector = ExprColCollector {
        alias_to_dataset,
        base_datasets,
        detected_pii: None,
    };
    let _ = expr.visit(&mut collector);
    collector.detected_pii
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
