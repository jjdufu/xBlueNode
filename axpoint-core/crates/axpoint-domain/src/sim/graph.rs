//! `SimGraph` in-memory representation, zero-index compiled graph, and `xb_edge_types.propagation` rules (`09-STEP10` §10.3.2).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::InterventionMode;
use crate::geo_ops::GeoShapeDto;
use crate::graph::GeoPoint;
use crate::sim::{ImpactRow, SimKpiRow};

/// Compact zero-based node index into [`IndexedSimGraph::nodes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeIdx(pub u32);

impl NodeIdx {
    /// Converts a zero-based `usize` slice offset into a [`NodeIdx`].
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if `idx` exceeds `u32::MAX`.
    pub fn from_usize(idx: usize) -> Result<Self, AppError> {
        let raw = u32::try_from(idx).map_err(|_| {
            AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("node index {idx} exceeds u32 capacity"),
            )
        })?;
        Ok(Self(raw))
    }

    /// Returns the zero-based `usize` offset into [`IndexedSimGraph::nodes`].
    #[must_use]
    pub fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// Single propagation rule inside `xb_edge_types.propagation.rules` (`§10.3.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropagationRule {
    /// Upstream metric key on source node.
    pub src_metric: String,
    /// Downstream metric key on target node.
    pub dst_metric: String,
    /// Propagation channel mode (`RATIO` or `ADD`).
    #[serde(default = "default_ratio_mode")]
    pub mode: InterventionMode,
    /// Optional explicit multiplier; `None` falls back to edge `weight`.
    #[serde(default)]
    pub coefficient: Option<f64>,
    /// Temporal lag in steps for `STEPPED` simulation (`0` = same-step).
    #[serde(default)]
    pub lag_steps: u32,
}

/// Target metric value clamp bounds (`§10.3.2`; default `min = Some(0.0)`, `max = None`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropagationClamp {
    /// Lower bound; `None` disables lower clamping.
    #[serde(default = "default_clamp_min")]
    pub min: Option<f64>,
    /// Upper bound; `None` disables upper clamping.
    #[serde(default)]
    pub max: Option<f64>,
}

impl Default for PropagationClamp {
    fn default() -> Self {
        Self {
            min: Some(0.0),
            max: None,
        }
    }
}

/// Complete causal propagation specification parsed from `xb_edge_types.propagation` (`§10.3.2`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropagationSpec {
    /// Explicit cross-metric rules; empty falls back to same-metric `RATIO` with link weight.
    #[serde(default)]
    pub rules: Vec<PropagationRule>,
    /// Output clamping bounds applied after converting delta to metric value.
    #[serde(default)]
    pub clamp: PropagationClamp,
}

/// Functional form applied to an upstream delta when traversing a compiled causal edge.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum PropagationCurve {
    /// Linear scaling `input * gain`.
    #[default]
    Linear,
    /// Activation deadband: returns `input * gain` when `|input| >= threshold`, else `0.0`.
    Threshold {
        /// Minimum absolute input magnitude required to propagate.
        threshold: f64,
    },
    /// Piecewise-constant step response `[(threshold, output)]` sorted ascending by `threshold`.
    StepFunction {
        /// Sorted `(threshold, output)` breakpoints; selects the last entry with `threshold <= input`.
        steps: Vec<(f64, f64)>,
    },
}

/// Optional saturation clamp `[min, max]` applied to a single edge's propagated delta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EdgeSaturation {
    /// Minimum delta emitted by this edge.
    pub min: Option<f64>,
    /// Maximum delta emitted by this edge.
    pub max: Option<f64>,
}

impl EdgeSaturation {
    /// Clamps a single edge's propagated delta to `[min, max]`.
    #[must_use]
    pub fn apply(self, delta: f64) -> f64 {
        let low = self.min.map_or(delta, |m| delta.max(m));
        self.max.map_or(low, |m| low.min(m))
    }
}

/// Effective node in an `AxSim` sandbox (`base` merged with `overlay`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimNode {
    /// Node UUID.
    pub id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business node key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Effective numeric metrics.
    pub metrics: BTreeMap<String, f64>,
    /// Effective static properties.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Optional WGS-84 point coordinate (`06-STEP28` §28.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoPoint>,
    /// Optional `GeoJSON` geometry (`Point`, `LineString`, or `Polygon`, `06-STEP28` §28.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_shape: Option<GeoShapeDto>,
    /// Production `row_version` captured at baseline clone (`0` for sandbox-created nodes).
    pub row_version: i64,
}

/// Effective directed link in an `AxSim` sandbox (`base` merged with `overlay`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimLink {
    /// Link UUID.
    pub id: Uuid,
    /// Relationship type name.
    pub link_type: String,
    /// Source node UUID.
    pub src_node_id: Uuid,
    /// Destination node UUID.
    pub dst_node_id: Uuid,
    /// Effective link weight.
    pub weight: f64,
}

/// Complete Copy-on-Write sandbox graph plus edge-type propagation specs for solving (`§10.2.2`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimGraph {
    /// Effective sandbox nodes.
    pub nodes: Vec<SimNode>,
    /// Effective sandbox links.
    pub links: Vec<SimLink>,
    /// `xb_edge_types.propagation` specifications keyed by `link_type` (or `(name, src, dst)`).
    #[serde(default)]
    pub edge_specs: BTreeMap<String, PropagationSpec>,
}

impl SimGraph {
    /// Builds a `Uuid -> usize` lookup map over [`Self::nodes`].
    #[must_use]
    pub fn node_index_map(&self) -> HashMap<Uuid, usize> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(idx, node)| (node.id, idx))
            .collect()
    }

    /// Compiles this [`SimGraph`] into a compact zero-indexed [`IndexedSimGraph`].
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if node count exceeds `u32::MAX`
    /// or non-finite numeric weights are encountered.
    pub fn compile(&self) -> Result<IndexedSimGraph, AppError> {
        IndexedSimGraph::from_sim_graph(self)
    }
}

/// Zero-indexed node metadata inside [`IndexedSimGraph`].
#[derive(Debug, Clone)]
pub struct IndexedSimNode {
    /// Compact zero-based index.
    pub idx: NodeIdx,
    /// Node UUID.
    pub id: Uuid,
    /// Node `type_name`.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Baseline metrics keyed by metric name.
    pub metrics: HashMap<String, f64>,
    /// Optional per-metric `[min, max]` clamping bounds parsed from schema/node properties.
    pub metric_bounds: HashMap<String, PropagationClamp>,
    /// Optional WGS-84 point coordinate (`06-STEP28` §28.3).
    pub geo: Option<GeoPoint>,
    /// Optional `GeoJSON` geometry (`06-STEP28` §28.3).
    pub geo_shape: Option<GeoShapeDto>,
}

/// Compiled directed propagation rule edge between zero-indexed nodes.
#[derive(Debug, Clone)]
pub struct CompiledEdge {
    /// Source node index.
    pub src_node: NodeIdx,
    /// Destination node index.
    pub dst_node: NodeIdx,
    /// Relationship type name.
    pub edge_type: String,
    /// Source metric key (`None` = same-metric default propagation).
    pub from_metric: Option<String>,
    /// Destination metric key (`None` = same-metric default propagation).
    pub to_metric: Option<String>,
    /// Propagation channel mode (`RATIO` or `ADD`).
    pub mode: InterventionMode,
    /// Effective linear gain / multiplier.
    pub gain: f64,
    /// Raw link weight.
    pub weight: f64,
    /// Temporal delay in discrete steps (`0` = same-step).
    pub delay_steps: u32,
    /// Functional propagation curve (`Linear`, `Threshold`, `StepFunction`).
    pub curve: PropagationCurve,
    /// Optional edge-level delta saturation bounds.
    pub saturation: EdgeSaturation,
    /// Target value clamping bounds from the edge spec.
    pub clamp: PropagationClamp,
}

/// Zero-indexed compact representation of [`SimGraph`] for fast traversal and numerical solving.
#[derive(Debug, Clone, Default)]
pub struct IndexedSimGraph {
    /// Compact dense node table indexed by `NodeIdx::index()`.
    pub nodes: Vec<IndexedSimNode>,
    /// Mapping from `Uuid` to zero-based index in `nodes`.
    pub node_index: HashMap<Uuid, usize>,
    /// Outgoing adjacency list storing indices into `edges`.
    pub out_edges: Vec<Vec<usize>>,
    /// Compiled propagation edges.
    pub edges: Vec<CompiledEdge>,
}

impl IndexedSimGraph {
    /// Compiles a [`SimGraph`] into a compact zero-indexed [`IndexedSimGraph`].
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] if node count exceeds `u32::MAX` or edge weights are non-finite.
    pub fn from_sim_graph(graph: &SimGraph) -> Result<Self, AppError> {
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        let mut node_index = HashMap::with_capacity(graph.nodes.len());

        for (i, node) in graph.nodes.iter().enumerate() {
            let idx = NodeIdx::from_usize(i)?;
            let metrics: HashMap<String, f64> =
                node.metrics.iter().map(|(k, &v)| (k.clone(), v)).collect();
            let metric_bounds = parse_metric_bounds(&node.properties);
            node_index.insert(node.id, i);
            nodes.push(IndexedSimNode {
                idx,
                id: node.id,
                type_name: node.type_name.clone(),
                node_key: node.node_key.clone(),
                title: node.title.clone(),
                metrics,
                metric_bounds,
                geo: node.geo,
                geo_shape: node.geo_shape.clone(),
            });
        }

        let mut out_edges = vec![Vec::new(); nodes.len()];
        let mut edges = Vec::with_capacity(graph.links.len());

        for link in &graph.links {
            let Some(&src_i) = node_index.get(&link.src_node_id) else {
                continue;
            };
            let Some(&dst_i) = node_index.get(&link.dst_node_id) else {
                continue;
            };
            if !link.weight.is_finite() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("non-finite weight on link {}", link.id),
                ));
            }
            let src_node = NodeIdx::from_usize(src_i)?;
            let dst_node = NodeIdx::from_usize(dst_i)?;
            let spec = graph.edge_specs.get(&link.link_type);
            compile_link_edges(
                link,
                src_node,
                dst_node,
                spec,
                &mut edges,
                &mut out_edges,
                src_i,
            )?;
        }

        Ok(Self {
            nodes,
            node_index,
            out_edges,
            edges,
        })
    }

    /// Configures per-metric `[min, max]` clamping bounds on an existing node.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `node_id` is not present.
    pub fn set_metric_bounds(
        &mut self,
        node_id: Uuid,
        metric: impl Into<String>,
        clamp: PropagationClamp,
    ) -> Result<(), AppError> {
        let Some(&idx) = self.node_index.get(&node_id) else {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("node {node_id} not found in indexed graph"),
            ));
        };
        let Some(node) = self.nodes.get_mut(idx) else {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("node {node_id} index out of bounds"),
            ));
        };
        node.metric_bounds.insert(metric.into(), clamp);
        Ok(())
    }

    /// Appends a compiled custom propagation edge (e.g., `Threshold`, `StepFunction`, or saturation bounds).
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if source/destination nodes are invalid
    /// or `gain` is non-finite.
    pub fn add_custom_edge(&mut self, edge: CompiledEdge) -> Result<(), AppError> {
        let src_i = edge.src_node.index();
        let dst_i = edge.dst_node.index();
        if src_i >= self.nodes.len() || dst_i >= self.nodes.len() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "custom edge references out-of-bounds NodeIdx",
            ));
        }
        if !edge.gain.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "custom edge gain must be finite",
            ));
        }
        let edge_idx = self.edges.len();
        self.edges.push(edge);
        if let Some(adj) = self.out_edges.get_mut(src_i) {
            adj.push(edge_idx);
        }
        Ok(())
    }

    /// Looks up a node's [`NodeIdx`] by UUID.
    #[must_use]
    pub fn node_idx(&self, id: Uuid) -> Option<NodeIdx> {
        self.node_index
            .get(&id)
            .and_then(|&idx| NodeIdx::from_usize(idx).ok())
    }

    /// Looks up an [`IndexedSimNode`] by UUID.
    #[must_use]
    pub fn node_by_id(&self, id: Uuid) -> Option<&IndexedSimNode> {
        self.node_index
            .get(&id)
            .and_then(|&idx| self.nodes.get(idx))
    }
}

/// Evaluates an edge's functional curve (`Linear`, `Threshold`, `StepFunction`) and applies saturation.
#[must_use]
pub fn eval_edge_delta(
    curve: &PropagationCurve,
    gain: f64,
    saturation: EdgeSaturation,
    input: f64,
    scale_multiplier: f64,
) -> f64 {
    let raw = match curve {
        PropagationCurve::Linear => input * gain * scale_multiplier,
        PropagationCurve::Threshold { threshold } => {
            if input.abs() >= *threshold {
                input * gain * scale_multiplier
            } else {
                0.0
            }
        }
        PropagationCurve::StepFunction { steps } => {
            let mut matched = 0.0;
            for &(thresh, out_val) in steps {
                if input >= thresh {
                    matched = out_val;
                } else {
                    break;
                }
            }
            matched * scale_multiplier
        }
    };
    saturation.apply(raw)
}

/// Clamps a simulated metric value against [`PropagationClamp`].
#[must_use]
pub fn apply_clamp(val: f64, clamp: &PropagationClamp) -> f64 {
    let low = clamp.min.map_or(val, |m| val.max(m));
    clamp.max.map_or(low, |m| low.min(m))
}

/// Intersects two [`PropagationClamp`] specifications (taking the tighter `[min, max]` interval).
#[must_use]
pub fn merge_clamps(a: &PropagationClamp, b: &PropagationClamp) -> PropagationClamp {
    let min = match (a.min, b.min) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let max = match (a.max, b.max) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    PropagationClamp { min, max }
}

/// Aggregates [`ImpactRow`] entries into per-`(type_name, metric)` [`SimKpiRow`] summaries (`§10.7.1`).
#[must_use]
pub fn aggregate_kpis(impacts: &[ImpactRow]) -> Vec<SimKpiRow> {
    let mut groups: BTreeMap<(String, String), (f64, f64, usize)> = BTreeMap::new();
    for row in impacts {
        let entry = groups
            .entry((row.type_name.clone(), row.metric.clone()))
            .or_insert((0.0, 0.0, 0));
        entry.0 += row.before_val.unwrap_or(0.0);
        entry.1 += row.after_val;
        entry.2 = entry.2.saturating_add(1);
    }

    groups
        .into_iter()
        .map(
            |((type_name, metric), (before_sum, after_sum, affected_nodes))| {
                let delta = after_sum - before_sum;
                let delta_ratio = if before_sum.abs() > 1e-12 {
                    Some(delta / before_sum)
                } else {
                    None
                };
                SimKpiRow {
                    type_name,
                    metric,
                    before_sum,
                    after_sum,
                    delta,
                    delta_ratio,
                    affected_nodes,
                }
            },
        )
        .collect()
}

fn compile_link_edges(
    link: &SimLink,
    src_node: NodeIdx,
    dst_node: NodeIdx,
    spec: Option<&PropagationSpec>,
    edges: &mut Vec<CompiledEdge>,
    out_edges: &mut [Vec<usize>],
    src_i: usize,
) -> Result<(), AppError> {
    let clamp = spec.map_or_default(|s| s.clamp.clone());
    let has_rules = spec.is_some_and(|s| !s.rules.is_empty());

    if has_rules && let Some(spec) = spec {
        for rule in &spec.rules {
            let gain = rule.coefficient.unwrap_or(link.weight);
            if !gain.is_finite() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("non-finite propagation coefficient on link {}", link.id),
                ));
            }
            let edge_idx = edges.len();
            edges.push(CompiledEdge {
                src_node,
                dst_node,
                edge_type: link.link_type.clone(),
                from_metric: Some(rule.src_metric.clone()),
                to_metric: Some(rule.dst_metric.clone()),
                mode: rule.mode,
                gain,
                weight: link.weight,
                delay_steps: rule.lag_steps,
                curve: PropagationCurve::Linear,
                saturation: EdgeSaturation::default(),
                clamp: clamp.clone(),
            });
            if let Some(adj) = out_edges.get_mut(src_i) {
                adj.push(edge_idx);
            }
        }
    } else {
        let edge_idx = edges.len();
        edges.push(CompiledEdge {
            src_node,
            dst_node,
            edge_type: link.link_type.clone(),
            from_metric: None,
            to_metric: None,
            mode: InterventionMode::Ratio,
            gain: link.weight,
            weight: link.weight,
            delay_steps: 0,
            curve: PropagationCurve::Linear,
            saturation: EdgeSaturation::default(),
            clamp,
        });
        if let Some(adj) = out_edges.get_mut(src_i) {
            adj.push(edge_idx);
        }
    }
    Ok(())
}

fn parse_metric_bounds(
    properties: &serde_json::Map<String, serde_json::Value>,
) -> HashMap<String, PropagationClamp> {
    let mut bounds = HashMap::new();
    let Some(raw) = properties.get("metric_bounds").and_then(|v| v.as_object()) else {
        return bounds;
    };
    for (metric, spec) in raw {
        if let Ok(clamp) = serde_json::from_value::<PropagationClamp>(spec.clone()) {
            bounds.insert(metric.clone(), clamp);
        }
    }
    bounds
}

const fn default_ratio_mode() -> InterventionMode {
    InterventionMode::Ratio
}

fn default_clamp_min() -> Option<f64> {
    Some(0.0_f64).filter(|v| v.is_finite())
}
