//! Integration tests for `07-STEP29` §29.3 (`> 500,000` edge `DuckDB` graph algorithm pushdown).

use std::time::Instant;

use axpoint_db::graph_algo_duckdb::{PETGRAPH_MAX_EDGES, execute_duckdb_pushdown};
use axpoint_domain::AppError;
use axpoint_domain::enums::GraphAlgo;
use uuid::Uuid;

type SyntheticGraph = (Vec<(Uuid, String)>, Vec<(Uuid, Uuid, f64)>);

fn build_550k_two_cluster_graph() -> SyntheticGraph {
    // Two disjoint dense clusters of 2,000 nodes each (4,000 nodes total, 550,000 directed edges).
    let cluster_size: usize = 2_000;
    let total_nodes = cluster_size.saturating_mul(2);
    let mut nodes = Vec::with_capacity(total_nodes);
    for idx in 0..total_nodes {
        let uid = Uuid::from_u128(u128::try_from(idx.saturating_add(1)).unwrap_or(1));
        nodes.push((uid, format!("Node-{idx}")));
    }

    // 275,000 edges inside cluster A (0..2000) + 275,000 edges inside cluster B (2000..4000) = 550,000 edges.
    let edges_per_cluster: usize = 275_000;
    let mut edges = Vec::with_capacity(edges_per_cluster.saturating_mul(2));

    for base in [0_usize, cluster_size] {
        let mut emitted = 0_usize;
        for step in 1..cluster_size {
            if emitted >= edges_per_cluster {
                break;
            }
            for offset in 0..cluster_size {
                if emitted >= edges_per_cluster {
                    break;
                }
                let u_idx = base.saturating_add(offset);
                let v_idx = base.saturating_add((offset.saturating_add(step)) % cluster_size);
                if let (Some(&(u_id, _)), Some(&(v_id, _))) = (nodes.get(u_idx), nodes.get(v_idx)) {
                    edges.push((u_id, v_id, 1.0));
                    emitted = emitted.saturating_add(1);
                }
            }
        }
    }

    (nodes, edges)
}

#[test]
fn duckdb_pushdown_solves_550k_edge_degree_cc_and_lpa_within_budget() -> Result<(), AppError> {
    let (nodes, edges) = build_550k_two_cluster_graph();
    assert!(edges.len() > PETGRAPH_MAX_EDGES);
    assert_eq!(edges.len(), 550_000);

    let budget_secs: f64 = if cfg!(debug_assertions) { 30.0 } else { 8.0 };
    let t_deg = Instant::now();
    let deg_top = execute_duckdb_pushdown(
        GraphAlgo::Degree,
        &nodes,
        &edges,
        &serde_json::json!({"direction": "BOTH"}),
    )?;
    assert!(
        t_deg.elapsed().as_secs_f64() < budget_secs,
        "Degree pushdown exceeded budget"
    );
    assert_eq!(deg_top.len(), 50);
    assert!(
        deg_top
            .first()
            .and_then(|e| e.score)
            .is_some_and(|s| (s - 1.0).abs() < 1e-6)
    );

    let t_cc = Instant::now();
    let cc_top = execute_duckdb_pushdown(
        GraphAlgo::ConnectedComponents,
        &nodes,
        &edges,
        &serde_json::json!({"weak": true}),
    )?;
    assert!(
        t_cc.elapsed().as_secs_f64() < budget_secs,
        "ConnectedComponents pushdown exceeded budget"
    );
    assert_eq!(cc_top.len(), 50);
    assert_eq!(cc_top.first().and_then(|e| e.community), Some(0));

    let t_lpa = Instant::now();
    let lpa_top = execute_duckdb_pushdown(
        GraphAlgo::LabelPropagation,
        &nodes,
        &edges,
        &serde_json::json!({"max_iter": 10}),
    )?;
    assert!(
        t_lpa.elapsed().as_secs_f64() < budget_secs,
        "LabelPropagation pushdown exceeded budget"
    );
    assert_eq!(lpa_top.len(), 50);
    assert!(lpa_top.first().and_then(|e| e.community).is_some());

    let unsupported =
        execute_duckdb_pushdown(GraphAlgo::Pagerank, &nodes, &edges, &serde_json::json!({}));
    assert!(matches!(unsupported, Err(AppError::BadRequest { .. })));

    Ok(())
}
