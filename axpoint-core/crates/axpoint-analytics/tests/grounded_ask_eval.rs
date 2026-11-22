//! Dual-track (`cloud_flagship` + `local_7b_qwen`) 50-case anti-hallucination & PII egress benchmark (`08-STEP22` §22.2).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::future::{Future, ready};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use axpoint_analytics::nl2sql::chart::{fallback_table_summary, verify_or_fallback_summary};
use axpoint_analytics::nl2sql::schema_link::{SchemaLinkExtras, compile_ontology_join_edges};
use axpoint_analytics::{
    AllowedDatasetJoinEdge, AskIntent, AskPipelineInput, AskRequest, CatalogDatasetEntry,
    GuardOptions, MetricDef, MetricDimension, ResultSerializeOptions, SchemaRecallBundle,
    TenantCatalog, batches_to_query_result, duckdb, filter_noise_recall_candidates, guard,
    guard_and_rewrite, register_mask_udfs, run_ask_pipeline_conn, sanitize_and_ground_doc_answer,
};
use axpoint_domain::analytics::AskCitation;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, GroundingMode, PiiCategory, Purpose, RollupGrain};
use axpoint_domain::grounding::{
    ABSTAIN_NO_DOC_EVIDENCE_ZH, count_grounded_numbers, should_abstain_zero_evidence,
    verify_and_sanitize_citations,
};
use axpoint_domain::json_schema::{
    JsonSchemaSpec, TenantOntologyEnums, parse_and_validate_json, strip_markdown_json_fence,
    validate_json_against_schema,
};
use axpoint_domain::ports::AiPort;
use axpoint_domain::studio::{
    EdgeTypeSpec, LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode, MappingSpec,
    NodeTypeSpec, OnSourceDelete, PropertyKind, PropertySpec, SchemaDoc,
};
use axpoint_domain::{
    AppError, ChannelCandidate, ExtractedTriple, FusedHit, RecallChannel, rrf_fuse,
};
use axpoint_security::pii_local::{
    EgressSession, analyze_pii_local, redact_for_egress, restore_from_egress,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// Machine-verified summary emitted by `grounded_ask_eval` for `step16.sh` & `step22.sh` (`08-STEP22` §22.2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundedEvalSummary {
    /// Cassette profile identifier (`"cloud_flagship"` or `"local_7b_qwen"`).
    pub track: String,
    /// Total adversarial cases executed (`50`).
    pub total_cases: usize,
    /// Cases either deterministically compiled via Stage 0 or intercepted by post-guards (`50`).
    pub intercepted_or_exact: usize,
    /// Count of cases where any unverified number or phantom citation reached the caller (`0`).
    pub hallucination_leaked: usize,
    /// Count of cases where raw PII appeared in outbound LLM payloads or final responses (`0`).
    pub pii_leaked: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct DualTrackManifest {
    track: String,
    structured_mode: String,
    temperature: f64,
    total_cases: usize,
    categories: Vec<String>,
    cassette_file: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DualTrackCase {
    id: String,
    category: String,
    question: String,
    #[serde(default)]
    sql: String,
    llm_output: String,
    #[serde(default)]
    outbound_prompt: String,
    #[serde(default)]
    raw_pii_tokens: Vec<String>,
    #[serde(default)]
    schema_name: String,
    #[serde(default)]
    zero_recall_mode: String,
}

#[derive(Default)]
struct CaseOutcome {
    intercepted: bool,
    hallucination_leaked: bool,
    pii_leaked: bool,
}

type DocCorpusItem = (Uuid, Uuid, String, String);

struct DualTrackEnv {
    conn: duckdb::Connection,
    catalog: TenantCatalog,
    allowed_joins: Vec<AllowedDatasetJoinEdge>,
    metrics: Vec<MetricDef>,
    schema_doc: SchemaDoc,
    ontology_enums: TenantOntologyEnums,
    orders_id: Uuid,
    wh_id: Uuid,
    citations_two: Vec<AskCitation>,
    corpus: Vec<DocCorpusItem>,
}

struct TrapAi<'a> {
    canned_output: &'a str,
    calls: AtomicUsize,
}

impl<'a> TrapAi<'a> {
    const fn new(canned_output: &'a str) -> Self {
        Self {
            canned_output,
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiPort for TrapAi<'_> {
    fn embedding_model_name(&self) -> &'static str {
        "eval-trap-deterministic"
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
        _query: &str,
        _hits: &[FusedHit],
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ready(Ok(String::from(self.canned_output)))
    }

    fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ready(Ok(String::from(self.canned_output)))
    }

    fn chat_structured(
        &self,
        _purpose: Purpose,
        _system_prompt: &str,
        _user_prompt: &str,
        _schema: &JsonSchemaSpec,
    ) -> impl Future<Output = Result<Value, AppError>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ready(Err(AppError::Llm(String::from(
            "unexpected structured call on zero-recall abstain",
        ))))
    }
}

#[tokio::test]
async fn dual_track_grounded_ask_eval_achieves_zero_leaks() -> Result<(), AppError> {
    let env_fx = build_dual_track_env()?;
    verify_positive_mapped_join_control(&env_fx)?;

    let tracks: Vec<String> = match env::var("AXPOINT_EVAL_TRACK") {
        Ok(val) if !val.trim().is_empty() => vec![String::from(val.trim())],
        _ => vec![
            String::from("cloud_flagship"),
            String::from("local_7b_qwen"),
        ],
    };

    for track in &tracks {
        let dir = resolve_track_cassette_dir(track)?;
        let summary = execute_track_suite(track, &dir, &env_fx).await?;
        emit_eval_summary_line(&summary)?;

        if summary.total_cases != 50
            || summary.intercepted_or_exact != 50
            || summary.hallucination_leaked != 0
            || summary.pii_leaked != 0
        {
            return Err(AppError::Config(format!(
                "dual-track benchmark failed for '{track}': {summary:?}"
            )));
        }
    }
    Ok(())
}

fn emit_eval_summary_line(summary: &GroundedEvalSummary) -> Result<(), AppError> {
    let json = serde_json::to_string(summary)
        .map_err(|e| AppError::Config(format!("serialize GroundedEvalSummary: {e}")))?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "EVAL_SUMMARY_JSON={json}")
        .map_err(|e| AppError::Config(format!("stdout write: {e}")))?;
    stdout
        .flush()
        .map_err(|e| AppError::Config(format!("stdout flush: {e}")))?;
    Ok(())
}

fn resolve_track_cassette_dir(track: &str) -> Result<PathBuf, AppError> {
    if let Ok(raw_env) = env::var("AXPOINT_LLM_CASSETTE") {
        let trimmed = raw_env.trim();
        if !trimmed.is_empty() {
            let base = PathBuf::from(trimmed);
            if base.join("manifest.json").is_file() {
                return Ok(base);
            }
            let nested = base.join("v2_dual_track").join(track);
            if nested.join("manifest.json").is_file() {
                return Ok(nested);
            }
        }
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fallback = manifest_dir
        .join("../../../tests/cassettes/v2_dual_track")
        .join(track);
    if fallback.join("manifest.json").is_file() {
        return Ok(fallback);
    }
    Err(AppError::Config(format!(
        "missing cassette manifest.json for track '{track}' (searched AXPOINT_LLM_CASSETTE and {})",
        fallback.display()
    )))
}

async fn execute_track_suite(
    expected_track: &str,
    dir: &Path,
    env_fx: &DualTrackEnv,
) -> Result<GroundedEvalSummary, AppError> {
    let manifest_raw = fs::read_to_string(dir.join("manifest.json"))
        .map_err(|e| AppError::Config(format!("read manifest.json in {}: {e}", dir.display())))?;
    let manifest: DualTrackManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| AppError::Config(format!("parse manifest.json: {e}")))?;

    if manifest.track != expected_track
        || manifest.structured_mode.is_empty()
        || manifest.temperature.abs() > f64::EPSILON
        || manifest.total_cases != 50
        || manifest.categories.len() != 5
    {
        return Err(AppError::Config(format!(
            "manifest contract mismatch for '{expected_track}': {manifest:?}"
        )));
    }

    let cases = load_jsonl_cases(&dir.join(&manifest.cassette_file))?;
    if cases.len() != manifest.total_cases {
        return Err(AppError::Config(format!(
            "expected {} cases in {}, got {}",
            manifest.total_cases,
            manifest.cassette_file,
            cases.len()
        )));
    }

    let is_local = manifest.track == "local_7b_qwen";
    let mut intercepted_or_exact = 0_usize;
    let mut hallucination_leaked = 0_usize;
    let mut pii_leaked = 0_usize;

    for case in &cases {
        let out = evaluate_case(case, is_local, env_fx).await?;
        if out.intercepted && !out.hallucination_leaked && !out.pii_leaked {
            intercepted_or_exact = intercepted_or_exact.saturating_add(1);
        }
        if out.hallucination_leaked {
            hallucination_leaked = hallucination_leaked.saturating_add(1);
        }
        if out.pii_leaked {
            pii_leaked = pii_leaked.saturating_add(1);
        }
    }

    Ok(GroundedEvalSummary {
        track: manifest.track,
        total_cases: cases.len(),
        intercepted_or_exact,
        hallucination_leaked,
        pii_leaked,
    })
}

fn load_jsonl_cases(path: &Path) -> Result<Vec<DualTrackCase>, AppError> {
    let text = fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("read {}: {e}", path.display())))?;
    let mut out = Vec::with_capacity(50);
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: DualTrackCase = serde_json::from_str(trimmed).map_err(|e| {
            AppError::Config(format!(
                "invalid case JSON at {}:{}: {e}",
                path.display(),
                idx.saturating_add(1)
            ))
        })?;
        out.push(parsed);
    }
    Ok(out)
}

async fn evaluate_case(
    case: &DualTrackCase,
    is_local: bool,
    env_fx: &DualTrackEnv,
) -> Result<CaseOutcome, AppError> {
    match case.category.as_str() {
        "G1_ARITHMETIC_DRIFT" => eval_g1_arithmetic_drift(case, env_fx),
        "G2_UNGROUNDED_JOIN" => eval_g2_ungrounded_join(case, env_fx),
        "G3_PHANTOM_CITATION" => eval_g3_phantom_citation(case, env_fx),
        "G4_ZERO_RECALL_ABSTAIN" => eval_g4_zero_recall_abstain(case, env_fx).await,
        "G5_SCHEMA_AND_PII_GUARD" => eval_g5_schema_and_pii_guard(case, is_local, env_fx),
        other => Err(AppError::Config(format!(
            "unknown adversarial category '{other}' in {}",
            case.id
        ))),
    }
}

fn eval_g1_arithmetic_drift(
    case: &DualTrackCase,
    env_fx: &DualTrackEnv,
) -> Result<CaseOutcome, AppError> {
    let opts = GuardOptions::default().with_ontology_joins(true, env_fx.allowed_joins.clone());
    let guarded = guard(&case.sql, &env_fx.catalog, &opts)?;
    let mut stmt = env_fx
        .conn
        .prepare(&guarded.sql)
        .map_err(|e| AppError::Database(format!("prepare {}: {e}", case.id)))?;
    let arrow_iter = stmt
        .query_arrow([])
        .map_err(|e| AppError::Database(format!("query_arrow {}: {e}", case.id)))?;
    let arrow_schema = arrow_iter.get_schema();
    let batches: Vec<arrow::record_batch::RecordBatch> = arrow_iter.collect();
    let ser = batches_to_query_result(
        &arrow_schema,
        &batches,
        ResultSerializeOptions {
            effective_max_rows: guarded.effective_max_rows,
            sql: case.sql.clone(),
            duration_ms: 0,
            reveal_pii: false,
            has_pii_raw_read: false,
            catalog: &env_fx.catalog,
            guarded: &guarded,
        },
    )?;

    if ser.result.rows.is_empty() {
        return Err(AppError::Config(format!(
            "G1 fixture query returned 0 rows for {}",
            case.id
        )));
    }

    let expected_fb = fallback_table_summary(ser.result.rows.len());
    let verified = verify_or_fallback_summary(&case.llm_output, &ser.result.rows);
    let rechecked = verify_or_fallback_summary(&verified, &ser.result.rows);
    let grounded_nums = count_grounded_numbers(&verified);

    let intercepted = verified == expected_fb
        && verified != case.llm_output.trim()
        && rechecked == verified
        && grounded_nums >= 1;
    Ok(CaseOutcome {
        intercepted,
        hallucination_leaked: !intercepted,
        pii_leaked: false,
    })
}

fn eval_g2_ungrounded_join(
    case: &DualTrackCase,
    env_fx: &DualTrackEnv,
) -> Result<CaseOutcome, AppError> {
    let sql_text = extract_sql_from_cassette_output(&case.llm_output)?;
    let rewrite_res = guard_and_rewrite(&sql_text, &env_fx.catalog, 100);
    let strict_opts =
        GuardOptions::default().with_ontology_joins(true, env_fx.allowed_joins.clone());
    let strict_res = guard(&sql_text, &env_fx.catalog, &strict_opts);

    let rejected_ungrounded = matches!(
        strict_res,
        Err(AppError::BadRequest {
            code: "JOIN_PATH_UNGROUNDED",
            ..
        })
    );
    let rejected_by_rewrite = rewrite_res.is_err();
    let intercepted = rejected_ungrounded || rejected_by_rewrite;

    Ok(CaseOutcome {
        intercepted,
        hallucination_leaked: !intercepted,
        pii_leaked: false,
    })
}

fn extract_sql_from_cassette_output(raw: &str) -> Result<String, AppError> {
    let unfenced = strip_markdown_json_fence(raw);
    let parsed: Value = serde_json::from_str(unfenced)
        .map_err(|e| AppError::Config(format!("invalid G2 draft JSON: {e}")))?;
    let sql = parsed
        .get("sql")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    if sql.is_empty() {
        return Err(AppError::Config(String::from("empty SQL in G2 case")));
    }
    Ok(String::from(sql))
}

fn eval_g3_phantom_citation(
    case: &DualTrackCase,
    env_fx: &DualTrackEnv,
) -> Result<CaseOutcome, AppError> {
    let recalled_len = env_fx.citations_two.len();
    let pre_check = verify_and_sanitize_citations(&case.llm_output, recalled_len);
    if pre_check.stripped_phantom_indices.is_empty() {
        return Err(AppError::Config(format!(
            "G3 case {} did not inject any phantom citation",
            case.id
        )));
    }

    let grounded = sanitize_and_ground_doc_answer(&case.llm_output, &env_fx.citations_two);
    let post_check = verify_and_sanitize_citations(&grounded.answer, recalled_len);

    let all_in_bounds = post_check
        .verified_indices
        .iter()
        .all(|&idx| (1..=recalled_len).contains(&idx));
    let intercepted = grounded.grounding_mode == GroundingMode::GroundedDoc
        && !grounded.abstained
        && grounded.verified_citations >= 1
        && post_check.stripped_phantom_indices.is_empty()
        && !post_check.verified_indices.is_empty()
        && all_in_bounds;

    Ok(CaseOutcome {
        intercepted,
        hallucination_leaked: !intercepted,
        pii_leaked: false,
    })
}

async fn eval_g4_zero_recall_abstain(
    case: &DualTrackCase,
    env_fx: &DualTrackEnv,
) -> Result<CaseOutcome, AppError> {
    if case.zero_recall_mode == "DOC" {
        let noisy = build_low_sim_noisy_candidates(&env_fx.corpus);
        let filtered = filter_noise_recall_candidates(&case.question, noisy);
        let hits = rrf_fuse(filtered, 5);
        let must_abstain = should_abstain_zero_evidence(false, false, hits.len(), false);
        let grounded = sanitize_and_ground_doc_answer(&case.llm_output, &[]);
        let intercepted = hits.is_empty()
            && must_abstain
            && grounded.grounding_mode == GroundingMode::Abstained
            && grounded.abstained
            && grounded.verified_citations == 0
            && grounded.answer == ABSTAIN_NO_DOC_EVIDENCE_ZH;
        return Ok(CaseOutcome {
            intercepted,
            hallucination_leaked: !intercepted,
            pii_leaked: false,
        });
    }

    let trap_ai = TrapAi::new(&case.llm_output);
    let req = AskRequest {
        question: case.question.clone(),
        session_id: None,
        context: None,
    };
    let is_scoped = case.zero_recall_mode == "SCOPED_DATA";
    let extras = SchemaLinkExtras {
        all_metrics: env_fx.metrics.clone(),
        published_schemas: vec![env_fx.schema_doc.clone()],
        enforce_ontology_joins: true,
        restrict_dataset_ids: if is_scoped {
            vec![env_fx.orders_id, env_fx.wh_id]
        } else {
            Vec::new()
        },
        excluded_dataset_names: if is_scoped {
            vec![
                String::from("demo_finance_ledger"),
                String::from("财务隔离总账表"),
            ]
        } else {
            Vec::new()
        },
        ..SchemaLinkExtras::default()
    };
    let recall = SchemaRecallBundle::default();
    let out = run_ask_pipeline_conn(
        &trap_ai,
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
    .await?;

    let intercepted = out.response.grounding_mode == GroundingMode::Abstained
        && out.response.abstained
        && out.response.rows.is_empty()
        && trap_ai.call_count() == 0;

    Ok(CaseOutcome {
        intercepted,
        hallucination_leaked: !intercepted,
        pii_leaked: false,
    })
}

fn build_low_sim_noisy_candidates(
    corpus: &[(Uuid, Uuid, String, String)],
) -> Vec<ChannelCandidate> {
    let mut out = Vec::with_capacity(corpus.len().saturating_mul(2));
    for (idx, &(pid, did, ref title, ref content)) in corpus.iter().enumerate() {
        let rank = u32::try_from(idx.saturating_add(1)).unwrap_or(1);
        out.push(ChannelCandidate {
            para_id: pid,
            doc_id: did,
            doc_title: title.clone(),
            content: content.clone(),
            channel: RecallChannel::Vector,
            rank,
            trace: Some(String::from("cosine_sim=0.2140")),
        });
        out.push(ChannelCandidate {
            para_id: pid,
            doc_id: did,
            doc_title: title.clone(),
            content: content.clone(),
            channel: RecallChannel::Bm25,
            rank,
            trace: Some(String::from("bm25_rank=0.5800")),
        });
    }
    out
}

fn eval_g5_schema_and_pii_guard(
    case: &DualTrackCase,
    is_local: bool,
    env_fx: &DualTrackEnv,
) -> Result<CaseOutcome, AppError> {
    if case.raw_pii_tokens.is_empty() {
        return Err(AppError::Config(format!(
            "G5 case {} missing raw_pii_tokens",
            case.id
        )));
    }
    let pre_spans = analyze_pii_local(&case.outbound_prompt);
    if pre_spans.is_empty() {
        return Err(AppError::Config(format!(
            "G5 case {} outbound_prompt had 0 detected PII spans",
            case.id
        )));
    }

    let (outbound, session) = redact_for_egress(&case.outbound_prompt);
    let outbound_has_pii = !analyze_pii_local(&outbound).is_empty()
        || case
            .raw_pii_tokens
            .iter()
            .any(|raw_tok| outbound.contains(raw_tok.as_str()));

    let spec = build_target_json_schema(&case.schema_name, &env_fx.ontology_enums)?;
    let target_schema = spec.effective_schema(is_local);

    let mut validated = validate_repair_or_fallback_json(
        &case.llm_output,
        &case.schema_name,
        is_local,
        target_schema,
    )?;
    let hmac_key = [0x5A_u8; 32];
    scrub_json_pii_leaves(&mut validated, &session, &hmac_key);

    let schema_errors = validate_json_against_schema(&validated, target_schema);
    let final_serialized = serde_json::to_string(&validated)
        .map_err(|e| AppError::Config(format!("serialize G5 output: {e}")))?;

    let response_has_pii = !analyze_pii_local(&final_serialized).is_empty()
        || case
            .raw_pii_tokens
            .iter()
            .any(|raw_tok| final_serialized.contains(raw_tok.as_str()));

    let pii_leaked = outbound_has_pii || response_has_pii;
    let schema_valid = schema_errors.is_empty();
    Ok(CaseOutcome {
        intercepted: schema_valid && !pii_leaked,
        hallucination_leaked: !schema_valid,
        pii_leaked,
    })
}

fn build_target_json_schema(
    schema_name: &str,
    enums: &TenantOntologyEnums,
) -> Result<JsonSchemaSpec, AppError> {
    match schema_name {
        "axpoint_nl2sql_draft" => Ok(JsonSchemaSpec::nl2sql_draft()),
        "axpoint_ontology_slots" => Ok(JsonSchemaSpec::ontology_slot_extraction(enums)),
        other => Err(AppError::Config(format!(
            "unsupported G5 schema_name '{other}'"
        ))),
    }
}

fn validate_repair_or_fallback_json(
    raw: &str,
    schema_name: &str,
    is_local: bool,
    target_schema: &Value,
) -> Result<Value, AppError> {
    if let Ok(direct) = parse_and_validate_json(raw, target_schema) {
        return Ok(direct);
    }

    if let Some(obj_slice) = extract_outer_json_object(raw) {
        let cleaned_commas = strip_trailing_json_commas(&obj_slice);
        if let Ok(mut candidate) = serde_json::from_str::<Value>(&cleaned_commas) {
            prune_undeclared_object_keys(&mut candidate, target_schema);
            if let Ok(re_ser) = serde_json::to_string(&candidate)
                && let Ok(repaired) = parse_and_validate_json(&re_ser, target_schema)
            {
                return Ok(repaired);
            }
        }
    }

    let fallback_raw = deterministic_schema_fallback(schema_name, is_local)?;
    parse_and_validate_json(&fallback_raw, target_schema)
}

fn extract_outer_json_object(raw: &str) -> Option<String> {
    let unfenced = strip_markdown_json_fence(raw);
    let start = unfenced.find('{')?;
    let end = unfenced.rfind('}')?;
    (end >= start).then(|| String::from(unfenced.get(start..=end).unwrap_or("")))
}

fn strip_trailing_json_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            idx = idx.saturating_add(1);
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            idx = idx.saturating_add(1);
            continue;
        }
        if ch == ',' {
            let mut look = idx.saturating_add(1);
            while chars.get(look).is_some_and(char::is_ascii_whitespace) {
                look = look.saturating_add(1);
            }
            if chars.get(look).is_some_and(|c| *c == '}' || *c == ']') {
                idx = idx.saturating_add(1);
                continue;
            }
        }
        out.push(ch);
        idx = idx.saturating_add(1);
    }
    out
}

fn prune_undeclared_object_keys(instance: &mut Value, schema: &Value) {
    let (Some(inst_obj), Some(schema_obj)) = (instance.as_object_mut(), schema.as_object()) else {
        return;
    };
    let Some(props) = schema_obj.get("properties").and_then(Value::as_object) else {
        return;
    };
    if schema_obj.get("additionalProperties") == Some(&Value::Bool(false)) {
        inst_obj.retain(|k, _| props.contains_key(k));
    }
    for (k, prop_schema) in props {
        if let Some(child) = inst_obj.get_mut(k) {
            prune_undeclared_object_keys(child, prop_schema);
        }
    }
}

fn deterministic_schema_fallback(schema_name: &str, is_local: bool) -> Result<String, AppError> {
    let val = match (schema_name, is_local) {
        ("axpoint_nl2sql_draft", _) => json!({
            "sql": "",
            "explanation": "结构化输出未通过 Schema 校验，已转入确定性澄清分支。"
        }),
        ("axpoint_ontology_slots", true) => json!({
            "route": "CLARIFY",
            "metric_names": [],
            "target_node_type": "",
            "dimensions": [],
            "filter_exprs": [],
            "time_from": "",
            "time_to": "",
            "grain": "",
            "compare": "",
            "include_links": [],
            "clarification": "请明确需要查询的认证指标名称或业务节点范围。"
        }),
        ("axpoint_ontology_slots", false) => json!({
            "route": "CLARIFY",
            "metric_names": [],
            "target_node_type": null,
            "dimensions": [],
            "filters": [],
            "time_from": null,
            "time_to": null,
            "grain": null,
            "compare": null,
            "include_links": [],
            "clarification": "请明确需要查询的认证指标名称或业务节点范围。"
        }),
        (other, _) => {
            return Err(AppError::Config(format!(
                "unknown fallback schema '{other}'"
            )));
        }
    };
    serde_json::to_string(&val)
        .map_err(|e| AppError::Config(format!("serialize fallback schema: {e}")))
}

fn scrub_json_pii_leaves(val: &mut Value, session: &EgressSession, hmac_key: &[u8; 32]) {
    match val {
        Value::String(s) => {
            let restored_masked = session.restore_text(s, false, hmac_key);
            let (re_outbound, echo_session) = redact_for_egress(&restored_masked);
            *s = restore_from_egress(&re_outbound, &echo_session, false, hmac_key);
        }
        Value::Array(items) => {
            for item in items {
                scrub_json_pii_leaves(item, session, hmac_key);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                scrub_json_pii_leaves(child, session, hmac_key);
            }
        }
        _ => {}
    }
}

fn verify_positive_mapped_join_control(env_fx: &DualTrackEnv) -> Result<(), AppError> {
    let legal_sql = "SELECT o.order_id, w.title, s.supplier_name \
                     FROM demo_orders o \
                     JOIN demo_warehouses w ON o.warehouse_code = w.warehouse_code \
                     JOIN demo_suppliers s ON w.supplier_id = s.supplier_id \
                     ORDER BY o.order_id";
    let opts = GuardOptions::default().with_ontology_joins(true, env_fx.allowed_joins.clone());
    let guarded = guard(legal_sql, &env_fx.catalog, &opts)?;
    if guarded.referenced_datasets.len() != 3 {
        return Err(AppError::Config(String::from(
            "expected legal 3-table ontology join control to reference 3 datasets",
        )));
    }
    Ok(())
}

fn build_dual_track_env() -> Result<DualTrackEnv, AppError> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| AppError::Database(format!("open in-memory duckdb: {e}")))?;
    register_mask_udfs(&conn, "eval", None)?;
    seed_dual_track_tables(&conn)?;

    let orders_id = Uuid::from_u128(0x2201);
    let wh_id = Uuid::from_u128(0x2202);
    let sup_id = Uuid::from_u128(0x2203);
    let ship_id = Uuid::from_u128(0x2204);
    let camp_id = Uuid::from_u128(0x2205);
    let fin_id = Uuid::from_u128(0x2206);

    let schema_doc = build_dual_track_schema_doc();
    let allowed_joins = compile_ontology_join_edges(std::slice::from_ref(&schema_doc));

    let catalog = TenantCatalog::new("eval")
        .with_dataset(orders_entry(orders_id))
        .with_dataset(warehouses_entry(wh_id))
        .with_dataset(suppliers_entry(sup_id))
        .with_dataset(shipments_entry(ship_id))
        .with_dataset(campaigns_entry(camp_id))
        .with_dataset(finance_entry(fin_id))
        .with_allowed_joins(allowed_joins.clone())
        .with_published_schemas(vec![schema_doc.clone()]);

    let metrics = build_dual_track_metrics(orders_id);
    let ontology_enums = build_ontology_enums();
    let (citations_two, corpus) = build_doc_fixtures();

    Ok(DualTrackEnv {
        conn,
        catalog,
        allowed_joins,
        metrics,
        schema_doc,
        ontology_enums,
        orders_id,
        wh_id,
        citations_two,
        corpus,
    })
}

fn seed_dual_track_tables(conn: &duckdb::Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE SCHEMA lake.\"tenant_eval\";
         CREATE TABLE lake.\"tenant_eval\".\"demo_orders\" (
             order_id VARCHAR, warehouse_code VARCHAR, status VARCHAR, region VARCHAR,
             amount DOUBLE, qty BIGINT, updated_at TIMESTAMP
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_orders\" VALUES
             ('O-1', 'W-1', 'PAID',     '华东',  1284.50, 40, '2025-01-15 08:00:00'),
             ('O-2', 'W-1', 'PAID',     '华东', 33239.25, 65, '2026-01-10 09:00:00'),
             ('O-3', 'W-2', 'PAID',     '华南', 12000.50, 25, '2026-01-18 10:00:00'),
             ('O-4', 'W-2', 'REFUNDED', '华南',  1500.75,  3, '2026-01-22 11:00:00'),
             ('O-5', 'W-1', 'PAID',     '华东', 28410.80, 50, '2026-02-05 09:00:00'),
             ('O-6', 'W-2', 'PENDING',  '华北',  4215.60,  8, '2026-02-12 14:00:00');
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
             ('SH-1', 'O-2', '顺丰快运', 120.5);
         CREATE TABLE lake.\"tenant_eval\".\"demo_campaigns\" (
             campaign_id VARCHAR, region VARCHAR, budget DOUBLE
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_campaigns\" VALUES
             ('CP-1', '华东', 50000.0);
         CREATE TABLE lake.\"tenant_eval\".\"demo_finance_ledger\" (
             ledger_id VARCHAR, region VARCHAR, ledger_amount DOUBLE
         );
         INSERT INTO lake.\"tenant_eval\".\"demo_finance_ledger\" VALUES
             ('FL-1', '华东', 980000.0);",
    )
    .map_err(|e| AppError::Database(format!("seed dual track tables: {e}")))
}

fn col_meta(ds_id: Uuid, pos: i32, name: &str, dt: DataType, term: &str) -> ColumnMeta {
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

fn make_ds_entry(id: Uuid, name: &str, disp: &str, cols: Vec<ColumnMeta>) -> CatalogDatasetEntry {
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

fn orders_entry(id: Uuid) -> CatalogDatasetEntry {
    make_ds_entry(
        id,
        "demo_orders",
        "销售订单表",
        vec![
            col_meta(id, 1, "order_id", DataType::String, "订单编号"),
            col_meta(id, 2, "warehouse_code", DataType::String, "仓库编码"),
            col_meta(id, 3, "status", DataType::String, "订单状态"),
            col_meta(id, 4, "region", DataType::String, "销售区域"),
            col_meta(id, 5, "amount", DataType::Float64, "订单金额"),
            col_meta(id, 6, "qty", DataType::Int64, "订单数量"),
            col_meta(id, 7, "updated_at", DataType::Timestamp, "更新时间"),
        ],
    )
}

fn warehouses_entry(id: Uuid) -> CatalogDatasetEntry {
    make_ds_entry(
        id,
        "demo_warehouses",
        "履约仓库表",
        vec![
            col_meta(id, 1, "warehouse_code", DataType::String, "仓库编号"),
            col_meta(id, 2, "supplier_id", DataType::String, "供应商编号"),
            col_meta(id, 3, "region", DataType::String, "所属区域"),
            col_meta(id, 4, "title", DataType::String, "仓库名称"),
            col_meta(id, 5, "safety_stock", DataType::Int64, "安全库存"),
        ],
    )
}

fn suppliers_entry(id: Uuid) -> CatalogDatasetEntry {
    make_ds_entry(
        id,
        "demo_suppliers",
        "上游供应商表",
        vec![
            col_meta(id, 1, "supplier_id", DataType::String, "供应商编号"),
            col_meta(id, 2, "supplier_name", DataType::String, "供应商名称"),
            col_meta(id, 3, "tier", DataType::String, "供应商等级"),
            col_meta(id, 4, "lead_days", DataType::Int64, "标准交期天数"),
        ],
    )
}

fn shipments_entry(id: Uuid) -> CatalogDatasetEntry {
    make_ds_entry(
        id,
        "demo_shipments",
        "物流运单表",
        vec![
            col_meta(id, 1, "shipment_id", DataType::String, "运单编号"),
            col_meta(id, 2, "order_id", DataType::String, "关联单号"),
            col_meta(id, 3, "carrier", DataType::String, "承运商"),
            col_meta(id, 4, "weight", DataType::Float64, "运单重量"),
        ],
    )
}

fn campaigns_entry(id: Uuid) -> CatalogDatasetEntry {
    make_ds_entry(
        id,
        "demo_campaigns",
        "营销活动预算表",
        vec![
            col_meta(id, 1, "campaign_id", DataType::String, "活动编号"),
            col_meta(id, 2, "region", DataType::String, "投放区域"),
            col_meta(id, 3, "budget", DataType::Float64, "活动预算"),
        ],
    )
}

fn finance_entry(id: Uuid) -> CatalogDatasetEntry {
    make_ds_entry(
        id,
        "demo_finance_ledger",
        "财务隔离总账表",
        vec![
            col_meta(id, 1, "ledger_id", DataType::String, "总账编号"),
            col_meta(id, 2, "region", DataType::String, "核算区域"),
            col_meta(id, 3, "ledger_amount", DataType::Float64, "账面余额"),
        ],
    )
}

fn build_dual_track_schema_doc() -> SchemaDoc {
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
        mappings: build_dual_track_mappings(),
    }
}

fn build_dual_track_mappings() -> Vec<MappingSpec> {
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

fn build_dual_track_metrics(orders_id: Uuid) -> Vec<MetricDef> {
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
            id: Uuid::from_u128(0x2301),
            name: String::from("paid_amount"),
            display_name: String::from("已支付金额"),
            description: String::from("已支付订单金额汇总"),
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
            id: Uuid::from_u128(0x2302),
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
            id: Uuid::from_u128(0x2303),
            name: String::from("refund_amount"),
            display_name: String::from("退款金额"),
            description: String::from("已退款订单金额汇总"),
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

fn build_ontology_enums() -> TenantOntologyEnums {
    let dims = vec![String::from("status"), String::from("region")];
    let mut metric_dimensions = BTreeMap::new();
    metric_dimensions.insert(String::from("paid_amount"), dims.clone());
    metric_dimensions.insert(String::from("order_count"), dims.clone());
    metric_dimensions.insert(String::from("refund_amount"), dims);
    TenantOntologyEnums {
        node_types: vec![
            String::from("Order"),
            String::from("Warehouse"),
            String::from("Supplier"),
        ],
        edge_types: vec![String::from("FULFILLS"), String::from("SUPPLIES")],
        metric_names: vec![
            String::from("paid_amount"),
            String::from("order_count"),
            String::from("refund_amount"),
        ],
        metric_dimensions,
        dataset_names: vec![
            String::from("demo_orders"),
            String::from("demo_warehouses"),
            String::from("demo_suppliers"),
        ],
    }
}

fn build_doc_fixtures() -> (Vec<AskCitation>, Vec<DocCorpusItem>) {
    let p1 = (
        Uuid::from_u128(0xD201),
        Uuid::from_u128(0xD301),
        String::from("补货审批制度"),
        String::from(
            "第一条 当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程。",
        ),
    );
    let p2 = (
        Uuid::from_u128(0xD202),
        Uuid::from_u128(0xD301),
        String::from("补货审批制度"),
        String::from(
            "第二条 紧急补货单须经区域仓储主管与供应链总监双级签批后方可下发执行；华南仓安全库存定额为 300 件。",
        ),
    );
    let citations_two = vec![
        AskCitation {
            para_id: p1.0,
            doc_id: p1.1,
            doc_title: p1.2.clone(),
            snippet: p1.3.clone(),
        },
        AskCitation {
            para_id: p2.0,
            doc_id: p2.1,
            doc_title: p2.2.clone(),
            snippet: p2.3.clone(),
        },
    ];
    let corpus = vec![
        p1,
        p2,
        (
            Uuid::from_u128(0xD203),
            Uuid::from_u128(0xD302),
            String::from("供应链准入制度"),
            String::from(
                "华东仓（W-1）负责履约订单 O-1 与 O-2。A级核心供应商晶圆一厂标准准入交期为 7 天。",
            ),
        ),
    ];
    (citations_two, corpus)
}
