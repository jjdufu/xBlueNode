//! `ActionSpec` static guardrails (`DERIVED_METRIC_WRITE_FORBIDDEN`, `SIDE_EFFECT_TIER_VIOLATION`)
//! and `DEAD_KNOWLEDGE` advisory emission (`04-STEP26` §26.2 & §26.4).

use std::collections::{BTreeMap, BTreeSet};

use crate::enums::{ActionSideEffectTier, DataType};
use crate::expr::{ExprType, ExprTypeEnv, parse as parse_expr, typecheck_detailed};
use crate::studio::cert::{ActionParamSpec, ActionSpec};
use crate::studio::cert_eval::{ParsedMutationTarget, parse_mutation_target, scan_dead_knowledge};
use crate::studio::doc::{
    EdgeTypeSpec, IssueSeverity, LinkDirection, NodeTypeSpec, PropertyKind, PropertySpec,
    SchemaDoc, ValidationIssue,
};
use crate::studio::validate_edge_rules::{add_scoped_props, push_err};

fn is_action_or_param_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    bytes.len() <= 63
        && first.is_ascii_alphabetic()
        && bytes
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

fn param_expr_type(dt: DataType) -> Option<ExprType> {
    match dt {
        DataType::Date | DataType::Timestamp => Some(ExprType::Text),
        other => ExprType::from_property_data_type(other),
    }
}

struct ActionValidateCtx<'a> {
    doc: &'a SchemaDoc,
    effective_types: &'a BTreeMap<String, &'a NodeTypeSpec>,
    nt: &'a NodeTypeSpec,
    base_env: ExprTypeEnv,
}

/// Validates all `ActionSpec` declarations across `doc.node_types` (`04-STEP26` §26.2).
pub(super) fn validate_all_actions(
    doc: &SchemaDoc,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    issues: &mut Vec<ValidationIssue>,
) {
    for (n_idx, nt) in doc.node_types.iter().enumerate() {
        let mut seen_actions = BTreeSet::new();
        for (a_idx, action) in nt.actions.iter().enumerate() {
            let a_path = format!("node_types[{n_idx}].actions[{a_idx}]");
            if !is_action_or_param_ident(&action.name) {
                push_err(
                    issues,
                    format!("{a_path}.name"),
                    "NAME_INVALID",
                    format!(
                        "action name '{}' must match ^[a-zA-Z][a-zA-Z0-9_]{{0,62}}$",
                        action.name
                    ),
                );
            } else if !seen_actions.insert(action.name.as_str()) {
                push_err(
                    issues,
                    format!("{a_path}.name"),
                    "NAME_TAKEN",
                    format!("duplicate action name '{}' on '{}'", action.name, nt.name),
                );
            }

            validate_single_action(doc, nt, action, &a_path, effective_types, issues);
        }
    }
}

fn validate_single_action(
    doc: &SchemaDoc,
    nt: &NodeTypeSpec,
    action: &ActionSpec,
    a_path: &str,
    effective_types: &BTreeMap<String, &NodeTypeSpec>,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut base_env = ExprTypeEnv::default();
    add_scoped_props(&mut base_env, &nt.properties, "self", true);
    validate_action_params(&action.parameters, a_path, &mut base_env, issues);

    if action
        .trigger_flow_slug
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        && action.side_effect_tier != ActionSideEffectTier::ExternalEffect
    {
        push_err(
            issues,
            format!("{a_path}.side_effect_tier"),
            "SIDE_EFFECT_TIER_VIOLATION",
            format!(
                "action '{}' declares trigger_flow_slug so side_effect_tier must be EXTERNAL_EFFECT",
                action.name
            ),
        );
    }

    if let Some(pre_raw) = action
        .precondition_expr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match parse_expr(pre_raw).and_then(|ast| typecheck_detailed(&ast, &base_env)) {
            Ok(info) if matches!(info.result_type, ExprType::Bool | ExprType::Null) => {}
            Ok(info) => push_err(
                issues,
                format!("{a_path}.precondition_expr"),
                "EXPR_INVALID",
                format!(
                    "action precondition_expr must evaluate to BOOLEAN, got {:?}",
                    info.result_type
                ),
            ),
            Err(e) => push_err(
                issues,
                format!("{a_path}.precondition_expr"),
                "EXPR_INVALID",
                e.message,
            ),
        }
    }

    let act_ctx = ActionValidateCtx {
        doc,
        effective_types,
        nt,
        base_env,
    };
    for (m_idx, mstep) in action.mutations.iter().enumerate() {
        let m_path = format!("{a_path}.mutations[{m_idx}]");
        validate_mutation_step(&act_ctx, action.side_effect_tier, mstep, &m_path, issues);
    }
}

fn validate_action_params(
    params: &[ActionParamSpec],
    a_path: &str,
    env: &mut ExprTypeEnv,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut seen = BTreeSet::new();
    for (p_idx, param) in params.iter().enumerate() {
        let p_path = format!("{a_path}.parameters[{p_idx}].name");
        if !is_action_or_param_ident(&param.name) {
            push_err(
                issues,
                p_path,
                "NAME_INVALID",
                format!(
                    "parameter name '{}' must match ^[a-zA-Z][a-zA-Z0-9_]{{0,62}}$",
                    param.name
                ),
            );
            continue;
        }
        if !seen.insert(param.name.as_str()) {
            push_err(
                issues,
                p_path,
                "NAME_TAKEN",
                format!("duplicate parameter name '{}'", param.name),
            );
            continue;
        }
        env.properties.insert(
            format!("param.{}", param.name),
            param_expr_type(param.data_type),
        );
    }
}

fn protected_node_metrics(nt: &NodeTypeSpec) -> BTreeSet<&str> {
    let mut set = BTreeSet::new();
    for r in &nt.agg_rules {
        set.insert(r.target_metric.as_str());
    }
    for d in &nt.derived_rules {
        if d.enabled {
            set.insert(d.target_metric.as_str());
        }
    }
    set
}

fn protected_edge_metrics(et: &EdgeTypeSpec) -> BTreeSet<&str> {
    let mut set = BTreeSet::new();
    for r in &et.agg_rules {
        set.insert(r.target_metric.as_str());
    }
    for d in &et.derived_rules {
        if d.enabled {
            set.insert(d.target_metric.as_str());
        }
    }
    set
}

fn validate_mutation_step(
    ctx: &ActionValidateCtx<'_>,
    tier: ActionSideEffectTier,
    mstep: &crate::studio::MutationStepSpec,
    m_path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let parsed = match parse_mutation_target(&mstep.target, &ctx.nt.name, ctx.doc) {
        Ok(p) => p,
        Err(msg) => {
            push_err(
                issues,
                format!("{m_path}.target"),
                "ACTION_TARGET_INVALID",
                msg,
            );
            return;
        }
    };

    if tier == ActionSideEffectTier::SelfOnly
        && !matches!(parsed, ParsedMutationTarget::SelfField { .. })
    {
        push_err(
            issues,
            format!("{m_path}.target"),
            "SIDE_EFFECT_TIER_VIOLATION",
            format!(
                "SELF_ONLY action cannot mutate traversal target '{}'; use RELATED_NODES or EXTERNAL_EFFECT",
                mstep.target
            ),
        );
    }

    match parsed {
        ParsedMutationTarget::SelfField { field } => {
            check_target_field_and_expr(
                (
                    &ctx.nt.name,
                    &ctx.nt.properties,
                    &protected_node_metrics(ctx.nt),
                ),
                &field,
                mstep,
                m_path,
                &ctx.base_env,
                issues,
            );
        }
        ParsedMutationTarget::NeighborNode {
            link_type,
            direction,
            field,
        } => {
            validate_neighbor_mutation(ctx, (&link_type, direction, &field), mstep, m_path, issues);
        }
        ParsedMutationTarget::IncidentEdge {
            link_type,
            direction,
            field,
        } => {
            validate_edge_mutation(ctx, (&link_type, direction, &field), mstep, m_path, issues);
        }
    }
}

fn validate_neighbor_mutation(
    ctx: &ActionValidateCtx<'_>,
    target_spec: (&str, LinkDirection, &str),
    mstep: &crate::studio::MutationStepSpec,
    m_path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let (link_type, direction, field) = target_spec;
    let Some(edge) = ctx.doc.edge_types.iter().find(|e| {
        e.name == link_type
            && match direction {
                LinkDirection::Out => e.src_type_name == ctx.nt.name,
                LinkDirection::In => e.dst_type_name == ctx.nt.name,
            }
    }) else {
        push_err(
            issues,
            format!("{m_path}.target"),
            "ACTION_TARGET_INVALID",
            format!(
                "edge '{}' ({}) does not connect from '{}'",
                link_type,
                direction.as_str(),
                ctx.nt.name
            ),
        );
        return;
    };

    let nb_type_name = match direction {
        LinkDirection::Out => &edge.dst_type_name,
        LinkDirection::In => &edge.src_type_name,
    };
    let Some(&nb_nt) = ctx.effective_types.get(nb_type_name) else {
        push_err(
            issues,
            format!("{m_path}.target"),
            "ACTION_TARGET_INVALID",
            format!("neighbor node type '{nb_type_name}' not found"),
        );
        return;
    };

    let mut step_env = ctx.base_env.clone();
    add_scoped_props(&mut step_env, &nb_nt.properties, "target", true);
    check_target_field_and_expr(
        (
            &nb_nt.name,
            &nb_nt.properties,
            &protected_node_metrics(nb_nt),
        ),
        field,
        mstep,
        m_path,
        &step_env,
        issues,
    );
}

fn validate_edge_mutation(
    ctx: &ActionValidateCtx<'_>,
    target_spec: (&str, LinkDirection, &str),
    mstep: &crate::studio::MutationStepSpec,
    m_path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let (link_type, direction, field) = target_spec;
    let Some(edge) = ctx.doc.edge_types.iter().find(|e| {
        e.name == link_type
            && match direction {
                LinkDirection::Out => e.src_type_name == ctx.nt.name,
                LinkDirection::In => e.dst_type_name == ctx.nt.name,
            }
    }) else {
        push_err(
            issues,
            format!("{m_path}.target"),
            "ACTION_TARGET_INVALID",
            format!(
                "edge '{}' ({}) is not incident to '{}'",
                link_type,
                direction.as_str(),
                ctx.nt.name
            ),
        );
        return;
    };

    let mut step_env = ctx.base_env.clone();
    step_env.metrics.insert(String::from("weight"));
    step_env
        .properties
        .insert(String::from("edge.weight"), Some(ExprType::Number));
    add_scoped_props(&mut step_env, &edge.properties, "edge", true);

    if field == "weight" {
        check_step_expr_and_filter(ExprType::Number, mstep, m_path, &step_env, issues);
        return;
    }

    check_target_field_and_expr(
        (&edge.name, &edge.properties, &protected_edge_metrics(edge)),
        field,
        mstep,
        m_path,
        &step_env,
        issues,
    );
}

fn check_target_field_and_expr(
    owner_meta: (&str, &[PropertySpec], &BTreeSet<&str>),
    field: &str,
    mstep: &crate::studio::MutationStepSpec,
    m_path: &str,
    env: &ExprTypeEnv,
    issues: &mut Vec<ValidationIssue>,
) {
    let (owner_name, props, protected) = owner_meta;
    if protected.contains(field) {
        push_err(
            issues,
            format!("{m_path}.target"),
            "DERIVED_METRIC_WRITE_FORBIDDEN",
            format!(
                "field '{owner_name}.{field}' is produced by a derived or aggregation rule and cannot be mutated directly"
            ),
        );
        return;
    }

    let Some(prop) = props.iter().find(|p| p.name == field) else {
        push_err(
            issues,
            format!("{m_path}.target"),
            "ACTION_TARGET_INVALID",
            format!("field '{field}' is not declared on '{owner_name}'"),
        );
        return;
    };

    let expected_type = if prop.kind == PropertyKind::Metric {
        ExprType::Number
    } else {
        param_expr_type(prop.data_type).unwrap_or(ExprType::Text)
    };
    check_step_expr_and_filter(expected_type, mstep, m_path, env, issues);
}

fn check_step_expr_and_filter(
    expected_type: ExprType,
    mstep: &crate::studio::MutationStepSpec,
    m_path: &str,
    env: &ExprTypeEnv,
    issues: &mut Vec<ValidationIssue>,
) {
    match parse_expr(&mstep.expression).and_then(|ast| typecheck_detailed(&ast, env)) {
        Ok(info) if info.result_type == expected_type || info.result_type == ExprType::Null => {}
        Ok(info) => push_err(
            issues,
            format!("{m_path}.expression"),
            "EXPR_INVALID",
            format!(
                "mutation expression must evaluate to {expected_type:?}, got {:?}",
                info.result_type
            ),
        ),
        Err(e) => push_err(
            issues,
            format!("{m_path}.expression"),
            "EXPR_INVALID",
            e.message,
        ),
    }

    if let Some(filter_raw) = mstep
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match parse_expr(filter_raw).and_then(|ast| typecheck_detailed(&ast, env)) {
            Ok(info) if matches!(info.result_type, ExprType::Bool | ExprType::Null) => {}
            Ok(info) => push_err(
                issues,
                format!("{m_path}.filter"),
                "EXPR_INVALID",
                format!(
                    "mutation filter must evaluate to BOOLEAN, got {:?}",
                    info.result_type
                ),
            ),
            Err(e) => push_err(
                issues,
                format!("{m_path}.filter"),
                "EXPR_INVALID",
                e.message,
            ),
        }
    }
}

/// Appends `WARN` diagnostics (`code = "DEAD_KNOWLEDGE"`) for unconsumed `derived_rules` targets (`04-STEP26` §26.4).
pub(super) fn append_dead_knowledge_warnings(doc: &SchemaDoc, issues: &mut Vec<ValidationIssue>) {
    for dk in scan_dead_knowledge(doc) {
        let path = find_dead_knowledge_path(doc, &dk.owner_type, &dk.metric);
        issues.push(ValidationIssue {
            severity: IssueSeverity::Warn,
            path,
            code: String::from("DEAD_KNOWLEDGE"),
            message: dk.message,
        });
    }
}

fn find_dead_knowledge_path(doc: &SchemaDoc, owner: &str, metric: &str) -> String {
    for (n_idx, nt) in doc.node_types.iter().enumerate() {
        if nt.name == owner {
            for (d_idx, dr) in nt.derived_rules.iter().enumerate() {
                if dr.enabled && dr.target_metric == metric {
                    return format!("node_types[{n_idx}].derived_rules[{d_idx}].target_metric");
                }
            }
        }
    }
    for (e_idx, et) in doc.edge_types.iter().enumerate() {
        if et.name == owner {
            for (d_idx, dr) in et.derived_rules.iter().enumerate() {
                if dr.enabled && dr.target_metric == metric {
                    return format!("edge_types[{e_idx}].derived_rules[{d_idx}].target_metric");
                }
            }
        }
    }
    format!("{owner}.{metric}")
}
