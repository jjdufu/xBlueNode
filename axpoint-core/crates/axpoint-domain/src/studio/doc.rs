//! Core `xBlueStudio` schema document, validation, diff, subgraph merge, mapping, and domain object contracts (`06-STEP05-06` §05.2–§05.7).

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

pub use crate::enums::PublishedKind;
use crate::enums::{
    AggFn, Cardinality, CopilotActionMode, DataType, DesignMaturity, Environment, PiiCategory,
    SemanticLayerTier, SubgraphMergeStrategy,
};
use crate::graph::GeoPoint;

/// Complete `xBlueStudio` schema document exchanged across drafts, published versions,
/// subgraph merges, AI generation, and template packs (`06-STEP05-06` §05.2).
///
/// Unknown envelope fields (such as `version` or `subgraph_slug` echoed from `GET /api/xblue/schema`)
/// are intentionally ignored so callers can pipe `GET /api/xblue/schema` directly into
/// `/api/xblue/schema/validate` and `/api/xblue/drafts`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SchemaDoc {
    /// Node type specifications in this subgraph.
    #[serde(default)]
    pub node_types: Vec<NodeTypeSpec>,
    /// Directed relationship type specifications in this subgraph.
    #[serde(default)]
    pub edge_types: Vec<EdgeTypeSpec>,
    /// Dataset-to-graph materialization mappings in this subgraph.
    #[serde(default)]
    pub mappings: Vec<MappingSpec>,
}

/// Classification of a property on a `NodeTypeSpec` (`06-STEP05-06` §05.2.2).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum PropertyKind {
    /// Static attribute stored in `xb_nodes.properties`.
    #[default]
    #[serde(rename = "ATTRIBUTE")]
    Attribute,
    /// Numeric metric stored in `xb_nodes.metrics` and `xb_metric_series`.
    #[serde(rename = "METRIC")]
    Metric,
}

impl PropertyKind {
    /// Returns the canonical wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attribute => "ATTRIBUTE",
            Self::Metric => "METRIC",
        }
    }
}

/// Deserializes `PropertySpec.data_type` while accepting legacy lowercase aliases
/// (`"number" -> FLOAT64`, `"string" -> STRING`, `"boolean" -> BOOLEAN`) per `06-STEP05-06` §05.2.2.
///
/// # Errors
/// Returns a serde deserialization error if the string does not match a known [`DataType`] or legacy alias.
pub fn deserialize_property_data_type<'de, D>(deserializer: D) -> Result<DataType, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "number" | "float" | "double" => Ok(DataType::Float64),
        "string" | "text" => Ok(DataType::String),
        "boolean" | "bool" => Ok(DataType::Boolean),
        "integer" | "int" => Ok(DataType::Int64),
        _ => DataType::from_str(trimmed).map_err(serde::de::Error::custom),
    }
}

/// Property or metric specification attached to a node or edge type (`06-STEP05-06` §05.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertySpec {
    /// Machine identifier matching `^[a-z][a-z0-9_]{0,62}$`.
    pub name: String,
    /// Human-readable label (defaults to empty / `name` when omitted in legacy payloads).
    #[serde(default)]
    pub display_name: String,
    /// Optional property description.
    #[serde(default)]
    pub description: String,
    /// Optional measurement unit (e.g., `件`, `元`, `%`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Logical data type (with backward-compatible alias deserialization).
    #[serde(deserialize_with = "deserialize_property_data_type")]
    pub data_type: DataType,
    /// Storage kind (`ATTRIBUTE` vs `METRIC`).
    #[serde(default)]
    pub kind: PropertyKind,
    /// Whether missing values reject rows during dataset mapping.
    #[serde(default)]
    pub required: bool,
    /// PII sensitivity classification for masking across GraphQL, REST, and NL2SQL.
    #[serde(default = "default_pii_none")]
    pub pii_category: PiiCategory,
    /// Optional enumerated allowed values for `STRING` attributes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<String>>,
    /// Design sign-off maturity (`PROPOSED` vs `CONFIRMED`, defaults to `CONFIRMED`, `04-STEP18` §18.2.1).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for PropertySpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            display_name: String::new(),
            description: String::new(),
            unit: None,
            data_type: DataType::String,
            kind: PropertyKind::Attribute,
            required: false,
            pii_category: PiiCategory::None,
            allowed_values: None,
            maturity: DesignMaturity::Confirmed,
        }
    }
}

const fn default_pii_none() -> PiiCategory {
    PiiCategory::None
}

/// Traversal direction for `AggRuleSpec` and `MappingLinkSpec`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum LinkDirection {
    /// Current node is `src_node_id` (`this -> target`).
    #[default]
    #[serde(rename = "OUT")]
    Out,
    /// Current node is `dst_node_id` (`target -> this`).
    #[serde(rename = "IN")]
    In,
}

impl LinkDirection {
    /// Returns the canonical wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Out => "OUT",
            Self::In => "IN",
        }
    }
}

/// Dynamic neighbor metric aggregation rule (`06-STEP05-06` §05.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggRuleSpec {
    /// Target metric name on the owning `NodeTypeSpec`.
    pub target_metric: String,
    /// Aggregation function (`SUM`, `AVG`, `COUNT`, `COUNT_DISTINCT`, `MIN`, `MAX`, `MEDIAN`, `LAST`).
    pub op: AggFn,
    /// Traversed `EdgeTypeSpec.name`.
    #[serde(default)]
    pub via_link_type: String,
    /// Direction of `via_link_type` relative to the owning node (`OUT` = owner is `src`, `IN` = owner is `dst`).
    #[serde(default)]
    pub direction: LinkDirection,
    /// Neighbor `NodeTypeSpec.name` providing `source_metric`.
    #[serde(default)]
    pub source_type_name: String,
    /// Metric property on `source_type_name` to aggregate.
    #[serde(default)]
    pub source_metric: String,
    /// Optional Step 06 boolean filter expression evaluated against each neighbor node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Whether `SUM` / `AVG` weights neighbor values by `xb_links.weight`.
    #[serde(default)]
    pub weight_by_link: bool,
    /// Design sign-off maturity (`PROPOSED` vs `CONFIRMED`, defaults to `CONFIRMED`, `04-STEP18` §18.2.1).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for AggRuleSpec {
    fn default() -> Self {
        Self {
            target_metric: String::new(),
            op: AggFn::Sum,
            via_link_type: String::new(),
            direction: LinkDirection::Out,
            source_type_name: String::new(),
            source_metric: String::new(),
            filter: None,
            weight_by_link: false,
            maturity: DesignMaturity::Confirmed,
        }
    }
}

/// Intra-node derived formula metric rule (`06-STEP05-06` §05.2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedRuleSpec {
    /// Target metric name on the owning `NodeTypeSpec`.
    pub target_metric: String,
    /// Deterministic Step 06 expression referencing metrics and `prop.<attr>` on the same node.
    pub expression: String,
    /// Whether this rule is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Design sign-off maturity (`PROPOSED` vs `CONFIRMED`, defaults to `CONFIRMED`, `04-STEP18` §18.2.1).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for DerivedRuleSpec {
    fn default() -> Self {
        Self {
            target_metric: String::new(),
            expression: String::new(),
            enabled: true,
            maturity: DesignMaturity::Confirmed,
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_pos() -> f64 {
    120.0
}

fn default_icon() -> String {
    String::from("box")
}

fn default_color() -> String {
    String::from("#2563eb")
}

fn default_title_template() -> String {
    String::from("{node_key}")
}

/// Business node type specification in `SchemaDoc` (`06-STEP05-06` §05.2.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTypeSpec {
    /// Optional persisted `xb_node_types.id` when hydrated from PostgreSQL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    /// `PascalCase` machine name matching `^[A-Z][A-Za-z0-9]{0,62}$`.
    pub name: String,
    /// Human-readable display label (1..=64 chars).
    pub display_name: String,
    /// Optional domain description.
    #[serde(default)]
    pub description: String,
    /// Studio icon key (defaults to `"box"`).
    #[serde(default = "default_icon")]
    pub icon: String,
    /// Studio hex accent color `#RRGGBB` (defaults to `"#2563eb"`).
    #[serde(default = "default_color")]
    pub color: String,
    /// Node title template referencing `{node_key}` and `{prop.<name>}`.
    #[serde(default = "default_title_template")]
    pub title_template: String,
    /// Static attribute and numeric metric definitions.
    #[serde(default)]
    pub properties: Vec<PropertySpec>,
    /// Dynamic cross-link aggregation rules.
    #[serde(default)]
    pub agg_rules: Vec<AggRuleSpec>,
    /// Intra-node derived metric rules.
    #[serde(default)]
    pub derived_rules: Vec<DerivedRuleSpec>,
    /// Whether runtime nodes of this type synchronize Markdown dossiers into the hypergraph (Step 06 §06.5).
    #[serde(default)]
    pub sync_to_knowledge: bool,
    /// Studio canvas X coordinate.
    #[serde(default = "default_pos")]
    pub pos_x: f64,
    /// Studio canvas Y coordinate.
    #[serde(default = "default_pos")]
    pub pos_y: f64,
    /// Design sign-off maturity (`PROPOSED` vs `CONFIRMED`, defaults to `CONFIRMED`, `04-STEP18` §18.2.1).
    #[serde(default)]
    pub maturity: DesignMaturity,
    /// Optional semantic architecture tier (`STATIC_FACT` | `EVENT_DYNAMIC` | `CUBE_ROLLUP`, `04-STEP18` §18.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_layer_tier: Option<SemanticLayerTier>,
    /// Optional business domain group label (e.g. `"采购域"`, `"履约域"`, `04-STEP18` §18.2.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_group: Option<String>,
}

/// Compact `Np·Nm·Nr` element counts displayed on `NodeTypeCard` and `NodeLineageGraph` (`04-STEP18` §18.2.1, §18.4.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeBadgeCounts {
    /// Total property count (`Np = properties.len()`).
    pub property_count: u32,
    /// Metric property count (`Nm`).
    pub metric_count: u32,
    /// Total compute rule count (`Nr = agg_rules.len() + derived_rules.len()`).
    pub rule_count: u32,
}

impl Default for NodeTypeSpec {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            display_name: String::new(),
            description: String::new(),
            icon: default_icon(),
            color: default_color(),
            title_template: default_title_template(),
            properties: Vec::new(),
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: default_pos(),
            pos_y: default_pos(),
            maturity: DesignMaturity::Confirmed,
            semantic_layer_tier: None,
            domain_group: None,
        }
    }
}

impl NodeTypeSpec {
    /// Computes compact `Np·Nm·Nr` badge counters (`04-STEP18` §18.2.1).
    #[must_use]
    pub fn badge_counts(&self) -> NodeBadgeCounts {
        let property_count = u32::try_from(self.properties.len()).unwrap_or(u32::MAX);
        let metric_count = u32::try_from(
            self.properties
                .iter()
                .filter(|p| p.kind == PropertyKind::Metric)
                .count(),
        )
        .unwrap_or(u32::MAX);
        let rule_count = u32::try_from(
            self.agg_rules
                .len()
                .saturating_add(self.derived_rules.len()),
        )
        .unwrap_or(u32::MAX);
        NodeBadgeCounts {
            property_count,
            metric_count,
            rule_count,
        }
    }

    /// Returns `true` if the node type itself or any child property/rule is `PROPOSED`.
    #[must_use]
    pub fn has_proposed_elements(&self) -> bool {
        self.maturity == DesignMaturity::Proposed
            || self
                .properties
                .iter()
                .any(|p| p.maturity == DesignMaturity::Proposed)
            || self
                .agg_rules
                .iter()
                .any(|r| r.maturity == DesignMaturity::Proposed)
            || self
                .derived_rules
                .iter()
                .any(|r| r.maturity == DesignMaturity::Proposed)
    }
}

const fn default_impact_weight() -> f64 {
    0.6
}

const fn default_cardinality() -> Cardinality {
    Cardinality::ManyToMany
}

fn default_empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Directed relationship type specification in `SchemaDoc` (`06-STEP05-06` §05.1–§05.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeTypeSpec {
    /// Optional persisted `xb_edge_types.id` when hydrated from PostgreSQL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    /// Relationship type identifier (e.g., `SUPPLIES`, `FULFILLS`).
    pub name: String,
    /// Source `NodeTypeSpec.name`.
    pub src_type_name: String,
    /// Destination `NodeTypeSpec.name`.
    pub dst_type_name: String,
    /// Default causal propagation weight in `(0.0, 1.0]`.
    #[serde(default = "default_impact_weight")]
    pub impact_weight: f64,
    /// Expected relationship cardinality.
    #[serde(default = "default_cardinality")]
    pub cardinality: Cardinality,
    /// Optional relationship description.
    #[serde(default)]
    pub description: String,
    /// Optional edge property definitions.
    #[serde(default)]
    pub properties: Vec<PropertySpec>,
    /// Causal propagation configuration used by `AxSim` (Step 10).
    #[serde(default = "default_empty_object")]
    pub propagation: serde_json::Value,
    /// Design sign-off maturity (`PROPOSED` vs `CONFIRMED`, defaults to `CONFIRMED`, `04-STEP18` §18.2.1).
    #[serde(default)]
    pub maturity: DesignMaturity,
}

impl Default for EdgeTypeSpec {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            src_type_name: String::new(),
            dst_type_name: String::new(),
            impact_weight: default_impact_weight(),
            cardinality: default_cardinality(),
            description: String::new(),
            properties: Vec::new(),
            propagation: default_empty_object(),
            maturity: DesignMaturity::Confirmed,
        }
    }
}

impl SchemaDoc {
    /// Counts all `PROPOSED` elements (`NodeTypeSpec`, `PropertySpec`, `EdgeTypeSpec`, `AggRuleSpec`, `DerivedRuleSpec`).
    #[must_use]
    pub fn unconfirmed_proposal_count(&self) -> u32 {
        let mut count = 0_u32;
        for nt in &self.node_types {
            if nt.maturity == DesignMaturity::Proposed {
                count = count.saturating_add(1);
            }
            for p in &nt.properties {
                if p.maturity == DesignMaturity::Proposed {
                    count = count.saturating_add(1);
                }
            }
            for r in &nt.agg_rules {
                if r.maturity == DesignMaturity::Proposed {
                    count = count.saturating_add(1);
                }
            }
            for d in &nt.derived_rules {
                if d.maturity == DesignMaturity::Proposed {
                    count = count.saturating_add(1);
                }
            }
        }
        for et in &self.edge_types {
            if et.maturity == DesignMaturity::Proposed {
                count = count.saturating_add(1);
            }
            for p in &et.properties {
                if p.maturity == DesignMaturity::Proposed {
                    count = count.saturating_add(1);
                }
            }
        }
        count
    }

    /// Returns `true` when at least one element in this `SchemaDoc` is still in `PROPOSED` state.
    #[must_use]
    pub fn has_unconfirmed_proposals(&self) -> bool {
        self.unconfirmed_proposal_count() > 0
    }

    /// Transitions matching `PROPOSED` elements (and their child items) to `CONFIRMED` (`04-STEP18` §18.3.1).
    pub fn confirm_proposals(&mut self, req: &ConfirmProposalsRequest) {
        let confirm_all = req.scope == ConfirmProposalsScope::All;
        for nt in &mut self.node_types {
            if confirm_all || req.node_types.iter().any(|n| n == &nt.name) {
                nt.maturity = DesignMaturity::Confirmed;
                for p in &mut nt.properties {
                    p.maturity = DesignMaturity::Confirmed;
                }
                for r in &mut nt.agg_rules {
                    r.maturity = DesignMaturity::Confirmed;
                }
                for d in &mut nt.derived_rules {
                    d.maturity = DesignMaturity::Confirmed;
                }
            }
        }
        for et in &mut self.edge_types {
            if confirm_all || req.edge_types.iter().any(|e| e == &et.name) {
                et.maturity = DesignMaturity::Confirmed;
                for p in &mut et.properties {
                    p.maturity = DesignMaturity::Confirmed;
                }
            }
        }
    }
}

/// Optional WGS-84 coordinate column binding in `MappingSpec` (`06-STEP05-06` §05.2.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingGeoSpec {
    /// Dataset column supplying WGS-84 longitude.
    #[serde(default)]
    pub lng_column: Option<String>,
    /// Dataset column supplying WGS-84 latitude.
    #[serde(default)]
    pub lat_column: Option<String>,
}

/// Link generation rule inside `MappingSpec` (`06-STEP05-06` §05.2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingLinkSpec {
    /// Target `EdgeTypeSpec.name`.
    pub link_type: String,
    /// `OUT` = mapped node is `src`; `IN` = mapped node is `dst`.
    #[serde(default)]
    pub direction: LinkDirection,
    /// Target `NodeTypeSpec.name`.
    pub target_type: String,
    /// Dataset foreign-key column holding the target node's `node_key`.
    pub target_key_column: String,
    /// Optional dataset column overriding `xb_links.weight`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_column: Option<String>,
    /// Whether to auto-create minimal target nodes when missing.
    #[serde(default)]
    pub create_missing_target: bool,
}

/// Deletion policy when source rows disappear from the dataset (`06-STEP05-06` §05.2.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnSourceDelete {
    /// Retain previously materialized `BlueNode` rows.
    #[default]
    #[serde(rename = "KEEP")]
    Keep,
    /// Delete `BlueNode` rows whose source row was deleted.
    #[serde(rename = "DELETE")]
    Delete,
}

/// Execution mode for `MAP_MATERIALIZE` (`06-STEP05-06` §05.2.5 & §06.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MappingMode {
    /// Re-scan the full dataset snapshot.
    #[serde(rename = "FULL")]
    Full,
    /// Read incremental changes via `ducklake_table_changes` (falling back to `FULL` on initial run).
    #[default]
    #[serde(rename = "INCREMENTAL")]
    Incremental,
}

impl MappingMode {
    /// Returns the canonical wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "FULL",
            Self::Incremental => "INCREMENTAL",
        }
    }
}

impl FromStr for MappingMode {
    type Err = crate::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "FULL" => Ok(Self::Full),
            "INCREMENTAL" => Ok(Self::Incremental),
            other => Err(crate::AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown MappingMode: {other}"),
            )),
        }
    }
}

fn default_mapping_title() -> String {
    String::from("{id}")
}

/// Dataset-to-graph mapping specification (`06-STEP05-06` §05.2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingSpec {
    /// Unique mapping name within the subgraph.
    pub name: String,
    /// Source `AxHub` dataset `name` (or UUID string).
    pub dataset: String,
    /// Target `NodeTypeSpec.name`.
    pub node_type: String,
    /// Dataset column mapped to `xb_nodes.node_key`.
    pub key_column: String,
    /// Node title template referencing `{column}` placeholders.
    #[serde(default = "default_mapping_title")]
    pub title: String,
    /// Optional row filter expression evaluated before mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Mapping from `NodeTypeSpec` `ATTRIBUTE` property name to dataset column name.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    /// Mapping from `NodeTypeSpec` `METRIC` property name to dataset column name.
    #[serde(default)]
    pub metrics: BTreeMap<String, String>,
    /// Optional timestamp column written to `xb_metric_series`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_time_column: Option<String>,
    /// Optional geographic coordinate column binding.
    #[serde(default)]
    pub geo: MappingGeoSpec,
    /// Relationship materialization bindings.
    #[serde(default)]
    pub links: Vec<MappingLinkSpec>,
    /// Policy when source dataset rows are deleted.
    #[serde(default)]
    pub on_source_delete: OnSourceDelete,
    /// Default materialization mode (`FULL` or `INCREMENTAL`).
    #[serde(default)]
    pub mode: MappingMode,
}

/// Severity of a schema validation issue (`06-STEP05-06` §05.3, `04-STEP18` §18.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IssueSeverity {
    /// Blocks schema publication.
    #[serde(rename = "ERROR")]
    Error,
    /// Advisory warning (or `BREAKING_CHANGE` requiring `force = true` on publish).
    #[serde(rename = "WARN")]
    Warn,
    /// Informational completeness hint (`04-STEP18` §18.2.2).
    #[serde(rename = "INFO")]
    Info,
}

/// Single schema validation diagnostic (`06-STEP05-06` §05.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// Diagnostic severity (`ERROR`, `WARN`, or `INFO`).
    pub severity: IssueSeverity,
    /// JSON-style path to the offending element, e.g., `node_types[0].derived_rules[0].expression`.
    pub path: String,
    /// Stable diagnostic code (`NAME_INVALID`, `CYCLE_DETECTED`, `BREAKING_CHANGE`, etc.).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

/// Response body for `POST /api/xblue/schema/validate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateSchemaResponse {
    /// `true` when `issues` contains zero `ERROR` items.
    pub valid: bool,
    /// Collected validation diagnostics.
    pub issues: Vec<ValidationIssue>,
    /// Count of positive completeness checks passed (`04-STEP18` §18.2.2).
    #[serde(default)]
    pub pass_count: u32,
}

/// Dataset metadata snapshot supplied to `studio::validate` for rule #8 (`MAPPING_INVALID`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetValidationMeta {
    /// Dataset UUID.
    pub id: Uuid,
    /// Dataset machine name.
    pub name: String,
    /// Dataset status (`READY`, `PENDING`, `FAILED`).
    pub status: String,
    /// Current row count.
    pub row_count: i64,
    /// Current `ds_datasets.schema_version`.
    pub schema_version: i32,
    /// Column name -> logical `DataType`.
    pub columns: BTreeMap<String, DataType>,
    /// Columns verified to have `null_rate == 0.0` (100% non-null).
    pub non_null_columns: BTreeSet<String>,
}

/// External tenant state supplied to pure `studio::validate` (`06-STEP05-06` §05.3).
#[derive(Debug, Clone, Default)]
pub struct ValidationContext {
    /// Target subgraph slug (e.g., `"main"` or `"logistics"`).
    pub subgraph_slug: String,
    /// Node type names currently published in enterprise `main` subgraph (rule #2 cross-subgraph target lookup).
    pub main_node_types: BTreeMap<String, NodeTypeSpec>,
    /// Currently published `SchemaDoc` of `subgraph_slug` for rule #10 (`BREAKING_CHANGE`).
    pub current_doc: Option<SchemaDoc>,
    /// Runtime `xb_nodes` counts per `type_name` (rule #10).
    pub existing_node_counts: BTreeMap<String, i64>,
    /// Runtime `xb_links` counts per `link_type` (rule #10).
    pub existing_link_counts: BTreeMap<String, i64>,
    /// Datasets keyed by both `dataset.name` and `dataset.id.to_string()` (rule #8).
    pub datasets: BTreeMap<String, DatasetValidationMeta>,
    /// Edge type names in runtime `xb_links` where a single destination has multiple sources (`ONE_TO_MANY` rule #9)
    /// or `ONE_TO_ONE` cardinality is violated.
    pub cardinality_violations: BTreeSet<String>,
}

/// Semantic role tag assigned to a profiled dataset column (`06-STEP05-06` §05.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ColumnRoleTag {
    /// Non-null unique identifier (`null_rate == 0.0 && distinct_count == row_count`).
    #[serde(rename = "PK_CANDIDATE")]
    PkCandidate,
    /// Numeric value suitable as a `METRIC` property.
    #[serde(rename = "NUMERIC_METRIC")]
    NumericMetric,
    /// `DATE` or `TIMESTAMP` column suitable as `metric_time_column`.
    #[serde(rename = "TEMPORAL")]
    Temporal,
    /// Low-cardinality (`distinct_count <= 50`) categorical attribute.
    #[serde(rename = "CATEGORICAL")]
    Categorical,
}

/// Deterministic column profile statistics produced by `axpoint-analytics::profiling` (`06-STEP05-06` §05.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnProfileStats {
    /// Column name.
    pub name: String,
    /// Logical column data type.
    pub data_type: DataType,
    /// Fraction of null values in `[0.0, 1.0]`.
    pub null_rate: f64,
    /// Distinct non-null count in sampled rows.
    pub distinct_count: u64,
    /// Inferred semantic role tags (`PK_CANDIDATE`, `NUMERIC_METRIC`, `TEMPORAL`, `CATEGORICAL`).
    #[serde(default)]
    pub roles: BTreeSet<ColumnRoleTag>,
    /// Up to 8 representative non-null sample values.
    #[serde(default)]
    pub sample_values: Vec<String>,
}

impl ColumnProfileStats {
    /// Returns `true` when this column is tagged `PK_CANDIDATE`.
    #[must_use]
    pub fn is_pk_candidate(&self) -> bool {
        self.roles.contains(&ColumnRoleTag::PkCandidate)
    }

    /// Returns `true` when this column is tagged `NUMERIC_METRIC`.
    #[must_use]
    pub fn is_numeric_metric(&self) -> bool {
        self.roles.contains(&ColumnRoleTag::NumericMetric)
    }

    /// Returns `true` when this column is tagged `TEMPORAL`.
    #[must_use]
    pub fn is_temporal(&self) -> bool {
        self.roles.contains(&ColumnRoleTag::Temporal)
    }

    /// Returns `true` when this column is tagged `CATEGORICAL`.
    #[must_use]
    pub fn is_categorical(&self) -> bool {
        self.roles.contains(&ColumnRoleTag::Categorical)
    }
}

/// Deterministic profile summary of a single `AxHub` dataset (`06-STEP05-06` §05.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetProfileSummary {
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Dataset machine name.
    pub dataset_name: String,
    /// Dataset display name.
    pub display_name: String,
    /// Sampled row count (up to 100,000 rows).
    pub row_count: u64,
    /// Per-column statistics.
    pub columns: Vec<ColumnProfileStats>,
    /// Ranked primary-key candidate column names (`id` / `*_id` / `*_code` / `*_no` prioritized).
    pub pk_candidates: Vec<String>,
}

/// Foreign-key relationship candidate inferred across two datasets (`06-STEP05-06` §05.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FkCandidate {
    /// Referencing dataset `name`.
    pub src_dataset: String,
    /// Referencing column on `src_dataset`.
    pub src_column: String,
    /// Referenced dataset `name`.
    pub dst_dataset: String,
    /// Referenced primary-key column on `dst_dataset`.
    pub dst_pk_column: String,
    /// Overlap ratio `|A.col ∩ B.pk| / |A.col distinct|` (`>= 0.9`).
    pub overlap_ratio: f64,
    /// Inferred relationship cardinality (`ONE_TO_ONE` if `distinct / rows >= 0.99`, else `MANY_TO_ONE`).
    pub cardinality: Cardinality,
}

/// Complete deterministic pre-analysis bundle fed into AI schema draft generation (`06-STEP05-06` §05.5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfilingBundle {
    /// Dataset profile summaries.
    pub datasets: Vec<DatasetProfileSummary>,
    /// Cross-dataset foreign-key candidates.
    pub fk_candidates: Vec<FkCandidate>,
}

/// Hypergraph document entity grouping fed into AI schema draft generation (`06-STEP05-06` §05.5 step 2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocEntityGroup {
    /// Hypergraph `entity_type`.
    pub entity_type: String,
    /// Representative entity names in the selected documents.
    pub entity_names: Vec<String>,
    /// Observed `(src_entity_type, dst_entity_type, fact_sample)` triples.
    pub relations: Vec<(String, String, String)>,
}

/// Field-level modification entry inside [`SchemaDiff`] (`06-STEP05-06` §05.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiff {
    /// Relative field path (e.g., `display_name`, `properties[inventory].data_type`, `derived_rules[fill_rate]`).
    pub path: String,
    /// Value in base version (`null` when newly added).
    pub before: serde_json::Value,
    /// Value in target version (`null` when removed).
    pub after: serde_json::Value,
}

/// Field-level diff summary for one named schema element (`06-STEP05-06` §05.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedItemDiff {
    /// Element key (`NodeTypeSpec.name`, `EdgeTypeSpec` key, or `MappingSpec.name`).
    pub name: String,
    /// Changed fields within this element.
    pub fields: Vec<FieldDiff>,
}

/// Added / removed / changed summary for one section of [`SchemaDiff`] (`06-STEP05-06` §05.4.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionDiff {
    /// Newly added item names.
    #[serde(default)]
    pub added: Vec<String>,
    /// Removed item names.
    #[serde(default)]
    pub removed: Vec<String>,
    /// Existing items with field-level modifications.
    #[serde(default)]
    pub changed: Vec<ChangedItemDiff>,
}

impl SectionDiff {
    /// Returns `true` when there are no added, removed, or changed items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Structural diff between two [`SchemaDoc`] snapshots (`06-STEP05-06` §05.4.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDiff {
    /// Node type changes.
    #[serde(default)]
    pub node_types: SectionDiff,
    /// Edge type changes.
    #[serde(default)]
    pub edge_types: SectionDiff,
    /// Mapping changes.
    #[serde(default)]
    pub mappings: SectionDiff,
}

impl SchemaDiff {
    /// Returns `true` when all three sections are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.node_types.is_empty() && self.edge_types.is_empty() && self.mappings.is_empty()
    }
}

/// Subgraph metadata record returned by `/api/xblue/subgraphs` (`06-STEP05-06` §05.1 & `03-STEP17` §17.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgraphInfo {
    /// Subgraph UUID.
    pub id: Uuid,
    /// Unique slug within the tenant (`"main"` for enterprise main graph).
    pub slug: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Optional subgraph description.
    pub description: String,
    /// Owning department identifier.
    pub owner_dept: String,
    /// Whether this is the authoritative enterprise `main` graph.
    pub is_main: bool,
    /// Whether this subgraph is an isolated dev sandbox (`03-STEP17`).
    #[serde(default)]
    pub is_dev_sandbox: bool,
    /// Optional source template pack identifier when cloned from a template (`03-STEP17`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_template_id: Option<String>,
    /// Timestamp of the most recent runtime node reset (`03-STEP17`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reset_at: Option<DateTime<Utc>>,
    /// Current published head version (`0` before first publish).
    pub current_version: i32,
    /// Creator user UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

fn default_owner_dept() -> String {
    String::from("default")
}

/// Request body for `POST /api/xblue/subgraphs` (`06-STEP05-06` §05.4.5 & `03-STEP17` §17.4).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSubgraphRequest {
    /// Subgraph slug matching `^[a-z][a-z0-9_-]{0,62}$` (`"main"` reserved).
    pub slug: String,
    /// Display name.
    pub display_name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Owning department ID.
    #[serde(default = "default_owner_dept")]
    pub owner_dept: String,
    /// Whether to copy initial node types from `main`.
    #[serde(default)]
    pub seed_from_main: bool,
    /// Optional subset of `main` node type names to copy when `seed_from_main = true`.
    #[serde(default)]
    pub seed_node_types: Option<Vec<String>>,
    /// Whether to initialize this subgraph as an isolated dev sandbox (`03-STEP17`).
    #[serde(default)]
    pub is_dev_sandbox: bool,
    /// Optional source template identifier recorded on dev sandboxes.
    #[serde(default)]
    pub source_template_id: Option<String>,
}

/// Request body for `PATCH /api/xblue/subgraphs/{slug}` (`A4` §4).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSubgraphRequest {
    /// Updated display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated owning department.
    #[serde(default)]
    pub owner_dept: Option<String>,
}

/// Optional `?subgraph=` query parameter shared across `/api/xblue/*` modeling routes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubgraphQueryParam {
    /// Target subgraph slug (defaults to `"main"`).
    #[serde(default)]
    pub subgraph: Option<String>,
}

impl SubgraphQueryParam {
    /// Returns the trimmed subgraph slug or `"main"` when omitted/empty.
    #[must_use]
    pub fn slug_or_main(&self) -> &str {
        match self.subgraph.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => s,
            _ => "main",
        }
    }
}

/// Response body for `GET /api/xblue/schema` (`06-STEP05-06` §05.4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedSchemaResponse {
    /// Subgraph slug (`"main"` by default).
    pub subgraph_slug: String,
    /// Current published version (`0` if never published).
    pub version: i32,
    /// Flattened `node_types`, `edge_types`, and `mappings`.
    #[serde(flatten)]
    pub doc: SchemaDoc,
}

/// Request body for direct `POST /api/xblue/schema?subgraph=` (`06-STEP05-06` §05.4.3).
#[derive(Debug, Clone, Deserialize)]
pub struct SaveSchemaDirectRequest {
    /// Schema document to validate and immediately publish.
    #[serde(flatten)]
    pub doc: SchemaDoc,
    /// Optional expected base version for optimistic concurrency.
    #[serde(default)]
    pub base_version: Option<i32>,
    /// Optional change summary (defaults to `"direct schema save"`).
    #[serde(default)]
    pub change_summary: Option<String>,
    /// Force publication across `BREAKING_CHANGE` warnings.
    #[serde(default)]
    pub force: bool,
    /// Cascade delete runtime nodes of removed types when `force = true`.
    #[serde(default)]
    pub cascade: bool,
}

/// Summary item returned by `GET /api/xblue/schema/versions` (`06-STEP05-06` §05.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersionSummary {
    /// Version row UUID.
    pub id: Uuid,
    /// Subgraph slug.
    pub subgraph_slug: String,
    /// Monotonic version number (`>= 1`).
    pub version: i32,
    /// Summary message provided on publish.
    pub change_summary: String,
    /// Publisher user UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_by: Option<Uuid>,
    /// Publication timestamp.
    pub published_at: DateTime<Utc>,
}

/// Full version snapshot returned by `GET /api/xblue/schema/versions/{v}` (`06-STEP05-06` §05.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaVersionDetail {
    /// Version row UUID.
    pub id: Uuid,
    /// Subgraph slug.
    pub subgraph_slug: String,
    /// Monotonic version number (`>= 1`).
    pub version: i32,
    /// Complete published `SchemaDoc` snapshot.
    pub schema_doc: SchemaDoc,
    /// Structural diff relative to `version - 1`.
    pub diff: SchemaDiff,
    /// Summary message provided on publish.
    pub change_summary: String,
    /// Publisher user UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_by: Option<Uuid>,
    /// Publication timestamp.
    pub published_at: DateTime<Utc>,
}

/// Origin category of a `SchemaDraft` (`06-STEP05-06` §05.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DraftSource {
    /// Created interactively in Studio.
    #[default]
    #[serde(rename = "MANUAL")]
    Manual,
    /// Generated by `POST /api/xblue/drafts/ai`.
    #[serde(rename = "AI")]
    Ai,
    /// Created by `POST /api/xblue/schema/versions/{v}/restore`.
    #[serde(rename = "RESTORE")]
    Restore,
    /// Installed from an industry template pack (Step 14).
    #[serde(rename = "TEMPLATE")]
    Template,
    /// Generated by merging a department subgraph into `main`.
    #[serde(rename = "MERGE")]
    Merge,
}

impl DraftSource {
    /// Returns the canonical database/wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "MANUAL",
            Self::Ai => "AI",
            Self::Restore => "RESTORE",
            Self::Template => "TEMPLATE",
            Self::Merge => "MERGE",
        }
    }
}

impl FromStr for DraftSource {
    type Err = crate::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "MANUAL" => Ok(Self::Manual),
            "AI" => Ok(Self::Ai),
            "RESTORE" => Ok(Self::Restore),
            "TEMPLATE" => Ok(Self::Template),
            "MERGE" => Ok(Self::Merge),
            other => Err(crate::AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown DraftSource: {other}"),
            )),
        }
    }
}

/// Lifecycle state of a `SchemaDraft` (`06-STEP05-06` §05.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DraftStatus {
    /// Editable draft awaiting publication.
    #[default]
    #[serde(rename = "DRAFT")]
    Draft,
    /// Published as a `SchemaVersion`.
    #[serde(rename = "PUBLISHED")]
    Published,
    /// Explicitly discarded.
    #[serde(rename = "DISCARDED")]
    Discarded,
}

impl DraftStatus {
    /// Returns the canonical database/wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Published => "PUBLISHED",
            Self::Discarded => "DISCARDED",
        }
    }
}

impl FromStr for DraftStatus {
    type Err = crate::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "DRAFT" => Ok(Self::Draft),
            "PUBLISHED" => Ok(Self::Published),
            "DISCARDED" => Ok(Self::Discarded),
            other => Err(crate::AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown DraftStatus: {other}"),
            )),
        }
    }
}

/// Persisted `xb_schema_drafts` record (`06-STEP05-06` §05.1 & §05.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaDraft {
    /// Draft UUID.
    pub id: Uuid,
    /// Owning subgraph slug.
    pub subgraph_slug: String,
    /// Draft title.
    pub title: String,
    /// Draft creation source.
    pub source: DraftSource,
    /// Published subgraph version on which this draft was based.
    pub base_version: i32,
    /// Draft `SchemaDoc` payload.
    pub schema_doc: SchemaDoc,
    /// Optional AI provenance context (`{prompt, dataset_ids, doc_ids, model_name}`).
    pub ai_context: serde_json::Value,
    /// Latest computed validation diagnostics.
    pub issues: Vec<ValidationIssue>,
    /// Positive static completeness check count (`04-STEP18` §18.1, §18.2.2).
    #[serde(default)]
    pub pass_count: u32,
    /// Lifecycle status (`DRAFT`, `PUBLISHED`, `DISCARDED`).
    pub status: DraftStatus,
    /// Creator user UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Request body for `POST /api/xblue/drafts` (`06-STEP05-06` §05.7, `04-STEP18` §18.6).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDraftRequest {
    /// Optional target subgraph slug in body (takes precedence over `?subgraph=`, `04-STEP18` §18.6).
    #[serde(default)]
    pub subgraph_slug: Option<String>,
    /// Draft title.
    pub title: String,
    /// Draft `SchemaDoc`.
    pub schema_doc: SchemaDoc,
    /// Optional base version (defaults to the subgraph's current head version).
    #[serde(default)]
    pub base_version: Option<i32>,
    /// Optional draft source (defaults to `MANUAL`).
    #[serde(default)]
    pub source: Option<DraftSource>,
    /// Optional AI context metadata.
    #[serde(default)]
    pub ai_context: Option<serde_json::Value>,
}

/// Request body for `PUT /api/xblue/drafts/{id}` (`06-STEP05-06` §05.7).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDraftRequest {
    /// Optional updated title.
    #[serde(default)]
    pub title: Option<String>,
    /// Replacement `SchemaDoc` to validate and store.
    pub schema_doc: SchemaDoc,
}

/// Request body for `POST /api/xblue/drafts/{id}/publish` (`06-STEP05-06` §05.4.1 & §05.7).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishDraftRequest {
    /// Human-readable summary of changes in this version.
    #[serde(default)]
    pub change_summary: String,
    /// Allow publishing despite `BREAKING_CHANGE` warnings.
    #[serde(default)]
    pub force: bool,
    /// Delete runtime `xb_nodes` belonging to removed `main` node types when `force = true`.
    #[serde(default)]
    pub cascade: bool,
}

/// Response body returned after publishing a draft or merging a subgraph (`06-STEP05-06` §05.4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishDraftResponse {
    /// Target subgraph slug.
    pub subgraph_slug: String,
    /// Newly published monotonic version number.
    pub version: i32,
    /// Published draft UUID.
    pub draft_id: Uuid,
    /// Published `SchemaDoc`.
    pub schema_doc: SchemaDoc,
    /// Structural diff relative to the previous head version.
    pub diff: SchemaDiff,
    /// Enqueued `MAP_MATERIALIZE` job IDs (when `subgraph_slug == "main"`).
    #[serde(default)]
    pub mapping_jobs: Vec<Uuid>,
    /// Enqueued `GRAPH_AGGREGATE` job IDs (when `subgraph_slug == "main"`).
    #[serde(default)]
    pub aggregate_jobs: Vec<Uuid>,
}

/// Request body for `POST /api/xblue/drafts/ai` (`06-STEP05-06` §05.5).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiDraftRequest {
    /// Draft title.
    pub title: String,
    /// Candidate `AxHub` dataset UUIDs to profile.
    #[serde(default)]
    pub dataset_ids: Vec<Uuid>,
    /// Candidate hypergraph document UUIDs for entity/relation extraction.
    #[serde(default)]
    pub doc_ids: Vec<Uuid>,
    /// Optional natural-language modeling prompt.
    #[serde(default)]
    pub description: String,
}

/// Cross-subgraph entity alignment candidate (`06-STEP05-06` §05.4.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeAlignmentCandidate {
    /// Department subgraph `NodeTypeSpec.name`.
    pub sub_type: String,
    /// Enterprise `main` `NodeTypeSpec.name`.
    pub main_type: String,
    /// Weighted similarity score in `[0.0, 1.0]` (`>= 0.65` or `1.0` on exact name match).
    pub score: f64,
    /// Recommended `SubgraphMergeStrategy`.
    pub suggested_strategy: SubgraphMergeStrategy,
}

/// Conflict category detected when previewing a subgraph merge into `main` (`06-STEP05-06` §05.4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MergeConflictKind {
    /// Same property name has incompatible `DataType`.
    #[serde(rename = "PROPERTY_TYPE_CONFLICT")]
    PropertyTypeConflict,
    /// Same metric has conflicting `AggRuleSpec` or `DerivedRuleSpec`.
    #[serde(rename = "RULE_CONFLICT")]
    RuleConflict,
    /// Same edge type has conflicting endpoints or `Cardinality`.
    #[serde(rename = "EDGE_CONFLICT")]
    EdgeConflict,
    /// Same mapping name targets a different dataset or node type.
    #[serde(rename = "MAPPING_NAME_CONFLICT")]
    MappingNameConflict,
}

/// Concrete conflict item reported in [`MergePreview`] (`06-STEP05-06` §05.4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    /// Conflict classification.
    pub kind: MergeConflictKind,
    /// Subgraph element identifier.
    pub sub_item: String,
    /// Conflicting `main` element identifier.
    pub main_item: String,
    /// Human-readable conflict description.
    pub detail: String,
}

/// Per-type alignment override supplied to merge preview or execution (`06-STEP05-06` §05.4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeMergeResolution {
    /// Department subgraph `NodeTypeSpec.name`.
    pub sub_type: String,
    /// Target `main` `NodeTypeSpec.name`.
    pub main_type: String,
    /// Selected merge strategy (`REUSE_MAIN`, `EXTEND_PROPS`, or `BRIDGE_LINK`).
    pub strategy: SubgraphMergeStrategy,
    /// Optional bridge edge name when `strategy == BRIDGE_LINK` (defaults to `"SAME_ENTITY_AS"`).
    #[serde(default)]
    pub bridge_edge_name: Option<String>,
}

/// Request body for `POST /api/xblue/subgraphs/{slug}/merge-preview`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergePreviewRequest {
    /// Optional explicit type resolutions to evaluate.
    #[serde(default)]
    pub resolutions: Vec<TypeMergeResolution>,
}

/// Alignment and conflict preview returned by `POST /api/xblue/subgraphs/{slug}/merge-preview` (`06-STEP05-06` §05.4.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergePreview {
    /// Source department subgraph slug.
    pub subgraph_slug: String,
    /// Source subgraph published version.
    pub subgraph_version: i32,
    /// Current `main` published version.
    pub main_version: i32,
    /// Candidate synonymous/matching node type alignments (`score >= 0.65`).
    pub alignments: Vec<TypeAlignmentCandidate>,
    /// Detected property, rule, edge, or mapping conflicts.
    pub conflicts: Vec<MergeConflict>,
    /// Net-new node type names to be added to `main`.
    pub added_node_types: Vec<String>,
    /// Net-new edge type names to be added to `main`.
    pub added_edge_types: Vec<String>,
    /// Net-new mapping names to be added to `main`.
    pub added_mappings: Vec<String>,
}

/// Request body for `POST /api/xblue/subgraphs/{slug}/merge` (`06-STEP05-06` §05.4.5).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeSubgraphRequest {
    /// Optional expected `main` head version (`409 VERSION_CONFLICT` on mismatch).
    #[serde(default)]
    pub base_main_version: Option<i32>,
    /// Default strategy applied to aligned types without an explicit resolution.
    pub default_strategy: SubgraphMergeStrategy,
    /// Optional per-type resolution overrides.
    #[serde(default)]
    pub resolutions: Vec<TypeMergeResolution>,
    /// Commit summary recorded on the new `main` `SchemaVersion`.
    #[serde(default)]
    pub change_summary: String,
    /// Force publication when breaking warnings exist.
    #[serde(default)]
    pub force: bool,
}

/// Persisted `xb_mappings` record returned by `GET /api/xblue/mappings` (`06-STEP05-06` §05.1 & §05.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingRecord {
    /// Mapping UUID.
    pub id: Uuid,
    /// Subgraph slug (`"main"` for executable runtime mappings).
    pub subgraph_slug: String,
    /// Unique mapping name within `subgraph_slug`.
    pub name: String,
    /// Bound `ds_datasets.id`.
    pub dataset_id: Uuid,
    /// Bound `ds_datasets.name`.
    pub dataset_name: String,
    /// Target `NodeTypeSpec.name`.
    pub node_type: String,
    /// Full mapping specification.
    pub spec: MappingSpec,
    /// Whether automatic sync triggers are enabled.
    pub enabled: bool,
    /// Dataset `schema_version` at definition time.
    pub schema_version: i32,
    /// Latest materialized `DuckLake` snapshot ID.
    pub last_snapshot_id: Option<i64>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Execution status of an `xb_mapping_runs` record (`06-STEP05-06` §05.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MappingRunStatus {
    /// Currently running.
    #[serde(rename = "RUNNING")]
    Running,
    /// Completed successfully.
    #[serde(rename = "SUCCEEDED")]
    Succeeded,
    /// Terminated with an error.
    #[serde(rename = "FAILED")]
    Failed,
}

impl MappingRunStatus {
    /// Returns the canonical database/wire string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

impl FromStr for MappingRunStatus {
    type Err = crate::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "RUNNING" => Ok(Self::Running),
            "SUCCEEDED" => Ok(Self::Succeeded),
            "FAILED" => Ok(Self::Failed),
            other => Err(crate::AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown MappingRunStatus: {other}"),
            )),
        }
    }
}

/// Rejected row sample recorded during mapping preview or execution (`06-STEP05-06` §05.1 & §06.2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingRejectSample {
    /// Source row JSON object.
    pub row: serde_json::Value,
    /// Rejection diagnostic reason.
    pub reason: String,
}

/// Persisted `xb_mapping_runs` execution record (`06-STEP05-06` §05.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingRunRecord {
    /// Run UUID.
    pub id: Uuid,
    /// Parent `xb_mappings.id`.
    pub mapping_id: Uuid,
    /// Associated `ax_jobs.id`.
    pub job_id: Uuid,
    /// Execution mode (`FULL` or `INCREMENTAL`).
    pub mode: MappingMode,
    /// Starting `DuckLake` snapshot (`None` for `FULL`).
    pub from_snapshot: Option<i64>,
    /// Target `DuckLake` snapshot.
    pub to_snapshot: Option<i64>,
    /// Upserted `xb_nodes` count.
    pub nodes_upserted: i64,
    /// Deleted `xb_nodes` count.
    pub nodes_deleted: i64,
    /// Upserted `xb_links` count.
    pub links_upserted: i64,
    /// Written `xb_metric_series` points count.
    pub points_written: i64,
    /// Rejected source rows count.
    pub rows_rejected: i64,
    /// Up to 20 rejected row samples.
    pub reject_samples: Vec<MappingRejectSample>,
    /// Run status (`RUNNING`, `SUCCEEDED`, `FAILED`).
    pub status: MappingRunStatus,
    /// Optional terminal failure message.
    pub error_message: Option<String>,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// Completion timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Paginated/list envelope returned by `GET /api/xblue/mappings/{id}/runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingRunsResponse {
    /// Mapping runs ordered by `started_at DESC`.
    pub items: Vec<MappingRunRecord>,
}

/// Request body for `POST /api/xblue/mappings/{id}/run` (`06-STEP05-06` §05.7 & §06.7).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerMappingRunRequest {
    /// Execution mode (`FULL` or `INCREMENTAL`).
    #[serde(default)]
    pub mode: MappingMode,
}

/// Response body for `POST /api/xblue/mappings/{id}/run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerMappingRunResponse {
    /// Enqueued `ax_jobs.id` (`MAP_MATERIALIZE`).
    pub job_id: Uuid,
    /// Target mapping UUID.
    pub mapping_id: Uuid,
    /// Requested execution mode.
    pub mode: MappingMode,
}

/// Dry-run mapped node preview row returned by `POST /api/xblue/mappings/{id}/preview` (`06-STEP05-06` §05.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MappingPreviewNodeRow {
    /// Evaluated `node_key`.
    pub node_key: String,
    /// Rendered `title`.
    pub title: String,
    /// Coerced static attributes.
    pub attributes: serde_json::Map<String, serde_json::Value>,
    /// Coerced numeric metrics.
    pub metrics: BTreeMap<String, f64>,
    /// Optional geographic coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoPoint>,
    /// Previewed outgoing/incoming link descriptors.
    pub links: Vec<serde_json::Value>,
}

/// Dry-run response body for `POST /api/xblue/mappings/{id}/preview` (first 50 rows, zero DB writes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MappingPreviewResponse {
    /// Previewed mapping UUID.
    pub mapping_id: Uuid,
    /// Number of source rows inspected (up to 50).
    pub sampled_rows: usize,
    /// Successfully mapped preview nodes.
    pub items: Vec<MappingPreviewNodeRow>,
    /// Number of rejected rows among `sampled_rows`.
    pub rows_rejected: usize,
    /// Up to 20 rejection diagnostics.
    pub reject_samples: Vec<MappingRejectSample>,
}

/// Strongly-typed field signature exposed on `GET /api/xblue/objects` (`06-STEP05-06` §05.6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectFieldSig {
    /// Property or metric identifier.
    pub name: String,
    /// Display label.
    pub display_name: String,
    /// `ATTRIBUTE` or `METRIC`.
    pub kind: PropertyKind,
    /// Canonical `DataType`.
    pub data_type: DataType,
    /// Required flag.
    pub required: bool,
    /// Optional measurement unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// PII sensitivity classification.
    pub pii_category: PiiCategory,
}

/// Neighbor traversal field signature exposed on `GET /api/xblue/objects` & GraphQL (`06-STEP05-06` §05.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectNeighborSig {
    /// Generated GraphQL traversal field name (e.g., `fulfillsOrders`, `fulfilledByWarehouses`).
    pub field_name: String,
    /// Underlying `EdgeTypeSpec.name`.
    pub link_type: String,
    /// Traversal direction (`OUT` or `IN`).
    pub direction: LinkDirection,
    /// Neighbor `NodeTypeSpec.name`.
    pub target_type: String,
    /// Declared relationship cardinality.
    pub cardinality: Cardinality,
}

/// Published domain object catalog entry (`GET /api/xblue/objects`, `06-STEP05-06` §05.6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectTypeCatalogItem {
    /// Published `NodeTypeSpec.name`.
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Type description.
    pub description: String,
    /// REST collection endpoint (`/api/xblue/objects/{type_name}`).
    pub collection_path: String,
    /// REST single-object endpoint (`/api/xblue/objects/{type_name}/{node_key}`).
    pub item_path: String,
    /// GraphQL root single-item field name (e.g., `warehouse`).
    pub graphql_field: String,
    /// GraphQL root paginated list field name (e.g., `warehouseList`).
    pub graphql_list_field: String,
    /// Strongly-typed property & metric signatures.
    pub fields: Vec<ObjectFieldSig>,
    /// Outgoing and incoming neighbor traversal signatures.
    pub neighbors: Vec<ObjectNeighborSig>,
    /// Ready-to-run sample GraphQL query.
    pub sample_graphql: String,
}

/// Response body for `GET /api/xblue/objects` (`06-STEP05-06` §05.6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectCatalogResponse {
    /// Active published `main` schema version.
    pub version: i32,
    /// Published domain object types.
    pub items: Vec<ObjectTypeCatalogItem>,
}

/// Query string parameters for `GET /api/xblue/objects/{type_name}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainObjectListQuery {
    /// Optional keyword search across `node_key` and `title`.
    #[serde(default)]
    pub q: Option<String>,
    /// Optional JSON attribute equality filter object (e.g., `{"status":"OPEN"}`).
    #[serde(default)]
    pub filter: Option<String>,
    /// Page size (`1..=200`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Query string parameters for `GET /api/xblue/objects/{type_name}/{node_key}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainObjectItemQuery {
    /// Whether to include 1-hop typed neighbors grouped by traversal field name.
    #[serde(default)]
    pub include_neighbors: bool,
}

/// Strongly-typed runtime domain object returned by `/api/xblue/objects/{type_name}*` (`06-STEP05-06` §05.6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainObjectRecord {
    /// Underlying `xb_nodes.id`.
    pub id: Uuid,
    /// Published `NodeTypeSpec.name`.
    pub type_name: String,
    /// Business primary key (`xb_nodes.node_key`).
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Coerced and PII-masked static attributes (`properties` on `xb_nodes`).
    pub attributes: serde_json::Map<String, serde_json::Value>,
    /// Typed numeric metrics snapshot (`metrics` on `xb_nodes`).
    pub metrics: BTreeMap<String, f64>,
    /// Optional WGS-84 geographic coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoPoint>,
    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
    /// Optional 1-hop neighbors keyed by GraphQL traversal field name when `include_neighbors = true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbors: Option<BTreeMap<String, Vec<DomainObjectRecord>>>,
}

/// Paginated response for `GET /api/xblue/objects/{type_name}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainObjectListResponse {
    /// Strongly-typed domain objects.
    pub items: Vec<DomainObjectRecord>,
    /// Next pagination cursor, if more objects exist.
    pub next_cursor: Option<String>,
}

/// Snapshot creation reason stored in `xb_schema_draft_snapshots.reason` (`04-STEP18` §18.1).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DraftSnapshotReason {
    /// Manually saved checkpoint.
    Manual,
    /// Recorded automatically before `PATCH_DRAFT` Copilot mutation.
    #[default]
    CopilotPatch,
    /// Recorded automatically before `confirm-proposals`.
    BatchConfirm,
    /// Recorded automatically before cross-subgraph merge.
    PreMerge,
}

impl DraftSnapshotReason {
    /// Canonical database string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "MANUAL",
            Self::CopilotPatch => "COPILOT_PATCH",
            Self::BatchConfirm => "BATCH_CONFIRM",
            Self::PreMerge => "PRE_MERGE",
        }
    }
}

impl FromStr for DraftSnapshotReason {
    type Err = crate::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "MANUAL" => Ok(Self::Manual),
            "COPILOT_PATCH" => Ok(Self::CopilotPatch),
            "BATCH_CONFIRM" => Ok(Self::BatchConfirm),
            "PRE_MERGE" => Ok(Self::PreMerge),
            other => Err(crate::AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown DraftSnapshotReason: {other}"),
            )),
        }
    }
}

/// Summary row returned by `GET /api/xblue/drafts/{id}/snapshots` (`04-STEP18` §18.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSnapshotSummary {
    /// Snapshot UUID.
    pub id: Uuid,
    /// Owning draft UUID.
    pub draft_id: Uuid,
    /// Monotonic per-draft snapshot sequence number (`>= 1`).
    pub snapshot_no: i32,
    /// Snapshot trigger reason.
    pub reason: DraftSnapshotReason,
    /// Actor user UUID, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Envelope response for `GET /api/xblue/drafts/{id}/snapshots`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSnapshotListResponse {
    /// Ordered snapshot summaries (`snapshot_no DESC`).
    pub items: Vec<DraftSnapshotSummary>,
}

/// Canvas selection focus passed to `POST /api/xblue/drafts/{id}/copilot` (`04-STEP18` §18.3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopilotFocusContext {
    /// Currently selected `NodeTypeSpec.name`, if any.
    #[serde(default)]
    pub selected_node_type: Option<String>,
    /// Currently selected `EdgeTypeSpec.name`, if any.
    #[serde(default)]
    pub selected_edge_type: Option<String>,
    /// Active inspector tab (`properties`, `rules`, `mappings`, etc.).
    #[serde(default)]
    pub active_tab: Option<String>,
}

/// Optional reference mounts (`@dataset`, `@spec`) passed to `POST /api/xblue/drafts/{id}/copilot`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopilotRefs {
    /// Referenced `AxHub` dataset UUIDs (profile metadata only; sample values stripped).
    #[serde(default)]
    pub dataset_ids: Vec<Uuid>,
    /// Whether to mount the compiled panoramic specification summary (`@spec`).
    #[serde(default)]
    pub include_spec_doc: bool,
}

/// Request body for `POST /api/xblue/drafts/{id}/copilot` (`04-STEP18` §18.3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioCopilotRequest {
    /// Execution mode (`DIAGNOSE_ONLY` vs `PATCH_DRAFT`).
    #[serde(default)]
    pub mode: CopilotActionMode,
    /// User instruction or quick-action prompt.
    pub prompt: String,
    /// Optional active canvas focus context.
    #[serde(default)]
    pub focus: Option<CopilotFocusContext>,
    /// Optional `@dataset` / `@spec` reference mounts.
    #[serde(default)]
    pub refs: Option<CopilotRefs>,
}

/// Single diagnostic finding produced by `xBlueStudio` Copilot (`04-STEP18` §18.3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotFinding {
    /// Severity (`ERROR`, `WARN`, `INFO`).
    pub severity: IssueSeverity,
    /// JSON-style target path (e.g. `node_types[Warehouse].properties[stock_qty]`).
    pub target_path: String,
    /// Factual rationale grounded in current schema/dataset metadata.
    pub rationale: String,
    /// Actionable remediation recommendation.
    pub suggested_fix: String,
}

/// Structured diagnosis block inside [`StudioCopilotResponse`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotDiagnosis {
    /// Concise overall assessment.
    pub summary: String,
    /// Structured findings list.
    #[serde(default)]
    pub findings: Vec<CopilotFinding>,
}

/// Response body for `POST /api/xblue/drafts/{id}/copilot` (`04-STEP18` §18.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudioCopilotResponse {
    /// Executed mode (`DIAGNOSE_ONLY` or `PATCH_DRAFT`).
    pub mode: CopilotActionMode,
    /// `true` iff `PATCH_DRAFT` modified `schema_doc` and persisted a pre-mutation snapshot.
    pub mutated: bool,
    /// Pre-mutation snapshot UUID (`None` in `DIAGNOSE_ONLY`).
    pub snapshot_id: Option<Uuid>,
    /// Pre-mutation snapshot sequence number (`None` in `DIAGNOSE_ONLY`).
    pub snapshot_no: Option<i32>,
    /// Structured inspection findings.
    pub diagnosis: CopilotDiagnosis,
    /// Structural diff introduced by `PATCH_DRAFT` (`None` in `DIAGNOSE_ONLY`).
    pub diff: Option<SchemaDiff>,
    /// Positive static validation check count.
    pub pass_count: u32,
    /// Current validation issues on the draft.
    pub issues: Vec<ValidationIssue>,
}

/// Scope selector for `POST /api/xblue/drafts/{id}/confirm-proposals` (`04-STEP18` §18.3.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfirmProposalsScope {
    /// Confirm every `PROPOSED` element in the draft.
    #[default]
    All,
    /// Confirm only elements listed in `node_types` and `edge_types`.
    Selected,
}

/// Request body for `POST /api/xblue/drafts/{id}/confirm-proposals` (`04-STEP18` §18.3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmProposalsRequest {
    /// Confirmation scope (`ALL` or `SELECTED`).
    #[serde(default)]
    pub scope: ConfirmProposalsScope,
    /// Selected `NodeTypeSpec.name` list when `scope == SELECTED`.
    #[serde(default)]
    pub node_types: Vec<String>,
    /// Selected `EdgeTypeSpec.name` list when `scope == SELECTED`.
    #[serde(default)]
    pub edge_types: Vec<String>,
}

/// Downstream consumer kind in Stage 5 of `NodeLineageGraph` (`04-STEP18` §18.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DownstreamConsumerKind {
    /// `AxFlow` operational metric alert rule (`ax_rules`).
    AlertRule,
    /// `AxAgent` published object endpoint (`ax_published_endpoints`).
    Endpoint,
    /// `xBlueView` twin dashboard (`xv_dashboards`).
    Dashboard,
}

impl DownstreamConsumerKind {
    /// Canonical wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlertRule => "ALERT_RULE",
            Self::Endpoint => "ENDPOINT",
            Self::Dashboard => "DASHBOARD",
        }
    }

    /// Lineage edge label connecting `NodeSchema` to this consumer.
    #[must_use]
    pub const fn edge_label(self) -> &'static str {
        match self {
            Self::AlertRule => "EVAL",
            Self::Endpoint => "EXPOSE",
            Self::Dashboard => "VIEW",
        }
    }
}

/// Downstream consumer reference resolved from `ax_rules`, `ax_published_endpoints`, and `xv_dashboards`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownstreamConsumerBinding {
    /// Lineage node ID (`"rule:..."`, `"ep:..."`, `"dash:..."`).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Consumer category.
    pub consumer_kind: DownstreamConsumerKind,
    /// Optional metric property referenced on the owning node type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_metric: Option<String>,
}

/// Request body for `POST /api/xblue/subgraphs/from-template` (`05-STEP19` §19.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialCloneTemplateRequest {
    /// Built-in template pack identifier (also accepts legacy `"template_id"` alias).
    #[serde(alias = "template_id")]
    pub pack_id: String,
    /// Optional custom trial subgraph slug (`^trial_[a-z0-9_-]{1,56}$`).
    #[serde(default)]
    pub slug_override: Option<String>,
    /// Optional subgraph display name override.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Response returned by `POST /api/xblue/subgraphs/from-template` (`05-STEP19` §19.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialCloneTemplateResponse {
    /// Created trial subgraph slug (`trial_<pack>_<hex4>`).
    pub subgraph_slug: String,
    /// Human-readable subgraph display name.
    pub display_name: String,
    /// Always `true` for isolated trial sandboxes.
    pub is_dev_sandbox: bool,
    /// Source built-in template pack identifier.
    pub source_template_id: String,
    /// Initial editable `SchemaDraft` UUID created inside the sandbox.
    pub draft_id: Uuid,
    /// Number of sample `BlueNode` records materialized into the sandbox.
    pub nodes_materialized: usize,
    /// Number of sample `BlueLink` records materialized into the sandbox.
    pub links_materialized: usize,
    /// Frontend `xBlueStudio` route targeting the cloned sandbox subgraph.
    pub studio_url: String,
}

/// Derives a deterministic `trial_<sanitized>_<hex4>` sandbox subgraph slug (`<= 63` chars, `05-STEP19` §19.2.3).
#[must_use]
pub fn derive_trial_subgraph_slug(pack_id: &str, short_hex4: &str) -> String {
    let mut sanitized = String::with_capacity(pack_id.len());
    for ch in pack_id.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if (ch == '_' || ch == '-') && !sanitized.is_empty() {
            if !sanitized.ends_with('_') && !sanitized.ends_with('-') {
                sanitized.push(ch);
            }
        } else if !sanitized.is_empty() && !sanitized.ends_with('_') && !sanitized.ends_with('-') {
            sanitized.push('_');
        }
    }
    let trimmed = sanitized.trim_matches(['_', '-']);
    let base_pack = if trimmed.is_empty() { "pack" } else { trimmed };

    let mut hex_part: String = short_hex4
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(4)
        .collect();
    if hex_part.is_empty() {
        hex_part.push_str("0000");
    }

    // Ensure `rest` after `"trial_"` fits in `1..=56` chars (`total <= 62 <= 63`).
    let max_pack_len = 56_usize.saturating_sub(hex_part.len().saturating_add(1));
    let truncated: String = base_pack.chars().take(max_pack_len).collect();
    let clean_pack = truncated.trim_end_matches(['_', '-']);
    let final_pack = if clean_pack.is_empty() {
        "pack"
    } else {
        clean_pack
    };
    format!("trial_{final_pack}_{hex_part}")
}

/// Validates a trial sandbox subgraph slug (`05-STEP19` §19.2.3).
///
/// Rejects `"main"` and requires `^trial_[a-z0-9_-]{1,56}$`.
///
/// # Errors
/// Returns [`crate::AppError::BadRequest`] (`INVALID_ARGUMENT`) when `slug` is `"main"` or fails `^trial_[a-z0-9_-]{1,56}$`.
pub fn validate_trial_subgraph_slug(slug: &str) -> Result<(), crate::AppError> {
    if slug == "main" {
        return Err(crate::AppError::bad(
            "INVALID_ARGUMENT",
            "trial subgraph slug must not be 'main'",
        ));
    }
    let Some(rest) = slug.strip_prefix("trial_") else {
        return Err(crate::AppError::bad(
            "INVALID_ARGUMENT",
            "trial subgraph slug must start with 'trial_'",
        ));
    };
    if rest.is_empty()
        || rest.len() > 56
        || !rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(crate::AppError::bad(
            "INVALID_ARGUMENT",
            "trial subgraph slug must match ^trial_[a-z0-9_-]{1,56}$",
        ));
    }
    Ok(())
}

const fn default_prod_env() -> Environment {
    Environment::Prod
}

/// Request body for `POST /api/xblue/subgraphs/{slug}/package-agent` (`05-STEP19` §19.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSubgraphAgentRequest {
    /// Target `AxAgent` machine/display name.
    pub agent_name: String,
    /// Optional domain assistant description.
    #[serde(default)]
    pub description: String,
    /// Published `NodeTypeSpec.name` items to expose as `OBJECT_TYPE` endpoints.
    #[serde(default)]
    pub node_types: Vec<String>,
    /// Certified `MetricDef.name` items to expose as `METRIC` endpoints.
    #[serde(default)]
    pub metric_names: Vec<String>,
    /// Target endpoint environment (`PROD` default; `TEST` required on sandbox subgraphs).
    #[serde(default = "default_prod_env")]
    pub environment: Environment,
    /// Whether upserted endpoints are exposed as MCP tools (`true` default).
    #[serde(default = "default_true")]
    pub expose_mcp: bool,
    /// Whether to publish a new immutable `ax_agent_versions` snapshot immediately (`true` default).
    #[serde(default = "default_true")]
    pub publish_agent: bool,
}

/// Upserted published endpoint summary inside [`PackageSubgraphAgentResponse`] (`05-STEP19` §19.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagedEndpointItem {
    /// Endpoint UUID (`ax_published_endpoints.id`).
    pub id: Uuid,
    /// Generated kebab-case endpoint slug.
    pub slug: String,
    /// Published endpoint capability kind (`OBJECT_TYPE` or `METRIC`).
    pub kind: PublishedKind,
    /// Whether this endpoint is exposed on `/api/mcp`.
    pub expose_mcp: bool,
}

/// Response returned by `POST /api/xblue/subgraphs/{slug}/package-agent` (`05-STEP19` §19.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSubgraphAgentResponse {
    /// Created or updated `AxAgent` UUID.
    pub agent_id: Uuid,
    /// Agent name.
    pub agent_name: String,
    /// Published `ax_agents.version` (`0` when draft-only).
    pub agent_version: i32,
    /// Upserted `OBJECT_TYPE` and `METRIC` published endpoints bound to the agent.
    pub endpoints_upserted: Vec<PackagedEndpointItem>,
    /// Count of upserted endpoints with `expose_mcp == true`.
    pub mcp_exposed_count: usize,
    /// MCP tool names registered for upserted endpoints when `expose_mcp == true`.
    #[serde(default)]
    pub mcp_tool_names: Vec<String>,
    /// Frontend `AxAgent` workbench URL targeting this agent.
    pub workbench_url: String,
}

fn to_kebab_segment(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len().saturating_add(4));
    for (idx, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && !out.is_empty() && !out.ends_with('-') {
                let prev = idx.checked_sub(1).and_then(|i| chars.get(i)).copied();
                let next = chars.get(idx.saturating_add(1)).copied();
                let split_here = prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                    || (prev.is_some_and(|p| p.is_ascii_uppercase())
                        && next.is_some_and(|n| n.is_ascii_lowercase()));
                if split_here {
                    out.push('-');
                }
            }
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Builds an idempotent lowercase kebab-case endpoint slug matching `^[a-z0-9][a-z0-9-]{2,62}$` (`05-STEP19` §19.3.1).
///
/// Examples:
/// - `("main", true, "Warehouse", "objects") -> "warehouse-objects"`
/// - `("logistics", false, "Warehouse", "objects") -> "logistics-warehouse-objects"`
/// - `("main", true, "paid_amount", "metric") -> "paid-amount-metric"`
#[must_use]
pub fn build_packaged_endpoint_slug(
    subgraph_slug: &str,
    is_main: bool,
    target_name: &str,
    suffix: &str,
) -> String {
    let target_seg = {
        let s = to_kebab_segment(target_name);
        if s.is_empty() {
            String::from("item")
        } else {
            s
        }
    };
    let suffix_seg = to_kebab_segment(suffix);
    let prefix_seg = if is_main {
        String::new()
    } else {
        to_kebab_segment(subgraph_slug)
    };

    let head = if prefix_seg.is_empty() {
        target_seg
    } else {
        format!("{prefix_seg}-{target_seg}")
    };

    let mut slug = if suffix_seg.is_empty() {
        let capped: String = head.chars().take(63).collect();
        capped.trim_end_matches('-').to_owned()
    } else if suffix_seg.len() >= 58 {
        let joined = format!("{head}-{suffix_seg}");
        let capped: String = joined.chars().take(63).collect();
        capped.trim_end_matches('-').to_owned()
    } else {
        let max_head_len = 63_usize.saturating_sub(suffix_seg.len().saturating_add(1));
        let capped_head: String = head.chars().take(max_head_len).collect();
        let clean_head = capped_head.trim_end_matches('-');
        let effective_head = if clean_head.is_empty() {
            "ep"
        } else {
            clean_head
        };
        format!("{effective_head}-{suffix_seg}")
    };

    if slug.is_empty() {
        slug = String::from("ep-0");
    } else if slug.len() < 3 {
        slug.push_str("-ep");
    }
    slug
}

/// Reverse downstream asset counts referencing a single `NodeTypeSpec` (`05-STEP19` §19.3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConsumptionCounts {
    /// Enabled `OBJECT_TYPE` published endpoints targeting this node type.
    pub endpoint_count: usize,
    /// `AxAgent` definitions binding any of those object endpoints.
    pub agent_count: usize,
    /// Distinct `xBlueView` dashboards referencing this node type in widget `deps` or `data_source`.
    pub dashboard_count: usize,
    /// Enabled `METRIC` alert rules (`ax_rules`) targeting this node type.
    pub rule_count: usize,
    /// Slugs of matching published endpoints.
    #[serde(default)]
    pub endpoint_slugs: Vec<String>,
    /// UUIDs of matching `AxAgent` records.
    #[serde(default)]
    pub agent_ids: Vec<Uuid>,
    /// Slugs of matching `xBlueView` dashboards.
    #[serde(default)]
    pub dashboard_slugs: Vec<String>,
}

/// Response returned by `GET /api/xblue/subgraphs/{slug}/consumption` (`05-STEP19` §19.3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgraphConsumptionResponse {
    /// Inspected subgraph slug.
    pub subgraph_slug: String,
    /// Downstream consumption counters keyed by `NodeTypeSpec.name`.
    #[serde(default)]
    pub by_node_type: BTreeMap<String, NodeConsumptionCounts>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_property_data_types_deserialize_cleanly() -> Result<(), serde_json::Error> {
        let json = r#"[
            {"name":"inventory","data_type":"number"},
            {"name":"status","data_type":"string"},
            {"name":"active","data_type":"boolean"},
            {"name":"amount","data_type":"DECIMAL","kind":"METRIC","required":true}
        ]"#;
        let specs: Vec<PropertySpec> = serde_json::from_str(json)?;
        assert_eq!(specs.len(), 4);
        assert_eq!(specs.first().map(|p| p.data_type), Some(DataType::Float64));
        assert_eq!(specs.get(1).map(|p| p.data_type), Some(DataType::String));
        assert_eq!(specs.get(2).map(|p| p.data_type), Some(DataType::Boolean));
        assert_eq!(specs.get(3).map(|p| p.kind), Some(PropertyKind::Metric));
        Ok(())
    }

    #[test]
    fn published_schema_response_roundtrips_into_schema_doc() -> Result<(), serde_json::Error> {
        let resp = PublishedSchemaResponse {
            subgraph_slug: String::from("main"),
            version: 3,
            doc: SchemaDoc {
                node_types: Vec::new(),
                edge_types: Vec::new(),
                mappings: Vec::new(),
            },
        };
        let wire = serde_json::to_string(&resp)?;
        let parsed_doc: SchemaDoc = serde_json::from_str(&wire)?;
        assert_eq!(parsed_doc.node_types.len(), 0);
        Ok(())
    }

    #[test]
    fn trial_subgraph_slug_derivation_and_validation() {
        let slug = derive_trial_subgraph_slug("supply_chain", "8A2F");
        assert_eq!(slug, "trial_supply_chain_8a2f");
        assert!(validate_trial_subgraph_slug(&slug).is_ok());

        let long_pack = "very_long_enterprise_manufacturing_template_pack_identifier_v2_extra";
        let derived_long = derive_trial_subgraph_slug(long_pack, "deadbeef");
        assert!(derived_long.len() <= 63);
        assert!(derived_long.ends_with("_dead"));
        assert!(validate_trial_subgraph_slug(&derived_long).is_ok());

        assert!(validate_trial_subgraph_slug("main").is_err());
        assert!(validate_trial_subgraph_slug("logistics").is_err());
        assert!(validate_trial_subgraph_slug("trial_").is_err());
        assert!(validate_trial_subgraph_slug("trial_UpperCase").is_err());
        let too_long_override = format!("trial_{}", "a".repeat(57));
        assert!(validate_trial_subgraph_slug(&too_long_override).is_err());
        let max_valid_override = format!("trial_{}", "a".repeat(56));
        assert!(validate_trial_subgraph_slug(&max_valid_override).is_ok());
    }

    #[test]
    fn packaged_endpoint_slug_converts_pascal_and_snake_and_caps_length() {
        assert_eq!(
            build_packaged_endpoint_slug("main", true, "Warehouse", "objects"),
            "warehouse-objects"
        );
        assert_eq!(
            build_packaged_endpoint_slug("main", true, "PurchaseOrder", "objects"),
            "purchase-order-objects"
        );
        assert_eq!(
            build_packaged_endpoint_slug("trial_supply_8a2f", false, "Warehouse", "objects"),
            "trial-supply-8a2f-warehouse-objects"
        );
        assert_eq!(
            build_packaged_endpoint_slug("main", true, "paid_amount", "metric"),
            "paid-amount-metric"
        );

        let huge_sg = "trial_extremely_verbose_department_sandbox_subgraph_identifier_1234";
        let capped =
            build_packaged_endpoint_slug(huge_sg, false, "DistributionWarehouse", "objects");
        assert!(capped.len() <= 63);
        assert!(capped.len() >= 3);
        assert!(!capped.ends_with('-'));
        assert!(capped.ends_with("-objects"));
        assert!(
            capped
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
    }

    #[test]
    fn trial_clone_request_accepts_template_id_alias() -> Result<(), serde_json::Error> {
        let req: TrialCloneTemplateRequest =
            serde_json::from_str(r#"{"template_id":"supply_chain"}"#)?;
        assert_eq!(req.pack_id, "supply_chain");
        assert_eq!(req.slug_override, None);
        Ok(())
    }
}
