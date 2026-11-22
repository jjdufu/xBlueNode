//! PostgreSQL (`PgStore`) implementation of [`AnalyticsRepository`] for Step 04 (`AxAnalytics`).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    AskCitation, AskFeedbackKind, AskIntent, AskTurnRecord, CreateMetricRequest,
    CreateSavedQueryRequest, FewShotTurn, LinkedColumnHit, LinkedMetricHit, LinkedObjects,
    MetricDef, MetricDimension, NewAskTurnInput, NewQueryHistoryEntry, NewTransformDraftInput,
    QueryHistoryEntry, QueryHistoryKind, QueryHistoryListQuery, QueryHistoryStatus, QueryParamSpec,
    SavedQuery, SavedQueryKind, SchemaRecallBundle, TransformDraft, TransformDraftStatus,
    TransformPreview, UpdateMetricRequest, UpdateSavedQueryRequest, ds_column_embedding_id,
};
use axpoint_domain::axhub::{ColumnProfile, Dataset, DatasetStatus, PaginatedList, SchemaPolicy};
use axpoint_domain::enums::{DataType, JobKind, PiiCategory, RollupGrain, SourceKind, SyncMode};
use axpoint_domain::graph::normalize_name;
use axpoint_domain::platform::NewJob;
use axpoint_domain::ports::AnalyticsRepository;
use chrono::{DateTime, Utc};
use pgvector::Vector;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::tenant::begin_tenant_tx;

const HISTORY_SQL_MAX_CHARS: usize = 4_000;
const EMBED_TEXT_MAX_CHARS: usize = 2_000;

const SQL_NON_COLUMN_WORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "XOR",
    "IN",
    "IS",
    "NULL",
    "TRUE",
    "FALSE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "DISTINCT",
    "AS",
    "CAST",
    "TRY_CAST",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "SIMILAR",
    "TO",
    "ESCAPE",
    "EXISTS",
    "ALL",
    "ANY",
    "SOME",
    "FILTER",
    "OVER",
    "PARTITION",
    "BY",
    "GROUP",
    "HAVING",
    "ORDER",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "INTERVAL",
    "EXTRACT",
    "YEAR",
    "YEARS",
    "QUARTER",
    "QUARTERS",
    "MONTH",
    "MONTHS",
    "WEEK",
    "WEEKS",
    "DAY",
    "DAYS",
    "HOUR",
    "HOURS",
    "MINUTE",
    "MINUTES",
    "SECOND",
    "SECONDS",
    "EPOCH",
    "INT",
    "INT2",
    "INT4",
    "INT8",
    "INTEGER",
    "BIGINT",
    "SMALLINT",
    "TINYINT",
    "HUGEINT",
    "FLOAT",
    "FLOAT4",
    "FLOAT8",
    "REAL",
    "DOUBLE",
    "PRECISION",
    "DECIMAL",
    "NUMERIC",
    "VARCHAR",
    "TEXT",
    "STRING",
    "CHAR",
    "BOOLEAN",
    "BOOL",
    "DATE",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "UUID",
    "JSON",
    "BLOB",
    "BYTEA",
    "SUM",
    "AVG",
    "COUNT",
    "MIN",
    "MAX",
    "MEDIAN",
    "STDDEV",
    "VARIANCE",
    "NULLIF",
    "COALESCE",
    "IFNULL",
    "GREATEST",
    "LEAST",
    "ABS",
    "ROUND",
    "CEIL",
    "CEILING",
    "FLOOR",
    "APPROX_COUNT_DISTINCT",
    "QUANTILE_CONT",
    "QUANTILE_DISC",
    "DATE_TRUNC",
    "DATE_PART",
];

const BILINGUAL_CONCEPT_GROUPS: &[(&[&str], &[&str])] = &[
    (
        &[
            "金额",
            "总额",
            "总金额",
            "销售额",
            "成交额",
            "实付",
            "费用",
            "单价",
            "价格",
            "营收",
            "收入",
        ],
        &["amount", "total", "price", "fee", "revenue", "cost", "gmv"],
    ),
    (
        &["已支付", "支付", "已付款", "付款"],
        &["paid", "status", "pay"],
    ),
    (
        &["未支付", "待支付", "未付款", "待处理"],
        &["open", "unpaid", "pending", "status"],
    ),
    (
        &["已发货", "发货", "配送", "出库"],
        &["shipped", "status", "delivery"],
    ),
    (&["状态"], &["status", "state"]),
    (&["订单号", "单号", "订单编号"], &["order_id"]),
    (&["订单"], &["order", "orders"]),
    (&["客户", "买家", "顾客"], &["customer", "client", "buyer"]),
    (
        &["手机", "手机号", "电话", "联系方式"],
        &["phone", "mobile", "tel"],
    ),
    (&["仓库", "仓"], &["warehouse", "warehouse_code"]),
    (
        &[
            "区域", "地区", "大区", "城市", "华东", "华南", "华北", "华中", "西南", "西北",
        ],
        &["region", "area", "city"],
    ),
    (
        &[
            "时间", "日期", "月份", "按月", "按天", "趋势", "同比", "环比",
        ],
        &["updated_at", "created_at", "date", "ts"],
    ),
    (
        &["数量", "销量", "件数", "库存", "产量"],
        &["quantity", "qty", "inventory", "output"],
    ),
];

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn is_foreign_key_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23503")
}

fn is_valid_ident_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn usize_to_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

const fn saved_query_kind_as_str(kind: SavedQueryKind) -> &'static str {
    match kind {
        SavedQueryKind::Sql => "SQL",
        SavedQueryKind::Pivot => "PIVOT",
        SavedQueryKind::Metric => "METRIC",
        SavedQueryKind::Ask => "ASK",
    }
}

fn parse_saved_query_kind(raw: &str) -> Result<SavedQueryKind, AppError> {
    match raw {
        "SQL" => Ok(SavedQueryKind::Sql),
        "PIVOT" => Ok(SavedQueryKind::Pivot),
        "METRIC" => Ok(SavedQueryKind::Metric),
        "ASK" => Ok(SavedQueryKind::Ask),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown SavedQueryKind: {raw}"),
        )),
    }
}

fn parse_query_history_kind(raw: &str) -> Result<QueryHistoryKind, AppError> {
    match raw {
        "SQL" => Ok(QueryHistoryKind::Sql),
        "PIVOT" => Ok(QueryHistoryKind::Pivot),
        "METRIC" => Ok(QueryHistoryKind::Metric),
        "ASK" => Ok(QueryHistoryKind::Ask),
        "TRANSFORM" => Ok(QueryHistoryKind::Transform),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown QueryHistoryKind: {raw}"),
        )),
    }
}

fn parse_query_history_status(raw: &str) -> Result<QueryHistoryStatus, AppError> {
    match raw {
        "SUCCEEDED" => Ok(QueryHistoryStatus::Succeeded),
        "FAILED" => Ok(QueryHistoryStatus::Failed),
        "REJECTED" => Ok(QueryHistoryStatus::Rejected),
        "TIMEOUT" => Ok(QueryHistoryStatus::Timeout),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown QueryHistoryStatus: {raw}"),
        )),
    }
}

fn parse_ask_intent(raw: &str) -> Result<AskIntent, AppError> {
    match raw {
        "DOC" => Ok(AskIntent::Doc),
        "DATA" => Ok(AskIntent::Data),
        "HYBRID" => Ok(AskIntent::Hybrid),
        "CHITCHAT" => Ok(AskIntent::Chitchat),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown AskIntent: {raw}"),
        )),
    }
}

fn parse_ask_feedback_kind(raw: &str) -> Result<AskFeedbackKind, AppError> {
    match raw {
        "UP" => Ok(AskFeedbackKind::Up),
        "DOWN" => Ok(AskFeedbackKind::Down),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown AskFeedbackKind: {raw}"),
        )),
    }
}

fn parse_transform_draft_status(raw: &str) -> Result<TransformDraftStatus, AppError> {
    match raw {
        "DRAFT" => Ok(TransformDraftStatus::Draft),
        "APPLIED" => Ok(TransformDraftStatus::Applied),
        "DISCARDED" => Ok(TransformDraftStatus::Discarded),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown TransformDraftStatus: {raw}"),
        )),
    }
}

fn parse_schema_policy(raw: &str) -> Result<SchemaPolicy, AppError> {
    match raw {
        "STRICT" => Ok(SchemaPolicy::Strict),
        "ADDITIVE" => Ok(SchemaPolicy::Additive),
        "EVOLVE" => Ok(SchemaPolicy::Evolve),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown SchemaPolicy: {raw}"),
        )),
    }
}

fn parse_dataset_status(raw: &str) -> Result<DatasetStatus, AppError> {
    match raw {
        "DRAFT" => Ok(DatasetStatus::Draft),
        "READY" => Ok(DatasetStatus::Ready),
        "SYNCING" => Ok(DatasetStatus::Syncing),
        "FAILED" => Ok(DatasetStatus::Failed),
        "DISABLED" => Ok(DatasetStatus::Disabled),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown DatasetStatus: {raw}"),
        )),
    }
}

fn is_sql_keyword_or_builtin(ident: &str) -> bool {
    let upper = ident.to_ascii_uppercase();
    SQL_NON_COLUMN_WORDS.contains(&upper.as_str())
}

/// Extracts lowercased column identifiers from an SQL expression or filter predicate.
///
/// Ignores single-quoted `'...'` literals, `:param` placeholders, `::type` cast suffixes,
/// `qualifier.` prefixes, function names followed by `(`, numeric literals, and SQL keywords.
fn extract_sql_column_identifiers(sql: &str) -> BTreeSet<String> {
    let mut cols = BTreeSet::new();
    let mut chars = sql.chars().peekable();
    let mut after_double_colon = false;

    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '\'' {
            skip_single_quoted(&mut chars);
            after_double_colon = false;
            continue;
        }
        if ch == '"' {
            let ident = read_double_quoted(&mut chars);
            let trimmed = ident.trim();
            if !trimmed.is_empty() && !after_double_colon {
                cols.insert(trimmed.to_ascii_lowercase());
            }
            after_double_colon = false;
            continue;
        }
        if ch == ':' {
            after_double_colon = consume_colon_token(&mut chars);
            continue;
        }
        if ch.is_ascii_digit() {
            skip_numeric_literal(&mut chars);
            after_double_colon = false;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let ident = read_unquoted_ident(&mut chars);
            let next_non_ws = peek_next_non_ws(&chars);
            let is_fn_or_qualifier = matches!(next_non_ws, Some('(' | '.'));
            if !after_double_colon && !is_fn_or_qualifier && !is_sql_keyword_or_builtin(&ident) {
                cols.insert(ident.to_ascii_lowercase());
            }
            after_double_colon = false;
            continue;
        }
        chars.next();
        after_double_colon = false;
    }

    cols
}

fn skip_single_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    chars.next();
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

fn read_double_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    chars.next();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                out.push('"');
                chars.next();
            } else {
                break;
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn consume_colon_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    chars.next();
    if chars.peek() == Some(&':') {
        chars.next();
        return true;
    }
    while let Some(&nc) = chars.peek() {
        if nc.is_ascii_alphanumeric() || nc == '_' {
            chars.next();
        } else {
            break;
        }
    }
    false
}

fn skip_numeric_literal(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    chars.next();
    while let Some(&nc) = chars.peek() {
        if nc.is_ascii_alphanumeric() || nc == '.' || nc == '_' {
            chars.next();
        } else {
            break;
        }
    }
}

fn read_unquoted_ident(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut ident = String::new();
    while let Some(&nc) = chars.peek() {
        if nc.is_ascii_alphanumeric() || nc == '_' {
            ident.push(nc);
            chars.next();
        } else {
            break;
        }
    }
    ident
}

fn peek_next_non_ws(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    let mut clone_iter = chars.clone();
    while let Some(&c) = clone_iter.peek() {
        if c.is_whitespace() {
            clone_iter.next();
        } else {
            return Some(c);
        }
    }
    None
}

fn compute_metric_broken(metric: &MetricDef, available_cols: Option<&BTreeSet<String>>) -> bool {
    let Some(cols) = available_cols else {
        return true;
    };
    if cols.is_empty() {
        return true;
    }

    if let Some(ref tc) = metric.time_column {
        let trimmed = tc.trim();
        if !trimmed.is_empty() && !cols.contains(&trimmed.to_ascii_lowercase()) {
            return true;
        }
    }

    for dim in &metric.dimensions {
        let dc = dim.column.trim();
        if dc.is_empty() || !cols.contains(&dc.to_ascii_lowercase()) {
            return true;
        }
    }

    for col in extract_sql_column_identifiers(&metric.expression) {
        if !cols.contains(&col) {
            return true;
        }
    }

    if !metric.filter_sql.trim().is_empty() {
        for col in extract_sql_column_identifiers(&metric.filter_sql) {
            if !cols.contains(&col) {
                return true;
            }
        }
    }

    false
}

fn decode_metric_row(row: &sqlx::postgres::PgRow, broken: bool) -> Result<MetricDef, AppError> {
    let default_grain = RollupGrain::from_str(&row.get::<String, _>("default_grain"))?;
    let dims_val: serde_json::Value = row.get("dimensions");
    let dimensions: Vec<MetricDimension> = serde_json::from_value(dims_val).unwrap_or_default();

    Ok(MetricDef {
        id: row.get("id"),
        name: row.get("name"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        dataset_id: row.get("dataset_id"),
        expression: row.get("expression"),
        filter_sql: row.get("filter_sql"),
        time_column: row.get("time_column"),
        default_grain,
        dimensions,
        unit: row.get("unit"),
        format: row.get("format"),
        synonyms: row.get("synonyms"),
        certified: row.get("certified"),
        schema_version: row.get("schema_version"),
        broken,
        owner_user_id: row.get("owner_user_id"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_saved_query_row(row: &sqlx::postgres::PgRow) -> Result<SavedQuery, AppError> {
    let kind = parse_saved_query_kind(&row.get::<String, _>("kind"))?;
    let params_val: serde_json::Value = row.get("parameters");
    let parameters: Vec<QueryParamSpec> = serde_json::from_value(params_val).unwrap_or_default();

    Ok(SavedQuery {
        id: row.get("id"),
        name: row.get("name"),
        kind,
        spec: row.get("spec"),
        parameters,
        owner_user_id: row.get("owner_user_id"),
        shared: row.get("shared"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_query_history_row(row: &sqlx::postgres::PgRow) -> Result<QueryHistoryEntry, AppError> {
    let kind = parse_query_history_kind(&row.get::<String, _>("kind"))?;
    let status = parse_query_history_status(&row.get::<String, _>("status"))?;

    Ok(QueryHistoryEntry {
        id: row.get("id"),
        user_id: row.get("user_id"),
        kind,
        sql_text: row.get("sql_text"),
        status,
        row_count: row.get("row_count"),
        duration_ms: row.get("duration_ms"),
        error_code: row.get("error_code"),
        created_at: row.get("created_at"),
    })
}

fn decode_ask_turn_row(row: &sqlx::postgres::PgRow) -> Result<AskTurnRecord, AppError> {
    let intent = parse_ask_intent(&row.get::<String, _>("intent"))?;
    let linked_val: serde_json::Value = row.get("linked_objects");
    let linked_objects: LinkedObjects = serde_json::from_value(linked_val).unwrap_or_default();
    let citations_val: serde_json::Value = row.get("citations");
    let citations: Vec<AskCitation> = serde_json::from_value(citations_val).unwrap_or_default();
    let raw_fb: Option<String> = row.get("feedback");
    let feedback = raw_fb.as_deref().map(parse_ask_feedback_kind).transpose()?;
    let raw_gm: String = row.get("grounding_mode");
    let grounding_mode = raw_gm
        .parse::<axpoint_domain::enums::GroundingMode>()
        .unwrap_or_default();
    let verified_numbers = u32::try_from(row.get::<i32, _>("verified_numbers").max(0)).unwrap_or(0);
    let verified_citations =
        u32::try_from(row.get::<i32, _>("verified_citations").max(0)).unwrap_or(0);

    Ok(AskTurnRecord {
        id: row.get("id"),
        session_id: row.get("session_id"),
        user_id: row.get("user_id"),
        question: row.get("question"),
        intent,
        linked_objects,
        sql_text: row.get("sql_text"),
        result_preview: row.get("result_preview"),
        answer: row.get("answer"),
        citations,
        feedback,
        corrected_sql: row.get("corrected_sql"),
        model_name: row.get("model_name"),
        created_at: row.get("created_at"),
        grounding_mode,
        verified_numbers,
        verified_citations,
        abstained: row.get("abstained"),
        slot_payload: row.get("slot_payload"),
    })
}

fn decode_transform_draft_row(row: &sqlx::postgres::PgRow) -> Result<TransformDraft, AppError> {
    let status = parse_transform_draft_status(&row.get::<String, _>("status"))?;
    let preview_val: serde_json::Value = row.get("preview");
    let preview: TransformPreview = serde_json::from_value(preview_val).unwrap_or_default();

    Ok(TransformDraft {
        id: row.get("id"),
        user_id: row.get("user_id"),
        instruction: row.get("instruction"),
        source_dataset_ids: row.get("source_dataset_ids"),
        sql_text: row.get("sql_text"),
        preview,
        target_dataset_name: row.get("target_dataset_name"),
        status,
        dataset_id: row.get("dataset_id"),
        created_at: row.get("created_at"),
    })
}

fn decode_dataset_row(row: &sqlx::postgres::PgRow) -> Result<Dataset, AppError> {
    let source_kind = SourceKind::from_str(&row.get::<String, _>("source_kind"))?;
    let sync_mode = SyncMode::from_str(&row.get::<String, _>("sync_mode"))?;
    let schema_policy = parse_schema_policy(&row.get::<String, _>("schema_policy"))?;
    let status = parse_dataset_status(&row.get::<String, _>("status"))?;
    let schema_val: serde_json::Value = row.get("schema_json");
    let schema_json: Vec<ColumnProfile> = serde_json::from_value(schema_val).unwrap_or_default();

    Ok(Dataset {
        id: row.get("id"),
        connection_id: row.get("connection_id"),
        name: row.get("name"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        source_kind,
        source_ref: row.get("source_ref"),
        sync_mode,
        primary_key: row.get("primary_key"),
        cursor_column: row.get("cursor_column"),
        cursor_value: row.get("cursor_value"),
        delete_column: row.get("delete_column"),
        schema_policy,
        lake_schema: row.get("lake_schema"),
        lake_table: row.get("lake_table"),
        schema_json,
        schema_version: row.get("schema_version"),
        row_count: row.get("row_count"),
        last_snapshot_id: row.get("last_snapshot_id"),
        last_synced_at: row.get("last_synced_at"),
        schedule_cron: row.get("schedule_cron"),
        tags: row.get("tags"),
        owner_user_id: row.get("owner_user_id"),
        status,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// Loads authoritative current lowercased column sets per dataset ID.
///
/// When `ds_datasets.schema_json` is non-empty, its `ColumnProfile` entries define the active physical
/// schema (so columns removed from `schema_json` are immediately detected as missing even if stale
/// metadata rows remain in `ds_column_meta`). When `schema_json` is empty, falls back to `ds_column_meta`.
async fn load_dataset_column_sets_tx(
    tx: &mut Transaction<'_, Postgres>,
    dataset_ids: &[Uuid],
) -> Result<HashMap<Uuid, BTreeSet<String>>, AppError> {
    if dataset_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let ds_rows = sqlx::query("SELECT id, schema_json FROM ds_datasets WHERE id = ANY($1)")
        .bind(dataset_ids)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load ds_datasets schema_json failed: {err}")))?;

    let meta_rows = sqlx::query(
        "SELECT dataset_id, column_name FROM ds_column_meta WHERE dataset_id = ANY($1)",
    )
    .bind(dataset_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ds_column_meta names failed: {err}")))?;

    let mut meta_by_ds: HashMap<Uuid, BTreeSet<String>> = HashMap::new();
    for r in meta_rows {
        let ds_id: Uuid = r.get("dataset_id");
        let cname: String = r.get("column_name");
        meta_by_ds
            .entry(ds_id)
            .or_default()
            .insert(cname.trim().to_ascii_lowercase());
    }

    let mut out: HashMap<Uuid, BTreeSet<String>> = HashMap::with_capacity(ds_rows.len());
    for r in ds_rows {
        let ds_id: Uuid = r.get("id");
        let schema_val: serde_json::Value = r.get("schema_json");
        let schema_cols = extract_schema_json_col_names(&schema_val);
        if schema_cols.is_empty() {
            out.insert(ds_id, meta_by_ds.remove(&ds_id).unwrap_or_default());
        } else {
            out.insert(ds_id, schema_cols);
        }
    }

    Ok(out)
}

fn extract_schema_json_col_names(val: &serde_json::Value) -> BTreeSet<String> {
    let Some(items) = val.as_array() else {
        return BTreeSet::new();
    };
    let mut set = BTreeSet::new();
    for item in items {
        if let Some(name) = item.get("name").and_then(serde_json::Value::as_str) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                set.insert(trimmed.to_ascii_lowercase());
            }
        }
    }
    set
}

fn build_metric_embed_hash(display_name: &str, description: &str, synonyms: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(display_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(description.as_bytes());
    hasher.update(b"\0");
    for syn in synonyms {
        hasher.update(syn.as_bytes());
        hasher.update(b"\x1f");
    }
    let full = hex::encode(hasher.finalize());
    full.chars().take(16).collect()
}

fn build_metric_embed_text(
    name: &str,
    display_name: &str,
    description: &str,
    synonyms: &[String],
) -> String {
    let mut parts = vec![format!("指标：{display_name} ({name})")];
    if !description.trim().is_empty() {
        parts.push(format!("描述：{}", description.trim()));
    }
    let syns: Vec<&str> = synonyms
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if !syns.is_empty() {
        parts.push(format!("同义词：{}", syns.join("、")));
    }
    parts.join(" ").chars().take(EMBED_TEXT_MAX_CHARS).collect()
}

fn build_column_embed_text(
    ds_name: &str,
    ds_display: &str,
    col_name: &str,
    description: &str,
    business_term: &str,
    unit: &str,
) -> String {
    let ds_label = if ds_display.trim().is_empty() {
        ds_name
    } else {
        ds_display.trim()
    };
    let mut parts = vec![format!("数据集：{ds_label} ({ds_name}) 列：{col_name}")];
    if !description.trim().is_empty() {
        parts.push(format!("描述：{}", description.trim()));
    }
    if !business_term.trim().is_empty() {
        parts.push(format!("业务术语：{}", business_term.trim()));
    }
    if !unit.trim().is_empty() {
        parts.push(format!("单位：{}", unit.trim()));
    }
    parts.join(" ").chars().take(EMBED_TEXT_MAX_CHARS).collect()
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_cjk_stop_particle(ch: char) -> bool {
    matches!(
        ch,
        '的' | '了'
            | '吗'
            | '呢'
            | '吧'
            | '啊'
            | '呀'
            | '是'
            | '有'
            | '在'
            | '和'
            | '与'
            | '请'
            | '问'
    )
}

fn extract_ascii_words(text: &str) -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            push_filtered_ascii_word(&mut words, &current);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_filtered_ascii_word(&mut words, &current);
    }
    words
}

fn push_filtered_ascii_word(words: &mut BTreeSet<String>, token: &str) {
    const STOP: &[&str] = &[
        "the", "is", "are", "of", "in", "to", "for", "and", "or", "on", "at", "by", "what", "how",
        "many", "much", "from", "with",
    ];
    if token.len() >= 2 && !STOP.contains(&token) {
        words.insert(String::from(token));
    }
    for part in token.split('_') {
        if part.len() >= 2 && !STOP.contains(&part) {
            words.insert(String::from(part));
        }
    }
}

fn extract_cjk_bigrams_and_chars(text: &str) -> (BTreeSet<String>, BTreeSet<char>) {
    let mut bigrams = BTreeSet::new();
    let mut chars_set = BTreeSet::new();
    let mut prev: Option<char> = None;

    for ch in text.chars() {
        if is_cjk_char(ch) {
            if is_cjk_stop_particle(ch) {
                prev = None;
                continue;
            }
            chars_set.insert(ch);
            if let Some(p) = prev {
                let mut bg = String::with_capacity(8);
                bg.push(p);
                bg.push(ch);
                if bg != "多少" && bg != "什么" && bg != "哪些" && bg != "怎么" {
                    bigrams.insert(bg);
                }
            }
            prev = Some(ch);
        } else {
            prev = None;
        }
    }
    (bigrams, chars_set)
}

fn compute_question_similarity(query: &str, candidate: &str, pg_fts_rank: f64) -> f64 {
    let q_trim = query.trim();
    let c_trim = candidate.trim();
    if q_trim.is_empty() || c_trim.is_empty() {
        return 0.0;
    }
    if q_trim.eq_ignore_ascii_case(c_trim) {
        return 1.0;
    }

    let (q_bg, q_ch) = extract_cjk_bigrams_and_chars(q_trim);
    let (c_bg, c_ch) = extract_cjk_bigrams_and_chars(c_trim);
    let q_words = extract_ascii_words(q_trim);
    let c_words = extract_ascii_words(c_trim);

    let bg_score = if !q_bg.is_empty() && !c_bg.is_empty() {
        let inter = q_bg.intersection(&c_bg).count();
        let denom = q_bg.len().max(1);
        usize_to_f64(inter) / usize_to_f64(denom)
    } else {
        0.0
    };

    let ch_score = if !q_ch.is_empty() && !c_ch.is_empty() {
        let inter = q_ch.intersection(&c_ch).count();
        let union_len = q_ch.union(&c_ch).count().max(1);
        usize_to_f64(inter) / usize_to_f64(union_len)
    } else {
        0.0
    };

    let word_score = if !q_words.is_empty() && !c_words.is_empty() {
        let inter = q_words.intersection(&c_words).count();
        let denom = q_words.len().max(1);
        usize_to_f64(inter) / usize_to_f64(denom)
    } else {
        0.0
    };

    let fts_norm = if pg_fts_rank.is_finite() {
        pg_fts_rank.clamp(0.0, 1.0)
    } else {
        0.0
    };

    let lexical = (0.55 * bg_score) + (0.20 * ch_score) + (0.25 * word_score);
    let combined = lexical.max(0.65 * lexical + 0.35 * fts_norm);
    combined.clamp(0.0, 1.0)
}

#[derive(Debug, Default)]
struct ExpandedQueryContext {
    raw_lower: String,
    phrases: BTreeSet<String>,
    exact_col_hints: BTreeSet<String>,
    ascii_tokens: BTreeSet<String>,
    cjk_bigrams: BTreeSet<String>,
    cjk_chars: BTreeSet<char>,
}

fn build_expanded_query_context(
    question: &str,
    glossary_rows: &[(String, Vec<String>)],
) -> ExpandedQueryContext {
    let raw_lower = question.trim().to_lowercase();
    if raw_lower.is_empty() {
        return ExpandedQueryContext::default();
    }

    let mut phrases = BTreeSet::new();
    phrases.insert(raw_lower.clone());

    let mut exact_col_hints = BTreeSet::new();

    for (term, synonyms) in glossary_rows {
        let term_low = term.trim().to_lowercase();
        let term_hit = !term_low.is_empty() && raw_lower.contains(&term_low);
        let syn_hit = synonyms.iter().any(|s| {
            let sl = s.trim().to_lowercase();
            !sl.is_empty() && raw_lower.contains(&sl)
        });
        if term_hit || syn_hit {
            if !term_low.is_empty() {
                phrases.insert(term_low.clone());
                exact_col_hints.insert(term_low);
            }
            for s in synonyms {
                let sl = s.trim().to_lowercase();
                if !sl.is_empty() {
                    phrases.insert(sl.clone());
                    exact_col_hints.insert(sl);
                }
            }
        }
    }

    for &(zh_triggers, en_targets) in BILINGUAL_CONCEPT_GROUPS {
        let hit_zh = zh_triggers.iter().any(|z| raw_lower.contains(z));
        let hit_en = en_targets.iter().any(|e| raw_lower.contains(e));
        if hit_zh || hit_en {
            for &e in en_targets {
                exact_col_hints.insert(String::from(e));
                phrases.insert(String::from(e));
            }
            for &z in zh_triggers {
                phrases.insert(String::from(z));
            }
        }
    }

    let mut ascii_tokens = BTreeSet::new();
    let mut cjk_bigrams = BTreeSet::new();
    let mut cjk_chars = BTreeSet::new();

    let (q_bg, q_ch) = extract_cjk_bigrams_and_chars(&raw_lower);
    cjk_bigrams.extend(q_bg);
    cjk_chars.extend(q_ch);

    for phrase in &phrases {
        ascii_tokens.extend(extract_ascii_words(phrase));
        if phrase != &raw_lower {
            let (p_bg, _) = extract_cjk_bigrams_and_chars(phrase);
            cjk_bigrams.extend(p_bg);
        }
    }

    ExpandedQueryContext {
        raw_lower,
        phrases,
        exact_col_hints,
        ascii_tokens,
        cjk_bigrams,
        cjk_chars,
    }
}

fn score_dataset_lexical(
    ctx: &ExpandedQueryContext,
    name: &str,
    display_name: &str,
    description: &str,
    tags: &[String],
    pg_fts: f64,
) -> f64 {
    if ctx.raw_lower.is_empty() {
        return 0.0;
    }

    let name_low = name.to_ascii_lowercase();
    let mut best = if pg_fts.is_finite() {
        pg_fts.clamp(0.0, 1.0)
    } else {
        0.0
    };

    if ctx.raw_lower.contains(&name_low) || ctx.exact_col_hints.contains(&name_low) {
        best = best.max(0.95);
    }
    for part in name_low.split('_') {
        if part.len() >= 2 && part != "demo" && ctx.ascii_tokens.contains(part) {
            best = best.max(0.80);
        }
    }

    let combined_text = format!("{display_name} {description} {}", tags.join(" "));
    let text_low = combined_text.to_lowercase();
    if !display_name.trim().is_empty()
        && ctx.raw_lower.contains(&display_name.trim().to_lowercase())
    {
        best = best.max(0.95);
    }

    let (t_bg, t_ch) = extract_cjk_bigrams_and_chars(&text_low);
    if !t_bg.is_empty() && !ctx.cjk_bigrams.is_empty() {
        let hits = t_bg.intersection(&ctx.cjk_bigrams).count();
        if hits > 0 {
            let ratio = usize_to_f64(hits) / usize_to_f64(t_bg.len().clamp(1, 4));
            best = best.max((0.55 + 0.40 * ratio).clamp(0.0, 0.95));
        }
    } else if !t_ch.is_empty() && !ctx.cjk_chars.is_empty() {
        let hits = t_ch.intersection(&ctx.cjk_chars).count();
        if hits >= 2 {
            best = best.max(0.65);
        }
    }

    let t_words = extract_ascii_words(&text_low);
    if !t_words.is_empty() && !ctx.ascii_tokens.is_empty() {
        let hits = t_words.intersection(&ctx.ascii_tokens).count();
        if hits > 0 {
            best = best.max(0.75);
        }
    }

    best.clamp(0.0, 1.0)
}

fn score_column_lexical(ctx: &ExpandedQueryContext, ds_score: f64, col: &LinkedColumnHit) -> f64 {
    if ctx.raw_lower.is_empty() {
        return 0.0;
    }

    let col_low = col.column_name.to_ascii_lowercase();
    let mut direct = 0.0_f64;

    if ctx.raw_lower.contains(&col_low) || ctx.exact_col_hints.contains(&col_low) {
        direct = direct.max(0.90);
    } else {
        for part in col_low.split('_') {
            if part.len() >= 2 && part != "id" && ctx.ascii_tokens.contains(part) {
                direct = direct.max(0.55);
            }
        }
    }

    if !col.business_term.trim().is_empty() {
        let bt_low = col.business_term.trim().to_lowercase();
        if ctx.raw_lower.contains(&bt_low)
            || ctx
                .phrases
                .iter()
                .any(|p| p == &bt_low || bt_low.contains(p))
        {
            direct = direct.max(0.92);
        }
    }

    for sample in &col.sample_values {
        let s_low = sample.trim().to_lowercase();
        if s_low.chars().count() >= 2
            && (ctx.raw_lower.contains(&s_low) || ctx.exact_col_hints.contains(&s_low))
        {
            direct = direct.max(0.85);
        }
    }

    let meta_text = format!("{} {} {}", col.description, col.business_term, col.unit);
    let (m_bg, _) = extract_cjk_bigrams_and_chars(&meta_text);
    if !m_bg.is_empty() && !ctx.cjk_bigrams.is_empty() {
        let hits = m_bg.intersection(&ctx.cjk_bigrams).count();
        if hits > 0 {
            let ratio = usize_to_f64(hits) / usize_to_f64(m_bg.len().clamp(1, 4));
            direct = direct.max((0.65 + 0.30 * ratio).clamp(0.0, 0.95));
        }
    }

    let m_words = extract_ascii_words(&meta_text);
    if !m_words.is_empty() && m_words.intersection(&ctx.ascii_tokens).next().is_some() {
        direct = direct.max(0.75);
    }

    if direct > 0.0 {
        (0.78 * direct + 0.22 * ds_score).clamp(0.0, 1.0)
    } else if ds_score > 0.0 {
        (0.20 * ds_score).clamp(0.0, 0.22)
    } else {
        0.0
    }
}

fn score_metric_lexical(ctx: &ExpandedQueryContext, metric: &MetricDef, pg_fts: f64) -> f64 {
    if ctx.raw_lower.is_empty() {
        return 0.0;
    }

    let mut best = if pg_fts.is_finite() {
        pg_fts.clamp(0.0, 1.0)
    } else {
        0.0
    };

    let name_low = metric.name.to_ascii_lowercase();
    if ctx.raw_lower.contains(&name_low) || ctx.exact_col_hints.contains(&name_low) {
        best = best.max(0.95);
    }

    let name_parts: Vec<&str> = name_low.split('_').filter(|p| p.len() >= 2).collect();
    if !name_parts.is_empty() {
        let matched_parts = name_parts
            .iter()
            .filter(|&&p| ctx.ascii_tokens.contains(p))
            .count();
        if matched_parts > 0 {
            let ratio = usize_to_f64(matched_parts) / usize_to_f64(name_parts.len());
            best = best.max((0.55 + 0.40 * ratio).clamp(0.0, 0.95));
        }
    }

    let disp_low = metric.display_name.trim().to_lowercase();
    if !disp_low.is_empty() && ctx.raw_lower.contains(&disp_low) {
        best = best.max(0.96);
    }

    for syn in &metric.synonyms {
        let s_low = syn.trim().to_lowercase();
        if !s_low.is_empty() && (ctx.raw_lower.contains(&s_low) || ctx.phrases.contains(&s_low)) {
            best = best.max(0.94);
        }
    }

    let full_text = format!(
        "{} {} {}",
        metric.display_name,
        metric.description,
        metric.synonyms.join(" ")
    );
    let (m_bg, _) = extract_cjk_bigrams_and_chars(&full_text);
    if !m_bg.is_empty() && !ctx.cjk_bigrams.is_empty() {
        let hits = m_bg.intersection(&ctx.cjk_bigrams).count();
        if hits > 0 {
            let ratio = usize_to_f64(hits) / usize_to_f64(m_bg.len().clamp(1, 4));
            best = best.max((0.65 + 0.30 * ratio).clamp(0.0, 0.96));
        }
    }

    best.clamp(0.0, 1.0)
}

#[derive(Debug)]
struct ReadyDatasetRecallRow {
    id: Uuid,
    name: String,
    display_name: String,
    description: String,
    tags: Vec<String>,
    schema_profiles: Vec<ColumnProfile>,
    pg_fts: f64,
}

async fn load_ready_datasets_for_recall_tx(
    tx: &mut Transaction<'_, Postgres>,
    question: &str,
    restrict_datasets: &[Uuid],
) -> Result<Vec<ReadyDatasetRecallRow>, AppError> {
    let q_opt = (!question.trim().is_empty()).then(|| question.trim());
    let rows = if restrict_datasets.is_empty() {
        sqlx::query(
            "SELECT id, name, display_name, description, tags, schema_json,
                    CASE WHEN $1::text IS NULL THEN 0.0
                         ELSE ts_rank_cd(fts, websearch_to_tsquery('simple', $1))::float8
                    END AS pg_fts
               FROM ds_datasets
              WHERE status = 'READY' OR last_snapshot_id IS NOT NULL
              ORDER BY name ASC",
        )
        .bind(q_opt)
        .fetch_all(&mut **tx)
        .await
    } else {
        sqlx::query(
            "SELECT id, name, display_name, description, tags, schema_json,
                    CASE WHEN $1::text IS NULL THEN 0.0
                         ELSE ts_rank_cd(fts, websearch_to_tsquery('simple', $1))::float8
                    END AS pg_fts
               FROM ds_datasets
              WHERE (status = 'READY' OR last_snapshot_id IS NOT NULL)
                AND id = ANY($2)
              ORDER BY name ASC",
        )
        .bind(q_opt)
        .bind(restrict_datasets)
        .fetch_all(&mut **tx)
        .await
    }
    .map_err(|err| AppError::Database(format!("load ready datasets for recall failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let schema_val: serde_json::Value = r.get("schema_json");
            let schema_profiles: Vec<ColumnProfile> =
                serde_json::from_value(schema_val).unwrap_or_default();
            ReadyDatasetRecallRow {
                id: r.get("id"),
                name: r.get("name"),
                display_name: r.get("display_name"),
                description: r.get("description"),
                tags: r.get("tags"),
                schema_profiles,
                pg_fts: r.try_get("pg_fts").unwrap_or(0.0),
            }
        })
        .collect())
}

async fn load_candidate_columns_tx(
    tx: &mut Transaction<'_, Postgres>,
    datasets: &[ReadyDatasetRecallRow],
) -> Result<Vec<LinkedColumnHit>, AppError> {
    if datasets.is_empty() {
        return Ok(Vec::new());
    }
    let ds_ids: Vec<Uuid> = datasets.iter().map(|d| d.id).collect();
    let meta_rows = sqlx::query(
        "SELECT dataset_id, column_name, data_type, pii_category, description, business_term, unit, ordinal
           FROM ds_column_meta
          WHERE dataset_id = ANY($1)
          ORDER BY dataset_id, ordinal ASC, column_name ASC",
    )
    .bind(&ds_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ds_column_meta for recall failed: {err}")))?;

    let mut meta_by_ds: HashMap<Uuid, Vec<sqlx::postgres::PgRow>> = HashMap::new();
    for row in meta_rows {
        meta_by_ds
            .entry(row.get("dataset_id"))
            .or_default()
            .push(row);
    }

    let mut hits = Vec::new();
    for ds in datasets {
        let profile_map: HashMap<String, &ColumnProfile> = ds
            .schema_profiles
            .iter()
            .map(|p| (p.name.to_ascii_lowercase(), p))
            .collect();
        let active_schema_names: Option<BTreeSet<String>> =
            (!ds.schema_profiles.is_empty()).then(|| {
                ds.schema_profiles
                    .iter()
                    .map(|p| p.name.to_ascii_lowercase())
                    .collect()
            });

        let mut seen_cols = BTreeSet::new();
        if let Some(mrows) = meta_by_ds.get(&ds.id) {
            for r in mrows {
                let col_name: String = r.get("column_name");
                let col_low = col_name.to_ascii_lowercase();
                if let Some(ref active) = active_schema_names
                    && !active.contains(&col_low)
                {
                    continue;
                }
                let data_type = DataType::from_str(&r.get::<String, _>("data_type"))?;
                let pii_category = PiiCategory::from_str(&r.get::<String, _>("pii_category"))?;
                let samples: Vec<String> = profile_map.get(&col_low).map_or_else(Vec::new, |p| {
                    p.sample_values.iter().take(3).cloned().collect()
                });
                seen_cols.insert(col_low);
                hits.push(LinkedColumnHit {
                    dataset_id: ds.id,
                    dataset_name: ds.name.clone(),
                    column_name: col_name,
                    data_type,
                    description: r.get("description"),
                    business_term: r.get("business_term"),
                    unit: r.get("unit"),
                    pii_category,
                    sample_values: samples,
                    fts_score: 0.0,
                    vec_score: 0.0,
                });
            }
        }

        for prof in &ds.schema_profiles {
            let col_low = prof.name.to_ascii_lowercase();
            if seen_cols.insert(col_low) {
                hits.push(LinkedColumnHit {
                    dataset_id: ds.id,
                    dataset_name: ds.name.clone(),
                    column_name: prof.name.clone(),
                    data_type: prof.data_type,
                    description: prof.source_comment.clone(),
                    business_term: String::new(),
                    unit: String::new(),
                    pii_category: prof.pii_category,
                    sample_values: prof.sample_values.iter().take(3).cloned().collect(),
                    fts_score: 0.0,
                    vec_score: 0.0,
                });
            }
        }
    }

    Ok(hits)
}

async fn load_candidate_metrics_tx(
    tx: &mut Transaction<'_, Postgres>,
    question: &str,
    restrict_datasets: &[Uuid],
) -> Result<Vec<(MetricDef, f64)>, AppError> {
    let q_opt = (!question.trim().is_empty()).then(|| question.trim());
    let rows = if restrict_datasets.is_empty() {
        sqlx::query(
            "SELECT id, name, display_name, description, dataset_id, expression, filter_sql,
                    time_column, default_grain, dimensions, unit, format, synonyms, certified,
                    schema_version, owner_user_id, version, created_at, updated_at,
                    CASE WHEN $1::text IS NULL THEN 0.0
                         ELSE ts_rank_cd(fts, websearch_to_tsquery('simple', $1))::float8
                    END AS pg_fts
               FROM an_metrics
              ORDER BY certified DESC, name ASC",
        )
        .bind(q_opt)
        .fetch_all(&mut **tx)
        .await
    } else {
        sqlx::query(
            "SELECT id, name, display_name, description, dataset_id, expression, filter_sql,
                    time_column, default_grain, dimensions, unit, format, synonyms, certified,
                    schema_version, owner_user_id, version, created_at, updated_at,
                    CASE WHEN $1::text IS NULL THEN 0.0
                         ELSE ts_rank_cd(fts, websearch_to_tsquery('simple', $1))::float8
                    END AS pg_fts
               FROM an_metrics
              WHERE dataset_id = ANY($2)
              ORDER BY certified DESC, name ASC",
        )
        .bind(q_opt)
        .bind(restrict_datasets)
        .fetch_all(&mut **tx)
        .await
    }
    .map_err(|err| AppError::Database(format!("load an_metrics for recall failed: {err}")))?;

    let ds_ids: Vec<Uuid> = rows
        .iter()
        .map(|r| r.get::<Uuid, _>("dataset_id"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let col_sets = load_dataset_column_sets_tx(tx, &ds_ids).await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let pg_fts: f64 = r.try_get("pg_fts").unwrap_or(0.0);
        let mut m = decode_metric_row(&r, false)?;
        m.broken = compute_metric_broken(&m, col_sets.get(&m.dataset_id));
        out.push((m, pg_fts));
    }
    Ok(out)
}

async fn load_analytics_vector_scores_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    qvec: &[f32],
) -> Result<(HashMap<Uuid, f64>, HashMap<Uuid, f64>), AppError> {
    let dim_i32 = i32::try_from(qvec.len()).map_err(|err| {
        AppError::bad("INVALID_ARGUMENT", format!("invalid embedding dim: {err}"))
    })?;

    let rows = sqlx::query(
        "SELECT node_type, node_id,
                (1.0 - (embedding <=> $2))::float8 AS sim
           FROM hg_embeddings
          WHERE tenant_id = $1
            AND node_type IN ('DS_COLUMN', 'AN_METRIC')
            AND embedding_dim = $3",
    )
    .bind(tenant_id)
    .bind(Vector::from(qvec.to_vec()))
    .bind(dim_i32)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query analytics hg_embeddings failed: {err}")))?;

    let mut col_scores: HashMap<Uuid, f64> = HashMap::new();
    let mut metric_scores: HashMap<Uuid, f64> = HashMap::new();

    for r in rows {
        let ntype: String = r.get("node_type");
        let nid: Uuid = r.get("node_id");
        let raw_sim: f64 = r.try_get("sim").unwrap_or(0.0);
        let sim = if raw_sim.is_finite() {
            raw_sim.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if ntype == "DS_COLUMN" {
            let entry = col_scores.entry(nid).or_insert(0.0);
            *entry = entry.max(sim);
        } else if ntype == "AN_METRIC" {
            let entry = metric_scores.entry(nid).or_insert(0.0);
            *entry = entry.max(sim);
        }
    }

    Ok((col_scores, metric_scores))
}

async fn match_business_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    question: &str,
) -> Result<Vec<(String, String, String)>, AppError> {
    if question.trim().is_empty() {
        return Ok(Vec::new());
    }

    let norm_q = normalize_name(question);
    let lower_q = question.to_lowercase();
    let rows = sqlx::query(
        "SELECT type_name, node_key, title
           FROM xb_nodes
          ORDER BY type_name ASC, node_key ASC",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load xb_nodes for schema recall failed: {err}")))?;

    let mut matched = Vec::new();
    for r in rows {
        let type_name: String = r.get("type_name");
        let node_key: String = r.get("node_key");
        let title: String = r.get("title");

        let norm_title = normalize_name(&title);
        let norm_key = normalize_name(&node_key);

        let title_matched = norm_title.chars().count() >= 2
            && (norm_q.contains(&norm_title) || lower_q.contains(&norm_title));
        let key_matched = norm_key.chars().count() >= 2
            && (norm_q.contains(&norm_key) || lower_q.contains(&norm_key));

        if title_matched || key_matched {
            matched.push((type_name, node_key, title));
        }
    }
    Ok(matched)
}

fn parse_depends_on_uuids(source_ref: &serde_json::Value) -> BTreeSet<Uuid> {
    let Some(arr) = source_ref
        .get("depends_on")
        .and_then(serde_json::Value::as_array)
    else {
        return BTreeSet::new();
    };
    arr.iter()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect()
}

fn topological_sort_downstream(
    upstream_dataset_id: Uuid,
    derived_datasets: Vec<Dataset>,
) -> Result<Vec<Dataset>, AppError> {
    if derived_datasets.is_empty() {
        return Ok(Vec::new());
    }

    let mut order_rank: HashMap<Uuid, usize> = HashMap::with_capacity(derived_datasets.len());
    let mut deps_map: HashMap<Uuid, BTreeSet<Uuid>> =
        HashMap::with_capacity(derived_datasets.len());
    let mut downstream_adj: HashMap<Uuid, BTreeSet<Uuid>> = HashMap::new();
    let mut by_id: HashMap<Uuid, Dataset> = HashMap::with_capacity(derived_datasets.len());

    for (idx, ds) in derived_datasets.into_iter().enumerate() {
        let deps = parse_depends_on_uuids(&ds.source_ref);
        for &up_id in &deps {
            downstream_adj.entry(up_id).or_default().insert(ds.id);
        }
        order_rank.insert(ds.id, idx);
        deps_map.insert(ds.id, deps);
        by_id.insert(ds.id, ds);
    }

    let mut reachable: BTreeSet<Uuid> = BTreeSet::new();
    let mut bfs_queue: VecDeque<Uuid> = VecDeque::new();
    if let Some(direct) = downstream_adj.get(&upstream_dataset_id) {
        for &nid in direct {
            if reachable.insert(nid) {
                bfs_queue.push_back(nid);
            }
        }
    }
    while let Some(curr) = bfs_queue.pop_front() {
        if let Some(children) = downstream_adj.get(&curr) {
            for &child in children {
                if reachable.insert(child) {
                    bfs_queue.push_back(child);
                }
            }
        }
    }

    if reachable.is_empty() {
        return Ok(Vec::new());
    }

    // Check self-loops and compute internal in-degrees within the reachable subgraph.
    let mut in_degree: HashMap<Uuid, usize> = HashMap::with_capacity(reachable.len());
    for &nid in &reachable {
        let deps = deps_map.get(&nid);
        if deps.is_some_and(|d| d.contains(&nid)) {
            return Err(AppError::Conflict("DEPENDENCY_CYCLE"));
        }
        let internal_deg = deps.map_or(0, |d| d.intersection(&reachable).count());
        in_degree.insert(nid, internal_deg);
    }

    // Kahn's algorithm using `(order_rank, id)` so tie-breaking is completely deterministic.
    let mut ready: BTreeSet<(usize, Uuid)> = BTreeSet::new();
    for (&nid, &deg) in &in_degree {
        if deg == 0 {
            let rank = order_rank.get(&nid).copied().unwrap_or(usize::MAX);
            ready.insert((rank, nid));
        }
    }

    let mut sorted_ids: Vec<Uuid> = Vec::with_capacity(reachable.len());
    while let Some(&first_item) = ready.iter().next() {
        ready.remove(&first_item);
        let nid = first_item.1;
        sorted_ids.push(nid);

        if let Some(children) = downstream_adj.get(&nid) {
            for &child in children {
                if let Some(deg) = in_degree.get_mut(&child) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        let rank = order_rank.get(&child).copied().unwrap_or(usize::MAX);
                        ready.insert((rank, child));
                    }
                }
            }
        }
    }

    if sorted_ids.len() != reachable.len() {
        return Err(AppError::Conflict("DEPENDENCY_CYCLE"));
    }

    let mut result = Vec::with_capacity(sorted_ids.len());
    for id in sorted_ids {
        if let Some(ds) = by_id.remove(&id) {
            result.push(ds);
        }
    }
    Ok(result)
}

async fn insert_metric_lineage_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dataset_id: Uuid,
    metric_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ds_lineage (tenant_id, src_kind, src_id, dst_kind, dst_id, via)
         VALUES ($1, 'DATASET', $2, 'METRIC', $3, 'metric_def')
         ON CONFLICT (tenant_id, src_kind, src_id, dst_kind, dst_id)
         DO UPDATE SET via = EXCLUDED.via",
    )
    .bind(tenant_id)
    .bind(dataset_id.to_string())
    .bind(metric_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert metric ds_lineage failed: {err}")))?;
    Ok(())
}

struct ResolvedMetricPatch {
    display_name: String,
    description: String,
    expression: String,
    filter_sql: String,
    time_column: Option<String>,
    default_grain: RollupGrain,
    dimensions: Vec<MetricDimension>,
    unit: String,
    format: String,
    synonyms: Vec<String>,
    certified: bool,
    schema_version: i32,
    version: i32,
    embed_changed: bool,
}

fn merge_metric_patch(
    old: &MetricDef,
    schema_version: i32,
    req: UpdateMetricRequest,
) -> ResolvedMetricPatch {
    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| old.display_name.clone(), String::from);
    let description = req
        .description
        .as_deref()
        .map_or_else(|| old.description.clone(), |s| String::from(s.trim()));
    let expression = req
        .expression
        .as_deref()
        .map_or_else(|| old.expression.clone(), |s| String::from(s.trim()));
    let filter_sql = req
        .filter_sql
        .as_deref()
        .map_or_else(|| old.filter_sql.clone(), |s| String::from(s.trim()));
    let time_column = match req.time_column {
        None => old.time_column.clone(),
        Some(None) => None,
        Some(Some(col)) => {
            let trimmed = col.trim();
            (!trimmed.is_empty()).then(|| String::from(trimmed))
        }
    };
    let default_grain = req.default_grain.unwrap_or(old.default_grain);
    let dimensions = req.dimensions.unwrap_or_else(|| old.dimensions.clone());
    let unit = req
        .unit
        .as_deref()
        .map_or_else(|| old.unit.clone(), |s| String::from(s.trim()));
    let format = req
        .format
        .as_deref()
        .map_or_else(|| old.format.clone(), |s| String::from(s.trim()));
    let synonyms = req.synonyms.unwrap_or_else(|| old.synonyms.clone());
    let certified = req.certified.unwrap_or(old.certified);

    let def_changed = expression != old.expression
        || filter_sql != old.filter_sql
        || time_column != old.time_column
        || default_grain != old.default_grain
        || dimensions != old.dimensions;

    let version = if def_changed {
        old.version.saturating_add(1)
    } else {
        old.version
    };
    let next_schema_ver = if def_changed {
        schema_version
    } else {
        old.schema_version
    };
    let embed_changed = display_name != old.display_name
        || description != old.description
        || synonyms != old.synonyms;

    ResolvedMetricPatch {
        display_name,
        description,
        expression,
        filter_sql,
        time_column,
        default_grain,
        dimensions,
        unit,
        format,
        synonyms,
        certified,
        schema_version: next_schema_ver,
        version,
        embed_changed,
    }
}

async fn update_metric_impl(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
    schema_version: i32,
    req: UpdateMetricRequest,
) -> Result<Option<MetricDef>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let maybe_row = sqlx::query(
        "SELECT id, name, display_name, description, dataset_id, expression, filter_sql,
                time_column, default_grain, dimensions, unit, format, synonyms, certified,
                schema_version, owner_user_id, version, created_at, updated_at
           FROM an_metrics
          WHERE name = $1
          FOR UPDATE",
    )
    .bind(name.trim())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("select an_metrics for update: {err}")))?;

    let Some(existing_row) = maybe_row else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit noop update_metric tx failed: {err}"))
        })?;
        return Ok(None);
    };

    let old = decode_metric_row(&existing_row, false)?;
    let patch = merge_metric_patch(&old, schema_version, req);
    let dims_json = serde_json::to_value(&patch.dimensions).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("serialize dimensions failed: {err}"),
        )
    })?;

    let updated_row = sqlx::query(
        "UPDATE an_metrics
            SET display_name = $2, description = $3, expression = $4, filter_sql = $5,
                time_column = $6, default_grain = $7, dimensions = $8, unit = $9,
                format = $10, synonyms = $11, certified = $12, schema_version = $13,
                version = $14, updated_at = NOW()
          WHERE id = $1
          RETURNING id, name, display_name, description, dataset_id, expression, filter_sql,
                    time_column, default_grain, dimensions, unit, format, synonyms, certified,
                    schema_version, owner_user_id, version, created_at, updated_at",
    )
    .bind(old.id)
    .bind(&patch.display_name)
    .bind(&patch.description)
    .bind(&patch.expression)
    .bind(&patch.filter_sql)
    .bind(&patch.time_column)
    .bind(patch.default_grain.as_str())
    .bind(dims_json)
    .bind(&patch.unit)
    .bind(&patch.format)
    .bind(&patch.synonyms)
    .bind(patch.certified)
    .bind(patch.schema_version)
    .bind(patch.version)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("update an_metrics failed: {err}")))?;

    if patch.embed_changed {
        refresh_metric_embedding_job_tx(&mut tx, tenant_id, old.id, &patch).await?;
    }

    let col_sets = load_dataset_column_sets_tx(&mut tx, &[old.dataset_id]).await?;
    let mut updated = decode_metric_row(&updated_row, false)?;
    updated.broken = compute_metric_broken(&updated, col_sets.get(&updated.dataset_id));

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit update_metric tx failed: {err}")))?;
    Ok(Some(updated))
}

async fn refresh_metric_embedding_job_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    metric_id: Uuid,
    patch: &ResolvedMetricPatch,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM hg_embeddings WHERE node_type = 'AN_METRIC' AND node_id = $1")
        .bind(metric_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("delete stale AN_METRIC embedding failed: {err}"))
        })?;

    let content_hash =
        build_metric_embed_hash(&patch.display_name, &patch.description, &patch.synonyms);
    let job = NewJob::new(
        JobKind::EmbedNodes,
        serde_json::json!({ "metric_id": metric_id }),
    )
    .with_idempotency_key(format!("embed-metric:{metric_id}:u:{content_hash}"));
    enqueue_job_tx(tx, tenant_id, &job).await?;
    Ok(())
}

async fn list_unembedded_columns_tx(
    tx: &mut Transaction<'_, Postgres>,
    existing: &BTreeSet<(String, Uuid)>,
    remaining: usize,
) -> Result<Vec<(String, Uuid, String)>, AppError> {
    if remaining == 0 {
        return Ok(Vec::new());
    }

    let ds_rows = sqlx::query(
        "SELECT id, name, display_name, schema_json
           FROM ds_datasets
          WHERE status = 'READY' OR last_snapshot_id IS NOT NULL
          ORDER BY created_at ASC, id ASC",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ready datasets for embed failed: {err}")))?;

    if ds_rows.is_empty() {
        return Ok(Vec::new());
    }

    let ds_ids: Vec<Uuid> = ds_rows.iter().map(|r| r.get("id")).collect();
    let meta_rows = sqlx::query(
        "SELECT dataset_id, column_name, description, business_term, unit
           FROM ds_column_meta
          WHERE dataset_id = ANY($1)
          ORDER BY dataset_id, ordinal ASC, column_name ASC",
    )
    .bind(&ds_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ds_column_meta for embed failed: {err}")))?;

    let mut meta_by_ds: HashMap<Uuid, Vec<(String, String, String, String)>> = HashMap::new();
    for r in meta_rows {
        meta_by_ds.entry(r.get("dataset_id")).or_default().push((
            r.get("column_name"),
            r.get("description"),
            r.get("business_term"),
            r.get("unit"),
        ));
    }

    let mut out = Vec::new();
    for ds_row in ds_rows {
        let ds_id: Uuid = ds_row.get("id");
        let ds_name: String = ds_row.get("name");
        let ds_display: String = ds_row.get("display_name");
        let schema_val: serde_json::Value = ds_row.get("schema_json");
        let profiles: Vec<ColumnProfile> = serde_json::from_value(schema_val).unwrap_or_default();
        let active_cols: Option<BTreeSet<String>> = (!profiles.is_empty()).then(|| {
            profiles
                .iter()
                .map(|p| p.name.to_ascii_lowercase())
                .collect()
        });

        let mut seen = BTreeSet::new();
        if let Some(cols) = meta_by_ds.get(&ds_id) {
            for (cname, desc, bterm, unit) in cols {
                let clow = cname.to_ascii_lowercase();
                if let Some(ref active) = active_cols
                    && !active.contains(&clow)
                {
                    continue;
                }
                seen.insert(clow);
                let emb_id = ds_column_embedding_id(ds_id, cname);
                if !existing.contains(&(String::from("DS_COLUMN"), emb_id)) {
                    let text =
                        build_column_embed_text(&ds_name, &ds_display, cname, desc, bterm, unit);
                    out.push((String::from("DS_COLUMN"), emb_id, text));
                    if out.len() >= remaining {
                        return Ok(out);
                    }
                }
            }
        }

        for prof in &profiles {
            if seen.insert(prof.name.to_ascii_lowercase()) {
                let emb_id = ds_column_embedding_id(ds_id, &prof.name);
                if !existing.contains(&(String::from("DS_COLUMN"), emb_id)) {
                    let text = build_column_embed_text(
                        &ds_name,
                        &ds_display,
                        &prof.name,
                        &prof.source_comment,
                        "",
                        "",
                    );
                    out.push((String::from("DS_COLUMN"), emb_id, text));
                    if out.len() >= remaining {
                        return Ok(out);
                    }
                }
            }
        }
    }

    Ok(out)
}

impl AnalyticsRepository for PgStore {
    async fn list_metrics(&self, tenant_id: &str) -> Result<Vec<MetricDef>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, name, display_name, description, dataset_id, expression, filter_sql,
                    time_column, default_grain, dimensions, unit, format, synonyms, certified,
                    schema_version, owner_user_id, version, created_at, updated_at
               FROM an_metrics
              ORDER BY certified DESC, name ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list an_metrics failed: {err}")))?;

        let ds_ids: Vec<Uuid> = rows
            .iter()
            .map(|r| r.get::<Uuid, _>("dataset_id"))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let col_sets = load_dataset_column_sets_tx(&mut tx, &ds_ids).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_metrics tx failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for r in rows {
            let mut m = decode_metric_row(&r, false)?;
            m.broken = compute_metric_broken(&m, col_sets.get(&m.dataset_id));
            items.push(m);
        }
        Ok(items)
    }

    async fn get_metric_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<MetricDef>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, name, display_name, description, dataset_id, expression, filter_sql,
                    time_column, default_grain, dimensions, unit, format, synonyms, certified,
                    schema_version, owner_user_id, version, created_at, updated_at
               FROM an_metrics
              WHERE name = $1",
        )
        .bind(name.trim())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get an_metrics by name failed: {err}")))?;

        let Some(row) = maybe_row else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty get_metric_by_name tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let mut m = decode_metric_row(&row, false)?;
        let col_sets = load_dataset_column_sets_tx(&mut tx, &[m.dataset_id]).await?;
        m.broken = compute_metric_broken(&m, col_sets.get(&m.dataset_id));

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_metric_by_name tx failed: {err}"))
        })?;
        Ok(Some(m))
    }

    async fn create_metric(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        schema_version: i32,
        req: CreateMetricRequest,
    ) -> Result<MetricDef, AppError> {
        let trimmed_name = req.name.trim();
        if !is_valid_ident_name(trimmed_name) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("metric name '{trimmed_name}' must match ^[a-z][a-z0-9_]{{0,62}}$"),
            ));
        }

        let display_name = if req.display_name.trim().is_empty() {
            String::from(trimmed_name)
        } else {
            String::from(req.display_name.trim())
        };
        let time_column = req
            .time_column
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let dims_json = serde_json::to_value(&req.dimensions).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize dimensions failed: {err}"),
            )
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let col_sets = load_dataset_column_sets_tx(&mut tx, &[req.dataset_id]).await?;
        if !col_sets.contains_key(&req.dataset_id) {
            return Err(AppError::NotFound("DATASET_NOT_FOUND"));
        }

        let insert_res = sqlx::query(
            "INSERT INTO an_metrics (
                 tenant_id, name, display_name, description, dataset_id, expression,
                 filter_sql, time_column, default_grain, dimensions, unit, format,
                 synonyms, certified, schema_version, owner_user_id
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             RETURNING id, name, display_name, description, dataset_id, expression, filter_sql,
                       time_column, default_grain, dimensions, unit, format, synonyms, certified,
                       schema_version, owner_user_id, version, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(display_name)
        .bind(req.description.trim())
        .bind(req.dataset_id)
        .bind(req.expression.trim())
        .bind(req.filter_sql.trim())
        .bind(time_column)
        .bind(req.default_grain.as_str())
        .bind(dims_json)
        .bind(req.unit.trim())
        .bind(req.format.trim())
        .bind(&req.synonyms)
        .bind(req.certified)
        .bind(schema_version)
        .bind(owner_user_id)
        .fetch_one(&mut *tx)
        .await;

        let row = match insert_res {
            Ok(r) => r,
            Err(err) if is_unique_violation(&err) => return Err(AppError::Conflict("CONFLICT")),
            Err(err) if is_foreign_key_violation(&err) => {
                return Err(AppError::NotFound("DATASET_NOT_FOUND"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "insert an_metrics failed: {err}"
                )));
            }
        };

        let mut metric = decode_metric_row(&row, false)?;
        metric.broken = compute_metric_broken(&metric, col_sets.get(&metric.dataset_id));

        insert_metric_lineage_tx(&mut tx, tenant_id, metric.dataset_id, metric.id).await?;

        let embed_job = NewJob::new(
            JobKind::EmbedNodes,
            serde_json::json!({ "metric_id": metric.id }),
        )
        .with_idempotency_key(format!("embed-metric:{}:v1", metric.id));
        enqueue_job_tx(&mut tx, tenant_id, &embed_job).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_metric tx failed: {err}")))?;
        Ok(metric)
    }

    async fn update_metric(
        &self,
        tenant_id: &str,
        name: &str,
        schema_version: i32,
        req: UpdateMetricRequest,
    ) -> Result<Option<MetricDef>, AppError> {
        update_metric_impl(self, tenant_id, name, schema_version, req).await
    }

    async fn delete_metric(&self, tenant_id: &str, name: &str) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let deleted = sqlx::query("DELETE FROM an_metrics WHERE name = $1 RETURNING id")
            .bind(name.trim())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete an_metrics failed: {err}")))?;

        let Some(row) = deleted else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop delete_metric tx failed: {err}"))
            })?;
            return Ok(false);
        };

        let metric_id: Uuid = row.get("id");
        sqlx::query("DELETE FROM hg_embeddings WHERE node_type = 'AN_METRIC' AND node_id = $1")
            .bind(metric_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("cleanup metric hg_embeddings failed: {err}"))
            })?;

        let id_str = metric_id.to_string();
        sqlx::query(
            "DELETE FROM ds_lineage
              WHERE (dst_kind = 'METRIC' AND dst_id = $1)
                 OR (src_kind = 'METRIC' AND src_id = $1)",
        )
        .bind(id_str)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("cleanup metric ds_lineage failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_metric tx failed: {err}")))?;
        Ok(true)
    }

    async fn list_saved_queries(
        &self,
        tenant_id: &str,
        viewer_user_id: Uuid,
        include_all: bool,
    ) -> Result<Vec<SavedQuery>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = if include_all {
            sqlx::query(
                "SELECT id, name, kind, spec, parameters, owner_user_id, shared, created_at, updated_at
                   FROM an_saved_queries
                  ORDER BY updated_at DESC, id DESC",
            )
            .fetch_all(&mut *tx)
            .await
        } else {
            sqlx::query(
                "SELECT id, name, kind, spec, parameters, owner_user_id, shared, created_at, updated_at
                   FROM an_saved_queries
                  WHERE shared = TRUE OR owner_user_id = $1
                  ORDER BY updated_at DESC, id DESC",
            )
            .bind(viewer_user_id)
            .fetch_all(&mut *tx)
            .await
        }
        .map_err(|err| AppError::Database(format!("list an_saved_queries failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_saved_queries tx failed: {err}"))
        })?;

        rows.iter().map(decode_saved_query_row).collect()
    }

    async fn get_saved_query(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> Result<Option<SavedQuery>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, name, kind, spec, parameters, owner_user_id, shared, created_at, updated_at
               FROM an_saved_queries
              WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get an_saved_queries failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_saved_query tx failed: {err}"))
        })?;

        maybe_row.as_ref().map(decode_saved_query_row).transpose()
    }

    async fn create_saved_query(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        req: CreateSavedQueryRequest,
    ) -> Result<SavedQuery, AppError> {
        let trimmed_name = req.name.trim();
        if trimmed_name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "saved query name cannot be empty",
            ));
        }
        let params_json = serde_json::to_value(&req.parameters).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize parameters failed: {err}"),
            )
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let insert_res = sqlx::query(
            "INSERT INTO an_saved_queries (
                 tenant_id, name, kind, spec, parameters, owner_user_id, shared
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, name, kind, spec, parameters, owner_user_id, shared, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(saved_query_kind_as_str(req.kind))
        .bind(&req.spec)
        .bind(params_json)
        .bind(owner_user_id)
        .bind(req.shared)
        .fetch_one(&mut *tx)
        .await;

        let row = match insert_res {
            Ok(r) => r,
            Err(err) if is_unique_violation(&err) => return Err(AppError::Conflict("CONFLICT")),
            Err(err) => {
                return Err(AppError::Database(format!(
                    "insert an_saved_queries failed: {err}"
                )));
            }
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_saved_query tx failed: {err}"))
        })?;
        decode_saved_query_row(&row)
    }

    async fn update_saved_query(
        &self,
        tenant_id: &str,
        id: Uuid,
        req: UpdateSavedQueryRequest,
    ) -> Result<Option<SavedQuery>, AppError> {
        if let Some(ref n) = req.name
            && n.trim().is_empty()
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "saved query name cannot be empty",
            ));
        }

        let params_json = req
            .parameters
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("serialize parameters failed: {err}"),
                )
            })?;
        let trimmed_name = req.name.as_deref().map(str::trim);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let update_res = sqlx::query(
            "UPDATE an_saved_queries
                SET name = COALESCE($2, name),
                    spec = COALESCE($3, spec),
                    parameters = COALESCE($4, parameters),
                    shared = COALESCE($5, shared),
                    updated_at = NOW()
              WHERE id = $1
              RETURNING id, name, kind, spec, parameters, owner_user_id, shared, created_at, updated_at",
        )
        .bind(id)
        .bind(trimmed_name)
        .bind(req.spec)
        .bind(params_json)
        .bind(req.shared)
        .fetch_optional(&mut *tx)
        .await;

        let maybe_row = match update_res {
            Ok(r) => r,
            Err(err) if is_unique_violation(&err) => return Err(AppError::Conflict("CONFLICT")),
            Err(err) => {
                return Err(AppError::Database(format!(
                    "update an_saved_queries failed: {err}"
                )));
            }
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_saved_query tx failed: {err}"))
        })?;
        maybe_row.as_ref().map(decode_saved_query_row).transpose()
    }

    async fn delete_saved_query(&self, tenant_id: &str, id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM an_saved_queries WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete an_saved_queries failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_saved_query tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn record_query_history(
        &self,
        tenant_id: &str,
        entry: NewQueryHistoryEntry,
    ) -> Result<i64, AppError> {
        let truncated_sql: String = entry.sql_text.chars().take(HISTORY_SQL_MAX_CHARS).collect();

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO an_query_history (
                 tenant_id, user_id, kind, sql_text, status, row_count, duration_ms, error_code
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(entry.user_id)
        .bind(entry.kind.as_str())
        .bind(truncated_sql)
        .bind(entry.status.as_str())
        .bind(entry.row_count)
        .bind(entry.duration_ms)
        .bind(entry.error_code)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert an_query_history failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit record_query_history tx failed: {err}"))
        })?;
        Ok(id)
    }

    async fn list_query_history(
        &self,
        tenant_id: &str,
        only_user_id: Option<Uuid>,
        query: QueryHistoryListQuery,
    ) -> Result<PaginatedList<QueryHistoryEntry>, AppError> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=500, got {limit}"),
            ));
        }

        let cursor_id: Option<i64> = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(raw) => Some(raw.trim().parse::<i64>().map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid history cursor: {err}"))
            })?),
            None => None,
        };

        let limit_usize = usize::try_from(limit).unwrap_or(50);
        let fetch_limit = i64::from(limit).saturating_add(1);
        let kind_str = query.kind.map(QueryHistoryKind::as_str);
        let status_str = query.status.map(QueryHistoryStatus::as_str);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, user_id, kind, sql_text, status, row_count, duration_ms, error_code, created_at
               FROM an_query_history
              WHERE ($1::uuid IS NULL OR user_id = $1)
                AND ($2::text IS NULL OR kind = $2)
                AND ($3::text IS NULL OR status = $3)
                AND ($4::bigint IS NULL OR id < $4)
              ORDER BY id DESC
              LIMIT $5",
        )
        .bind(only_user_id)
        .bind(kind_str)
        .bind(status_str)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list an_query_history failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_query_history tx failed: {err}"))
        })?;

        let mut items: Vec<QueryHistoryEntry> = rows
            .iter()
            .map(decode_query_history_row)
            .collect::<Result<_, _>>()?;

        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items.last().map(|e| e.id.to_string())
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn insert_ask_turn(
        &self,
        tenant_id: &str,
        turn: NewAskTurnInput,
    ) -> Result<AskTurnRecord, AppError> {
        let linked_json = serde_json::to_value(&turn.linked_objects).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize linked_objects failed: {err}"),
            )
        })?;
        let citations_json = serde_json::to_value(&turn.citations).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize citations failed: {err}"),
            )
        })?;

        let verified_numbers = i32::try_from(turn.verified_numbers).unwrap_or(i32::MAX);
        let verified_citations = i32::try_from(turn.verified_citations).unwrap_or(i32::MAX);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO an_ask_turns (
                 tenant_id, session_id, user_id, question, intent, linked_objects,
                 sql_text, result_preview, answer, citations, model_name,
                 grounding_mode, verified_numbers, verified_citations, abstained, slot_payload
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             RETURNING id, session_id, user_id, question, intent, linked_objects, sql_text,
                       result_preview, answer, citations, feedback, corrected_sql, model_name, created_at,
                       grounding_mode, verified_numbers, verified_citations, abstained, slot_payload",
        )
        .bind(tenant_id)
        .bind(turn.session_id)
        .bind(turn.user_id)
        .bind(&turn.question)
        .bind(turn.intent.as_str())
        .bind(linked_json)
        .bind(turn.sql_text)
        .bind(turn.result_preview)
        .bind(&turn.answer)
        .bind(citations_json)
        .bind(&turn.model_name)
        .bind(turn.grounding_mode.as_str())
        .bind(verified_numbers)
        .bind(verified_citations)
        .bind(turn.abstained)
        .bind(turn.slot_payload)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert an_ask_turns failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit insert_ask_turn tx failed: {err}"))
        })?;
        decode_ask_turn_row(&row)
    }

    async fn get_ask_turn(
        &self,
        tenant_id: &str,
        turn_id: Uuid,
    ) -> Result<Option<AskTurnRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, session_id, user_id, question, intent, linked_objects, sql_text,
                    result_preview, answer, citations, feedback, corrected_sql, model_name, created_at,
                    grounding_mode, verified_numbers, verified_citations, abstained, slot_payload
               FROM an_ask_turns
              WHERE id = $1",
        )
        .bind(turn_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get an_ask_turns failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_ask_turn tx failed: {err}")))?;
        maybe_row.as_ref().map(decode_ask_turn_row).transpose()
    }

    async fn update_ask_feedback(
        &self,
        tenant_id: &str,
        turn_id: Uuid,
        feedback: AskFeedbackKind,
        corrected_sql: Option<&str>,
    ) -> Result<Option<AskTurnRecord>, AppError> {
        let cleaned_corrected = corrected_sql.map(str::trim).filter(|s| !s.is_empty());

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE an_ask_turns
                SET feedback = $2,
                    corrected_sql = COALESCE($3, corrected_sql)
              WHERE id = $1
              RETURNING id, session_id, user_id, question, intent, linked_objects, sql_text,
                        result_preview, answer, citations, feedback, corrected_sql, model_name, created_at,
                        grounding_mode, verified_numbers, verified_citations, abstained, slot_payload",
        )
        .bind(turn_id)
        .bind(feedback.as_str())
        .bind(cleaned_corrected)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update an_ask_turns feedback failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_ask_feedback tx failed: {err}"))
        })?;
        maybe_row.as_ref().map(decode_ask_turn_row).transpose()
    }

    async fn list_session_turns(
        &self,
        tenant_id: &str,
        session_id: Uuid,
    ) -> Result<Vec<AskTurnRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, session_id, user_id, question, intent, linked_objects, sql_text,
                    result_preview, answer, citations, feedback, corrected_sql, model_name, created_at,
                    grounding_mode, verified_numbers, verified_citations, abstained, slot_payload
               FROM an_ask_turns
              WHERE session_id = $1
              ORDER BY created_at ASC, id ASC",
        )
        .bind(session_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list session an_ask_turns failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_session_turns tx failed: {err}"))
        })?;
        rows.iter().map(decode_ask_turn_row).collect()
    }

    async fn search_few_shot_turns(
        &self,
        tenant_id: &str,
        question: &str,
        limit: u32,
    ) -> Result<Vec<FewShotTurn>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let q_trim = question.trim();
        if limit == 0 || q_trim.is_empty() {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty search_few_shot_turns tx: {err}"))
            })?;
            return Ok(Vec::new());
        }

        let rows = sqlx::query(
            "SELECT id, question,
                    COALESCE(NULLIF(btrim(corrected_sql), ''), NULLIF(btrim(sql_text), '')) AS effective_sql,
                    ts_rank_cd(fts, websearch_to_tsquery('simple', $1))::float8 AS pg_fts_rank,
                    created_at
               FROM an_ask_turns
              WHERE (feedback = 'UP' OR (corrected_sql IS NOT NULL AND btrim(corrected_sql) <> ''))
                AND COALESCE(NULLIF(btrim(corrected_sql), ''), NULLIF(btrim(sql_text), '')) IS NOT NULL
              ORDER BY created_at DESC
              LIMIT 500",
        )
        .bind(q_trim)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("search_few_shot_turns query failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit search_few_shot_turns tx failed: {err}"))
        })?;

        let mut scored: Vec<(f64, DateTime<Utc>, Uuid, FewShotTurn)> = Vec::new();
        for r in rows {
            let cand_q: String = r.get("question");
            let eff_sql: Option<String> = r.get("effective_sql");
            let Some(sql_text) = eff_sql else {
                continue;
            };
            let pg_rank: f64 = r.try_get("pg_fts_rank").unwrap_or(0.0);
            let sim = compute_question_similarity(q_trim, &cand_q, pg_rank);
            if sim > 0.05 {
                scored.push((
                    sim,
                    r.get("created_at"),
                    r.get("id"),
                    FewShotTurn {
                        question: cand_q,
                        sql_text,
                    },
                ));
            }
        }

        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let cap = usize::try_from(limit.min(100)).unwrap_or(5);
        scored.truncate(cap);
        Ok(scored.into_iter().map(|(_, _, _, t)| t).collect())
    }

    async fn recall_schema_candidates(
        &self,
        tenant_id: &str,
        question: &str,
        query_embedding: Option<&[f32]>,
        restrict_datasets: &[Uuid],
    ) -> Result<SchemaRecallBundle, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let glossary_db_rows = sqlx::query("SELECT term, synonyms FROM ds_glossary_terms")
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("load ds_glossary_terms for recall failed: {err}"))
            })?;
        let glossary_pairs: Vec<(String, Vec<String>)> = glossary_db_rows
            .into_iter()
            .map(|r| (r.get("term"), r.get("synonyms")))
            .collect();
        let ctx = build_expanded_query_context(question, &glossary_pairs);

        let datasets =
            load_ready_datasets_for_recall_tx(&mut tx, question, restrict_datasets).await?;
        let ds_scores: HashMap<Uuid, f64> = datasets
            .iter()
            .map(|d| {
                (
                    d.id,
                    score_dataset_lexical(
                        &ctx,
                        &d.name,
                        &d.display_name,
                        &d.description,
                        &d.tags,
                        d.pg_fts,
                    ),
                )
            })
            .collect();

        let mut columns = load_candidate_columns_tx(&mut tx, &datasets).await?;
        let metric_rows = load_candidate_metrics_tx(&mut tx, question, restrict_datasets).await?;

        let (col_vec_map, metric_vec_map) = match query_embedding {
            Some(qvec) if !qvec.is_empty() => {
                load_analytics_vector_scores_tx(&mut tx, tenant_id, qvec).await?
            }
            _ => (HashMap::new(), HashMap::new()),
        };

        let matched_nodes = match_business_nodes_tx(&mut tx, question).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit recall_schema_candidates tx failed: {err}"))
        })?;

        for col in &mut columns {
            let ds_s = ds_scores.get(&col.dataset_id).copied().unwrap_or(0.0);
            col.fts_score = score_column_lexical(&ctx, ds_s, col);
            let emb_id = ds_column_embedding_id(col.dataset_id, &col.column_name);
            col.vec_score = col_vec_map.get(&emb_id).copied().unwrap_or(0.0);
        }
        columns.sort_by(|a, b| {
            (b.fts_score + b.vec_score)
                .total_cmp(&(a.fts_score + a.vec_score))
                .then_with(|| a.dataset_name.cmp(&b.dataset_name))
                .then_with(|| a.column_name.cmp(&b.column_name))
        });

        let mut metrics: Vec<LinkedMetricHit> = metric_rows
            .into_iter()
            .map(|(m, pg_fts)| {
                let fts_score = score_metric_lexical(&ctx, &m, pg_fts);
                let vec_score = metric_vec_map.get(&m.id).copied().unwrap_or(0.0);
                LinkedMetricHit {
                    metric: m,
                    fts_score,
                    vec_score,
                }
            })
            .collect();
        metrics.sort_by(|a, b| {
            (b.fts_score + b.vec_score)
                .total_cmp(&(a.fts_score + a.vec_score))
                .then_with(|| b.metric.certified.cmp(&a.metric.certified))
                .then_with(|| a.metric.name.cmp(&b.metric.name))
        });

        Ok(SchemaRecallBundle {
            columns,
            metrics,
            matched_nodes,
        })
    }

    async fn list_unembedded_analytics_items(
        &self,
        tenant_id: &str,
        model_name: &str,
        limit: u32,
    ) -> Result<Vec<(String, Uuid, String)>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let trimmed_model = model_name.trim();
        if limit == 0 || trimmed_model.is_empty() {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!(
                    "commit empty list_unembedded_analytics_items: {err}"
                ))
            })?;
            return Ok(Vec::new());
        }

        let max_items = usize::try_from(limit).unwrap_or(usize::MAX);
        let existing_rows = sqlx::query(
            "SELECT node_type, node_id
               FROM hg_embeddings
              WHERE model_name = $1
                AND node_type IN ('DS_COLUMN', 'AN_METRIC')",
        )
        .bind(trimmed_model)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("query existing analytics embeddings failed: {err}"))
        })?;

        let existing: BTreeSet<(String, Uuid)> = existing_rows
            .into_iter()
            .map(|r| (r.get("node_type"), r.get("node_id")))
            .collect();

        let mut items: Vec<(String, Uuid, String)> = Vec::new();
        let metric_rows = sqlx::query(
            "SELECT id, name, display_name, description, synonyms
               FROM an_metrics
              ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load an_metrics for embed failed: {err}")))?;

        for r in metric_rows {
            let mid: Uuid = r.get("id");
            if !existing.contains(&(String::from("AN_METRIC"), mid)) {
                let name: String = r.get("name");
                let disp: String = r.get("display_name");
                let desc: String = r.get("description");
                let syns: Vec<String> = r.get("synonyms");
                let text = build_metric_embed_text(&name, &disp, &desc, &syns);
                items.push((String::from("AN_METRIC"), mid, text));
                if items.len() >= max_items {
                    break;
                }
            }
        }

        if items.len() < max_items {
            let rem = max_items.saturating_sub(items.len());
            let mut col_items = list_unembedded_columns_tx(&mut tx, &existing, rem).await?;
            items.append(&mut col_items);
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_unembedded_analytics_items tx: {err}"))
        })?;
        Ok(items)
    }

    async fn upsert_analytics_embeddings(
        &self,
        tenant_id: &str,
        model_name: &str,
        dim: usize,
        items: &[(String, Uuid, Vec<f32>)],
    ) -> Result<usize, AppError> {
        let trimmed_model = model_name.trim();
        if trimmed_model.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "embedding model_name must be non-empty",
            ));
        }
        let dim_i32 = i32::try_from(dim).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid embedding dim: {err}"))
        })?;
        if dim_i32 <= 0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "embedding dim must be positive",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut written = 0_usize;
        for (node_type, node_id, vec_data) in items {
            let ntype = node_type.trim();
            if ntype != "DS_COLUMN" && ntype != "AN_METRIC" {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("unsupported analytics embedding node_type: {ntype}"),
                ));
            }
            if vec_data.len() != dim {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!(
                        "embedding vector len {} does not match dim {dim}",
                        vec_data.len()
                    ),
                ));
            }

            if ntype == "AN_METRIC" {
                let exists = sqlx::query(
                    "SELECT 1 FROM an_metrics WHERE tenant_id = $1 AND id = $2 FOR KEY SHARE",
                )
                .bind(tenant_id)
                .bind(*node_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock an_metrics for embedding failed: {err}"))
                })?;
                if exists.is_none() {
                    continue;
                }
            }

            let res = sqlx::query(
                "INSERT INTO hg_embeddings (
                     tenant_id, node_type, node_id, model_name, embedding_dim, embedding
                 )
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (node_type, node_id, model_name) DO UPDATE SET
                     tenant_id = EXCLUDED.tenant_id,
                     embedding_dim = EXCLUDED.embedding_dim,
                     embedding = EXCLUDED.embedding",
            )
            .bind(tenant_id)
            .bind(ntype)
            .bind(*node_id)
            .bind(trimmed_model)
            .bind(dim_i32)
            .bind(Vector::from(vec_data.clone()))
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("upsert analytics hg_embeddings failed: {err}"))
            })?;
            written = written.saturating_add(usize::try_from(res.rows_affected()).unwrap_or(0));
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit upsert_analytics_embeddings tx: {err}"))
        })?;
        Ok(written)
    }

    async fn create_transform_draft(
        &self,
        tenant_id: &str,
        input: NewTransformDraftInput,
    ) -> Result<TransformDraft, AppError> {
        let target_name = input.target_dataset_name.trim();
        if !is_valid_ident_name(target_name) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("target_dataset_name '{target_name}' must match ^[a-z][a-z0-9_]{{0,62}}$"),
            ));
        }

        let preview_json = serde_json::to_value(&input.preview).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize transform preview failed: {err}"),
            )
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO an_transform_drafts (
                 tenant_id, user_id, instruction, source_dataset_ids, sql_text,
                 preview, target_dataset_name, status
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'DRAFT')
             RETURNING id, user_id, instruction, source_dataset_ids, sql_text,
                       preview, target_dataset_name, status, dataset_id, created_at",
        )
        .bind(tenant_id)
        .bind(input.user_id)
        .bind(&input.instruction)
        .bind(&input.source_dataset_ids)
        .bind(&input.sql_text)
        .bind(preview_json)
        .bind(target_name)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert an_transform_drafts failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_transform_draft tx failed: {err}"))
        })?;
        decode_transform_draft_row(&row)
    }

    async fn get_transform_draft(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> Result<Option<TransformDraft>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, user_id, instruction, source_dataset_ids, sql_text,
                    preview, target_dataset_name, status, dataset_id, created_at
               FROM an_transform_drafts
              WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get an_transform_drafts failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_transform_draft tx failed: {err}"))
        })?;
        maybe_row
            .as_ref()
            .map(decode_transform_draft_row)
            .transpose()
    }

    async fn update_transform_draft(
        &self,
        tenant_id: &str,
        id: Uuid,
        sql_text: &str,
        preview: &TransformPreview,
    ) -> Result<Option<TransformDraft>, AppError> {
        let preview_json = serde_json::to_value(preview).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize transform preview failed: {err}"),
            )
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let status_opt: Option<String> =
            sqlx::query_scalar("SELECT status FROM an_transform_drafts WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock an_transform_drafts failed: {err}"))
                })?;

        let Some(status) = status_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop update_transform_draft tx: {err}"))
            })?;
            return Ok(None);
        };
        if status != "DRAFT" {
            return Err(AppError::Conflict("CONFLICT"));
        }

        let row = sqlx::query(
            "UPDATE an_transform_drafts
                SET sql_text = $2,
                    preview = $3
              WHERE id = $1
              RETURNING id, user_id, instruction, source_dataset_ids, sql_text,
                        preview, target_dataset_name, status, dataset_id, created_at",
        )
        .bind(id)
        .bind(sql_text)
        .bind(preview_json)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update an_transform_drafts failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_transform_draft tx failed: {err}"))
        })?;
        Ok(Some(decode_transform_draft_row(&row)?))
    }

    async fn mark_transform_applied(
        &self,
        tenant_id: &str,
        id: Uuid,
        dataset_id: Uuid,
    ) -> Result<Option<TransformDraft>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let status_opt: Option<String> =
            sqlx::query_scalar("SELECT status FROM an_transform_drafts WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock an_transform_drafts failed: {err}"))
                })?;

        let Some(status) = status_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop mark_transform_applied tx: {err}"))
            })?;
            return Ok(None);
        };
        if status != "DRAFT" {
            return Err(AppError::Conflict("CONFLICT"));
        }

        let row = sqlx::query(
            "UPDATE an_transform_drafts
                SET status = 'APPLIED',
                    dataset_id = $2
              WHERE id = $1
              RETURNING id, user_id, instruction, source_dataset_ids, sql_text,
                        preview, target_dataset_name, status, dataset_id, created_at",
        )
        .bind(id)
        .bind(dataset_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("mark an_transform_drafts APPLIED failed: {err}"))
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit mark_transform_applied tx failed: {err}"))
        })?;
        Ok(Some(decode_transform_draft_row(&row)?))
    }

    async fn mark_transform_discarded(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> Result<Option<TransformDraft>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let status_opt: Option<String> =
            sqlx::query_scalar("SELECT status FROM an_transform_drafts WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock an_transform_drafts failed: {err}"))
                })?;

        let Some(status) = status_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop mark_transform_discarded tx: {err}"))
            })?;
            return Ok(None);
        };
        if status != "DRAFT" {
            return Err(AppError::Conflict("CONFLICT"));
        }

        let row = sqlx::query(
            "UPDATE an_transform_drafts
                SET status = 'DISCARDED'
              WHERE id = $1
              RETURNING id, user_id, instruction, source_dataset_ids, sql_text,
                        preview, target_dataset_name, status, dataset_id, created_at",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("mark an_transform_drafts DISCARDED failed: {err}"))
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit mark_transform_discarded tx failed: {err}"))
        })?;
        Ok(Some(decode_transform_draft_row(&row)?))
    }

    async fn list_dependent_derived_datasets(
        &self,
        tenant_id: &str,
        upstream_dataset_id: Uuid,
    ) -> Result<Vec<Dataset>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, connection_id, name, display_name, description, source_kind, source_ref,
                    sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                    lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                    last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at
               FROM ds_datasets
              WHERE source_kind = 'DERIVED_VIEW'
              ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load DERIVED_VIEW datasets failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_dependent_derived_datasets tx: {err}"))
        })?;

        let derived_datasets: Vec<Dataset> = rows
            .iter()
            .map(decode_dataset_row)
            .collect::<Result<_, _>>()?;
        topological_sort_downstream(upstream_dataset_id, derived_datasets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_sql_identifiers_handles_quotes_literals_casts_and_functions() {
        let expr = "SUM(amount) / NULLIF(COUNT(DISTINCT order_id), 0)";
        let cols = extract_sql_column_identifiers(expr);
        assert_eq!(
            cols,
            BTreeSet::from([String::from("amount"), String::from("order_id")])
        );

        let filter =
            "status = 'PAID' AND \"MixedCol\"::TEXT <> 'it''s' AND demo_orders.region IN ('华东')";
        let fcols = extract_sql_column_identifiers(filter);
        assert_eq!(
            fcols,
            BTreeSet::from([
                String::from("mixedcol"),
                String::from("region"),
                String::from("status"),
            ])
        );
    }

    #[test]
    fn cjk_question_similarity_distinguishes_related_vs_unrelated() {
        let s_rel = compute_question_similarity(
            "已支付订单金额合计是多少",
            "已支付订单的总金额是多少",
            0.0,
        );
        let s_unrel =
            compute_question_similarity("北京明天天气如何", "已支付订单的总金额是多少", 0.0);
        assert!(s_rel > 0.45, "expected high CJK similarity, got {s_rel}");
        assert!(
            s_unrel < 0.05,
            "expected near-zero similarity, got {s_unrel}"
        );
    }
}
