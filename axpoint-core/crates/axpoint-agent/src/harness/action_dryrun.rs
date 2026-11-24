//! Deterministic in-memory dev-sandbox `ActionSpec` dry-run evaluator (`07-STEP29` §29.4).
//!
//! Evaluates `PROPOSED` and `CONFIRMED` actions from a non-`main` subgraph's active `SchemaDraft`
//! (or published `SchemaDoc`) strictly against `slug`'s sandbox nodes and links with zero database
//! writes, zero `main` queries/locks, and mandatory PII masking when `PII_RAW_READ` is absent.

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::{ActionSideEffectTier, DataType, DesignMaturity, Permission};
use axpoint_domain::expr::{ExprValue, eval as eval_expr, parse as parse_expr};
use axpoint_domain::graph::{LinkRecord, NodeListQuery, SubgraphQuery};
use axpoint_domain::json_schema::validate_json_against_schema;
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository, GraphQueryPort,
    GraphStorePort, StudioRepository,
};
use axpoint_domain::studio::{
    ActionParamSpec, ActionSpec, DraftStatus, EdgeTypeSpec, InvokeObjectActionResponse,
    LinkDirection, MutationStepSpec, NodeTypeSpec, ParsedMutationTarget, PropertyKind,
    PropertySpec, SchemaDoc, build_action_input_json_schema, parse_mutation_target,
};
use axpoint_domain::topology::BlueNode;
use axpoint_security::AuthPrincipal;
use uuid::Uuid;

use super::sandbox_eval::{
    add_runtime_scope, build_self_action_env, parse_optional_filter,
    recompute_sandbox_rules_in_memory,
};
use crate::mcp_builder::{optional_uuid_arg, to_json_val};
use crate::mcp_server::{McpServerHub, required_str_arg};

impl<R> McpServerHub<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + DatasetRepository
        + GraphStorePort
        + GraphQueryPort
        + 'static,
{
    pub(super) async fn tool_dryrun_action_spec(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let slug = required_str_arg(args, "subgraph_slug")?;
        if slug == "main" {
            return Err(AppError::bad(
                "DEV_SANDBOX_REQUIRED",
                "axbuilder_dryrun_action_spec requires a non-main dev/trial subgraph",
            ));
        }

        let subgraphs = self.store.list_subgraphs(&principal.tenant_id).await?;
        let sg = subgraphs
            .into_iter()
            .find(|s| s.slug == slug)
            .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;
        if sg.is_main {
            return Err(AppError::bad(
                "DEV_SANDBOX_REQUIRED",
                "axbuilder_dryrun_action_spec requires a non-main dev/trial subgraph",
            ));
        }

        let requested_draft_id = optional_uuid_arg(args, "draft_id")?;
        let (doc, resolved_draft_id) = self
            .resolve_dev_action_schema(&principal.tenant_id, slug, requested_draft_id)
            .await?;

        let type_name = required_str_arg(args, "type_name")?;
        let node_key = required_str_arg(args, "node_key")?;
        let action_name = required_str_arg(args, "action_name")?;
        let raw_inputs = args
            .get("inputs")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let nodes_page = self
            .store
            .list_nodes(
                &principal.tenant_id,
                NodeListQuery {
                    subgraph: Some(String::from(slug)),
                    limit: Some(5_000),
                    ..NodeListQuery::default()
                },
            )
            .await?;
        let dev_links = self
            .store
            .query_subgraph(
                &principal.tenant_id,
                SubgraphQuery {
                    subgraph: Some(String::from(slug)),
                    limit: 2_000,
                    ..SubgraphQuery::default()
                },
            )
            .await
            .map_or_default(|r| r.links);

        let mut resp = simulate_dev_action_dryrun(
            &doc,
            (type_name, node_key, action_name),
            &raw_inputs,
            nodes_page.nodes,
            dev_links,
        )?;

        if !principal.has(Permission::PiiRawRead)
            && let Some(ref mut out_node) = resp.node
        {
            self.mask_sandbox_sample_nodes(
                &principal.tenant_id,
                slug,
                resolved_draft_id,
                std::slice::from_mut(out_node),
            )
            .await;
        }

        to_json_val(&resp, "InvokeObjectActionResponse")
    }

    async fn resolve_dev_action_schema(
        &self,
        tenant_id: &str,
        slug: &str,
        draft_id: Option<Uuid>,
    ) -> Result<(SchemaDoc, Option<Uuid>), AppError> {
        if let Some(did) = draft_id {
            let draft = self
                .store
                .get_draft(tenant_id, did)
                .await?
                .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
            if draft.subgraph_slug != slug {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "draft_id does not belong to target subgraph",
                ));
            }
            return Ok((draft.schema_doc, Some(did)));
        }

        let drafts = self
            .store
            .list_drafts(tenant_id, slug, Some(DraftStatus::Draft))
            .await?;
        if let Some(latest) = drafts.into_iter().next() {
            return Ok((latest.schema_doc, Some(latest.id)));
        }

        let published = self.store.get_published_schema(tenant_id, slug).await?;
        Ok((published.doc, None))
    }
}

fn simulate_dev_action_dryrun(
    doc: &SchemaDoc,
    target: (&str, &str, &str),
    raw_inputs: &serde_json::Value,
    dev_nodes: Vec<BlueNode>,
    mut dev_links: Vec<LinkRecord>,
) -> Result<InvokeObjectActionResponse, AppError> {
    let (type_name, node_key, action_name) = target;
    let nt = doc
        .node_types
        .iter()
        .find(|n| n.name == type_name)
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;
    let action = nt
        .actions
        .iter()
        .find(|a| {
            a.name == action_name
                && matches!(
                    a.maturity,
                    DesignMaturity::Proposed | DesignMaturity::Confirmed
                )
        })
        .ok_or(AppError::NotFound("ACTION_NOT_FOUND"))?;

    let param_env = normalize_and_validate_action_inputs(action, raw_inputs)?;

    let mut nodes_by_id: BTreeMap<Uuid, BlueNode> =
        dev_nodes.into_iter().map(|n| (n.id, n)).collect();
    let self_id = nodes_by_id
        .values()
        .find(|n| n.type_name == type_name && n.node_key == node_key)
        .map(|n| n.id)
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

    if let Some(self_node) = nodes_by_id.get(&self_id) {
        evaluate_action_precondition(action, self_node, &param_env)?;
    }

    let (mutated_nodes, mutated_links) = apply_action_mutations_in_memory(
        doc,
        nt,
        action,
        self_id,
        &param_env,
        &mut nodes_by_id,
        &mut dev_links,
    )?;

    let recomputed_metrics = recompute_sandbox_rules_in_memory(doc, &mut nodes_by_id, &dev_links);

    Ok(InvokeObjectActionResponse {
        status: String::from("DRY_RUN"),
        node: nodes_by_id.remove(&self_id),
        mutated_nodes: mutated_nodes.len(),
        mutated_links: mutated_links.len(),
        recomputed_metrics,
        action_log_id: None,
        approval_id: None,
        idempotency_replay: false,
    })
}

fn normalize_and_validate_action_inputs(
    action: &ActionSpec,
    raw_inputs: &serde_json::Value,
) -> Result<BTreeMap<String, ExprValue>, AppError> {
    let mut map = match raw_inputs {
        serde_json::Value::Null => serde_json::Map::new(),
        serde_json::Value::Object(m) => m.clone(),
        _ => {
            return Err(AppError::bad(
                "ACTION_INPUT_INVALID",
                "action inputs must be a JSON object",
            ));
        }
    };

    for p in &action.parameters {
        if (!map.contains_key(&p.name) || map.get(&p.name) == Some(&serde_json::Value::Null))
            && let Some(def) = &p.default_value
        {
            map.insert(p.name.clone(), def.clone());
        }
    }

    let normalized = serde_json::Value::Object(map.clone());
    let schema = build_action_input_json_schema(action);
    let errs = validate_json_against_schema(&normalized, &schema);
    if !errs.is_empty() {
        return Err(AppError::bad("ACTION_INPUT_INVALID", errs.join("; ")));
    }

    let mut param_env = BTreeMap::new();
    for p in &action.parameters {
        let val = map.get(&p.name).unwrap_or(&serde_json::Value::Null);
        let ev = json_to_expr_value(val, p)?;
        param_env.insert(format!("param.{}", p.name), ev);
    }
    Ok(param_env)
}

fn json_to_expr_value(
    val: &serde_json::Value,
    param: &ActionParamSpec,
) -> Result<ExprValue, AppError> {
    match val {
        serde_json::Value::Null => Ok(ExprValue::Null),
        serde_json::Value::Number(n) => {
            let f = n.as_f64().filter(|v| v.is_finite()).ok_or_else(|| {
                AppError::bad(
                    "ACTION_INPUT_INVALID",
                    format!("parameter '{}' must be a finite number", param.name),
                )
            })?;
            Ok(ExprValue::Number(f))
        }
        serde_json::Value::String(s) => Ok(ExprValue::Text(s.clone())),
        serde_json::Value::Bool(b) => Ok(ExprValue::Bool(*b)),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            if matches!(
                param.data_type,
                DataType::List | DataType::Json | DataType::Struct
            ) {
                Ok(ExprValue::Text(val.to_string()))
            } else {
                Err(AppError::bad(
                    "ACTION_INPUT_INVALID",
                    format!("parameter '{}' must be a scalar value", param.name),
                ))
            }
        }
    }
}

fn evaluate_action_precondition(
    action: &ActionSpec,
    node: &BlueNode,
    param_env: &BTreeMap<String, ExprValue>,
) -> Result<(), AppError> {
    let Some(pre_raw) = action
        .precondition_expr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let ast = parse_expr(pre_raw).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let env = build_self_action_env(&node.metrics, &node.properties, param_env);
    let rep = eval_expr(&ast, &env);
    if !matches!(rep.value, ExprValue::Bool(true)) {
        return Err(AppError::unprocessable(
            "ACTION_PRECONDITION_FAILED",
            format!(
                "ACTION_PRECONDITION_FAILED: action '{}' precondition '{}' evaluated to {:?} on {}:{}",
                action.name, pre_raw, rep.value, node.type_name, node.node_key
            ),
        ));
    }
    Ok(())
}

fn apply_action_mutations_in_memory(
    doc: &SchemaDoc,
    nt: &NodeTypeSpec,
    action: &ActionSpec,
    self_id: Uuid,
    param_env: &BTreeMap<String, ExprValue>,
    nodes_by_id: &mut BTreeMap<Uuid, BlueNode>,
    dev_links: &mut [LinkRecord],
) -> Result<(BTreeSet<Uuid>, BTreeSet<Uuid>), AppError> {
    let mut mutated_nodes = BTreeSet::new();
    let mut mutated_links = BTreeSet::new();

    for mstep in &action.mutations {
        let parsed = parse_mutation_target(&mstep.target, &nt.name, doc)
            .map_err(|msg| AppError::bad("ACTION_TARGET_INVALID", msg))?;
        if action.side_effect_tier == ActionSideEffectTier::SelfOnly
            && !matches!(parsed, ParsedMutationTarget::SelfField { .. })
        {
            return Err(AppError::bad(
                "SIDE_EFFECT_TIER_VIOLATION",
                format!(
                    "SELF_ONLY action '{}' cannot mutate traversal target '{}'",
                    action.name, mstep.target
                ),
            ));
        }

        match parsed {
            ParsedMutationTarget::SelfField { field } => {
                apply_self_mutation_mem(nt, self_id, param_env, mstep, &field, nodes_by_id)?;
                mutated_nodes.insert(self_id);
            }
            ParsedMutationTarget::NeighborNode {
                link_type,
                direction,
                field,
            } => {
                let nids = apply_neighbor_mutation_mem(
                    doc,
                    self_id,
                    param_env,
                    (&link_type, direction, &field),
                    mstep,
                    nodes_by_id,
                    dev_links,
                )?;
                mutated_nodes.extend(nids);
            }
            ParsedMutationTarget::IncidentEdge {
                link_type,
                direction,
                field,
            } => {
                let lids = apply_edge_mutation_mem(
                    doc,
                    self_id,
                    param_env,
                    (&link_type, direction, &field),
                    mstep,
                    nodes_by_id,
                    dev_links,
                )?;
                mutated_links.extend(lids);
            }
        }
    }

    Ok((mutated_nodes, mutated_links))
}

fn ensure_writable_node_field<'a>(
    nt: &'a NodeTypeSpec,
    field: &str,
) -> Result<&'a PropertySpec, AppError> {
    let is_protected = nt.agg_rules.iter().any(|r| r.target_metric == field)
        || nt
            .derived_rules
            .iter()
            .any(|d| d.enabled && d.target_metric == field);
    if is_protected {
        return Err(AppError::bad(
            "DERIVED_METRIC_WRITE_FORBIDDEN",
            format!(
                "DERIVED_METRIC_WRITE_FORBIDDEN: metric '{}.{field}' is produced by a rule and cannot be mutated directly",
                nt.name
            ),
        ));
    }
    nt.properties
        .iter()
        .find(|p| p.name == field)
        .ok_or_else(|| {
            AppError::bad(
                "ACTION_TARGET_INVALID",
                format!("field '{field}' is not declared on '{}'", nt.name),
            )
        })
}

fn write_node_field_mem(
    node: &mut BlueNode,
    prop: &PropertySpec,
    val: &ExprValue,
) -> Result<(), AppError> {
    if prop.kind == PropertyKind::Metric {
        let ExprValue::Number(num) = val else {
            return Err(AppError::unprocessable(
                "ACTION_MUTATION_INVALID",
                format!(
                    "metric '{}' mutation evaluated to non-numeric value",
                    prop.name
                ),
            ));
        };
        node.metrics.insert(prop.name.clone(), *num);
    } else {
        node.properties
            .insert(prop.name.clone(), val.to_json_value());
    }
    node.row_version = node.row_version.saturating_add(1);
    Ok(())
}

fn apply_self_mutation_mem(
    nt: &NodeTypeSpec,
    self_id: Uuid,
    param_env: &BTreeMap<String, ExprValue>,
    mstep: &MutationStepSpec,
    field: &str,
    nodes_by_id: &mut BTreeMap<Uuid, BlueNode>,
) -> Result<(), AppError> {
    let prop = ensure_writable_node_field(nt, field)?;
    let ast =
        parse_expr(&mstep.expression).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let self_node = nodes_by_id
        .get_mut(&self_id)
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    let env = build_self_action_env(&self_node.metrics, &self_node.properties, param_env);
    let val = eval_expr(&ast, &env).value;
    write_node_field_mem(self_node, prop, &val)
}

fn resolve_neighbor_props_map<'a>(
    doc: &'a SchemaDoc,
    owner_type: &str,
    link_type: &str,
    dir: LinkDirection,
    field: &str,
) -> Result<BTreeMap<String, &'a PropertySpec>, AppError> {
    let matched_edges: Vec<&EdgeTypeSpec> = doc
        .edge_types
        .iter()
        .filter(|e| {
            e.name == link_type
                && match dir {
                    LinkDirection::Out => e.src_type_name == owner_type,
                    LinkDirection::In => e.dst_type_name == owner_type,
                }
        })
        .collect();
    if matched_edges.is_empty() {
        return Err(AppError::bad(
            "ACTION_TARGET_INVALID",
            "edge traversal not found",
        ));
    }

    let mut props_by_type: BTreeMap<String, &'a PropertySpec> = BTreeMap::new();
    for edge in matched_edges {
        let nb_type_name = match dir {
            LinkDirection::Out => &edge.dst_type_name,
            LinkDirection::In => &edge.src_type_name,
        };
        if let std::collections::btree_map::Entry::Vacant(e) =
            props_by_type.entry(nb_type_name.clone())
        {
            let nb_nt = doc
                .node_types
                .iter()
                .find(|n| n.name == *nb_type_name)
                .ok_or_else(|| {
                    AppError::bad("ACTION_TARGET_INVALID", "neighbor node type not found")
                })?;
            let prop = ensure_writable_node_field(nb_nt, field)?;
            e.insert(prop);
        }
    }
    Ok(props_by_type)
}

fn apply_neighbor_mutation_mem(
    doc: &SchemaDoc,
    self_id: Uuid,
    param_env: &BTreeMap<String, ExprValue>,
    traversal: (&str, LinkDirection, &str),
    mstep: &MutationStepSpec,
    nodes_by_id: &mut BTreeMap<Uuid, BlueNode>,
    dev_links: &[LinkRecord],
) -> Result<Vec<Uuid>, AppError> {
    let (link_type, dir, field) = traversal;
    let self_snap = nodes_by_id
        .get(&self_id)
        .cloned()
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    let props_by_type =
        resolve_neighbor_props_map(doc, &self_snap.type_name, link_type, dir, field)?;

    let expr_ast =
        parse_expr(&mstep.expression).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let filter_ast = parse_optional_filter(mstep.filter.as_deref())?;

    let mut target_ids = Vec::new();
    for l in dev_links {
        if l.link_type != link_type {
            continue;
        }
        match dir {
            LinkDirection::Out if l.src_node_id == self_id => target_ids.push(l.dst_node_id),
            LinkDirection::In if l.dst_node_id == self_id => target_ids.push(l.src_node_id),
            _ => {}
        }
    }

    let mut mutated = Vec::new();
    for nid in target_ids {
        let Some(nb_node) = nodes_by_id.get_mut(&nid) else {
            continue;
        };
        let Some(&prop) = props_by_type.get(&nb_node.type_name) else {
            continue;
        };
        let mut env = build_self_action_env(&self_snap.metrics, &self_snap.properties, param_env);
        add_runtime_scope(
            &mut env,
            "target",
            &nb_node.metrics,
            &nb_node.properties,
            true,
        );
        if let Some(ref f_ast) = filter_ast
            && !matches!(eval_expr(f_ast, &env).value, ExprValue::Bool(true))
        {
            continue;
        }
        let val = eval_expr(&expr_ast, &env).value;
        write_node_field_mem(nb_node, prop, &val)?;
        mutated.push(nid);
    }
    Ok(mutated)
}

fn ensure_writable_edge_spec_field(edge_spec: &EdgeTypeSpec, field: &str) -> Result<(), AppError> {
    let is_protected = edge_spec.agg_rules.iter().any(|r| r.target_metric == field)
        || edge_spec
            .derived_rules
            .iter()
            .any(|d| d.enabled && d.target_metric == field);
    if is_protected {
        return Err(AppError::bad(
            "DERIVED_METRIC_WRITE_FORBIDDEN",
            format!(
                "edge metric '{}.{field}' is produced by a rule",
                edge_spec.name
            ),
        ));
    }
    if field != "weight" && !edge_spec.properties.iter().any(|p| p.name == field) {
        return Err(AppError::bad(
            "ACTION_TARGET_INVALID",
            format!(
                "field '{field}' is not declared on edge '{}'",
                edge_spec.name
            ),
        ));
    }
    Ok(())
}

fn apply_edge_mutation_mem(
    doc: &SchemaDoc,
    self_id: Uuid,
    param_env: &BTreeMap<String, ExprValue>,
    traversal: (&str, LinkDirection, &str),
    mstep: &MutationStepSpec,
    nodes_by_id: &BTreeMap<Uuid, BlueNode>,
    dev_links: &mut [LinkRecord],
) -> Result<Vec<Uuid>, AppError> {
    let (link_type, dir, field) = traversal;
    let self_snap = nodes_by_id
        .get(&self_id)
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    let matched_edges: Vec<&EdgeTypeSpec> = doc
        .edge_types
        .iter()
        .filter(|e| {
            e.name == link_type
                && match dir {
                    LinkDirection::Out => e.src_type_name == self_snap.type_name,
                    LinkDirection::In => e.dst_type_name == self_snap.type_name,
                }
        })
        .collect();
    let Some(&fallback_spec) = matched_edges.first() else {
        return Err(AppError::bad(
            "ACTION_TARGET_INVALID",
            "edge type not found",
        ));
    };
    for es in &matched_edges {
        ensure_writable_edge_spec_field(es, field)?;
    }

    let expr_ast =
        parse_expr(&mstep.expression).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let filter_ast = parse_optional_filter(mstep.filter.as_deref())?;
    let mut mutated = Vec::new();

    for l in dev_links.iter_mut() {
        let incident = l.link_type == link_type
            && match dir {
                LinkDirection::Out => l.src_node_id == self_id,
                LinkDirection::In => l.dst_node_id == self_id,
            };
        if !incident {
            continue;
        }
        let src_t = nodes_by_id
            .get(&l.src_node_id)
            .map(|n| n.type_name.as_str());
        let dst_t = nodes_by_id
            .get(&l.dst_node_id)
            .map(|n| n.type_name.as_str());
        let exact_spec = match (src_t, dst_t) {
            (Some(st), Some(dt)) => matched_edges
                .iter()
                .copied()
                .find(|e| e.src_type_name == st && e.dst_type_name == dt)
                .unwrap_or(fallback_spec),
            _ => fallback_spec,
        };
        ensure_writable_edge_spec_field(exact_spec, field)?;

        let mut env = build_self_action_env(&self_snap.metrics, &self_snap.properties, param_env);
        env.metrics.insert(String::from("weight"), l.weight);
        env.properties
            .insert(String::from("edge.weight"), ExprValue::Number(l.weight));
        add_runtime_scope(&mut env, "edge", &l.metrics, &l.properties, true);
        if let Some(ref f_ast) = filter_ast
            && !matches!(eval_expr(f_ast, &env).value, ExprValue::Bool(true))
        {
            continue;
        }
        let val = eval_expr(&expr_ast, &env).value;
        write_link_field_mem(l, exact_spec, field, &val)?;
        mutated.push(l.id);
    }

    Ok(mutated)
}

fn write_link_field_mem(
    link: &mut LinkRecord,
    edge_spec: &EdgeTypeSpec,
    field: &str,
    val: &ExprValue,
) -> Result<(), AppError> {
    if field == "weight" {
        let ExprValue::Number(w) = val else {
            return Err(AppError::unprocessable(
                "ACTION_MUTATION_INVALID",
                "edge weight must evaluate to a finite positive number",
            ));
        };
        link.weight = w.max(1e-6);
    } else {
        let prop = edge_spec
            .properties
            .iter()
            .find(|p| p.name == field)
            .ok_or_else(|| {
                AppError::bad(
                    "ACTION_TARGET_INVALID",
                    format!(
                        "field '{field}' is not declared on edge '{}'",
                        edge_spec.name
                    ),
                )
            })?;
        if prop.kind == PropertyKind::Metric {
            let ExprValue::Number(num) = val else {
                return Err(AppError::unprocessable(
                    "ACTION_MUTATION_INVALID",
                    format!(
                        "edge metric '{}' mutation evaluated to non-numeric value",
                        prop.name
                    ),
                ));
            };
            link.metrics.insert(prop.name.clone(), *num);
        } else {
            link.properties
                .insert(prop.name.clone(), val.to_json_value());
        }
    }
    link.row_version = link.row_version.saturating_add(1);
    Ok(())
}
