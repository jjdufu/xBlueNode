//! Pure in-memory runtime scope binding and topological metric recomputation for dev sandboxes.

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::enums::AggFn;
use axpoint_domain::expr::{EvalEnv, Expr, ExprValue, eval as eval_expr, parse as parse_expr};
use axpoint_domain::graph::LinkRecord;
use axpoint_domain::rollup::{MetricDependencyPlan, MetricRuleStep};
use axpoint_domain::studio::{AggRuleSpec, LinkDirection, SchemaDoc};
use axpoint_domain::topology::BlueNode;
use uuid::Uuid;

pub(super) fn add_runtime_scope(
    env: &mut EvalEnv,
    prefix: &str,
    metrics: &BTreeMap<String, f64>,
    props: &serde_json::Map<String, serde_json::Value>,
    include_bare: bool,
) {
    for (k, &v) in metrics {
        if !v.is_finite() {
            continue;
        }
        let nv = if v == 0.0 { 0.0 } else { v };
        if include_bare {
            env.metrics.insert(k.clone(), nv);
        }
        env.properties
            .insert(format!("{prefix}.{k}"), ExprValue::Number(nv));
        env.properties
            .insert(format!("{prefix}.metric.{k}"), ExprValue::Number(nv));
    }
    for (k, v) in props {
        if k.starts_with("__") {
            continue;
        }
        let ev = match v {
            serde_json::Value::Number(n) => n
                .as_f64()
                .filter(|f| f.is_finite())
                .map_or(ExprValue::Null, ExprValue::Number),
            serde_json::Value::String(s) => ExprValue::Text(s.clone()),
            serde_json::Value::Bool(b) => ExprValue::Bool(*b),
            serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => ExprValue::Null,
        };
        if include_bare {
            env.properties.insert(k.clone(), ev.clone());
            env.properties.insert(format!("prop.{k}"), ev.clone());
        }
        env.properties.insert(format!("{prefix}.{k}"), ev.clone());
        env.properties.insert(format!("{prefix}.prop.{k}"), ev);
    }
}

pub(super) fn build_self_action_env(
    metrics: &BTreeMap<String, f64>,
    props: &serde_json::Map<String, serde_json::Value>,
    param_env: &BTreeMap<String, ExprValue>,
) -> EvalEnv {
    let mut env = EvalEnv::default();
    add_runtime_scope(&mut env, "self", metrics, props, true);
    for (k, v) in param_env {
        env.properties.insert(k.clone(), v.clone());
    }
    env
}

pub(super) fn parse_optional_filter(raw: Option<&str>) -> Result<Option<Expr>, AppError> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    parse_expr(trimmed)
        .map(Some)
        .map_err(|e| AppError::bad("EXPR_INVALID", e.message))
}

pub(super) fn recompute_sandbox_rules_in_memory(
    doc: &SchemaDoc,
    nodes_by_id: &mut BTreeMap<Uuid, BlueNode>,
    dev_links: &[LinkRecord],
) -> usize {
    let layers = MetricDependencyPlan::from_schema(doc).layers();
    let mut recomputed = 0_usize;
    for layer in &layers {
        for step in layer {
            recomputed =
                recomputed.saturating_add(apply_rule_step_in_memory(step, nodes_by_id, dev_links));
        }
    }
    recomputed
}

fn apply_rule_step_in_memory(
    step: &MetricRuleStep,
    nodes_by_id: &mut BTreeMap<Uuid, BlueNode>,
    dev_links: &[LinkRecord],
) -> usize {
    let target_v = step.target_vertex();
    let target_ids: Vec<Uuid> = nodes_by_id
        .values()
        .filter(|n| n.type_name == target_v.type_name)
        .map(|n| n.id)
        .collect();

    let mut updates: Vec<(Uuid, f64)> = Vec::new();
    match step {
        MetricRuleStep::Derived { expression, .. } => {
            let Ok(ast) = parse_expr(expression) else {
                return 0;
            };
            for nid in target_ids {
                if let Some(n) = nodes_by_id.get(&nid) {
                    let env = EvalEnv::from_node(&n.metrics, &n.properties);
                    if let ExprValue::Number(v) = eval_expr(&ast, &env).value
                        && v.is_finite()
                    {
                        updates.push((nid, v));
                    }
                }
            }
        }
        MetricRuleStep::Aggregation { rule, .. } => {
            for nid in target_ids {
                if let Some(val) = eval_agg_in_memory(nid, rule, nodes_by_id, dev_links) {
                    updates.push((nid, val));
                }
            }
        }
    }

    let mut written = 0_usize;
    for (nid, val) in updates {
        if let Some(n) = nodes_by_id.get_mut(&nid) {
            n.metrics.insert(target_v.metric.clone(), val);
            written = written.saturating_add(1);
        }
    }
    written
}

fn eval_agg_in_memory(
    parent_id: Uuid,
    rule: &AggRuleSpec,
    nodes_by_id: &BTreeMap<Uuid, BlueNode>,
    dev_links: &[LinkRecord],
) -> Option<f64> {
    let filter_ast = rule
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|f| parse_expr(f).ok());

    let mut vals: Vec<(f64, f64)> = Vec::new();
    let mut matched_children = 0_u32;
    for l in dev_links {
        if l.link_type != rule.via_link_type {
            continue;
        }
        let child_id = match rule.direction {
            LinkDirection::Out if l.src_node_id == parent_id => l.dst_node_id,
            LinkDirection::In if l.dst_node_id == parent_id => l.src_node_id,
            _ => continue,
        };
        let Some(child) = nodes_by_id.get(&child_id) else {
            continue;
        };
        if child.type_name != rule.source_type_name {
            continue;
        }
        if let Some(ref ast) = filter_ast {
            let env = EvalEnv::from_node(&child.metrics, &child.properties);
            if !matches!(eval_expr(ast, &env).value, ExprValue::Bool(true)) {
                continue;
            }
        }
        matched_children = matched_children.saturating_add(1);
        if let Some(&v) = child.metrics.get(&rule.source_metric)
            && v.is_finite()
        {
            vals.push((v, l.weight.max(0.0)));
        }
    }

    match rule.op {
        AggFn::Count => Some(f64::from(matched_children)),
        AggFn::Sum => Some(vals.iter().fold(0.0, |acc, &(v, w)| {
            acc + if rule.weight_by_link { v * w } else { v }
        })),
        AggFn::Avg if !vals.is_empty() => {
            if rule.weight_by_link {
                let w_sum: f64 = vals.iter().map(|&(_, w)| w).sum();
                (w_sum > 0.0).then(|| vals.iter().map(|&(v, w)| v * w).sum::<f64>() / w_sum)
            } else {
                let cnt = u32::try_from(vals.len()).ok()?;
                Some(vals.iter().map(|&(v, _)| v).sum::<f64>() / f64::from(cnt))
            }
        }
        AggFn::Min => vals.iter().map(|&(v, _)| v).reduce(f64::min),
        AggFn::Max => vals.iter().map(|&(v, _)| v).reduce(f64::max),
        AggFn::Last => vals.last().map(|&(v, _)| v),
        _ => None,
    }
}
