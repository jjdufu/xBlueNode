//! Step 10 `AxSim` Copy-on-Write sandbox, overlay, scenario, checkpoint, and decision repository (`09-STEP10`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, InterventionMode, MetricSourceTag, OutboxTopic};
use axpoint_domain::graph::TraversalDirection;
use axpoint_domain::platform::NewOutboxEvent;
use axpoint_domain::ports::SimRepository;
use axpoint_domain::sim::{
    Checkpoint, CreateDecisionRequest, CreateOverlayLinkRequest, CreateOverlayNodeRequest,
    CreateScenarioRequest, DEFAULT_SANDBOX_MAX_NODES, DecisionChangeInput, DecisionChangeItem,
    DecisionConflictItem, DecisionRecord, DecisionStatus, ImpactListQuery, ImpactRow,
    MAX_SANDBOX_SCOPE_DEPTH, PatchOverlayLinkRequest, PatchOverlayNodeRequest, PropagationSpec,
    RefreshBaseResponse, SandboxGraphLink, SandboxGraphNode, SandboxGraphResponse, SandboxScope,
    SandboxStatus, ScenarioRecord, ScenarioStatus, ScenarioSummary, SimGraph, SimLink, SimMode,
    SimNode, SimResult,
};
use axpoint_domain::topology::{
    AxSandboxRecord, AxSimAction, AxSimImpactRow, AxSimResult, CreateAxSandboxRequest,
    SimulateAxSandboxRequest,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

const UNNEST_CHUNK_SIZE: usize = 5_000;
const TOP_SUMMARY_IMPACTS: usize = 200;

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn parse_sandbox_status(raw: &str) -> Result<SandboxStatus, AppError> {
    match raw {
        "CLONING" => Ok(SandboxStatus::Cloning),
        "READY" => Ok(SandboxStatus::Ready),
        "ARCHIVED" => Ok(SandboxStatus::Archived),
        _ => Err(AppError::Database(format!(
            "unknown ax_sandboxes.status value: {raw}"
        ))),
    }
}

fn parse_scenario_status(raw: &str) -> Result<ScenarioStatus, AppError> {
    match raw {
        "PENDING" => Ok(ScenarioStatus::Pending),
        "RUNNING" => Ok(ScenarioStatus::Running),
        "SUCCEEDED" => Ok(ScenarioStatus::Succeeded),
        "FAILED" => Ok(ScenarioStatus::Failed),
        _ => Err(AppError::Database(format!(
            "unknown ax_sim_scenarios.status value: {raw}"
        ))),
    }
}

fn parse_decision_status(raw: &str) -> Result<DecisionStatus, AppError> {
    match raw {
        "PENDING_APPROVAL" => Ok(DecisionStatus::PendingApproval),
        "APPROVED" => Ok(DecisionStatus::Approved),
        "APPLIED" => Ok(DecisionStatus::Applied),
        "REJECTED" => Ok(DecisionStatus::Rejected),
        "CONFLICT" => Ok(DecisionStatus::Conflict),
        "FAILED" => Ok(DecisionStatus::Failed),
        _ => Err(AppError::Database(format!(
            "unknown ax_sim_decisions.status value: {raw}"
        ))),
    }
}

fn validate_scope(scope: &SandboxScope) -> Result<(), AppError> {
    if scope.depth > MAX_SANDBOX_SCOPE_DEPTH {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!(
                "scope.depth must be <= {MAX_SANDBOX_SCOPE_DEPTH}, got {}",
                scope.depth
            ),
        ));
    }
    if scope.max_nodes == 0 || scope.max_nodes > DEFAULT_SANDBOX_MAX_NODES {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!(
                "scope.max_nodes must be in 1..={DEFAULT_SANDBOX_MAX_NODES}, got {}",
                scope.max_nodes
            ),
        ));
    }
    Ok(())
}

fn validate_finite_metrics(metrics: &BTreeMap<String, f64>) -> Result<(), AppError> {
    for (key, val) in metrics {
        if key.trim().is_empty() || !val.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("metric '{key}' has empty key or non-finite value"),
            ));
        }
    }
    Ok(())
}

fn normalize_filter_list(items: &[String]) -> Option<Vec<String>> {
    let cleaned: Vec<String> = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn decode_json_object(val: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match val {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

fn decode_metrics_map(val: serde_json::Value) -> BTreeMap<String, f64> {
    serde_json::from_value(val).unwrap_or_default()
}

fn decode_sandbox_row(row: &sqlx::postgres::PgRow) -> Result<AxSandboxRecord, AppError> {
    let status_raw: String = row.get("status");
    let status = parse_sandbox_status(&status_raw)?;
    let scope_val: serde_json::Value = row.get("scope");
    let scope: SandboxScope = serde_json::from_value(scope_val).unwrap_or_default();
    let node_count_i32: i32 = row.get("node_count");
    let link_count_i32: i32 = row.get("link_count");
    let actions: Vec<AxSimAction> = serde_json::from_value(row.get("actions")).unwrap_or_default();
    let res_val: serde_json::Value = row.get("sim_result");
    let sim_result: Option<AxSimResult> = serde_json::from_value(res_val).ok();

    Ok(AxSandboxRecord {
        id: row.get("id"),
        name: row.get("name"),
        status,
        scope,
        node_count: usize::try_from(node_count_i32).unwrap_or(0),
        link_count: usize::try_from(link_count_i32).unwrap_or(0),
        base_captured_at: row.get("base_captured_at"),
        schema_version: row.get("schema_version"),
        created_by: row.get("created_by"),
        actions,
        sim_result,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_scenario_row(
    row: &sqlx::postgres::PgRow,
    current_overlay_hash: &str,
) -> Result<ScenarioRecord, AppError> {
    let status_raw: String = row.get("status");
    let status = parse_scenario_status(&status_raw)?;
    let actions: Vec<AxSimAction> = serde_json::from_value(row.get("actions")).map_err(|err| {
        AppError::Database(format!("decode ax_sim_scenarios.actions failed: {err}"))
    })?;
    let params = serde_json::from_value(row.get("params")).map_err(|err| {
        AppError::Database(format!("decode ax_sim_scenarios.params failed: {err}"))
    })?;
    let result_val: Option<serde_json::Value> = row.get("result");
    let result: Option<ScenarioSummary> = result_val
        .map(serde_json::from_value)
        .transpose()
        .map_err(|err| {
            AppError::Database(format!("decode ax_sim_scenarios.result failed: {err}"))
        })?;
    let overlay_hash: String = row.get("overlay_hash");

    Ok(ScenarioRecord {
        id: row.get("id"),
        sandbox_id: row.get("sandbox_id"),
        name: row.get("name"),
        description: row.get("description"),
        actions,
        params,
        is_stale: overlay_hash != current_overlay_hash,
        overlay_hash,
        result,
        status,
        error_message: row.get("error_message"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
    })
}

fn decode_decision_row(row: &sqlx::postgres::PgRow) -> Result<DecisionRecord, AppError> {
    let status_raw: String = row.get("status");
    let status = parse_decision_status(&status_raw)?;
    let changes: Vec<DecisionChangeItem> =
        serde_json::from_value(row.get("changes")).map_err(|err| {
            AppError::Database(format!("decode ax_sim_decisions.changes failed: {err}"))
        })?;
    let conflict_report: Vec<DecisionConflictItem> =
        serde_json::from_value(row.get("conflict_report")).map_err(|err| {
            AppError::Database(format!(
                "decode ax_sim_decisions.conflict_report failed: {err}"
            ))
        })?;

    Ok(DecisionRecord {
        id: row.get("id"),
        scenario_id: row.get("scenario_id"),
        title: row.get("title"),
        changes,
        trigger_flow_id: row.get("trigger_flow_id"),
        status,
        approval_id: row.get("approval_id"),
        conflict_report,
        tolerance_ratio: row.get("tolerance_ratio"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        applied_at: row.get("applied_at"),
        applied_by: row.get("applied_by"),
    })
}

async fn ensure_sandbox_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ax_sandboxes WHERE id = $1 AND tenant_id = $2)",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check ax_sandboxes exists failed: {err}")))?;
    if !exists {
        return Err(AppError::NotFound("SANDBOX_NOT_FOUND"));
    }
    Ok(())
}

async fn current_main_schema_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<Option<i32>, AppError> {
    sqlx::query_scalar(
        "SELECT current_version
           FROM xb_schema_heads
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND current_version > 0",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query xb_schema_heads failed: {err}")))
}

fn scope_walk_cte_sql(direction: TraversalDirection) -> &'static str {
    match direction {
        TraversalDirection::Out => {
            "WITH RECURSIVE walk(node_id, hop) AS (
                 SELECT id, 0
                   FROM xb_nodes
                  WHERE tenant_id = $1 AND id = ANY($2)
                 UNION
                 SELECT l.dst_node_id, g.hop + 1
                   FROM walk g
                   JOIN xb_links l ON l.src_node_id = g.node_id
                  WHERE g.hop < $3
                    AND l.tenant_id = $1
                    AND ($4::text[] IS NULL OR l.link_type = ANY($4))
             )
             SELECT n.id
               FROM (SELECT DISTINCT node_id FROM walk) w
               JOIN xb_nodes n ON n.id = w.node_id
              WHERE n.tenant_id = $1
                AND ($5::text[] IS NULL OR n.type_name = ANY($5))
              ORDER BY n.id ASC
              LIMIT $6"
        }
        TraversalDirection::In => {
            "WITH RECURSIVE walk(node_id, hop) AS (
                 SELECT id, 0
                   FROM xb_nodes
                  WHERE tenant_id = $1 AND id = ANY($2)
                 UNION
                 SELECT l.src_node_id, g.hop + 1
                   FROM walk g
                   JOIN xb_links l ON l.dst_node_id = g.node_id
                  WHERE g.hop < $3
                    AND l.tenant_id = $1
                    AND ($4::text[] IS NULL OR l.link_type = ANY($4))
             )
             SELECT n.id
               FROM (SELECT DISTINCT node_id FROM walk) w
               JOIN xb_nodes n ON n.id = w.node_id
              WHERE n.tenant_id = $1
                AND ($5::text[] IS NULL OR n.type_name = ANY($5))
              ORDER BY n.id ASC
              LIMIT $6"
        }
        TraversalDirection::Both => {
            "WITH RECURSIVE walk(node_id, hop) AS (
                 SELECT id, 0
                   FROM xb_nodes
                  WHERE tenant_id = $1 AND id = ANY($2)
                 UNION
                 SELECT CASE WHEN l.src_node_id = g.node_id THEN l.dst_node_id ELSE l.src_node_id END,
                        g.hop + 1
                   FROM walk g
                   JOIN xb_links l ON (l.src_node_id = g.node_id OR l.dst_node_id = g.node_id)
                  WHERE g.hop < $3
                    AND l.tenant_id = $1
                    AND ($4::text[] IS NULL OR l.link_type = ANY($4))
             )
             SELECT n.id
               FROM (SELECT DISTINCT node_id FROM walk) w
               JOIN xb_nodes n ON n.id = w.node_id
              WHERE n.tenant_id = $1
                AND ($5::text[] IS NULL OR n.type_name = ANY($5))
              ORDER BY n.id ASC
              LIMIT $6"
        }
    }
}

async fn collect_scoped_node_ids_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    scope: &SandboxScope,
) -> Result<Vec<Uuid>, AppError> {
    validate_scope(scope)?;
    let type_filter = normalize_filter_list(&scope.type_names);
    let link_filter = normalize_filter_list(&scope.link_types);
    let max_nodes_i64 = i64::try_from(scope.max_nodes).unwrap_or(50_000);
    let fetch_limit = max_nodes_i64.saturating_add(1);

    let node_ids: Vec<Uuid> = if scope.root_node_ids.is_empty() {
        sqlx::query_scalar(
            "SELECT id
               FROM xb_nodes
              WHERE tenant_id = $1
                AND ($2::text[] IS NULL OR type_name = ANY($2))
              ORDER BY id ASC
              LIMIT $3",
        )
        .bind(tenant_id)
        .bind(&type_filter)
        .bind(fetch_limit)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("collect tenant nodes failed: {err}")))?
    } else {
        let depth_i32 = i32::try_from(scope.depth).unwrap_or(6);
        let sql = scope_walk_cte_sql(scope.direction);
        sqlx::query_scalar(sql)
            .bind(tenant_id)
            .bind(&scope.root_node_ids)
            .bind(depth_i32)
            .bind(&link_filter)
            .bind(&type_filter)
            .bind(fetch_limit)
            .fetch_all(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("scope walk CTE failed: {err}")))?
    };

    if node_ids.len() > scope.max_nodes {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!(
                "scoped node count exceeds max_nodes ({}); specify a narrower scope",
                scope.max_nodes
            ),
        ));
    }

    Ok(node_ids)
}

async fn clone_base_from_prod_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    scope: &SandboxScope,
) -> Result<(i32, i32), AppError> {
    let node_ids = collect_scoped_node_ids_tx(tx, tenant_id, scope).await?;
    if node_ids.is_empty() {
        return Ok((0, 0));
    }

    let link_filter = normalize_filter_list(&scope.link_types);
    let inserted_nodes = sqlx::query(
        "INSERT INTO ax_sim_base_nodes (
             tenant_id, sandbox_id, node_id, type_name, node_key, title,
             metrics, properties, row_version
         )
         SELECT $1, $2, id, type_name, node_key, title, metrics, properties, row_version
           FROM xb_nodes
          WHERE tenant_id = $1 AND id = ANY($3)",
    )
    .bind(tenant_id)
    .bind(sandbox_id)
    .bind(&node_ids)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("clone ax_sim_base_nodes failed: {err}")))?;

    let inserted_links = sqlx::query(
        "INSERT INTO ax_sim_base_links (
             tenant_id, sandbox_id, link_id, link_type, src_node_id, dst_node_id, weight
         )
         SELECT $1, $2, id, link_type, src_node_id, dst_node_id, weight
           FROM xb_links
          WHERE tenant_id = $1
            AND src_node_id = ANY($3)
            AND dst_node_id = ANY($3)
            AND ($4::text[] IS NULL OR link_type = ANY($4))",
    )
    .bind(tenant_id)
    .bind(sandbox_id)
    .bind(&node_ids)
    .bind(&link_filter)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("clone ax_sim_base_links failed: {err}")))?;

    let node_count = i32::try_from(inserted_nodes.rows_affected()).unwrap_or(i32::MAX);
    let link_count = i32::try_from(inserted_links.rows_affected()).unwrap_or(i32::MAX);
    Ok((node_count, link_count))
}

async fn load_edge_specs_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<BTreeMap<String, PropagationSpec>, AppError> {
    let rows = sqlx::query(
        "SELECT name, propagation
           FROM xb_edge_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main'
          ORDER BY created_at ASC, name ASC",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query xb_edge_types.propagation failed: {err}")))?;

    let mut specs: BTreeMap<String, PropagationSpec> = BTreeMap::new();
    for row in rows {
        let name: String = row.get("name");
        let prop_val: serde_json::Value = row.get("propagation");
        let parsed: PropagationSpec = serde_json::from_value(prop_val).unwrap_or_default();
        match specs.get_mut(&name) {
            Some(existing) if !parsed.rules.is_empty() => {
                existing.rules.extend(parsed.rules);
                existing.clamp = parsed.clamp;
            }
            Some(_) => {}
            None => {
                specs.insert(name, parsed);
            }
        }
    }

    Ok(specs)
}

async fn load_sandbox_nodes_merged_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
) -> Result<Vec<(SandboxGraphNode, i64)>, AppError> {
    let base_rows = sqlx::query(
        "SELECT b.node_id, b.type_name, b.node_key, b.title, b.metrics, b.properties, b.row_version,
                o.title AS o_title, o.metrics AS o_metrics, o.properties AS o_properties,
                o.deleted AS o_deleted, (o.node_id IS NOT NULL) AS has_overlay
           FROM ax_sim_base_nodes b
           LEFT JOIN ax_sim_overlay_nodes o
             ON o.sandbox_id = b.sandbox_id AND o.node_id = b.node_id
          WHERE b.sandbox_id = $1 AND b.tenant_id = $2
          ORDER BY b.node_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load base+overlay nodes failed: {err}")))?;

    let mut out = Vec::with_capacity(base_rows.len());
    for row in base_rows {
        if row.get::<Option<bool>, _>("o_deleted") == Some(true) {
            continue;
        }
        let has_overlay: bool = row.get("has_overlay");
        let base_title: String = row.get("title");
        let o_title: Option<String> = row.get("o_title");
        let mut metrics = decode_metrics_map(row.get("metrics"));
        if let Some(o_metrics_val) = row.get::<Option<serde_json::Value>, _>("o_metrics") {
            for (k, v) in decode_metrics_map(o_metrics_val) {
                metrics.insert(k, v);
            }
        }
        let mut properties = decode_json_object(row.get("properties"));
        if let Some(o_props_val) = row.get::<Option<serde_json::Value>, _>("o_properties") {
            for (k, v) in decode_json_object(o_props_val) {
                properties.insert(k, v);
            }
        }
        let row_version: i64 = row.get("row_version");
        out.push((
            SandboxGraphNode {
                id: row.get("node_id"),
                type_name: row.get("type_name"),
                node_key: row.get("node_key"),
                title: o_title.unwrap_or(base_title),
                metrics,
                properties,
                is_modified: has_overlay,
                is_new: false,
            },
            row_version,
        ));
    }

    let new_rows = sqlx::query(
        "SELECT node_id, type_name, node_key, title, metrics, properties
           FROM ax_sim_overlay_nodes
          WHERE sandbox_id = $1 AND tenant_id = $2
            AND is_new = TRUE AND deleted = FALSE
          ORDER BY node_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load new overlay nodes failed: {err}")))?;

    for row in new_rows {
        let metrics = row
            .get::<Option<serde_json::Value>, _>("metrics")
            .map_or_default(decode_metrics_map);
        let properties = row
            .get::<Option<serde_json::Value>, _>("properties")
            .map_or_default(decode_json_object);
        out.push((
            SandboxGraphNode {
                id: row.get("node_id"),
                type_name: row
                    .get::<Option<String>, _>("type_name")
                    .unwrap_or_default(),
                node_key: row.get::<Option<String>, _>("node_key").unwrap_or_default(),
                title: row.get::<Option<String>, _>("title").unwrap_or_default(),
                metrics,
                properties,
                is_modified: false,
                is_new: true,
            },
            0,
        ));
    }

    Ok(out)
}

async fn load_sandbox_links_merged_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    active_node_ids: &BTreeSet<Uuid>,
) -> Result<Vec<SandboxGraphLink>, AppError> {
    let base_rows = sqlx::query(
        "SELECT b.link_id, b.link_type, b.src_node_id, b.dst_node_id, b.weight,
                o.link_type AS o_link_type, o.weight AS o_weight,
                o.deleted AS o_deleted, (o.link_id IS NOT NULL) AS has_overlay
           FROM ax_sim_base_links b
           LEFT JOIN ax_sim_overlay_links o
             ON o.sandbox_id = b.sandbox_id AND o.link_id = b.link_id
          WHERE b.sandbox_id = $1 AND b.tenant_id = $2
          ORDER BY b.link_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load base+overlay links failed: {err}")))?;

    let mut out = Vec::with_capacity(base_rows.len());
    for row in base_rows {
        if row.get::<Option<bool>, _>("o_deleted") == Some(true) {
            continue;
        }
        let src: Uuid = row.get("src_node_id");
        let dst: Uuid = row.get("dst_node_id");
        if !active_node_ids.contains(&src) || !active_node_ids.contains(&dst) {
            continue;
        }
        let has_overlay: bool = row.get("has_overlay");
        let link_type: String = row
            .get::<Option<String>, _>("o_link_type")
            .unwrap_or_else(|| row.get("link_type"));
        let weight: f64 = row
            .get::<Option<f64>, _>("o_weight")
            .unwrap_or_else(|| row.get("weight"));
        out.push(SandboxGraphLink {
            id: row.get("link_id"),
            link_type,
            src_node_id: src,
            dst_node_id: dst,
            weight,
            is_modified: has_overlay,
            is_new: false,
        });
    }

    let new_rows = sqlx::query(
        "SELECT link_id, link_type, src_node_id, dst_node_id, weight
           FROM ax_sim_overlay_links
          WHERE sandbox_id = $1 AND tenant_id = $2
            AND is_new = TRUE AND deleted = FALSE
          ORDER BY link_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load new overlay links failed: {err}")))?;

    for row in new_rows {
        let Some(src) = row.get::<Option<Uuid>, _>("src_node_id") else {
            continue;
        };
        let Some(dst) = row.get::<Option<Uuid>, _>("dst_node_id") else {
            continue;
        };
        if !active_node_ids.contains(&src) || !active_node_ids.contains(&dst) {
            continue;
        }
        out.push(SandboxGraphLink {
            id: row.get("link_id"),
            link_type: row
                .get::<Option<String>, _>("link_type")
                .unwrap_or_default(),
            src_node_id: src,
            dst_node_id: dst,
            weight: row.get::<Option<f64>, _>("weight").unwrap_or(1.0),
            is_modified: false,
            is_new: true,
        });
    }

    Ok(out)
}

async fn refresh_sandbox_counts_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
) -> Result<(i32, i32), AppError> {
    let nodes = load_sandbox_nodes_merged_tx(tx, tenant_id, sandbox_id).await?;
    let active_ids: BTreeSet<Uuid> = nodes.iter().map(|(n, _)| n.id).collect();
    let links = load_sandbox_links_merged_tx(tx, tenant_id, sandbox_id, &active_ids).await?;
    let node_count = i32::try_from(nodes.len()).unwrap_or(i32::MAX);
    let link_count = i32::try_from(links.len()).unwrap_or(i32::MAX);

    sqlx::query(
        "UPDATE ax_sandboxes
            SET node_count = $2,
                link_count = $3,
                updated_at = NOW()
          WHERE id = $1 AND tenant_id = $4",
    )
    .bind(sandbox_id)
    .bind(node_count)
    .bind(link_count)
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("update ax_sandboxes counts failed: {err}")))?;

    Ok((node_count, link_count))
}

async fn compute_overlay_hash_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
) -> Result<String, AppError> {
    let node_rows = sqlx::query(
        "SELECT node_id, type_name, node_key, title, metrics, properties, is_new, deleted
           FROM ax_sim_overlay_nodes
          WHERE sandbox_id = $1 AND tenant_id = $2
          ORDER BY node_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("hash ax_sim_overlay_nodes failed: {err}")))?;

    let link_rows = sqlx::query(
        "SELECT link_id, link_type, src_node_id, dst_node_id, weight, is_new, deleted
           FROM ax_sim_overlay_links
          WHERE sandbox_id = $1 AND tenant_id = $2
          ORDER BY link_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("hash ax_sim_overlay_links failed: {err}")))?;

    let mut hasher = Sha256::new();
    hasher.update(b"axsim_overlay_v1:");
    for row in node_rows {
        let nid: Uuid = row.get("node_id");
        let is_new: bool = row.get("is_new");
        let deleted: bool = row.get("deleted");
        let type_name: Option<String> = row.get("type_name");
        let node_key: Option<String> = row.get("node_key");
        let title: Option<String> = row.get("title");
        let metrics = row
            .get::<Option<serde_json::Value>, _>("metrics")
            .map_or_default(decode_metrics_map);
        let props = row
            .get::<Option<serde_json::Value>, _>("properties")
            .map_or_default(decode_json_object);
        let header = format!("N:{nid}:{is_new}:{deleted}:{type_name:?}:{node_key:?}:{title:?}:");
        hasher.update(header.as_bytes());
        if let Ok(m_json) = serde_json::to_vec(&metrics) {
            hasher.update(&m_json);
        }
        if let Ok(p_json) = serde_json::to_vec(&props) {
            hasher.update(&p_json);
        }
        hasher.update(b";");
    }

    for row in link_rows {
        let lid: Uuid = row.get("link_id");
        let is_new: bool = row.get("is_new");
        let deleted: bool = row.get("deleted");
        let link_type: Option<String> = row.get("link_type");
        let src: Option<Uuid> = row.get("src_node_id");
        let dst: Option<Uuid> = row.get("dst_node_id");
        let weight_bits = row.get::<Option<f64>, _>("weight").map(f64::to_bits);
        let line =
            format!("L:{lid}:{is_new}:{deleted}:{link_type:?}:{src:?}:{dst:?}:{weight_bits:?};");
        hasher.update(line.as_bytes());
    }

    Ok(hex::encode(hasher.finalize()))
}

async fn load_effective_node_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    node_id: Uuid,
) -> Result<Option<SandboxGraphNode>, AppError> {
    let base = sqlx::query(
        "SELECT type_name, node_key, title, metrics, properties
           FROM ax_sim_base_nodes
          WHERE sandbox_id = $1 AND tenant_id = $2 AND node_id = $3",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load base node failed: {err}")))?;

    let overlay = sqlx::query(
        "SELECT type_name, node_key, title, metrics, properties, is_new, deleted
           FROM ax_sim_overlay_nodes
          WHERE sandbox_id = $1 AND tenant_id = $2 AND node_id = $3",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load overlay node failed: {err}")))?;

    if let Some(ref ov) = overlay
        && ov.get::<bool, _>("deleted")
    {
        return Ok(None);
    }

    if let Some(b_row) = base {
        let mut metrics = decode_metrics_map(b_row.get("metrics"));
        let mut properties = decode_json_object(b_row.get("properties"));
        let mut title: String = b_row.get("title");
        let has_overlay = overlay.is_some();
        if let Some(ov) = overlay {
            if let Some(t) = ov.get::<Option<String>, _>("title") {
                title = t;
            }
            if let Some(m_val) = ov.get::<Option<serde_json::Value>, _>("metrics") {
                for (k, v) in decode_metrics_map(m_val) {
                    metrics.insert(k, v);
                }
            }
            if let Some(p_val) = ov.get::<Option<serde_json::Value>, _>("properties") {
                for (k, v) in decode_json_object(p_val) {
                    properties.insert(k, v);
                }
            }
        }
        return Ok(Some(SandboxGraphNode {
            id: node_id,
            type_name: b_row.get("type_name"),
            node_key: b_row.get("node_key"),
            title,
            metrics,
            properties,
            is_modified: has_overlay,
            is_new: false,
        }));
    }

    if let Some(ov) = overlay
        && ov.get::<bool, _>("is_new")
    {
        return Ok(Some(SandboxGraphNode {
            id: node_id,
            type_name: ov.get::<Option<String>, _>("type_name").unwrap_or_default(),
            node_key: ov.get::<Option<String>, _>("node_key").unwrap_or_default(),
            title: ov.get::<Option<String>, _>("title").unwrap_or_default(),
            metrics: ov
                .get::<Option<serde_json::Value>, _>("metrics")
                .map_or_default(decode_metrics_map),
            properties: ov
                .get::<Option<serde_json::Value>, _>("properties")
                .map_or_default(decode_json_object),
            is_modified: false,
            is_new: true,
        }));
    }

    Ok(None)
}

async fn load_effective_link_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    link_id: Uuid,
) -> Result<Option<SandboxGraphLink>, AppError> {
    let base = sqlx::query(
        "SELECT link_type, src_node_id, dst_node_id, weight
           FROM ax_sim_base_links
          WHERE sandbox_id = $1 AND tenant_id = $2 AND link_id = $3",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .bind(link_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load base link failed: {err}")))?;

    let overlay = sqlx::query(
        "SELECT link_type, src_node_id, dst_node_id, weight, is_new, deleted
           FROM ax_sim_overlay_links
          WHERE sandbox_id = $1 AND tenant_id = $2 AND link_id = $3",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .bind(link_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load overlay link failed: {err}")))?;

    if let Some(ref ov) = overlay
        && ov.get::<bool, _>("deleted")
    {
        return Ok(None);
    }

    if let Some(b_row) = base {
        let mut link_type: String = b_row.get("link_type");
        let mut weight: f64 = b_row.get("weight");
        let has_overlay = overlay.is_some();
        if let Some(ov) = overlay {
            if let Some(lt) = ov.get::<Option<String>, _>("link_type") {
                link_type = lt;
            }
            if let Some(w) = ov.get::<Option<f64>, _>("weight") {
                weight = w;
            }
        }
        return Ok(Some(SandboxGraphLink {
            id: link_id,
            link_type,
            src_node_id: b_row.get("src_node_id"),
            dst_node_id: b_row.get("dst_node_id"),
            weight,
            is_modified: has_overlay,
            is_new: false,
        }));
    }

    if let Some(ov) = overlay
        && ov.get::<bool, _>("is_new")
    {
        return Ok(Some(SandboxGraphLink {
            id: link_id,
            link_type: ov.get::<Option<String>, _>("link_type").unwrap_or_default(),
            src_node_id: ov.get::<Option<Uuid>, _>("src_node_id").unwrap_or_default(),
            dst_node_id: ov.get::<Option<Uuid>, _>("dst_node_id").unwrap_or_default(),
            weight: ov.get::<Option<f64>, _>("weight").unwrap_or(1.0),
            is_modified: false,
            is_new: true,
        }));
    }

    Ok(None)
}

async fn upsert_overlay_node_patch_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    node_id: Uuid,
    req: &PatchOverlayNodeRequest,
) -> Result<(), AppError> {
    let existing = sqlx::query(
        "SELECT title, metrics, properties, is_new
           FROM ax_sim_overlay_nodes
          WHERE sandbox_id = $1 AND tenant_id = $2 AND node_id = $3",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .bind(node_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query overlay node for patch failed: {err}")))?;

    let mut ov_metrics = existing
        .as_ref()
        .and_then(|r| r.get::<Option<serde_json::Value>, _>("metrics"))
        .map_or_default(decode_metrics_map);
    if let Some(ref incoming) = req.metrics {
        for (k, v) in incoming {
            ov_metrics.insert(k.clone(), *v);
        }
    }

    let mut ov_props = existing
        .as_ref()
        .and_then(|r| r.get::<Option<serde_json::Value>, _>("properties"))
        .map_or_default(decode_json_object);
    if let Some(ref incoming) = req.properties {
        for (k, v) in incoming {
            ov_props.insert(k.clone(), v.clone());
        }
    }

    let existing_title = existing
        .as_ref()
        .and_then(|r| r.get::<Option<String>, _>("title"));
    let next_title = req
        .title
        .as_deref()
        .map(str::trim)
        .map(String::from)
        .or(existing_title);
    let is_new = existing
        .as_ref()
        .is_some_and(|r| r.get::<bool, _>("is_new"));

    let metrics_json = if req.metrics.is_some()
        || existing
            .as_ref()
            .and_then(|r| r.get::<Option<serde_json::Value>, _>("metrics"))
            .is_some()
    {
        Some(serde_json::to_value(&ov_metrics).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize metrics: {err}"))
        })?)
    } else {
        None
    };

    let props_json = if req.properties.is_some()
        || existing
            .as_ref()
            .and_then(|r| r.get::<Option<serde_json::Value>, _>("properties"))
            .is_some()
    {
        Some(serde_json::Value::Object(ov_props))
    } else {
        None
    };

    sqlx::query(
        "INSERT INTO ax_sim_overlay_nodes (
             tenant_id, sandbox_id, node_id, title, metrics, properties, is_new, deleted, updated_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, NOW())
         ON CONFLICT (sandbox_id, node_id) DO UPDATE SET
             title = EXCLUDED.title,
             metrics = EXCLUDED.metrics,
             properties = EXCLUDED.properties,
             deleted = FALSE,
             updated_at = NOW()",
    )
    .bind(tenant_id)
    .bind(sandbox_id)
    .bind(node_id)
    .bind(next_title)
    .bind(metrics_json)
    .bind(props_json)
    .bind(is_new)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert ax_sim_overlay_nodes failed: {err}")))?;

    sqlx::query("UPDATE ax_sandboxes SET updated_at = NOW() WHERE id = $1 AND tenant_id = $2")
        .bind(sandbox_id)
        .bind(tenant_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("touch ax_sandboxes.updated_at failed: {err}"))
        })?;

    Ok(())
}

async fn batch_insert_impacts_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    scenario_id: Uuid,
    impacts: &[ImpactRow],
) -> Result<(), AppError> {
    for chunk in impacts.chunks(UNNEST_CHUNK_SIZE) {
        let cap = chunk.len();
        let mut node_ids = Vec::with_capacity(cap);
        let mut metrics = Vec::with_capacity(cap);
        let mut before_vals = Vec::with_capacity(cap);
        let mut after_vals = Vec::with_capacity(cap);
        let mut deltas = Vec::with_capacity(cap);
        let mut delta_ratios = Vec::with_capacity(cap);
        let mut p10s = Vec::with_capacity(cap);
        let mut p90s = Vec::with_capacity(cap);
        let mut hops = Vec::with_capacity(cap);
        let mut top_paths = Vec::with_capacity(cap);

        for row in chunk {
            node_ids.push(row.node_id);
            metrics.push(row.metric.clone());
            before_vals.push(row.before_val);
            after_vals.push(row.after_val);
            deltas.push(row.delta);
            delta_ratios.push(row.delta_ratio);
            p10s.push(row.p10);
            p90s.push(row.p90);
            hops.push(i32::try_from(row.hop_distance).unwrap_or(i32::MAX));
            top_paths.push(serde_json::to_value(&row.top_path).unwrap_or_default());
        }

        sqlx::query(
            "INSERT INTO ax_sim_impacts (
                 tenant_id, scenario_id, node_id, metric, before_val, after_val,
                 delta, delta_ratio, p10, p90, hop_distance, top_path
             )
             SELECT $1, $2, u.node_id, u.metric, u.before_val, u.after_val,
                    u.delta, u.delta_ratio, u.p10, u.p90, u.hop_distance, u.top_path
               FROM UNNEST(
                   $3::uuid[], $4::text[], $5::float8[], $6::float8[], $7::float8[],
                   $8::float8[], $9::float8[], $10::float8[], $11::int4[], $12::jsonb[]
               ) AS u(
                   node_id, metric, before_val, after_val, delta,
                   delta_ratio, p10, p90, hop_distance, top_path
               )
             ON CONFLICT (scenario_id, node_id, metric) DO UPDATE SET
                 before_val = EXCLUDED.before_val,
                 after_val = EXCLUDED.after_val,
                 delta = EXCLUDED.delta,
                 delta_ratio = EXCLUDED.delta_ratio,
                 p10 = EXCLUDED.p10,
                 p90 = EXCLUDED.p90,
                 hop_distance = EXCLUDED.hop_distance,
                 top_path = EXCLUDED.top_path",
        )
        .bind(tenant_id)
        .bind(scenario_id)
        .bind(&node_ids)
        .bind(&metrics)
        .bind(&before_vals)
        .bind(&after_vals)
        .bind(&deltas)
        .bind(&delta_ratios)
        .bind(&p10s)
        .bind(&p90s)
        .bind(&hops)
        .bind(&top_paths)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("batch insert ax_sim_impacts failed: {err}")))?;
    }
    Ok(())
}

async fn batch_insert_checkpoints_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    scenario_id: Uuid,
    checkpoints: &[Checkpoint],
) -> Result<(), AppError> {
    for chunk in checkpoints.chunks(UNNEST_CHUNK_SIZE) {
        let cap = chunk.len();
        let mut seqs = Vec::with_capacity(cap);
        let mut labels = Vec::with_capacity(cap);
        let mut changes = Vec::with_capacity(cap);
        for cp in chunk {
            seqs.push(cp.seq);
            labels.push(cp.label.clone());
            changes.push(cp.changes.clone());
        }

        sqlx::query(
            "INSERT INTO ax_sim_checkpoints (tenant_id, scenario_id, seq, label, changes)
             SELECT $1, $2, u.seq, u.label, u.changes
               FROM UNNEST($3::int4[], $4::text[], $5::jsonb[]) AS u(seq, label, changes)
             ON CONFLICT (scenario_id, seq) DO UPDATE SET
                 label = EXCLUDED.label,
                 changes = EXCLUDED.changes",
        )
        .bind(tenant_id)
        .bind(scenario_id)
        .bind(&seqs)
        .bind(&labels)
        .bind(&changes)
        .execute(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("batch insert ax_sim_checkpoints failed: {err}"))
        })?;
    }
    Ok(())
}

fn decode_impact_row(row: &sqlx::postgres::PgRow) -> Result<ImpactRow, AppError> {
    let hop_i32: i32 = row.get("hop_distance");
    let top_path: Vec<Uuid> = serde_json::from_value(row.get("top_path"))
        .map_err(|err| AppError::Database(format!("decode ax_sim_impacts.top_path: {err}")))?;

    Ok(ImpactRow {
        node_id: row.get("node_id"),
        type_name: row.get("type_name"),
        title: row.get("title"),
        metric: row.get("metric"),
        before_val: row.get("before_val"),
        after_val: row.get("after_val"),
        delta: row.get("delta"),
        delta_ratio: row.get("delta_ratio"),
        p10: row.get("p10"),
        p90: row.get("p90"),
        hop_distance: u32::try_from(hop_i32).unwrap_or(0),
        top_path,
        series: Vec::new(),
    })
}

async fn load_baseline_metrics_map_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    node_ids: &[Uuid],
) -> Result<HashMap<Uuid, BTreeMap<String, f64>>, AppError> {
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        "SELECT node_id, metrics
           FROM ax_sim_base_nodes
          WHERE sandbox_id = $1 AND tenant_id = $2 AND node_id = ANY($3)",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .bind(node_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ax_sim_base_nodes metrics failed: {err}")))?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let nid: Uuid = row.get("node_id");
        let metrics = decode_metrics_map(row.get("metrics"));
        map.insert(nid, metrics);
    }

    let missing_ids: Vec<Uuid> = node_ids
        .iter()
        .copied()
        .filter(|id| !map.contains_key(id))
        .collect();
    if !missing_ids.is_empty() {
        let prod_rows =
            sqlx::query("SELECT id, metrics FROM xb_nodes WHERE tenant_id = $1 AND id = ANY($2)")
                .bind(tenant_id)
                .bind(&missing_ids)
                .fetch_all(&mut **tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("fallback load xb_nodes metrics: {err}"))
                })?;
        for row in prod_rows {
            let nid: Uuid = row.get("id");
            let metrics = decode_metrics_map(row.get("metrics"));
            map.insert(nid, metrics);
        }
    }

    Ok(map)
}

fn resolve_action_mode_value(act: &AxSimAction) -> Result<(InterventionMode, f64), AppError> {
    let (mode, value) = match (act.delta_ratio, act.mode, act.value) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "cannot specify both delta_ratio and mode/value",
            ));
        }
        (Some(dr), None, None) => (InterventionMode::Ratio, dr),
        (None, Some(m), Some(v)) => (m, v),
        (None, Some(_), None) => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "value is required when mode is specified",
            ));
        }
        (None, None, _) => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "either delta_ratio or mode + value must be specified",
            ));
        }
    };
    if !value.is_finite() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "intervention value must be finite",
        ));
    }
    if mode == InterventionMode::Ratio && value <= -1.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "RATIO intervention value must be > -1.0",
        ));
    }
    Ok((mode, value))
}

async fn resolve_decision_changes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    explicit: Option<Vec<DecisionChangeInput>>,
    actions: &[AxSimAction],
) -> Result<Vec<DecisionChangeItem>, AppError> {
    if let Some(inputs) = explicit {
        if inputs.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "decision changes cannot be empty",
            ));
        }
        let target_ids: Vec<Uuid> = inputs
            .iter()
            .map(|c| c.node_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let base_map = load_baseline_metrics_map_tx(tx, tenant_id, sandbox_id, &target_ids).await?;
        let mut items = Vec::with_capacity(inputs.len());
        for input in inputs {
            let metric = input.metric.trim();
            if metric.is_empty() || !input.target_value.is_finite() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "decision change metric must be non-empty and target_value finite",
                ));
            }
            let base_metrics = base_map
                .get(&input.node_id)
                .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
            items.push(DecisionChangeItem {
                node_id: input.node_id,
                metric: String::from(metric),
                base_value: base_metrics.get(metric).copied(),
                target_value: input.target_value,
            });
        }
        return Ok(items);
    }

    if actions.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "scenario has no actions to create a decision",
        ));
    }
    let target_ids: Vec<Uuid> = actions
        .iter()
        .map(|a| a.target_node_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let base_map = load_baseline_metrics_map_tx(tx, tenant_id, sandbox_id, &target_ids).await?;
    let mut items = Vec::with_capacity(actions.len());
    for act in actions {
        let (mode, value) = resolve_action_mode_value(act)?;
        let metric = act.metric_key.trim();
        let base_metrics = base_map
            .get(&act.target_node_id)
            .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
        let base_value = base_metrics.get(metric).copied();
        let target_value = match mode {
            InterventionMode::Absolute => value,
            InterventionMode::Ratio => {
                let base = base_value.ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!(
                            "metric missing on baseline node {}: {metric}",
                            act.target_node_id
                        ),
                    )
                })?;
                base * (1.0 + value)
            }
            InterventionMode::Add => {
                let base = base_value.ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!(
                            "metric missing on baseline node {}: {metric}",
                            act.target_node_id
                        ),
                    )
                })?;
                base + value
            }
        };
        items.push(DecisionChangeItem {
            node_id: act.target_node_id,
            metric: String::from(metric),
            base_value,
            target_value,
        });
    }
    Ok(items)
}

async fn insert_sim_approval_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    decision_id: Uuid,
    scenario_id: Uuid,
    title: &str,
    changes: &[DecisionChangeItem],
) -> Result<Uuid, AppError> {
    let detail = serde_json::json!({
        "decision_id": decision_id,
        "scenario_id": scenario_id,
        "changes": changes,
    });
    let approval_id: Uuid = sqlx::query_scalar(
        "INSERT INTO ax_approvals (
             tenant_id, run_id, node_id, source_kind, source_ref, title, detail,
             assignee_roles, assignee_users, status
         )
         VALUES ($1, NULL, NULL, 'SIM_APPLY', $2, $3, $4, ARRAY['ADMIN','OPERATOR']::text[], ARRAY[]::uuid[], 'PENDING')
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(decision_id.to_string())
    .bind(title)
    .bind(detail)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert SIM_APPLY ax_approvals failed: {err}")))?;

    Ok(approval_id)
}

#[derive(Debug, Clone)]
struct LockedProdNode {
    metrics: BTreeMap<String, f64>,
    properties: serde_json::Map<String, serde_json::Value>,
}

async fn lock_prod_nodes_for_decision_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    changes: &[DecisionChangeItem],
) -> Result<HashMap<Uuid, LockedProdNode>, AppError> {
    let node_ids: Vec<Uuid> = changes
        .iter()
        .map(|c| c.node_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if node_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        "SELECT id, metrics, properties
           FROM xb_nodes
          WHERE tenant_id = $1 AND id = ANY($2)
          ORDER BY id ASC
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(&node_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock xb_nodes FOR UPDATE failed: {err}")))?;

    let mut locked = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.get("id");
        let metrics = decode_metrics_map(row.get("metrics"));
        let properties = decode_json_object(row.get("properties"));
        locked.insert(
            id,
            LockedProdNode {
                metrics,
                properties,
            },
        );
    }
    Ok(locked)
}

fn detect_decision_conflicts(
    changes: &[DecisionChangeItem],
    tolerance_ratio: f64,
    locked_nodes: &HashMap<Uuid, LockedProdNode>,
) -> Vec<DecisionConflictItem> {
    let mut conflicts = Vec::new();
    for ch in changes {
        let Some(prod) = locked_nodes.get(&ch.node_id) else {
            conflicts.push(DecisionConflictItem {
                node_id: ch.node_id,
                metric: ch.metric.clone(),
                base_value: ch.base_value,
                current_value: None,
                target_value: ch.target_value,
                drift_ratio: 1.0,
                reason: String::from("TARGET_NODE_MISSING"),
            });
            continue;
        };
        let current_value = prod.metrics.get(&ch.metric).copied();
        match (ch.base_value, current_value) {
            (Some(base), Some(curr)) => {
                let denom = base.abs().max(1e-9);
                let drift = (curr - base).abs() / denom;
                if drift > tolerance_ratio {
                    conflicts.push(DecisionConflictItem {
                        node_id: ch.node_id,
                        metric: ch.metric.clone(),
                        base_value: Some(base),
                        current_value: Some(curr),
                        target_value: ch.target_value,
                        drift_ratio: drift,
                        reason: format!(
                            "metric '{}' drifted from {base} to {curr} (drift {drift:.6} > tolerance {tolerance_ratio})",
                            ch.metric
                        ),
                    });
                }
            }
            (None, Some(curr)) => {
                conflicts.push(DecisionConflictItem {
                    node_id: ch.node_id,
                    metric: ch.metric.clone(),
                    base_value: None,
                    current_value: Some(curr),
                    target_value: ch.target_value,
                    drift_ratio: 1.0,
                    reason: format!(
                        "metric '{}' was absent at baseline but now has value {curr}",
                        ch.metric
                    ),
                });
            }
            (Some(base), None) => {
                conflicts.push(DecisionConflictItem {
                    node_id: ch.node_id,
                    metric: ch.metric.clone(),
                    base_value: Some(base),
                    current_value: None,
                    target_value: ch.target_value,
                    drift_ratio: 1.0,
                    reason: format!(
                        "metric '{}' had baseline {base} but is now missing in production",
                        ch.metric
                    ),
                });
            }
            (None, None) => {}
        }
    }
    conflicts
}

async fn update_prod_nodes_for_decision_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    decision: &DecisionRecord,
    mut locked_nodes: HashMap<Uuid, LockedProdNode>,
) -> Result<(Vec<Uuid>, chrono::DateTime<Utc>), AppError> {
    let now = Utc::now();
    let now_rfc3339 = now.to_rfc3339();
    let mut touched_ids = BTreeSet::new();

    for ch in &decision.changes {
        if let Some(node) = locked_nodes.get_mut(&ch.node_id) {
            node.metrics.insert(ch.metric.clone(), ch.target_value);
            let mut ts_obj = match node.properties.remove("__metric_ts") {
                Some(serde_json::Value::Object(m)) => m,
                _ => serde_json::Map::new(),
            };
            ts_obj.insert(
                ch.metric.clone(),
                serde_json::Value::String(now_rfc3339.clone()),
            );
            node.properties.insert(
                String::from("__metric_ts"),
                serde_json::Value::Object(ts_obj),
            );
            touched_ids.insert(ch.node_id);
        }
    }

    for node_id in &touched_ids {
        let Some(node) = locked_nodes.get(node_id) else {
            continue;
        };
        let metrics_json = serde_json::to_value(&node.metrics).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize metrics failed: {err}"),
            )
        })?;
        let props_json = serde_json::Value::Object(node.properties.clone());
        sqlx::query(
            "UPDATE xb_nodes
                SET metrics = $2,
                    properties = $3,
                    row_version = row_version + 1,
                    updated_at = NOW()
              WHERE id = $1 AND tenant_id = $4",
        )
        .bind(*node_id)
        .bind(metrics_json)
        .bind(props_json)
        .bind(tenant_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("apply decision to xb_nodes failed: {err}")))?;
    }

    Ok((touched_ids.into_iter().collect(), now))
}

async fn append_decision_series_and_actions_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    decision: &DecisionRecord,
    ts: chrono::DateTime<Utc>,
) -> Result<(), AppError> {
    for (idx, ch) in decision.changes.iter().enumerate() {
        sqlx::query(
            "INSERT INTO xb_metric_series (
                 tenant_id, node_id, metric, ts, value, value_text, source_tag, batch_id
             )
             VALUES ($1, $2, $3, $4, $5, NULL, $6, $7)
             ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
                 value = EXCLUDED.value,
                 batch_id = EXCLUDED.batch_id",
        )
        .bind(tenant_id)
        .bind(ch.node_id)
        .bind(&ch.metric)
        .bind(ts)
        .bind(ch.target_value)
        .bind(MetricSourceTag::Action.as_str())
        .bind(decision.id)
        .execute(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!(
                "append xb_metric_series for SIM_APPLY failed: {err}"
            ))
        })?;

        let idem_key = format!("sim_decision:{}:{}:{idx}", decision.id, ch.node_id);
        let action_payload = serde_json::json!({
            "decision_id": decision.id,
            "scenario_id": decision.scenario_id,
            "node_id": ch.node_id,
            "metric": ch.metric,
            "base_value": ch.base_value,
            "target_value": ch.target_value,
        });
        sqlx::query(
            "INSERT INTO ax_action_log (
                 tenant_id, run_id, node_id, source_kind, action_kind,
                 target, payload, status, idempotency_key
             )
             VALUES ($1, NULL, NULL, 'SIM_APPLY', 'WRITE_NODE_METRIC', $2, $3, 'DONE', $4)
             ON CONFLICT (tenant_id, idempotency_key) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(format!("{}:{}", ch.node_id, ch.metric))
        .bind(action_payload)
        .bind(idem_key)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_action_log failed: {err}")))?;
    }
    Ok(())
}

async fn apply_decision_writeback_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    decision: &DecisionRecord,
    decided_by: Option<Uuid>,
    locked_nodes: HashMap<Uuid, LockedProdNode>,
) -> Result<(), AppError> {
    let (node_ids, ts) =
        update_prod_nodes_for_decision_tx(tx, tenant_id, decision, locked_nodes).await?;
    append_decision_series_and_actions_tx(tx, tenant_id, decision, ts).await?;

    insert_outbox_tx(
        tx,
        tenant_id,
        &NewOutboxEvent {
            topic: OutboxTopic::SimApplied,
            payload: serde_json::json!({
                "decision_id": decision.id,
                "scenario_id": decision.scenario_id,
                "node_ids": node_ids,
                "trigger_flow_id": decision.trigger_flow_id,
            }),
        },
    )
    .await?;

    if !node_ids.is_empty() {
        insert_outbox_tx(
            tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeMetricsChanged,
                payload: serde_json::json!({ "node_ids": node_ids }),
            },
        )
        .await?;
    }

    let audit_detail = serde_json::json!({
        "decision_id": decision.id,
        "scenario_id": decision.scenario_id,
        "node_ids": node_ids,
        "changes": decision.changes,
    });
    let actor_username = match decided_by {
        Some(uid) => sqlx::query("SELECT username FROM sec_users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("lookup sim actor username failed: {err}")))?
            .and_then(|r| r.try_get("username").ok())
            .unwrap_or_else(|| uid.to_string()),
        None => String::from("system"),
    };
    crate::audit::insert_chained_audit_tx(
        tx,
        &axpoint_domain::platform::AuditRecord {
            tenant_id: String::from(tenant_id),
            user_id: decided_by,
            username: actor_username,
            action: AuditAction::SimApply,
            resource_ref: format!("decision:{}", decision.id),
            status_code: 200,
            client_ip: String::new(),
            trace_id: String::new(),
            detail_json: audit_detail,
            created_at: Utc::now(),
        },
    )
    .await?;

    Ok(())
}

async fn reject_decision_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    decision: &DecisionRecord,
    decided_by: Option<Uuid>,
) -> Result<DecisionRecord, AppError> {
    sync_linked_approval_tx(tx, tenant_id, decision.approval_id, decided_by, "REJECTED").await?;
    let rejected_row = sqlx::query(
        "UPDATE ax_sim_decisions
            SET status = 'REJECTED',
                applied_by = $2
          WHERE id = $1 AND tenant_id = $3
      RETURNING id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by",
    )
    .bind(decision.id)
    .bind(decided_by)
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("reject decision failed: {err}")))?;
    decode_decision_row(&rejected_row)
}

async fn sync_linked_approval_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    approval_id: Option<Uuid>,
    decided_by: Option<Uuid>,
    next_status: &str,
) -> Result<(), AppError> {
    let Some(aid) = approval_id else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE ax_approvals
            SET status = $2,
                decided_by = COALESCE($3, decided_by),
                decided_at = COALESCE(decided_at, NOW())
          WHERE id = $1 AND tenant_id = $4 AND status = 'PENDING'",
    )
    .bind(aid)
    .bind(next_status)
    .bind(decided_by)
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("sync ax_approvals status failed: {err}")))?;
    Ok(())
}

impl SimRepository for PgStore {
    async fn list_sandboxes(&self, tenant_id: &str) -> Result<Vec<AxSandboxRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, name, status, scope, node_count, link_count, base_captured_at,
                    schema_version, created_by, actions, sim_result, created_at, updated_at
               FROM ax_sandboxes
              WHERE tenant_id = $1
              ORDER BY created_at DESC, id DESC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_sandboxes failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_sandboxes tx failed: {err}")))?;

        rows.iter().map(decode_sandbox_row).collect()
    }

    async fn get_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> Result<Option<AxSandboxRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT id, name, status, scope, node_count, link_count, base_captured_at,
                    schema_version, created_by, actions, sim_result, created_at, updated_at
               FROM ax_sandboxes
              WHERE id = $1 AND tenant_id = $2",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_sandboxes failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_sandbox tx failed: {err}")))?;

        row_opt.as_ref().map(decode_sandbox_row).transpose()
    }

    async fn create_sandbox(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxSandboxRequest,
    ) -> Result<AxSandboxRecord, AppError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "sandbox name cannot be empty",
            ));
        }
        let scope = req.scope.unwrap_or_default();
        validate_scope(&scope)?;
        let scope_json = serde_json::to_value(&scope).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid scope json: {err}"))
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let schema_version = current_main_schema_version_tx(&mut tx, tenant_id).await?;

        let sandbox_id: Uuid = sqlx::query_scalar(
            "INSERT INTO ax_sandboxes (
                 tenant_id, name, scope, status, schema_version, created_by
             )
             VALUES ($1, $2, $3, 'CLONING', $4, $5)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(name)
        .bind(scope_json)
        .bind(schema_version)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_sandboxes failed: {err}")))?;

        let (node_count, link_count) =
            clone_base_from_prod_tx(&mut tx, tenant_id, sandbox_id, &scope).await?;

        let row = sqlx::query(
            "UPDATE ax_sandboxes
                SET status = 'READY',
                    node_count = $2,
                    link_count = $3,
                    base_captured_at = NOW(),
                    updated_at = NOW()
              WHERE id = $1 AND tenant_id = $4
          RETURNING id, name, status, scope, node_count, link_count, base_captured_at,
                    schema_version, created_by, actions, sim_result, created_at, updated_at",
        )
        .bind(sandbox_id)
        .bind(node_count)
        .bind(link_count)
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("finalize ax_sandboxes failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_sandbox tx failed: {err}")))?;

        decode_sandbox_row(&row)
    }

    async fn delete_sandbox(&self, tenant_id: &str, sandbox_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM ax_sandboxes WHERE id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ax_sandboxes failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_sandbox tx failed: {err}")))?;
        Ok(res.rows_affected() > 0)
    }

    async fn load_graph(&self, tenant_id: &str, sandbox_id: Uuid) -> Result<SimGraph, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let merged_nodes = load_sandbox_nodes_merged_tx(&mut tx, tenant_id, sandbox_id).await?;
        let active_ids: BTreeSet<Uuid> = merged_nodes.iter().map(|(n, _)| n.id).collect();
        let merged_links =
            load_sandbox_links_merged_tx(&mut tx, tenant_id, sandbox_id, &active_ids).await?;
        let edge_specs = load_edge_specs_tx(&mut tx, tenant_id).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit load_graph tx failed: {err}")))?;

        let nodes = merged_nodes
            .into_iter()
            .map(|(n, row_version)| SimNode {
                id: n.id,
                type_name: n.type_name,
                node_key: n.node_key,
                title: n.title,
                metrics: n.metrics,
                properties: n.properties,
                row_version,
            })
            .collect();
        let links = merged_links
            .into_iter()
            .map(|l| SimLink {
                id: l.id,
                link_type: l.link_type,
                src_node_id: l.src_node_id,
                dst_node_id: l.dst_node_id,
                weight: l.weight,
            })
            .collect();

        Ok(SimGraph {
            nodes,
            links,
            edge_specs,
        })
    }

    async fn get_sandbox_graph(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> Result<SandboxGraphResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let merged_nodes = load_sandbox_nodes_merged_tx(&mut tx, tenant_id, sandbox_id).await?;
        let nodes: Vec<SandboxGraphNode> = merged_nodes.into_iter().map(|(n, _)| n).collect();
        let active_ids: BTreeSet<Uuid> = nodes.iter().map(|n| n.id).collect();
        let links =
            load_sandbox_links_merged_tx(&mut tx, tenant_id, sandbox_id, &active_ids).await?;
        let overlay_hash = compute_overlay_hash_tx(&mut tx, tenant_id, sandbox_id).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_sandbox_graph tx failed: {err}"))
        })?;

        Ok(SandboxGraphResponse {
            sandbox_id,
            nodes,
            links,
            overlay_hash,
        })
    }

    async fn compute_overlay_hash(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> Result<String, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;
        let hash = compute_overlay_hash_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit compute_overlay_hash tx failed: {err}"))
        })?;
        Ok(hash)
    }

    async fn patch_overlay_node(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        node_id: Uuid,
        req: PatchOverlayNodeRequest,
    ) -> Result<SandboxGraphNode, AppError> {
        if let Some(ref title) = req.title
            && title.trim().is_empty()
        {
            return Err(AppError::bad("INVALID_ARGUMENT", "title cannot be empty"));
        }
        if let Some(ref metrics) = req.metrics {
            validate_finite_metrics(metrics)?;
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let current = load_effective_node_tx(&mut tx, tenant_id, sandbox_id, node_id)
            .await?
            .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

        if req.title.is_none() && req.metrics.is_none() && req.properties.is_none() {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop patch_overlay_node tx failed: {err}"))
            })?;
            return Ok(current);
        }

        upsert_overlay_node_patch_tx(&mut tx, tenant_id, sandbox_id, node_id, &req).await?;
        let updated = load_effective_node_tx(&mut tx, tenant_id, sandbox_id, node_id)
            .await?
            .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit patch_overlay_node tx failed: {err}"))
        })?;
        Ok(updated)
    }

    async fn create_overlay_node(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        req: CreateOverlayNodeRequest,
    ) -> Result<SandboxGraphNode, AppError> {
        let type_name = req.type_name.trim();
        let node_key = req.node_key.trim();
        let title = req.title.trim();
        if type_name.is_empty() || node_key.is_empty() || title.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "type_name, node_key, and title must be non-empty",
            ));
        }
        validate_finite_metrics(&req.metrics)?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let node_id = Uuid::new_v4();
        let metrics_json = serde_json::to_value(&req.metrics).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize metrics: {err}"))
        })?;
        let props_json = serde_json::Value::Object(req.properties.clone());

        sqlx::query(
            "INSERT INTO ax_sim_overlay_nodes (
                 tenant_id, sandbox_id, node_id, type_name, node_key, title,
                 metrics, properties, is_new, deleted, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, FALSE, NOW())",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(node_id)
        .bind(type_name)
        .bind(node_key)
        .bind(title)
        .bind(metrics_json)
        .bind(props_json)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert overlay node failed: {err}")))?;

        let _counts = refresh_sandbox_counts_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_overlay_node tx failed: {err}"))
        })?;

        Ok(SandboxGraphNode {
            id: node_id,
            type_name: String::from(type_name),
            node_key: String::from(node_key),
            title: String::from(title),
            metrics: req.metrics,
            properties: req.properties,
            is_modified: false,
            is_new: true,
        })
    }

    async fn delete_overlay_node(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        node_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let Some(existing) =
            load_effective_node_tx(&mut tx, tenant_id, sandbox_id, node_id).await?
        else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit missing delete_overlay_node tx: {err}"))
            })?;
            return Ok(false);
        };

        if existing.is_new {
            sqlx::query(
                "DELETE FROM ax_sim_overlay_nodes
                  WHERE sandbox_id = $1 AND tenant_id = $2 AND node_id = $3",
            )
            .bind(sandbox_id)
            .bind(tenant_id)
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete new overlay node failed: {err}")))?;
        } else {
            sqlx::query(
                "INSERT INTO ax_sim_overlay_nodes (
                     tenant_id, sandbox_id, node_id, is_new, deleted, updated_at
                 )
                 VALUES ($1, $2, $3, FALSE, TRUE, NOW())
                 ON CONFLICT (sandbox_id, node_id) DO UPDATE SET
                     deleted = TRUE,
                     updated_at = NOW()",
            )
            .bind(tenant_id)
            .bind(sandbox_id)
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("tombstone overlay node failed: {err}")))?;
        }

        sqlx::query(
            "DELETE FROM ax_sim_overlay_links
              WHERE sandbox_id = $1 AND tenant_id = $2 AND is_new = TRUE
                AND (src_node_id = $3 OR dst_node_id = $3)",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .bind(node_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete incident new links failed: {err}")))?;

        sqlx::query(
            "INSERT INTO ax_sim_overlay_links (
                 tenant_id, sandbox_id, link_id, is_new, deleted, updated_at
             )
             SELECT $1, $2, link_id, FALSE, TRUE, NOW()
               FROM ax_sim_base_links
              WHERE sandbox_id = $2 AND tenant_id = $1
                AND (src_node_id = $3 OR dst_node_id = $3)
             ON CONFLICT (sandbox_id, link_id) DO UPDATE SET
                 deleted = TRUE,
                 updated_at = NOW()",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(node_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("tombstone incident base links failed: {err}"))
        })?;

        let _counts = refresh_sandbox_counts_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_overlay_node tx failed: {err}"))
        })?;
        Ok(true)
    }

    async fn create_overlay_link(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        req: CreateOverlayLinkRequest,
    ) -> Result<SandboxGraphLink, AppError> {
        let link_type = req.link_type.trim();
        if link_type.is_empty() || !req.weight.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "link_type must be non-empty and weight must be finite",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        if load_effective_node_tx(&mut tx, tenant_id, sandbox_id, req.src_node_id)
            .await?
            .is_none()
            || load_effective_node_tx(&mut tx, tenant_id, sandbox_id, req.dst_node_id)
                .await?
                .is_none()
        {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }

        let link_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ax_sim_overlay_links (
                 tenant_id, sandbox_id, link_id, link_type, src_node_id, dst_node_id,
                 weight, is_new, deleted, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, FALSE, NOW())",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(link_id)
        .bind(link_type)
        .bind(req.src_node_id)
        .bind(req.dst_node_id)
        .bind(req.weight)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert overlay link failed: {err}")))?;

        let _counts = refresh_sandbox_counts_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_overlay_link tx failed: {err}"))
        })?;

        Ok(SandboxGraphLink {
            id: link_id,
            link_type: String::from(link_type),
            src_node_id: req.src_node_id,
            dst_node_id: req.dst_node_id,
            weight: req.weight,
            is_modified: false,
            is_new: true,
        })
    }

    async fn patch_overlay_link(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        link_id: Uuid,
        req: PatchOverlayLinkRequest,
    ) -> Result<SandboxGraphLink, AppError> {
        if let Some(ref lt) = req.link_type
            && lt.trim().is_empty()
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "link_type cannot be empty",
            ));
        }
        if let Some(w) = req.weight
            && !w.is_finite()
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "link weight must be finite",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let current = load_effective_link_tx(&mut tx, tenant_id, sandbox_id, link_id)
            .await?
            .ok_or(AppError::NotFound("LINK_NOT_FOUND"))?;

        if req.link_type.is_none() && req.weight.is_none() {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop patch_overlay_link tx: {err}"))
            })?;
            return Ok(current);
        }

        let next_type = req.link_type.as_deref().map(str::trim).map(String::from);
        sqlx::query(
            "INSERT INTO ax_sim_overlay_links (
                 tenant_id, sandbox_id, link_id, link_type, weight, is_new, deleted, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, FALSE, NOW())
             ON CONFLICT (sandbox_id, link_id) DO UPDATE SET
                 link_type = COALESCE(EXCLUDED.link_type, ax_sim_overlay_links.link_type),
                 weight = COALESCE(EXCLUDED.weight, ax_sim_overlay_links.weight),
                 deleted = FALSE,
                 updated_at = NOW()",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(link_id)
        .bind(next_type)
        .bind(req.weight)
        .bind(current.is_new)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert ax_sim_overlay_links failed: {err}")))?;

        sqlx::query("UPDATE ax_sandboxes SET updated_at = NOW() WHERE id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("touch ax_sandboxes failed: {err}")))?;

        let updated = load_effective_link_tx(&mut tx, tenant_id, sandbox_id, link_id)
            .await?
            .ok_or(AppError::NotFound("LINK_NOT_FOUND"))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit patch_overlay_link tx failed: {err}"))
        })?;
        Ok(updated)
    }

    async fn delete_overlay_link(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        link_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        let Some(existing) =
            load_effective_link_tx(&mut tx, tenant_id, sandbox_id, link_id).await?
        else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit missing delete_overlay_link tx: {err}"))
            })?;
            return Ok(false);
        };

        if existing.is_new {
            sqlx::query(
                "DELETE FROM ax_sim_overlay_links
                  WHERE sandbox_id = $1 AND tenant_id = $2 AND link_id = $3",
            )
            .bind(sandbox_id)
            .bind(tenant_id)
            .bind(link_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete new overlay link failed: {err}")))?;
        } else {
            sqlx::query(
                "INSERT INTO ax_sim_overlay_links (
                     tenant_id, sandbox_id, link_id, is_new, deleted, updated_at
                 )
                 VALUES ($1, $2, $3, FALSE, TRUE, NOW())
                 ON CONFLICT (sandbox_id, link_id) DO UPDATE SET
                     deleted = TRUE,
                     updated_at = NOW()",
            )
            .bind(tenant_id)
            .bind(sandbox_id)
            .bind(link_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("tombstone overlay link failed: {err}")))?;
        }

        let _counts = refresh_sandbox_counts_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_overlay_link tx failed: {err}"))
        })?;
        Ok(true)
    }

    async fn reset_overlay(&self, tenant_id: &str, sandbox_id: Uuid) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;

        sqlx::query("DELETE FROM ax_sim_overlay_links WHERE sandbox_id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("reset overlay links failed: {err}")))?;

        sqlx::query("DELETE FROM ax_sim_overlay_nodes WHERE sandbox_id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("reset overlay nodes failed: {err}")))?;

        let _counts = refresh_sandbox_counts_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit reset_overlay tx failed: {err}")))?;
        Ok(())
    }

    async fn refresh_sandbox_base(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> Result<RefreshBaseResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let scope_val: serde_json::Value = sqlx::query_scalar(
            "SELECT scope FROM ax_sandboxes WHERE id = $1 AND tenant_id = $2 FOR UPDATE",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock ax_sandboxes for refresh: {err}")))?
        .ok_or(AppError::NotFound("SANDBOX_NOT_FOUND"))?;

        let scope: SandboxScope = serde_json::from_value(scope_val).unwrap_or_default();
        let schema_version = current_main_schema_version_tx(&mut tx, tenant_id).await?;

        sqlx::query("DELETE FROM ax_sim_base_links WHERE sandbox_id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("clear ax_sim_base_links failed: {err}")))?;

        sqlx::query("DELETE FROM ax_sim_base_nodes WHERE sandbox_id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("clear ax_sim_base_nodes failed: {err}")))?;

        let _cloned = clone_base_from_prod_tx(&mut tx, tenant_id, sandbox_id, &scope).await?;
        let (node_count, link_count) =
            refresh_sandbox_counts_tx(&mut tx, tenant_id, sandbox_id).await?;

        let orphaned_node_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT o.node_id
               FROM ax_sim_overlay_nodes o
              WHERE o.sandbox_id = $1
                AND o.tenant_id = $2
                AND o.is_new = FALSE
                AND NOT EXISTS (
                    SELECT 1 FROM ax_sim_base_nodes b
                     WHERE b.sandbox_id = o.sandbox_id AND b.node_id = o.node_id
                )
              ORDER BY o.node_id ASC",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("detect orphaned overlay nodes failed: {err}"))
        })?;

        let orphaned_link_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT o.link_id
               FROM ax_sim_overlay_links o
              WHERE o.sandbox_id = $1
                AND o.tenant_id = $2
                AND o.is_new = FALSE
                AND NOT EXISTS (
                    SELECT 1 FROM ax_sim_base_links b
                     WHERE b.sandbox_id = o.sandbox_id AND b.link_id = o.link_id
                )
              ORDER BY o.link_id ASC",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("detect orphaned overlay links failed: {err}"))
        })?;

        let sb_row = sqlx::query(
            "UPDATE ax_sandboxes
                SET base_captured_at = NOW(),
                    schema_version = $2,
                    node_count = $3,
                    link_count = $4,
                    updated_at = NOW()
              WHERE id = $1 AND tenant_id = $5
          RETURNING id, name, status, scope, node_count, link_count, base_captured_at,
                    schema_version, created_by, actions, sim_result, created_at, updated_at",
        )
        .bind(sandbox_id)
        .bind(schema_version)
        .bind(node_count)
        .bind(link_count)
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update ax_sandboxes on refresh: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit refresh_sandbox_base tx failed: {err}"))
        })?;

        Ok(RefreshBaseResponse {
            sandbox: decode_sandbox_row(&sb_row)?,
            orphaned_node_ids,
            orphaned_link_ids,
        })
    }

    async fn quick_simulate_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        req: SimulateAxSandboxRequest,
    ) -> Result<AxSimResult, AppError> {
        let graph = self.load_graph(tenant_id, sandbox_id).await?;
        let params = req.params.unwrap_or_default();
        let sim_res = if params.uncertainty.is_some() {
            axpoint_domain::sim::uncertainty::solve(&graph, &req.actions, &params)?
        } else if params.mode == SimMode::Stepped {
            axpoint_domain::sim::stepped::solve(&graph, &req.actions, &params)?
        } else {
            axpoint_domain::sim::steady::solve(&graph, &req.actions, &params)?
        };

        let impacts = sim_res
            .impacts
            .into_iter()
            .map(|r| AxSimImpactRow {
                node_id: r.node_id,
                type_name: r.type_name,
                title: r.title,
                metric_key: r.metric,
                before_val: r.before_val.unwrap_or(0.0),
                after_val: r.after_val,
                delta: r.delta.unwrap_or(0.0),
                delta_ratio: r.delta_ratio,
                hop_distance: r.hop_distance,
                top_path: r.top_path,
            })
            .collect();

        let ax_res = AxSimResult {
            impacts,
            affected_count: sim_res.affected_count,
            kpis: sim_res.kpis,
            warnings: sim_res.warnings,
            converged: sim_res.converged,
        };

        let actions_json = serde_json::to_value(&req.actions)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid actions: {err}")))?;
        let res_json = serde_json::to_value(&ax_res).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid sim result: {err}"))
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let updated = sqlx::query(
            "UPDATE ax_sandboxes
                SET actions = $1,
                    sim_result = $2,
                    updated_at = NOW()
              WHERE id = $3 AND tenant_id = $4",
        )
        .bind(actions_json)
        .bind(res_json)
        .bind(sandbox_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("save quick_simulate_sandbox failed: {err}")))?;

        if updated.rows_affected() == 0 {
            return Err(AppError::NotFound("SANDBOX_NOT_FOUND"));
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit quick_simulate_sandbox tx failed: {err}"))
        })?;
        Ok(ax_res)
    }

    async fn list_scenarios(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
    ) -> Result<Vec<ScenarioRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;
        let current_hash = compute_overlay_hash_tx(&mut tx, tenant_id, sandbox_id).await?;

        let rows = sqlx::query(
            "SELECT id, sandbox_id, name, description, actions, params, overlay_hash,
                    result, status, error_message, created_by, created_at, finished_at
               FROM ax_sim_scenarios
              WHERE sandbox_id = $1 AND tenant_id = $2
              ORDER BY created_at DESC, id DESC",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_sim_scenarios failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_scenarios tx failed: {err}")))?;

        rows.iter()
            .map(|r| decode_scenario_row(r, &current_hash))
            .collect()
    }

    async fn get_scenario(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> Result<Option<ScenarioRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT id, sandbox_id, name, description, actions, params, overlay_hash,
                    result, status, error_message, created_by, created_at, finished_at
               FROM ax_sim_scenarios
              WHERE id = $1 AND tenant_id = $2",
        )
        .bind(scenario_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_sim_scenarios failed: {err}")))?;

        let Some(row) = row_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit missing get_scenario tx: {err}"))
            })?;
            return Ok(None);
        };

        let sandbox_id: Uuid = row.get("sandbox_id");
        let current_hash = compute_overlay_hash_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_scenario tx failed: {err}")))?;

        Ok(Some(decode_scenario_row(&row, &current_hash)?))
    }

    async fn create_scenario_record(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        created_by: Option<Uuid>,
        overlay_hash: &str,
        req: CreateScenarioRequest,
    ) -> Result<ScenarioRecord, AppError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "scenario name cannot be empty",
            ));
        }
        let actions_json = serde_json::to_value(&req.actions)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid actions: {err}")))?;
        let params_json = serde_json::to_value(&req.params)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid params: {err}")))?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_sandbox_exists_tx(&mut tx, tenant_id, sandbox_id).await?;
        let current_hash = compute_overlay_hash_tx(&mut tx, tenant_id, sandbox_id).await?;

        let row = sqlx::query(
            "INSERT INTO ax_sim_scenarios (
                 tenant_id, sandbox_id, name, description, actions, params,
                 overlay_hash, status, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'PENDING', $8)
         RETURNING id, sandbox_id, name, description, actions, params, overlay_hash,
                   result, status, error_message, created_by, created_at, finished_at",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(name)
        .bind(req.description.trim())
        .bind(actions_json)
        .bind(params_json)
        .bind(overlay_hash)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("NAME_TAKEN")
            } else {
                AppError::Database(format!("insert ax_sim_scenarios failed: {err}"))
            }
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_scenario_record tx failed: {err}"))
        })?;

        decode_scenario_row(&row, &current_hash)
    }

    async fn mark_scenario_running(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        overlay_hash: &str,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE ax_sim_scenarios
                SET status = 'RUNNING',
                    overlay_hash = $2,
                    error_message = NULL,
                    finished_at = NULL
              WHERE id = $1 AND tenant_id = $3",
        )
        .bind(scenario_id)
        .bind(overlay_hash)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark scenario RUNNING failed: {err}")))?;

        if res.rows_affected() == 0 {
            return Err(AppError::NotFound("SCENARIO_NOT_FOUND"));
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit mark_scenario_running tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn save_scenario_success(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        overlay_hash: &str,
        result: &SimResult,
    ) -> Result<ScenarioRecord, AppError> {
        let mut top_impacts = result.impacts.clone();
        top_impacts.truncate(TOP_SUMMARY_IMPACTS);
        let summary = ScenarioSummary {
            affected_count: result.affected_count,
            kpis: result.kpis.clone(),
            top_impacts,
            warnings: result.warnings.clone(),
            converged: result.converged,
            reason: result.reason.clone(),
            iterations: result.iterations,
        };
        let summary_json = serde_json::to_value(&summary).map_err(|err| {
            AppError::Database(format!("serialize ScenarioSummary failed: {err}"))
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "UPDATE ax_sim_scenarios
                SET status = 'SUCCEEDED',
                    overlay_hash = $2,
                    result = $3,
                    error_message = NULL,
                    finished_at = NOW()
              WHERE id = $1 AND tenant_id = $4
          RETURNING id, sandbox_id, name, description, actions, params, overlay_hash,
                    result, status, error_message, created_by, created_at, finished_at",
        )
        .bind(scenario_id)
        .bind(overlay_hash)
        .bind(summary_json)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("save scenario success failed: {err}")))?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;

        sqlx::query("DELETE FROM ax_sim_impacts WHERE scenario_id = $1 AND tenant_id = $2")
            .bind(scenario_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("clear ax_sim_impacts failed: {err}")))?;

        sqlx::query("DELETE FROM ax_sim_checkpoints WHERE scenario_id = $1 AND tenant_id = $2")
            .bind(scenario_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("clear ax_sim_checkpoints failed: {err}")))?;

        batch_insert_impacts_tx(&mut tx, tenant_id, scenario_id, &result.impacts).await?;
        batch_insert_checkpoints_tx(&mut tx, tenant_id, scenario_id, &result.checkpoints).await?;

        let sandbox_id: Uuid = row.get("sandbox_id");
        let current_hash = compute_overlay_hash_tx(&mut tx, tenant_id, sandbox_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit save_scenario_success tx failed: {err}"))
        })?;

        decode_scenario_row(&row, &current_hash)
    }

    async fn save_scenario_failure(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        error_message: &str,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE ax_sim_scenarios
                SET status = 'FAILED',
                    error_message = $2,
                    finished_at = NOW()
              WHERE id = $1 AND tenant_id = $3",
        )
        .bind(scenario_id)
        .bind(error_message)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("save scenario failure failed: {err}")))?;

        if res.rows_affected() == 0 {
            return Err(AppError::NotFound("SCENARIO_NOT_FOUND"));
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit save_scenario_failure tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn delete_scenario(&self, tenant_id: &str, scenario_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM ax_sim_scenarios WHERE id = $1 AND tenant_id = $2")
            .bind(scenario_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ax_sim_scenarios failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_scenario tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn list_scenario_impacts(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        query: ImpactListQuery,
    ) -> Result<PaginatedList<ImpactRow>, AppError> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=500, got {limit}"),
            ));
        }
        let offset: i64 = match query
            .cursor
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => {
                let parsed = raw.parse::<i64>().map_err(|_| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        "impact cursor must be a non-negative integer",
                    )
                })?;
                if parsed < 0 {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "impact cursor must be >= 0",
                    ));
                }
                parsed
            }
            None => 0,
        };

        let type_filter = query
            .type_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let metric_filter = query
            .metric
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let limit_usize = usize::try_from(limit).unwrap_or(50);
        let fetch_limit = i64::from(limit).saturating_add(1);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let sandbox_id: Uuid = sqlx::query_scalar(
            "SELECT sandbox_id FROM ax_sim_scenarios WHERE id = $1 AND tenant_id = $2",
        )
        .bind(scenario_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lookup scenario sandbox failed: {err}")))?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;

        let rows = sqlx::query(
            "SELECT i.node_id,
                    COALESCE(o.type_name, b.type_name, '') AS type_name,
                    COALESCE(o.title, b.title, '') AS title,
                    i.metric, i.before_val, i.after_val, i.delta, i.delta_ratio,
                    i.p10, i.p90, i.hop_distance, i.top_path
               FROM ax_sim_impacts i
               LEFT JOIN ax_sim_base_nodes b
                 ON b.sandbox_id = $2 AND b.node_id = i.node_id
               LEFT JOIN ax_sim_overlay_nodes o
                 ON o.sandbox_id = $2 AND o.node_id = i.node_id
              WHERE i.scenario_id = $1
                AND i.tenant_id = $3
                AND ($4::text IS NULL OR COALESCE(o.type_name, b.type_name, '') = $4)
                AND ($5::text IS NULL OR i.metric = $5)
              ORDER BY COALESCE(ABS(i.delta), 0.0) DESC, i.hop_distance ASC, i.node_id ASC, i.metric ASC
              LIMIT $6 OFFSET $7",
        )
        .bind(scenario_id)
        .bind(sandbox_id)
        .bind(tenant_id)
        .bind(type_filter)
        .bind(metric_filter)
        .bind(fetch_limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_sim_impacts failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_scenario_impacts tx failed: {err}"))
        })?;

        let mut items: Vec<ImpactRow> = rows
            .iter()
            .map(decode_impact_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            Some(offset.saturating_add(i64::from(limit)).to_string())
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn list_all_scenario_impacts(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> Result<Vec<ImpactRow>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let sandbox_id: Uuid = sqlx::query_scalar(
            "SELECT sandbox_id FROM ax_sim_scenarios WHERE id = $1 AND tenant_id = $2",
        )
        .bind(scenario_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lookup scenario sandbox failed: {err}")))?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;

        let rows = sqlx::query(
            "SELECT i.node_id,
                    COALESCE(o.type_name, b.type_name, '') AS type_name,
                    COALESCE(o.title, b.title, '') AS title,
                    i.metric, i.before_val, i.after_val, i.delta, i.delta_ratio,
                    i.p10, i.p90, i.hop_distance, i.top_path
               FROM ax_sim_impacts i
               LEFT JOIN ax_sim_base_nodes b
                 ON b.sandbox_id = $2 AND b.node_id = i.node_id
               LEFT JOIN ax_sim_overlay_nodes o
                 ON o.sandbox_id = $2 AND o.node_id = i.node_id
              WHERE i.scenario_id = $1 AND i.tenant_id = $3
              ORDER BY COALESCE(ABS(i.delta), 0.0) DESC, i.hop_distance ASC, i.node_id ASC, i.metric ASC",
        )
        .bind(scenario_id)
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list all ax_sim_impacts failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_all_scenario_impacts tx failed: {err}"))
        })?;

        rows.iter().map(decode_impact_row).collect()
    }

    async fn list_scenario_checkpoints(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
    ) -> Result<Vec<Checkpoint>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ax_sim_scenarios WHERE id = $1 AND tenant_id = $2)",
        )
        .bind(scenario_id)
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("check scenario exists failed: {err}")))?;
        if !exists {
            return Err(AppError::NotFound("SCENARIO_NOT_FOUND"));
        }

        let rows = sqlx::query(
            "SELECT seq, label, changes
               FROM ax_sim_checkpoints
              WHERE scenario_id = $1 AND tenant_id = $2
              ORDER BY seq ASC",
        )
        .bind(scenario_id)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_sim_checkpoints failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_scenario_checkpoints tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| Checkpoint {
                seq: r.get("seq"),
                label: r.get("label"),
                changes: r.get("changes"),
            })
            .collect())
    }

    async fn create_decision(
        &self,
        tenant_id: &str,
        scenario_id: Uuid,
        created_by: Option<Uuid>,
        req: CreateDecisionRequest,
    ) -> Result<DecisionRecord, AppError> {
        let title = req.title.trim();
        if title.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "decision title cannot be empty",
            ));
        }
        if !req.tolerance_ratio.is_finite() || req.tolerance_ratio < 0.0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "tolerance_ratio must be a finite number >= 0.0",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let sc_row = sqlx::query(
            "SELECT sandbox_id, actions FROM ax_sim_scenarios WHERE id = $1 AND tenant_id = $2",
        )
        .bind(scenario_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load scenario for decision failed: {err}")))?
        .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;

        if let Some(flow_id) = req.trigger_flow_id {
            let flow_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM ax_flows WHERE id = $1 AND tenant_id = $2)",
            )
            .bind(flow_id)
            .bind(tenant_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("check trigger_flow_id failed: {err}")))?;
            if !flow_exists {
                return Err(AppError::NotFound("FLOW_NOT_FOUND"));
            }
        }

        let sandbox_id: Uuid = sc_row.get("sandbox_id");
        let actions: Vec<AxSimAction> = serde_json::from_value(sc_row.get("actions"))
            .map_err(|err| AppError::Database(format!("decode scenario actions: {err}")))?;
        let changes =
            resolve_decision_changes_tx(&mut tx, tenant_id, sandbox_id, req.changes, &actions)
                .await?;

        let decision_id = Uuid::new_v4();
        let approval_id = insert_sim_approval_tx(
            &mut tx,
            tenant_id,
            decision_id,
            scenario_id,
            title,
            &changes,
        )
        .await?;
        let changes_json = serde_json::to_value(&changes).map_err(|err| {
            AppError::Database(format!("serialize DecisionChangeItem failed: {err}"))
        })?;

        let row = sqlx::query(
            "INSERT INTO ax_sim_decisions (
                 id, tenant_id, scenario_id, title, changes, trigger_flow_id,
                 status, approval_id, tolerance_ratio, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, 'PENDING_APPROVAL', $7, $8, $9)
         RETURNING id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                   conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by",
        )
        .bind(decision_id)
        .bind(tenant_id)
        .bind(scenario_id)
        .bind(title)
        .bind(changes_json)
        .bind(req.trigger_flow_id)
        .bind(approval_id)
        .bind(req.tolerance_ratio)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_sim_decisions failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_decision tx failed: {err}"))
        })?;
        decode_decision_row(&row)
    }

    async fn get_decision(
        &self,
        tenant_id: &str,
        decision_id: Uuid,
    ) -> Result<Option<DecisionRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                    conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by
               FROM ax_sim_decisions
              WHERE id = $1 AND tenant_id = $2",
        )
        .bind(decision_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_sim_decisions failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_decision tx failed: {err}")))?;

        row_opt.as_ref().map(decode_decision_row).transpose()
    }

    async fn apply_or_reject_decision(
        &self,
        tenant_id: &str,
        decision_id: Uuid,
        decided_by: Option<Uuid>,
        approved: bool,
    ) -> Result<DecisionRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let locked_row = sqlx::query(
            "SELECT id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                    conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by
               FROM ax_sim_decisions
              WHERE id = $1 AND tenant_id = $2
              FOR UPDATE",
        )
        .bind(decision_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock ax_sim_decisions failed: {err}")))?
        .ok_or(AppError::NotFound("DECISION_NOT_FOUND"))?;

        let decision = decode_decision_row(&locked_row)?;
        if !matches!(
            decision.status,
            DecisionStatus::PendingApproval | DecisionStatus::Approved
        ) {
            return Err(AppError::Conflict("CONFLICT"));
        }

        if !approved {
            let rejected = reject_decision_tx(&mut tx, tenant_id, &decision, decided_by).await?;
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit reject decision tx failed: {err}"))
            })?;
            return Ok(rejected);
        }

        sync_linked_approval_tx(
            &mut tx,
            tenant_id,
            decision.approval_id,
            decided_by,
            "APPROVED",
        )
        .await?;

        let locked_nodes =
            lock_prod_nodes_for_decision_tx(&mut tx, tenant_id, &decision.changes).await?;
        let conflicts =
            detect_decision_conflicts(&decision.changes, decision.tolerance_ratio, &locked_nodes);

        if !conflicts.is_empty() {
            let conflicts_json = serde_json::to_value(&conflicts).map_err(|err| {
                AppError::Database(format!("serialize DecisionConflictItem failed: {err}"))
            })?;
            let conflict_row = sqlx::query(
                "UPDATE ax_sim_decisions
                    SET status = 'CONFLICT',
                        conflict_report = $2
                  WHERE id = $1 AND tenant_id = $3
              RETURNING id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                        conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by",
            )
            .bind(decision_id)
            .bind(conflicts_json)
            .bind(tenant_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("mark decision CONFLICT failed: {err}")))?;
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit conflict decision tx failed: {err}"))
            })?;
            return decode_decision_row(&conflict_row);
        }

        apply_decision_writeback_tx(&mut tx, tenant_id, &decision, decided_by, locked_nodes)
            .await?;

        let applied_row = sqlx::query(
            "UPDATE ax_sim_decisions
                SET status = 'APPLIED',
                    conflict_report = '[]'::jsonb,
                    applied_at = NOW(),
                    applied_by = $2
              WHERE id = $1 AND tenant_id = $3
          RETURNING id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                    conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by",
        )
        .bind(decision_id)
        .bind(decided_by)
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark decision APPLIED failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit apply decision tx failed: {err}")))?;
        decode_decision_row(&applied_row)
    }

    async fn rebase_decision(
        &self,
        tenant_id: &str,
        decision_id: Uuid,
        _actor_user_id: Option<Uuid>,
    ) -> Result<DecisionRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let locked_row = sqlx::query(
            "SELECT id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                    conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by
               FROM ax_sim_decisions
              WHERE id = $1 AND tenant_id = $2
              FOR UPDATE",
        )
        .bind(decision_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock decision for rebase failed: {err}")))?
        .ok_or(AppError::NotFound("DECISION_NOT_FOUND"))?;

        let mut decision = decode_decision_row(&locked_row)?;
        if !matches!(
            decision.status,
            DecisionStatus::Conflict | DecisionStatus::PendingApproval
        ) {
            return Err(AppError::Conflict("CONFLICT"));
        }

        let prod_nodes =
            lock_prod_nodes_for_decision_tx(&mut tx, tenant_id, &decision.changes).await?;
        for ch in &mut decision.changes {
            let prod = prod_nodes
                .get(&ch.node_id)
                .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
            ch.base_value = prod.metrics.get(&ch.metric).copied();
        }

        let new_approval_id = insert_sim_approval_tx(
            &mut tx,
            tenant_id,
            decision.id,
            decision.scenario_id,
            &decision.title,
            &decision.changes,
        )
        .await?;
        let changes_json = serde_json::to_value(&decision.changes).map_err(|err| {
            AppError::Database(format!("serialize rebased changes failed: {err}"))
        })?;

        let updated_row = sqlx::query(
            "UPDATE ax_sim_decisions
                SET changes = $2,
                    status = 'PENDING_APPROVAL',
                    approval_id = $3,
                    conflict_report = '[]'::jsonb,
                    applied_at = NULL,
                    applied_by = NULL
              WHERE id = $1 AND tenant_id = $4
          RETURNING id, scenario_id, title, changes, trigger_flow_id, status, approval_id,
                    conflict_report, tolerance_ratio, created_by, created_at, applied_at, applied_by",
        )
        .bind(decision_id)
        .bind(changes_json)
        .bind(new_approval_id)
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update rebased decision failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit rebase_decision tx failed: {err}"))
        })?;
        decode_decision_row(&updated_row)
    }
}
