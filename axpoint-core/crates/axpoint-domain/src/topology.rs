//! `xBlueStudio` node/edge modeling, `AxFlow` pipelines, and `AxSim` causal sandbox domain logic.

use std::collections::{BTreeMap, HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::{GeoPoint, MetricsMerge};

/// Property definition on an `xBlueStudio` node type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDef {
    /// Property key, e.g., `output`.
    pub name: String,
    /// Data type (`number`, `string`, `boolean`).
    pub data_type: String,
}

/// Dynamic aggregation rule attached to an `xBlueStudio` node type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggRuleDef {
    /// Target metric field name.
    pub target_metric: String,
    /// Aggregation operator (`SUM`, `AVG`, `MAX`, `COUNT`).
    pub op: String,
}

/// `xBlueStudio` node type definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueNodeTypeDef {
    /// Type identifier.
    pub id: Uuid,
    /// Unique machine name, e.g., `Factory`.
    pub name: String,
    /// Human-readable display name, e.g., `制造工厂`.
    pub display_name: String,
    /// Property definitions.
    pub properties: Vec<PropertyDef>,
    /// Dynamic aggregation rules.
    pub agg_rules: Vec<AggRuleDef>,
    /// Canvas X coordinate.
    pub pos_x: f64,
    /// Canvas Y coordinate.
    pub pos_y: f64,
}

/// `xBlueStudio` edge type definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueEdgeTypeDef {
    /// Edge type identifier.
    pub id: Uuid,
    /// Relation name, e.g., `SUPPLIES`.
    pub name: String,
    /// Source node type name.
    pub src_type_name: String,
    /// Target node type name.
    pub dst_type_name: String,
    /// Causal propagation attenuation weight in `(0.0, 1.0]`.
    pub impact_weight: f64,
}

/// Complete `xBlueStudio` schema bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XBlueSchema {
    /// Defined node types.
    pub node_types: Vec<BlueNodeTypeDef>,
    /// Defined edge types.
    pub edge_types: Vec<BlueEdgeTypeDef>,
}

/// Request payload to save/upsert an `xBlueStudio` schema.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveXBlueSchemaRequest {
    /// Node types to upsert.
    pub node_types: Vec<BlueNodeTypeInput>,
    /// Edge types to upsert.
    pub edge_types: Vec<BlueEdgeTypeInput>,
}

/// Input node type definition.
#[derive(Debug, Clone, Deserialize)]
pub struct BlueNodeTypeInput {
    /// Unique machine name.
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// Properties.
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
    /// Aggregation rules.
    #[serde(default)]
    pub agg_rules: Vec<AggRuleDef>,
    /// Canvas X coordinate.
    #[serde(default = "default_pos")]
    pub pos_x: f64,
    /// Canvas Y coordinate.
    #[serde(default = "default_pos")]
    pub pos_y: f64,
}

/// Input edge type definition.
#[derive(Debug, Clone, Deserialize)]
pub struct BlueEdgeTypeInput {
    /// Relation name.
    pub name: String,
    /// Source node type name.
    pub src_type_name: String,
    /// Target node type name.
    pub dst_type_name: String,
    /// Causal propagation weight.
    #[serde(default = "default_weight")]
    pub impact_weight: f64,
}

/// Runtime `xBlueNode` business node instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueNode {
    /// Node UUID.
    pub id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Arbitrary JSON static properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Numeric/business metrics snapshot map.
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    /// Optional WGS-84 geographic coordinate.
    #[serde(default)]
    pub geo: Option<GeoPoint>,
    /// Optional originating dataset UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dataset_id: Option<Uuid>,
    /// Optional originating dataset primary key string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_row_key: Option<String>,
    /// Optimistic concurrency row version (`>= 1`).
    #[serde(default = "default_row_version")]
    pub row_version: i64,
    /// Creation timestamp.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

const fn default_row_version() -> i64 {
    1
}

/// Runtime `xBlueNode` business link instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueLink {
    /// Link UUID.
    pub id: Uuid,
    /// Link type name.
    pub link_type: String,
    /// Source node UUID.
    pub src_node_id: Uuid,
    /// Target node UUID.
    pub dst_node_id: Uuid,
    /// Causal propagation weight.
    pub weight: f64,
}

/// Request to upsert a runtime `BlueNode` (and optionally link it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertBlueNodeRequest {
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Static JSON properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Numeric metrics snapshot.
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    /// Merge strategy (`MERGE` by default, or `REPLACE`).
    #[serde(default)]
    pub metrics_merge: MetricsMerge,
    /// Optional WGS-84 coordinate (`{ lng, lat }`).
    #[serde(default)]
    pub geo: Option<GeoPoint>,
    /// Optional optimistic concurrency check (`409 VERSION_CONFLICT` on mismatch).
    #[serde(default)]
    pub if_row_version: Option<i64>,
    /// Optional target node ID to link to.
    #[serde(default)]
    pub link_to_node_id: Option<Uuid>,
    /// Optional link type name (required when `link_to_node_id` is set).
    #[serde(default)]
    pub link_type: Option<String>,
    /// Optional link propagation weight (defaults to `xb_edge_types.impact_weight`).
    #[serde(default)]
    pub link_weight: Option<f64>,
}

/// A single step in an `AxFlow` pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxFlowStep {
    /// Step identifier.
    pub id: String,
    /// Step kind (`INGEST`, `TRANSFORM`, `NODE_MAP`, `ACTION`).
    pub kind: String,
    /// Step label.
    pub label: String,
    /// Step configuration JSON.
    pub config: serde_json::Value,
}

/// `AxFlow` pipeline definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxFlowDef {
    /// Flow UUID.
    pub id: Uuid,
    /// Flow name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Ordered pipeline steps.
    pub steps: Vec<AxFlowStep>,
    /// Status (`DRAFT`, `PUBLISHED`).
    pub status: String,
    /// Last execution timestamp.
    pub last_run_at: Option<DateTime<Utc>>,
}

/// Request to create an `AxFlow` pipeline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAxFlowRequest {
    /// Flow name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Flow steps.
    pub steps: Vec<AxFlowStep>,
}

/// Result of executing an `AxFlow` pipeline.
#[derive(Debug, Clone, Serialize)]
pub struct AxFlowRunResult {
    /// Flow UUID.
    pub flow_id: Uuid,
    /// Execution status.
    pub status: String,
    /// Number of steps executed.
    pub steps_executed: usize,
    /// Execution trace logs.
    pub logs: Vec<String>,
}

/// Isolated snapshot of production `BlueNode`s and `BlueLink`s cloned into `AxSim`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSimSnapshot {
    /// Cloned nodes.
    pub nodes: Vec<BlueNode>,
    /// Cloned links.
    pub links: Vec<BlueLink>,
}

/// A hypothetical "What-If" intervention applied inside `AxSim`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSimAction {
    /// Target node UUID.
    pub target_node_id: Uuid,
    /// Metric key to perturb (e.g., `output`, `inventory`).
    pub metric_key: String,
    /// Relative percentage delta (e.g., `-0.5` for -50% capacity drop).
    pub delta_ratio: f64,
}

/// Request to create a new isolated `AxSim` sandbox.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAxSandboxRequest {
    /// Sandbox name.
    pub name: String,
}

/// Request to run causal simulation inside `AxSim`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulateAxSandboxRequest {
    /// Hypothetical actions to apply.
    pub actions: Vec<AxSimAction>,
}

/// Impact row comparing baseline vs. simulated metric for one node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSimImpactRow {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Node title.
    pub title: String,
    /// Metric key.
    pub metric_key: String,
    /// Baseline value before simulation.
    pub before_val: f64,
    /// Simulated value after causal propagation.
    pub after_val: f64,
    /// Absolute change (`after_val - before_val`).
    pub delta: f64,
    /// Propagation hop distance from the intervention origin (`0` = direct target).
    pub hop_distance: u32,
}

/// Complete `AxSim` simulation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSimResult {
    /// Affected nodes sorted by absolute impact.
    pub impacts: Vec<AxSimImpactRow>,
    /// Total nodes affected.
    pub affected_count: usize,
}

/// `AxSim` sandbox record returned to API clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSandboxRecord {
    /// Sandbox UUID.
    pub id: Uuid,
    /// Sandbox name.
    pub name: String,
    /// Snapshot node count.
    pub node_count: usize,
    /// Snapshot link count.
    pub link_count: usize,
    /// Applied actions.
    pub actions: Vec<AxSimAction>,
    /// Latest simulation result.
    pub sim_result: Option<AxSimResult>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Copy-on-Write causal propagation engine for `AxSim` sandboxes.
///
/// Clones the baseline snapshot in memory and propagates metric perturbations
/// along directed `BlueLink` edges with weight attenuation, leaving production untouched.
#[must_use]
pub fn simulate_causal_propagation(
    snapshot: &AxSimSnapshot,
    actions: &[AxSimAction],
) -> AxSimResult {
    let node_map: HashMap<Uuid, &BlueNode> = snapshot.nodes.iter().map(|n| (n.id, n)).collect();

    let mut adj: HashMap<Uuid, Vec<&BlueLink>> = HashMap::new();
    for link in &snapshot.links {
        adj.entry(link.src_node_id).or_default().push(link);
    }

    let mut ratio_changes: HashMap<(Uuid, String), (f64, u32)> = HashMap::new();
    let mut queue: VecDeque<(Uuid, String, f64, u32)> = VecDeque::new();

    for act in actions {
        queue.push_back((
            act.target_node_id,
            act.metric_key.clone(),
            act.delta_ratio,
            0,
        ));
    }

    while let Some((curr_id, metric_key, delta_ratio, hop)) = queue.pop_front() {
        if delta_ratio.abs() < 1e-4 || hop > 6 {
            continue;
        }

        let entry = ratio_changes
            .entry((curr_id, metric_key.clone()))
            .or_insert((0.0, hop));
        entry.0 += delta_ratio;
        if hop < entry.1 {
            entry.1 = hop;
        }

        if let Some(neighbors) = adj.get(&curr_id) {
            for link in neighbors {
                let propagated = delta_ratio * link.weight.clamp(0.0, 1.0);
                queue.push_back((
                    link.dst_node_id,
                    metric_key.clone(),
                    propagated,
                    hop.saturating_add(1),
                ));
            }
        }
    }

    let mut impacts = Vec::new();
    for ((node_id, metric_key), (total_ratio, hop_distance)) in ratio_changes {
        if let Some(node) = node_map.get(&node_id) {
            let before_val = node.metrics.get(&metric_key).copied().unwrap_or(100.0);
            let after_val = (before_val * (1.0 + total_ratio)).max(0.0);
            let delta = after_val - before_val;
            impacts.push(AxSimImpactRow {
                node_id,
                type_name: node.type_name.clone(),
                title: node.title.clone(),
                metric_key,
                before_val,
                after_val,
                delta,
                hop_distance,
            });
        }
    }

    impacts.sort_by(|a, b| b.delta.abs().total_cmp(&a.delta.abs()));
    let affected_count = impacts.len();
    AxSimResult {
        impacts,
        affected_count,
    }
}

const fn default_pos() -> f64 {
    120.0
}

const fn default_weight() -> f64 {
    0.6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_causal_propagation_multi_hop() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let id_c = Uuid::new_v4();

        let now = Utc::now();
        let snapshot = AxSimSnapshot {
            nodes: vec![
                BlueNode {
                    id: id_a,
                    type_name: String::from("Factory"),
                    node_key: String::from("F-A"),
                    title: String::from("华东晶圆厂"),
                    properties: serde_json::Map::new(),
                    metrics: BTreeMap::from([(String::from("output"), 1000.0)]),
                    geo: None,
                    source_dataset_id: None,
                    source_row_key: None,
                    row_version: 1,
                    created_at: now,
                    updated_at: now,
                },
                BlueNode {
                    id: id_b,
                    type_name: String::from("Warehouse"),
                    node_key: String::from("W-B"),
                    title: String::from("中心配送仓"),
                    properties: serde_json::Map::new(),
                    metrics: BTreeMap::from([(String::from("output"), 500.0)]),
                    geo: None,
                    source_dataset_id: None,
                    source_row_key: None,
                    row_version: 1,
                    created_at: now,
                    updated_at: now,
                },
                BlueNode {
                    id: id_c,
                    type_name: String::from("Order"),
                    node_key: String::from("O-C"),
                    title: String::from("重点客户订单"),
                    properties: serde_json::Map::new(),
                    metrics: BTreeMap::from([(String::from("output"), 200.0)]),
                    geo: None,
                    source_dataset_id: None,
                    source_row_key: None,
                    row_version: 1,
                    created_at: now,
                    updated_at: now,
                },
            ],
            links: vec![
                BlueLink {
                    id: Uuid::new_v4(),
                    link_type: String::from("SUPPLIES"),
                    src_node_id: id_a,
                    dst_node_id: id_b,
                    weight: 0.8,
                },
                BlueLink {
                    id: Uuid::new_v4(),
                    link_type: String::from("FULFILLS"),
                    src_node_id: id_b,
                    dst_node_id: id_c,
                    weight: 0.5,
                },
            ],
        };

        let actions = vec![AxSimAction {
            target_node_id: id_a,
            metric_key: String::from("output"),
            delta_ratio: -0.5,
        }];

        let res = simulate_causal_propagation(&snapshot, &actions);
        assert_eq!(res.affected_count, 3);

        let impact_c = res.impacts.iter().find(|r| r.node_id == id_c);
        assert!(impact_c.is_some());
        if let Some(c) = impact_c {
            assert!((c.after_val - 160.0).abs() < 1e-6);
            assert_eq!(c.hop_distance, 2);
        }
    }
}
