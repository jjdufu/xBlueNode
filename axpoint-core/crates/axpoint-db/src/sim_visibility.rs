//! State-driven row-level visibility and property masking enforcement for `AxSim` sandboxes (`05-STEP27` §27.3).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use axpoint_domain::AppError;
use axpoint_domain::graph::TraversalDirection;
use axpoint_domain::guard::{NodeTypePolicySpec, PolicyContext};
use axpoint_domain::ports::{SimPreflightPort as _, SimRepository as _, SimVisibilityPort};
use axpoint_domain::sim::{
    DEFAULT_SANDBOX_MAX_NODES, MAX_SANDBOX_SCOPE_DEPTH, RefreshBaseResponse, SandboxGraphNode,
    SandboxGraphResponse, SandboxScope, SandboxStatus, ScopeBoundaryEdgeGroup,
    ScopeInducedEdgeGroup, ScopePreflightSlice,
};
use axpoint_domain::studio::LinkDirection;
use axpoint_domain::topology::{AxSandboxRecord, AxSimAction, AxSimResult, CreateAxSandboxRequest};
use axpoint_security::masking::{MaskKeyRing, mask_json_properties};
use sqlx::{Postgres, QueryBuilder, Row as _, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn has_any_row_filter(policies: &BTreeMap<String, NodeTypePolicySpec>) -> bool {
    policies.values().any(|spec| spec.pg_row_filter.is_some())
}

fn has_any_property_masks(policies: &BTreeMap<String, NodeTypePolicySpec>) -> bool {
    policies
        .values()
        .any(|spec| !spec.property_masks.is_empty())
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

fn lookup_node_policy<'a>(
    policies: &'a BTreeMap<String, NodeTypePolicySpec>,
    type_name: &str,
) -> Option<&'a NodeTypePolicySpec> {
    policies.get(type_name).or_else(|| {
        policies
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(type_name))
            .map(|(_, v)| v)
    })
}

fn append_policy_case_for_alias(
    qb: &mut QueryBuilder<Postgres>,
    col_prefix: &str,
    policies: &BTreeMap<String, NodeTypePolicySpec>,
) {
    let active: Vec<(&String, &String)> = policies
        .iter()
        .filter_map(|(tname, spec)| spec.pg_row_filter.as_ref().map(|pred| (tname, pred)))
        .collect();
    if active.is_empty() {
        return;
    }
    qb.push(" AND (CASE");
    for (tname, pred) in active {
        qb.push(" WHEN LOWER(");
        qb.push(col_prefix);
        qb.push("type_name) = LOWER(");
        qb.push_bind(tname.clone());
        qb.push(") THEN (");
        qb.push(pred.as_str());
        qb.push(")");
    }
    qb.push(" ELSE TRUE END)");
}

async fn filter_visible_in_subgraph_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    candidate_ids: &[Uuid],
    policies: &BTreeMap<String, NodeTypePolicySpec>,
) -> Result<HashSet<Uuid>, AppError> {
    if candidate_ids.is_empty() {
        return Ok(HashSet::new());
    }
    if !has_any_row_filter(policies) {
        return Ok(candidate_ids.iter().copied().collect());
    }
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT id FROM xb_nodes WHERE tenant_id = ");
    qb.push_bind(String::from(tenant_id));
    qb.push(" AND subgraph_slug = ");
    qb.push_bind(String::from(subgraph_slug));
    qb.push(" AND id = ANY(");
    qb.push_bind(candidate_ids.to_vec());
    qb.push(")");
    append_policy_case_for_alias(&mut qb, "", policies);

    let rows =
        qb.build().fetch_all(&mut **tx).await.map_err(|err| {
            AppError::Database(format!("filter governed sim nodes failed: {err}"))
        })?;
    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

async fn sample_governed_scoped_node_ids_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    scope: &SandboxScope,
    fetch_limit: i64,
    policies: &BTreeMap<String, NodeTypePolicySpec>,
) -> Result<Vec<Uuid>, AppError> {
    let type_filter = normalize_filter_list(&scope.type_names);
    if scope.root_node_ids.is_empty() {
        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new("SELECT id FROM xb_nodes WHERE tenant_id = ");
        qb.push_bind(String::from(tenant_id));
        qb.push(" AND subgraph_slug = ");
        qb.push_bind(String::from(subgraph_slug));
        if let Some(ref tf) = type_filter {
            qb.push(" AND type_name = ANY(");
            qb.push_bind(tf.clone());
            qb.push(")");
        }
        append_policy_case_for_alias(&mut qb, "", policies);
        qb.push(" ORDER BY id ASC LIMIT ");
        qb.push_bind(fetch_limit);

        let rows =
            qb.build().fetch_all(&mut **tx).await.map_err(|err| {
                AppError::Database(format!("collect governed nodes failed: {err}"))
            })?;
        return Ok(rows.into_iter().map(|r| r.get("id")).collect());
    }

    bfs_governed_scope_walk_tx(tx, tenant_id, subgraph_slug, scope, fetch_limit, policies).await
}

async fn bfs_governed_scope_walk_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    scope: &SandboxScope,
    fetch_limit: i64,
    policies: &BTreeMap<String, NodeTypePolicySpec>,
) -> Result<Vec<Uuid>, AppError> {
    let visible_roots =
        filter_visible_in_subgraph_tx(tx, tenant_id, subgraph_slug, &scope.root_node_ids, policies)
            .await?;
    if visible_roots.is_empty() {
        return Ok(Vec::new());
    }
    let link_filter = normalize_filter_list(&scope.link_types);
    let mut visited: BTreeSet<Uuid> = visible_roots.iter().copied().collect();
    let mut frontier: Vec<Uuid> = visited.iter().copied().collect();

    for _hop in 0..scope.depth {
        if frontier.is_empty() {
            break;
        }
        let next_candidates = expand_one_hop_tx(
            tx,
            tenant_id,
            subgraph_slug,
            &frontier,
            scope.direction,
            link_filter.as_deref(),
        )
        .await?;
        let unseen: Vec<Uuid> = next_candidates
            .into_iter()
            .filter(|id| !visited.contains(id))
            .collect();
        let visible_next =
            filter_visible_in_subgraph_tx(tx, tenant_id, subgraph_slug, &unseen, policies).await?;
        frontier.clear();
        for id in visible_next {
            if visited.insert(id) {
                frontier.push(id);
            }
        }
    }

    let all_visited: Vec<Uuid> = visited.into_iter().collect();
    let type_filter = normalize_filter_list(&scope.type_names);
    let rows = sqlx::query(
        "SELECT id FROM xb_nodes
          WHERE tenant_id = $1
            AND subgraph_slug = $2
            AND id = ANY($3)
            AND ($4::text[] IS NULL OR type_name = ANY($4))
          ORDER BY id ASC
          LIMIT $5",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(&all_visited)
    .bind(&type_filter)
    .bind(fetch_limit)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("finalize governed BFS nodes failed: {err}")))?;
    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

async fn expand_one_hop_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    frontier: &[Uuid],
    direction: TraversalDirection,
    link_filter: Option<&[String]>,
) -> Result<Vec<Uuid>, AppError> {
    let sql = match direction {
        TraversalDirection::Out => {
            "SELECT DISTINCT dst_node_id AS nid FROM xb_links
              WHERE tenant_id = $1 AND subgraph_slug = $2 AND src_node_id = ANY($3)
                AND ($4::text[] IS NULL OR link_type = ANY($4))"
        }
        TraversalDirection::In => {
            "SELECT DISTINCT src_node_id AS nid FROM xb_links
              WHERE tenant_id = $1 AND subgraph_slug = $2 AND dst_node_id = ANY($3)
                AND ($4::text[] IS NULL OR link_type = ANY($4))"
        }
        TraversalDirection::Both => {
            "SELECT DISTINCT CASE WHEN src_node_id = ANY($3) THEN dst_node_id ELSE src_node_id END AS nid
               FROM xb_links
              WHERE tenant_id = $1 AND subgraph_slug = $2
                AND (src_node_id = ANY($3) OR dst_node_id = ANY($3))
                AND ($4::text[] IS NULL OR link_type = ANY($4))"
        }
    };
    let rows = sqlx::query(sql)
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(frontier)
        .bind(link_filter)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("expand sim scope hop failed: {err}")))?;
    Ok(rows.into_iter().map(|r| r.get("nid")).collect())
}

async fn build_governed_slice_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    node_ids: &[Uuid],
    link_filter: Option<&[String]>,
    policies: &BTreeMap<String, NodeTypePolicySpec>,
    schema_doc: axpoint_domain::studio::SchemaDoc,
) -> Result<ScopePreflightSlice, AppError> {
    let count_rows = sqlx::query(
        "SELECT type_name, COUNT(*)::bigint AS cnt
           FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = $2 AND id = ANY($3)
          GROUP BY type_name",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(node_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("count governed node types failed: {err}")))?;

    let mut node_counts_by_type = BTreeMap::new();
    for row in count_rows {
        let cnt: i64 = row.get("cnt");
        node_counts_by_type.insert(
            row.get("type_name"),
            usize::try_from(cnt.max(0)).unwrap_or(0),
        );
    }

    let isolated_i64: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
           FROM unnest($3::uuid[]) AS cand(node_id)
          WHERE NOT EXISTS (
              SELECT 1 FROM xb_links l
               WHERE l.tenant_id = $1 AND l.subgraph_slug = $2
                 AND (l.src_node_id = cand.node_id OR l.dst_node_id = cand.node_id)
                 AND l.src_node_id = ANY($3) AND l.dst_node_id = ANY($3)
                 AND ($4::text[] IS NULL OR l.link_type = ANY($4))
          )",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(node_ids)
    .bind(link_filter)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("count governed isolated nodes failed: {err}")))?;

    let induced_edges =
        query_governed_induced_tx(tx, tenant_id, subgraph_slug, node_ids, link_filter).await?;
    let boundary_edges =
        query_governed_boundary_tx(tx, tenant_id, subgraph_slug, node_ids, policies).await?;
    let observed_metric_keys =
        query_governed_metric_keys_tx(tx, tenant_id, subgraph_slug, node_ids).await?;

    Ok(ScopePreflightSlice {
        node_counts_by_type,
        isolated_nodes: usize::try_from(isolated_i64.max(0)).unwrap_or(0),
        induced_edges,
        boundary_edges,
        observed_metric_keys,
        schema_doc,
    })
}

async fn query_governed_induced_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    node_ids: &[Uuid],
    link_filter: Option<&[String]>,
) -> Result<Vec<ScopeInducedEdgeGroup>, AppError> {
    let rows = sqlx::query(
        "SELECT l.link_type, s.type_name AS src_type_name, d.type_name AS dst_type_name, COUNT(*)::bigint AS cnt
           FROM xb_links l
           JOIN xb_nodes s ON s.id = l.src_node_id AND s.tenant_id = $1 AND s.subgraph_slug = $2
           JOIN xb_nodes d ON d.id = l.dst_node_id AND d.tenant_id = $1 AND d.subgraph_slug = $2
          WHERE l.tenant_id = $1 AND l.subgraph_slug = $2
            AND l.src_node_id = ANY($3) AND l.dst_node_id = ANY($3)
            AND ($4::text[] IS NULL OR l.link_type = ANY($4))
          GROUP BY l.link_type, s.type_name, d.type_name
          ORDER BY l.link_type ASC, s.type_name ASC, d.type_name ASC",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(node_ids)
    .bind(link_filter)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query governed induced edges failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let cnt: i64 = row.get("cnt");
            ScopeInducedEdgeGroup {
                link_type: row.get("link_type"),
                src_type_name: row.get("src_type_name"),
                dst_type_name: row.get("dst_type_name"),
                count: usize::try_from(cnt.max(0)).unwrap_or(0),
            }
        })
        .collect())
}

async fn query_governed_boundary_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    node_ids: &[Uuid],
    policies: &BTreeMap<String, NodeTypePolicySpec>,
) -> Result<Vec<ScopeBoundaryEdgeGroup>, AppError> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "WITH visible_nodes AS (SELECT id, type_name FROM xb_nodes WHERE tenant_id = ",
    );
    qb.push_bind(String::from(tenant_id));
    qb.push(" AND subgraph_slug = ");
    qb.push_bind(String::from(subgraph_slug));
    append_policy_case_for_alias(&mut qb, "", policies);
    qb.push(
        ") SELECT l.link_type,
                CASE WHEN l.src_node_id = ANY(",
    );
    qb.push_bind(node_ids.to_vec());
    qb.push(
        ") THEN 'OUT' ELSE 'IN' END AS dir,
                CASE WHEN l.src_node_id = ANY(",
    );
    qb.push_bind(node_ids.to_vec());
    qb.push(
        ") THEN s.type_name ELSE d.type_name END AS in_scope_type,
                CASE WHEN l.src_node_id = ANY(",
    );
    qb.push_bind(node_ids.to_vec());
    qb.push(
        ") THEN d.type_name ELSE s.type_name END AS excluded_type,
                COUNT(*)::bigint AS cnt
           FROM xb_links l
           JOIN visible_nodes s ON s.id = l.src_node_id
           JOIN visible_nodes d ON d.id = l.dst_node_id
          WHERE l.tenant_id = ",
    );
    qb.push_bind(String::from(tenant_id));
    qb.push(" AND l.subgraph_slug = ");
    qb.push_bind(String::from(subgraph_slug));
    qb.push(" AND ((l.src_node_id = ANY(");
    qb.push_bind(node_ids.to_vec());
    qb.push(") AND NOT (l.dst_node_id = ANY(");
    qb.push_bind(node_ids.to_vec());
    qb.push("))) OR (NOT (l.src_node_id = ANY(");
    qb.push_bind(node_ids.to_vec());
    qb.push(")) AND l.dst_node_id = ANY(");
    qb.push_bind(node_ids.to_vec());
    qb.push(
        ")))
          GROUP BY 1, 2, 3, 4
          ORDER BY 1 ASC, 2 ASC, 3 ASC, 4 ASC",
    );

    let rows = qb.build().fetch_all(&mut **tx).await.map_err(|err| {
        AppError::Database(format!("query governed boundary edges failed: {err}"))
    })?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let dir_raw: String = row.get("dir");
            let cnt: i64 = row.get("cnt");
            ScopeBoundaryEdgeGroup {
                link_type: row.get("link_type"),
                direction: if dir_raw == "IN" {
                    LinkDirection::In
                } else {
                    LinkDirection::Out
                },
                in_scope_type_name: row.get("in_scope_type"),
                excluded_type_name: row.get("excluded_type"),
                count: usize::try_from(cnt.max(0)).unwrap_or(0),
            }
        })
        .collect())
}

async fn query_governed_metric_keys_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    node_ids: &[Uuid],
) -> Result<BTreeMap<String, BTreeSet<String>>, AppError> {
    let rows = sqlx::query(
        "SELECT DISTINCT n.type_name, k.key AS metric_key
           FROM xb_nodes n
           CROSS JOIN LATERAL jsonb_object_keys(
               CASE WHEN jsonb_typeof(n.metrics) = 'object' THEN n.metrics ELSE '{}'::jsonb END
           ) AS k(key)
          WHERE n.tenant_id = $1 AND n.subgraph_slug = $2 AND n.id = ANY($3)",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(node_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query governed metric keys failed: {err}")))?;

    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        let tname: String = row.get("type_name");
        let mkey: String = row.get("metric_key");
        if !mkey.trim().is_empty() {
            map.entry(tname).or_default().insert(mkey);
        }
    }
    Ok(map)
}

async fn evaluate_visible_sandbox_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    nodes: &[SandboxGraphNode],
    policies: &BTreeMap<String, NodeTypePolicySpec>,
) -> Result<HashSet<Uuid>, AppError> {
    let cap = nodes.len();
    let mut ids = Vec::with_capacity(cap);
    let mut types = Vec::with_capacity(cap);
    let mut keys = Vec::with_capacity(cap);
    let mut titles = Vec::with_capacity(cap);
    let mut props = Vec::with_capacity(cap);
    let mut metrics = Vec::with_capacity(cap);
    let mut is_news = Vec::with_capacity(cap);

    for n in nodes {
        ids.push(n.id);
        types.push(n.type_name.clone());
        keys.push(n.node_key.clone());
        titles.push(n.title.clone());
        props.push(serde_json::Value::Object(n.properties.clone()));
        metrics.push(serde_json::to_value(&n.metrics).unwrap_or_default());
        is_news.push(n.is_new);
    }

    let governed_types_lower: Vec<String> = policies
        .iter()
        .filter(|(_, spec)| spec.pg_row_filter.is_some())
        .map(|(t, _)| t.to_ascii_lowercase())
        .collect();

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT id FROM (
             SELECT u.id,
                    COALESCE(live.type_name, u.type_name) AS type_name,
                    COALESCE(live.node_key, u.node_key) AS node_key,
                    COALESCE(live.title, u.title) AS title,
                    CASE WHEN u.is_new THEN u.properties ELSE live.properties END AS properties,
                    CASE WHEN u.is_new THEN u.metrics ELSE live.metrics END AS metrics,
                    CASE WHEN u.is_new THEN NOW() ELSE live.updated_at END AS updated_at,
                    CASE WHEN u.is_new THEN NOW() ELSE live.created_at END AS created_at,
                    u.is_new,
                    (live.id IS NOT NULL) AS has_live
               FROM UNNEST(",
    );
    qb.push_bind(ids);
    qb.push("::uuid[], ");
    qb.push_bind(types);
    qb.push("::text[], ");
    qb.push_bind(keys);
    qb.push("::text[], ");
    qb.push_bind(titles);
    qb.push("::text[], ");
    qb.push_bind(props);
    qb.push("::jsonb[], ");
    qb.push_bind(metrics);
    qb.push("::jsonb[], ");
    qb.push_bind(is_news);
    qb.push(
        "::bool[]) AS u(id, type_name, node_key, title, properties, metrics, is_new)
               LEFT JOIN xb_nodes live
                      ON live.id = u.id
                     AND live.tenant_id = ",
    );
    qb.push_bind(String::from(tenant_id));
    qb.push(
        " AND live.subgraph_slug = 'main') AS sb_eval
          WHERE (is_new OR has_live OR NOT (LOWER(type_name) = ANY(",
    );
    qb.push_bind(governed_types_lower);
    qb.push(")))");
    append_policy_case_for_alias(&mut qb, "", policies);

    let rows = qb
        .build()
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("evaluate sandbox node policies: {err}")))?;
    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

async fn insert_masked_base_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    node_ids: &[Uuid],
    policy: &PolicyContext,
    tenant_hmac_key: &[u8; 32],
) -> Result<i32, AppError> {
    if !has_any_property_masks(&policy.node_types) {
        let res = sqlx::query(
            "INSERT INTO ax_sim_base_nodes (
                 tenant_id, sandbox_id, node_id, type_name, node_key, title,
                 metrics, properties, row_version
             )
             SELECT $1, $2, id, type_name, node_key, title, metrics, properties, row_version
               FROM xb_nodes
              WHERE tenant_id = $1 AND subgraph_slug = 'main' AND id = ANY($3)",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(node_ids)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clone governed base nodes failed: {err}")))?;
        return Ok(i32::try_from(res.rows_affected()).unwrap_or(i32::MAX));
    }

    let rows = sqlx::query(
        "SELECT id, type_name, node_key, title, metrics, properties, row_version
           FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND id = ANY($2)
          ORDER BY id ASC",
    )
    .bind(tenant_id)
    .bind(node_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load nodes for masked sim clone: {err}")))?;

    let mut inserted = 0_i32;
    for row in rows {
        let nid: Uuid = row.get("id");
        let type_name: String = row.get("type_name");
        let node_key: String = row.get("node_key");
        let title: String = row.get("title");
        let metrics_val: serde_json::Value = row.get("metrics");
        let mut props_map = match row.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        if let Some(spec) = lookup_node_policy(&policy.node_types, &type_name) {
            mask_json_properties(&mut props_map, &spec.property_masks, tenant_hmac_key);
        }
        let row_ver: i64 = row.get("row_version");
        sqlx::query(
            "INSERT INTO ax_sim_base_nodes (
                 tenant_id, sandbox_id, node_id, type_name, node_key, title,
                 metrics, properties, row_version
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(tenant_id)
        .bind(sandbox_id)
        .bind(nid)
        .bind(&type_name)
        .bind(&node_key)
        .bind(&title)
        .bind(metrics_val)
        .bind(serde_json::Value::Object(props_map))
        .bind(row_ver)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert masked base node failed: {err}")))?;
        inserted = inserted.saturating_add(1);
    }
    Ok(inserted)
}

fn validate_sandbox_scope(scope: &SandboxScope) -> Result<(), AppError> {
    if scope.depth > MAX_SANDBOX_SCOPE_DEPTH {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("scope.depth must be <= {MAX_SANDBOX_SCOPE_DEPTH}"),
        ));
    }
    if scope.max_nodes == 0 || scope.max_nodes > DEFAULT_SANDBOX_MAX_NODES {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("scope.max_nodes must be in 1..={DEFAULT_SANDBOX_MAX_NODES}"),
        ));
    }
    Ok(())
}

async fn populate_governed_base_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
    scope: &SandboxScope,
    policy: &PolicyContext,
    tenant_key: &[u8; 32],
) -> Result<(i32, i32), AppError> {
    validate_sandbox_scope(scope)?;
    let fetch_limit = i64::try_from(scope.max_nodes)
        .unwrap_or(50_000)
        .saturating_add(1);
    let empty_policies = BTreeMap::new();
    let active_policies = if !policy.bypass_row_policies && has_any_row_filter(&policy.node_types) {
        &policy.node_types
    } else {
        &empty_policies
    };
    let node_ids = sample_governed_scoped_node_ids_tx(
        tx,
        tenant_id,
        "main",
        scope,
        fetch_limit,
        active_policies,
    )
    .await?;
    if node_ids.len() > scope.max_nodes {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("scoped node count exceeds max_nodes ({})", scope.max_nodes),
        ));
    }
    if node_ids.is_empty() {
        return Ok((0, 0));
    }
    let n_cnt =
        insert_masked_base_nodes_tx(tx, tenant_id, sandbox_id, &node_ids, policy, tenant_key)
            .await?;
    let link_filter = normalize_filter_list(&scope.link_types);
    let l_res = sqlx::query(
        "INSERT INTO ax_sim_base_links (
             tenant_id, sandbox_id, link_id, link_type, src_node_id, dst_node_id, weight
         )
         SELECT $1, $2, id, link_type, src_node_id, dst_node_id, weight
           FROM xb_links
          WHERE tenant_id = $1 AND subgraph_slug = 'main'
            AND src_node_id = ANY($3) AND dst_node_id = ANY($3)
            AND ($4::text[] IS NULL OR link_type = ANY($4))",
    )
    .bind(tenant_id)
    .bind(sandbox_id)
    .bind(&node_ids)
    .bind(&link_filter)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("clone governed base links: {err}")))?;
    Ok((
        n_cnt,
        i32::try_from(l_res.rows_affected()).unwrap_or(i32::MAX),
    ))
}

async fn clone_governed_sandbox_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    name: &str,
    created_by: Option<Uuid>,
    scope: &SandboxScope,
    policy: &PolicyContext,
    tenant_key: &[u8; 32],
) -> Result<(Uuid, i32, i32), AppError> {
    let scope_json = serde_json::to_value(scope)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid scope: {err}")))?;
    let schema_version: Option<i32> = sqlx::query_scalar(
        "SELECT current_version FROM xb_schema_heads
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND current_version > 0",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query xb_schema_heads failed: {err}")))?;

    let sandbox_id: Uuid = sqlx::query_scalar(
        "INSERT INTO ax_sandboxes (tenant_id, name, scope, status, schema_version, created_by)
         VALUES ($1, $2, $3, 'CLONING', $4, $5) RETURNING id",
    )
    .bind(tenant_id)
    .bind(name)
    .bind(scope_json)
    .bind(schema_version)
    .bind(created_by)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert governed ax_sandboxes: {err}")))?;

    let (n_cnt, l_cnt) =
        populate_governed_base_tx(tx, tenant_id, sandbox_id, scope, policy, tenant_key).await?;
    Ok((sandbox_id, n_cnt, l_cnt))
}

async fn recount_governed_sandbox_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
) -> Result<(i32, i32), AppError> {
    let row = sqlx::query(
        "WITH active_nodes AS (
             SELECT b.node_id
               FROM ax_sim_base_nodes b
               LEFT JOIN ax_sim_overlay_nodes o
                 ON o.sandbox_id = b.sandbox_id AND o.node_id = b.node_id
              WHERE b.sandbox_id = $1 AND b.tenant_id = $2
                AND COALESCE(o.deleted, FALSE) = FALSE
             UNION
             SELECT node_id
               FROM ax_sim_overlay_nodes
              WHERE sandbox_id = $1 AND tenant_id = $2
                AND is_new = TRUE AND deleted = FALSE
         ),
         active_links AS (
             SELECT b.link_id
               FROM ax_sim_base_links b
               LEFT JOIN ax_sim_overlay_links o
                 ON o.sandbox_id = b.sandbox_id AND o.link_id = b.link_id
               JOIN active_nodes s ON s.node_id = b.src_node_id
               JOIN active_nodes d ON d.node_id = b.dst_node_id
              WHERE b.sandbox_id = $1 AND b.tenant_id = $2
                AND COALESCE(o.deleted, FALSE) = FALSE
             UNION
             SELECT l.link_id
               FROM ax_sim_overlay_links l
               JOIN active_nodes s ON s.node_id = l.src_node_id
               JOIN active_nodes d ON d.node_id = l.dst_node_id
              WHERE l.sandbox_id = $1 AND l.tenant_id = $2
                AND l.is_new = TRUE AND l.deleted = FALSE
         )
         SELECT (SELECT COUNT(*)::bigint FROM active_nodes) AS node_cnt,
                (SELECT COUNT(*)::bigint FROM active_links) AS link_cnt",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("recount governed sandbox failed: {err}")))?;

    let node_cnt: i64 = row.get("node_cnt");
    let link_cnt: i64 = row.get("link_cnt");
    Ok((
        i32::try_from(node_cnt.max(0)).unwrap_or(i32::MAX),
        i32::try_from(link_cnt.max(0)).unwrap_or(i32::MAX),
    ))
}

async fn query_orphaned_overlay_ids_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    sandbox_id: Uuid,
) -> Result<(Vec<Uuid>, Vec<Uuid>), AppError> {
    let orphaned_node_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT o.node_id
           FROM ax_sim_overlay_nodes o
          WHERE o.sandbox_id = $1 AND o.tenant_id = $2 AND o.is_new = FALSE
            AND NOT EXISTS (
                SELECT 1 FROM ax_sim_base_nodes b
                 WHERE b.sandbox_id = o.sandbox_id AND b.node_id = o.node_id
            )
          ORDER BY o.node_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("detect governed orphaned nodes: {err}")))?;

    let orphaned_link_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT o.link_id
           FROM ax_sim_overlay_links o
          WHERE o.sandbox_id = $1 AND o.tenant_id = $2 AND o.is_new = FALSE
            AND NOT EXISTS (
                SELECT 1 FROM ax_sim_base_links b
                 WHERE b.sandbox_id = o.sandbox_id AND b.link_id = o.link_id
            )
          ORDER BY o.link_id ASC",
    )
    .bind(sandbox_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("detect governed orphaned links: {err}")))?;

    Ok((orphaned_node_ids, orphaned_link_ids))
}

fn decode_governed_sandbox_row(row: &sqlx::postgres::PgRow) -> Result<AxSandboxRecord, AppError> {
    let status_raw: String = row.get("status");
    let status = match status_raw.as_str() {
        "CLONING" => SandboxStatus::Cloning,
        "READY" => SandboxStatus::Ready,
        "ARCHIVED" => SandboxStatus::Archived,
        _ => {
            return Err(AppError::Database(format!(
                "unknown ax_sandboxes.status value: {status_raw}"
            )));
        }
    };
    let scope: SandboxScope = serde_json::from_value(row.get("scope")).unwrap_or_default();
    let node_count_i32: i32 = row.get("node_count");
    let link_count_i32: i32 = row.get("link_count");
    let actions: Vec<AxSimAction> = serde_json::from_value(row.get("actions")).unwrap_or_default();
    let sim_result: Option<AxSimResult> =
        serde_json::from_value(row.get::<serde_json::Value, _>("sim_result")).ok();

    Ok(AxSandboxRecord {
        id: row.get("id"),
        name: row.get("name"),
        status,
        scope,
        node_count: usize::try_from(node_count_i32.max(0)).unwrap_or(0),
        link_count: usize::try_from(link_count_i32.max(0)).unwrap_or(0),
        base_captured_at: row.get("base_captured_at"),
        schema_version: row.get("schema_version"),
        created_by: row.get("created_by"),
        actions,
        sim_result,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

impl PgStore {
    /// Filters candidate node UUIDs inside `subgraph_slug` under caller `PolicyContext` row policies.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] on database transaction or query failure.
    pub async fn filter_visible_subgraph_node_ids(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        candidate_ids: &[Uuid],
        policy: &PolicyContext,
    ) -> Result<HashSet<Uuid>, AppError> {
        if candidate_ids.is_empty() {
            return Ok(HashSet::new());
        }
        if policy.bypass_row_policies || !has_any_row_filter(&policy.node_types) {
            return Ok(candidate_ids.iter().copied().collect());
        }
        let clean_slug = {
            let trimmed = subgraph_slug.trim();
            if trimmed.is_empty() { "main" } else { trimmed }
        };
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let visible = filter_visible_in_subgraph_tx(
            &mut tx,
            tenant_id,
            clean_slug,
            candidate_ids,
            &policy.node_types,
        )
        .await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit filter_visible_subgraph_node_ids tx: {err}"))
        })?;
        Ok(visible)
    }

    /// Evaluates live-state and overlay row-policy visibility for all nodes in `sandbox_id`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] if the sandbox is missing (`SANDBOX_NOT_FOUND`) or SQL evaluation fails.
    pub async fn filter_sandbox_visible_node_ids(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        policy: &PolicyContext,
    ) -> Result<HashSet<Uuid>, AppError> {
        let graph = self.get_sandbox_graph(tenant_id, sandbox_id).await?;
        if graph.nodes.is_empty() {
            return Ok(HashSet::new());
        }
        if policy.bypass_row_policies || !has_any_row_filter(&policy.node_types) {
            return Ok(graph.nodes.into_iter().map(|n| n.id).collect());
        }
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let visible =
            evaluate_visible_sandbox_nodes_tx(&mut tx, tenant_id, &graph.nodes, &policy.node_types)
                .await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit filter_sandbox_visible_node_ids tx: {err}"))
        })?;
        Ok(visible)
    }

    /// Re-clones `ax_sim_base_*` for `sandbox_id` while enforcing caller `PolicyContext` row filters and PII masks.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] if the sandbox does not exist, scope bounds fail, or SQL execution fails.
    pub async fn refresh_sandbox_base_governed(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        policy: &PolicyContext,
    ) -> Result<RefreshBaseResponse, AppError> {
        let enforce_rows = !policy.bypass_row_policies && has_any_row_filter(&policy.node_types);
        let enforce_masks = has_any_property_masks(&policy.node_types);
        if !enforce_rows && !enforce_masks {
            return self.refresh_sandbox_base(tenant_id, sandbox_id).await;
        }
        let tenant_key = MaskKeyRing::from_env()?.tenant_key(tenant_id);
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let scope_val: serde_json::Value = sqlx::query_scalar(
            "SELECT scope FROM ax_sandboxes WHERE id = $1 AND tenant_id = $2 FOR UPDATE",
        )
        .bind(sandbox_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("lock ax_sandboxes for governed refresh: {err}"))
        })?
        .ok_or(AppError::NotFound("SANDBOX_NOT_FOUND"))?;

        let scope: SandboxScope = serde_json::from_value(scope_val).unwrap_or_default();
        let schema_version: Option<i32> = sqlx::query_scalar(
            "SELECT current_version FROM xb_schema_heads
              WHERE tenant_id = $1 AND subgraph_slug = 'main' AND current_version > 0",
        )
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query xb_schema_heads failed: {err}")))?;

        sqlx::query("DELETE FROM ax_sim_base_links WHERE sandbox_id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("clear governed base links: {err}")))?;
        sqlx::query("DELETE FROM ax_sim_base_nodes WHERE sandbox_id = $1 AND tenant_id = $2")
            .bind(sandbox_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("clear governed base nodes: {err}")))?;

        let _cloned =
            populate_governed_base_tx(&mut tx, tenant_id, sandbox_id, &scope, policy, &tenant_key)
                .await?;
        let (node_count, link_count) =
            recount_governed_sandbox_tx(&mut tx, tenant_id, sandbox_id).await?;
        let (orphaned_node_ids, orphaned_link_ids) =
            query_orphaned_overlay_ids_tx(&mut tx, tenant_id, sandbox_id).await?;

        let sb_row = sqlx::query(
            "UPDATE ax_sandboxes
                SET base_captured_at = NOW(), schema_version = $2, node_count = $3,
                    link_count = $4, updated_at = NOW()
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
        .map_err(|err| AppError::Database(format!("update governed ax_sandboxes: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit refresh_sandbox_base_governed tx: {err}"))
        })?;

        Ok(RefreshBaseResponse {
            sandbox: decode_governed_sandbox_row(&sb_row)?,
            orphaned_node_ids,
            orphaned_link_ids,
        })
    }
}

impl SimVisibilityPort for PgStore {
    async fn preview_scope_slice_governed(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        scope: &SandboxScope,
        policy: &PolicyContext,
    ) -> Result<ScopePreflightSlice, AppError> {
        if policy.bypass_row_policies || !has_any_row_filter(&policy.node_types) {
            return self
                .preview_scope_slice(tenant_id, subgraph_slug, scope)
                .await;
        }
        let base_slice = self
            .preview_scope_slice(tenant_id, subgraph_slug, scope)
            .await?;
        let clean_slug = {
            let trimmed = subgraph_slug.trim();
            if trimmed.is_empty() { "main" } else { trimmed }
        };
        let cap_usize = scope.max_nodes.saturating_add(1).min(100_000);
        let fetch_limit = i64::try_from(cap_usize).unwrap_or(100_000);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let node_ids = sample_governed_scoped_node_ids_tx(
            &mut tx,
            tenant_id,
            clean_slug,
            scope,
            fetch_limit,
            &policy.node_types,
        )
        .await?;
        if node_ids.is_empty() {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty governed preflight tx: {err}"))
            })?;
            return Ok(ScopePreflightSlice {
                schema_doc: base_slice.schema_doc,
                ..ScopePreflightSlice::default()
            });
        }
        let link_filter = normalize_filter_list(&scope.link_types);
        let slice = build_governed_slice_tx(
            &mut tx,
            tenant_id,
            clean_slug,
            &node_ids,
            link_filter.as_deref(),
            &policy.node_types,
            base_slice.schema_doc,
        )
        .await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit governed preflight slice tx: {err}"))
        })?;
        Ok(slice)
    }

    async fn create_sandbox_governed(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxSandboxRequest,
        policy: &PolicyContext,
    ) -> Result<AxSandboxRecord, AppError> {
        let enforce_rows = !policy.bypass_row_policies && has_any_row_filter(&policy.node_types);
        let enforce_masks = has_any_property_masks(&policy.node_types);
        if !enforce_rows && !enforce_masks {
            return self.create_sandbox(tenant_id, created_by, req).await;
        }

        let name = req.name.trim();
        if name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "sandbox name cannot be empty",
            ));
        }
        let scope = req.scope.unwrap_or_default();
        validate_sandbox_scope(&scope)?;
        let tenant_key = MaskKeyRing::from_env()?.tenant_key(tenant_id);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let (sandbox_id, node_count, link_count) = clone_governed_sandbox_tx(
            &mut tx,
            tenant_id,
            name,
            created_by,
            &scope,
            policy,
            &tenant_key,
        )
        .await?;

        let row = sqlx::query(
            "UPDATE ax_sandboxes
                SET status = 'READY', node_count = $2, link_count = $3,
                    base_captured_at = NOW(), updated_at = NOW()
              WHERE id = $1 AND tenant_id = $4
          RETURNING id, name, scope, base_captured_at, schema_version, created_by, created_at, updated_at",
        )
        .bind(sandbox_id)
        .bind(node_count)
        .bind(link_count)
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("finalize governed sandbox: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_sandbox_governed tx: {err}"))
        })?;

        Ok(AxSandboxRecord {
            id: row.get("id"),
            name: row.get("name"),
            status: SandboxStatus::Ready,
            scope,
            node_count: usize::try_from(node_count.max(0)).unwrap_or(0),
            link_count: usize::try_from(link_count.max(0)).unwrap_or(0),
            base_captured_at: row.get("base_captured_at"),
            schema_version: row.get("schema_version"),
            created_by: row.get("created_by"),
            actions: Vec::new(),
            sim_result: None,
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }

    async fn get_sandbox_graph_governed(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        policy: &PolicyContext,
    ) -> Result<SandboxGraphResponse, AppError> {
        let mut graph = self.get_sandbox_graph(tenant_id, sandbox_id).await?;
        if graph.nodes.is_empty() {
            return Ok(graph);
        }

        if !policy.bypass_row_policies && has_any_row_filter(&policy.node_types) {
            let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
            let visible_ids = evaluate_visible_sandbox_nodes_tx(
                &mut tx,
                tenant_id,
                &graph.nodes,
                &policy.node_types,
            )
            .await?;
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit get_sandbox_graph_governed tx: {err}"))
            })?;
            graph.nodes.retain(|n| visible_ids.contains(&n.id));
            graph.links.retain(|l| {
                visible_ids.contains(&l.src_node_id) && visible_ids.contains(&l.dst_node_id)
            });
        }

        if has_any_property_masks(&policy.node_types) {
            let tenant_key = MaskKeyRing::from_env()?.tenant_key(tenant_id);
            for node in &mut graph.nodes {
                if let Some(spec) = lookup_node_policy(&policy.node_types, &node.type_name) {
                    mask_json_properties(&mut node.properties, &spec.property_masks, &tenant_key);
                }
            }
        }

        Ok(graph)
    }
}
