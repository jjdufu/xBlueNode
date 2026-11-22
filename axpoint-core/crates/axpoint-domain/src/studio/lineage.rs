//! Deterministic 5-stage horizontal data lineage graph builder for a single `NodeTypeSpec` (`04-STEP18` §18.4.1).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppError;
use crate::enums::DataType;
use crate::studio::doc::{
    DownstreamConsumerBinding, NodeBadgeCounts, NodeTypeSpec, PropertyKind, SchemaDoc,
};

/// Five horizontal pipeline stages in `NodeLineageGraph` (`04-STEP18` §18.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LineageStageKind {
    /// Stage 1: Physical `AxHub` dataset columns feeding mappings.
    DatasetColumns,
    /// Stage 2: `MappingSpec` key/attribute/metric/link transforms.
    MappingSpec,
    /// Stage 3: `NodeTypeSpec` primary key, attributes, and metrics (plus upstream neighbor metric inputs).
    NodeSchema,
    /// Stage 4: Intra-node `DerivedRuleSpec` and cross-link `AggRuleSpec` compute formulas.
    ComputeRules,
    /// Stage 5: Downstream alert rules, published endpoints, and `xBlueView` dashboards.
    DownstreamConsumers,
}

/// Single node item within a [`LineageStage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageStageNode {
    /// Stable node identifier within the lineage graph (e.g. `"prop:Warehouse.stock_qty"`).
    pub id: String,
    /// Compact display label.
    pub label: String,
    /// Structured metadata map (`data_type`, `kind`, `maturity`, `consumer_kind`, etc.).
    #[serde(default)]
    pub meta: serde_json::Map<String, serde_json::Value>,
}

/// Ordered column bucket in [`NodeLineageGraph`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageStage {
    /// Pipeline stage discriminator.
    pub stage: LineageStageKind,
    /// Ordered nodes rendered in this stage column.
    pub nodes: Vec<LineageStageNode>,
}

/// Directed edge between two [`LineageStageNode`] items.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineageEdge {
    /// Source `LineageStageNode.id`.
    pub from: String,
    /// Target `LineageStageNode.id`.
    pub to: String,
    /// Concise transformation label (`DIRECT`, `SYNC`, `KEY`, `INPUT`, `AGG`, `DERIVED`, `EVAL`, `EXPOSE`, `VIEW`).
    pub label: String,
}

/// Complete 5-stage left-to-right lineage pipeline response for `GET /api/xblue/subgraphs/{slug}/lineage/{node_type}` (`04-STEP18` §18.4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLineageGraph {
    /// Subgraph slug.
    pub subgraph_slug: String,
    /// Target `NodeTypeSpec.name`.
    pub node_type: String,
    /// Human-readable `NodeTypeSpec.display_name`.
    pub display_name: String,
    /// Compact `Np·Nm·Nr` element counts.
    pub badge_counts: NodeBadgeCounts,
    /// Always 5 ordered stage columns (`DATASET_COLUMNS`..`DOWNSTREAM_CONSUMERS`).
    pub stages: Vec<LineageStage>,
    /// Directed lineage edges connecting stage nodes.
    pub edges: Vec<LineageEdge>,
}

/// Deterministically compiles the 5-stage horizontal lineage graph for `node_type` inside `doc`.
///
/// # Errors
/// Returns [`AppError::NotFound`] (`"NODE_TYPE_NOT_FOUND"`) when `node_type` does not exist in `doc`.
pub fn build_node_lineage(
    subgraph_slug: &str,
    doc: &SchemaDoc,
    node_type: &str,
    dataset_columns: &BTreeMap<String, BTreeMap<String, DataType>>,
    downstream: &[DownstreamConsumerBinding],
) -> Result<NodeLineageGraph, AppError> {
    let nt = doc
        .node_types
        .iter()
        .find(|item| item.name == node_type)
        .ok_or(AppError::NotFound("NODE_TYPE_NOT_FOUND"))?;

    let mut stage1 = Vec::new();
    let mut stage1_seen = BTreeSet::new();
    let mut stage2 = Vec::new();
    let mut stage3 = Vec::new();
    let mut stage3_seen = BTreeSet::new();
    let mut stage4 = Vec::new();
    let mut stage5 = Vec::new();
    let mut edges = BTreeSet::new();

    populate_schema_nodes(nt, &mut stage3, &mut stage3_seen);
    populate_mapping_stages(
        doc,
        nt,
        dataset_columns,
        &mut stage1,
        &mut stage1_seen,
        &mut stage2,
        &mut edges,
    );
    populate_rule_stage(nt, &mut stage3, &mut stage3_seen, &mut stage4, &mut edges);
    populate_downstream_stage(nt, downstream, &mut stage5, &mut edges);

    Ok(NodeLineageGraph {
        subgraph_slug: String::from(subgraph_slug),
        node_type: nt.name.clone(),
        display_name: nt.display_name.clone(),
        badge_counts: nt.badge_counts(),
        stages: vec![
            LineageStage {
                stage: LineageStageKind::DatasetColumns,
                nodes: stage1,
            },
            LineageStage {
                stage: LineageStageKind::MappingSpec,
                nodes: stage2,
            },
            LineageStage {
                stage: LineageStageKind::NodeSchema,
                nodes: stage3,
            },
            LineageStage {
                stage: LineageStageKind::ComputeRules,
                nodes: stage4,
            },
            LineageStage {
                stage: LineageStageKind::DownstreamConsumers,
                nodes: stage5,
            },
        ],
        edges: edges.into_iter().collect(),
    })
}

fn populate_schema_nodes(
    nt: &NodeTypeSpec,
    stage3: &mut Vec<LineageStageNode>,
    seen: &mut BTreeSet<String>,
) {
    let key_id = format!("key:{}.node_key", nt.name);
    seen.insert(key_id.clone());
    let mut key_meta = serde_json::Map::new();
    key_meta.insert(String::from("kind"), json!("PRIMARY_KEY"));
    key_meta.insert(String::from("maturity"), json!(nt.maturity.as_str()));
    stage3.push(LineageStageNode {
        id: key_id,
        label: String::from("node_key"),
        meta: key_meta,
    });

    for prop in &nt.properties {
        let pid = format!("prop:{}.{}", nt.name, prop.name);
        seen.insert(pid.clone());
        let label = match prop.unit.as_deref().filter(|u| !u.trim().is_empty()) {
            Some(u) => format!("{} ({u})", prop.name),
            None => prop.name.clone(),
        };
        let mut meta = serde_json::Map::new();
        meta.insert(String::from("kind"), json!(prop.kind.as_str()));
        meta.insert(String::from("data_type"), json!(prop.data_type.as_str()));
        meta.insert(String::from("maturity"), json!(prop.maturity.as_str()));
        meta.insert(String::from("display_name"), json!(prop.display_name));
        stage3.push(LineageStageNode {
            id: pid,
            label,
            meta,
        });
    }
}

fn populate_mapping_stages(
    doc: &SchemaDoc,
    nt: &NodeTypeSpec,
    dataset_columns: &BTreeMap<String, BTreeMap<String, DataType>>,
    stage1: &mut Vec<LineageStageNode>,
    stage1_seen: &mut BTreeSet<String>,
    stage2: &mut Vec<LineageStageNode>,
    edges: &mut BTreeSet<LineageEdge>,
) {
    let key_target_id = format!("key:{}.node_key", nt.name);
    for mapping in doc.mappings.iter().filter(|m| m.node_type == nt.name) {
        let map_id = format!("map:{}", mapping.name);
        let mut map_meta = serde_json::Map::new();
        map_meta.insert(String::from("dataset"), json!(mapping.dataset));
        map_meta.insert(String::from("key_column"), json!(mapping.key_column));
        map_meta.insert(String::from("mode"), json!(mapping.mode));
        let mode_str = match mapping.mode {
            crate::studio::doc::MappingMode::Full => "FULL",
            crate::studio::doc::MappingMode::Incremental => "INCREMENTAL",
        };
        stage2.push(LineageStageNode {
            id: map_id.clone(),
            label: format!("{} ({mode_str})", mapping.name),
            meta: map_meta,
        });

        let key_col_id = ensure_dataset_col_node(
            &mapping.dataset,
            &mapping.key_column,
            dataset_columns,
            stage1,
            stage1_seen,
        );
        edges.insert(LineageEdge {
            from: key_col_id,
            to: map_id.clone(),
            label: String::from("KEY"),
        });
        edges.insert(LineageEdge {
            from: map_id.clone(),
            to: key_target_id.clone(),
            label: String::from("SYNC"),
        });

        for (attr_name, src_col) in &mapping.attributes {
            let col_id = ensure_dataset_col_node(
                &mapping.dataset,
                src_col,
                dataset_columns,
                stage1,
                stage1_seen,
            );
            edges.insert(LineageEdge {
                from: col_id,
                to: map_id.clone(),
                label: String::from("DIRECT"),
            });
            edges.insert(LineageEdge {
                from: map_id.clone(),
                to: format!("prop:{}.{attr_name}", nt.name),
                label: String::from("SYNC"),
            });
        }

        for (metric_name, src_col) in &mapping.metrics {
            let col_id = ensure_dataset_col_node(
                &mapping.dataset,
                src_col,
                dataset_columns,
                stage1,
                stage1_seen,
            );
            edges.insert(LineageEdge {
                from: col_id,
                to: map_id.clone(),
                label: String::from("DIRECT"),
            });
            edges.insert(LineageEdge {
                from: map_id.clone(),
                to: format!("prop:{}.{metric_name}", nt.name),
                label: String::from("SYNC"),
            });
        }
    }
}

fn ensure_dataset_col_node(
    dataset: &str,
    col: &str,
    dataset_columns: &BTreeMap<String, BTreeMap<String, DataType>>,
    stage1: &mut Vec<LineageStageNode>,
    seen: &mut BTreeSet<String>,
) -> String {
    let id = format!("col:{dataset}.{col}");
    if seen.insert(id.clone()) {
        let dt = dataset_columns
            .get(dataset)
            .and_then(|cols| cols.get(col))
            .copied()
            .unwrap_or(DataType::String);
        let mut meta = serde_json::Map::new();
        meta.insert(String::from("dataset"), json!(dataset));
        meta.insert(String::from("column"), json!(col));
        meta.insert(String::from("data_type"), json!(dt.as_str()));
        stage1.push(LineageStageNode {
            id: id.clone(),
            label: format!("{dataset}.{col}"),
            meta,
        });
    }
    id
}

fn populate_rule_stage(
    nt: &NodeTypeSpec,
    stage3: &mut Vec<LineageStageNode>,
    stage3_seen: &mut BTreeSet<String>,
    stage4: &mut Vec<LineageStageNode>,
    edges: &mut BTreeSet<LineageEdge>,
) {
    for agg in &nt.agg_rules {
        let rule_id = format!("agg:{}.{}", nt.name, agg.target_metric);
        let mut meta = serde_json::Map::new();
        meta.insert(String::from("kind"), json!("AGG"));
        meta.insert(String::from("op"), json!(agg.op.as_str()));
        meta.insert(String::from("via_link_type"), json!(agg.via_link_type));
        meta.insert(String::from("maturity"), json!(agg.maturity.as_str()));
        stage4.push(LineageStageNode {
            id: rule_id.clone(),
            label: format!(
                "{}({}.{} via {})",
                agg.op.as_str(),
                agg.source_type_name,
                agg.source_metric,
                agg.via_link_type
            ),
            meta,
        });

        let upstream_id = format!("upstream:{}.{}", agg.source_type_name, agg.source_metric);
        if stage3_seen.insert(upstream_id.clone()) {
            let mut up_meta = serde_json::Map::new();
            up_meta.insert(String::from("kind"), json!("UPSTREAM_METRIC"));
            up_meta.insert(String::from("source_type"), json!(agg.source_type_name));
            stage3.push(LineageStageNode {
                id: upstream_id.clone(),
                label: format!("{}.{}", agg.source_type_name, agg.source_metric),
                meta: up_meta,
            });
        }

        edges.insert(LineageEdge {
            from: upstream_id,
            to: rule_id.clone(),
            label: String::from("INPUT"),
        });
        edges.insert(LineageEdge {
            from: rule_id,
            to: format!("prop:{}.{}", nt.name, agg.target_metric),
            label: String::from("AGG"),
        });
    }

    for derived in &nt.derived_rules {
        let rule_id = format!("derived:{}.{}", nt.name, derived.target_metric);
        let mut meta = serde_json::Map::new();
        meta.insert(String::from("kind"), json!("DERIVED"));
        meta.insert(String::from("enabled"), json!(derived.enabled));
        meta.insert(String::from("maturity"), json!(derived.maturity.as_str()));
        stage4.push(LineageStageNode {
            id: rule_id.clone(),
            label: derived.expression.clone(),
            meta,
        });

        for prop in &nt.properties {
            if prop.name != derived.target_metric
                && expression_mentions(&derived.expression, &prop.name)
            {
                edges.insert(LineageEdge {
                    from: format!("prop:{}.{}", nt.name, prop.name),
                    to: rule_id.clone(),
                    label: String::from("INPUT"),
                });
            }
        }
        edges.insert(LineageEdge {
            from: rule_id,
            to: format!("prop:{}.{}", nt.name, derived.target_metric),
            label: String::from("DERIVED"),
        });
    }
}

fn populate_downstream_stage(
    nt: &NodeTypeSpec,
    downstream: &[DownstreamConsumerBinding],
    stage5: &mut Vec<LineageStageNode>,
    edges: &mut BTreeSet<LineageEdge>,
) {
    let key_id = format!("key:{}.node_key", nt.name);
    for item in downstream {
        let mut meta = serde_json::Map::new();
        meta.insert(
            String::from("consumer_kind"),
            json!(item.consumer_kind.as_str()),
        );
        if let Some(m) = &item.target_metric {
            meta.insert(String::from("target_metric"), json!(m));
        }
        stage5.push(LineageStageNode {
            id: item.id.clone(),
            label: item.label.clone(),
            meta,
        });

        let from_id = item
            .target_metric
            .as_deref()
            .filter(|m| {
                nt.properties
                    .iter()
                    .any(|p| p.kind == PropertyKind::Metric && p.name == *m)
            })
            .map_or_else(|| key_id.clone(), |m| format!("prop:{}.{m}", nt.name));
        edges.insert(LineageEdge {
            from: from_id,
            to: item.id.clone(),
            label: String::from(item.consumer_kind.edge_label()),
        });
    }
}

fn expression_mentions(expr: &str, ident: &str) -> bool {
    if ident.is_empty() {
        return false;
    }
    let bytes = expr.as_bytes();
    let needle = ident.as_bytes();
    if needle.len() > bytes.len() {
        return false;
    }
    let mut pos = 0_usize;
    while pos.saturating_add(needle.len()) <= bytes.len() {
        if bytes.get(pos..pos.saturating_add(needle.len())) == Some(needle) {
            let left_ok = pos == 0
                || bytes
                    .get(pos.saturating_sub(1))
                    .is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'));
            let right_ok = bytes
                .get(pos.saturating_add(needle.len()))
                .is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'));
            if left_ok && right_ok {
                return true;
            }
        }
        pos = pos.saturating_add(1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enums::{AggFn, Cardinality};
    use crate::studio::doc::{
        AggRuleSpec, DerivedRuleSpec, DownstreamConsumerKind, EdgeTypeSpec, LinkDirection,
        MappingMode, MappingSpec, PropertySpec,
    };

    fn sample_warehouse_doc() -> SchemaDoc {
        SchemaDoc {
            node_types: vec![
                NodeTypeSpec {
                    name: String::from("Supplier"),
                    display_name: String::from("供应商"),
                    properties: vec![PropertySpec {
                        name: String::from("lead_days"),
                        display_name: String::from("交付周期"),
                        data_type: DataType::Float64,
                        kind: PropertyKind::Metric,
                        ..PropertySpec::default()
                    }],
                    ..NodeTypeSpec::default()
                },
                NodeTypeSpec {
                    name: String::from("Warehouse"),
                    display_name: String::from("区域仓"),
                    properties: vec![
                        PropertySpec {
                            name: String::from("stock_qty"),
                            display_name: String::from("在库量"),
                            unit: Some(String::from("件")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                        PropertySpec {
                            name: String::from("safety_stock"),
                            display_name: String::from("安全库存"),
                            unit: Some(String::from("件")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                        PropertySpec {
                            name: String::from("shortage_gap"),
                            display_name: String::from("缺口"),
                            unit: Some(String::from("件")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                        PropertySpec {
                            name: String::from("avg_lead"),
                            display_name: String::from("上游交期"),
                            unit: Some(String::from("天")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                    ],
                    agg_rules: vec![AggRuleSpec {
                        target_metric: String::from("avg_lead"),
                        op: AggFn::Avg,
                        via_link_type: String::from("SUPPLIES"),
                        direction: LinkDirection::In,
                        source_type_name: String::from("Supplier"),
                        source_metric: String::from("lead_days"),
                        ..AggRuleSpec::default()
                    }],
                    derived_rules: vec![DerivedRuleSpec {
                        target_metric: String::from("shortage_gap"),
                        expression: String::from("max(0, safety_stock - stock_qty)"),
                        enabled: true,
                        ..DerivedRuleSpec::default()
                    }],
                    ..NodeTypeSpec::default()
                },
            ],
            edge_types: vec![EdgeTypeSpec {
                name: String::from("SUPPLIES"),
                src_type_name: String::from("Supplier"),
                dst_type_name: String::from("Warehouse"),
                cardinality: Cardinality::ManyToMany,
                ..EdgeTypeSpec::default()
            }],
            mappings: vec![MappingSpec {
                name: String::from("wh_sync"),
                dataset: String::from("demo_warehouses"),
                node_type: String::from("Warehouse"),
                key_column: String::from("wh_code"),
                title: String::from("{wh_code}"),
                filter: None,
                attributes: BTreeMap::new(),
                metrics: BTreeMap::from([(String::from("stock_qty"), String::from("stock_qty"))]),
                metric_time_column: None,
                geo: crate::studio::doc::MappingGeoSpec::default(),
                links: Vec::new(),
                on_source_delete: crate::studio::doc::OnSourceDelete::Keep,
                mode: MappingMode::Full,
            }],
        }
    }

    #[test]
    fn builds_five_stage_lineage_with_mappings_rules_and_consumers() -> Result<(), AppError> {
        let doc = sample_warehouse_doc();
        let mut cols = BTreeMap::new();
        cols.insert(
            String::from("demo_warehouses"),
            BTreeMap::from([
                (String::from("wh_code"), DataType::String),
                (String::from("stock_qty"), DataType::Float64),
            ]),
        );
        let consumers = vec![DownstreamConsumerBinding {
            id: String::from("rule:low_stock_alert"),
            label: String::from("库存低于安全线告警"),
            consumer_kind: DownstreamConsumerKind::AlertRule,
            target_metric: Some(String::from("shortage_gap")),
        }];

        let lineage = build_node_lineage("main", &doc, "Warehouse", &cols, &consumers)?;
        assert_eq!(lineage.stages.len(), 5);
        assert_eq!(lineage.badge_counts.property_count, 4);
        assert_eq!(lineage.badge_counts.metric_count, 4);
        assert_eq!(lineage.badge_counts.rule_count, 2);
        assert!(lineage.edges.iter().any(|e| e.to == "rule:low_stock_alert"));
        Ok(())
    }
}
