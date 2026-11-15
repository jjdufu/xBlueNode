//! Metric dependency DAG construction, Kahn layered topological sort, and dirty-instance propagation planning (`06-STEP05-06` §06.3.1).

use std::collections::{BTreeMap, BTreeSet};

use crate::enums::AggFn;
use crate::expr::{collect_references, parse};
use crate::studio::{LinkDirection, PropertyKind, SchemaDoc};
use uuid::Uuid;

use super::{DirtyInstance, MetricDependencyPlan, MetricRuleStep, MetricVertex};

impl MetricRuleStep {
    /// Returns the target [`MetricVertex`] produced by this rule step.
    #[must_use]
    pub fn target_vertex(&self) -> MetricVertex {
        match self {
            Self::Derived {
                type_name,
                target_metric,
                ..
            } => MetricVertex::new(type_name.clone(), target_metric.clone()),
            Self::Aggregation { parent_type, rule } => {
                MetricVertex::new(parent_type.clone(), rule.target_metric.clone())
            }
        }
    }

    /// Returns the upstream [`MetricVertex`] inputs directly consumed by this rule step.
    #[must_use]
    pub fn upstream_vertices(&self) -> BTreeSet<MetricVertex> {
        let mut out = BTreeSet::new();
        match self {
            Self::Derived {
                type_name,
                source_metrics,
                ..
            } => {
                for m in source_metrics {
                    out.insert(MetricVertex::new(type_name.clone(), m.clone()));
                }
            }
            Self::Aggregation { rule, .. } => {
                if !rule.source_metric.trim().is_empty() {
                    out.insert(MetricVertex::new(
                        rule.source_type_name.clone(),
                        rule.source_metric.clone(),
                    ));
                }
                if let Some(filter_raw) = &rule.filter
                    && !filter_raw.trim().is_empty()
                    && let Ok(ast) = parse(filter_raw)
                {
                    let (filter_metrics, _) = collect_references(&ast);
                    for fm in filter_metrics {
                        out.insert(MetricVertex::new(rule.source_type_name.clone(), fm));
                    }
                }
            }
        }
        out
    }

    /// Returns `true` if a change to `src` can invalidate this rule step.
    #[must_use]
    pub fn depends_on(&self, src: &MetricVertex) -> bool {
        match self {
            Self::Derived {
                type_name,
                source_metrics,
                ..
            } => type_name == &src.type_name && source_metrics.iter().any(|m| m == &src.metric),
            Self::Aggregation { rule, .. } => {
                if rule.source_type_name != src.type_name {
                    return false;
                }
                if rule.op == AggFn::Count || rule.source_metric == src.metric {
                    return true;
                }
                if let Some(filter_raw) = &rule.filter
                    && !filter_raw.trim().is_empty()
                    && let Ok(ast) = parse(filter_raw)
                {
                    let (filter_metrics, _) = collect_references(&ast);
                    return filter_metrics.contains(&src.metric);
                }
                false
            }
        }
    }
}

/// Builds a [`MetricDependencyPlan`] from a published [`SchemaDoc`] using Kahn's algorithm.
#[must_use]
pub fn build_dependency_plan(doc: &SchemaDoc) -> MetricDependencyPlan {
    let mut vertices: BTreeSet<MetricVertex> = BTreeSet::new();
    let mut adj: BTreeMap<MetricVertex, BTreeSet<MetricVertex>> = BTreeMap::new();
    let mut in_degree: BTreeMap<MetricVertex, usize> = BTreeMap::new();
    let mut step_by_target: BTreeMap<MetricVertex, MetricRuleStep> = BTreeMap::new();

    for nt in &doc.node_types {
        for prop in &nt.properties {
            if prop.kind == PropertyKind::Metric {
                vertices.insert(MetricVertex::new(nt.name.clone(), prop.name.clone()));
            }
        }

        for rule in &nt.agg_rules {
            let step = MetricRuleStep::Aggregation {
                parent_type: nt.name.clone(),
                rule: rule.clone(),
            };
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

        for drule in &nt.derived_rules {
            if !drule.enabled {
                continue;
            }
            let source_metrics: Vec<String> = parse(&drule.expression)
                .map_or_default(|ast| collect_references(&ast).0.into_iter().collect());
            let step = MetricRuleStep::Derived {
                type_name: nt.name.clone(),
                target_metric: drule.target_metric.clone(),
                expression: drule.expression.clone(),
                source_metrics,
            };
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
    }

    let mut ready: BTreeSet<MetricVertex> = vertices
        .iter()
        .filter(|v| in_degree.get(*v).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();

    let mut order = Vec::with_capacity(vertices.len());
    let mut visited: BTreeSet<MetricVertex> = BTreeSet::new();

    while !ready.is_empty() {
        let wave: Vec<MetricVertex> = std::mem::take(&mut ready).into_iter().collect();
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

    let steps = order
        .iter()
        .filter_map(|v| step_by_target.remove(v))
        .collect();

    MetricDependencyPlan { order, steps }
}

/// Computes topological execution layers from `plan` so rules within the same layer have no mutual dependencies.
#[must_use]
pub fn compute_rule_layers(plan: &MetricDependencyPlan) -> Vec<Vec<MetricRuleStep>> {
    if plan.steps.is_empty() {
        return Vec::new();
    }

    let mut depth_by_vertex: BTreeMap<MetricVertex, usize> = BTreeMap::new();
    let mut max_depth = 0_usize;
    let mut step_depths: Vec<(usize, MetricRuleStep)> = Vec::with_capacity(plan.steps.len());

    for step in &plan.steps {
        let mut d = 0_usize;
        for up in step.upstream_vertices() {
            if let Some(&up_depth) = depth_by_vertex.get(&up) {
                d = d.max(up_depth.saturating_add(1));
            }
        }
        let target = step.target_vertex();
        depth_by_vertex.insert(target, d);
        max_depth = max_depth.max(d);
        step_depths.push((d, step.clone()));
    }

    let mut layers: Vec<Vec<MetricRuleStep>> = vec![Vec::new(); max_depth.saturating_add(1)];
    for (d, step) in step_depths {
        if let Some(bucket) = layers.get_mut(d) {
            bucket.push(step);
        }
    }
    layers.retain(|l| !l.is_empty());
    layers
}

/// Directed runtime link edge used for pure in-memory propagation planning and unit testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLinkEdge {
    /// `xb_links.link_type`.
    pub link_type: String,
    /// `xb_links.src_node_id`.
    pub src_node_id: Uuid,
    /// `src_node` `type_name`.
    pub src_type_name: String,
    /// `xb_links.dst_node_id`.
    pub dst_node_id: Uuid,
    /// `dst_node` `type_name`.
    pub dst_type_name: String,
}

/// Planned batch of dirty target node IDs for a single [`MetricRuleStep`] inside a topological layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedLayerTask {
    /// Rule step to evaluate.
    pub step: MetricRuleStep,
    /// Deduplicated target `node_id`s to recompute in this batch.
    pub target_node_ids: BTreeSet<Uuid>,
}

/// Simulates full worst-case dirty-instance propagation across all topological layers for `seeds` and `links` (`06-STEP05-06` §06.3.1).
///
/// Assumes every recomputed target changes so all transitive upstream parents and intra-node derived
/// metrics are scheduled in topological layer order without duplicates per `(layer, target_vertex)`.
#[must_use]
pub fn plan_dirty_propagation(
    plan: &MetricDependencyPlan,
    seeds: &[DirtyInstance],
    links: &[RuntimeLinkEdge],
) -> Vec<Vec<PlannedLayerTask>> {
    let layers = compute_rule_layers(plan);
    if layers.is_empty() || seeds.is_empty() {
        return Vec::new();
    }

    let mut dirty_sources: BTreeSet<DirtyInstance> = seeds.iter().cloned().collect();
    let mut scheduled_targets: BTreeMap<MetricVertex, BTreeSet<Uuid>> = BTreeMap::new();
    let mut output_layers = Vec::new();

    for layer in layers {
        expand_dirty_sources(&layer, &dirty_sources, links, &mut scheduled_targets);

        let mut layer_tasks = Vec::new();
        for step in layer {
            let target_v = step.target_vertex();
            if let Some(target_ids) = scheduled_targets.remove(&target_v)
                && !target_ids.is_empty()
            {
                for &nid in &target_ids {
                    dirty_sources.insert(DirtyInstance {
                        node_id: nid,
                        type_name: target_v.type_name.clone(),
                        metric: target_v.metric.clone(),
                    });
                }
                layer_tasks.push(PlannedLayerTask {
                    step,
                    target_node_ids: target_ids,
                });
            }
        }
        if !layer_tasks.is_empty() {
            output_layers.push(layer_tasks);
        }
    }

    output_layers
}

fn expand_dirty_sources(
    layer: &[MetricRuleStep],
    dirty_sources: &BTreeSet<DirtyInstance>,
    links: &[RuntimeLinkEdge],
    scheduled_targets: &mut BTreeMap<MetricVertex, BTreeSet<Uuid>>,
) {
    for step in layer {
        let target_v = step.target_vertex();
        for dirty in dirty_sources {
            let src_v = MetricVertex::new(dirty.type_name.clone(), dirty.metric.clone());
            if !step.depends_on(&src_v) {
                continue;
            }
            match step {
                MetricRuleStep::Derived { .. } => {
                    scheduled_targets
                        .entry(target_v.clone())
                        .or_default()
                        .insert(dirty.node_id);
                }
                MetricRuleStep::Aggregation { parent_type, rule } => {
                    for edge in links {
                        if edge.link_type != rule.via_link_type {
                            continue;
                        }
                        match rule.direction {
                            LinkDirection::Out => {
                                if edge.dst_node_id == dirty.node_id
                                    && edge.dst_type_name == rule.source_type_name
                                    && edge.src_type_name == *parent_type
                                {
                                    scheduled_targets
                                        .entry(target_v.clone())
                                        .or_default()
                                        .insert(edge.src_node_id);
                                }
                            }
                            LinkDirection::In => {
                                if edge.src_node_id == dirty.node_id
                                    && edge.src_type_name == rule.source_type_name
                                    && edge.dst_type_name == *parent_type
                                {
                                    scheduled_targets
                                        .entry(target_v.clone())
                                        .or_default()
                                        .insert(edge.dst_node_id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
