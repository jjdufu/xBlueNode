//! Metric dependency DAG, dirty-instance propagation plan, job payloads, explainability, and knowledge sync contracts (`06-STEP05-06` §06.2–§06.6).

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::expr::EvalWarningCode;
use crate::studio::{AggRuleSpec, MappingMode, SchemaDoc};
use crate::topology::BlueNode;

/// Metric dependency DAG construction, Kahn layered topological sort, and dirty-instance propagation planning.
pub mod plan;

pub use plan::{
    PlannedLayerTask, RuntimeLinkEdge, build_dependency_plan, compute_rule_layers,
    plan_dirty_propagation,
};

/// Payload for `JobKind::MapMaterialize` (`06-STEP05-06` §06.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapMaterializeJobPayload {
    /// Target `xb_mappings.id`.
    pub mapping_id: Uuid,
    /// Execution mode (`FULL` or `INCREMENTAL`).
    #[serde(default)]
    pub mode: MappingMode,
    /// Optional schema version that enqueued this job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i32>,
}

impl MapMaterializeJobPayload {
    /// Builds the deterministic idempotency key `<mapping_id>:<target_snapshot>` (`06-STEP05-06` §06.2).
    #[must_use]
    pub fn idempotency_key_for_snapshot(mapping_id: Uuid, target_snapshot: i64) -> String {
        format!("{mapping_id}:{target_snapshot}")
    }
}

/// Payload for `JobKind::GraphAggregate` (`06-STEP05-06` §06.3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphAggregateJobPayload {
    /// Recompute all rules across all types in `main`.
    #[serde(default)]
    pub full: bool,
    /// Recompute all nodes of a single `NodeTypeSpec.name` (and propagate upstream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_type: Option<String>,
    /// Dirty runtime `xb_nodes.id` list.
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Optional dirty metric names (when empty, all rules on/from `node_ids` are treated as dirty).
    #[serde(default)]
    pub metrics: Vec<String>,
    /// Optional schema version trigger tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i32>,
}

impl GraphAggregateJobPayload {
    /// Computes a canonical SHA-256 hex digest of normalized payload fields (`06-STEP05-06` §06.3.2).
    #[must_use]
    pub fn canonical_hash(&self) -> String {
        let sorted_nodes: BTreeSet<Uuid> = self.node_ids.iter().copied().collect();
        let sorted_metrics: BTreeSet<&str> = self.metrics.iter().map(String::as_str).collect();
        let canonical = serde_json::json!({
            "full": self.full,
            "full_type": self.full_type.as_deref().map(str::trim),
            "node_ids": sorted_nodes,
            "metrics": sorted_metrics,
            "schema_version": self.schema_version,
        });
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string().as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// Payload for `JobKind::KnowledgeSync` (`06-STEP05-06` §06.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSyncJobPayload {
    /// Single upserted `xb_nodes.id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Uuid>,
    /// Batch of upserted `xb_nodes.id`s.
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Deleted `xb_nodes.id` whose `xb_node:<node_id>` dossier should be removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_node_id: Option<Uuid>,
    /// Full sync of all runtime nodes belonging to newly enabled `sync_to_knowledge = true` types.
    #[serde(default)]
    pub type_names: Vec<String>,
    /// Optional schema version trigger tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i32>,
}

/// Vertex `(type_name, metric)` in the `main` ontology's metric dependency DAG `G` (`06-STEP05-06` §06.3.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MetricVertex {
    /// Owning `NodeTypeSpec.name`.
    pub type_name: String,
    /// Metric property name.
    pub metric: String,
}

impl MetricVertex {
    /// Creates a new [`MetricVertex`].
    #[must_use]
    pub fn new(type_name: impl Into<String>, metric: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            metric: metric.into(),
        }
    }
}

impl fmt::Display for MetricVertex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.type_name, self.metric)
    }
}

/// Dirty runtime `(node_id, type_name, metric)` seed passed into propagation planning (`06-STEP05-06` §06.3.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DirtyInstance {
    /// Runtime `xb_nodes.id`.
    pub node_id: Uuid,
    /// Runtime `xb_nodes.type_name`.
    pub type_name: String,
    /// Metric name.
    pub metric: String,
}

/// Rule computation descriptor attached to a target [`MetricVertex`] in topological order (`06-STEP05-06` §06.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricRuleStep {
    /// Intra-node derived formula `target_metric = expression`.
    Derived {
        /// Owning `NodeTypeSpec.name`.
        type_name: String,
        /// Target metric name.
        target_metric: String,
        /// Expression string.
        expression: String,
        /// Referenced source metrics on the same node type.
        source_metrics: Vec<String>,
    },
    /// Cross-link neighbor aggregation rule on `parent_type`.
    Aggregation {
        /// Owning parent `NodeTypeSpec.name`.
        parent_type: String,
        /// Aggregation rule specification.
        rule: AggRuleSpec,
    },
}

/// Topologically ordered metric dependency plan compiled from a published `SchemaDoc` (`06-STEP05-06` §06.3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricDependencyPlan {
    /// Topological vertex order `order(G)` from upstream inputs to downstream derived/aggregated metrics.
    pub order: Vec<MetricVertex>,
    /// Ordered rule steps aligned with `order(G)`.
    pub steps: Vec<MetricRuleStep>,
}

impl MetricDependencyPlan {
    /// Builds a [`MetricDependencyPlan`] from the published `main` [`SchemaDoc`].
    #[must_use]
    pub fn from_schema(doc: &SchemaDoc) -> Self {
        plan::build_dependency_plan(doc)
    }

    /// Groups `self.steps` into topological execution layers so steps within the same layer have no mutual dependencies.
    #[must_use]
    pub fn layers(&self) -> Vec<Vec<MetricRuleStep>> {
        plan::compute_rule_layers(self)
    }

    /// Looks up the rule step producing `(type_name, metric)`, if any.
    #[must_use]
    pub fn rule_for_target(&self, type_name: &str, metric: &str) -> Option<&MetricRuleStep> {
        self.steps.iter().find(|s| {
            let tv = s.target_vertex();
            tv.type_name == type_name && tv.metric == metric
        })
    }
}

/// Request body for `POST /api/xblue/aggregate/recompute` (`06-STEP05-06` §06.3.3 & §06.6).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecomputeAggregateRequest {
    /// Optional single `NodeTypeSpec.name` to recompute (when omitted, recomputes all types in `main`).
    #[serde(default)]
    pub type_name: Option<String>,
}

/// Response body for `POST /api/xblue/aggregate/recompute`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecomputeAggregateResponse {
    /// Enqueued `GRAPH_AGGREGATE` job UUID.
    pub job_id: Uuid,
    /// Target `type_name` if scoped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
}

/// Provenance classification returned by `GET /api/xblue/nodes/{id}/explain/{metric}` (`06-STEP05-06` §06.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricProducer {
    /// Computed by an `AggRuleSpec` (`"AGG"`).
    #[serde(rename = "AGG")]
    Agg,
    /// Computed by a `DerivedRuleSpec` (`"DERIVED"`).
    #[serde(rename = "DERIVED")]
    Derived,
    /// Synchronized from an `AxHub` dataset mapping (`"SYNC"`).
    #[serde(rename = "SYNC")]
    Sync,
    /// Written manually via node REST API (`"MANUAL"`).
    #[serde(rename = "MANUAL")]
    Manual,
    /// Emitted by an `AxFlow` / `xBlueView` action (`"ACTION"`).
    #[serde(rename = "ACTION")]
    Action,
    /// Computed by a graph topology algorithm (`"ALGORITHM"`).
    #[serde(rename = "ALGORITHM")]
    Algorithm,
}

/// Upstream input item in [`MetricExplainResponse`] (`06-STEP05-06` §06.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainInputItem {
    /// Input `xb_nodes.id` (same node for `DERIVED`, neighbor child node for `AGG`).
    pub node_id: Uuid,
    /// Input `xb_nodes.node_key`.
    pub node_key: String,
    /// Input `xb_nodes.type_name`.
    pub type_name: String,
    /// Input `xb_nodes.title`.
    pub title: String,
    /// Input metric (or `prop.<attr>`) name.
    pub metric: String,
    /// Current numeric value (`null` when missing).
    pub value: Option<f64>,
    /// Optional incident link weight when `producer == AGG` and `weight_by_link == true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

/// Explainability report returned by `GET /api/xblue/nodes/{id}/explain/{metric}` (`06-STEP05-06` §06.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricExplainResponse {
    /// Explained `xb_nodes.id`.
    pub node_id: Uuid,
    /// Explained metric name.
    pub metric: String,
    /// Current metric value on the node (`null` if absent).
    pub value: Option<f64>,
    /// Producer category (`AGG`, `DERIVED`, `SYNC`, `MANUAL`, `ACTION`, `ALGORITHM`).
    pub producer: MetricProducer,
    /// Serialized `AggRuleSpec` or `DerivedRuleSpec` (or mapping metadata for `SYNC`).
    pub rule: Option<serde_json::Value>,
    /// Upstream metric inputs feeding this calculation step.
    pub inputs: Vec<ExplainInputItem>,
    /// Latest computation timestamp recorded for this metric.
    pub computed_at: Option<DateTime<Utc>>,
    /// Any warnings encountered when evaluating a `DERIVED` expression (`DIV_BY_ZERO`, `DOMAIN_ERROR`).
    pub warnings: Vec<EvalWarningCode>,
}

/// Unlinked hypergraph entity candidate eligible for promotion into a business node (`06-STEP05-06` §06.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeNodeCandidate {
    /// Hypergraph `hg_entities.id`.
    pub entity_id: Uuid,
    /// Entity surface name.
    pub name: String,
    /// Hypergraph `hg_entities.entity_type`.
    pub entity_type: String,
    /// Matched `main` `NodeTypeSpec.name`.
    pub suggested_type_name: String,
    /// Mention count (`>= 3`).
    pub occur_count: i64,
    /// Entity extraction confidence.
    pub confidence: f64,
    /// Up to 5 document titles mentioning this entity.
    pub sample_docs: Vec<String>,
}

/// Response body for `GET /api/xblue/knowledge/node-candidates` (`06-STEP05-06` §06.5 & §06.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeNodeCandidatesResponse {
    /// Candidate entities ordered by `occur_count DESC, confidence DESC`.
    pub items: Vec<KnowledgeNodeCandidate>,
}

/// Request body for `POST /api/xblue/knowledge/node-candidates/{entity_id}/accept` (`06-STEP05-06` §06.5 & §06.6).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptNodeCandidateRequest {
    /// Target `main` `NodeTypeSpec.name`.
    pub type_name: String,
    /// Business key for the created/upserted `BlueNode`.
    pub node_key: String,
    /// Optional custom node title (defaults to entity `name`).
    #[serde(default)]
    pub title: Option<String>,
}

/// Response body for `POST /api/xblue/knowledge/node-candidates/{entity_id}/accept`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptNodeCandidateResponse {
    /// Created or linked `BlueNode`.
    pub node: BlueNode,
    /// Linked `hg_entities.id`.
    pub entity_id: Uuid,
    /// Confirmed link flag (`true`).
    pub confirmed: bool,
}
