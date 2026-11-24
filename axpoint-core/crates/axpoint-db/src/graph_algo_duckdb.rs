//! Vectorized `DuckDB` relational-algebra pushdown engine for large-subgraph (`(500_000, 10_000_000]` edges)
//! graph algorithms (`Degree`, `ConnectedComponents`, `LabelPropagation`, `07-STEP29` §29.3).

use axpoint_domain::AppError;
use axpoint_domain::enums::GraphAlgo;
use axpoint_domain::graph::AlgoTopEntry;
use uuid::Uuid;

/// Maximum edge count processed in-memory by `petgraph` before switching to [`execute_duckdb_pushdown`].
pub const PETGRAPH_MAX_EDGES: usize = 500_000;

/// Hard upper bound on edge count accepted by `DuckDB` relational-algebra pushdown (`07-STEP29` §29.3).
pub const DUCKDB_PUSHDOWN_MAX_EDGES: usize = 10_000_000;

/// Executes `algo` via bounded `DuckDB` relational-algebra iterations (stubbed until Step 29 implementation).
///
/// # Errors
/// Returns [`AppError::Unsupported`] until Step 29 implementation lands, or [`AppError::Database`]
/// / [`AppError::BadRequest`] if `DuckDB` session creation or input validation fails.
pub fn execute_duckdb_pushdown(
    algo: GraphAlgo,
    nodes: &[(Uuid, String)],
    edges: &[(Uuid, Uuid, f64)],
    params: &serde_json::Value,
) -> Result<Vec<AlgoTopEntry>, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|err| AppError::Database(format!("open duckdb pushdown session failed: {err}")))?;
    drop((conn, algo, nodes, edges, params));
    Err(AppError::Unsupported("DUCKDB_ALGO_PUSHDOWN_UNIMPLEMENTED"))
}
