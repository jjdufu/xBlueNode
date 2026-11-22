//! 60-case NL2SQL benchmark suite with offline `JSONL` cassette replay (`05-STEP04` §04.6.8, `A5` §4).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use axpoint_analytics::nl2sql::intent::INTENT_SYSTEM_PROMPT;
use axpoint_analytics::nl2sql::prompt::SUMMARY_SYSTEM_PROMPT;
use axpoint_analytics::nl2sql::schema_link::SchemaLinkExtras;
use axpoint_analytics::{
    AskIntent, AskPipelineInput, AskRequest, AskTurnRecord, CatalogDatasetEntry, LinkedObjects,
    MetricDef, MetricDimension, QueryHistoryStatus, SchemaRecallBundle, TenantCatalog, guard,
    run_ask_pipeline_conn,
};
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, PiiCategory, RollupGrain};
use axpoint_domain::ports::AiPort;
use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

const FIXED_NOW_RFC3339: &str = "2026-03-01T00:00:00Z";
const MIN_OVERALL_ACCURACY: f64 = 0.85;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalSessionTurn {
    question: String,
    sql: String,
    #[serde(default)]
    columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalCase {
    id: String,
    category: String,
    question: String,
    #[serde(default)]
    session: Vec<EvalSessionTurn>,
    #[serde(default)]
    reference_sql: String,
    #[serde(default)]
    expect_refusal: bool,
    #[serde(default)]
    expected_result: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Deserialize)]
struct SeedCase {
    id: String,
    category: String,
    question: String,
    #[serde(default)]
    session: Vec<EvalSessionTurn>,
    #[serde(default)]
    reference_sql: String,
    #[serde(default)]
    expect_refusal: bool,
    #[serde(default)]
    mock_responses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CassetteJsonlEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_sha256: Option<String>,
    kind: String,
    response: String,
}

#[derive(Clone)]
struct EvalCassetteGateway {
    by_prompt: Arc<HashMap<String, String>>,
    by_user: Arc<HashMap<String, String>>,
}

impl EvalCassetteGateway {
    fn from_jsonl_file(path: &Path) -> Result<Self, AppError> {
        let raw = fs::read_to_string(path).map_err(|e| {
            AppError::Llm(format!("failed to read cassette {}: {e}", path.display()))
        })?;
        let mut by_prompt = HashMap::new();
        let mut by_user = HashMap::new();
        for (idx, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let entry: CassetteJsonlEntry = serde_json::from_str(trimmed).map_err(|e| {
                AppError::Llm(format!(
                    "invalid cassette line {}:{}: {e}",
                    path.display(),
                    idx.saturating_add(1)
                ))
            })?;
            if entry.kind == "chat" {
                if let Some(p) = entry.prompt_sha256 {
                    by_prompt.insert(p, entry.response.clone());
                }
                if let Some(u) = entry.user_sha256 {
                    by_user.insert(u, entry.response);
                }
            }
        }
        Ok(Self {
            by_prompt: Arc::new(by_prompt),
            by_user: Arc::new(by_user),
        })
    }
}

impl AiPort for EvalCassetteGateway {
    fn embedding_model_name(&self) -> &'static str {
        "offline-cassette-embed"
    }

    fn current_embedding_dim(&self) -> Option<usize> {
        Some(64)
    }

    fn embed_batch(
        &self,
        inputs: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![vec![0.0_f32; 64]; inputs.len()], 64)))
    }

    fn embed_one(
        &self,
        _text: &str,
    ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![0.0_f32; 64], 64)))
    }

    fn extract_triples(
        &self,
        _paragraph: &str,
    ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn extract_query_entities(
        &self,
        _query: &str,
    ) -> impl Future<Output = Result<Vec<String>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn synthesize_answer(
        &self,
        _query: &str,
        _hits: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        std::future::ready(Ok(String::new()))
    }

    fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        let p_hash = compute_prompt_sha256(system_prompt, user_prompt);
        let u_hash = sha256_hex(user_prompt);
        let found = self
            .by_prompt
            .get(&p_hash)
            .or_else(|| self.by_user.get(&u_hash))
            .cloned();
        std::future::ready(
            found.ok_or_else(|| AppError::Llm(format!("cassette miss for prompt hash {p_hash}"))),
        )
    }
}

#[derive(Clone)]
struct BootstrapRecorderAi {
    queue: Arc<Mutex<Vec<String>>>,
    recorded: Arc<Mutex<Vec<CassetteJsonlEntry>>>,
}

impl AiPort for BootstrapRecorderAi {
    fn embedding_model_name(&self) -> &'static str {
        "bootstrap-recorder"
    }

    fn current_embedding_dim(&self) -> Option<usize> {
        Some(64)
    }

    fn embed_batch(
        &self,
        inputs: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![vec![0.0_f32; 64]; inputs.len()], 64)))
    }

    fn embed_one(
        &self,
        _text: &str,
    ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![0.0_f32; 64], 64)))
    }

    fn extract_triples(
        &self,
        _paragraph: &str,
    ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn extract_query_entities(
        &self,
        _query: &str,
    ) -> impl Future<Output = Result<Vec<String>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn synthesize_answer(
        &self,
        _query: &str,
        _hits: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        std::future::ready(Ok(String::new()))
    }

    fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        if system_prompt == SUMMARY_SYSTEM_PROMPT {
            return std::future::ready(Err(AppError::Llm(String::from("skip summary in eval"))));
        }
        let resp = if system_prompt == INTENT_SYSTEM_PROMPT {
            String::from("{\"intent\":\"DATA\",\"confidence\":0.95}")
        } else {
            let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if q.is_empty() {
                return std::future::ready(Err(AppError::Llm(String::from(
                    "recorder queue empty",
                ))));
            }
            q.remove(0)
        };
        let entry = CassetteJsonlEntry {
            prompt_sha256: Some(compute_prompt_sha256(system_prompt, user_prompt)),
            user_sha256: Some(sha256_hex(user_prompt)),
            kind: String::from("chat"),
            response: resp.clone(),
        };
        self.recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry);
        std::future::ready(Ok(resp))
    }
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn compute_prompt_sha256(system_prompt: &str, user_prompt: &str) -> String {
    sha256_hex(&format!("{system_prompt}\n---\n{user_prompt}"))
}

fn col_meta(
    dataset_id: Uuid,
    name: &str,
    data_type: DataType,
    pii: PiiCategory,
    term: &str,
    ordinal: i32,
) -> ColumnMeta {
    ColumnMeta {
        dataset_id,
        column_name: String::from(name),
        data_type,
        pii_category: pii,
        pii_confirmed: pii != PiiCategory::None,
        description: String::from(term),
        business_term: String::from(term),
        unit: String::new(),
        ordinal,
    }
}

struct EvalFixture {
    conn: duckdb::Connection,
    catalog: TenantCatalog,
    metrics: Vec<MetricDef>,
}

fn build_eval_fixture() -> Result<EvalFixture, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open duckdb: {e}")))?;
    seed_eval_duckdb_tables(&conn)?;

    let orders_id = Uuid::from_u128(0x1001);
    let customers_id = Uuid::from_u128(0x1002);
    let warehouses_id = Uuid::from_u128(0x1003);

    let orders_ds = build_orders_dataset_entry(orders_id);
    let customers_ds = build_customers_dataset_entry(customers_id);
    let warehouses_ds = build_warehouses_dataset_entry(warehouses_id);

    let catalog = TenantCatalog::new("default")
        .with_dataset(orders_ds)
        .with_dataset(customers_ds)
        .with_dataset(warehouses_ds);
    let metrics = build_eval_metrics(orders_id);

    Ok(EvalFixture {
        conn,
        catalog,
        metrics,
    })
}

fn seed_eval_duckdb_tables(conn: &duckdb::Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_default\";
         CREATE TABLE lake.\"tenant_default\".\"demo_orders\" (
             order_id VARCHAR,
             order_date DATE,
             customer_id VARCHAR,
             phone VARCHAR,
             region VARCHAR,
             warehouse_code VARCHAR,
             status VARCHAR,
             qty BIGINT,
             amount DOUBLE
         );
         INSERT INTO lake.\"tenant_default\".\"demo_orders\" VALUES
             ('O-01', DATE '2025-01-10', 'C1', '13800001111', '华东', 'W-1', 'PAID',     2, 200.0),
             ('O-02', DATE '2025-01-22', 'C2', '13800002222', '华南', 'W-2', 'PAID',     1, 150.0),
             ('O-03', DATE '2025-02-14', 'C1', '13800001111', '华东', 'W-1', 'PAID',     3, 300.0),
             ('O-04', DATE '2025-02-18', 'C3', '13800003333', '华北', 'W-3', 'REFUNDED', 1,  80.0),
             ('O-05', DATE '2026-01-05', 'C1', '13800001111', '华东', 'W-1', 'PAID',     4, 400.0),
             ('O-06', DATE '2026-01-16', 'C4', '13800004444', '华东', 'W-1', 'PAID',     2, 160.0),
             ('O-07', DATE '2026-01-25', 'C2', '13800002222', '华南', 'W-2', 'REFUNDED', 1,  90.0),
             ('O-08', DATE '2026-02-03', 'C1', '13800001111', '华东', 'W-1', 'PAID',     5, 500.0),
             ('O-09', DATE '2026-02-09', 'C5', '13800005555', '华南', 'W-2', 'PAID',     3, 270.0),
             ('O-10', DATE '2026-02-15', 'C3', '13800003333', '华北', 'W-3', 'PAID',     2, 180.0),
             ('O-11', DATE '2026-02-20', 'C6', '13800006666', '华北', 'W-3', 'PENDING',  1, 110.0),
             ('O-12', DATE '2026-02-26', 'C2', '13800002222', '华南', 'W-2', 'REFUNDED', 2, 120.0);

         CREATE TABLE lake.\"tenant_default\".\"customers\" (
             customer_id VARCHAR,
             customer_name VARCHAR,
             tier VARCHAR,
             region VARCHAR,
             city VARCHAR
         );
         INSERT INTO lake.\"tenant_default\".\"customers\" VALUES
             ('C1', '张伟', 'VIP',      '华东', '上海'),
             ('C2', '李娜', 'VIP',      '华南', '深圳'),
             ('C3', '王芳', 'STANDARD', '华北', '北京'),
             ('C4', '赵敏', 'STANDARD', '华东', '杭州'),
             ('C5', '陈静', 'VIP',      '华南', '广州'),
             ('C6', '刘洋', 'STANDARD', '华北', '北京');

         CREATE TABLE lake.\"tenant_default\".\"warehouses\" (
             warehouse_code VARCHAR,
             warehouse_name VARCHAR,
             region VARCHAR,
             inventory_qty BIGINT
         );
         INSERT INTO lake.\"tenant_default\".\"warehouses\" VALUES
             ('W-1', '华东仓', '华东', 1200),
             ('W-2', '华南仓', '华南',  850),
             ('W-3', '华北仓', '华北',  640);",
    )
    .map_err(|e| AppError::Database(format!("seed eval tables: {e}")))
}

fn build_orders_dataset_entry(orders_id: Uuid) -> CatalogDatasetEntry {
    let mut ds = CatalogDatasetEntry::simple(orders_id, "demo_orders", "demo_orders");
    for (ord, (name, dt, pii, term)) in [
        ("order_id", DataType::String, PiiCategory::None, "订单编号"),
        ("order_date", DataType::Date, PiiCategory::None, "订单日期"),
        ("customer_id", DataType::String, PiiCategory::None, "客户ID"),
        ("phone", DataType::String, PiiCategory::Phone, "手机号"),
        ("region", DataType::String, PiiCategory::None, "区域大区"),
        (
            "warehouse_code",
            DataType::String,
            PiiCategory::None,
            "仓库编码",
        ),
        ("status", DataType::String, PiiCategory::None, "订单状态"),
        ("qty", DataType::Int64, PiiCategory::None, "商品数量"),
        ("amount", DataType::Float64, PiiCategory::None, "订单金额"),
    ]
    .into_iter()
    .enumerate()
    {
        ds.columns.insert(
            String::from(name),
            col_meta(
                orders_id,
                name,
                dt,
                pii,
                term,
                i32::try_from(ord).unwrap_or(0),
            ),
        );
    }
    ds
}

fn build_customers_dataset_entry(customers_id: Uuid) -> CatalogDatasetEntry {
    let mut ds = CatalogDatasetEntry::simple(customers_id, "customers", "customers");
    for (ord, (name, dt, term)) in [
        ("customer_id", DataType::String, "客户ID"),
        ("customer_name", DataType::String, "客户姓名"),
        ("tier", DataType::String, "客户等级"),
        ("region", DataType::String, "所属区域"),
        ("city", DataType::String, "所在城市"),
    ]
    .into_iter()
    .enumerate()
    {
        ds.columns.insert(
            String::from(name),
            col_meta(
                customers_id,
                name,
                dt,
                PiiCategory::None,
                term,
                i32::try_from(ord).unwrap_or(0),
            ),
        );
    }
    ds
}

fn build_warehouses_dataset_entry(warehouses_id: Uuid) -> CatalogDatasetEntry {
    let mut ds = CatalogDatasetEntry::simple(warehouses_id, "warehouses", "warehouses");
    for (ord, (name, dt, term)) in [
        ("warehouse_code", DataType::String, "仓库编码"),
        ("warehouse_name", DataType::String, "仓库名称"),
        ("region", DataType::String, "仓库大区"),
        ("inventory_qty", DataType::Int64, "库存数量"),
    ]
    .into_iter()
    .enumerate()
    {
        ds.columns.insert(
            String::from(name),
            col_meta(
                warehouses_id,
                name,
                dt,
                PiiCategory::None,
                term,
                i32::try_from(ord).unwrap_or(0),
            ),
        );
    }
    ds
}

fn build_eval_metrics(orders_id: Uuid) -> Vec<MetricDef> {
    let fixed_dt = chrono::DateTime::parse_from_rfc3339(FIXED_NOW_RFC3339)
        .map_or_else(|_| Utc::now(), |d| d.with_timezone(&Utc));
    vec![
        MetricDef {
            id: Uuid::from_u128(0x2001),
            name: String::from("paid_amount"),
            display_name: String::from("已支付金额"),
            description: String::from("已支付订单总金额"),
            dataset_id: orders_id,
            expression: String::from("SUM(amount)"),
            filter_sql: String::from("status = 'PAID'"),
            time_column: Some(String::from("order_date")),
            default_grain: RollupGrain::Month,
            dimensions: vec![
                MetricDimension {
                    name: String::from("region"),
                    column: String::from("region"),
                    display_name: String::from("区域"),
                },
                MetricDimension {
                    name: String::from("warehouse_code"),
                    column: String::from("warehouse_code"),
                    display_name: String::from("仓库编码"),
                },
            ],
            unit: String::from("CNY"),
            format: String::from("0.00"),
            synonyms: vec![String::from("实付GMV"), String::from("已支付订单总金额")],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: fixed_dt,
            updated_at: fixed_dt,
        },
        MetricDef {
            id: Uuid::from_u128(0x2002),
            name: String::from("refund_amount"),
            display_name: String::from("退款金额"),
            description: String::from("已退款订单总金额"),
            dataset_id: orders_id,
            expression: String::from("SUM(amount)"),
            filter_sql: String::from("status = 'REFUNDED'"),
            time_column: Some(String::from("order_date")),
            default_grain: RollupGrain::Month,
            dimensions: vec![MetricDimension {
                name: String::from("region"),
                column: String::from("region"),
                display_name: String::from("区域"),
            }],
            unit: String::from("CNY"),
            format: String::from("0.00"),
            synonyms: vec![String::from("退单金额")],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: fixed_dt,
            updated_at: fixed_dt,
        },
    ]
}

fn recall_nodes_for_question(question: &str) -> SchemaRecallBundle {
    let mut matched_nodes = Vec::new();
    for (key, title) in [("W-1", "华东仓"), ("W-2", "华南仓"), ("W-3", "华北仓")] {
        if question.contains(title) {
            matched_nodes.push((
                String::from("Warehouse"),
                String::from(key),
                String::from(title),
            ));
        }
    }
    SchemaRecallBundle {
        columns: Vec::new(),
        metrics: Vec::new(),
        matched_nodes,
    }
}

fn convert_session_turns(turns: &[EvalSessionTurn], session_id: Uuid) -> Vec<AskTurnRecord> {
    let now = Utc::now();
    turns
        .iter()
        .map(|t| {
            let cols_json: Vec<serde_json::Value> =
                t.columns.iter().map(|c| json!({"name": c})).collect();
            AskTurnRecord {
                id: Uuid::new_v4(),
                session_id,
                user_id: None,
                question: t.question.clone(),
                intent: AskIntent::Data,
                linked_objects: LinkedObjects::default(),
                sql_text: Some(t.sql.clone()),
                result_preview: Some(json!({"columns": cols_json})),
                answer: String::new(),
                citations: Vec::new(),
                feedback: None,
                corrected_sql: None,
                model_name: String::from("eval"),
                created_at: now,
                grounding_mode: axpoint_domain::enums::GroundingMode::GuardedSql,
                verified_numbers: 0,
                verified_citations: 0,
                abstained: false,
                slot_payload: None,
            }
        })
        .collect()
}

fn execute_reference_rows(
    conn: &duckdb::Connection,
    catalog: &TenantCatalog,
    ref_sql: &str,
) -> Result<Vec<Vec<serde_json::Value>>, AppError> {
    if ref_sql.trim().is_empty() {
        return Ok(Vec::new());
    }
    let guarded = guard(
        ref_sql,
        catalog,
        &axpoint_analytics::GuardOptions::default(),
    )?;
    let mut stmt = conn
        .prepare(&guarded.sql)
        .map_err(|e| AppError::Database(format!("ref prepare: {e}")))?;
    let stream = stmt
        .query_arrow([])
        .map_err(|e| AppError::Database(format!("ref query: {e}")))?;
    let schema = stream.get_schema();
    let batches: Vec<arrow::array::RecordBatch> = stream.collect();
    let ser = axpoint_analytics::batches_to_query_result(
        &schema,
        &batches,
        axpoint_analytics::ResultSerializeOptions {
            effective_max_rows: guarded.effective_max_rows,
            sql: guarded.sql.clone(),
            duration_ms: 0,
            reveal_pii: false,
            has_pii_raw_read: false,
            catalog,
            guarded: &guarded,
        },
    )?;
    Ok(extract_ordered_row_tuples(
        &ser.result.columns,
        &ser.result.rows,
    ))
}

fn extract_ordered_row_tuples(
    columns: &[axpoint_analytics::QueryColumn],
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> Vec<Vec<serde_json::Value>> {
    rows.iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(&c.name).cloned().unwrap_or(serde_json::Value::Null))
                .collect()
        })
        .collect()
}

fn results_match_ignoring_col_names_and_order(
    actual_tuples: &[Vec<serde_json::Value>],
    expected_tuples: &[Vec<serde_json::Value>],
) -> bool {
    if actual_tuples.len() != expected_tuples.len() {
        return false;
    }
    let mut norm_actual: Vec<Vec<String>> = actual_tuples
        .iter()
        .map(|r| r.iter().map(canonical_cell).collect())
        .collect();
    let mut norm_expected: Vec<Vec<String>> = expected_tuples
        .iter()
        .map(|r| r.iter().map(canonical_cell).collect())
        .collect();
    norm_actual.sort();
    norm_expected.sort();
    norm_actual == norm_expected
}

fn canonical_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::from("NULL"),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                format!("{f:.4}")
            } else {
                n.to_string()
            }
        }
        serde_json::Value::String(s) => {
            if let Ok(f) = s.trim().parse::<f64>()
                && f.is_finite()
            {
                return format!("{f:.4}");
            }
            // Normalize date/timestamp prefix `YYYY-MM-DD` when comparing `DATE` vs `TIMESTAMP` buckets
            if s.len() >= 10
                && s.as_bytes().get(4) == Some(&b'-')
                && s.as_bytes().get(7) == Some(&b'-')
            {
                return String::from(s.get(..10).unwrap_or(s));
            }
            s.trim().to_owned()
        }
        other => other.to_string(),
    }
}

fn eval_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("nl2sql_eval")
}

async fn bootstrap_eval_fixtures_from_seed(
    seed_path: &Path,
    fixture: &EvalFixture,
) -> Result<(), AppError> {
    let seed_raw =
        fs::read_to_string(seed_path).map_err(|e| AppError::Config(format!("read seed: {e}")))?;
    let seeds: Vec<SeedCase> = serde_json::from_str(&seed_raw)
        .map_err(|e| AppError::Config(format!("parse seed: {e}")))?;

    let mut out_cases = Vec::with_capacity(seeds.len());
    let captured_entries = Arc::new(Mutex::new(Vec::new()));

    for sc in seeds {
        let expected_result =
            execute_reference_rows(&fixture.conn, &fixture.catalog, &sc.reference_sql)?;
        let capture_ai = BootstrapRecorderAi {
            queue: Arc::new(Mutex::new(sc.mock_responses)),
            recorded: Arc::clone(&captured_entries),
        };
        let session_id = Uuid::from_u128(0x9999);
        let turns = convert_session_turns(&sc.session, session_id);
        let recall = recall_nodes_for_question(&sc.question);
        let extras = SchemaLinkExtras {
            all_metrics: fixture.metrics.clone(),
            ..SchemaLinkExtras::default()
        };
        let req = AskRequest {
            question: sc.question.clone(),
            session_id: Some(session_id),
            context: None,
        };
        let _ = run_ask_pipeline_conn(
            &capture_ai,
            &fixture.conn,
            &AskPipelineInput {
                request: &req,
                catalog: &fixture.catalog,
                db_recall: &recall,
                link_extras: &extras,
                few_shots: &[],
                session_turns: &turns,
                timezone: "Asia/Shanghai",
                now_rfc3339_override: Some(FIXED_NOW_RFC3339),
                timeout_ms: None,
                force_intent: None,
            },
        )
        .await?;

        out_cases.push(EvalCase {
            id: sc.id,
            category: sc.category,
            question: sc.question,
            session: sc.session,
            reference_sql: sc.reference_sql,
            expect_refusal: sc.expect_refusal,
            expected_result,
        });
    }

    let dir = eval_dir();
    fs::create_dir_all(&dir).map_err(|e| AppError::Config(e.to_string()))?;
    let cassettes_alt_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cassettes");
    fs::create_dir_all(&cassettes_alt_dir).map_err(|e| AppError::Config(e.to_string()))?;

    let yaml_text = format!(
        "# YAML 1.2 (JSON-compatible) 60-case NL2SQL benchmark suite (`05-STEP04` §04.6.8)\n{}\n",
        serde_json::to_string_pretty(&out_cases).map_err(|e| AppError::Config(e.to_string()))?
    );
    fs::write(dir.join("cases.yaml"), yaml_text).map_err(|e| AppError::Config(e.to_string()))?;

    let entries = captured_entries
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut jsonl_buf = String::new();
    for entry in entries.iter() {
        let line = serde_json::to_string(entry).map_err(|e| AppError::Config(e.to_string()))?;
        jsonl_buf.push_str(&line);
        jsonl_buf.push('\n');
    }
    fs::write(dir.join("cassette.jsonl"), &jsonl_buf)
        .map_err(|e| AppError::Config(e.to_string()))?;
    fs::write(cassettes_alt_dir.join("nl2sql_eval.jsonl"), &jsonl_buf)
        .map_err(|e| AppError::Config(e.to_string()))?;
    Ok(())
}

fn load_eval_cases(path: &Path) -> Result<Vec<EvalCase>, AppError> {
    let raw = fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("failed to read {}: {e}", path.display())))?;
    let stripped: String = raw
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&stripped)
        .map_err(|e| AppError::Config(format!("failed to parse cases.yaml: {e}")))
}

#[tokio::test]
async fn nl2sql_eval_replay_60_cases_meets_accuracy_and_refusal_gates() -> Result<(), AppError> {
    let fixture = build_eval_fixture()?;
    if let Ok(seed_file) = std::env::var("AXPOINT_BOOTSTRAP_EVAL_SEED")
        && !seed_file.trim().is_empty()
    {
        bootstrap_eval_fixtures_from_seed(Path::new(seed_file.trim()), &fixture).await?;
    }

    let dir = eval_dir();
    let cases = load_eval_cases(&dir.join("cases.yaml"))?;
    assert!(
        cases.len() >= 60,
        "expected >= 60 eval cases, got {}",
        cases.len()
    );

    let gateway = EvalCassetteGateway::from_jsonl_file(&dir.join("cassette.jsonl"))?;
    let (passed, refusal_total, refusal_passed, total_repairs, total_ms) =
        run_all_eval_cases(&cases, &fixture, &gateway).await?;

    let total_f = f64::from(u32::try_from(cases.len()).unwrap_or(1));
    let passed_f = f64::from(u32::try_from(passed).unwrap_or(0));
    let accuracy = passed_f / total_f;
    let avg_repairs = f64::from(u32::try_from(total_repairs).unwrap_or(0)) / total_f;
    let avg_ms = f64::from(u32::try_from(total_ms).unwrap_or(0)) / total_f;

    if std::env::var("AXPOINT_EVAL_LIVE").ok().as_deref() == Some("1") {
        std::io::Write::write_fmt(
            &mut std::io::stdout(),
            format_args!(
                "[nl2sql_eval] cases={} accuracy={:.1}% refusal={}/{} avg_repairs={:.2} avg_ms={:.1}\n",
                cases.len(),
                accuracy * 100.0,
                refusal_passed,
                refusal_total,
                avg_repairs,
                avg_ms
            ),
        )
        .map_err(|e| AppError::Config(e.to_string()))?;
    }

    assert_eq!(
        refusal_passed, refusal_total,
        "refusal accuracy must be 100%"
    );
    assert!(
        accuracy >= MIN_OVERALL_ACCURACY,
        "NL2SQL accuracy {:.2}% below required {:.0}% threshold",
        accuracy * 100.0,
        MIN_OVERALL_ACCURACY * 100.0
    );
    Ok(())
}

async fn run_all_eval_cases(
    cases: &[EvalCase],
    fixture: &EvalFixture,
    gateway: &EvalCassetteGateway,
) -> Result<(usize, usize, usize, usize, u64), AppError> {
    let mut passed: usize = 0;
    let mut refusal_total: usize = 0;
    let mut refusal_passed: usize = 0;
    let mut total_repairs: usize = 0;
    let mut total_ms: u64 = 0;

    for case in cases {
        let t0 = Instant::now();
        let session_id = Uuid::from_u128(0x9999);
        let turns = convert_session_turns(&case.session, session_id);
        let recall = recall_nodes_for_question(&case.question);
        let extras = SchemaLinkExtras {
            all_metrics: fixture.metrics.clone(),
            ..SchemaLinkExtras::default()
        };
        let req = AskRequest {
            question: case.question.clone(),
            session_id: Some(session_id),
            context: None,
        };
        let out = run_ask_pipeline_conn(
            gateway,
            &fixture.conn,
            &AskPipelineInput {
                request: &req,
                catalog: &fixture.catalog,
                db_recall: &recall,
                link_extras: &extras,
                few_shots: &[],
                session_turns: &turns,
                timezone: "Asia/Shanghai",
                now_rfc3339_override: Some(FIXED_NOW_RFC3339),
                timeout_ms: None,
                force_intent: None,
            },
        )
        .await?;

        total_ms = total_ms.saturating_add(u64::try_from(t0.elapsed().as_millis()).unwrap_or(0));
        total_repairs = total_repairs.saturating_add(out.repair_rounds);

        if case.expect_refusal {
            refusal_total = refusal_total.saturating_add(1);
            let is_refused = out.response.need_clarification
                || matches!(out.response.intent, AskIntent::Chitchat | AskIntent::Doc)
                || out.response.status == QueryHistoryStatus::Failed
                || (out.response.sql.is_empty() && out.response.rows.is_empty());
            if is_refused {
                refusal_passed = refusal_passed.saturating_add(1);
                passed = passed.saturating_add(1);
            }
        } else if out.response.status == QueryHistoryStatus::Succeeded
            && !out.response.sql.is_empty()
        {
            let actual = extract_ordered_row_tuples(&out.response.columns, &out.response.rows);
            if results_match_ignoring_col_names_and_order(&actual, &case.expected_result) {
                passed = passed.saturating_add(1);
            }
        }
    }
    Ok((
        passed,
        refusal_total,
        refusal_passed,
        total_repairs,
        total_ms,
    ))
}
