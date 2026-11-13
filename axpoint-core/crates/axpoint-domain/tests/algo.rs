//! Pure functional graph algorithm unit tests (`04-STEP03` §03.6.2).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::algo::{
    AlgoEdge, DegreeParams, LabelPropagationParams, NodeAlgoOutput, PageRankParams,
    build_weighted_digraph, degree_centrality, execute_algorithm, label_propagation, pagerank,
    strongly_connected_components, unweighted_shortest_path, weakly_connected_components,
    weighted_shortest_path,
};
use axpoint_domain::enums::GraphAlgo;
use axpoint_domain::graph::TraversalDirection;
use uuid::Uuid;

fn deterministic_uuid(index: u128) -> Uuid {
    Uuid::from_u128(index.saturating_add(1))
}

fn score_map(outputs: &[NodeAlgoOutput]) -> BTreeMap<Uuid, f64> {
    let mut map = BTreeMap::new();
    for item in outputs {
        if let Some(s) = item.score {
            map.insert(item.node_id, s);
        }
    }
    map
}

fn community_map(outputs: &[NodeAlgoOutput]) -> BTreeMap<Uuid, i64> {
    let mut map = BTreeMap::new();
    for item in outputs {
        if let Some(c) = item.community {
            map.insert(item.node_id, c);
        }
    }
    map
}

#[test]
fn algo_pagerank_five_node_ring_plus_dangling_matches_reference() -> Result<(), AppError> {
    // 5-node directed ring: n0 -> n1 -> n2 -> n3 -> n4 -> n0, plus n0 -> n5 where n5 is dangling.
    let n0 = deterministic_uuid(0);
    let n1 = deterministic_uuid(1);
    let n2 = deterministic_uuid(2);
    let n3 = deterministic_uuid(3);
    let n4 = deterministic_uuid(4);
    let n5 = deterministic_uuid(5);
    let nodes = [n0, n1, n2, n3, n4, n5];

    let edges = [
        AlgoEdge {
            src: n0,
            dst: n1,
            weight: 1.0,
        },
        AlgoEdge {
            src: n1,
            dst: n2,
            weight: 1.0,
        },
        AlgoEdge {
            src: n2,
            dst: n3,
            weight: 1.0,
        },
        AlgoEdge {
            src: n3,
            dst: n4,
            weight: 1.0,
        },
        AlgoEdge {
            src: n4,
            dst: n0,
            weight: 1.0,
        },
        AlgoEdge {
            src: n0,
            dst: n5,
            weight: 1.0,
        },
    ];

    let graph = build_weighted_digraph(&nodes, &edges);
    let results = pagerank(&graph, PageRankParams::default())?;
    assert_eq!(results.len(), 6);

    let total_prob: f64 = results.iter().filter_map(|r| r.score).sum();
    assert!(
        (total_prob - 1.0).abs() < 1e-9,
        "PageRank probabilities must sum to 1.0, got {total_prob}"
    );

    // Exact stationary fixed-point reference constants (damping = 0.85):
    // r0*(1 - d^5/2) = c*(1 + d + d^2 + d^3 + d^4), r1* = r5* = c + (d/2)*r0*, r_{k}* = c + d*r_{k-1}*.
    let reference: [(Uuid, f64); 6] = [
        (n0, 0.208_528_469_103_864_13),
        (n1, 0.132_378_174_115_726_4),
        (n2, 0.156_275_067_537_269_25),
        (n3, 0.176_587_426_945_580_7),
        (n4, 0.193_852_688_181_833_16),
        (n5, 0.132_378_174_115_726_4),
    ];

    let scores = score_map(&results);
    for (node_id, expected) in reference {
        let actual = scores
            .get(&node_id)
            .copied()
            .ok_or_else(|| AppError::bad("MISSING_SCORE", "missing PageRank score"))?;
        let err = (actual - expected).abs();
        assert!(
            err < 1e-6,
            "node {node_id} PageRank {actual} differed from reference {expected} by {err}"
        );
    }

    Ok(())
}

#[test]
fn algo_pagerank_isolated_dangling_and_validation() -> Result<(), AppError> {
    let n0 = deterministic_uuid(0);
    let n1 = deterministic_uuid(1);
    let n2 = deterministic_uuid(2);
    let n3 = deterministic_uuid(3);
    let n4 = deterministic_uuid(4);
    let n5 = deterministic_uuid(5);
    let nodes = [n0, n1, n2, n3, n4, n5];

    // Symmetric 5-node ring + 1 dangling node n5 whose non-positive outgoing edge degenerates
    // to uniform redistribution across all N = 6 nodes.
    let sym_edges = [
        AlgoEdge {
            src: n0,
            dst: n1,
            weight: 2.0,
        },
        AlgoEdge {
            src: n1,
            dst: n2,
            weight: 2.0,
        },
        AlgoEdge {
            src: n2,
            dst: n3,
            weight: 2.0,
        },
        AlgoEdge {
            src: n3,
            dst: n4,
            weight: 2.0,
        },
        AlgoEdge {
            src: n4,
            dst: n0,
            weight: 2.0,
        },
        AlgoEdge {
            src: n5,
            dst: n0,
            weight: 0.0,
        },
    ];

    let sym_out = execute_algorithm(
        GraphAlgo::Pagerank,
        &nodes,
        &sym_edges,
        &serde_json::json!({ "damping": 0.85, "max_iter": 100, "tol": 1e-9 }),
    )?;
    let sym_scores = score_map(&sym_out);
    let sym_sum: f64 = sym_scores.values().copied().sum();
    assert!((sym_sum - 1.0).abs() < 1e-9);

    let expected_dangling = 0.15 / 5.15;
    let expected_ring = (1.0 - expected_dangling) / 5.0;
    let got_n5 = sym_scores
        .get(&n5)
        .copied()
        .ok_or_else(|| AppError::bad("MISSING_SCORE", "missing n5"))?;
    assert!((got_n5 - expected_dangling).abs() < 1e-6);
    for ring_node in [n0, n1, n2, n3, n4] {
        let got = sym_scores
            .get(&ring_node)
            .copied()
            .ok_or_else(|| AppError::bad("MISSING_SCORE", "missing ring node"))?;
        assert!((got - expected_ring).abs() < 1e-6);
    }

    let empty_res = pagerank(&build_weighted_digraph(&[], &[]), PageRankParams::default())?;
    assert_eq!(empty_res.len(), 0);

    assert!(
        execute_algorithm(
            GraphAlgo::Pagerank,
            &nodes,
            &sym_edges,
            &serde_json::json!({ "damping": 1.0 }),
        )
        .is_err()
    );
    assert!(
        execute_algorithm(
            GraphAlgo::Pagerank,
            &nodes,
            &sym_edges,
            &serde_json::json!({ "max_iter": 501 }),
        )
        .is_err()
    );

    Ok(())
}

#[test]
fn algo_label_propagation_two_triangles_and_weak_bridge_deterministic() -> Result<(), AppError> {
    let a0 = deterministic_uuid(10);
    let a1 = deterministic_uuid(11);
    let a2 = deterministic_uuid(12);
    let b0 = deterministic_uuid(20);
    let b1 = deterministic_uuid(21);
    let b2 = deterministic_uuid(22);
    let nodes = [a0, a1, a2, b0, b1, b2];

    let disjoint_edges = [
        AlgoEdge {
            src: a0,
            dst: a1,
            weight: 1.0,
        },
        AlgoEdge {
            src: a1,
            dst: a2,
            weight: 1.0,
        },
        AlgoEdge {
            src: a2,
            dst: a0,
            weight: 1.0,
        },
        AlgoEdge {
            src: b0,
            dst: b1,
            weight: 1.0,
        },
        AlgoEdge {
            src: b1,
            dst: b2,
            weight: 1.0,
        },
        AlgoEdge {
            src: b2,
            dst: b0,
            weight: 1.0,
        },
    ];

    let graph = build_weighted_digraph(&nodes, &disjoint_edges);
    let params = LabelPropagationParams {
        max_iter: 30,
        seed: 42,
    };
    let out_disjoint = label_propagation(&graph, params)?;
    let comm_disjoint = community_map(&out_disjoint);

    let ca0 = comm_disjoint.get(&a0).copied();
    let ca1 = comm_disjoint.get(&a1).copied();
    let ca2 = comm_disjoint.get(&a2).copied();
    let cb0 = comm_disjoint.get(&b0).copied();
    let cb1 = comm_disjoint.get(&b1).copied();
    let cb2 = comm_disjoint.get(&b2).copied();

    assert!(ca0.is_some());
    assert_eq!(ca0, ca1);
    assert_eq!(ca1, ca2);
    assert!(cb0.is_some());
    assert_eq!(cb0, cb1);
    assert_eq!(cb1, cb2);
    assert_ne!(ca0, cb0);

    let distinct_disjoint: BTreeSet<i64> = comm_disjoint.values().copied().collect();
    assert_eq!(distinct_disjoint.len(), 2);

    // Weak bridge (weight 0.1 << internal triangle weight 2.0) preserves 2 independent communities.
    let mut bridged_edges = disjoint_edges.to_vec();
    bridged_edges.push(AlgoEdge {
        src: a2,
        dst: b0,
        weight: 0.1,
    });

    let reversed_nodes = [b2, a1, b0, a2, b1, a0];
    let baseline = execute_algorithm(
        GraphAlgo::LabelPropagation,
        &nodes,
        &bridged_edges,
        &serde_json::json!({ "max_iter": 30, "seed": 42 }),
    )?;

    let bridged_comms = community_map(&baseline);
    let distinct_bridged: BTreeSet<i64> = bridged_comms.values().copied().collect();
    assert_eq!(distinct_bridged.len(), 2);
    assert_eq!(bridged_comms.get(&a0), bridged_comms.get(&a1));
    assert_eq!(bridged_comms.get(&a1), bridged_comms.get(&a2));
    assert_eq!(bridged_comms.get(&b0), bridged_comms.get(&b1));
    assert_eq!(bridged_comms.get(&b1), bridged_comms.get(&b2));
    assert_ne!(bridged_comms.get(&a0), bridged_comms.get(&b0));

    for _ in 0..5 {
        let rerun = execute_algorithm(
            GraphAlgo::LabelPropagation,
            &reversed_nodes,
            &bridged_edges,
            &serde_json::json!({ "max_iter": 30, "seed": 42 }),
        )?;
        assert_eq!(rerun, baseline);
    }

    Ok(())
}

#[test]
fn algo_components_scc_and_wcc_with_self_loops_and_isolated_nodes() -> Result<(), AppError> {
    // Ranks in ascending Uuid order:
    // 0: c0, 1: c1, 2: c2 (3-cycle + self-loop on c1)
    // 3: d0, 4: d1 (2-cycle reachable one-way from c2 -> d0)
    // 5: s0 (singleton with self-loop s0 -> s0)
    // 6: i0 (completely isolated node)
    let c0 = deterministic_uuid(100);
    let c1 = deterministic_uuid(101);
    let c2 = deterministic_uuid(102);
    let d0 = deterministic_uuid(103);
    let d1 = deterministic_uuid(104);
    let s0 = deterministic_uuid(105);
    let i0 = deterministic_uuid(106);

    let nodes = [i0, s0, d1, d0, c2, c1, c0];
    let edges = [
        AlgoEdge {
            src: c0,
            dst: c1,
            weight: 1.0,
        },
        AlgoEdge {
            src: c1,
            dst: c2,
            weight: 1.0,
        },
        AlgoEdge {
            src: c2,
            dst: c0,
            weight: 1.0,
        },
        AlgoEdge {
            src: c1,
            dst: c1,
            weight: 1.0,
        },
        AlgoEdge {
            src: c2,
            dst: d0,
            weight: 1.0,
        },
        AlgoEdge {
            src: d0,
            dst: d1,
            weight: 1.0,
        },
        AlgoEdge {
            src: d1,
            dst: d0,
            weight: 1.0,
        },
        AlgoEdge {
            src: s0,
            dst: s0,
            weight: 1.0,
        },
    ];

    let graph = build_weighted_digraph(&nodes, &edges);

    let scc = strongly_connected_components(&graph)?;
    let scc_map = community_map(&scc);
    let strong_communities: BTreeSet<i64> = scc_map.values().copied().collect();
    assert_eq!(strong_communities.len(), 4);
    assert_eq!(scc_map.get(&c0), Some(&0));
    assert_eq!(scc_map.get(&c1), Some(&0));
    assert_eq!(scc_map.get(&c2), Some(&0));
    assert_eq!(scc_map.get(&d0), Some(&3));
    assert_eq!(scc_map.get(&d1), Some(&3));
    assert_eq!(scc_map.get(&s0), Some(&5));
    assert_eq!(scc_map.get(&i0), Some(&6));

    let cc_strong = execute_algorithm(
        GraphAlgo::ConnectedComponents,
        &nodes,
        &edges,
        &serde_json::json!({}),
    )?;
    assert_eq!(cc_strong, scc);

    let wcc = weakly_connected_components(&graph)?;
    let wcc_map = community_map(&wcc);
    let weak_communities: BTreeSet<i64> = wcc_map.values().copied().collect();
    assert_eq!(weak_communities.len(), 3);
    for merged in [c0, c1, c2, d0, d1] {
        assert_eq!(wcc_map.get(&merged), Some(&0));
    }
    assert_eq!(wcc_map.get(&s0), Some(&5));
    assert_eq!(wcc_map.get(&i0), Some(&6));

    let cc_weak = execute_algorithm(
        GraphAlgo::ConnectedComponents,
        &nodes,
        &edges,
        &serde_json::json!({ "weak": true }),
    )?;
    assert_eq!(cc_weak, wcc);

    let direct_weak = execute_algorithm(
        GraphAlgo::WeaklyConnected,
        &nodes,
        &edges,
        &serde_json::Value::Null,
    )?;
    assert_eq!(direct_weak, wcc);

    let direct_strong = execute_algorithm(
        GraphAlgo::StronglyConnected,
        &nodes,
        &edges,
        &serde_json::Value::Null,
    )?;
    assert_eq!(direct_strong, scc);

    Ok(())
}

#[test]
fn algo_degree_centrality_normalized_directions_and_weights() -> Result<(), AppError> {
    let start = deterministic_uuid(200);
    let mid = deterministic_uuid(201);
    let goal = deterministic_uuid(202);
    let isolated = deterministic_uuid(203);
    let nodes = [start, mid, goal, isolated];

    let edges = [
        AlgoEdge {
            src: start,
            dst: mid,
            weight: 0.9,
        },
        AlgoEdge {
            src: mid,
            dst: goal,
            weight: 0.9,
        },
        AlgoEdge {
            src: start,
            dst: goal,
            weight: 0.05,
        },
    ];

    let graph = build_weighted_digraph(&nodes, &edges);
    let out_deg = score_map(&degree_centrality(
        &graph,
        DegreeParams {
            direction: TraversalDirection::Out,
            weighted: false,
        },
    )?);
    assert!((out_deg.get(&start).copied().unwrap_or(-1.0) - (2.0 / 3.0)).abs() < 1e-12);
    assert!((out_deg.get(&mid).copied().unwrap_or(-1.0) - (1.0 / 3.0)).abs() < 1e-12);
    assert!((out_deg.get(&goal).copied().unwrap_or(-1.0)).abs() < 1e-12);
    assert!((out_deg.get(&isolated).copied().unwrap_or(-1.0)).abs() < 1e-12);

    let in_deg = score_map(&execute_algorithm(
        GraphAlgo::Degree,
        &nodes,
        &edges,
        &serde_json::json!({ "direction": "IN" }),
    )?);
    assert!((in_deg.get(&goal).copied().unwrap_or(-1.0) - (2.0 / 3.0)).abs() < 1e-12);
    assert!((in_deg.get(&mid).copied().unwrap_or(-1.0) - (1.0 / 3.0)).abs() < 1e-12);
    assert!((in_deg.get(&start).copied().unwrap_or(-1.0)).abs() < 1e-12);

    let both_deg = score_map(&execute_algorithm(
        GraphAlgo::Degree,
        &nodes,
        &edges,
        &serde_json::json!({ "direction": "BOTH" }),
    )?);
    assert!((both_deg.get(&mid).copied().unwrap_or(-1.0) - (2.0 / 3.0)).abs() < 1e-12);

    let weighted_out = score_map(&execute_algorithm(
        GraphAlgo::Degree,
        &nodes,
        &edges,
        &serde_json::json!({ "direction": "OUT", "weighted": true }),
    )?);
    assert!((weighted_out.get(&start).copied().unwrap_or(-1.0) - (0.95 / 3.0)).abs() < 1e-12);

    Ok(())
}

#[test]
fn algo_shortest_paths_weighted_inverse_cost_and_bidirectional_bfs() {
    let start = deterministic_uuid(200);
    let mid = deterministic_uuid(201);
    let goal = deterministic_uuid(202);
    let isolated = deterministic_uuid(203);
    let nodes = [start, mid, goal, isolated];

    let edges = [
        AlgoEdge {
            src: start,
            dst: mid,
            weight: 0.9,
        },
        AlgoEdge {
            src: mid,
            dst: goal,
            weight: 0.9,
        },
        AlgoEdge {
            src: start,
            dst: goal,
            weight: 0.05,
        },
    ];

    let graph = build_weighted_digraph(&nodes, &edges);

    // Unweighted bidirectional BFS picks the 1-hop path [start, goal] (cost = 1.0).
    let bfs_res = unweighted_shortest_path(&graph, start, goal, 6);
    assert_eq!(bfs_res.path, vec![start, goal]);
    assert_eq!(bfs_res.edges.len(), 1);
    assert!((bfs_res.total_cost.unwrap_or(-1.0) - 1.0).abs() < 1e-12);

    // Weighted Dijkstra (cost = 1.0 / weight) picks the 2-hop high-weight path [start, mid, goal]
    // with cost 1/0.9 + 1/0.9 = 2.222... instead of direct 1/0.05 = 20.0.
    let dijkstra_res = weighted_shortest_path(&graph, start, goal, Some(6));
    assert_eq!(dijkstra_res.path, vec![start, mid, goal]);
    assert_eq!(dijkstra_res.edges.len(), 2);
    let expected_cost = (1.0 / 0.9) + (1.0 / 0.9);
    assert!((dijkstra_res.total_cost.unwrap_or(-1.0) - expected_cost).abs() < 1e-12);

    // Hop limit max_depth = Some(1) forces weighted_shortest_path onto the 1-hop direct edge.
    let hop1_res = weighted_shortest_path(&graph, start, goal, Some(1));
    assert_eq!(hop1_res.path, vec![start, goal]);
    assert!((hop1_res.total_cost.unwrap_or(-1.0) - 20.0).abs() < 1e-9);

    // Unreachable isolated node returns empty path and None cost in both modes.
    let unreach_w = weighted_shortest_path(&graph, start, isolated, None);
    assert_eq!(unreach_w.path.len(), 0);
    assert_eq!(unreach_w.edges.len(), 0);
    assert!(unreach_w.total_cost.is_none());

    let unreach_bfs = unweighted_shortest_path(&graph, start, isolated, 6);
    assert_eq!(unreach_bfs.path.len(), 0);
    assert!(unreach_bfs.total_cost.is_none());
}
