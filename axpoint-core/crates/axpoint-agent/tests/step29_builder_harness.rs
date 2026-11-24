//! Unit and contract tests for Step 29 22-tool `AxBuilder` catalog and FK candidate discovery (`07-STEP29` §29.4).

use axpoint_agent::harness::{discover_fk_join_candidates, step29_axbuilder_tool_schema};
use axpoint_agent::{ALL_AXBUILDER_TOOL_NAMES, axbuilder_tool_schema};
use axpoint_domain::axhub::{ColumnProfile, Dataset, DatasetStatus, SchemaPolicy};
use axpoint_domain::enums::{DataType, PiiCategory, SourceKind, SyncMode};
use chrono::Utc;
use uuid::Uuid;

fn sample_dataset(id: u128, name: &str, pks: Vec<&str>, cols: Vec<(&str, DataType)>) -> Dataset {
    let now = Utc::now();
    Dataset {
        id: Uuid::from_u128(id),
        connection_id: None,
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        source_kind: SourceKind::SqlTable,
        source_ref: serde_json::json!({}),
        sync_mode: SyncMode::Snapshot,
        primary_key: pks.into_iter().map(String::from).collect(),
        cursor_column: None,
        cursor_value: None,
        delete_column: None,
        schema_policy: SchemaPolicy::Additive,
        lake_schema: String::from("tenant_default"),
        lake_table: String::from(name),
        schema_json: cols
            .into_iter()
            .enumerate()
            .map(|(idx, (cname, dt))| ColumnProfile {
                name: String::from(cname),
                ordinal: i32::try_from(idx).unwrap_or(0),
                data_type: dt,
                nullable: false,
                null_count: 0,
                null_ratio: 0.0,
                distinct_estimate: 10,
                sample_values: Vec::new(),
                pii_category: PiiCategory::None,
                pii_hit_ratio: 0.0,
                source_comment: String::new(),
            })
            .collect(),
        schema_version: 1,
        row_count: 100,
        last_snapshot_id: Some(1),
        last_synced_at: Some(now),
        schedule_cron: None,
        continuous_stream: false,
        cdc_slot_meta: serde_json::json!({}),
        tags: Vec::new(),
        owner_user_id: None,
        status: DatasetStatus::Ready,
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn axbuilder_catalog_contains_all_22_orthogonal_tools_with_valid_schemas() {
    assert_eq!(ALL_AXBUILDER_TOOL_NAMES.len(), 22);
    for &tool in ALL_AXBUILDER_TOOL_NAMES {
        let schema = axbuilder_tool_schema(tool);
        assert_eq!(
            schema.get("type"),
            Some(&serde_json::json!("object")),
            "schema missing object type for {tool}"
        );
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&serde_json::json!(false)),
            "schema should disallow additionalProperties for {tool}"
        );
    }
    assert!(step29_axbuilder_tool_schema("axbuilder_verify_competency_suite").is_some());
}

#[test]
fn fk_candidate_discovery_aligns_exact_pk_and_singular_entity_stems() {
    let warehouses = sample_dataset(
        1,
        "erp_warehouses",
        vec!["warehouse_id"],
        vec![
            ("warehouse_id", DataType::String),
            ("region", DataType::String),
        ],
    );
    let suppliers = sample_dataset(
        2,
        "suppliers",
        vec!["id"],
        vec![("id", DataType::Int64), ("name", DataType::String)],
    );
    let orders = sample_dataset(
        3,
        "sales_orders",
        vec!["order_id"],
        vec![
            ("order_id", DataType::String),
            ("warehouse_id", DataType::String),
            ("supplier_id", DataType::Int64),
            ("amount", DataType::Float64),
        ],
    );

    let candidates = discover_fk_join_candidates(&orders, &[warehouses, suppliers, orders.clone()]);
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().any(|c| {
        c.source_column == "warehouse_id"
            && c.target_dataset == "erp_warehouses"
            && c.target_column == "warehouse_id"
            && c.match_reason == "EXACT_PK_NAME"
    }));
    assert!(candidates.iter().any(|c| {
        c.source_column == "supplier_id"
            && c.target_dataset == "suppliers"
            && c.target_column == "id"
            && c.match_reason == "ENTITY_ID_STEM"
    }));
}
