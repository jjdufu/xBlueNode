//! Pure `AxSim` candidate `SandboxScope` static readiness and boundary-truncation preflight kernel (`06-STEP20` §20.1).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::sim::{PropagationSpec, SandboxScope};
use crate::studio::{AggRuleSpec, EdgeTypeSpec, LinkDirection, PropertyKind, SchemaDoc};

/// Readiness classification returned by `POST /api/axsim/sandboxes/preflight` (`06-STEP20` §20.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScopeReadinessGrade {
    /// Scope forms a well-connected causal and aggregation-closed subgraph.
    Ready,
    /// Cloning is allowed, but truncated causal edges or open aggregation rules may degrade fidelity.
    Degraded,
    /// Scope is empty, exceeds `max_nodes`, or lacks any valid causal propagation edge across multiple nodes.
    Blocked,
}

impl ScopeReadinessGrade {
    /// Returns the canonical `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::Degraded => "DEGRADED",
            Self::Blocked => "BLOCKED",
        }
    }
}

fn default_main_subgraph() -> String {
    String::from("main")
}

/// Request payload for `POST /api/axsim/sandboxes/preflight` (`06-STEP20` §20.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopePreflightRequest {
    /// Target subgraph slug (defaults to `"main"`).
    #[serde(default = "default_main_subgraph")]
    pub subgraph_slug: String,
    /// Candidate extraction scope to evaluate before cloning.
    #[serde(default)]
    pub scope: SandboxScope,
}

impl Default for ScopePreflightRequest {
    fn default() -> Self {
        Self {
            subgraph_slug: default_main_subgraph(),
            scope: SandboxScope::default(),
        }
    }
}

/// Aggregated count of induced in-scope links (`u ∈ S ∧ v ∈ S`) grouped by `(link_type, src_type_name, dst_type_name)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeInducedEdgeGroup {
    /// Relationship type identifier (`xb_links.link_type`).
    pub link_type: String,
    /// Source node `type_name` inside `S`.
    pub src_type_name: String,
    /// Destination node `type_name` inside `S`.
    pub dst_type_name: String,
    /// Count of induced links in this group.
    pub count: usize,
}

/// Aggregated count of links crossing the candidate scope boundary `∂S` (`06-STEP20` §20.1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeBoundaryEdgeGroup {
    /// Relationship type identifier (`xb_links.link_type`).
    pub link_type: String,
    /// Direction relative to the in-scope endpoint (`OUT` when `u ∈ S -> v ∉ S`; `IN` when `u ∉ S -> v ∈ S`).
    pub direction: LinkDirection,
    /// Optional `type_name` of the in-scope endpoint (`u ∈ S` or `v ∈ S`); empty matches any in-scope owner type.
    #[serde(default)]
    pub in_scope_type_name: String,
    /// Node `type_name` of the excluded neighbor outside `S`.
    pub excluded_type_name: String,
    /// Count of dropped boundary links in this group.
    pub count: usize,
}

/// Read-only subgraph slice statistics sampled from storage for [`evaluate_scope_preflight`] (`06-STEP20` §20.1.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScopePreflightSlice {
    /// Matched node counts in candidate scope `S` keyed by `type_name`.
    #[serde(default)]
    pub node_counts_by_type: BTreeMap<String, usize>,
    /// Count of nodes in `S` having zero incident induced links in `G[S]`.
    #[serde(default)]
    pub isolated_nodes: usize,
    /// Induced in-scope link counts grouped by `(link_type, src_type_name, dst_type_name)`.
    #[serde(default)]
    pub induced_edges: Vec<ScopeInducedEdgeGroup>,
    /// Boundary-dropped link counts grouped across `∂S`.
    #[serde(default)]
    pub boundary_edges: Vec<ScopeBoundaryEdgeGroup>,
    /// Observed runtime metric keys on `xb_nodes.metrics` keyed by `type_name`.
    #[serde(default)]
    pub observed_metric_keys: BTreeMap<String, BTreeSet<String>>,
    /// Active `SchemaDoc` for the target subgraph.
    #[serde(default)]
    pub schema_doc: SchemaDoc,
}

/// Per-edge-type truncation diagnostic across the candidate scope boundary (`06-STEP20` §20.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncatedEdgeDiagnostic {
    /// Relationship type identifier (`EdgeTypeSpec.name`).
    pub link_type: String,
    /// Count of links of this type whose both endpoints lie inside the candidate scope.
    pub in_scope_count: usize,
    /// Count of links incident to in-scope nodes whose opposite endpoint was excluded by `scope`.
    pub boundary_dropped_count: usize,
    /// Upstream or downstream node type excluded at the boundary.
    pub excluded_neighbor_type: String,
    /// Whether `EdgeTypeSpec.propagation` declares active causal rules on this link type.
    pub has_causal_rule: bool,
    /// Whether an `AggRuleSpec` on an in-scope node type aggregates across this link type.
    pub has_agg_rule: bool,
}

/// Deterministic health report produced by [`evaluate_scope_preflight`] (`06-STEP20` §20.1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopePreflightReport {
    /// Overall readiness grade (`READY` | `DEGRADED` | `BLOCKED`).
    pub readiness_grade: ScopeReadinessGrade,
    /// Total matched nodes in candidate scope `S`.
    pub estimated_nodes: usize,
    /// Total induced links with both `src_node_id ∈ S` and `dst_node_id ∈ S`.
    pub estimated_links: usize,
    /// Ratio (`0.0..=1.0`) of in-scope links covered by executable `PropagationSpec` rules.
    pub causal_edge_coverage_ratio: f64,
    /// Ratio (`0.0..=1.0`) of incident aggregation links whose neighbor nodes also belong to `S`.
    pub agg_rule_closure_ratio: f64,
    /// Ratio (`0.0..=1.0`) of in-scope nodes having zero incident in-scope links.
    pub isolated_node_ratio: f64,
    /// Sorted node type names excluded by `scope` yet referenced by causal propagation or `AggRuleSpec`.
    pub missing_upstream_types: Vec<String>,
    /// Detailed edge truncation breakdown ordered by `boundary_dropped_count` descending.
    pub truncated_edge_types: Vec<TruncatedEdgeDiagnostic>,
    /// Concise machine codes explaining `DEGRADED` or `BLOCKED` status.
    pub reason_codes: Vec<String>,
}

fn usize_to_f64(v: usize) -> f64 {
    u32::try_from(v).map_or(f64::from(u32::MAX), f64::from)
}

fn type_has_metric(type_name: &str, metric: &str, slice: &ScopePreflightSlice) -> bool {
    let trimmed = metric.trim();
    if trimmed.is_empty() {
        return false;
    }
    if slice
        .observed_metric_keys
        .get(type_name)
        .is_some_and(|keys| keys.contains(trimmed))
    {
        return true;
    }
    slice
        .schema_doc
        .node_types
        .iter()
        .find(|nt| nt.name == type_name)
        .is_some_and(|nt| {
            nt.properties
                .iter()
                .any(|p| p.kind == PropertyKind::Metric && p.name == trimmed)
                || nt.agg_rules.iter().any(|r| r.target_metric == trimmed)
                || nt
                    .derived_rules
                    .iter()
                    .any(|d| d.enabled && d.target_metric == trimmed)
        })
}

fn parse_propagation_spec(et: &EdgeTypeSpec) -> Option<PropagationSpec> {
    serde_json::from_value::<PropagationSpec>(et.propagation.clone()).ok()
}

fn edge_type_has_causal_rule(link_type: &str, schema_doc: &SchemaDoc) -> bool {
    schema_doc
        .edge_types
        .iter()
        .filter(|et| et.name == link_type)
        .filter_map(parse_propagation_spec)
        .any(|spec| {
            spec.rules
                .iter()
                .any(|r| !r.src_metric.trim().is_empty() && !r.dst_metric.trim().is_empty())
        })
}

fn is_executable_causal_group(group: &ScopeInducedEdgeGroup, slice: &ScopePreflightSlice) -> bool {
    slice
        .schema_doc
        .edge_types
        .iter()
        .filter(|et| {
            et.name == group.link_type
                && (et.src_type_name.is_empty() || et.src_type_name == group.src_type_name)
                && (et.dst_type_name.is_empty() || et.dst_type_name == group.dst_type_name)
        })
        .filter_map(parse_propagation_spec)
        .any(|spec| {
            spec.rules.iter().any(|r| {
                !r.src_metric.trim().is_empty()
                    && !r.dst_metric.trim().is_empty()
                    && type_has_metric(&group.src_type_name, &r.src_metric, slice)
                    && type_has_metric(&group.dst_type_name, &r.dst_metric, slice)
            })
        })
}

fn compute_causal_coverage(
    estimated_nodes: usize,
    estimated_links: usize,
    slice: &ScopePreflightSlice,
) -> f64 {
    if estimated_links == 0 {
        return if estimated_nodes <= 1 { 1.0 } else { 0.0 };
    }
    let covered_links = slice
        .induced_edges
        .iter()
        .filter(|g| g.count > 0 && is_executable_causal_group(g, slice))
        .map(|g| g.count)
        .fold(0_usize, usize::saturating_add);
    (usize_to_f64(covered_links) / usize_to_f64(estimated_links)).clamp(0.0, 1.0)
}

fn induced_matches_agg_rule(
    owner_type: &str,
    rule: &AggRuleSpec,
    group: &ScopeInducedEdgeGroup,
) -> bool {
    if group.link_type != rule.via_link_type {
        return false;
    }
    match rule.direction {
        LinkDirection::Out => {
            group.src_type_name == owner_type
                && (rule.source_type_name.is_empty()
                    || group.dst_type_name == rule.source_type_name)
        }
        LinkDirection::In => {
            group.dst_type_name == owner_type
                && (rule.source_type_name.is_empty()
                    || group.src_type_name == rule.source_type_name)
        }
    }
}

fn boundary_matches_agg_rule(
    owner_type: &str,
    rule: &AggRuleSpec,
    group: &ScopeBoundaryEdgeGroup,
) -> bool {
    if group.link_type != rule.via_link_type {
        return false;
    }
    if group.direction != rule.direction {
        return false;
    }
    let owner_ok = group.in_scope_type_name.is_empty() || group.in_scope_type_name == owner_type;
    let source_ok =
        rule.source_type_name.is_empty() || group.excluded_type_name == rule.source_type_name;
    owner_ok && source_ok
}

fn compute_agg_closure_and_missing(slice: &ScopePreflightSlice) -> (f64, BTreeSet<String>) {
    let mut seen_patterns: BTreeSet<(String, String, LinkDirection, String)> = BTreeSet::new();
    let mut m_in = 0_usize;
    let mut m_drop = 0_usize;
    let mut missing = BTreeSet::new();

    for nt in &slice.schema_doc.node_types {
        if slice
            .node_counts_by_type
            .get(&nt.name)
            .is_none_or(|&c| c == 0)
        {
            continue;
        }
        for rule in &nt.agg_rules {
            if rule.via_link_type.trim().is_empty() {
                continue;
            }
            let key = (
                nt.name.clone(),
                rule.via_link_type.clone(),
                rule.direction,
                rule.source_type_name.clone(),
            );
            if !seen_patterns.insert(key) {
                continue;
            }
            let rule_in = slice
                .induced_edges
                .iter()
                .filter(|g| induced_matches_agg_rule(&nt.name, rule, g))
                .map(|g| g.count)
                .fold(0_usize, usize::saturating_add);
            let rule_drop = slice
                .boundary_edges
                .iter()
                .filter(|b| b.count > 0 && boundary_matches_agg_rule(&nt.name, rule, b))
                .map(|b| b.count)
                .fold(0_usize, usize::saturating_add);

            m_in = m_in.saturating_add(rule_in);
            m_drop = m_drop.saturating_add(rule_drop);
            if rule_drop > 0 && !rule.source_type_name.trim().is_empty() {
                missing.insert(rule.source_type_name.trim().to_owned());
            }
        }
    }

    let total = m_in.saturating_add(m_drop);
    let ratio = if total == 0 {
        1.0
    } else {
        (usize_to_f64(m_in) / usize_to_f64(total)).clamp(0.0, 1.0)
    };
    (ratio, missing)
}

fn in_scope_has_agg_on_link(link_type: &str, slice: &ScopePreflightSlice) -> bool {
    slice.schema_doc.node_types.iter().any(|nt| {
        slice
            .node_counts_by_type
            .get(&nt.name)
            .is_some_and(|&c| c > 0)
            && nt.agg_rules.iter().any(|r| r.via_link_type == link_type)
    })
}

fn build_truncation_diagnostics(
    slice: &ScopePreflightSlice,
    missing_types: &mut BTreeSet<String>,
) -> Vec<TruncatedEdgeDiagnostic> {
    let mut in_scope_by_link: BTreeMap<String, usize> = BTreeMap::new();
    for g in &slice.induced_edges {
        if g.count > 0 {
            let entry = in_scope_by_link.entry(g.link_type.clone()).or_insert(0);
            *entry = entry.saturating_add(g.count);
        }
    }

    let mut dropped_by_pair: BTreeMap<(String, String), usize> = BTreeMap::new();
    for b in &slice.boundary_edges {
        if b.count == 0 {
            continue;
        }
        let excluded = b.excluded_type_name.trim();
        let entry = dropped_by_pair
            .entry((b.link_type.clone(), excluded.to_owned()))
            .or_insert(0);
        *entry = entry.saturating_add(b.count);
    }

    let mut diagnostics = Vec::with_capacity(dropped_by_pair.len());
    for ((link_type, excluded_neighbor_type), boundary_dropped_count) in dropped_by_pair {
        let has_causal_rule = edge_type_has_causal_rule(&link_type, &slice.schema_doc);
        let has_agg_rule = in_scope_has_agg_on_link(&link_type, slice);
        if (has_causal_rule || has_agg_rule) && !excluded_neighbor_type.is_empty() {
            missing_types.insert(excluded_neighbor_type.clone());
        }
        diagnostics.push(TruncatedEdgeDiagnostic {
            in_scope_count: in_scope_by_link.get(&link_type).copied().unwrap_or(0),
            link_type,
            boundary_dropped_count,
            excluded_neighbor_type,
            has_causal_rule,
            has_agg_rule,
        });
    }

    diagnostics.sort_by(|a, b| {
        b.boundary_dropped_count
            .cmp(&a.boundary_dropped_count)
            .then_with(|| a.link_type.cmp(&b.link_type))
            .then_with(|| a.excluded_neighbor_type.cmp(&b.excluded_neighbor_type))
    });
    diagnostics
}

fn classify_readiness(
    scope: &SandboxScope,
    estimated_nodes: usize,
    estimated_links: usize,
    causal_edge_coverage_ratio: f64,
    agg_rule_closure_ratio: f64,
    isolated_node_ratio: f64,
    missing_upstream_types: &[String],
) -> (ScopeReadinessGrade, Vec<String>) {
    if estimated_nodes == 0 {
        return (
            ScopeReadinessGrade::Blocked,
            vec![String::from("EMPTY_SCOPE")],
        );
    }
    if estimated_nodes > scope.max_nodes {
        return (
            ScopeReadinessGrade::Blocked,
            vec![String::from("MAX_NODES_EXCEEDED")],
        );
    }
    if estimated_nodes > 1 && (estimated_links == 0 || causal_edge_coverage_ratio <= 0.0) {
        return (
            ScopeReadinessGrade::Blocked,
            vec![String::from("NO_CAUSAL_PATH")],
        );
    }

    let mut reasons = Vec::new();
    if agg_rule_closure_ratio < 0.80 {
        reasons.push(String::from("OPEN_AGG_BOUNDARY"));
    }
    if causal_edge_coverage_ratio < 0.50 {
        reasons.push(String::from("LOW_CAUSAL_COVERAGE"));
    }
    if estimated_nodes > 1 && isolated_node_ratio > 0.40 {
        reasons.push(String::from("HIGH_ISOLATED_RATIO"));
    }
    if !missing_upstream_types.is_empty() {
        reasons.push(String::from("MISSING_UPSTREAM_TYPES"));
    }

    if reasons.is_empty() {
        (ScopeReadinessGrade::Ready, reasons)
    } else {
        (ScopeReadinessGrade::Degraded, reasons)
    }
}

/// Evaluates candidate [`SandboxScope`] structural health, causal propagation coverage,
/// cross-link aggregation closure, and boundary truncation diagnostics (`06-STEP20` §20.1.2).
#[must_use]
pub fn evaluate_scope_preflight(
    scope: &SandboxScope,
    slice: &ScopePreflightSlice,
) -> ScopePreflightReport {
    let estimated_nodes = slice
        .node_counts_by_type
        .values()
        .copied()
        .fold(0_usize, usize::saturating_add);
    let estimated_links = slice
        .induced_edges
        .iter()
        .map(|g| g.count)
        .fold(0_usize, usize::saturating_add);

    let causal_edge_coverage_ratio =
        compute_causal_coverage(estimated_nodes, estimated_links, slice);
    let (agg_rule_closure_ratio, mut missing_set) = compute_agg_closure_and_missing(slice);
    let isolated_node_ratio = if estimated_nodes == 0 {
        0.0
    } else {
        (usize_to_f64(slice.isolated_nodes.min(estimated_nodes)) / usize_to_f64(estimated_nodes))
            .clamp(0.0, 1.0)
    };

    let truncated_edge_types = build_truncation_diagnostics(slice, &mut missing_set);
    let missing_upstream_types: Vec<String> = missing_set.into_iter().collect();

    let (readiness_grade, reason_codes) = classify_readiness(
        scope,
        estimated_nodes,
        estimated_links,
        causal_edge_coverage_ratio,
        agg_rule_closure_ratio,
        isolated_node_ratio,
        &missing_upstream_types,
    );

    ScopePreflightReport {
        readiness_grade,
        estimated_nodes,
        estimated_links,
        causal_edge_coverage_ratio,
        agg_rule_closure_ratio,
        isolated_node_ratio,
        missing_upstream_types,
        truncated_edge_types,
        reason_codes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enums::{AggFn, DataType};
    use crate::studio::{NodeTypeSpec, PropertySpec};

    fn metric_prop(name: &str) -> PropertySpec {
        PropertySpec {
            name: String::from(name),
            display_name: String::from(name),
            data_type: DataType::Float64,
            kind: PropertyKind::Metric,
            ..PropertySpec::default()
        }
    }

    fn supply_chain_schema() -> SchemaDoc {
        SchemaDoc {
            node_types: vec![
                NodeTypeSpec {
                    name: String::from("Factory"),
                    display_name: String::from("工厂"),
                    properties: vec![metric_prop("output")],
                    ..NodeTypeSpec::default()
                },
                NodeTypeSpec {
                    name: String::from("Warehouse"),
                    display_name: String::from("仓库"),
                    properties: vec![metric_prop("inventory"), metric_prop("total_demand")],
                    agg_rules: vec![AggRuleSpec {
                        target_metric: String::from("total_demand"),
                        op: AggFn::Sum,
                        via_link_type: String::from("FULFILLS"),
                        direction: LinkDirection::Out,
                        source_type_name: String::from("Order"),
                        source_metric: String::from("demand"),
                        ..AggRuleSpec::default()
                    }],
                    ..NodeTypeSpec::default()
                },
                NodeTypeSpec {
                    name: String::from("Order"),
                    display_name: String::from("订单"),
                    properties: vec![metric_prop("demand")],
                    ..NodeTypeSpec::default()
                },
            ],
            edge_types: vec![
                EdgeTypeSpec {
                    name: String::from("SUPPLIES"),
                    src_type_name: String::from("Factory"),
                    dst_type_name: String::from("Warehouse"),
                    propagation: serde_json::json!({
                        "rules": [
                            { "src_metric": "output", "dst_metric": "inventory", "mode": "RATIO", "coefficient": 0.5 }
                        ]
                    }),
                    ..EdgeTypeSpec::default()
                },
                EdgeTypeSpec {
                    name: String::from("FULFILLS"),
                    src_type_name: String::from("Warehouse"),
                    dst_type_name: String::from("Order"),
                    propagation: serde_json::json!({
                        "rules": [
                            { "src_metric": "inventory", "dst_metric": "fulfillment", "mode": "RATIO", "coefficient": 0.2 }
                        ]
                    }),
                    ..EdgeTypeSpec::default()
                },
            ],
            mappings: Vec::new(),
        }
    }

    #[test]
    fn empty_and_over_limit_scopes_are_blocked() {
        let scope = SandboxScope {
            max_nodes: 10,
            ..SandboxScope::default()
        };
        let empty_report = evaluate_scope_preflight(&scope, &ScopePreflightSlice::default());
        assert_eq!(empty_report.readiness_grade, ScopeReadinessGrade::Blocked);
        assert_eq!(empty_report.reason_codes, vec![String::from("EMPTY_SCOPE")]);

        let mut counts = BTreeMap::new();
        counts.insert(String::from("Factory"), 12);
        let over_slice = ScopePreflightSlice {
            node_counts_by_type: counts,
            ..ScopePreflightSlice::default()
        };
        let over_report = evaluate_scope_preflight(&scope, &over_slice);
        assert_eq!(over_report.readiness_grade, ScopeReadinessGrade::Blocked);
        assert_eq!(
            over_report.reason_codes,
            vec![String::from("MAX_NODES_EXCEEDED")]
        );
    }

    #[test]
    fn multi_node_scope_without_causal_edges_is_blocked() {
        let mut counts = BTreeMap::new();
        counts.insert(String::from("Factory"), 2);
        let slice = ScopePreflightSlice {
            node_counts_by_type: counts,
            isolated_nodes: 2,
            schema_doc: supply_chain_schema(),
            ..ScopePreflightSlice::default()
        };
        let report = evaluate_scope_preflight(&SandboxScope::default(), &slice);
        assert_eq!(report.readiness_grade, ScopeReadinessGrade::Blocked);
        assert_eq!(report.reason_codes, vec![String::from("NO_CAUSAL_PATH")]);
    }

    #[test]
    fn factory_only_scope_reports_degraded_and_missing_warehouse_neighbor() {
        let mut counts = BTreeMap::new();
        counts.insert(String::from("Factory"), 1);
        let slice = ScopePreflightSlice {
            node_counts_by_type: counts,
            isolated_nodes: 1,
            boundary_edges: vec![ScopeBoundaryEdgeGroup {
                link_type: String::from("SUPPLIES"),
                direction: LinkDirection::Out,
                in_scope_type_name: String::from("Factory"),
                excluded_type_name: String::from("Warehouse"),
                count: 1,
            }],
            schema_doc: supply_chain_schema(),
            ..ScopePreflightSlice::default()
        };
        let scope = SandboxScope {
            type_names: vec![String::from("Factory")],
            ..SandboxScope::default()
        };
        let report = evaluate_scope_preflight(&scope, &slice);
        assert_eq!(report.readiness_grade, ScopeReadinessGrade::Degraded);
        assert_eq!(
            report.missing_upstream_types,
            vec![String::from("Warehouse")]
        );
        assert!(
            report
                .reason_codes
                .contains(&String::from("MISSING_UPSTREAM_TYPES"))
        );
        assert_eq!(report.truncated_edge_types.len(), 1);
        assert_eq!(
            report
                .truncated_edge_types
                .first()
                .map(|d| d.has_causal_rule),
            Some(true)
        );
    }

    #[test]
    fn closed_supply_chain_with_observed_runtime_metric_is_ready() {
        let mut counts = BTreeMap::new();
        counts.insert(String::from("Factory"), 1);
        counts.insert(String::from("Warehouse"), 1);
        counts.insert(String::from("Order"), 2);

        let mut observed = BTreeMap::new();
        observed.insert(
            String::from("Order"),
            BTreeSet::from([String::from("fulfillment")]),
        );

        let slice = ScopePreflightSlice {
            node_counts_by_type: counts,
            isolated_nodes: 0,
            induced_edges: vec![
                ScopeInducedEdgeGroup {
                    link_type: String::from("SUPPLIES"),
                    src_type_name: String::from("Factory"),
                    dst_type_name: String::from("Warehouse"),
                    count: 1,
                },
                ScopeInducedEdgeGroup {
                    link_type: String::from("FULFILLS"),
                    src_type_name: String::from("Warehouse"),
                    dst_type_name: String::from("Order"),
                    count: 2,
                },
            ],
            boundary_edges: Vec::new(),
            observed_metric_keys: observed,
            schema_doc: supply_chain_schema(),
        };

        let report = evaluate_scope_preflight(&SandboxScope::default(), &slice);
        assert_eq!(report.readiness_grade, ScopeReadinessGrade::Ready);
        assert_eq!(report.estimated_nodes, 4);
        assert_eq!(report.estimated_links, 3);
        assert!((report.causal_edge_coverage_ratio - 1.0).abs() < 1e-9);
        assert!((report.agg_rule_closure_ratio - 1.0).abs() < 1e-9);
        assert_eq!(report.missing_upstream_types, Vec::<String>::new());
        assert_eq!(report.reason_codes, Vec::<String>::new());
    }

    #[test]
    fn open_agg_boundary_and_high_isolation_trigger_degraded_reasons() {
        let mut counts = BTreeMap::new();
        counts.insert(String::from("Factory"), 3);
        counts.insert(String::from("Warehouse"), 2);

        let slice = ScopePreflightSlice {
            node_counts_by_type: counts,
            isolated_nodes: 3,
            induced_edges: vec![ScopeInducedEdgeGroup {
                link_type: String::from("SUPPLIES"),
                src_type_name: String::from("Factory"),
                dst_type_name: String::from("Warehouse"),
                count: 1,
            }],
            boundary_edges: vec![ScopeBoundaryEdgeGroup {
                link_type: String::from("FULFILLS"),
                direction: LinkDirection::Out,
                in_scope_type_name: String::from("Warehouse"),
                excluded_type_name: String::from("Order"),
                count: 4,
            }],
            schema_doc: supply_chain_schema(),
            ..ScopePreflightSlice::default()
        };

        let report = evaluate_scope_preflight(&SandboxScope::default(), &slice);
        assert_eq!(report.readiness_grade, ScopeReadinessGrade::Degraded);
        assert!((report.agg_rule_closure_ratio - 0.0).abs() < 1e-9);
        assert!((report.isolated_node_ratio - 0.6).abs() < 1e-9);
        assert_eq!(report.missing_upstream_types, vec![String::from("Order")]);
        assert!(
            report
                .reason_codes
                .contains(&String::from("OPEN_AGG_BOUNDARY"))
        );
        assert!(
            report
                .reason_codes
                .contains(&String::from("HIGH_ISOLATED_RATIO"))
        );
    }

    #[test]
    fn request_defaults_subgraph_slug_to_main_and_rejects_unknown_fields() {
        let req: ScopePreflightRequest =
            serde_json::from_str(r#"{"scope":{"depth":2}}"#).unwrap_or_default();
        assert_eq!(req.subgraph_slug, "main");
        assert_eq!(req.scope.depth, 2);

        let bad = serde_json::from_str::<ScopePreflightRequest>(
            r#"{"tenant_id":"evil","scope":{"depth":1}}"#,
        );
        assert!(bad.is_err());
    }
}
