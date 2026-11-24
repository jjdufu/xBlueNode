//! Transactional `ActionSpec` CAS executor and dev-sandbox `Trial Tick` runtime readiness certifier (`04-STEP26` §26.2 & §26.4).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::{ActionSideEffectTier, DataType, DesignMaturity, OutboxTopic};
use axpoint_domain::expr::{EvalEnv, ExprValue, eval as eval_expr, parse as parse_expr};
use axpoint_domain::json_schema::validate_json_against_schema;
use axpoint_domain::platform::NewOutboxEvent;
use axpoint_domain::studio::{
    ActionParamSpec, ActionSpec, CertifySubgraphRequest, EdgeTypeSpec, InvokeObjectActionRequest,
    InvokeObjectActionResponse, IssueSeverity, LinkDirection, MutationStepSpec, NodeTypeSpec,
    ParsedMutationTarget, PropertyKind, PropertySpec, RuntimeCertReport, SchemaDoc,
    build_action_input_json_schema, count_high_fanout_edges, evaluate_cert_tier_and_score,
    parse_mutation_target, scan_dead_knowledge_with_external, validate_with_metrics,
};
use axpoint_domain::topology::BlueNode;
use chrono::Utc;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::aggregate_exec::{
    add_runtime_scope, propagate_subgraph_4path_tx, record_metric_snapshot_delta,
};
use crate::nodes::decode_blue_node;
use crate::outbox::insert_outbox_tx;
use crate::studio_repo::{
    load_schema_doc_tx, load_validation_context_tx, require_subgraph_exists_tx,
};
use crate::tenant::begin_tenant_tx;

/// Target `(type_name, node_key, action_name)` tuple for [`invoke_object_action_impl`].
pub type ObjectActionTarget<'a> = (&'a str, &'a str, &'a str);

/// Executes a published `ActionSpec` under `SELECT ... FOR UPDATE` CAS, idempotency,
/// precondition guard (`422`), optional approval ticket (`202`), and synchronous 4-path propagation (`04-STEP26` §26.2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] on invalid inputs or protected metric writes,
/// [`AppError::NotFound`] when the type/node/action is missing, [`AppError::Conflict`] on CAS or idempotency mismatch,
/// [`AppError::Unprocessable`] (`ACTION_PRECONDITION_FAILED`) when `precondition_expr` evaluates to non-`true`,
/// or [`AppError::Database`] on PostgreSQL failure.
pub async fn invoke_object_action_impl(
    store: &PgStore,
    tenant_id: &str,
    actor_user_id: Option<Uuid>,
    target: ObjectActionTarget<'_>,
    req: InvokeObjectActionRequest,
) -> Result<InvokeObjectActionResponse, AppError> {
    let (raw_type, raw_key, raw_action) = target;
    let type_name = raw_type.trim();
    let node_key = raw_key.trim();
    let action_name = raw_action.trim();
    let idem_key = req.idempotency_key.trim();
    if idem_key.is_empty() || idem_key.len() > 128 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "idempotency_key must be 1..=128 characters",
        ));
    }

    let target_ref = format!("{type_name}:{node_key}:{action_name}");
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    if !req.dry_run {
        let lock_key = format!("{tenant_id}:act_idem:{idem_key}");
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(&lock_key)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("acquire action idempotency advisory lock: {e}"))
            })?;
    }

    let _ = require_subgraph_exists_tx(&mut tx, tenant_id, "main").await?;
    let doc = load_schema_doc_tx(&mut tx, tenant_id, "main").await?;
    let nt = doc
        .node_types
        .iter()
        .find(|n| n.name == type_name)
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;
    let action = nt
        .actions
        .iter()
        .find(|a| a.name == action_name && a.maturity == DesignMaturity::Confirmed)
        .ok_or(AppError::NotFound("ACTION_NOT_FOUND"))?;

    let (norm_inputs, param_env) = normalize_and_validate_inputs(action, &req.inputs)?;

    if !req.dry_run
        && let Some(replayed) = check_action_idempotency_tx(
            &mut tx,
            tenant_id,
            idem_key,
            &target_ref,
            &norm_inputs,
            (type_name, node_key),
        )
        .await?
    {
        tx.commit().await.map_err(|e| {
            AppError::Database(format!("commit replayed invoke_object_action: {e}"))
        })?;
        return Ok(replayed);
    }

    let node = lock_target_node_tx(&mut tx, tenant_id, type_name, node_key).await?;
    if let Some(expected_ver) = req.expected_version
        && expected_ver != node.row_version
    {
        return Err(AppError::Conflict("VERSION_CONFLICT"));
    }

    evaluate_action_precondition(action, &node, &param_env)?;

    if action.require_approval && !req.dry_run {
        let resp = create_action_approval_tx(
            &mut tx,
            tenant_id,
            actor_user_id,
            (nt, action, &node),
            (&target_ref, idem_key, &norm_inputs),
        )
        .await?;
        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("commit pending action approval tx: {e}")))?;
        return Ok(resp);
    }

    let resp = execute_action_mutations_and_commit_tx(
        tx,
        tenant_id,
        (&doc, nt, action),
        (node, param_env, norm_inputs),
        (&target_ref, idem_key, req.dry_run),
    )
    .await?;
    Ok(resp)
}

async fn check_action_idempotency_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    idem_key: &str,
    target_ref: &str,
    norm_inputs: &serde_json::Value,
    node_lookup: (&str, &str),
) -> Result<Option<InvokeObjectActionResponse>, AppError> {
    let row_opt = sqlx::query(
        "SELECT id, target, payload, status
           FROM ax_action_log
          WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(idem_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lookup ax_action_log idempotency: {err}")))?;

    if let Some(r) = row_opt {
        let stored_target: String = r.get("target");
        let status: String = r.get("status");
        let payload: serde_json::Value = r.get("payload");
        if stored_target != target_ref
            || status == "FAILED"
            || payload.get("inputs") != Some(norm_inputs)
        {
            return Err(AppError::Conflict("IDEMPOTENCY_KEY_CONFLICT"));
        }
        let log_id: i64 = r.get("id");
        let cached_node: Option<BlueNode> = payload
            .get("node")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());
        let node_out = match cached_node {
            Some(n) => Some(n),
            None => fetch_node_optional_tx(tx, tenant_id, node_lookup.0, node_lookup.1).await?,
        };
        let mutated_nodes = payload
            .get("mutated_nodes")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(1);
        let mutated_links = payload
            .get("mutated_links")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0);
        let recomputed_metrics = payload
            .get("recomputed_metrics")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0);
        return Ok(Some(InvokeObjectActionResponse {
            status: String::from("APPLIED"),
            node: node_out,
            mutated_nodes,
            mutated_links,
            recomputed_metrics,
            action_log_id: Some(log_id),
            approval_id: None,
            idempotency_replay: true,
        }));
    }

    let pending_row = sqlx::query(
        "SELECT id, source_ref, detail FROM ax_approvals
          WHERE tenant_id = $1
            AND source_kind = 'OBJECT_ACTION'
            AND detail->>'idempotency_key' = $2
          LIMIT 1",
    )
    .bind(tenant_id)
    .bind(idem_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lookup ax_approvals idempotency: {err}")))?;

    if let Some(arow) = pending_row {
        let app_id: Uuid = arow.get("id");
        let stored_ref: String = arow.get("source_ref");
        let detail: serde_json::Value = arow.get("detail");
        if stored_ref != target_ref || detail.get("inputs") != Some(norm_inputs) {
            return Err(AppError::Conflict("IDEMPOTENCY_KEY_CONFLICT"));
        }
        let fresh_node =
            fetch_node_optional_tx(tx, tenant_id, node_lookup.0, node_lookup.1).await?;
        return Ok(Some(InvokeObjectActionResponse {
            status: String::from("WAITING_APPROVAL"),
            node: fresh_node,
            mutated_nodes: 0,
            mutated_links: 0,
            recomputed_metrics: 0,
            action_log_id: None,
            approval_id: Some(app_id),
            idempotency_replay: true,
        }));
    }

    Ok(None)
}

fn normalize_and_validate_inputs(
    action: &ActionSpec,
    raw_inputs: &serde_json::Value,
) -> Result<(serde_json::Value, BTreeMap<String, ExprValue>), AppError> {
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
    Ok((normalized, param_env))
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

async fn lock_target_node_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    node_key: &str,
) -> Result<BlueNode, AppError> {
    let row = sqlx::query(
        "SELECT id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, source_dataset_id, source_row_key,
                row_version, created_at, updated_at
           FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = $2 AND node_key = $3
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(node_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock target node for action: {err}")))?
    .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    Ok(decode_blue_node(&row))
}

async fn fetch_node_optional_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    node_key: &str,
) -> Result<Option<BlueNode>, AppError> {
    let row_opt = sqlx::query(
        "SELECT id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, source_dataset_id, source_row_key,
                row_version, created_at, updated_at
           FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = $2 AND node_key = $3",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(node_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("fetch target node after action: {err}")))?;
    Ok(row_opt.as_ref().map(decode_blue_node))
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
                "action '{}' precondition '{}' evaluated to {:?} on {}:{}",
                action.name, pre_raw, rep.value, node.type_name, node.node_key
            ),
        ));
    }
    Ok(())
}

fn build_self_action_env(
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

async fn create_action_approval_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    actor_user_id: Option<Uuid>,
    schema_meta: (&NodeTypeSpec, &ActionSpec, &BlueNode),
    req_meta: (&str, &str, &serde_json::Value),
) -> Result<InvokeObjectActionResponse, AppError> {
    let (nt, action, node) = schema_meta;
    let (target_ref, idem_key, norm_inputs) = req_meta;
    let title = format!(
        "{} · {} ({})",
        nt.display_name, action.display_name, node.node_key
    );
    let detail = serde_json::json!({
        "type_name": nt.name,
        "node_key": node.node_key,
        "node_id": node.id,
        "action_name": action.name,
        "inputs": norm_inputs,
        "idempotency_key": idem_key,
        "requested_by": actor_user_id,
    });
    let approval_id: Uuid = sqlx::query_scalar(
        "INSERT INTO ax_approvals (tenant_id, source_kind, source_ref, title, detail, status)
         VALUES ($1, 'OBJECT_ACTION', $2, $3, $4, 'PENDING')
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(target_ref)
    .bind(title)
    .bind(detail)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ax_approvals for OBJECT_ACTION: {err}")))?;

    Ok(InvokeObjectActionResponse {
        status: String::from("WAITING_APPROVAL"),
        node: Some(node.clone()),
        mutated_nodes: 0,
        mutated_links: 0,
        recomputed_metrics: 0,
        action_log_id: None,
        approval_id: Some(approval_id),
        idempotency_replay: false,
    })
}

async fn execute_action_mutations_and_commit_tx(
    mut tx: Transaction<'_, Postgres>,
    tenant_id: &str,
    schema_ctx: (&SchemaDoc, &NodeTypeSpec, &ActionSpec),
    mut exec_state: (BlueNode, BTreeMap<String, ExprValue>, serde_json::Value),
    log_meta: (&str, &str, bool),
) -> Result<InvokeObjectActionResponse, AppError> {
    let (doc, nt, action) = schema_ctx;
    let (ref mut self_node, ref param_env, ref norm_inputs) = exec_state;
    let (_, _, dry_run) = log_meta;

    if dry_run {
        sqlx::query("SAVEPOINT action_dry_run")
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("create SAVEPOINT action_dry_run: {e}")))?;
    }

    let mut mutated_nodes: BTreeSet<Uuid> = BTreeSet::new();
    let mut mutated_links: BTreeSet<Uuid> = BTreeSet::new();

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
                apply_self_mutation_tx(&mut tx, tenant_id, nt, self_node, param_env, mstep, &field)
                    .await?;
                mutated_nodes.insert(self_node.id);
            }
            ParsedMutationTarget::NeighborNode {
                link_type,
                direction,
                field,
            } => {
                let nids = apply_neighbor_mutation_tx(
                    &mut tx,
                    tenant_id,
                    doc,
                    (self_node, param_env),
                    (&link_type, direction, &field),
                    mstep,
                )
                .await?;
                mutated_nodes.extend(nids);
            }
            ParsedMutationTarget::IncidentEdge {
                link_type,
                direction,
                field,
            } => {
                let lids = apply_edge_mutation_tx(
                    &mut tx,
                    tenant_id,
                    doc,
                    (self_node, param_env),
                    (&link_type, direction, &field),
                    mstep,
                )
                .await?;
                mutated_links.extend(lids);
            }
        }
    }

    let prop_stats = propagate_subgraph_4path_tx(&mut tx, tenant_id, "main", doc).await?;
    let updated_node = fetch_node_optional_tx(
        &mut tx,
        tenant_id,
        &self_node.type_name,
        &self_node.node_key,
    )
    .await?;

    finalize_action_tx(
        tx,
        tenant_id,
        self_node.id,
        updated_node,
        (&mutated_nodes, mutated_links.len(), &prop_stats),
        norm_inputs,
        log_meta,
    )
    .await
}

async fn finalize_action_tx(
    mut tx: Transaction<'_, Postgres>,
    tenant_id: &str,
    self_node_id: Uuid,
    updated_node: Option<BlueNode>,
    counts: (
        &BTreeSet<Uuid>,
        usize,
        &crate::aggregate_exec::FourPathExecStats,
    ),
    norm_inputs: &serde_json::Value,
    log_meta: (&str, &str, bool),
) -> Result<InvokeObjectActionResponse, AppError> {
    let (mutated_nodes, mutated_links_cnt, prop_stats) = counts;
    let (target_ref, idem_key, dry_run) = log_meta;

    if dry_run {
        sqlx::query("ROLLBACK TO SAVEPOINT action_dry_run")
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("rollback SAVEPOINT action_dry_run: {e}")))?;
        sqlx::query("RELEASE SAVEPOINT action_dry_run")
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("release SAVEPOINT action_dry_run: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("commit dry_run action tx: {e}")))?;
        return Ok(InvokeObjectActionResponse {
            status: String::from("DRY_RUN"),
            node: updated_node,
            mutated_nodes: mutated_nodes.len(),
            mutated_links: mutated_links_cnt,
            recomputed_metrics: prop_stats.recomputed_metrics,
            action_log_id: None,
            approval_id: None,
            idempotency_replay: false,
        });
    }

    let log_payload = serde_json::json!({
        "inputs": norm_inputs,
        "node": &updated_node,
        "mutated_nodes": mutated_nodes.len(),
        "mutated_links": mutated_links_cnt,
        "recomputed_metrics": prop_stats.recomputed_metrics,
    });
    let log_id: i64 = sqlx::query_scalar(
        "INSERT INTO ax_action_log (
             tenant_id, node_id, source_kind, action_kind, target, payload, status, idempotency_key
         )
         VALUES ($1, $2, 'OBJECT_ACTION', 'OBJECT_ACTION', $3, $4, 'DONE', $5)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(self_node_id.to_string())
    .bind(target_ref)
    .bind(log_payload)
    .bind(idem_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("insert ax_action_log failed: {e}")))?;

    let mut notify_ids = mutated_nodes.clone();
    notify_ids.extend(prop_stats.changed_nodes.iter().copied());
    let notify_vec: Vec<Uuid> = notify_ids.into_iter().collect();
    for chunk in notify_vec.chunks(1_000) {
        insert_outbox_tx(
            &mut tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeMetricsChanged,
                payload: serde_json::json!({ "node_ids": chunk, "origin": "OBJECT_ACTION" }),
            },
        )
        .await?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit invoke_object_action tx: {e}")))?;

    Ok(InvokeObjectActionResponse {
        status: String::from("APPLIED"),
        node: updated_node,
        mutated_nodes: mutated_nodes.len(),
        mutated_links: mutated_links_cnt,
        recomputed_metrics: prop_stats.recomputed_metrics,
        action_log_id: Some(log_id),
        approval_id: None,
        idempotency_replay: false,
    })
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
                "metric '{}.{field}' is produced by a rule and cannot be mutated directly",
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

async fn apply_self_mutation_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    nt: &NodeTypeSpec,
    self_node: &mut BlueNode,
    param_env: &BTreeMap<String, ExprValue>,
    mstep: &MutationStepSpec,
    field: &str,
) -> Result<(), AppError> {
    let prop = ensure_writable_node_field(nt, field)?;
    let ast =
        parse_expr(&mstep.expression).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let env = build_self_action_env(&self_node.metrics, &self_node.properties, param_env);
    let val = eval_expr(&ast, &env).value;
    write_node_field_tx(
        tx,
        tenant_id,
        self_node.id,
        (&mut self_node.properties, &mut self_node.metrics),
        prop,
        val,
    )
    .await?;
    self_node.row_version = self_node.row_version.saturating_add(1);
    Ok(())
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

async fn apply_neighbor_mutation_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    doc: &SchemaDoc,
    self_ctx: (&BlueNode, &BTreeMap<String, ExprValue>),
    traversal: (&str, LinkDirection, &str),
    mstep: &MutationStepSpec,
) -> Result<Vec<Uuid>, AppError> {
    let (self_node, param_env) = self_ctx;
    let (link_type, dir, field) = traversal;
    let props_by_type =
        resolve_neighbor_props_map(doc, &self_node.type_name, link_type, dir, field)?;
    let nb_types: Vec<String> = props_by_type.keys().cloned().collect();
    let sql = match dir {
        LinkDirection::Out => {
            "SELECT n.id, n.type_name, n.properties, n.metrics FROM xb_links l
               JOIN xb_nodes n ON n.tenant_id = l.tenant_id AND n.id = l.dst_node_id
              WHERE l.tenant_id = $1 AND l.subgraph_slug = 'main'
                AND l.link_type = $2 AND l.src_node_id = $3 AND n.type_name = ANY($4)
              FOR UPDATE OF n"
        }
        LinkDirection::In => {
            "SELECT n.id, n.type_name, n.properties, n.metrics FROM xb_links l
               JOIN xb_nodes n ON n.tenant_id = l.tenant_id AND n.id = l.src_node_id
              WHERE l.tenant_id = $1 AND l.subgraph_slug = 'main'
                AND l.link_type = $2 AND l.dst_node_id = $3 AND n.type_name = ANY($4)
              FOR UPDATE OF n"
        }
    };
    let rows = sqlx::query(sql)
        .bind(tenant_id)
        .bind(link_type)
        .bind(self_node.id)
        .bind(&nb_types)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("lock neighbor nodes for action: {e}")))?;

    let expr_ast =
        parse_expr(&mstep.expression).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let filter_ast = parse_optional_filter(mstep.filter.as_deref())?;
    let mut mutated = Vec::new();
    for r in rows {
        let nid: Uuid = r.get("id");
        let ntype: String = r.get("type_name");
        let Some(&prop) = props_by_type.get(&ntype) else {
            continue;
        };
        let mut props = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let mut metrics: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();
        let mut env = build_self_action_env(&self_node.metrics, &self_node.properties, param_env);
        add_runtime_scope(&mut env, "target", &metrics, &props, true);
        if let Some(ref f_ast) = filter_ast
            && !matches!(eval_expr(f_ast, &env).value, ExprValue::Bool(true))
        {
            continue;
        }
        let val = eval_expr(&expr_ast, &env).value;
        write_node_field_tx(tx, tenant_id, nid, (&mut props, &mut metrics), prop, val).await?;
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

async fn lock_incident_links_for_action_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    link_type: &str,
    dir: LinkDirection,
    self_node_id: Uuid,
) -> Result<Vec<sqlx::postgres::PgRow>, AppError> {
    let sql = match dir {
        LinkDirection::Out => {
            "SELECT l.id, s.type_name AS src_type, d.type_name AS dst_type,
                    COALESCE(l.weight, 1.0) AS weight, l.properties, l.metrics
               FROM xb_links l
               LEFT JOIN xb_nodes s ON s.tenant_id = l.tenant_id AND s.id = l.src_node_id
               LEFT JOIN xb_nodes d ON d.tenant_id = l.tenant_id AND d.id = l.dst_node_id
              WHERE l.tenant_id = $1 AND l.subgraph_slug = 'main'
                AND l.link_type = $2 AND l.src_node_id = $3
              FOR UPDATE OF l"
        }
        LinkDirection::In => {
            "SELECT l.id, s.type_name AS src_type, d.type_name AS dst_type,
                    COALESCE(l.weight, 1.0) AS weight, l.properties, l.metrics
               FROM xb_links l
               LEFT JOIN xb_nodes s ON s.tenant_id = l.tenant_id AND s.id = l.src_node_id
               LEFT JOIN xb_nodes d ON d.tenant_id = l.tenant_id AND d.id = l.dst_node_id
              WHERE l.tenant_id = $1 AND l.subgraph_slug = 'main'
                AND l.link_type = $2 AND l.dst_node_id = $3
              FOR UPDATE OF l"
        }
    };
    sqlx::query(sql)
        .bind(tenant_id)
        .bind(link_type)
        .bind(self_node_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("lock incident links for action: {e}")))
}

async fn apply_edge_mutation_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    doc: &SchemaDoc,
    self_ctx: (&BlueNode, &BTreeMap<String, ExprValue>),
    traversal: (&str, LinkDirection, &str),
    mstep: &MutationStepSpec,
) -> Result<Vec<Uuid>, AppError> {
    let (self_node, param_env) = self_ctx;
    let (link_type, dir, field) = traversal;
    let matched_edges: Vec<&EdgeTypeSpec> = doc
        .edge_types
        .iter()
        .filter(|e| {
            e.name == link_type
                && match dir {
                    LinkDirection::Out => e.src_type_name == self_node.type_name,
                    LinkDirection::In => e.dst_type_name == self_node.type_name,
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

    let rows =
        lock_incident_links_for_action_tx(tx, tenant_id, link_type, dir, self_node.id).await?;
    let expr_ast =
        parse_expr(&mstep.expression).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
    let filter_ast = parse_optional_filter(mstep.filter.as_deref())?;
    let mut mutated = Vec::new();
    for r in rows {
        let lid: Uuid = r.get("id");
        let src_type: Option<String> = r.get("src_type");
        let dst_type: Option<String> = r.get("dst_type");
        let exact_spec = match (&src_type, &dst_type) {
            (Some(st), Some(dt)) => matched_edges
                .iter()
                .copied()
                .find(|e| e.src_type_name == *st && e.dst_type_name == *dt)
                .unwrap_or(fallback_spec),
            _ => fallback_spec,
        };
        ensure_writable_edge_spec_field(exact_spec, field)?;
        let mut weight: f64 = r.get("weight");
        let mut props = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let mut metrics: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();
        let mut env = build_self_action_env(&self_node.metrics, &self_node.properties, param_env);
        env.metrics.insert(String::from("weight"), weight);
        env.properties
            .insert(String::from("edge.weight"), ExprValue::Number(weight));
        add_runtime_scope(&mut env, "edge", &metrics, &props, true);
        if let Some(ref f_ast) = filter_ast
            && !matches!(eval_expr(f_ast, &env).value, ExprValue::Bool(true))
        {
            continue;
        }
        let val = eval_expr(&expr_ast, &env).value;
        write_link_field_tx(
            tx,
            tenant_id,
            lid,
            exact_spec,
            field,
            (&mut weight, &mut props, &mut metrics),
            val,
        )
        .await?;
        mutated.push(lid);
    }
    Ok(mutated)
}

fn parse_optional_filter(
    raw: Option<&str>,
) -> Result<Option<axpoint_domain::expr::Expr>, AppError> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    parse_expr(trimmed)
        .map(Some)
        .map_err(|e| AppError::bad("EXPR_INVALID", e.message))
}

async fn write_node_field_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_id: Uuid,
    state: (
        &mut serde_json::Map<String, serde_json::Value>,
        &mut BTreeMap<String, f64>,
    ),
    prop: &PropertySpec,
    val: ExprValue,
) -> Result<(), AppError> {
    let (props, metrics) = state;
    let now = Utc::now();
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
        let old_val = metrics.get(&prop.name).copied();
        metrics.insert(prop.name.clone(), num);
        record_metric_snapshot_delta(props, &prop.name, old_val, &now.to_rfc3339());
        sqlx::query(
            "INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag)
             VALUES ($1, $2, $3, $4, $5, 'ACTION')
             ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(tenant_id)
        .bind(node_id)
        .bind(&prop.name)
        .bind(now)
        .bind(num)
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("insert action xb_metric_series: {e}")))?;
    } else {
        props.insert(prop.name.clone(), val.to_json_value());
    }

    let m_json = serde_json::to_value(&*metrics)
        .map_err(|e| AppError::Database(format!("serialize mutated node metrics: {e}")))?;
    sqlx::query(
        "UPDATE xb_nodes
            SET properties = $2, metrics = $3, row_version = row_version + 1, updated_at = NOW()
          WHERE tenant_id = $1 AND id = $4",
    )
    .bind(tenant_id)
    .bind(serde_json::Value::Object(props.clone()))
    .bind(m_json)
    .bind(node_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("update mutated xb_nodes: {e}")))?;
    Ok(())
}

async fn write_link_field_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    link_id: Uuid,
    edge_spec: &EdgeTypeSpec,
    field: &str,
    state: (
        &mut f64,
        &mut serde_json::Map<String, serde_json::Value>,
        &mut BTreeMap<String, f64>,
    ),
    val: ExprValue,
) -> Result<(), AppError> {
    let (weight, props, metrics) = state;
    let now = Utc::now();
    if field == "weight" {
        let ExprValue::Number(w) = val else {
            return Err(AppError::unprocessable(
                "ACTION_MUTATION_INVALID",
                "edge weight must evaluate to a finite positive number",
            ));
        };
        *weight = w.max(1e-6);
    } else {
        let prop = edge_spec
            .properties
            .iter()
            .find(|p| p.name == field)
            .ok_or_else(|| AppError::bad("ACTION_TARGET_INVALID", "edge field not declared"))?;
        if prop.kind == PropertyKind::Metric {
            let ExprValue::Number(num) = val else {
                return Err(AppError::unprocessable(
                    "ACTION_MUTATION_INVALID",
                    format!("edge metric '{field}' evaluated to non-numeric value"),
                ));
            };
            let old_val = metrics.get(field).copied();
            metrics.insert(String::from(field), num);
            record_metric_snapshot_delta(props, field, old_val, &now.to_rfc3339());
            sqlx::query(
                "INSERT INTO xb_link_metric_series (tenant_id, link_id, metric_key, ts, val_num, source_tag)
                 VALUES ($1, $2, $3, $4, $5, 'ACTION')
                 ON CONFLICT (tenant_id, link_id, metric_key, ts) DO UPDATE SET val_num = EXCLUDED.val_num, source_tag = 'ACTION'",
            )
            .bind(tenant_id)
            .bind(link_id)
            .bind(field)
            .bind(now)
            .bind(num)
            .execute(&mut **tx)
            .await
            .map_err(|e| AppError::Database(format!("insert action xb_link_metric_series: {e}")))?;
        } else {
            props.insert(String::from(field), val.to_json_value());
        }
    }

    let m_json = serde_json::to_value(&*metrics)
        .map_err(|e| AppError::Database(format!("serialize mutated link metrics: {e}")))?;
    sqlx::query(
        "UPDATE xb_links
            SET weight = $2, properties = $3, metrics = $4, row_version = row_version + 1, updated_at = NOW()
          WHERE tenant_id = $1 AND id = $5",
    )
    .bind(tenant_id)
    .bind(*weight)
    .bind(serde_json::Value::Object(props.clone()))
    .bind(m_json)
    .bind(link_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("update mutated xb_links: {e}")))?;
    Ok(())
}

/// Evaluates L0–L4 runtime readiness, Dead Knowledge closure, and `SAVEPOINT` `Trial Tick` certification (`04-STEP26` §26.4).
///
/// # Errors
/// Returns [`AppError::NotFound`] when `subgraph_slug` or `req.draft_id` does not exist, or [`AppError::Database`] on SQL failure.
pub async fn certify_subgraph_runtime_impl(
    store: &PgStore,
    tenant_id: &str,
    subgraph_slug: &str,
    actor_user_id: Option<Uuid>,
    req: CertifySubgraphRequest,
) -> Result<RuntimeCertReport, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let head_ver = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

    let (doc, schema_version, draft_id) = if let Some(did) = req.draft_id {
        let drow = sqlx::query(
            "SELECT schema_doc FROM xb_schema_drafts
              WHERE tenant_id = $1 AND id = $2 AND subgraph_slug = $3",
        )
        .bind(tenant_id)
        .bind(did)
        .bind(subgraph_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("load draft for certify: {e}")))?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
        let ddoc: SchemaDoc = serde_json::from_value(drow.get("schema_doc"))
            .map_err(|e| AppError::Database(format!("decode draft schema_doc: {e}")))?;
        (ddoc, None, Some(did))
    } else {
        let sdoc = load_schema_doc_tx(&mut tx, tenant_id, subgraph_slug).await?;
        (sdoc, Some(head_ver), None)
    };

    let val_ctx = load_validation_context_tx(&mut tx, tenant_id, subgraph_slug).await?;
    let mut summary = validate_with_metrics(&doc, &val_ctx);
    let ext_consumed = load_external_metric_consumers_tx(&mut tx, tenant_id, &doc).await?;
    let dead_knowledge = scan_dead_knowledge_with_external(&doc, &ext_consumed);
    let dk_pairs: BTreeSet<(&str, &str)> = dead_knowledge
        .iter()
        .map(|d| (d.owner_type.as_str(), d.metric.as_str()))
        .collect();
    if dk_pairs.is_empty() {
        summary.issues.retain(|i| i.code != "DEAD_KNOWLEDGE");
    }

    let high_fanout_cnt = count_high_fanout_edges(&doc);
    let has_errors = summary
        .issues
        .iter()
        .any(|i| i.severity == IssueSeverity::Error);
    let has_proposals = doc.has_unconfirmed_proposals();

    let (trial_tick_passed, triggered_rules) =
        if !has_errors && !has_proposals && !doc.node_types.is_empty() {
            execute_trial_tick_savepoint_tx(&mut tx, tenant_id, subgraph_slug, &doc).await?
        } else {
            (false, 0_usize)
        };

    let (cert_level, readiness_score) = evaluate_cert_tier_and_score(
        &summary.issues,
        has_proposals,
        !doc.node_types.is_empty(),
        dead_knowledge.len(),
        high_fanout_cnt,
        trial_tick_passed,
    );

    persist_cert_report_and_commit_tx(
        tx,
        tenant_id,
        subgraph_slug,
        actor_user_id,
        (schema_version, draft_id),
        (
            cert_level,
            readiness_score,
            high_fanout_cnt,
            trial_tick_passed,
            triggered_rules,
        ),
        (dead_knowledge, summary.issues),
    )
    .await
}

async fn load_external_metric_consumers_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    doc: &SchemaDoc,
) -> Result<BTreeSet<(String, String)>, AppError> {
    let mut out = BTreeSet::new();
    let rule_rows = sqlx::query(
        "SELECT target->>'type_name' AS type_name, target->>'metric' AS metric
           FROM ax_rules WHERE tenant_id = $1 AND scope_kind = 'METRIC'",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| AppError::Database(format!("query ax_rules metric consumers: {e}")))?;

    for r in rule_rows {
        let m_opt: Option<String> = r.get("metric");
        let t_opt: Option<String> = r.get("type_name");
        if let Some(m) = m_opt.filter(|s| !s.trim().is_empty()) {
            if let Some(t) = t_opt.filter(|s| !s.trim().is_empty()) {
                out.insert((t, m));
            } else {
                for nt in &doc.node_types {
                    out.insert((nt.name.clone(), m.clone()));
                }
            }
        }
    }
    Ok(out)
}

async fn execute_trial_tick_savepoint_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    doc: &SchemaDoc,
) -> Result<(bool, usize), AppError> {
    sqlx::query("SAVEPOINT trial_tick")
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("create SAVEPOINT trial_tick: {e}")))?;

    let tick_res = run_trial_tick_inner_tx(tx, tenant_id, subgraph_slug, doc).await;

    sqlx::query("ROLLBACK TO SAVEPOINT trial_tick")
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("rollback SAVEPOINT trial_tick: {e}")))?;
    sqlx::query("RELEASE SAVEPOINT trial_tick")
        .execute(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("release SAVEPOINT trial_tick: {e}")))?;

    Ok(tick_res.unwrap_or((false, 0)))
}

async fn run_trial_tick_inner_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    doc: &SchemaDoc,
) -> Result<(bool, usize), AppError> {
    let mut synth_nodes: BTreeMap<String, Uuid> = BTreeMap::new();
    for nt in &doc.node_types {
        let (props, metrics) = build_synthetic_props_and_metrics(&nt.properties, 10.0);
        let nid: Uuid = sqlx::query_scalar(
            "INSERT INTO xb_nodes (tenant_id, subgraph_slug, type_name, node_key, title, properties, metrics)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(&nt.name)
        .bind(format!("__trial_{}_{}", nt.name, Uuid::new_v4().simple()))
        .bind(format!("Trial {}", nt.display_name))
        .bind(serde_json::Value::Object(props))
        .bind(serde_json::to_value(&metrics).unwrap_or_default())
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("insert trial_tick node: {e}")))?;
        synth_nodes.insert(nt.name.clone(), nid);
    }

    for et in &doc.edge_types {
        if let (Some(&src_id), Some(&dst_id)) = (
            synth_nodes.get(&et.src_type_name),
            synth_nodes.get(&et.dst_type_name),
        ) {
            let (props, metrics) = build_synthetic_props_and_metrics(&et.properties, 5.0);
            sqlx::query(
                "INSERT INTO xb_links (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id, weight, properties, metrics)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(tenant_id)
            .bind(subgraph_slug)
            .bind(&et.name)
            .bind(src_id)
            .bind(dst_id)
            .bind(et.impact_weight.max(0.1))
            .bind(serde_json::Value::Object(props))
            .bind(serde_json::to_value(&metrics).unwrap_or_default())
            .execute(&mut **tx)
            .await
            .map_err(|e| AppError::Database(format!("insert trial_tick link: {e}")))?;
        }
    }

    let prop_stats = propagate_subgraph_4path_tx(tx, tenant_id, subgraph_slug, doc).await?;
    let action_checked = dry_check_trial_actions_tx(tx, tenant_id, doc, &synth_nodes).await?;
    Ok((
        true,
        prop_stats.triggered_rules.saturating_add(action_checked),
    ))
}

fn build_synthetic_props_and_metrics(
    props_spec: &[PropertySpec],
    default_num: f64,
) -> (
    serde_json::Map<String, serde_json::Value>,
    BTreeMap<String, f64>,
) {
    let mut props = serde_json::Map::new();
    let mut metrics = BTreeMap::new();
    for p in props_spec {
        if p.kind == PropertyKind::Metric {
            metrics.insert(p.name.clone(), default_num);
        } else {
            let sample = match p.data_type {
                DataType::Int64 | DataType::Float64 | DataType::Decimal => serde_json::json!(1),
                DataType::Boolean => serde_json::json!(true),
                _ => p
                    .allowed_values
                    .as_ref()
                    .and_then(|v| v.first())
                    .map_or_else(|| serde_json::json!("trial"), |s| serde_json::json!(s)),
            };
            props.insert(p.name.clone(), sample);
        }
    }
    (props, metrics)
}

async fn dry_check_trial_actions_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    doc: &SchemaDoc,
    synth_nodes: &BTreeMap<String, Uuid>,
) -> Result<usize, AppError> {
    let mut checked = 0_usize;
    for nt in &doc.node_types {
        let Some(&nid) = synth_nodes.get(&nt.name) else {
            continue;
        };
        let r = sqlx::query(
            "SELECT properties, metrics FROM xb_nodes WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(nid)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| AppError::Database(format!("load trial node for action check: {e}")))?;
        let props = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let metrics: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();

        for action in &nt.actions {
            let mut param_env = BTreeMap::new();
            for p in &action.parameters {
                let sample = match p.data_type {
                    DataType::Int64 | DataType::Float64 | DataType::Decimal => {
                        ExprValue::Number(1.0)
                    }
                    DataType::Boolean => ExprValue::Bool(true),
                    _ => ExprValue::Text(String::from("trial")),
                };
                param_env.insert(format!("param.{}", p.name), sample);
            }
            let env = build_self_action_env(&metrics, &props, &param_env);
            if let Some(pre) = action
                .precondition_expr
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let ast = parse_expr(pre).map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
                let rep = eval_expr(&ast, &env);
                if !rep.warnings.is_empty() {
                    return Err(AppError::unprocessable(
                        "TRIAL_TICK_DOMAIN_ERROR",
                        format!("precondition '{}' produced domain warnings", action.name),
                    ));
                }
                checked = checked.saturating_add(1);
            }
            for mstep in &action.mutations {
                let ast = parse_expr(&mstep.expression)
                    .map_err(|e| AppError::bad("EXPR_INVALID", e.message))?;
                let mut m_env = env.clone();
                add_runtime_scope(&mut m_env, "target", &metrics, &props, false);
                add_runtime_scope(&mut m_env, "edge", &metrics, &props, false);
                let rep = eval_expr(&ast, &m_env);
                if !rep.warnings.is_empty() {
                    return Err(AppError::unprocessable(
                        "TRIAL_TICK_DOMAIN_ERROR",
                        format!("mutation '{}' produced domain warnings", mstep.target),
                    ));
                }
                checked = checked.saturating_add(1);
            }
        }
    }
    Ok(checked)
}

async fn persist_cert_report_and_commit_tx(
    mut tx: Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    actor_user_id: Option<Uuid>,
    target_ref: (Option<i32>, Option<Uuid>),
    scores: (
        axpoint_domain::enums::RuntimeCertLevel,
        i32,
        usize,
        bool,
        usize,
    ),
    diagnostics: (
        Vec<axpoint_domain::studio::DeadKnowledgeIssue>,
        Vec<axpoint_domain::studio::ValidationIssue>,
    ),
) -> Result<RuntimeCertReport, AppError> {
    let (schema_version, draft_id) = target_ref;
    let (cert_level, readiness_score, high_fanout_cnt, trial_tick_passed, triggered_rules) = scores;
    let (dead_knowledge, issues) = diagnostics;
    let dk_cnt = i32::try_from(dead_knowledge.len()).unwrap_or(i32::MAX);
    let hf_cnt = i32::try_from(high_fanout_cnt).unwrap_or(i32::MAX);
    let tr_cnt = i32::try_from(triggered_rules).unwrap_or(i32::MAX);

    let report_json = serde_json::json!({
        "dead_knowledge": &dead_knowledge,
        "issues": &issues,
    });
    let row = sqlx::query(
        "INSERT INTO xb_runtime_certifications (
             tenant_id, subgraph_slug, schema_version, draft_id, cert_level,
             readiness_score, dead_knowledge_cnt, high_fanout_cnt, trial_tick_passed,
             triggered_rules_cnt, report_json, certified_by
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         RETURNING id, created_at",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(schema_version)
    .bind(draft_id)
    .bind(cert_level.as_str())
    .bind(readiness_score)
    .bind(dk_cnt)
    .bind(hf_cnt)
    .bind(trial_tick_passed)
    .bind(tr_cnt)
    .bind(report_json)
    .bind(actor_user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("insert xb_runtime_certifications: {e}")))?;

    let id: Uuid = row.get("id");
    let created_at = row.get("created_at");
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit certify_subgraph_runtime tx: {e}")))?;

    Ok(RuntimeCertReport {
        id,
        subgraph_slug: String::from(subgraph_slug),
        schema_version,
        draft_id,
        cert_level,
        readiness_score,
        dead_knowledge_cnt: dk_cnt,
        high_fanout_cnt: hf_cnt,
        trial_tick_passed,
        triggered_rules_cnt: tr_cnt,
        dead_knowledge,
        issues,
        created_at,
    })
}
