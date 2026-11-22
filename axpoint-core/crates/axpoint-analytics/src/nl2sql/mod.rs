//! Controlled NL2SQL 7-stage pipeline: intent routing, hybrid schema linking, Stage 0 ontology
//! slot routing, prompt building, SQL guard + `EXPLAIN` dry-run, 2-round self-repair, masked
//! execution, and grounded summary (`05-STEP04` §04.6, `02-STEP16` §16.2–§16.4).

pub mod chart;
pub mod intent;
pub mod ontology_route;
pub mod prompt;
pub mod schema_link;

use std::collections::BTreeMap;
use std::time::Instant;

use arrow::array::RecordBatch;
use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    AskIntent, AskRequest, AskResponse, AskTurnRecord, FewShotTurn, QueryHistoryStatus,
    QueryResult, SchemaRecallBundle,
};
use axpoint_domain::enums::GroundingMode;
use axpoint_domain::grounding::{
    ABSTAIN_NO_DATA_OR_DOC_ZH, count_grounded_numbers, should_abstain_zero_evidence,
};
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::ports::AiPort;
use chrono::{SecondsFormat, Utc};
use uuid::Uuid;

use crate::nl2sql::chart::{
    fallback_table_summary, recommend_chart, verify_or_fallback_summary_with_count,
};
use crate::nl2sql::intent::route_intent;
use crate::nl2sql::ontology_route::{
    OntologyCompileArgs, OntologyRouteOutcome, compile_and_execute_slots_conn,
    compile_and_execute_slots_session, extract_ontology_slots, should_attempt_ontology_route,
};
use crate::nl2sql::prompt::{
    CHITCHAT_SYSTEM_PROMPT, LlmSqlDraft, SUMMARY_SYSTEM_PROMPT, build_nl2sql_system_prompt,
    build_nl2sql_user_prompt, build_repair_user_prompt_with_joins, build_summary_user_prompt,
    parse_llm_sql_response, strip_trailing_semicolons,
};
use crate::nl2sql::schema_link::{LinkedSchemaContext, SchemaLinkExtras, build_linked_schema};
use crate::result::{ResultSerializeOptions, batches_to_query_result};
use crate::session::TenantDb;
use crate::sql_guard::{GuardOptions, GuardedSql, TenantCatalog, guard};

/// Hard `max_rows` cap applied to all NL2SQL executions (`05-STEP04` §04.6.1).
pub const NL2SQL_MAX_ROWS: u32 = 1_000;

/// Maximum self-repair rounds when `sql_guard` or `EXPLAIN` fails (`05-STEP04` §04.6.5).
pub const MAX_REPAIR_ROUNDS: usize = 2;

/// Maximum rows sent to the LLM for natural-language summarization (`05-STEP04` §04.6.6).
pub const MAX_SUMMARY_ROWS: usize = 20;

/// Input bundle for executing a single turn of the NL2SQL pipeline.
#[derive(Debug, Clone)]
pub struct AskPipelineInput<'a> {
    /// Incoming user [`AskRequest`].
    pub request: &'a AskRequest,
    /// Tenant catalog for schema linking and `sql_guard`.
    pub catalog: &'a TenantCatalog,
    /// Pre-recalled FTS/vector candidates from `AnalyticsRepository` (or empty default).
    pub db_recall: &'a SchemaRecallBundle,
    /// Extra metrics, masked samples, and scoping constraints.
    pub link_extras: &'a SchemaLinkExtras,
    /// Up to 5 positive-feedback few-shot examples.
    pub few_shots: &'a [FewShotTurn],
    /// Chronological turns in the same `session_id`.
    pub session_turns: &'a [AskTurnRecord],
    /// Tenant IANA timezone string (defaults to `"UTC"`).
    pub timezone: &'a str,
    /// Optional fixed RFC 3339 timestamp for deterministic evaluation replay.
    pub now_rfc3339_override: Option<&'a str>,
    /// Optional query execution timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Forced intent override (e.g., `ChatQueryMode::Data`).
    pub force_intent: Option<AskIntent>,
}

/// Complete output of the NL2SQL pipeline, including telemetry for history/audit/evals.
#[derive(Debug, Clone)]
pub struct AskPipelineOutput {
    /// API-ready [`AskResponse`] (`turn_id` pre-populated; route may overwrite with DB row UUID).
    pub response: AskResponse,
    /// Rewritten `lake."tenant_<id>"` SQL executed against `DuckDB` (empty if not executed).
    pub rewritten_sql: String,
    /// Number of self-repair rounds used (`0..=2`).
    pub repair_rounds: usize,
    /// Total pipeline wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// Validates user-submitted `corrected_sql` on `POST /api/analytics/ask/{turn_id}/feedback`
/// through `sql_guard` (`05-STEP04` §04.6.7).
///
/// # Errors
/// Returns `SQL_REJECTED` or `INVALID_ARGUMENT` if `corrected_sql` violates any guard rule.
pub fn validate_corrected_sql(
    corrected_sql: &str,
    catalog: &TenantCatalog,
) -> Result<String, AppError> {
    let clean = strip_trailing_semicolons(corrected_sql.trim());
    if clean.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "corrected_sql cannot be empty",
        ));
    }
    let _ = guard(
        &clean,
        catalog,
        &GuardOptions {
            max_rows: NL2SQL_MAX_ROWS,
            allow_live: false,
            params: BTreeMap::new(),
            policy: None,
            ..GuardOptions::default()
        },
    )?;
    Ok(clean)
}

/// Executes the full NL2SQL pipeline against an async [`TenantDb`] session (`05-STEP04` §04.6).
///
/// # Errors
/// Propagates [`AppError::BadRequest`] if `question` is empty, [`AppError::Llm`] (`UPSTREAM_LLM`)
/// if primary SQL generation fails at the AI gateway, or [`AppError::Timeout`] on `DuckDB` timeout.
pub async fn run_ask_pipeline<A: AiPort>(
    ai: &A,
    session: &TenantDb,
    input: &AskPipelineInput<'_>,
) -> Result<AskPipelineOutput, AppError> {
    run_ask_pipeline_with_policy(ai, session, input, None).await
}

/// Executes the full NL2SQL pipeline against an async [`TenantDb`] session under an
/// optional compiled `AxGuard` [`PolicyContext`] (`11-STEP13` §13.6.3, `02-STEP16` §16.2).
///
/// # Errors
/// Propagates [`AppError`] on empty question, `POLICY_DENIED`, upstream LLM failure, or `DuckDB` timeout.
pub async fn run_ask_pipeline_with_policy<A: AiPort>(
    ai: &A,
    session: &TenantDb,
    input: &AskPipelineInput<'_>,
    policy: Option<&PolicyContext>,
) -> Result<AskPipelineOutput, AppError> {
    let started = Instant::now();
    let question = input.request.question.trim();
    if question.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "question cannot be empty",
        ));
    }

    let session_id = input.request.session_id.unwrap_or_else(Uuid::new_v4);
    let linked = build_linked_schema_with_session(input);
    let intent = determine_intent(ai, input, &linked).await?;

    if matches!(intent, AskIntent::Chitchat | AskIntent::Doc) {
        return finish_non_sql_intent(ai, intent, session_id, question, linked, started).await;
    }
    if linked.targeted_excluded_dataset
        || should_abstain_zero_evidence(
            linked.has_positive_hit,
            !input.session_turns.is_empty(),
            0,
            false,
        )
    {
        return Ok(build_zero_evidence_output(
            session_id, intent, linked, started,
        ));
    }
    let s0_meta = Stage0TurnMeta {
        session_id,
        intent,
        question,
        started,
        policy,
    };
    if let Some(stage0_out) = try_stage0_session(ai, session, input, &linked, s0_meta).await? {
        return Ok(stage0_out);
    }

    let (sys_prompt, user_prompt) = build_prompts(input, &linked);
    let initial_raw = ai.chat(&sys_prompt, &user_prompt).await?;
    let initial_draft = parse_llm_sql_response(&initial_raw)?;
    if initial_draft.sql.is_empty() {
        return Ok(build_clarification_output(
            session_id,
            intent,
            &initial_draft,
            linked,
            started,
        ));
    }

    let repair_outcome = guard_and_explain_session_loop(
        ai,
        session,
        input.catalog,
        (&sys_prompt, &user_prompt),
        initial_draft,
        policy,
        &linked,
    )
    .await?;

    let ValidatedDraftOutcome::Ready {
        clean_sql,
        explanation,
        guarded,
        repair_rounds,
    } = repair_outcome
    else {
        return Ok(finalize_non_ready_outcome(
            session_id,
            intent,
            repair_outcome,
            linked,
            started,
        ));
    };

    let exec_out = session
        .execute_guarded(&guarded, input.catalog, input.timeout_ms, false, false)
        .await?;
    finalize_succeeded_output(
        ai,
        SucceededTurnArgs {
            session_id,
            intent,
            question,
            clean_sql,
            explanation,
            rewritten_sql: guarded.sql,
            qr: exec_out.result,
            linked,
            repair_rounds,
            started,
            grounding_mode: GroundingMode::GuardedSql,
            slot_payload: None,
        },
    )
    .await
}

/// Executes the full NL2SQL pipeline against an in-memory/open `duckdb::Connection`
/// (used by deterministic `nl2sql_eval`, `hallucination_eval`, and unit tests).
///
/// # Errors
/// Propagates [`AppError`] on empty question or upstream LLM generation failure.
pub async fn run_ask_pipeline_conn<A: AiPort>(
    ai: &A,
    conn: &duckdb::Connection,
    input: &AskPipelineInput<'_>,
) -> Result<AskPipelineOutput, AppError> {
    let started = Instant::now();
    let question = input.request.question.trim();
    if question.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "question cannot be empty",
        ));
    }

    let session_id = input.request.session_id.unwrap_or_else(Uuid::new_v4);
    let linked = build_linked_schema_with_session(input);
    let intent = determine_intent(ai, input, &linked).await?;

    if matches!(intent, AskIntent::Chitchat | AskIntent::Doc) {
        return finish_non_sql_intent(ai, intent, session_id, question, linked, started).await;
    }
    if linked.targeted_excluded_dataset
        || should_abstain_zero_evidence(
            linked.has_positive_hit,
            !input.session_turns.is_empty(),
            0,
            false,
        )
    {
        return Ok(build_zero_evidence_output(
            session_id, intent, linked, started,
        ));
    }
    if let Some(stage0_out) = try_stage0_conn(
        ai,
        conn,
        input,
        &linked,
        Stage0TurnMeta {
            session_id,
            intent,
            question,
            started,
            policy: None,
        },
    )
    .await?
    {
        return Ok(stage0_out);
    }

    let (sys_prompt, user_prompt) = build_prompts(input, &linked);
    let initial_raw = ai.chat(&sys_prompt, &user_prompt).await?;
    let initial_draft = parse_llm_sql_response(&initial_raw)?;
    if initial_draft.sql.is_empty() {
        return Ok(build_clarification_output(
            session_id,
            intent,
            &initial_draft,
            linked,
            started,
        ));
    }

    let repair_outcome = guard_and_explain_conn_loop(
        ai,
        conn,
        input.catalog,
        (&sys_prompt, &user_prompt),
        initial_draft,
        &linked,
    )
    .await?;

    let ValidatedDraftOutcome::Ready {
        clean_sql,
        explanation,
        guarded,
        repair_rounds,
    } = repair_outcome
    else {
        return Ok(finalize_non_ready_outcome(
            session_id,
            intent,
            repair_outcome,
            linked,
            started,
        ));
    };

    let qr = execute_guarded_conn(conn, input.catalog, &guarded)?;
    finalize_succeeded_output(
        ai,
        SucceededTurnArgs {
            session_id,
            intent,
            question,
            clean_sql,
            explanation,
            rewritten_sql: guarded.sql,
            qr,
            linked,
            repair_rounds,
            started,
            grounding_mode: GroundingMode::GuardedSql,
            slot_payload: None,
        },
    )
    .await
}

struct Stage0TurnMeta<'a> {
    session_id: Uuid,
    intent: AskIntent,
    question: &'a str,
    started: Instant,
    policy: Option<&'a PolicyContext>,
}

async fn try_stage0_session<A: AiPort>(
    ai: &A,
    session: &TenantDb,
    input: &AskPipelineInput<'_>,
    linked: &LinkedSchemaContext,
    meta: Stage0TurnMeta<'_>,
) -> Result<Option<AskPipelineOutput>, AppError> {
    if !should_attempt_ontology_route(meta.question, linked, input.catalog) {
        return Ok(None);
    }
    let (tz, now_str) = resolve_tz_and_now(input);
    let Some(draft) = extract_ontology_slots(
        ai,
        meta.question,
        linked,
        input.catalog,
        tz,
        &now_str,
        input.request.context.as_ref(),
    )
    .await
    else {
        return Ok(None);
    };

    let outcome = compile_and_execute_slots_session(
        session,
        &OntologyCompileArgs {
            draft: &draft,
            linked,
            catalog: input.catalog,
            context: input.request.context.as_ref(),
            timeout_ms: input.timeout_ms,
            policy: meta.policy,
        },
    )
    .await?;
    finish_stage0_outcome(ai, outcome, linked, meta).await
}

async fn try_stage0_conn<A: AiPort>(
    ai: &A,
    conn: &duckdb::Connection,
    input: &AskPipelineInput<'_>,
    linked: &LinkedSchemaContext,
    meta: Stage0TurnMeta<'_>,
) -> Result<Option<AskPipelineOutput>, AppError> {
    if !should_attempt_ontology_route(meta.question, linked, input.catalog) {
        return Ok(None);
    }
    let (tz, now_str) = resolve_tz_and_now(input);
    let Some(draft) = extract_ontology_slots(
        ai,
        meta.question,
        linked,
        input.catalog,
        tz,
        &now_str,
        input.request.context.as_ref(),
    )
    .await
    else {
        return Ok(None);
    };

    let outcome = compile_and_execute_slots_conn(
        conn,
        &OntologyCompileArgs {
            draft: &draft,
            linked,
            catalog: input.catalog,
            context: input.request.context.as_ref(),
            timeout_ms: input.timeout_ms,
            policy: meta.policy,
        },
    )?;
    finish_stage0_outcome(ai, outcome, linked, meta).await
}

async fn finish_stage0_outcome<A: AiPort>(
    ai: &A,
    outcome: OntologyRouteOutcome,
    linked: &LinkedSchemaContext,
    meta: Stage0TurnMeta<'_>,
) -> Result<Option<AskPipelineOutput>, AppError> {
    match outcome {
        OntologyRouteOutcome::Executed {
            grounding_mode,
            clean_sql,
            rewritten_sql,
            qr,
            explanation,
            slot_payload,
        } => {
            let out = finalize_succeeded_output(
                ai,
                SucceededTurnArgs {
                    session_id: meta.session_id,
                    intent: meta.intent,
                    question: meta.question,
                    clean_sql,
                    explanation,
                    rewritten_sql,
                    qr,
                    linked: linked.clone(),
                    repair_rounds: 0,
                    started: meta.started,
                    grounding_mode,
                    slot_payload: Some(slot_payload),
                },
            )
            .await?;
            Ok(Some(out))
        }
        OntologyRouteOutcome::Clarify {
            message,
            slot_payload,
        } => {
            let draft = LlmSqlDraft {
                sql: String::new(),
                explanation: message,
                used_metrics: Vec::new(),
            };
            let mut out = build_clarification_output(
                meta.session_id,
                meta.intent,
                &draft,
                linked.clone(),
                meta.started,
            );
            out.response.slot_payload = slot_payload;
            Ok(Some(out))
        }
        OntologyRouteOutcome::FallbackSql => Ok(None),
    }
}

enum ValidatedDraftOutcome {
    Ready {
        clean_sql: String,
        explanation: String,
        guarded: GuardedSql,
        repair_rounds: usize,
    },
    Clarify(LlmSqlDraft, usize),
    RejectedJoin {
        last_sql: String,
        last_error: String,
        repair_rounds: usize,
    },
    Exhausted {
        last_sql: String,
        last_error: String,
        repair_rounds: usize,
    },
}

fn build_linked_schema_with_session(input: &AskPipelineInput<'_>) -> LinkedSchemaContext {
    let mut extras = input.link_extras.clone();
    if let Some(ctx) = input.request.context.as_ref()
        && !ctx.dataset_ids.is_empty()
    {
        extras.restrict_dataset_ids.clone_from(&ctx.dataset_ids);
    }
    for t in input.session_turns {
        if let Some(ref s) = t.sql_text
            && !s.trim().is_empty()
        {
            extras.prior_session_sqls.push(s.clone());
        }
    }
    build_linked_schema(
        &input.request.question,
        input.catalog,
        input.db_recall,
        &extras,
    )
}

async fn determine_intent<A: AiPort>(
    ai: &A,
    input: &AskPipelineInput<'_>,
    linked: &LinkedSchemaContext,
) -> Result<AskIntent, AppError> {
    if let Some(forced) = input.force_intent {
        return Ok(forced);
    }
    if linked.targeted_excluded_dataset {
        return Ok(AskIntent::Data);
    }
    let has_prior_data = input
        .session_turns
        .iter()
        .any(|t| matches!(t.intent, AskIntent::Data | AskIntent::Hybrid) && t.sql_text.is_some());
    route_intent(
        ai,
        &input.request.question,
        linked.has_positive_hit,
        has_prior_data,
    )
    .await
}

fn resolve_tz_and_now<'a>(input: &'a AskPipelineInput<'_>) -> (&'a str, String) {
    let tz = if input.timezone.trim().is_empty() {
        "UTC"
    } else {
        input.timezone.trim()
    };
    let now_str = input.now_rfc3339_override.map_or_else(
        || Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        String::from,
    );
    (tz, now_str)
}

fn build_prompts(input: &AskPipelineInput<'_>, linked: &LinkedSchemaContext) -> (String, String) {
    let (tz, now_str) = resolve_tz_and_now(input);
    let sys = build_nl2sql_system_prompt(&now_str, tz);
    let usr = build_nl2sql_user_prompt(
        &input.request.question,
        linked,
        input.few_shots,
        input.session_turns,
        input.request.context.as_ref(),
    );
    (sys, usr)
}

fn build_zero_evidence_output(
    session_id: Uuid,
    intent: AskIntent,
    linked: LinkedSchemaContext,
    started: Instant,
) -> AskPipelineOutput {
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    AskPipelineOutput {
        response: AskResponse {
            turn_id: Uuid::new_v4(),
            session_id,
            intent,
            status: QueryHistoryStatus::Succeeded,
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            answer: String::from(ABSTAIN_NO_DATA_OR_DOC_ZH),
            chart: None,
            need_clarification: false,
            explanation: String::from(ABSTAIN_NO_DATA_OR_DOC_ZH),
            linked_objects: linked.linked_objects,
            citations: Vec::new(),
            grounding_mode: GroundingMode::Abstained,
            verified_numbers: 0,
            verified_citations: 0,
            abstained: true,
            slot_payload: None,
        },
        rewritten_sql: String::new(),
        repair_rounds: 0,
        duration_ms,
    }
}

async fn finish_non_sql_intent<A: AiPort>(
    ai: &A,
    intent: AskIntent,
    session_id: Uuid,
    question: &str,
    linked: LinkedSchemaContext,
    started: Instant,
) -> Result<AskPipelineOutput, AppError> {
    let answer = if intent == AskIntent::Chitchat {
        ai.chat(CHITCHAT_SYSTEM_PROMPT, question)
            .await
            .unwrap_or_else(|_| {
                String::from(
                    "您好！我是 xBlueNode 智能分析助手，请问有什么数据或文档问题需要帮您分析？",
                )
            })
    } else {
        String::from("该问题属于制度/文档知识范畴，请通过文档问答模式检索相关依据。")
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(AskPipelineOutput {
        response: AskResponse {
            turn_id: Uuid::new_v4(),
            session_id,
            intent,
            status: QueryHistoryStatus::Succeeded,
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            answer,
            chart: None,
            need_clarification: false,
            explanation: String::new(),
            linked_objects: linked.linked_objects,
            citations: Vec::new(),
            grounding_mode: GroundingMode::Clarification,
            verified_numbers: 0,
            verified_citations: 0,
            abstained: false,
            slot_payload: None,
        },
        rewritten_sql: String::new(),
        repair_rounds: 0,
        duration_ms,
    })
}

fn build_clarification_output(
    session_id: Uuid,
    intent: AskIntent,
    draft: &LlmSqlDraft,
    linked: LinkedSchemaContext,
    started: Instant,
) -> AskPipelineOutput {
    let msg = if draft.explanation.trim().is_empty() {
        String::from("请补充说明您需要查询的具体指标、时间范围或筛选条件。")
    } else {
        draft.explanation.trim().to_owned()
    };
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    AskPipelineOutput {
        response: AskResponse {
            turn_id: Uuid::new_v4(),
            session_id,
            intent,
            status: QueryHistoryStatus::Succeeded,
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            answer: msg.clone(),
            chart: None,
            need_clarification: true,
            explanation: msg,
            linked_objects: linked.linked_objects,
            citations: Vec::new(),
            grounding_mode: GroundingMode::Clarification,
            verified_numbers: 0,
            verified_citations: 0,
            abstained: false,
            slot_payload: None,
        },
        rewritten_sql: String::new(),
        repair_rounds: 0,
        duration_ms,
    }
}

async fn guard_and_explain_session_loop<A: AiPort>(
    ai: &A,
    session: &TenantDb,
    catalog: &TenantCatalog,
    prompts: (&str, &str),
    mut current_draft: LlmSqlDraft,
    policy: Option<&PolicyContext>,
    linked: &LinkedSchemaContext,
) -> Result<ValidatedDraftOutcome, AppError> {
    let (sys_prompt, user_prompt) = prompts;
    let guard_opts = GuardOptions {
        max_rows: NL2SQL_MAX_ROWS,
        allow_live: false,
        params: BTreeMap::new(),
        policy: policy.cloned(),
        strict_ontology_join: linked.enforce_ontology_joins,
        allowed_joins: linked.allowed_joins.clone(),
    };

    for round in 0..=MAX_REPAIR_ROUNDS {
        let (is_ungrounded_join, last_error) = match guard(&current_draft.sql, catalog, &guard_opts)
        {
            Ok(guarded) => match session.explain_guarded(&guarded, None).await {
                Ok(_plan) => {
                    return Ok(ValidatedDraftOutcome::Ready {
                        clean_sql: current_draft.sql,
                        explanation: current_draft.explanation,
                        guarded,
                        repair_rounds: round,
                    });
                }
                Err(explain_err) => (false, first_error_line(&explain_err)),
            },
            Err(AppError::Forbidden(code)) => return Err(AppError::Forbidden(code)),
            Err(guard_err) => (
                guard_err.error_code() == "JOIN_PATH_UNGROUNDED",
                first_error_line(&guard_err),
            ),
        };

        if is_ungrounded_join && (linked.allowed_joins.is_empty() || round == MAX_REPAIR_ROUNDS) {
            return Ok(ValidatedDraftOutcome::RejectedJoin {
                last_sql: current_draft.sql,
                last_error,
                repair_rounds: round,
            });
        }
        if round == MAX_REPAIR_ROUNDS {
            return Ok(ValidatedDraftOutcome::Exhausted {
                last_sql: current_draft.sql,
                last_error,
                repair_rounds: round,
            });
        }

        let next_round = round.saturating_add(1);
        let repair_usr = build_repair_user_prompt_with_joins(
            user_prompt,
            &current_draft.sql,
            &last_error,
            next_round,
            &linked.allowed_joins,
        );
        let repaired_raw = ai.chat(sys_prompt, &repair_usr).await?;
        current_draft = parse_llm_sql_response(&repaired_raw)?;
        if current_draft.sql.is_empty() {
            return Ok(ValidatedDraftOutcome::Clarify(current_draft, next_round));
        }
    }

    Ok(ValidatedDraftOutcome::Exhausted {
        last_sql: current_draft.sql,
        last_error: String::from("repair loop exhausted"),
        repair_rounds: MAX_REPAIR_ROUNDS,
    })
}

async fn guard_and_explain_conn_loop<A: AiPort>(
    ai: &A,
    conn: &duckdb::Connection,
    catalog: &TenantCatalog,
    prompts: (&str, &str),
    mut current_draft: LlmSqlDraft,
    linked: &LinkedSchemaContext,
) -> Result<ValidatedDraftOutcome, AppError> {
    let (sys_prompt, user_prompt) = prompts;
    let guard_opts = GuardOptions {
        max_rows: NL2SQL_MAX_ROWS,
        allow_live: false,
        params: BTreeMap::new(),
        policy: None,
        strict_ontology_join: linked.enforce_ontology_joins,
        allowed_joins: linked.allowed_joins.clone(),
    };

    for round in 0..=MAX_REPAIR_ROUNDS {
        let (is_ungrounded_join, last_error) = match guard(&current_draft.sql, catalog, &guard_opts)
        {
            Ok(guarded) => match explain_on_conn(conn, &guarded.sql) {
                Ok(()) => {
                    return Ok(ValidatedDraftOutcome::Ready {
                        clean_sql: current_draft.sql,
                        explanation: current_draft.explanation,
                        guarded,
                        repair_rounds: round,
                    });
                }
                Err(explain_err) => (false, first_error_line(&explain_err)),
            },
            Err(AppError::Forbidden(code)) => return Err(AppError::Forbidden(code)),
            Err(guard_err) => (
                guard_err.error_code() == "JOIN_PATH_UNGROUNDED",
                first_error_line(&guard_err),
            ),
        };

        if is_ungrounded_join && (linked.allowed_joins.is_empty() || round == MAX_REPAIR_ROUNDS) {
            return Ok(ValidatedDraftOutcome::RejectedJoin {
                last_sql: current_draft.sql,
                last_error,
                repair_rounds: round,
            });
        }
        if round == MAX_REPAIR_ROUNDS {
            return Ok(ValidatedDraftOutcome::Exhausted {
                last_sql: current_draft.sql,
                last_error,
                repair_rounds: round,
            });
        }

        let next_round = round.saturating_add(1);
        let repair_usr = build_repair_user_prompt_with_joins(
            user_prompt,
            &current_draft.sql,
            &last_error,
            next_round,
            &linked.allowed_joins,
        );
        let repaired_raw = ai.chat(sys_prompt, &repair_usr).await?;
        current_draft = parse_llm_sql_response(&repaired_raw)?;
        if current_draft.sql.is_empty() {
            return Ok(ValidatedDraftOutcome::Clarify(current_draft, next_round));
        }
    }

    Ok(ValidatedDraftOutcome::Exhausted {
        last_sql: current_draft.sql,
        last_error: String::from("repair loop exhausted"),
        repair_rounds: MAX_REPAIR_ROUNDS,
    })
}

fn finalize_non_ready_outcome(
    session_id: Uuid,
    intent: AskIntent,
    outcome: ValidatedDraftOutcome,
    linked: LinkedSchemaContext,
    started: Instant,
) -> AskPipelineOutput {
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        ValidatedDraftOutcome::Clarify(draft, rounds) => {
            let mut out = build_clarification_output(session_id, intent, &draft, linked, started);
            out.repair_rounds = rounds;
            out
        }
        ValidatedDraftOutcome::RejectedJoin {
            last_sql,
            last_error,
            repair_rounds,
        } => AskPipelineOutput {
            response: AskResponse {
                turn_id: Uuid::new_v4(),
                session_id,
                intent,
                status: QueryHistoryStatus::Rejected,
                sql: last_sql,
                columns: Vec::new(),
                rows: Vec::new(),
                truncated: false,
                answer: format!("跨表关联缺少已发布本体外键依据，已拦截：{last_error}"),
                chart: None,
                need_clarification: false,
                explanation: last_error,
                linked_objects: linked.linked_objects,
                citations: Vec::new(),
                grounding_mode: GroundingMode::Abstained,
                verified_numbers: 0,
                verified_citations: 0,
                abstained: true,
                slot_payload: None,
            },
            rewritten_sql: String::new(),
            repair_rounds,
            duration_ms,
        },
        ValidatedDraftOutcome::Exhausted {
            last_sql,
            last_error,
            repair_rounds,
        } => AskPipelineOutput {
            response: AskResponse {
                turn_id: Uuid::new_v4(),
                session_id,
                intent,
                status: QueryHistoryStatus::Failed,
                sql: last_sql,
                columns: Vec::new(),
                rows: Vec::new(),
                truncated: false,
                answer: format!("SQL 校验或试运行未通过：{last_error}"),
                chart: None,
                need_clarification: false,
                explanation: last_error,
                linked_objects: linked.linked_objects,
                citations: Vec::new(),
                grounding_mode: GroundingMode::GuardedSql,
                verified_numbers: 0,
                verified_citations: 0,
                abstained: false,
                slot_payload: None,
            },
            rewritten_sql: String::new(),
            repair_rounds,
            duration_ms,
        },
        ValidatedDraftOutcome::Ready { .. } => AskPipelineOutput {
            response: AskResponse {
                turn_id: Uuid::new_v4(),
                session_id,
                intent,
                status: QueryHistoryStatus::Failed,
                sql: String::new(),
                columns: Vec::new(),
                rows: Vec::new(),
                truncated: false,
                answer: String::new(),
                chart: None,
                need_clarification: false,
                explanation: String::new(),
                linked_objects: linked.linked_objects,
                citations: Vec::new(),
                grounding_mode: GroundingMode::GuardedSql,
                verified_numbers: 0,
                verified_citations: 0,
                abstained: false,
                slot_payload: None,
            },
            rewritten_sql: String::new(),
            repair_rounds: 0,
            duration_ms,
        },
    }
}

struct SucceededTurnArgs<'a> {
    session_id: Uuid,
    intent: AskIntent,
    question: &'a str,
    clean_sql: String,
    explanation: String,
    rewritten_sql: String,
    qr: QueryResult,
    linked: LinkedSchemaContext,
    repair_rounds: usize,
    started: Instant,
    grounding_mode: GroundingMode,
    slot_payload: Option<serde_json::Value>,
}

async fn finalize_succeeded_output<A: AiPort>(
    ai: &A,
    args: SucceededTurnArgs<'_>,
) -> Result<AskPipelineOutput, AppError> {
    let chart = recommend_chart(args.question, &args.qr.columns, args.qr.rows.len());
    let (answer, verified_numbers) =
        if args.qr.rows.is_empty() || args.qr.rows.len() > MAX_SUMMARY_ROWS {
            let fb = fallback_table_summary(args.qr.rows.len());
            let cnt = count_grounded_numbers(&fb);
            (fb, cnt)
        } else {
            let sum_usr = build_summary_user_prompt(args.question, &args.qr.columns, &args.qr.rows);
            if let Ok(raw_summary) = ai.chat(SUMMARY_SYSTEM_PROMPT, &sum_usr).await {
                verify_or_fallback_summary_with_count(&raw_summary, &args.qr.rows)
            } else {
                let fb = fallback_table_summary(args.qr.rows.len());
                let cnt = count_grounded_numbers(&fb);
                (fb, cnt)
            }
        };

    let duration_ms = u64::try_from(args.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let response = AskResponse {
        turn_id: Uuid::new_v4(),
        session_id: args.session_id,
        intent: args.intent,
        status: QueryHistoryStatus::Succeeded,
        sql: args.clean_sql,
        columns: args.qr.columns,
        rows: args.qr.rows,
        truncated: args.qr.truncated,
        answer,
        chart: Some(chart),
        need_clarification: false,
        explanation: args.explanation,
        linked_objects: args.linked.linked_objects,
        citations: Vec::new(),
        grounding_mode: args.grounding_mode,
        verified_numbers,
        verified_citations: 0,
        abstained: false,
        slot_payload: args.slot_payload,
    };
    Ok(AskPipelineOutput {
        response,
        rewritten_sql: args.rewritten_sql,
        repair_rounds: args.repair_rounds,
        duration_ms,
    })
}

pub(crate) fn explain_on_conn(conn: &duckdb::Connection, sql: &str) -> Result<(), AppError> {
    let explain_sql = format!("EXPLAIN {sql}");
    let mut stmt = conn
        .prepare(&explain_sql)
        .map_err(|err| AppError::bad("SQL_REJECTED", format!("EXPLAIN failed: {err}")))?;
    let stream = stmt
        .query_arrow([])
        .map_err(|err| AppError::bad("SQL_REJECTED", format!("EXPLAIN execution failed: {err}")))?;
    let _batches: Vec<RecordBatch> = stream.collect();
    Ok(())
}

pub(crate) fn execute_guarded_conn(
    conn: &duckdb::Connection,
    catalog: &TenantCatalog,
    guarded: &GuardedSql,
) -> Result<QueryResult, AppError> {
    let started = Instant::now();
    let mut stmt = conn
        .prepare(&guarded.sql)
        .map_err(|err| AppError::Database(format!("DuckDB prepare failed: {err}")))?;
    let stream = stmt
        .query_arrow([])
        .map_err(|err| AppError::Database(format!("DuckDB query failed: {err}")))?;
    let schema = stream.get_schema();
    let batches: Vec<RecordBatch> = stream.collect();
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let out = batches_to_query_result(
        &schema,
        &batches,
        ResultSerializeOptions {
            effective_max_rows: guarded.effective_max_rows,
            sql: guarded.sql.clone(),
            duration_ms,
            reveal_pii: false,
            has_pii_raw_read: false,
            catalog,
            guarded,
        },
    )?;
    Ok(out.result)
}

fn first_error_line(err: &AppError) -> String {
    let full = format!("{}: {err}", err.error_code());
    full.lines().next().map_or_else(
        || String::from(err.error_code()),
        |s| String::from(s.trim()),
    )
}
