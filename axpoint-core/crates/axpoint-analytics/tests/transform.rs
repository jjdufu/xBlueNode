//! Integration tests for `nl2sql` self-repair, grounded summary verification, chart heuristics,
//! and `transform` preview/schema-diff/cycle-detection (`05-STEP04` §04.6–§04.7).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axpoint_analytics::nl2sql::chart::{recommend_chart, verify_or_fallback_summary};
use axpoint_analytics::nl2sql::schema_link::SchemaLinkExtras;
use axpoint_analytics::{
    AskIntent, AskPipelineInput, AskRequest, CatalogDatasetEntry, QueryColumn, QueryHistoryStatus,
    SchemaRecallBundle, TenantCatalog, ensure_no_dependency_cycle, generate_transform_sql,
    preview_transform_sql_conn, run_ask_pipeline_conn, topological_downstream_dependents,
    validate_corrected_sql, validate_target_dataset_name,
};
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, PiiCategory, WidgetKind};
use axpoint_domain::ports::AiPort;
use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
use serde_json::json;
use uuid::Uuid;

#[derive(Clone, Default)]
struct ScriptedAi {
    responses: Arc<Mutex<Vec<String>>>,
}

impl ScriptedAi {
    fn with_queue(items: Vec<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(items)),
        }
    }
}

impl AiPort for ScriptedAi {
    fn embedding_model_name(&self) -> &'static str {
        "offline-test"
    }

    fn current_embedding_dim(&self) -> Option<usize> {
        Some(64)
    }

    fn embed_batch(
        &self,
        inputs: &[String],
    ) -> impl std::future::Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![vec![0.0_f32; 64]; inputs.len()], 64)))
    }

    fn embed_one(
        &self,
        _text: &str,
    ) -> impl std::future::Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![0.0_f32; 64], 64)))
    }

    fn extract_triples(
        &self,
        _paragraph: &str,
    ) -> impl std::future::Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn extract_query_entities(
        &self,
        _query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<String>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn synthesize_answer(
        &self,
        _query: &str,
        _hits: &[FusedHit],
    ) -> impl std::future::Future<Output = Result<String, AppError>> + Send {
        std::future::ready(Ok(String::new()))
    }

    fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
    ) -> impl std::future::Future<Output = Result<String, AppError>> + Send {
        let mut q = self
            .responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.is_empty() {
            return std::future::ready(Err(AppError::Llm(String::from("queue empty"))));
        }
        std::future::ready(Ok(q.remove(0)))
    }
}

fn col_meta(
    dataset_id: Uuid,
    name: &str,
    data_type: DataType,
    pii: PiiCategory,
    ordinal: i32,
) -> ColumnMeta {
    ColumnMeta {
        dataset_id,
        column_name: String::from(name),
        data_type,
        pii_category: pii,
        pii_confirmed: pii != PiiCategory::None,
        description: String::new(),
        business_term: String::new(),
        unit: String::new(),
        ordinal,
    }
}

fn setup_orders_conn() -> Result<(duckdb::Connection, TenantCatalog, Uuid), AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open duckdb: {e}")))?;
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_default\";
         CREATE TABLE lake.\"tenant_default\".\"raw_orders\" (
             order_id VARCHAR,
             status VARCHAR,
             phone VARCHAR,
             amount DOUBLE
         );
         INSERT INTO lake.\"tenant_default\".\"raw_orders\" VALUES
             ('O1', 'PAID', '13812345678', 100.0),
             ('O1', 'PAID', '13812345678', 100.0),
             ('O2', 'CANCELLED', '13987654321', 50.0),
             ('O3', 'PAID', '13700001111', 250.0);",
    )
    .map_err(|e| AppError::Database(format!("seed raw_orders: {e}")))?;

    let ds_id = Uuid::new_v4();
    let mut ds = CatalogDatasetEntry::simple(ds_id, "raw_orders", "raw_orders");
    ds.columns.insert(
        String::from("order_id"),
        col_meta(ds_id, "order_id", DataType::String, PiiCategory::None, 0),
    );
    ds.columns.insert(
        String::from("status"),
        col_meta(ds_id, "status", DataType::String, PiiCategory::None, 1),
    );
    ds.columns.insert(
        String::from("phone"),
        col_meta(ds_id, "phone", DataType::String, PiiCategory::Phone, 2),
    );
    ds.columns.insert(
        String::from("amount"),
        col_meta(ds_id, "amount", DataType::Float64, PiiCategory::None, 3),
    );
    let catalog = TenantCatalog::new("default").with_dataset(ds);
    Ok((conn, catalog, ds_id))
}

#[tokio::test]
async fn transform_generates_previews_masks_pii_and_computes_column_diff() -> Result<(), AppError> {
    let (conn, catalog, ds_id) = setup_orders_conn()?;
    validate_target_dataset_name("clean_paid_orders", &catalog)?;
    assert!(matches!(
        validate_target_dataset_name("raw_orders", &catalog),
        Err(AppError::Conflict("NAME_TAKEN"))
    ));
    assert_eq!(
        validate_target_dataset_name("123bad", &catalog)
            .err()
            .as_ref()
            .map(AppError::error_code),
        Some("INVALID_ARGUMENT")
    );

    let ai = ScriptedAi::with_queue(vec![String::from(
        "{\"sql\": \"SELECT DISTINCT order_id, phone, amount, amount * 1.13 AS amount_with_tax FROM raw_orders WHERE status = 'PAID'\"}",
    )]);
    let sql = generate_transform_sql(
        &ai,
        "去重已支付订单并增加含税金额列，去掉status列",
        &[ds_id],
        &catalog,
    )
    .await?;
    let plan = preview_transform_sql_conn(&conn, &catalog, &sql, &[ds_id])?;

    assert_eq!(plan.depends_on, vec![ds_id]);
    assert!(
        plan.materialized_sql
            .contains("lake.\"tenant_default\".\"raw_orders\"")
    );
    assert!(!plan.materialized_sql.to_ascii_uppercase().contains("LIMIT"));
    assert_eq!(plan.preview.rows.len(), 2);
    assert_eq!(plan.preview.added_columns, vec!["amount_with_tax"]);
    assert_eq!(plan.preview.removed_columns, vec!["status"]);
    // PII phone column in preview MUST be masked!
    let phones: Vec<&str> = plan
        .preview
        .rows
        .iter()
        .filter_map(|r| r.get("phone").and_then(serde_json::Value::as_str))
        .collect();
    assert!(phones.contains(&"138****5678"));
    Ok(())
}

#[test]
fn transform_detects_dependency_cycles_and_orders_downstream_topologically() -> Result<(), AppError>
{
    let raw = Uuid::from_u128(1);
    let stage1 = Uuid::from_u128(2);
    let stage2 = Uuid::from_u128(3);
    let stage3 = Uuid::from_u128(4);

    let mut graph: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    graph.insert(stage1, vec![raw]);
    graph.insert(stage2, vec![stage1]);
    graph.insert(stage3, vec![raw, stage2]);

    ensure_no_dependency_cycle(Uuid::from_u128(5), &[stage3], &graph)?;
    // Adding `stage1 -> stage3` creates cycle `stage1 -> stage3 -> stage2 -> stage1`!
    let cyc = ensure_no_dependency_cycle(stage1, &[stage3], &graph);
    assert_eq!(
        cyc.err().as_ref().map(AppError::error_code),
        Some("CYCLE_DETECTED")
    );

    let topo = topological_downstream_dependents(raw, &graph)?;
    assert_eq!(topo, vec![stage1, stage2, stage3]);
    Ok(())
}

#[test]
fn chart_and_summary_verifier_enforce_hallucination_free_numbers() {
    let rows = vec![
        serde_json::from_value(json!({"region": "华东", "amt": 1200.5, "share": 0.25}))
            .unwrap_or_default(),
    ];
    // Grounded numbers (including comma separator and percentage conversion) pass!
    let valid = "华东大区销售额为 1,200.5，占比达 25%。";
    assert_eq!(verify_or_fallback_summary(valid, &rows), valid);

    // Hallucinated number `999` triggers immediate fallback template!
    let hallucinated = "华东大区销售额为 1,200.5，比去年增长 999 元。";
    assert_eq!(
        verify_or_fallback_summary(hallucinated, &rows),
        "查询返回 1 行，主要结果见表格"
    );

    let kpi_col = vec![QueryColumn {
        name: String::from("total"),
        data_type: DataType::Float64,
        pii_category: PiiCategory::None,
        masked: false,
    }];
    assert_eq!(
        recommend_chart("总额是多少", &kpi_col, 1).kind,
        WidgetKind::KpiCard
    );

    let cat_num = vec![
        QueryColumn {
            name: String::from("region"),
            data_type: DataType::String,
            pii_category: PiiCategory::None,
            masked: false,
        },
        QueryColumn {
            name: String::from("total"),
            data_type: DataType::Float64,
            pii_category: PiiCategory::None,
            masked: false,
        },
    ];
    assert_eq!(
        recommend_chart("各区域占比是多少", &cat_num, 4).kind,
        WidgetKind::PieChart
    );
    assert_eq!(
        recommend_chart("各区域销售额对比", &cat_num, 4).kind,
        WidgetKind::BarChart
    );
}

#[tokio::test]
async fn nl2sql_self_repairs_invalid_column_and_stops_after_two_failed_repairs()
-> Result<(), AppError> {
    let (conn, catalog, _) = setup_orders_conn()?;
    assert!(validate_corrected_sql("SELECT count(*) FROM raw_orders", &catalog).is_ok());
    assert!(validate_corrected_sql("COPY raw_orders TO '/tmp/x'", &catalog).is_err());

    // Round 0 emits nonexistent column `wrong_amt`, Round 1 repairs to `amount`, Summary grounded!
    let ai_ok = ScriptedAi::with_queue(vec![
        String::from(
            "{\"sql\": \"SELECT SUM(wrong_amt) AS total FROM raw_orders WHERE status = 'PAID'\", \"explanation\": \"draft\"}",
        ),
        String::from(
            "{\"sql\": \"SELECT SUM(amount) AS total FROM raw_orders WHERE status = 'PAID'\", \"explanation\": \"repaired\"}",
        ),
        String::from("已支付订单总金额为 450。"),
    ]);
    let req = AskRequest {
        question: String::from("已支付订单的总金额是多少"),
        session_id: None,
        context: None,
    };
    let recall = SchemaRecallBundle::default();
    let extras = SchemaLinkExtras::default();
    let out = run_ask_pipeline_conn(
        &ai_ok,
        &conn,
        &AskPipelineInput {
            request: &req,
            catalog: &catalog,
            db_recall: &recall,
            link_extras: &extras,
            few_shots: &[],
            session_turns: &[],
            timezone: "Asia/Shanghai",
            now_rfc3339_override: Some("2026-03-01T00:00:00Z"),
            timeout_ms: None,
            force_intent: None,
        },
    )
    .await?;

    assert_eq!(out.response.intent, AskIntent::Data);
    assert_eq!(out.response.status, QueryHistoryStatus::Succeeded);
    assert_eq!(out.repair_rounds, 1);
    assert_eq!(out.response.answer, "已支付订单总金额为 450。");

    // Exhausting all 3 attempts (initial + 2 repairs) returns status=FAILED without executing!
    let ai_fail = ScriptedAi::with_queue(vec![
        String::from("{\"sql\": \"SELECT bad1 FROM raw_orders\", \"explanation\": \"\"}"),
        String::from("{\"sql\": \"SELECT bad2 FROM raw_orders\", \"explanation\": \"\"}"),
        String::from("{\"sql\": \"SELECT bad3 FROM raw_orders\", \"explanation\": \"\"}"),
    ]);
    let fail_out = run_ask_pipeline_conn(
        &ai_fail,
        &conn,
        &AskPipelineInput {
            request: &req,
            catalog: &catalog,
            db_recall: &recall,
            link_extras: &extras,
            few_shots: &[],
            session_turns: &[],
            timezone: "UTC",
            now_rfc3339_override: Some("2026-03-01T00:00:00Z"),
            timeout_ms: None,
            force_intent: None,
        },
    )
    .await?;
    assert_eq!(fail_out.response.status, QueryHistoryStatus::Failed);
    assert_eq!(fail_out.repair_rounds, 2);
    Ok(())
}
