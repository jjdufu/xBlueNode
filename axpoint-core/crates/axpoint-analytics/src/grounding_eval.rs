//! Dual-track 50-case anti-hallucination benchmark runner and shared RAG/hybrid grounding helpers (`02-STEP16` §16.4–§16.5).

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::future::{Future, ready};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use axpoint_domain::analytics::{
    AskCitation, AskIntent, AskRequest, FilterSpec, GroundingEvalTrack, MetricCompareMode,
    MetricDef, MetricDimension, MetricQuery, MetricTimeRange, MultiMetricQuery,
    NewGroundingEvalRunInput, QueryHistoryStatus, SchemaRecallBundle,
};
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, GroundingMode, PiiCategory, Purpose, RollupGrain};
use axpoint_domain::grounding::{
    ABSTAIN_NO_DATA_OR_DOC_ZH, ABSTAIN_NO_DOC_EVIDENCE_ZH, count_grounded_numbers,
    strip_citation_and_table_markers, verify_and_sanitize_citations,
};
use axpoint_domain::json_schema::{JsonSchemaSpec, parse_and_validate_json};
use axpoint_domain::ports::AiPort;
use axpoint_domain::studio::{
    EdgeTypeSpec, LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode, MappingSpec,
    NodeTypeSpec, OnSourceDelete, PropertyKind, PropertySpec, SchemaDoc,
};
use axpoint_domain::{
    AppError, ChannelCandidate, ExtractedTriple, FusedHit, RecallChannel, rrf_fuse,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::nl2sql::chart::{
    fallback_table_summary, verify_hybrid_numbers_grounded, verify_or_fallback_summary,
};
use crate::nl2sql::schema_link::{SchemaLinkExtras, compile_ontology_join_edges};
use crate::nl2sql::{AskPipelineInput, run_ask_pipeline_conn};
use crate::semantic::{
    MetricExecuteOptions, execute_metric_query_conn, execute_multi_metric_query_conn,
};
use crate::session::register_mask_udfs;
use crate::sql_guard::{CatalogDatasetEntry, TenantCatalog};

/// Minimum vector cosine similarity required to retain a `Vector` recall candidate (`02-STEP16` §16.4.2).
pub const MIN_DOC_VECTOR_COSINE_SIM: f64 = 0.45;

const SUITE_FIXTURE_JSON: &str = include_str!("../tests/fixtures/hallucination_50.json");
const CLOUD_CASSETTE_FALLBACK: &str =
    include_str!("../../../../tests/cassettes/hallucination_cloud_50.jsonl");
const LOCAL7B_CASSETTE_FALLBACK: &str =
    include_str!("../../../../tests/cassettes/hallucination_local7b_50.jsonl");

const QUERY_STOPWORDS: &[&str] = &[
    "是什么",
    "是什",
    "什么",
    "为什么",
    "怎么",
    "如何",
    "哪些",
    "多少",
    "是否",
    "请问",
    "具体",
    "相关",
    "关于",
    "根据",
    "按照",
    "条件",
    "要求",
    "规定",
    "制度",
    "规程",
    "细则",
    "规范",
    "办法",
    "条款",
    "维护",
    "管理",
    "流程",
];

/// Sanitized and verified `DOC` synthesis result (`02-STEP16` §16.4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedDocSynthesis {
    /// Sanitized answer text with out-of-bounds `[文N]` / `[Doc N]` markers stripped.
    pub answer: String,
    /// Resulting grounding mode (`GroundedDoc` or `Abstained`).
    pub grounding_mode: GroundingMode,
    /// Count of unique verified in-bounds citations (`1..=citations.len()`).
    pub verified_citations: u32,
    /// `true` when recalled citations were empty.
    pub abstained: bool,
}

/// Input bundle for [`sanitize_and_ground_hybrid_answer`].
pub struct HybridGroundingInput<'a> {
    /// Optional raw LLM hybrid synthesis text (`None` if synthesis was skipped or failed).
    pub raw_hybrid: Option<&'a str>,
    /// Verified structured `DATA` answer.
    pub data_answer: &'a str,
    /// Structured `DATA` result rows.
    pub data_rows: &'a [serde_json::Map<String, serde_json::Value>],
    /// Verified `DATA` grounding mode.
    pub data_grounding_mode: GroundingMode,
    /// Whether the `DATA` branch abstained.
    pub data_abstained: bool,
    /// Verified `DOC` answer.
    pub doc_answer: &'a str,
    /// Recalled document citations.
    pub doc_citations: &'a [AskCitation],
    /// Whether the `DOC` branch abstained.
    pub doc_abstained: bool,
}

/// Sanitized and verified `HYBRID` synthesis result (`02-STEP16` §16.4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedHybridSynthesis {
    /// Verified combined answer text.
    pub answer: String,
    /// Resulting grounding mode.
    pub grounding_mode: GroundingMode,
    /// Count of verified standalone numbers in `answer`.
    pub verified_numbers: u32,
    /// Count of verified in-bounds citations in `answer`.
    pub verified_citations: u32,
    /// `true` when both `DATA` and `DOC` abstained.
    pub abstained: bool,
}

/// Filters pure vector/BM25 stopword noise from 5-way hypergraph recall candidates when no graph
/// entity match exists (`02-STEP16` §16.4.2).
#[must_use]
pub fn filter_noise_recall_candidates(
    query: &str,
    candidates: Vec<ChannelCandidate>,
) -> Vec<ChannelCandidate> {
    let graph_paras: BTreeSet<Uuid> = candidates
        .iter()
        .filter(|c| {
            matches!(
                c.channel,
                RecallChannel::EntityLink | RecallChannel::MultiHop
            )
        })
        .map(|c| c.para_id)
        .collect();

    candidates
        .into_iter()
        .filter(|cand| {
            if graph_paras.contains(&cand.para_id) {
                return true;
            }
            match cand.channel {
                RecallChannel::EntityLink | RecallChannel::MultiHop | RecallChannel::Context => {
                    true
                }
                RecallChannel::Vector => {
                    parse_trace_cosine_sim(cand.trace.as_deref()) >= MIN_DOC_VECTOR_COSINE_SIM
                }
                RecallChannel::Bm25 => {
                    candidate_has_substantive_term(query, &cand.doc_title, &cand.content)
                }
            }
        })
        .collect()
}

/// Returns `true` when `doc_title` or `content` contains substantive non-stopword terms from `query`
/// (requiring `>= 2` matched CJK bigrams when `query` has `>= 3` substantive CJK bigrams so a single
/// generic suffix like `"引擎"` cannot trigger false recall).
#[must_use]
pub fn candidate_has_substantive_term(query: &str, doc_title: &str, content: &str) -> bool {
    let (cjk_terms, ascii_terms) = extract_substantive_query_terms(query);
    let title_low = doc_title.to_lowercase();
    let content_low = content.to_lowercase();

    if cjk_terms.is_empty() && ascii_terms.is_empty() {
        let q_trim = query.trim().to_lowercase();
        return q_trim.chars().count() >= 2 && title_low.contains(&q_trim);
    }
    if ascii_terms
        .iter()
        .any(|t| title_low.contains(t.as_str()) || content_low.contains(t.as_str()))
    {
        return true;
    }
    let min_cjk_hits = if cjk_terms.len() >= 3 { 2 } else { 1 };
    let matched_cjk = cjk_terms
        .iter()
        .filter(|t| title_low.contains(t.as_str()) || content_low.contains(t.as_str()))
        .count();
    matched_cjk >= min_cjk_hits
}

fn parse_trace_cosine_sim(trace: Option<&str>) -> f64 {
    let Some(raw) = trace else {
        return 0.0;
    };
    let Some(after) = raw.split("cosine_sim=").nth(1) else {
        return 0.0;
    };
    let num_part: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num_part.parse::<f64>().unwrap_or(0.0)
}

fn extract_substantive_query_terms(query: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut cleaned = strip_clause_ordinals(&query.to_lowercase());
    for sw in QUERY_STOPWORDS {
        cleaned = cleaned.replace(sw, " ");
    }

    let mut cjk_terms = BTreeSet::new();
    let mut ascii_terms = BTreeSet::new();
    let mut cjk_run: Vec<char> = Vec::new();
    let mut ascii_run = String::new();

    for ch in cleaned.chars() {
        if is_cjk_ideograph(ch)
            && !matches!(
                ch,
                '的' | '了' | '吗' | '呢' | '与' | '和' | '或' | '及' | '中' | '有' | '在'
            )
        {
            flush_ascii_run(&mut ascii_run, &mut ascii_terms);
            cjk_run.push(ch);
        } else if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            flush_cjk_run(&mut cjk_run, &mut cjk_terms);
            ascii_run.push(ch);
        } else {
            flush_cjk_run(&mut cjk_run, &mut cjk_terms);
            flush_ascii_run(&mut ascii_run, &mut ascii_terms);
        }
    }
    flush_cjk_run(&mut cjk_run, &mut cjk_terms);
    flush_ascii_run(&mut ascii_run, &mut ascii_terms);
    (cjk_terms, ascii_terms)
}

fn flush_cjk_run(run: &mut Vec<char>, terms: &mut BTreeSet<String>) {
    if run.len() >= 2 {
        for win in run.windows(2) {
            if let (Some(&a), Some(&b)) = (win.first(), win.get(1)) {
                let mut bg = String::with_capacity(6);
                bg.push(a);
                bg.push(b);
                if !QUERY_STOPWORDS.contains(&bg.as_str()) {
                    terms.insert(bg);
                }
            }
        }
    }
    run.clear();
}

fn flush_ascii_run(run: &mut String, terms: &mut BTreeSet<String>) {
    let trimmed = run.trim_matches(|c: char| c == '-' || c == '_');
    if trimmed.len() >= 2 && !trimmed.chars().all(|c| c.is_ascii_digit()) {
        terms.insert(String::from(trimmed));
    }
    run.clear();
}

fn strip_clause_ordinals(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut idx = 0_usize;
    while let Some(&ch) = chars.get(idx) {
        if ch == '第' {
            let mut cur = idx.saturating_add(1);
            while let Some(&d) = chars.get(cur) {
                if d.is_ascii_digit()
                    || matches!(
                        d,
                        '零' | '一'
                            | '二'
                            | '三'
                            | '四'
                            | '五'
                            | '六'
                            | '七'
                            | '八'
                            | '九'
                            | '十'
                            | '百'
                            | '千'
                    )
                {
                    cur = cur.saturating_add(1);
                } else {
                    break;
                }
            }
            if cur > idx.saturating_add(1)
                && chars
                    .get(cur)
                    .is_some_and(|u| matches!(u, '条' | '款' | '章' | '节' | '项'))
            {
                out.push(' ');
                idx = cur.saturating_add(1);
                continue;
            }
        }
        out.push(ch);
        idx = idx.saturating_add(1);
    }
    out
}

const fn is_cjk_ideograph(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
    )
}

/// Sanitizes inline `[文N]` / `[Doc N]` markers against `1..=citations.len()`, anchoring `[文1]`
/// when all markers were stripped or omitted on a non-empty recall (`02-STEP16` §16.4.2).
#[must_use]
pub fn sanitize_and_ground_doc_answer(
    raw_answer: &str,
    citations: &[AskCitation],
) -> GroundedDocSynthesis {
    let Some(first_cit) = citations.first() else {
        return GroundedDocSynthesis {
            answer: String::from(ABSTAIN_NO_DOC_EVIDENCE_ZH),
            grounding_mode: GroundingMode::Abstained,
            verified_citations: 0,
            abstained: true,
        };
    };

    let outcome = verify_and_sanitize_citations(raw_answer, citations.len());
    let verified_citations = outcome.verified_count();
    if verified_citations > 0 && !outcome.sanitized_answer.trim().is_empty() {
        return GroundedDocSynthesis {
            answer: outcome.sanitized_answer,
            grounding_mode: GroundingMode::GroundedDoc,
            verified_citations,
            abstained: false,
        };
    }

    let body = if outcome.sanitized_answer.trim().is_empty() {
        first_cit.snippet.trim()
    } else {
        outcome.sanitized_answer.trim()
    };
    GroundedDocSynthesis {
        answer: format!("根据《{}》[文1]：{body}", first_cit.doc_title),
        grounding_mode: GroundingMode::GroundedDoc,
        verified_citations: 1,
        abstained: false,
    }
}

/// Sanitizes `[文N]` citations and verifies numeric literals against `data_rows` + cited snippets
/// for `HYBRID` queries (`02-STEP16` §16.4.2).
#[must_use]
pub fn sanitize_and_ground_hybrid_answer(
    inp: &HybridGroundingInput<'_>,
) -> GroundedHybridSynthesis {
    if inp.data_abstained && inp.doc_abstained {
        return GroundedHybridSynthesis {
            answer: String::from(ABSTAIN_NO_DATA_OR_DOC_ZH),
            grounding_mode: GroundingMode::Abstained,
            verified_numbers: 0,
            verified_citations: 0,
            abstained: true,
        };
    }

    let fallback = build_deterministic_hybrid_fallback(inp);
    let Some(raw) = inp.raw_hybrid.map(str::trim).filter(|s| !s.is_empty()) else {
        return finalize_hybrid_fallback(&fallback, inp);
    };
    if inp.data_rows.is_empty() || inp.doc_citations.is_empty() {
        return finalize_hybrid_fallback(&fallback, inp);
    }

    let cit_out = verify_and_sanitize_citations(raw, inp.doc_citations.len());
    let cited_snippets: Vec<String> = if cit_out.verified_indices.is_empty() {
        inp.doc_citations
            .iter()
            .map(|c| c.snippet.clone())
            .collect()
    } else {
        cit_out
            .verified_indices
            .iter()
            .filter_map(|&idx| inp.doc_citations.get(idx.saturating_sub(1)))
            .map(|c| c.snippet.clone())
            .collect()
    };

    if !verify_hybrid_numbers_grounded(&cit_out.sanitized_answer, inp.data_rows, &cited_snippets) {
        return finalize_hybrid_fallback(&fallback, inp);
    }

    let mut verified_citations = cit_out.verified_count();
    let mut final_text = if cit_out.sanitized_answer.contains("[表]") {
        cit_out.sanitized_answer
    } else {
        format!("[表] {}", cit_out.sanitized_answer)
    };
    if verified_citations == 0 && !inp.doc_citations.is_empty() {
        final_text.push_str(" [文1]");
        verified_citations = 1;
    }
    let verified_numbers = count_grounded_numbers(&final_text);
    GroundedHybridSynthesis {
        answer: final_text,
        grounding_mode: inp.data_grounding_mode,
        verified_numbers,
        verified_citations,
        abstained: false,
    }
}

fn build_deterministic_hybrid_fallback(inp: &HybridGroundingInput<'_>) -> String {
    if inp.data_abstained {
        return inp.doc_answer.to_owned();
    }
    if inp.doc_abstained || inp.doc_citations.is_empty() {
        return format!("[表] {}", inp.data_answer);
    }
    let clean_doc = strip_citation_and_table_markers(inp.doc_answer);
    format!("[表] {}\n\n[文1] {}", inp.data_answer, clean_doc.trim())
}

fn finalize_hybrid_fallback(
    fallback: &str,
    inp: &HybridGroundingInput<'_>,
) -> GroundedHybridSynthesis {
    let cit_out = verify_and_sanitize_citations(fallback, inp.doc_citations.len());
    let mode = if inp.data_abstained {
        GroundingMode::GroundedDoc
    } else {
        inp.data_grounding_mode
    };
    GroundedHybridSynthesis {
        verified_numbers: count_grounded_numbers(&cit_out.sanitized_answer),
        verified_citations: cit_out.verified_count(),
        answer: cit_out.sanitized_answer,
        grounding_mode: mode,
        abstained: false,
    }
}

/// Executes both `CLOUD` and `LOCAL_7B` tracks of the built-in 50-case anti-hallucination benchmark (`02-STEP16` §16.5).
///
/// # Errors
/// Returns [`AppError`] if the embedded fixture, cassette JSONL, or in-memory `DuckDB` initialization fails.
pub async fn run_grounding_eval_suite() -> Result<[NewGroundingEvalRunInput; 2], AppError> {
    let cloud = run_grounding_eval_track(GroundingEvalTrack::Cloud).await?;
    let local = run_grounding_eval_track(GroundingEvalTrack::Local7b).await?;
    Ok([cloud, local])
}

/// Executes a single track (`CLOUD` or `LOCAL_7B`) of the 50-case anti-hallucination suite (`02-STEP16` §16.5).
///
/// # Errors
/// Returns [`AppError`] if fixture parsing, cassette loading, or `DuckDB` execution fails.
pub async fn run_grounding_eval_track(
    track: GroundingEvalTrack,
) -> Result<NewGroundingEvalRunInput, AppError> {
    let started = Instant::now();
    let suite: EvalSuiteSpec = serde_json::from_str(SUITE_FIXTURE_JSON)
        .map_err(|e| AppError::Config(format!("invalid hallucination_50.json: {e}")))?;
    let rules = load_track_cassette_rules(track)?;
    let env_fx = build_eval_environment()?;

    let mut cat_totals: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut intercepted = 0_u32;

    for case in &suite.cases {
        let entry = cat_totals.entry(case.category.clone()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);

        match evaluate_single_case(track, &rules, &env_fx, case).await {
            Ok(()) => {
                intercepted = intercepted.saturating_add(1);
                entry.1 = entry.1.saturating_add(1);
            }
            Err(reason) => {
                failures.push(serde_json::json!({
                    "case_id": case.id,
                    "category": case.category,
                    "reason": reason,
                }));
            }
        }
    }

    let total_cases = u32::try_from(suite.cases.len()).unwrap_or(50);
    let pass_rate = if total_cases == 0 {
        0.0
    } else {
        f64::from(intercepted) / f64::from(total_cases)
    };
    let breakdown = build_category_breakdown_json(&cat_totals);
    let model_name = match track {
        GroundingEvalTrack::Cloud => String::from("cassette-cloud-frontier"),
        GroundingEvalTrack::Local7b => String::from("cassette-local-qwen2.5-7b"),
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(NewGroundingEvalRunInput {
        suite_name: suite.suite_name,
        track,
        model_name,
        total_cases,
        intercepted_cases: intercepted,
        pass_rate,
        category_breakdown: breakdown,
        failures: serde_json::Value::Array(failures),
        duration_ms,
    })
}

fn build_category_breakdown_json(cat_totals: &BTreeMap<String, (u32, u32)>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (cat, &(total, passed)) in cat_totals {
        let rate = if total == 0 {
            0.0
        } else {
            f64::from(passed) / f64::from(total)
        };
        map.insert(
            cat.clone(),
            serde_json::json!({
                "total": total,
                "intercepted": passed,
                "pass_rate": rate,
            }),
        );
    }
    serde_json::Value::Object(map)
}

#[derive(Debug, Deserialize)]
struct EvalSuiteSpec {
    suite_name: String,
    cases: Vec<EvalCaseSpec>,
}

#[derive(Debug, Deserialize)]
struct EvalCaseSpec {
    id: String,
    category: String,
    mode: String,
    question: String,
    #[serde(default)]
    expected_metric: Option<ExpectedMetricSpec>,
    #[serde(default)]
    expect_join_rejected: Option<bool>,
    #[serde(default)]
    recalled_doc_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ExpectedMetricSpec {
    metrics: Vec<String>,
    #[serde(default)]
    dimensions: Vec<String>,
    #[serde(default)]
    filters: Vec<FilterSpec>,
    #[serde(default)]
    time_from: Option<String>,
    #[serde(default)]
    time_to: Option<String>,
    #[serde(default)]
    grain: Option<RollupGrain>,
    #[serde(default)]
    compare: Option<MetricCompareMode>,
}

#[derive(Debug, Clone, Deserialize)]
struct EvalCassetteRule {
    #[serde(default)]
    response_schema_name: Option<String>,
    #[serde(default)]
    match_all: Vec<String>,
    #[serde(default)]
    exclude_any: Vec<String>,
    response: String,
}

impl EvalCassetteRule {
    fn matches(&self, transcript: &str, schema_name: Option<&str>) -> bool {
        if self.response_schema_name.as_deref() != schema_name {
            return false;
        }
        !self.match_all.is_empty()
            && self
                .match_all
                .iter()
                .all(|m| transcript.contains(m.as_str()))
            && !self
                .exclude_any
                .iter()
                .any(|ex| transcript.contains(ex.as_str()))
    }
}

fn load_track_cassette_rules(track: GroundingEvalTrack) -> Result<Vec<EvalCassetteRule>, AppError> {
    let filename = match track {
        GroundingEvalTrack::Cloud => "hallucination_cloud_50.jsonl",
        GroundingEvalTrack::Local7b => "hallucination_local7b_50.jsonl",
    };
    let raw = read_cassette_file_or_embedded(filename, track);
    let mut rules = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rule: EvalCassetteRule = serde_json::from_str(trimmed).map_err(|e| {
            AppError::Config(format!(
                "invalid cassette rule in {filename}:{}: {e}",
                idx.saturating_add(1)
            ))
        })?;
        rules.push(rule);
    }
    Ok(rules)
}

fn read_cassette_file_or_embedded(filename: &str, track: GroundingEvalTrack) -> String {
    if let Ok(env_path) = env::var("AXPOINT_LLM_CASSETTE") {
        let p = Path::new(&env_path);
        let candidate = if p.is_dir() {
            p.join(filename)
        } else {
            p.parent().map_or_else(
                || Path::new("tests/cassettes").join(filename),
                |d| d.join(filename),
            )
        };
        if let Ok(text) = fs::read_to_string(&candidate) {
            return text;
        }
    }
    for rel in [
        format!("tests/cassettes/{filename}"),
        format!("../tests/cassettes/{filename}"),
        format!("../../../tests/cassettes/{filename}"),
    ] {
        if let Ok(text) = fs::read_to_string(&rel) {
            return text;
        }
    }
    match track {
        GroundingEvalTrack::Cloud => String::from(CLOUD_CASSETTE_FALLBACK),
        GroundingEvalTrack::Local7b => String::from(LOCAL7B_CASSETTE_FALLBACK),
    }
}

#[derive(Default)]
struct CallTelemetry {
    slot: usize,
    stage3_sql: usize,
    summary: usize,
    doc_synthesis: usize,
}

struct EvalTrackAi<'a> {
    track: GroundingEvalTrack,
    case_id: &'a str,
    rules: &'a [EvalCassetteRule],
    telemetry: Arc<Mutex<CallTelemetry>>,
}

impl EvalTrackAi<'_> {
    fn lookup(&self, sys: &str, usr: &str, schema_name: Option<&str>) -> Result<String, AppError> {
        let transcript = format!(
            "[EVAL_TRACK:{}][EVAL_CASE:{}]\n{sys}\n---\n{usr}",
            self.track.as_str(),
            self.case_id
        );
        for rule in self.rules {
            if rule.matches(&transcript, schema_name) {
                return Ok(rule.response.clone());
            }
        }
        Err(AppError::Llm(format!(
            "eval cassette miss for {} {} (schema={schema_name:?})",
            self.track.as_str(),
            self.case_id
        )))
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        let g = self
            .telemetry
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (g.slot, g.stage3_sql, g.summary, g.doc_synthesis)
    }
}

impl AiPort for EvalTrackAi<'_> {
    fn embedding_model_name(&self) -> &'static str {
        "eval-deterministic"
    }

    fn current_embedding_dim(&self) -> Option<usize> {
        Some(64)
    }

    fn embed_batch(
        &self,
        inputs: &[String],
    ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
        ready(Ok((vec![vec![0.0_f32; 64]; inputs.len()], 64)))
    }

    fn embed_one(
        &self,
        _text: &str,
    ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
        ready(Ok((vec![0.0_f32; 64], 64)))
    }

    fn extract_triples(
        &self,
        _paragraph: &str,
    ) -> impl Future<Output = Result<Vec<ExtractedTriple>, AppError>> + Send {
        ready(Ok(Vec::new()))
    }

    fn extract_query_entities(
        &self,
        _query: &str,
    ) -> impl Future<Output = Result<Vec<String>, AppError>> + Send {
        ready(Ok(Vec::new()))
    }

    fn synthesize_answer(
        &self,
        query: &str,
        hits: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        {
            let mut g = self
                .telemetry
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            g.doc_synthesis = g.doc_synthesis.saturating_add(1);
        }
        let mut ctx = String::new();
        for (i, h) in hits.iter().enumerate() {
            let _fmt_ok = writeln!(
                ctx,
                "[{}] {}: {}",
                i.saturating_add(1),
                h.doc_title,
                h.content
            );
        }
        let usr = format!("问题：{query}\n\n检索证据：\n{ctx}");
        ready(self.lookup("RAG_SYNTH", &usr, None))
    }

    fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        {
            let mut g = self
                .telemetry
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if system_prompt.contains("你是严谨的数据分析助手") {
                g.summary = g.summary.saturating_add(1);
            } else if system_prompt.contains("你是 DuckDB SQL 专家") {
                g.stage3_sql = g.stage3_sql.saturating_add(1);
            }
        }
        ready(self.lookup(system_prompt, user_prompt, None))
    }

    fn chat_structured(
        &self,
        _purpose: Purpose,
        system_prompt: &str,
        user_prompt: &str,
        schema: &JsonSchemaSpec,
    ) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send {
        {
            let mut g = self
                .telemetry
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            g.slot = g.slot.saturating_add(1);
        }
        let is_local = self.track == GroundingEvalTrack::Local7b;
        let res = self
            .lookup(system_prompt, user_prompt, Some(&schema.name))
            .and_then(|raw| parse_and_validate_json(&raw, schema.effective_schema(is_local)));
        ready(res)
    }
}

struct EvalEnvironment {
    conn: duckdb::Connection,
    catalog: TenantCatalog,
    metrics: Vec<MetricDef>,
    schema_doc: SchemaDoc,
    corpus: Vec<(Uuid, Uuid, String, String)>,
}

async fn evaluate_single_case(
    track: GroundingEvalTrack,
    rules: &[EvalCassetteRule],
    env_fx: &EvalEnvironment,
    case: &EvalCaseSpec,
) -> Result<(), String> {
    let ai = EvalTrackAi {
        track,
        case_id: &case.id,
        rules,
        telemetry: Arc::new(Mutex::new(CallTelemetry::default())),
    };

    match case.category.as_str() {
        "CAT1_METRIC_SLOT" => check_cat1_metric_slot(&ai, env_fx, case).await,
        "CAT2_JOIN_GUARD" => check_cat2_join_guard(&ai, env_fx, case).await,
        "CAT3_NUMERIC_VERIFY" => check_cat3_numeric_verify(&ai, env_fx, case).await,
        "CAT4_CITATION_GUARD" => check_cat4_citation_guard(&ai, env_fx, case).await,
        "CAT5_ZERO_EVIDENCE" => check_cat5_zero_evidence(&ai, env_fx, case).await,
        other => Err(format!("unknown category '{other}'")),
    }
}

async fn run_eval_data_turn(
    ai: &EvalTrackAi<'_>,
    env_fx: &EvalEnvironment,
    question: &str,
) -> Result<crate::nl2sql::AskPipelineOutput, String> {
    let req = AskRequest {
        question: String::from(question),
        session_id: None,
        context: None,
    };
    let extras = SchemaLinkExtras {
        all_metrics: env_fx.metrics.clone(),
        published_schemas: vec![env_fx.schema_doc.clone()],
        enforce_ontology_joins: true,
        ..SchemaLinkExtras::default()
    };
    let recall = SchemaRecallBundle::default();
    run_ask_pipeline_conn(
        ai,
        &env_fx.conn,
        &AskPipelineInput {
            request: &req,
            catalog: &env_fx.catalog,
            db_recall: &recall,
            link_extras: &extras,
            few_shots: &[],
            session_turns: &[],
            timezone: "UTC",
            now_rfc3339_override: Some("2026-03-01T00:00:00Z"),
            timeout_ms: None,
            force_intent: Some(AskIntent::Data),
        },
    )
    .await
    .map_err(|e| format!("pipeline error: {e}"))
}

async fn check_cat1_metric_slot(
    ai: &EvalTrackAi<'_>,
    env_fx: &EvalEnvironment,
    case: &EvalCaseSpec,
) -> Result<(), String> {
    let out = run_eval_data_turn(ai, env_fx, &case.question).await?;
    if out.response.grounding_mode != GroundingMode::SemanticMetric {
        return Err(format!(
            "expected SEMANTIC_METRIC, got {:?}",
            out.response.grounding_mode
        ));
    }
    if out.response.abstained {
        return Err(String::from("unexpected abstained=true on metric slot"));
    }
    let (_, stage3_calls, _, _) = ai.counts();
    if stage3_calls != 0 {
        return Err(format!(
            "expected 0 free-form SQL calls, got {stage3_calls}"
        ));
    }
    let Some(ref spec) = case.expected_metric else {
        return Err(String::from("missing expected_metric in fixture"));
    };
    let expected_rows = compute_baseline_semantic_rows(env_fx, spec)?;
    if out.response.rows != expected_rows {
        return Err(format!(
            "slot rows {:?} != baseline semantic rows {:?}",
            out.response.rows, expected_rows
        ));
    }
    Ok(())
}

fn compute_baseline_semantic_rows(
    env_fx: &EvalEnvironment,
    spec: &ExpectedMetricSpec,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
    let time_range = match (&spec.time_from, &spec.time_to) {
        (Some(f), Some(t)) => {
            let from = DateTime::parse_from_rfc3339(f)
                .map_err(|e| format!("bad time_from: {e}"))?
                .with_timezone(&Utc);
            let to = DateTime::parse_from_rfc3339(t)
                .map_err(|e| format!("bad time_to: {e}"))?
                .with_timezone(&Utc);
            Some(MetricTimeRange { from, to })
        }
        _ => None,
    };

    if spec.metrics.len() == 1
        && let Some(m_name) = spec.metrics.first()
    {
        let metric = env_fx
            .metrics
            .iter()
            .find(|m| &m.name == m_name)
            .ok_or_else(|| format!("metric {m_name} missing"))?;
        let res = execute_metric_query_conn(
            &env_fx.conn,
            &env_fx.catalog,
            metric,
            &MetricQuery {
                dimensions: spec.dimensions.clone(),
                filters: spec.filters.clone(),
                time_range,
                grain: spec.grain,
                compare: spec.compare,
            },
            MetricExecuteOptions::default(),
        )
        .map_err(|e| format!("baseline metric error: {e}"))?;
        return Ok(res.rows);
    }

    let res = execute_multi_metric_query_conn(
        &env_fx.conn,
        &env_fx.catalog,
        &env_fx.metrics,
        &MultiMetricQuery {
            metric_names: spec.metrics.clone(),
            dimensions: spec.dimensions.clone(),
            filters: spec.filters.clone(),
            time_range,
            grain: spec.grain,
            compare: spec.compare,
        },
        MetricExecuteOptions::default(),
    )
    .map_err(|e| format!("baseline multi-metric error: {e}"))?;
    Ok(res.rows)
}

async fn check_cat2_join_guard(
    ai: &EvalTrackAi<'_>,
    env_fx: &EvalEnvironment,
    case: &EvalCaseSpec,
) -> Result<(), String> {
    let out = run_eval_data_turn(ai, env_fx, &case.question).await?;
    let expect_rejected = case.expect_join_rejected.unwrap_or(true);
    if expect_rejected {
        if out.response.status != QueryHistoryStatus::Rejected
            || !out.response.explanation.contains("JOIN_PATH_UNGROUNDED")
            || !out.response.rows.is_empty()
        {
            return Err(format!(
                "expected JOIN_PATH_UNGROUNDED rejection with 0 rows, got status={:?} explanation={}",
                out.response.status, out.response.explanation
            ));
        }
        Ok(())
    } else {
        if out.response.status != QueryHistoryStatus::Succeeded || out.response.rows.is_empty() {
            return Err(format!(
                "expected legal mapped join to succeed with rows, got status={:?} explanation={}",
                out.response.status, out.response.explanation
            ));
        }
        Ok(())
    }
}

async fn check_cat3_numeric_verify(
    ai: &EvalTrackAi<'_>,
    env_fx: &EvalEnvironment,
    case: &EvalCaseSpec,
) -> Result<(), String> {
    let out = run_eval_data_turn(ai, env_fx, &case.question).await?;
    if out.response.status != QueryHistoryStatus::Succeeded || out.response.rows.is_empty() {
        return Err(format!(
            "expected query execution to succeed for {}",
            case.id
        ));
    }
    let expected_fb = fallback_table_summary(out.response.rows.len());
    if out.response.answer != expected_fb {
        return Err(format!(
            "hallucinated summary was not intercepted: got '{}', expected '{expected_fb}'",
            out.response.answer
        ));
    }
    let rechecked = verify_or_fallback_summary(&out.response.answer, &out.response.rows);
    if rechecked != out.response.answer || out.response.verified_numbers == 0 {
        return Err(String::from("final answer failed post-verification"));
    }
    Ok(())
}

async fn check_cat4_citation_guard(
    ai: &EvalTrackAi<'_>,
    env_fx: &EvalEnvironment,
    case: &EvalCaseSpec,
) -> Result<(), String> {
    let k = case
        .recalled_doc_count
        .unwrap_or(2)
        .clamp(1, env_fx.corpus.len());
    let selected = env_fx.corpus.get(..k).unwrap_or(&[]);
    let hits: Vec<FusedHit> = selected
        .iter()
        .map(|(pid, did, title, content)| FusedHit {
            para_id: *pid,
            doc_id: *did,
            doc_title: title.clone(),
            content: content.clone(),
            rrf_score: 0.03,
            channels: vec![RecallChannel::Bm25],
            traces: Vec::new(),
        })
        .collect();
    let citations: Vec<AskCitation> = hits
        .iter()
        .map(|h| AskCitation {
            para_id: h.para_id,
            doc_id: h.doc_id,
            doc_title: h.doc_title.clone(),
            snippet: h.content.clone(),
        })
        .collect();

    let raw = ai
        .synthesize_answer(&case.question, &hits)
        .await
        .map_err(|e| format!("doc synth error: {e}"))?;
    let grounded = sanitize_and_ground_doc_answer(&raw, &citations);
    if grounded.grounding_mode != GroundingMode::GroundedDoc
        || grounded.abstained
        || grounded.verified_citations == 0
    {
        return Err(format!("unexpected doc grounding state: {grounded:?}"));
    }
    let check = verify_and_sanitize_citations(&grounded.answer, citations.len());
    if !check.stripped_phantom_indices.is_empty() || check.verified_indices.is_empty() {
        return Err(format!(
            "phantom citations survived in '{}': {check:?}",
            grounded.answer
        ));
    }
    Ok(())
}

async fn check_cat5_zero_evidence(
    ai: &EvalTrackAi<'_>,
    env_fx: &EvalEnvironment,
    case: &EvalCaseSpec,
) -> Result<(), String> {
    if case.mode == "DATA" {
        let out = run_eval_data_turn(ai, env_fx, &case.question).await?;
        let (slot_c, sql_c, sum_c, doc_c) = ai.counts();
        if out.response.grounding_mode != GroundingMode::Abstained || !out.response.abstained {
            return Err(format!(
                "expected DATA zero-evidence ABSTAINED, got {:?}",
                out.response.grounding_mode
            ));
        }
        if slot_c + sql_c + sum_c + doc_c != 0 {
            return Err(format!(
                "expected 0 downstream synthesis calls on DATA abstain, got {}",
                slot_c + sql_c + sum_c + doc_c
            ));
        }
        return Ok(());
    }

    let mut noisy_candidates = Vec::new();
    for (idx, &(pid, did, ref title, ref content)) in env_fx.corpus.iter().enumerate() {
        let rank = u32::try_from(idx.saturating_add(1)).unwrap_or(1);
        noisy_candidates.push(ChannelCandidate {
            para_id: pid,
            doc_id: did,
            doc_title: title.clone(),
            content: content.clone(),
            channel: RecallChannel::Vector,
            rank,
            trace: Some(String::from("cosine_sim=0.2140")),
        });
        noisy_candidates.push(ChannelCandidate {
            para_id: pid,
            doc_id: did,
            doc_title: title.clone(),
            content: content.clone(),
            channel: RecallChannel::Bm25,
            rank,
            trace: Some(String::from("bm25_rank=0.6000")),
        });
    }

    let filtered = filter_noise_recall_candidates(&case.question, noisy_candidates);
    let hits = rrf_fuse(filtered, 5);
    if !hits.is_empty() {
        return Err(format!(
            "expected 0 recalled hits after noise filter for '{}', got {}",
            case.question,
            hits.len()
        ));
    }
    let grounded = sanitize_and_ground_doc_answer("", &[]);
    let (_, _, _, doc_c) = ai.counts();
    if grounded.grounding_mode != GroundingMode::Abstained || !grounded.abstained || doc_c != 0 {
        return Err(String::from(
            "expected DOC zero-evidence ABSTAINED with 0 LLM calls",
        ));
    }
    Ok(())
}

fn build_eval_environment() -> Result<EvalEnvironment, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open eval duckdb: {e}")))?;
    register_mask_udfs(&conn, "eval", None)?;
    seed_eval_tables(&conn)?;

    let orders_id = Uuid::from_u128(0xE101);
    let wh_id = Uuid::from_u128(0xE102);
    let sup_id = Uuid::from_u128(0xE103);
    let ship_id = Uuid::from_u128(0xE104);
    let camp_id = Uuid::from_u128(0xE105);

    let schema_doc = build_eval_schema_doc();
    let allowed_joins = compile_ontology_join_edges(std::slice::from_ref(&schema_doc));

    let mut catalog = TenantCatalog::new("eval")
        .with_dataset(eval_orders_entry(orders_id))
        .with_dataset(eval_warehouses_entry(wh_id))
        .with_dataset(eval_suppliers_entry(sup_id))
        .with_dataset(eval_shipments_entry(ship_id))
        .with_dataset(eval_campaigns_entry(camp_id));
    catalog.allowed_joins = allowed_joins;
    catalog.published_schemas = vec![schema_doc.clone()];

    let metrics = build_eval_metrics(orders_id);
    let corpus = vec![
        (
            Uuid::from_u128(0xD001),
            Uuid::from_u128(0xD101),
            String::from("补货审批制度"),
            String::from(
                "第一条 补货审批制度触发条件：当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程。",
            ),
        ),
        (
            Uuid::from_u128(0xD002),
            Uuid::from_u128(0xD101),
            String::from("补货审批制度"),
            String::from(
                "第二条 紧急补货单须经区域仓储主管与供应链总监双级签批后方可下发执行。华南仓安全库存定额为 300 件。",
            ),
        ),
        (
            Uuid::from_u128(0xD003),
            Uuid::from_u128(0xD102),
            String::from("供应链与准入制度"),
            String::from(
                "华东仓（W-1）负责履约订单 O-1 与 O-2。A级核心供应商晶圆一厂标准准入交期为 7 天。",
            ),
        ),
    ];

    Ok(EvalEnvironment {
        conn,
        catalog,
        metrics,
        schema_doc,
        corpus,
    })
}

fn seed_eval_tables(conn: &duckdb::Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_eval\";
         CREATE TABLE lake.\"tenant_eval\".\"demo_orders\" (
             order_id VARCHAR, warehouse_code VARCHAR, status VARCHAR, region VARCHAR,
             amount DOUBLE, qty BIGINT, updated_at TIMESTAMP
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_orders\" VALUES
             ('O-1', 'W-1', 'PAID',     '华东', 20000.00, 40, '2025-01-15 08:00:00'),
             ('O-2', 'W-1', 'PAID',     '华东', 33239.25, 65, '2026-01-10 09:00:00'),
             ('O-3', 'W-2', 'PAID',     '华南', 12000.50, 25, '2026-01-18 10:00:00'),
             ('O-4', 'W-2', 'REFUNDED', '华南',  1500.00,  3, '2026-01-22 11:00:00'),
             ('O-5', 'W-1', 'PAID',     '华东', 28000.00, 50, '2026-02-05 09:00:00'),
             ('O-6', 'W-2', 'PENDING',  '华北',  4200.00,  8, '2026-02-12 14:00:00');
         CREATE TABLE lake.\"tenant_eval\".\"demo_warehouses\" (
             warehouse_code VARCHAR, supplier_id VARCHAR, region VARCHAR, title VARCHAR, safety_stock BIGINT
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_warehouses\" VALUES
             ('W-1', 'S-1', '华东', '华东仓', 500),
             ('W-2', 'S-2', '华南', '华南仓', 300);
         CREATE TABLE lake.\"tenant_eval\".\"demo_suppliers\" (
             supplier_id VARCHAR, supplier_name VARCHAR, tier VARCHAR, lead_days BIGINT
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_suppliers\" VALUES
             ('S-1', '晶圆一厂', 'A', 7),
             ('S-2', '华南精密', 'B', 14);
         CREATE TABLE lake.\"tenant_eval\".\"demo_shipments\" (
             shipment_id VARCHAR, order_id VARCHAR, carrier VARCHAR, weight DOUBLE
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_shipments\" VALUES
             ('SH-1', 'O-2', '顺丰冷链', 120.5);
         CREATE TABLE lake.\"tenant_eval\".\"demo_campaigns\" (
             campaign_id VARCHAR, region VARCHAR, budget DOUBLE
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_campaigns\" VALUES
             ('CP-1', '华东', 50000.0);",
    )
    .map_err(|e| AppError::Database(format!("seed eval tables: {e}")))
}

fn eval_col(ds_id: Uuid, pos: i32, name: &str, dt: DataType, term: &str) -> ColumnMeta {
    ColumnMeta {
        dataset_id: ds_id,
        column_name: String::from(name),
        ordinal: pos,
        data_type: dt,
        pii_category: PiiCategory::None,
        pii_confirmed: false,
        description: String::from(term),
        business_term: String::from(term),
        unit: String::new(),
    }
}

fn pack_dataset(id: Uuid, name: &str, disp: &str, cols: Vec<ColumnMeta>) -> CatalogDatasetEntry {
    let mut map = BTreeMap::new();
    for c in cols {
        map.insert(c.column_name.to_ascii_lowercase(), c);
    }
    CatalogDatasetEntry {
        id,
        name: String::from(name),
        display_name: String::from(disp),
        lake_table: String::from(name),
        schema_version: 1,
        row_count: 6,
        columns: map,
    }
}

fn eval_orders_entry(id: Uuid) -> CatalogDatasetEntry {
    pack_dataset(
        id,
        "demo_orders",
        "销售订单表",
        vec![
            eval_col(id, 1, "order_id", DataType::String, "订单编号"),
            eval_col(id, 2, "warehouse_code", DataType::String, "仓库编码"),
            eval_col(id, 3, "status", DataType::String, "订单状态"),
            eval_col(id, 4, "region", DataType::String, "销售区域"),
            eval_col(id, 5, "amount", DataType::Float64, "订单金额"),
            eval_col(id, 6, "qty", DataType::Int64, "订单数量"),
            eval_col(id, 7, "updated_at", DataType::Timestamp, "更新时间"),
        ],
    )
}

fn eval_warehouses_entry(id: Uuid) -> CatalogDatasetEntry {
    pack_dataset(
        id,
        "demo_warehouses",
        "履约仓库表",
        vec![
            eval_col(id, 1, "warehouse_code", DataType::String, "仓库编号"),
            eval_col(id, 2, "supplier_id", DataType::String, "供应商编号"),
            eval_col(id, 3, "region", DataType::String, "所属区域"),
            eval_col(id, 4, "title", DataType::String, "仓库名称"),
            eval_col(id, 5, "safety_stock", DataType::Int64, "安全库存"),
        ],
    )
}

fn eval_suppliers_entry(id: Uuid) -> CatalogDatasetEntry {
    pack_dataset(
        id,
        "demo_suppliers",
        "上游供应商表",
        vec![
            eval_col(id, 1, "supplier_id", DataType::String, "供应商编号"),
            eval_col(id, 2, "supplier_name", DataType::String, "供应商名称"),
            eval_col(id, 3, "tier", DataType::String, "供应商等级"),
            eval_col(id, 4, "lead_days", DataType::Int64, "标准交期天数"),
        ],
    )
}

fn eval_shipments_entry(id: Uuid) -> CatalogDatasetEntry {
    pack_dataset(
        id,
        "demo_shipments",
        "物流运单表",
        vec![
            eval_col(id, 1, "shipment_id", DataType::String, "运单编号"),
            eval_col(id, 2, "order_id", DataType::String, "关联单号"),
            eval_col(id, 3, "carrier", DataType::String, "承运商"),
            eval_col(id, 4, "weight", DataType::Float64, "运单重量"),
        ],
    )
}

fn eval_campaigns_entry(id: Uuid) -> CatalogDatasetEntry {
    pack_dataset(
        id,
        "demo_campaigns",
        "营销活动预算表",
        vec![
            eval_col(id, 1, "campaign_id", DataType::String, "活动编号"),
            eval_col(id, 2, "region", DataType::String, "投放区域"),
            eval_col(id, 3, "budget", DataType::Float64, "活动预算"),
        ],
    )
}

fn build_eval_metrics(orders_id: Uuid) -> Vec<MetricDef> {
    let dims = vec![
        MetricDimension {
            name: String::from("status"),
            column: String::from("status"),
            display_name: String::from("状态"),
        },
        MetricDimension {
            name: String::from("region"),
            column: String::from("region"),
            display_name: String::from("区域"),
        },
    ];
    let now = Utc::now();
    vec![
        MetricDef {
            id: Uuid::from_u128(0xF001),
            name: String::from("paid_amount"),
            display_name: String::from("已支付金额"),
            description: String::from("状态为 PAID 的已支付订单金额汇总"),
            dataset_id: orders_id,
            expression: String::from("SUM(amount)"),
            filter_sql: String::from("status = 'PAID'"),
            time_column: Some(String::from("updated_at")),
            default_grain: RollupGrain::Month,
            dimensions: dims.clone(),
            unit: String::from("CNY"),
            format: String::from("0,0.00"),
            synonyms: vec![String::from("实收金额")],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: now,
            updated_at: now,
        },
        MetricDef {
            id: Uuid::from_u128(0xF002),
            name: String::from("order_count"),
            display_name: String::from("订单笔数"),
            description: String::from("订单总笔数"),
            dataset_id: orders_id,
            expression: String::from("COUNT(order_id)"),
            filter_sql: String::new(),
            time_column: Some(String::from("updated_at")),
            default_grain: RollupGrain::Month,
            dimensions: dims.clone(),
            unit: String::from("笔"),
            format: String::from("0,0"),
            synonyms: vec![String::from("订单量")],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: now,
            updated_at: now,
        },
        MetricDef {
            id: Uuid::from_u128(0xF003),
            name: String::from("refund_amount"),
            display_name: String::from("退款金额"),
            description: String::from("状态为 REFUNDED 的退款金额汇总"),
            dataset_id: orders_id,
            expression: String::from("SUM(amount)"),
            filter_sql: String::from("status = 'REFUNDED'"),
            time_column: Some(String::from("updated_at")),
            default_grain: RollupGrain::Month,
            dimensions: dims,
            unit: String::from("CNY"),
            format: String::from("0,0.00"),
            synonyms: vec![String::from("退单额")],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: None,
            version: 1,
            created_at: now,
            updated_at: now,
        },
    ]
}

fn build_eval_schema_doc() -> SchemaDoc {
    let str_prop = |name: &str, disp: &str| PropertySpec {
        name: String::from(name),
        display_name: String::from(disp),
        data_type: DataType::String,
        kind: PropertyKind::Attribute,
        ..PropertySpec::default()
    };
    SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                name: String::from("Order"),
                display_name: String::from("订单"),
                properties: vec![str_prop("status", "状态"), str_prop("region", "区域")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Warehouse"),
                display_name: String::from("仓库"),
                properties: vec![str_prop("region", "区域"), str_prop("title", "名称")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Supplier"),
                display_name: String::from("供应商"),
                properties: vec![str_prop("tier", "等级")],
                ..NodeTypeSpec::default()
            },
        ],
        edge_types: vec![
            EdgeTypeSpec {
                name: String::from("FULFILLS"),
                src_type_name: String::from("Warehouse"),
                dst_type_name: String::from("Order"),
                ..EdgeTypeSpec::default()
            },
            EdgeTypeSpec {
                name: String::from("SUPPLIES"),
                src_type_name: String::from("Supplier"),
                dst_type_name: String::from("Warehouse"),
                ..EdgeTypeSpec::default()
            },
        ],
        mappings: build_eval_schema_mappings(),
    }
}

fn build_eval_schema_mappings() -> Vec<MappingSpec> {
    vec![
        MappingSpec {
            name: String::from("orders_map"),
            dataset: String::from("demo_orders"),
            node_type: String::from("Order"),
            key_column: String::from("order_id"),
            title: String::from("{order_id}"),
            filter: None,
            attributes: BTreeMap::from([(String::from("status"), String::from("status"))]),
            metrics: BTreeMap::new(),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: vec![MappingLinkSpec {
                link_type: String::from("FULFILLS"),
                direction: LinkDirection::In,
                target_type: String::from("Warehouse"),
                target_key_column: String::from("warehouse_code"),
                weight_column: None,
                create_missing_target: false,
            }],
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        },
        MappingSpec {
            name: String::from("wh_map"),
            dataset: String::from("demo_warehouses"),
            node_type: String::from("Warehouse"),
            key_column: String::from("warehouse_code"),
            title: String::from("{title}"),
            filter: None,
            attributes: BTreeMap::from([(String::from("region"), String::from("region"))]),
            metrics: BTreeMap::new(),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: vec![MappingLinkSpec {
                link_type: String::from("SUPPLIES"),
                direction: LinkDirection::In,
                target_type: String::from("Supplier"),
                target_key_column: String::from("supplier_id"),
                weight_column: None,
                create_missing_target: false,
            }],
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        },
        MappingSpec {
            name: String::from("sup_map"),
            dataset: String::from("demo_suppliers"),
            node_type: String::from("Supplier"),
            key_column: String::from("supplier_id"),
            title: String::from("{supplier_name}"),
            filter: None,
            attributes: BTreeMap::from([(String::from("tier"), String::from("tier"))]),
            metrics: BTreeMap::new(),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: Vec::new(),
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_filter_drops_antimatter_query_and_retains_valid_queries() {
        let mk_vec = |title: &str, content: &str, sim: &str| ChannelCandidate {
            para_id: Uuid::from_u128(1),
            doc_id: Uuid::from_u128(2),
            doc_title: String::from(title),
            content: String::from(content),
            channel: RecallChannel::Vector,
            rank: 1,
            trace: Some(format!("cosine_sim={sim}")),
        };
        let mk_bm25 = |title: &str, content: &str| ChannelCandidate {
            para_id: Uuid::from_u128(1),
            doc_id: Uuid::from_u128(2),
            doc_title: String::from(title),
            content: String::from(content),
            channel: RecallChannel::Bm25,
            rank: 1,
            trace: Some(String::from("bm25_rank=0.6000")),
        };

        let docs = [
            (
                "供应链制度",
                "华东仓（W-1）负责履约订单 O-1 与 O-2。晶圆一厂又称 Fab-1。",
            ),
            (
                "补货审批制度",
                "第一条 补货审批制度触发条件：当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程。",
            ),
            ("系统白皮书", "xBlueNode 采用五路动态超图混合检索引擎。"),
        ];

        let mut noisy = Vec::new();
        for (t, c) in docs {
            noisy.push(mk_vec(t, c, "0.1820"));
            noisy.push(mk_bm25(t, c));
        }
        let empty = filter_noise_recall_candidates(
            "反物质曲率引擎反应堆舱门维护规程第88条是什么？",
            noisy.clone(),
        );
        assert!(empty.is_empty(), "expected empty hits, got {empty:?}");

        for valid_q in [
            "补货审批制度的具体触发条件是什么？",
            "动态超图",
            "供应链制度",
            "华东仓",
        ] {
            let kept = filter_noise_recall_candidates(valid_q, noisy.clone());
            assert!(!kept.is_empty(), "expected '{valid_q}' to retain hits");
        }
    }
}
