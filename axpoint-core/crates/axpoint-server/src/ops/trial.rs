//! Step 19 read-only template preview and isolated trial sandbox subgraph cloning
//! (`05-STEP19-ONBOARDING-TEMPLATE-HUB-AND-APP-PACKAGING.md` §19.2).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::enums::{AggFn, SemanticLayerTier};
use axpoint_domain::expr::{EvalEnv, ExprValue, eval as eval_expr, parse as parse_expr};
use axpoint_domain::graph::GeoPoint;
use axpoint_domain::ops::{
    TemplateAgentPreviewItem, TemplateDashboardPreviewItem, TemplateEdgeTypePreview,
    TemplateManifest, TemplateNodeTypePreview, TemplatePreviewResponse,
    TemplateSampleMetricPreview, TemplateSampleRowsPreview, TemplateTopologySummary,
};
use axpoint_domain::rollup::{MetricDependencyPlan, MetricRuleStep};
use axpoint_domain::studio::{
    AggRuleSpec, LinkDirection, MappingLinkSpec, MappingSpec, NodeTypeSpec, PropertyKind,
    SchemaDoc, TrialCloneTemplateRequest, TrialCloneTemplateResponse, ValidationContext,
    derive_trial_subgraph_slug, diff_schema, validate_trial_subgraph_slug, validate_with_metrics,
};
use chrono::Utc;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::OpsServices;
use super::templates::{
    PackDashboardDef, default_pack_dataset_ref_map, extract_json_array, json_opt_str,
    normalize_agent_spec_envelope, parse_resolved_pack_mappings, parse_simple_csv_rows,
    read_manifest, read_pack_json, resolve_pack_dir, resolve_templates_dir, safe_pack_file,
    truncate_chars, verify_manifest_checksums,
};

const MAX_SAMPLE_PREVIEW_ROWS: usize = 5;
const MAX_TRIAL_CSV_ROWS: usize = 5_000;

/// Single sample `BlueNode` materialized in memory for a trial sandbox subgraph.
#[derive(Debug, Clone)]
pub struct MaterializedTrialNode {
    /// Assigned node UUID.
    pub id: Uuid,
    /// `NodeTypeSpec.name`.
    pub type_name: String,
    /// Business `node_key`.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Static attribute map.
    pub attributes: serde_json::Map<String, serde_json::Value>,
    /// Numeric metrics map (including evaluated derived and aggregated metrics).
    pub metrics: BTreeMap<String, f64>,
    /// Optional WGS-84 coordinates.
    pub geo: Option<GeoPoint>,
}

/// Single sample `BlueLink` materialized in memory for a trial sandbox subgraph.
#[derive(Debug, Clone)]
pub struct MaterializedTrialLink {
    /// `EdgeTypeSpec.name`.
    pub link_type: String,
    /// Source `xb_nodes.id`.
    pub src_id: Uuid,
    /// Destination `xb_nodes.id`.
    pub dst_id: Uuid,
    /// Link weight.
    pub weight: f64,
}

/// Single `xb_metric_series` sample point materialized in memory for a trial sandbox subgraph.
#[derive(Debug, Clone)]
pub struct MaterializedTrialPoint {
    /// Target `xb_nodes.id`.
    pub node_id: Uuid,
    /// Metric property key.
    pub metric: String,
    /// Metric numeric value.
    pub value: f64,
    /// Source provenance tag (`"SYNC"`, `"DERIVED"`, or `"AGGREGATION"`).
    pub source_tag: &'static str,
}

/// Complete in-memory graph bundle produced from a template pack's CSV files and schema rules.
#[derive(Debug, Clone, Default)]
pub struct MaterializedTrialGraph {
    /// Materialized sample nodes.
    pub nodes: Vec<MaterializedTrialNode>,
    /// Materialized sample directed links.
    pub links: Vec<MaterializedTrialLink>,
    /// Materialized initial and rule-computed metric series points.
    pub points: Vec<MaterializedTrialPoint>,
}

#[derive(Clone)]
struct PendingTrialLink {
    link_type: String,
    direction: LinkDirection,
    src_type: String,
    src_key: String,
    target_type: String,
    target_key: String,
    weight: f64,
    create_missing_target: bool,
}

/// Builds a [`TemplatePreviewResponse`] from `pack_id` inside `catalog_dir` after verifying `SHA-256` checksums.
///
/// # Errors
/// Returns [`AppError::NotFound`] (`TEMPLATE_NOT_FOUND`) when `pack_id` is absent, or
/// [`AppError::Unprocessable`] on `CHECKSUM_MISMATCH` / malformed pack JSON.
pub fn preview_template_in_dir(
    catalog_dir: &Path,
    pack_id: &str,
) -> Result<TemplatePreviewResponse, AppError> {
    let pack_dir = resolve_pack_dir(catalog_dir, pack_id)?;
    let manifest = read_manifest(&pack_dir)?;
    verify_manifest_checksums(&pack_dir, &manifest)?;

    let readme_path = pack_dir.join("README.md");
    let readme_markdown = if readme_path.is_file() {
        std::fs::read_to_string(&readme_path)
            .map_err(|err| AppError::Config(format!("read template README.md failed: {err}")))?
    } else {
        String::new()
    };

    let schema_doc: SchemaDoc =
        serde_json::from_value(read_pack_json(&pack_dir, &manifest.contents.schema)?).map_err(
            |err| AppError::unprocessable("PARSE_FAILED", format!("invalid schema.json: {err}")),
        )?;

    let topology_summary = summarize_topology(&schema_doc);
    let sample_metrics = load_sample_metrics(&pack_dir, &manifest, &schema_doc)?;
    let dashboard_previews = load_dashboard_previews(&pack_dir, &manifest.contents.dashboards)?;
    let agents = load_agent_previews(&pack_dir, &manifest.contents.agents)?;
    let sample_rows = load_sample_csv_previews(&pack_dir, &manifest)?;

    Ok(TemplatePreviewResponse {
        manifest,
        readme_markdown,
        topology_summary,
        sample_metrics,
        dashboard_previews,
        sample_rows,
        agents,
    })
}

fn summarize_topology(schema_doc: &SchemaDoc) -> TemplateTopologySummary {
    let mut agg_rule_count = 0_usize;
    let mut derived_rule_count = 0_usize;
    let mut node_types = Vec::with_capacity(schema_doc.node_types.len());

    for nt in &schema_doc.node_types {
        agg_rule_count = agg_rule_count.saturating_add(nt.agg_rules.len());
        derived_rule_count = derived_rule_count.saturating_add(nt.derived_rules.len());
        let metric_count = nt
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Metric)
            .count();
        node_types.push(TemplateNodeTypePreview {
            name: nt.name.clone(),
            display_name: nt.display_name.clone(),
            property_count: nt.properties.len(),
            metric_count,
        });
    }

    let edge_types = schema_doc
        .edge_types
        .iter()
        .map(|et| TemplateEdgeTypePreview {
            name: et.name.clone(),
            src_type_name: et.src_type_name.clone(),
            dst_type_name: et.dst_type_name.clone(),
            cardinality: et.cardinality,
        })
        .collect();

    TemplateTopologySummary {
        node_types,
        edge_types,
        agg_rule_count,
        derived_rule_count,
    }
}

fn load_sample_metrics(
    pack_dir: &Path,
    manifest: &TemplateManifest,
    schema_doc: &SchemaDoc,
) -> Result<Vec<TemplateSampleMetricPreview>, AppError> {
    let mut out = Vec::new();
    if let Some(rel_path) = manifest
        .contents
        .metrics
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let items = extract_json_array(read_pack_json(pack_dir, rel_path)?, "metrics")?;
        for item in items {
            if let Some(name) = json_opt_str(&item, "name") {
                let display_name =
                    json_opt_str(&item, "display_name").unwrap_or_else(|| name.clone());
                let unit = json_opt_str(&item, "unit").unwrap_or_default();
                out.push(TemplateSampleMetricPreview {
                    name,
                    display_name,
                    unit,
                });
            }
        }
    }

    if out.is_empty() {
        for nt in &schema_doc.node_types {
            for prop in &nt.properties {
                if prop.kind == PropertyKind::Metric {
                    out.push(TemplateSampleMetricPreview {
                        name: prop.name.clone(),
                        display_name: if prop.display_name.trim().is_empty() {
                            prop.name.clone()
                        } else {
                            prop.display_name.clone()
                        },
                        unit: prop.unit.clone().unwrap_or_default(),
                    });
                }
            }
        }
    }
    Ok(out)
}

fn load_dashboard_previews(
    pack_dir: &Path,
    rel_paths: &[String],
) -> Result<Vec<TemplateDashboardPreviewItem>, AppError> {
    let mut out = Vec::with_capacity(rel_paths.len());
    for rel_path in rel_paths {
        let def: PackDashboardDef = serde_json::from_value(read_pack_json(pack_dir, rel_path)?)
            .map_err(|err| {
                AppError::unprocessable(
                    "PARSE_FAILED",
                    format!("invalid dashboard '{rel_path}': {err}"),
                )
            })?;
        out.push(TemplateDashboardPreviewItem {
            slug: def.slug,
            name: def.name,
            widget_count: def.widgets.len(),
        });
    }
    Ok(out)
}

fn load_agent_previews(
    pack_dir: &Path,
    rel_paths: &[String],
) -> Result<Vec<TemplateAgentPreviewItem>, AppError> {
    let mut out = Vec::with_capacity(rel_paths.len());
    for rel_path in rel_paths {
        let mut val = read_pack_json(pack_dir, rel_path)?;
        normalize_agent_spec_envelope(&mut val);
        let name = json_opt_str(&val, "name").unwrap_or_else(|| String::from("agent"));
        let description = json_opt_str(&val, "description").unwrap_or_default();
        let tool_count = val
            .get("spec")
            .and_then(|s| s.get("tools"))
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        out.push(TemplateAgentPreviewItem {
            name,
            description,
            tool_count,
        });
    }
    Ok(out)
}

fn load_sample_csv_previews(
    pack_dir: &Path,
    manifest: &TemplateManifest,
) -> Result<Vec<TemplateSampleRowsPreview>, AppError> {
    let mut out = Vec::with_capacity(manifest.contents.datasets.len());
    for ds in &manifest.contents.datasets {
        let dataset_name = ds
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&ds.key)
            .to_owned();
        let csv_path = safe_pack_file(pack_dir, &ds.file)?;
        let content = std::fs::read_to_string(&csv_path).map_err(|err| {
            AppError::unprocessable("PARSE_FAILED", format!("read '{}' failed: {err}", ds.file))
        })?;
        out.push(parse_csv_preview(&dataset_name, &content));
    }
    Ok(out)
}

fn parse_csv_preview(dataset_name: &str, content: &str) -> TemplateSampleRowsPreview {
    let mut lines = content
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return TemplateSampleRowsPreview {
            dataset_name: String::from(dataset_name),
            headers: Vec::new(),
            rows: Vec::new(),
            total_rows: 0,
        };
    };
    let headers: Vec<String> = header_line
        .split(',')
        .map(|h| String::from(h.trim()))
        .collect();
    let mut rows = Vec::new();
    let mut total_rows = 0_usize;
    for line in lines {
        total_rows = total_rows.saturating_add(1);
        if rows.len() < MAX_SAMPLE_PREVIEW_ROWS {
            let raw_cells: Vec<&str> = line.split(',').map(str::trim).collect();
            let aligned: Vec<String> = (0..headers.len())
                .map(|idx| String::from(raw_cells.get(idx).copied().unwrap_or("")))
                .collect();
            rows.push(aligned);
        }
    }
    TemplateSampleRowsPreview {
        dataset_name: String::from(dataset_name),
        headers,
        rows,
        total_rows,
    }
}

/// Materializes a template pack's `mappings.json` + `sample_data/*.csv` in memory and evaluates
/// all `derived_rules` and `agg_rules` declared in `schema_doc` (`05-STEP19` §19.2.3).
///
/// # Errors
/// Returns [`AppError::Unprocessable`] if a CSV file is unreadable or malformed.
pub fn materialize_pack_trial_graph(
    pack_dir: &Path,
    manifest: &TemplateManifest,
    schema_doc: &SchemaDoc,
    mappings: &[MappingSpec],
) -> Result<MaterializedTrialGraph, AppError> {
    let csv_by_ds = load_all_pack_csv_rows(pack_dir, manifest)?;
    let nt_map: BTreeMap<&str, &NodeTypeSpec> = schema_doc
        .node_types
        .iter()
        .map(|n| (n.name.as_str(), n))
        .collect();
    let edge_weights: BTreeMap<String, f64> = schema_doc
        .edge_types
        .iter()
        .map(|e| (e.name.clone(), e.impact_weight))
        .collect();

    let mut nodes_by_key: BTreeMap<(String, String), MaterializedTrialNode> = BTreeMap::new();
    let mut pending_links: Vec<PendingTrialLink> = Vec::new();
    let mut points: Vec<MaterializedTrialPoint> = Vec::new();

    for mapping in mappings {
        let Some(rows) = csv_by_ds.get(&mapping.dataset) else {
            continue;
        };
        let nt_opt = nt_map.get(mapping.node_type.as_str()).copied();
        for row in rows {
            if let Ok((mapped, links)) = coerce_trial_row(row, mapping, nt_opt, &edge_weights) {
                let node_id = upsert_mem_node(&mut nodes_by_key, mapped, &mut points);
                let _ = node_id;
                pending_links.extend(links);
            }
        }
    }

    let links = build_mem_links(&mut nodes_by_key, &pending_links);
    let mut nodes_by_id: BTreeMap<Uuid, MaterializedTrialNode> =
        nodes_by_key.into_values().map(|n| (n.id, n)).collect();
    evaluate_mem_rules(schema_doc, &mut nodes_by_id, &links, &mut points);

    Ok(MaterializedTrialGraph {
        nodes: nodes_by_id.into_values().collect(),
        links,
        points,
    })
}

fn load_all_pack_csv_rows(
    pack_dir: &Path,
    manifest: &TemplateManifest,
) -> Result<BTreeMap<String, Vec<serde_json::Map<String, serde_json::Value>>>, AppError> {
    let mut out = BTreeMap::new();
    for ds in &manifest.contents.datasets {
        let csv_path = safe_pack_file(pack_dir, &ds.file)?;
        let content = std::fs::read_to_string(&csv_path).map_err(|err| {
            AppError::unprocessable("PARSE_FAILED", format!("read '{}' failed: {err}", ds.file))
        })?;
        let rows = parse_simple_csv_rows(&content, MAX_TRIAL_CSV_ROWS);
        if let Some(name) = ds.name.as_deref().map(str::trim).filter(|s| !s.is_empty())
            && name != ds.key
        {
            out.insert(String::from(name), rows.clone());
        }
        out.insert(ds.key.clone(), rows);
    }
    Ok(out)
}

fn upsert_mem_node(
    nodes_by_key: &mut BTreeMap<(String, String), MaterializedTrialNode>,
    incoming: MaterializedTrialNode,
    points: &mut Vec<MaterializedTrialPoint>,
) -> Uuid {
    let key = (incoming.type_name.clone(), incoming.node_key.clone());
    let nid = if let Some(existing) = nodes_by_key.get_mut(&key) {
        existing.title = incoming.title;
        for (k, v) in incoming.attributes {
            existing.attributes.insert(k, v);
        }
        for (m, val) in &incoming.metrics {
            existing.metrics.insert(m.clone(), *val);
        }
        if incoming.geo.is_some() {
            existing.geo = incoming.geo;
        }
        existing.id
    } else {
        let id = incoming.id;
        nodes_by_key.insert(
            key,
            MaterializedTrialNode {
                metrics: incoming.metrics.clone(),
                ..incoming
            },
        );
        id
    };

    for (m_name, &val) in &incoming.metrics {
        if is_valid_series_metric_name(m_name) {
            points.push(MaterializedTrialPoint {
                node_id: nid,
                metric: m_name.clone(),
                value: val,
                source_tag: "SYNC",
            });
        }
    }
    nid
}

fn build_mem_links(
    nodes_by_key: &mut BTreeMap<(String, String), MaterializedTrialNode>,
    pending_links: &[PendingTrialLink],
) -> Vec<MaterializedTrialLink> {
    let mut dedup: BTreeMap<(String, Uuid, Uuid), f64> = BTreeMap::new();
    for l in pending_links {
        let Some(origin_id) = nodes_by_key
            .get(&(l.src_type.clone(), l.src_key.clone()))
            .map(|n| n.id)
        else {
            continue;
        };
        let target_pair = (l.target_type.clone(), l.target_key.clone());
        let target_id = if let Some(existing) = nodes_by_key.get(&target_pair) {
            existing.id
        } else if l.create_missing_target {
            let new_id = Uuid::new_v4();
            nodes_by_key.insert(
                target_pair,
                MaterializedTrialNode {
                    id: new_id,
                    type_name: l.target_type.clone(),
                    node_key: l.target_key.clone(),
                    title: l.target_key.clone(),
                    attributes: serde_json::Map::new(),
                    metrics: BTreeMap::new(),
                    geo: None,
                },
            );
            new_id
        } else {
            continue;
        };

        let (src_id, dst_id) = match l.direction {
            LinkDirection::Out => (origin_id, target_id),
            LinkDirection::In => (target_id, origin_id),
        };
        dedup.insert((l.link_type.clone(), src_id, dst_id), l.weight);
    }

    dedup
        .into_iter()
        .map(
            |((link_type, src_id, dst_id), weight)| MaterializedTrialLink {
                link_type,
                src_id,
                dst_id,
                weight,
            },
        )
        .collect()
}

fn evaluate_mem_rules(
    schema_doc: &SchemaDoc,
    nodes_by_id: &mut BTreeMap<Uuid, MaterializedTrialNode>,
    links: &[MaterializedTrialLink],
    points: &mut Vec<MaterializedTrialPoint>,
) {
    let layers = MetricDependencyPlan::from_schema(schema_doc).layers();
    for layer in &layers {
        for step in layer {
            apply_mem_rule_step(step, nodes_by_id, links, points);
        }
    }
}

fn apply_mem_rule_step(
    step: &MetricRuleStep,
    nodes_by_id: &mut BTreeMap<Uuid, MaterializedTrialNode>,
    links: &[MaterializedTrialLink],
    points: &mut Vec<MaterializedTrialPoint>,
) {
    let target_v = step.target_vertex();
    let target_ids: Vec<Uuid> = nodes_by_id
        .values()
        .filter(|n| n.type_name == target_v.type_name)
        .map(|n| n.id)
        .collect();

    let mut updates: Vec<(Uuid, f64)> = Vec::new();
    let source_tag = match step {
        MetricRuleStep::Derived { expression, .. } => {
            let Ok(ast) = parse_expr(expression) else {
                return;
            };
            for nid in target_ids {
                if let Some(n) = nodes_by_id.get(&nid) {
                    let env = EvalEnv::from_node(&n.metrics, &n.attributes);
                    if let ExprValue::Number(v) = eval_expr(&ast, &env).value
                        && v.is_finite()
                    {
                        updates.push((nid, v));
                    }
                }
            }
            "DERIVED"
        }
        MetricRuleStep::Aggregation { rule, .. } => {
            for nid in target_ids {
                if let Some(v) = eval_trial_agg(nid, rule, nodes_by_id, links)
                    && v.is_finite()
                {
                    updates.push((nid, v));
                }
            }
            "AGGREGATION"
        }
    };

    for (nid, val) in updates {
        if let Some(node) = nodes_by_id.get_mut(&nid) {
            node.metrics.insert(target_v.metric.clone(), val);
            if is_valid_series_metric_name(&target_v.metric) {
                points.push(MaterializedTrialPoint {
                    node_id: nid,
                    metric: target_v.metric.clone(),
                    value: val,
                    source_tag,
                });
            }
        }
    }
}

fn eval_trial_agg(
    parent_id: Uuid,
    rule: &AggRuleSpec,
    nodes_by_id: &BTreeMap<Uuid, MaterializedTrialNode>,
    links: &[MaterializedTrialLink],
) -> Option<f64> {
    let filter_ast = rule
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|f| parse_expr(f).ok());

    let mut vals: Vec<(f64, f64)> = Vec::new();
    let mut distinct_keys: BTreeSet<String> = BTreeSet::new();
    let mut matched_children = 0_u32;

    for link in links {
        if link.link_type != rule.via_link_type {
            continue;
        }
        let child_id = match rule.direction {
            LinkDirection::Out if link.src_id == parent_id => link.dst_id,
            LinkDirection::In if link.dst_id == parent_id => link.src_id,
            _ => continue,
        };
        let Some(child) = nodes_by_id.get(&child_id) else {
            continue;
        };
        if child.type_name != rule.source_type_name {
            continue;
        }
        if let Some(ref ast) = filter_ast {
            let env = EvalEnv::from_node(&child.metrics, &child.attributes);
            if !matches!(eval_expr(ast, &env).value, ExprValue::Bool(true)) {
                continue;
            }
        }
        matched_children = matched_children.saturating_add(1);
        distinct_keys.insert(child.node_key.clone());
        if let Some(&v) = child.metrics.get(&rule.source_metric)
            && v.is_finite()
        {
            vals.push((v, link.weight.max(0.0)));
        }
    }

    compute_agg_value(rule, matched_children, distinct_keys.len(), &vals)
}

fn compute_agg_value(
    rule: &AggRuleSpec,
    matched_children: u32,
    distinct_count: usize,
    vals: &[(f64, f64)],
) -> Option<f64> {
    match rule.op {
        AggFn::Count => Some(f64::from(matched_children)),
        AggFn::CountDistinct => u32::try_from(distinct_count).ok().map(f64::from),
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
        AggFn::Median if !vals.is_empty() => {
            let mut sorted: Vec<f64> = vals.iter().map(|&(v, _)| v).collect();
            sorted.sort_by(f64::total_cmp);
            let mid = sorted.len() / 2;
            if sorted.len() % 2 == 1 {
                sorted.get(mid).copied()
            } else {
                let lo = sorted.get(mid.saturating_sub(1)).copied()?;
                let hi = sorted.get(mid).copied()?;
                Some(f64::midpoint(lo, hi))
            }
        }
        AggFn::Last => vals.last().map(|&(v, _)| v),
        _ => None,
    }
}

fn lookup_cell_ci<'a>(
    row: &'a serde_json::Map<String, serde_json::Value>,
    col: &str,
) -> Option<&'a serde_json::Value> {
    row.get(col).or_else(|| {
        row.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(col))
            .map(|(_, v)| v)
    })
}

fn cell_to_string(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| String::from(trimmed))
        }
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}

fn cell_to_f64(val: &serde_json::Value) -> Option<f64> {
    match val {
        serde_json::Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn render_trial_title(
    spec: &MappingSpec,
    nt: Option<&NodeTypeSpec>,
    node_key: &str,
    row: &serde_json::Map<String, serde_json::Value>,
    attrs: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let tpl = if !spec.title.trim().is_empty() {
        spec.title.trim()
    } else if let Some(n) = nt
        && !n.title_template.trim().is_empty()
    {
        n.title_template.trim()
    } else {
        return String::from(node_key);
    };

    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(open) = rest.find('{') {
        if let Some(prefix) = rest.get(..open) {
            out.push_str(prefix);
        }
        let after_open = rest.get(open.saturating_add(1)..).unwrap_or("");
        if let Some(close) = after_open.find('}') {
            let token = after_open.get(..close).unwrap_or("").trim();
            let resolved = if token == "key" || token == "node_key" {
                Some(String::from(node_key))
            } else if let Some(prop_name) = token.strip_prefix("prop.") {
                attrs.get(prop_name).and_then(cell_to_string)
            } else {
                lookup_cell_ci(row, token)
                    .and_then(cell_to_string)
                    .or_else(|| attrs.get(token).and_then(cell_to_string))
            };
            out.push_str(resolved.as_deref().unwrap_or(node_key));
            rest = after_open.get(close.saturating_add(1)..).unwrap_or("");
        } else {
            out.push('{');
            out.push_str(after_open);
            rest = "";
        }
    }
    out.push_str(rest);
    let trimmed = out.trim();
    if trimmed.is_empty() {
        String::from(node_key)
    } else {
        String::from(trimmed)
    }
}

fn coerce_trial_row(
    row: &serde_json::Map<String, serde_json::Value>,
    spec: &MappingSpec,
    nt: Option<&NodeTypeSpec>,
    edge_weights: &BTreeMap<String, f64>,
) -> Result<(MaterializedTrialNode, Vec<PendingTrialLink>), String> {
    let key_col = spec.key_column.trim();
    let node_key = lookup_cell_ci(row, key_col)
        .and_then(cell_to_string)
        .ok_or_else(|| format!("missing key_column '{key_col}'"))?;

    let mut attributes = serde_json::Map::new();
    for (attr_name, col_name) in &spec.attributes {
        if let Some(val) = lookup_cell_ci(row, col_name)
            && !val.is_null()
        {
            attributes.insert(attr_name.clone(), val.clone());
        }
    }

    let mut metrics = BTreeMap::new();
    for (m_name, col_name) in &spec.metrics {
        if let Some(val) = lookup_cell_ci(row, col_name)
            && !val.is_null()
        {
            let num = cell_to_f64(val)
                .ok_or_else(|| format!("invalid metric '{m_name}' in '{col_name}'"))?;
            metrics.insert(m_name.clone(), num);
        }
    }

    let geo = extract_trial_geo(row, spec)?;
    let links = extract_trial_links(row, &spec.node_type, &node_key, &spec.links, edge_weights);
    let title = render_trial_title(spec, nt, &node_key, row, &attributes);

    Ok((
        MaterializedTrialNode {
            id: Uuid::new_v4(),
            type_name: spec.node_type.clone(),
            node_key,
            title,
            attributes,
            metrics,
            geo,
        },
        links,
    ))
}

fn extract_trial_geo(
    row: &serde_json::Map<String, serde_json::Value>,
    spec: &MappingSpec,
) -> Result<Option<GeoPoint>, String> {
    let (Some(lng_c), Some(lat_c)) = (
        spec.geo.lng_column.as_deref().map(str::trim),
        spec.geo.lat_column.as_deref().map(str::trim),
    ) else {
        return Ok(None);
    };
    if lng_c.is_empty() || lat_c.is_empty() {
        return Ok(None);
    }
    let (Some(vl), Some(vt)) = (lookup_cell_ci(row, lng_c), lookup_cell_ci(row, lat_c)) else {
        return Ok(None);
    };
    if vl.is_null() || vt.is_null() {
        return Ok(None);
    }
    let lng = cell_to_f64(vl).ok_or_else(|| String::from("invalid geo lng"))?;
    let lat = cell_to_f64(vt).ok_or_else(|| String::from("invalid geo lat"))?;
    let pt = GeoPoint { lng, lat };
    pt.validate().map_err(|e| e.client_message())?;
    Ok(Some(pt))
}

fn extract_trial_links(
    row: &serde_json::Map<String, serde_json::Value>,
    src_type: &str,
    src_key: &str,
    specs: &[MappingLinkSpec],
    edge_weights: &BTreeMap<String, f64>,
) -> Vec<PendingTrialLink> {
    let mut out = Vec::new();
    for ls in specs {
        let Some(target_key) =
            lookup_cell_ci(row, ls.target_key_column.trim()).and_then(cell_to_string)
        else {
            continue;
        };
        let weight = ls
            .weight_column
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|wc| lookup_cell_ci(row, wc))
            .and_then(cell_to_f64)
            .or_else(|| edge_weights.get(&ls.link_type).copied())
            .unwrap_or(0.5);
        out.push(PendingTrialLink {
            link_type: ls.link_type.clone(),
            direction: ls.direction,
            src_type: String::from(src_type),
            src_key: String::from(src_key),
            target_type: ls.target_type.clone(),
            target_key,
            weight,
            create_missing_target: ls.create_missing_target,
        });
    }
    out
}

fn is_valid_series_metric_name(metric: &str) -> bool {
    !metric.is_empty()
        && metric.len() <= 63
        && metric
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        && metric
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl<P: Sync> OpsServices<PgStore, P> {
    /// Clones a built-in template pack into an isolated `is_dev_sandbox = true` trial subgraph
    /// and materializes its sample CSV nodes, links, and computed metrics inside a single tenant transaction (`05-STEP19` §19.2.3).
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] (`TEMPLATE_NOT_FOUND`), [`AppError::BadRequest`] (`INVALID_ARGUMENT`),
    /// [`AppError::Conflict`] (`SUBGRAPH_ALREADY_EXISTS`), [`AppError::Unprocessable`] (`CHECKSUM_MISMATCH`),
    /// or [`AppError::Database`] on PostgreSQL persistence failures.
    pub async fn clone_trial_subgraph(
        &self,
        tenant_id: &str,
        dept_id: &str,
        user_id: Option<Uuid>,
        req: &TrialCloneTemplateRequest,
    ) -> Result<TrialCloneTemplateResponse, AppError> {
        let catalog_dir =
            resolve_templates_dir().ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
        let pack_dir = resolve_pack_dir(&catalog_dir, &req.pack_id)?;
        let manifest = read_manifest(&pack_dir)?;
        verify_manifest_checksums(&pack_dir, &manifest)?;

        let trial_slug = resolve_effective_trial_slug(&manifest.id, req.slug_override.as_deref())?;
        let display_name = req
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(
                || truncate_chars(&format!("{}（演练沙盒）", manifest.name), 64),
                |s| truncate_chars(s, 64),
            );
        let eff_dept = if dept_id.trim().is_empty() {
            "default"
        } else {
            dept_id.trim()
        };

        let (mut published_doc, resolved_mappings) =
            load_pack_schema_and_mappings(&pack_dir, &manifest)?;
        let graph =
            materialize_pack_trial_graph(&pack_dir, &manifest, &published_doc, &resolved_mappings)?;

        let mut tx = axpoint_db::begin_tenant_tx(self.store.pool(), tenant_id).await?;
        insert_trial_subgraph_row_tx(
            &mut tx,
            tenant_id,
            &trial_slug,
            &display_name,
            &manifest.description,
            eff_dept,
            (&manifest.id, user_id),
        )
        .await?;
        let draft_id = persist_trial_schema_and_draft_tx(
            &mut tx,
            tenant_id,
            &trial_slug,
            &manifest,
            &mut published_doc,
            user_id,
        )
        .await?;
        persist_trial_graph_tx(&mut tx, tenant_id, &trial_slug, &graph).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit clone_trial_subgraph tx failed: {err}"))
        })?;

        Ok(TrialCloneTemplateResponse {
            subgraph_slug: trial_slug.clone(),
            display_name,
            is_dev_sandbox: true,
            source_template_id: manifest.id,
            draft_id,
            nodes_materialized: graph.nodes.len(),
            links_materialized: graph.links.len(),
            studio_url: format!("/studio?subgraph={trial_slug}"),
        })
    }
}

fn resolve_effective_trial_slug(
    pack_id: &str,
    slug_override: Option<&str>,
) -> Result<String, AppError> {
    if let Some(ovr) = slug_override.map(str::trim).filter(|s| !s.is_empty()) {
        validate_trial_subgraph_slug(ovr)?;
        return Ok(String::from(ovr));
    }
    let hex = Uuid::new_v4().simple().to_string();
    Ok(derive_trial_subgraph_slug(pack_id, &hex))
}

fn load_pack_schema_and_mappings(
    pack_dir: &Path,
    manifest: &TemplateManifest,
) -> Result<(SchemaDoc, Vec<MappingSpec>), AppError> {
    let mut doc: SchemaDoc =
        serde_json::from_value(read_pack_json(pack_dir, &manifest.contents.schema)?).map_err(
            |err| AppError::unprocessable("PARSE_FAILED", format!("invalid schema.json: {err}")),
        )?;
    doc.mappings.clear();

    let resolved_mappings = match manifest
        .contents
        .mappings
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(rel_path) => {
            let raw = read_pack_json(pack_dir, rel_path)?;
            let ref_map = default_pack_dataset_ref_map(manifest);
            parse_resolved_pack_mappings(raw, &ref_map)?
        }
        None => Vec::new(),
    };
    Ok((doc, resolved_mappings))
}

async fn insert_trial_subgraph_row_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    trial_slug: &str,
    display_name: &str,
    description: &str,
    owner_dept: &str,
    source_and_actor: (&str, Option<Uuid>),
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xb_subgraphs WHERE tenant_id = $1 AND slug = $2)",
    )
    .bind(tenant_id)
    .bind(trial_slug)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check trial subgraph slug failed: {err}")))?;

    if exists {
        return Err(AppError::Conflict("SUBGRAPH_ALREADY_EXISTS"));
    }

    let (source_pack_id, created_by) = source_and_actor;
    sqlx::query(
        "INSERT INTO xb_subgraphs (
             tenant_id, slug, display_name, description, owner_dept,
             is_main, is_dev_sandbox, source_template_id, created_by
         )
         VALUES ($1, $2, $3, $4, $5, FALSE, TRUE, $6, $7)",
    )
    .bind(tenant_id)
    .bind(trial_slug)
    .bind(display_name)
    .bind(description)
    .bind(owner_dept)
    .bind(source_pack_id)
    .bind(created_by)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert trial xb_subgraphs failed: {err}")))?;

    Ok(())
}

async fn persist_trial_schema_and_draft_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    trial_slug: &str,
    manifest: &TemplateManifest,
    doc: &mut SchemaDoc,
    user_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    persist_trial_node_and_edge_types_tx(tx, tenant_id, trial_slug, doc).await?;

    sqlx::query(
        "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version, updated_at)
         VALUES ($1, $2, 1, NOW())",
    )
    .bind(tenant_id)
    .bind(trial_slug)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert trial xb_schema_heads failed: {err}")))?;

    let diff = diff_schema(&SchemaDoc::default(), doc);
    let doc_json = serde_json::to_value(&*doc)
        .map_err(|err| AppError::Database(format!("serialize trial SchemaDoc: {err}")))?;
    let diff_json = serde_json::to_value(&diff)
        .map_err(|err| AppError::Database(format!("serialize trial SchemaDiff: {err}")))?;

    sqlx::query(
        "INSERT INTO xb_schema_versions (
             tenant_id, subgraph_slug, version, schema_doc, diff, change_summary, published_by
         )
         VALUES ($1, $2, 1, $3, $4, $5, $6)",
    )
    .bind(tenant_id)
    .bind(trial_slug)
    .bind(&doc_json)
    .bind(diff_json)
    .bind(format!("cloned from template '{}'", manifest.id))
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert trial xb_schema_versions failed: {err}")))?;

    let val_ctx = ValidationContext {
        subgraph_slug: String::from(trial_slug),
        current_doc: Some(doc.clone()),
        ..ValidationContext::default()
    };
    let summary = validate_with_metrics(doc, &val_ctx);
    let pass_count_i32 = i32::try_from(summary.pass_count).unwrap_or(i32::MAX);
    let issues_json = serde_json::to_value(&summary.issues)
        .map_err(|err| AppError::Database(format!("serialize trial draft issues: {err}")))?;

    let draft_title = truncate_chars(&format!("{} 演练草案", manifest.name), 120);
    let ai_ctx = serde_json::json!({ "source_template_id": manifest.id });
    let draft_row = sqlx::query(
        "INSERT INTO xb_schema_drafts (
             tenant_id, subgraph_slug, title, source, base_version,
             schema_doc, ai_context, issues, pass_count, status, created_by
         )
         VALUES ($1, $2, $3, 'TEMPLATE', 1, $4, $5, $6, $7, 'DRAFT', $8)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(trial_slug)
    .bind(draft_title)
    .bind(doc_json)
    .bind(ai_ctx)
    .bind(issues_json)
    .bind(pass_count_i32)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert trial xb_schema_drafts failed: {err}")))?;

    Ok(draft_row.get("id"))
}

async fn persist_trial_node_and_edge_types_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    trial_slug: &str,
    doc: &mut SchemaDoc,
) -> Result<(), AppError> {
    for nt in &mut doc.node_types {
        let props_json = serde_json::to_value(&nt.properties)
            .map_err(|err| AppError::Database(format!("serialize trial properties: {err}")))?;
        let agg_json = serde_json::to_value(&nt.agg_rules)
            .map_err(|err| AppError::Database(format!("serialize trial agg_rules: {err}")))?;
        let derived_json = serde_json::to_value(&nt.derived_rules)
            .map_err(|err| AppError::Database(format!("serialize trial derived_rules: {err}")))?;

        let row = sqlx::query(
            "INSERT INTO xb_node_types (
                 tenant_id, subgraph_slug, name, display_name, description, icon, color,
                 title_template, properties, agg_rules, derived_rules, sync_to_knowledge,
                 pos_x, pos_y, semantic_layer_tier, domain_group
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(trial_slug)
        .bind(&nt.name)
        .bind(&nt.display_name)
        .bind(&nt.description)
        .bind(&nt.icon)
        .bind(&nt.color)
        .bind(&nt.title_template)
        .bind(props_json)
        .bind(agg_json)
        .bind(derived_json)
        .bind(nt.sync_to_knowledge)
        .bind(nt.pos_x)
        .bind(nt.pos_y)
        .bind(nt.semantic_layer_tier.map(SemanticLayerTier::as_str))
        .bind(&nt.domain_group)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert trial xb_node_types failed: {err}")))?;
        nt.id = Some(row.get("id"));
    }

    for et in &mut doc.edge_types {
        let props_json = serde_json::to_value(&et.properties)
            .map_err(|err| AppError::Database(format!("serialize trial edge properties: {err}")))?;
        let row = sqlx::query(
            "INSERT INTO xb_edge_types (
                 tenant_id, subgraph_slug, name, src_type_name, dst_type_name,
                 impact_weight, cardinality, description, properties, propagation
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(trial_slug)
        .bind(&et.name)
        .bind(&et.src_type_name)
        .bind(&et.dst_type_name)
        .bind(et.impact_weight)
        .bind(et.cardinality.as_str())
        .bind(&et.description)
        .bind(props_json)
        .bind(&et.propagation)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert trial xb_edge_types failed: {err}")))?;
        et.id = Some(row.get("id"));
    }
    Ok(())
}

async fn persist_trial_graph_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    trial_slug: &str,
    graph: &MaterializedTrialGraph,
) -> Result<(), AppError> {
    let now = Utc::now();

    for node in &graph.nodes {
        let props_val = serde_json::Value::Object(node.attributes.clone());
        let metrics_val = serde_json::to_value(&node.metrics)
            .map_err(|err| AppError::Database(format!("serialize trial node metrics: {err}")))?;
        sqlx::query(
            "INSERT INTO xb_nodes (
                 id, tenant_id, subgraph_slug, type_name, node_key, title,
                 properties, metrics, geo_lng, geo_lat, source_row_key
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $5)",
        )
        .bind(node.id)
        .bind(tenant_id)
        .bind(trial_slug)
        .bind(&node.type_name)
        .bind(&node.node_key)
        .bind(&node.title)
        .bind(props_val)
        .bind(metrics_val)
        .bind(node.geo.map(|g| g.lng))
        .bind(node.geo.map(|g| g.lat))
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert trial xb_nodes failed: {err}")))?;
    }

    for link in &graph.links {
        sqlx::query(
            "INSERT INTO xb_links (
                 tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id, weight
             )
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(tenant_id)
        .bind(trial_slug)
        .bind(&link.link_type)
        .bind(link.src_id)
        .bind(link.dst_id)
        .bind(link.weight)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert trial xb_links failed: {err}")))?;
    }

    for pt in &graph.points {
        sqlx::query(
            "INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
                 value = EXCLUDED.value",
        )
        .bind(tenant_id)
        .bind(pt.node_id)
        .bind(&pt.metric)
        .bind(now)
        .bind(pt.value)
        .bind(pt.source_tag)
        .execute(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("insert trial xb_metric_series failed: {err}"))
        })?;
    }

    Ok(())
}
