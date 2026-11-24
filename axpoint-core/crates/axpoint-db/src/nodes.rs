//! Business graph node & link persistence, bulk `UNNEST` ingestion, bbox queries, and hybrid search (`GraphStorePort`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::{GeoPredicateOp, JobKind, OutboxTopic};
use axpoint_domain::geo_ops::{
    GeoShapeDto, matches_spatial_filter, shape_vertices, spatial_filter_bbox, validate_geo_shape,
};
use axpoint_domain::graph::{
    BulkLinkInput, BulkNodeInput, BulkNodesRequest, BulkNodesResponse, EntityCandidatesResponse,
    EntityLink, GeoPoint, LinkEndpointRef, LinkListQuery, LinkRecord, MetricsMerge,
    NodeDocumentsResponse, NodeListQuery, NodeListResponse, NodeRef, NodeSearchHit,
    NodeSearchQuery, NodeSearchResponse, PatchNodeRequest, ResolveEntitiesRequest,
    ResolveEntitiesResponse, UnresolvedLink, UpsertLinkRequest, build_node_embed_text,
};
use axpoint_domain::guard::NodeTypePolicySpec;
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::ports::GraphStorePort;
use axpoint_domain::topology::{BlueLink, BlueNode, UpsertBlueNodeRequest};
use chrono::{DateTime, Utc};
use pgvector::Vector;
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

pub(crate) fn decode_blue_node(row: &sqlx::postgres::PgRow) -> BlueNode {
    let p_val: serde_json::Value = row.get("properties");
    let properties = match p_val {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let m_val: serde_json::Value = row.get("metrics");
    let metrics: BTreeMap<String, f64> = serde_json::from_value(m_val).unwrap_or_default();
    let geo_lng: Option<f64> = row.get("geo_lng");
    let geo_lat: Option<f64> = row.get("geo_lat");
    let geo = match (geo_lng, geo_lat) {
        (Some(lng), Some(lat)) => Some(GeoPoint { lng, lat }),
        _ => None,
    };
    let geo_shape = row
        .try_get::<Option<serde_json::Value>, _>("geo_shape")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok());

    BlueNode {
        id: row.get("id"),
        type_name: row.get("type_name"),
        node_key: row.get("node_key"),
        title: row.get("title"),
        properties,
        metrics,
        geo,
        geo_shape,
        source_dataset_id: row.get("source_dataset_id"),
        source_row_key: row.get("source_row_key"),
        row_version: row.get("row_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

pub(crate) fn decode_link_record(row: &sqlx::postgres::PgRow) -> LinkRecord {
    let p_val: serde_json::Value = row.get("properties");
    let properties = match p_val {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let metrics: BTreeMap<String, f64> = row
        .try_get::<serde_json::Value, _>("metrics")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let row_version: i64 = row.try_get("row_version").unwrap_or(1);

    LinkRecord {
        id: row.get("id"),
        link_type: row.get("link_type"),
        src_node_id: row.get("src_node_id"),
        dst_node_id: row.get("dst_node_id"),
        weight: row.get("weight"),
        properties,
        metrics,
        row_version,
        source_dataset_id: row.get("source_dataset_id"),
        valid_from: row.get("valid_from"),
        valid_to: row.get("valid_to"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn validate_finite_metrics(metrics: &BTreeMap<String, f64>) -> Result<(), AppError> {
    for (k, v) in metrics {
        if k.trim().is_empty() || !v.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("metric '{k}' has empty name or non-finite value"),
            ));
        }
    }
    Ok(())
}

async fn ensure_node_type_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    type_name: &str,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xb_node_types WHERE subgraph_slug = 'main' AND name = $1)",
    )
    .bind(type_name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check xb_node_types failed: {err}")))?;

    if !exists {
        return Err(AppError::bad(
            "SCHEMA_INVALID",
            format!("node type '{type_name}' is not defined in xBlueStudio schema"),
        ));
    }
    Ok(())
}

async fn lookup_edge_impact_weight_tx(
    tx: &mut Transaction<'_, Postgres>,
    link_type: &str,
) -> Result<f64, AppError> {
    let maybe_weight: Option<f64> = sqlx::query_scalar(
        "SELECT impact_weight FROM xb_edge_types WHERE subgraph_slug = 'main' AND name = $1 ORDER BY created_at ASC LIMIT 1",
    )
    .bind(link_type)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lookup xb_edge_types failed: {err}")))?;

    maybe_weight.ok_or_else(|| {
        AppError::bad(
            "SCHEMA_INVALID",
            format!("edge type '{link_type}' is not defined in xBlueStudio schema"),
        )
    })
}

async fn resolve_node_cursor_tx(
    tx: &mut Transaction<'_, Postgres>,
    raw_cursor: &str,
) -> Result<(DateTime<Utc>, Uuid), AppError> {
    if let Some((ts_part, id_part)) = raw_cursor.split_once('|') {
        let ts = DateTime::parse_from_rfc3339(ts_part)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
            .with_timezone(&Utc);
        let id = Uuid::parse_str(id_part).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}"))
        })?;
        return Ok((ts, id));
    }

    let id = Uuid::parse_str(raw_cursor)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor: {err}")))?;
    let maybe_ts: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT updated_at FROM xb_nodes WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("resolve node cursor failed: {err}")))?;

    let ts = maybe_ts.ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "cursor node not found"))?;
    Ok((ts, id))
}

impl PgStore {
    /// Builds canonical embedding text (`"{display_name}：{title}（{node_key}）..."`, truncated to 2,000 chars)
    /// per `04-STEP03` §03.4.
    #[must_use]
    pub fn build_node_embedding_text(type_display_name: &str, node: &BlueNode) -> String {
        let label = if type_display_name.trim().is_empty() {
            node.type_name.as_str()
        } else {
            type_display_name.trim()
        };
        build_node_embed_text(label, node)
    }

    /// Upserts `XB_NODE` vector embeddings into `hg_embeddings` inside a tenant-scoped RLS transaction.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] if `dim == 0` or vector lengths mismatch, or [`AppError::Database`] on SQL failure.
    pub async fn upsert_node_embeddings(
        &self,
        tenant_id: &str,
        model_name: &str,
        dim: usize,
        items: &[(Uuid, Vec<f32>)],
    ) -> Result<(), AppError> {
        if items.is_empty() {
            return Ok(());
        }
        let dim_i32 = i32::try_from(dim).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid embedding dim: {err}"))
        })?;
        if dim_i32 <= 0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "embedding dim must be positive",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        for (node_id, vec_data) in items {
            if vec_data.len() != dim {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!(
                        "node embedding length {} does not match dim {dim}",
                        vec_data.len()
                    ),
                ));
            }
            let exists = sqlx::query(
                "SELECT 1 FROM xb_nodes WHERE tenant_id = $1 AND id = $2 FOR KEY SHARE",
            )
            .bind(tenant_id)
            .bind(*node_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("lock xb_nodes for embedding failed: {err}"))
            })?;
            if exists.is_none() {
                continue;
            }
            sqlx::query(
                "INSERT INTO hg_embeddings (tenant_id, node_type, node_id, model_name, embedding_dim, embedding)
                 VALUES ($1, 'XB_NODE', $2, $3, $4, $5)
                 ON CONFLICT (node_type, node_id, model_name) DO UPDATE SET
                    embedding_dim = EXCLUDED.embedding_dim,
                    embedding = EXCLUDED.embedding",
            )
            .bind(tenant_id)
            .bind(*node_id)
            .bind(model_name)
            .bind(dim_i32)
            .bind(Vector::from(vec_data.clone()))
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("upsert XB_NODE embedding failed: {err}")))?;
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit upsert_node_embeddings tx failed: {err}"))
        })?;
        Ok(())
    }

    /// Lists `xb_nodes` with optional compiled per-`type_name` PostgreSQL row-policy predicates (`11-STEP13` §13.6.3).
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] if `query` pagination/bbox arguments are invalid, or
    /// [`AppError::Database`] if querying `xb_nodes` fails.
    pub async fn list_nodes_with_row_policies(
        &self,
        tenant_id: &str,
        query: NodeListQuery,
        node_type_policies: Option<&BTreeMap<String, NodeTypePolicySpec>>,
    ) -> Result<NodeListResponse, AppError> {
        let spatial_filter = query.parse_spatial_filter()?;
        let stage1_bbox = match &spatial_filter {
            Some(sf) => Some(spatial_filter_bbox(sf)?),
            None => None,
        };
        let needs_stage2 = spatial_filter
            .as_ref()
            .is_some_and(|sf| sf.op != GeoPredicateOp::Bbox);
        let default_limit: u32 = if stage1_bbox.is_some() { 10_000 } else { 200 };
        let limit = query.limit.unwrap_or(default_limit);
        if !(1..=10_000).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=10000, got {limit}"),
            ));
        }

        let limit_usize = usize::try_from(limit).unwrap_or(200);
        let fetch_limit = if needs_stage2 {
            10_001_i64
        } else {
            i64::from(limit).saturating_add(1)
        };

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let cursor_pair = match query.cursor.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(c) => Some(resolve_node_cursor_tx(&mut tx, c.trim()).await?),
            None => None,
        };

        let mut qb = build_list_nodes_query(
            &query,
            stage1_bbox,
            needs_stage2,
            cursor_pair,
            node_type_policies,
        );
        qb.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
        qb.push_bind(fetch_limit);

        let rows = qb
            .build()
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("list xb_nodes failed: {err}")))?;

        let mut nodes: Vec<BlueNode> = rows.iter().map(decode_blue_node).collect();
        if let Some(ref sf) = spatial_filter {
            let mut refined = Vec::with_capacity(nodes.len());
            for node in nodes {
                if matches_spatial_filter(node.geo, node.geo_shape.as_ref(), sf)? {
                    refined.push(node);
                }
            }
            nodes = refined;
        }
        let next_cursor = if nodes.len() > limit_usize {
            nodes.truncate(limit_usize);
            nodes
                .last()
                .map(|n| format!("{}|{}", n.updated_at.to_rfc3339(), n.id))
        } else {
            None
        };

        let subgraph_slug = query
            .subgraph
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main");
        let node_ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
        let links = load_incident_blue_links_tx(&mut tx, &node_ids, subgraph_slug).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_nodes tx failed: {err}")))?;

        Ok(NodeListResponse {
            items: nodes.clone(),
            nodes,
            links,
            next_cursor,
        })
    }

    /// Collects governed WGS-84 coordinates from `xb_nodes` for `POST /api/xblue/geo/convex-hull` (`06-STEP28` §28.3).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `xb_nodes` fails.
    pub async fn collect_governed_geo_points(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        type_name: Option<&str>,
        node_ids: &[Uuid],
        node_type_policies: Option<&BTreeMap<String, NodeTypePolicySpec>>,
    ) -> Result<Vec<GeoPoint>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "SELECT geo_lng, geo_lat, geo_shape
               FROM xb_nodes
              WHERE subgraph_slug = ",
        );
        qb.push_bind(String::from(subgraph_slug));
        qb.push(" AND (geo_lng IS NOT NULL OR geo_shape IS NOT NULL)");
        if let Some(tf) = type_name.map(str::trim).filter(|s| !s.is_empty()) {
            qb.push(" AND type_name = ");
            qb.push_bind(String::from(tf));
        }
        if !node_ids.is_empty() {
            qb.push(" AND id = ANY(");
            qb.push_bind(node_ids.to_vec());
            qb.push(")");
        }
        if let Some(policies) = node_type_policies {
            append_pg_row_policy_case(&mut qb, policies);
        }
        qb.push(" ORDER BY id ASC LIMIT 50000");

        let rows = qb
            .build()
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("collect geo points failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit collect_governed_geo_points tx: {err}"))
        })?;

        let mut pts = Vec::with_capacity(rows.len());
        for row in rows {
            if let (Some(lng), Some(lat)) = (
                row.get::<Option<f64>, _>("geo_lng"),
                row.get::<Option<f64>, _>("geo_lat"),
            ) {
                pts.push(GeoPoint { lng, lat });
            }
            if let Some(shape_val) = row.get::<Option<serde_json::Value>, _>("geo_shape")
                && let Ok(shape) = serde_json::from_value::<GeoShapeDto>(shape_val)
            {
                for [lng, lat] in shape_vertices(&shape) {
                    let pt = GeoPoint { lng, lat };
                    if !matches!(shape, GeoShapeDto::Point(_)) || !pts.contains(&pt) {
                        pts.push(pt);
                    }
                }
            }
        }
        Ok(pts)
    }

    /// Evaluates compiled PostgreSQL row-policy predicates against a candidate `node_ids` slice,
    /// returning the subset of node IDs visible to the caller (`11-STEP13` §13.6.3).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if evaluating the predicate query against `xb_nodes` fails.
    pub async fn filter_visible_node_ids(
        &self,
        tenant_id: &str,
        node_ids: &[Uuid],
        node_type_policies: &BTreeMap<String, NodeTypePolicySpec>,
    ) -> Result<HashSet<Uuid>, AppError> {
        if node_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let has_any_filter = node_type_policies
            .values()
            .any(|spec| spec.pg_row_filter.is_some());
        if !has_any_filter {
            return Ok(node_ids.iter().copied().collect());
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new("SELECT id FROM xb_nodes WHERE id = ANY(");
        qb.push_bind(node_ids.to_vec());
        qb.push(")");
        append_pg_row_policy_case(&mut qb, node_type_policies);

        let rows =
            qb.build().fetch_all(&mut *tx).await.map_err(|err| {
                AppError::Database(format!("filter_visible_node_ids failed: {err}"))
            })?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit filter_visible_node_ids tx failed: {err}"))
        })?;

        Ok(rows.into_iter().map(|r| r.get("id")).collect())
    }
}

fn build_list_nodes_query(
    query: &NodeListQuery,
    bbox: Option<[f64; 4]>,
    include_shape_without_point: bool,
    cursor_pair: Option<(DateTime<Utc>, Uuid)>,
    node_type_policies: Option<&BTreeMap<String, NodeTypePolicySpec>>,
) -> QueryBuilder<Postgres> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key,
                row_version, created_at, updated_at
           FROM xb_nodes
          WHERE subgraph_slug = ",
    );
    let subgraph_slug = query
        .subgraph
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main");
    qb.push_bind(String::from(subgraph_slug));

    if let Some(tf) = query
        .type_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        qb.push(" AND type_name = ");
        qb.push_bind(String::from(tf));
    }
    if let Some(qf) = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like_pat = format!("%{qf}%");
        qb.push(" AND (fts @@ plainto_tsquery('simple', ");
        qb.push_bind(String::from(qf));
        qb.push(") OR title ILIKE ");
        qb.push_bind(like_pat.clone());
        qb.push(" OR node_key ILIKE ");
        qb.push_bind(like_pat);
        qb.push(")");
    }
    if let Some([min_lng, min_lat, max_lng, max_lat]) = bbox {
        if include_shape_without_point {
            qb.push(" AND ((geo_lng IS NOT NULL AND geo_lat IS NOT NULL AND geo_lng BETWEEN ");
            qb.push_bind(min_lng);
            qb.push(" AND ");
            qb.push_bind(max_lng);
            qb.push(" AND geo_lat BETWEEN ");
            qb.push_bind(min_lat);
            qb.push(" AND ");
            qb.push_bind(max_lat);
            qb.push(") OR (geo_lng IS NULL AND geo_shape IS NOT NULL))");
        } else {
            qb.push(" AND geo_lng IS NOT NULL AND geo_lat IS NOT NULL AND geo_lng BETWEEN ");
            qb.push_bind(min_lng);
            qb.push(" AND ");
            qb.push_bind(max_lng);
            qb.push(" AND geo_lat BETWEEN ");
            qb.push_bind(min_lat);
            qb.push(" AND ");
            qb.push_bind(max_lat);
        }
    }
    if let Some((cursor_ts, cursor_id)) = cursor_pair {
        qb.push(" AND (updated_at, id) < (");
        qb.push_bind(cursor_ts);
        qb.push(", ");
        qb.push_bind(cursor_id);
        qb.push(")");
    }
    if let Some(policies) = node_type_policies {
        append_pg_row_policy_case(&mut qb, policies);
    }
    qb
}

fn append_pg_row_policy_case(
    qb: &mut QueryBuilder<Postgres>,
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
        qb.push(" WHEN type_name = ");
        qb.push_bind(tname.clone());
        qb.push(" THEN (");
        qb.push(pred.as_str());
        qb.push(")");
    }
    qb.push(" ELSE TRUE END)");
}

impl GraphStorePort for PgStore {
    async fn list_nodes(
        &self,
        tenant_id: &str,
        query: NodeListQuery,
    ) -> Result<NodeListResponse, AppError> {
        self.list_nodes_with_row_policies(tenant_id, query, None)
            .await
    }

    async fn get_node(&self, tenant_id: &str, node_id: Uuid) -> Result<Option<BlueNode>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, type_name, node_key, title, properties, metrics,
                    geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key,
                    row_version, created_at, updated_at
               FROM xb_nodes
              WHERE id = $1 AND subgraph_slug = 'main'",
        )
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xb_nodes failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_node tx failed: {err}")))?;

        Ok(maybe_row.as_ref().map(decode_blue_node))
    }

    async fn upsert_node(
        &self,
        tenant_id: &str,
        req: UpsertBlueNodeRequest,
    ) -> Result<BlueNode, AppError> {
        let type_name = req.type_name.trim();
        let node_key = req.node_key.trim();
        let title = req.title.trim();
        if type_name.is_empty() || node_key.is_empty() || title.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "type_name, node_key, and title must be non-empty",
            ));
        }
        if let Some(geo) = req.geo {
            geo.validate()?;
        }
        let shape_json = serialize_validated_geo_shape(req.geo_shape.as_ref())?;
        validate_finite_metrics(&req.metrics)?;

        let link_spec = validate_upsert_link_spec(&req)?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_node_type_exists_tx(&mut tx, type_name).await?;
        let resolved_link = resolve_upsert_link_tx(&mut tx, link_spec).await?;
        let (final_props, final_metrics, metrics_changed) =
            merge_single_upsert_node_tx(&mut tx, type_name, node_key, &req).await?;

        let props_json = serde_json::Value::Object(final_props);
        let metrics_json = serde_json::to_value(&final_metrics)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid metrics: {err}")))?;
        let geo_lng = req.geo.map(|g| g.lng);
        let geo_lat = req.geo.map(|g| g.lat);

        let row = sqlx::query(
            "INSERT INTO xb_nodes (
                 tenant_id, type_name, node_key, title, properties, metrics,
                 geo_lng, geo_lat, geo_shape
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (tenant_id, subgraph_slug, type_name, node_key) DO UPDATE SET
                 title = EXCLUDED.title,
                 properties = EXCLUDED.properties,
                 metrics = EXCLUDED.metrics,
                 geo_lng = COALESCE(EXCLUDED.geo_lng, xb_nodes.geo_lng),
                 geo_lat = COALESCE(EXCLUDED.geo_lat, xb_nodes.geo_lat),
                 geo_shape = COALESCE(EXCLUDED.geo_shape, xb_nodes.geo_shape),
                 row_version = xb_nodes.row_version + 1,
                 updated_at = NOW()
             RETURNING id, type_name, node_key, title, properties, metrics,
                       geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key,
                       row_version, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(type_name)
        .bind(node_key)
        .bind(title)
        .bind(props_json)
        .bind(metrics_json)
        .bind(geo_lng)
        .bind(geo_lat)
        .bind(shape_json)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_nodes failed: {err}")))?;

        let node = decode_blue_node(&row);

        if let Some((target_id, ltype, weight)) = resolved_link {
            sqlx::query(
                "INSERT INTO xb_links (tenant_id, link_type, src_node_id, dst_node_id, weight)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id) DO UPDATE SET
                     weight = EXCLUDED.weight,
                     updated_at = NOW()",
            )
            .bind(tenant_id)
            .bind(ltype)
            .bind(node.id)
            .bind(target_id)
            .bind(weight)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("upsert linked xb_links failed: {err}")))?;
        }

        emit_node_side_effects_tx(&mut tx, tenant_id, &node, metrics_changed).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit upsert_node tx failed: {err}")))?;

        Ok(node)
    }

    async fn patch_node(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        req: PatchNodeRequest,
    ) -> Result<BlueNode, AppError> {
        validate_patch_node_request(&req)?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing_row = sqlx::query(
            "SELECT id, type_name, node_key, title, properties, metrics,
                    geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key,
                    row_version, created_at, updated_at
               FROM xb_nodes
              WHERE id = $1 AND subgraph_slug = 'main'
              FOR UPDATE",
        )
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock xb_nodes for patch: {err}")))?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

        let current = decode_blue_node(&existing_row);
        if let Some(expected_ver) = req.if_row_version
            && current.row_version != expected_ver
        {
            return Err(AppError::Conflict("VERSION_CONFLICT"));
        }

        let (new_title, props_json, metrics_json, new_geo, shape_json, metrics_changed) =
            prepare_patched_node_fields(current, req)?;

        let updated_row = sqlx::query(
            "UPDATE xb_nodes
                SET title = $2,
                    properties = $3,
                    metrics = $4,
                    geo_lng = $5,
                    geo_lat = $6,
                    geo_shape = $7,
                    row_version = row_version + 1,
                    updated_at = NOW()
              WHERE id = $1 AND subgraph_slug = 'main'
              RETURNING id, type_name, node_key, title, properties, metrics,
                        geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key,
                        row_version, created_at, updated_at",
        )
        .bind(node_id)
        .bind(new_title)
        .bind(props_json)
        .bind(metrics_json)
        .bind(new_geo.map(|g| g.lng))
        .bind(new_geo.map(|g| g.lat))
        .bind(shape_json)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("patch xb_nodes failed: {err}")))?;

        let updated_node = decode_blue_node(&updated_row);
        emit_node_side_effects_tx(&mut tx, tenant_id, &updated_node, metrics_changed).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit patch_node tx failed: {err}")))?;
        Ok(updated_node)
    }

    async fn delete_node(&self, tenant_id: &str, node_id: Uuid) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let peer_rows = sqlx::query(
            "SELECT DISTINCT CASE WHEN src_node_id = $1 THEN dst_node_id ELSE src_node_id END AS peer_id
               FROM xb_links
              WHERE subgraph_slug = 'main'
                AND (src_node_id = $1 OR dst_node_id = $1)
                AND src_node_id <> dst_node_id",
        )
        .bind(node_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query peer links before delete_node: {err}")))?;
        let peer_ids: Vec<Uuid> = peer_rows.into_iter().map(|r| r.get("peer_id")).collect();

        let deleted = sqlx::query(
            "DELETE FROM xb_nodes WHERE id = $1 AND subgraph_slug = 'main' RETURNING id",
        )
        .bind(node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete xb_nodes failed: {err}")))?;

        if deleted.is_none() {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }

        sqlx::query("DELETE FROM xb_metric_series WHERE node_id = $1")
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete node metric_series: {err}")))?;

        sqlx::query("DELETE FROM xb_metric_rollups WHERE node_id = $1")
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete node metric_rollups: {err}")))?;

        sqlx::query("DELETE FROM xb_rollup_dirty WHERE node_id = $1")
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete node rollup_dirty: {err}")))?;

        sqlx::query("DELETE FROM hg_embeddings WHERE node_type = 'XB_NODE' AND node_id = $1")
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete node embeddings: {err}")))?;

        insert_outbox_tx(
            &mut tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeUpserted,
                payload: serde_json::json!({ "deleted_node_id": node_id }),
            },
        )
        .await?;

        if !peer_ids.is_empty() {
            insert_outbox_tx(
                &mut tx,
                tenant_id,
                &NewOutboxEvent {
                    topic: OutboxTopic::NodeMetricsChanged,
                    payload: serde_json::json!({ "node_ids": peer_ids }),
                },
            )
            .await?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_node tx failed: {err}")))?;
        Ok(())
    }

    async fn bulk_upsert(
        &self,
        tenant_id: &str,
        req: BulkNodesRequest,
    ) -> Result<BulkNodesResponse, AppError> {
        if req.nodes.len() > 5_000 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "bulk_upsert accepts at most 5,000 nodes, got {}",
                    req.nodes.len()
                ),
            ));
        }
        if req.links.len() > 20_000 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "bulk_upsert accepts at most 20,000 links, got {}",
                    req.links.len()
                ),
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let (upserted_nodes, metrics_changed_ids) =
            bulk_upsert_nodes_tx(&mut tx, tenant_id, &req.nodes).await?;
        let (upserted_links, unresolved) =
            bulk_upsert_links_tx(&mut tx, tenant_id, &req.links).await?;

        for chunk in metrics_changed_ids.chunks(1_000) {
            insert_outbox_tx(
                &mut tx,
                tenant_id,
                &NewOutboxEvent {
                    topic: OutboxTopic::NodeMetricsChanged,
                    payload: serde_json::json!({ "node_ids": chunk }),
                },
            )
            .await?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit bulk_upsert tx failed: {err}")))?;

        Ok(BulkNodesResponse {
            upserted_nodes,
            upserted_links,
            unresolved,
        })
    }

    async fn list_links(
        &self,
        tenant_id: &str,
        query: LinkListQuery,
    ) -> Result<Vec<LinkRecord>, AppError> {
        let limit = query.limit.unwrap_or(500);
        if !(1..=5_000).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=5000, got {limit}"),
            ));
        }

        let ltype = query
            .link_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, link_type, src_node_id, dst_node_id, weight,
                    properties, metrics, row_version, source_dataset_id, valid_from, valid_to,
                    created_at, updated_at
               FROM xb_links
              WHERE subgraph_slug = 'main'
                AND ($1::uuid IS NULL OR src_node_id = $1 OR dst_node_id = $1)
                AND ($2::text IS NULL OR link_type = $2)
              ORDER BY updated_at DESC, id DESC
              LIMIT $3",
        )
        .bind(query.node_id)
        .bind(ltype)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xb_links failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_links tx failed: {err}")))?;

        Ok(rows.iter().map(decode_link_record).collect())
    }

    async fn upsert_link(
        &self,
        tenant_id: &str,
        req: UpsertLinkRequest,
    ) -> Result<LinkRecord, AppError> {
        let ltype = req.link_type.trim();
        if ltype.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "link_type cannot be empty",
            ));
        }
        if let Some(w) = req.weight
            && (!w.is_finite() || w <= 0.0)
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "weight must be a positive finite number",
            ));
        }

        validate_finite_metrics(&req.metrics)?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let default_w = lookup_edge_impact_weight_tx(&mut tx, ltype).await?;
        let weight = req.weight.unwrap_or(default_w);

        let endpoints_exist: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM xb_nodes WHERE id = $1 AND subgraph_slug = 'main')
                AND EXISTS(SELECT 1 FROM xb_nodes WHERE id = $2 AND subgraph_slug = 'main')",
        )
        .bind(req.src_node_id)
        .bind(req.dst_node_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("check link endpoints failed: {err}")))?;

        if !endpoints_exist {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }

        let props_json = serde_json::Value::Object(req.properties);
        let metrics_json = serde_json::to_value(&req.metrics).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid link metrics: {err}"))
        })?;
        let row = sqlx::query(
            "INSERT INTO xb_links (
                 tenant_id, link_type, src_node_id, dst_node_id, weight,
                 properties, metrics, valid_from, valid_to
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id) DO UPDATE SET
                 weight = EXCLUDED.weight,
                 properties = EXCLUDED.properties,
                 metrics = EXCLUDED.metrics,
                 valid_from = EXCLUDED.valid_from,
                 valid_to = EXCLUDED.valid_to,
                 row_version = xb_links.row_version + 1,
                 updated_at = NOW()
             RETURNING id, link_type, src_node_id, dst_node_id, weight,
                       properties, metrics, row_version, source_dataset_id,
                       valid_from, valid_to, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(ltype)
        .bind(req.src_node_id)
        .bind(req.dst_node_id)
        .bind(weight)
        .bind(props_json)
        .bind(metrics_json)
        .bind(req.valid_from)
        .bind(req.valid_to)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_links failed: {err}")))?;

        insert_outbox_tx(
            &mut tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeMetricsChanged,
                payload: serde_json::json!({
                    "node_ids": [req.src_node_id, req.dst_node_id],
                }),
            },
        )
        .await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit upsert_link tx failed: {err}")))?;

        Ok(decode_link_record(&row))
    }

    async fn delete_link(&self, tenant_id: &str, link_id: Uuid) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let deleted_row = sqlx::query(
            "DELETE FROM xb_links WHERE id = $1 AND subgraph_slug = 'main' RETURNING src_node_id, dst_node_id",
        )
        .bind(link_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete xb_links failed: {err}")))?;

        let Some(row) = deleted_row else {
            return Err(AppError::NotFound("LINK_NOT_FOUND"));
        };
        let src_node_id: Uuid = row.get("src_node_id");
        let dst_node_id: Uuid = row.get("dst_node_id");

        insert_outbox_tx(
            &mut tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeMetricsChanged,
                payload: serde_json::json!({
                    "node_ids": [src_node_id, dst_node_id],
                }),
            },
        )
        .await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_link tx failed: {err}")))?;
        Ok(())
    }

    async fn search_nodes(
        &self,
        tenant_id: &str,
        query: NodeSearchQuery,
        query_embedding: Option<&[f32]>,
    ) -> Result<NodeSearchResponse, AppError> {
        if !(1..=100).contains(&query.k) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("k must be in 1..=100, got {}", query.k),
            ));
        }

        let q_trimmed = query.q.trim();
        let has_vec = query_embedding.is_some_and(|s| !s.is_empty());
        if q_trimmed.is_empty() && !has_vec {
            return Ok(NodeSearchResponse { items: Vec::new() });
        }

        let type_filter = query
            .type_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let fts_nodes = if q_trimmed.is_empty() {
            Vec::new()
        } else {
            query_fts_nodes_tx(&mut tx, q_trimmed, type_filter.as_deref()).await?
        };

        let vec_nodes = if let Some(slice) = query_embedding
            && !slice.is_empty()
        {
            query_vector_nodes_tx(&mut tx, slice, type_filter.as_deref()).await?
        } else {
            Vec::new()
        };

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit search_nodes tx failed: {err}")))?;

        let k_usize = usize::try_from(query.k).unwrap_or(20);
        Ok(fuse_node_search_hits(fts_nodes, vec_nodes, k_usize))
    }

    async fn list_node_documents(
        &self,
        tenant_id: &str,
        node_id: Uuid,
    ) -> Result<NodeDocumentsResponse, AppError> {
        crate::entity_link::list_node_documents_impl(self, tenant_id, node_id).await
    }

    async fn resolve_entity_links(
        &self,
        tenant_id: &str,
        req: ResolveEntitiesRequest,
    ) -> Result<ResolveEntitiesResponse, AppError> {
        crate::entity_link::resolve_entity_links_impl(self, tenant_id, req).await
    }

    async fn list_entity_candidates(
        &self,
        tenant_id: &str,
    ) -> Result<EntityCandidatesResponse, AppError> {
        crate::entity_link::list_entity_candidates_impl(self, tenant_id).await
    }

    async fn confirm_entity_link(
        &self,
        tenant_id: &str,
        entity_id: Uuid,
        node_id: Uuid,
    ) -> Result<EntityLink, AppError> {
        crate::entity_link::confirm_or_reject_entity_link_impl(
            self, tenant_id, entity_id, node_id, true,
        )
        .await
    }

    async fn reject_entity_link(
        &self,
        tenant_id: &str,
        entity_id: Uuid,
        node_id: Uuid,
    ) -> Result<EntityLink, AppError> {
        crate::entity_link::confirm_or_reject_entity_link_impl(
            self, tenant_id, entity_id, node_id, false,
        )
        .await
    }
}

async fn emit_node_side_effects_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node: &BlueNode,
    metrics_changed: bool,
) -> Result<(), AppError> {
    insert_outbox_tx(
        tx,
        tenant_id,
        &NewOutboxEvent {
            topic: OutboxTopic::NodeUpserted,
            payload: serde_json::json!({
                "node_id": node.id,
                "type_name": node.type_name,
            }),
        },
    )
    .await?;

    if metrics_changed {
        insert_outbox_tx(
            tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeMetricsChanged,
                payload: serde_json::json!({ "node_ids": [node.id] }),
            },
        )
        .await?;
    }

    let embed_job = NewJob::new(
        JobKind::EmbedNodes,
        serde_json::json!({ "node_ids": [node.id] }),
    )
    .with_idempotency_key(format!("{tenant_id}:{}:{}", node.id, node.row_version));
    enqueue_job_tx(tx, tenant_id, &embed_job).await?;
    Ok(())
}

async fn load_incident_blue_links_tx(
    tx: &mut Transaction<'_, Postgres>,
    node_ids: &[Uuid],
    subgraph_slug: &str,
) -> Result<Vec<BlueLink>, AppError> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let link_rows = sqlx::query(
        "SELECT id, link_type, src_node_id, dst_node_id, weight
           FROM xb_links
          WHERE subgraph_slug = $2 AND src_node_id = ANY($1) AND dst_node_id = ANY($1)",
    )
    .bind(node_ids)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("list incident xb_links failed: {err}")))?;

    Ok(link_rows
        .into_iter()
        .map(|r| BlueLink {
            id: r.get("id"),
            link_type: r.get("link_type"),
            src_node_id: r.get("src_node_id"),
            dst_node_id: r.get("dst_node_id"),
            weight: r.get("weight"),
        })
        .collect())
}

fn validate_upsert_link_spec(
    req: &UpsertBlueNodeRequest,
) -> Result<Option<(Uuid, String, Option<f64>)>, AppError> {
    let Some(target_id) = req.link_to_node_id else {
        return Ok(None);
    };
    let ltype = req
        .link_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "link_type is required when link_to_node_id is provided",
            )
        })?;
    if let Some(w) = req.link_weight
        && (!w.is_finite() || w <= 0.0)
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "link_weight must be a positive finite number",
        ));
    }
    Ok(Some((target_id, String::from(ltype), req.link_weight)))
}

async fn resolve_upsert_link_tx(
    tx: &mut Transaction<'_, Postgres>,
    link_spec: Option<(Uuid, String, Option<f64>)>,
) -> Result<Option<(Uuid, String, f64)>, AppError> {
    let Some((target_id, ltype, explicit_weight)) = link_spec else {
        return Ok(None);
    };
    let default_w = lookup_edge_impact_weight_tx(tx, &ltype).await?;
    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xb_nodes WHERE id = $1 AND subgraph_slug = 'main')",
    )
    .bind(target_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("verify link target: {err}")))?;
    if !target_exists {
        return Err(AppError::NotFound("NODE_NOT_FOUND"));
    }
    Ok(Some((
        target_id,
        ltype,
        explicit_weight.unwrap_or(default_w),
    )))
}

async fn merge_single_upsert_node_tx(
    tx: &mut Transaction<'_, Postgres>,
    type_name: &str,
    node_key: &str,
    req: &UpsertBlueNodeRequest,
) -> Result<
    (
        serde_json::Map<String, serde_json::Value>,
        BTreeMap<String, f64>,
        bool,
    ),
    AppError,
> {
    let existing = sqlx::query(
        "SELECT id, properties, metrics, row_version
           FROM xb_nodes
          WHERE subgraph_slug = 'main' AND type_name = $1 AND node_key = $2
          FOR UPDATE",
    )
    .bind(type_name)
    .bind(node_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock existing xb_node failed: {err}")))?;

    if let Some(ref ex_row) = existing {
        let ex_ver: i64 = ex_row.get("row_version");
        if let Some(expected_ver) = req.if_row_version
            && ex_ver != expected_ver
        {
            return Err(AppError::Conflict("VERSION_CONFLICT"));
        }
        let ex_metrics_val: serde_json::Value = ex_row.get("metrics");
        let ex_metrics: BTreeMap<String, f64> =
            serde_json::from_value(ex_metrics_val).unwrap_or_default();
        let merged_metrics = match req.metrics_merge {
            MetricsMerge::Merge => {
                let mut m = ex_metrics.clone();
                for (k, v) in &req.metrics {
                    m.insert(k.clone(), *v);
                }
                m
            }
            MetricsMerge::Replace => req.metrics.clone(),
        };
        let mut props = req.properties.clone();
        if let serde_json::Value::Object(ex_props) =
            ex_row.get::<serde_json::Value, _>("properties")
            && let Some(ts_meta) = ex_props.get("__metric_ts")
            && !props.contains_key("__metric_ts")
        {
            props.insert(String::from("__metric_ts"), ts_meta.clone());
        }
        let changed = merged_metrics != ex_metrics;
        Ok((props, merged_metrics, changed))
    } else {
        if let Some(expected_ver) = req.if_row_version
            && expected_ver != 0
        {
            return Err(AppError::Conflict("VERSION_CONFLICT"));
        }
        let changed = !req.metrics.is_empty();
        Ok((req.properties.clone(), req.metrics.clone(), changed))
    }
}

fn serialize_validated_geo_shape(
    shape: Option<&GeoShapeDto>,
) -> Result<Option<serde_json::Value>, AppError> {
    let Some(s) = shape else {
        return Ok(None);
    };
    validate_geo_shape(s)?;
    let val = serde_json::to_value(s)
        .map_err(|err| AppError::bad("INVALID_GEOMETRY", format!("serialize geo_shape: {err}")))?;
    Ok(Some(val))
}

fn validate_patch_node_request(req: &PatchNodeRequest) -> Result<(), AppError> {
    if let Some(ref title) = req.title
        && title.trim().is_empty()
    {
        return Err(AppError::bad("INVALID_ARGUMENT", "title cannot be empty"));
    }
    if let Some(geo) = req.geo {
        geo.validate()?;
    }
    if let Some(ref shape) = req.geo_shape {
        validate_geo_shape(shape)?;
    }
    if let Some(ref m) = req.metrics {
        validate_finite_metrics(m)?;
    }
    Ok(())
}

type PatchedNodeFields = (
    String,
    serde_json::Value,
    serde_json::Value,
    Option<GeoPoint>,
    Option<serde_json::Value>,
    bool,
);

fn prepare_patched_node_fields(
    current: BlueNode,
    req: PatchNodeRequest,
) -> Result<PatchedNodeFields, AppError> {
    let new_title = req
        .title
        .as_deref()
        .map(str::trim)
        .map(String::from)
        .unwrap_or(current.title);

    let new_props = match req.properties {
        Some(mut p) => {
            if let Some(ts_meta) = current.properties.get("__metric_ts")
                && !p.contains_key("__metric_ts")
            {
                p.insert(String::from("__metric_ts"), ts_meta.clone());
            }
            p
        }
        None => current.properties,
    };

    let (new_metrics, metrics_changed) = match req.metrics {
        Some(incoming) => {
            let merged = match req.metrics_merge {
                MetricsMerge::Merge => {
                    let mut m = current.metrics.clone();
                    for (k, v) in incoming {
                        m.insert(k, v);
                    }
                    m
                }
                MetricsMerge::Replace => incoming,
            };
            let changed = merged != current.metrics;
            (merged, changed)
        }
        None => (current.metrics, false),
    };

    let new_geo = req.geo.or(current.geo);
    let new_shape = req.geo_shape.or(current.geo_shape);
    let shape_json = serialize_validated_geo_shape(new_shape.as_ref())?;
    let props_json = serde_json::Value::Object(new_props);
    let metrics_json = serde_json::to_value(&new_metrics)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid metrics: {err}")))?;

    Ok((
        new_title,
        props_json,
        metrics_json,
        new_geo,
        shape_json,
        metrics_changed,
    ))
}

type DedupedBulkNodesMap = BTreeMap<(String, String), BulkNodeInput>;

fn dedup_and_validate_bulk_nodes(
    inputs: &[BulkNodeInput],
) -> Result<(Vec<String>, DedupedBulkNodesMap), AppError> {
    let mut distinct_types = BTreeSet::new();
    let mut deduped: DedupedBulkNodesMap = BTreeMap::new();

    for raw in inputs {
        let tname = raw.type_name.trim();
        let nkey = raw.node_key.trim();
        let title = raw.title.trim();
        if tname.is_empty() || nkey.is_empty() || title.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "bulk node type_name, node_key, and title must be non-empty",
            ));
        }
        if let Some(geo) = raw.geo {
            geo.validate()?;
        }
        if let Some(ref shape) = raw.geo_shape {
            validate_geo_shape(shape)?;
        }
        validate_finite_metrics(&raw.metrics)?;
        distinct_types.insert(String::from(tname));

        let key_pair = (String::from(tname), String::from(nkey));
        if let Some(prev) = deduped.get_mut(&key_pair) {
            prev.title = String::from(title);
            prev.properties.clone_from(&raw.properties);
            prev.geo = raw.geo.or(prev.geo);
            prev.geo_shape = raw.geo_shape.clone().or_else(|| prev.geo_shape.clone());
            prev.source_dataset_id = raw.source_dataset_id.or(prev.source_dataset_id);
            prev.source_row_key = raw
                .source_row_key
                .clone()
                .or_else(|| prev.source_row_key.clone());
            match raw.metrics_merge {
                MetricsMerge::Merge => {
                    for (mk, mv) in &raw.metrics {
                        prev.metrics.insert(mk.clone(), *mv);
                    }
                }
                MetricsMerge::Replace => {
                    prev.metrics.clone_from(&raw.metrics);
                    prev.metrics_merge = MetricsMerge::Replace;
                }
            }
        } else {
            let mut normalized = raw.clone();
            normalized.type_name = String::from(tname);
            normalized.node_key = String::from(nkey);
            normalized.title = String::from(title);
            deduped.insert(key_pair, normalized);
        }
    }

    Ok((distinct_types.into_iter().collect(), deduped))
}

type PrevNodeMetricsMap = HashMap<(String, String), (BTreeMap<String, f64>, serde_json::Value)>;

async fn preload_bulk_nodes_state_tx(
    tx: &mut Transaction<'_, Postgres>,
    type_vec: &[String],
    deduped: &BTreeMap<(String, String), BulkNodeInput>,
) -> Result<PrevNodeMetricsMap, AppError> {
    let existing_type_rows = sqlx::query(
        "SELECT name FROM xb_node_types WHERE subgraph_slug = 'main' AND name = ANY($1)",
    )
    .bind(type_vec)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("validate bulk xb_node_types: {err}")))?;
    let valid_types: BTreeSet<String> = existing_type_rows
        .into_iter()
        .map(|r| r.get("name"))
        .collect();
    for t in type_vec {
        if !valid_types.contains(t) {
            return Err(AppError::bad(
                "SCHEMA_INVALID",
                format!("node type '{t}' is not defined in xBlueStudio schema"),
            ));
        }
    }

    let probe_types: Vec<String> = deduped.keys().map(|(t, _)| t.clone()).collect();
    let probe_keys: Vec<String> = deduped.keys().map(|(_, k)| k.clone()).collect();
    let prev_rows = sqlx::query(
        "SELECT x.type_name, x.node_key, x.metrics, x.properties
           FROM xb_nodes x
           JOIN UNNEST($1::text[], $2::text[]) AS p(type_name, node_key)
             ON x.type_name = p.type_name AND x.node_key = p.node_key
          WHERE x.subgraph_slug = 'main'",
    )
    .bind(&probe_types)
    .bind(&probe_keys)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("preload existing xb_nodes metrics: {err}")))?;

    let mut prev_map: PrevNodeMetricsMap = HashMap::with_capacity(prev_rows.len());
    for r in prev_rows {
        let m_val: serde_json::Value = r.get("metrics");
        let m: BTreeMap<String, f64> = serde_json::from_value(m_val).unwrap_or_default();
        prev_map.insert(
            (r.get("type_name"), r.get("node_key")),
            (m, r.get("properties")),
        );
    }
    Ok(prev_map)
}

async fn bulk_upsert_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    inputs: &[BulkNodeInput],
) -> Result<(usize, Vec<Uuid>), AppError> {
    if inputs.is_empty() {
        return Ok((0, Vec::new()));
    }

    let (type_vec, deduped) = dedup_and_validate_bulk_nodes(inputs)?;
    let prev_map = preload_bulk_nodes_state_tx(tx, &type_vec, &deduped).await?;

    let cap = deduped.len();
    let mut col_types = Vec::with_capacity(cap);
    let mut col_keys = Vec::with_capacity(cap);
    let mut col_titles = Vec::with_capacity(cap);
    let mut col_props = Vec::with_capacity(cap);
    let mut col_metrics = Vec::with_capacity(cap);
    let mut col_lngs: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut col_lats: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut col_shapes: Vec<Option<serde_json::Value>> = Vec::with_capacity(cap);
    let mut col_ds_ids: Vec<Option<Uuid>> = Vec::with_capacity(cap);
    let mut col_row_keys: Vec<Option<String>> = Vec::with_capacity(cap);
    let mut changed_keys: BTreeSet<(String, String)> = BTreeSet::new();

    for ((tname, nkey), item) in deduped {
        let shape_json = serialize_validated_geo_shape(item.geo_shape.as_ref())?;
        let (final_m, final_p, changed) =
            merge_bulk_node_item(item, prev_map.get(&(tname.clone(), nkey.clone())));
        if changed {
            changed_keys.insert((tname.clone(), nkey.clone()));
        }
        col_types.push(tname);
        col_keys.push(nkey);
        col_titles.push(final_p.0);
        col_props.push(serde_json::Value::Object(final_p.1));
        col_metrics.push(
            serde_json::to_value(&final_m).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid metrics: {err}"))
            })?,
        );
        col_lngs.push(final_p.2.map(|g| g.lng));
        col_lats.push(final_p.2.map(|g| g.lat));
        col_shapes.push(shape_json);
        col_ds_ids.push(final_p.3);
        col_row_keys.push(final_p.4);
    }

    let returned = sqlx::query(
        "INSERT INTO xb_nodes (
             tenant_id, type_name, node_key, title, properties, metrics,
             geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key
         )
         SELECT $1, u.type_name, u.node_key, u.title, u.properties, u.metrics,
                u.geo_lng, u.geo_lat, u.geo_shape, u.source_dataset_id, u.source_row_key
           FROM UNNEST(
               $2::text[], $3::text[], $4::text[], $5::jsonb[], $6::jsonb[],
               $7::float8[], $8::float8[], $9::jsonb[], $10::uuid[], $11::text[]
           ) AS u(
               type_name, node_key, title, properties, metrics,
               geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key
           )
         ON CONFLICT (tenant_id, subgraph_slug, type_name, node_key) DO UPDATE SET
             title = EXCLUDED.title, properties = EXCLUDED.properties, metrics = EXCLUDED.metrics,
             geo_lng = COALESCE(EXCLUDED.geo_lng, xb_nodes.geo_lng),
             geo_lat = COALESCE(EXCLUDED.geo_lat, xb_nodes.geo_lat),
             geo_shape = COALESCE(EXCLUDED.geo_shape, xb_nodes.geo_shape),
             source_dataset_id = COALESCE(EXCLUDED.source_dataset_id, xb_nodes.source_dataset_id),
             source_row_key = COALESCE(EXCLUDED.source_row_key, xb_nodes.source_row_key),
             row_version = xb_nodes.row_version + 1, updated_at = NOW()
         RETURNING id, type_name, node_key, row_version",
    )
    .bind(tenant_id)
    .bind(&col_types)
    .bind(&col_keys)
    .bind(&col_titles)
    .bind(&col_props)
    .bind(&col_metrics)
    .bind(&col_lngs)
    .bind(&col_lats)
    .bind(&col_shapes)
    .bind(&col_ds_ids)
    .bind(&col_row_keys)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("bulk UNNEST xb_nodes failed: {err}")))?;

    emit_bulk_nodes_post_effects_tx(tx, tenant_id, &returned, &changed_keys).await
}

type BulkNodeRestFields = (
    String,
    serde_json::Map<String, serde_json::Value>,
    Option<GeoPoint>,
    Option<Uuid>,
    Option<String>,
);

fn merge_bulk_node_item(
    item: BulkNodeInput,
    prev: Option<&(BTreeMap<String, f64>, serde_json::Value)>,
) -> (BTreeMap<String, f64>, BulkNodeRestFields, bool) {
    let (final_m, final_p, changed) = if let Some((ex_m, ex_p)) = prev {
        let m = match item.metrics_merge {
            MetricsMerge::Merge => {
                let mut merged = ex_m.clone();
                for (k, v) in &item.metrics {
                    merged.insert(k.clone(), *v);
                }
                merged
            }
            MetricsMerge::Replace => item.metrics,
        };
        let mut p = item.properties;
        if let serde_json::Value::Object(ex_obj) = ex_p
            && let Some(ts_meta) = ex_obj.get("__metric_ts")
            && !p.contains_key("__metric_ts")
        {
            p.insert(String::from("__metric_ts"), ts_meta.clone());
        }
        let is_diff = &m != ex_m;
        (m, p, is_diff)
    } else {
        let is_diff = !item.metrics.is_empty();
        (item.metrics, item.properties, is_diff)
    };

    (
        final_m,
        (
            item.title,
            final_p,
            item.geo,
            item.source_dataset_id,
            item.source_row_key,
        ),
        changed,
    )
}

async fn emit_bulk_nodes_post_effects_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    returned: &[sqlx::postgres::PgRow],
    changed_keys: &BTreeSet<(String, String)>,
) -> Result<(usize, Vec<Uuid>), AppError> {
    let mut all_node_ids = Vec::with_capacity(returned.len());
    let mut metrics_changed_ids = Vec::new();
    let mut max_ver: i64 = 1;

    for r in returned {
        let nid: Uuid = r.get("id");
        let tname: String = r.get("type_name");
        let nkey: String = r.get("node_key");
        let rver: i64 = r.get("row_version");
        if rver > max_ver {
            max_ver = rver;
        }
        all_node_ids.push(nid);
        if changed_keys.contains(&(tname.clone(), nkey)) {
            metrics_changed_ids.push(nid);
        }
        insert_outbox_tx(
            tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeUpserted,
                payload: serde_json::json!({
                    "node_id": nid,
                    "type_name": tname,
                }),
            },
        )
        .await?;
    }

    for chunk in all_node_ids.chunks(500) {
        if let Some(&first_id) = chunk.first() {
            let job = NewJob::new(
                JobKind::EmbedNodes,
                serde_json::json!({ "node_ids": chunk }),
            )
            .with_idempotency_key(format!(
                "{tenant_id}:bulk:{first_id}:{max_ver}:{}",
                chunk.len()
            ));
            enqueue_job_tx(tx, tenant_id, &job).await?;
        }
    }

    Ok((returned.len(), metrics_changed_ids))
}

type BulkLinkLookups = (
    HashMap<String, f64>,
    BTreeSet<Uuid>,
    HashMap<(String, String), Uuid>,
);

async fn load_bulk_link_lookups_tx(
    tx: &mut Transaction<'_, Postgres>,
    inputs: &[BulkLinkInput],
) -> Result<BulkLinkLookups, AppError> {
    let mut distinct_link_types = BTreeSet::new();
    let mut needed_ids = BTreeSet::new();
    let mut needed_keys = BTreeSet::new();

    for l in inputs {
        let lt = l.link_type.trim();
        if lt.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "bulk link_type cannot be empty",
            ));
        }
        if let Some(w) = l.weight
            && (!w.is_finite() || w <= 0.0)
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "bulk link weight must be positive finite",
            ));
        }
        distinct_link_types.insert(String::from(lt));
        collect_endpoint_probe(&l.src, &mut needed_ids, &mut needed_keys);
        collect_endpoint_probe(&l.dst, &mut needed_ids, &mut needed_keys);
    }

    let lt_vec: Vec<String> = distinct_link_types.into_iter().collect();
    let edge_rows = sqlx::query(
        "SELECT DISTINCT ON (name) name, impact_weight
           FROM xb_edge_types
          WHERE subgraph_slug = 'main' AND name = ANY($1)
          ORDER BY name, created_at ASC",
    )
    .bind(&lt_vec)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("validate bulk xb_edge_types: {err}")))?;

    let mut edge_weights: HashMap<String, f64> = HashMap::with_capacity(edge_rows.len());
    for r in edge_rows {
        edge_weights.insert(r.get("name"), r.get("impact_weight"));
    }
    for lt in &lt_vec {
        if !edge_weights.contains_key(lt) {
            return Err(AppError::bad(
                "SCHEMA_INVALID",
                format!("edge type '{lt}' is not defined in xBlueStudio schema"),
            ));
        }
    }

    let existing_ids: BTreeSet<Uuid> = if needed_ids.is_empty() {
        BTreeSet::new()
    } else {
        let id_vec: Vec<Uuid> = needed_ids.into_iter().collect();
        let rows =
            sqlx::query("SELECT id FROM xb_nodes WHERE id = ANY($1) AND subgraph_slug = 'main'")
                .bind(&id_vec)
                .fetch_all(&mut **tx)
                .await
                .map_err(|err| AppError::Database(format!("resolve bulk link UUIDs: {err}")))?;
        rows.into_iter().map(|r| r.get("id")).collect()
    };

    let key_to_id: HashMap<(String, String), Uuid> = if needed_keys.is_empty() {
        HashMap::new()
    } else {
        let kt: Vec<String> = needed_keys.iter().map(|(t, _)| t.clone()).collect();
        let kk: Vec<String> = needed_keys.iter().map(|(_, k)| k.clone()).collect();
        let rows = sqlx::query(
            "SELECT x.id, x.type_name, x.node_key
               FROM xb_nodes x
               JOIN UNNEST($1::text[], $2::text[]) AS p(type_name, node_key)
                 ON x.type_name = p.type_name AND x.node_key = p.node_key
              WHERE x.subgraph_slug = 'main'",
        )
        .bind(&kt)
        .bind(&kk)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("resolve bulk link NodeRefs: {err}")))?;
        rows.into_iter()
            .map(|r| ((r.get("type_name"), r.get("node_key")), r.get("id")))
            .collect()
    };

    Ok((edge_weights, existing_ids, key_to_id))
}

type DedupedLinkPayload = (
    f64,
    serde_json::Value,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

async fn bulk_upsert_links_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    inputs: &[BulkLinkInput],
) -> Result<(usize, Vec<UnresolvedLink>), AppError> {
    if inputs.is_empty() {
        return Ok((0, Vec::new()));
    }

    let (edge_weights, existing_ids, key_to_id) = load_bulk_link_lookups_tx(tx, inputs).await?;
    let mut unresolved = Vec::new();
    let mut deduped_links: BTreeMap<(String, Uuid, Uuid), DedupedLinkPayload> = BTreeMap::new();

    for (index, item) in inputs.iter().enumerate() {
        let src_id = resolve_endpoint(&item.src, &existing_ids, &key_to_id);
        let dst_id = resolve_endpoint(&item.dst, &existing_ids, &key_to_id);
        let (Some(sid), Some(did)) = (src_id, dst_id) else {
            let reason = match (src_id, dst_id) {
                (None, None) => String::from("both src and dst endpoints not found"),
                (None, Some(_)) => String::from("src endpoint not found"),
                (Some(_), None) => String::from("dst endpoint not found"),
                (Some(_), Some(_)) => String::new(),
            };
            unresolved.push(UnresolvedLink {
                index,
                link_type: item.link_type.clone(),
                reason,
            });
            continue;
        };

        let lt = String::from(item.link_type.trim());
        let default_w = edge_weights.get(&lt).copied().unwrap_or(0.5);
        let w = item.weight.unwrap_or(default_w);
        deduped_links.insert(
            (lt, sid, did),
            (
                w,
                serde_json::Value::Object(item.properties.clone()),
                item.valid_from,
                item.valid_to,
            ),
        );
    }

    if deduped_links.is_empty() {
        return Ok((0, unresolved));
    }

    let cap = deduped_links.len();
    let mut l_types = Vec::with_capacity(cap);
    let mut l_srcs = Vec::with_capacity(cap);
    let mut l_dsts = Vec::with_capacity(cap);
    let mut l_weights = Vec::with_capacity(cap);
    let mut l_props = Vec::with_capacity(cap);
    let mut l_vfrom: Vec<Option<DateTime<Utc>>> = Vec::with_capacity(cap);
    let mut l_vto: Vec<Option<DateTime<Utc>>> = Vec::with_capacity(cap);

    for ((lt, sid, did), (w, p, vf, vt)) in deduped_links {
        l_types.push(lt);
        l_srcs.push(sid);
        l_dsts.push(did);
        l_weights.push(w);
        l_props.push(p);
        l_vfrom.push(vf);
        l_vto.push(vt);
    }

    let res = sqlx::query(
        "INSERT INTO xb_links (
             tenant_id, link_type, src_node_id, dst_node_id, weight, properties, valid_from, valid_to
         )
         SELECT $1, u.link_type, u.src_id, u.dst_id, u.weight, u.properties, u.valid_from, u.valid_to
           FROM UNNEST(
               $2::text[], $3::uuid[], $4::uuid[], $5::float8[],
               $6::jsonb[], $7::timestamptz[], $8::timestamptz[]
           ) AS u(link_type, src_id, dst_id, weight, properties, valid_from, valid_to)
         ON CONFLICT (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id) DO UPDATE SET
             weight = EXCLUDED.weight, properties = EXCLUDED.properties,
             valid_from = EXCLUDED.valid_from, valid_to = EXCLUDED.valid_to, updated_at = NOW()",
    )
    .bind(tenant_id)
    .bind(&l_types)
    .bind(&l_srcs)
    .bind(&l_dsts)
    .bind(&l_weights)
    .bind(&l_props)
    .bind(&l_vfrom)
    .bind(&l_vto)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("bulk UNNEST xb_links failed: {err}")))?;

    let affected = usize::try_from(res.rows_affected()).unwrap_or(cap);
    Ok((affected, unresolved))
}

fn collect_endpoint_probe(
    ep: &LinkEndpointRef,
    ids: &mut BTreeSet<Uuid>,
    keys: &mut BTreeSet<(String, String)>,
) {
    match ep {
        LinkEndpointRef::Id(id) => {
            ids.insert(*id);
        }
        LinkEndpointRef::Key(NodeRef {
            type_name,
            node_key,
        }) => {
            keys.insert((
                String::from(type_name.trim()),
                String::from(node_key.trim()),
            ));
        }
    }
}

fn resolve_endpoint(
    ep: &LinkEndpointRef,
    ids: &BTreeSet<Uuid>,
    keys: &HashMap<(String, String), Uuid>,
) -> Option<Uuid> {
    match ep {
        LinkEndpointRef::Id(id) => ids.contains(id).then_some(*id),
        LinkEndpointRef::Key(NodeRef {
            type_name,
            node_key,
        }) => keys
            .get(&(
                String::from(type_name.trim()),
                String::from(node_key.trim()),
            ))
            .copied(),
    }
}

async fn query_fts_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    q: &str,
    type_name: Option<&str>,
) -> Result<Vec<BlueNode>, AppError> {
    let like_pattern = format!("%{q}%");
    let rows = sqlx::query(
        "SELECT id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, geo_shape, source_dataset_id, source_row_key,
                row_version, created_at, updated_at,
                (
                    CASE WHEN lower(node_key) = lower($1) OR lower(title) = lower($1) THEN 10.0 ELSE 0.0 END
                    + (ts_rank_cd(fts, plainto_tsquery('simple', $1))::float8 * 5.0)
                    + (CASE WHEN title ILIKE $2
                            THEN char_length($1)::float8 / GREATEST(char_length(title), 1)::float8
                            ELSE 0.0 END)
                ) AS lex_score
           FROM xb_nodes
          WHERE subgraph_slug = 'main'
            AND ($3::text IS NULL OR type_name = $3)
            AND (fts @@ plainto_tsquery('simple', $1) OR title ILIKE $2 OR node_key ILIKE $2)
          ORDER BY lex_score DESC, updated_at DESC
          LIMIT 50",
    )
    .bind(q)
    .bind(like_pattern)
    .bind(type_name)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("search_nodes FTS query failed: {err}")))?;

    Ok(rows.iter().map(decode_blue_node).collect())
}

async fn query_vector_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    query_vec: &[f32],
    type_name: Option<&str>,
) -> Result<Vec<BlueNode>, AppError> {
    let dim_i32 = i32::try_from(query_vec.len()).map_err(|err| {
        AppError::bad("INVALID_ARGUMENT", format!("invalid embedding dim: {err}"))
    })?;

    let rows = sqlx::query(
        "SELECT n.id, n.type_name, n.node_key, n.title, n.properties, n.metrics,
                n.geo_lng, n.geo_lat, n.geo_shape, n.source_dataset_id, n.source_row_key,
                n.row_version, n.created_at, n.updated_at
           FROM hg_embeddings e
           JOIN xb_nodes n ON n.id = e.node_id
          WHERE e.node_type = 'XB_NODE'
            AND n.subgraph_slug = 'main'
            AND e.embedding_dim = $1
            AND ($2::text IS NULL OR n.type_name = $2)
          ORDER BY e.embedding <=> $3 ASC
          LIMIT 50",
    )
    .bind(dim_i32)
    .bind(type_name)
    .bind(Vector::from(query_vec.to_vec()))
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("search_nodes VECTOR query failed: {err}")))?;

    Ok(rows.iter().map(decode_blue_node).collect())
}

fn fuse_node_search_hits(
    fts_nodes: Vec<BlueNode>,
    vec_nodes: Vec<BlueNode>,
    k: usize,
) -> NodeSearchResponse {
    const RRF_K: f64 = 60.0;
    let mut map: HashMap<Uuid, (DateTime<Utc>, NodeSearchHit)> = HashMap::new();

    for (idx, node) in fts_nodes.into_iter().enumerate() {
        let rank = f64::from(u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX));
        let delta = 1.0 / (RRF_K + rank);
        let entry = map.entry(node.id).or_insert_with(|| {
            let updated_at = node.updated_at;
            (
                updated_at,
                NodeSearchHit {
                    id: node.id,
                    type_name: node.type_name,
                    node_key: node.node_key,
                    title: node.title,
                    properties: node.properties,
                    metrics: node.metrics,
                    geo: node.geo,
                    geo_shape: node.geo_shape,
                    score: 0.0,
                    channels: Vec::new(),
                },
            )
        });
        entry.1.score += delta;
        let ch = String::from("FTS");
        if !entry.1.channels.contains(&ch) {
            entry.1.channels.push(ch);
        }
    }

    for (idx, node) in vec_nodes.into_iter().enumerate() {
        let rank = f64::from(u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX));
        let delta = 1.0 / (RRF_K + rank);
        let entry = map.entry(node.id).or_insert_with(|| {
            let updated_at = node.updated_at;
            (
                updated_at,
                NodeSearchHit {
                    id: node.id,
                    type_name: node.type_name,
                    node_key: node.node_key,
                    title: node.title,
                    properties: node.properties,
                    metrics: node.metrics,
                    geo: node.geo,
                    geo_shape: node.geo_shape,
                    score: 0.0,
                    channels: Vec::new(),
                },
            )
        });
        entry.1.score += delta;
        let ch = String::from("VECTOR");
        if !entry.1.channels.contains(&ch) {
            entry.1.channels.push(ch);
        }
    }

    let mut ranked: Vec<(DateTime<Utc>, NodeSearchHit)> = map.into_values().collect();
    ranked.sort_by(|a, b| {
        b.1.score
            .total_cmp(&a.1.score)
            .then_with(|| b.0.cmp(&a.0))
            .then_with(|| a.1.id.cmp(&b.1.id))
    });
    ranked.truncate(k);

    NodeSearchResponse {
        items: ranked.into_iter().map(|(_, hit)| hit).collect(),
    }
}
