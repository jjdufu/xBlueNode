//! 4-path edge dynamics (`VERTEX_TO_EDGE`, `EDGE_TO_VERTEX`, `EDGE_TO_EDGE`) static validation rules (`04-STEP26` §26.3).

use std::collections::{BTreeMap, BTreeSet};

use crate::enums::{AggFn, PropagationPathKind};
use crate::expr::{ExprType, ExprTypeEnv, parse as parse_expr, typecheck_detailed};
use crate::studio::cert::{EdgeAggRuleSpec, EdgeDerivedRuleSpec};
use crate::studio::doc::{
    AggRuleSpec, EdgeTypeSpec, IssueSeverity, LinkDirection, NodeTypeSpec, PropertyKind,
    PropertySpec, SchemaDoc, ValidationIssue,
};

pub(super) type MetricNode = (String, String);
pub(super) type DepEdgeMap = BTreeMap<MetricNode, Vec<(MetricNode, String)>>;

pub(super) fn push_err(
    issues: &mut Vec<ValidationIssue>,
    path: String,
    code: &str,
    message: String,
) {
    issues.push(ValidationIssue {
        severity: IssueSeverity::Error,
        path,
        code: String::from(code),
        message,
    });
}

pub(super) fn add_scoped_props(
    env: &mut ExprTypeEnv,
    props: &[PropertySpec],
    scope_prefix: &str,
    include_bare_metrics: bool,
) {
    for p in props {
        match p.kind {
            PropertyKind::Metric => {
                if include_bare_metrics {
                    env.metrics.insert(p.name.clone());
                }
                env.properties
                    .insert(format!("{scope_prefix}.{}", p.name), Some(ExprType::Number));
                env.properties.insert(
                    format!("{scope_prefix}.metric.{}", p.name),
                    Some(ExprType::Number),
                );
            }
            PropertyKind::Attribute => {
                let et = ExprType::from_property_data_type(p.data_type);
                if include_bare_metrics {
                    env.properties.insert(p.name.clone(), et);
                }
                env.properties
                    .insert(format!("{scope_prefix}.{}", p.name), et);
                env.properties
                    .insert(format!("{scope_prefix}.prop.{}", p.name), et);
            }
        }
    }
}

pub(super) fn build_edge_type_env(
    edge: &EdgeTypeSpec,
    path_kind: PropagationPathKind,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
) -> ExprTypeEnv {
    let mut env = ExprTypeEnv::default();
    env.metrics.insert(String::from("weight"));
    env.properties
        .insert(String::from("self.weight"), Some(ExprType::Number));
    add_scoped_props(&mut env, &edge.properties, "self", true);

    if path_kind == PropagationPathKind::VertexToEdge {
        if let Some(src_nt) = effective_types.get(&edge.src_type_name) {
            add_scoped_props(&mut env, &src_nt.properties, "src", false);
        }
        if let Some(dst_nt) = effective_types.get(&edge.dst_type_name) {
            add_scoped_props(&mut env, &dst_nt.properties, "dst", false);
        }
    }
    env
}

/// Returns `true` when `rule` is an `EDGE_TO_VERTEX` aggregation (`source_type_name == via_link_type`).
pub(super) fn is_edge_to_vertex_agg(doc: &SchemaDoc, rule: &AggRuleSpec) -> bool {
    !rule.via_link_type.trim().is_empty()
        && (rule.source_type_name == rule.via_link_type
            || (rule.source_type_name.trim().is_empty()
                && doc.edge_types.iter().any(|e| e.name == rule.via_link_type)))
}

/// Validates an `EDGE_TO_VERTEX` `AggRuleSpec` and appends dependency edges into `dep_graph`.
pub(super) fn validate_edge_to_vertex_agg(
    doc: &SchemaDoc,
    nt: &NodeTypeSpec,
    rule: &AggRuleSpec,
    rule_path: &str,
    dep_graph: &mut DepEdgeMap,
    issues: &mut Vec<ValidationIssue>,
) {
    let matching_edges: Vec<&EdgeTypeSpec> = doc
        .edge_types
        .iter()
        .filter(|e| {
            e.name == rule.via_link_type
                && match rule.direction {
                    LinkDirection::Out => e.src_type_name == nt.name,
                    LinkDirection::In => e.dst_type_name == nt.name,
                }
        })
        .collect();

    let Some(&first_edge) = matching_edges.first() else {
        push_err(
            issues,
            format!("{rule_path}.via_link_type"),
            "AGG_REF_INVALID",
            format!(
                "via_link_type '{}' ({}) is not incident to '{}'",
                rule.via_link_type,
                rule.direction.as_str(),
                nt.name
            ),
        );
        return;
    };

    let has_metric = (rule.op == AggFn::Count && rule.source_metric.trim().is_empty())
        || matching_edges.iter().any(|e| {
            e.properties
                .iter()
                .any(|p| p.kind == PropertyKind::Metric && p.name == rule.source_metric)
        });
    if !has_metric {
        push_err(
            issues,
            format!("{rule_path}.source_metric"),
            "AGG_REF_INVALID",
            format!(
                "source_metric '{}' is not a METRIC on edge type '{}'",
                rule.source_metric, rule.via_link_type
            ),
        );
    }

    if let Some(filter_raw) = &rule.filter
        && !filter_raw.trim().is_empty()
    {
        let empty_nt = BTreeMap::new();
        let edge_env = build_edge_type_env(first_edge, PropagationPathKind::EdgeToEdge, &empty_nt);
        match parse_expr(filter_raw).and_then(|ast| typecheck_detailed(&ast, &edge_env)) {
            Ok(info) if info.result_type == ExprType::Bool => {}
            Ok(info) => push_err(
                issues,
                format!("{rule_path}.filter"),
                "EXPR_INVALID",
                format!(
                    "agg_rule filter must evaluate to BOOLEAN, got {:?}",
                    info.result_type
                ),
            ),
            Err(e) => push_err(
                issues,
                format!("{rule_path}.filter"),
                "EXPR_INVALID",
                e.message,
            ),
        }
    }

    if !rule.source_metric.is_empty() && !rule.target_metric.is_empty() {
        let src_node = (
            format!("edge:{}", rule.via_link_type),
            rule.source_metric.clone(),
        );
        let dst_node = (nt.name.clone(), rule.target_metric.clone());
        dep_graph
            .entry(src_node)
            .or_default()
            .push((dst_node, String::from(rule_path)));
    }
}

/// Validates `EdgeTypeSpec` `derived_rules` and `agg_rules`, single-producer uniqueness, and dependency edges.
pub(super) fn validate_edge_rules_and_deps(
    doc: &SchemaDoc,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    dep_graph: &mut DepEdgeMap,
    issues: &mut Vec<ValidationIssue>,
) {
    for (e_idx, et) in doc.edge_types.iter().enumerate() {
        validate_edge_single_producer(et, e_idx, issues);
        let edge_metrics: BTreeSet<&str> = et
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Metric)
            .map(|p| p.name.as_str())
            .collect();

        for (d_idx, drule) in et.derived_rules.iter().enumerate() {
            let d_path = format!("edge_types[{e_idx}].derived_rules[{d_idx}]");
            validate_single_edge_derived_rule(
                et,
                &edge_metrics,
                drule,
                &d_path,
                effective_types,
                dep_graph,
                issues,
            );
        }

        for (a_idx, arule) in et.agg_rules.iter().enumerate() {
            let a_path = format!("edge_types[{e_idx}].agg_rules[{a_idx}]");
            validate_single_edge_agg_rule(
                doc,
                et,
                &edge_metrics,
                arule,
                &a_path,
                dep_graph,
                issues,
            );
        }
    }
}

fn validate_edge_single_producer(
    et: &EdgeTypeSpec,
    e_idx: usize,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (a_idx, r) in et.agg_rules.iter().enumerate() {
        let cnt = seen.entry(r.target_metric.as_str()).or_insert(0);
        *cnt = cnt.saturating_add(1);
        if *cnt > 1 {
            push_err(
                issues,
                format!("edge_types[{e_idx}].agg_rules[{a_idx}].target_metric"),
                "METRIC_MULTIPLE_PRODUCERS",
                format!(
                    "edge metric '{}.{}' has multiple producing rules",
                    et.name, r.target_metric
                ),
            );
        }
    }
    for (d_idx, r) in et.derived_rules.iter().enumerate() {
        if !r.enabled {
            continue;
        }
        let cnt = seen.entry(r.target_metric.as_str()).or_insert(0);
        *cnt = cnt.saturating_add(1);
        if *cnt > 1 {
            push_err(
                issues,
                format!("edge_types[{e_idx}].derived_rules[{d_idx}].target_metric"),
                "METRIC_MULTIPLE_PRODUCERS",
                format!(
                    "edge metric '{}.{}' has multiple producing rules",
                    et.name, r.target_metric
                ),
            );
        }
    }
}

fn validate_single_edge_derived_rule(
    et: &EdgeTypeSpec,
    edge_metrics: &BTreeSet<&str>,
    drule: &EdgeDerivedRuleSpec,
    d_path: &str,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    dep_graph: &mut DepEdgeMap,
    issues: &mut Vec<ValidationIssue>,
) {
    if !matches!(
        drule.path_kind,
        PropagationPathKind::VertexToEdge | PropagationPathKind::EdgeToEdge
    ) {
        push_err(
            issues,
            format!("{d_path}.path_kind"),
            "EXPR_INVALID",
            format!(
                "edge derived_rule path_kind must be VERTEX_TO_EDGE or EDGE_TO_EDGE, got {}",
                drule.path_kind
            ),
        );
    }
    if !edge_metrics.contains(drule.target_metric.as_str()) {
        push_err(
            issues,
            format!("{d_path}.target_metric"),
            "EXPR_INVALID",
            format!(
                "derived target_metric '{}' is not a METRIC on edge '{}'",
                drule.target_metric, et.name
            ),
        );
    }

    let env = build_edge_type_env(et, drule.path_kind, effective_types);
    match parse_expr(&drule.expression).and_then(|ast| typecheck_detailed(&ast, &env)) {
        Ok(info) => {
            if info.result_type != ExprType::Number {
                push_err(
                    issues,
                    format!("{d_path}.expression"),
                    "EXPR_INVALID",
                    format!(
                        "edge derived expression must evaluate to NUMBER, got {:?}",
                        info.result_type
                    ),
                );
            }
            if drule.enabled && !drule.target_metric.is_empty() {
                record_edge_derived_deps(
                    et,
                    edge_metrics,
                    drule,
                    d_path,
                    &info.referenced_metrics,
                    &info.referenced_properties,
                    dep_graph,
                );
            }
        }
        Err(err) => push_err(
            issues,
            format!("{d_path}.expression"),
            "EXPR_INVALID",
            err.message,
        ),
    }
}

fn record_edge_derived_deps(
    et: &EdgeTypeSpec,
    edge_metrics: &BTreeSet<&str>,
    drule: &EdgeDerivedRuleSpec,
    d_path: &str,
    ref_metrics: &BTreeSet<String>,
    ref_props: &BTreeSet<String>,
    dep_graph: &mut DepEdgeMap,
) {
    let dst_node = (format!("edge:{}", et.name), drule.target_metric.clone());
    let expr_path = format!("{d_path}.expression");

    for rm in ref_metrics {
        if edge_metrics.contains(rm.as_str()) {
            dep_graph
                .entry((format!("edge:{}", et.name), rm.clone()))
                .or_default()
                .push((dst_node.clone(), expr_path.clone()));
        }
    }
    for rp in ref_props {
        if rp.starts_with("src.prop.")
            || rp.starts_with("dst.prop.")
            || rp.starts_with("self.prop.")
        {
            continue;
        }
        if let Some(m) = rp
            .strip_prefix("src.metric.")
            .or_else(|| rp.strip_prefix("src."))
        {
            dep_graph
                .entry((et.src_type_name.clone(), String::from(m)))
                .or_default()
                .push((dst_node.clone(), expr_path.clone()));
        } else if let Some(m) = rp
            .strip_prefix("dst.metric.")
            .or_else(|| rp.strip_prefix("dst."))
        {
            dep_graph
                .entry((et.dst_type_name.clone(), String::from(m)))
                .or_default()
                .push((dst_node.clone(), expr_path.clone()));
        } else if let Some(m) = rp
            .strip_prefix("self.metric.")
            .or_else(|| rp.strip_prefix("self."))
            && edge_metrics.contains(m)
        {
            dep_graph
                .entry((format!("edge:{}", et.name), String::from(m)))
                .or_default()
                .push((dst_node.clone(), expr_path.clone()));
        }
    }
}

fn validate_single_edge_agg_rule(
    doc: &SchemaDoc,
    et: &EdgeTypeSpec,
    edge_metrics: &BTreeSet<&str>,
    rule: &EdgeAggRuleSpec,
    rule_path: &str,
    dep_graph: &mut DepEdgeMap,
    issues: &mut Vec<ValidationIssue>,
) {
    if !edge_metrics.contains(rule.target_metric.as_str()) {
        push_err(
            issues,
            format!("{rule_path}.target_metric"),
            "AGG_REF_INVALID",
            format!(
                "target_metric '{}' is not a METRIC on edge '{}'",
                rule.target_metric, et.name
            ),
        );
    }

    let matching: Vec<&EdgeTypeSpec> = doc
        .edge_types
        .iter()
        .filter(|u| {
            u.name == rule.via_link_type
                && match rule.direction {
                    LinkDirection::In => u.dst_type_name == et.src_type_name,
                    LinkDirection::Out => u.src_type_name == et.dst_type_name,
                }
        })
        .collect();

    let Some(&src_edge) = matching.first() else {
        push_err(
            issues,
            format!("{rule_path}.via_link_type"),
            "AGG_REF_INVALID",
            format!(
                "via_link_type '{}' ({}) does not connect at pivot of '{}'",
                rule.via_link_type,
                rule.direction.as_str(),
                et.name
            ),
        );
        return;
    };

    let has_metric = (rule.op == AggFn::Count && rule.source_metric.trim().is_empty())
        || matching.iter().any(|u| {
            u.properties
                .iter()
                .any(|p| p.kind == PropertyKind::Metric && p.name == rule.source_metric)
        });
    if !has_metric {
        push_err(
            issues,
            format!("{rule_path}.source_metric"),
            "AGG_REF_INVALID",
            format!(
                "source_metric '{}' is not a METRIC on edge '{}'",
                rule.source_metric, rule.via_link_type
            ),
        );
    }

    if let Some(filter_raw) = &rule.filter
        && !filter_raw.trim().is_empty()
    {
        let empty_nt = BTreeMap::new();
        let env = build_edge_type_env(src_edge, PropagationPathKind::EdgeToEdge, &empty_nt);
        match parse_expr(filter_raw).and_then(|ast| typecheck_detailed(&ast, &env)) {
            Ok(info) if info.result_type == ExprType::Bool => {}
            Ok(info) => push_err(
                issues,
                format!("{rule_path}.filter"),
                "EXPR_INVALID",
                format!(
                    "edge agg filter must evaluate to BOOLEAN, got {:?}",
                    info.result_type
                ),
            ),
            Err(e) => push_err(
                issues,
                format!("{rule_path}.filter"),
                "EXPR_INVALID",
                e.message,
            ),
        }
    }

    if !rule.source_metric.is_empty() && !rule.target_metric.is_empty() {
        dep_graph
            .entry((
                format!("edge:{}", rule.via_link_type),
                rule.source_metric.clone(),
            ))
            .or_default()
            .push((
                (format!("edge:{}", et.name), rule.target_metric.clone()),
                String::from(rule_path),
            ));
    }
}
