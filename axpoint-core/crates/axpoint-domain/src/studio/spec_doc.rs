//! Deterministic 7-section subgraph specification compiler and `q1..q6` acceptance prompt generator (`04-STEP18` §18.4.2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::enums::{Cardinality, GroundingMode, SemanticLayerTier};
use crate::studio::doc::{NodeTypeSpec, PropertyKind, SchemaDoc};

/// Recommended acceptance query item (`q1..q6`) synthesized deterministically from a subgraph's ontology (`04-STEP18` §18.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptancePromptItem {
    /// Stable prompt identifier (`"q1"`..`"q6"`).
    pub id: String,
    /// Natural-language business question ready to run in `AxQuery`.
    pub question: String,
    /// Expected `AxQuery` grounding mode (`DOMAIN_OBJECT`, `SEMANTIC_METRIC`, `GUARDED_SQL`).
    pub expected_grounding_mode: GroundingMode,
    /// Target `NodeTypeSpec.name` identifiers involved.
    pub target_node_types: Vec<String>,
    /// Target metric property identifiers involved.
    pub target_metrics: Vec<String>,
}

/// Single structured section inside [`SubgraphSpecDoc`] (`sec_01_overview`..`sec_07_acceptance_prompts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDocSection {
    /// Canonical section key (`sec_01_overview`..`sec_07_acceptance_prompts`).
    pub key: String,
    /// Human-readable section heading.
    pub title: String,
    /// Concise summary line.
    pub summary: String,
    /// Structured row/card records for UI rendering.
    pub rows: Vec<serde_json::Value>,
}

/// Mapping sync status snapshot injected into Section 6 of [`SubgraphSpecDoc`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecMappingSyncMeta {
    /// Mapping name.
    pub mapping_name: String,
    /// Latest sync status (`SUCCEEDED`, `RUNNING`, `FAILED`, `NEVER`).
    pub last_status: String,
    /// Last synced row/node count.
    pub nodes_upserted: i64,
}

/// Input parameters for [`compile_subgraph_spec_doc`].
#[derive(Debug, Clone)]
pub struct SpecDocCompileInput<'a> {
    /// Subgraph slug.
    pub subgraph_slug: &'a str,
    /// Subgraph display name.
    pub subgraph_display_name: &'a str,
    /// Published version number (`0` when unpublished).
    pub version: i32,
    /// Optional draft UUID when compiled from a working draft.
    pub draft_id: Option<Uuid>,
    /// Positive validation checks passed (`pass_count`).
    pub pass_count: u32,
    /// Target `SchemaDoc`.
    pub doc: &'a SchemaDoc,
    /// Optional latest mapping run metadata keyed by `mapping.name`.
    pub mapping_sync: &'a BTreeMap<String, SpecMappingSyncMeta>,
}

/// Complete 7-section subgraph panoramic specification document (`GET /api/xblue/subgraphs/{slug}/spec-doc`, `04-STEP18` §18.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgraphSpecDoc {
    /// Subgraph slug.
    pub subgraph_slug: String,
    /// Subgraph display title.
    pub display_name: String,
    /// Published version number.
    pub version: i32,
    /// Draft UUID if compiled against a draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_id: Option<Uuid>,
    /// Positive completeness check count.
    pub pass_count: u32,
    /// Count of remaining `PROPOSED` elements awaiting human sign-off.
    pub unconfirmed_count: u32,
    /// Exactly 7 structured specification sections.
    pub sections: Vec<SpecDocSection>,
    /// Deterministic `q1..q6` acceptance prompt suite.
    pub acceptance_prompts: Vec<AcceptancePromptItem>,
    /// Clean Markdown rendering suitable for `POST /api/xblueview/exports` (`MARKDOWN` / `DOCX` / `PDF`).
    pub markdown: String,
}

/// Deterministically compiles the 7-section panoramic specification document and `q1..q6` acceptance prompts.
#[must_use]
pub fn compile_subgraph_spec_doc(input: &SpecDocCompileInput<'_>) -> SubgraphSpecDoc {
    let unconfirmed_count = input.doc.unconfirmed_proposal_count();
    let total_metrics: usize = input
        .doc
        .node_types
        .iter()
        .map(|nt| {
            nt.properties
                .iter()
                .filter(|p| p.kind == PropertyKind::Metric)
                .count()
        })
        .sum();
    let acceptance_prompts = build_acceptance_prompts(input.doc);

    let sec1 = build_sec_overview(input, total_metrics, unconfirmed_count);
    let sec2 = build_sec_semantic_layers(input.doc);
    let sec3 = build_sec_entity_dictionary(input.doc);
    let sec4 = build_sec_topology_matrix(input.doc);
    let sec5 = build_sec_compute_formulas(input.doc);
    let sec6 = build_sec_source_mappings(input.doc, input.mapping_sync);
    let sec7 = build_sec_acceptance(&acceptance_prompts);

    let sections = vec![sec1, sec2, sec3, sec4, sec5, sec6, sec7];
    let markdown =
        render_spec_markdown(input, total_metrics, unconfirmed_count, &acceptance_prompts);

    SubgraphSpecDoc {
        subgraph_slug: String::from(input.subgraph_slug),
        display_name: String::from(input.subgraph_display_name),
        version: input.version,
        draft_id: input.draft_id,
        pass_count: input.pass_count,
        unconfirmed_count,
        sections,
        acceptance_prompts,
        markdown,
    }
}

fn build_sec_overview(
    input: &SpecDocCompileInput<'_>,
    total_metrics: usize,
    unconfirmed_count: u32,
) -> SpecDocSection {
    let state_label = if input.draft_id.is_some() {
        "DRAFT"
    } else {
        "PUBLISHED"
    };
    SpecDocSection {
        key: String::from("sec_01_overview"),
        title: String::from("模型总览与健康度"),
        summary: format!(
            "节点类型 {} · 关系类型 {} · 指标 {} · 通过项 {}",
            input.doc.node_types.len(),
            input.doc.edge_types.len(),
            total_metrics,
            input.pass_count
        ),
        rows: vec![json!({
            "subgraph_slug": input.subgraph_slug,
            "display_name": input.subgraph_display_name,
            "state": state_label,
            "version": input.version,
            "node_type_count": input.doc.node_types.len(),
            "edge_type_count": input.doc.edge_types.len(),
            "metric_count": total_metrics,
            "mapping_count": input.doc.mappings.len(),
            "pass_count": input.pass_count,
            "unconfirmed_count": unconfirmed_count,
        })],
    }
}

fn build_sec_semantic_layers(doc: &SchemaDoc) -> SpecDocSection {
    let rows: Vec<serde_json::Value> = doc
        .node_types
        .iter()
        .map(|nt| {
            let tier = nt
                .semantic_layer_tier
                .map_or("UNCLASSIFIED", SemanticLayerTier::as_str);
            let group = nt.domain_group.as_deref().unwrap_or("默认域");
            json!({
                "node_type": nt.name,
                "display_name": nt.display_name,
                "domain_group": group,
                "semantic_layer_tier": tier,
                "maturity": nt.maturity.as_str(),
            })
        })
        .collect();
    SpecDocSection {
        key: String::from("sec_02_semantic_layers"),
        title: String::from("业务域与语义分层矩阵"),
        summary: format!("共覆盖 {} 个领域实体分层定义", rows.len()),
        rows,
    }
}

fn build_sec_entity_dictionary(doc: &SchemaDoc) -> SpecDocSection {
    let mut rows = Vec::new();
    for nt in &doc.node_types {
        for prop in &nt.properties {
            rows.push(json!({
                "node_type": nt.name,
                "node_display_name": nt.display_name,
                "title_template": nt.title_template,
                "property": prop.name,
                "display_name": prop.display_name,
                "kind": prop.kind.as_str(),
                "data_type": prop.data_type.as_str(),
                "required": prop.required,
                "unit": prop.unit,
                "pii_category": prop.pii_category.as_str(),
                "maturity": prop.maturity.as_str(),
            }));
        }
    }
    SpecDocSection {
        key: String::from("sec_03_entity_dictionary"),
        title: String::from("实体、属性与指标字典"),
        summary: format!("共登记 {} 项属性与指标字段", rows.len()),
        rows,
    }
}

fn build_sec_topology_matrix(doc: &SchemaDoc) -> SpecDocSection {
    let rows: Vec<serde_json::Value> = doc
        .edge_types
        .iter()
        .map(|et| {
            json!({
                "edge_type": et.name,
                "src_type_name": et.src_type_name,
                "dst_type_name": et.dst_type_name,
                "cardinality": cardinality_display(et.cardinality),
                "impact_weight": et.impact_weight,
                "maturity": et.maturity.as_str(),
            })
        })
        .collect();
    SpecDocSection {
        key: String::from("sec_04_topology_matrix"),
        title: String::from("拓扑关系与因果传导"),
        summary: format!("共定义 {} 条有向业务关系边", rows.len()),
        rows,
    }
}

fn build_sec_compute_formulas(doc: &SchemaDoc) -> SpecDocSection {
    let mut rows = Vec::new();
    for nt in &doc.node_types {
        for agg in &nt.agg_rules {
            rows.push(json!({
                "node_type": nt.name,
                "rule_kind": "AGG",
                "target_metric": agg.target_metric,
                "formula": format!(
                    "{}({}.{} via {} [{}])",
                    agg.op.as_str(),
                    agg.source_type_name,
                    agg.source_metric,
                    agg.via_link_type,
                    agg.direction.as_str()
                ),
                "filter": agg.filter,
                "weight_by_link": agg.weight_by_link,
                "maturity": agg.maturity.as_str(),
            }));
        }
        for der in &nt.derived_rules {
            rows.push(json!({
                "node_type": nt.name,
                "rule_kind": "DERIVED",
                "target_metric": der.target_metric,
                "formula": der.expression,
                "enabled": der.enabled,
                "maturity": der.maturity.as_str(),
            }));
        }
    }
    SpecDocSection {
        key: String::from("sec_05_compute_formulas"),
        title: String::from("跨边聚合与节点内派生公式"),
        summary: format!("共定义 {} 条确定性计算规则", rows.len()),
        rows,
    }
}

fn build_sec_source_mappings(
    doc: &SchemaDoc,
    mapping_sync: &BTreeMap<String, SpecMappingSyncMeta>,
) -> SpecDocSection {
    let rows: Vec<serde_json::Value> = doc
        .mappings
        .iter()
        .map(|m| {
            let sync_meta = mapping_sync.get(&m.name);
            json!({
                "mapping_name": m.name,
                "dataset": m.dataset,
                "node_type": m.node_type,
                "key_column": m.key_column,
                "link_count": m.links.len(),
                "last_status": sync_meta.map_or("NEVER", |s| s.last_status.as_str()),
                "nodes_upserted": sync_meta.map_or(0, |s| s.nodes_upserted),
            })
        })
        .collect();
    SpecDocSection {
        key: String::from("sec_06_source_mappings"),
        title: String::from("数据湖映射与入图规则"),
        summary: format!("共绑定 {} 条数据集入图规则", rows.len()),
        rows,
    }
}

fn build_sec_acceptance(prompts: &[AcceptancePromptItem]) -> SpecDocSection {
    let rows: Vec<serde_json::Value> = prompts
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "question": p.question,
                "expected_grounding_mode": p.expected_grounding_mode.as_str(),
                "target_node_types": p.target_node_types,
                "target_metrics": p.target_metrics,
            })
        })
        .collect();
    SpecDocSection {
        key: String::from("sec_07_acceptance_prompts"),
        title: String::from("推荐验收问数集"),
        summary: format!("内置 {} 条标准领域验收提问", rows.len()),
        rows,
    }
}

fn build_acceptance_prompts(doc: &SchemaDoc) -> Vec<AcceptancePromptItem> {
    let primary_nt = doc.node_types.first();
    let secondary_nt = doc.node_types.get(1).or(primary_nt);
    let primary_name = primary_nt.map_or("Entity", |n| n.name.as_str());
    let primary_label = primary_nt.map_or("业务节点", |n| n.display_name.as_str());
    let secondary_name = secondary_nt.map_or(primary_name, |n| n.name.as_str());
    let secondary_label = secondary_nt.map_or(primary_label, |n| n.display_name.as_str());

    let primary_attr = primary_nt
        .and_then(|nt| first_attr_label(nt))
        .unwrap_or("状态");
    let (m1_name, m1_label) = primary_nt
        .and_then(first_metric_pair)
        .or_else(|| secondary_nt.and_then(first_metric_pair))
        .unwrap_or(("value", "核心指标"));
    let (m2_name, m2_label) = secondary_nt
        .and_then(second_or_first_metric_pair)
        .unwrap_or((m1_name, m1_label));

    let edge_info = doc.edge_types.first();
    let edge_src = edge_info.map_or(primary_name, |e| e.src_type_name.as_str());
    let edge_dst = edge_info.map_or(secondary_name, |e| e.dst_type_name.as_str());
    let edge_name = edge_info.map_or("关联关系", |e| e.name.as_str());

    vec![
        AcceptancePromptItem {
            id: String::from("q1"),
            question: format!("按{primary_attr}筛选并列出{primary_label}的{m1_label}明细"),
            expected_grounding_mode: GroundingMode::DomainObject,
            target_node_types: vec![String::from(primary_name)],
            target_metrics: vec![String::from(m1_name)],
        },
        AcceptancePromptItem {
            id: String::from("q2"),
            question: format!("按{primary_attr}汇总统计{primary_label}的{m1_label}"),
            expected_grounding_mode: GroundingMode::SemanticMetric,
            target_node_types: vec![String::from(primary_name)],
            target_metrics: vec![String::from(m1_name)],
        },
        AcceptancePromptItem {
            id: String::from("q3"),
            question: format!(
                "关联 {edge_src} 与 {edge_dst}（沿 {edge_name}）对比{m1_label}与{m2_label}"
            ),
            expected_grounding_mode: GroundingMode::GuardedSql,
            target_node_types: unique_pair(edge_src, edge_dst),
            target_metrics: unique_pair(m1_name, m2_name),
        },
        AcceptancePromptItem {
            id: String::from("q4"),
            question: format!("查询{secondary_label}中{m2_label}排名前 10 的对象"),
            expected_grounding_mode: GroundingMode::DomainObject,
            target_node_types: vec![String::from(secondary_name)],
            target_metrics: vec![String::from(m2_name)],
        },
        AcceptancePromptItem {
            id: String::from("q5"),
            question: format!("排查{secondary_label}中{m2_label}异常偏低或存在缺口的记录"),
            expected_grounding_mode: GroundingMode::GuardedSql,
            target_node_types: vec![String::from(secondary_name)],
            target_metrics: vec![String::from(m2_name)],
        },
        AcceptancePromptItem {
            id: String::from("q6"),
            question: format!(
                "穿透分析 {edge_src} 波动沿 {edge_name} 对下游 {edge_dst} 的影响范围"
            ),
            expected_grounding_mode: GroundingMode::DomainObject,
            target_node_types: unique_pair(edge_src, edge_dst),
            target_metrics: unique_pair(m1_name, m2_name),
        },
    ]
}

fn first_attr_label(nt: &NodeTypeSpec) -> Option<&str> {
    nt.properties
        .iter()
        .find(|p| p.kind == PropertyKind::Attribute)
        .map(|p| {
            if p.display_name.trim().is_empty() {
                p.name.as_str()
            } else {
                p.display_name.as_str()
            }
        })
}

fn first_metric_pair(nt: &NodeTypeSpec) -> Option<(&str, &str)> {
    nt.properties
        .iter()
        .find(|p| p.kind == PropertyKind::Metric)
        .map(|p| {
            let label = if p.display_name.trim().is_empty() {
                p.name.as_str()
            } else {
                p.display_name.as_str()
            };
            (p.name.as_str(), label)
        })
}

fn second_or_first_metric_pair(nt: &NodeTypeSpec) -> Option<(&str, &str)> {
    let mut metrics = nt
        .properties
        .iter()
        .filter(|p| p.kind == PropertyKind::Metric);
    let first = metrics.next()?;
    let chosen = metrics.next().unwrap_or(first);
    let label = if chosen.display_name.trim().is_empty() {
        chosen.name.as_str()
    } else {
        chosen.display_name.as_str()
    };
    Some((chosen.name.as_str(), label))
}

fn unique_pair(a: &str, b: &str) -> Vec<String> {
    if a == b {
        vec![String::from(a)]
    } else {
        vec![String::from(a), String::from(b)]
    }
}

const fn cardinality_display(c: Cardinality) -> &'static str {
    match c {
        Cardinality::OneToOne => "1:1",
        Cardinality::OneToMany => "1:N",
        Cardinality::ManyToOne => "N:1",
        Cardinality::ManyToMany => "N:M",
    }
}

fn render_spec_markdown(
    input: &SpecDocCompileInput<'_>,
    total_metrics: usize,
    unconfirmed_count: u32,
    prompts: &[AcceptancePromptItem],
) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(64);
    lines.push(format!(
        "# {}（{}）模型全景规格书",
        input.subgraph_display_name, input.subgraph_slug
    ));
    lines.push(String::new());
    lines.push(String::from("## 1. 模型总览与健康度"));
    lines.push(String::new());
    lines.push(String::from(
        "| 子图标识 | 版本 | 节点类型数 | 关系数 | 指标数 | 通过项 | 待确认提议 |",
    ));
    lines.push(String::from(
        "| :--- | :--- | :--- | :--- | :--- | :--- | :--- |",
    ));
    lines.push(format!(
        "| `{}` | `v{}` | {} | {} | {} | {} | {} |",
        input.subgraph_slug,
        input.version,
        input.doc.node_types.len(),
        input.doc.edge_types.len(),
        total_metrics,
        input.pass_count,
        unconfirmed_count
    ));
    lines.push(String::new());

    lines.push(String::from("## 2. 业务域与语义分层矩阵"));
    lines.push(String::new());
    for nt in &input.doc.node_types {
        let tier = nt
            .semantic_layer_tier
            .map_or("UNCLASSIFIED", SemanticLayerTier::as_str);
        let group = nt.domain_group.as_deref().unwrap_or("默认域");
        lines.push(format!(
            "- **{}**（`{}`）：业务域 `{}` · 分层 `{}` · 成熟度 `{}`",
            nt.display_name,
            nt.name,
            group,
            tier,
            nt.maturity.as_str()
        ));
    }
    lines.push(String::new());

    lines.push(String::from("## 3. 实体、属性与指标字典"));
    lines.push(String::new());
    for nt in &input.doc.node_types {
        lines.push(format!("### {}（`{}`）", nt.display_name, nt.name));
        lines.push(String::new());
        for prop in &nt.properties {
            let unit_str = prop.unit.as_deref().unwrap_or("-");
            lines.push(format!(
                "- `{}`（{}）：`{}` / `{}` · 单位 `{}` · 脱敏 `{}`",
                prop.name,
                prop.display_name,
                prop.kind.as_str(),
                prop.data_type.as_str(),
                unit_str,
                prop.pii_category.as_str()
            ));
        }
        lines.push(String::new());
    }

    lines.push(String::from("## 4. 拓扑关系与因果传导"));
    lines.push(String::new());
    for et in &input.doc.edge_types {
        lines.push(format!(
            "- `{}`：`{}` → `{}`（{}，传导权重 `{:.2}`）",
            et.name,
            et.src_type_name,
            et.dst_type_name,
            cardinality_display(et.cardinality),
            et.impact_weight
        ));
    }
    lines.push(String::new());
    append_spec_markdown_formulas_and_prompts(&mut lines, input.doc, prompts);
    lines.join("\n")
}

fn append_spec_markdown_formulas_and_prompts(
    lines: &mut Vec<String>,
    doc: &SchemaDoc,
    prompts: &[AcceptancePromptItem],
) {
    lines.push(String::from("## 5. 跨边聚合与节点内派生公式"));
    lines.push(String::new());
    for nt in &doc.node_types {
        for agg in &nt.agg_rules {
            lines.push(format!(
                "- `{}.{}` = `{}({}.{} via {})`",
                nt.name,
                agg.target_metric,
                agg.op.as_str(),
                agg.source_type_name,
                agg.source_metric,
                agg.via_link_type
            ));
        }
        for der in &nt.derived_rules {
            lines.push(format!(
                "- `{}.{}` = `{}`",
                nt.name, der.target_metric, der.expression
            ));
        }
    }
    lines.push(String::new());

    lines.push(String::from("## 6. 数据湖映射与入图规则"));
    lines.push(String::new());
    for m in &doc.mappings {
        lines.push(format!(
            "- `{}`：数据集 `{}`（主键 `{}`）→ 节点 `{}`",
            m.name, m.dataset, m.key_column, m.node_type
        ));
    }
    lines.push(String::new());

    lines.push(String::from("## 7. 推荐验收问数集"));
    lines.push(String::new());
    for p in prompts {
        lines.push(format!(
            "- **{}**（`{}`）：{}",
            p.id,
            p.expected_grounding_mode.as_str(),
            p.question
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enums::DataType;
    use crate::studio::doc::PropertySpec;

    #[test]
    fn compiles_seven_sections_and_six_acceptance_prompts() {
        let doc = SchemaDoc {
            node_types: vec![NodeTypeSpec {
                name: String::from("Warehouse"),
                display_name: String::from("区域仓"),
                semantic_layer_tier: Some(SemanticLayerTier::CubeRollup),
                domain_group: Some(String::from("履约域")),
                properties: vec![
                    PropertySpec {
                        name: String::from("region"),
                        display_name: String::from("大区"),
                        data_type: DataType::String,
                        kind: PropertyKind::Attribute,
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
                ],
                ..NodeTypeSpec::default()
            }],
            edge_types: Vec::new(),
            mappings: Vec::new(),
        };
        let sync = BTreeMap::new();
        let spec = compile_subgraph_spec_doc(&SpecDocCompileInput {
            subgraph_slug: "main",
            subgraph_display_name: "企业主图",
            version: 1,
            draft_id: None,
            pass_count: 4,
            doc: &doc,
            mapping_sync: &sync,
        });
        assert_eq!(spec.sections.len(), 7);
        assert_eq!(spec.acceptance_prompts.len(), 6);
        assert_eq!(
            spec.acceptance_prompts.first().map(|p| p.id.as_str()),
            Some("q1")
        );
        assert!(spec.markdown.contains("## 7. 推荐验收问数集"));
    }
}
