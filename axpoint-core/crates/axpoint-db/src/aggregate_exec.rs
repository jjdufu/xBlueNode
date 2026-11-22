//! Derived metric and dynamic neighbor aggregation executor & explainability (`06-STEP05-06` §06.3–§06.4).

use std::collections::{BTreeMap, BTreeSet};
use std::num::FpCategory;

use axpoint_domain::AppError;
use axpoint_domain::enums::{AggFn, JobKind, OutboxTopic};
use axpoint_domain::expr::{
    BinaryOp, EvalEnv, EvalWarningCode, Expr, ExprValue, UnaryOp, collect_references,
    eval as eval_expr, parse as parse_expr,
};
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::rollup::{
    DirtyInstance, ExplainInputItem, GraphAggregateJobPayload, MetricDependencyPlan,
    MetricExplainResponse, MetricProducer, MetricRuleStep, MetricVertex,
};
use axpoint_domain::studio::{
    AggRuleSpec, DerivedRuleSpec, LinkDirection, NodeTypeSpec, PropertyKind, SchemaDoc,
};
use axpoint_domain::topology::BlueNode;
use chrono::{DateTime, Timelike, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::nodes::decode_blue_node;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

const RELATIVE_TOLERANCE: f64 = 1e-12;
const BATCH_LIMIT: usize = 5_000;
const MAX_EXPLAIN_INPUTS: i64 = 200;

struct RollupExecStats {
    changed_nodes: BTreeSet<Uuid>,
    changed_metrics: BTreeSet<String>,
    points_written: usize,
}

struct ChildMetricSample {
    node_id: Uuid,
    node_key: String,
    type_name: String,
    title: String,
    properties: serde_json::Map<String, serde_json::Value>,
    metrics: BTreeMap<String, f64>,
    link_weight: f64,
    updated_at: DateTime<Utc>,
}

struct TargetResolveCtx<'a> {
    is_full_all: bool,
    full_type_opt: Option<&'a str>,
    direct_seed_by_type: &'a BTreeMap<String, BTreeSet<Uuid>>,
    dirty_instances: &'a BTreeSet<DirtyInstance>,
}

struct MetricUpdateCtx<'a> {
    tenant_id: &'a str,
    batch_id: Uuid,
    target_v: &'a MetricVertex,
    source_tag: &'a str,
}

/// Enqueues a `JobKind::GraphAggregate` job, coalescing into an existing `PENDING` job when matching (`06-STEP05-06` §06.3.3).
///
/// # Errors
/// Returns [`AppError::Database`] if enqueueing into `ax_jobs` fails.
pub async fn enqueue_graph_aggregate_impl(
    store: &PgStore,
    tenant_id: &str,
    payload: GraphAggregateJobPayload,
    idempotency_key: Option<String>,
) -> Result<Uuid, AppError> {
    let base_key = idempotency_key
        .map(|k| String::from(k.trim()))
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| format!("agg:{}", payload.canonical_hash()));

    let payload_json = serde_json::to_value(&payload)
        .map_err(|err| AppError::Database(format!("serialize GraphAggregateJobPayload: {err}")))?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let pending_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM ax_jobs
          WHERE tenant_id = $1
            AND kind = 'GRAPH_AGGREGATE'
            AND status = 'PENDING'
            AND (idempotency_key = $2 OR idempotency_key LIKE ($2 || ':%'))
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(tenant_id)
    .bind(&base_key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check pending graph_aggregate job: {err}")))?;

    if let Some(existing_id) = pending_id {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit coalesced graph_aggregate job: {err}"))
        })?;
        return Ok(existing_id);
    }

    let key_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM ax_jobs
              WHERE tenant_id = $1 AND kind = 'GRAPH_AGGREGATE' AND idempotency_key = $2
         )",
    )
    .bind(tenant_id)
    .bind(&base_key)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check historical graph_aggregate key: {err}")))?;

    let effective_key = if key_taken {
        format!("{base_key}:{}", Uuid::new_v4())
    } else {
        base_key
    };

    let job =
        NewJob::new(JobKind::GraphAggregate, payload_json).with_idempotency_key(effective_key);
    let job_id = enqueue_job_tx(&mut tx, tenant_id, &job).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit enqueue graph_aggregate job: {err}")))?;
    Ok(job_id)
}

/// Executes `GRAPH_AGGREGATE` under session-level `pg_advisory_lock(hashtext('xb_agg:' || tenant))` (`06-STEP05-06` §06.3.2).
///
/// # Errors
/// Returns [`AppError::Database`] if acquiring the advisory lock or executing rollup SQL fails.
pub async fn execute_graph_aggregate_impl(
    store: &PgStore,
    tenant_id: &str,
    payload: &GraphAggregateJobPayload,
) -> Result<serde_json::Value, AppError> {
    let mut lock_conn =
        store.pool().acquire().await.map_err(|err| {
            AppError::Database(format!("acquire advisory lock connection: {err}"))
        })?;

    let lock_name = format!("xb_agg:{tenant_id}");
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(&lock_name)
        .execute(&mut *lock_conn)
        .await
        .map_err(|err| AppError::Database(format!("acquire pg_advisory_lock failed: {err}")))?;

    let exec_result = run_graph_aggregate_locked(store, tenant_id, payload).await;

    if let Err(unlock_err) = sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(&lock_name)
        .execute(&mut *lock_conn)
        .await
    {
        tracing::warn!(
            tenant_id = tenant_id,
            lock_name = %lock_name,
            error = %unlock_err,
            "failed to release pg_advisory_lock cleanly"
        );
    }

    exec_result
}

async fn run_graph_aggregate_locked(
    store: &PgStore,
    tenant_id: &str,
    payload: &GraphAggregateJobPayload,
) -> Result<serde_json::Value, AppError> {
    let doc = load_main_schema_doc(store, tenant_id).await?;
    let plan = MetricDependencyPlan::from_schema(&doc);
    let layers = plan.layers();
    if layers.is_empty() {
        return Ok(serde_json::json!({ "nodes_changed": 0, "points_written": 0 }));
    }

    let full_type_opt = payload
        .full_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let is_full_all = payload.full && full_type_opt.is_none();

    let (mut dirty_instances, direct_seed_by_type) =
        initialize_dirty_seeds(store, tenant_id, &doc, payload, is_full_all, full_type_opt).await?;

    let mut stats = RollupExecStats {
        changed_nodes: BTreeSet::new(),
        changed_metrics: BTreeSet::new(),
        points_written: 0,
    };
    let batch_id = Uuid::new_v4();

    for layer in &layers {
        for step in layer {
            let resolve_ctx = TargetResolveCtx {
                is_full_all,
                full_type_opt,
                direct_seed_by_type: &direct_seed_by_type,
                dirty_instances: &dirty_instances,
            };
            let target_ids = resolve_step_targets(store, tenant_id, step, &resolve_ctx).await?;
            if target_ids.is_empty() {
                continue;
            }
            execute_rule_step_batches(
                store,
                tenant_id,
                batch_id,
                step,
                &target_ids,
                &mut dirty_instances,
                &mut stats,
            )
            .await?;
        }
    }

    if !stats.changed_nodes.is_empty() {
        emit_aggregate_outbox_and_rollup(store, tenant_id, &stats).await?;
    }

    Ok(serde_json::json!({
        "nodes_changed": stats.changed_nodes.len(),
        "points_written": stats.points_written,
    }))
}

async fn initialize_dirty_seeds(
    store: &PgStore,
    tenant_id: &str,
    doc: &SchemaDoc,
    payload: &GraphAggregateJobPayload,
    is_full_all: bool,
    full_type_opt: Option<&str>,
) -> Result<(BTreeSet<DirtyInstance>, BTreeMap<String, BTreeSet<Uuid>>), AppError> {
    if is_full_all {
        return Ok((BTreeSet::new(), BTreeMap::new()));
    }

    let declared_metrics: BTreeMap<&str, Vec<String>> = doc
        .node_types
        .iter()
        .map(|nt| {
            let ms = nt
                .properties
                .iter()
                .filter(|p| p.kind == PropertyKind::Metric)
                .map(|p| p.name.clone())
                .collect();
            (nt.name.as_str(), ms)
        })
        .collect();

    let mut dirty = BTreeSet::new();
    let mut direct_by_type: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    if let Some(ft) = full_type_opt {
        let rows = sqlx::query(
            "SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = $2",
        )
            .bind(tenant_id)
            .bind(ft)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("load full_type seed nodes: {err}")))?;
        let ms = declared_metrics.get(ft).cloned().unwrap_or_default();
        for r in rows {
            let nid: Uuid = r.get("id");
            direct_by_type
                .entry(String::from(ft))
                .or_default()
                .insert(nid);
            for m in &ms {
                dirty.insert(DirtyInstance {
                    node_id: nid,
                    type_name: String::from(ft),
                    metric: m.clone(),
                });
            }
        }
    }

    if !payload.node_ids.is_empty() {
        let rows = sqlx::query(
            "SELECT id, type_name, metrics FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = 'main' AND id = ANY($2)",
        )
        .bind(tenant_id)
        .bind(&payload.node_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load payload seed nodes: {err}")))?;

        for r in rows {
            let nid: Uuid = r.get("id");
            let tname: String = r.get("type_name");
            if payload.metrics.is_empty() {
                direct_by_type.entry(tname.clone()).or_default().insert(nid);
                let mut all_m: BTreeSet<String> = declared_metrics
                    .get(tname.as_str())
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let cur_m: BTreeMap<String, f64> =
                    serde_json::from_value(r.get("metrics")).unwrap_or_default();
                all_m.extend(cur_m.into_keys());
                for m in all_m {
                    dirty.insert(DirtyInstance {
                        node_id: nid,
                        type_name: tname.clone(),
                        metric: m,
                    });
                }
            } else {
                for m in &payload.metrics {
                    dirty.insert(DirtyInstance {
                        node_id: nid,
                        type_name: tname.clone(),
                        metric: m.clone(),
                    });
                }
            }
        }
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit initialize_dirty_seeds: {err}")))?;
    Ok((dirty, direct_by_type))
}

async fn resolve_step_targets(
    store: &PgStore,
    tenant_id: &str,
    step: &MetricRuleStep,
    ctx: &TargetResolveCtx<'_>,
) -> Result<BTreeSet<Uuid>, AppError> {
    let target_v = step.target_vertex();
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    if ctx.is_full_all || ctx.full_type_opt == Some(target_v.type_name.as_str()) {
        let rows = sqlx::query(
            "SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = $2",
        )
            .bind(tenant_id)
            .bind(&target_v.type_name)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("load full target nodes: {err}")))?;
        let out: BTreeSet<Uuid> = rows.into_iter().map(|r| r.get("id")).collect();
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit full target lookup: {err}")))?;
        return Ok(out);
    }

    let mut targets: BTreeSet<Uuid> = ctx
        .direct_seed_by_type
        .get(&target_v.type_name)
        .cloned()
        .unwrap_or_default();

    match step {
        MetricRuleStep::Derived { type_name, .. } => {
            for d in ctx.dirty_instances {
                let sv = MetricVertex::new(d.type_name.clone(), d.metric.clone());
                if d.type_name == *type_name && step.depends_on(&sv) {
                    targets.insert(d.node_id);
                }
            }
        }
        MetricRuleStep::Aggregation { parent_type, rule } => {
            let dirty_children: Vec<Uuid> = ctx
                .dirty_instances
                .iter()
                .filter(|d| {
                    let sv = MetricVertex::new(d.type_name.clone(), d.metric.clone());
                    step.depends_on(&sv)
                })
                .map(|d| d.node_id)
                .collect();

            if !dirty_children.is_empty() {
                let sql = match rule.direction {
                    LinkDirection::Out => {
                        "SELECT DISTINCT p.id
                           FROM xb_links l
                           JOIN xb_nodes p ON p.tenant_id = l.tenant_id AND p.id = l.src_node_id
                          WHERE l.tenant_id = $1
                            AND l.subgraph_slug = 'main'
                            AND p.subgraph_slug = 'main'
                            AND l.link_type = $2
                            AND l.dst_node_id = ANY($3)
                            AND p.type_name = $4"
                    }
                    LinkDirection::In => {
                        "SELECT DISTINCT p.id
                           FROM xb_links l
                           JOIN xb_nodes p ON p.tenant_id = l.tenant_id AND p.id = l.dst_node_id
                          WHERE l.tenant_id = $1
                            AND l.subgraph_slug = 'main'
                            AND p.subgraph_slug = 'main'
                            AND l.link_type = $2
                            AND l.src_node_id = ANY($3)
                            AND p.type_name = $4"
                    }
                };
                let rows = sqlx::query(sql)
                    .bind(tenant_id)
                    .bind(&rule.via_link_type)
                    .bind(&dirty_children)
                    .bind(parent_type)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(|err| {
                        AppError::Database(format!("lookup dirty parent nodes: {err}"))
                    })?;
                for r in rows {
                    targets.insert(r.get("id"));
                }
            }
        }
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit resolve_step_targets: {err}")))?;
    Ok(targets)
}

async fn execute_rule_step_batches(
    store: &PgStore,
    tenant_id: &str,
    batch_id: Uuid,
    step: &MetricRuleStep,
    target_ids: &BTreeSet<Uuid>,
    dirty_instances: &mut BTreeSet<DirtyInstance>,
    stats: &mut RollupExecStats,
) -> Result<(), AppError> {
    let all_ids: Vec<Uuid> = target_ids.iter().copied().collect();
    let target_v = step.target_vertex();
    let source_tag = match step {
        MetricRuleStep::Derived { .. } => "DERIVED",
        MetricRuleStep::Aggregation { .. } => "AGGREGATION",
    };

    for chunk in all_ids.chunks(BATCH_LIMIT) {
        let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
        let computed = match step {
            MetricRuleStep::Derived { expression, .. } => {
                compute_derived_batch_tx(&mut tx, tenant_id, chunk, expression).await?
            }
            MetricRuleStep::Aggregation { rule, .. } => {
                compute_aggregation_batch_tx(&mut tx, tenant_id, chunk, rule).await?
            }
        };

        let upd_ctx = MetricUpdateCtx {
            tenant_id,
            batch_id,
            target_v: &target_v,
            source_tag,
        };
        apply_computed_metric_updates_tx(&mut tx, &upd_ctx, &computed, dirty_instances, stats)
            .await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit rule step batch failed: {err}")))?;
    }
    Ok(())
}

async fn compute_derived_batch_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_ids: &[Uuid],
    expression: &str,
) -> Result<BTreeMap<Uuid, Option<f64>>, AppError> {
    let ast =
        parse_expr(expression).map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;

    let rows = sqlx::query(
        "SELECT id, properties, metrics
           FROM xb_nodes
          WHERE tenant_id = $1 AND id = ANY($2)",
    )
    .bind(tenant_id)
    .bind(node_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load nodes for derived batch: {err}")))?;

    let mut out = BTreeMap::new();
    for r in rows {
        let nid: Uuid = r.get("id");
        let props_map = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let metrics_map: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();
        let env = EvalEnv::from_node(&metrics_map, &props_map);
        let rep = eval_expr(&ast, &env);
        let val = match rep.value {
            ExprValue::Number(v) if v.is_finite() => Some(normalize_zero(v)),
            _ => None,
        };
        out.insert(nid, val);
    }
    Ok(out)
}

async fn compute_aggregation_batch_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    parent_ids: &[Uuid],
    rule: &AggRuleSpec,
) -> Result<BTreeMap<Uuid, Option<f64>>, AppError> {
    let filter_ast = match rule.filter.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() => {
            Some(parse_expr(f).map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?)
        }
        _ => None,
    };

    if rule.op != AggFn::Last {
        let pg_filter = match &filter_ast {
            None => Some(String::from("TRUE")),
            Some(ast) => try_translate_pg_child_filter(ast),
        };
        if let Some(where_pred) = pg_filter {
            return compute_aggregation_sql_tx(tx, tenant_id, parent_ids, rule, &where_pred).await;
        }
    }

    compute_aggregation_fallback_tx(tx, tenant_id, parent_ids, rule, filter_ast.as_ref()).await
}

async fn compute_aggregation_sql_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    parent_ids: &[Uuid],
    rule: &AggRuleSpec,
    where_pred: &str,
) -> Result<BTreeMap<Uuid, Option<f64>>, AppError> {
    let mv = "(c.metrics ->> $5)::double precision";
    let lw = "COALESCE(l.weight, 1.0)";
    let agg_expr = match (rule.op, rule.weight_by_link) {
        (AggFn::Sum, false) => format!(
            "CASE WHEN COUNT(c.id) = 0 THEN 0.0::double precision ELSE COALESCE(SUM({mv}), 0.0::double precision) END"
        ),
        (AggFn::Sum, true) => format!(
            "CASE WHEN COUNT(c.id) = 0 THEN 0.0::double precision ELSE COALESCE(SUM(({mv}) * ({lw})), 0.0::double precision) END"
        ),
        (AggFn::Count, _) => String::from("COUNT(c.id)::double precision"),
        (AggFn::Avg, false) => format!("AVG({mv})"),
        (AggFn::Avg, true) => format!(
            "CASE WHEN COALESCE(SUM(CASE WHEN {mv} IS NOT NULL THEN {lw} ELSE 0.0 END), 0.0) > 0.0 \
             THEN SUM(({mv}) * ({lw})) / SUM(CASE WHEN {mv} IS NOT NULL THEN {lw} ELSE 0.0 END) \
             ELSE NULL END"
        ),
        (AggFn::Min, _) => format!("MIN({mv})"),
        (AggFn::Max, _) => format!("MAX({mv})"),
        (AggFn::CountDistinct, _) => format!(
            "CASE WHEN COUNT({mv}) = 0 THEN NULL ELSE COUNT(DISTINCT {mv})::double precision END"
        ),
        (AggFn::Median, _) => format!("percentile_cont(0.5) WITHIN GROUP (ORDER BY {mv})"),
        (AggFn::Last, _) => String::from("NULL::double precision"),
    };

    let join_cond = match rule.direction {
        LinkDirection::Out => "l.src_node_id = p.parent_id AND c.id = l.dst_node_id",
        LinkDirection::In => "l.dst_node_id = p.parent_id AND c.id = l.src_node_id",
    };

    let sql = format!(
        "SELECT p.parent_id, ({agg_expr}) AS agg_val
           FROM UNNEST($2::uuid[]) AS p(parent_id)
           LEFT JOIN (
               xb_links l
               JOIN xb_nodes c
                 ON c.tenant_id = l.tenant_id
                AND c.subgraph_slug = l.subgraph_slug
                AND c.type_name = $4
                AND ({where_pred})
           ) ON l.tenant_id = $1
            AND l.subgraph_slug = 'main'
            AND l.link_type = $3
            AND {join_cond}
          GROUP BY p.parent_id"
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(tenant_id)
        .bind(parent_ids)
        .bind(&rule.via_link_type)
        .bind(&rule.source_type_name)
        .bind(&rule.source_metric)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("execute SQL neighbor aggregation: {err}")))?;

    let mut out = BTreeMap::new();
    for r in rows {
        let pid: Uuid = r.get("parent_id");
        let val: Option<f64> = r.get("agg_val");
        out.insert(pid, val.filter(|v| v.is_finite()).map(normalize_zero));
    }
    Ok(out)
}

async fn compute_aggregation_fallback_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    parent_ids: &[Uuid],
    rule: &AggRuleSpec,
    filter_ast: Option<&Expr>,
) -> Result<BTreeMap<Uuid, Option<f64>>, AppError> {
    let grouped = fetch_neighbor_samples_tx(tx, tenant_id, parent_ids, rule, None).await?;
    let mut out = BTreeMap::new();
    for &pid in parent_ids {
        let samples = grouped.get(&pid).map_or(&[][..], Vec::as_slice);
        let filtered: Vec<&ChildMetricSample> = samples
            .iter()
            .filter(|s| {
                filter_ast.is_none_or(|ast| {
                    let env = EvalEnv::from_node(&s.metrics, &s.properties);
                    matches!(eval_expr(ast, &env).value, ExprValue::Bool(true))
                })
            })
            .collect();
        out.insert(pid, eval_agg_samples(&filtered, rule));
    }
    Ok(out)
}

fn eval_agg_samples(samples: &[&ChildMetricSample], rule: &AggRuleSpec) -> Option<f64> {
    if samples.is_empty() {
        return match rule.op {
            AggFn::Sum | AggFn::Count => Some(0.0),
            _ => None,
        };
    }
    if rule.op == AggFn::Count {
        let cnt = u32::try_from(samples.len()).map_or_else(
            |_| samples.len().to_string().parse::<f64>().unwrap_or(0.0),
            f64::from,
        );
        return Some(cnt);
    }

    let mut pairs: Vec<(f64, f64, DateTime<Utc>, &str)> = Vec::new();
    for s in samples {
        if let Some(&v) = s.metrics.get(&rule.source_metric)
            && v.is_finite()
        {
            let ts = extract_metric_ts(&s.properties, &rule.source_metric).unwrap_or(s.updated_at);
            pairs.push((v, s.link_weight, ts, s.node_key.as_str()));
        }
    }

    if pairs.is_empty() {
        return match rule.op {
            AggFn::Sum => Some(0.0),
            _ => None,
        };
    }

    let res = match rule.op {
        AggFn::Sum => {
            if rule.weight_by_link {
                Some(pairs.iter().map(|(v, w, _, _)| v * w).sum())
            } else {
                Some(pairs.iter().map(|(v, _, _, _)| *v).sum())
            }
        }
        AggFn::Avg => {
            if rule.weight_by_link {
                let w_sum: f64 = pairs.iter().map(|(_, w, _, _)| *w).sum();
                if w_sum > 0.0 {
                    Some(pairs.iter().map(|(v, w, _, _)| v * w).sum::<f64>() / w_sum)
                } else {
                    None
                }
            } else {
                let cnt = u32::try_from(pairs.len()).map_or_else(
                    |_| pairs.len().to_string().parse::<f64>().unwrap_or(1.0),
                    f64::from,
                );
                Some(pairs.iter().map(|(v, _, _, _)| *v).sum::<f64>() / cnt)
            }
        }
        AggFn::Min => pairs.iter().map(|(v, _, _, _)| *v).reduce(f64::min),
        AggFn::Max => pairs.iter().map(|(v, _, _, _)| *v).reduce(f64::max),
        AggFn::Median => {
            let mut vals: Vec<f64> = pairs.iter().map(|(v, _, _, _)| *v).collect();
            vals.sort_by(f64::total_cmp);
            let mid = vals.len() / 2;
            if vals.len() % 2 == 1 {
                vals.get(mid).copied()
            } else {
                let lo = vals.get(mid.saturating_sub(1)).copied()?;
                let hi = vals.get(mid).copied()?;
                Some(f64::midpoint(lo, hi))
            }
        }
        AggFn::CountDistinct => {
            let distinct: BTreeSet<u64> = pairs
                .iter()
                .map(|(v, _, _, _)| normalize_zero(*v).to_bits())
                .collect();
            u32::try_from(distinct.len()).map_or_else(
                |_| distinct.len().to_string().parse::<f64>().ok(),
                |n| Some(f64::from(n)),
            )
        }
        AggFn::Last => pairs
            .iter()
            .max_by(|a, b| a.2.cmp(&b.2).then_with(|| a.3.cmp(b.3)))
            .map(|(v, _, _, _)| *v),
        AggFn::Count => None,
    };

    res.filter(|v| v.is_finite()).map(normalize_zero)
}

async fn fetch_neighbor_samples_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    parent_ids: &[Uuid],
    rule: &AggRuleSpec,
    row_limit: Option<i64>,
) -> Result<BTreeMap<Uuid, Vec<ChildMetricSample>>, AppError> {
    let join_sql = match rule.direction {
        LinkDirection::Out => {
            "SELECT l.src_node_id AS parent_id, c.id, c.node_key, c.type_name, c.title,
                    c.properties, c.metrics, COALESCE(l.weight, 1.0) AS link_weight, c.updated_at
               FROM xb_links l
               JOIN xb_nodes c ON c.tenant_id = l.tenant_id AND c.id = l.dst_node_id
              WHERE l.tenant_id = $1
                AND l.link_type = $2
                AND l.src_node_id = ANY($3)
                AND c.type_name = $4
              ORDER BY c.node_key ASC
              LIMIT $5"
        }
        LinkDirection::In => {
            "SELECT l.dst_node_id AS parent_id, c.id, c.node_key, c.type_name, c.title,
                    c.properties, c.metrics, COALESCE(l.weight, 1.0) AS link_weight, c.updated_at
               FROM xb_links l
               JOIN xb_nodes c ON c.tenant_id = l.tenant_id AND c.id = l.src_node_id
              WHERE l.tenant_id = $1
                AND l.link_type = $2
                AND l.dst_node_id = ANY($3)
                AND c.type_name = $4
              ORDER BY c.node_key ASC
              LIMIT $5"
        }
    };

    let limit_val = row_limit.unwrap_or(1_000_000_000_i64);
    let rows = sqlx::query(join_sql)
        .bind(tenant_id)
        .bind(&rule.via_link_type)
        .bind(parent_ids)
        .bind(&rule.source_type_name)
        .bind(limit_val)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("fetch neighbor samples failed: {err}")))?;

    let mut out: BTreeMap<Uuid, Vec<ChildMetricSample>> = BTreeMap::new();
    for r in rows {
        let pid: Uuid = r.get("parent_id");
        let props = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let metrics: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();
        out.entry(pid).or_default().push(ChildMetricSample {
            node_id: r.get("id"),
            node_key: r.get("node_key"),
            type_name: r.get("type_name"),
            title: r.get("title"),
            properties: props,
            metrics,
            link_weight: r.get("link_weight"),
            updated_at: r.get("updated_at"),
        });
    }
    Ok(out)
}

async fn apply_computed_metric_updates_tx(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &MetricUpdateCtx<'_>,
    computed: &BTreeMap<Uuid, Option<f64>>,
    dirty_instances: &mut BTreeSet<DirtyInstance>,
    stats: &mut RollupExecStats,
) -> Result<(), AppError> {
    if computed.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = computed.keys().copied().collect();
    let rows = sqlx::query(
        "SELECT id, properties, metrics
           FROM xb_nodes
          WHERE tenant_id = $1 AND id = ANY($2)
          FOR UPDATE",
    )
    .bind(ctx.tenant_id)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock xb_nodes for computed update: {err}")))?;

    let now = Utc::now();
    let now_rfc = now.to_rfc3339();
    let m_name = ctx.target_v.metric.as_str();

    let mut upd_ids = Vec::new();
    let mut upd_props = Vec::new();
    let mut upd_metrics = Vec::new();
    let mut series_ids = Vec::new();
    let mut series_vals = Vec::new();

    for r in rows {
        let nid: Uuid = r.get("id");
        let Some(&new_val) = computed.get(&nid) else {
            continue;
        };
        let mut metrics_map: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();
        let old_val = metrics_map.get(m_name).copied();

        if values_effectively_equal(old_val, new_val) {
            continue;
        }

        let mut props_map = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let mut ts_obj = match props_map.remove("__metric_ts") {
            Some(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };

        if let Some(v) = new_val {
            metrics_map.insert(String::from(m_name), v);
            ts_obj.insert(
                String::from(m_name),
                serde_json::Value::String(now_rfc.clone()),
            );
            series_ids.push(nid);
            series_vals.push(v);
        } else {
            let _ = metrics_map.remove(m_name);
            let _ = ts_obj.remove(m_name);
        }

        if !ts_obj.is_empty() {
            props_map.insert(
                String::from("__metric_ts"),
                serde_json::Value::Object(ts_obj),
            );
        }

        upd_ids.push(nid);
        upd_props.push(serde_json::Value::Object(props_map));
        upd_metrics.push(
            serde_json::to_value(&metrics_map)
                .map_err(|err| AppError::Database(format!("serialize updated metrics: {err}")))?,
        );

        dirty_instances.insert(DirtyInstance {
            node_id: nid,
            type_name: ctx.target_v.type_name.clone(),
            metric: ctx.target_v.metric.clone(),
        });
        stats.changed_nodes.insert(nid);
        stats.changed_metrics.insert(ctx.target_v.metric.clone());
    }

    if !upd_ids.is_empty() {
        persist_node_and_series_batch_tx(
            tx,
            ctx,
            now,
            (&upd_ids, &upd_props, &upd_metrics),
            (&series_ids, &series_vals),
            stats,
        )
        .await?;
    }
    Ok(())
}

async fn persist_node_and_series_batch_tx(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &MetricUpdateCtx<'_>,
    now: DateTime<Utc>,
    node_batch: (&[Uuid], &[serde_json::Value], &[serde_json::Value]),
    series_batch: (&[Uuid], &[f64]),
    stats: &mut RollupExecStats,
) -> Result<(), AppError> {
    let (upd_ids, upd_props, upd_metrics) = node_batch;
    sqlx::query(
        "UPDATE xb_nodes AS n
            SET properties = u.properties,
                metrics = u.metrics,
                row_version = n.row_version + 1,
                updated_at = NOW()
           FROM UNNEST($2::uuid[], $3::jsonb[], $4::jsonb[])
                AS u(id, properties, metrics)
          WHERE n.tenant_id = $1 AND n.id = u.id",
    )
    .bind(ctx.tenant_id)
    .bind(upd_ids)
    .bind(upd_props)
    .bind(upd_metrics)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("batch update xb_nodes computed metrics: {err}")))?;

    let (series_ids, series_vals) = series_batch;
    if !series_ids.is_empty() && is_valid_series_metric_name(&ctx.target_v.metric) {
        sqlx::query(
            "INSERT INTO xb_metric_series (
                 tenant_id, node_id, metric, ts, value, source_tag, batch_id
             )
             SELECT $1, u.node_id, $2, $3, u.val, $4, $5
               FROM UNNEST($6::uuid[], $7::float8[]) AS u(node_id, val)
             ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
                 value = EXCLUDED.value,
                 batch_id = EXCLUDED.batch_id",
        )
        .bind(ctx.tenant_id)
        .bind(&ctx.target_v.metric)
        .bind(now)
        .bind(ctx.source_tag)
        .bind(ctx.batch_id)
        .bind(series_ids)
        .bind(series_vals)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert computed xb_metric_series: {err}")))?;

        let minute_start = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(now);
        sqlx::query(
            "INSERT INTO xb_rollup_dirty (tenant_id, node_id, metric, minute_start)
             SELECT $1, u.node_id, $2, $3
               FROM UNNEST($4::uuid[]) AS u(node_id)
             ON CONFLICT DO NOTHING",
        )
        .bind(ctx.tenant_id)
        .bind(&ctx.target_v.metric)
        .bind(minute_start)
        .bind(series_ids)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert computed xb_rollup_dirty: {err}")))?;

        stats.points_written = stats.points_written.saturating_add(series_ids.len());
    }

    Ok(())
}

async fn emit_aggregate_outbox_and_rollup(
    store: &PgStore,
    tenant_id: &str,
    stats: &RollupExecStats,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let node_ids: Vec<Uuid> = stats.changed_nodes.iter().copied().collect();
    let metrics: Vec<String> = stats.changed_metrics.iter().cloned().collect();

    let ev = NewOutboxEvent {
        topic: OutboxTopic::NodeMetricsChanged,
        payload: serde_json::json!({
            "node_ids": node_ids,
            "metrics": metrics,
            "origin": "AGGREGATE",
        }),
    };
    let _ = insert_outbox_tx(&mut tx, tenant_id, &ev).await?;

    if stats.points_written > 0 {
        let minute_key = format!("{tenant_id}:{}", Utc::now().format("%Y-%m-%dT%H:%MZ"));
        let rollup_job =
            NewJob::new(JobKind::Rollup, serde_json::json!({})).with_idempotency_key(minute_key);
        let _ = enqueue_job_tx(&mut tx, tenant_id, &rollup_job).await?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit aggregate outbox tx failed: {err}")))?;
    Ok(())
}

/// Explains the provenance, rule, inputs, and warnings of `metric` on `node_id` (`06-STEP05-06` §06.4).
///
/// # Errors
/// Returns [`AppError::NotFound`] if the node does not exist in `tenant_id`, or [`AppError::Database`] on SQL failure.
pub async fn explain_node_metric_impl(
    store: &PgStore,
    tenant_id: &str,
    node_id: Uuid,
    metric: &str,
) -> Result<MetricExplainResponse, AppError> {
    let metric_name = metric.trim();
    if metric_name.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "metric name must not be empty",
        ));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let nrow = sqlx::query(
        "SELECT id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, source_dataset_id, source_row_key, row_version,
                created_at, updated_at
           FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND id = $2",
    )
    .bind(tenant_id)
    .bind(node_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load node for metric explain: {err}")))?
    .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

    let node = decode_blue_node(&nrow);
    let value = node.metrics.get(metric_name).copied();
    let nt_opt = load_single_main_node_type_tx(&mut tx, tenant_id, &node.type_name).await?;

    let latest_series = sqlx::query(
        "SELECT ts, source_tag
           FROM xb_metric_series
          WHERE tenant_id = $1 AND node_id = $2 AND metric = $3
          ORDER BY ts DESC
          LIMIT 1",
    )
    .bind(tenant_id)
    .bind(node_id)
    .bind(metric_name)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| {
        AppError::Database(format!("load latest xb_metric_series for explain: {err}"))
    })?;

    let computed_at = extract_metric_ts(&node.properties, metric_name)
        .or_else(|| latest_series.as_ref().map(|r| r.get("ts")));
    let latest_tag: Option<String> = latest_series.map(|r| r.get("source_tag"));

    let response = if let Some(drule) = nt_opt.as_ref().and_then(|nt| {
        nt.derived_rules
            .iter()
            .find(|r| r.enabled && r.target_metric == metric_name)
    }) {
        build_derived_explain(&node, metric_name, value, computed_at, drule)?
    } else if let Some(arule) = nt_opt
        .as_ref()
        .and_then(|nt| nt.agg_rules.iter().find(|r| r.target_metric == metric_name))
    {
        build_agg_explain_tx(
            &mut tx,
            tenant_id,
            &node,
            metric_name,
            value,
            computed_at,
            arule,
        )
        .await?
    } else {
        build_non_rule_explain_tx(
            &mut tx,
            tenant_id,
            &node,
            metric_name,
            value,
            computed_at,
            latest_tag.as_deref(),
        )
        .await?
    };

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit explain_node_metric tx: {err}")))?;
    Ok(response)
}

fn build_derived_explain(
    node: &BlueNode,
    metric_name: &str,
    value: Option<f64>,
    computed_at: Option<DateTime<Utc>>,
    drule: &DerivedRuleSpec,
) -> Result<MetricExplainResponse, AppError> {
    let rule_json = serde_json::to_value(drule)
        .map_err(|err| AppError::Database(format!("serialize derived rule explain: {err}")))?;

    let mut inputs = Vec::new();
    let mut warnings: Vec<EvalWarningCode> = Vec::new();

    if let Ok(ast) = parse_expr(&drule.expression) {
        let env = EvalEnv::from_node(&node.metrics, &node.properties);
        let rep = eval_expr(&ast, &env);
        warnings = rep.warnings;

        let (ref_metrics, ref_props) = collect_references(&ast);
        for rm in ref_metrics {
            inputs.push(ExplainInputItem {
                node_id: node.id,
                node_key: node.node_key.clone(),
                type_name: node.type_name.clone(),
                title: node.title.clone(),
                metric: rm.clone(),
                value: node.metrics.get(&rm).copied(),
                weight: None,
            });
        }
        for rp in ref_props {
            let num_val = node
                .properties
                .get(&rp)
                .and_then(serde_json::Value::as_f64)
                .filter(|v| v.is_finite());
            inputs.push(ExplainInputItem {
                node_id: node.id,
                node_key: node.node_key.clone(),
                type_name: node.type_name.clone(),
                title: node.title.clone(),
                metric: format!("prop.{rp}"),
                value: num_val,
                weight: None,
            });
        }
    }

    Ok(MetricExplainResponse {
        node_id: node.id,
        metric: String::from(metric_name),
        value,
        producer: MetricProducer::Derived,
        rule: Some(rule_json),
        inputs,
        computed_at,
        warnings,
    })
}

async fn build_agg_explain_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node: &BlueNode,
    metric_name: &str,
    value: Option<f64>,
    computed_at: Option<DateTime<Utc>>,
    arule: &AggRuleSpec,
) -> Result<MetricExplainResponse, AppError> {
    let rule_json = serde_json::to_value(arule)
        .map_err(|err| AppError::Database(format!("serialize agg rule explain: {err}")))?;

    let filter_ast = arule
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|f| parse_expr(f).ok());

    let grouped =
        fetch_neighbor_samples_tx(tx, tenant_id, &[node.id], arule, Some(MAX_EXPLAIN_INPUTS))
            .await?;

    let mut inputs = Vec::new();
    let mut warnings_set = BTreeSet::new();
    if let Some(samples) = grouped.get(&node.id) {
        for s in samples {
            if let Some(ast) = &filter_ast {
                let env = EvalEnv::from_node(&s.metrics, &s.properties);
                let rep = eval_expr(ast, &env);
                warnings_set.extend(rep.warnings);
                if !matches!(rep.value, ExprValue::Bool(true)) {
                    continue;
                }
            }
            inputs.push(ExplainInputItem {
                node_id: s.node_id,
                node_key: s.node_key.clone(),
                type_name: s.type_name.clone(),
                title: s.title.clone(),
                metric: arule.source_metric.clone(),
                value: s.metrics.get(&arule.source_metric).copied(),
                weight: if arule.weight_by_link {
                    Some(s.link_weight)
                } else {
                    None
                },
            });
        }
    }

    Ok(MetricExplainResponse {
        node_id: node.id,
        metric: String::from(metric_name),
        value,
        producer: MetricProducer::Agg,
        rule: Some(rule_json),
        inputs,
        computed_at,
        warnings: warnings_set.into_iter().collect(),
    })
}

async fn build_non_rule_explain_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node: &BlueNode,
    metric_name: &str,
    value: Option<f64>,
    computed_at: Option<DateTime<Utc>>,
    latest_tag: Option<&str>,
) -> Result<MetricExplainResponse, AppError> {
    let mapping_row = if let Some(ds_id) = node.source_dataset_id {
        sqlx::query(
            "SELECT m.id, m.name, d.name AS dataset_name, m.spec
               FROM xb_mappings m
               JOIN ds_datasets d ON d.id = m.dataset_id AND d.tenant_id = m.tenant_id
              WHERE m.tenant_id = $1
                AND m.subgraph_slug = 'main'
                AND m.dataset_id = $2
                AND m.node_type = $3
              ORDER BY m.enabled DESC, m.name ASC
              LIMIT 1",
        )
        .bind(tenant_id)
        .bind(ds_id)
        .bind(&node.type_name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("lookup mapping for explain: {err}")))?
    } else {
        None
    };

    let producer = match latest_tag {
        Some("SYNC") => MetricProducer::Sync,
        Some("ACTION") => MetricProducer::Action,
        Some("ALGORITHM") => MetricProducer::Algorithm,
        Some("AGGREGATION") => MetricProducer::Agg,
        Some("DERIVED") => MetricProducer::Derived,
        _ => {
            if mapping_row.is_some() {
                MetricProducer::Sync
            } else {
                MetricProducer::Manual
            }
        }
    };

    let rule = mapping_row.map(|mr| {
        let m_id: Uuid = mr.get("id");
        let m_name: String = mr.get("name");
        let ds_name: String = mr.get("dataset_name");
        let spec_v: serde_json::Value = mr.get("spec");
        let src_col = spec_v
            .get("metrics")
            .and_then(|m| m.get(metric_name))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        serde_json::json!({
            "mapping_id": m_id,
            "mapping_name": m_name,
            "dataset": ds_name,
            "source_column": src_col,
        })
    });

    Ok(MetricExplainResponse {
        node_id: node.id,
        metric: String::from(metric_name),
        value,
        producer,
        rule,
        inputs: Vec::new(),
        computed_at,
        warnings: Vec::new(),
    })
}

async fn load_main_schema_doc(store: &PgStore, tenant_id: &str) -> Result<SchemaDoc, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let nt_rows = sqlx::query(
        "SELECT id, name, display_name, description, icon, color, title_template,
                properties, agg_rules, derived_rules, sync_to_knowledge, pos_x, pos_y
           FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main'
          ORDER BY name ASC",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load main node_types for aggregate: {err}")))?;

    let node_types = nt_rows
        .into_iter()
        .map(|r| NodeTypeSpec {
            id: Some(r.get("id")),
            name: r.get("name"),
            display_name: r.get("display_name"),
            description: r.get("description"),
            icon: r.get("icon"),
            color: r.get("color"),
            title_template: r.get("title_template"),
            properties: serde_json::from_value(r.get("properties")).unwrap_or_default(),
            agg_rules: serde_json::from_value(r.get("agg_rules")).unwrap_or_default(),
            derived_rules: serde_json::from_value(r.get("derived_rules")).unwrap_or_default(),
            sync_to_knowledge: r.get("sync_to_knowledge"),
            pos_x: r.get("pos_x"),
            pos_y: r.get("pos_y"),
        })
        .collect();

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit load_main_schema_doc: {err}")))?;
    Ok(SchemaDoc {
        node_types,
        edge_types: Vec::new(),
        mappings: Vec::new(),
    })
}

async fn load_single_main_node_type_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
) -> Result<Option<NodeTypeSpec>, AppError> {
    let row_opt = sqlx::query(
        "SELECT id, name, display_name, description, icon, color, title_template,
                properties, agg_rules, derived_rules, sync_to_knowledge, pos_x, pos_y
           FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND name = $2",
    )
    .bind(tenant_id)
    .bind(type_name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load node type for explain: {err}")))?;

    Ok(row_opt.map(|r| NodeTypeSpec {
        id: Some(r.get("id")),
        name: r.get("name"),
        display_name: r.get("display_name"),
        description: r.get("description"),
        icon: r.get("icon"),
        color: r.get("color"),
        title_template: r.get("title_template"),
        properties: serde_json::from_value(r.get("properties")).unwrap_or_default(),
        agg_rules: serde_json::from_value(r.get("agg_rules")).unwrap_or_default(),
        derived_rules: serde_json::from_value(r.get("derived_rules")).unwrap_or_default(),
        sync_to_knowledge: r.get("sync_to_knowledge"),
        pos_x: r.get("pos_x"),
        pos_y: r.get("pos_y"),
    }))
}

fn try_translate_pg_child_filter(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(ExprValue::Bool(b)) => Some(if *b {
            String::from("TRUE")
        } else {
            String::from("FALSE")
        }),
        Expr::Literal(ExprValue::Number(v)) if v.is_finite() => Some(format!("{v}")),
        Expr::Literal(ExprValue::Text(s)) => Some(format!("'{}'", s.replace('\'', "''"))),
        Expr::MetricRef(m) if is_safe_json_key(m) => {
            Some(format!("((c.metrics ->> '{m}')::double precision)"))
        }
        Expr::PropRef(p) if is_safe_json_key(p) => Some(format!("(c.properties ->> '{p}')")),
        Expr::Unary {
            op: UnaryOp::Not,
            expr: inner,
        } => Some(format!("(NOT ({}))", try_translate_pg_child_filter(inner)?)),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => Some(format!("(-({}))", try_translate_pg_child_filter(inner)?)),
        Expr::Binary { op, left, right } => translate_pg_binary(*op, left, right),
        Expr::Call { name, args } if name == "is_null" && args.len() == 1 => {
            let arg = args.first()?;
            match arg {
                Expr::MetricRef(m) if is_safe_json_key(m) => {
                    Some(format!("((c.metrics ->> '{m}') IS NULL)"))
                }
                Expr::PropRef(p) if is_safe_json_key(p) => {
                    Some(format!("((c.properties ->> '{p}') IS NULL)"))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn translate_pg_binary(op: BinaryOp, left: &Expr, right: &Expr) -> Option<String> {
    // Only push down PropRef comparisons when compared against Text literals (avoiding text vs numeric mismatch in SQL;
    // numeric/boolean PropRef filters fall back cleanly to the exact Rust evaluator!).
    if matches!(left, Expr::PropRef(_)) && !matches!(right, Expr::Literal(ExprValue::Text(_))) {
        return None;
    }
    if matches!(right, Expr::PropRef(_)) && !matches!(left, Expr::Literal(ExprValue::Text(_))) {
        return None;
    }

    let l = try_translate_pg_child_filter(left)?;
    let r = try_translate_pg_child_filter(right)?;
    let sql_op = match op {
        BinaryOp::Or => "OR",
        BinaryOp::And => "AND",
        BinaryOp::Eq => "=",
        BinaryOp::Ne => "<>",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div | BinaryOp::Mod | BinaryOp::Pow => return None,
    };
    Some(format!("({l} {sql_op} {r})"))
}

fn is_safe_json_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_valid_series_metric_name(metric: &str) -> bool {
    let bytes = metric.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    bytes.len() <= 63
        && first.is_ascii_lowercase()
        && bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn extract_metric_ts(
    props: &serde_json::Map<String, serde_json::Value>,
    metric: &str,
) -> Option<DateTime<Utc>> {
    props
        .get("__metric_ts")
        .and_then(serde_json::Value::as_object)
        .and_then(|m| m.get(metric))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

const fn normalize_zero(v: f64) -> f64 {
    if matches!(v.classify(), FpCategory::Zero) {
        0.0
    } else {
        v
    }
}

fn values_effectively_equal(old_val: Option<f64>, new_val: Option<f64>) -> bool {
    match (old_val, new_val) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(a), Some(b)) => {
            let diff = (a - b).abs();
            let scale = a.abs().max(b.abs()).max(1.0);
            diff / scale < RELATIVE_TOLERANCE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_sample(key: &str, val: f64, weight: f64, sec: i64) -> ChildMetricSample {
        let ts = DateTime::<Utc>::from_timestamp(1_700_000_000 + sec, 0).unwrap_or_else(Utc::now);
        ChildMetricSample {
            node_id: Uuid::new_v4(),
            node_key: String::from(key),
            type_name: String::from("Order"),
            title: String::from(key),
            properties: serde_json::Map::new(),
            metrics: BTreeMap::from([(String::from("demand"), val)]),
            link_weight: weight,
            updated_at: ts,
        }
    }

    fn mk_agg_rule(op: AggFn, weight_by_link: bool) -> AggRuleSpec {
        AggRuleSpec {
            target_metric: String::from("out"),
            op,
            via_link_type: String::from("FULFILLS"),
            direction: LinkDirection::Out,
            source_type_name: String::from("Order"),
            source_metric: String::from("demand"),
            filter: None,
            weight_by_link,
        }
    }

    #[test]
    fn aggregate_exec_evaluates_all_eight_ops_weights_and_empty_sets() {
        let empty: Vec<&ChildMetricSample> = Vec::new();
        assert_eq!(
            eval_agg_samples(&empty, &mk_agg_rule(AggFn::Sum, false)),
            Some(0.0)
        );
        assert_eq!(
            eval_agg_samples(&empty, &mk_agg_rule(AggFn::Count, false)),
            Some(0.0)
        );
        assert_eq!(
            eval_agg_samples(&empty, &mk_agg_rule(AggFn::Avg, false)),
            None
        );
        assert_eq!(
            eval_agg_samples(&empty, &mk_agg_rule(AggFn::Median, false)),
            None
        );

        let s1 = mk_sample("O-1", 10.0, 2.0, 1);
        let s2 = mk_sample("O-2", 20.0, 3.0, 5);
        let s3 = mk_sample("O-3", 20.0, 5.0, 3);
        let refs = vec![&s1, &s2, &s3];

        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Sum, false)),
            Some(50.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Sum, true)),
            Some(180.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Avg, true)),
            Some(18.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Min, false)),
            Some(10.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Max, false)),
            Some(20.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Count, false)),
            Some(3.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::CountDistinct, false)),
            Some(2.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Median, false)),
            Some(20.0)
        );
        assert_eq!(
            eval_agg_samples(&refs, &mk_agg_rule(AggFn::Last, false)),
            Some(20.0)
        );

        assert!(values_effectively_equal(Some(100.0), Some(100.0 + 1e-13)));
        assert!(!values_effectively_equal(Some(100.0), Some(100.001)));
        assert!(!values_effectively_equal(Some(0.0), None));
    }
}
