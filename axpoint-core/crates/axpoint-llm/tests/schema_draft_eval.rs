//! Offline cassette evaluation suite for `axpoint-llm::schema_draft` (`06-STEP05-06` §05.5 & §05.10).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axpoint_domain::AppError;
use axpoint_domain::enums::DataType;
use axpoint_domain::studio::{
    ColumnProfileStats, ColumnRoleTag, DatasetProfileSummary, DatasetValidationMeta,
    DocEntityGroup, IssueSeverity, ProfilingBundle, SchemaDoc, ValidationContext,
};
use axpoint_llm::{
    AiGateway, CassetteMode, EmbeddingConfig, LlmConfig, SchemaDraftInput, generate_schema_draft,
};

fn eval_cassette_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cassettes/schema_draft_eval.jsonl")
}

fn offline_gateway() -> Result<AiGateway, AppError> {
    AiGateway::with_cassette(
        LlmConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("offline-eval-model"),
        },
        EmbeddingConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("offline-embed-model"),
            configured_dim: Some(64),
        },
        eval_cassette_path(),
        CassetteMode::Replay,
    )
}

fn sample_orders_profiling_and_ctx() -> (ProfilingBundle, Vec<DocEntityGroup>, ValidationContext) {
    let ds_id = DatasetValidationMeta::default().id;
    let profiling = ProfilingBundle {
        datasets: vec![DatasetProfileSummary {
            dataset_id: ds_id,
            dataset_name: String::from("demo_orders"),
            display_name: String::from("演示订单表"),
            row_count: 20,
            pk_candidates: vec![String::from("order_id")],
            columns: vec![
                ColumnProfileStats {
                    name: String::from("order_id"),
                    data_type: DataType::Int64,
                    null_rate: 0.0,
                    distinct_count: 20,
                    roles: BTreeSet::from([ColumnRoleTag::PkCandidate]),
                    sample_values: vec![String::from("1001"), String::from("1002")],
                },
                ColumnProfileStats {
                    name: String::from("warehouse_code"),
                    data_type: DataType::String,
                    null_rate: 0.0,
                    distinct_count: 2,
                    roles: BTreeSet::from([ColumnRoleTag::Categorical]),
                    sample_values: vec![String::from("W-1")],
                },
                ColumnProfileStats {
                    name: String::from("customer"),
                    data_type: DataType::String,
                    null_rate: 0.0,
                    distinct_count: 15,
                    roles: BTreeSet::from([ColumnRoleTag::Categorical]),
                    sample_values: vec![String::from("客户1")],
                },
                ColumnProfileStats {
                    name: String::from("amount"),
                    data_type: DataType::Decimal,
                    null_rate: 0.0,
                    distinct_count: 20,
                    roles: BTreeSet::from([ColumnRoleTag::NumericMetric]),
                    sample_values: vec![String::from("110.00")],
                },
                ColumnProfileStats {
                    name: String::from("status"),
                    data_type: DataType::String,
                    null_rate: 0.0,
                    distinct_count: 2,
                    roles: BTreeSet::from([ColumnRoleTag::Categorical]),
                    sample_values: vec![String::from("OPEN"), String::from("SHIPPED")],
                },
                ColumnProfileStats {
                    name: String::from("updated_at"),
                    data_type: DataType::Timestamp,
                    null_rate: 0.0,
                    distinct_count: 20,
                    roles: BTreeSet::from([ColumnRoleTag::Temporal]),
                    sample_values: vec![String::from("2026-01-01T00:00:00Z")],
                },
            ],
        }],
        fk_candidates: Vec::new(),
    };

    let docs = vec![DocEntityGroup {
        entity_type: String::from("Facility"),
        entity_names: vec![String::from("华东仓"), String::from("W-1")],
        relations: vec![(
            String::from("Facility"),
            String::from("Order"),
            String::from("华东仓负责履约订单"),
        )],
    }];

    let mut cols = BTreeMap::new();
    cols.insert(String::from("order_id"), DataType::Int64);
    cols.insert(String::from("warehouse_code"), DataType::String);
    cols.insert(String::from("customer"), DataType::String);
    cols.insert(String::from("amount"), DataType::Decimal);
    cols.insert(String::from("status"), DataType::String);
    cols.insert(String::from("updated_at"), DataType::Timestamp);

    let mut ctx = ValidationContext::default();
    ctx.datasets.insert(
        String::from("demo_orders"),
        DatasetValidationMeta {
            id: ds_id,
            name: String::from("demo_orders"),
            status: String::from("READY"),
            row_count: 20,
            schema_version: 1,
            columns: cols,
            non_null_columns: BTreeSet::from([String::from("order_id")]),
        },
    );

    (profiling, docs, ctx)
}

#[tokio::test]
async fn schema_draft_eval_single_shot_passes_zero_errors_and_spreads_layout()
-> Result<(), AppError> {
    let gw = offline_gateway()?;
    let (profiling, doc_entities, ctx) = sample_orders_profiling_and_ctx();
    let current = SchemaDoc::default();

    let result = generate_schema_draft(
        &gw,
        &SchemaDraftInput {
            profiling: &profiling,
            doc_entities: &doc_entities,
            current_schema: &current,
            description: "构建供应链仓库与订单履约本体模型",
            validation_ctx: &ctx,
        },
    )
    .await?;

    assert_eq!(result.attempts, 1);
    let err_cnt = result
        .issues
        .iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .count();
    assert_eq!(
        err_cnt, 0,
        "unexpected validation errors: {:?}",
        result.issues
    );
    assert_eq!(result.schema_doc.node_types.len(), 2);
    assert_eq!(result.schema_doc.edge_types.len(), 1);
    assert_eq!(result.schema_doc.mappings.len(), 1);

    let n0 = result
        .schema_doc
        .node_types
        .first()
        .ok_or(AppError::NotFound("NODE_0"))?;
    let n1 = result
        .schema_doc
        .node_types
        .get(1)
        .ok_or(AppError::NotFound("NODE_1"))?;
    assert!(
        (n0.pos_x - n1.pos_x).abs() > 10.0,
        "expected overlapping coordinates to be spread onto grid"
    );

    Ok(())
}

#[tokio::test]
async fn schema_draft_eval_self_heals_validation_errors_on_second_attempt() -> Result<(), AppError>
{
    let gw = offline_gateway()?;
    let (profiling, doc_entities, ctx) = sample_orders_profiling_and_ctx();
    let current = SchemaDoc::default();

    let result = generate_schema_draft(
        &gw,
        &SchemaDraftInput {
            profiling: &profiling,
            doc_entities: &doc_entities,
            current_schema: &current,
            description: "[SELF_HEAL_CASE] 测试自动修复闭环",
            validation_ctx: &ctx,
        },
    )
    .await?;

    assert_eq!(result.attempts, 2);
    let err_cnt = result
        .issues
        .iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .count();
    assert_eq!(
        err_cnt, 0,
        "expected self-healed zero errors: {:?}",
        result.issues
    );
    assert_eq!(result.schema_doc.node_types.len(), 2);

    Ok(())
}
