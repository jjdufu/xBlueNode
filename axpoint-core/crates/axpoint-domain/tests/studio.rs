//! Comprehensive unit tests for `axpoint-domain::studio` (`validate`, `diff_schema`, `preview_merge`, `apply_merge`).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::{AggFn, Cardinality, DataType, PiiCategory, SubgraphMergeStrategy};
use axpoint_domain::studio::merge::apply_merge;
use axpoint_domain::studio::{
    AggRuleSpec, DatasetValidationMeta, DerivedRuleSpec, EdgeTypeSpec, IssueSeverity,
    LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode, MappingSpec, MergeConflictKind,
    MergePreviewRequest, MergeSubgraphRequest, NodeTypeSpec, OnSourceDelete, PropertyKind,
    PropertySpec, SchemaDoc, TypeMergeResolution, ValidationContext, diff_schema, preview_merge,
    validate,
};
use uuid::Uuid;

fn attr_prop(name: &str, dt: DataType) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: None,
        data_type: dt,
        kind: PropertyKind::Attribute,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
    }
}

fn metric_prop(name: &str, dt: DataType) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: None,
        data_type: dt,
        kind: PropertyKind::Metric,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
    }
}

fn sample_supply_chain_doc() -> SchemaDoc {
    let warehouse = NodeTypeSpec {
        id: None,
        name: String::from("Warehouse"),
        display_name: String::from("仓库"),
        description: String::from("区域仓储中心"),
        icon: String::from("warehouse"),
        color: String::from("#2563eb"),
        title_template: String::from("{node_key} - {prop.region}"),
        properties: vec![
            attr_prop("region", DataType::String),
            metric_prop("inventory", DataType::Float64),
            metric_prop("total_demand", DataType::Float64),
            metric_prop("fill_rate", DataType::Float64),
        ],
        agg_rules: vec![AggRuleSpec {
            target_metric: String::from("total_demand"),
            op: AggFn::Sum,
            via_link_type: String::from("FULFILLS"),
            direction: LinkDirection::Out,
            source_type_name: String::from("Order"),
            source_metric: String::from("demand"),
            filter: Some(String::from("prop.status != 'CANCELED' and demand > 0")),
            weight_by_link: false,
        }],
        derived_rules: vec![DerivedRuleSpec {
            target_metric: String::from("fill_rate"),
            expression: String::from(
                "if(total_demand > 0, safe_div(inventory, total_demand, 0), null)",
            ),
            enabled: true,
        }],
        sync_to_knowledge: false,
        pos_x: 360.0,
        pos_y: 140.0,
    };

    let order = NodeTypeSpec {
        id: None,
        name: String::from("Order"),
        display_name: String::from("订单"),
        description: String::from("销售履约订单"),
        icon: String::from("shopping-cart"),
        color: String::from("#16a34a"),
        title_template: String::from("{node_key}"),
        properties: vec![
            attr_prop("status", DataType::String),
            attr_prop("customer", DataType::String),
            metric_prop("demand", DataType::Float64),
        ],
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: false,
        pos_x: 640.0,
        pos_y: 140.0,
    };

    let fulfills = EdgeTypeSpec {
        id: None,
        name: String::from("FULFILLS"),
        src_type_name: String::from("Warehouse"),
        dst_type_name: String::from("Order"),
        impact_weight: 0.8,
        cardinality: Cardinality::OneToMany,
        description: String::new(),
        properties: Vec::new(),
        propagation: serde_json::json!({}),
    };

    SchemaDoc {
        node_types: vec![warehouse, order],
        edge_types: vec![fulfills],
        mappings: Vec::new(),
    }
}

#[test]
fn studio_valid_supply_chain_doc_passes_cleanly() {
    let doc = sample_supply_chain_doc();
    let ctx = ValidationContext::default();
    let issues = validate(&doc, &ctx);
    assert!(issues.is_empty(), "unexpected issues: {issues:?}");
}

#[test]
fn studio_rule1_names_reserved_colors_and_templates() {
    let mut doc = sample_supply_chain_doc();
    if let Some(wh) = doc.node_types.first_mut() {
        wh.name = String::from("bad_name");
        wh.display_name = String::new();
        wh.color = String::from("blue");
        wh.title_template = String::from("{prop.missing}");
        wh.properties.push(attr_prop("tenant_id", DataType::String));
        wh.properties.push(attr_prop("region", DataType::String));
    }
    let dup_order = NodeTypeSpec {
        id: None,
        name: String::from("Order"),
        display_name: String::from("重复订单"),
        description: String::new(),
        icon: String::from("box"),
        color: String::from("#000000"),
        title_template: String::from("{node_key}"),
        properties: Vec::new(),
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: false,
        pos_x: 0.0,
        pos_y: 0.0,
    };
    doc.node_types.push(dup_order);

    let issues = validate(&doc, &ValidationContext::default());
    let codes: BTreeSet<&str> = issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains("NAME_INVALID"));
    assert!(codes.contains("NAME_TAKEN"));
}

#[test]
fn studio_rule2_and_rule3_edge_endpoints_cross_subgraph_and_weights() {
    let shipment = NodeTypeSpec {
        id: None,
        name: String::from("Shipment"),
        display_name: String::from("运单"),
        description: String::new(),
        icon: String::from("truck"),
        color: String::from("#2563eb"),
        title_template: String::from("{node_key}"),
        properties: Vec::new(),
        agg_rules: Vec::new(),
        derived_rules: Vec::new(),
        sync_to_knowledge: false,
        pos_x: 100.0,
        pos_y: 100.0,
    };
    let sub_doc = SchemaDoc {
        node_types: vec![shipment],
        edge_types: vec![EdgeTypeSpec {
            id: None,
            name: String::from("SHIPS_ORDER"),
            src_type_name: String::from("Shipment"),
            dst_type_name: String::from("Order"),
            impact_weight: 0.7,
            cardinality: Cardinality::ManyToOne,
            description: String::new(),
            properties: Vec::new(),
            propagation: serde_json::json!({}),
        }],
        mappings: Vec::new(),
    };

    // Without Order in main_node_types -> TYPE_NOT_FOUND
    let issues_missing = validate(&sub_doc, &ValidationContext::default());
    assert!(issues_missing.iter().any(|i| i.code == "TYPE_NOT_FOUND"));

    // With Order in main_node_types for department subgraph -> valid!
    let main_doc = sample_supply_chain_doc();
    let mut ctx = ValidationContext {
        subgraph_slug: String::from("logistics"),
        ..ValidationContext::default()
    };
    for nt in main_doc.node_types {
        ctx.main_node_types.insert(nt.name.clone(), nt);
    }
    let issues_ok = validate(&sub_doc, &ctx);
    assert!(
        issues_ok.is_empty(),
        "expected clean cross-subgraph ref: {issues_ok:?}"
    );

    // Out-of-range weight -> WEIGHT_OUT_OF_RANGE
    let mut bad_weight_doc = sub_doc;
    if let Some(edge) = bad_weight_doc.edge_types.first_mut() {
        edge.impact_weight = 1.5;
    }
    let issues_weight = validate(&bad_weight_doc, &ctx);
    assert!(
        issues_weight
            .iter()
            .any(|i| i.code == "WEIGHT_OUT_OF_RANGE")
    );
}

#[test]
fn studio_rule4_rule5_rule6_rule7_expressions_cycles_and_producers() {
    let mut doc = sample_supply_chain_doc();
    if let Some(wh) = doc.node_types.first_mut() {
        // Introduce cycle: inventory = fill_rate * 2 (while fill_rate depends on inventory)
        wh.derived_rules.push(DerivedRuleSpec {
            target_metric: String::from("inventory"),
            expression: String::from("fill_rate * 2"),
            enabled: true,
        });
        // Introduce duplicate producer on total_demand
        wh.derived_rules.push(DerivedRuleSpec {
            target_metric: String::from("total_demand"),
            expression: String::from("round(abs(-10.5), 1) + 2 ^ 3 ^ 2"),
            enabled: true,
        });
        // Invalid chained comparison in another rule
        wh.derived_rules.push(DerivedRuleSpec {
            target_metric: String::from("fill_rate"),
            expression: String::from("if(1 < 2 < 3, 1, 0)"),
            enabled: false,
        });
    }

    let issues = validate(&doc, &ValidationContext::default());
    let codes: BTreeSet<&str> = issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains("CYCLE_DETECTED"));
    assert!(codes.contains("METRIC_MULTIPLE_PRODUCERS"));
    assert!(codes.contains("EXPR_INVALID"));
}

#[test]
fn studio_rule8_mapping_validation_checks_status_nulls_and_types() {
    let mut doc = sample_supply_chain_doc();
    let mut attrs = BTreeMap::new();
    attrs.insert(String::from("status"), String::from("status"));
    attrs.insert(String::from("customer"), String::from("customer"));
    let mut metrics = BTreeMap::new();
    metrics.insert(String::from("demand"), String::from("amount"));

    doc.mappings.push(MappingSpec {
        name: String::from("orders_to_order_nodes"),
        dataset: String::from("demo_orders"),
        node_type: String::from("Order"),
        key_column: String::from("order_id"),
        title: String::from("订单 {order_id}"),
        filter: None,
        attributes: attrs,
        metrics,
        metric_time_column: Some(String::from("updated_at")),
        geo: MappingGeoSpec::default(),
        links: vec![MappingLinkSpec {
            link_type: String::from("FULFILLS"),
            direction: LinkDirection::In,
            target_type: String::from("Warehouse"),
            target_key_column: String::from("warehouse_code"),
            weight_column: None,
            create_missing_target: false,
        }],
        on_source_delete: OnSourceDelete::Keep,
        mode: MappingMode::Incremental,
    });

    let mut cols = BTreeMap::new();
    cols.insert(String::from("order_id"), DataType::Int64);
    cols.insert(String::from("status"), DataType::String);
    cols.insert(String::from("customer"), DataType::String);
    cols.insert(String::from("amount"), DataType::Decimal);
    cols.insert(String::from("warehouse_code"), DataType::String);
    cols.insert(String::from("updated_at"), DataType::Timestamp);

    let mut ctx = ValidationContext::default();
    ctx.datasets.insert(
        String::from("demo_orders"),
        DatasetValidationMeta {
            id: Uuid::new_v4(),
            name: String::from("demo_orders"),
            status: String::from("READY"),
            row_count: 100,
            schema_version: 1,
            columns: cols,
            non_null_columns: BTreeSet::from([String::from("order_id")]),
        },
    );

    let clean_issues = validate(&doc, &ctx);
    assert!(
        clean_issues.is_empty(),
        "expected valid mapping: {clean_issues:?}"
    );

    // Nullable key_column when non_null_columns is populated -> MAPPING_INVALID
    if let Some(ds) = ctx.datasets.get_mut("demo_orders") {
        ds.non_null_columns = BTreeSet::from([String::from("status")]);
    }
    let bad_issues = validate(&doc, &ctx);
    assert!(bad_issues.iter().any(|i| i.code == "MAPPING_INVALID"));
}

#[test]
fn studio_rule9_and_rule10_cardinality_and_breaking_warnings() {
    let base_doc = sample_supply_chain_doc();
    let mut next_doc = base_doc.clone();
    // Remove Order type and FULFILLS edge while nodes/links exist, and narrow Warehouse.region STRING -> INT64
    next_doc.node_types.retain(|nt| nt.name != "Order");
    next_doc.edge_types.clear();
    if let Some(wh) = next_doc.node_types.first_mut() {
        wh.agg_rules.clear();
        if let Some(prop) = wh.properties.first_mut() {
            prop.data_type = DataType::Int64;
        }
    }

    let mut ctx = ValidationContext {
        subgraph_slug: String::from("main"),
        current_doc: Some(base_doc.clone()),
        ..ValidationContext::default()
    };
    ctx.existing_node_counts.insert(String::from("Order"), 20);
    ctx.existing_link_counts
        .insert(String::from("FULFILLS"), 20);

    let issues = validate(&next_doc, &ctx);
    let breaking_cnt = issues
        .iter()
        .filter(|i| i.code == "BREAKING_CHANGE" && i.severity == IssueSeverity::Warn)
        .count();
    assert_eq!(breaking_cnt, 3);

    // Cardinality violation warning on ONE_TO_MANY
    let mut card_ctx = ValidationContext::default();
    card_ctx
        .cardinality_violations
        .insert(String::from("FULFILLS"));
    let card_issues = validate(&base_doc, &card_ctx);
    assert!(
        card_issues
            .iter()
            .any(|i| { i.code == "CARDINALITY_VIOLATION" && i.severity == IssueSeverity::Warn })
    );
}

#[test]
fn studio_diff_schema_detects_added_removed_and_changed_fields() {
    let before = SchemaDoc {
        node_types: vec![NodeTypeSpec {
            id: None,
            name: String::from("Warehouse"),
            display_name: String::from("旧仓库"),
            description: String::new(),
            icon: String::from("box"),
            color: String::from("#2563eb"),
            title_template: String::from("{node_key}"),
            properties: Vec::new(),
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 120.0,
            pos_y: 120.0,
        }],
        edge_types: Vec::new(),
        mappings: Vec::new(),
    };
    let after = sample_supply_chain_doc();
    let diff = diff_schema(&before, &after);

    assert_eq!(diff.node_types.added, vec![String::from("Order")]);
    assert_eq!(diff.node_types.removed.len(), 0);
    assert_eq!(diff.node_types.changed.len(), 1);
    assert_eq!(
        diff.node_types.changed.first().map(|c| c.name.as_str()),
        Some("Warehouse")
    );
    assert_eq!(diff.edge_types.added, vec![String::from("FULFILLS")]);
}

fn logistics_subgraph_doc() -> SchemaDoc {
    SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                id: None,
                name: String::from("Warehouse"),
                display_name: String::from("仓库"),
                description: String::from("物流子图仓库视图"),
                icon: String::from("warehouse"),
                color: String::from("#2563eb"),
                title_template: String::from("{node_key}"),
                properties: vec![
                    attr_prop("region", DataType::String),
                    attr_prop("dock_count", DataType::Int64),
                ],
                agg_rules: Vec::new(),
                derived_rules: Vec::new(),
                sync_to_knowledge: false,
                pos_x: 200.0,
                pos_y: 200.0,
            },
            NodeTypeSpec {
                id: None,
                name: String::from("Shipment"),
                display_name: String::from("运单"),
                description: String::from("干线运输单"),
                icon: String::from("truck"),
                color: String::from("#0ea5e9"),
                title_template: String::from("{node_key}"),
                properties: vec![attr_prop("carrier", DataType::String)],
                agg_rules: Vec::new(),
                derived_rules: Vec::new(),
                sync_to_knowledge: false,
                pos_x: 420.0,
                pos_y: 160.0,
            },
        ],
        edge_types: vec![EdgeTypeSpec {
            id: None,
            name: String::from("SHIPS_FROM"),
            src_type_name: String::from("Shipment"),
            dst_type_name: String::from("Warehouse"),
            impact_weight: 0.75,
            cardinality: Cardinality::ManyToOne,
            description: String::new(),
            properties: Vec::new(),
            propagation: serde_json::json!({}),
        }],
        mappings: Vec::new(),
    }
}

#[test]
fn studio_merge_preview_and_apply_all_three_strategies() -> Result<(), AppError> {
    let main_doc = sample_supply_chain_doc();
    let sub_doc = logistics_subgraph_doc();

    let preview = preview_merge(
        "logistics",
        1,
        &sub_doc,
        2,
        &main_doc,
        &MergePreviewRequest::default(),
        None,
    );
    assert_eq!(preview.alignments.len(), 1);
    assert_eq!(preview.alignments.first().map(|a| a.score), Some(1.0));
    assert_eq!(preview.conflicts.len(), 0);
    assert_eq!(preview.added_node_types, vec![String::from("Shipment")]);

    let extended = apply_merge(
        "logistics",
        &sub_doc,
        &main_doc,
        &MergeSubgraphRequest {
            base_main_version: Some(2),
            default_strategy: SubgraphMergeStrategy::ExtendProps,
            resolutions: Vec::new(),
            change_summary: String::from("extend"),
            force: false,
        },
        None,
    )?;
    let ext_wh = extended
        .node_types
        .iter()
        .find(|nt| nt.name == "Warehouse")
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;
    assert!(ext_wh.properties.iter().any(|p| p.name == "dock_count"));
    assert!(extended.node_types.iter().any(|nt| nt.name == "Shipment"));

    let reused = apply_merge(
        "logistics",
        &sub_doc,
        &main_doc,
        &MergeSubgraphRequest {
            base_main_version: Some(2),
            default_strategy: SubgraphMergeStrategy::ReuseMain,
            resolutions: Vec::new(),
            change_summary: String::from("reuse"),
            force: false,
        },
        None,
    )?;
    let reuse_wh = reused
        .node_types
        .iter()
        .find(|nt| nt.name == "Warehouse")
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;
    assert!(!reuse_wh.properties.iter().any(|p| p.name == "dock_count"));

    let bridged = apply_merge(
        "logistics",
        &sub_doc,
        &main_doc,
        &MergeSubgraphRequest {
            base_main_version: Some(2),
            default_strategy: SubgraphMergeStrategy::ExtendProps,
            resolutions: vec![TypeMergeResolution {
                sub_type: String::from("Warehouse"),
                main_type: String::from("Warehouse"),
                strategy: SubgraphMergeStrategy::BridgeLink,
                bridge_edge_name: None,
            }],
            change_summary: String::from("bridge"),
            force: false,
        },
        None,
    )?;
    assert!(
        bridged
            .node_types
            .iter()
            .any(|nt| nt.name == "LogisticsWarehouse")
    );
    assert!(bridged.edge_types.iter().any(|e| {
        e.name == "SAME_ENTITY_AS"
            && e.src_type_name == "LogisticsWarehouse"
            && e.dst_type_name == "Warehouse"
            && e.cardinality == Cardinality::OneToOne
    }));

    Ok(())
}

#[test]
fn studio_merge_extend_props_rejects_unresolved_type_conflict() {
    let main_doc = sample_supply_chain_doc();
    let conflict_sub = SchemaDoc {
        node_types: vec![NodeTypeSpec {
            id: None,
            name: String::from("Warehouse"),
            display_name: String::from("仓库"),
            description: String::new(),
            icon: String::from("box"),
            color: String::from("#2563eb"),
            title_template: String::from("{node_key}"),
            // Conflict: region is INT64 instead of STRING
            properties: vec![attr_prop("region", DataType::Int64)],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 0.0,
            pos_y: 0.0,
        }],
        edge_types: Vec::new(),
        mappings: Vec::new(),
    };

    let preview = preview_merge(
        "fin",
        1,
        &conflict_sub,
        1,
        &main_doc,
        &MergePreviewRequest::default(),
        None,
    );
    assert!(
        preview
            .conflicts
            .iter()
            .any(|c| c.kind == MergeConflictKind::PropertyTypeConflict)
    );

    let err_res = apply_merge(
        "fin",
        &conflict_sub,
        &main_doc,
        &MergeSubgraphRequest {
            base_main_version: None,
            default_strategy: SubgraphMergeStrategy::ExtendProps,
            resolutions: Vec::new(),
            change_summary: String::from("conflict"),
            force: false,
        },
        None,
    );
    assert!(matches!(
        err_res,
        Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"))
    ));
}
