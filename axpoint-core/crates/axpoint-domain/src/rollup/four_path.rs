//! Unified 4-path (`VERTEX_TO_VERTEX`, `VERTEX_TO_EDGE`, `EDGE_TO_VERTEX`, `EDGE_TO_EDGE`)
//! metric dependency DAG, Kahn topological layering, and causal rule-chain builder (`04-STEP26` §26.3).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::enums::PropagationPathKind;
use crate::expr::{collect_references, parse as parse_expr};
use crate::studio::{
    AggRuleSpec, EdgeAggRuleSpec, EdgeDerivedRuleSpec, EdgeTypeSpec, NodeTypeSpec, PropertyKind,
    SchemaDoc,
};

/// Unified vertex in the 4-path propagation dependency DAG (`04-STEP26` §26.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PropagationVertex {
    /// Node metric `(type_name, metric)`.
    Node {
        /// Owning `NodeTypeSpec.name`.
        type_name: String,
        /// Metric property identifier.
        metric: String,
    },
    /// Relationship edge metric `(link_type, metric)`.
    Edge {
        /// Owning `EdgeTypeSpec.name`.
        link_type: String,
        /// Metric property identifier.
        metric: String,
    },
}

impl PropagationVertex {
    /// Constructs a [`PropagationVertex::Node`] vertex.
    #[must_use]
    pub fn node(type_name: impl Into<String>, metric: impl Into<String>) -> Self {
        Self::Node {
            type_name: type_name.into(),
            metric: metric.into(),
        }
    }

    /// Constructs a [`PropagationVertex::Edge`] vertex.
    #[must_use]
    pub fn edge(link_type: impl Into<String>, metric: impl Into<String>) -> Self {
        Self::Edge {
            link_type: link_type.into(),
            metric: metric.into(),
        }
    }
}

impl fmt::Display for PropagationVertex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node { type_name, metric } => write!(f, "{type_name}.{metric}"),
            Self::Edge { link_type, metric } => write!(f, "{link_type}.{metric}"),
        }
    }
}

/// Rule computation step in the unified 4-path propagation plan (`04-STEP26` §26.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FourPathRuleStep {
    /// Intra-node derived formula (`VERTEX_TO_VERTEX`).
    NodeDerived {
        /// Owning `NodeTypeSpec.name`.
        type_name: String,
        /// Target metric name.
        target_metric: String,
        /// Deterministic expression string.
        expression: String,
        /// Referenced source metrics on the same node type.
        source_metrics: Vec<String>,
    },
    /// Node aggregation rule (`VERTEX_TO_VERTEX` or `EDGE_TO_VERTEX`).
    NodeAgg {
        /// Owning parent `NodeTypeSpec.name`.
        parent_type: String,
        /// Aggregation rule specification.
        rule: AggRuleSpec,
        /// Resolved propagation path (`VERTEX_TO_VERTEX` vs `EDGE_TO_VERTEX`).
        path_kind: PropagationPathKind,
    },
    /// Relationship edge derived formula (`VERTEX_TO_EDGE` or `EDGE_TO_EDGE`).
    EdgeDerived {
        /// Owning `EdgeTypeSpec.name`.
        link_type: String,
        /// Source `NodeTypeSpec.name`.
        src_type_name: String,
        /// Destination `NodeTypeSpec.name`.
        dst_type_name: String,
        /// Edge derived rule specification.
        rule: EdgeDerivedRuleSpec,
        /// Referenced `src.<metric>` metrics.
        src_metrics: Vec<String>,
        /// Referenced `dst.<metric>` metrics.
        dst_metrics: Vec<String>,
        /// Referenced edge-local metrics.
        self_metrics: Vec<String>,
    },
    /// Relationship edge cascade aggregation rule (`EDGE_TO_EDGE`).
    EdgeAgg {
        /// Owning `EdgeTypeSpec.name`.
        link_type: String,
        /// Source `NodeTypeSpec.name`.
        src_type_name: String,
        /// Destination `NodeTypeSpec.name`.
        dst_type_name: String,
        /// Edge aggregation rule specification.
        rule: EdgeAggRuleSpec,
    },
}

impl FourPathRuleStep {
    /// Returns the target [`PropagationVertex`] produced by this rule step.
    #[must_use]
    pub fn target_vertex(&self) -> PropagationVertex {
        match self {
            Self::NodeDerived {
                type_name,
                target_metric,
                ..
            } => PropagationVertex::node(type_name.clone(), target_metric.clone()),
            Self::NodeAgg {
                parent_type, rule, ..
            } => PropagationVertex::node(parent_type.clone(), rule.target_metric.clone()),
            Self::EdgeDerived {
                link_type, rule, ..
            } => PropagationVertex::edge(link_type.clone(), rule.target_metric.clone()),
            Self::EdgeAgg {
                link_type, rule, ..
            } => PropagationVertex::edge(link_type.clone(), rule.target_metric.clone()),
        }
    }

    /// Returns the [`PropagationPathKind`] of this rule step.
    #[must_use]
    pub const fn path_kind(&self) -> PropagationPathKind {
        match self {
            Self::NodeDerived { .. } => PropagationPathKind::VertexToVertex,
            Self::NodeAgg { path_kind, .. } => *path_kind,
            Self::EdgeDerived { rule, .. } => rule.path_kind,
            Self::EdgeAgg { .. } => PropagationPathKind::EdgeToEdge,
        }
    }

    /// Returns the upstream [`PropagationVertex`] inputs directly consumed by this rule step.
    #[must_use]
    pub fn upstream_vertices(&self) -> BTreeSet<PropagationVertex> {
        let mut out = BTreeSet::new();
        match self {
            Self::NodeDerived {
                type_name,
                source_metrics,
                ..
            } => {
                for m in source_metrics {
                    out.insert(PropagationVertex::node(type_name.clone(), m.clone()));
                }
            }
            Self::NodeAgg {
                rule, path_kind, ..
            } => collect_node_agg_upstreams(rule, *path_kind, &mut out),
            Self::EdgeDerived {
                link_type,
                src_type_name,
                dst_type_name,
                src_metrics,
                dst_metrics,
                self_metrics,
                ..
            } => {
                for m in src_metrics {
                    out.insert(PropagationVertex::node(src_type_name.clone(), m.clone()));
                }
                for m in dst_metrics {
                    out.insert(PropagationVertex::node(dst_type_name.clone(), m.clone()));
                }
                for m in self_metrics {
                    out.insert(PropagationVertex::edge(link_type.clone(), m.clone()));
                }
            }
            Self::EdgeAgg { rule, .. } => {
                if !rule.source_metric.trim().is_empty() {
                    out.insert(PropagationVertex::edge(
                        rule.via_link_type.clone(),
                        rule.source_metric.clone(),
                    ));
                }
                if let Some(filter_raw) = &rule.filter
                    && !filter_raw.trim().is_empty()
                    && let Ok(ast) = parse_expr(filter_raw)
                {
                    for fm in collect_references(&ast).0 {
                        out.insert(PropagationVertex::edge(rule.via_link_type.clone(), fm));
                    }
                }
            }
        }
        out
    }

    /// Formats a human-readable summary string for the 3-track explain `rule_chain` (`WHY` track).
    #[must_use]
    pub fn describe_rule(&self) -> String {
        match self {
            Self::NodeDerived {
                type_name,
                target_metric,
                expression,
                ..
            } => format!("VERTEX_TO_VERTEX: {type_name}.{target_metric} = {expression}"),
            Self::NodeAgg {
                parent_type,
                rule,
                path_kind,
            } => {
                let src_owner = if *path_kind == PropagationPathKind::EdgeToVertex {
                    &rule.via_link_type
                } else {
                    &rule.source_type_name
                };
                format!(
                    "{path_kind}: {parent_type}.{} = {}({src_owner}.{} {})",
                    rule.target_metric,
                    rule.op,
                    rule.source_metric,
                    rule.direction.as_str()
                )
            }
            Self::EdgeDerived {
                link_type, rule, ..
            } => format!(
                "{}: {link_type}.{} = {}",
                rule.path_kind, rule.target_metric, rule.expression
            ),
            Self::EdgeAgg {
                link_type, rule, ..
            } => format!(
                "EDGE_TO_EDGE: {link_type}.{} = {}({}.{} {})",
                rule.target_metric,
                rule.op,
                rule.via_link_type,
                rule.source_metric,
                rule.direction.as_str()
            ),
        }
    }
}

fn collect_node_agg_upstreams(
    rule: &AggRuleSpec,
    path_kind: PropagationPathKind,
    out: &mut BTreeSet<PropagationVertex>,
) {
    let is_edge = path_kind == PropagationPathKind::EdgeToVertex;
    let owner = if is_edge {
        &rule.via_link_type
    } else {
        &rule.source_type_name
    };
    if !rule.source_metric.trim().is_empty() {
        let v = if is_edge {
            PropagationVertex::edge(owner.clone(), rule.source_metric.clone())
        } else {
            PropagationVertex::node(owner.clone(), rule.source_metric.clone())
        };
        out.insert(v);
    }
    if let Some(filter_raw) = &rule.filter
        && !filter_raw.trim().is_empty()
        && let Ok(ast) = parse_expr(filter_raw)
    {
        for fm in collect_references(&ast).0 {
            let v = if is_edge {
                PropagationVertex::edge(owner.clone(), fm)
            } else {
                PropagationVertex::node(owner.clone(), fm)
            };
            out.insert(v);
        }
    }
}

/// Topologically sorted 4-path propagation dependency plan compiled from a [`SchemaDoc`] (`04-STEP26` §26.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FourPathDependencyPlan {
    /// Topological vertex order across both node and edge metrics.
    pub order: Vec<PropagationVertex>,
    /// Ordered 4-path computation steps aligned with `order`.
    pub steps: Vec<FourPathRuleStep>,
}

impl FourPathDependencyPlan {
    /// Compiles a [`FourPathDependencyPlan`] from `doc` using Kahn's algorithm.
    #[must_use]
    pub fn from_schema(doc: &SchemaDoc) -> Self {
        build_four_path_plan(doc)
    }

    /// Groups `self.steps` into topological layers so rules within the same layer have no mutual dependencies.
    #[must_use]
    pub fn layers(&self) -> Vec<Vec<FourPathRuleStep>> {
        compute_four_path_layers(self)
    }

    /// Looks up the [`FourPathRuleStep`] producing `target`, if any.
    #[must_use]
    pub fn rule_for_target(&self, target: &PropagationVertex) -> Option<&FourPathRuleStep> {
        self.steps.iter().find(|s| &s.target_vertex() == target)
    }

    /// Builds the ordered upstream-to-target causal `rule_chain` (`WHY` track) for `target`.
    #[must_use]
    pub fn build_rule_chain(&self, target: &PropagationVertex) -> Vec<String> {
        let mut reachable = BTreeSet::new();
        let mut stack = vec![target.clone()];
        while let Some(cur) = stack.pop() {
            if !reachable.insert(cur.clone()) {
                continue;
            }
            if let Some(step) = self.rule_for_target(&cur) {
                for up in step.upstream_vertices() {
                    stack.push(up);
                }
            }
        }
        self.steps
            .iter()
            .filter(|s| reachable.contains(&s.target_vertex()))
            .map(FourPathRuleStep::describe_rule)
            .collect()
    }
}

/// Builds a [`FourPathDependencyPlan`] from `doc` using Kahn's topological sort (`04-STEP26` §26.3).
#[must_use]
pub fn build_four_path_plan(doc: &SchemaDoc) -> FourPathDependencyPlan {
    let mut vertices: BTreeSet<PropagationVertex> = BTreeSet::new();
    let mut adj: BTreeMap<PropagationVertex, BTreeSet<PropagationVertex>> = BTreeMap::new();
    let mut in_degree: BTreeMap<PropagationVertex, usize> = BTreeMap::new();
    let mut step_by_target: BTreeMap<PropagationVertex, FourPathRuleStep> = BTreeMap::new();

    for nt in &doc.node_types {
        collect_node_type_steps(
            doc,
            nt,
            &mut vertices,
            &mut adj,
            &mut in_degree,
            &mut step_by_target,
        );
    }
    for et in &doc.edge_types {
        collect_edge_type_steps(
            et,
            &mut vertices,
            &mut adj,
            &mut in_degree,
            &mut step_by_target,
        );
    }

    let order = kahn_topological_order(vertices, &adj, &mut in_degree);
    let steps = order
        .iter()
        .filter_map(|v| step_by_target.remove(v))
        .collect();
    FourPathDependencyPlan { order, steps }
}

fn register_step_edges(
    step: FourPathRuleStep,
    vertices: &mut BTreeSet<PropagationVertex>,
    adj: &mut BTreeMap<PropagationVertex, BTreeSet<PropagationVertex>>,
    in_degree: &mut BTreeMap<PropagationVertex, usize>,
    step_by_target: &mut BTreeMap<PropagationVertex, FourPathRuleStep>,
) {
    let dst = step.target_vertex();
    vertices.insert(dst.clone());
    for src in step.upstream_vertices() {
        vertices.insert(src.clone());
        if src != dst && adj.entry(src).or_default().insert(dst.clone()) {
            *in_degree.entry(dst.clone()).or_insert(0) += 1;
        }
    }
    step_by_target.insert(dst, step);
}

fn collect_node_type_steps(
    doc: &SchemaDoc,
    nt: &NodeTypeSpec,
    vertices: &mut BTreeSet<PropagationVertex>,
    adj: &mut BTreeMap<PropagationVertex, BTreeSet<PropagationVertex>>,
    in_degree: &mut BTreeMap<PropagationVertex, usize>,
    step_by_target: &mut BTreeMap<PropagationVertex, FourPathRuleStep>,
) {
    let node_metrics: BTreeSet<&str> = nt
        .properties
        .iter()
        .filter(|p| p.kind == PropertyKind::Metric)
        .map(|p| p.name.as_str())
        .collect();
    for m in &node_metrics {
        vertices.insert(PropagationVertex::node(nt.name.clone(), String::from(*m)));
    }

    for rule in &nt.agg_rules {
        let is_e2v = !rule.via_link_type.trim().is_empty()
            && (rule.source_type_name == rule.via_link_type
                || (rule.source_type_name.trim().is_empty()
                    && doc.edge_types.iter().any(|e| e.name == rule.via_link_type)));
        let path_kind = if is_e2v {
            PropagationPathKind::EdgeToVertex
        } else {
            PropagationPathKind::VertexToVertex
        };
        let step = FourPathRuleStep::NodeAgg {
            parent_type: nt.name.clone(),
            rule: rule.clone(),
            path_kind,
        };
        register_step_edges(step, vertices, adj, in_degree, step_by_target);
    }

    for drule in &nt.derived_rules {
        if !drule.enabled {
            continue;
        }
        let source_metrics = extract_node_source_metrics(&drule.expression, &node_metrics);
        let step = FourPathRuleStep::NodeDerived {
            type_name: nt.name.clone(),
            target_metric: drule.target_metric.clone(),
            expression: drule.expression.clone(),
            source_metrics,
        };
        register_step_edges(step, vertices, adj, in_degree, step_by_target);
    }
}

fn extract_node_source_metrics(raw: &str, node_metrics: &BTreeSet<&str>) -> Vec<String> {
    let Ok(ast) = parse_expr(raw) else {
        return Vec::new();
    };
    let (mut metrics, props) = collect_references(&ast);
    for p in props {
        if let Some(m) = p
            .strip_prefix("self.metric.")
            .or_else(|| p.strip_prefix("self."))
            && node_metrics.contains(m)
        {
            metrics.insert(String::from(m));
        }
    }
    metrics.into_iter().collect()
}

fn collect_edge_type_steps(
    et: &EdgeTypeSpec,
    vertices: &mut BTreeSet<PropagationVertex>,
    adj: &mut BTreeMap<PropagationVertex, BTreeSet<PropagationVertex>>,
    in_degree: &mut BTreeMap<PropagationVertex, usize>,
    step_by_target: &mut BTreeMap<PropagationVertex, FourPathRuleStep>,
) {
    let edge_metrics: BTreeSet<&str> = et
        .properties
        .iter()
        .filter(|p| p.kind == PropertyKind::Metric)
        .map(|p| p.name.as_str())
        .collect();
    for m in &edge_metrics {
        vertices.insert(PropagationVertex::edge(et.name.clone(), String::from(*m)));
    }

    for rule in &et.agg_rules {
        let step = FourPathRuleStep::EdgeAgg {
            link_type: et.name.clone(),
            src_type_name: et.src_type_name.clone(),
            dst_type_name: et.dst_type_name.clone(),
            rule: rule.clone(),
        };
        register_step_edges(step, vertices, adj, in_degree, step_by_target);
    }

    for drule in &et.derived_rules {
        if !drule.enabled {
            continue;
        }
        let (src_metrics, dst_metrics, self_metrics) =
            extract_edge_refs(&drule.expression, &edge_metrics);
        let step = FourPathRuleStep::EdgeDerived {
            link_type: et.name.clone(),
            src_type_name: et.src_type_name.clone(),
            dst_type_name: et.dst_type_name.clone(),
            rule: drule.clone(),
            src_metrics,
            dst_metrics,
            self_metrics,
        };
        register_step_edges(step, vertices, adj, in_degree, step_by_target);
    }
}

fn extract_edge_refs(
    raw: &str,
    edge_metrics: &BTreeSet<&str>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let Ok(ast) = parse_expr(raw) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let (bare_metrics, props) = collect_references(&ast);
    let mut src_set = BTreeSet::new();
    let mut dst_set = BTreeSet::new();
    let mut self_set: BTreeSet<String> = bare_metrics
        .into_iter()
        .filter(|m| edge_metrics.contains(m.as_str()))
        .collect();

    for p in props {
        if p.starts_with("src.prop.") || p.starts_with("dst.prop.") || p.starts_with("self.prop.") {
            continue;
        }
        if let Some(m) = p
            .strip_prefix("src.metric.")
            .or_else(|| p.strip_prefix("src."))
        {
            src_set.insert(String::from(m));
        } else if let Some(m) = p
            .strip_prefix("dst.metric.")
            .or_else(|| p.strip_prefix("dst."))
        {
            dst_set.insert(String::from(m));
        } else if let Some(m) = p
            .strip_prefix("self.metric.")
            .or_else(|| p.strip_prefix("self."))
            && edge_metrics.contains(m)
        {
            self_set.insert(String::from(m));
        }
    }
    (
        src_set.into_iter().collect(),
        dst_set.into_iter().collect(),
        self_set.into_iter().collect(),
    )
}

fn kahn_topological_order(
    vertices: BTreeSet<PropagationVertex>,
    adj: &BTreeMap<PropagationVertex, BTreeSet<PropagationVertex>>,
    in_degree: &mut BTreeMap<PropagationVertex, usize>,
) -> Vec<PropagationVertex> {
    let mut ready: BTreeSet<PropagationVertex> = vertices
        .iter()
        .filter(|v| in_degree.get(*v).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();
    let mut order = Vec::with_capacity(vertices.len());
    let mut visited: BTreeSet<PropagationVertex> = BTreeSet::new();

    while !ready.is_empty() {
        let wave: Vec<PropagationVertex> = std::mem::take(&mut ready).into_iter().collect();
        for v in wave {
            if !visited.insert(v.clone()) {
                continue;
            }
            if let Some(neighbors) = adj.get(&v) {
                for next in neighbors {
                    if let Some(deg) = in_degree.get_mut(next) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 && !visited.contains(next) {
                            ready.insert(next.clone());
                        }
                    }
                }
            }
            order.push(v);
        }
    }
    for v in vertices {
        if visited.insert(v.clone()) {
            order.push(v);
        }
    }
    order
}

/// Computes topological execution layers from `plan` (`04-STEP26` §26.3).
#[must_use]
pub fn compute_four_path_layers(plan: &FourPathDependencyPlan) -> Vec<Vec<FourPathRuleStep>> {
    if plan.steps.is_empty() {
        return Vec::new();
    }
    let mut depth_by_vertex: BTreeMap<PropagationVertex, usize> = BTreeMap::new();
    let mut max_depth = 0_usize;
    let mut step_depths: Vec<(usize, FourPathRuleStep)> = Vec::with_capacity(plan.steps.len());

    for step in &plan.steps {
        let mut d = 0_usize;
        for up in step.upstream_vertices() {
            if let Some(&up_depth) = depth_by_vertex.get(&up) {
                d = d.max(up_depth.saturating_add(1));
            }
        }
        depth_by_vertex.insert(step.target_vertex(), d);
        max_depth = max_depth.max(d);
        step_depths.push((d, step.clone()));
    }

    let mut layers: Vec<Vec<FourPathRuleStep>> = vec![Vec::new(); max_depth.saturating_add(1)];
    for (d, step) in step_depths {
        if let Some(bucket) = layers.get_mut(d) {
            bucket.push(step);
        }
    }
    layers.retain(|l| !l.is_empty());
    layers
}
