//! Static 8-rule `AxFlow` DAG, operator schema, template, and trigger validator (`07-STEP07-08` §07.2).

use std::collections::HashSet;
use std::str::FromStr;

use axpoint_domain::enums::{FlowOperator, OutboxTopic, TriggerType};
use axpoint_domain::expr::parser as expr_parser;
use axpoint_domain::flow::{
    FlowEdge, FlowGraph, FlowIssue, FlowIssueSeverity, FlowNode, OperatorRegistry, TriggerCfg,
    ValidateFlowResponse,
};

use crate::graph::{ancestors, root_nodes, topological_order_or_cycle};
use crate::operators::action_emit::contains_unsupported_work_item;
use crate::operators::step07_schema_only_registry;
use crate::scheduler::{parse_timezone_offset, to_six_field_cron};
use crate::template::collect_config_template_errors;

const MAX_NODES: usize = 200;
const MAX_EDGES: usize = 1_000;

/// Runs all 8 static validation rules on `(graph, trigger_cfg, slug)` using the Step 07 operator schema registry.
#[must_use]
pub fn validate_flow_default(
    graph: &FlowGraph,
    trigger_cfg: &TriggerCfg,
    slug: Option<&str>,
) -> ValidateFlowResponse {
    let reg = step07_schema_only_registry();
    validate_flow(graph, trigger_cfg, slug, &reg)
}

/// Runs all 8 static validation rules on `(graph, trigger_cfg, slug)` against `registry` (`07-STEP07-08` §07.2).
#[must_use]
pub fn validate_flow(
    graph: &FlowGraph,
    trigger_cfg: &TriggerCfg,
    slug: Option<&str>,
    registry: &OperatorRegistry,
) -> ValidateFlowResponse {
    let mut issues = Vec::new();

    // Rule 1: Node count (1..=200), unique valid IDs, retry & timeout bounds.
    let known_ids = check_rule1_nodes(&graph.nodes, &mut issues);

    // Rule 2: Edge count (<= 1000), valid endpoints, no self-loops, no duplicate (from, to).
    check_rule2_edges(&graph.edges, &known_ids, &mut issues);

    // Rule 3: Kahn DAG cycle detection -> CYCLE_DETECTED.
    if let Err(cycle_path) = topological_order_or_cycle(graph) {
        let first_node = cycle_path.first().cloned();
        issues.push(error_issue(
            "CYCLE_DETECTED",
            format!("directed cycle detected: {}", cycle_path.join(" -> ")),
            first_node,
        ));
    }

    // Rule 4: At least one root node (in_degree == 0).
    if !graph.nodes.is_empty() && root_nodes(graph).is_empty() {
        issues.push(error_issue(
            "INVALID_ARGUMENT",
            "flow DAG must have at least one root node with zero incoming edges",
            None,
        ));
    }

    // Rule 5 & Rule 6: Operator registration (UNSUPPORTED_FORMAT) and JSON Schema (SCHEMA_INVALID).
    check_rule5_and_6_operators(&graph.nodes, registry, &mut issues);

    // Rule 7: Edge `when` expressions and node `{{ ... }}` ancestor templates -> EXPR_INVALID.
    check_rule7_expressions_and_templates(graph, &mut issues);

    // Rule 8: Trigger configuration validity.
    check_rule8_trigger(trigger_cfg, slug, &mut issues);

    let valid = !issues
        .iter()
        .any(|issue| issue.severity == FlowIssueSeverity::Error);
    ValidateFlowResponse { valid, issues }
}

fn check_rule1_nodes(nodes: &[FlowNode], issues: &mut Vec<FlowIssue>) -> HashSet<String> {
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        issues.push(error_issue(
            "INVALID_ARGUMENT",
            format!(
                "flow must contain between 1 and {MAX_NODES} nodes, got {}",
                nodes.len()
            ),
            None,
        ));
    }

    let mut seen = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if !is_valid_node_id(&node.id) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!("node id '{}' must match ^[a-z][a-z0-9_]{{0,31}}$", node.id),
                Some(node.id.clone()),
            ));
        }
        if !seen.insert(node.id.clone()) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!("duplicate node id '{}'", node.id),
                Some(node.id.clone()),
            ));
        }
        if !(1..=10).contains(&node.retry.max_attempts) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!(
                    "node '{}' retry.max_attempts must be in 1..=10, got {}",
                    node.id, node.retry.max_attempts
                ),
                Some(node.id.clone()),
            ));
        }
        if !(1..=3_600).contains(&node.retry.backoff_s) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!(
                    "node '{}' retry.backoff_s must be in 1..=3600, got {}",
                    node.id, node.retry.backoff_s
                ),
                Some(node.id.clone()),
            ));
        }
        if !(1..=86_400).contains(&node.timeout_s) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!(
                    "node '{}' timeout_s must be in 1..=86400, got {}",
                    node.id, node.timeout_s
                ),
                Some(node.id.clone()),
            ));
        }
    }
    seen
}

fn check_rule2_edges(edges: &[FlowEdge], known_ids: &HashSet<String>, issues: &mut Vec<FlowIssue>) {
    if edges.len() > MAX_EDGES {
        issues.push(error_issue(
            "INVALID_ARGUMENT",
            format!(
                "flow edges count must be <= {MAX_EDGES}, got {}",
                edges.len()
            ),
            None,
        ));
    }

    let mut seen_pairs: HashSet<(&str, &str)> = HashSet::with_capacity(edges.len());
    for edge in edges {
        if !known_ids.contains(&edge.from) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!("edge source node '{}' does not exist", edge.from),
                None,
            ));
        }
        if !known_ids.contains(&edge.to) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!("edge target node '{}' does not exist", edge.to),
                None,
            ));
        }
        if edge.from == edge.to {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!("self-loop edge on node '{}' is not allowed", edge.from),
                Some(edge.from.clone()),
            ));
        }
        if !seen_pairs.insert((edge.from.as_str(), edge.to.as_str())) {
            issues.push(error_issue(
                "INVALID_ARGUMENT",
                format!("duplicate edge '{} -> {}'", edge.from, edge.to),
                Some(edge.to.clone()),
            ));
        }
    }
}

fn check_rule5_and_6_operators(
    nodes: &[FlowNode],
    registry: &OperatorRegistry,
    issues: &mut Vec<FlowIssue>,
) {
    for node in nodes {
        let Some(op) = registry.get(node.kind) else {
            issues.push(error_issue(
                "UNSUPPORTED_FORMAT",
                format!(
                    "operator '{}' is not registered in the current engine stage",
                    node.kind
                ),
                Some(node.id.clone()),
            ));
            continue;
        };

        if node.kind == FlowOperator::ActionEmit && contains_unsupported_work_item(&node.config) {
            issues.push(error_issue(
                "UNSUPPORTED_FORMAT",
                "WORK_ITEM action is deferred until Step 14",
                Some(node.id.clone()),
            ));
            continue;
        }

        let schema = op.config_schema();
        match jsonschema::validator_for(&schema) {
            Ok(validator) => {
                let err_msgs: Vec<String> = validator
                    .iter_errors(&node.config)
                    .take(5)
                    .map(|e| e.to_string())
                    .collect();
                if !err_msgs.is_empty() {
                    issues.push(error_issue(
                        "SCHEMA_INVALID",
                        format!(
                            "node '{}' ({}) invalid config: {}",
                            node.id,
                            node.kind,
                            err_msgs.join("; ")
                        ),
                        Some(node.id.clone()),
                    ));
                }
            }
            Err(err) => {
                issues.push(error_issue(
                    "SCHEMA_INVALID",
                    format!("failed to compile operator schema for {}: {err}", node.kind),
                    Some(node.id.clone()),
                ));
            }
        }
    }
}

fn check_rule7_expressions_and_templates(graph: &FlowGraph, issues: &mut Vec<FlowIssue>) {
    for edge in &graph.edges {
        if let Some(when_expr) = &edge.when
            && let Err(expr_issue) = expr_parser::parse_str(when_expr)
        {
            issues.push(error_issue(
                "EXPR_INVALID",
                format!(
                    "edge '{} -> {}' invalid when expression: {}",
                    edge.from, edge.to, expr_issue.message
                ),
                Some(edge.to.clone()),
            ));
        }
    }

    for node in &graph.nodes {
        let anc = ancestors(graph, &node.id);
        for err_msg in collect_config_template_errors(&node.config, &anc) {
            issues.push(error_issue(
                "EXPR_INVALID",
                format!("node '{}' template error: {err_msg}", node.id),
                Some(node.id.clone()),
            ));
        }
    }
}

fn check_rule8_trigger(trigger_cfg: &TriggerCfg, slug: Option<&str>, issues: &mut Vec<FlowIssue>) {
    match trigger_cfg.kind {
        TriggerType::Manual
        | TriggerType::Api
        | TriggerType::Mcp
        | TriggerType::Rule
        | TriggerType::SimApply => {}
        TriggerType::Cron => {
            let Some(cron_raw) = trigger_cfg.cron.as_deref().filter(|s| !s.trim().is_empty())
            else {
                issues.push(error_issue(
                    "INVALID_ARGUMENT",
                    "CRON trigger requires non-empty 'cron' expression",
                    None,
                ));
                return;
            };
            if let Err(err) = to_six_field_cron(cron_raw) {
                issues.push(error_issue("INVALID_ARGUMENT", err.client_message(), None));
            }
            if let Some(tz_raw) = trigger_cfg
                .timezone
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                && let Err(err) = parse_timezone_offset(tz_raw)
            {
                issues.push(error_issue("INVALID_ARGUMENT", err.client_message(), None));
            }
        }
        TriggerType::OutboxEvent => {
            let Some(topic_raw) = trigger_cfg
                .topic
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                issues.push(error_issue(
                    "INVALID_ARGUMENT",
                    "OUTBOX_EVENT trigger requires non-empty 'topic'",
                    None,
                ));
                return;
            };
            if OutboxTopic::from_str(topic_raw).is_err() {
                issues.push(error_issue(
                    "INVALID_ARGUMENT",
                    format!("unknown OutboxTopic '{topic_raw}'"),
                    None,
                ));
            }
            if let Some(filter_raw) = trigger_cfg
                .filter
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                && let Err(expr_issue) = expr_parser::parse_str(filter_raw)
            {
                issues.push(error_issue(
                    "EXPR_INVALID",
                    format!(
                        "OUTBOX_EVENT invalid filter expression: {}",
                        expr_issue.message
                    ),
                    None,
                ));
            }
        }
        TriggerType::Webhook => {
            if slug.is_none_or(|s| s.trim().is_empty()) {
                issues.push(error_issue(
                    "INVALID_ARGUMENT",
                    "WEBHOOK trigger requires flow 'slug' to be set",
                    None,
                ));
            }
        }
    }
}

fn is_valid_node_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 32 || !first.is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn error_issue(
    code: &'static str,
    message: impl Into<String>,
    node_id: Option<String>,
) -> FlowIssue {
    FlowIssue {
        code: String::from(code),
        message: message.into(),
        severity: FlowIssueSeverity::Error,
        node_id,
    }
}
