//! Vectorized `DuckDB` relational-algebra pushdown engine for large-subgraph (`(500_000, 10_000_000]` edges)
//! graph algorithms (`Degree`, `ConnectedComponents`, `LabelPropagation`, `07-STEP29` §29.3).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use axpoint_domain::AppError;
use axpoint_domain::enums::GraphAlgo;
use axpoint_domain::graph::AlgoTopEntry;
use uuid::Uuid;

/// Maximum edge count processed in-memory by `petgraph` before switching to [`execute_duckdb_pushdown`].
pub const PETGRAPH_MAX_EDGES: usize = 500_000;

/// Hard upper bound on edge count accepted by `DuckDB` relational-algebra pushdown (`07-STEP29` §29.3).
pub const DUCKDB_PUSHDOWN_MAX_EDGES: usize = 10_000_000;

/// Hard iteration cap for relational-algebra graph convergence loops (`07-STEP29` §29.3).
const MAX_PUSHDOWN_ITERATIONS: usize = 20;

/// Per-node algorithm result produced by [`execute_duckdb_pushdown_all`].
#[derive(Debug, Clone)]
pub(crate) struct PushdownNodeOutcome {
    /// Node UUID.
    pub(crate) node_id: Uuid,
    /// Node title.
    pub(crate) title: String,
    /// Centrality score (`Some` for `Degree`, `None` for community algorithms).
    pub(crate) score: Option<f64>,
    /// Community / component label (`Some` for `ConnectedComponents` / `LabelPropagation`).
    pub(crate) community: Option<i64>,
}

struct TempSpillDir(PathBuf);

impl TempSpillDir {
    fn create() -> Result<Self, AppError> {
        let dir =
            std::env::temp_dir().join(format!("axpoint_algo_spill_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).map_err(|err| {
            AppError::Database(format!("create duckdb algo spill directory failed: {err}"))
        })?;
        Ok(Self(dir))
    }
}

impl Drop for TempSpillDir {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            tracing::debug!(error = %err, "cleanup duckdb algo spill directory");
        }
    }
}

/// Returns `true` if `algo` is supported by the `DuckDB` relational-algebra pushdown engine.
#[must_use]
pub const fn supports_duckdb_pushdown(algo: GraphAlgo) -> bool {
    matches!(
        algo,
        GraphAlgo::Degree
            | GraphAlgo::ConnectedComponents
            | GraphAlgo::WeaklyConnected
            | GraphAlgo::LabelPropagation
    )
}

/// Executes `algo` via bounded `DuckDB` relational-algebra iterations and returns the Top-50 [`AlgoTopEntry`] items.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE` / `INVALID_ARGUMENT`) if `algo` or parameters
/// are unsupported for pushdown, or [`AppError::Database`] if `DuckDB` execution fails.
pub fn execute_duckdb_pushdown(
    algo: GraphAlgo,
    nodes: &[(Uuid, String)],
    edges: &[(Uuid, Uuid, f64)],
    params: &serde_json::Value,
) -> Result<Vec<AlgoTopEntry>, AppError> {
    let outcomes = execute_duckdb_pushdown_all(algo, nodes, edges, params)?;
    Ok(rank_pushdown_top(&outcomes, 50))
}

/// Executes `algo` via `DuckDB` relational algebra and returns full node outcomes for persistence.
pub(crate) fn execute_duckdb_pushdown_all(
    algo: GraphAlgo,
    nodes: &[(Uuid, String)],
    edges: &[(Uuid, Uuid, f64)],
    params: &serde_json::Value,
) -> Result<Vec<PushdownNodeOutcome>, AppError> {
    if !supports_duckdb_pushdown(algo) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!(
                "algorithm {} exceeds {PETGRAPH_MAX_EDGES} edges in-memory limit and is not supported by DUCKDB_PUSHDOWN",
                algo.as_str()
            ),
        ));
    }
    if edges.len() > DUCKDB_PUSHDOWN_MAX_EDGES {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!(
                "subgraph edge count {} exceeds maximum pushdown limit {DUCKDB_PUSHDOWN_MAX_EDGES}",
                edges.len()
            ),
        ));
    }
    if nodes.is_empty() {
        return Ok(Vec::new());
    }

    let spill = TempSpillDir::create()?;
    let conn = open_isolated_pushdown_conn(&spill)?;
    load_compact_graph_tables(&conn, nodes, edges)?;

    match algo {
        GraphAlgo::Degree => solve_degree_pushdown(&conn, nodes, params),
        GraphAlgo::ConnectedComponents | GraphAlgo::WeaklyConnected => {
            let max_iters = resolve_pushdown_iters(params);
            solve_connected_components_pushdown(&conn, nodes, max_iters)
        }
        GraphAlgo::LabelPropagation => {
            let max_iters = resolve_pushdown_iters(params);
            solve_label_propagation_pushdown(&conn, nodes, max_iters)
        }
        GraphAlgo::Pagerank | GraphAlgo::StronglyConnected => Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            "algorithm is not supported by DUCKDB_PUSHDOWN",
        )),
    }
}

fn open_isolated_pushdown_conn(spill: &TempSpillDir) -> Result<duckdb::Connection, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|err| AppError::Database(format!("open duckdb pushdown session failed: {err}")))?;
    let escaped_dir = spill.0.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!(
        "SET autoinstall_known_extensions = false;
         SET temp_directory = '{escaped_dir}';
         SET memory_limit = '1GB';
         SET max_temp_directory_size = '2GB';
         SET enable_external_access = false;"
    ))
    .map_err(|err| AppError::Database(format!("configure duckdb pushdown limits failed: {err}")))?;
    Ok(conn)
}

fn load_compact_graph_tables(
    conn: &duckdb::Connection,
    nodes: &[(Uuid, String)],
    edges: &[(Uuid, Uuid, f64)],
) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE _tmp_nodes (idx UINTEGER NOT NULL, node_id VARCHAR NOT NULL, title VARCHAR NOT NULL);
         CREATE TABLE _tmp_edges (src_idx UINTEGER NOT NULL, dst_idx UINTEGER NOT NULL, weight DOUBLE NOT NULL);",
    )
    .map_err(|err| AppError::Database(format!("create _tmp_nodes/_tmp_edges failed: {err}")))?;

    let mut idx_by_uuid: HashMap<Uuid, u32> = HashMap::with_capacity(nodes.len());
    {
        let mut node_app = conn
            .appender("_tmp_nodes")
            .map_err(|err| AppError::Database(format!("open _tmp_nodes appender failed: {err}")))?;
        for (pos, (nid, title)) in nodes.iter().enumerate() {
            let idx = u32::try_from(pos).map_err(|_| {
                AppError::bad("LIMIT_OUT_OF_RANGE", "node count exceeds u32 capacity")
            })?;
            idx_by_uuid.insert(*nid, idx);
            let nid_str = nid.to_string();
            node_app
                .append_row(duckdb::params![idx, nid_str.as_str(), title.as_str()])
                .map_err(|err| AppError::Database(format!("append _tmp_nodes failed: {err}")))?;
        }
        node_app
            .flush()
            .map_err(|err| AppError::Database(format!("flush _tmp_nodes failed: {err}")))?;
    }

    {
        let mut edge_app = conn
            .appender("_tmp_edges")
            .map_err(|err| AppError::Database(format!("open _tmp_edges appender failed: {err}")))?;
        for &(src, dst, raw_w) in edges {
            if let (Some(&u), Some(&v)) = (idx_by_uuid.get(&src), idx_by_uuid.get(&dst)) {
                let w = if raw_w.is_finite() {
                    raw_w.max(1e-9)
                } else {
                    1.0
                };
                edge_app
                    .append_row(duckdb::params![u, v, w])
                    .map_err(|err| {
                        AppError::Database(format!("append _tmp_edges failed: {err}"))
                    })?;
            }
        }
        edge_app
            .flush()
            .map_err(|err| AppError::Database(format!("flush _tmp_edges failed: {err}")))?;
    }
    Ok(())
}

fn resolve_pushdown_iters(params: &serde_json::Value) -> usize {
    params
        .get("max_iter")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .map_or(MAX_PUSHDOWN_ITERATIONS, |v| {
            v.clamp(1, MAX_PUSHDOWN_ITERATIONS)
        })
}

fn solve_degree_pushdown(
    conn: &duckdb::Connection,
    nodes: &[(Uuid, String)],
    params: &serde_json::Value,
) -> Result<Vec<PushdownNodeOutcome>, AppError> {
    let dir_str = params
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("BOTH")
        .to_ascii_uppercase();

    let sql = match dir_str.as_str() {
        "OUT" => {
            "WITH deg AS (
                 SELECT n.idx, COALESCE(o.cnt, 0)::DOUBLE AS d
                   FROM _tmp_nodes n
                   LEFT JOIN (SELECT src_idx AS idx, COUNT(*) AS cnt FROM _tmp_edges GROUP BY src_idx) o
                     ON o.idx = n.idx
             )
             SELECT idx, COALESCE(d / NULLIF(MAX(d) OVER (), 0.0), 0.0) AS score
               FROM deg
              ORDER BY idx ASC"
        }
        "IN" => {
            "WITH deg AS (
                 SELECT n.idx, COALESCE(i.cnt, 0)::DOUBLE AS d
                   FROM _tmp_nodes n
                   LEFT JOIN (SELECT dst_idx AS idx, COUNT(*) AS cnt FROM _tmp_edges GROUP BY dst_idx) i
                     ON i.idx = n.idx
             )
             SELECT idx, COALESCE(d / NULLIF(MAX(d) OVER (), 0.0), 0.0) AS score
               FROM deg
              ORDER BY idx ASC"
        }
        "BOTH" => {
            "WITH deg AS (
                 SELECT n.idx, (COALESCE(o.cnt, 0) + COALESCE(i.cnt, 0))::DOUBLE AS d
                   FROM _tmp_nodes n
                   LEFT JOIN (SELECT src_idx AS idx, COUNT(*) AS cnt FROM _tmp_edges GROUP BY src_idx) o
                     ON o.idx = n.idx
                   LEFT JOIN (SELECT dst_idx AS idx, COUNT(*) AS cnt FROM _tmp_edges GROUP BY dst_idx) i
                     ON i.idx = n.idx
             )
             SELECT idx, COALESCE(d / NULLIF(MAX(d) OVER (), 0.0), 0.0) AS score
               FROM deg
              ORDER BY idx ASC"
        }
        other => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid degree direction '{other}' (expected IN, OUT, or BOTH)"),
            ));
        }
    };

    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| AppError::Database(format!("prepare degree pushdown SQL failed: {err}")))?;
    let mut rows = stmt
        .query([])
        .map_err(|err| AppError::Database(format!("execute degree pushdown SQL failed: {err}")))?;

    let mut scores = vec![0.0_f64; nodes.len()];
    while let Some(row) = rows
        .next()
        .map_err(|err| AppError::Database(format!("read degree row failed: {err}")))?
    {
        let idx_u32: u32 = row
            .get(0)
            .map_err(|err| AppError::Database(format!("decode degree idx failed: {err}")))?;
        let score: f64 = row
            .get(1)
            .map_err(|err| AppError::Database(format!("decode degree score failed: {err}")))?;
        if let Ok(pos) = usize::try_from(idx_u32)
            && let Some(slot) = scores.get_mut(pos)
        {
            *slot = score.clamp(0.0, 1.0);
        }
    }

    Ok(nodes
        .iter()
        .enumerate()
        .map(|(i, (nid, title))| PushdownNodeOutcome {
            node_id: *nid,
            title: title.clone(),
            score: Some(scores.get(i).copied().unwrap_or(0.0)),
            community: None,
        })
        .collect())
}

fn solve_connected_components_pushdown(
    conn: &duckdb::Connection,
    nodes: &[(Uuid, String)],
    max_iters: usize,
) -> Result<Vec<PushdownNodeOutcome>, AppError> {
    conn.execute_batch(
        "CREATE TABLE _sym_edges AS
         SELECT DISTINCT src_idx AS u, dst_idx AS v FROM _tmp_edges WHERE src_idx <> dst_idx
         UNION
         SELECT DISTINCT dst_idx AS u, src_idx AS v FROM _tmp_edges WHERE src_idx <> dst_idx;
         CREATE TABLE _labels AS SELECT idx, idx AS label FROM _tmp_nodes;",
    )
    .map_err(|err| AppError::Database(format!("init CC pushdown tables failed: {err}")))?;

    for _ in 0..max_iters {
        conn.execute_batch(
            "DROP TABLE IF EXISTS _nbr_step;
             CREATE TABLE _nbr_step AS
             SELECT cur.idx,
                    LEAST(cur.label, COALESCE(nbr.min_lbl, cur.label)) AS label
               FROM _labels cur
               LEFT JOIN (
                    SELECT e.u AS idx, MIN(l.label) AS min_lbl
                      FROM _sym_edges e
                      JOIN _labels l ON l.idx = e.v
                     GROUP BY e.u
               ) nbr ON nbr.idx = cur.idx;
             DROP TABLE IF EXISTS _next_labels;
             CREATE TABLE _next_labels AS
             SELECT s.idx, p.label AS label
               FROM _nbr_step s
               JOIN _nbr_step p ON p.idx = s.label;",
        )
        .map_err(|err| AppError::Database(format!("iterate CC pushdown step failed: {err}")))?;

        let changed = count_changed_labels(conn)?;
        conn.execute_batch("DROP TABLE _labels; ALTER TABLE _next_labels RENAME TO _labels;")
            .map_err(|err| AppError::Database(format!("swap CC _labels failed: {err}")))?;
        if changed == 0 {
            break;
        }
    }

    read_canonical_communities(conn, nodes)
}

fn solve_label_propagation_pushdown(
    conn: &duckdb::Connection,
    nodes: &[(Uuid, String)],
    max_iters: usize,
) -> Result<Vec<PushdownNodeOutcome>, AppError> {
    conn.execute_batch(
        "CREATE TABLE _w_edges AS
         SELECT u, v, SUM(weight) AS weight
           FROM (
               SELECT src_idx AS u, dst_idx AS v, weight FROM _tmp_edges WHERE src_idx <> dst_idx
               UNION ALL
               SELECT dst_idx AS u, src_idx AS v, weight FROM _tmp_edges WHERE src_idx <> dst_idx
               UNION ALL
               SELECT idx AS u, idx AS v, 1e-6 AS weight FROM _tmp_nodes
           ) raw
          GROUP BY u, v;
         CREATE TABLE _labels AS SELECT idx, idx AS label FROM _tmp_nodes;",
    )
    .map_err(|err| AppError::Database(format!("init LPA pushdown tables failed: {err}")))?;

    for _ in 0..max_iters {
        conn.execute_batch(
            "DROP TABLE IF EXISTS _next_labels;
             CREATE TABLE _next_labels AS
             WITH ranked AS (
                 SELECT e.v AS dst_idx,
                        l.label AS cand_label,
                        ROW_NUMBER() OVER (
                            PARTITION BY e.v
                            ORDER BY SUM(e.weight) DESC, l.label ASC
                        ) AS rn
                   FROM _w_edges e
                   JOIN _labels l ON l.idx = e.u
                  GROUP BY e.v, l.label
             )
             SELECT cur.idx,
                    COALESCE(r.cand_label, cur.label) AS label
               FROM _labels cur
               LEFT JOIN ranked r ON r.dst_idx = cur.idx AND r.rn = 1;",
        )
        .map_err(|err| AppError::Database(format!("iterate LPA pushdown step failed: {err}")))?;

        let changed = count_changed_labels(conn)?;
        conn.execute_batch("DROP TABLE _labels; ALTER TABLE _next_labels RENAME TO _labels;")
            .map_err(|err| AppError::Database(format!("swap LPA _labels failed: {err}")))?;
        if changed == 0 {
            break;
        }
    }

    read_canonical_communities(conn, nodes)
}

fn count_changed_labels(conn: &duckdb::Connection) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM _next_labels n JOIN _labels c ON c.idx = n.idx WHERE n.label <> c.label",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|err| AppError::Database(format!("count changed pushdown labels failed: {err}")))
}

fn read_canonical_communities(
    conn: &duckdb::Connection,
    nodes: &[(Uuid, String)],
) -> Result<Vec<PushdownNodeOutcome>, AppError> {
    let mut stmt = conn
        .prepare(
            "WITH comp_stats AS (
                 SELECT label, COUNT(*) AS comp_size, MIN(idx) AS first_idx
                   FROM _labels
                  GROUP BY label
             ),
             comp_remap AS (
                 SELECT label,
                        (ROW_NUMBER() OVER (ORDER BY comp_size DESC, first_idx ASC) - 1)::BIGINT AS comm_id
                   FROM comp_stats
             )
             SELECT l.idx, r.comm_id
               FROM _labels l
               JOIN comp_remap r ON r.label = l.label
              ORDER BY l.idx ASC",
        )
        .map_err(|err| AppError::Database(format!("prepare canonical community SQL failed: {err}")))?;

    let mut rows = stmt
        .query([])
        .map_err(|err| AppError::Database(format!("query canonical communities failed: {err}")))?;
    let mut comms = vec![0_i64; nodes.len()];
    while let Some(row) = rows
        .next()
        .map_err(|err| AppError::Database(format!("read community row failed: {err}")))?
    {
        let idx_u32: u32 = row
            .get(0)
            .map_err(|err| AppError::Database(format!("decode community idx failed: {err}")))?;
        let comm_id: i64 = row
            .get(1)
            .map_err(|err| AppError::Database(format!("decode community id failed: {err}")))?;
        if let Ok(pos) = usize::try_from(idx_u32)
            && let Some(slot) = comms.get_mut(pos)
        {
            *slot = comm_id;
        }
    }

    Ok(nodes
        .iter()
        .enumerate()
        .map(|(i, (nid, title))| PushdownNodeOutcome {
            node_id: *nid,
            title: title.clone(),
            score: None,
            community: Some(comms.get(i).copied().unwrap_or(0)),
        })
        .collect())
}

pub(crate) fn rank_pushdown_top(
    outcomes: &[PushdownNodeOutcome],
    limit: usize,
) -> Vec<AlgoTopEntry> {
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
