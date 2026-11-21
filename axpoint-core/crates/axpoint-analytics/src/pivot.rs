//! OLAP cross-tabulation (`GROUPING SETS`) SQL compiler and matrix builder (`05-STEP04` §04.4).

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use arrow::array::RecordBatch;
use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    FieldRef, FilterOp, FilterSpec, PivotResult, PivotRowKind, PivotSpec, QueryColumn, QueryResult,
    SortDir, ValueSpec,
};
use axpoint_domain::enums::{AggFn, DataType, PiiCategory, RollupGrain};
use axpoint_domain::guard::PolicyContext;
use serde::{Deserialize, Serialize};

use crate::result::{ResultSerializeOptions, batches_to_query_result, mask_pii_str};
use crate::session::TenantDb;
use crate::sql_guard::{GuardOptions, GuardedSql, HARD_MAX_ROWS, TenantCatalog, guard};

/// Maximum row dimensions allowed in a [`PivotSpec`].
pub const MAX_PIVOT_ROW_DIMS: usize = 5;

/// Maximum column dimensions allowed in a [`PivotSpec`].
pub const MAX_PIVOT_COL_DIMS: usize = 3;

/// Maximum value measures allowed in a [`PivotSpec`].
pub const MAX_PIVOT_VALUES: usize = 10;

/// Default matrix row limit when `row_limit == 0`.
pub const DEFAULT_PIVOT_ROW_LIMIT: u32 = 1_000;

/// Hard upper bound on matrix rows returned by `pivot`.
pub const MAX_PIVOT_ROW_LIMIT: u32 = 10_000;

/// Default distinct column-combination limit when `column_limit == 0`.
pub const DEFAULT_PIVOT_COLUMN_LIMIT: u32 = 50;

/// Hard upper bound on distinct column combinations before truncation / `"其他"` folding.
pub const MAX_PIVOT_COLUMN_LIMIT: u32 = 200;

const ALLOWED_PLATFORM_VIEWS: &[&str] = &[
    "nodes",
    "links",
    "metric_points",
    "metric_series",
    "metric_rollups",
];

/// Execution options for [`execute_pivot`] and [`execute_pivot_conn`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PivotExecuteOptions {
    /// Optional query execution timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Whether unmasked PII values are requested.
    pub reveal_pii: bool,
    /// Whether the caller holds `Permission::PiiRawRead`.
    pub has_pii_raw_read: bool,
    /// Optional compiled `AxGuard` row/column security context (`11-STEP13` §13.6.3).
    #[serde(default)]
    pub policy: Option<PolicyContext>,
}

/// Alias for [`PivotExecuteOptions`].
pub type PivotRunOptions = PivotExecuteOptions;

impl PivotExecuteOptions {
    /// Builder helper attaching an optional [`PolicyContext`].
    #[must_use]
    pub fn with_policy(mut self, policy: Option<PolicyContext>) -> Self {
        self.policy = policy;
        self
    }
}

#[derive(Debug, Clone)]
struct ResolvedDim {
    field: String,
    label: String,
    sql_expr: String,
    alias: String,
    pii_category: PiiCategory,
}

#[derive(Debug, Clone)]
struct ResolvedMeasure {
    agg: AggFn,
    label: String,
    sql_expr: String,
    alias: String,
    format: Option<String>,
}

/// Validated and guarded SQL plan compiled from a [`PivotSpec`].
#[derive(Debug, Clone)]
pub struct CompiledPivot {
    /// Pre-guard SQL query generated from the [`PivotSpec`].
    pub raw_sql: String,
    /// AST-validated and rewritten SQL ready for `DuckDB` execution.
    pub guarded: GuardedSql,
    /// Effective matrix row cap (`1..=10_000`).
    pub effective_row_limit: usize,
    /// Effective distinct column combination cap (`1..=200`).
    pub effective_column_limit: usize,
    /// Whether `subtotals` was requested.
    pub subtotals: bool,
    /// Whether `grand_total` was requested.
    pub grand_total: bool,
    row_dims: Vec<ResolvedDim>,
    col_dims: Vec<ResolvedDim>,
    measures: Vec<ResolvedMeasure>,
}

/// Maps a [`RollupGrain`] to its lowercase `DuckDB` `date_trunc` unit string.
#[must_use]
pub(crate) const fn rollup_grain_sql(grain: RollupGrain) -> &'static str {
    match grain {
        RollupGrain::Minute => "minute",
        RollupGrain::Hour => "hour",
        RollupGrain::Day => "day",
        RollupGrain::Month => "month",
    }
}

/// Escapes a validated SQL identifier inside double quotes (`"foo""bar"`).
#[must_use]
pub(crate) fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Compiles a structured [`FilterSpec`] slice into `WHERE` predicates and binds all values into
/// `params` as `:__f<idx>` placeholders so `sql_guard` safely converts them to AST literals.
///
/// # Errors
/// Returns `INVALID_ARGUMENT` if any column is unknown or filter value shape is invalid for `op`.
pub(crate) fn compile_filters_with_resolver<F>(
    filters: &[FilterSpec],
    param_prefix: &str,
    mut resolve_col: F,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Result<Vec<String>, AppError>
where
    F: FnMut(&str) -> Result<String, AppError>,
{
    let mut clauses = Vec::with_capacity(filters.len());
    for (idx, flt) in filters.iter().enumerate() {
        let canonical = resolve_col(&flt.field)?;
        let col_q = quote_ident(&canonical);
        let clause = compile_single_filter(&col_q, flt.op, &flt.value, param_prefix, idx, params)?;
        clauses.push(clause);
    }
    Ok(clauses)
}

fn compile_single_filter(
    col_q: &str,
    op: FilterOp,
    val: &serde_json::Value,
    prefix: &str,
    idx: usize,
    params: &mut BTreeMap<String, serde_json::Value>,
) -> Result<String, AppError> {
    match op {
        FilterOp::IsNull => Ok(format!("{col_q} IS NULL")),
        FilterOp::IsNotNull => Ok(format!("{col_q} IS NOT NULL")),
        FilterOp::Eq
        | FilterOp::Ne
        | FilterOp::Gt
        | FilterOp::Gte
        | FilterOp::Lt
        | FilterOp::Lte => {
            ensure_scalar_non_null(val, op)?;
            let pname = format!("{prefix}{idx}");
            params.insert(pname.clone(), val.clone());
            let sql_op = match op {
                FilterOp::Ne => "<>",
                FilterOp::Gt => ">",
                FilterOp::Gte => ">=",
                FilterOp::Lt => "<",
                FilterOp::Lte => "<=",
                _ => "=",
            };
            Ok(format!("{col_q} {sql_op} :{pname}"))
        }
        FilterOp::Like => {
            if !val.is_string() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "LIKE filter requires a string value",
                ));
            }
            let pname = format!("{prefix}{idx}");
            params.insert(pname.clone(), val.clone());
            Ok(format!("{col_q} LIKE :{pname}"))
        }
        FilterOp::Between => {
            let serde_json::Value::Array(arr) = val else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "BETWEEN filter requires a 2-element array [low, high]",
                ));
            };
            let (Some(lo), Some(hi)) = (arr.first(), arr.get(1)) else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "BETWEEN filter requires exactly 2 bounds",
                ));
            };
            if arr.len() != 2 {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "BETWEEN filter requires exactly 2 bounds",
                ));
            }
            ensure_scalar_non_null(lo, op)?;
            ensure_scalar_non_null(hi, op)?;
            let p_lo = format!("{prefix}{idx}_lo");
            let p_hi = format!("{prefix}{idx}_hi");
            params.insert(p_lo.clone(), lo.clone());
            params.insert(p_hi.clone(), hi.clone());
            Ok(format!("{col_q} BETWEEN :{p_lo} AND :{p_hi}"))
        }
        FilterOp::In | FilterOp::NotIn => {
            let serde_json::Value::Array(arr) = val else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "IN / NOT_IN filter requires a non-empty array",
                ));
            };
            if arr.is_empty() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "IN / NOT_IN filter array cannot be empty",
                ));
            }
            let mut placeholders = Vec::with_capacity(arr.len());
            for (elem_idx, item) in arr.iter().enumerate() {
                ensure_scalar_non_null(item, op)?;
                let pname = format!("{prefix}{idx}_{elem_idx}");
                params.insert(pname.clone(), item.clone());
                placeholders.push(format!(":{pname}"));
            }
            let kw = if op == FilterOp::In { "IN" } else { "NOT IN" };
            Ok(format!("{col_q} {kw} ({})", placeholders.join(", ")))
        }
    }
}

fn ensure_scalar_non_null(val: &serde_json::Value, op: FilterOp) -> Result<(), AppError> {
    if val.is_null() || val.is_array() || val.is_object() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("filter operator {op:?} requires a non-null scalar value"),
        ));
    }
    Ok(())
}

/// Validates a [`PivotSpec`] against `catalog` and compiles it into a guarded [`CompiledPivot`].
///
/// # Errors
/// - Returns `AppError::NotFound("DATASET_NOT_FOUND")` if `spec.dataset` does not exist.
/// - Returns `AppError::BadRequest` (`INVALID_ARGUMENT` or `SQL_REJECTED`) if dimension/measure
///   bounds, column names, filter operands, or SQL guard checks fail.
pub fn compile_pivot(spec: &PivotSpec, catalog: &TenantCatalog) -> Result<CompiledPivot, AppError> {
    compile_pivot_with_policy(spec, catalog, None)
}

/// Validates a [`PivotSpec`] against `catalog` and compiles it into a guarded [`CompiledPivot`]
/// under an optional `AxGuard` [`PolicyContext`] (`11-STEP13` §13.6.3).
///
/// # Errors
/// Returns `POLICY_DENIED`, `DATASET_NOT_FOUND`, `INVALID_ARGUMENT`, or `SQL_REJECTED`.
pub fn compile_pivot_with_policy(
    spec: &PivotSpec,
    catalog: &TenantCatalog,
    policy: Option<&PolicyContext>,
) -> Result<CompiledPivot, AppError> {
    validate_pivot_bounds(spec)?;

    let effective_row_limit_u32 = if spec.row_limit == 0 {
        DEFAULT_PIVOT_ROW_LIMIT
    } else {
        spec.row_limit.min(MAX_PIVOT_ROW_LIMIT)
    };
    let effective_col_limit_u32 = if spec.column_limit == 0 {
        DEFAULT_PIVOT_COLUMN_LIMIT
    } else {
        spec.column_limit.min(MAX_PIVOT_COLUMN_LIMIT)
    };
    let effective_row_limit = usize::try_from(effective_row_limit_u32).unwrap_or(1_000);
    let effective_column_limit = usize::try_from(effective_col_limit_u32).unwrap_or(50);

    let relation_sql = resolve_pivot_relation(&spec.dataset, catalog)?;
    let (row_dims, col_dims) = resolve_pivot_axes(spec, catalog)?;
    let measures = resolve_measures(&spec.dataset, &spec.values, catalog)?;

    let mut params = BTreeMap::new();
    let where_clauses = compile_filters_with_resolver(
        &spec.filters,
        "__pf",
        |f| {
            let (canon, _, _) = resolve_dataset_column(&spec.dataset, f, catalog)?;
            Ok(canon)
        },
        &mut params,
    )?;

    let raw_sql = build_pivot_sql(
        &relation_sql,
        &row_dims,
        &col_dims,
        &measures,
        &where_clauses,
        spec,
    )?;

    let sql_max_rows = if col_dims.is_empty() {
        effective_row_limit_u32
            .saturating_mul(8)
            .clamp(DEFAULT_PIVOT_ROW_LIMIT, HARD_MAX_ROWS)
    } else {
        HARD_MAX_ROWS
    };
    let guarded = guard(
        &raw_sql,
        catalog,
        &GuardOptions {
            max_rows: sql_max_rows,
            allow_live: false,
            params,
            policy: policy.cloned(),
        },
    )?;

    Ok(CompiledPivot {
        raw_sql,
        guarded,
        effective_row_limit,
        effective_column_limit,
        subtotals: spec.subtotals,
        grand_total: spec.grand_total,
        row_dims,
        col_dims,
        measures,
    })
}

fn resolve_pivot_axes(
    spec: &PivotSpec,
    catalog: &TenantCatalog,
) -> Result<(Vec<ResolvedDim>, Vec<ResolvedDim>), AppError> {
    let tb_resolved = if let Some(ref tb) = spec.time_bucket {
        let (canon, dt, _pii) = resolve_dataset_column(&spec.dataset, &tb.field, catalog)?;
        if !matches!(dt, DataType::Date | DataType::Timestamp) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "time_bucket field '{}' must have DATE or TIMESTAMP type, got {dt}",
                    tb.field
                ),
            ));
        }
        Some((canon, tb.grain))
    } else {
        None
    };

    let mut tb_matched = false;
    let mut row_dims = resolve_dim_list(
        &spec.dataset,
        &spec.rows,
        "__r",
        tb_resolved.as_ref(),
        &mut tb_matched,
        catalog,
    )?;
    let col_dims = resolve_dim_list(
        &spec.dataset,
        &spec.columns,
        "__c",
        tb_resolved.as_ref(),
        &mut tb_matched,
        catalog,
    )?;

    if let Some((ref tb_col, tb_grain)) = tb_resolved
        && !tb_matched
    {
        if row_dims.len() >= MAX_PIVOT_ROW_DIMS {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "adding unreferenced time_bucket exceeds 5 row dimensions",
            ));
        }
        let expr = format!(
            "date_trunc('{}', CAST({} AS TIMESTAMP))",
            rollup_grain_sql(tb_grain),
            quote_ident(tb_col)
        );
        row_dims.insert(
            0,
            ResolvedDim {
                field: tb_col.clone(),
                label: tb_col.clone(),
                sql_expr: expr,
                alias: String::new(),
                pii_category: PiiCategory::None,
            },
        );
        for (idx, d) in row_dims.iter_mut().enumerate() {
            d.alias = format!("__r{idx}");
        }
    }
    Ok((row_dims, col_dims))
}

/// Compiles and executes `spec` against a hardened [`TenantDb`] session (`05-STEP04` §04.4, `11-STEP13` §13.6.3).
///
/// # Errors
/// Returns `PERMISSION_DENIED`, `POLICY_DENIED`, `DATASET_NOT_FOUND`, `INVALID_ARGUMENT`, `SQL_REJECTED`,
/// `QUERY_TIMEOUT`, or `Database`.
pub async fn execute_pivot(
    session: &TenantDb,
    catalog: &TenantCatalog,
    spec: &PivotSpec,
    opts: PivotExecuteOptions,
) -> Result<PivotResult, AppError> {
    let pii_reveal_allowed = opts.has_pii_raw_read
        || opts
            .policy
            .as_ref()
            .is_some_and(|pol| pol.reveal_pii_authorized);
    if opts.reveal_pii && !pii_reveal_allowed {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let compiled = compile_pivot_with_policy(spec, catalog, opts.policy.as_ref())?;
    let out = session
        .execute_guarded(
            &compiled.guarded,
            catalog,
            opts.timeout_ms,
            opts.reveal_pii,
            pii_reveal_allowed,
        )
        .await?;
    let revealed_pii = opts.reveal_pii && pii_reveal_allowed;
    Ok(materialize_pivot_matrix(
        &compiled,
        &out.result,
        revealed_pii,
    ))
}

/// Synchronously compiles and executes `spec` against an open `duckdb::Connection`
/// (with `lake."tenant_<id>"` attached), enforcing `sql_guard` and dynamic PII masking.
///
/// # Errors
/// Returns `PERMISSION_DENIED`, `POLICY_DENIED`, `DATASET_NOT_FOUND`, `INVALID_ARGUMENT`, `SQL_REJECTED`, or `Database`.
pub fn execute_pivot_conn(
    conn: &duckdb::Connection,
    catalog: &TenantCatalog,
    spec: &PivotSpec,
    opts: PivotExecuteOptions,
) -> Result<PivotResult, AppError> {
    let PivotExecuteOptions {
        reveal_pii,
        has_pii_raw_read,
        policy,
        ..
    } = opts;
    let pii_reveal_allowed =
        has_pii_raw_read || policy.as_ref().is_some_and(|pol| pol.reveal_pii_authorized);
    if reveal_pii && !pii_reveal_allowed {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let compiled = compile_pivot_with_policy(spec, catalog, policy.as_ref())?;
    let started = Instant::now();
    let mut stmt = conn
        .prepare(&compiled.guarded.sql)
        .map_err(|err| AppError::Database(format!("pivot prepare failed: {err}")))?;
    let stream = stmt
        .query_arrow([])
        .map_err(|err| AppError::Database(format!("pivot execution failed: {err}")))?;
    let schema = stream.get_schema();
    let batches: Vec<RecordBatch> = stream.collect();
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let out = batches_to_query_result(
        &schema,
        &batches,
        ResultSerializeOptions {
            effective_max_rows: compiled.guarded.effective_max_rows,
            sql: compiled.guarded.sql.clone(),
            duration_ms,
            reveal_pii,
            has_pii_raw_read: pii_reveal_allowed,
            catalog,
            guarded: &compiled.guarded,
        },
    )?;
    let revealed_pii = reveal_pii && pii_reveal_allowed;
    Ok(materialize_pivot_matrix(
        &compiled,
        &out.result,
        revealed_pii,
    ))
}

fn validate_pivot_bounds(spec: &PivotSpec) -> Result<(), AppError> {
    if spec.rows.len() > MAX_PIVOT_ROW_DIMS {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("pivot rows cannot exceed {MAX_PIVOT_ROW_DIMS} dimensions"),
        ));
    }
    if spec.columns.len() > MAX_PIVOT_COL_DIMS {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("pivot columns cannot exceed {MAX_PIVOT_COL_DIMS} dimensions"),
        ));
    }
    if spec.values.is_empty() || spec.values.len() > MAX_PIVOT_VALUES {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("pivot values must contain 1..={MAX_PIVOT_VALUES} measures"),
        ));
    }
    Ok(())
}

fn resolve_pivot_relation(dataset: &str, catalog: &TenantCatalog) -> Result<String, AppError> {
    let trimmed = dataset.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "pivot dataset is empty"));
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(view) = lower.strip_prefix("ax.") {
        if ALLOWED_PLATFORM_VIEWS.contains(&view) {
            return Ok(format!("ax.{view}"));
        }
        return Err(AppError::NotFound("DATASET_NOT_FOUND"));
    }
    let Some(entry) = catalog.dataset(trimmed) else {
        return Err(AppError::NotFound("DATASET_NOT_FOUND"));
    };
    Ok(quote_ident(&entry.name))
}

fn resolve_dataset_column(
    dataset: &str,
    field: &str,
    catalog: &TenantCatalog,
) -> Result<(String, DataType, PiiCategory), AppError> {
    let ds_trim = dataset.trim();
    let f_trim = field.trim();
    if f_trim.is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "column field is empty"));
    }
    let ds_lower = ds_trim.to_ascii_lowercase();
    if ds_lower.starts_with("ax.") {
        let Some(dt) = catalog.resolve_column_type(&ds_lower, f_trim) else {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown column '{f_trim}' on platform view '{ds_lower}'"),
            ));
        };
        return Ok((f_trim.to_ascii_lowercase(), dt, PiiCategory::None));
    }
    let Some(entry) = catalog.dataset(ds_trim) else {
        return Err(AppError::NotFound("DATASET_NOT_FOUND"));
    };
    let Some(meta) = entry.columns.get(&f_trim.to_ascii_lowercase()) else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown column '{f_trim}' in dataset '{}'", entry.name),
        ));
    };
    Ok((meta.column_name.clone(), meta.data_type, meta.pii_category))
}

fn resolve_dim_list(
    dataset: &str,
    refs: &[FieldRef],
    prefix: &str,
    tb: Option<&(String, RollupGrain)>,
    tb_matched: &mut bool,
    catalog: &TenantCatalog,
) -> Result<Vec<ResolvedDim>, AppError> {
    let mut out = Vec::with_capacity(refs.len());
    for (idx, r) in refs.iter().enumerate() {
        let (canon, _, pii) = resolve_dataset_column(dataset, &r.field, catalog)?;
        let sql_expr = if let Some((tb_col, grain)) = tb
            && canon.eq_ignore_ascii_case(tb_col)
        {
            *tb_matched = true;
            format!(
                "date_trunc('{}', CAST({} AS TIMESTAMP))",
                rollup_grain_sql(*grain),
                quote_ident(&canon)
            )
        } else {
            quote_ident(&canon)
        };
        let label = r
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(|| canon.clone(), String::from);
        out.push(ResolvedDim {
            field: canon,
            label,
            sql_expr,
            alias: format!("{prefix}{idx}"),
            pii_category: pii,
        });
    }
    Ok(out)
}

fn resolve_measures(
    dataset: &str,
    values: &[ValueSpec],
    catalog: &TenantCatalog,
) -> Result<Vec<ResolvedMeasure>, AppError> {
    let mut field_counts: HashMap<String, usize> = HashMap::new();
    for v in values {
        if v.alias.as_deref().map(str::trim).is_none_or(str::is_empty) {
            let key = v.field.trim().to_ascii_lowercase();
            let entry = field_counts.entry(key).or_insert(0);
            *entry = entry.saturating_add(1);
        }
    }

    let mut out = Vec::with_capacity(values.len());
    for (idx, v) in values.iter().enumerate() {
        let (canon, _, _) = resolve_dataset_column(dataset, &v.field, catalog)?;
        let col_q = quote_ident(&canon);
        let sql_expr = match v.agg {
            AggFn::Sum => format!("SUM({col_q})"),
            AggFn::Avg => format!("AVG({col_q})"),
            AggFn::Count => format!("COUNT({col_q})"),
            AggFn::CountDistinct => format!("COUNT(DISTINCT {col_q})"),
            AggFn::Min => format!("MIN({col_q})"),
            AggFn::Max => format!("MAX({col_q})"),
            AggFn::Median => format!("MEDIAN({col_q})"),
            AggFn::Last => format!("LAST({col_q})"),
        };
        let label =
            if let Some(alias_str) = v.alias.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                String::from(alias_str)
            } else {
                let count = field_counts
                    .get(&v.field.trim().to_ascii_lowercase())
                    .copied()
                    .unwrap_or(1);
                if count > 1 {
                    format!("{}_{}", canon, v.agg.as_str().to_ascii_lowercase())
                } else {
                    canon
                }
            };
        out.push(ResolvedMeasure {
            agg: v.agg,
            label,
            sql_expr,
            alias: format!("__m{idx}"),
            format: v.format.clone(),
        });
    }
    Ok(out)
}

fn build_pivot_sql(
    relation_sql: &str,
    row_dims: &[ResolvedDim],
    col_dims: &[ResolvedDim],
    measures: &[ResolvedMeasure],
    where_clauses: &[String],
    spec: &PivotSpec,
) -> Result<String, AppError> {
    let mut projections = Vec::new();
    for d in row_dims {
        projections.push(format!("{} AS {}", d.sql_expr, quote_ident(&d.alias)));
    }
    for c in col_dims {
        projections.push(format!("{} AS {}", c.sql_expr, quote_ident(&c.alias)));
    }
    for (idx, d) in row_dims.iter().enumerate() {
        projections.push(format!("GROUPING({}) AS \"__gr{idx}\"", d.sql_expr));
    }
    projections.push(String::from("COUNT(*) AS \"__cnt\""));
    for m in measures {
        projections.push(format!("{} AS {}", m.sql_expr, quote_ident(&m.alias)));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };

    if row_dims.is_empty() && col_dims.is_empty() {
        return Ok(format!(
            "SELECT {} FROM {relation_sql}{where_sql}",
            projections.join(", ")
        ));
    }

    let grouping_sets = build_grouping_sets(row_dims, col_dims, spec.subtotals, spec.grand_total);
    let order_by = build_order_by_clauses(row_dims, col_dims, measures, spec)?;

    Ok(format!(
        "SELECT {} FROM {relation_sql}{where_sql} GROUP BY GROUPING SETS ({}) ORDER BY {}",
        projections.join(", "),
        grouping_sets.join(", "),
        order_by.join(", ")
    ))
}

fn build_grouping_sets(
    row_dims: &[ResolvedDim],
    col_dims: &[ResolvedDim],
    subtotals: bool,
    grand_total: bool,
) -> Vec<String> {
    let n = row_dims.len();
    let format_set = |r_slice: &[ResolvedDim], c_slice: &[ResolvedDim]| -> String {
        let mut exprs: Vec<&str> = Vec::with_capacity(r_slice.len().saturating_add(c_slice.len()));
        for r in r_slice {
            exprs.push(&r.sql_expr);
        }
        for c in c_slice {
            exprs.push(&c.sql_expr);
        }
        format!("({})", exprs.join(", "))
    };

    let mut sets = vec![format_set(row_dims, col_dims)];
    if subtotals && n >= 2 {
        for prefix_len in (1..n).rev() {
            if let Some(prefix) = row_dims.get(..prefix_len) {
                sets.push(format_set(prefix, col_dims));
            }
        }
    }
    if grand_total && n >= 1 {
        sets.push(format_set(&[], col_dims));
    }
    sets
}

fn build_order_by_clauses(
    row_dims: &[ResolvedDim],
    col_dims: &[ResolvedDim],
    measures: &[ResolvedMeasure],
    spec: &PivotSpec,
) -> Result<Vec<String>, AppError> {
    let mut row_dir_overrides: HashMap<usize, SortDir> = HashMap::new();
    let mut extra_measure_sorts: Vec<String> = Vec::new();

    for srt in &spec.sort {
        let target = srt.field.trim();
        if let Some((r_idx, _)) = row_dims.iter().enumerate().find(|(_, d)| {
            d.field.eq_ignore_ascii_case(target) || d.label.eq_ignore_ascii_case(target)
        }) {
            row_dir_overrides.insert(r_idx, srt.dir);
            continue;
        }
        if let Some(m) = measures
            .iter()
            .find(|m| m.label.eq_ignore_ascii_case(target))
        {
            if col_dims.is_empty() {
                let dir_kw = match srt.dir {
                    SortDir::Asc => "ASC NULLS LAST",
                    SortDir::Desc => "DESC NULLS LAST",
                };
                extra_measure_sorts.push(format!("{} {dir_kw}", quote_ident(&m.alias)));
            }
            continue;
        }
        if col_dims
            .iter()
            .any(|c| c.field.eq_ignore_ascii_case(target) || c.label.eq_ignore_ascii_case(target))
        {
            continue;
        }
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown sort field '{target}' in pivot spec"),
        ));
    }

    let mut order_items = Vec::new();
    if !extra_measure_sorts.is_empty() && !spec.subtotals {
        // Keep grand total at the very bottom when sorting detail rows by a measure.
        if !row_dims.is_empty() {
            order_items.push(String::from("\"__gr0\" ASC"));
        }
        order_items.extend(extra_measure_sorts);
    }
    for (idx, d) in row_dims.iter().enumerate() {
        order_items.push(format!("\"__gr{idx}\" ASC"));
        let dir_kw = match row_dir_overrides.get(&idx).copied().unwrap_or(SortDir::Asc) {
            SortDir::Asc => "ASC NULLS LAST",
            SortDir::Desc => "DESC NULLS LAST",
        };
        order_items.push(format!("{} {dir_kw}", quote_ident(&d.alias)));
    }
    for c in col_dims {
        order_items.push(format!("{} ASC NULLS LAST", quote_ident(&c.alias)));
    }
    Ok(order_items)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MatrixRowIdentity {
    grouping_bits: Vec<bool>,
    dim_values: Vec<String>,
}

type IndexedCellMap = HashMap<(usize, Vec<String>), Vec<serde_json::Value>>;

struct CollectedPivotCells {
    ordered_rows: Vec<(MatrixRowIdentity, Vec<String>, PivotRowKind)>,
    col_freq: BTreeMap<Vec<String>, u64>,
    cell_lookup: IndexedCellMap,
}

fn materialize_pivot_matrix(
    compiled: &CompiledPivot,
    qr: &QueryResult,
    revealed_pii: bool,
) -> PivotResult {
    if compiled.row_dims.is_empty() && compiled.col_dims.is_empty() {
        return materialize_scalar_pivot(compiled, qr);
    }

    let CollectedPivotCells {
        ordered_rows,
        col_freq,
        cell_lookup,
    } = collect_pivot_cells(compiled, qr, revealed_pii);
    let total_rows = ordered_rows.len().min(compiled.effective_row_limit);
    let active_rows = ordered_rows.get(..total_rows).unwrap_or(&[]);

    if compiled.col_dims.is_empty() {
        return build_no_coldim_result(compiled, qr, active_rows, &cell_lookup);
    }

    let (kept_cols, overflow_cols, columns_truncated) =
        select_pivot_columns(&col_freq, compiled.effective_column_limit);
    let all_mergeable = compiled.measures.iter().all(|m| is_mergeable_agg(m.agg));
    let emit_other_col = columns_truncated && all_mergeable && !overflow_cols.is_empty();

    let column_headers = build_matrix_column_headers(compiled, &kept_cols, emit_other_col);
    let mut row_headers = Vec::with_capacity(active_rows.len());
    let mut row_kinds = Vec::with_capacity(active_rows.len());
    let mut cells = Vec::with_capacity(active_rows.len());

    for (r_idx, (_, r_hdr, r_kind)) in active_rows.iter().enumerate() {
        row_headers.push(r_hdr.clone());
        row_kinds.push(*r_kind);
        let mut row_cells = Vec::with_capacity(column_headers.len());

        for col_tuple in &kept_cols {
            let maybe_vals = cell_lookup.get(&(r_idx, col_tuple.clone()));
            for (m_idx, measure) in compiled.measures.iter().enumerate() {
                let v = maybe_vals
                    .and_then(|vec| vec.get(m_idx))
                    .unwrap_or(&serde_json::Value::Null);
                row_cells.push(format_measure_cell(v, measure.format.as_deref()));
            }
        }

        if emit_other_col {
            for (m_idx, measure) in compiled.measures.iter().enumerate() {
                let merged =
                    merge_overflow_measure(r_idx, m_idx, measure.agg, &overflow_cols, &cell_lookup);
                row_cells.push(format_measure_cell(&merged, measure.format.as_deref()));
            }
        }

        cells.push(row_cells);
    }

    PivotResult {
        row_headers,
        column_headers,
        cells,
        row_kinds,
        columns_truncated,
        sql: qr.sql.clone(),
        duration_ms: qr.duration_ms,
    }
}

fn collect_pivot_cells(
    compiled: &CompiledPivot,
    qr: &QueryResult,
    revealed_pii: bool,
) -> CollectedPivotCells {
    let col_masked_flags: HashMap<&str, bool> = qr
        .columns
        .iter()
        .map(|QueryColumn { name, masked, .. }| (name.as_str(), *masked))
        .collect();

    let mut ordered_rows = Vec::new();
    let mut row_index_map: HashMap<MatrixRowIdentity, usize> = HashMap::new();
    let mut col_freq: BTreeMap<Vec<String>, u64> = BTreeMap::new();
    let mut cell_lookup: IndexedCellMap = HashMap::new();

    for sql_row in &qr.rows {
        let (identity, header, kind) =
            extract_row_header(compiled, sql_row, &col_masked_flags, revealed_pii);
        let r_idx = if let Some(&existing) = row_index_map.get(&identity) {
            existing
        } else {
            let next_idx = ordered_rows.len();
            row_index_map.insert(identity.clone(), next_idx);
            ordered_rows.push((identity, header, kind));
            next_idx
        };

        let col_tuple = extract_col_tuple(compiled, sql_row, &col_masked_flags, revealed_pii);
        if !compiled.col_dims.is_empty() && kind == PivotRowKind::Data {
            let cnt = sql_row.get("__cnt").and_then(json_to_u64).unwrap_or(1);
            let entry = col_freq.entry(col_tuple.clone()).or_insert(0);
            *entry = entry.saturating_add(cnt);
        } else if !compiled.col_dims.is_empty() {
            col_freq.entry(col_tuple.clone()).or_insert(0);
        }

        let vals: Vec<serde_json::Value> = compiled
            .measures
            .iter()
            .map(|m| {
                sql_row
                    .get(&m.alias)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            })
            .collect();
        cell_lookup.insert((r_idx, col_tuple), vals);
    }

    CollectedPivotCells {
        ordered_rows,
        col_freq,
        cell_lookup,
    }
}

fn materialize_scalar_pivot(compiled: &CompiledPivot, qr: &QueryResult) -> PivotResult {
    let column_headers: Vec<Vec<String>> = compiled
        .measures
        .iter()
        .map(|m| vec![m.label.clone()])
        .collect();
    let kind = if compiled.grand_total {
        PivotRowKind::GrandTotal
    } else {
        PivotRowKind::Data
    };
    let first_row = qr.rows.first();
    let vals: Vec<String> = compiled
        .measures
        .iter()
        .map(|m| {
            let v = first_row
                .and_then(|r| r.get(&m.alias))
                .unwrap_or(&serde_json::Value::Null);
            format_measure_cell(v, m.format.as_deref())
        })
        .collect();
    PivotResult {
        row_headers: vec![Vec::new()],
        column_headers,
        cells: vec![vals],
        row_kinds: vec![kind],
        columns_truncated: false,
        sql: qr.sql.clone(),
        duration_ms: qr.duration_ms,
    }
}

fn build_no_coldim_result(
    compiled: &CompiledPivot,
    qr: &QueryResult,
    active_rows: &[(MatrixRowIdentity, Vec<String>, PivotRowKind)],
    cell_lookup: &HashMap<(usize, Vec<String>), Vec<serde_json::Value>>,
) -> PivotResult {
    let column_headers: Vec<Vec<String>> = compiled
        .measures
        .iter()
        .map(|m| vec![m.label.clone()])
        .collect();
    let empty_col = Vec::new();
    let mut row_headers = Vec::with_capacity(active_rows.len());
    let mut row_kinds = Vec::with_capacity(active_rows.len());
    let mut cells = Vec::with_capacity(active_rows.len());

    for (r_idx, (_, hdr, kind)) in active_rows.iter().enumerate() {
        row_headers.push(hdr.clone());
        row_kinds.push(*kind);
        let maybe_vals = cell_lookup.get(&(r_idx, empty_col.clone()));
        let mut row_cells = Vec::with_capacity(compiled.measures.len());
        for (m_idx, m) in compiled.measures.iter().enumerate() {
            let v = maybe_vals
                .and_then(|vec| vec.get(m_idx))
                .unwrap_or(&serde_json::Value::Null);
            row_cells.push(format_measure_cell(v, m.format.as_deref()));
        }
        cells.push(row_cells);
    }

    PivotResult {
        row_headers,
        column_headers,
        cells,
        row_kinds,
        columns_truncated: false,
        sql: qr.sql.clone(),
        duration_ms: qr.duration_ms,
    }
}

fn extract_row_header(
    compiled: &CompiledPivot,
    sql_row: &serde_json::Map<String, serde_json::Value>,
    col_masked_flags: &HashMap<&str, bool>,
    revealed_pii: bool,
) -> (MatrixRowIdentity, Vec<String>, PivotRowKind) {
    let n = compiled.row_dims.len();
    if n == 0 {
        return (
            MatrixRowIdentity {
                grouping_bits: Vec::new(),
                dim_values: Vec::new(),
            },
            Vec::new(),
            PivotRowKind::Data,
        );
    }

    let mut grouping_bits = Vec::with_capacity(n);
    for idx in 0..n {
        let g_col = format!("__gr{idx}");
        let is_grouped = sql_row.get(&g_col).and_then(json_to_u64).unwrap_or(0) != 0;
        grouping_bits.push(is_grouped);
    }

    let all_grouped = grouping_bits.iter().all(|&b| b);
    let any_grouped = grouping_bits.iter().any(|&b| b);
    let kind = if all_grouped {
        PivotRowKind::GrandTotal
    } else if any_grouped {
        PivotRowKind::Subtotal
    } else {
        PivotRowKind::Data
    };

    let mut dim_values = Vec::with_capacity(n);
    let mut header = Vec::with_capacity(n);
    let mut subtotal_labeled = false;

    for (idx, dim) in compiled.row_dims.iter().enumerate() {
        let grouped = grouping_bits.get(idx).copied().unwrap_or(false);
        if grouped {
            dim_values.push(String::new());
            if kind == PivotRowKind::GrandTotal {
                if idx == 0 {
                    header.push(String::from("总计"));
                } else {
                    header.push(String::new());
                }
            } else if !subtotal_labeled {
                subtotal_labeled = true;
                header.push(String::from("小计"));
            } else {
                header.push(String::new());
            }
        } else {
            let formatted = format_dim_value(
                sql_row,
                dim,
                col_masked_flags,
                revealed_pii,
                compiled.guarded.sql_policy_applied,
            );
            dim_values.push(formatted.clone());
            header.push(formatted);
        }
    }

    (
        MatrixRowIdentity {
            grouping_bits,
            dim_values,
        },
        header,
        kind,
    )
}

fn extract_col_tuple(
    compiled: &CompiledPivot,
    sql_row: &serde_json::Map<String, serde_json::Value>,
    col_masked_flags: &HashMap<&str, bool>,
    revealed_pii: bool,
) -> Vec<String> {
    compiled
        .col_dims
        .iter()
        .map(|dim| {
            format_dim_value(
                sql_row,
                dim,
                col_masked_flags,
                revealed_pii,
                compiled.guarded.sql_policy_applied,
            )
        })
        .collect()
}

fn format_dim_value(
    sql_row: &serde_json::Map<String, serde_json::Value>,
    dim: &ResolvedDim,
    col_masked_flags: &HashMap<&str, bool>,
    revealed_pii: bool,
    sql_policy_applied: bool,
) -> String {
    let Some(val) = sql_row.get(&dim.alias) else {
        return String::new();
    };
    if val.is_null() {
        return String::new();
    }
    let raw = json_scalar_to_string(val);
    let already_masked = sql_policy_applied
        || col_masked_flags
            .get(dim.alias.as_str())
            .copied()
            .unwrap_or(false);
    if !already_masked && !revealed_pii && dim.pii_category != PiiCategory::None {
        mask_pii_str(&raw, dim.pii_category)
    } else {
        raw
    }
}

fn select_pivot_columns(
    col_freq: &BTreeMap<Vec<String>, u64>,
    column_limit: usize,
) -> (Vec<Vec<String>>, Vec<Vec<String>>, bool) {
    if col_freq.len() <= column_limit {
        let kept: Vec<Vec<String>> = col_freq.keys().cloned().collect();
        return (kept, Vec::new(), false);
    }

    let mut ranked: Vec<(Vec<String>, u64)> =
        col_freq.iter().map(|(k, &v)| (k.clone(), v)).collect();
    ranked.sort_by(|(ka, va), (kb, vb)| Reverse(*va).cmp(&Reverse(*vb)).then_with(|| ka.cmp(kb)));

    let mut kept = Vec::with_capacity(column_limit);
    let mut overflow = Vec::with_capacity(ranked.len().saturating_sub(column_limit));
    for (idx, (col_key, _)) in ranked.into_iter().enumerate() {
        if idx < column_limit {
            kept.push(col_key);
        } else {
            overflow.push(col_key);
        }
    }
    (kept, overflow, true)
}

fn build_matrix_column_headers(
    compiled: &CompiledPivot,
    kept_cols: &[Vec<String>],
    emit_other_col: bool,
) -> Vec<Vec<String>> {
    let m_dims = compiled.col_dims.len();
    let total_combos = kept_cols.len().saturating_add(usize::from(emit_other_col));
    let mut headers = Vec::with_capacity(total_combos.saturating_mul(compiled.measures.len()));

    for col_tuple in kept_cols {
        for measure in &compiled.measures {
            let mut h = Vec::with_capacity(m_dims.saturating_add(1));
            h.extend(col_tuple.iter().cloned());
            h.push(measure.label.clone());
            headers.push(h);
        }
    }

    if emit_other_col {
        for measure in &compiled.measures {
            let mut h = Vec::with_capacity(m_dims.saturating_add(1));
            h.push(String::from("其他"));
            for _ in 1..m_dims {
                h.push(String::new());
            }
            h.push(measure.label.clone());
            headers.push(h);
        }
    }
    headers
}

const fn is_mergeable_agg(agg: AggFn) -> bool {
    matches!(agg, AggFn::Sum | AggFn::Count | AggFn::Min | AggFn::Max)
}

#[derive(Debug, Clone, Copy)]
struct FixedDecimal {
    mantissa: i128,
    scale: u32,
}

impl FixedDecimal {
    fn parse(val: &serde_json::Value) -> Option<Self> {
        let text = match val {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.trim().to_owned(),
            _ => return None,
        };
        if text.is_empty() || text.contains(['e', 'E']) {
            return None;
        }
        let (neg, rest) = if let Some(stripped) = text.strip_prefix('-') {
            (true, stripped)
        } else if let Some(stripped) = text.strip_prefix('+') {
            (false, stripped)
        } else {
            (false, text.as_str())
        };
        let (int_part, frac_part) = match rest.split_once('.') {
            Some((i, f)) => (i, f),
            None => (rest, ""),
        };
        if frac_part.len() > 9 {
            return None;
        }
        let combined = format!("{int_part}{frac_part}");
        if combined.is_empty() || !combined.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let unsigned: i128 = combined.parse().ok()?;
        let scale = u32::try_from(frac_part.len()).ok()?;
        Some(Self {
            mantissa: if neg { -unsigned } else { unsigned },
            scale,
        })
    }

    fn align(self, other: Self) -> Option<(i128, i128, u32)> {
        let target_scale = self.scale.max(other.scale);
        let lhs = self
            .mantissa
            .checked_mul(10_i128.checked_pow(target_scale.saturating_sub(self.scale))?)?;
        let rhs = other
            .mantissa
            .checked_mul(10_i128.checked_pow(target_scale.saturating_sub(other.scale))?)?;
        Some((lhs, rhs, target_scale))
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        let (lhs, rhs, scale) = self.align(other)?;
        Some(Self {
            mantissa: lhs.checked_add(rhs)?,
            scale,
        })
    }

    fn cmp_exact(self, other: Self) -> Option<std::cmp::Ordering> {
        let (lhs, rhs, _) = self.align(other)?;
        Some(lhs.cmp(&rhs))
    }

    fn to_json_value(self) -> serde_json::Value {
        if self.scale == 0 {
            if let Ok(i) = i64::try_from(self.mantissa) {
                return serde_json::Value::Number(serde_json::Number::from(i));
            }
            return serde_json::Value::String(self.mantissa.to_string());
        }
        let sign = if self.mantissa < 0 { "-" } else { "" };
        let abs_str = self.mantissa.unsigned_abs().to_string();
        let sc = usize::try_from(self.scale).unwrap_or(0);
        let raw = if abs_str.len() <= sc {
            let pad = "0".repeat(sc.saturating_sub(abs_str.len()));
            format!("{sign}0.{pad}{abs_str}")
        } else {
            let split = abs_str.len().saturating_sub(sc);
            let hi = abs_str.get(..split).unwrap_or("0");
            let lo = abs_str.get(split..).unwrap_or("0");
            format!("{sign}{hi}.{lo}")
        };
        let trimmed = raw.trim_end_matches('0').trim_end_matches('.');
        serde_json::Value::String(String::from(if trimmed.is_empty() || trimmed == "-" {
            "0"
        } else {
            trimmed
        }))
    }
}

fn merge_overflow_measure(
    r_idx: usize,
    m_idx: usize,
    agg: AggFn,
    overflow_cols: &[Vec<String>],
    cell_lookup: &HashMap<(usize, Vec<String>), Vec<serde_json::Value>>,
) -> serde_json::Value {
    let mut non_null: Vec<&serde_json::Value> = Vec::new();
    for col_key in overflow_cols {
        if let Some(vals) = cell_lookup.get(&(r_idx, col_key.clone()))
            && let Some(v) = vals.get(m_idx)
            && !v.is_null()
        {
            non_null.push(v);
        }
    }
    if non_null.is_empty() {
        return serde_json::Value::Null;
    }

    match agg {
        AggFn::Sum | AggFn::Count => {
            let mut dec_acc = FixedDecimal {
                mantissa: 0,
                scale: 0,
            };
            let mut all_dec = true;
            for v in &non_null {
                if let Some(d) = FixedDecimal::parse(v)
                    && let Some(next) = dec_acc.checked_add(d)
                {
                    dec_acc = next;
                } else {
                    all_dec = false;
                    break;
                }
            }
            if all_dec {
                return dec_acc.to_json_value();
            }
            let mut sum_f = 0.0_f64;
            for v in &non_null {
                if let Some(f) = json_to_f64(v) {
                    sum_f += f;
                }
            }
            serde_json::Number::from_f64(sum_f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        AggFn::Min | AggFn::Max => {
            let Some(&first) = non_null.first() else {
                return serde_json::Value::Null;
            };
            let mut best = first;
            for &cand in non_null.iter().skip(1) {
                let ord = compare_json_values(cand, best);
                let replace = match agg {
                    AggFn::Min => ord == std::cmp::Ordering::Less,
                    _ => ord == std::cmp::Ordering::Greater,
                };
                if replace {
                    best = cand;
                }
            }
            best.clone()
        }
        _ => serde_json::Value::Null,
    }
}

fn compare_json_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    if let (Some(da), Some(db)) = (FixedDecimal::parse(a), FixedDecimal::parse(b))
        && let Some(ord) = da.cmp_exact(db)
    {
        return ord;
    }
    if let (Some(fa), Some(fb)) = (json_to_f64(a), json_to_f64(b)) {
        return fa.total_cmp(&fb);
    }
    json_scalar_to_string(a).cmp(&json_scalar_to_string(b))
}

fn json_to_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn json_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn json_scalar_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return i.to_string();
            }
            if let Some(u) = n.as_u64() {
                return u.to_string();
            }
            n.as_f64().map_or_else(String::new, format_clean_f64)
        }
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn format_clean_f64(f: f64) -> String {
    if !f.is_finite() {
        return String::new();
    }
    if f == 0.0 {
        return String::from("0");
    }
    f.to_string()
}

fn format_measure_cell(val: &serde_json::Value, fmt_spec: Option<&str>) -> String {
    if val.is_null() {
        return String::new();
    }
    let Some(pattern) = fmt_spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return json_scalar_to_string(val);
    };
    let Some(mut num) = json_to_f64(val) else {
        return json_scalar_to_string(val);
    };
    if !num.is_finite() {
        return String::new();
    }

    let is_percent = pattern.ends_with('%');
    let body = pattern.strip_suffix('%').unwrap_or(pattern);
    if is_percent {
        num *= 100.0;
    }
    if num == 0.0 {
        num = 0.0;
    }

    let use_grouping = body.contains(',');
    let precision = body.split_once('.').map_or(0, |(_, frac)| {
        frac.chars().filter(|&c| c == '0' || c == '#').count()
    });

    let formatted = format!("{num:.precision$}");
    let with_commas = if use_grouping {
        insert_thousands_commas(&formatted)
    } else {
        formatted
    };

    if is_percent {
        format!("{with_commas}%")
    } else {
        with_commas
    }
}

fn insert_thousands_commas(formatted_num: &str) -> String {
    let (sign, unsigned) = if let Some(rest) = formatted_num.strip_prefix('-') {
        ("-", rest)
    } else {
        ("", formatted_num)
    };
    let (int_part, frac_opt) = match unsigned.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (unsigned, None),
    };
    let digits: Vec<char> = int_part.chars().collect();
    let len = digits.len();
    let mut grouped = String::with_capacity(len.saturating_add(len / 3));
    for (idx, &ch) in digits.iter().enumerate() {
        if idx > 0 && (len.saturating_sub(idx)).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac_opt {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}
