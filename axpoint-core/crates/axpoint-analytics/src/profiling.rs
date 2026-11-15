//! Deterministic `DuckDB` dataset profiling, PK/FK/cardinality inference, and dry-run mapping preview (`06-STEP05-06` §05.2.5, §05.5.1, §05.7).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::analytics::QueryColumn;
use axpoint_domain::axhub::Dataset;
use axpoint_domain::enums::{Cardinality, DataType};
use axpoint_domain::graph::GeoPoint;
use axpoint_domain::studio::{
    ColumnProfileStats, ColumnRoleTag, DatasetProfileSummary, FkCandidate, MappingLinkSpec,
    MappingPreviewNodeRow, MappingPreviewResponse, MappingRejectSample, MappingSpec, NodeTypeSpec,
    ProfilingBundle, PropertyKind, PropertySpec,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime};
use sqlparser::ast::{
    BinaryOperator, Expr, InTarget, Query, SelectItem, SetExpr, Statement, UnaryOperator,
};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use uuid::Uuid;

use crate::session::{SessionPool, TenantDb};

/// Maximum rows sampled per dataset during deterministic profiling (`06-STEP05-06` §05.5.1).
pub const MAX_PROFILE_SAMPLE_ROWS: usize = 100_000;

/// Maximum distinct non-null sample values retained per column (`06-STEP05-06` §05.5.1).
pub const MAX_PROFILE_SAMPLE_VALUES: usize = 8;

/// Maximum distinct count for tagging a `STRING` or `BOOLEAN` column as `CATEGORICAL` (`06-STEP05-06` §05.5.1).
pub const CATEGORICAL_MAX_DISTINCT: u64 = 50;

/// Minimum `|A.col ∩ B.pk| / |A.col distinct|` overlap ratio to emit an [`FkCandidate`] (`06-STEP05-06` §05.5.1).
pub const FK_MIN_OVERLAP_RATIO: f64 = 0.9;

/// Minimum `A.col distinct / A.non_null_rows` ratio to classify an [`FkCandidate`] as `ONE_TO_ONE` (`06-STEP05-06` §05.5.1).
pub const ONE_TO_ONE_MIN_UNIQUE_RATIO: f64 = 0.99;

/// Hard upper bound on rows inspected by [`preview_mapping_rows`] (`06-STEP05-06` §05.7).
pub const MAX_MAPPING_PREVIEW_ROWS: usize = 50;

/// Maximum rejected row diagnostics attached to [`MappingPreviewResponse`] (`06-STEP05-06` §05.1 & §05.7).
pub const MAX_REJECT_SAMPLES: usize = 20;

const STRIP_TABLE_PREFIXES: &[&str] = &[
    "demo_", "raw_", "dim_", "fact_", "dwd_", "ods_", "stg_", "tbl_", "t_",
];

#[derive(Debug, Clone)]
struct ProfiledDatasetCtx {
    table_ref: String,
    entity_stem: String,
    explicit_pks: BTreeSet<String>,
    summary: DatasetProfileSummary,
}

/// Formats the canonical tenant `DuckLake` table reference `lake."tenant_<sanitized_tenant>"."<storage_table>"`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `tenant_id` or `storage_table` is empty or invalid.
pub fn format_tenant_lake_table(tenant_id: &str, storage_table: &str) -> Result<String, AppError> {
    validate_tenant_id(tenant_id)?;
    let tbl = storage_table.trim();
    validate_sql_ident(tbl, "storage_table")?;
    let sanitized_tenant = sanitize_tenant_segment(tenant_id);
    let schema_name = format!("tenant_{sanitized_tenant}");
    Ok(format!(
        "lake.{}.{}",
        quote_ident(&schema_name),
        quote_ident(tbl)
    ))
}

/// Profiles `datasets` asynchronously via `SessionPool` (`06-STEP05-06` §05.5.1).
///
/// # Errors
/// Returns [`AppError::BadRequest`], [`AppError::NotFound`], or [`AppError::Database`] if `tenant_id`
/// is invalid, a dataset table cannot be resolved, or `DuckDB` execution fails.
pub async fn profile_datasets(
    pool: &SessionPool,
    tenant_id: &str,
    datasets: &[Dataset],
) -> Result<ProfilingBundle, AppError> {
    validate_tenant_id(tenant_id)?;
    if datasets.is_empty() {
        return Ok(ProfilingBundle::default());
    }
    let db = pool.get_or_open(tenant_id).await?;
    let tid = String::from(tenant_id);
    let ds_owned = datasets.to_vec();
    tokio::task::spawn_blocking(move || profile_datasets_conn(&db, &tid, &ds_owned))
        .await
        .map_err(|err| AppError::Database(format!("dataset profiling task join failed: {err}")))?
}

/// Synchronously profiles `datasets` against an open [`TenantDb`] connection (`06-STEP05-06` §05.5.1).
///
/// Supports both `DuckLake` tenant tables (`lake."tenant_<sanitized_tenant>"."<storage_table>"`)
/// and local/in-memory tables or views seeded in unit tests.
///
/// # Errors
/// Returns [`AppError::BadRequest`], [`AppError::NotFound`], or [`AppError::Database`] if `tenant_id`
/// is invalid, a dataset table cannot be resolved, or `DuckDB` query execution fails.
pub fn profile_datasets_conn(
    db: &TenantDb,
    tenant_id: &str,
    datasets: &[Dataset],
) -> Result<ProfilingBundle, AppError> {
    validate_tenant_id(tenant_id)?;
    if datasets.is_empty() {
        return Ok(ProfilingBundle::default());
    }

    let mut contexts = Vec::with_capacity(datasets.len());
    for dataset in datasets {
        contexts.push(profile_single_dataset(db, tenant_id, dataset)?);
    }

    let fk_candidates = infer_fk_candidates(db, &contexts)?;
    let summaries = contexts.into_iter().map(|ctx| ctx.summary).collect();

    Ok(ProfilingBundle {
        datasets: summaries,
        fk_candidates,
    })
}

/// Dry-run previews up to `limit.clamp(1, 50)` mapped nodes asynchronously via [`SessionPool`]
/// without writing to `PostgreSQL` (`06-STEP05-06` §05.2.5 & §05.7).
///
/// # Errors
/// Returns [`AppError::BadRequest`], [`AppError::NotFound`], or [`AppError::Database`] if `spec`
/// has an invalid filter/geo configuration, the dataset table is missing, or `DuckDB` fails.
pub async fn preview_mapping_rows(
    pool: &SessionPool,
    tenant_id: &str,
    mapping_id: Uuid,
    dataset: &Dataset,
    node_type: &NodeTypeSpec,
    spec: &MappingSpec,
    limit: usize,
) -> Result<MappingPreviewResponse, AppError> {
    validate_tenant_id(tenant_id)?;
    let db = pool.get_or_open(tenant_id).await?;
    let tid = String::from(tenant_id);
    let ds = dataset.clone();
    let nt = node_type.clone();
    let sp = spec.clone();
    tokio::task::spawn_blocking(move || {
        preview_mapping_rows_conn(&db, &tid, mapping_id, &ds, &nt, &sp, limit)
    })
    .await
    .map_err(|err| AppError::Database(format!("mapping preview task join failed: {err}")))?
}

/// Synchronously dry-run previews up to `limit.clamp(1, 50)` mapped nodes against an open [`TenantDb`]
/// without performing any database writes (`06-STEP05-06` §05.2.5 & §05.7).
///
/// # Errors
/// Returns [`AppError::BadRequest`], [`AppError::NotFound`], or [`AppError::Database`] if `spec`
/// has an invalid filter/geo configuration, the dataset table is missing, or `DuckDB` fails.
pub fn preview_mapping_rows_conn(
    db: &TenantDb,
    tenant_id: &str,
    mapping_id: Uuid,
    dataset: &Dataset,
    node_type: &NodeTypeSpec,
    spec: &MappingSpec,
    limit: usize,
) -> Result<MappingPreviewResponse, AppError> {
    validate_tenant_id(tenant_id)?;
    validate_geo_spec_pair(spec)?;

    let effective_limit = limit.clamp(1, MAX_MAPPING_PREVIEW_ROWS);
    let table_ref = resolve_dataset_table_ref(db, tenant_id, dataset)?;
    let where_clause = match spec.filter.as_deref().map(str::trim) {
        Some(raw_filter) if !raw_filter.is_empty() => {
            let compiled = compile_safe_row_filter(raw_filter)?;
            format!(" WHERE {compiled}")
        }
        _ => String::new(),
    };

    let sql = format!("SELECT * FROM {table_ref}{where_clause} LIMIT {effective_limit}");
    let qr = db.run_internal(&sql, effective_limit)?;

    let sampled_rows = qr.rows.len();
    let mut items = Vec::with_capacity(sampled_rows);
    let mut rows_rejected = 0_usize;
    let mut reject_samples = Vec::new();

    for row in qr.rows {
        match evaluate_preview_row(&row, node_type, spec) {
            Ok(mapped) => items.push(mapped),
            Err(reason) => {
                rows_rejected = rows_rejected.saturating_add(1);
                if reject_samples.len() < MAX_REJECT_SAMPLES {
                    reject_samples.push(MappingRejectSample {
                        row: serde_json::Value::Object(row),
                        reason,
                    });
                }
            }
        }
    }

    Ok(MappingPreviewResponse {
        mapping_id,
        sampled_rows,
        items,
        rows_rejected,
        reject_samples,
    })
}

fn profile_single_dataset(
    db: &TenantDb,
    tenant_id: &str,
    dataset: &Dataset,
) -> Result<ProfiledDatasetCtx, AppError> {
    let table_ref = resolve_dataset_table_ref(db, tenant_id, dataset)?;
    let schema_probe = db.run_internal(&format!("SELECT * FROM {table_ref} LIMIT 0"), 0)?;
    let physical_cols = select_profileable_columns(&schema_probe.columns, dataset);
    let entity_stem = normalize_entity_stem(&dataset.name);
    let display_name = if dataset.display_name.trim().is_empty() {
        dataset.name.clone()
    } else {
        dataset.display_name.clone()
    };
    let explicit_pks: BTreeSet<String> = dataset
        .primary_key
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    if physical_cols.is_empty() {
        let cnt_qr = db.run_internal(
            &format!(
                "SELECT COUNT(*) AS \"__row_count\" FROM (SELECT 1 FROM {table_ref} LIMIT {MAX_PROFILE_SAMPLE_ROWS}) AS s"
            ),
            1,
        )?;
        let row_count = cnt_qr
            .rows
            .first()
            .and_then(|r| json_to_u64(r.get("__row_count")))
            .unwrap_or(0);
        return Ok(ProfiledDatasetCtx {
            table_ref,
            entity_stem,
            explicit_pks,
            summary: DatasetProfileSummary {
                dataset_id: dataset.id,
                dataset_name: dataset.name.clone(),
                display_name,
                row_count,
                columns: Vec::new(),
                pk_candidates: Vec::new(),
            },
        });
    }

    let stats_sql = build_dataset_profile_sql(&table_ref, &physical_cols);
    let stats_qr = db.run_internal(&stats_sql, 1)?;
    let stats_row = stats_qr.rows.first();

    let row_count = stats_row
        .and_then(|r| json_to_u64(r.get("__row_count")))
        .unwrap_or(0);

    let mut raw_cols = Vec::with_capacity(physical_cols.len());
    let mut ranked_pks: Vec<(u8, usize, String)> = Vec::new();

    for (idx, (col_name, data_type)) in physical_cols.iter().enumerate() {
        let non_null = stats_row
            .and_then(|r| json_to_u64(r.get(&format!("__nn_{idx}"))))
            .unwrap_or(0);
        let distinct_count = stats_row
            .and_then(|r| json_to_u64(r.get(&format!("__dc_{idx}"))))
            .unwrap_or(0);
        let sample_values = stats_row
            .and_then(|r| r.get(&format!("__sv_{idx}")))
            .map_or_else(Vec::new, extract_sample_strings);

        let null_rate = if row_count == 0 || non_null >= row_count {
            0.0
        } else {
            let nulls = row_count.saturating_sub(non_null);
            (u64_to_f64(nulls) / u64_to_f64(row_count)).clamp(0.0, 1.0)
        };

        let mut roles = BTreeSet::new();
        if row_count > 0 && non_null == row_count && distinct_count == row_count {
            roles.insert(ColumnRoleTag::PkCandidate);
            let rank = pk_priority_rank(col_name, &entity_stem, *data_type, &explicit_pks);
            ranked_pks.push((rank, idx, col_name.clone()));
        }

        raw_cols.push(ColumnProfileStats {
            name: col_name.clone(),
            data_type: *data_type,
            null_rate,
            distinct_count,
            roles,
            sample_values,
        });
    }

    ranked_pks.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let pk_candidates: Vec<String> = ranked_pks.into_iter().map(|(_, _, name)| name).collect();
    let top_pk = pk_candidates.first().map(String::as_str);

    for col in &mut raw_cols {
        assign_secondary_roles(col, top_pk);
    }

    Ok(ProfiledDatasetCtx {
        table_ref,
        entity_stem,
        explicit_pks,
        summary: DatasetProfileSummary {
            dataset_id: dataset.id,
            dataset_name: dataset.name.clone(),
            display_name,
            row_count,
            columns: raw_cols,
            pk_candidates,
        },
    })
}

fn select_profileable_columns(
    physical: &[QueryColumn],
    dataset: &Dataset,
) -> Vec<(String, DataType)> {
    let mut declared_types: BTreeMap<String, DataType> = BTreeMap::new();
    for prof in &dataset.schema_json {
        declared_types.insert(prof.name.to_ascii_lowercase(), prof.data_type);
    }

    let mut out = Vec::with_capacity(physical.len());
    for col in physical {
        let lower = col.name.to_ascii_lowercase();
        if lower.starts_with("_ax_") && !declared_types.contains_key(&lower) {
            continue;
        }
        let dt = match (declared_types.get(&lower), col.data_type) {
            (Some(&DataType::Json), DataType::String) => DataType::Json,
            _ => col.data_type,
        };
        out.push((col.name.clone(), dt));
    }
    out
}

fn build_dataset_profile_sql(table_ref: &str, columns: &[(String, DataType)]) -> String {
    let mut projections = Vec::with_capacity(columns.len().saturating_mul(3).saturating_add(1));
    projections.push(String::from("COUNT(*) AS \"__row_count\""));

    for (idx, (col_name, data_type)) in columns.iter().enumerate() {
        let col_q = quote_ident(col_name);
        let dist_expr = distinct_sql_expr(&col_q, *data_type);
        let str_expr = varchar_sql_expr(&col_q, *data_type);

        projections.push(format!("COUNT({col_q}) AS \"__nn_{idx}\""));
        projections.push(format!("COUNT(DISTINCT {dist_expr}) AS \"__dc_{idx}\""));
        projections.push(format!(
            "(SELECT list(v ORDER BY v) FROM (SELECT DISTINCT {str_expr} AS v FROM sampled WHERE {col_q} IS NOT NULL LIMIT {MAX_PROFILE_SAMPLE_VALUES})) AS \"__sv_{idx}\""
        ));
    }

    format!(
        "WITH sampled AS (SELECT * FROM {table_ref} LIMIT {MAX_PROFILE_SAMPLE_ROWS}) SELECT {} FROM sampled",
        projections.join(", ")
    )
}

fn distinct_sql_expr(col_q: &str, data_type: DataType) -> String {
    match data_type {
        DataType::Timestamp => format!("CAST({col_q} AS TIMESTAMP)"),
        DataType::Json | DataType::List | DataType::Struct => format!("CAST({col_q} AS VARCHAR)"),
        _ => String::from(col_q),
    }
}

fn varchar_sql_expr(col_q: &str, data_type: DataType) -> String {
    if data_type == DataType::Timestamp {
        format!("CAST(CAST({col_q} AS TIMESTAMP) AS VARCHAR)")
    } else {
        format!("CAST({col_q} AS VARCHAR)")
    }
}

fn assign_secondary_roles(col: &mut ColumnProfileStats, top_pk: Option<&str>) {
    let is_top_pk = top_pk == Some(col.name.as_str());
    let id_like = is_id_like_column(&col.name);

    if matches!(
        col.data_type,
        DataType::Int64 | DataType::Float64 | DataType::Decimal
    ) && !id_like
        && !is_top_pk
    {
        col.roles.insert(ColumnRoleTag::NumericMetric);
    }

    if matches!(col.data_type, DataType::Date | DataType::Timestamp) {
        col.roles.insert(ColumnRoleTag::Temporal);
    }

    if matches!(col.data_type, DataType::String | DataType::Boolean)
        && col.distinct_count > 0
        && col.distinct_count <= CATEGORICAL_MAX_DISTINCT
        && !is_top_pk
        && !(col.roles.contains(&ColumnRoleTag::PkCandidate) && id_like)
    {
        col.roles.insert(ColumnRoleTag::Categorical);
    }
}

fn pk_priority_rank(
    col_name: &str,
    dataset_stem: &str,
    data_type: DataType,
    explicit_pks: &BTreeSet<String>,
) -> u8 {
    let lower = col_name.trim().to_ascii_lowercase();
    if explicit_pks.contains(&lower) || lower == "id" {
        return 0;
    }
    if let Some(stem) = extract_fk_stem(&lower)
        && !dataset_stem.is_empty()
        && stems_match(&stem, dataset_stem)
    {
        return 1;
    }
    if lower.ends_with("_id") {
        return 2;
    }
    if lower.ends_with("_code") {
        return 3;
    }
    if lower.ends_with("_no") {
        return 4;
    }
    if matches!(data_type, DataType::String | DataType::Int64) {
        return 5;
    }
    6
}

fn infer_fk_candidates(
    db: &TenantDb,
    contexts: &[ProfiledDatasetCtx],
) -> Result<Vec<FkCandidate>, AppError> {
    let mut results = Vec::new();

    for (idx_a, ctx_a) in contexts.iter().enumerate() {
        if ctx_a.summary.row_count == 0 {
            continue;
        }
        let top_pk_a = ctx_a.summary.pk_candidates.first().map(String::as_str);

        for col_a in &ctx_a.summary.columns {
            if !is_fk_eligible_column(col_a) {
                continue;
            }
            let fk_stem_opt = extract_fk_stem(&col_a.name);
            let is_a_top_pk = top_pk_a == Some(col_a.name.as_str());

            let mut matched_for_col: Vec<(FkCandidate, bool)> = Vec::new();

            for (idx_b, ctx_b) in contexts.iter().enumerate() {
                if idx_a == idx_b || ctx_b.summary.row_count == 0 {
                    continue;
                }
                let Some(pk_b) = select_target_pk(ctx_b, &col_a.name) else {
                    continue;
                };

                let same_pk_name =
                    !col_a.name.eq_ignore_ascii_case("id") && col_a.name.eq_ignore_ascii_case(pk_b);
                if !same_pk_name && fk_stem_opt.is_none() {
                    continue;
                }

                if should_skip_self_pk_fk(
                    is_a_top_pk,
                    &col_a.name,
                    fk_stem_opt.as_deref(),
                    &ctx_a.entity_stem,
                    &ctx_b.entity_stem,
                    same_pk_name,
                ) {
                    continue;
                }

                let pk_b_dt = ctx_b
                    .summary
                    .columns
                    .iter()
                    .find(|c| c.name == pk_b)
                    .map_or(DataType::String, |c| c.data_type);

                if let Some(cand) = probe_fk_overlap(db, ctx_a, col_a, ctx_b, pk_b, pk_b_dt)? {
                    let pk_b_stem = extract_fk_stem(pk_b);
                    let stem_aligned = same_pk_name
                        || fk_stem_opt.as_deref().is_some_and(|cs| {
                            stems_match(cs, &ctx_b.entity_stem)
                                || pk_b_stem.as_deref().is_some_and(|bs| stems_match(cs, bs))
                        });
                    matched_for_col.push((cand, stem_aligned));
                }
            }

            let any_aligned = matched_for_col.iter().any(|(_, aligned)| *aligned);
            for (cand, aligned) in matched_for_col {
                if !any_aligned || aligned {
                    results.push(cand);
                }
            }
        }
    }

    Ok(results)
}

fn is_fk_eligible_column(col: &ColumnProfileStats) -> bool {
    col.distinct_count > 0
        && matches!(
            col.data_type,
            DataType::String | DataType::Int64 | DataType::Decimal
        )
}

fn select_target_pk<'a>(ctx_b: &'a ProfiledDatasetCtx, src_col_name: &str) -> Option<&'a str> {
    let first_pk = ctx_b.summary.pk_candidates.first()?;
    if first_pk.eq_ignore_ascii_case(src_col_name) {
        return Some(first_pk.as_str());
    }
    if !src_col_name.eq_ignore_ascii_case("id") {
        for cand in &ctx_b.summary.pk_candidates {
            let lower = cand.to_ascii_lowercase();
            let authoritative = is_id_like_column(cand) || ctx_b.explicit_pks.contains(&lower);
            if authoritative && cand.eq_ignore_ascii_case(src_col_name) {
                return Some(cand.as_str());
            }
        }
    }
    let first_lower = first_pk.to_ascii_lowercase();
    if is_id_like_column(first_pk)
        || ctx_b.explicit_pks.contains(&first_lower)
        || ctx_b.summary.pk_candidates.len() == 1
    {
        Some(first_pk.as_str())
    } else {
        None
    }
}

fn should_skip_self_pk_fk(
    is_a_top_pk: bool,
    col_a_name: &str,
    fk_stem_opt: Option<&str>,
    a_entity_stem: &str,
    b_entity_stem: &str,
    same_pk_name: bool,
) -> bool {
    if !is_a_top_pk {
        return false;
    }
    if col_a_name.eq_ignore_ascii_case("id") {
        return true;
    }
    if let Some(c_stem) = fk_stem_opt
        && !a_entity_stem.is_empty()
        && stems_match(c_stem, a_entity_stem)
        && !stems_match(c_stem, b_entity_stem)
    {
        return true;
    }
    !same_pk_name && fk_stem_opt.is_none()
}

fn probe_fk_overlap(
    db: &TenantDb,
    ctx_a: &ProfiledDatasetCtx,
    col_a: &ColumnProfileStats,
    ctx_b: &ProfiledDatasetCtx,
    pk_b: &str,
    pk_b_dt: DataType,
) -> Result<Option<FkCandidate>, AppError> {
    let col_a_q = format!("sa.{}", quote_ident(&col_a.name));
    let pk_b_q = format!("sb.{}", quote_ident(pk_b));
    let expr_a = varchar_sql_expr(&col_a_q, col_a.data_type);
    let expr_b = varchar_sql_expr(&pk_b_q, pk_b_dt);

    let sql = format!(
        "WITH a_sample AS (
            SELECT {expr_a} AS k
            FROM (SELECT * FROM {} LIMIT {MAX_PROFILE_SAMPLE_ROWS}) AS sa
            WHERE {col_a_q} IS NOT NULL
        ),
        a_keys AS (
            SELECT DISTINCT k FROM a_sample WHERE k <> ''
        ),
        b_keys AS (
            SELECT DISTINCT {expr_b} AS k
            FROM (SELECT * FROM {} LIMIT {MAX_PROFILE_SAMPLE_ROWS}) AS sb
            WHERE {pk_b_q} IS NOT NULL
        )
        SELECT
            (SELECT COUNT(*) FROM a_sample WHERE k <> '') AS non_null_rows,
            (SELECT COUNT(*) FROM a_keys) AS a_distinct,
            (SELECT COUNT(*) FROM a_keys INNER JOIN b_keys USING (k)) AS overlap_cnt",
        ctx_a.table_ref, ctx_b.table_ref,
    );

    let qr = db.run_internal(&sql, 1)?;
    let Some(row) = qr.rows.first() else {
        return Ok(None);
    };
    let non_null_rows = json_to_u64(row.get("non_null_rows")).unwrap_or(0);
    let a_distinct = json_to_u64(row.get("a_distinct")).unwrap_or(0);
    let overlap_cnt = json_to_u64(row.get("overlap_cnt")).unwrap_or(0);

    if a_distinct == 0 || non_null_rows == 0 {
        return Ok(None);
    }

    let overlap_ratio = (u64_to_f64(overlap_cnt) / u64_to_f64(a_distinct)).clamp(0.0, 1.0);
    if overlap_ratio < FK_MIN_OVERLAP_RATIO {
        return Ok(None);
    }

    let unique_ratio = u64_to_f64(a_distinct) / u64_to_f64(non_null_rows);
    let cardinality = if unique_ratio >= ONE_TO_ONE_MIN_UNIQUE_RATIO {
        Cardinality::OneToOne
    } else {
        Cardinality::ManyToOne
    };

    Ok(Some(FkCandidate {
        src_dataset: ctx_a.summary.dataset_name.clone(),
        src_column: col_a.name.clone(),
        dst_dataset: ctx_b.summary.dataset_name.clone(),
        dst_pk_column: String::from(pk_b),
        overlap_ratio,
        cardinality,
    }))
}

fn evaluate_preview_row(
    row: &serde_json::Map<String, serde_json::Value>,
    node_type: &NodeTypeSpec,
    spec: &MappingSpec,
) -> Result<MappingPreviewNodeRow, String> {
    let key_col = spec.key_column.trim();
    if key_col.is_empty() {
        return Err(String::from("key_column is empty in MappingSpec"));
    }
    let node_key = extract_required_key(row, key_col)?;

    if let Some(ts_col) = spec
        .metric_time_column
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && let Some(raw_ts) = lookup_row_cell(row, ts_col)
        && !raw_ts.is_null()
    {
        validate_timestamp_cell(raw_ts, ts_col)?;
    }

    let attributes = coerce_row_attributes(row, node_type, spec)?;
    let metrics = coerce_row_metrics(row, node_type, spec)?;
    let geo = coerce_row_geo(row, spec)?;
    let links = build_row_links_preview(row, &spec.links)?;
    let title = render_mapping_title(spec, node_type, &node_key, row, &attributes);

    Ok(MappingPreviewNodeRow {
        node_key,
        title,
        attributes,
        metrics,
        geo,
        links,
    })
}

fn extract_required_key(
    row: &serde_json::Map<String, serde_json::Value>,
    key_col: &str,
) -> Result<String, String> {
    let Some(val) = lookup_row_cell(row, key_col) else {
        return Err(format!("missing key_column '{key_col}'"));
    };
    match val {
        serde_json::Value::Null => Err(format!("null value in key_column '{key_col}'")),
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Err(format!("empty value in key_column '{key_col}'"))
            } else {
                Ok(String::from(trimmed))
            }
        }
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Err(format!("non-scalar value in key_column '{key_col}'"))
        }
    }
}

fn coerce_row_attributes(
    row: &serde_json::Map<String, serde_json::Value>,
    node_type: &NodeTypeSpec,
    spec: &MappingSpec,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let prop_by_name: BTreeMap<&str, &PropertySpec> = node_type
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    for prop in &node_type.properties {
        if prop.kind == PropertyKind::Attribute
            && prop.required
            && !spec.attributes.contains_key(&prop.name)
        {
            return Err(format!(
                "required attribute '{}' is not bound in mapping attributes",
                prop.name
            ));
        }
    }

    let mut out = serde_json::Map::with_capacity(spec.attributes.len());
    for (attr_name, col_name) in &spec.attributes {
        let prop_opt = prop_by_name.get(attr_name.as_str()).copied();
        let required = prop_opt.is_some_and(|p| p.required);
        let raw_opt = lookup_row_cell(row, col_name);

        let Some(raw_val) = raw_opt else {
            if required {
                return Err(format!(
                    "missing required attribute '{attr_name}' (column '{col_name}')"
                ));
            }
            continue;
        };

        if raw_val.is_null() {
            if required {
                return Err(format!(
                    "null required attribute '{attr_name}' (column '{col_name}')"
                ));
            }
            continue;
        }

        let coerced = if let Some(prop) = prop_opt {
            coerce_value_to_property(raw_val, prop, col_name)?
        } else {
            raw_val.clone()
        };
        out.insert(attr_name.clone(), coerced);
    }

    Ok(out)
}

fn coerce_value_to_property(
    raw: &serde_json::Value,
    prop: &PropertySpec,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    let coerced = match prop.data_type {
        DataType::String => coerce_to_string(raw, prop, col_name)?,
        DataType::Int64 => coerce_to_int64(raw, &prop.name, col_name)?,
        DataType::Float64 | DataType::Decimal => coerce_to_float(raw, &prop.name, col_name)?,
        DataType::Boolean => coerce_to_bool(raw, &prop.name, col_name)?,
        DataType::Date => coerce_to_date(raw, &prop.name, col_name)?,
        DataType::Timestamp => {
            validate_timestamp_cell(raw, col_name)?;
            match raw {
                serde_json::Value::String(s) => serde_json::Value::String(String::from(s.trim())),
                _ => {
                    return Err(format!(
                        "invalid TIMESTAMP in column '{col_name}' for '{}'",
                        prop.name
                    ));
                }
            }
        }
        DataType::Json => coerce_to_json(raw, &prop.name, col_name)?,
        DataType::List => coerce_to_list(raw, &prop.name, col_name)?,
        DataType::Struct => coerce_to_struct(raw, &prop.name, col_name)?,
        DataType::Binary => match raw {
            serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
            _ => {
                return Err(format!(
                    "expected hex/text BINARY for attribute '{}' in column '{col_name}'",
                    prop.name
                ));
            }
        },
    };
    Ok(coerced)
}

fn coerce_to_string(
    raw: &serde_json::Value,
    prop: &PropertySpec,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    let text = match raw {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    };
    if prop.required && text.trim().is_empty() {
        return Err(format!(
            "empty required attribute '{}' in column '{col_name}'",
            prop.name
        ));
    }
    if let Some(allowed) = &prop.allowed_values
        && !allowed.is_empty()
        && !allowed.contains(&text)
    {
        return Err(format!(
            "attribute '{}' value '{text}' in column '{col_name}' is not in allowed_values",
            prop.name
        ));
    }
    Ok(serde_json::Value::String(text))
}

fn coerce_to_int64(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    match raw {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(serde_json::Value::Number(i.into()));
            }
            if let Some(u) = n.as_u64()
                && let Ok(i) = i64::try_from(u)
            {
                return Ok(serde_json::Value::Number(i.into()));
            }
            if let Some(f) = n.as_f64()
                && f.is_finite()
                && f.fract().abs() < f64::EPSILON
                && let Ok(i) = format!("{f:.0}").parse::<i64>()
            {
                return Ok(serde_json::Value::Number(i.into()));
            }
            Err(format!(
                "cannot convert '{raw}' in column '{col_name}' to INT64 for '{prop_name}'"
            ))
        }
        serde_json::Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map(|i| serde_json::Value::Number(i.into()))
            .map_err(|_| {
                format!("cannot parse '{s}' in column '{col_name}' as INT64 for '{prop_name}'")
            }),
        _ => Err(format!(
            "incompatible type in column '{col_name}' for INT64 attribute '{prop_name}'"
        )),
    }
}

fn coerce_to_float(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    let val = parse_finite_f64(raw, prop_name, col_name)?;
    serde_json::Number::from_f64(val)
        .map(serde_json::Value::Number)
        .ok_or_else(|| format!("non-finite float in column '{col_name}' for '{prop_name}'"))
}

fn coerce_to_bool(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    match raw {
        serde_json::Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(1) => Ok(serde_json::Value::Bool(true)),
            Some(0) => Ok(serde_json::Value::Bool(false)),
            _ => Err(format!(
                "cannot convert '{raw}' in column '{col_name}' to BOOLEAN for '{prop_name}'"
            )),
        },
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => Ok(serde_json::Value::Bool(true)),
            "false" | "0" | "no" | "n" => Ok(serde_json::Value::Bool(false)),
            _ => Err(format!(
                "cannot parse '{s}' in column '{col_name}' as BOOLEAN for '{prop_name}'"
            )),
        },
        _ => Err(format!(
            "incompatible type in column '{col_name}' for BOOLEAN attribute '{prop_name}'"
        )),
    }
}

fn coerce_to_date(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    let serde_json::Value::String(s) = raw else {
        return Err(format!(
            "expected DATE string in column '{col_name}' for '{prop_name}'"
        ));
    };
    let trimmed = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(trimmed, "%Y/%m/%d"))
    {
        return Ok(serde_json::Value::String(d.format("%Y-%m-%d").to_string()));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(serde_json::Value::String(
            dt.date_naive().format("%Y-%m-%d").to_string(),
        ));
    }
    Err(format!(
        "invalid DATE '{trimmed}' in column '{col_name}' for '{prop_name}'"
    ))
}

fn coerce_to_json(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    match raw {
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s.trim())
            .map_err(|_| format!("invalid JSON in column '{col_name}' for '{prop_name}'")),
        other => Ok(other.clone()),
    }
}

fn coerce_to_list(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    match raw {
        serde_json::Value::Array(_) => Ok(raw.clone()),
        serde_json::Value::String(s) => {
            let parsed: serde_json::Value = serde_json::from_str(s.trim()).map_err(|_| {
                format!("invalid LIST JSON in column '{col_name}' for '{prop_name}'")
            })?;
            if parsed.is_array() {
                Ok(parsed)
            } else {
                Err(format!(
                    "expected JSON array in column '{col_name}' for LIST '{prop_name}'"
                ))
            }
        }
        _ => Err(format!(
            "expected array in column '{col_name}' for LIST '{prop_name}'"
        )),
    }
}

fn coerce_to_struct(
    raw: &serde_json::Value,
    prop_name: &str,
    col_name: &str,
) -> Result<serde_json::Value, String> {
    match raw {
        serde_json::Value::Object(_) => Ok(raw.clone()),
        serde_json::Value::String(s) => {
            let parsed: serde_json::Value = serde_json::from_str(s.trim()).map_err(|_| {
                format!("invalid STRUCT JSON in column '{col_name}' for '{prop_name}'")
            })?;
            if parsed.is_object() {
                Ok(parsed)
            } else {
                Err(format!(
                    "expected JSON object in column '{col_name}' for STRUCT '{prop_name}'"
                ))
            }
        }
        _ => Err(format!(
            "expected object in column '{col_name}' for STRUCT '{prop_name}'"
        )),
    }
}

fn coerce_row_metrics(
    row: &serde_json::Map<String, serde_json::Value>,
    node_type: &NodeTypeSpec,
    spec: &MappingSpec,
) -> Result<BTreeMap<String, f64>, String> {
    let prop_by_name: BTreeMap<&str, &PropertySpec> = node_type
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    for prop in &node_type.properties {
        if prop.kind == PropertyKind::Metric
            && prop.required
            && !is_rule_produced_metric(node_type, &prop.name)
            && !spec.metrics.contains_key(&prop.name)
        {
            return Err(format!(
                "required metric '{}' is not bound in mapping metrics",
                prop.name
            ));
        }
    }

    let mut out = BTreeMap::new();
    for (metric_name, col_name) in &spec.metrics {
        let required = prop_by_name
            .get(metric_name.as_str())
            .is_some_and(|p| p.required);
        let Some(raw_val) = lookup_row_cell(row, col_name) else {
            if required {
                return Err(format!(
                    "missing required metric '{metric_name}' (column '{col_name}')"
                ));
            }
            continue;
        };

        if raw_val.is_null() {
            if required {
                return Err(format!(
                    "null required metric '{metric_name}' (column '{col_name}')"
                ));
            }
            continue;
        }

        let num = parse_finite_f64(raw_val, metric_name, col_name)?;
        out.insert(metric_name.clone(), num);
    }

    Ok(out)
}

fn is_rule_produced_metric(node_type: &NodeTypeSpec, metric_name: &str) -> bool {
    node_type
        .agg_rules
        .iter()
        .any(|r| r.target_metric == metric_name)
        || node_type
            .derived_rules
            .iter()
            .any(|r| r.enabled && r.target_metric == metric_name)
}

fn parse_finite_f64(
    raw: &serde_json::Value,
    field_label: &str,
    col_name: &str,
) -> Result<f64, String> {
    let parsed = match raw {
        serde_json::Value::Number(n) => n.as_f64().ok_or_else(|| {
            format!(
                "numeric value '{raw}' in column '{col_name}' out of f64 range for '{field_label}'"
            )
        })?,
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(format!(
                    "empty numeric string in column '{col_name}' for '{field_label}'"
                ));
            }
            trimmed.parse::<f64>().map_err(|_| {
                format!("cannot parse '{trimmed}' in column '{col_name}' as finite number for '{field_label}'")
            })?
        }
        _ => {
            return Err(format!(
                "non-numeric value '{raw}' in column '{col_name}' for '{field_label}'"
            ));
        }
    };

    if !parsed.is_finite() {
        return Err(format!(
            "non-finite numeric value in column '{col_name}' for '{field_label}'"
        ));
    }
    Ok(parsed)
}

fn validate_geo_spec_pair(spec: &MappingSpec) -> Result<(), AppError> {
    let has_lng = spec
        .geo
        .lng_column
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    let has_lat = spec
        .geo
        .lat_column
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    if has_lng != has_lat {
        return Err(AppError::bad(
            "MAPPING_INVALID",
            "geo mapping requires both lng_column and lat_column when configured",
        ));
    }
    Ok(())
}

fn coerce_row_geo(
    row: &serde_json::Map<String, serde_json::Value>,
    spec: &MappingSpec,
) -> Result<Option<GeoPoint>, String> {
    let lng_col_opt = spec
        .geo
        .lng_column
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let lat_col_opt = spec
        .geo
        .lat_column
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let (Some(lng_col), Some(lat_col)) = (lng_col_opt, lat_col_opt) else {
        return Ok(None);
    };

    let raw_lng = lookup_row_cell(row, lng_col);
    let raw_lat = lookup_row_cell(row, lat_col);

    let lng_missing = raw_lng.is_none_or(serde_json::Value::is_null);
    let lat_missing = raw_lat.is_none_or(serde_json::Value::is_null);
    if lng_missing && lat_missing {
        return Ok(None);
    }
    let (Some(v_lng), Some(v_lat)) = (raw_lng, raw_lat) else {
        return Err(format!(
            "incomplete geo coordinates across '{lng_col}' and '{lat_col}'"
        ));
    };
    if v_lng.is_null() || v_lat.is_null() {
        return Err(format!(
            "incomplete geo coordinates across '{lng_col}' and '{lat_col}'"
        ));
    }

    let lng = parse_finite_f64(v_lng, "geo.lng", lng_col)?;
    let lat = parse_finite_f64(v_lat, "geo.lat", lat_col)?;
    let pt = GeoPoint { lng, lat };
    pt.validate().map_err(|err| err.client_message())?;
    Ok(Some(pt))
}

fn build_row_links_preview(
    row: &serde_json::Map<String, serde_json::Value>,
    link_specs: &[MappingLinkSpec],
) -> Result<Vec<serde_json::Value>, String> {
    let mut links = Vec::new();
    for ls in link_specs {
        let Some(raw_target) = lookup_row_cell(row, ls.target_key_column.trim()) else {
            continue;
        };
        if raw_target.is_null() {
            continue;
        }
        let target_key = scalar_json_to_string(raw_target);
        let trimmed_target = target_key.trim();
        if trimmed_target.is_empty() {
            continue;
        }

        let weight_opt = if let Some(w_col) = ls
            .weight_column
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && let Some(raw_w) = lookup_row_cell(row, w_col)
            && !raw_w.is_null()
        {
            let w = parse_finite_f64(raw_w, "link.weight", w_col)?;
            Some(w)
        } else {
            None
        };

        let mut obj = serde_json::Map::with_capacity(6);
        obj.insert(
            String::from("link_type"),
            serde_json::Value::String(ls.link_type.clone()),
        );
        obj.insert(
            String::from("direction"),
            serde_json::Value::String(String::from(ls.direction.as_str())),
        );
        obj.insert(
            String::from("target_type"),
            serde_json::Value::String(ls.target_type.clone()),
        );
        obj.insert(
            String::from("target_key"),
            serde_json::Value::String(String::from(trimmed_target)),
        );
        if let Some(num) = weight_opt.and_then(serde_json::Number::from_f64) {
            obj.insert(String::from("weight"), serde_json::Value::Number(num));
        }
        obj.insert(
            String::from("create_missing_target"),
            serde_json::Value::Bool(ls.create_missing_target),
        );
        links.push(serde_json::Value::Object(obj));
    }
    Ok(links)
}

fn render_mapping_title(
    spec: &MappingSpec,
    node_type: &NodeTypeSpec,
    node_key: &str,
    row: &serde_json::Map<String, serde_json::Value>,
    attributes: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let tpl = if !spec.title.trim().is_empty() {
        spec.title.as_str()
    } else if !node_type.title_template.trim().is_empty() {
        node_type.title_template.as_str()
    } else {
        return String::from(node_key);
    };

    let mut rendered = String::with_capacity(tpl.len().saturating_add(16));
    let mut chars = tpl.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut token = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    closed = true;
                    break;
                }
                token.push(inner);
            }
            if closed {
                let key = token.trim();
                let replacement = resolve_title_token(key, node_key, row, attributes);
                rendered.push_str(&replacement);
            } else {
                rendered.push('{');
                rendered.push_str(&token);
            }
        } else {
            rendered.push(ch);
        }
    }

    if rendered.trim().is_empty() {
        String::from(node_key)
    } else {
        rendered
    }
}

fn resolve_title_token(
    token: &str,
    node_key: &str,
    row: &serde_json::Map<String, serde_json::Value>,
    attributes: &serde_json::Map<String, serde_json::Value>,
) -> String {
    if token == "node_key" {
        return String::from(node_key);
    }
    if let Some(prop_name) = token.strip_prefix("prop.") {
        if let Some(attr_val) = attributes.get(prop_name) {
            return scalar_json_to_string(attr_val);
        }
        if let Some(row_val) = lookup_row_cell(row, prop_name) {
            return scalar_json_to_string(row_val);
        }
        return String::new();
    }
    lookup_row_cell(row, token).map_or_else(String::new, scalar_json_to_string)
}

fn validate_timestamp_cell(raw: &serde_json::Value, col_name: &str) -> Result<(), String> {
    let serde_json::Value::String(s) = raw else {
        return Err(format!(
            "expected timestamp string in column '{col_name}', got '{raw}'"
        ));
    };
    let trimmed = s.trim();
    let valid = DateTime::parse_from_rfc3339(trimmed).is_ok()
        || NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%.f").is_ok()
        || NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
        || NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_ok()
        || DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%.f%#z").is_ok();
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid timestamp '{trimmed}' in column '{col_name}'"
        ))
    }
}

fn compile_safe_row_filter(raw_filter: &str) -> Result<String, AppError> {
    if raw_filter.contains('\0') || raw_filter.contains(';') {
        return Err(AppError::bad(
            "MAPPING_INVALID",
            "mapping filter contains forbidden characters",
        ));
    }
    let normalized = normalize_expr_operators(raw_filter);
    let probe_sql = format!("SELECT 1 WHERE {normalized}");
    let stmts = Parser::parse_sql(&DuckDbDialect {}, &probe_sql).map_err(|err| {
        AppError::bad(
            "MAPPING_INVALID",
            format!("invalid mapping filter syntax: {err}"),
        )
    })?;
    let [Statement::Query(query)] = stmts.as_slice() else {
        return Err(AppError::bad(
            "MAPPING_INVALID",
            "mapping filter must be a single boolean expression",
        ));
    };
    let Some(pred) = extract_where_predicate(query) else {
        return Err(AppError::bad(
            "MAPPING_INVALID",
            "mapping filter expression is missing",
        ));
    };
    validate_safe_filter_expr(pred)?;
    Ok(pred.to_string())
}

fn extract_where_predicate(query: &Query) -> Option<&Expr> {
    let SetExpr::Select(sel) = query.body.as_ref() else {
        return None;
    };
    if !sel.from.is_empty() || sel.projection.len() != 1 {
        return None;
    }
    if !matches!(sel.projection.first(), Some(SelectItem::UnnamedExpr(_))) {
        return None;
    }
    sel.selection.as_ref()
}

fn validate_safe_filter_expr(expr: &Expr) -> Result<(), AppError> {
    match expr {
        Expr::Identifier(_) | Expr::Value(_) => Ok(()),
        Expr::CompoundIdentifier(parts) => {
            if parts.len() == 2
                && parts
                    .first()
                    .is_some_and(|p| p.value.eq_ignore_ascii_case("prop"))
            {
                Err(AppError::bad(
                    "MAPPING_INVALID",
                    "dataset mapping filter should reference column names directly without 'prop.'",
                ))
            } else if parts.len() == 1 {
                Ok(())
            } else {
                Err(AppError::bad(
                    "MAPPING_INVALID",
                    "qualified table references are forbidden in mapping filter",
                ))
            }
        }
        Expr::Nested(inner)
        | Expr::IsNull(inner)
        | Expr::IsNotNull(inner)
        | Expr::IsTrue(inner)
        | Expr::IsFalse(inner) => validate_safe_filter_expr(inner),
        Expr::UnaryOp { op, expr: inner } => match op {
            UnaryOperator::Not | UnaryOperator::Minus | UnaryOperator::Plus => {
                validate_safe_filter_expr(inner)
            }
            _ => Err(AppError::bad(
                "MAPPING_INVALID",
                "unsupported unary operator in mapping filter",
            )),
        },
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Lt
            | BinaryOperator::LtEq
            | BinaryOperator::Gt
            | BinaryOperator::GtEq
            | BinaryOperator::And
            | BinaryOperator::Or
            | BinaryOperator::Plus
            | BinaryOperator::Minus
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo => {
                validate_safe_filter_expr(left)?;
                validate_safe_filter_expr(right)
            }
            _ => Err(AppError::bad(
                "MAPPING_INVALID",
                format!("unsupported binary operator '{op}' in mapping filter"),
            )),
        },
        Expr::In {
            expr: target,
            target: InTarget::List(list),
            ..
        } => {
            validate_safe_filter_expr(target)?;
            for item in list {
                validate_safe_filter_expr(item)?;
            }
            Ok(())
        }
        Expr::Between {
            expr: target,
            low,
            high,
            ..
        } => {
            validate_safe_filter_expr(target)?;
            validate_safe_filter_expr(low)?;
            validate_safe_filter_expr(high)
        }
        _ => Err(AppError::bad(
            "MAPPING_INVALID",
            "only column comparisons, literals, and boolean logic are allowed in mapping filter",
        )),
    }
}

fn normalize_expr_operators(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().saturating_add(8));
    let mut chars = raw.chars().peekable();
    let mut in_single_quote = false;

    while let Some(c) = chars.next() {
        if c == '\'' {
            out.push(c);
            if in_single_quote && chars.peek() == Some(&'\'') {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else {
                in_single_quote = !in_single_quote;
            }
            continue;
        }
        if !in_single_quote {
            if c == '=' && chars.peek() == Some(&'=') {
                let _ = chars.next();
                out.push('=');
                continue;
            }
            if c == '&' && chars.peek() == Some(&'&') {
                let _ = chars.next();
                out.push_str(" AND ");
                continue;
            }
            if c == '|' && chars.peek() == Some(&'|') {
                let _ = chars.next();
                out.push_str(" OR ");
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn resolve_dataset_table_ref(
    db: &TenantDb,
    tenant_id: &str,
    dataset: &Dataset,
) -> Result<String, AppError> {
    let storage_table = if dataset.lake_table.trim().is_empty() {
        dataset.name.trim()
    } else {
        dataset.lake_table.trim()
    };
    validate_sql_ident(storage_table, "dataset.lake_table")?;
    let ds_name = dataset.name.trim();
    validate_sql_ident(ds_name, "dataset.name")?;

    let mut candidates = Vec::with_capacity(5);
    let canonical_lake = format_tenant_lake_table(tenant_id, storage_table)?;
    candidates.push(canonical_lake);

    let raw_tenant_lake = format!(
        "lake.{}.{}",
        quote_ident(&format!("tenant_{tenant_id}")),
        quote_ident(storage_table)
    );
    push_unique(&mut candidates, raw_tenant_lake);

    let lake_schema = dataset.lake_schema.trim();
    if !lake_schema.is_empty() && !lake_schema.contains('\0') {
        let explicit_lake = format!(
            "lake.{}.{}",
            quote_ident(lake_schema),
            quote_ident(storage_table)
        );
        push_unique(&mut candidates, explicit_lake);
    }

    push_unique(&mut candidates, quote_ident(storage_table));
    if ds_name != storage_table {
        push_unique(&mut candidates, quote_ident(ds_name));
    }

    for cand in &candidates {
        let probe = format!("SELECT 1 FROM {cand} LIMIT 0");
        if db.run_internal(&probe, 0).is_ok() {
            return Ok(cand.clone());
        }
    }

    Err(AppError::NotFound("DATASET_NOT_FOUND"))
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn lookup_row_cell<'a>(
    row: &'a serde_json::Map<String, serde_json::Value>,
    col_name: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(exact) = row.get(col_name) {
        return Some(exact);
    }
    row.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(col_name))
        .map(|(_, v)| v)
}

fn scalar_json_to_string(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn extract_sample_strings(val: &serde_json::Value) -> Vec<String> {
    let serde_json::Value::Array(arr) = val else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len().min(MAX_PROFILE_SAMPLE_VALUES));
    for item in arr.iter().take(MAX_PROFILE_SAMPLE_VALUES) {
        if !item.is_null() {
            out.push(scalar_json_to_string(item));
        }
    }
    out
}

fn json_to_u64(val: Option<&serde_json::Value>) -> Option<u64> {
    let v = val?;
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_i64().and_then(|i| u64::try_from(i).ok())),
        serde_json::Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn u64_to_f64(v: u64) -> f64 {
    if let Ok(small) = u32::try_from(v) {
        f64::from(small)
    } else {
        let hi = u32::try_from(v >> 32).unwrap_or(u32::MAX);
        let lo = u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0);
        f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo))
    }
}

fn is_id_like_column(col_name: &str) -> bool {
    let lower = col_name.trim().to_ascii_lowercase();
    lower == "id" || lower.ends_with("_id") || lower.ends_with("_code") || lower.ends_with("_no")
}

fn extract_fk_stem(col_name: &str) -> Option<String> {
    let lower = col_name.trim().to_ascii_lowercase();
    let raw_stem = lower
        .strip_suffix("_id")
        .or_else(|| lower.strip_suffix("_code"))
        .or_else(|| lower.strip_suffix("_no"))?;
    let norm = normalize_entity_stem(raw_stem);
    if norm.is_empty() { None } else { Some(norm) }
}

fn normalize_entity_stem(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    for prefix in STRIP_TABLE_PREFIXES {
        if let Some(stripped) = s.strip_prefix(prefix)
            && !stripped.is_empty()
        {
            s = String::from(stripped);
            break;
        }
    }
    singularize_english(&s)
}

fn singularize_english(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies")
        && stem.len() >= 2
    {
        return format!("{stem}y");
    }
    for suffix in ["sses", "xes", "zes", "ches", "shes", "statuses", "buses"] {
        if word.ends_with(suffix)
            && let Some(stem) = word.strip_suffix("es")
            && stem.len() >= 2
        {
            return String::from(stem);
        }
    }
    if let Some(stem) = word.strip_suffix('s')
        && !stem.ends_with('s')
        && !word.ends_with("us")
        && !word.ends_with("is")
        && stem.len() >= 2
    {
        return String::from(stem);
    }
    String::from(word)
}

fn stems_match(col_stem: &str, entity_stem: &str) -> bool {
    if col_stem.is_empty() || entity_stem.is_empty() {
        return false;
    }
    col_stem == entity_stem
        || col_stem.ends_with(&format!("_{entity_stem}"))
        || entity_stem.ends_with(&format!("_{col_stem}"))
}

fn sanitize_tenant_segment(tenant_id: &str) -> String {
    tenant_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn validate_tenant_id(tenant_id: &str) -> Result<(), AppError> {
    let bytes = tenant_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "invalid tenant_id length",
        ));
    }
    let valid = bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !valid {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "invalid tenant_id format",
        ));
    }
    Ok(())
}

fn validate_sql_ident(ident: &str, label: &str) -> Result<(), AppError> {
    if ident.is_empty() || ident.len() > 128 || ident.contains('\0') {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid SQL identifier for {label}"),
        ));
    }
    Ok(())
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
