//! `xBlueStudio` node/edge modeling, `AxFlow` pipelines, and `AxSim` causal sandbox domain logic.

use std::collections::{BTreeMap, HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{FlowOperator, PiiCategory};
use crate::flow::{
    FlowEdge, FlowIssue, FlowNode, JoinMode, OnFailure, Pos, RetryPolicy, TriggerCfg,
};
use crate::graph::{GeoPoint, MetricsMerge};
use crate::studio::{LinkDirection, PropertyKind};

/// Property definition on an `xBlueStudio` node type (`06-STEP05-06` §05.2.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDef {
    /// Property key, e.g., `output`.
    pub name: String,
    /// Optional human-readable display label.
    #[serde(default)]
    pub display_name: String,
    /// Optional property description.
    #[serde(default)]
    pub description: String,
    /// Optional measurement unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Data type (`FLOAT64` / `STRING` / `BOOLEAN` or legacy `number` / `string` / `boolean`).
    pub data_type: String,
    /// Storage kind (`ATTRIBUTE` or `METRIC`).
    #[serde(default)]
    pub kind: PropertyKind,
    /// Whether missing values reject rows during dataset mapping.
    #[serde(default)]
    pub required: bool,
    /// Sensitive PII classification.
    #[serde(default = "default_pii_none")]
    pub pii_category: PiiCategory,
    /// Optional enumerated allowed values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<String>>,
}

const fn default_pii_none() -> PiiCategory {
    PiiCategory::None
}

/// Dynamic aggregation rule attached to an `xBlueStudio` node type (`06-STEP05-06` §05.2.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggRuleDef {
    /// Target metric field name.
    pub target_metric: String,
    /// Aggregation operator (`SUM`, `AVG`, `MAX`, `COUNT`, etc.).
    pub op: String,
    /// Traversed relationship type name.
    #[serde(default)]
    pub via_link_type: String,
    /// Relationship traversal direction (`OUT` or `IN`).
    #[serde(default)]
    pub direction: LinkDirection,
    /// Neighbor node type name providing `source_metric`.
    #[serde(default)]
    pub source_type_name: String,
    /// Neighbor metric field name.
    #[serde(default)]
    pub source_metric: String,
    /// Optional Step 06 filter expression evaluated on neighbor nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Whether `SUM` / `AVG` weights neighbor values by link weight.
    #[serde(default)]
    pub weight_by_link: bool,
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

/// `AxFlow` pipeline definition (`07-STEP07-08` §07.1 & §07.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxFlowDef {
    /// Flow UUID.
    pub id: Uuid,
    /// Flow name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Optional unique tenant-scoped webhook slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Draft DAG operator nodes.
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    /// Draft DAG dependency edges.
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
    /// Draft trigger configuration.
    #[serde(default)]
    pub trigger_cfg: TriggerCfg,
    /// Status (`DRAFT`, `PUBLISHED`).
    pub status: String,
    /// Creator user UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,
    /// Last execution timestamp.
    pub last_run_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// Last draft update timestamp.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
    /// Validation issues returned when saving/inspecting a draft.
    #[serde(default)]
    pub issues: Vec<FlowIssue>,
}

/// Request to create an `AxFlow` pipeline (`07-STEP07-08` §07.0 & §07.1).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAxFlowRequest {
    /// Flow name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Optional webhook slug.
    #[serde(default)]
    pub slug: Option<String>,
    /// DAG operator nodes.
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    /// DAG dependency edges.
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
    /// Trigger configuration (defaults to `MANUAL`).
    #[serde(default)]
    pub trigger_cfg: TriggerCfg,
    /// Legacy linear steps converted to a sequential chain DAG when `nodes` is empty (`07-STEP07-08` §07.1).
    #[serde(default)]
    pub steps: Option<Vec<AxFlowStep>>,
}

impl CreateAxFlowRequest {
    /// Resolves `(nodes, edges)` by preferring explicit `nodes`/`edges` or converting legacy `steps` into a chain DAG (`07-STEP07-08` §07.1).
    #[must_use]
    pub fn resolve_dag(&self) -> (Vec<FlowNode>, Vec<FlowEdge>) {
        if !self.nodes.is_empty() || !self.edges.is_empty() {
            return (self.nodes.clone(), self.edges.clone());
        }
        let Some(steps) = &self.steps else {
            return (Vec::new(), Vec::new());
        };
        convert_legacy_steps_to_dag(steps)
    }
}

/// Converts legacy linear [`AxFlowStep`]s into a sequential `(nodes, edges)` DAG (`07-STEP07-08` §07.1).
#[must_use]
pub fn convert_legacy_steps_to_dag(steps: &[AxFlowStep]) -> (Vec<FlowNode>, Vec<FlowEdge>) {
    let mut nodes = Vec::with_capacity(steps.len());
    let mut edges = Vec::with_capacity(steps.len().saturating_sub(1));
    let mut prev_id: Option<String> = None;

    for (idx, step) in steps.iter().enumerate() {
        let (kind, config) = match step.kind.as_str() {
            "INGEST" => (FlowOperator::SourceIngest, step.config.clone()),
            "TRANSFORM" => (FlowOperator::SqlTransform, step.config.clone()),
            "NODE_MAP" => (FlowOperator::XblueMap, step.config.clone()),
            "ACTION" => (FlowOperator::ActionEmit, step.config.clone()),
            _ => {
                let mut map = match &step.config {
                    serde_json::Value::Object(m) => m.clone(),
                    _ => serde_json::Map::new(),
                };
                map.insert(String::from("dry_run"), serde_json::Value::Bool(true));
                (FlowOperator::ActionEmit, serde_json::Value::Object(map))
            }
        };
        let x_index = u32::try_from(idx).unwrap_or(0);
        nodes.push(FlowNode {
            id: step.id.clone(),
            kind,
            label: step.label.clone(),
            config,
            retry: RetryPolicy::default(),
            timeout_s: 600,
            on_failure: OnFailure::FailRun,
            join: JoinMode::AllSuccess,
            pos: Pos {
                x: 200.0 * f64::from(x_index),
                y: 120.0,
            },
        });
        if let Some(from) = prev_id.replace(step.id.clone()) {
            edges.push(FlowEdge {
                from,
                to: step.id.clone(),
                when: None,
            });
        }
    }

    (nodes, edges)
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

/// A hypothetical "What-If" intervention applied inside `AxSim` (`09-STEP10` §10.3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxSimAction {
    /// Target node UUID.
    pub target_node_id: Uuid,
    /// Metric key to perturb (e.g., `output`, `inventory`).
    pub metric_key: String,
    /// Perturbation mode (`RATIO`, `ADD`, or `ABSOLUTE`).
    #[serde(default)]
    pub mode: Option<crate::enums::InterventionMode>,
    /// Perturbation numeric value used with `mode`.
    #[serde(default)]
    pub value: Option<f64>,
    /// Shorthand equivalent to `mode = RATIO`, `value = delta_ratio`.
    #[serde(default)]
    pub delta_ratio: Option<f64>,
    /// Time step when the intervention takes effect in `STEPPED` mode (default `0`).
    #[serde(default)]
    pub at_step: u32,
}

/// Request to create a new isolated `AxSim` sandbox (`09-STEP10` §10.2.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAxSandboxRequest {
    /// Sandbox name.
    pub name: String,
    /// Optional scope filter for bounded subgraph cloning.
    #[serde(default)]
    pub scope: Option<crate::sim::SandboxScope>,
}

/// Request to run quick causal simulation inside `AxSim` (`09-STEP10` §10.9).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulateAxSandboxRequest {
    /// Hypothetical actions to apply.
    pub actions: Vec<AxSimAction>,
    /// Optional solver parameters (defaults to steady-state mode).
    #[serde(default)]
    pub params: Option<crate::sim::SimParams>,
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
    /// Relative change ratio, if baseline is non-zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_ratio: Option<f64>,
    /// Propagation hop distance from the intervention origin (`0` = direct target).
    pub hop_distance: u32,
    /// Dominant propagation path `[seed, ..., node_id]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_path: Vec<Uuid>,
}

/// Complete `AxSim` simulation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSimResult {
    /// Affected nodes sorted by absolute impact.
    pub impacts: Vec<AxSimImpactRow>,
    /// Total nodes affected.
    pub affected_count: usize,
    /// Per-`(type_name, metric)` aggregated KPIs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kpis: Vec<crate::sim::SimKpiRow>,
    /// Diagnostic warnings emitted during propagation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::sim::SimWarning>,
    /// Whether cyclic SCC iteration converged.
    #[serde(default = "default_sim_converged")]
    pub converged: bool,
}

/// `AxSim` sandbox record returned to API clients (`09-STEP10` §10.1–§10.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSandboxRecord {
    /// Sandbox UUID.
    pub id: Uuid,
    /// Sandbox name.
    pub name: String,
    /// Provisioning status (`CLONING`, `READY`, `ARCHIVED`).
    #[serde(default)]
    pub status: crate::sim::SandboxStatus,
    /// Scope configuration used to clone production nodes.
    #[serde(default)]
    pub scope: crate::sim::SandboxScope,
    /// Snapshot node count.
    pub node_count: usize,
    /// Snapshot link count.
    pub link_count: usize,
    /// Timestamp when baseline was captured from production.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_captured_at: Option<DateTime<Utc>>,
    /// Published `xBlueStudio` schema version at capture time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i32>,
    /// Creator user UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,
    /// Applied actions (from quick `/simulate`).
    pub actions: Vec<AxSimAction>,
    /// Latest simulation result (from quick `/simulate`).
    pub sim_result: Option<AxSimResult>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

const fn default_sim_converged() -> bool {
    true
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
        let ratio = match (act.delta_ratio, act.mode, act.value) {
            (Some(dr), _, _) => dr,
            (None, Some(crate::enums::InterventionMode::Ratio), Some(v)) => v,
            _ => continue,
        };
        queue.push_back((act.target_node_id, act.metric_key.clone(), ratio, 0));
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
        if let Some(node) = node_map.get(&node_id)
            && let Some(&before_val) = node.metrics.get(&metric_key)
        {
            let after_val = (before_val * (1.0 + total_ratio)).max(0.0);
            let delta = after_val - before_val;
            let delta_ratio = if before_val.abs() > f64::EPSILON {
                Some(delta / before_val)
            } else {
                None
            };
            impacts.push(AxSimImpactRow {
                node_id,
                type_name: node.type_name.clone(),
                title: node.title.clone(),
                metric_key,
                before_val,
                after_val,
                delta,
                delta_ratio,
                hop_distance,
                top_path: Vec::new(),
            });
        }
    }

    impacts.sort_by(|a, b| b.delta.abs().total_cmp(&a.delta.abs()));
    let affected_count = impacts.len();
    AxSimResult {
        impacts,
        affected_count,
        kpis: Vec::new(),
        warnings: Vec::new(),
        converged: true,
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
            mode: None,
            value: None,
            delta_ratio: Some(-0.5),
            at_step: 0,
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
