//! AI-assisted data transformation draft generator, `EXPLAIN` + 100-row dry-run previewer,
//! schema diff calculator, and `DERIVED_VIEW` dependency cycle detector (`05-STEP04` §04.7).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{QueryColumn, TransformPreview};
use axpoint_domain::ports::AiPort;
use serde::Deserialize;
use uuid::Uuid;

use crate::nl2sql::intent::extract_json_object;
use crate::nl2sql::prompt::strip_trailing_semicolons;
use crate::nl2sql::{execute_guarded_conn, explain_on_conn};
use crate::session::TenantDb;
use crate::sql_guard::{
    CatalogDatasetEntry, GuardOptions, TenantCatalog, guard, guard_for_materialize,
};

/// Maximum preview rows returned when creating or editing a transform draft (`05-STEP04` §04.7).
pub const TRANSFORM_PREVIEW_MAX_ROWS: u32 = 100;

/// System prompt for generating a single `DuckDB` `SELECT` transformation query (`§04.7`).
pub const TRANSFORM_SYSTEM_PROMPT: &str = "你是 DuckDB 数据工程与清洗专家。请根据用户的清洗/去重/类型转换/拆分列/关联/聚合/派生列指令，基于给定源表生成且仅生成单条 DuckDB SELECT 语句。表名直接使用给出的源表名。只输出一个 JSON 对象：{\"sql\": \"SELECT ...\"}";

#[derive(Debug, Deserialize)]
struct LlmTransformDraft {
    sql: String,
}

/// Validated transformation plan produced by [`preview_transform_sql`] / [`preview_transform_sql_conn`].
#[derive(Debug, Clone)]
pub struct ValidatedTransformPlan {
    /// Clean user-editable `SELECT` SQL (before `lake."tenant_<id>"` rewrite).
    pub clean_sql: String,
    /// Guarded & rewritten `lake."tenant_<id>".*` SQL without forced `LIMIT` (stored in `source_ref.sql` for Sidecar `POST /lake/materialize`).
    pub materialized_sql: String,
    /// Upstream dataset UUIDs referenced by `clean_sql` (`source_ref.depends_on`).
    pub depends_on: Vec<Uuid>,
    /// Dry-run preview (`<= 100` PII-masked rows + added/removed column diff).
    pub preview: TransformPreview,
}

/// Validates that `target_dataset_name` matches `^[a-z][a-z0-9_]{0,62}$` and does not collide
/// with an existing dataset in `catalog` (`05-STEP04` §04.7).
///
/// # Errors
/// - Returns `AppError::BadRequest("INVALID_ARGUMENT", ...)` on invalid identifier syntax.
/// - Returns `AppError::Conflict("NAME_TAKEN")` if `catalog` already contains `target_dataset_name`.
pub fn validate_target_dataset_name(
    target_dataset_name: &str,
    catalog: &TenantCatalog,
) -> Result<(), AppError> {
    let bytes = target_dataset_name.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !valid {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "target_dataset_name '{target_dataset_name}' must match ^[a-z][a-z0-9_]{{0,62}}$"
            ),
        ));
    }
    if catalog.dataset(target_dataset_name).is_some() {
        return Err(AppError::Conflict("NAME_TAKEN"));
    }
    Ok(())
}

/// Generates a candidate `SELECT` transformation query from `instruction` and `source_dataset_ids`
/// using `AiPort::chat` (`05-STEP04` §04.7).
///
/// # Errors
/// Returns `INVALID_ARGUMENT`, `DATASET_NOT_FOUND`, or `AppError::Llm`.
pub async fn generate_transform_sql<A: AiPort>(
    ai: &A,
    instruction: &str,
    source_dataset_ids: &[Uuid],
    catalog: &TenantCatalog,
) -> Result<String, AppError> {
    if instruction.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "transform instruction cannot be empty",
        ));
    }
    if source_dataset_ids.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "source_datasets must contain at least one dataset",
        ));
    }

    let mut prompt_buf = String::from("### 源数据集 Schema\n");
    for &ds_id in source_dataset_ids {
        let Some(entry) = catalog.dataset_by_id(ds_id) else {
            return Err(AppError::NotFound("DATASET_NOT_FOUND"));
        };
        append_source_ddl(&mut prompt_buf, entry);
    }
    let _ = write!(prompt_buf, "\n### 数据处理指令\n{}", instruction.trim());

    let raw = ai.chat(TRANSFORM_SYSTEM_PROMPT, &prompt_buf).await?;
    parse_transform_sql_response(&raw)
}

/// Validates `sql_text` via `sql_guard::guard_for_materialize`, runs `EXPLAIN`, executes a
/// `<= 100`-row PII-masked preview on [`TenantDb`], and computes column diffs against
/// `source_dataset_ids` (`05-STEP04` §04.7).
///
/// # Errors
/// Returns `DATASET_NOT_FOUND`, `INVALID_ARGUMENT`, `SQL_REJECTED`, `QUERY_TIMEOUT`, or `Database`.
pub async fn preview_transform_sql(
    session: &TenantDb,
    catalog: &TenantCatalog,
    sql_text: &str,
    source_dataset_ids: &[Uuid],
    timeout_ms: Option<u64>,
) -> Result<ValidatedTransformPlan, AppError> {
    let (clean_sql, materialized_sql, depends_on, preview_guarded) =
        prepare_transform_guards(sql_text, source_dataset_ids, catalog)?;

    let _ = session
        .explain_guarded(&preview_guarded, timeout_ms)
        .await?;
    let exec_out = session
        .execute_guarded(&preview_guarded, catalog, timeout_ms, false, false)
        .await?;

    let preview = build_transform_preview(
        &exec_out.result.columns,
        exec_out.result.rows,
        source_dataset_ids,
        &depends_on,
        catalog,
    );

    Ok(ValidatedTransformPlan {
        clean_sql,
        materialized_sql,
        depends_on,
        preview,
    })
}

/// Synchronously validates, dry-runs `EXPLAIN`, and previews `sql_text` (`<= 100` rows) against
/// an open `duckdb::Connection`.
///
/// # Errors
/// Returns `DATASET_NOT_FOUND`, `INVALID_ARGUMENT`, `SQL_REJECTED`, or `Database`.
pub fn preview_transform_sql_conn(
    conn: &duckdb::Connection,
    catalog: &TenantCatalog,
    sql_text: &str,
    source_dataset_ids: &[Uuid],
) -> Result<ValidatedTransformPlan, AppError> {
    let (clean_sql, materialized_sql, depends_on, preview_guarded) =
        prepare_transform_guards(sql_text, source_dataset_ids, catalog)?;

    explain_on_conn(conn, &preview_guarded.sql)?;
    let qr = execute_guarded_conn(conn, catalog, &preview_guarded)?;

    let preview = build_transform_preview(
        &qr.columns,
        qr.rows,
        source_dataset_ids,
        &depends_on,
        catalog,
    );

    Ok(ValidatedTransformPlan {
        clean_sql,
        materialized_sql,
        depends_on,
        preview,
    })
}

/// Verifies that adding `candidate_dataset_id -> direct_deps` introduces no cycle into
/// `existing_deps_by_dataset` (`dataset_id -> depends_on[]`, per `05-STEP04` §04.7).
///
/// # Errors
/// Returns `AppError::BadRequest("CYCLE_DETECTED", ...)` if a self-dependency or cycle exists.
pub fn ensure_no_dependency_cycle<S: std::hash::BuildHasher>(
    candidate_dataset_id: Uuid,
    direct_deps: &[Uuid],
    existing_deps_by_dataset: &HashMap<Uuid, Vec<Uuid>, S>,
) -> Result<(), AppError> {
    if direct_deps.contains(&candidate_dataset_id) {
        return Err(AppError::bad(
            "CYCLE_DETECTED",
            "derived dataset cannot depend on itself",
        ));
    }

    // Check if `candidate_dataset_id` is reachable upstream from any `dep` in `direct_deps`.
    let mut visited = BTreeSet::new();
    let mut stack: Vec<Uuid> = direct_deps.to_vec();
    while let Some(curr) = stack.pop() {
        if curr == candidate_dataset_id {
            return Err(AppError::bad(
                "CYCLE_DETECTED",
                "derived dataset dependency graph contains a cycle",
            ));
        }
        if !visited.insert(curr) {
            continue;
        }
        if let Some(upstreams) = existing_deps_by_dataset.get(&curr) {
            for &up in upstreams {
                stack.push(up);
            }
        }
    }
    Ok(())
}

/// Computes all downstream `DERIVED_VIEW` datasets transitively depending on `upstream_dataset_id`
/// in topological execution order (`05-STEP04` §04.7).
///
/// `deps_by_dataset` maps each `derived_dataset_id -> its upstream depends_on UUIDs`.
///
/// # Errors
/// Returns `AppError::BadRequest("CYCLE_DETECTED", ...)` if a dependency cycle is encountered.
pub fn topological_downstream_dependents<S: std::hash::BuildHasher>(
    upstream_dataset_id: Uuid,
    deps_by_dataset: &HashMap<Uuid, Vec<Uuid>, S>,
) -> Result<Vec<Uuid>, AppError> {
    // Build reverse adjacency `u -> [downstream datasets depending on u]`.
    let mut rev_adj: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for (&ds_id, deps) in deps_by_dataset {
        for &up_id in deps {
            rev_adj.entry(up_id).or_default().push(ds_id);
        }
    }
    for list in rev_adj.values_mut() {
        list.sort();
        list.dedup();
    }

    // 1. Collect all reachable downstream nodes from `upstream_dataset_id`.
    let mut affected = BTreeSet::new();
    let mut bfs = VecDeque::new();
    if let Some(direct) = rev_adj.get(&upstream_dataset_id) {
        for &d in direct {
            if d == upstream_dataset_id {
                return Err(AppError::bad(
                    "CYCLE_DETECTED",
                    "cycle detected in derived dataset dependencies",
                ));
            }
            if affected.insert(d) {
                bfs.push_back(d);
            }
        }
    }
    while let Some(curr) = bfs.pop_front() {
        if let Some(next_list) = rev_adj.get(&curr) {
            for &nxt in next_list {
                if nxt == upstream_dataset_id {
                    return Err(AppError::bad(
                        "CYCLE_DETECTED",
                        "cycle detected in derived dataset dependencies",
                    ));
                }
                if affected.insert(nxt) {
                    bfs.push_back(nxt);
                }
            }
        }
    }

    if affected.is_empty() {
        return Ok(Vec::new());
    }

    // 2. Kahn's topological sort over the induced subgraph `affected`.
    let mut in_degree: BTreeMap<Uuid, usize> = BTreeMap::new();
    for &node in &affected {
        let deg = deps_by_dataset
            .get(&node)
            .map_or(0, |ups| ups.iter().filter(|u| affected.contains(u)).count());
        in_degree.insert(node, deg);
    }

    let mut ready: VecDeque<Uuid> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut ordered = Vec::with_capacity(affected.len());

    while let Some(u) = ready.pop_front() {
        ordered.push(u);
        if let Some(children) = rev_adj.get(&u) {
            for &v in children {
                if let Some(deg) = in_degree.get_mut(&v) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.push_back(v);
                    }
                }
            }
        }
    }

    if ordered.len() != affected.len() {
        return Err(AppError::bad(
            "CYCLE_DETECTED",
            "cycle detected among downstream derived datasets",
        ));
    }
    Ok(ordered)
}

fn append_source_ddl(out: &mut String, entry: &CatalogDatasetEntry) {
    let mut cols: Vec<_> = entry.columns.values().collect();
    cols.sort_by_key(|c| c.ordinal);
    let _ = writeln!(out, "CREATE TABLE {} (", entry.name);
    for (idx, col) in cols.iter().enumerate() {
        let comma = if idx.saturating_add(1) < cols.len() {
            ","
        } else {
            ""
        };
        let _ = writeln!(out, "  {} {}{comma}", col.column_name, col.data_type);
    }
    out.push_str(");\n");
}

fn parse_transform_sql_response(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if let Some(json_slice) = extract_json_object(trimmed)
        && let Ok(parsed) = serde_json::from_str::<LlmTransformDraft>(json_slice)
    {
        let clean = strip_trailing_semicolons(parsed.sql.trim());
        if !clean.is_empty() {
            return Ok(clean);
        }
    }
    // Fallback: strip optional ```sql ... ``` fences if model emitted raw SQL
    let unfenced = trimmed
        .strip_prefix("```sql")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |s| s.trim_end_matches("```").trim());
    let clean = strip_trailing_semicolons(unfenced);
    if clean.is_empty() {
        return Err(AppError::Llm(String::from(
            "transform model returned empty SQL",
        )));
    }
    Ok(clean)
}

fn prepare_transform_guards(
    sql_text: &str,
    source_dataset_ids: &[Uuid],
    catalog: &TenantCatalog,
) -> Result<(String, String, Vec<Uuid>, crate::sql_guard::GuardedSql), AppError> {
    let clean_sql = strip_trailing_semicolons(sql_text.trim());
    if clean_sql.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "transform sql_text cannot be empty",
        ));
    }
    for &ds_id in source_dataset_ids {
        if catalog.dataset_by_id(ds_id).is_none() {
            return Err(AppError::NotFound("DATASET_NOT_FOUND"));
        }
    }

    // Sidecar `POST /lake/materialize` runs with `enable_external_access=false` and only allows `lake."tenant_<id>".*`!
    let mat_guarded = guard_for_materialize(&clean_sql, catalog)?;
    if mat_guarded.uses_platform_views {
        return Err(AppError::bad(
            "SQL_REJECTED",
            "DERIVED_VIEW transform SQL may only reference tenant lake datasets, not ax.* views",
        ));
    }
    if mat_guarded.referenced_datasets.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "transform SQL must reference at least one tenant dataset",
        ));
    }

    let dep_ids = mat_guarded.referenced_datasets.clone();

    let preview_guarded = guard(
        &clean_sql,
        catalog,
        &GuardOptions {
            max_rows: TRANSFORM_PREVIEW_MAX_ROWS,
            allow_live: false,
            params: BTreeMap::new(),
        },
    )?;

    Ok((clean_sql, mat_guarded.sql, dep_ids, preview_guarded))
}

fn build_transform_preview(
    out_columns: &[QueryColumn],
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    source_dataset_ids: &[Uuid],
    depends_on: &[Uuid],
    catalog: &TenantCatalog,
) -> TransformPreview {
    let mut source_cols_canonical: BTreeMap<String, String> = BTreeMap::new();
    let effective_sources = if source_dataset_ids.is_empty() {
        depends_on
    } else {
        source_dataset_ids
    };
    for &ds_id in effective_sources {
        if let Some(entry) = catalog.dataset_by_id(ds_id) {
            for meta in entry.columns.values() {
                source_cols_canonical
                    .entry(meta.column_name.to_ascii_lowercase())
                    .or_insert_with(|| meta.column_name.clone());
            }
        }
    }

    let out_lower: BTreeSet<String> = out_columns
        .iter()
        .map(|c| c.name.to_ascii_lowercase())
        .collect();

    let mut added_columns = Vec::new();
    for col in out_columns {
        if !source_cols_canonical.contains_key(&col.name.to_ascii_lowercase()) {
            added_columns.push(col.name.clone());
        }
    }

    let mut removed_columns = Vec::new();
    for (lower_k, orig_name) in &source_cols_canonical {
        if !out_lower.contains(lower_k) {
            removed_columns.push(orig_name.clone());
        }
    }

    let row_count_estimate = i64::try_from(rows.len()).unwrap_or(0);
    TransformPreview {
        columns: out_columns.to_vec(),
        rows,
        row_count_estimate,
        added_columns,
        removed_columns,
    }
}
