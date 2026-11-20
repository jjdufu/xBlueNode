//! Document ingestion, Sidecar upload forwarding, and hybrid `AxQuery` (`DOC` / `DATA` / `HYBRID`) routes.

use std::fmt::Write as _;
use std::time::Instant;

use axpoint_analytics::nl2sql::chart::verify_or_fallback_summary;
use axpoint_analytics::nl2sql::intent::{classify_intent_by_rules, route_intent};
use axpoint_analytics::nl2sql::schema_link::{SchemaLinkExtras, build_linked_schema};
use axpoint_db::{DocumentIngestPayload, ParaIngestInput};
use axpoint_domain::analytics::{
    AskCitation, AskIntent, AskRequest, ChatQueryMode, NewAskTurnInput,
};
use axpoint_domain::enums::{AuditAction, Permission};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::AnalyticsRepository as _;
use axpoint_domain::{
    AppError, ChatQueryRequest, ChatQueryResponse, FusedHit, IngestResponse, IngestTextRequest,
    rrf_fuse, split_paragraphs,
};
use axpoint_security::AuthPrincipal;
use axum::extract::{Multipart, State};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use tracing::warn;
use uuid::Uuid;

use crate::routes::analytics::{execute_ask_for_principal, load_tenant_catalog_and_live};
use crate::state::{ApiJson, AppState, HttpError, ParserSidecarResponse};
use crate::telemetry::{current_trace_id, outbound_traceparent};

const HYBRID_SYSTEM_PROMPT: &str = "你是企业知识与数据分析混合问答助手。请综合给定的【结构化查询结果表】与【召回文档段落】回答用户问题。数据结论标注 [表]，文档依据标注 [文1]、[文2] 等编号；严禁编造结果表中不存在的数值。";

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/documents/ingest", post(ingest_text_handler))
        .route("/documents/upload", post(upload_document_handler))
        .route("/chat/query", post(chat_query_handler))
}

async fn ingest_text_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<IngestTextRequest>,
) -> Result<Json<IngestResponse>, HttpError> {
    principal.require(Permission::DataSync)?;
    let resp = run_ingest_pipeline(&state, &principal.tenant_id, req).await?;
    Ok(Json(resp))
}

async fn upload_document_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    mut multipart: Multipart,
) -> Result<Json<IngestResponse>, HttpError> {
    principal.require(Permission::DataSync)?;
    let mut file_name = String::from("uploaded.pdf");
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart.next_field().await.map_err(|err| {
        HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            format!("multipart error: {err}"),
        ))
    })? {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "tenant_id" {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "tenant_id parameter is forbidden",
            )));
        }
        if name == "file" {
            if let Some(orig) = field.file_name() {
                file_name = orig.to_owned();
            }
            let bytes = field.bytes().await.map_err(|err| {
                HttpError(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("read file failed: {err}"),
                ))
            })?;
            file_bytes = Some(bytes.to_vec());
        }
    }

    let raw_bytes = file_bytes
        .ok_or_else(|| HttpError(AppError::bad("INVALID_ARGUMENT", "missing 'file' field")))?;

    let part = reqwest::multipart::Part::bytes(raw_bytes).file_name(file_name.clone());
    let form = reqwest::multipart::Form::new().part("file", part);
    let parse_url = format!("{}/parse", state.parser_base_url.trim_end_matches('/'));
    let traceparent = outbound_traceparent();

    let parser_resp = state
        .http
        .post(&parse_url)
        .header("X-AxPoint-Tenant", &principal.tenant_id)
        .header("traceparent", traceparent)
        .multipart(form)
        .send()
        .await
        .map_err(|err| HttpError(AppError::Parser(format!("sidecar request failed: {err}"))))?;

    if !parser_resp.status().is_success() {
        let status = parser_resp.status();
        let text = parser_resp.text().await.unwrap_or_default();
        return Err(HttpError(AppError::Parser(format!(
            "sidecar returned {status}: {text}"
        ))));
    }

    let parsed: ParserSidecarResponse = parser_resp
        .json()
        .await
        .map_err(|err| HttpError(AppError::Parser(format!("invalid sidecar JSON: {err}"))))?;

    let resp = run_ingest_pipeline(
        &state,
        &principal.tenant_id,
        IngestTextRequest {
            title: parsed.title,
            source_name: file_name,
            content: parsed.markdown,
        },
    )
    .await?;

    Ok(Json(resp))
}

async fn run_ingest_pipeline(
    state: &AppState,
    tenant_id: &str,
    req: IngestTextRequest,
) -> Result<IngestResponse, AppError> {
    ingest_text_with_deps(&state.store, &state.ai, tenant_id, req).await
}

/// Runs the hypergraph text ingestion pipeline (`split_paragraphs` -> `embed_batch` -> `extract_triples` -> `ingest_document`).
pub(crate) async fn ingest_text_with_deps(
    store: &axpoint_db::PgStore,
    ai: &axpoint_llm::AiGateway,
    tenant_id: &str,
    req: IngestTextRequest,
) -> Result<IngestResponse, AppError> {
    if req.content.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "document content is empty",
        ));
    }

    let chunks = split_paragraphs(&req.content, 900);
    if chunks.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "no valid paragraphs found",
        ));
    }

    let (vectors, embed_dim) = ai.embed_batch(&chunks).await?;
    let mut para_inputs = Vec::with_capacity(chunks.len());

    for (idx, (chunk, vec)) in chunks.into_iter().zip(vectors).enumerate() {
        let triples = match ai.extract_triples(&chunk).await {
            Ok(t) => t,
            Err(err) => {
                warn!("triple extraction skipped for chunk {idx}: {err}");
                Vec::new()
            }
        };
        let seq_no = i32::try_from(idx).unwrap_or(i32::MAX);
        para_inputs.push(ParaIngestInput {
            seq_no,
            content: chunk,
            triples,
            embedding: vec,
        });
    }

    let summary = para_inputs
        .first()
        .map_or_default(|p| p.content.chars().take(200).collect::<String>());

    let title_clone = req.title.clone();
    let commit_res = store
        .ingest_document(DocumentIngestPayload {
            tenant_id: tenant_id.to_owned(),
            title: req.title,
            source_name: req.source_name,
            summary,
            doc_embedding: None,
            embed_model: ai.embedding_model_name().to_owned(),
            embed_dim,
            paras: para_inputs,
        })
        .await?;

    if let Err(err) = axpoint_domain::ports::GraphStorePort::resolve_entity_links(
        store,
        tenant_id,
        axpoint_domain::graph::ResolveEntitiesRequest::default(),
    )
    .await
    {
        tracing::warn!(tenant_id = %tenant_id, error = %err, "post-ingest resolve_entity_links failed");
    }

    Ok(IngestResponse {
        doc_id: commit_res.doc_id,
        title: title_clone,
        para_count: commit_res.para_count,
        entity_count: commit_res.entity_count,
        edge_count: commit_res.edge_count,
        embedding_model: ai.embedding_model_name().to_owned(),
        embedding_dim: embed_dim,
    })
}

struct DocRagOutcome {
    answer: String,
    hits: Vec<FusedHit>,
    seeds: Vec<String>,
    citations: Vec<AskCitation>,
}

/// Executes 5-channel hypergraph document recall (`vector + bm25 + entity_link + multi_hop + context`)
/// fused via RRF, optionally filtered by `doc_title_prefix` (`08-STEP09` §09.5.1).
pub(crate) async fn recall_knowledge_hits(
    state: &AppState,
    tenant_id: &str,
    query: &str,
    top_k: usize,
    doc_title_prefix: Option<&str>,
) -> Result<(Vec<FusedHit>, Vec<String>), AppError> {
    let trimmed = query.trim();
    if trimmed.is_empty() || top_k == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let tenant_ai = state.ai.for_tenant(tenant_id);
    let (embed_res, seed_entities) = tokio::join!(
        tenant_ai.embed_one(trimmed),
        tenant_ai.extract_query_entities(trimmed)
    );

    let (q_vec, q_dim) = embed_res?;
    let seeds = seed_entities.unwrap_or_default();

    let recall_started = Instant::now();
    let (vec_res, bm25_res, ent_res, hop_res) = tokio::join!(
        state.store.recall_vector(tenant_id, q_vec, q_dim, 12),
        state.store.recall_bm25(tenant_id, trimmed, 12),
        state.store.recall_entity_link(tenant_id, &seeds, 10),
        state.store.recall_multi_hop(tenant_id, &seeds, 10)
    );
    metrics::histogram!("axpoint_recall_channel_seconds", "channel" => "parallel")
        .record(recall_started.elapsed().as_secs_f64());

    let mut all_candidates = Vec::new();
    all_candidates.extend(vec_res?);
    all_candidates.extend(bm25_res?);
    all_candidates.extend(ent_res?);
    all_candidates.extend(hop_res?);

    let seed_para_ids: Vec<Uuid> = all_candidates.iter().take(4).map(|c| c.para_id).collect();
    let ctx_res = state
        .store
        .recall_context(tenant_id, &seed_para_ids, 6)
        .await?;
    all_candidates.extend(ctx_res);

    let clean_prefix = doc_title_prefix.map(str::trim).filter(|s| !s.is_empty());
    let fetch_k = if clean_prefix.is_some() {
        top_k.saturating_mul(4).max(20)
    } else {
        top_k
    };
    let mut hits = rrf_fuse(all_candidates, fetch_k);
    if let Some(prefix) = clean_prefix {
        hits.retain(|h| h.doc_title.starts_with(prefix));
        hits.truncate(top_k);
    }
    Ok((hits, seeds))
}

async fn run_doc_rag_pipeline(
    state: &AppState,
    tenant_id: &str,
    query: &str,
    top_k: usize,
) -> Result<DocRagOutcome, AppError> {
    let (hits, seeds) = recall_knowledge_hits(state, tenant_id, query, top_k, None).await?;
    let answer = state
        .ai
        .for_tenant(tenant_id)
        .with_purpose_override(axpoint_domain::enums::Purpose::Summarize)
        .synthesize_answer(query, &hits)
        .await?;
    let citations = hits
        .iter()
        .map(|h| AskCitation {
            para_id: h.para_id,
            doc_id: h.doc_id,
            doc_title: h.doc_title.clone(),
            snippet: h.content.chars().take(240).collect(),
        })
        .collect();

    Ok(DocRagOutcome {
        answer,
        hits,
        seeds,
        citations,
    })
}

async fn resolve_auto_chat_intent(
    state: &AppState,
    tenant_id: &str,
    question: &str,
    session_id: Option<Uuid>,
) -> Result<AskIntent, AppError> {
    let (catalog, _, _, _) = load_tenant_catalog_and_live(state, tenant_id, false).await?;
    let q_embed = state
        .ai
        .embed_one(question.trim())
        .await
        .ok()
        .map(|(v, _)| v);
    let (db_recall, all_metrics) = tokio::try_join!(
        state
            .store
            .recall_schema_candidates(tenant_id, question.trim(), q_embed.as_deref(), &[]),
        state.store.list_metrics(tenant_id),
    )?;
    let session_turns = match session_id {
        Some(sid) => state.store.list_session_turns(tenant_id, sid).await?,
        None => Vec::new(),
    };
    let has_prior_data_turn = session_turns
        .iter()
        .any(|t| matches!(t.intent, AskIntent::Data | AskIntent::Hybrid));

    let linked = build_linked_schema(
        question.trim(),
        &catalog,
        &db_recall,
        &SchemaLinkExtras {
            all_metrics,
            ..SchemaLinkExtras::default()
        },
    );

    if let Some(by_rule) =
        classify_intent_by_rules(question, linked.has_positive_hit, has_prior_data_turn)
    {
        return Ok(by_rule);
    }

    Ok(route_intent(
        &state.ai,
        question,
        linked.has_positive_hit,
        has_prior_data_turn,
    )
    .await
    .unwrap_or(AskIntent::Doc))
}

fn strip_doc_citation_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut idx = 0_usize;
    while let Some(&ch) = chars.get(idx) {
        if ch == '[' && chars.get(idx + 1) == Some(&'文') {
            let mut end = idx + 2;
            while let Some(&d) = chars.get(end) {
                if d.is_ascii_digit() {
                    end += 1;
                } else {
                    break;
                }
            }
            if end > idx + 2 && chars.get(end) == Some(&']') {
                idx = end + 1;
                continue;
            }
        }
        out.push(ch);
        idx += 1;
    }
    out
}

async fn synthesize_hybrid_answer(
    state: &AppState,
    question: &str,
    doc: &DocRagOutcome,
    data_answer: &str,
    data_rows: &[serde_json::Map<String, serde_json::Value>],
) -> String {
    let fallback = if doc.citations.is_empty() {
        format!("[表] {data_answer}")
    } else {
        format!("[表] {data_answer}\n\n[文1] {}", doc.answer)
    };

    if data_rows.is_empty() || doc.citations.is_empty() {
        return fallback;
    }

    let mut prompt = String::from("### 结构化查询结果 [表]\n");
    let sample: Vec<_> = data_rows.iter().take(20).collect();
    let _fmt_ok = writeln!(
        prompt,
        "{}",
        serde_json::to_string(&sample).unwrap_or_default()
    );
    prompt.push_str("\n### 文档段落依据\n");
    for (i, c) in doc.citations.iter().take(6).enumerate() {
        let _fmt_ok = writeln!(prompt, "[文{}] 《{}》：{}", i + 1, c.doc_title, c.snippet);
    }
    let _fmt_ok = write!(prompt, "\n### 用户问题\n{}", question.trim());

    let Ok(raw) = state.ai.chat(HYBRID_SYSTEM_PROMPT, &prompt).await else {
        return fallback;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return fallback;
    }

    let stripped = strip_doc_citation_tags(trimmed);
    let checked = verify_or_fallback_summary(&stripped, data_rows);
    if checked == stripped.trim() {
        if trimmed.contains("[表]") {
            String::from(trimmed)
        } else {
            format!("[表] {trimmed}")
        }
    } else {
        fallback
    }
}

async fn handle_doc_chat_query(
    state: &AppState,
    principal: &AuthPrincipal,
    req: &ChatQueryRequest,
    started: Instant,
) -> Result<ChatQueryResponse, AppError> {
    let tenant_id = &principal.tenant_id;
    let doc = run_doc_rag_pipeline(state, tenant_id, &req.query, req.top_k).await?;
    let hit_para_ids: Vec<Uuid> = doc.hits.iter().map(|h| h.para_id).collect();
    let related_nodes = state
        .store
        .list_related_nodes(tenant_id, &hit_para_ids, &[])
        .await?;
    state.audit.record(AuditRecord {
        tenant_id: tenant_id.clone(),
        user_id: Some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::RagAsk,
        resource_ref: String::from("chat:doc"),
        status_code: 200,
        client_ip: String::from("127.0.0.1"),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({ "hit_count": doc.hits.len() }),
        created_at: Utc::now(),
    });
    Ok(ChatQueryResponse {
        answer: doc.answer,
        hits: doc.hits,
        seed_entities: doc.seeds,
        elapsed_ms: started.elapsed().as_millis(),
        intent: AskIntent::Doc,
        session_id: req.session_id,
        turn_id: None,
        citations: doc.citations,
        sql: None,
        columns: Vec::new(),
        rows: Vec::new(),
        chart: None,
        related_nodes,
    })
}

async fn handle_hybrid_chat_query(
    state: &AppState,
    principal: &AuthPrincipal,
    req: &ChatQueryRequest,
    started: Instant,
) -> Result<ChatQueryResponse, AppError> {
    let tenant_id = &principal.tenant_id;
    let ask_req = AskRequest {
        question: req.query.clone(),
        session_id: req.session_id,
        context: None,
    };
    let (doc_res, data_res) = tokio::join!(
        run_doc_rag_pipeline(state, tenant_id, &req.query, req.top_k),
        execute_ask_for_principal(state, principal, ask_req, Some(AskIntent::Data), false)
    );
    let doc = doc_res?;
    let data = data_res?;
    let answer = synthesize_hybrid_answer(state, &req.query, &doc, &data.answer, &data.rows).await;

    let preview = (!data.sql.is_empty()).then(|| {
        serde_json::json!({
            "columns": data.columns,
            "rows": data.rows,
            "truncated": data.truncated,
            "chart": data.chart,
        })
    });
    let turn = state
        .store
        .insert_ask_turn(
            tenant_id,
            NewAskTurnInput {
                session_id: data.session_id,
                user_id: Some(principal.user_id),
                question: req.query.trim().to_owned(),
                intent: AskIntent::Hybrid,
                linked_objects: data.linked_objects.clone(),
                sql_text: (!data.sql.is_empty()).then(|| data.sql.clone()),
                result_preview: preview,
                answer: answer.clone(),
                citations: doc.citations.clone(),
                model_name: String::from("axpoint-llm"),
            },
        )
        .await?;

    let hit_para_ids: Vec<Uuid> = doc.hits.iter().map(|h| h.para_id).collect();
    let related_nodes = state
        .store
        .list_related_nodes(tenant_id, &hit_para_ids, &data.linked_objects.nodes)
        .await?;

    Ok(ChatQueryResponse {
        answer,
        hits: doc.hits,
        seed_entities: doc.seeds,
        elapsed_ms: started.elapsed().as_millis(),
        intent: AskIntent::Hybrid,
        session_id: Some(data.session_id),
        turn_id: Some(turn.id),
        citations: doc.citations,
        sql: (!data.sql.is_empty()).then_some(data.sql),
        columns: data.columns,
        rows: data.rows,
        chart: data.chart,
        related_nodes,
    })
}

async fn chat_query_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<ChatQueryRequest>,
) -> Result<Json<ChatQueryResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    if req.query.trim().is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "query cannot be empty",
        )));
    }

    let tenant_id = &principal.tenant_id;
    let started = Instant::now();
    let intent = match req.mode {
        ChatQueryMode::Doc => AskIntent::Doc,
        ChatQueryMode::Data => AskIntent::Data,
        ChatQueryMode::Auto => {
            resolve_auto_chat_intent(&state, tenant_id, &req.query, req.session_id).await?
        }
    };

    match intent {
        AskIntent::Doc => Ok(Json(
            Box::pin(handle_doc_chat_query(&state, &principal, &req, started)).await?,
        )),
        AskIntent::Data | AskIntent::Chitchat => {
            let ask_resp = Box::pin(execute_ask_for_principal(
                &state,
                &principal,
                AskRequest {
                    question: req.query,
                    session_id: req.session_id,
                    context: None,
                },
                Some(intent),
                true,
            ))
            .await?;
            let related_nodes = state
                .store
                .list_related_nodes(tenant_id, &[], &ask_resp.linked_objects.nodes)
                .await?;
            Ok(Json(ChatQueryResponse {
                answer: ask_resp.answer,
                hits: Vec::new(),
                seed_entities: Vec::new(),
                elapsed_ms: started.elapsed().as_millis(),
                intent: ask_resp.intent,
                session_id: Some(ask_resp.session_id),
                turn_id: Some(ask_resp.turn_id),
                citations: ask_resp.citations,
                sql: (!ask_resp.sql.is_empty()).then_some(ask_resp.sql),
                columns: ask_resp.columns,
                rows: ask_resp.rows,
                chart: ask_resp.chart,
                related_nodes,
            }))
        }
        AskIntent::Hybrid => Ok(Json(
            Box::pin(handle_hybrid_chat_query(&state, &principal, &req, started)).await?,
        )),
    }
}
