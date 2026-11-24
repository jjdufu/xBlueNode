//! Cassette-backed 10-case evaluation suite for Step 11 `ai_gen` (`AiDashboardDraft` generation,
//! 24-column bounds, valid `WidgetKind`, entity/metric/dataset/asset/flow bindings, and
//! 1-round LLM self-repair) enforcing >= 90% accuracy (`10-STEP11-12` §11.6, `A5` §4).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use axpoint_analytics::CatalogDatasetEntry;
use axpoint_domain::AppError;
use axpoint_domain::analytics::{MetricDef, MetricDimension};
use axpoint_domain::axhub::{ColumnMeta, Dataset, DatasetStatus, SchemaPolicy};
use axpoint_domain::enums::{
    AssetKind, DataType, PiiCategory, RollupGrain, SourceKind, SyncMode, TriggerType, WidgetKind,
};
use axpoint_domain::flow::TriggerCfg;
use axpoint_domain::studio::{EdgeTypeSpec, NodeTypeSpec, PropertyKind, PropertySpec};
use axpoint_domain::topology::AxFlowDef;
use axpoint_domain::view::{AiGenerateDashboardRequest, AssetRecord};
use axpoint_llm::{
    AiGateway, CASSETTE_ENV_VAR, CassetteMode, EVAL_RECORD_ENV_VAR, EmbeddingConfig, LlmConfig,
};
use axpoint_server::view::ai_gen::{
    DashboardGenContext, draft_dashboard_with_ai, validate_ai_dashboard_draft,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

const MIN_EVAL_ACCURACY: f64 = 0.90;

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    prompt: String,
    expect_min_widgets: usize,
    #[serde(default)]
    expect_kinds: Vec<WidgetKind>,
}

#[derive(Debug)]
enum EvalMode {
    Replay(PathBuf),
    Record(PathBuf),
    Live,
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        let trimmed = v.trim();
        trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
    })
}

fn env_non_empty(primary: &str, fallback: &str, default_val: &str) -> String {
    std::env::var(primary)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var(fallback)
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| String::from(default_val))
}

fn resolve_cassette_path() -> PathBuf {
    std::env::var(CASSETTE_ENV_VAR)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/cassettes/step11.jsonl"),
            PathBuf::from,
        )
}

fn resolve_eval_mode() -> EvalMode {
    if env_flag_enabled(EVAL_RECORD_ENV_VAR) {
        EvalMode::Record(resolve_cassette_path())
    } else if env_flag_enabled("AXPOINT_EVAL_LIVE") {
        EvalMode::Live
    } else {
        EvalMode::Replay(resolve_cassette_path())
    }
}

fn build_eval_gateway(mode: &EvalMode) -> Result<(AiGateway, Option<PathBuf>), AppError> {
    let llm = LlmConfig {
        api_base: env_non_empty(
            "AXPOINT_LLM_API_BASE",
            "OPENAI_BASE_URL",
            "http://127.0.0.1:9",
        ),
        api_key: env_non_empty("AXPOINT_LLM_API_KEY", "OPENAI_API_KEY", ""),
        model: env_non_empty("AXPOINT_LLM_MODEL", "OPENAI_MODEL", "offline-eval-model"),
    };
    let embed = EmbeddingConfig {
        api_base: String::from("http://127.0.0.1:9"),
        api_key: String::new(),
        model: String::from("offline-embed-model"),
        configured_dim: Some(64),
    };

    match mode {
        EvalMode::Replay(path) => Ok((
            AiGateway::with_cassette(llm, embed, path, CassetteMode::Replay)?,
            None,
        )),
        EvalMode::Record(path) => Ok((
            AiGateway::with_cassette(llm, embed, path, CassetteMode::Record)?,
            None,
        )),
        EvalMode::Live => {
            let scratch = std::env::temp_dir().join(format!(
                "axpoint_dashboard_gen_live_{}.jsonl",
                Uuid::new_v4()
            ));
            let gw = AiGateway::with_cassette(llm, embed, &scratch, CassetteMode::Record)?;
            Ok((gw, Some(scratch)))
        }
    }
}

fn build_eval_context() -> DashboardGenContext {
    let ds_orders_id = Uuid::from_u128(0x101);
    let mut ctx = DashboardGenContext {
        catalog: axpoint_analytics::TenantCatalog::new("default"),
        ..DashboardGenContext::default()
    };
    ctx.schema.node_types = vec![
        make_node_type("Factory", "工厂", &["output"]),
        make_node_type("Warehouse", "仓库", &["inventory", "fill_rate"]),
        make_node_type("Order", "订单", &["demand"]),
    ];
    ctx.schema.edge_types = vec![EdgeTypeSpec {
        id: None,
        name: String::from("SUPPLIES"),
        src_type_name: String::from("Factory"),
        dst_type_name: String::from("Warehouse"),
        impact_weight: 0.7,
        cardinality: axpoint_domain::enums::Cardinality::ManyToMany,
        description: String::from("供应"),
        properties: Vec::new(),
        propagation: serde_json::json!({}),
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        derived_rules: Vec::new(),
        agg_rules: Vec::new(),
    }];
    seed_eval_metrics(&mut ctx, ds_orders_id);
    seed_eval_datasets(&mut ctx, ds_orders_id);
    seed_eval_assets_and_flows(&mut ctx);
    ctx
}

fn seed_eval_metrics(ctx: &mut DashboardGenContext, ds_orders_id: Uuid) {
    let now = Utc::now();
    ctx.metrics.push(MetricDef {
        id: Uuid::from_u128(0x201),
        name: String::from("paid_amount"),
        display_name: String::from("已支付订单总金额"),
        description: String::new(),
        dataset_id: ds_orders_id,
        expression: String::from("SUM(amount)"),
        filter_sql: String::from("status = 'PAID'"),
        time_column: Some(String::from("updated_at")),
        default_grain: RollupGrain::Day,
        dimensions: vec![
            MetricDimension {
                name: String::from("status"),
                column: String::from("status"),
                display_name: String::from("状态"),
            },
            MetricDimension {
                name: String::from("warehouse_code"),
                column: String::from("warehouse_code"),
                display_name: String::from("仓库"),
            },
        ],
        unit: String::from("CNY"),
        format: String::from("0,0.00"),
        synonyms: Vec::new(),
        certified: true,
        schema_version: 1,
        broken: false,
        owner_user_id: None,
        version: 1,
        created_at: now,
        updated_at: now,
    });
}

fn seed_eval_datasets(ctx: &mut DashboardGenContext, ds_orders_id: Uuid) {
    let now = Utc::now();
    let orders_ds = Dataset {
        id: ds_orders_id,
        connection_id: None,
        name: String::from("demo_orders"),
        display_name: String::from("演示订单表"),
        description: String::new(),
        tags: Vec::new(),
        source_kind: SourceKind::SqlTable,
        source_ref: serde_json::json!({}),
        sync_mode: SyncMode::Snapshot,
        cursor_column: None,
        cursor_value: None,
        primary_key: Vec::new(),
        delete_column: None,
        schema_policy: SchemaPolicy::Evolve,
        lake_schema: String::from("tenant_default"),
        schedule_cron: None,
        lake_table: String::from("demo_orders"),
        schema_version: 1,
        schema_json: Vec::new(),
        row_count: 100,
        last_snapshot_id: None,
        last_synced_at: Some(now),
        status: DatasetStatus::Ready,
        owner_user_id: None,
        created_at: now,
        updated_at: now,
    };
    let mut entry = CatalogDatasetEntry::simple(ds_orders_id, "demo_orders", "demo_orders");
    for (idx, (cname, dt)) in [
        ("order_id", DataType::String),
        ("status", DataType::String),
        ("warehouse_code", DataType::String),
        ("amount", DataType::Float64),
        ("updated_at", DataType::Timestamp),
    ]
    .iter()
    .enumerate()
    {
        entry.columns.insert(
            (*cname).to_owned(),
            ColumnMeta {
                dataset_id: ds_orders_id,
                column_name: (*cname).to_owned(),
                data_type: *dt,
                pii_category: PiiCategory::None,
                pii_confirmed: false,
                description: String::new(),
                business_term: String::new(),
                unit: String::new(),
                ordinal: i32::try_from(idx).unwrap_or(0),
            },
        );
    }
    ctx.datasets.push(orders_ds);
    ctx.catalog.insert_dataset(entry);
}

fn seed_eval_assets_and_flows(ctx: &mut DashboardGenContext) {
    let now = Utc::now();
    ctx.assets.push(AssetRecord {
        id: Uuid::from_u128(0x301),
        tenant_id: String::from("default"),
        asset_kind: AssetKind::Model3d,
        name: String::from("warehouse_twin.glb"),
        file_path: String::from("assets/default/warehouse_twin.glb"),
        mime_type: String::from("model/gltf-binary"),
        sha256: String::from("00"),
        width_px: None,
        height_px: None,
        created_at: now,
    });
    ctx.assets.push(AssetRecord {
        id: Uuid::from_u128(0x302),
        tenant_id: String::from("default"),
        asset_kind: AssetKind::FloorPlanImage,
        name: String::from("floor.png"),
        file_path: String::from("assets/default/floor.png"),
        mime_type: String::from("image/png"),
        sha256: String::from("00"),
        width_px: Some(1200),
        height_px: Some(800),
        created_at: now,
    });
    ctx.prod_flows.push(AxFlowDef {
        id: Uuid::from_u128(0x501),
        name: String::from("replenish_flow"),
        description: String::new(),
        slug: Some(String::from("replenish")),
        nodes: Vec::new(),
        edges: Vec::new(),
        trigger_cfg: TriggerCfg {
            kind: TriggerType::Manual,
            cron: None,
            timezone: None,
            catch_up: None,
            topic: None,
            filter: None,
        },
        status: String::from("PUBLISHED"),
        issues: Vec::new(),
        created_by: None,
        last_run_at: None,
        created_at: now,
        updated_at: now,
    });
}

fn make_node_type(name: &str, display_name: &str, metrics: &[&str]) -> NodeTypeSpec {
    NodeTypeSpec {
        id: None,
        name: name.to_owned(),
        display_name: display_name.to_owned(),
        description: String::new(),
        icon: String::from("box"),
        color: String::from("#2563eb"),
        title_template: String::from("{node_key}"),
        properties: metrics
            .iter()
            .map(|m| PropertySpec {
                name: (*m).to_owned(),
                display_name: (*m).to_owned(),
                description: String::new(),
                unit: None,
                data_type: DataType::Float64,
                kind: PropertyKind::Metric,
                required: false,
                pii_category: PiiCategory::None,
                allowed_values: None,
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            })
            .collect(),
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: false,
        pos_x: 0.0,
        pos_y: 0.0,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
        semantic_layer_tier: None,
        domain_group: None,
        actions: Vec::new(),
    }
}

async fn run_single_case(ai: &AiGateway, case: &EvalCase, ctx: &DashboardGenContext) -> bool {
    let req = AiGenerateDashboardRequest {
        prompt: case.prompt.clone(),
        title: None,
        slug: None,
        theme: None,
    };

    let Ok(draft) = draft_dashboard_with_ai(ai, ctx, &req).await else {
        return false;
    };

    if !validate_ai_dashboard_draft(&draft, ctx).is_empty() {
        return false;
    }
    if draft.widgets.len() < case.expect_min_widgets {
        return false;
    }

    for expected_kind in &case.expect_kinds {
        if !draft
            .widgets
            .iter()
            .any(|w| w.widget_kind == *expected_kind)
        {
            return false;
        }
    }
    true
}

#[tokio::test]
async fn dashboard_gen_eval_ten_cases_meet_accuracy_threshold() -> Result<(), AppError> {
    let raw = include_str!("dashboard_gen_eval/cases.json");
    let cases: Vec<EvalCase> = serde_json::from_str(raw)
        .map_err(|err| AppError::Config(format!("parse dashboard_gen_eval cases.json: {err}")))?;
    assert!(
        cases.len() >= 10,
        "expected >= 10 eval cases, found {}",
        cases.len()
    );

    let mode = resolve_eval_mode();
    let (ai, temp_cassette) = build_eval_gateway(&mode)?;
    let ctx = build_eval_context();

    let mut passed = 0_usize;
    let mut failed_ids = Vec::new();
    for case in &cases {
        if run_single_case(&ai, case, &ctx).await {
            passed = passed.saturating_add(1);
        } else {
            failed_ids.push(case.id.clone());
        }
    }

    if let Some(scratch) = temp_cassette {
        drop(std::fs::remove_file(scratch));
    }

    let total_f = f64::from(u32::try_from(cases.len()).unwrap_or(1));
    let passed_f = f64::from(u32::try_from(passed).unwrap_or(0));
    let accuracy = passed_f / total_f;

    if matches!(mode, EvalMode::Live) {
        let mut stderr = std::io::stderr();
        drop(writeln!(
            stderr,
            "dashboard_gen_eval live accuracy: {accuracy:.3} ({passed}/{}); failed: {failed_ids:?}",
            cases.len()
        ));
        return Ok(());
    }

    assert!(
        accuracy >= MIN_EVAL_ACCURACY,
        "dashboard_gen_eval accuracy {accuracy:.3} ({passed}/{}) < {MIN_EVAL_ACCURACY}; failed: {:?}",
        cases.len(),
        failed_ids
    );
    Ok(())
}
