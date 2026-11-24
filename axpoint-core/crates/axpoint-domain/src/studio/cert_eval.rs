//! Pure `ActionSpec` target/JSON-Schema helpers, Dead Knowledge closure analysis,
//! and L0–L4 runtime readiness level evaluation (`04-STEP26` §26.2 & §26.4).

use std::collections::BTreeSet;
use std::str::FromStr;

use crate::enums::{Cardinality, DataType, Permission, RuntimeCertLevel};
use crate::expr::{collect_references, parse as parse_expr};
use crate::studio::cert::{ActionParamSpec, ActionSpec, DeadKnowledgeIssue};
use crate::studio::doc::{IssueSeverity, LinkDirection, SchemaDoc, ValidationIssue};

/// Resolves `ActionSpec.required_permission` fail-closed (`"ACTION_EXECUTE"` aliases [`Permission::FlowRun`]).
///
/// # Errors
/// Returns the trimmed invalid permission string when `raw` does not match `"ACTION_EXECUTE"`
/// or a canonical [`Permission`] variant.
pub fn resolve_action_permission(raw: &str) -> Result<Permission, String> {
    let trimmed = raw.trim();
    if trimmed == "ACTION_EXECUTE" {
        return Ok(Permission::FlowRun);
    }
    Permission::from_str(trimmed).map_err(|_| String::from(trimmed))
}

/// Parsed destination of a [`crate::studio::MutationStepSpec::target`] string (`04-STEP26` §26.2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedMutationTarget {
    /// Mutates a property or non-derived metric on the target node instance itself (`self.<field>`).
    SelfField {
        /// Property or metric identifier on the owning `NodeTypeSpec`.
        field: String,
    },
    /// Mutates a property or non-derived metric on traversed 1-hop neighbor nodes (`via(...)`).
    NeighborNode {
        /// Traversed `EdgeTypeSpec.name`.
        link_type: String,
        /// Traversal direction relative to the action's owning node.
        direction: LinkDirection,
        /// Property or metric identifier on the neighbor `NodeTypeSpec`.
        field: String,
    },
    /// Mutates a property or non-derived metric on incident relationship links (`edge(...)`).
    IncidentEdge {
        /// Incident `EdgeTypeSpec.name`.
        link_type: String,
        /// Direction of the incident link relative to the action's owning node.
        direction: LinkDirection,
        /// Property or metric identifier on the `EdgeTypeSpec`.
        field: String,
    },
}

/// Parses a `MutationStepSpec.target` path (`self.<field>`, `via(<EDGE>[, OUT|IN]).<field>`,
/// `via_in(<EDGE>).<field>`, `edge(<EDGE>[, OUT|IN]).<field>`, or `edge_in(<EDGE>).<field>`).
///
/// # Errors
/// Returns a descriptive error string when `raw` does not match a valid mutation target path.
pub fn parse_mutation_target(
    raw: &str,
    owner_node_type: &str,
    doc: &SchemaDoc,
) -> Result<ParsedMutationTarget, String> {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed
        .strip_prefix("self.prop.")
        .or_else(|| trimmed.strip_prefix("self.metric."))
        .or_else(|| trimmed.strip_prefix("self."))
    {
        if !is_valid_field_ident(rest) {
            return Err(format!(
                "invalid self mutation field '{rest}' in '{trimmed}'"
            ));
        }
        return Ok(ParsedMutationTarget::SelfField {
            field: String::from(rest),
        });
    }

    for (prefix, forced_dir, is_edge) in [
        ("via_in(", Some(LinkDirection::In), false),
        ("via(", None, false),
        ("edge_in(", Some(LinkDirection::In), true),
        ("edge(", None, true),
    ] {
        if let Some(after_prefix) = trimmed.strip_prefix(prefix) {
            return parse_traversal_target(
                trimmed,
                after_prefix,
                forced_dir,
                is_edge,
                owner_node_type,
                doc,
            );
        }
    }

    Err(format!(
        "unsupported mutation target '{trimmed}'; expected 'self.<field>', 'via(<EDGE>).<field>', or 'edge(<EDGE>).<field>'"
    ))
}

fn parse_traversal_target(
    full: &str,
    after_prefix: &str,
    forced_dir: Option<LinkDirection>,
    is_edge: bool,
    owner_node_type: &str,
    doc: &SchemaDoc,
) -> Result<ParsedMutationTarget, String> {
    let Some((inside_parens, tail)) = after_prefix.split_once(").") else {
        return Err(format!(
            "mutation target '{full}' must match '<via|edge>(<EDGE_TYPE>).<field>'"
        ));
    };
    if !is_valid_field_ident(tail) {
        return Err(format!("invalid field identifier '{tail}' in '{full}'"));
    }

    let (link_type, explicit_dir) = if let Some((lt_raw, dir_raw)) = inside_parens.split_once(',') {
        if forced_dir.is_some() {
            return Err(format!(
                "direction cannot be specified twice in mutation target '{full}'"
            ));
        }
        let dir = match dir_raw.trim() {
            "OUT" => LinkDirection::Out,
            "IN" => LinkDirection::In,
            other => {
                return Err(format!(
                    "invalid direction '{other}' in '{full}'; expected OUT or IN"
                ));
            }
        };
        (lt_raw.trim(), Some(dir))
    } else {
        (inside_parens.trim(), forced_dir)
    };

    if link_type.is_empty() {
        return Err(format!("empty edge type in mutation target '{full}'"));
    }

    let direction = explicit_dir.unwrap_or_else(|| {
        let has_out = doc
            .edge_types
            .iter()
            .any(|e| e.name == link_type && e.src_type_name == owner_node_type);
        let has_in = doc
            .edge_types
            .iter()
            .any(|e| e.name == link_type && e.dst_type_name == owner_node_type);
        if !has_out && has_in {
            LinkDirection::In
        } else {
            LinkDirection::Out
        }
    });

    if is_edge {
        Ok(ParsedMutationTarget::IncidentEdge {
            link_type: String::from(link_type),
            direction,
            field: String::from(tail),
        })
    } else {
        Ok(ParsedMutationTarget::NeighborNode {
            link_type: String::from(link_type),
            direction,
            field: String::from(tail),
        })
    }
}

fn is_valid_field_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    bytes.len() <= 63
        && first.is_ascii_lowercase()
        && bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Builds a strict JSON Schema Draft 2020-12 object schema for [`ActionSpec::parameters`].
#[must_use]
pub fn build_action_input_json_schema(action: &ActionSpec) -> serde_json::Value {
    let mut props = serde_json::Map::with_capacity(action.parameters.len());
    let mut required = Vec::new();
    for param in &action.parameters {
        props.insert(param.name.clone(), json_schema_for_param(param));
        if param.required && param.default_value.is_none() {
            required.push(serde_json::Value::String(param.name.clone()));
        }
    }
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn json_schema_for_param(param: &ActionParamSpec) -> serde_json::Value {
    let mut obj = match param.data_type {
        DataType::Int64 => serde_json::json!({ "type": "integer" }),
        DataType::Float64 | DataType::Decimal => serde_json::json!({ "type": "number" }),
        DataType::Boolean => serde_json::json!({ "type": "boolean" }),
        DataType::String | DataType::Date | DataType::Timestamp | DataType::Binary => {
            serde_json::json!({ "type": "string" })
        }
        DataType::List => serde_json::json!({ "type": "array" }),
        DataType::Json | DataType::Struct => serde_json::json!({ "type": "object" }),
    };
    if !param.description.trim().is_empty()
        && let Some(map) = obj.as_object_mut()
    {
        map.insert(
            String::from("description"),
            serde_json::Value::String(param.description.clone()),
        );
    } else if !param.display_name.trim().is_empty()
        && let Some(map) = obj.as_object_mut()
    {
        map.insert(
            String::from("description"),
            serde_json::Value::String(param.display_name.clone()),
        );
    }
    obj
}

/// Scans `doc` for isolated `derived_rules` targets not consumed by any downstream rule,
/// edge dynamic formula, `ActionSpec` precondition/mutation, or propagation spec (`DEAD_KNOWLEDGE`, `04-STEP26` §26.4).
#[must_use]
pub fn scan_dead_knowledge(doc: &SchemaDoc) -> Vec<DeadKnowledgeIssue> {
    let empty = BTreeSet::new();
    scan_dead_knowledge_with_external(doc, &empty)
}

/// Scans `doc` for unconsumed `derived_rules` targets while accounting for external `(owner_type, metric)` consumers
/// such as `ax_rules`, `xv_charts`, and `xv_widgets` (`04-STEP26` §26.4).
#[must_use]
pub fn scan_dead_knowledge_with_external(
    doc: &SchemaDoc,
    external_consumed: &BTreeSet<(String, String)>,
) -> Vec<DeadKnowledgeIssue> {
    let mut consumed = external_consumed.clone();
    collect_schema_metric_consumers(doc, &mut consumed);

    let mut out = Vec::new();
    for nt in &doc.node_types {
        for drule in &nt.derived_rules {
            if !drule.enabled || drule.target_metric.trim().is_empty() {
                continue;
            }
            if !consumed.contains(&(nt.name.clone(), drule.target_metric.clone())) {
                out.push(DeadKnowledgeIssue {
                    owner_type: nt.name.clone(),
                    metric: drule.target_metric.clone(),
                    producer_kind: String::from("DERIVED"),
                    message: format!(
                        "derived metric '{}.{}' is not consumed by any downstream rule, edge dynamic, action contract, or monitor",
                        nt.name, drule.target_metric
                    ),
                });
            }
        }
    }

    for et in &doc.edge_types {
        for drule in &et.derived_rules {
            if !drule.enabled || drule.target_metric.trim().is_empty() {
                continue;
            }
            if !consumed.contains(&(et.name.clone(), drule.target_metric.clone())) {
                out.push(DeadKnowledgeIssue {
                    owner_type: et.name.clone(),
                    metric: drule.target_metric.clone(),
                    producer_kind: String::from("DERIVED"),
                    message: format!(
                        "edge derived metric '{}.{}' is not consumed by any downstream rule, action contract, or monitor",
                        et.name, drule.target_metric
                    ),
                });
            }
        }
    }

    out
}

fn collect_schema_metric_consumers(doc: &SchemaDoc, consumed: &mut BTreeSet<(String, String)>) {
    for nt in &doc.node_types {
        for drule in &nt.derived_rules {
            if drule.enabled {
                record_self_expr_refs(&nt.name, &drule.expression, consumed);
            }
        }
        for arule in &nt.agg_rules {
            let src_owner = if arule.source_type_name.trim().is_empty() {
                arule.via_link_type.as_str()
            } else {
                arule.source_type_name.as_str()
            };
            if !src_owner.is_empty() && !arule.source_metric.is_empty() {
                consumed.insert((String::from(src_owner), arule.source_metric.clone()));
            }
            if let Some(filter_raw) = &arule.filter {
                record_self_expr_refs(src_owner, filter_raw, consumed);
            }
        }
        collect_action_metric_consumers(doc, &nt.name, &nt.actions, consumed);
    }

    for et in &doc.edge_types {
        for drule in &et.derived_rules {
            if drule.enabled {
                record_edge_expr_refs(
                    &et.name,
                    &et.src_type_name,
                    &et.dst_type_name,
                    &drule.expression,
                    consumed,
                );
            }
        }
        for arule in &et.agg_rules {
            if !arule.via_link_type.is_empty() && !arule.source_metric.is_empty() {
                consumed.insert((arule.via_link_type.clone(), arule.source_metric.clone()));
            }
            if let Some(filter_raw) = &arule.filter {
                record_self_expr_refs(&arule.via_link_type, filter_raw, consumed);
            }
        }
        collect_propagation_json_refs(
            &et.propagation,
            &et.name,
            &et.src_type_name,
            &et.dst_type_name,
            consumed,
        );
    }
}

fn collect_action_metric_consumers(
    doc: &SchemaDoc,
    nt_name: &str,
    actions: &[ActionSpec],
    consumed: &mut BTreeSet<(String, String)>,
) {
    for action in actions {
        if let Some(pre) = &action.precondition_expr {
            record_self_expr_refs(nt_name, pre, consumed);
        }
        for mstep in &action.mutations {
            record_self_expr_refs(nt_name, &mstep.expression, consumed);
            if let Some(f) = &mstep.filter {
                record_self_expr_refs(nt_name, f, consumed);
            }
            if let Ok(parsed) = parse_mutation_target(&mstep.target, nt_name, doc)
                && let Some(target_owner) = resolve_mutation_owner(doc, nt_name, &parsed)
            {
                record_self_expr_refs(&target_owner, &mstep.expression, consumed);
                if let Some(f) = &mstep.filter {
                    record_self_expr_refs(&target_owner, f, consumed);
                }
            }
        }
    }
}

fn resolve_mutation_owner(
    doc: &SchemaDoc,
    nt_name: &str,
    target: &ParsedMutationTarget,
) -> Option<String> {
    match target {
        ParsedMutationTarget::SelfField { .. } => Some(String::from(nt_name)),
        ParsedMutationTarget::IncidentEdge { link_type, .. } => Some(link_type.clone()),
        ParsedMutationTarget::NeighborNode {
            link_type,
            direction,
            ..
        } => doc.edge_types.iter().find_map(|e| {
            if e.name != *link_type {
                return None;
            }
            match direction {
                LinkDirection::Out if e.src_type_name == nt_name => Some(e.dst_type_name.clone()),
                LinkDirection::In if e.dst_type_name == nt_name => Some(e.src_type_name.clone()),
                _ => None,
            }
        }),
    }
}

fn record_self_expr_refs(owner: &str, raw: &str, consumed: &mut BTreeSet<(String, String)>) {
    if owner.is_empty() || raw.trim().is_empty() {
        return;
    }
    let Ok(ast) = parse_expr(raw) else {
        return;
    };
    let (metrics, props) = collect_references(&ast);
    for m in metrics {
        consumed.insert((String::from(owner), m));
    }
    for p in props {
        if p.starts_with("self.prop.")
            || p.starts_with("target.prop.")
            || p.starts_with("edge.prop.")
        {
            continue;
        }
        if let Some(leaf) = p
            .strip_prefix("self.metric.")
            .or_else(|| p.strip_prefix("self."))
            .or_else(|| p.strip_prefix("target.metric."))
            .or_else(|| p.strip_prefix("target."))
            .or_else(|| p.strip_prefix("edge.metric."))
            .or_else(|| p.strip_prefix("edge."))
        {
            consumed.insert((String::from(owner), String::from(leaf)));
        }
    }
}

fn record_edge_expr_refs(
    edge_name: &str,
    src_type: &str,
    dst_type: &str,
    raw: &str,
    consumed: &mut BTreeSet<(String, String)>,
) {
    let Ok(ast) = parse_expr(raw) else {
        return;
    };
    let (metrics, props) = collect_references(&ast);
    for m in metrics {
        consumed.insert((String::from(edge_name), m));
    }
    for p in props {
        if p.starts_with("src.prop.") || p.starts_with("dst.prop.") || p.starts_with("self.prop.") {
            continue;
        }
        if let Some(m) = p
            .strip_prefix("src.metric.")
            .or_else(|| p.strip_prefix("src."))
        {
            consumed.insert((String::from(src_type), String::from(m)));
        } else if let Some(m) = p
            .strip_prefix("dst.metric.")
            .or_else(|| p.strip_prefix("dst."))
        {
            consumed.insert((String::from(dst_type), String::from(m)));
        } else if let Some(m) = p
            .strip_prefix("self.metric.")
            .or_else(|| p.strip_prefix("self."))
        {
            consumed.insert((String::from(edge_name), String::from(m)));
        }
    }
}

fn collect_propagation_json_refs(
    val: &serde_json::Value,
    edge_name: &str,
    src_type: &str,
    dst_type: &str,
    consumed: &mut BTreeSet<(String, String)>,
) {
    match val {
        serde_json::Value::String(s) if is_valid_field_ident(s) => {
            consumed.insert((String::from(edge_name), s.clone()));
            consumed.insert((String::from(src_type), s.clone()));
            consumed.insert((String::from(dst_type), s.clone()));
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_propagation_json_refs(item, edge_name, src_type, dst_type, consumed);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                collect_propagation_json_refs(v, edge_name, src_type, dst_type, consumed);
            }
        }
        _ => {}
    }
}

/// Counts undamped high-fanout relationship types (`ONE_TO_MANY` or `MANY_TO_MANY` with `impact_weight > 0.95`
/// and empty `propagation` damping configuration, `04-STEP26` §26.4).
#[must_use]
pub fn count_high_fanout_edges(doc: &SchemaDoc) -> usize {
    doc.edge_types
        .iter()
        .filter(|e| {
            matches!(
                e.cardinality,
                Cardinality::OneToMany | Cardinality::ManyToMany
            ) && e.impact_weight > 0.95
                && e.propagation
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty)
        })
        .count()
}

/// Pure L0–L4 readiness tier and composite score (`0..=100`) evaluator (`04-STEP26` §26.4).
#[must_use]
pub fn evaluate_cert_tier_and_score(
    issues: &[ValidationIssue],
    has_unconfirmed_proposals: bool,
    has_node_types: bool,
    dead_knowledge_cnt: usize,
    high_fanout_cnt: usize,
    trial_tick_passed: bool,
) -> (RuntimeCertLevel, i32) {
    let error_cnt = issues
        .iter()
        .filter(|i| i.severity == IssueSeverity::Error)
        .count();
    if error_cnt > 0 || !has_node_types {
        let penalty = i32::try_from(error_cnt.saturating_mul(5)).unwrap_or(20);
        return (RuntimeCertLevel::L0Invalid, (20 - penalty).clamp(0, 20));
    }
    if has_unconfirmed_proposals {
        return (RuntimeCertLevel::L1Configured, 40);
    }
    if !trial_tick_passed {
        return (RuntimeCertLevel::L2Runnable, 60);
    }
    if dead_knowledge_cnt > 0 || high_fanout_cnt > 0 {
        let warn_pen = i32::try_from(
            dead_knowledge_cnt
                .saturating_add(high_fanout_cnt)
                .saturating_mul(4),
        )
        .unwrap_or(15);
        return (RuntimeCertLevel::L3Verified, (88 - warn_pen).clamp(70, 88));
    }
    (RuntimeCertLevel::L4Certified, 100)
}
