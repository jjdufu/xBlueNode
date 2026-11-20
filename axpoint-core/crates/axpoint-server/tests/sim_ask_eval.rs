//! Offline 15-case evaluation suite for Step 10 `sim_ask` (`WHAT_IF`, `DuckDB` `QUERY_RESULT`,
//! and grounded `CompareBriefing`) enforcing >= 90% accuracy (`09-STEP10` §10.12, `A5` §4).

use std::collections::BTreeMap;
use std::future::Future;

use axpoint_domain::enums::InterventionMode;
use axpoint_domain::ports::AiPort;
use axpoint_domain::sim::{
    CompareKpiCell, CompareKpiRow, ImpactRow, ScenarioRecord, ScenarioStatus, ScenarioSummary,
    SimNode, SimParams,
};
use axpoint_domain::topology::AxSimAction;
use axpoint_domain::{AppError, ExtractedTriple, FusedHit};
use axpoint_server::sim_ask::{
    DisambiguationOutcome, disambiguate_what_if_actions, extract_what_if_interventions,
    generate_grounded_briefing, query_impacts_with_nl2sql,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

const MIN_EVAL_ACCURACY: f64 = 0.90;

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    kind: String,
    #[serde(default)]
    question: String,
    #[serde(default)]
    llm_response: String,
    #[serde(default)]
    sql_json: String,
    #[serde(default)]
    summary_text: String,
    #[serde(default)]
    expect_outcome: Option<String>,
    #[serde(default)]
    expect_node_key: Option<String>,
    #[serde(default)]
    expect_metric: Option<String>,
    #[serde(default)]
    expect_mode: Option<String>,
    #[serde(default)]
    expect_value: Option<f64>,
    #[serde(default)]
    expect_candidates_min: Option<usize>,
    #[serde(default)]
    expect_min_rows: Option<usize>,
    #[serde(default)]
    expect_sql_contains: Option<String>,
    #[serde(default)]
    expect_recommended: Option<String>,
    #[serde(default)]
    expect_deterministic_fallback: Option<bool>,
}

struct ScriptedCaseAi {
    what_if_raw: String,
    sql_json: String,
    summary_text: String,
}

impl AiPort for ScriptedCaseAi {
    fn embedding_model_name(&self) -> &'static str {
        "offline-eval"
    }

    fn current_embedding_dim(&self) -> Option<usize> {
        Some(4)
    }

    fn embed_batch(
        &self,
        inputs: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![vec![0.0; 4]; inputs.len()], 4)))
    }

    fn embed_one(
        &self,
        _text: &str,
    ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
        std::future::ready(Ok((vec![0.0; 4], 4)))
    }

    fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        let reply = if system_prompt.contains("SIM_ASK_WHATIF")
            || system_prompt.contains("SIM_COMPARE_BRIEFING")
        {
            self.what_if_raw.clone()
        } else if system_prompt.contains("结果总结")
            || user_prompt.contains("结果列:")
            || system_prompt.contains("数据分析报告")
        {
            self.summary_text.clone()
        } else {
            self.sql_json.clone()
        };
        std::future::ready(Ok(reply))
    }

    fn extract_triples(
        &self,
        _paragraph: &str,
    ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn extract_query_entities(
        &self,
        _query: &str,
    ) -> impl Future<Output = Result<Vec<String>, AppError>> + Send {
        std::future::ready(Ok(Vec::new()))
    }

    fn synthesize_answer(
        &self,
        _query: &str,
        _hits: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        std::future::ready(Ok(String::new()))
    }
}

fn fixture_nodes() -> (Vec<SimNode>, BTreeMap<String, Uuid>) {
    let mut ids = BTreeMap::new();
    let f1 = Uuid::from_u128(1);
    let w1 = Uuid::from_u128(2);
    let w2 = Uuid::from_u128(3);
    let o1 = Uuid::from_u128(4);
    let o2 = Uuid::from_u128(5);
    ids.insert(String::from("F-1"), f1);
    ids.insert(String::from("W-1"), w1);
    ids.insert(String::from("W-2"), w2);
    ids.insert(String::from("O-1"), o1);
    ids.insert(String::from("O-2"), o2);

    let mk = |id: Uuid, ty: &str, key: &str, title: &str, metric: &str, val: f64| {
        let mut metrics = BTreeMap::new();
        metrics.insert(String::from(metric), val);
        SimNode {
            id,
            type_name: String::from(ty),
            node_key: String::from(key),
            title: String::from(title),
            metrics,
            properties: serde_json::Map::new(),
            row_version: 1,
        }
    };

    let nodes = vec![
        mk(f1, "Factory", "F-1", "晶圆一厂 F-1", "output", 1000.0),
        mk(w1, "Warehouse", "W-1", "华东中心仓 W-1", "inventory", 500.0),
        mk(w2, "Warehouse", "W-2", "华东备用仓 W-2", "inventory", 250.0),
        mk(o1, "Order", "O-1", "订单 O-1", "fulfillment", 100.0),
        mk(o2, "Order", "O-2", "订单 O-2", "fulfillment", 120.0),
    ];
    (nodes, ids)
}

fn fixture_impacts(ids: &BTreeMap<String, Uuid>) -> Vec<ImpactRow> {
    let id = |k: &str| ids.get(k).copied().unwrap_or_else(Uuid::nil);
    vec![
        ImpactRow {
            node_id: id("F-1"),
            type_name: String::from("Factory"),
            title: String::from("晶圆一厂 F-1"),
            metric: String::from("output"),
            before_val: Some(1000.0),
            after_val: 700.0,
            delta: Some(-300.0),
            delta_ratio: Some(-0.30),
            p10: Some(670.0),
            p90: Some(730.0),
            hop_distance: 0,
            top_path: vec![id("F-1")],
            series: Vec::new(),
        },
        ImpactRow {
            node_id: id("W-1"),
            type_name: String::from("Warehouse"),
            title: String::from("华东中心仓 W-1"),
            metric: String::from("inventory"),
            before_val: Some(500.0),
            after_val: 380.0,
            delta: Some(-120.0),
            delta_ratio: Some(-0.24),
            p10: Some(360.0),
            p90: Some(400.0),
            hop_distance: 1,
            top_path: vec![id("F-1"), id("W-1")],
            series: Vec::new(),
        },
        ImpactRow {
            node_id: id("O-1"),
            type_name: String::from("Order"),
            title: String::from("订单 O-1"),
            metric: String::from("fulfillment"),
            before_val: Some(100.0),
            after_val: 76.0,
            delta: Some(-24.0),
            delta_ratio: Some(-0.24),
            p10: Some(71.0),
            p90: Some(81.0),
            hop_distance: 2,
            top_path: vec![id("F-1"), id("W-1"), id("O-1")],
            series: Vec::new(),
        },
        ImpactRow {
            node_id: id("O-2"),
            type_name: String::from("Order"),
            title: String::from("订单 O-2"),
            metric: String::from("fulfillment"),
            before_val: Some(120.0),
            after_val: 88.8,
            delta: Some(-31.2),
            delta_ratio: Some(-0.26),
            p10: Some(84.0),
            p90: Some(93.6),
            hop_distance: 2,
            top_path: vec![id("F-1"), id("W-1"), id("O-2")],
            series: Vec::new(),
        },
    ]
}

async fn run_single_case(case: &EvalCase) -> bool {
    let (nodes, ids) = fixture_nodes();
    let impacts = fixture_impacts(&ids);
    let ai = ScriptedCaseAi {
        what_if_raw: case.llm_response.clone(),
        sql_json: case.sql_json.clone(),
        summary_text: case.summary_text.clone(),
    };

    match case.kind.as_str() {
        "what_if" => eval_what_if_case(&ai, case, &nodes, &ids).await,
        "query_result" => eval_query_result_case(&ai, case, &impacts).await,
        "briefing" => eval_briefing_case(&ai, case).await,
        _ => false,
    }
}

async fn eval_what_if_case(
    ai: &ScriptedCaseAi,
    case: &EvalCase,
    nodes: &[SimNode],
    ids: &BTreeMap<String, Uuid>,
) -> bool {
    let Ok(extracted) = extract_what_if_interventions(ai, &case.question, nodes).await else {
        return false;
    };
    let outcome = disambiguate_what_if_actions(&extracted.interventions, nodes, &[]);
    match case.expect_outcome.as_deref() {
        Some("resolved") => match outcome {
            DisambiguationOutcome::Resolved(actions) => {
                let Some(first) = actions.first() else {
                    return false;
                };
                let expected_id = case
                    .expect_node_key
                    .as_deref()
                    .and_then(|k| ids.get(k))
                    .copied();
                let expected_mode = match case.expect_mode.as_deref() {
                    Some("ADD") => InterventionMode::Add,
                    Some("ABSOLUTE") => InterventionMode::Absolute,
                    _ => InterventionMode::Ratio,
                };
                Some(first.target_node_id) == expected_id
                    && Some(first.metric_key.as_str()) == case.expect_metric.as_deref()
                    && first.mode == Some(expected_mode)
                    && case
                        .expect_value
                        .is_none_or(|v| first.value.is_some_and(|fv| (fv - v).abs() < 1e-6))
            }
            _ => false,
        },
        Some("ambiguous") => match outcome {
            DisambiguationOutcome::Ambiguous { candidates, .. } => {
                candidates.len() >= case.expect_candidates_min.unwrap_or(2)
            }
            _ => false,
        },
        Some("not_found") => matches!(outcome, DisambiguationOutcome::NotFound { .. }),
        _ => false,
    }
}

async fn eval_query_result_case(
    ai: &ScriptedCaseAi,
    case: &EvalCase,
    impacts: &[ImpactRow],
) -> bool {
    let Ok(resp) = query_impacts_with_nl2sql(ai, "eval-tenant", impacts, &case.question).await
    else {
        return false;
    };
    let min_rows = case.expect_min_rows.unwrap_or(1);
    let sql_ok = case
        .expect_sql_contains
        .as_deref()
        .is_none_or(|needle| resp.sql.to_uppercase().contains(&needle.to_uppercase()));
    sql_ok && resp.rows.len() >= min_rows && !resp.answer.trim().is_empty()
}

fn briefing_fixture_scenario(sb: Uuid, id: Uuid, name: &str, max_d: f64) -> ScenarioRecord {
    let now = Utc::now();
    ScenarioRecord {
        id,
        sandbox_id: sb,
        name: String::from(name),
        description: String::new(),
        status: ScenarioStatus::Succeeded,
        actions: vec![AxSimAction {
            target_node_id: Uuid::from_u128(1),
            metric_key: String::from("output"),
            mode: Some(InterventionMode::Ratio),
            value: Some(-0.3),
            delta_ratio: Some(-0.3),
            at_step: 0,
        }],
        params: SimParams::default(),
        result: Some(ScenarioSummary {
            affected_count: 4,
            top_impacts: vec![ImpactRow {
                node_id: Uuid::from_u128(1),
                type_name: String::from("Factory"),
                title: String::from("晶圆一厂 F-1"),
                metric: String::from("output"),
                before_val: Some(1000.0),
                after_val: 1000.0 - max_d,
                delta: Some(-max_d),
                delta_ratio: Some(-max_d / 1000.0),
                p10: None,
                p90: None,
                hop_distance: 0,
                top_path: vec![Uuid::from_u128(1)],
                series: Vec::new(),
            }],
            kpis: Vec::new(),
            warnings: Vec::new(),
            converged: true,
            reason: None,
            iterations: 1,
        }),
        error_message: None,
        overlay_hash: String::from("h0"),
        is_stale: false,
        created_by: None,
        created_at: now,
        finished_at: Some(now),
    }
}

async fn eval_briefing_case(ai: &ScriptedCaseAi, case: &EvalCase) -> bool {
    let sb = Uuid::from_u128(10);
    let s1 = Uuid::from_u128(11);
    let s2 = Uuid::from_u128(12);
    let scenarios = vec![
        briefing_fixture_scenario(sb, s1, "s1", 300.0),
        briefing_fixture_scenario(sb, s2, "s2", 150.0),
    ];
    let mut kpi_vals = BTreeMap::new();
    kpi_vals.insert(
        s1.to_string(),
        CompareKpiCell {
            after_sum: 700.0,
            delta: -300.0,
            delta_ratio: Some(-0.3),
            affected_nodes: 1,
        },
    );
    kpi_vals.insert(
        s2.to_string(),
        CompareKpiCell {
            after_sum: 850.0,
            delta: -150.0,
            delta_ratio: Some(-0.15),
            affected_nodes: 1,
        },
    );
    let kpis = vec![CompareKpiRow {
        type_name: String::from("Factory"),
        metric: String::from("output"),
        before_sum: 1000.0,
        values: kpi_vals,
    }];

    let Ok(briefing) = generate_grounded_briefing(ai, &scenarios, &kpis, &[]).await else {
        return false;
    };
    let expected_rec = match case.expect_recommended.as_deref() {
        Some("s2") => Some(s2),
        _ => Some(s1),
    };
    let used_fallback = briefing
        .risks
        .iter()
        .any(|r| r == "BRIEFING_DETERMINISTIC_GROUNDED");
    briefing.recommended_scenario_id == expected_rec
        && Some(used_fallback) == case.expect_deterministic_fallback
}

#[tokio::test]
async fn sim_ask_eval_fifteen_cases_meet_accuracy_threshold() -> Result<(), AppError> {
    let raw = include_str!("sim_ask_eval/cases.json");
    let cases: Vec<EvalCase> = serde_json::from_str(raw)
        .map_err(|err| AppError::Config(format!("parse sim_ask_eval cases.json: {err}")))?;
    assert!(
        cases.len() >= 15,
        "expected >= 15 eval cases, found {}",
        cases.len()
    );

    let mut passed = 0_usize;
    let mut failed_ids = Vec::new();
    for case in &cases {
        if run_single_case(case).await {
            passed = passed.saturating_add(1);
        } else {
            failed_ids.push(case.id.clone());
        }
    }

    let total_f = f64::from(u32::try_from(cases.len()).unwrap_or(1));
    let passed_f = f64::from(u32::try_from(passed).unwrap_or(0));
    let accuracy = passed_f / total_f;
    assert!(
        accuracy >= MIN_EVAL_ACCURACY,
        "sim_ask_eval accuracy {accuracy:.3} ({passed}/{}) < {MIN_EVAL_ACCURACY}; failed: {:?}",
        cases.len(),
        failed_ids
    );
    Ok(())
}
