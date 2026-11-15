//! Real PostgreSQL RLS integration tests for Step 06 `mapping_exec`, `aggregate_exec`, and `knowledge_sync`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::analytics::{QueryColumn, QueryResult};
use axpoint_domain::enums::{AggFn, Cardinality, DataType, PiiCategory};
use axpoint_domain::expr::EvalWarningCode;
use axpoint_domain::ports::{
    LakeInternalQueryPort, StudioExecutionPort as _, StudioRepository as _, TextEmbedderPort,
};
use axpoint_domain::rollup::{
    AcceptNodeCandidateRequest, GraphAggregateJobPayload, KnowledgeSyncJobPayload,
    MapMaterializeJobPayload, MetricProducer,
};
use axpoint_domain::studio::{
    AggRuleSpec, DerivedRuleSpec, EdgeTypeSpec, LinkDirection, MappingGeoSpec, MappingLinkSpec,
    MappingMode, MappingRunStatus, MappingSpec, NodeTypeSpec, OnSourceDelete, PropertyKind,
    PropertySpec, SaveSchemaDirectRequest, SchemaDoc,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type JsonRow = serde_json::Map<String, serde_json::Value>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

#[derive(Clone, Default)]
struct MockLake {
    columns: Arc<Mutex<Vec<String>>>,
    full_rows: Arc<Mutex<Vec<JsonRow>>>,
    delta_rows: Arc<Mutex<Option<Vec<JsonRow>>>>,
}

impl LakeInternalQueryPort for MockLake {
    async fn query_internal(
        &self,
        _tenant_id: &str,
        sql: &str,
        max_rows: usize,
    ) -> Result<QueryResult, AppError> {
        tokio::task::yield_now().await;
        let cols: Vec<QueryColumn> = self
            .columns
            .lock()
            .map_err(|e| AppError::Database(e.to_string()))?
            .iter()
            .map(|n| QueryColumn {
                name: n.clone(),
                data_type: DataType::String,
                pii_category: PiiCategory::None,
                masked: false,
            })
            .collect();

        if sql.contains("ducklake_table_changes") {
            let delta_guard = self
                .delta_rows
                .lock()
                .map_err(|e| AppError::Database(e.to_string()))?;
            let Some(rows) = delta_guard.as_ref() else {
                return Err(AppError::Database(String::from(
                    "ducklake_table_changes unavailable",
                )));
            };
            let take_n = if sql.contains("LIMIT 0") { 0 } else { max_rows };
            let sliced: Vec<_> = rows.iter().take(take_n).cloned().collect();
            let rc = sliced.len();
            return Ok(QueryResult {
                columns: cols,
                rows: sliced,
                row_count: rc,
                truncated: false,
                sql: String::from(sql),
                duration_ms: 1,
            });
        }

        let full_guard = self
            .full_rows
            .lock()
            .map_err(|e| AppError::Database(e.to_string()))?;
        let take_n = if sql.contains("LIMIT 0") { 0 } else { max_rows };
        let offset = sql
            .split("OFFSET ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        let sliced: Vec<_> = full_guard.iter().skip(offset).take(take_n).cloned().collect();
        let rc = sliced.len();
        Ok(QueryResult {
            columns: cols,
            rows: sliced,
            row_count: rc,
            truncated: false,
            sql: String::from(sql),
            duration_ms: 1,
        })
    }

    async fn invalidate_tenant_session(&self, _tenant_id: &str) -> bool {
        tokio::task::yield_now().await;
        true
    }
}

struct MockEmbedder;

impl TextEmbedderPort for MockEmbedder {
    fn embedding_model_name(&self) -> &'static str {
        "mock-bge-small"
    }

    async fn embed_texts(&self, texts: &[String]) -> Result<(Vec<Vec<f32>>, usize), AppError> {
        tokio::task::yield_now().await;
        let vecs = texts.iter().map(|_| vec![0.125_f32; 8]).collect();
        Ok((vecs, 8))
    }
}

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let require_db = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let owner_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));

    let app_pool = match PgPoolOptions::new()
        .max_connections(6)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };
    let owner_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };
    Ok(Some((app_pool, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("{prefix}_{short}")
}

async fn setup_tenant_and_dataset(
    owner_pool: &PgPool,
    tenant_id: &str,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name)
         VALUES ($1, $2)
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    let ds_id = Uuid::new_v4();
    let schema_json = serde_json::json!([
        {"name": "order_id", "data_type": "STRING", "nullable": false, "null_count": 0},
        {"name": "wh_key", "data_type": "STRING", "nullable": false, "null_count": 0},
        {"name": "status", "data_type": "STRING", "nullable": false, "null_count": 0},
        {"name": "amount", "data_type": "FLOAT64", "nullable": false, "null_count": 0}
    ]);
    sqlx::query(
        "INSERT INTO ds_datasets (
             id, tenant_id, name, lake_schema, lake_table, status, row_count, last_snapshot_id, schema_json
         )
         VALUES ($1, $2, 'orders', $3, 'orders', 'READY', 3, 10, $4)",
    )
    .bind(ds_id)
    .bind(tenant_id)
    .bind(format!("tenant_{tenant_id}"))
    .bind(schema_json)
    .execute(owner_pool)
    .await?;
    Ok(ds_id)
}

fn mk_prop(name: &str, dt: DataType, kind: PropertyKind) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: None,
        data_type: dt,
        kind,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
    }
}

fn build_order_and_warehouse_types() -> Vec<NodeTypeSpec> {
    let order = NodeTypeSpec {
        id: None,
        name: String::from("Order"),
        display_name: String::from("订单"),
        description: String::new(),
        icon: String::from("box"),
        color: String::from("#16a34a"),
        title_template: String::from("订单-{node_key}"),
        properties: vec![
            mk_prop("status", DataType::String, PropertyKind::Attribute),
            mk_prop("demand", DataType::Float64, PropertyKind::Metric),
        ],
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: false,
        pos_x: 0.0,
        pos_y: 0.0,
    };

    let warehouse = NodeTypeSpec {
        id: None,
        name: String::from("Warehouse"),
        display_name: String::from("仓库"),
        description: String::new(),
        icon: String::from("warehouse"),
        color: String::from("#2563eb"),
        title_template: String::from("{node_key}"),
        properties: vec![
            mk_prop("inventory", DataType::Float64, PropertyKind::Metric),
            mk_prop("total_demand", DataType::Float64, PropertyKind::Metric),
            mk_prop("fill_rate", DataType::Float64, PropertyKind::Metric),
        ],
        agg_rules: vec![AggRuleSpec {
            target_metric: String::from("total_demand"),
            op: AggFn::Sum,
            via_link_type: String::from("FULFILLS"),
            direction: LinkDirection::Out,
            source_type_name: String::from("Order"),
            source_metric: String::from("demand"),
            filter: Some(String::from("prop.status != 'CANCELED'")),
            weight_by_link: false,
        }],
        derived_rules: vec![DerivedRuleSpec {
            target_metric: String::from("fill_rate"),
            expression: String::from("inventory / total_demand"),
            enabled: true,
        }],
        sync_to_knowledge: true,
        pos_x: 0.0,
        pos_y: 0.0,
    };
    vec![warehouse, order]
}

fn build_supply_chain_schema() -> SchemaDoc {
    SchemaDoc {
        node_types: build_order_and_warehouse_types(),
        edge_types: vec![EdgeTypeSpec {
            id: None,
            name: String::from("FULFILLS"),
            src_type_name: String::from("Warehouse"),
            dst_type_name: String::from("Order"),
            impact_weight: 1.0,
            cardinality: Cardinality::OneToMany,
            description: String::new(),
            properties: Vec::new(),
            propagation: serde_json::json!({}),
        }],
        mappings: vec![MappingSpec {
            name: String::from("orders_to_graph"),
            dataset: String::from("orders"),
            node_type: String::from("Order"),
            key_column: String::from("order_id"),
            title: String::from("订单-{order_id}"),
            filter: None,
            attributes: BTreeMap::from([(String::from("status"), String::from("status"))]),
            metrics: BTreeMap::from([(String::from("demand"), String::from("amount"))]),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: vec![MappingLinkSpec {
                link_type: String::from("FULFILLS"),
                direction: LinkDirection::In,
                target_type: String::from("Warehouse"),
                target_key_column: String::from("wh_key"),
                weight_column: None,
                create_missing_target: true,
            }],
            on_source_delete: OnSourceDelete::Delete,
            mode: MappingMode::Incremental,
        }],
    }
}

fn json_obj(val: serde_json::Value) -> JsonRow {
    match val {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

#[tokio::test]
async fn step06_full_and_incremental_mapping_rollup_and_explain_end_to_end() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_id = unique_tenant("s06_e2e");
    let ds_id = setup_tenant_and_dataset(&owner_pool, &tenant_id).await?;
    let store = PgStore::new(app_pool);

    let pub_res = store
        .save_schema_direct(
            &tenant_id,
            "main",
            None,
            SaveSchemaDirectRequest {
                doc: build_supply_chain_schema(),
                base_version: None,
                change_summary: Some(String::from("init")),
                force: true,
                cascade: false,
            },
        )
        .await?;

    let mappings = store
        .list_enabled_main_mappings_for_dataset(&tenant_id, ds_id)
        .await?;
    let mapping_id = mappings.first().map_or_else(Uuid::nil, |m| m.id);

    let lake = MockLake {
        columns: Arc::new(Mutex::new(vec![
            String::from("order_id"),
            String::from("wh_key"),
            String::from("status"),
            String::from("amount"),
        ])),
        full_rows: Arc::new(Mutex::new(vec![
            json_obj(serde_json::json!({"order_id": "O-1", "wh_key": "WH-1", "status": "ACTIVE", "amount": 40.0})),
            json_obj(serde_json::json!({"order_id": "O-2", "wh_key": "WH-1", "status": "CANCELED", "amount": 999.0})),
            json_obj(serde_json::json!({"order_id": "", "wh_key": "WH-1", "status": "ACTIVE", "amount": 10.0})),
        ])),
        delta_rows: Arc::new(Mutex::new(None)),
    };

    let full_run = store
        .execute_mapping_materialize(
            &tenant_id,
            Uuid::new_v4(),
            &MapMaterializeJobPayload {
                mapping_id,
                mode: MappingMode::Full,
                schema_version: Some(pub_res.version),
            },
            &lake,
        )
        .await?;
    assert_eq!(full_run.status, MappingRunStatus::Succeeded);
    assert_eq!(full_run.nodes_upserted, 2);
    assert_eq!(full_run.rows_rejected, 1);

    verify_rollup_and_incremental_delete(&store, &owner_pool, &tenant_id, ds_id, mapping_id, pub_res.version, &lake).await
}

async fn verify_rollup_and_incremental_delete(
    store: &PgStore,
    owner_pool: &PgPool,
    tenant_id: &str,
    ds_id: Uuid,
    mapping_id: Uuid,
    schema_ver: i32,
    lake: &MockLake,
) -> TestResult {
    sqlx::query("UPDATE xb_nodes SET metrics = '{\"inventory\": 100.0}'::jsonb WHERE tenant_id = $1 AND type_name = 'Warehouse' AND node_key = 'WH-1'")
        .bind(tenant_id)
        .execute(owner_pool)
        .await?;

    let _ = store
        .execute_graph_aggregate(
            tenant_id,
            &GraphAggregateJobPayload {
                full: true,
                full_type: None,
                node_ids: Vec::new(),
                metrics: Vec::new(),
                schema_version: Some(schema_ver),
            },
        )
        .await?;

    let wh_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM xb_nodes WHERE tenant_id = $1 AND type_name = 'Warehouse' AND node_key = 'WH-1'",
    )
    .bind(tenant_id)
    .fetch_one(owner_pool)
    .await?;

    let exp_td = store.explain_node_metric(tenant_id, wh_id, "total_demand").await?;
    assert_eq!(exp_td.producer, MetricProducer::Agg);
    assert_eq!(exp_td.value, Some(40.0));
    assert_eq!(exp_td.inputs.len(), 1);

    let exp_fr = store.explain_node_metric(tenant_id, wh_id, "fill_rate").await?;
    assert_eq!(exp_fr.producer, MetricProducer::Derived);
    assert_eq!(exp_fr.value, Some(2.5));

    sqlx::query("UPDATE ds_datasets SET last_snapshot_id = 11 WHERE id = $1")
        .bind(ds_id)
        .execute(owner_pool)
        .await?;
    *lake.delta_rows.lock().map_err(|e| e.to_string())? = Some(vec![json_obj(serde_json::json!({
        "change_type": "delete",
        "snapshot_id": 11,
        "order_id": "O-1",
        "wh_key": "WH-1",
        "status": "ACTIVE",
        "amount": 40.0
    }))]);

    let inc_run = store
        .execute_mapping_materialize(
            tenant_id,
            Uuid::new_v4(),
            &MapMaterializeJobPayload {
                mapping_id,
                mode: MappingMode::Incremental,
                schema_version: Some(schema_ver),
            },
            lake,
        )
        .await?;
    assert_eq!(inc_run.mode, MappingMode::Incremental);
    assert_eq!(inc_run.nodes_deleted, 1);

    let _ = store
        .execute_graph_aggregate(
            tenant_id,
            &GraphAggregateJobPayload {
                full: false,
                full_type: None,
                node_ids: vec![wh_id],
                metrics: Vec::new(),
                schema_version: Some(schema_ver),
            },
        )
        .await?;

    let exp_zero = store.explain_node_metric(tenant_id, wh_id, "fill_rate").await?;
    assert_eq!(exp_zero.value, None);
    assert_eq!(exp_zero.warnings, vec![EvalWarningCode::DivByZero]);
    Ok(())
}

#[tokio::test]
async fn step06_knowledge_sync_dossier_and_candidate_promotion_end_to_end() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_id = unique_tenant("s06_ksync");
    let _ds_id = setup_tenant_and_dataset(&owner_pool, &tenant_id).await?;
    let store = PgStore::new(app_pool);

    let _ = store
        .save_schema_direct(
            &tenant_id,
            "main",
            None,
            SaveSchemaDirectRequest {
                doc: build_supply_chain_schema(),
                base_version: None,
                change_summary: Some(String::from("init")),
                force: true,
                cascade: false,
            },
        )
        .await?;

    let ent_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO hg_entities (id, tenant_id, name, name_norm, entity_type, occur_count, confidence)
         VALUES ($1, $2, '华南中心仓', '华南中心仓', 'Warehouse', 4, 0.95)",
    )
    .bind(ent_id)
    .bind(&tenant_id)
    .execute(&owner_pool)
    .await?;

    let cands_before = store.list_knowledge_node_candidates(&tenant_id).await?;
    assert_eq!(cands_before.items.len(), 1);
    assert_eq!(
        cands_before.items.first().map(|c| c.entity_id),
        Some(ent_id)
    );

    let accepted = store
        .accept_knowledge_node_candidate(
            &tenant_id,
            ent_id,
            AcceptNodeCandidateRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("WH-SOUTH"),
                title: None,
            },
        )
        .await?;
    assert!(accepted.confirmed);
    assert_eq!(accepted.node.node_key, "WH-SOUTH");

    let cands_after = store.list_knowledge_node_candidates(&tenant_id).await?;
    assert_eq!(cands_after.items.len(), 0);

    let res1 = store
        .execute_knowledge_sync(
            &tenant_id,
            &KnowledgeSyncJobPayload {
                node_id: Some(accepted.node.id),
                ..KnowledgeSyncJobPayload::default()
            },
            &MockEmbedder,
        )
        .await?;
    assert_eq!(res1.get("docs_synced").and_then(serde_json::Value::as_u64), Some(1));

    let res_del = store
        .execute_knowledge_sync(
            &tenant_id,
            &KnowledgeSyncJobPayload {
                deleted_node_id: Some(accepted.node.id),
                ..KnowledgeSyncJobPayload::default()
            },
            &MockEmbedder,
        )
        .await?;
    assert_eq!(res_del.get("docs_deleted").and_then(serde_json::Value::as_u64), Some(1));
    Ok(())
}
