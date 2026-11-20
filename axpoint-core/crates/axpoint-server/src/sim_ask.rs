//! Natural-language `AxSim` Ask (`WHAT_IF` + `QUERY_RESULT`) and grounded comparison briefing engine (`09-STEP10` §10.7).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use axpoint_analytics::nl2sql::schema_link::SchemaLinkExtras;
use axpoint_analytics::{
    AskPipelineInput, AskTurnRecord, CatalogDatasetEntry, SchemaRecallBundle, TenantCatalog,
    TenantDb, run_ask_pipeline,
};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{AskIntent, AskRequest, AskResponse};
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, InterventionMode, PiiCategory};
use axpoint_domain::graph::NodeSearchHit;
use axpoint_domain::ports::AiPort;
use axpoint_domain::sim::{
    CompareBriefing, CompareKpiRow, CompareNodeRow, ImpactRow, ScenarioRecord, SimAskIntent,
    SimGraph, SimNode, SimResult,
};
use axpoint_domain::topology::AxSimAction;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ASK_FIXED_NOW_DEFAULT: &str = "2026-11-20T00:00:00Z";

/// Raw intervention entry extracted from a natural-language `WHAT_IF` question before node disambiguation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawWhatIfIntervention {
    /// Entity key, UUID, or title substring mentioned in the question.
    pub node_query: String,
    /// Metric key to perturb (defaults to `"output"` if empty).
    #[serde(default = "default_metric_key")]
    pub metric_key: String,
    /// Intervention mode (`RATIO` | `ADD` | `ABSOLUTE`).
    #[serde(default = "default_intervention_mode")]
    pub mode: InterventionMode,
    /// Numeric perturbation value.
    pub value: f64,
    /// Optional discrete replay step (`0` = immediate).
    #[serde(default)]
    pub at_step: u32,
}

fn default_metric_key() -> String {
    String::from("output")
}

const fn default_intervention_mode() -> InterventionMode {
    InterventionMode::Ratio
}

/// Structured LLM extraction result for `WHAT_IF` questions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhatIfExtraction {
    /// Extracted raw intervention specs.
    #[serde(default)]
    pub interventions: Vec<RawWhatIfIntervention>,
    /// True when the LLM needs clarification before acting.
    #[serde(default)]
    pub need_clarification: bool,
    /// Optional clarification prompt from the model.
    #[serde(default)]
    pub clarification_message: Option<String>,
}

/// Outcome of resolving raw `WHAT_IF` mentions against sandbox & graph nodes.
#[derive(Debug, Clone)]
pub enum DisambiguationOutcome {
    /// All interventions uniquely matched target nodes and normalized cleanly.
    Resolved(Vec<AxSimAction>),
    /// Multiple candidate nodes matched without an unambiguous winner.
    Ambiguous {
        /// Candidate nodes ordered by match confidence.
        candidates: Vec<axpoint_domain::sim::SimAskCandidate>,
        /// Human-readable clarification prompt.
        reason: String,
    },
    /// No matching nodes or interventions could be resolved.
    NotFound {
        /// Human-readable clarification prompt.
        reason: String,
    },
}

/// Classifies a `/api/axsim/ask` question into [`SimAskIntent::WhatIf`] or [`SimAskIntent::QueryResult`].
///
/// Uses deterministic linguistic cues first to avoid ambiguity, falling back to `ai.chat` only
/// when neither clear hypothetical cues nor result-query cues dominate.
///
/// # Errors
/// Propagates [`AppError`] only when an LLM routing fallback fails critically.
pub async fn classify_sim_ask_intent<A: AiPort>(
    ai: &A,
    question: &str,
    has_scenario: bool,
    has_sandbox: bool,
) -> Result<SimAskIntent, AppError> {
    let q = question.trim();
    let lower = q.to_lowercase();

    let hypothetical_cues = [
        "如果", "假如", "假设", "若", "设想", "what if", "what-if", "suppose", "降到", "下降",
        "降低", "减少", "下调", "提升", "增加", "上调", "停产", "断供", "中断", "drop by",
        "reduce", "increase",
    ];
    let query_cues = [
        "哪个",
        "哪些",
        "多少",
        "几个",
        "几条",
        "统计",
        "汇总",
        "总和",
        "平均",
        "最大",
        "最小",
        "最高",
        "最低",
        "排名",
        "前 ",
        "top ",
        "受影响",
        "影响最大",
        "列表",
        "列出",
        "查询",
        "显示",
        "how many",
        "which ",
        "top-",
    ];

    let has_hyp = hypothetical_cues.iter().any(|kw| lower.contains(kw));
    let has_qry = query_cues.iter().any(|kw| lower.contains(kw));

    if has_hyp && !has_qry {
        return Ok(SimAskIntent::WhatIf);
    }
    if has_qry && !has_hyp && has_scenario {
        return Ok(SimAskIntent::QueryResult);
    }
    if has_hyp {
        return Ok(SimAskIntent::WhatIf);
    }
    if has_qry && !has_sandbox {
        return Ok(SimAskIntent::QueryResult);
    }

    let sys = "PURPOSE:SIM_ASK_INTENT\nYou classify an AxSim user question as either WHAT_IF (hypothetical intervention/simulation) or QUERY_RESULT (querying existing scenario impacts). Reply with ONLY a single token: WHAT_IF or QUERY_RESULT.";
    let usr = format!("has_scenario={has_scenario} has_sandbox={has_sandbox}\nquestion: {q}");
    match ai.chat(sys, &usr).await {
        Ok(raw) => {
            let token = raw.trim().to_ascii_uppercase();
            if token.contains("QUERY_RESULT") {
                Ok(SimAskIntent::QueryResult)
            } else if token.contains("WHAT_IF") {
                Ok(SimAskIntent::WhatIf)
            } else if has_scenario {
                Ok(SimAskIntent::QueryResult)
            } else {
                Ok(SimAskIntent::WhatIf)
            }
        }
        Err(_) => {
            if has_scenario {
                Ok(SimAskIntent::QueryResult)
            } else {
                Ok(SimAskIntent::WhatIf)
            }
        }
    }
}

/// Prompts the LLM to extract structured `WHAT_IF` interventions from natural language.
///
/// # Errors
/// Returns [`AppError`] if the LLM call fails or emits malformed JSON that cannot be recovered.
pub async fn extract_what_if_interventions<A: AiPort>(
    ai: &A,
    question: &str,
    hint_nodes: &[SimNode],
) -> Result<WhatIfExtraction, AppError> {
    let mut hints = String::new();
    for node in hint_nodes.iter().take(40) {
        let metric_keys: Vec<&str> = node.metrics.keys().map(String::as_str).collect();
        let _w = writeln!(
            hints,
            "- id={} key={} type={} title={} metrics=[{}]",
            node.id,
            node.node_key,
            node.type_name,
            node.title,
            metric_keys.join(",")
        );
    }

    let sys = "PURPOSE:SIM_ASK_WHATIF\nYou extract causal simulation interventions from a natural-language What-If question. Output ONLY valid JSON: {\"interventions\":[{\"node_query\":\"<key or title>\",\"metric_key\":\"<metric>\",\"mode\":\"RATIO|ADD|ABSOLUTE\",\"value\":-0.3,\"at_step\":0}],\"need_clarification\":false,\"clarification_message\":null}. Use RATIO with decimal fraction (e.g. -30% => -0.3) for percentage changes, ADD for additive delta, ABSOLUTE for setting an exact target value.";
    let usr = format!("question: {}\ncandidate_nodes:\n{}", question.trim(), hints);

    let raw = ai.chat(sys, &usr).await?;
    parse_what_if_extraction(&raw)
}

/// Leniently parses [`WhatIfExtraction`] JSON, stripping markdown fences if present.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`PARSE_ERROR`) when no valid JSON object can be decoded.
pub fn parse_what_if_extraction(raw: &str) -> Result<WhatIfExtraction, AppError> {
    let trimmed = raw.trim();
    if let Ok(parsed) = serde_json::from_str::<WhatIfExtraction>(trimmed) {
        return Ok(parsed);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Some(slice) = trimmed.get(start..=end)
        && let Ok(parsed) = serde_json::from_str::<WhatIfExtraction>(slice)
    {
        return Ok(parsed);
    }
    Err(AppError::bad(
        "PARSE_ERROR",
        "failed to decode WHAT_IF extraction JSON",
    ))
}

/// Resolves extracted raw entity mentions against `graph_nodes` and hybrid search `extra_hits`.
#[must_use]
pub fn disambiguate_what_if_actions(
    raw: &[RawWhatIfIntervention],
    graph_nodes: &[SimNode],
    extra_hits: &[NodeSearchHit],
) -> DisambiguationOutcome {
    if raw.is_empty() {
        return DisambiguationOutcome::NotFound {
            reason: String::from(
                "未能从问题中识别出明确的节点干预参数，请指定目标节点与指标变化。",
            ),
        };
    }

    let mut actions = Vec::with_capacity(raw.len());
    for item in raw {
        let query = item.node_query.trim();
        if query.is_empty() {
            return DisambiguationOutcome::NotFound {
                reason: String::from("提取到的目标节点名称为空，请补充具体节点标识。"),
            };
        }

        let matched = match_single_node(query, graph_nodes, extra_hits);
        match matched {
            SingleNodeMatch::Unique(node_id, resolved_metric) => {
                let metric = if item.metric_key.trim().is_empty() {
                    resolved_metric.unwrap_or_else(default_metric_key)
                } else {
                    item.metric_key.trim().to_owned()
                };
                let delta_ratio = if item.mode == InterventionMode::Ratio {
                    Some(item.value)
                } else {
                    None
                };
                actions.push(AxSimAction {
                    target_node_id: node_id,
                    metric_key: metric,
                    mode: Some(item.mode),
                    value: Some(item.value),
                    delta_ratio,
                    at_step: item.at_step,
                });
            }
            SingleNodeMatch::Ambiguous(candidates) => {
                return DisambiguationOutcome::Ambiguous {
                    reason: format!(
                        "节点“{query}”匹配到多个候选对象，请选择具体目标节点后再执行推演。"
                    ),
                    candidates,
                };
            }
            SingleNodeMatch::None => {
                return DisambiguationOutcome::NotFound {
                    reason: format!("未能在当前图谱中定位到节点“{query}”，请确认节点名称或标识。"),
                };
            }
        }
    }

    DisambiguationOutcome::Resolved(actions)
}

enum SingleNodeMatch {
    Unique(Uuid, Option<String>),
    Ambiguous(Vec<axpoint_domain::sim::SimAskCandidate>),
    None,
}

fn nodes_to_candidates(nodes: &[&SimNode]) -> Vec<axpoint_domain::sim::SimAskCandidate> {
    nodes
        .iter()
        .take(8)
        .map(|n| axpoint_domain::sim::SimAskCandidate {
            node_id: n.id,
            type_name: n.type_name.clone(),
            title: n.title.clone(),
            node_key: n.node_key.clone(),
        })
        .collect()
}

fn match_single_node(
    query: &str,
    graph_nodes: &[SimNode],
    extra_hits: &[NodeSearchHit],
) -> SingleNodeMatch {
    if let Ok(uuid) = Uuid::parse_str(query)
        && let Some(node) = graph_nodes.iter().find(|n| n.id == uuid)
    {
        return SingleNodeMatch::Unique(node.id, node.metrics.keys().next().cloned());
    }

    let exact: Vec<&SimNode> = graph_nodes
        .iter()
        .filter(|n| n.node_key.eq_ignore_ascii_case(query) || n.title.eq_ignore_ascii_case(query))
        .collect();
    if let Some(first) = exact.first()
        && exact.len() == 1
    {
        return SingleNodeMatch::Unique(first.id, first.metrics.keys().next().cloned());
    }
    if exact.len() > 1 {
        return SingleNodeMatch::Ambiguous(nodes_to_candidates(&exact));
    }

    let q_lower = query.to_lowercase();
    let partial: Vec<&SimNode> = graph_nodes
        .iter()
        .filter(|n| {
            n.node_key.to_lowercase().contains(&q_lower)
                || n.title.to_lowercase().contains(&q_lower)
        })
        .collect();
    if let Some(first) = partial.first()
        && partial.len() == 1
    {
        return SingleNodeMatch::Unique(first.id, first.metrics.keys().next().cloned());
    }
    if partial.len() > 1 {
        return SingleNodeMatch::Ambiguous(nodes_to_candidates(&partial));
    }

    match_from_hybrid_hits(query, graph_nodes, extra_hits)
}

fn match_from_hybrid_hits(
    query: &str,
    graph_nodes: &[SimNode],
    extra_hits: &[NodeSearchHit],
) -> SingleNodeMatch {
    let filtered_hits: Vec<&NodeSearchHit> = if graph_nodes.is_empty() {
        extra_hits.iter().collect()
    } else {
        let in_graph: BTreeSet<Uuid> = graph_nodes.iter().map(|n| n.id).collect();
        extra_hits
            .iter()
            .filter(|h| in_graph.contains(&h.id))
            .collect()
    };

    if let Some(first) = filtered_hits.first()
        && (filtered_hits.len() == 1
            || first.node_key.eq_ignore_ascii_case(query)
            || first.title.eq_ignore_ascii_case(query))
    {
        return SingleNodeMatch::Unique(first.id, None);
    }
    if filtered_hits.len() > 1 {
        return SingleNodeMatch::Ambiguous(
            filtered_hits
                .into_iter()
                .take(8)
                .map(|h| axpoint_domain::sim::SimAskCandidate {
                    node_id: h.id,
                    type_name: h.type_name.clone(),
                    title: h.title.clone(),
                    node_key: h.node_key.clone(),
                })
                .collect(),
        );
    }

    SingleNodeMatch::None
}

/// Builds an in-memory `DuckDB` database and [`TenantCatalog`] populated with a single `impacts` table.
///
/// # Errors
/// Returns [`AppError::Database`] if in-memory `DuckDB` creation or row insertion fails.
pub fn build_impacts_duckdb(
    tenant_id: &str,
    impacts: &[ImpactRow],
) -> Result<(axpoint_analytics::duckdb::Connection, TenantCatalog), AppError> {
    let valid_tenant = !tenant_id.is_empty()
        && tenant_id.len() <= 64
        && tenant_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !valid_tenant {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "invalid tenant_id format for DuckDB impacts catalog",
        ));
    }
    let tenant_schema = format!("tenant_{tenant_id}");

    let conn = axpoint_analytics::duckdb::Connection::open_in_memory()
        .map_err(|err| AppError::Database(format!("open in-memory duckdb failed: {err}")))?;

    let ddl = format!(
        "ATTACH IF NOT EXISTS ':memory:' AS lake;
         CREATE SCHEMA IF NOT EXISTS lake.\"{tenant_schema}\";
         CREATE TABLE lake.\"{tenant_schema}\".\"impacts\" (
            node_id VARCHAR,
            type_name VARCHAR,
            title VARCHAR,
            metric VARCHAR,
            before_val DOUBLE,
            after_val DOUBLE,
            delta DOUBLE,
            delta_ratio DOUBLE,
            hop_distance INTEGER,
            p10 DOUBLE,
            p90 DOUBLE
        );"
    );

    conn.execute_batch(&ddl)
        .map_err(|err| AppError::Database(format!("create duckdb impacts table failed: {err}")))?;

    {
        let insert_sql = format!(
            "INSERT INTO lake.\"{tenant_schema}\".\"impacts\" (
                node_id, type_name, title, metric, before_val, after_val,
                delta, delta_ratio, hop_distance, p10, p90
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        );
        let mut stmt = conn
            .prepare(&insert_sql)
            .map_err(|err| AppError::Database(format!("prepare impacts insert failed: {err}")))?;

        for row in impacts {
            let hop_i64 = i64::from(row.hop_distance);
            stmt.execute(axpoint_analytics::duckdb::params![
                row.node_id.to_string(),
                row.type_name.as_str(),
                row.title.as_str(),
                row.metric.as_str(),
                row.before_val,
                row.after_val,
                row.delta,
                row.delta_ratio,
                hop_i64,
                row.p10,
                row.p90,
            ])
            .map_err(|err| AppError::Database(format!("insert impact row failed: {err}")))?;
        }
    }

    let col_specs = [
        ("node_id", DataType::String),
        ("type_name", DataType::String),
        ("title", DataType::String),
        ("metric", DataType::String),
        ("before_val", DataType::Float64),
        ("after_val", DataType::Float64),
        ("delta", DataType::Float64),
        ("delta_ratio", DataType::Float64),
        ("hop_distance", DataType::Int64),
        ("p10", DataType::Float64),
        ("p90", DataType::Float64),
    ];

    let mut ds_entry = CatalogDatasetEntry::simple(Uuid::nil(), "impacts", "impacts");
    ds_entry.display_name = String::from("仿真影响明细表");
    ds_entry.row_count = i64::try_from(impacts.len()).unwrap_or(i64::MAX);
    for (idx, (col_name, data_type)) in col_specs.iter().enumerate() {
        let ordinal = i32::try_from(idx).unwrap_or(0);
        ds_entry.columns.insert(
            String::from(*col_name),
            col_meta(col_name, *data_type, ordinal),
        );
    }

    let mut catalog = TenantCatalog::new(tenant_id);
    catalog.insert_dataset(ds_entry);

    Ok((conn, catalog))
}

fn col_meta(name: &str, data_type: DataType, ordinal: i32) -> ColumnMeta {
    ColumnMeta {
        dataset_id: Uuid::nil(),
        column_name: String::from(name),
        data_type,
        pii_category: PiiCategory::None,
        pii_confirmed: false,
        description: String::new(),
        business_term: String::new(),
        unit: String::new(),
        ordinal,
    }
}

/// Runs the Step 04 NL2SQL pipeline against an in-memory `DuckDB` `impacts` table (`§10.7`).
///
/// # Errors
/// Propagates [`AppError`] on empty question, `DuckDB` error, or upstream LLM failure.
pub async fn query_impacts_with_nl2sql<A: AiPort>(
    ai: &A,
    tenant_id: &str,
    impacts: &[ImpactRow],
    question: &str,
) -> Result<AskResponse, AppError> {
    let (conn, catalog) = build_impacts_duckdb(tenant_id, impacts)?;
    let session = TenantDb::from_connection(tenant_id, conn)?;
    session.refresh_catalog(catalog.clone());
    let req = AskRequest {
        question: question.to_owned(),
        session_id: None,
        context: None,
    };
    let recall = SchemaRecallBundle::default();
    let extras = SchemaLinkExtras {
        restrict_dataset_ids: vec![Uuid::nil()],
        ..SchemaLinkExtras::default()
    };
    let turns: Vec<AskTurnRecord> = Vec::new();
    let input = AskPipelineInput {
        request: &req,
        catalog: &catalog,
        db_recall: &recall,
        link_extras: &extras,
        few_shots: &[],
        session_turns: &turns,
        timezone: "UTC",
        now_rfc3339_override: Some(ASK_FIXED_NOW_DEFAULT),
        timeout_ms: Some(15_000),
        force_intent: Some(AskIntent::Data),
    };
    let out = run_ask_pipeline(ai, &session, &input).await?;
    Ok(out.response)
}

/// Checks that every numeric literal mentioned in `text` is grounded in `allowed_numbers`
/// (within 1.5% relative tolerance or `1e-2` absolute tolerance) or appears as a tiny integer
/// ordinal (`<= 10`) / substring inside `allowed_tokens` (such as `F-1` or `O-2`).
#[must_use]
pub fn verify_grounded_numbers(
    text: &str,
    allowed_numbers: &[f64],
    allowed_tokens: &[String],
) -> bool {
    let sanitized = strip_allowed_tokens(text, allowed_tokens);
    let extracted = extract_numeric_tokens(&sanitized);
    if extracted.is_empty() {
        return true;
    }

    for num in extracted {
        // Allow tiny step/rank ordinals (1..=10 integers) when comparing items, e.g. "2 个方案"
        let is_small_int = num.fract().abs() < 1e-9 && (0.0..=10.0).contains(&num);
        if is_small_int {
            continue;
        }
        let abs_match = allowed_numbers.iter().any(|allowed| {
            if !allowed.is_finite() {
                return false;
            }
            let diff = (allowed - num).abs();
            let tol = (allowed.abs() * 0.015).max(0.015);
            diff <= tol
                || ((allowed.abs() * 100.0) - num.abs()).abs() <= (allowed.abs() * 1.5).max(0.15)
        });
        if !abs_match {
            return false;
        }
    }
    true
}

fn strip_allowed_tokens(text: &str, allowed_tokens: &[String]) -> String {
    let mut out = text.to_owned();
    for tok in allowed_tokens {
        if !tok.trim().is_empty() {
            out = out.replace(tok, " ");
        }
    }
    out
}

fn extract_numeric_tokens(text: &str) -> Vec<f64> {
    let mut nums = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (idx, ch) in chars.iter().enumerate() {
        let is_sign = (*ch == '-' || *ch == '+')
            && cur.is_empty()
            && chars
                .get(idx.saturating_add(1))
                .is_some_and(char::is_ascii_digit);
        if ch.is_ascii_digit() || *ch == '.' || is_sign {
            cur.push(*ch);
        } else if !cur.is_empty() {
            if let Ok(parsed) = cur.parse::<f64>()
                && parsed.is_finite()
            {
                nums.push(parsed);
            }
            cur.clear();
        }
    }
    if !cur.is_empty()
        && let Ok(parsed) = cur.parse::<f64>()
        && parsed.is_finite()
    {
        nums.push(parsed);
    }
    nums
}

/// Generates a grounded natural-language summary of a `WHAT_IF` simulation result.
pub async fn summarize_what_if_result<A: AiPort>(
    ai: &A,
    question: &str,
    result: &SimResult,
) -> String {
    let mut top_rows: Vec<&ImpactRow> = result.impacts.iter().collect();
    top_rows.sort_by(|a, b| {
        b.delta
            .unwrap_or(0.0)
            .abs()
            .total_cmp(&a.delta.unwrap_or(0.0).abs())
    });

    let mut allowed_nums = vec![f64::from(
        u32::try_from(result.affected_count).unwrap_or(u32::MAX),
    )];
    let mut allowed_tokens = Vec::new();
    let mut facts = String::new();
    for row in top_rows.iter().take(12) {
        allowed_tokens.push(row.title.clone());
        allowed_tokens.push(row.metric.clone());
        allowed_nums.push(row.after_val);
        if let Some(b) = row.before_val {
            allowed_nums.push(b);
        }
        if let Some(d) = row.delta {
            allowed_nums.push(d);
            allowed_nums.push(d.abs());
        }
        if let Some(dr) = row.delta_ratio {
            allowed_nums.push(dr * 100.0);
        }
        let _w = writeln!(
            facts,
            "- {} [{}] {}.{}: before={:?}, after={:.2}, delta={:?}, hop={}",
            row.title,
            row.type_name,
            row.node_id,
            row.metric,
            row.before_val,
            row.after_val,
            row.delta,
            row.hop_distance
        );
    }
    for kpi in &result.kpis {
        allowed_tokens.push(kpi.type_name.clone());
        allowed_tokens.push(kpi.metric.clone());
        allowed_nums.push(kpi.before_sum);
        allowed_nums.push(kpi.after_sum);
        allowed_nums.push(kpi.delta);
        if let Some(dr) = kpi.delta_ratio {
            allowed_nums.push(dr * 100.0);
        }
    }

    let sys = "PURPOSE:SIM_ASK_SUMMARIZE\nYou summarize causal simulation results in concise Chinese using ONLY numbers present in the computed metrics. Never invent numbers.";
    let usr = format!(
        "question: {question}\naffected_count: {}\nimpacts:\n{facts}",
        result.affected_count
    );

    if let Ok(reply) = ai.chat(sys, &usr).await {
        let trimmed = reply.trim();
        if !trimmed.is_empty() && verify_grounded_numbers(trimmed, &allowed_nums, &allowed_tokens) {
            return trimmed.to_owned();
        }
    }

    deterministic_what_if_summary(result, &top_rows)
}

fn deterministic_what_if_summary(result: &SimResult, top_rows: &[&ImpactRow]) -> String {
    if top_rows.is_empty() {
        return String::from("推演已完成，当前扰动未产生超过阈值的指标影响。");
    }
    let mut parts = Vec::new();
    for row in top_rows.iter().take(3) {
        let delta_str = row.delta.map_or_else(
            || format!("={:.2}", row.after_val),
            |d| format!("{d:+.2} -> {:.2}", row.after_val),
        );
        parts.push(format!("{}({}) {}", row.title, row.metric, delta_str));
    }
    format!(
        "推演完成，共影响 {} 项节点指标；主要影响包括：{}。",
        result.affected_count,
        parts.join("，")
    )
}

#[derive(Debug, Deserialize)]
struct LlmBriefingRaw {
    #[serde(default)]
    recommended_scenario_id: Option<String>,
    #[serde(default)]
    recommended_scenario: Option<String>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    reasons: Vec<String>,
    #[serde(default)]
    key_tradeoffs: Vec<String>,
    #[serde(default)]
    risks: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

/// Generates a grounded comparison briefing for `GET /api/axsim/scenarios/compare?briefing=true` (`§10.7.2`).
///
/// Accepts either exact scenario UUIDs or unique scenario names from `recommended_scenario_id` /
/// `recommended_scenario`, verifies every quoted numeric literal against the comparison tables,
/// and degrades cleanly to a deterministic grounded summary if LLM validation fails.
///
/// # Errors
/// Returns [`AppError`] only on unexpected internal formatting issues (LLM failures degrade gracefully).
pub async fn generate_grounded_briefing<A: AiPort>(
    ai: &A,
    scenarios: &[ScenarioRecord],
    kpis: &[CompareKpiRow],
    key_nodes: &[CompareNodeRow],
) -> Result<CompareBriefing, AppError> {
    let (allowed_nums, allowed_tokens, table_text) =
        collect_briefing_grounding(scenarios, kpis, key_nodes);

    let sys = "PURPOSE:SIM_COMPARE_BRIEFING\nYou are a causal simulation decision analyst. Compare the scenarios below and output ONLY valid JSON: {\"recommended_scenario_id\":\"<exact uuid or scenario name>\",\"summary\":\"...\",\"reasons\":[\"...\"],\"risks\":[\"...\"]}. Cite ONLY exact numbers from the table.";
    let usr = format!("scenarios_and_kpis:\n{table_text}");

    if let Ok(raw) = ai.chat(sys, &usr).await
        && let Some(parsed) = parse_briefing_json(&raw)
    {
        let rec_ref = parsed
            .recommended_scenario_id
            .as_deref()
            .or(parsed.recommended_scenario.as_deref());
        let resolved_id = resolve_scenario_ref(rec_ref, scenarios);
        let reasons = if parsed.reasons.is_empty() {
            parsed.key_tradeoffs
        } else {
            parsed.reasons
        };
        let risks = if parsed.risks.is_empty() {
            parsed.warnings
        } else {
            parsed.risks
        };

        let mut combined_text = parsed.summary.clone();
        for r in &reasons {
            combined_text.push(' ');
            combined_text.push_str(r);
        }
        for rk in &risks {
            combined_text.push(' ');
            combined_text.push_str(rk);
        }

        if resolved_id.is_some()
            && !parsed.summary.trim().is_empty()
            && verify_grounded_numbers(&combined_text, &allowed_nums, &allowed_tokens)
        {
            return Ok(CompareBriefing {
                summary: parsed.summary,
                recommended_scenario_id: resolved_id,
                reasons,
                risks,
                error: None,
            });
        }
    }

    Ok(deterministic_compare_briefing(scenarios, kpis))
}

fn collect_briefing_grounding(
    scenarios: &[ScenarioRecord],
    kpis: &[CompareKpiRow],
    key_nodes: &[CompareNodeRow],
) -> (Vec<f64>, Vec<String>, String) {
    let mut allowed_nums = Vec::new();
    let mut allowed_tokens = Vec::new();
    let mut table_text = String::new();

    for sc in scenarios {
        allowed_tokens.push(sc.id.to_string());
        allowed_tokens.push(sc.name.clone());
        let max_d = scenario_max_abs_delta(sc);
        let _w = writeln!(
            table_text,
            "- scenario id={} name={} status={} affected={}",
            sc.id,
            sc.name,
            sc.status.as_str(),
            sc.result.as_ref().map_or(0, |s| s.affected_count)
        );
        if let Some(ref s) = sc.result {
            allowed_nums.push(f64::from(
                u32::try_from(s.affected_count).unwrap_or(u32::MAX),
            ));
            allowed_nums.push(max_d);
        }
    }

    for kpi in kpis {
        allowed_tokens.push(kpi.type_name.clone());
        allowed_tokens.push(kpi.metric.clone());
        allowed_nums.push(kpi.before_sum);
        for (sc_id, cell) in &kpi.values {
            allowed_tokens.push(sc_id.clone());
            allowed_nums.push(cell.after_sum);
            allowed_nums.push(cell.delta);
            if let Some(dr) = cell.delta_ratio {
                allowed_nums.push(dr * 100.0);
            }
            let _w = writeln!(
                table_text,
                "kpi {}.{} [{}]: before={:.2} after={:.2} delta={:+.2}",
                kpi.type_name, kpi.metric, sc_id, kpi.before_sum, cell.after_sum, cell.delta
            );
        }
    }

    for node in key_nodes {
        allowed_tokens.push(node.node_id.to_string());
        allowed_tokens.push(node.title.clone());
        allowed_tokens.push(node.metric.clone());
        if let Some(b) = node.before_val {
            allowed_nums.push(b);
        }
        for (sc_id, cell) in &node.values {
            allowed_tokens.push(sc_id.clone());
            allowed_nums.push(cell.after_val);
            if let Some(d) = cell.delta {
                allowed_nums.push(d);
            }
        }
    }

    (allowed_nums, allowed_tokens, table_text)
}

fn scenario_max_abs_delta(sc: &ScenarioRecord) -> f64 {
    sc.result.as_ref().map_or(0.0, |s| {
        s.top_impacts
            .iter()
            .filter_map(|r| r.delta)
            .map(f64::abs)
            .fold(0.0_f64, f64::max)
    })
}

fn parse_briefing_json(raw: &str) -> Option<LlmBriefingRaw> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<LlmBriefingRaw>(trimmed) {
        return Some(v);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Some(slice) = trimmed.get(start..=end)
        && let Ok(v) = serde_json::from_str::<LlmBriefingRaw>(slice)
    {
        return Some(v);
    }
    None
}

fn resolve_scenario_ref(raw_ref: Option<&str>, scenarios: &[ScenarioRecord]) -> Option<Uuid> {
    let needle = raw_ref?.trim();
    if needle.is_empty() {
        return None;
    }
    if let Ok(uuid) = Uuid::parse_str(needle)
        && scenarios.iter().any(|s| s.id == uuid)
    {
        return Some(uuid);
    }
    let name_matches: Vec<Uuid> = scenarios
        .iter()
        .filter(|s| s.name.eq_ignore_ascii_case(needle))
        .map(|s| s.id)
        .collect();
    if name_matches.len() == 1 {
        return name_matches.first().copied();
    }
    None
}

fn deterministic_compare_briefing(
    scenarios: &[ScenarioRecord],
    kpis: &[CompareKpiRow],
) -> CompareBriefing {
    let best = scenarios.iter().min_by(|a, b| {
        let da = a
            .result
            .as_ref()
            .map_or(f64::MAX, |_| scenario_max_abs_delta(a));
        let db = b
            .result
            .as_ref()
            .map_or(f64::MAX, |_| scenario_max_abs_delta(b));
        da.total_cmp(&db)
    });

    let rec_id = best.map(|s| s.id);
    let rec_label = best.map_or_else(
        || String::from("未确定"),
        |s| format!("{} ({})", s.name, s.id),
    );
    let summary = format!(
        "共对比 {} 个推演场景（{} 项聚合 KPI），综合最大波动幅度最小场景为 {}。",
        scenarios.len(),
        kpis.len(),
        rec_label
    );

    let mut reasons = Vec::new();
    for sc in scenarios {
        let affected = sc.result.as_ref().map_or(0, |s| s.affected_count);
        let max_d = scenario_max_abs_delta(sc);
        reasons.push(format!(
            "场景 {}：受影响指标 {} 项，最大绝对变动 {:.2}。",
            sc.name, affected, max_d
        ));
    }

    CompareBriefing {
        summary,
        recommended_scenario_id: rec_id,
        reasons,
        risks: vec![String::from("BRIEFING_DETERMINISTIC_GROUNDED")],
        error: None,
    }
}

/// Converts a sandbox graph's nodes into lightweight hint records for `WHAT_IF` prompts.
#[must_use]
pub fn graph_hint_nodes(graph: &SimGraph) -> Vec<SimNode> {
    graph.nodes.clone()
}
