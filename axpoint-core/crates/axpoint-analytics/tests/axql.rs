//! Unit and in-memory `DuckDB` execution tests for Step 27 `AxQL` ontology compiler,
//! cross-tier 1:1 folding, `SUM_BY_ENTITY` fanout guard, `MATCH_KNOWLEDGE` pushdown,
//! and dynamic state visibility (`05-STEP27` §27.2).

use std::collections::BTreeMap;

use axpoint_analytics::{
    AxqlCompilePlan, CatalogDatasetEntry, TenantCatalog, compile_axql_query, execute_axql_conn,
    extract_axql_knowledge_requests, register_mask_udfs,
};
use axpoint_domain::AppError;
use axpoint_domain::analytics::AxqlQueryRequest;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{Cardinality, DataType, MaskRule, PiiCategory, SemanticLayerTier};
use axpoint_domain::guard::{NodeTypePolicySpec, PolicyContext};
use axpoint_domain::studio::{
    EdgeTypeSpec, LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode, MappingSpec,
    NodeTypeSpec, OnSourceDelete, PropertyKind, PropertySpec, SchemaDoc,
};
use duckdb::Connection;
use serde_json::json;
use uuid::Uuid;

fn make_prop(
    name: &str,
    data_type: DataType,
    kind: PropertyKind,
    pii: PiiCategory,
) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        data_type,
        kind,
        pii_category: pii,
        ..PropertySpec::default()
    }
}

fn step27_schema_doc(with_mappings: bool) -> SchemaDoc {
    let warehouse = NodeTypeSpec {
        name: String::from("Step27Warehouse"),
        display_name: String::from("测试仓库"),
        semantic_layer_tier: Some(SemanticLayerTier::StaticFact),
        properties: vec![
            make_prop(
                "region",
                DataType::String,
                PropertyKind::Attribute,
                PiiCategory::None,
            ),
            make_prop(
                "phone",
                DataType::String,
                PropertyKind::Attribute,
                PiiCategory::Phone,
            ),
            make_prop(
                "inventory",
                DataType::Float64,
                PropertyKind::Metric,
                PiiCategory::None,
            ),
        ],
        ..NodeTypeSpec::default()
    };
    let order = NodeTypeSpec {
        name: String::from("Step27Order"),
        display_name: String::from("测试订单"),
        semantic_layer_tier: Some(SemanticLayerTier::EventDynamic),
        properties: vec![make_prop(
            "amount",
            DataType::Float64,
            PropertyKind::Metric,
            PiiCategory::None,
        )],
        ..NodeTypeSpec::default()
    };
    let fulfills = EdgeTypeSpec {
        name: String::from("STEP27_FULFILLS"),
        src_type_name: String::from("Step27Warehouse"),
        dst_type_name: String::from("Step27Order"),
        cardinality: Cardinality::OneToMany,
        ..EdgeTypeSpec::default()
    };

    let mappings = if with_mappings {
        vec![
            MappingSpec {
                name: String::from("map_warehouses"),
                dataset: String::from("warehouses"),
                node_type: String::from("Step27Warehouse"),
                key_column: String::from("wh_code"),
                title: String::from("{wh_code}"),
                filter: None,
                attributes: BTreeMap::from([
                    (String::from("region"), String::from("region")),
                    (String::from("phone"), String::from("phone")),
                ]),
                metrics: BTreeMap::from([(String::from("inventory"), String::from("inventory"))]),
                metric_time_column: None,
                geo: MappingGeoSpec::default(),
                links: Vec::new(),
                on_source_delete: OnSourceDelete::default(),
                mode: MappingMode::default(),
            },
            MappingSpec {
                name: String::from("map_orders"),
                dataset: String::from("orders"),
                node_type: String::from("Step27Order"),
                key_column: String::from("order_id"),
                title: String::from("{order_id}"),
                filter: None,
                attributes: BTreeMap::new(),
                metrics: BTreeMap::from([(String::from("amount"), String::from("amount"))]),
                metric_time_column: None,
                geo: MappingGeoSpec::default(),
                links: vec![MappingLinkSpec {
                    link_type: String::from("STEP27_FULFILLS"),
                    direction: LinkDirection::In,
                    target_type: String::from("Step27Warehouse"),
                    target_key_column: String::from("wh_code"),
                    weight_column: None,
                    create_missing_target: false,
                }],
                on_source_delete: OnSourceDelete::default(),
                mode: MappingMode::default(),
            },
        ]
    } else {
        Vec::new()
    };

    SchemaDoc {
        node_types: vec![warehouse, order],
        edge_types: vec![fulfills],
        mappings,
    }
}

fn setup_ax_views_duckdb() -> Result<(Connection, Uuid, Uuid), AppError> {
    let conn = Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open_in_memory: {e}")))?;
    register_mask_udfs(&conn, "default", None)?;
    let w1_id = Uuid::new_v4();
    let w2_id = Uuid::new_v4();
    let o1_id = Uuid::new_v4();
    let o2_id = Uuid::new_v4();
    let l1_id = Uuid::new_v4();
    let l2_id = Uuid::new_v4();

    let sql = format!(
        "CREATE SCHEMA ax;
         CREATE TABLE ax.nodes (
             id VARCHAR, type_name VARCHAR, node_key VARCHAR, title VARCHAR,
             properties VARCHAR, metrics VARCHAR, geo_lng DOUBLE, geo_lat DOUBLE, updated_at TIMESTAMPTZ
         );
         CREATE TABLE ax.links (
             id VARCHAR, link_type VARCHAR, src_node_id VARCHAR, dst_node_id VARCHAR,
             weight DOUBLE, properties VARCHAR
         );
         INSERT INTO ax.nodes VALUES
             ('{w1_id}', 'Step27Warehouse', 'W-27-1', '华东一号仓', '{{\"region\":\"east\",\"phone\":\"13812345678\"}}', '{{\"inventory\":500}}', NULL, NULL, CURRENT_TIMESTAMP),
             ('{w2_id}', 'Step27Warehouse', 'W-27-2', '华南二号仓', '{{\"region\":\"south\",\"phone\":\"13987654321\"}}', '{{\"inventory\":60}}', NULL, NULL, CURRENT_TIMESTAMP),
             ('{o1_id}', 'Step27Order', 'O-27-1', '订单一', '{{}}', '{{\"amount\":120}}', NULL, NULL, CURRENT_TIMESTAMP),
             ('{o2_id}', 'Step27Order', 'O-27-2', '订单二', '{{}}', '{{\"amount\":180}}', NULL, NULL, CURRENT_TIMESTAMP);
         INSERT INTO ax.links VALUES
             ('{l1_id}', 'STEP27_FULFILLS', '{w1_id}', '{o1_id}', 1.0, '{{}}'),
             ('{l2_id}', 'STEP27_FULFILLS', '{w1_id}', '{o2_id}', 1.0, '{{}}');"
    );
    conn.execute_batch(&sql)
        .map_err(|e| AppError::Database(format!("seed ax views: {e}")))?;
    Ok((conn, w1_id, w2_id))
}

#[test]
fn axql_rejects_unmapped_symbols_and_ungrounded_joins() {
    let schema = step27_schema_doc(false);
    let catalog = TenantCatalog::new("default");

    let bad_prop = AxqlQueryRequest {
        axql: String::from("SELECT w.non_existent_prop FROM OBJECT(Step27Warehouse) w"),
        ..AxqlQueryRequest::default()
    };
    let err1 = compile_axql_query(&bad_prop, &schema, &catalog, None, false, &BTreeMap::new());
    assert!(matches!(err1, Err(ref e) if e.error_code() == "AXQL_UNMAPPED_SYMBOL"));

    let bad_type = AxqlQueryRequest {
        axql: String::from("SELECT x.node_key FROM OBJECT(UnknownType) x"),
        ..AxqlQueryRequest::default()
    };
    let err2 = compile_axql_query(&bad_type, &schema, &catalog, None, false, &BTreeMap::new());
    assert!(matches!(err2, Err(ref e) if e.error_code() == "AXQL_UNMAPPED_SYMBOL"));

    let bad_edge = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.node_key FROM MATCH (w:Step27Warehouse)-[:FAKE_EDGE]->(o:Step27Order)",
        ),
        ..AxqlQueryRequest::default()
    };
    let err3 = compile_axql_query(&bad_edge, &schema, &catalog, None, false, &BTreeMap::new());
    assert!(matches!(err3, Err(ref e) if e.error_code() == "JOIN_PATH_UNGROUNDED"));

    let manual_join = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.node_key FROM Step27Warehouse w JOIN Step27Order o ON w.id = o.id",
        ),
        ..AxqlQueryRequest::default()
    };
    let err4 = compile_axql_query(
        &manual_join,
        &schema,
        &catalog,
        None,
        false,
        &BTreeMap::new(),
    );
    assert!(matches!(err4, Err(ref e) if e.error_code() == "JOIN_PATH_UNGROUNDED"));
}

#[test]
fn axql_object_cross_tier_folding_and_sum_by_entity_prevent_fanout() -> Result<(), AppError> {
    let (conn, _, _) = setup_ax_views_duckdb()?;
    let schema = step27_schema_doc(false);
    let catalog = TenantCatalog::new("default");

    let obj_req = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.node_key, w.region, w.phone, w.inventory, w.amount \
             FROM OBJECT(Step27Warehouse) w WHERE w.node_key = 'W-27-1'",
        ),
        ..AxqlQueryRequest::default()
    };
    let obj_resp = execute_axql_conn(
        &conn,
        &obj_req,
        &schema,
        &catalog,
        None,
        false,
        &BTreeMap::new(),
    )?;
    assert_eq!(obj_resp.result.row_count, 1);
    assert!(obj_resp.plan.fanout_protected);
    let row0 = obj_resp
        .result
        .rows
        .first()
        .ok_or_else(|| AppError::Config(String::from("missing row0")))?;
    assert_eq!(
        row0.get("inventory").and_then(serde_json::Value::as_f64),
        Some(500.0)
    );
    assert_eq!(
        row0.get("amount").and_then(serde_json::Value::as_f64),
        Some(300.0)
    );
    let masked_phone = row0
        .get("phone")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert_ne!(masked_phone, "13812345678");
    assert!(masked_phone.contains('*'));

    let match_req = AxqlQueryRequest {
        axql: String::from(
            "SELECT SUM_BY_ENTITY(w.inventory BY w) AS exact_inv, \
                    AVG_BY_ENTITY(w.inventory BY w) AS avg_inv, \
                    COUNT(o.node_key) AS order_cnt, \
                    SUM(o.amount) AS total_amt \
             FROM MATCH (w:Step27Warehouse)-[:STEP27_FULFILLS]->(o:Step27Order) \
             WHERE w.node_key = 'W-27-1'",
        ),
        ..AxqlQueryRequest::default()
    };
    let match_resp = execute_axql_conn(
        &conn,
        &match_req,
        &schema,
        &catalog,
        None,
        false,
        &BTreeMap::new(),
    )?;
    assert!(match_resp.plan.fanout_protected);
    assert_eq!(match_resp.plan.dedup_entities, vec![String::from("w")]);
    let mrow = match_resp
        .result
        .rows
        .first()
        .ok_or_else(|| AppError::Config(String::from("missing match row")))?;
    assert_eq!(
        mrow.get("exact_inv").and_then(serde_json::Value::as_f64),
        Some(500.0)
    );
    assert_eq!(
        mrow.get("avg_inv").and_then(serde_json::Value::as_f64),
        Some(500.0)
    );
    assert_eq!(
        mrow.get("order_cnt").and_then(serde_json::Value::as_i64),
        Some(2)
    );
    assert_eq!(
        mrow.get("total_amt").and_then(serde_json::Value::as_f64),
        Some(300.0)
    );
    Ok(())
}

#[test]
fn axql_mapped_ducklake_datasets_fold_1_to_n_child_metrics_without_inflation()
-> Result<(), AppError> {
    let conn = Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open_in_memory: {e}")))?;
    register_mask_udfs(&conn, "default", None)?;
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_default\";
         CREATE TABLE lake.\"tenant_default\".\"warehouses\" (
             wh_code VARCHAR, region VARCHAR, phone VARCHAR, inventory DOUBLE
         );
         CREATE TABLE lake.\"tenant_default\".\"orders\" (
             order_id VARCHAR, wh_code VARCHAR, amount DOUBLE
         );
         INSERT INTO lake.\"tenant_default\".\"warehouses\" VALUES ('W-1', 'east', '13800001111', 500.0);
         INSERT INTO lake.\"tenant_default\".\"orders\" VALUES ('O-1', 'W-1', 120.0), ('O-2', 'W-1', 180.0);",
    )
    .map_err(|e| AppError::Database(format!("seed lake tables: {e}")))?;

    let wh_id = Uuid::new_v4();
    let ord_id = Uuid::new_v4();
    let mut wh_ds = CatalogDatasetEntry::simple(wh_id, "warehouses", "warehouses");
    wh_ds.columns.insert(
        String::from("phone"),
        ColumnMeta {
            dataset_id: wh_id,
            column_name: String::from("phone"),
            data_type: DataType::String,
            pii_category: PiiCategory::Phone,
            pii_confirmed: true,
            description: String::new(),
            business_term: String::new(),
            unit: String::new(),
            ordinal: 2,
        },
    );
    let ord_ds = CatalogDatasetEntry::simple(ord_id, "orders", "orders");
    let catalog = TenantCatalog::new("default")
        .with_dataset(wh_ds)
        .with_dataset(ord_ds);
    let mut schema = step27_schema_doc(true);
    for nt in &mut schema.node_types {
        for prop in &mut nt.properties {
            prop.pii_category = PiiCategory::None;
        }
    }

    let req = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.node_key, w.phone, w.inventory, w.amount FROM OBJECT(Step27Warehouse) w WHERE w.region = $reg",
        ),
        params: BTreeMap::from([(String::from("reg"), json!("east"))]),
        ..AxqlQueryRequest::default()
    };
    let resp = execute_axql_conn(
        &conn,
        &req,
        &schema,
        &catalog,
        None,
        false,
        &BTreeMap::new(),
    )?;
    assert_eq!(resp.result.row_count, 1);
    assert!(resp.plan.fanout_protected);
    assert!(resp.plan.compiled_sql.contains("tenant_default"));
    let r0 = resp
        .result
        .rows
        .first()
        .ok_or_else(|| AppError::Config(String::from("missing row")))?;
    assert!(
        r0.get("phone")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .contains('*')
    );
    assert_eq!(
        r0.get("inventory").and_then(serde_json::Value::as_f64),
        Some(500.0)
    );
    assert_eq!(
        r0.get("amount").and_then(serde_json::Value::as_f64),
        Some(300.0)
    );
    Ok(())
}

#[test]
fn axql_match_knowledge_pushdown_and_state_row_policy_filtering() -> Result<(), AppError> {
    let (conn, _w1_id, w2_id) = setup_ax_views_duckdb()?;
    let schema = step27_schema_doc(false);
    let catalog = TenantCatalog::new("default");

    let req = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.node_key, w.inventory FROM OBJECT(Step27Warehouse) w \
             WHERE MATCH_KNOWLEDGE($q, top_k = 5)",
        ),
        params: BTreeMap::from([(String::from("$q"), json!("低库存风险仓库"))]),
        ..AxqlQueryRequest::default()
    };
    let (_, calls) = extract_axql_knowledge_requests(&req)?;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().map(|c| c.top_k), Some(5));

    let empty_resp = execute_axql_conn(
        &conn,
        &req,
        &schema,
        &catalog,
        None,
        false,
        &BTreeMap::new(),
    )?;
    assert_eq!(empty_resp.result.row_count, 0);

    let mut recall_map = BTreeMap::new();
    recall_map.insert(0, vec![w2_id]);
    let hit_resp = execute_axql_conn(&conn, &req, &schema, &catalog, None, false, &recall_map)?;
    assert_eq!(hit_resp.plan.knowledge_recall_nodes, 1);
    assert_eq!(hit_resp.result.row_count, 1);
    assert_eq!(
        hit_resp
            .result
            .rows
            .first()
            .and_then(|r| r.get("node_key"))
            .and_then(serde_json::Value::as_str),
        Some("W-27-2")
    );

    let mut pol = PolicyContext::default();
    pol.node_types.insert(
        String::from("Step27Warehouse"),
        NodeTypePolicySpec {
            duckdb_row_filter: Some(String::from(
                "TRY_CAST(json_extract_string(metrics, '$.inventory') AS DOUBLE) < 100",
            )),
            pg_row_filter: None,
            property_masks: BTreeMap::from([(String::from("phone"), MaskRule::Redact)]),
        },
    );
    let governed_req = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.node_key, w.phone, w.inventory FROM OBJECT(Step27Warehouse) w",
        ),
        ..AxqlQueryRequest::default()
    };
    let gov_resp = execute_axql_conn(
        &conn,
        &governed_req,
        &schema,
        &catalog,
        Some(&pol),
        false,
        &BTreeMap::new(),
    )?;
    assert_eq!(gov_resp.result.row_count, 1);
    let gr = gov_resp
        .result
        .rows
        .first()
        .ok_or_else(|| AppError::Config(String::from("missing governed row")))?;
    assert_eq!(
        gr.get("node_key").and_then(serde_json::Value::as_str),
        Some("W-27-2")
    );
    assert_eq!(
        gr.get("phone").and_then(serde_json::Value::as_str),
        Some("[REDACTED]")
    );

    let _: AxqlCompilePlan = gov_resp.plan;
    Ok(())
}

#[test]
fn axql_blocks_self_alias_bypass_bad_aliases_and_subqueries() -> Result<(), AppError> {
    let schema = step27_schema_doc(false);
    let catalog = TenantCatalog::new("default");

    for bad_select in [
        "SELECT hallucinated AS hallucinated FROM OBJECT(Step27Warehouse) w",
        "SELECT UPPER(hallucinated) AS hallucinated FROM OBJECT(Step27Warehouse) w",
    ] {
        let req = AxqlQueryRequest {
            axql: String::from(bad_select),
            ..AxqlQueryRequest::default()
        };
        let res = compile_axql_query(&req, &schema, &catalog, None, false, &BTreeMap::new());
        assert!(matches!(res, Err(ref e) if e.error_code() == "AXQL_UNMAPPED_SYMBOL"));
    }

    let valid_group_alias = AxqlQueryRequest {
        axql: String::from(
            "SELECT w.region AS reg_alias, COUNT(w.node_key) AS cnt \
             FROM OBJECT(Step27Warehouse) w GROUP BY reg_alias ORDER BY reg_alias",
        ),
        ..AxqlQueryRequest::default()
    };
    let plan = compile_axql_query(
        &valid_group_alias,
        &schema,
        &catalog,
        None,
        false,
        &BTreeMap::new(),
    )?;
    assert!(plan.plan.compiled_sql.contains("GROUP BY"));

    for bad_alias in [
        "SELECT w.node_key FROM OBJECT(Step27Warehouse) AS \"bad;alias\"",
        "SELECT x.node_key FROM Step27Warehouse AS \"bad;alias\"",
        "SELECT w.node_key FROM MATCH (\"bad;w\":Step27Warehouse)-[:STEP27_FULFILLS]->(o:Step27Order)",
    ] {
        let req = AxqlQueryRequest {
            axql: String::from(bad_alias),
            ..AxqlQueryRequest::default()
        };
        let res = compile_axql_query(&req, &schema, &catalog, None, false, &BTreeMap::new());
        assert!(res.is_err());
    }

    for subq in [
        "SELECT w.node_key FROM OBJECT(Step27Warehouse) w WHERE w.id IN (SELECT id FROM ax.nodes)",
        "SELECT w.node_key FROM OBJECT(Step27Warehouse) w WHERE EXISTS (SELECT 1 FROM ax.nodes)",
        "SELECT (SELECT COUNT(*) FROM ax.nodes) AS c FROM OBJECT(Step27Warehouse) w",
    ] {
        let req = AxqlQueryRequest {
            axql: String::from(subq),
            ..AxqlQueryRequest::default()
        };
        let res = compile_axql_query(&req, &schema, &catalog, None, false, &BTreeMap::new());
        assert!(matches!(res, Err(ref e) if e.error_code() == "JOIN_PATH_UNGROUNDED"));
    }
    Ok(())
}

#[test]
fn axql_mapped_node_governance_and_edge_metric_masking() -> Result<(), AppError> {
    let (conn, w1_id, _) = setup_ax_views_duckdb()?;
    conn.execute(
        &format!(
            "UPDATE ax.links SET properties = '{{\"contact_phone\":\"13700009999\"}}' WHERE src_node_id = '{w1_id}'"
        ),
        [],
    )
    .map_err(|e| AppError::Database(format!("update link props: {e}")))?;

    let mut schema = step27_schema_doc(true);
    if let Some(edge) = schema.edge_types.first_mut() {
        edge.properties.push(make_prop(
            "contact_phone",
            DataType::String,
            PropertyKind::Attribute,
            PiiCategory::Phone,
        ));
    }
    let catalog = TenantCatalog::new("default")
        .with_dataset(CatalogDatasetEntry::simple(
            Uuid::new_v4(),
            "warehouses",
            "warehouses",
        ))
        .with_dataset(CatalogDatasetEntry::simple(
            Uuid::new_v4(),
            "orders",
            "orders",
        ));

    let mut pol = PolicyContext::default();
    pol.node_types.insert(
        String::from("Step27Warehouse"),
        NodeTypePolicySpec {
            duckdb_row_filter: Some(String::from("node_key = 'W-27-1'")),
            pg_row_filter: None,
            property_masks: BTreeMap::from([(String::from("inventory"), MaskRule::Redact)]),
        },
    );

    let obj_req = AxqlQueryRequest {
        axql: String::from("SELECT w.node_key, w.inventory FROM OBJECT(Step27Warehouse) w"),
        ..AxqlQueryRequest::default()
    };
    let obj_resp = execute_axql_conn(
        &conn,
        &obj_req,
        &schema,
        &catalog,
        Some(&pol),
        false,
        &BTreeMap::new(),
    )?;
    assert!(obj_resp.plan.compiled_sql.contains("ax.nodes"));
    assert_eq!(obj_resp.result.row_count, 1);
    let r0 = obj_resp
        .result
        .rows
        .first()
        .ok_or_else(|| AppError::Config(String::from("missing governed mapped row")))?;
    assert_eq!(
        r0.get("inventory").and_then(serde_json::Value::as_str),
        Some("[REDACTED]")
    );

    let match_req = AxqlQueryRequest {
        axql: String::from(
            "SELECT e.contact_phone FROM MATCH (w:Step27Warehouse)-[e:STEP27_FULFILLS]->(o:Step27Order)",
        ),
        ..AxqlQueryRequest::default()
    };
    let match_resp = execute_axql_conn(
        &conn,
        &match_req,
        &schema,
        &catalog,
        Some(&pol),
        false,
        &BTreeMap::new(),
    )?;
    let ep = match_resp
        .result
        .rows
        .first()
        .and_then(|r| r.get("contact_phone"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(ep.contains('*'));
    Ok(())
}
