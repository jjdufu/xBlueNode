//! Focus-aware `xBlueStudio` AI Copilot constrained-decoding schemas, prompt builder,
//! deterministic `PROPOSED` patch merger, and cassette-ready runners (`04-STEP18` §18.3).

use std::fmt::Write as _;

use axpoint_domain::AppError;
use axpoint_domain::axhub::Dataset;
use axpoint_domain::enums::{
    AggFn, Cardinality, CopilotActionMode, DataType, DesignMaturity, PiiCategory, Purpose,
    SemanticLayerTier,
};
use axpoint_domain::json_schema::JsonSchemaSpec;
use axpoint_domain::ports::AiPort;
use axpoint_domain::studio::{
    AggRuleSpec, CopilotDiagnosis, CopilotFocusContext, DerivedRuleSpec, EdgeTypeSpec,
    LinkDirection, NodeTypeSpec, PropertyKind, PropertySpec, SchemaDiff, SchemaDoc, diff_schema,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Canonical `JsonSchemaSpec.name` for `DIAGNOSE_ONLY` constrained decoding (`04-STEP18` §18.3.2).
pub const COPILOT_DIAGNOSIS_SCHEMA_NAME: &str = "axpoint_studio_copilot_diagnosis";

/// Canonical `JsonSchemaSpec.name` for `PATCH_DRAFT` constrained decoding (`04-STEP18` §18.3.2).
pub const COPILOT_PATCH_SCHEMA_NAME: &str = "axpoint_studio_copilot_patch";

/// System prompt governing read-only `DIAGNOSE_ONLY` completeness and modeling inspections.
pub const STUDIO_COPILOT_DIAGNOSE_SYSTEM_PROMPT: &str = "\
你是 xBlueStudio 焦点感知领域建模审查助手（DIAGNOSE_ONLY 只读诊断模式）。\
请严格基于当前子图 SchemaDoc、画布选中焦点（Focus）、可选数据集列统计画像与规格摘要，输出严格符合 JSON Schema 的结构化体检结论。\
硬性约束：\n\
1. 严禁虚构不存在的数据集、列名或业务数值。\n\
2. 每条 finding 必须给出准确的 JSON 路径 target_path（如 node_types[Warehouse].mappings）、客观依据 rationale 与可执行修复建议 suggested_fix。";

/// System prompt governing `PATCH_DRAFT` incremental ontology patching.
pub const STUDIO_COPILOT_PATCH_SYSTEM_PROMPT: &str = "\
你是 xBlueStudio 焦点感知领域建模改写助手（PATCH_DRAFT 增量补丁模式）。\
请严格基于当前子图 SchemaDoc、画布选中焦点（Focus）与用户指令，输出严格符合 JSON Schema 的增量补丁包（CopilotPatchEnvelope）。\
硬性约束：\n\
1. 仅输出需要新增或修改的节点类型、属性/指标、有向关系边、跨边聚合规则与派生公式规则；未变更要素置为空数组 []。\n\
2. 属性名必须符合 ^[a-z][a-z0-9_]{0,62}$，严禁使用保留字段 id, tenant_id, node_key, title, metrics, properties。\n\
3. 新增派生规则（upsert_derived_rules）或聚合规则（upsert_agg_rules）时，对应的 target_metric 必须同时作为 kind=METRIC 属性存在或一并在 upsert_properties 中声明（如 net_margin = revenue - cost）。\n\
4. 边权重 impact_weight 必须在 (0.0, 1.0] 内，严禁引入指标循环依赖。";

/// Sanitized dataset column metadata safe for outbound LLM prompts (never includes sample values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotColumnProfile {
    /// Column machine name.
    pub name: String,
    /// Logical column data type.
    pub data_type: DataType,
    /// Null fraction in `[0.0, 1.0]`.
    pub null_rate: f64,
    /// Distinct cardinality estimate.
    pub distinct_count: u64,
}

/// Sanitized dataset metadata mounted via `@dataset` in `xBlueStudio` Copilot (`04-STEP18` §18.3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotDatasetProfile {
    /// Dataset machine name.
    pub dataset_name: String,
    /// Dataset display name.
    pub display_name: String,
    /// Total profiled row count.
    pub row_count: u64,
    /// Sanitized column profiles (sample values stripped).
    pub columns: Vec<CopilotColumnProfile>,
}

impl CopilotDatasetProfile {
    /// Builds a sanitized [`CopilotDatasetProfile`] from an `AxHub` [`Dataset`], dropping all raw sample values.
    #[must_use]
    pub fn from_dataset(ds: &Dataset) -> Self {
        let row_count = u64::try_from(ds.row_count.max(0)).unwrap_or(0);
        let columns = ds
            .schema_json
            .iter()
            .map(|col| CopilotColumnProfile {
                name: col.name.clone(),
                data_type: col.data_type,
                null_rate: col.null_ratio.clamp(0.0, 1.0),
                distinct_count: u64::try_from(col.distinct_estimate.max(0)).unwrap_or(0),
            })
            .collect();
        Self {
            dataset_name: ds.name.clone(),
            display_name: ds.display_name.clone(),
            row_count,
            columns,
        }
    }
}

/// Context bundle passed to [`build_copilot_user_prompt`], [`run_copilot_diagnose`], and [`run_copilot_patch`].
#[derive(Debug, Clone)]
pub struct CopilotPromptInput<'a> {
    /// Target subgraph slug.
    pub subgraph_slug: &'a str,
    /// Current working draft `SchemaDoc`.
    pub schema_doc: &'a SchemaDoc,
    /// User instruction or quick-action prompt.
    pub user_prompt: &'a str,
    /// Optional canvas focus (`selected_node_type`, `selected_edge_type`, `active_tab`).
    pub focus: Option<&'a CopilotFocusContext>,
    /// Optional sanitized `@dataset` column profiles.
    pub datasets: &'a [CopilotDatasetProfile],
    /// Optional compiled `@spec` overview text when `refs.include_spec_doc` is `true`.
    pub spec_overview: Option<&'a str>,
}

/// Node type header patch entry in [`CopilotPatchEnvelope`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotNodeTypePatch {
    /// `PascalCase` node type name.
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Domain description.
    pub description: String,
    /// Optional semantic architecture tier.
    pub semantic_layer_tier: Option<SemanticLayerTier>,
    /// Optional business domain group label.
    pub domain_group: Option<String>,
}

/// Property or metric patch payload inside [`CopilotNodePropertyUpsert`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotPropertyPatch {
    /// `snake_case` property or metric identifier.
    pub name: String,
    /// Human-readable label.
    pub display_name: String,
    /// Field description.
    pub description: String,
    /// Optional measurement unit (e.g. `"件"`, `"元"`, `"天"`).
    pub unit: Option<String>,
    /// Logical data type.
    pub data_type: DataType,
    /// `ATTRIBUTE` or `METRIC`.
    pub kind: PropertyKind,
    /// Required flag.
    pub required: bool,
    /// PII sensitivity classification.
    #[serde(default = "default_pii_none")]
    pub pii_category: PiiCategory,
}

const fn default_pii_none() -> PiiCategory {
    PiiCategory::None
}

/// Scoped property/metric upsert targeting `node_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotNodePropertyUpsert {
    /// Owning `NodeTypeSpec.name`.
    pub node_type: String,
    /// Patched property or metric definition.
    pub property: CopilotPropertyPatch,
}

/// Directed edge type patch entry in [`CopilotPatchEnvelope`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotEdgeTypePatch {
    /// `UPPER_SNAKE_CASE` relationship identifier.
    pub name: String,
    /// Source `NodeTypeSpec.name`.
    pub src_type_name: String,
    /// Destination `NodeTypeSpec.name`.
    pub dst_type_name: String,
    /// Relationship cardinality.
    pub cardinality: Cardinality,
    /// Causal propagation weight in `(0.0, 1.0]`.
    pub impact_weight: f64,
    /// Optional edge description.
    pub description: String,
}

/// Cross-link aggregation rule patch payload inside [`CopilotNodeAggRuleUpsert`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotAggRulePatch {
    /// Target `METRIC` property name on `node_type`.
    pub target_metric: String,
    /// Aggregation function.
    pub op: AggFn,
    /// Incident `EdgeTypeSpec.name`.
    pub via_link_type: String,
    /// Traversal direction (`OUT` or `IN`).
    pub direction: LinkDirection,
    /// Neighbor `NodeTypeSpec.name`.
    pub source_type_name: String,
    /// Neighbor `METRIC` property name.
    pub source_metric: String,
    /// Optional boolean filter expression.
    pub filter: Option<String>,
    /// Whether to weight by `xb_links.weight`.
    pub weight_by_link: bool,
}

/// Scoped `AggRuleSpec` upsert targeting `node_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotNodeAggRuleUpsert {
    /// Owning `NodeTypeSpec.name`.
    pub node_type: String,
    /// Aggregation rule specification.
    pub rule: CopilotAggRulePatch,
}

/// Intra-node derived formula patch payload inside [`CopilotNodeDerivedRuleUpsert`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotDerivedRulePatch {
    /// Target `METRIC` property name on `node_type`.
    pub target_metric: String,
    /// Deterministic Step 06 numeric expression.
    pub expression: String,
    /// Whether the rule is active.
    pub enabled: bool,
}

/// Scoped `DerivedRuleSpec` upsert targeting `node_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotNodeDerivedRuleUpsert {
    /// Owning `NodeTypeSpec.name`.
    pub node_type: String,
    /// Derived formula rule specification.
    pub rule: CopilotDerivedRulePatch,
}

/// Constrained-decoding incremental patch envelope returned by `PATCH_DRAFT` (`04-STEP18` §18.3.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CopilotPatchEnvelope {
    /// Accompanying structured inspection diagnosis.
    pub diagnosis: CopilotDiagnosis,
    /// Added or updated node type headers.
    #[serde(default)]
    pub upsert_node_types: Vec<CopilotNodeTypePatch>,
    /// Added or updated node properties/metrics.
    #[serde(default)]
    pub upsert_properties: Vec<CopilotNodePropertyUpsert>,
    /// Added or updated directed edge types.
    #[serde(default)]
    pub upsert_edge_types: Vec<CopilotEdgeTypePatch>,
    /// Added or updated cross-edge aggregation rules.
    #[serde(default)]
    pub upsert_agg_rules: Vec<CopilotNodeAggRuleUpsert>,
    /// Added or updated intra-node derived metric rules.
    #[serde(default)]
    pub upsert_derived_rules: Vec<CopilotNodeDerivedRuleUpsert>,
}

/// Result produced by [`run_copilot_patch`].
#[derive(Debug, Clone, PartialEq)]
pub struct CopilotPatchOutcome {
    /// Structured inspection findings accompanying the patch.
    pub diagnosis: CopilotDiagnosis,
    /// Patched `SchemaDoc` with new/modified elements marked `DesignMaturity::Proposed`.
    pub patched_doc: SchemaDoc,
    /// Deterministic structural diff relative to the pre-patch `SchemaDoc`.
    pub diff: SchemaDiff,
    /// `true` when `patched_doc` differs from the base `SchemaDoc`.
    pub mutated: bool,
}

fn diagnosis_json_value() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "severity": { "type": "string", "enum": ["ERROR", "WARN", "INFO"] },
                        "target_path": { "type": "string" },
                        "rationale": { "type": "string" },
                        "suggested_fix": { "type": "string" }
                    },
                    "required": ["severity", "target_path", "rationale", "suggested_fix"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["summary", "findings"],
        "additionalProperties": false
    })
}

/// Builds the strict Draft 2020-12 [`JsonSchemaSpec`] for `DIAGNOSE_ONLY` (`04-STEP18` §18.3.2).
#[must_use]
pub fn copilot_diagnosis_schema() -> JsonSchemaSpec {
    JsonSchemaSpec::new_strict(
        COPILOT_DIAGNOSIS_SCHEMA_NAME,
        "Read-only xBlueStudio ontology completeness and focus inspection report.",
        diagnosis_json_value(),
    )
}

fn patch_node_and_prop_schemas() -> (Value, Value) {
    let nullable_str = json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] });
    let nt_item = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "display_name": { "type": "string" },
            "description": { "type": "string" },
            "semantic_layer_tier": {
                "anyOf": [
                    { "type": "string", "enum": ["STATIC_FACT", "EVENT_DYNAMIC", "CUBE_ROLLUP"] },
                    { "type": "null" }
                ]
            },
            "domain_group": nullable_str
        }
    });
    let prop_item = json!({
        "type": "object",
        "properties": {
            "node_type": { "type": "string" },
            "property": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "display_name": { "type": "string" },
                    "description": { "type": "string" },
                    "unit": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                    "data_type": {
                        "type": "string",
                        "enum": ["STRING", "INT64", "FLOAT64", "DECIMAL", "BOOLEAN", "DATE", "TIMESTAMP", "JSON"]
                    },
                    "kind": { "type": "string", "enum": ["ATTRIBUTE", "METRIC"] },
                    "required": { "type": "boolean" },
                    "pii_category": {
                        "type": "string",
                        "enum": ["NONE", "PHONE", "ID_CARD", "EMAIL", "BANK_CARD", "NAME", "ADDRESS", "CUSTOM"]
                    }
                }
            }
        }
    });
    (nt_item, prop_item)
}

fn patch_edge_and_rule_schemas() -> (Value, Value, Value) {
    let edge_item = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "src_type_name": { "type": "string" },
            "dst_type_name": { "type": "string" },
            "cardinality": {
                "type": "string",
                "enum": ["ONE_TO_ONE", "ONE_TO_MANY", "MANY_TO_ONE", "MANY_TO_MANY"]
            },
            "impact_weight": { "type": "number" },
            "description": { "type": "string" }
        }
    });
    let agg_item = json!({
        "type": "object",
        "properties": {
            "node_type": { "type": "string" },
            "rule": {
                "type": "object",
                "properties": {
                    "target_metric": { "type": "string" },
                    "op": {
                        "type": "string",
                        "enum": ["SUM", "AVG", "COUNT", "COUNT_DISTINCT", "MIN", "MAX", "MEDIAN", "LAST"]
                    },
                    "via_link_type": { "type": "string" },
                    "direction": { "type": "string", "enum": ["OUT", "IN"] },
                    "source_type_name": { "type": "string" },
                    "source_metric": { "type": "string" },
                    "filter": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                    "weight_by_link": { "type": "boolean" }
                }
            }
        }
    });
    let derived_item = json!({
        "type": "object",
        "properties": {
            "node_type": { "type": "string" },
            "rule": {
                "type": "object",
                "properties": {
                    "target_metric": { "type": "string" },
                    "expression": { "type": "string" },
                    "enabled": { "type": "boolean" }
                }
            }
        }
    });
    (edge_item, agg_item, derived_item)
}

/// Builds the strict Draft 2020-12 [`JsonSchemaSpec`] for `PATCH_DRAFT` (`04-STEP18` §18.3.2).
#[must_use]
pub fn copilot_patch_envelope_schema() -> JsonSchemaSpec {
    let (nt_item, prop_item) = patch_node_and_prop_schemas();
    let (edge_item, agg_item, derived_item) = patch_edge_and_rule_schemas();
    JsonSchemaSpec::new_strict(
        COPILOT_PATCH_SCHEMA_NAME,
        "Incremental xBlueStudio draft patch envelope whose mutated elements enter PROPOSED state.",
        json!({
            "type": "object",
            "properties": {
                "diagnosis": diagnosis_json_value(),
                "upsert_node_types": { "type": "array", "items": nt_item },
                "upsert_properties": { "type": "array", "items": prop_item },
                "upsert_edge_types": { "type": "array", "items": edge_item },
                "upsert_agg_rules": { "type": "array", "items": agg_item },
                "upsert_derived_rules": { "type": "array", "items": derived_item }
            }
        }),
    )
}

/// Assembles the focus-aware user prompt for `xBlueStudio` Copilot (`04-STEP18` §18.3).
#[must_use]
pub fn build_copilot_user_prompt(
    mode: CopilotActionMode,
    input: &CopilotPromptInput<'_>,
) -> String {
    let mut out = String::with_capacity(1_024);
    let _fmt_ok = writeln!(out, "### xBlueStudio Copilot ({})", mode.as_str());
    let _fmt_ok = writeln!(out, "Subgraph: `{}`\n", input.subgraph_slug);

    append_focus_section(&mut out, input.focus, input.schema_doc);
    append_schema_summary(&mut out, input.schema_doc);

    if !input.datasets.is_empty() {
        append_sanitized_datasets(&mut out, input.datasets);
    }
    if let Some(spec) = input.spec_overview.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("### @spec 模型规格书摘要\n");
        out.push_str(spec);
        out.push_str("\n\n");
    }

    out.push_str("### 用户指令\n");
    out.push_str(input.user_prompt.trim());
    out.push('\n');
    out
}

fn append_focus_section(out: &mut String, focus: Option<&CopilotFocusContext>, doc: &SchemaDoc) {
    out.push_str("### 当前画布焦点\n");
    let Some(f) = focus else {
        out.push_str("- 焦点：全局子图\n\n");
        return;
    };
    let node_sel = f
        .selected_node_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let edge_sel = f
        .selected_edge_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tab_sel = f
        .active_tab
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("canvas");

    let _fmt_ok = writeln!(
        out,
        "- selected_node_type={} selected_edge_type={} active_tab={tab_sel}",
        node_sel.unwrap_or("NONE"),
        edge_sel.unwrap_or("NONE"),
    );

    if let Some(nt_name) = node_sel
        && let Some(nt) = doc.node_types.iter().find(|n| n.name == nt_name)
    {
        let _fmt_ok = writeln!(
            out,
            "- 焦点节点详情 `{}` ({}) tier={} group={}:",
            nt.name,
            nt.display_name,
            nt.semantic_layer_tier
                .map_or("NONE", SemanticLayerTier::as_str),
            nt.domain_group.as_deref().unwrap_or("NONE"),
        );
        for p in &nt.properties {
            let _fmt_ok = writeln!(
                out,
                "  * prop `{}` kind={} type={} unit={} maturity={}",
                p.name,
                p.kind.as_str(),
                p.data_type.as_str(),
                p.unit.as_deref().unwrap_or("-"),
                p.maturity.as_str(),
            );
        }
        for r in &nt.agg_rules {
            let _fmt_ok = writeln!(
                out,
                "  * agg `{}` = {}({}.{} via {} [{}])",
                r.target_metric,
                r.op.as_str(),
                r.source_type_name,
                r.source_metric,
                r.via_link_type,
                r.direction.as_str(),
            );
        }
        for d in &nt.derived_rules {
            let _fmt_ok = writeln!(out, "  * derived `{}` = {}", d.target_metric, d.expression);
        }
    }
    out.push('\n');
}

fn append_schema_summary(out: &mut String, doc: &SchemaDoc) {
    out.push_str("### 当前草案模型摘要\n");
    if doc.node_types.is_empty() && doc.edge_types.is_empty() {
        out.push_str("（空草案）\n\n");
        return;
    }
    for nt in &doc.node_types {
        let counts = nt.badge_counts();
        let props: Vec<String> = nt
            .properties
            .iter()
            .map(|p| format!("{}:{}", p.name, p.kind.as_str()))
            .collect();
        let _fmt_ok = writeln!(
            out,
            "- NodeType `{}` ({}) [{}p·{}m·{}r] props=[{}]",
            nt.name,
            nt.display_name,
            counts.property_count,
            counts.metric_count,
            counts.rule_count,
            props.join(", "),
        );
    }
    for et in &doc.edge_types {
        let _fmt_ok = writeln!(
            out,
            "- EdgeType `{}`: {} -> {} ({}, w={:.2})",
            et.name, et.src_type_name, et.dst_type_name, et.cardinality, et.impact_weight
        );
    }
    for m in &doc.mappings {
        let _fmt_ok = writeln!(
            out,
            "- Mapping `{}`: dataset=`{}` key=`{}` -> node_type=`{}`",
            m.name, m.dataset, m.key_column, m.node_type
        );
    }
    out.push('\n');
}

fn append_sanitized_datasets(out: &mut String, datasets: &[CopilotDatasetProfile]) {
    out.push_str("### @dataset 数据集列统计画像（已剥离原始样本）\n");
    for ds in datasets {
        let _fmt_ok = writeln!(
            out,
            "- Dataset `{}` ({}) rows={}",
            ds.dataset_name, ds.display_name, ds.row_count
        );
        for col in &ds.columns {
            let _fmt_ok = writeln!(
                out,
                "  * col `{}` type={} null_rate={:.3} distinct={}",
                col.name,
                col.data_type.as_str(),
                col.null_rate,
                col.distinct_count
            );
        }
    }
    out.push('\n');
}

/// Applies a [`CopilotPatchEnvelope`] onto a clone of `base_doc`, marking every inserted or
/// modified element `DesignMaturity::Proposed` while preserving untouched `Confirmed` elements.
///
/// Returns `(patched_doc, mutated)`.
#[must_use]
pub fn apply_copilot_patch(
    base_doc: &SchemaDoc,
    patch: &CopilotPatchEnvelope,
) -> (SchemaDoc, bool) {
    let mut next_doc = base_doc.clone();

    for nt_patch in &patch.upsert_node_types {
        upsert_node_type_header(&mut next_doc, nt_patch);
    }
    for prop_upsert in &patch.upsert_properties {
        upsert_node_property(&mut next_doc, prop_upsert);
    }
    for edge_patch in &patch.upsert_edge_types {
        upsert_edge_type(&mut next_doc, edge_patch);
    }
    for agg_upsert in &patch.upsert_agg_rules {
        upsert_node_agg_rule(&mut next_doc, agg_upsert);
    }
    for der_upsert in &patch.upsert_derived_rules {
        upsert_node_derived_rule(&mut next_doc, der_upsert);
    }

    let diff = diff_schema(base_doc, &next_doc);
    let mutated = !diff.is_empty() || base_doc != &next_doc;
    (next_doc, mutated)
}

fn ensure_target_node_type<'a>(
    doc: &'a mut SchemaDoc,
    node_type_name: &str,
) -> Option<&'a mut NodeTypeSpec> {
    if let Some(idx) = doc.node_types.iter().position(|n| n.name == node_type_name) {
        return doc.node_types.get_mut(idx);
    }
    let idx = doc.node_types.len();
    let col = u32::try_from(idx % 4).unwrap_or(0);
    let row = u32::try_from(idx / 4).unwrap_or(0);
    doc.node_types.push(NodeTypeSpec {
        name: String::from(node_type_name),
        display_name: String::from(node_type_name),
        pos_x: 120.0 + f64::from(col) * 280.0,
        pos_y: 140.0 + f64::from(row) * 220.0,
        maturity: DesignMaturity::Proposed,
        ..NodeTypeSpec::default()
    });
    doc.node_types.last_mut()
}

fn upsert_node_type_header(doc: &mut SchemaDoc, patch: &CopilotNodeTypePatch) {
    let name = patch.name.trim();
    if name.is_empty() {
        return;
    }
    if let Some(existing) = doc.node_types.iter_mut().find(|n| n.name == name) {
        let mut changed = false;
        if !patch.display_name.trim().is_empty() && existing.display_name != patch.display_name {
            existing.display_name.clone_from(&patch.display_name);
            changed = true;
        }
        if !patch.description.trim().is_empty() && existing.description != patch.description {
            existing.description.clone_from(&patch.description);
            changed = true;
        }
        if patch.semantic_layer_tier.is_some()
            && existing.semantic_layer_tier != patch.semantic_layer_tier
        {
            existing.semantic_layer_tier = patch.semantic_layer_tier;
            changed = true;
        }
        if patch.domain_group.is_some() && existing.domain_group != patch.domain_group {
            existing.domain_group.clone_from(&patch.domain_group);
            changed = true;
        }
        if changed {
            existing.maturity = DesignMaturity::Proposed;
        }
        return;
    }

    let idx = doc.node_types.len();
    let col = u32::try_from(idx % 4).unwrap_or(0);
    let row = u32::try_from(idx / 4).unwrap_or(0);
    let display_name = if patch.display_name.trim().is_empty() {
        String::from(name)
    } else {
        patch.display_name.clone()
    };
    doc.node_types.push(NodeTypeSpec {
        name: String::from(name),
        display_name,
        description: patch.description.clone(),
        pos_x: 120.0 + f64::from(col) * 280.0,
        pos_y: 140.0 + f64::from(row) * 220.0,
        maturity: DesignMaturity::Proposed,
        semantic_layer_tier: patch.semantic_layer_tier,
        domain_group: patch.domain_group.clone(),
        ..NodeTypeSpec::default()
    });
}

fn upsert_node_property(doc: &mut SchemaDoc, item: &CopilotNodePropertyUpsert) {
    let nt_name = item.node_type.trim();
    let prop_name = item.property.name.trim();
    if nt_name.is_empty() || prop_name.is_empty() {
        return;
    }
    let Some(nt) = ensure_target_node_type(doc, nt_name) else {
        return;
    };
    let normalized_unit = item
        .property
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(String::from);
    let display_name = if item.property.display_name.trim().is_empty() {
        String::from(prop_name)
    } else {
        item.property.display_name.clone()
    };

    if let Some(existing) = nt.properties.iter_mut().find(|p| p.name == prop_name) {
        let content_same = existing.display_name == display_name
            && existing.description == item.property.description
            && existing.unit == normalized_unit
            && existing.data_type == item.property.data_type
            && existing.kind == item.property.kind
            && existing.required == item.property.required
            && existing.pii_category == item.property.pii_category;
        if !content_same {
            existing.display_name = display_name;
            existing.description.clone_from(&item.property.description);
            existing.unit = normalized_unit;
            existing.data_type = item.property.data_type;
            existing.kind = item.property.kind;
            existing.required = item.property.required;
            existing.pii_category = item.property.pii_category;
            existing.maturity = DesignMaturity::Proposed;
        }
    } else {
        nt.properties.push(PropertySpec {
            name: String::from(prop_name),
            display_name,
            description: item.property.description.clone(),
            unit: normalized_unit,
            data_type: item.property.data_type,
            kind: item.property.kind,
            required: item.property.required,
            pii_category: item.property.pii_category,
            allowed_values: None,
            maturity: DesignMaturity::Proposed,
        });
    }
}

fn upsert_edge_type(doc: &mut SchemaDoc, patch: &CopilotEdgeTypePatch) {
    let name = patch.name.trim();
    if name.is_empty() {
        return;
    }
    let weight = if patch.impact_weight.is_finite()
        && patch.impact_weight > 0.0
        && patch.impact_weight <= 1.0
    {
        patch.impact_weight
    } else {
        0.7
    };

    if let Some(existing) = doc.edge_types.iter_mut().find(|e| {
        e.name == name
            && e.src_type_name == patch.src_type_name
            && e.dst_type_name == patch.dst_type_name
    }) {
        let same = existing.cardinality == patch.cardinality
            && (existing.impact_weight - weight).abs() <= 1e-9
            && existing.description == patch.description;
        if !same {
            existing.cardinality = patch.cardinality;
            existing.impact_weight = weight;
            existing.description.clone_from(&patch.description);
            existing.maturity = DesignMaturity::Proposed;
        }
    } else {
        doc.edge_types.push(EdgeTypeSpec {
            name: String::from(name),
            src_type_name: patch.src_type_name.clone(),
            dst_type_name: patch.dst_type_name.clone(),
            impact_weight: weight,
            cardinality: patch.cardinality,
            description: patch.description.clone(),
            maturity: DesignMaturity::Proposed,
            ..EdgeTypeSpec::default()
        });
    }
}

fn ensure_metric_property_exists(nt: &mut NodeTypeSpec, metric_name: &str) {
    if nt.properties.iter().any(|p| p.name == metric_name) {
        return;
    }
    nt.properties.push(PropertySpec {
        name: String::from(metric_name),
        display_name: String::from(metric_name),
        description: String::new(),
        unit: None,
        data_type: DataType::Float64,
        kind: PropertyKind::Metric,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
        maturity: DesignMaturity::Proposed,
    });
}

fn upsert_node_agg_rule(doc: &mut SchemaDoc, item: &CopilotNodeAggRuleUpsert) {
    let nt_name = item.node_type.trim();
    let target_metric = item.rule.target_metric.trim();
    if nt_name.is_empty() || target_metric.is_empty() {
        return;
    }
    let Some(nt) = ensure_target_node_type(doc, nt_name) else {
        return;
    };
    ensure_metric_property_exists(nt, target_metric);
    let clean_filter = item
        .rule
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(String::from);

    if let Some(existing) = nt
        .agg_rules
        .iter_mut()
        .find(|r| r.target_metric == target_metric)
    {
        let same = existing.op == item.rule.op
            && existing.via_link_type == item.rule.via_link_type
            && existing.direction == item.rule.direction
            && existing.source_type_name == item.rule.source_type_name
            && existing.source_metric == item.rule.source_metric
            && existing.filter == clean_filter
            && existing.weight_by_link == item.rule.weight_by_link;
        if !same {
            existing.op = item.rule.op;
            existing.via_link_type.clone_from(&item.rule.via_link_type);
            existing.direction = item.rule.direction;
            existing
                .source_type_name
                .clone_from(&item.rule.source_type_name);
            existing.source_metric.clone_from(&item.rule.source_metric);
            existing.filter = clean_filter;
            existing.weight_by_link = item.rule.weight_by_link;
            existing.maturity = DesignMaturity::Proposed;
        }
    } else {
        nt.agg_rules.push(AggRuleSpec {
            target_metric: String::from(target_metric),
            op: item.rule.op,
            via_link_type: item.rule.via_link_type.clone(),
            direction: item.rule.direction,
            source_type_name: item.rule.source_type_name.clone(),
            source_metric: item.rule.source_metric.clone(),
            filter: clean_filter,
            weight_by_link: item.rule.weight_by_link,
            maturity: DesignMaturity::Proposed,
        });
    }
}

fn upsert_node_derived_rule(doc: &mut SchemaDoc, item: &CopilotNodeDerivedRuleUpsert) {
    let nt_name = item.node_type.trim();
    let target_metric = item.rule.target_metric.trim();
    if nt_name.is_empty() || target_metric.is_empty() {
        return;
    }
    let Some(nt) = ensure_target_node_type(doc, nt_name) else {
        return;
    };
    ensure_metric_property_exists(nt, target_metric);

    if let Some(existing) = nt
        .derived_rules
        .iter_mut()
        .find(|r| r.target_metric == target_metric)
    {
        let same =
            existing.expression == item.rule.expression && existing.enabled == item.rule.enabled;
        if !same {
            existing.expression.clone_from(&item.rule.expression);
            existing.enabled = item.rule.enabled;
            existing.maturity = DesignMaturity::Proposed;
        }
    } else {
        nt.derived_rules.push(DerivedRuleSpec {
            target_metric: String::from(target_metric),
            expression: item.rule.expression.clone(),
            enabled: item.rule.enabled,
            maturity: DesignMaturity::Proposed,
        });
    }
}

/// Executes read-only `DIAGNOSE_ONLY` Copilot inspection with constrained decoding (`Purpose::Extract`, `temperature = 0.0`).
///
/// # Errors
/// Returns [`AppError`] if the structured LLM call fails or violates [`copilot_diagnosis_schema`].
pub async fn run_copilot_diagnose<P: AiPort>(
    llm: &P,
    input: &CopilotPromptInput<'_>,
) -> Result<CopilotDiagnosis, AppError> {
    let user_prompt = build_copilot_user_prompt(CopilotActionMode::DiagnoseOnly, input);
    let spec = copilot_diagnosis_schema();
    let val = llm
        .chat_structured(
            Purpose::Extract,
            STUDIO_COPILOT_DIAGNOSE_SYSTEM_PROMPT,
            &user_prompt,
            &spec,
        )
        .await?;
    serde_json::from_value(val).map_err(|err| {
        AppError::unprocessable(
            "STRUCTURED_OUTPUT_INVALID",
            format!("failed decoding CopilotDiagnosis: {err}"),
        )
    })
}

/// Executes `PATCH_DRAFT` Copilot constrained decoding and deterministically merges `PROPOSED` elements onto `input.schema_doc`.
///
/// # Errors
/// Returns [`AppError`] if the structured LLM call fails or violates [`copilot_patch_envelope_schema`].
pub async fn run_copilot_patch<P: AiPort>(
    llm: &P,
    input: &CopilotPromptInput<'_>,
) -> Result<CopilotPatchOutcome, AppError> {
    let user_prompt = build_copilot_user_prompt(CopilotActionMode::PatchDraft, input);
    let spec = copilot_patch_envelope_schema();
    let val = llm
        .chat_structured(
            Purpose::Extract,
            STUDIO_COPILOT_PATCH_SYSTEM_PROMPT,
            &user_prompt,
            &spec,
        )
        .await?;
    let envelope: CopilotPatchEnvelope = serde_json::from_value(val).map_err(|err| {
        AppError::unprocessable(
            "STRUCTURED_OUTPUT_INVALID",
            format!("failed decoding CopilotPatchEnvelope: {err}"),
        )
    })?;
    let (patched_doc, mutated) = apply_copilot_patch(input.schema_doc, &envelope);
    let diff = diff_schema(input.schema_doc, &patched_doc);
    Ok(CopilotPatchOutcome {
        diagnosis: envelope.diagnosis,
        patched_doc,
        diff,
        mutated,
    })
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use axpoint_domain::axhub::{ColumnProfile, DatasetStatus};
    use axpoint_domain::enums::SourceKind;
    use axpoint_domain::studio::{IssueSeverity, ValidationContext, validate_with_metrics};
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::structured::{is_strict_draft2020_12_schema, validate_with_jsonschema};
    use crate::{AiGateway, CassetteMode, EmbeddingConfig, LlmConfig};

    fn sample_step18_base_doc() -> SchemaDoc {
        SchemaDoc {
            node_types: vec![
                NodeTypeSpec {
                    name: String::from("Supplier"),
                    display_name: String::from("供应商"),
                    description: String::from("上游物料供应商"),
                    semantic_layer_tier: Some(SemanticLayerTier::StaticFact),
                    domain_group: Some(String::from("采购域")),
                    properties: vec![
                        PropertySpec {
                            name: String::from("tier_code"),
                            display_name: String::from("等级"),
                            data_type: DataType::String,
                            kind: PropertyKind::Attribute,
                            required: true,
                            ..PropertySpec::default()
                        },
                        PropertySpec {
                            name: String::from("lead_days"),
                            display_name: String::from("交付周期"),
                            unit: Some(String::from("天")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                    ],
                    ..NodeTypeSpec::default()
                },
                NodeTypeSpec {
                    name: String::from("Warehouse"),
                    display_name: String::from("区域仓"),
                    description: String::from("区域履约仓库"),
                    semantic_layer_tier: Some(SemanticLayerTier::CubeRollup),
                    domain_group: Some(String::from("履约域")),
                    properties: vec![
                        PropertySpec {
                            name: String::from("wh_region"),
                            display_name: String::from("大区"),
                            data_type: DataType::String,
                            kind: PropertyKind::Attribute,
                            required: true,
                            ..PropertySpec::default()
                        },
                        PropertySpec {
                            name: String::from("stock_qty"),
                            display_name: String::from("在库量"),
                            unit: Some(String::from("件")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                        PropertySpec {
                            name: String::from("avg_supplier_lead"),
                            display_name: String::from("上游平均交期"),
                            unit: Some(String::from("天")),
                            data_type: DataType::Float64,
                            kind: PropertyKind::Metric,
                            ..PropertySpec::default()
                        },
                    ],
                    agg_rules: vec![AggRuleSpec {
                        target_metric: String::from("avg_supplier_lead"),
                        op: AggFn::Avg,
                        via_link_type: String::from("SUPPLIES"),
                        direction: LinkDirection::In,
                        source_type_name: String::from("Supplier"),
                        source_metric: String::from("lead_days"),
                        ..AggRuleSpec::default()
                    }],
                    ..NodeTypeSpec::default()
                },
            ],
            edge_types: vec![EdgeTypeSpec {
                name: String::from("SUPPLIES"),
                src_type_name: String::from("Supplier"),
                dst_type_name: String::from("Warehouse"),
                cardinality: Cardinality::ManyToMany,
                impact_weight: 0.7,
                ..EdgeTypeSpec::default()
            }],
            mappings: Vec::new(),
        }
    }

    #[test]
    fn schemas_satisfy_strict_draft2020_12_grammar() {
        let diag_spec = copilot_diagnosis_schema();
        assert!(is_strict_draft2020_12_schema(&diag_spec.schema));

        let patch_spec = copilot_patch_envelope_schema();
        assert!(is_strict_draft2020_12_schema(&patch_spec.schema));

        let sample_diag = json!({
            "summary": "Warehouse 尚未绑定源数据集映射",
            "findings": [{
                "severity": "WARN",
                "target_path": "node_types[Warehouse].mappings",
                "rationale": "在库量 stock_qty 尚未配置入图同步规则",
                "suggested_fix": "补充 warehouses 数据集到 Warehouse 的 MappingSpec"
            }]
        });
        assert!(validate_with_jsonschema(&sample_diag.to_string(), &diag_spec.schema).is_ok());
    }

    fn sample_safety_stock_patch() -> CopilotPatchEnvelope {
        CopilotPatchEnvelope {
            diagnosis: CopilotDiagnosis {
                summary: String::from("为 Warehouse 新增安全库存与缺口派生公式"),
                findings: Vec::new(),
            },
            upsert_node_types: Vec::new(),
            upsert_properties: vec![
                CopilotNodePropertyUpsert {
                    node_type: String::from("Warehouse"),
                    property: CopilotPropertyPatch {
                        name: String::from("safety_stock"),
                        display_name: String::from("安全库存"),
                        description: String::from("最低安全库存水位"),
                        unit: Some(String::from("件")),
                        data_type: DataType::Float64,
                        kind: PropertyKind::Metric,
                        required: false,
                        pii_category: PiiCategory::None,
                    },
                },
                CopilotNodePropertyUpsert {
                    node_type: String::from("Warehouse"),
                    property: CopilotPropertyPatch {
                        name: String::from("shortage_gap"),
                        display_name: String::from("库存缺口"),
                        description: String::from("安全库存与当前在库量正缺口"),
                        unit: Some(String::from("件")),
                        data_type: DataType::Float64,
                        kind: PropertyKind::Metric,
                        required: false,
                        pii_category: PiiCategory::None,
                    },
                },
            ],
            upsert_edge_types: Vec::new(),
            upsert_agg_rules: Vec::new(),
            upsert_derived_rules: vec![CopilotNodeDerivedRuleUpsert {
                node_type: String::from("Warehouse"),
                rule: CopilotDerivedRulePatch {
                    target_metric: String::from("shortage_gap"),
                    expression: String::from("max(0, safety_stock - stock_qty)"),
                    enabled: true,
                },
            }],
        }
    }

    #[test]
    fn apply_patch_marks_new_items_proposed_and_preserves_confirmed_elements() {
        let base = sample_step18_base_doc();
        assert_eq!(base.unconfirmed_proposal_count(), 0);

        let patch = sample_safety_stock_patch();
        let (patched, mutated) = apply_copilot_patch(&base, &patch);
        assert!(mutated);
        assert!(patched.has_unconfirmed_proposals());
        assert_eq!(patched.unconfirmed_proposal_count(), 3);

        let wh_opt = patched.node_types.iter().find(|n| n.name == "Warehouse");
        assert!(wh_opt.is_some());
        if let Some(wh) = wh_opt {
            assert_eq!(wh.maturity, DesignMaturity::Confirmed);
            assert_eq!(
                wh.properties
                    .iter()
                    .find(|p| p.name == "wh_region")
                    .map(|p| p.maturity),
                Some(DesignMaturity::Confirmed)
            );
            assert_eq!(
                wh.properties
                    .iter()
                    .find(|p| p.name == "stock_qty")
                    .map(|p| p.maturity),
                Some(DesignMaturity::Confirmed)
            );
            assert_eq!(
                wh.properties
                    .iter()
                    .find(|p| p.name == "safety_stock")
                    .map(|p| p.maturity),
                Some(DesignMaturity::Proposed)
            );
            assert_eq!(
                wh.derived_rules
                    .iter()
                    .find(|r| r.target_metric == "shortage_gap")
                    .map(|r| r.maturity),
                Some(DesignMaturity::Proposed)
            );
        }

        let ctx = ValidationContext {
            subgraph_slug: String::from("dev_step18_studio"),
            ..ValidationContext::default()
        };
        let summary = validate_with_metrics(&patched, &ctx);
        assert!(
            !summary
                .issues
                .iter()
                .any(|i| i.severity == IssueSeverity::Error)
        );
        assert!(
            summary
                .issues
                .iter()
                .any(|i| i.code == "UNCONFIRMED_PROPOSAL")
        );
    }

    #[test]
    fn dataset_profile_strips_raw_sample_values() {
        let now = Utc::now();
        let ds = Dataset {
            id: Uuid::new_v4(),
            connection_id: None,
            name: String::from("warehouses"),
            display_name: String::from("仓库台账"),
            description: String::new(),
            source_kind: SourceKind::FileTable,
            source_ref: json!({}),
            sync_mode: axpoint_domain::enums::SyncMode::Snapshot,
            primary_key: vec![String::from("wh_code")],
            cursor_column: None,
            cursor_value: None,
            delete_column: None,
            schema_policy: axpoint_domain::axhub::SchemaPolicy::Additive,
            lake_schema: String::from("tenant_default"),
            lake_table: String::from("warehouses"),
            schema_json: vec![ColumnProfile {
                name: String::from("manager_phone"),
                ordinal: 0,
                data_type: DataType::String,
                nullable: false,
                null_count: 0,
                null_ratio: 0.0,
                distinct_estimate: 12,
                sample_values: vec![String::from("13800138000")],
                pii_category: PiiCategory::Phone,
                pii_hit_ratio: 1.0,
                source_comment: String::new(),
            }],
            schema_version: 1,
            row_count: 12,
            last_snapshot_id: None,
            last_synced_at: None,
            schedule_cron: None,
            continuous_stream: false,
            cdc_slot_meta: json!({}),
            tags: Vec::new(),
            owner_user_id: None,
            status: DatasetStatus::Ready,
            created_at: now,
            updated_at: now,
        };

        let sanitized = CopilotDatasetProfile::from_dataset(&ds);
        let focus = CopilotFocusContext {
            selected_node_type: Some(String::from("Warehouse")),
            selected_edge_type: None,
            active_tab: Some(String::from("properties")),
        };
        let prompt = build_copilot_user_prompt(
            CopilotActionMode::DiagnoseOnly,
            &CopilotPromptInput {
                subgraph_slug: "dev_step18_studio",
                schema_doc: &sample_step18_base_doc(),
                user_prompt: "检查敏感字段",
                focus: Some(&focus),
                datasets: &[sanitized],
                spec_overview: Some("## 1. 模型总览"),
            },
        );
        assert!(prompt.contains("manager_phone"));
        assert!(!prompt.contains("13800138000"));
        assert!(prompt.contains("## 1. 模型总览"));
    }

    #[tokio::test]
    async fn copilot_diagnose_and_patch_roundtrip_via_json_cassettes() -> Result<(), AppError> {
        let dir = env::temp_dir().join(format!(
            "axpoint_studio_copilot_cassette_{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).map_err(|err| AppError::Llm(err.to_string()))?;
        fs::write(
            dir.join("studio_copilot_diagnose.json"),
            STEP18_DIAG_CASSETTE_JSON,
        )
        .map_err(|err| AppError::Llm(err.to_string()))?;
        fs::write(
            dir.join("studio_copilot_patch.json"),
            STEP18_PATCH_CASSETTE_JSON,
        )
        .map_err(|err| AppError::Llm(err.to_string()))?;

        let gw = AiGateway::with_cassette(
            LlmConfig {
                api_base: String::from("http://127.0.0.1:9"),
                api_key: String::new(),
                model: String::from("offline"),
            },
            EmbeddingConfig {
                api_base: String::from("http://127.0.0.1:9"),
                api_key: String::new(),
                model: String::from("offline-embed"),
                configured_dim: Some(64),
            },
            &dir,
            CassetteMode::Replay,
        )?;

        let base_doc = sample_step18_base_doc();
        let focus = CopilotFocusContext {
            selected_node_type: Some(String::from("Warehouse")),
            selected_edge_type: None,
            active_tab: Some(String::from("properties")),
        };
        let diag = run_copilot_diagnose(
            &gw,
            &CopilotPromptInput {
                subgraph_slug: "dev_step18_studio",
                schema_doc: &base_doc,
                user_prompt: "完备度体检：检查 Warehouse 的指标与映射完备度",
                focus: Some(&focus),
                datasets: &[],
                spec_overview: None,
            },
        )
        .await?;
        assert_eq!(diag.findings.len(), 2);

        let patch_out = run_copilot_patch(
            &gw,
            &CopilotPromptInput {
                subgraph_slug: "dev_step18_studio",
                schema_doc: &base_doc,
                user_prompt: "为 Warehouse 增加安全库存指标 safety_stock（单位：件）与缺口派生公式 shortage_gap = max(0, safety_stock - stock_qty)",
                focus: Some(&focus),
                datasets: &[],
                spec_overview: None,
            },
        )
        .await?;
        assert!(patch_out.mutated);
        assert!(!patch_out.diff.is_empty());
        assert_eq!(patch_out.patched_doc.unconfirmed_proposal_count(), 3);

        fs::remove_dir_all(&dir).map_err(|err| AppError::Llm(err.to_string()))?;
        Ok(())
    }

    const STEP18_DIAG_CASSETTE_JSON: &str = r#"[
  {
    "match_all": ["Warehouse"],
    "response_schema_name": "axpoint_studio_copilot_diagnosis",
    "kind": "chat",
    "response": {
      "summary": "Warehouse 当前定义了在库量与上游平均交期，但尚未绑定源数据集映射且缺少安全库存水位指标。",
      "findings": [
        {
          "severity": "WARN",
          "target_path": "node_types[Warehouse].properties",
          "rationale": "缺少安全库存基准指标，无法直接评估在库量缺口风险。",
          "suggested_fix": "新增 safety_stock（单位：件）指标与缺口派生规则。"
        },
        {
          "severity": "INFO",
          "target_path": "node_types[Warehouse].mappings",
          "rationale": "Warehouse 为 CUBE_ROLLUP 汇总节点，基础库存字段仍建议绑定入图映射。",
          "suggested_fix": "关联仓库台账数据集配置 MappingSpec。"
        }
      ]
    }
  },
  {
    "match_all": ["xBlueStudio"],
    "response_schema_name": "axpoint_studio_copilot_diagnosis",
    "kind": "chat",
    "response": {
      "summary": "子图基础模型结构合法，建议进一步核对指标单位与源表映射完备度。",
      "findings": [
        {
          "severity": "INFO",
          "target_path": "schema_doc.mappings",
          "rationale": "检查核心业务实体是否均已配置主键列映射。",
          "suggested_fix": "为未映射主数据或事件实体补充数据集入图映射。"
        }
      ]
    }
  }
]"#;

    const STEP18_PATCH_CASSETTE_JSON: &str = r#"[
  {
    "match_all": ["Warehouse", "safety_stock", "shortage_gap"],
    "response_schema_name": "axpoint_studio_copilot_patch",
    "kind": "chat",
    "response": {
      "diagnosis": {
        "summary": "已为 Warehouse 补充安全库存指标 safety_stock、库存缺口指标 shortage_gap 及派生公式。",
        "findings": [
          {
            "severity": "INFO",
            "target_path": "node_types[Warehouse].derived_rules[shortage_gap]",
            "rationale": "新增要素已标记为 PROPOSED，确认提议后即可发布正式版本。",
            "suggested_fix": "核对单位与公式后执行提议确认。"
          }
        ]
      },
      "upsert_node_types": [],
      "upsert_properties": [
        {
          "node_type": "Warehouse",
          "property": {
            "name": "safety_stock",
            "display_name": "安全库存",
            "description": "区域仓安全库存水位",
            "unit": "件",
            "data_type": "FLOAT64",
            "kind": "METRIC",
            "required": false,
            "pii_category": "NONE"
          }
        },
        {
          "node_type": "Warehouse",
          "property": {
            "name": "shortage_gap",
            "display_name": "库存缺口",
            "description": "安全库存与当前在库量的正缺口",
            "unit": "件",
            "data_type": "FLOAT64",
            "kind": "METRIC",
            "required": false,
            "pii_category": "NONE"
          }
        }
      ],
      "upsert_edge_types": [],
      "upsert_agg_rules": [],
      "upsert_derived_rules": [
        {
          "node_type": "Warehouse",
          "rule": {
            "target_metric": "shortage_gap",
            "expression": "max(0, safety_stock - stock_qty)",
            "enabled": true
          }
        }
      ]
    }
  },
  {
    "match_all": ["Warehouse", "safety_stock"],
    "exclude_any": ["shortage_gap"],
    "response_schema_name": "axpoint_studio_copilot_patch",
    "kind": "chat",
    "response": {
      "diagnosis": {
        "summary": "已为 Warehouse 新增安全库存指标 safety_stock（单位：件）。",
        "findings": [
          {
            "severity": "INFO",
            "target_path": "node_types[Warehouse].properties[safety_stock]",
            "rationale": "新增指标已标记为 PROPOSED，待人工签收确认后可发布。",
            "suggested_fix": "确认 safety_stock 单位与精度后执行全部确认。"
          }
        ]
      },
      "upsert_node_types": [],
      "upsert_properties": [
        {
          "node_type": "Warehouse",
          "property": {
            "name": "safety_stock",
            "display_name": "安全库存",
            "description": "区域仓安全库存水位",
            "unit": "件",
            "data_type": "FLOAT64",
            "kind": "METRIC",
            "required": false,
            "pii_category": "NONE"
          }
        }
      ],
      "upsert_edge_types": [],
      "upsert_agg_rules": [],
      "upsert_derived_rules": []
    }
  }
]"#;

    #[tokio::test]
    async fn router_replays_step18_gate_diagnose_and_both_patch_prompts() -> Result<(), AppError> {
        let dir = env::temp_dir().join(format!(
            "axpoint_studio_copilot_router_{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).map_err(|err| AppError::Llm(err.to_string()))?;
        fs::write(
            dir.join("studio_copilot_diagnose.json"),
            STEP18_DIAG_CASSETTE_JSON,
        )
        .map_err(|err| AppError::Llm(err.to_string()))?;
        fs::write(
            dir.join("studio_copilot_patch.json"),
            STEP18_PATCH_CASSETTE_JSON,
        )
        .map_err(|err| AppError::Llm(err.to_string()))?;

        let cassette = crate::Cassette::open(&dir, CassetteMode::Replay)?;
        let llm_cfg = LlmConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("offline"),
        };
        let router = std::sync::Arc::new(
            crate::LlmRouter::new(llm_cfg.clone()).with_cassette(Some(cassette)),
        );
        let gw = AiGateway::new(
            llm_cfg,
            EmbeddingConfig {
                api_base: String::from("http://127.0.0.1:9"),
                api_key: String::new(),
                model: String::from("offline-embed"),
                configured_dim: Some(64),
            },
        )
        .with_router(router);

        let base_doc = sample_step18_base_doc();
        let focus_wh = CopilotFocusContext {
            selected_node_type: Some(String::from("Warehouse")),
            selected_edge_type: None,
            active_tab: Some(String::from("rules")),
        };

        let diag = run_copilot_diagnose(
            &gw,
            &CopilotPromptInput {
                subgraph_slug: "dev_step18_studio",
                schema_doc: &base_doc,
                user_prompt: "完备度体检：检查 Warehouse 的指标与映射完备度",
                focus: Some(&focus_wh),
                datasets: &[],
                spec_overview: None,
            },
        )
        .await?;
        assert_eq!(diag.findings.len(), 2);

        let patch1 = run_copilot_patch(
            &gw,
            &CopilotPromptInput {
                subgraph_slug: "dev_step18_studio",
                schema_doc: &base_doc,
                user_prompt: "为 Warehouse 增加安全库存指标 safety_stock（单位：件）与缺口派生公式 shortage_gap = max(0, safety_stock - stock_qty)",
                focus: Some(&focus_wh),
                datasets: &[],
                spec_overview: None,
            },
        )
        .await?;
        assert!(patch1.mutated);
        assert_eq!(patch1.patched_doc.unconfirmed_proposal_count(), 3);

        let patch2 = run_copilot_patch(
            &gw,
            &CopilotPromptInput {
                subgraph_slug: "dev_step18_studio",
                schema_doc: &base_doc,
                user_prompt: "为 Warehouse 增加安全库存指标 safety_stock（单位：件）",
                focus: Some(&focus_wh),
                datasets: &[],
                spec_overview: None,
            },
        )
        .await?;
        assert!(patch2.mutated);
        assert_eq!(patch2.patched_doc.unconfirmed_proposal_count(), 1);

        let ctx = ValidationContext {
            subgraph_slug: String::from("dev_step18_studio"),
            ..ValidationContext::default()
        };
        let mut confirmed_doc = patch2.patched_doc;
        confirmed_doc
            .confirm_proposals(&axpoint_domain::studio::ConfirmProposalsRequest::default());
        let val = validate_with_metrics(&confirmed_doc, &ctx);
        assert!(
            !val.issues
                .iter()
                .any(|i| i.severity == IssueSeverity::Error)
        );
        assert_eq!(confirmed_doc.unconfirmed_proposal_count(), 0);

        fs::remove_dir_all(&dir).map_err(|err| AppError::Llm(err.to_string()))?;
        Ok(())
    }
}
