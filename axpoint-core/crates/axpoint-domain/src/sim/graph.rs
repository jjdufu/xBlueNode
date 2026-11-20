//! `SimGraph` in-memory representation and `xb_edge_types.propagation` rule parsing (`09-STEP10` §10.3.2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::InterventionMode;

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

const fn default_ratio_mode() -> InterventionMode {
    InterventionMode::Ratio
}

fn default_clamp_min() -> Option<f64> {
    Some(0.0_f64).filter(|v| v.is_finite())
}
