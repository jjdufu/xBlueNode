//! Graph traversal (`neighbors`, bidirectional shortest `path`, `subgraph`) and `petgraph`-backed
//! graph algorithms (`PageRank`, `Degree`, `LabelPropagation`, `ConnectedComponents`) for Step 03 (`AxGraph`).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::time::Instant;

use axpoint_domain::AppError;
use axpoint_domain::enums::GraphAlgo;
use axpoint_domain::graph::{
    AlgoResultRow, AlgoResultsQuery, AlgoResultsResponse, AlgoRunRequest, AlgoRunResponse,
    AlgoTopEntry, LinkRecord, NeighborQuery, PathQuery, PathResponse, SubgraphQuery,
    SubgraphResponse, TraversalDirection, compute_params_hash,
};
use axpoint_domain::ports::GraphQueryPort;
use axpoint_domain::topology::BlueNode;
use chrono::{DateTime, Utc};
use petgraph::Direction;
use petgraph::algo::{dijkstra, kosaraju_scc};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::nodes::{decode_blue_node, decode_link_record};
use crate::tenant::begin_tenant_tx;

fn parse_csv_list(raw: Option<&str>) -> Option<Vec<String>> {
    let items: Vec<String> = raw?
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    (!items.is_empty()).then_some(items)
}

fn usize_to_f64(val: usize) -> f64 {
    u32::try_from(val).map_or_else(|_| val.to_string().parse::<f64>().unwrap_or(0.0), f64::from)
}

fn i64_to_f64(val: i64) -> f64 {
    i32::try_from(val).map_or_else(|_| val.to_string().parse::<f64>().unwrap_or(0.0), f64::from)
}

#[derive(Debug, Clone)]
struct AlgoNodeMeta {
    id: Uuid,
    title: String,
}

#[derive(Debug, Clone)]
struct ComputedNodeOutcome {
    node_id: Uuid,
    title: String,
    score: Option<f64>,
    community: Option<i64>,
}

#[derive(Debug, Clone)]
struct HopDijkstraState {
    cost: f64,
    hops: u32,
    node: NodeIndex,
}

impl PartialEq for HopDijkstraState {
    fn eq(&self, other: &Self) -> bool {
        self.cost.to_bits() == other.cost.to_bits()
            && self.hops == other.hops
            && self.node == other.node
    }
}

impl Eq for HopDijkstraState {}

impl Ord for HopDijkstraState {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.hops.cmp(&self.hops))
            .then_with(|| other.node.index().cmp(&self.node.index()))
    }
}

impl PartialOrd for HopDijkstraState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Deterministic `SplitMix64` PRNG for seed-reproducible label propagation without `unsafe`.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn shuffle<T>(&mut self, slice: &mut [T]) {
        let len = slice.len();
        if len <= 1 {
            return;
        }
        for i in (1..len).rev() {
            let bound = u64::try_from(i.saturating_add(1)).unwrap_or(1);
            let j_u64 = self.next_u64() % bound;
            let j = usize::try_from(j_u64).unwrap_or(0);
            slice.swap(i, j);
        }
    }
}

impl GraphQueryPort for PgStore {
    async fn query_neighbors(
        &self,
        tenant_id: &str,
        query: NeighborQuery,
    ) -> Result<SubgraphResponse, AppError> {
        if !(1..=4).contains(&query.depth) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("depth must be in 1..=4, got {}", query.depth),
            ));
        }
        if !(1..=2_000).contains(&query.limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=2000, got {}", query.limit),
            ));
        }

        let max_depth = i32::try_from(query.depth).unwrap_or(1);
        let limit_usize = usize::try_from(query.limit).unwrap_or(500);
        let fetch_limit = i64::from(query.limit).saturating_add(1);
        let link_types = parse_csv_list(query.link_types.as_deref());

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let seed_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM xb_nodes WHERE id = $1 AND subgraph_slug = 'main')",
        )
        .bind(query.node_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("check seed node failed: {err}")))?;
        if !seed_exists {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }

        let cte_sql = neighbor_walk_cte_sql(query.direction);

        let id_rows = sqlx::query(cte_sql)
            .bind(query.node_id)
            .bind(max_depth)
            .bind(&link_types)
            .bind(fetch_limit)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("query_neighbors CTE failed: {err}")))?;

        let mut ordered_ids: Vec<Uuid> = id_rows.into_iter().map(|r| r.get("node_id")).collect();
        let truncated = ordered_ids.len() > limit_usize;
        if truncated {
            ordered_ids.truncate(limit_usize);
        }

        let nodes = load_nodes_preserving_order_tx(&mut tx, &ordered_ids).await?;
        let links = load_induced_links_tx(&mut tx, &ordered_ids, link_types.as_deref()).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_neighbors tx failed: {err}"))
        })?;

        Ok(SubgraphResponse {
            nodes,
            links,
            truncated,
        })
    }

    async fn query_shortest_path(
        &self,
        tenant_id: &str,
        query: PathQuery,
    ) -> Result<PathResponse, AppError> {
        if !(1..=6).contains(&query.max_depth) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("max_depth must be in 1..=6, got {}", query.max_depth),
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let endpoint_rows = sqlx::query(
            "SELECT id, type_name, node_key, title, properties, metrics,
                    geo_lng, geo_lat, source_dataset_id, source_row_key,
                    row_version, created_at, updated_at
               FROM xb_nodes
              WHERE subgraph_slug = 'main' AND id = ANY($1)",
        )
        .bind(vec![query.from, query.to])
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("verify path endpoints failed: {err}")))?;

        let endpoint_nodes: Vec<BlueNode> = endpoint_rows.iter().map(decode_blue_node).collect();
        let from_node = endpoint_nodes
            .iter()
            .find(|n| n.id == query.from)
            .cloned()
            .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
        if !endpoint_nodes.iter().any(|n| n.id == query.to) {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }

        if query.from == query.to {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit trivial path tx failed: {err}"))
            })?;
            return Ok(PathResponse {
                path: vec![query.from],
                nodes: vec![from_node],
                links: Vec::new(),
                total_cost: Some(0.0),
            });
        }

        let resp = if query.weighted {
            compute_weighted_path_tx(&mut tx, query.from, query.to, query.max_depth).await?
        } else {
            compute_unweighted_bidir_path_tx(&mut tx, query.from, query.to, query.max_depth).await?
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_shortest_path tx failed: {err}"))
        })?;
        Ok(resp)
    }

    async fn query_subgraph(
        &self,
        tenant_id: &str,
        query: SubgraphQuery,
    ) -> Result<SubgraphResponse, AppError> {
        let subgraph_slug = query
            .subgraph
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main");
        let src_type = query
            .src_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let edge_type = query
            .edge_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let dst_type = query
            .dst_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let type_names = parse_csv_list(query.type_names.as_deref());
        let link_types = parse_csv_list(query.link_types.as_deref());

        if src_type.is_some() || edge_type.is_some() || dst_type.is_some() {
            let clamped = query.limit.clamp(1, 2_000);
            let limit_usize = usize::try_from(clamped).unwrap_or(2_000);
            return self
                .query_pattern_subgraph(
                    tenant_id,
                    PatternSubgraphFilter {
                        subgraph_slug,
                        src_type,
                        edge_type,
                        dst_type,
                        type_names: type_names.as_deref(),
                        link_types: link_types.as_deref(),
                        limit_usize,
                    },
                )
                .await;
        }

        if !(1..=2_000).contains(&query.limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=2000, got {}", query.limit),
            ));
        }

        let limit_usize = usize::try_from(query.limit).unwrap_or(2_000);
        let fetch_limit = i64::from(query.limit).saturating_add(1);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, type_name, node_key, title, properties, metrics,
                    geo_lng, geo_lat, source_dataset_id, source_row_key,
                    row_version, created_at, updated_at
               FROM xb_nodes
              WHERE subgraph_slug = $1
                AND ($2::text[] IS NULL OR type_name = ANY($2))
              ORDER BY updated_at DESC, id ASC
              LIMIT $3",
        )
        .bind(subgraph_slug)
        .bind(&type_names)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_subgraph nodes failed: {err}")))?;

        let mut nodes: Vec<BlueNode> = rows.iter().map(decode_blue_node).collect();
        let truncated = nodes.len() > limit_usize;
        if truncated {
            nodes.truncate(limit_usize);
        }

        let node_ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
        let links = load_induced_links_for_subgraph_tx(
            &mut tx,
            subgraph_slug,
            &node_ids,
            link_types.as_deref(),
        )
        .await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit query_subgraph tx failed: {err}")))?;

        Ok(SubgraphResponse {
            nodes,
            links,
            truncated,
        })
    }

    async fn run_algorithm(
        &self,
        tenant_id: &str,
        algo: GraphAlgo,
        req: AlgoRunRequest,
    ) -> Result<AlgoRunResponse, AppError> {
        let started = Instant::now();
        let subgraph_slug = req
            .subgraph
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main");
        let params_hash = compute_subgraph_algo_hash(algo, subgraph_slug, &req);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let type_filter = (!req.type_names.is_empty()).then_some(req.type_names.as_slice());
        let link_filter = (!req.link_types.is_empty()).then_some(req.link_types.as_slice());
        let (metas, edges) =
            load_algo_subgraph_tx(&mut tx, subgraph_slug, type_filter, link_filter).await?;

        if edges.len() > crate::graph_algo_duckdb::DUCKDB_PUSHDOWN_MAX_EDGES {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                "subgraph exceeds 10,000,000 edges limit for algorithm execution",
            ));
        }

        let edge_count = edges.len();
        let node_count = metas.len();
        let (backend, outcomes) = if edge_count <= crate::graph_algo_duckdb::PETGRAPH_MAX_EDGES {
            let out = solve_petgraph_in_memory(algo, &metas, &edges, &req.params)?;
            (axpoint_domain::enums::GraphAlgoBackend::PetgraphMemory, out)
        } else {
            let out = solve_duckdb_pushdown_worker(algo, &metas, edges, req.params.clone())?;
            (axpoint_domain::enums::GraphAlgoBackend::DuckdbPushdown, out)
        };

        let computed_at = Utc::now();
        persist_algo_outcomes_tx(
            &mut tx,
            tenant_id,
            algo,
            &params_hash,
            computed_at,
            &outcomes,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit run_algorithm tx failed: {err}")))?;

        let top = rank_top_entries(&outcomes, 50);
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        Ok(AlgoRunResponse {
            algo,
            backend,
            params_hash,
            node_count,
            edge_count,
            duration_ms,
            top,
        })
    }

    async fn list_algorithm_results(
        &self,
        tenant_id: &str,
        algo: GraphAlgo,
        query: AlgoResultsQuery,
    ) -> Result<AlgoResultsResponse, AppError> {
        let limit = query.limit.unwrap_or(100);
        if !(1..=1_000).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=1000, got {limit}"),
            ));
        }

        let offset: i64 = match query.cursor.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(raw) => {
                let digits = raw.trim().strip_prefix("offset:").unwrap_or(raw.trim());
                let parsed: i64 = digits.parse().map_err(|err| {
                    AppError::bad("INVALID_ARGUMENT", format!("invalid cursor: {err}"))
                })?;
                if parsed < 0 {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "cursor cannot be negative",
                    ));
                }
                parsed
            }
            None => 0,
        };

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let resolved_hash = match query
            .params_hash
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            Some(h) => Some(String::from(h.trim())),
            None => sqlx::query_scalar(
                "SELECT params_hash
                   FROM xb_graph_algo_results
                  WHERE algo = $1
                  ORDER BY computed_at DESC
                  LIMIT 1",
            )
            .bind(algo.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("resolve latest params_hash: {err}")))?,
        };

        let Some(phash) = resolved_hash else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty list_algorithm_results tx: {err}"))
            })?;
            return Ok(AlgoResultsResponse {
                items: Vec::new(),
                next_cursor: None,
            });
        };

        let limit_usize = usize::try_from(limit).unwrap_or(100);
        let fetch_limit = i64::from(limit).saturating_add(1);

        let rows = sqlx::query(
            "SELECT r.params_hash, r.node_id, n.title, r.score, r.community, r.computed_at
               FROM xb_graph_algo_results r
               JOIN xb_nodes n ON n.id = r.node_id
              WHERE r.algo = $1 AND r.params_hash = $2
              ORDER BY r.score DESC NULLS LAST, r.community ASC NULLS LAST, r.node_id ASC
              LIMIT $3 OFFSET $4",
        )
        .bind(algo.as_str())
        .bind(&phash)
        .bind(fetch_limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xb_graph_algo_results: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_algorithm_results tx: {err}"))
        })?;

        let mut items: Vec<AlgoResultRow> = rows
            .into_iter()
            .map(|r| AlgoResultRow {
                algo,
                params_hash: r.get("params_hash"),
                node_id: r.get("node_id"),
                title: r.get("title"),
                score: r.get("score"),
                community: r.get("community"),
                computed_at: r.get("computed_at"),
            })
            .collect();

        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            let next_off = offset.saturating_add(i64::from(limit));
            Some(format!("offset:{next_off}"))
        } else {
            None
        };

        Ok(AlgoResultsResponse { items, next_cursor })
    }
}

struct PatternSubgraphFilter<'a> {
    subgraph_slug: &'a str,
    src_type: Option<&'a str>,
    edge_type: Option<&'a str>,
    dst_type: Option<&'a str>,
    type_names: Option<&'a [String]>,
    link_types: Option<&'a [String]>,
    limit_usize: usize,
}

impl PgStore {
    async fn query_pattern_subgraph(
        &self,
        tenant_id: &str,
        filter: PatternSubgraphFilter<'_>,
    ) -> Result<SubgraphResponse, AppError> {
        const PATTERN_EDGE_SAFETY_CAP: usize = 200_000;
        let fetch_cap = i64::try_from(PATTERN_EDGE_SAFETY_CAP.saturating_add(1)).unwrap_or(200_001);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let link_rows = sqlx::query(
            "SELECT l.id, l.link_type, l.src_node_id, l.dst_node_id, l.weight,
                    l.properties, l.source_dataset_id, l.valid_from, l.valid_to,
                    l.created_at, l.updated_at
               FROM xb_links l
               JOIN xb_nodes s
                 ON s.tenant_id = l.tenant_id
                AND s.subgraph_slug = l.subgraph_slug
                AND s.id = l.src_node_id
               JOIN xb_nodes d
                 ON d.tenant_id = l.tenant_id
                AND d.subgraph_slug = l.subgraph_slug
                AND d.id = l.dst_node_id
              WHERE l.subgraph_slug = $1
                AND ($2::text IS NULL OR s.type_name = $2)
                AND ($3::text IS NULL OR l.link_type = $3)
                AND ($4::text IS NULL OR d.type_name = $4)
                AND ($5::text[] IS NULL OR (s.type_name = ANY($5) AND d.type_name = ANY($5)))
                AND ($6::text[] IS NULL OR l.link_type = ANY($6))
              ORDER BY l.created_at ASC, l.id ASC
              LIMIT $7",
        )
        .bind(filter.subgraph_slug)
        .bind(filter.src_type)
        .bind(filter.edge_type)
        .bind(filter.dst_type)
        .bind(filter.type_names)
        .bind(filter.link_types)
        .bind(fetch_cap)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_pattern_subgraph links failed: {err}")))?;

        let hit_edge_cap = link_rows.len() > PATTERN_EDGE_SAFETY_CAP;
        let decoded_links: Vec<LinkRecord> = link_rows
            .iter()
            .take(PATTERN_EDGE_SAFETY_CAP)
            .map(decode_link_record)
            .collect();
        let (ordered_ids, links, truncated) =
            select_pattern_links_within_node_limit(decoded_links, filter.limit_usize, hit_edge_cap);

        let nodes = load_nodes_preserving_order_for_subgraph_tx(
            &mut tx,
            filter.subgraph_slug,
            &ordered_ids,
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_pattern_subgraph tx failed: {err}"))
        })?;

        Ok(SubgraphResponse {
            nodes,
            links,
            truncated,
        })
    }
}

fn select_pattern_links_within_node_limit(
    all_links: Vec<LinkRecord>,
    limit_usize: usize,
    hit_edge_cap: bool,
) -> (Vec<Uuid>, Vec<LinkRecord>, bool) {
    let mut ordered_node_ids = Vec::new();
    let mut node_set = HashSet::new();
    let mut kept_links = Vec::new();
    let mut truncated = hit_edge_cap;

    for link in all_links {
        let need_src = usize::from(!node_set.contains(&link.src_node_id));
        let need_dst = usize::from(
            link.dst_node_id != link.src_node_id && !node_set.contains(&link.dst_node_id),
        );
        let needed = need_src.saturating_add(need_dst);
        if node_set.len().saturating_add(needed) <= limit_usize {
            if node_set.insert(link.src_node_id) {
                ordered_node_ids.push(link.src_node_id);
            }
            if node_set.insert(link.dst_node_id) {
                ordered_node_ids.push(link.dst_node_id);
            }
            kept_links.push(link);
        } else {
            truncated = true;
            if limit_usize == 1 && node_set.is_empty() {
                node_set.insert(link.src_node_id);
                ordered_node_ids.push(link.src_node_id);
            }
        }
    }

    (ordered_node_ids, kept_links, truncated)
}

async fn load_nodes_preserving_order_tx(
    tx: &mut Transaction<'_, Postgres>,
    ordered_ids: &[Uuid],
) -> Result<Vec<BlueNode>, AppError> {
    load_nodes_preserving_order_for_subgraph_tx(tx, "main", ordered_ids).await
}

async fn load_nodes_preserving_order_for_subgraph_tx(
    tx: &mut Transaction<'_, Postgres>,
    subgraph_slug: &str,
    ordered_ids: &[Uuid],
) -> Result<Vec<BlueNode>, AppError> {
    if ordered_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, source_dataset_id, source_row_key,
                row_version, created_at, updated_at
           FROM xb_nodes
          WHERE subgraph_slug = $1
            AND id = ANY($2)",
    )
    .bind(subgraph_slug)
    .bind(ordered_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ordered xb_nodes failed: {err}")))?;

    let mut by_id: HashMap<Uuid, BlueNode> = rows
        .iter()
        .map(|r| {
            let n = decode_blue_node(r);
            (n.id, n)
        })
        .collect();

    Ok(ordered_ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .collect())
}

async fn load_induced_links_tx(
    tx: &mut Transaction<'_, Postgres>,
    node_ids: &[Uuid],
    link_types: Option<&[String]>,
) -> Result<Vec<LinkRecord>, AppError> {
    load_induced_links_for_subgraph_tx(tx, "main", node_ids, link_types).await
}

async fn load_induced_links_for_subgraph_tx(
    tx: &mut Transaction<'_, Postgres>,
    subgraph_slug: &str,
    node_ids: &[Uuid],
    link_types: Option<&[String]>,
) -> Result<Vec<LinkRecord>, AppError> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, link_type, src_node_id, dst_node_id, weight,
                properties, source_dataset_id, valid_from, valid_to, created_at, updated_at
           FROM xb_links
          WHERE subgraph_slug = $1
            AND src_node_id = ANY($2)
            AND dst_node_id = ANY($2)
            AND ($3::text[] IS NULL OR link_type = ANY($3))
          ORDER BY created_at ASC, id ASC",
    )
    .bind(subgraph_slug)
    .bind(node_ids)
    .bind(link_types)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load induced xb_links failed: {err}")))?;

    Ok(rows.iter().map(decode_link_record).collect())
}

async fn ensure_subgraph_edge_budget_tx(
    tx: &mut Transaction<'_, Postgres>,
    from: Uuid,
    max_depth: i32,
) -> Result<(), AppError> {
    let edge_cnt: i64 = sqlx::query_scalar(
        "WITH RECURSIVE reach AS (
             SELECT $1::uuid AS node_id, 0 AS hop, ARRAY[$1::uuid] AS path
             UNION ALL
             SELECT l.dst_node_id, r.hop + 1, r.path || l.dst_node_id
               FROM reach r
               JOIN xb_links l ON l.src_node_id = r.node_id AND l.subgraph_slug = 'main'
              WHERE r.hop < $2 AND NOT (l.dst_node_id = ANY(r.path))
         ),
         distinct_nodes AS (SELECT DISTINCT node_id FROM reach)
         SELECT COUNT(*) FROM (
             SELECT 1 FROM xb_links l
              WHERE l.subgraph_slug = 'main'
                AND l.src_node_id IN (SELECT node_id FROM distinct_nodes)
              LIMIT 200001
         ) capped",
    )
    .bind(from)
    .bind(max_depth)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("count path subgraph edges failed: {err}")))?;

    if edge_cnt > 200_000 {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            "candidate subgraph exceeds 200,000 edges limit",
        ));
    }
    Ok(())
}

async fn compute_unweighted_bidir_path_tx(
    tx: &mut Transaction<'_, Postgres>,
    from: Uuid,
    to: Uuid,
    max_depth: u32,
) -> Result<PathResponse, AppError> {
    let max_d = i32::try_from(max_depth).unwrap_or(6);
    ensure_subgraph_edge_budget_tx(tx, from, max_d).await?;

    let fwd_max = (max_d + 1) / 2;
    let bwd_max = max_d / 2;

    let maybe_meet = sqlx::query(
        "WITH RECURSIVE fwd AS (
             SELECT $1::uuid AS node_id, 0 AS hop, ARRAY[$1::uuid] AS path
             UNION ALL
             SELECT l.dst_node_id, f.hop + 1, f.path || l.dst_node_id
               FROM fwd f
               JOIN xb_links l ON l.src_node_id = f.node_id AND l.subgraph_slug = 'main'
              WHERE f.hop < $3 AND NOT (l.dst_node_id = ANY(f.path))
         ),
         bwd AS (
             SELECT $2::uuid AS node_id, 0 AS hop, ARRAY[$2::uuid] AS path
             UNION ALL
             SELECT l.src_node_id, b.hop + 1, b.path || l.src_node_id
               FROM bwd b
               JOIN xb_links l ON l.dst_node_id = b.node_id AND l.subgraph_slug = 'main'
              WHERE b.hop < $4 AND NOT (l.src_node_id = ANY(b.path))
         )
         SELECT f.path AS fwd_path, b.path AS bwd_path, (f.hop + b.hop) AS total_hops
           FROM fwd f
           JOIN bwd b ON f.node_id = b.node_id
          WHERE (f.hop + b.hop) <= $5
          ORDER BY total_hops ASC, f.path ASC
          LIMIT 1",
    )
    .bind(from)
    .bind(to)
    .bind(fwd_max)
    .bind(bwd_max)
    .bind(max_d)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("bidirectional BFS CTE failed: {err}")))?;

    let Some(row) = maybe_meet else {
        return Ok(PathResponse {
            path: Vec::new(),
            nodes: Vec::new(),
            links: Vec::new(),
            total_cost: None,
        });
    };

    let mut full_path: Vec<Uuid> = row.get("fwd_path");
    let bwd_path: Vec<Uuid> = row.get("bwd_path");
    for &nid in bwd_path.iter().rev().skip(1) {
        full_path.push(nid);
    }

    let nodes = load_nodes_preserving_order_tx(tx, &full_path).await?;
    let links = load_consecutive_links_tx(tx, &full_path).await?;
    let hops = full_path.len().saturating_sub(1);

    Ok(PathResponse {
        path: full_path,
        nodes,
        links,
        total_cost: Some(usize_to_f64(hops)),
    })
}

async fn compute_weighted_path_tx(
    tx: &mut Transaction<'_, Postgres>,
    from: Uuid,
    to: Uuid,
    max_depth: u32,
) -> Result<PathResponse, AppError> {
    let max_d = i32::try_from(max_depth).unwrap_or(6);
    let rows = sqlx::query(
        "WITH RECURSIVE reach AS (
             SELECT $1::uuid AS node_id, 0 AS hop, ARRAY[$1::uuid] AS path
             UNION ALL
             SELECT l.dst_node_id, r.hop + 1, r.path || l.dst_node_id
               FROM reach r
               JOIN xb_links l ON l.src_node_id = r.node_id AND l.subgraph_slug = 'main'
              WHERE r.hop < $2 AND NOT (l.dst_node_id = ANY(r.path))
         ),
         distinct_nodes AS (SELECT DISTINCT node_id FROM reach)
         SELECT id, link_type, src_node_id, dst_node_id, weight,
                properties, source_dataset_id, valid_from, valid_to, created_at, updated_at
           FROM xb_links
          WHERE subgraph_slug = 'main'
            AND src_node_id IN (SELECT node_id FROM distinct_nodes)
          LIMIT 200001",
    )
    .bind(from)
    .bind(max_d)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load weighted path subgraph: {err}")))?;

    if rows.len() > 200_000 {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            "candidate subgraph exceeds 200,000 edges limit",
        ));
    }

    let mut graph = DiGraph::<Uuid, LinkRecord>::new();
    let mut node_map: HashMap<Uuid, NodeIndex> = HashMap::new();
    let start_idx = *node_map.entry(from).or_insert_with(|| graph.add_node(from));

    for r in &rows {
        let link = decode_link_record(r);
        let u = *node_map
            .entry(link.src_node_id)
            .or_insert_with(|| graph.add_node(link.src_node_id));
        let v = *node_map
            .entry(link.dst_node_id)
            .or_insert_with(|| graph.add_node(link.dst_node_id));
        graph.add_edge(u, v, link);
    }

    let Some(&goal_idx) = node_map.get(&to) else {
        return Ok(PathResponse {
            path: Vec::new(),
            nodes: Vec::new(),
            links: Vec::new(),
            total_cost: None,
        });
    };

    let unconstrained = dijkstra(&graph, start_idx, Some(goal_idx), |e| {
        1.0 / e.weight().weight.max(1e-9)
    });
    if !unconstrained.contains_key(&goal_idx) {
        return Ok(PathResponse {
            path: Vec::new(),
            nodes: Vec::new(),
            links: Vec::new(),
            total_cost: None,
        });
    }

    let Some((rev_nodes, rev_links, total_cost)) =
        run_hop_bounded_dijkstra(&graph, start_idx, goal_idx, to, max_depth)
    else {
        return Ok(PathResponse {
            path: Vec::new(),
            nodes: Vec::new(),
            links: Vec::new(),
            total_cost: None,
        });
    };

    let nodes = load_nodes_preserving_order_tx(tx, &rev_nodes).await?;

    Ok(PathResponse {
        path: rev_nodes,
        nodes,
        links: rev_links,
        total_cost: Some(total_cost),
    })
}

fn neighbor_walk_cte_sql(direction: TraversalDirection) -> &'static str {
    match direction {
        TraversalDirection::Out => {
            "WITH RECURSIVE walk AS (
                 SELECT $1::uuid AS node_id, 0 AS hop, ARRAY[$1::uuid] AS path
                 UNION ALL
                 SELECT l.dst_node_id AS node_id, g.hop + 1 AS hop, g.path || l.dst_node_id AS path
                   FROM walk g
                   JOIN xb_links l ON l.src_node_id = g.node_id AND l.subgraph_slug = 'main'
                  WHERE g.hop < $2
                    AND ($3::text[] IS NULL OR l.link_type = ANY($3))
                    AND NOT (l.dst_node_id = ANY(g.path))
             )
             SELECT node_id, MIN(hop) AS min_hop
               FROM walk
              GROUP BY node_id
              ORDER BY min_hop ASC, node_id ASC
              LIMIT $4"
        }
        TraversalDirection::In => {
            "WITH RECURSIVE walk AS (
                 SELECT $1::uuid AS node_id, 0 AS hop, ARRAY[$1::uuid] AS path
                 UNION ALL
                 SELECT l.src_node_id AS node_id, g.hop + 1 AS hop, g.path || l.src_node_id AS path
                   FROM walk g
                   JOIN xb_links l ON l.dst_node_id = g.node_id AND l.subgraph_slug = 'main'
                  WHERE g.hop < $2
                    AND ($3::text[] IS NULL OR l.link_type = ANY($3))
                    AND NOT (l.src_node_id = ANY(g.path))
             )
             SELECT node_id, MIN(hop) AS min_hop
               FROM walk
              GROUP BY node_id
              ORDER BY min_hop ASC, node_id ASC
              LIMIT $4"
        }
        TraversalDirection::Both => {
            "WITH RECURSIVE walk AS (
                 SELECT $1::uuid AS node_id, 0 AS hop, ARRAY[$1::uuid] AS path
                 UNION ALL
                 SELECT CASE WHEN l.src_node_id = g.node_id THEN l.dst_node_id ELSE l.src_node_id END AS node_id,
                        g.hop + 1 AS hop,
                        g.path || (CASE WHEN l.src_node_id = g.node_id THEN l.dst_node_id ELSE l.src_node_id END) AS path
                   FROM walk g
                   JOIN xb_links l ON (l.src_node_id = g.node_id OR l.dst_node_id = g.node_id) AND l.subgraph_slug = 'main'
                  WHERE g.hop < $2
                    AND ($3::text[] IS NULL OR l.link_type = ANY($3))
                    AND NOT ((CASE WHEN l.src_node_id = g.node_id THEN l.dst_node_id ELSE l.src_node_id END) = ANY(g.path))
             )
             SELECT node_id, MIN(hop) AS min_hop
               FROM walk
              GROUP BY node_id
              ORDER BY min_hop ASC, node_id ASC
              LIMIT $4"
        }
    }
}

fn run_hop_bounded_dijkstra(
    graph: &DiGraph<Uuid, LinkRecord>,
    start_idx: NodeIndex,
    goal_idx: NodeIndex,
    goal_uuid: Uuid,
    max_depth: u32,
) -> Option<(Vec<Uuid>, Vec<LinkRecord>, f64)> {
    let mut best_dist: HashMap<(NodeIndex, u32), f64> = HashMap::new();
    let mut parent: HashMap<(NodeIndex, u32), (NodeIndex, u32, LinkRecord)> = HashMap::new();
    let mut heap = BinaryHeap::new();

    best_dist.insert((start_idx, 0), 0.0);
    heap.push(HopDijkstraState {
        cost: 0.0,
        hops: 0,
        node: start_idx,
    });

    let mut goal_state: Option<(u32, f64)> = None;
    while let Some(HopDijkstraState { cost, hops, node }) = heap.pop() {
        if let Some(&known) = best_dist.get(&(node, hops))
            && cost > known + 1e-12
        {
            continue;
        }
        if node == goal_idx {
            goal_state = Some((hops, cost));
            break;
        }
        if hops >= max_depth {
            continue;
        }
        let next_hops = hops.saturating_add(1);
        for edge in graph.edges_directed(node, Direction::Outgoing) {
            let step_cost = 1.0 / edge.weight().weight.max(1e-9);
            let next_cost = cost + step_cost;
            let next_node = edge.target();
            let improve = best_dist
                .get(&(next_node, next_hops))
                .is_none_or(|&prev| next_cost + 1e-12 < prev);
            if improve {
                best_dist.insert((next_node, next_hops), next_cost);
                parent.insert((next_node, next_hops), (node, hops, edge.weight().clone()));
                heap.push(HopDijkstraState {
                    cost: next_cost,
                    hops: next_hops,
                    node: next_node,
                });
            }
        }
    }

    let (final_hops, total_cost) = goal_state?;
    let mut rev_nodes = vec![goal_uuid];
    let mut rev_links = Vec::new();
    let mut cur_node = goal_idx;
    let mut cur_hops = final_hops;

    while cur_hops > 0 {
        let (prev_node, prev_hops, link) = parent.remove(&(cur_node, cur_hops))?;
        if let Some(&uid) = graph.node_weight(prev_node) {
            rev_nodes.push(uid);
        }
        rev_links.push(link);
        cur_node = prev_node;
        cur_hops = prev_hops;
    }

    rev_nodes.reverse();
    rev_links.reverse();
    Some((rev_nodes, rev_links, total_cost))
}

async fn load_consecutive_links_tx(
    tx: &mut Transaction<'_, Postgres>,
    path: &[Uuid],
) -> Result<Vec<LinkRecord>, AppError> {
    if path.len() < 2 {
        return Ok(Vec::new());
    }
    let mut links = Vec::with_capacity(path.len().saturating_sub(1));
    for pair in path.windows(2) {
        if let [u, v] = *pair {
            let maybe_row = sqlx::query(
                "SELECT id, link_type, src_node_id, dst_node_id, weight,
                        properties, source_dataset_id, valid_from, valid_to, created_at, updated_at
                   FROM xb_links
                  WHERE subgraph_slug = 'main' AND src_node_id = $1 AND dst_node_id = $2
                  ORDER BY weight DESC, created_at ASC
                  LIMIT 1",
            )
            .bind(u)
            .bind(v)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("load path link failed: {err}")))?;
            if let Some(r) = maybe_row {
                links.push(decode_link_record(&r));
            }
        }
    }
    Ok(links)
}

fn compute_subgraph_algo_hash(
    algo: GraphAlgo,
    subgraph_slug: &str,
    req: &AlgoRunRequest,
) -> String {
    if subgraph_slug == "main" {
        return compute_params_hash(algo, &req.type_names, &req.link_types, &req.params);
    }
    let mut scoped_params = match req.params.as_object() {
        Some(obj) => obj.clone(),
        None => serde_json::Map::new(),
    };
    scoped_params.insert(
        String::from("__subgraph"),
        serde_json::Value::String(String::from(subgraph_slug)),
    );
    compute_params_hash(
        algo,
        &req.type_names,
        &req.link_types,
        &serde_json::Value::Object(scoped_params),
    )
}

async fn load_algo_subgraph_tx(
    tx: &mut Transaction<'_, Postgres>,
    subgraph_slug: &str,
    type_filter: Option<&[String]>,
    link_filter: Option<&[String]>,
) -> Result<(Vec<AlgoNodeMeta>, Vec<(Uuid, Uuid, f64)>), AppError> {
    let node_rows = sqlx::query(
        "SELECT id, title
           FROM xb_nodes
          WHERE subgraph_slug = $1
            AND ($2::text[] IS NULL OR type_name = ANY($2))
          ORDER BY id ASC",
    )
    .bind(subgraph_slug)
    .bind(type_filter)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load algorithm nodes failed: {err}")))?;

    let metas: Vec<AlgoNodeMeta> = node_rows
        .into_iter()
        .map(|r| AlgoNodeMeta {
            id: r.get("id"),
            title: r.get("title"),
        })
        .collect();
    if metas.is_empty() {
        return Ok((metas, Vec::new()));
    }

    let fetch_cap =
        i64::try_from(crate::graph_algo_duckdb::DUCKDB_PUSHDOWN_MAX_EDGES.saturating_add(1))
            .unwrap_or(10_000_001);

    let edge_rows = if type_filter.is_some() {
        let node_ids: Vec<Uuid> = metas.iter().map(|m| m.id).collect();
        sqlx::query(
            "SELECT src_node_id, dst_node_id, weight
               FROM xb_links
              WHERE subgraph_slug = $1
                AND src_node_id = ANY($2)
                AND dst_node_id = ANY($2)
                AND ($3::text[] IS NULL OR link_type = ANY($3))
              LIMIT $4",
        )
        .bind(subgraph_slug)
        .bind(&node_ids)
        .bind(link_filter)
        .bind(fetch_cap)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load filtered algorithm edges failed: {err}")))?
    } else {
        sqlx::query(
            "SELECT src_node_id, dst_node_id, weight
               FROM xb_links
              WHERE subgraph_slug = $1
                AND ($2::text[] IS NULL OR link_type = ANY($2))
              LIMIT $3",
        )
        .bind(subgraph_slug)
        .bind(link_filter)
        .bind(fetch_cap)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load algorithm edges failed: {err}")))?
    };

    let mut edges: Vec<(Uuid, Uuid, f64)> = edge_rows
        .into_iter()
        .map(|r| (r.get("src_node_id"), r.get("dst_node_id"), r.get("weight")))
        .collect();
    if edges.len() <= crate::graph_algo_duckdb::PETGRAPH_MAX_EDGES {
        edges.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    }
    Ok((metas, edges))
}

fn solve_petgraph_in_memory(
    algo: GraphAlgo,
    metas: &[AlgoNodeMeta],
    edges: &[(Uuid, Uuid, f64)],
    params: &serde_json::Value,
) -> Result<Vec<ComputedNodeOutcome>, AppError> {
    let mut graph = DiGraph::<Uuid, f64>::with_capacity(metas.len(), edges.len());
    let mut idx_by_uuid: HashMap<Uuid, NodeIndex> = HashMap::with_capacity(metas.len());
    for m in metas {
        let idx = graph.add_node(m.id);
        idx_by_uuid.insert(m.id, idx);
    }
    for &(src, dst, w) in edges {
        if let (Some(&u), Some(&v)) = (idx_by_uuid.get(&src), idx_by_uuid.get(&dst)) {
            graph.add_edge(u, v, w.max(1e-9));
        }
    }
    execute_graph_algorithm(algo, metas, &graph, params)
}

fn solve_duckdb_pushdown_worker(
    algo: GraphAlgo,
    metas: &[AlgoNodeMeta],
    edges: Vec<(Uuid, Uuid, f64)>,
    params: serde_json::Value,
) -> Result<Vec<ComputedNodeOutcome>, AppError> {
    let nodes_vec: Vec<(Uuid, String)> = metas.iter().map(|m| (m.id, m.title.clone())).collect();
    let pushed = std::thread::spawn(move || {
        crate::graph_algo_duckdb::execute_duckdb_pushdown_all(algo, &nodes_vec, &edges, &params)
    })
    .join()
    .map_err(|_| AppError::Database(String::from("duckdb pushdown worker thread panicked")))??;

    Ok(pushed
        .into_iter()
        .map(|p| ComputedNodeOutcome {
            node_id: p.node_id,
            title: p.title,
            score: p.score,
            community: p.community,
        })
        .collect())
}

fn execute_graph_algorithm(
    algo: GraphAlgo,
    metas: &[AlgoNodeMeta],
    graph: &DiGraph<Uuid, f64>,
    params: &serde_json::Value,
) -> Result<Vec<ComputedNodeOutcome>, AppError> {
    if metas.is_empty() {
        return Ok(Vec::new());
    }

    match algo {
        GraphAlgo::Pagerank => run_pagerank(metas, graph, params),
        GraphAlgo::Degree => run_degree_centrality(metas, graph, params),
        GraphAlgo::LabelPropagation => run_label_propagation(metas, graph, params),
        GraphAlgo::StronglyConnected => Ok(run_strongly_connected(metas, graph)),
        GraphAlgo::WeaklyConnected => Ok(run_weakly_connected(metas, graph)),
        GraphAlgo::ConnectedComponents => {
            let weak = params
                .get("weak")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if weak {
                Ok(run_weakly_connected(metas, graph))
            } else {
                Ok(run_strongly_connected(metas, graph))
            }
        }
    }
}

fn run_pagerank(
    metas: &[AlgoNodeMeta],
    graph: &DiGraph<Uuid, f64>,
    params: &serde_json::Value,
) -> Result<Vec<ComputedNodeOutcome>, AppError> {
    let damping = params
        .get("damping")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.85);
    if !(0.0 < damping && damping < 1.0 && damping.is_finite()) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "damping must be finite in (0.0, 1.0)",
        ));
    }

    let max_iter = params
        .get("max_iter")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(100);
    if !(1..=10_000).contains(&max_iter) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "max_iter must be in 1..=10000",
        ));
    }

    let tol = params
        .get("tol")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1e-6);
    if !(tol > 0.0 && tol.is_finite()) {
        return Err(AppError::bad("INVALID_ARGUMENT", "tol must be positive"));
    }

    let n = metas.len();
    let n_f64 = usize_to_f64(n);
    let init_rank = 1.0 / n_f64;
    let mut ranks = vec![init_rank; n];

    let mut out_weight_sum = vec![0.0_f64; n];
    for idx in graph.node_indices() {
        let sum_w: f64 = graph
            .edges_directed(idx, Direction::Outgoing)
            .map(|e| *e.weight())
            .sum();
        if let Some(slot) = out_weight_sum.get_mut(idx.index()) {
            *slot = sum_w;
        }
    }

    let teleport = (1.0 - damping) / n_f64;
    for _ in 0..max_iter {
        let mut dangling_mass = 0.0_f64;
        for (i, &ow) in out_weight_sum.iter().enumerate() {
            if ow <= 0.0
                && let Some(&r) = ranks.get(i)
            {
                dangling_mass += r;
            }
        }

        let base = teleport + damping * (dangling_mass / n_f64);
        let mut next_ranks = vec![base; n];

        for u in graph.node_indices() {
            let ow = out_weight_sum.get(u.index()).copied().unwrap_or(0.0);
            if ow <= 0.0 {
                continue;
            }
            let ru = ranks.get(u.index()).copied().unwrap_or(0.0);
            for edge in graph.edges_directed(u, Direction::Outgoing) {
                let v_idx = edge.target().index();
                if let Some(dst_slot) = next_ranks.get_mut(v_idx) {
                    *dst_slot += damping * ru * (*edge.weight() / ow);
                }
            }
        }

        let diff: f64 = ranks
            .iter()
            .zip(next_ranks.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        ranks = next_ranks;
        if diff < tol {
            break;
        }
    }

    Ok(metas
        .iter()
        .enumerate()
        .map(|(i, m)| ComputedNodeOutcome {
            node_id: m.id,
            title: m.title.clone(),
            score: Some(ranks.get(i).copied().unwrap_or(0.0)),
            community: None,
        })
        .collect())
}

fn run_degree_centrality(
    metas: &[AlgoNodeMeta],
    graph: &DiGraph<Uuid, f64>,
    params: &serde_json::Value,
) -> Result<Vec<ComputedNodeOutcome>, AppError> {
    let dir_str = params
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("BOTH")
        .to_ascii_uppercase();

    if !matches!(dir_str.as_str(), "IN" | "OUT" | "BOTH") {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid degree direction '{dir_str}' (expected IN, OUT, or BOTH)"),
        ));
    }

    let mut raw_degs = Vec::with_capacity(metas.len());
    let mut max_deg: usize = 0;

    for idx in graph.node_indices() {
        let deg = match dir_str.as_str() {
            "IN" => graph.edges_directed(idx, Direction::Incoming).count(),
            "OUT" => graph.edges_directed(idx, Direction::Outgoing).count(),
            _ => graph
                .edges_directed(idx, Direction::Incoming)
                .count()
                .saturating_add(graph.edges_directed(idx, Direction::Outgoing).count()),
        };
        if deg > max_deg {
            max_deg = deg;
        }
        raw_degs.push(deg);
    }

    let denom = usize_to_f64(max_deg);
    Ok(metas
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let d = raw_degs.get(i).copied().unwrap_or(0);
            let score = if max_deg == 0 {
                0.0
            } else {
                (usize_to_f64(d) / denom).clamp(0.0, 1.0)
            };
            ComputedNodeOutcome {
                node_id: m.id,
                title: m.title.clone(),
                score: Some(score),
                community: None,
            }
        })
        .collect())
}

fn run_label_propagation(
    metas: &[AlgoNodeMeta],
    graph: &DiGraph<Uuid, f64>,
    params: &serde_json::Value,
) -> Result<Vec<ComputedNodeOutcome>, AppError> {
    let max_iter = params
        .get("max_iter")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(50);
    if !(1..=1_000).contains(&max_iter) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "max_iter must be in 1..=1000",
        ));
    }

    let seed = params
        .get("seed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(42);

    let n = metas.len();
    let mut labels: Vec<i64> = (0..n)
        .map(|i| i64::try_from(i).unwrap_or(i64::MAX))
        .collect();

    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for edge in graph.edge_references() {
        let u = edge.source().index();
        let v = edge.target().index();
        let w = *edge.weight();
        if u != v {
            if let Some(u_list) = adj.get_mut(u) {
                u_list.push((v, w));
            }
            if let Some(v_list) = adj.get_mut(v) {
                v_list.push((u, w));
            }
        }
    }

    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = SplitMix64::new(seed);

    for _ in 0..max_iter {
        rng.shuffle(&mut order);
        let mut changed = false;

        for &u in &order {
            let Some(neighbors) = adj.get(u) else {
                continue;
            };
            if neighbors.is_empty() {
                continue;
            }

            let mut label_weights: BTreeMap<i64, f64> = BTreeMap::new();
            for &(v, w) in neighbors {
                if let Some(&lbl) = labels.get(v) {
                    *label_weights.entry(lbl).or_insert(0.0) += w;
                }
            }

            let mut best: Option<(i64, f64)> = None;
            for (lbl, total_w) in label_weights {
                let take = match best {
                    None => true,
                    Some((_, best_w)) => total_w > best_w + 1e-12,
                };
                if take {
                    best = Some((lbl, total_w));
                }
            }

            if let (Some((best_lbl, _)), Some(cur_slot)) = (best, labels.get_mut(u))
                && *cur_slot != best_lbl
            {
                *cur_slot = best_lbl;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    let canonical = canonicalize_communities(&labels);
    Ok(metas
        .iter()
        .enumerate()
        .map(|(i, m)| ComputedNodeOutcome {
            node_id: m.id,
            title: m.title.clone(),
            score: None,
            community: Some(canonical.get(i).copied().unwrap_or(0)),
        })
        .collect())
}

fn run_strongly_connected(
    metas: &[AlgoNodeMeta],
    graph: &DiGraph<Uuid, f64>,
) -> Vec<ComputedNodeOutcome> {
    let sccs = kosaraju_scc(graph);
    let mut raw_labels = vec![0_i64; metas.len()];
    for (comp_idx, comp_nodes) in sccs.iter().enumerate() {
        let lbl = i64::try_from(comp_idx).unwrap_or(i64::MAX);
        for nidx in comp_nodes {
            if let Some(slot) = raw_labels.get_mut(nidx.index()) {
                *slot = lbl;
            }
        }
    }

    let canonical = canonicalize_communities(&raw_labels);
    metas
        .iter()
        .enumerate()
        .map(|(i, m)| ComputedNodeOutcome {
            node_id: m.id,
            title: m.title.clone(),
            score: None,
            community: Some(canonical.get(i).copied().unwrap_or(0)),
        })
        .collect()
}

fn run_weakly_connected(
    metas: &[AlgoNodeMeta],
    graph: &DiGraph<Uuid, f64>,
) -> Vec<ComputedNodeOutcome> {
    let n = metas.len();
    let mut parent: Vec<usize> = (0..n).collect();

    for edge in graph.edge_references() {
        let ru = uf_find(&mut parent, edge.source().index());
        let rv = uf_find(&mut parent, edge.target().index());
        if ru != rv {
            let (lo, hi) = if ru < rv { (ru, rv) } else { (rv, ru) };
            if let Some(slot) = parent.get_mut(hi) {
                *slot = lo;
            }
        }
    }

    let mut raw_labels = Vec::with_capacity(n);
    for i in 0..n {
        let root = uf_find(&mut parent, i);
        raw_labels.push(i64::try_from(root).unwrap_or(i64::MAX));
    }

    let canonical = canonicalize_communities(&raw_labels);
    metas
        .iter()
        .enumerate()
        .map(|(i, m)| ComputedNodeOutcome {
            node_id: m.id,
            title: m.title.clone(),
            score: None,
            community: Some(canonical.get(i).copied().unwrap_or(0)),
        })
        .collect()
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while let Some(&p) = parent.get(x) {
        if p == x {
            break;
        }
        if let Some(&gp) = parent.get(p)
            && let Some(slot) = parent.get_mut(x)
        {
            *slot = gp;
        }
        x = p;
    }
    x
}

fn canonicalize_communities(raw: &[i64]) -> Vec<i64> {
    let mut stats: HashMap<i64, (usize, usize)> = HashMap::new();
    for (idx, &lbl) in raw.iter().enumerate() {
        stats
            .entry(lbl)
            .and_modify(|(cnt, _)| *cnt = cnt.saturating_add(1))
            .or_insert((1, idx));
    }

    let mut ordered: Vec<(i64, usize, usize)> = stats
        .into_iter()
        .map(|(lbl, (cnt, first_idx))| (lbl, cnt, first_idx))
        .collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));

    let remap: HashMap<i64, i64> = ordered
        .into_iter()
        .enumerate()
        .map(|(new_id, (old_lbl, _, _))| (old_lbl, i64::try_from(new_id).unwrap_or(i64::MAX)))
        .collect();

    raw.iter()
        .map(|lbl| remap.get(lbl).copied().unwrap_or(0))
        .collect()
}

async fn persist_algo_outcomes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    algo: GraphAlgo,
    params_hash: &str,
    computed_at: DateTime<Utc>,
    outcomes: &[ComputedNodeOutcome],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM xb_graph_algo_results WHERE algo = $1 AND params_hash = $2")
        .bind(algo.as_str())
        .bind(params_hash)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clear xb_graph_algo_results: {err}")))?;

    if outcomes.is_empty() {
        return Ok(());
    }

    let cap = outcomes.len();
    let mut nids = Vec::with_capacity(cap);
    let mut scores: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut comms: Vec<Option<i64>> = Vec::with_capacity(cap);
    let mut metric_vals: Vec<f64> = Vec::with_capacity(cap);

    for o in outcomes {
        nids.push(o.node_id);
        scores.push(o.score);
        comms.push(o.community);
        metric_vals.push(
            o.score
                .or_else(|| o.community.map(i64_to_f64))
                .unwrap_or(0.0),
        );
    }

    sqlx::query(
        "INSERT INTO xb_graph_algo_results (
             tenant_id, algo, params_hash, node_id, score, community, computed_at
         )
         SELECT $1, $2, $3, u.node_id, u.score, u.community, $4
           FROM UNNEST($5::uuid[], $6::float8[], $7::bigint[])
                AS u(node_id, score, community)",
    )
    .bind(tenant_id)
    .bind(algo.as_str())
    .bind(params_hash)
    .bind(computed_at)
    .bind(&nids)
    .bind(&scores)
    .bind(&comms)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert xb_graph_algo_results: {err}")))?;

    let metric_name = format!("algo_{}", algo.as_str().to_ascii_lowercase());
    sqlx::query(
        "INSERT INTO xb_metric_series (
             tenant_id, node_id, metric, ts, value, source_tag
         )
         SELECT $1, u.node_id, $2, $3, u.val, 'ALGORITHM'
           FROM UNNEST($4::uuid[], $5::float8[]) AS u(node_id, val)
         ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
             value = EXCLUDED.value",
    )
    .bind(tenant_id)
    .bind(&metric_name)
    .bind(computed_at)
    .bind(&nids)
    .bind(&metric_vals)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("write algorithm xb_metric_series: {err}")))?;

    Ok(())
}

fn rank_top_entries(outcomes: &[ComputedNodeOutcome], limit: usize) -> Vec<AlgoTopEntry> {
    let mut entries: Vec<AlgoTopEntry> = outcomes
        .iter()
        .map(|o| AlgoTopEntry {
            node_id: o.node_id,
            title: o.title.clone(),
            score: o.score,
            community: o.community,
        })
        .collect();

    entries.sort_by(|a, b| match (a.score, b.score) {
        (Some(sa), Some(sb)) => sb.total_cmp(&sa).then_with(|| a.node_id.cmp(&b.node_id)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a
            .community
            .cmp(&b.community)
            .then_with(|| a.node_id.cmp(&b.node_id)),
    });
    entries.truncate(limit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagerank_5_cycle_plus_dangling_matches_analytical_networkx_within_1e6()
    -> Result<(), AppError> {
        // 5-node cycle (0 -> 1 -> 2 -> 3 -> 4 -> 0) + 1 dangling sink (0 -> 5, where 5 has out-degree 0).
        let metas: Vec<AlgoNodeMeta> = (0..6)
            .map(|i| AlgoNodeMeta {
                id: Uuid::from_u128(u128::try_from(i + 1).unwrap_or(1)),
                title: format!("N-{i}"),
            })
            .collect();

        let mut g = DiGraph::<Uuid, f64>::new();
        let idx: Vec<NodeIndex> = metas.iter().map(|m| g.add_node(m.id)).collect();
        for (u, v) in [(0_usize, 1_usize), (1, 2), (2, 3), (3, 4), (4, 0), (0, 5)] {
            if let (Some(&nu), Some(&nv)) = (idx.get(u), idx.get(v)) {
                g.add_edge(nu, nv, 1.0);
            }
        }

        let res = run_pagerank(
            &metas,
            &g,
            &serde_json::json!({"damping": 0.85, "max_iter": 500, "tol": 1e-11}),
        )?;

        // Analytical stationary solution for d = 0.85, N = 6:
        // Let b = (1 - d)/6 + (d/6) * r5.
        // r1 = r5 = b + (d/2)*r0, r2 = b + d*r1, r3 = b + d*r2, r4 = b + d*r3, r0 = b + d*r4.
        let d = 0.85_f64;
        // Express r0..r5 in terms of b:
        // r0 = b*(1 + d + d^2 + d^3 + d^4) / (1 - 0.5 * d^5)
        let r0_per_b = (1.0 + d + d.powi(2) + d.powi(3) + d.powi(4)) / (1.0 - 0.5 * d.powi(5));
        let r1_per_b = 1.0 + 0.5 * d * r0_per_b;
        let r2_per_b = 1.0 + d * r1_per_b;
        let r3_per_b = 1.0 + d * r2_per_b;
        let r4_per_b = 1.0 + d * r3_per_b;
        let r5_per_b = r1_per_b;
        let sum_per_b = r0_per_b + r1_per_b + r2_per_b + r3_per_b + r4_per_b + r5_per_b;
        let b = 1.0 / sum_per_b;
        let expected = [
            b * r0_per_b,
            b * r1_per_b,
            b * r2_per_b,
            b * r3_per_b,
            b * r4_per_b,
            b * r5_per_b,
        ];

        let total_mass: f64 = res.iter().filter_map(|o| o.score).sum();
        assert!((total_mass - 1.0).abs() < 1e-6);

        for (o, &exp) in res.iter().zip(expected.iter()) {
            let got = o.score.unwrap_or(0.0);
            assert!(
                (got - exp).abs() < 1e-6,
                "PageRank mismatch for {}: got {got}, expected {exp}",
                o.title
            );
        }
        Ok(())
    }

    #[test]
    fn degree_label_propagation_and_connected_components_are_deterministic() -> Result<(), AppError>
    {
        // Two directed 3-cycles (0->1->2->0 and 3->4->5->3) linked one-way 2->3 with tiny weight 0.01.
        let metas: Vec<AlgoNodeMeta> = (0..6)
            .map(|i| AlgoNodeMeta {
                id: Uuid::from_u128(u128::try_from(i + 10).unwrap_or(10)),
                title: format!("C-{i}"),
            })
            .collect();

        let mut g = DiGraph::<Uuid, f64>::new();
        let idx: Vec<NodeIndex> = metas.iter().map(|m| g.add_node(m.id)).collect();
        for (u, v, w) in [
            (0_usize, 1_usize, 1.0),
            (1, 2, 1.0),
            (2, 0, 1.0),
            (3, 4, 1.0),
            (4, 5, 1.0),
            (5, 3, 1.0),
            (2, 3, 0.01),
        ] {
            if let (Some(&nu), Some(&nv)) = (idx.get(u), idx.get(v)) {
                g.add_edge(nu, nv, w);
            }
        }

        let deg = run_degree_centrality(&metas, &g, &serde_json::json!({"direction": "OUT"}))?;
        // Node 2 has out-degree 2 (max), others have out-degree 1 -> 0.5.
        assert_eq!(deg.get(2).and_then(|o| o.score), Some(1.0));
        assert_eq!(deg.first().and_then(|o| o.score), Some(0.5));

        let scc = run_strongly_connected(&metas, &g);
        let wcc = run_weakly_connected(&metas, &g);
        // Strongly connected: 2 SCCs ({0,1,2} and {3,4,5}); Weakly connected: 1 WCC ({0..5}).
        assert_ne!(
            scc.first().and_then(|o| o.community),
            scc.get(3).and_then(|o| o.community)
        );
        for item in &wcc {
            assert_eq!(item.community, Some(0));
        }

        let lpa_a = run_label_propagation(&metas, &g, &serde_json::json!({"seed": 42}))?;
        let lpa_b = run_label_propagation(&metas, &g, &serde_json::json!({"seed": 42}))?;
        let comm_a: Vec<Option<i64>> = lpa_a.iter().map(|o| o.community).collect();
        let comm_b: Vec<Option<i64>> = lpa_b.iter().map(|o| o.community).collect();
        assert_eq!(comm_a, comm_b);
        Ok(())
    }

    #[test]
    fn select_pattern_links_deduplicates_endpoints_and_marks_truncated() {
        let now = Utc::now();
        let mk_link = |lid: u128, u: u128, v: u128| LinkRecord {
            id: Uuid::from_u128(lid),
            link_type: String::from("SUPPLIES"),
            src_node_id: Uuid::from_u128(u),
            dst_node_id: Uuid::from_u128(v),
            weight: 1.0,
            properties: serde_json::Map::new(),
            metrics: BTreeMap::new(),
            row_version: 1,
            source_dataset_id: None,
            valid_from: None,
            valid_to: None,
            created_at: now,
            updated_at: now,
        };

        let links = vec![
            mk_link(1, 10, 20),
            mk_link(2, 20, 30),
            mk_link(3, 40, 50),
            mk_link(4, 10, 30),
        ];

        let (node_ids, kept, truncated) = select_pattern_links_within_node_limit(links, 3, false);
        assert!(truncated);
        assert_eq!(
            node_ids,
            vec![
                Uuid::from_u128(10),
                Uuid::from_u128(20),
                Uuid::from_u128(30)
            ]
        );
        // Link 4 (10 -> 30) connects already-selected endpoints so it is retained, while Link 3 (40 -> 50) is dropped.
        assert_eq!(
            kept.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(4)]
        );
    }
}
