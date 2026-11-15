//! Pure `xBlueStudio` schema validation engine implementing all 10 rules in `06-STEP05-06` §05.3
//! and the Step 06 §06.1 deterministic expression syntax/type checker.

use std::collections::{BTreeMap, BTreeSet};
use std::iter::Peekable;
use std::str::Chars;

use crate::enums::{Cardinality, DataType};
use crate::studio::doc::{
    AggRuleSpec, DatasetValidationMeta, IssueSeverity, LinkDirection, MappingSpec, NodeTypeSpec,
    PropertyKind, PropertySpec, SchemaDoc, ValidationContext, ValidationIssue,
};

const RESERVED_PROPERTY_NAMES: &[&str] = &[
    "id",
    "tenant_id",
    "node_key",
    "title",
    "metrics",
    "properties",
];

const MAX_EXPR_CHARS: usize = 2_000;
const MAX_EXPR_DEPTH: usize = 64;
const MAX_CALL_ARGS: usize = 16;

/// Validates a [`SchemaDoc`] against [`ValidationContext`] according to `06-STEP05-06` §05.3 (rules 1–10).
///
/// Pure function with zero I/O. If the returned vector contains any [`IssueSeverity::Error`],
/// schema publication must be blocked (`400 SCHEMA_INVALID`).
#[must_use]
pub fn validate(doc: &SchemaDoc, ctx: &ValidationContext) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let effective_types = build_effective_node_types(doc, ctx);

    validate_node_type_names_and_props(doc, &mut issues);
    validate_edge_types(doc, ctx, &effective_types, &mut issues);
    let dep_graph = validate_rules_and_build_deps(doc, &effective_types, &mut issues);
    detect_metric_cycles(&dep_graph, &mut issues);
    validate_single_producer(doc, &mut issues);
    validate_mappings(doc, ctx, &effective_types, &mut issues);
    validate_breaking_changes(doc, ctx, &mut issues);

    issues
}

/// Returns `true` when changing an existing property's [`DataType`] from `from` to `to` is lossless
/// (`06-STEP05-06` §05.3 rule #10).
#[must_use]
pub const fn is_lossless_widening(from: DataType, to: DataType) -> bool {
    if matches!(
        (from, to),
        (DataType::String, DataType::String)
            | (DataType::Int64, DataType::Int64)
            | (DataType::Float64, DataType::Float64)
            | (DataType::Decimal, DataType::Decimal)
            | (DataType::Boolean, DataType::Boolean)
            | (DataType::Date, DataType::Date)
            | (DataType::Timestamp, DataType::Timestamp)
            | (DataType::Binary, DataType::Binary)
            | (DataType::Json, DataType::Json)
            | (DataType::List, DataType::List)
            | (DataType::Struct, DataType::Struct)
    ) {
        return true;
    }
    matches!(
        (from, to),
        (
            DataType::Int64,
            DataType::Float64 | DataType::Decimal | DataType::String | DataType::Json
        ) | (
            DataType::Float64,
            DataType::Decimal | DataType::String | DataType::Json
        ) | (
            DataType::Decimal,
            DataType::Float64 | DataType::String | DataType::Json
        ) | (
            DataType::Boolean | DataType::Timestamp,
            DataType::String | DataType::Json
        ) | (
            DataType::Date,
            DataType::Timestamp | DataType::String | DataType::Json
        )
    )
}

/// Returns `true` when a dataset column of type `src` can be coerced into a node property/metric of type `dst`
/// (`06-STEP05-06` §05.3 rule #8).
#[must_use]
pub const fn is_mapping_type_convertible(src: DataType, dst: DataType) -> bool {
    if is_lossless_widening(src, dst) {
        return true;
    }
    matches!(
        (src, dst),
        (_, DataType::String | DataType::Json)
            | (
                DataType::Int64 | DataType::Float64 | DataType::Decimal,
                DataType::Int64 | DataType::Float64 | DataType::Decimal
            )
            | (
                DataType::Date | DataType::Timestamp | DataType::String,
                DataType::Date | DataType::Timestamp
            )
    )
}

fn build_effective_node_types<'a>(
    doc: &'a SchemaDoc,
    ctx: &'a ValidationContext,
) -> BTreeMap<String, &'a NodeTypeSpec> {
    let mut map = BTreeMap::new();
    if ctx.subgraph_slug != "main" {
        for (name, spec) in &ctx.main_node_types {
            map.insert(name.clone(), spec);
        }
    }
    for nt in &doc.node_types {
        map.insert(nt.name.clone(), nt);
    }
    map
}

fn push_err(issues: &mut Vec<ValidationIssue>, path: String, code: &str, message: String) {
    issues.push(ValidationIssue {
        severity: IssueSeverity::Error,
        path,
        code: String::from(code),
        message,
    });
}

fn push_warn(issues: &mut Vec<ValidationIssue>, path: String, code: &str, message: String) {
    issues.push(ValidationIssue {
        severity: IssueSeverity::Warn,
        path,
        code: String::from(code),
        message,
    });
}

fn is_pascal_node_type_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_uppercase() {
        return false;
    }
    bytes.iter().all(u8::is_ascii_alphanumeric)
}

fn is_upper_snake_edge_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_uppercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn is_snake_prop_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn is_hex_color(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 7 || bytes.first().copied() != Some(b'#') {
        return false;
    }
    bytes.iter().skip(1).all(u8::is_ascii_hexdigit)
}

fn is_valid_mapping_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_title_template(
    template: &str,
    declared_props: &BTreeSet<String>,
) -> Result<(), String> {
    if template.trim().is_empty() {
        return Err(String::from("title_template must not be empty"));
    }
    let mut chars = template.chars();
    while let Some(ch) = chars.next() {
        if ch == '}' {
            return Err(String::from("unmatched '}' in title_template"));
        }
        if ch == '{' {
            let mut token = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '{' {
                    return Err(String::from("nested '{' in title_template"));
                }
                if inner == '}' {
                    closed = true;
                    break;
                }
                token.push(inner);
            }
            if !closed {
                return Err(String::from("unclosed '{' in title_template"));
            }
            if token == "node_key" {
                continue;
            }
            if let Some(prop_name) = token.strip_prefix("prop.") {
                if declared_props.contains(prop_name) {
                    continue;
                }
                return Err(format!(
                    "title_template references undeclared property 'prop.{prop_name}'"
                ));
            }
            return Err(format!(
                "title_template placeholder '{{{token}}}' is invalid; only {{node_key}} and {{prop.<name>}} are allowed"
            ));
        }
    }
    Ok(())
}

fn validate_property_list(
    props: &[PropertySpec],
    owner_prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    for (p_idx, prop) in props.iter().enumerate() {
        let p_path = format!("{owner_prefix}.properties[{p_idx}].name");
        if !is_snake_prop_name(&prop.name) {
            push_err(
                issues,
                p_path,
                "NAME_INVALID",
                format!(
                    "property name '{}' must match ^[a-z][a-z0-9_]{{0,62}}$",
                    prop.name
                ),
            );
            continue;
        }
        if RESERVED_PROPERTY_NAMES.contains(&prop.name.as_str()) {
            push_err(
                issues,
                p_path,
                "NAME_INVALID",
                format!("property name '{}' is a reserved system field", prop.name),
            );
            continue;
        }
        if !seen.insert(prop.name.clone()) {
            push_err(
                issues,
                p_path,
                "NAME_TAKEN",
                format!("duplicate property name '{}'", prop.name),
            );
        }
    }
    seen
}

fn validate_node_type_names_and_props(doc: &SchemaDoc, issues: &mut Vec<ValidationIssue>) {
    let mut seen_nodes = BTreeSet::new();
    for (idx, nt) in doc.node_types.iter().enumerate() {
        let prefix = format!("node_types[{idx}]");
        if !is_pascal_node_type_name(&nt.name) {
            push_err(
                issues,
                format!("{prefix}.name"),
                "NAME_INVALID",
                format!(
                    "node type name '{}' must match ^[A-Z][A-Za-z0-9]{{0,62}}$",
                    nt.name
                ),
            );
        } else if !seen_nodes.insert(nt.name.clone()) {
            push_err(
                issues,
                format!("{prefix}.name"),
                "NAME_TAKEN",
                format!("duplicate node type name '{}'", nt.name),
            );
        }

        let dn_chars = nt.display_name.trim().chars().count();
        if !(1..=64).contains(&dn_chars) {
            push_err(
                issues,
                format!("{prefix}.display_name"),
                "NAME_INVALID",
                String::from("display_name must be between 1 and 64 characters"),
            );
        }

        if !is_hex_color(&nt.color) {
            push_err(
                issues,
                format!("{prefix}.color"),
                "NAME_INVALID",
                format!("color '{}' must match ^#[0-9A-Fa-f]{{6}}$", nt.color),
            );
        }

        let declared_props = validate_property_list(&nt.properties, &prefix, issues);
        if let Err(msg) = validate_title_template(&nt.title_template, &declared_props) {
            push_err(
                issues,
                format!("{prefix}.title_template"),
                "NAME_INVALID",
                msg,
            );
        }
    }
}

fn validate_edge_types(
    doc: &SchemaDoc,
    ctx: &ValidationContext,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut seen_triples = BTreeSet::new();
    for (idx, edge) in doc.edge_types.iter().enumerate() {
        let prefix = format!("edge_types[{idx}]");
        if is_upper_snake_edge_name(&edge.name) {
            let triple = (
                edge.name.clone(),
                edge.src_type_name.clone(),
                edge.dst_type_name.clone(),
            );
            if !seen_triples.insert(triple) {
                push_err(
                    issues,
                    format!("{prefix}.name"),
                    "NAME_TAKEN",
                    format!(
                        "duplicate edge type '{}' between '{}' and '{}'",
                        edge.name, edge.src_type_name, edge.dst_type_name
                    ),
                );
            }
        } else {
            push_err(
                issues,
                format!("{prefix}.name"),
                "NAME_INVALID",
                format!(
                    "edge type name '{}' must match ^[A-Z][A-Z0-9_]{{0,62}}$",
                    edge.name
                ),
            );
        }

        let _ = validate_property_list(&edge.properties, &prefix, issues);

        if !effective_types.contains_key(&edge.src_type_name) {
            push_err(
                issues,
                format!("{prefix}.src_type_name"),
                "TYPE_NOT_FOUND",
                format!("source node type '{}' does not exist", edge.src_type_name),
            );
        }
        if !effective_types.contains_key(&edge.dst_type_name) {
            push_err(
                issues,
                format!("{prefix}.dst_type_name"),
                "TYPE_NOT_FOUND",
                format!(
                    "destination node type '{}' does not exist",
                    edge.dst_type_name
                ),
            );
        }

        if !edge.impact_weight.is_finite() || edge.impact_weight <= 0.0 || edge.impact_weight > 1.0
        {
            push_err(
                issues,
                format!("{prefix}.impact_weight"),
                "WEIGHT_OUT_OF_RANGE",
                format!("impact_weight {} must be in (0.0, 1.0]", edge.impact_weight),
            );
        }

        if matches!(
            edge.cardinality,
            Cardinality::OneToMany | Cardinality::OneToOne
        ) && (ctx.cardinality_violations.contains(&edge.name)
            || ctx.cardinality_violations.contains(&format!(
                "{}:{}->{}",
                edge.name, edge.src_type_name, edge.dst_type_name
            )))
        {
            push_warn(
                issues,
                format!("{prefix}.cardinality"),
                "CARDINALITY_VIOLATION",
                format!(
                    "existing runtime links violate {} cardinality on '{}'",
                    edge.cardinality, edge.name
                ),
            );
        }
    }
}

type MetricNode = (String, String);
type DepEdgeMap = BTreeMap<MetricNode, Vec<(MetricNode, String)>>;

fn build_scope_for_type(nt: &NodeTypeSpec) -> (BTreeSet<String>, BTreeMap<String, DataType>) {
    let mut metrics = BTreeSet::new();
    let mut attrs = BTreeMap::new();
    for p in &nt.properties {
        match p.kind {
            PropertyKind::Metric => {
                metrics.insert(p.name.clone());
            }
            PropertyKind::Attribute => {
                attrs.insert(p.name.clone(), p.data_type);
            }
        }
    }
    (metrics, attrs)
}

fn validate_agg_rule(
    doc: &SchemaDoc,
    owner: (&NodeTypeSpec, &BTreeSet<String>),
    rule: &AggRuleSpec,
    rule_path: &str,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    dep_graph: &mut DepEdgeMap,
    issues: &mut Vec<ValidationIssue>,
) {
    let (nt, this_metrics) = owner;
    if !this_metrics.contains(&rule.target_metric) {
        push_err(
            issues,
            format!("{rule_path}.target_metric"),
            "AGG_REF_INVALID",
            format!(
                "target_metric '{}' is not a METRIC on '{}'",
                rule.target_metric, nt.name
            ),
        );
    }

    let edge_matches = !rule.via_link_type.trim().is_empty()
        && doc.edge_types.iter().any(|e| {
            if e.name != rule.via_link_type {
                return false;
            }
            match rule.direction {
                LinkDirection::Out => {
                    e.src_type_name == nt.name && e.dst_type_name == rule.source_type_name
                }
                LinkDirection::In => {
                    e.dst_type_name == nt.name && e.src_type_name == rule.source_type_name
                }
            }
        });

    if !edge_matches {
        push_err(
            issues,
            format!("{rule_path}.via_link_type"),
            "AGG_REF_INVALID",
            format!(
                "via_link_type '{}' ({}) does not connect '{}' and '{}'",
                rule.via_link_type,
                rule.direction.as_str(),
                nt.name,
                rule.source_type_name
            ),
        );
    }

    if let Some(src_nt) = effective_types.get(&rule.source_type_name) {
        let (src_metrics, src_attrs) = build_scope_for_type(src_nt);
        if !src_metrics.contains(&rule.source_metric) {
            push_err(
                issues,
                format!("{rule_path}.source_metric"),
                "AGG_REF_INVALID",
                format!(
                    "source_metric '{}' is not a METRIC on '{}'",
                    rule.source_metric, rule.source_type_name
                ),
            );
        }
        if let Some(filter_raw) = &rule.filter
            && !filter_raw.trim().is_empty()
        {
            let scope = ExprScope {
                metrics: &src_metrics,
                attributes: &src_attrs,
            };
            let mut unused_refs = BTreeSet::new();
            match check_expression(filter_raw, &scope, &mut unused_refs) {
                Ok(ExprValType::Bool) => {}
                Ok(other) => push_err(
                    issues,
                    format!("{rule_path}.filter"),
                    "EXPR_INVALID",
                    format!("agg_rule filter must evaluate to BOOLEAN, got {other:?}"),
                ),
                Err(err) => push_err(issues, format!("{rule_path}.filter"), "EXPR_INVALID", err),
            }
        }
    } else {
        push_err(
            issues,
            format!("{rule_path}.source_type_name"),
            "AGG_REF_INVALID",
            format!(
                "source_type_name '{}' does not exist",
                rule.source_type_name
            ),
        );
    }

    if !rule.source_type_name.is_empty()
        && !rule.source_metric.is_empty()
        && !rule.target_metric.is_empty()
    {
        let src_node = (rule.source_type_name.clone(), rule.source_metric.clone());
        let dst_node = (nt.name.clone(), rule.target_metric.clone());
        dep_graph
            .entry(src_node)
            .or_default()
            .push((dst_node, String::from(rule_path)));
    }
}

fn validate_rules_and_build_deps(
    doc: &SchemaDoc,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    issues: &mut Vec<ValidationIssue>,
) -> DepEdgeMap {
    let mut dep_graph: DepEdgeMap = BTreeMap::new();
    for (n_idx, nt) in doc.node_types.iter().enumerate() {
        let (this_metrics, this_attrs) = build_scope_for_type(nt);
        for (a_idx, rule) in nt.agg_rules.iter().enumerate() {
            let r_path = format!("node_types[{n_idx}].agg_rules[{a_idx}]");
            validate_agg_rule(
                doc,
                (nt, &this_metrics),
                rule,
                &r_path,
                effective_types,
                &mut dep_graph,
                issues,
            );
        }

        let scope = ExprScope {
            metrics: &this_metrics,
            attributes: &this_attrs,
        };
        for (d_idx, drule) in nt.derived_rules.iter().enumerate() {
            let d_path = format!("node_types[{n_idx}].derived_rules[{d_idx}]");
            if !this_metrics.contains(&drule.target_metric) {
                push_err(
                    issues,
                    format!("{d_path}.target_metric"),
                    "EXPR_INVALID",
                    format!(
                        "derived target_metric '{}' is not a METRIC on '{}'",
                        drule.target_metric, nt.name
                    ),
                );
            }

            let mut ref_metrics = BTreeSet::new();
            match check_expression(&drule.expression, &scope, &mut ref_metrics) {
                Ok(ExprValType::Number) => {}
                Ok(other) => push_err(
                    issues,
                    format!("{d_path}.expression"),
                    "EXPR_INVALID",
                    format!("derived expression must evaluate to NUMBER, got {other:?}"),
                ),
                Err(err) => push_err(issues, format!("{d_path}.expression"), "EXPR_INVALID", err),
            }

            if drule.enabled && !drule.target_metric.is_empty() {
                for rm in ref_metrics {
                    let src_node = (nt.name.clone(), rm);
                    let dst_node = (nt.name.clone(), drule.target_metric.clone());
                    dep_graph
                        .entry(src_node)
                        .or_default()
                        .push((dst_node, format!("{d_path}.expression")));
                }
            }
        }
    }
    dep_graph
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitColor {
    Gray,
    Black,
}

fn detect_metric_cycles(dep_graph: &DepEdgeMap, issues: &mut Vec<ValidationIssue>) {
    let mut colors: BTreeMap<MetricNode, VisitColor> = BTreeMap::new();
    let mut stack: Vec<MetricNode> = Vec::new();
    for start in dep_graph.keys() {
        if !colors.contains_key(start) {
            dfs_cycle(start, dep_graph, &mut colors, &mut stack, issues);
        }
    }
}

fn dfs_cycle(
    node: &MetricNode,
    dep_graph: &DepEdgeMap,
    colors: &mut BTreeMap<MetricNode, VisitColor>,
    stack: &mut Vec<MetricNode>,
    issues: &mut Vec<ValidationIssue>,
) {
    colors.insert(node.clone(), VisitColor::Gray);
    stack.push(node.clone());

    if let Some(neighbors) = dep_graph.get(node) {
        for (next, edge_path) in neighbors {
            match colors.get(next).copied() {
                Some(VisitColor::Gray) => {
                    let chain = format_cycle_chain(stack, next);
                    push_err(
                        issues,
                        edge_path.clone(),
                        "CYCLE_DETECTED",
                        format!("metric dependency cycle detected: {chain}"),
                    );
                }
                Some(VisitColor::Black) => {}
                None => dfs_cycle(next, dep_graph, colors, stack, issues),
            }
        }
    }

    let _ = stack.pop();
    colors.insert(node.clone(), VisitColor::Black);
}

fn format_cycle_chain(stack: &[MetricNode], target: &MetricNode) -> String {
    let start_idx = stack.iter().position(|n| n == target).unwrap_or(0);
    let mut parts: Vec<String> = stack
        .iter()
        .skip(start_idx)
        .map(|(t, m)| format!("{t}.{m}"))
        .collect();
    parts.push(format!("{}.{}", target.0, target.1));
    parts.join(" -> ")
}

fn validate_single_producer(doc: &SchemaDoc, issues: &mut Vec<ValidationIssue>) {
    for (n_idx, nt) in doc.node_types.iter().enumerate() {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for (a_idx, r) in nt.agg_rules.iter().enumerate() {
            let cnt = seen.entry(r.target_metric.clone()).or_insert(0);
            *cnt = cnt.saturating_add(1);
            if *cnt > 1 {
                push_err(
                    issues,
                    format!("node_types[{n_idx}].agg_rules[{a_idx}].target_metric"),
                    "METRIC_MULTIPLE_PRODUCERS",
                    format!(
                        "metric '{}.{}' has multiple producing rules",
                        nt.name, r.target_metric
                    ),
                );
            }
        }
        for (d_idx, r) in nt.derived_rules.iter().enumerate() {
            if !r.enabled {
                continue;
            }
            let cnt = seen.entry(r.target_metric.clone()).or_insert(0);
            *cnt = cnt.saturating_add(1);
            if *cnt > 1 {
                push_err(
                    issues,
                    format!("node_types[{n_idx}].derived_rules[{d_idx}].target_metric"),
                    "METRIC_MULTIPLE_PRODUCERS",
                    format!(
                        "metric '{}.{}' has multiple producing rules",
                        nt.name, r.target_metric
                    ),
                );
            }
        }
    }
}

fn validate_mappings(
    doc: &SchemaDoc,
    ctx: &ValidationContext,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut seen_names = BTreeSet::new();
    for (m_idx, mapping) in doc.mappings.iter().enumerate() {
        let prefix = format!("mappings[{m_idx}]");
        if !is_valid_mapping_name(&mapping.name) {
            push_err(
                issues,
                format!("{prefix}.name"),
                "NAME_INVALID",
                format!("invalid mapping name '{}'", mapping.name),
            );
        } else if !seen_names.insert(mapping.name.clone()) {
            push_err(
                issues,
                format!("{prefix}.name"),
                "NAME_TAKEN",
                format!("duplicate mapping name '{}'", mapping.name),
            );
        }

        let target_nt = if let Some(&nt) = effective_types.get(&mapping.node_type) {
            Some(nt)
        } else {
            push_err(
                issues,
                format!("{prefix}.node_type"),
                "MAPPING_INVALID",
                format!("target node_type '{}' does not exist", mapping.node_type),
            );
            None
        };

        let Some(ds_meta) = ctx.datasets.get(&mapping.dataset) else {
            push_err(
                issues,
                format!("{prefix}.dataset"),
                "MAPPING_INVALID",
                format!("dataset '{}' not found", mapping.dataset),
            );
            continue;
        };

        if ds_meta.status != "READY" {
            push_err(
                issues,
                format!("{prefix}.dataset"),
                "MAPPING_INVALID",
                format!(
                    "dataset '{}' must be in READY status (current: {})",
                    mapping.dataset, ds_meta.status
                ),
            );
        }

        validate_single_mapping_columns(
            doc,
            mapping,
            ds_meta,
            target_nt,
            effective_types,
            &prefix,
            issues,
        );
    }
}

fn validate_single_mapping_columns(
    doc: &SchemaDoc,
    mapping: &MappingSpec,
    ds_meta: &DatasetValidationMeta,
    target_nt: Option<&NodeTypeSpec>,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if !ds_meta.columns.contains_key(&mapping.key_column) {
        push_err(
            issues,
            format!("{prefix}.key_column"),
            "MAPPING_INVALID",
            format!(
                "key_column '{}' not found in dataset '{}'",
                mapping.key_column, ds_meta.name
            ),
        );
    } else if !ds_meta.non_null_columns.is_empty()
        && !ds_meta.non_null_columns.contains(&mapping.key_column)
    {
        push_err(
            issues,
            format!("{prefix}.key_column"),
            "MAPPING_INVALID",
            format!(
                "key_column '{}' must be 100% non-null in dataset '{}'",
                mapping.key_column, ds_meta.name
            ),
        );
    }

    validate_mapping_field_bindings(mapping, ds_meta, target_nt, prefix, issues);
    validate_mapping_time_and_geo(mapping, ds_meta, prefix, issues);
    validate_mapping_links(doc, mapping, ds_meta, effective_types, prefix, issues);
}

fn validate_mapping_field_bindings(
    mapping: &MappingSpec,
    ds_meta: &DatasetValidationMeta,
    target_nt: Option<&NodeTypeSpec>,
    prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for (attr_name, col_name) in &mapping.attributes {
        let fpath = format!("{prefix}.attributes.{attr_name}");
        let Some(&col_dt) = ds_meta.columns.get(col_name) else {
            push_err(
                issues,
                fpath,
                "MAPPING_INVALID",
                format!(
                    "column '{col_name}' not found in dataset '{}'",
                    ds_meta.name
                ),
            );
            continue;
        };
        if let Some(nt) = target_nt {
            match nt
                .properties
                .iter()
                .find(|p| p.name == *attr_name && p.kind == PropertyKind::Attribute)
            {
                Some(prop) if !is_mapping_type_convertible(col_dt, prop.data_type) => push_err(
                    issues,
                    fpath,
                    "MAPPING_INVALID",
                    format!(
                        "column '{col_name}' ({col_dt}) cannot convert to attribute '{attr_name}' ({})",
                        prop.data_type
                    ),
                ),
                None => push_err(
                    issues,
                    fpath,
                    "MAPPING_INVALID",
                    format!("attribute '{attr_name}' not declared on '{}'", nt.name),
                ),
                Some(_) => {}
            }
        }
    }

    for (metric_name, col_name) in &mapping.metrics {
        let fpath = format!("{prefix}.metrics.{metric_name}");
        let Some(&col_dt) = ds_meta.columns.get(col_name) else {
            push_err(
                issues,
                fpath,
                "MAPPING_INVALID",
                format!(
                    "column '{col_name}' not found in dataset '{}'",
                    ds_meta.name
                ),
            );
            continue;
        };
        if let Some(nt) = target_nt {
            match nt
                .properties
                .iter()
                .find(|p| p.name == *metric_name && p.kind == PropertyKind::Metric)
            {
                Some(prop) if !is_mapping_type_convertible(col_dt, prop.data_type) => push_err(
                    issues,
                    fpath,
                    "MAPPING_INVALID",
                    format!(
                        "column '{col_name}' ({col_dt}) cannot convert to metric '{metric_name}' ({})",
                        prop.data_type
                    ),
                ),
                None => push_err(
                    issues,
                    fpath,
                    "MAPPING_INVALID",
                    format!("metric '{metric_name}' not declared on '{}'", nt.name),
                ),
                Some(_) => {}
            }
        }
    }
}

fn validate_mapping_time_and_geo(
    mapping: &MappingSpec,
    ds_meta: &DatasetValidationMeta,
    prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(ts_col) = &mapping.metric_time_column
        && !ts_col.trim().is_empty()
    {
        match ds_meta.columns.get(ts_col) {
            None => push_err(
                issues,
                format!("{prefix}.metric_time_column"),
                "MAPPING_INVALID",
                format!(
                    "metric_time_column '{ts_col}' not found in dataset '{}'",
                    ds_meta.name
                ),
            ),
            Some(&dt) if !is_mapping_type_convertible(dt, DataType::Timestamp) => push_err(
                issues,
                format!("{prefix}.metric_time_column"),
                "MAPPING_INVALID",
                format!("metric_time_column '{ts_col}' ({dt}) cannot convert to TIMESTAMP"),
            ),
            Some(_) => {}
        }
    }

    let lng = mapping.geo.lng_column.as_deref().filter(|s| !s.is_empty());
    let lat = mapping.geo.lat_column.as_deref().filter(|s| !s.is_empty());
    if lng.is_some() != lat.is_some() {
        push_err(
            issues,
            format!("{prefix}.geo"),
            "MAPPING_INVALID",
            String::from("geo.lng_column and geo.lat_column must both be set or both omitted"),
        );
    }
    for (field, col_opt) in [("lng_column", lng), ("lat_column", lat)] {
        if let Some(col) = col_opt {
            match ds_meta.columns.get(col) {
                None => push_err(
                    issues,
                    format!("{prefix}.geo.{field}"),
                    "MAPPING_INVALID",
                    format!("geo column '{col}' not found in dataset '{}'", ds_meta.name),
                ),
                Some(&dt) if !is_mapping_type_convertible(dt, DataType::Float64) => push_err(
                    issues,
                    format!("{prefix}.geo.{field}"),
                    "MAPPING_INVALID",
                    format!("geo column '{col}' ({dt}) must be numeric"),
                ),
                Some(_) => {}
            }
        }
    }
}

fn validate_mapping_links(
    doc: &SchemaDoc,
    mapping: &MappingSpec,
    ds_meta: &DatasetValidationMeta,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    prefix: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    for (l_idx, link) in mapping.links.iter().enumerate() {
        let lpath = format!("{prefix}.links[{l_idx}]");
        if !ds_meta.columns.contains_key(&link.target_key_column) {
            push_err(
                issues,
                format!("{lpath}.target_key_column"),
                "MAPPING_INVALID",
                format!(
                    "target_key_column '{}' not found in dataset '{}'",
                    link.target_key_column, ds_meta.name
                ),
            );
        }
        if let Some(wcol) = &link.weight_column
            && !wcol.trim().is_empty()
        {
            match ds_meta.columns.get(wcol) {
                None => push_err(
                    issues,
                    format!("{lpath}.weight_column"),
                    "MAPPING_INVALID",
                    format!(
                        "weight_column '{wcol}' not found in dataset '{}'",
                        ds_meta.name
                    ),
                ),
                Some(&dt) if !is_mapping_type_convertible(dt, DataType::Float64) => push_err(
                    issues,
                    format!("{lpath}.weight_column"),
                    "MAPPING_INVALID",
                    format!("weight_column '{wcol}' ({dt}) must be numeric"),
                ),
                Some(_) => {}
            }
        }
        if !effective_types.contains_key(&link.target_type) {
            push_err(
                issues,
                format!("{lpath}.target_type"),
                "MAPPING_INVALID",
                format!("link target_type '{}' does not exist", link.target_type),
            );
        }
        let edge_exists = doc.edge_types.iter().any(|e| {
            e.name == link.link_type
                && match link.direction {
                    LinkDirection::Out => {
                        e.src_type_name == mapping.node_type && e.dst_type_name == link.target_type
                    }
                    LinkDirection::In => {
                        e.dst_type_name == mapping.node_type && e.src_type_name == link.target_type
                    }
                }
        });
        if !edge_exists {
            push_err(
                issues,
                format!("{lpath}.link_type"),
                "MAPPING_INVALID",
                format!(
                    "link_type '{}' ({}) does not connect '{}' and '{}'",
                    link.link_type,
                    link.direction.as_str(),
                    mapping.node_type,
                    link.target_type
                ),
            );
        }
    }
}

fn validate_breaking_changes(
    doc: &SchemaDoc,
    ctx: &ValidationContext,
    issues: &mut Vec<ValidationIssue>,
) {
    let Some(current) = &ctx.current_doc else {
        return;
    };

    for old_nt in &current.node_types {
        if let Some((new_idx, new_nt)) = doc
            .node_types
            .iter()
            .enumerate()
            .find(|(_, nt)| nt.name == old_nt.name)
        {
            for old_prop in &old_nt.properties {
                if let Some((p_idx, new_prop)) = new_nt
                    .properties
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.name == old_prop.name)
                    && !is_lossless_widening(old_prop.data_type, new_prop.data_type)
                {
                    push_warn(
                        issues,
                        format!("node_types[{new_idx}].properties[{p_idx}].data_type"),
                        "BREAKING_CHANGE",
                        format!(
                            "changing '{}.{}' from {} to {} is not a lossless conversion",
                            old_nt.name, old_prop.name, old_prop.data_type, new_prop.data_type
                        ),
                    );
                }
            }
        } else if ctx
            .existing_node_counts
            .get(&old_nt.name)
            .copied()
            .unwrap_or(0)
            > 0
        {
            push_warn(
                issues,
                format!("node_types[{}]", old_nt.name),
                "BREAKING_CHANGE",
                format!(
                    "removing node type '{}' while runtime nodes still exist requires force=true",
                    old_nt.name
                ),
            );
        }
    }

    for old_et in &current.edge_types {
        let still_exists = doc.edge_types.iter().any(|et| {
            et.name == old_et.name
                && et.src_type_name == old_et.src_type_name
                && et.dst_type_name == old_et.dst_type_name
        });
        if !still_exists
            && ctx
                .existing_link_counts
                .get(&old_et.name)
                .copied()
                .unwrap_or(0)
                > 0
        {
            push_warn(
                issues,
                format!("edge_types[{}]", old_et.name),
                "BREAKING_CHANGE",
                format!(
                    "removing edge type '{}' while runtime links still exist requires force=true",
                    old_et.name
                ),
            );
        }
    }
}

// ============================================================================
// Deterministic Step 06 §06.1 Expression Lexer, Pratt Parser & Type Checker
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprValType {
    Number,
    Text,
    Bool,
    Null,
}

impl ExprValType {
    const fn unify(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Null, t) | (t, Self::Null) => Some(t),
            (Self::Number, Self::Number) => Some(Self::Number),
            (Self::Text, Self::Text) => Some(Self::Text),
            (Self::Bool, Self::Bool) => Some(Self::Bool),
            _ => None,
        }
    }

    const fn is_numeric_compatible(self) -> bool {
        matches!(self, Self::Number | Self::Null)
    }

    const fn is_bool_compatible(self) -> bool {
        matches!(self, Self::Bool | Self::Null)
    }

    const fn is_text_compatible(self) -> bool {
        matches!(self, Self::Text | Self::Null)
    }
}

struct ExprScope<'a> {
    metrics: &'a BTreeSet<String>,
    attributes: &'a BTreeMap<String, DataType>,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number,
    Text,
    Bool,
    Null,
    Ident(String),
    Or,
    And,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Comma,
    Dot,
}

enum ExprAst {
    Literal(ExprValType),
    MetricRef(String),
    AttrRef(String),
    UnaryNeg(Box<ExprAst>),
    UnaryNot(Box<ExprAst>),
    BinaryArith(Box<ExprAst>, Box<ExprAst>),
    Compare {
        equality_only: bool,
        lhs: Box<ExprAst>,
        rhs: Box<ExprAst>,
    },
    Logical(Box<ExprAst>, Box<ExprAst>),
    Call {
        name: String,
        args: Vec<ExprAst>,
    },
}

fn check_expression(
    raw: &str,
    scope: &ExprScope<'_>,
    referenced_metrics: &mut BTreeSet<String>,
) -> Result<ExprValType, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(String::from("expression must not be empty"));
    }
    if raw.chars().count() > MAX_EXPR_CHARS {
        return Err(format!("expression exceeds {MAX_EXPR_CHARS} characters"));
    }
    let tokens = tokenize_expr(trimmed)?;
    let mut parser = ExprParser { tokens, pos: 0 };
    let ast = parser.parse_expr(0, 1)?;
    if let Some(extra) = parser.peek() {
        return Err(format!("unexpected trailing token: {extra:?}"));
    }
    typecheck_ast(&ast, scope, referenced_metrics)
}

fn tokenize_expr(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            let _ = chars.next();
            continue;
        }
        if ch.is_ascii_digit() {
            lex_number(&mut chars)?;
            tokens.push(Token::Number);
            continue;
        }
        if ch == '\'' {
            lex_string(&mut chars)?;
            tokens.push(Token::Text);
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            tokens.push(lex_ident_or_keyword(&mut chars));
            continue;
        }
        tokens.push(lex_operator(&mut chars)?);
    }
    Ok(tokens)
}

fn lex_number(chars: &mut Peekable<Chars<'_>>) -> Result<(), String> {
    let mut buf = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            buf.push(c);
            let _ = chars.next();
        } else {
            break;
        }
    }
    if chars.peek() == Some(&'.') {
        buf.push('.');
        let _ = chars.next();
        let mut frac_digits = 0usize;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                buf.push(c);
                let _ = chars.next();
                frac_digits = frac_digits.saturating_add(1);
            } else {
                break;
            }
        }
        if frac_digits == 0 {
            return Err(format!("malformed number literal '{buf}'"));
        }
    }
    if matches!(chars.peek(), Some('e' | 'E')) {
        buf.push('e');
        let _ = chars.next();
        if let Some(&sign) = chars.peek()
            && (sign == '+' || sign == '-')
        {
            buf.push(sign);
            let _ = chars.next();
        }
        let mut exp_digits = 0usize;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                buf.push(c);
                let _ = chars.next();
                exp_digits = exp_digits.saturating_add(1);
            } else {
                break;
            }
        }
        if exp_digits == 0 {
            return Err(format!("malformed exponent in '{buf}'"));
        }
    }
    let parsed: f64 = buf
        .parse()
        .map_err(|_| format!("invalid number literal '{buf}'"))?;
    if !parsed.is_finite() {
        return Err(format!("non-finite number literal '{buf}'"));
    }
    Ok(())
}

fn lex_string(chars: &mut Peekable<Chars<'_>>) -> Result<(), String> {
    let _ = chars.next(); // consume opening '\''
    loop {
        let Some(c) = chars.next() else {
            return Err(String::from("unterminated string literal"));
        };
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                let _ = chars.next();
                continue;
            }
            return Ok(());
        }
    }
}

fn lex_ident_or_keyword(chars: &mut Peekable<Chars<'_>>) -> Token {
    let mut ident = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_alphanumeric() || c == '_' {
            ident.push(c);
            let _ = chars.next();
        } else {
            break;
        }
    }
    match ident.as_str() {
        "or" | "OR" => Token::Or,
        "and" | "AND" => Token::And,
        "not" | "NOT" => Token::Not,
        "true" | "TRUE" | "false" | "FALSE" => Token::Bool,
        "null" | "NULL" => Token::Null,
        _ => Token::Ident(ident),
    }
}

fn lex_operator(chars: &mut Peekable<Chars<'_>>) -> Result<Token, String> {
    let Some(ch) = chars.next() else {
        return Err(String::from("unexpected end of expression"));
    };
    match ch {
        '+' => Ok(Token::Plus),
        '-' => Ok(Token::Minus),
        '*' => Ok(Token::Star),
        '/' => Ok(Token::Slash),
        '%' => Ok(Token::Percent),
        '^' => Ok(Token::Caret),
        '(' => Ok(Token::LParen),
        ')' => Ok(Token::RParen),
        ',' => Ok(Token::Comma),
        '.' => Ok(Token::Dot),
        '=' => {
            if chars.peek() == Some(&'=') {
                let _ = chars.next();
                Ok(Token::Eq)
            } else {
                Err(String::from("unexpected '='; use '==' for equality"))
            }
        }
        '!' => {
            if chars.peek() == Some(&'=') {
                let _ = chars.next();
                Ok(Token::Ne)
            } else {
                Ok(Token::Not)
            }
        }
        '<' => {
            if chars.peek() == Some(&'=') {
                let _ = chars.next();
                Ok(Token::Le)
            } else {
                Ok(Token::Lt)
            }
        }
        '>' => {
            if chars.peek() == Some(&'=') {
                let _ = chars.next();
                Ok(Token::Ge)
            } else {
                Ok(Token::Gt)
            }
        }
        '|' => {
            if chars.peek() == Some(&'|') {
                let _ = chars.next();
                Ok(Token::Or)
            } else {
                Err(String::from("unexpected '|'; use '||' or 'or'"))
            }
        }
        '&' => {
            if chars.peek() == Some(&'&') {
                let _ = chars.next();
                Ok(Token::And)
            } else {
                Err(String::from("unexpected '&'; use '&&' or 'and'"))
            }
        }
        other => Err(format!("unexpected character '{other}' in expression")),
    }
}

struct ExprParser {
    tokens: Vec<Token>,
    pos: usize,
}

impl ExprParser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next_tok(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        tok
    }

    fn parse_expr(&mut self, min_bp: u8, depth: usize) -> Result<ExprAst, String> {
        if depth > MAX_EXPR_DEPTH {
            return Err(format!(
                "expression exceeds maximum AST depth {MAX_EXPR_DEPTH}"
            ));
        }

        let mut lhs = self.parse_prefix(depth)?;

        while let Some(op) = self.peek().cloned() {
            let Some((l_bp, r_bp)) = infix_binding_power(&op) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            let _ = self.next_tok();
            let rhs = self.parse_expr(r_bp, depth.saturating_add(1))?;

            lhs = match op {
                Token::Or | Token::And => ExprAst::Logical(Box::new(lhs), Box::new(rhs)),
                Token::Eq | Token::Ne => {
                    if self.peek().is_some_and(is_cmp_token) {
                        return Err(String::from("chained comparison operators are not allowed"));
                    }
                    ExprAst::Compare {
                        equality_only: true,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    }
                }
                Token::Lt | Token::Le | Token::Gt | Token::Ge => {
                    if self.peek().is_some_and(is_cmp_token) {
                        return Err(String::from("chained comparison operators are not allowed"));
                    }
                    ExprAst::Compare {
                        equality_only: false,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    }
                }
                Token::Plus
                | Token::Minus
                | Token::Star
                | Token::Slash
                | Token::Percent
                | Token::Caret => ExprAst::BinaryArith(Box::new(lhs), Box::new(rhs)),
                _ => return Err(String::from("internal infix dispatch mismatch")),
            };
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self, depth: usize) -> Result<ExprAst, String> {
        let Some(tok) = self.next_tok() else {
            return Err(String::from("unexpected end of expression"));
        };
        match tok {
            Token::Number => Ok(ExprAst::Literal(ExprValType::Number)),
            Token::Text => Ok(ExprAst::Literal(ExprValType::Text)),
            Token::Bool => Ok(ExprAst::Literal(ExprValType::Bool)),
            Token::Null => Ok(ExprAst::Literal(ExprValType::Null)),
            Token::Minus => {
                let inner = self.parse_expr(11, depth.saturating_add(1))?;
                Ok(ExprAst::UnaryNeg(Box::new(inner)))
            }
            Token::Not => {
                let inner = self.parse_expr(5, depth.saturating_add(1))?;
                Ok(ExprAst::UnaryNot(Box::new(inner)))
            }
            Token::LParen => {
                let inner = self.parse_expr(0, depth.saturating_add(1))?;
                if self.next_tok() != Some(Token::RParen) {
                    return Err(String::from("expected closing ')'"));
                }
                Ok(inner)
            }
            Token::Ident(name) => self.parse_ident_tail(name, depth),
            other => Err(format!("unexpected token {other:?}")),
        }
    }

    fn parse_ident_tail(&mut self, name: String, depth: usize) -> Result<ExprAst, String> {
        if self.peek() == Some(&Token::Dot) {
            let _ = self.next_tok();
            if name != "prop" {
                return Err(format!(
                    "dot property access is only supported via 'prop.<attr>', got '{name}.'"
                ));
            }
            match self.next_tok() {
                Some(Token::Ident(attr)) => return Ok(ExprAst::AttrRef(attr)),
                _ => return Err(String::from("expected attribute identifier after 'prop.'")),
            }
        }
        if name == "prop" {
            return Err(String::from(
                "reserved identifier 'prop' must be followed by '.<attr>'",
            ));
        }
        if self.peek() == Some(&Token::LParen) {
            let _ = self.next_tok();
            let mut args = Vec::new();
            if self.peek() != Some(&Token::RParen) {
                loop {
                    if args.len() >= MAX_CALL_ARGS {
                        return Err(format!(
                            "function '{name}' exceeds maximum {MAX_CALL_ARGS} arguments"
                        ));
                    }
                    args.push(self.parse_expr(0, depth.saturating_add(1))?);
                    if self.peek() == Some(&Token::Comma) {
                        let _ = self.next_tok();
                    } else {
                        break;
                    }
                }
            }
            if self.next_tok() != Some(Token::RParen) {
                return Err(format!("expected ')' closing call to '{name}'"));
            }
            return Ok(ExprAst::Call { name, args });
        }
        Ok(ExprAst::MetricRef(name))
    }
}

const fn is_cmp_token(tok: &Token) -> bool {
    matches!(
        tok,
        Token::Eq | Token::Ne | Token::Lt | Token::Le | Token::Gt | Token::Ge
    )
}

const fn infix_binding_power(tok: &Token) -> Option<(u8, u8)> {
    match tok {
        Token::Or => Some((1, 2)),
        Token::And => Some((3, 4)),
        Token::Eq | Token::Ne | Token::Lt | Token::Le | Token::Gt | Token::Ge => Some((6, 7)),
        Token::Plus | Token::Minus => Some((8, 9)),
        Token::Star | Token::Slash | Token::Percent => Some((10, 11)),
        Token::Caret => Some((12, 11)),
        _ => None,
    }
}

fn typecheck_ast(
    ast: &ExprAst,
    scope: &ExprScope<'_>,
    referenced_metrics: &mut BTreeSet<String>,
) -> Result<ExprValType, String> {
    match ast {
        ExprAst::Literal(t) => Ok(*t),
        ExprAst::MetricRef(name) => {
            if !scope.metrics.contains(name) {
                return Err(format!("unknown metric '{name}'"));
            }
            referenced_metrics.insert(name.clone());
            Ok(ExprValType::Number)
        }
        ExprAst::AttrRef(attr) => {
            let Some(&dt) = scope.attributes.get(attr) else {
                return Err(format!("unknown attribute 'prop.{attr}'"));
            };
            match dt {
                DataType::Int64 | DataType::Float64 | DataType::Decimal => Ok(ExprValType::Number),
                DataType::String => Ok(ExprValType::Text),
                DataType::Boolean => Ok(ExprValType::Bool),
                other => Err(format!(
                    "attribute 'prop.{attr}' of type {other} cannot be referenced in expressions"
                )),
            }
        }
        ExprAst::UnaryNeg(inner) => {
            let t = typecheck_ast(inner, scope, referenced_metrics)?;
            if !t.is_numeric_compatible() {
                return Err(format!("unary '-' requires NUMBER, got {t:?}"));
            }
            Ok(ExprValType::Number)
        }
        ExprAst::UnaryNot(inner) => {
            let t = typecheck_ast(inner, scope, referenced_metrics)?;
            if !t.is_bool_compatible() {
                return Err(format!("unary 'not'/'!' requires BOOLEAN, got {t:?}"));
            }
            Ok(ExprValType::Bool)
        }
        ExprAst::BinaryArith(lhs, rhs) => {
            let lt = typecheck_ast(lhs, scope, referenced_metrics)?;
            let rt = typecheck_ast(rhs, scope, referenced_metrics)?;
            if !lt.is_numeric_compatible() || !rt.is_numeric_compatible() {
                return Err(format!(
                    "arithmetic operator requires NUMBER operands, got ({lt:?}, {rt:?})"
                ));
            }
            Ok(ExprValType::Number)
        }
        ExprAst::Logical(lhs, rhs) => {
            let lt = typecheck_ast(lhs, scope, referenced_metrics)?;
            let rt = typecheck_ast(rhs, scope, referenced_metrics)?;
            if !lt.is_bool_compatible() || !rt.is_bool_compatible() {
                return Err(format!(
                    "logical operator requires BOOLEAN operands, got ({lt:?}, {rt:?})"
                ));
            }
            Ok(ExprValType::Bool)
        }
        ExprAst::Compare {
            equality_only,
            lhs,
            rhs,
        } => {
            let lt = typecheck_ast(lhs, scope, referenced_metrics)?;
            let rt = typecheck_ast(rhs, scope, referenced_metrics)?;
            let Some(unified) = lt.unify(rt) else {
                return Err(format!(
                    "cannot compare incompatible types {lt:?} and {rt:?}"
                ));
            };
            if !equality_only && unified == ExprValType::Bool {
                return Err(String::from(
                    "ordering comparison is not supported on BOOLEAN",
                ));
            }
            Ok(ExprValType::Bool)
        }
        ExprAst::Call { name, args } => typecheck_call(name, args, scope, referenced_metrics),
    }
}

fn typecheck_call(
    name: &str,
    args: &[ExprAst],
    scope: &ExprScope<'_>,
    referenced_metrics: &mut BTreeSet<String>,
) -> Result<ExprValType, String> {
    let mut arg_types = Vec::with_capacity(args.len());
    for a in args {
        arg_types.push(typecheck_ast(a, scope, referenced_metrics)?);
    }

    match name {
        "abs" | "sqrt" | "exp" | "ln" | "log10" | "floor" | "ceil" => {
            require_numeric_args(name, &arg_types, 1, 1)
        }
        "round" => require_numeric_args(name, &arg_types, 1, 2),
        "pow" => require_numeric_args(name, &arg_types, 2, 2),
        "min" | "max" => require_numeric_args(name, &arg_types, 2, MAX_CALL_ARGS),
        "clamp" => require_numeric_args(name, &arg_types, 3, 3),
        "safe_div" => require_numeric_args(name, &arg_types, 2, 3),
        "if" => {
            if arg_types.len() != 3 {
                return Err(format!("'if' expects 3 arguments, got {}", arg_types.len()));
            }
            let cond = arg_types.first().copied().unwrap_or(ExprValType::Null);
            let then_t = arg_types.get(1).copied().unwrap_or(ExprValType::Null);
            let else_t = arg_types.get(2).copied().unwrap_or(ExprValType::Null);
            if !cond.is_bool_compatible() {
                return Err(format!("'if' condition must be BOOLEAN, got {cond:?}"));
            }
            then_t.unify(else_t).ok_or_else(|| {
                format!("'if' branches have incompatible types {then_t:?} and {else_t:?}")
            })
        }
        "coalesce" => {
            if arg_types.is_empty() {
                return Err(String::from("'coalesce' expects at least 1 argument"));
            }
            let mut acc = ExprValType::Null;
            for &t in &arg_types {
                acc = acc.unify(t).ok_or_else(|| {
                    format!("'coalesce' arguments have incompatible types ({acc:?} vs {t:?})")
                })?;
            }
            Ok(acc)
        }
        "is_null" => {
            if arg_types.len() != 1 {
                return Err(format!(
                    "'is_null' expects 1 argument, got {}",
                    arg_types.len()
                ));
            }
            Ok(ExprValType::Bool)
        }
        "contains" | "starts_with" => {
            if arg_types.len() != 2 {
                return Err(format!(
                    "'{name}' expects 2 arguments, got {}",
                    arg_types.len()
                ));
            }
            if !arg_types.iter().all(|t| t.is_text_compatible()) {
                return Err(format!("'{name}' requires TEXT arguments"));
            }
            Ok(ExprValType::Bool)
        }
        "lower" | "upper" => {
            if arg_types.len() != 1 {
                return Err(format!(
                    "'{name}' expects 1 argument, got {}",
                    arg_types.len()
                ));
            }
            if !arg_types.iter().all(|t| t.is_text_compatible()) {
                return Err(format!("'{name}' requires a TEXT argument"));
            }
            Ok(ExprValType::Text)
        }
        other => Err(format!("unknown function '{other}'")),
    }
}

fn require_numeric_args(
    name: &str,
    arg_types: &[ExprValType],
    min_len: usize,
    max_len: usize,
) -> Result<ExprValType, String> {
    if arg_types.len() < min_len || arg_types.len() > max_len {
        return Err(format!(
            "'{name}' expects {min_len}..={max_len} arguments, got {}",
            arg_types.len()
        ));
    }
    for &t in arg_types {
        if !t.is_numeric_compatible() {
            return Err(format!("'{name}' requires NUMBER arguments, got {t:?}"));
        }
    }
    Ok(ExprValType::Number)
}
