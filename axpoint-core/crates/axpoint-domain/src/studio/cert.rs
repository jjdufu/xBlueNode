//! First-class `ActionSpec` contracts, 4-path edge dynamics rules, Dead Knowledge diagnostics,
//! and L0–L4 runtime readiness certification DTOs (`04-STEP26` §26.2–§26.4).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{
    ActionSideEffectTier, AggFn, DataType, DesignMaturity, PropagationPathKind, RuntimeCertLevel,
};
use crate::studio::{LinkDirection, ValidationIssue, deserialize_property_data_type};
use crate::topology::BlueNode;

const fn default_true() -> bool {
    true
}

fn default_action_perm() -> String {
    String::from("ACTION_EXECUTE")
}

/// Typed input parameter specification declared on an [`ActionSpec`] (`04-STEP26` §26.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionParamSpec {
    /// Parameter identifier (`^[a-zA-Z][a-zA-Z0-9_]{0,62}$`) referenced as `param.<name>` in expressions.
    pub name: String,
    /// Localized display label.
    #[serde(default)]
    pub display_name: String,
    /// Logical data type validated before action execution.
    #[serde(deserialize_with = "deserialize_property_data_type")]
    pub data_type: DataType,
    /// Whether callers must supply a non-null argument for this parameter.
    #[serde(default = "default_true")]
    pub required: bool,
    /// Optional default literal value used when omitted and `required == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
    /// Optional parameter description.
    #[serde(default)]
    pub description: String,
}

impl Default for ActionParamSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            display_name: String::new(),
            data_type: DataType::String,
            required: true,
            default_value: None,
            description: String::new(),
        }
    }
}

/// Atomic field/metric writeback template inside an [`ActionSpec`] (`04-STEP26` §26.2.1).
///
/// Supported `target` paths:
/// - `self.<field>` (or `self.prop.<field>` / `self.metric.<field>`) — mutates the target node itself (`SELF_ONLY`);
/// - `via(<LINK_TYPE>).<field>` or `via_in(<LINK_TYPE>).<field>` — mutates traversed 1-hop neighbor nodes (`RELATED_NODES`);
/// - `edge(<LINK_TYPE>).<field>` or `edge_in(<LINK_TYPE>).<field>` — mutates incident relationship links (`RELATED_NODES`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationStepSpec {
    /// Target write path (`self.<field>`, `via(<edge>).<field>`, or `edge(<edge>).<field>`).
    pub target: String,
    /// Deterministic `axpoint-domain::expr` formula evaluated against `self.*`, `param.*`, and target context.
    pub expression: String,
    /// Optional boolean filter expression restricting which traversed 1-hop neighbors/edges are mutated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// Declarative business action bound to a [`crate::studio::NodeTypeSpec`] (`04-STEP26` §26.2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Snake-case or camelCase identifier unique within the owning `NodeTypeSpec` (e.g., `"rebalance_stock"`).
    pub name: String,
    /// Localized display title (e.g., `"紧急库存调拨"`).
    #[serde(default)]
    pub display_name: String,
    /// Optional action description exposed in `OpenAPI`, `GraphQL` SDL, and MCP tool metadata.
    #[serde(default)]
    pub description: String,
    /// Typed input parameters validated via JSON Schema Draft 2020-12 before execution.
    #[serde(default)]
    pub parameters: Vec<ActionParamSpec>,
    /// Deterministic guard expression evaluated against `self.*` and `param.*` (must evaluate to `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precondition_expr: Option<String>,
    /// Declared side-effect blast radius enforced at compile and execution time.
    #[serde(default)]
    pub side_effect_tier: ActionSideEffectTier,
    /// Ordered atomic field/metric write operations.
    #[serde(default)]
    pub mutations: Vec<MutationStepSpec>,
    /// Optional outbound `AxFlow` slug or notification channel hook when `side_effect_tier == EXTERNAL_EFFECT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_flow_slug: Option<String>,
    /// Required RBAC/ABAC permission code (defaults to `"ACTION_EXECUTE"`, mapped to `FLOW_RUN`).
    #[serde(default = "default_action_perm")]
    pub required_permission: String,
    /// When `true`, invocation creates a `PENDING` ticket in `ax_approvals(source_kind = 'OBJECT_ACTION')` returning HTTP `202`.
    #[serde(default)]
    pub require_approval: bool,
    /// Optional idempotency key template (e.g., `"{node_key}:{param.request_id}"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key_template: Option<String>,
    /// Proposal maturity (`PROPOSED` | `CONFIRMED`).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for ActionSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            display_name: String::new(),
            description: String::new(),
            parameters: Vec::new(),
            precondition_expr: None,
            side_effect_tier: ActionSideEffectTier::SelfOnly,
            mutations: Vec::new(),
            trigger_flow_slug: None,
            required_permission: default_action_perm(),
            require_approval: false,
            idempotency_key_template: None,
            maturity: DesignMaturity::Confirmed,
        }
    }
}

fn default_v_to_e_path() -> PropagationPathKind {
    PropagationPathKind::VertexToEdge
}

/// Edge-level derived metric rule supporting `VERTEX_TO_EDGE` (`src.*`, `dst.*`) and `EDGE_TO_EDGE` (`self.*`, `prop.*`) (`04-STEP26` §26.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDerivedRuleSpec {
    /// Target metric property name declared on `EdgeTypeSpec.properties` (`kind == METRIC`).
    pub target_metric: String,
    /// Deterministic expression referencing `src.<metric>`, `src.prop.<attr>`, `dst.<metric>`, `dst.prop.<attr>`, `prop.<attr>`, `weight`, and edge metrics.
    pub expression: String,
    /// Propagation path classification (`VERTEX_TO_EDGE` or `EDGE_TO_EDGE`).
    #[serde(default = "default_v_to_e_path")]
    pub path_kind: PropagationPathKind,
    /// Whether this rule is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Design sign-off maturity (`PROPOSED` | `CONFIRMED`).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for EdgeDerivedRuleSpec {
    fn default() -> Self {
        Self {
            target_metric: String::new(),
            expression: String::new(),
            path_kind: PropagationPathKind::VertexToEdge,
            enabled: true,
            maturity: DesignMaturity::Confirmed,
        }
    }
}

/// Edge-to-edge cascade aggregation rule (`EDGE_TO_EDGE`) aggregating upstream/downstream incident links onto this edge (`04-STEP26` §26.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeAggRuleSpec {
    /// Target metric name on the owning `EdgeTypeSpec`.
    pub target_metric: String,
    /// Aggregation function (`SUM`, `AVG`, `MIN`, `MAX`, `COUNT`, etc.).
    pub op: AggFn,
    /// Connected `EdgeTypeSpec.name` sharing the pivot vertex (`IN` = upstream links entering `src_node_id`, `OUT` = downstream links leaving `dst_node_id`).
    pub via_link_type: String,
    /// Direction at the shared pivot vertex (`IN` = upstream edges into `src`, `OUT` = downstream edges out of `dst`).
    #[serde(default)]
    pub direction: LinkDirection,
    /// Source metric on `via_link_type`.
    pub source_metric: String,
    /// Optional boolean filter expression evaluated on candidate edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Design sign-off maturity (`PROPOSED` | `CONFIRMED`).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for EdgeAggRuleSpec {
    fn default() -> Self {
        Self {
            target_metric: String::new(),
            op: AggFn::Sum,
            via_link_type: String::new(),
            direction: LinkDirection::In,
            source_metric: String::new(),
            filter: None,
            maturity: DesignMaturity::Confirmed,
        }
    }
}

/// Isolated unconsumed derived/aggregated metric diagnostic (`DEAD_KNOWLEDGE`, `04-STEP26` §26.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadKnowledgeIssue {
    /// Owning `NodeTypeSpec.name` or `EdgeTypeSpec.name`.
    pub owner_type: String,
    /// Unconsumed derived or aggregated `target_metric`.
    pub metric: String,
    /// Producer rule category (`"DERIVED"` or `"AGG"`).
    pub producer_kind: String,
    /// Human-readable diagnostic explanation.
    pub message: String,
}

/// Request payload for `POST /api/xblue/subgraphs/{slug}/certify` (`04-STEP26` §26.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertifySubgraphRequest {
    /// Optional `xb_schema_drafts.id` to certify before publishing (defaults to published `SchemaDoc` on `slug`).
    #[serde(default)]
    pub draft_id: Option<Uuid>,
    /// Optional row sample cap per dataset mapping during `Trial Tick` (`1..=500`, default `50`).
    #[serde(default)]
    pub sample_limit: Option<u32>,
}

/// Persisted L0–L4 runtime readiness certification report (`xb_runtime_certifications`, `04-STEP26` §26.1 & §26.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCertReport {
    /// Certification record UUID.
    pub id: Uuid,
    /// Certified subgraph slug.
    pub subgraph_slug: String,
    /// Certified published schema version, if evaluated against head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i32>,
    /// Certified draft UUID, if evaluated against an unpublished draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_id: Option<Uuid>,
    /// Achieved L0–L4 readiness level.
    pub cert_level: RuntimeCertLevel,
    /// Composite readiness score (`0..=100`).
    pub readiness_score: i32,
    /// Count of isolated unconsumed derived/aggregated metrics (`DEAD_KNOWLEDGE`).
    pub dead_knowledge_cnt: i32,
    /// Count of undamped high-fanout propagation edges.
    pub high_fanout_cnt: i32,
    /// Whether the sandbox `Trial Tick` smoke execution converged with zero errors.
    pub trial_tick_passed: bool,
    /// Total derived, aggregation, and action precondition rules exercised during `Trial Tick`.
    pub triggered_rules_cnt: i32,
    /// Detailed unconsumed `DEAD_KNOWLEDGE` items.
    #[serde(default)]
    pub dead_knowledge: Vec<DeadKnowledgeIssue>,
    /// Static validation issues (`ERROR` and `WARN`, including `DEAD_KNOWLEDGE` warnings).
    #[serde(default)]
    pub issues: Vec<ValidationIssue>,
    /// Timestamp when this certification was recorded (UTC).
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/xblue/objects/{type_name}/{key}/actions/{action_name}` (`04-STEP26` §26.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeObjectActionRequest {
    /// Action input arguments validated against `ActionSpec.parameters`.
    #[serde(default = "default_empty_inputs")]
    pub inputs: serde_json::Value,
    /// Client-supplied idempotency key (`1..=128` chars) enforced via `ax_action_log(tenant_id, idempotency_key)`.
    pub idempotency_key: String,
    /// Optional optimistic concurrency guard on target `xb_nodes.row_version` (`409 VERSION_CONFLICT` on mismatch).
    #[serde(default)]
    pub expected_version: Option<i64>,
    /// Optional dry-run flag evaluating precondition and mutations without committing changes.
    #[serde(default)]
    pub dry_run: bool,
}

fn default_empty_inputs() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Response payload returned by `POST /api/xblue/objects/{type_name}/{key}/actions/{action_name}` and `GraphQL` `Mutation` (`04-STEP26` §26.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeObjectActionResponse {
    /// Execution status (`"APPLIED"`, `"WAITING_APPROVAL"`, or `"DRY_RUN"`).
    pub status: String,
    /// Updated target business node snapshot (masked per caller permissions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<BlueNode>,
    /// Count of mutated `xb_nodes` rows (target + traversed neighbors).
    pub mutated_nodes: usize,
    /// Count of mutated `xb_links` rows.
    pub mutated_links: usize,
    /// Count of downstream 4-path metric recomputations triggered.
    pub recomputed_metrics: usize,
    /// Recorded `ax_action_log.id` when committed or replayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_log_id: Option<i64>,
    /// Created `ax_approvals.id` when `ActionSpec.require_approval == true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<Uuid>,
    /// `true` when `idempotency_key` matched a previously committed `ax_action_log` entry.
    pub idempotency_replay: bool,
}

/// Published object action descriptor exposed to `AxAgent` MCP server (`04-STEP26` §26.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedObjectActionDescriptor {
    /// Owning `NodeTypeSpec.name`.
    pub type_name: String,
    /// Owning `NodeTypeSpec.display_name`.
    pub type_display_name: String,
    /// Confirmed `ActionSpec`.
    pub action: ActionSpec,
}
