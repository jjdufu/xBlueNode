//! Document ingestion, Sidecar upload forwarding, and hybrid `AxQuery` (`DOC` / `DATA` / `HYBRID`) routes.

use std::fmt::Write as _;
use std::time::Instant;

use axpoint_analytics::nl2sql::intent::{classify_intent_by_rules, route_intent};
use axpoint_analytics::nl2sql::schema_link::{SchemaLinkExtras, build_linked_schema};
use axpoint_analytics::{
    GroundedHybridSynthesis, HybridGroundingInput, filter_noise_recall_candidates,
    sanitize_and_ground_doc_answer, sanitize_and_ground_hybrid_answer,
};
use axpoint_db::{DocRecallScope, DocumentIngestPayload, ParaIngestInput};
use axpoint_domain::agent::EndpointSpec;
use axpoint_domain::analytics::{
    AskCitation, AskIntent, AskRequest, ChatQueryMode, LinkedObjects, NewAskTurnInput,
};
use axpoint_domain::enums::{AuditAction, GroundingMode, Permission};
use axpoint_domain::grounding::ABSTAIN_NO_DOC_EVIDENCE_ZH;
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::AnalyticsRepository as _;
use axpoint_domain::{
    AppError, ChatQueryRequest, ChatQueryResponse, ChatQueryScope, DocumentListResponse, FusedHit,
    IngestResponse, IngestTextRequest, RecallChannel, rrf_fuse, split_paragraphs,
};
use axpoint_security::AuthPrincipal;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use tracing::warn;
use uuid::Uuid;

use crate::routes::analytics::{
    ScopeAuthContext, ask_context_from_scope, authorize_and_resolve_query_scope,
    execute_ask_for_principal_with_ip, load_tenant_catalog_and_live,
    prune_catalog_and_metrics_for_scope,
};
use crate::state::{ApiJson, AppState, HttpError, ParserSidecarResponse};
use crate::telemetry::{current_trace_id, outbound_traceparent};

const HYBRID_SYSTEM_PROMPT: &str = "你是企业知识与数据分析混合问答助手。请综合给定的【结构化查询结果表】与【召回文档段落】回答用户问题。数据结论标注 [表]，文档依据标注 [文1]、[文2] 等编号；严禁编造结果表中不存在的数值。";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDocumentsQuery {
    q: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/documents", get(list_documents_handler))
        .route("/documents/{id}", delete(delete_document_handler))
        .route("/documents/ingest", post(ingest_text_handler))
        .route("/documents/upload", post(upload_document_handler))
        .route("/chat/query", post(chat_query_handler))
}

async fn list_documents_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    Query(params): Query<ListDocumentsQuery>,
) -> Result<Json<DocumentListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let offset = match params.cursor.as_deref() {
        Some(raw) if !raw.trim().is_empty() => raw
            .trim()
            .parse::<i64>()
            .map_err(|_| HttpError(AppError::bad("INVALID_ARGUMENT", "invalid cursor")))?
            .max(0),
        _ => 0,
    };
    let resp = state
        .store
        .list_documents(&principal.tenant_id, params.q.as_deref(), limit, offset)
        .await?;
    Ok(Json(resp))
}

async fn delete_document_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, HttpError> {
    principal.require(Permission::DataSync)?;
    state
        .store
        .delete_document(&principal.tenant_id, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ingest_text_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<IngestTextRequest>,
) -> Result<Json<IngestResponse>, HttpError> {
    principal.require(Permission::DataSync)?;
    let resp = run_ingest_pipeline(&state, &principal.tenant_id, principal.user_id, req).await?;
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
        principal.user_id,
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
    user_id: Uuid,
    req: IngestTextRequest,
) -> Result<IngestResponse, AppError> {
    let scoped_ai = state.ai.for_tenant(tenant_id).for_user(user_id);
    ingest_text_with_deps(
        &state.store,
        &scoped_ai,
        Some(state.sidecar.as_ref()),
        tenant_id,
        req,
    )
    .await
}

/// Runs the hypergraph text ingestion pipeline (`split_paragraphs` -> `/pii/analyze` -> `embed_batch` -> `extract_triples` -> `ingest_document`).
pub(crate) async fn ingest_text_with_deps(
    store: &axpoint_db::PgStore,
    ai: &axpoint_llm::AiGateway,
    sidecar: Option<&crate::sidecar::HttpSidecarClient>,
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

    let detected_spans: Vec<Vec<axpoint_domain::guard::PiiSpan>> = if let Some(sc) = sidecar {
        use axpoint_domain::ports::SidecarPort as _;
        match sc
            .analyze_pii(
                tenant_id,
                &axpoint_domain::guard::PiiAnalyzeRequest {
                    texts: chunks.clone(),
                    language: String::from("zh"),
                    score_threshold: 0.6,
                },
            )
            .await
        {
            Ok(resp) => resp.results,
            Err(err) => {
                warn!("sidecar pii/analyze skipped during document ingest: {err}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let tenant_ai = ai.for_tenant(tenant_id);
    let (vectors, embed_dim) = tenant_ai.embed_batch(&chunks).await?;
    let mut para_inputs = Vec::with_capacity(chunks.len());

    for (idx, (chunk, vec)) in chunks.into_iter().zip(vectors).enumerate() {
        let triples = match tenant_ai.extract_triples(&chunk).await {
            Ok(t) => t,
            Err(err) => {
                warn!("triple extraction skipped for chunk {idx}: {err}");
                Vec::new()
            }
        };
        let pii_spans = detected_spans.get(idx).cloned().unwrap_or_default();
        let seq_no = i32::try_from(idx).unwrap_or(i32::MAX);
        para_inputs.push(ParaIngestInput {
            seq_no,
            content: chunk,
            triples,
            embedding: vec,
            pii_spans,
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
    grounding_mode: GroundingMode,
    verified_citations: u32,
    abstained: bool,
}

/// Masks `hg_paras.pii_spans` character ranges across recalled [`FusedHit`] items when `has_pii_raw` is `false` (`§13.6.3`, `§13.8.2`).
pub(crate) async fn mask_recalled_hits(
    state: &AppState,
    tenant_id: &str,
    hits: &mut [FusedHit],
    has_pii_raw: bool,
) -> Result<(), AppError> {
    if has_pii_raw || hits.is_empty() {
        return Ok(());
    }
    let para_ids: Vec<Uuid> = hits.iter().map(|h| h.para_id).collect();
    let spans_map = state
        .store
        .load_para_pii_spans(tenant_id, &para_ids)
        .await?;
    if spans_map.is_empty() {
        return Ok(());
    }
    let tenant_key = state.mask_keys.tenant_key(tenant_id);
    let empty_rules = std::collections::BTreeMap::new();
    for hit in hits.iter_mut() {
        if let Some(spans) = spans_map.get(&hit.para_id)
            && !spans.is_empty()
        {
            hit.content = axpoint_security::masking::mask_text_spans(
                &hit.content,
                spans,
                &empty_rules,
                &tenant_key,
            );
        }
    }
    Ok(())
}

fn build_doc_recall_scope(
    doc_ids: &[Uuid],
    doc_title_prefix: Option<&str>,
) -> Option<DocRecallScope> {
    let clean_prefix = doc_title_prefix
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    if doc_ids.is_empty() && clean_prefix.is_none() {
        None
    } else {
        Some(DocRecallScope {
            doc_ids: doc_ids.to_vec(),
            doc_title_prefix: clean_prefix,
        })
    }
}

fn build_doc_scope_from_chat_scope(scope: Option<&ChatQueryScope>) -> Option<DocRecallScope> {
    let s = scope?;
    build_doc_recall_scope(&s.doc_ids, s.doc_title_prefix.as_deref())
}

/// Executes 5-channel hypergraph document recall (`vector + bm25 + entity_link + multi_hop + context`)
/// fused via RRF, optionally filtered by `doc_title_prefix` (`08-STEP09` §09.5.1, `06-STEP20` §20.2.2).
pub(crate) async fn recall_knowledge_hits(
    state: &AppState,
    tenant_id: &str,
    query: &str,
    top_k: usize,
    doc_title_prefix: Option<&str>,
) -> Result<(Vec<FusedHit>, Vec<String>), AppError> {
    let doc_scope = build_doc_recall_scope(&[], doc_title_prefix);
    recall_knowledge_hits_for_user(state, tenant_id, None, query, top_k, doc_scope.as_ref()).await
}

async fn recall_knowledge_hits_for_user(
    state: &AppState,
    tenant_id: &str,
    caller_user_id: Option<Uuid>,
    query: &str,
    top_k: usize,
    doc_scope: Option<&DocRecallScope>,
) -> Result<(Vec<FusedHit>, Vec<String>), AppError> {
    let trimmed = query.trim();
    if trimmed.is_empty() || top_k == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let tenant_ai = match caller_user_id {
        Some(uid) => state.ai.for_tenant(tenant_id).for_user(uid),
        None => state.ai.for_tenant(tenant_id),
    };
    let (embed_res, seed_entities) = tokio::join!(
        tenant_ai.embed_one(trimmed),
        tenant_ai.extract_query_entities(trimmed)
    );

    let (q_vec, _q_dim) = embed_res?;
    let seeds = seed_entities.unwrap_or_default();

    let recall_started = Instant::now();
    let (base_res, ent_res, hop_res) = tokio::join!(
        state
            .store
            .recall_with_scope(tenant_id, trimmed, Some(&q_vec), 12, doc_scope),
        state
            .store
            .recall_entity_link_scoped(tenant_id, &seeds, 10, doc_scope),
        state
            .store
            .recall_multi_hop_scoped(tenant_id, &seeds, 10, doc_scope),
    );
    metrics::histogram!("axpoint_recall_channel_seconds", "channel" => "parallel")
        .record(recall_started.elapsed().as_secs_f64());

    let mut non_ctx: Vec<_> = base_res?
        .into_iter()
        .filter(|c| c.channel != RecallChannel::Context)
        .collect();
    non_ctx.extend(ent_res?);
    non_ctx.extend(hop_res?);

    let mut filtered = filter_noise_recall_candidates(trimmed, non_ctx);
    if !filtered.is_empty() {
        let seed_para_ids: Vec<Uuid> = filtered.iter().take(4).map(|c| c.para_id).collect();
        let extra_ctx = state
            .store
            .recall_context_scoped(tenant_id, &seed_para_ids, 6, doc_scope)
            .await?;
        filtered.extend(extra_ctx);
    }

    let hits = rrf_fuse(filtered, top_k);
    Ok((hits, seeds))
}

async fn run_doc_rag_pipeline(
    state: &AppState,
    principal: &AuthPrincipal,
    query: &str,
    top_k: usize,
    doc_scope: Option<&DocRecallScope>,
) -> Result<DocRagOutcome, AppError> {
    let tenant_id = &principal.tenant_id;
    let has_pii_raw = principal.has(Permission::PiiRawRead);
    let (mut hits, seeds) = recall_knowledge_hits_for_user(
        state,
        tenant_id,
        Some(principal.user_id),
        query,
        top_k,
        doc_scope,
    )
    .await?;
    mask_recalled_hits(state, tenant_id, &mut hits, has_pii_raw).await?;
    if hits.is_empty() {
        return Ok(DocRagOutcome {
            answer: String::from(ABSTAIN_NO_DOC_EVIDENCE_ZH),
            hits: Vec::new(),
            seeds,
            citations: Vec::new(),
            grounding_mode: GroundingMode::Abstained,
            verified_citations: 0,
            abstained: true,
        });
    }

    let raw_answer = state
        .ai
        .for_tenant(tenant_id)
        .for_user(principal.user_id)
        .with_purpose_override(axpoint_domain::enums::Purpose::Summarize)
        .synthesize_answer(query, &hits)
        .await?;
    let citations: Vec<AskCitation> = hits
        .iter()
        .map(|h| AskCitation {
            para_id: h.para_id,
            doc_id: h.doc_id,
            doc_title: h.doc_title.clone(),
            snippet: h.content.chars().take(240).collect(),
        })
        .collect();
    let grounded = sanitize_and_ground_doc_answer(&raw_answer, &citations);

    Ok(DocRagOutcome {
        answer: grounded.answer,
        hits,
        seeds,
        citations,
        grounding_mode: grounded.grounding_mode,
        verified_citations: grounded.verified_citations,
        abstained: grounded.abstained,
    })
}

async fn resolve_auto_chat_intent(
    state: &AppState,
    principal: &AuthPrincipal,
    question: &str,
    session_id: Option<Uuid>,
    scope: Option<&ChatQueryScope>,
) -> Result<AskIntent, AppError> {
    let tenant_id = &principal.tenant_id;
    let scoped_ai = state.ai.for_tenant(tenant_id).for_user(principal.user_id);
    let (mut catalog, all_datasets, _, _) =
        load_tenant_catalog_and_live(state, tenant_id, false).await?;
    let restrict_ids: Vec<Uuid> = scope.map_or_else(Vec::new, |s| s.dataset_ids.clone());
    let subgraph_slug = scope.and_then(|s| s.subgraph_slug.as_deref());
    let q_embed = scoped_ai
        .embed_one(question.trim())
        .await
        .ok()
        .map(|(v, _)| v);
    let (db_recall, mut all_metrics) = tokio::try_join!(
        state.store.recall_schema_candidates_scoped(
            tenant_id,
            question.trim(),
            q_embed.as_deref(),
            &restrict_ids,
            subgraph_slug,
        ),
        state.store.list_metrics(tenant_id),
    )?;
    let excluded_dataset_names = prune_catalog_and_metrics_for_scope(
        state,
        tenant_id,
        &mut catalog,
        &all_datasets,
        &mut all_metrics,
        scope,
    )
    .await;

    let session_turns = match session_id {
        Some(sid) => state.store.list_session_turns(tenant_id, sid).await?,
        None => Vec::new(),
    };
    let has_prior_data_turn = session_turns
        .iter()
        .any(|t| matches!(t.intent, AskIntent::Data | AskIntent::Hybrid));

    let enforce_ontology_joins = catalog
        .published_schemas
        .iter()
        .any(|s| !s.mappings.is_empty());
    let linked = build_linked_schema(
        question.trim(),
        &catalog,
        &db_recall,
        &SchemaLinkExtras {
            all_metrics,
            restrict_dataset_ids: restrict_ids,
            excluded_dataset_names,
            published_schemas: catalog.published_schemas.clone(),
            enforce_ontology_joins,
            ..SchemaLinkExtras::default()
        },
    );
    if linked.targeted_excluded_dataset {
        return Ok(AskIntent::Data);
    }

    if let Some(by_rule) =
        classify_intent_by_rules(question, linked.has_positive_hit, has_prior_data_turn)
    {
        return Ok(by_rule);
    }

    Ok(route_intent(
        &scoped_ai,
        question,
        linked.has_positive_hit,
        has_prior_data_turn,
    )
    .await
    .unwrap_or(AskIntent::Doc))
}

async fn synthesize_hybrid_answer(
    state: &AppState,
    principal: &AuthPrincipal,
    question: &str,
    doc: &DocRagOutcome,
    data: &axpoint_domain::analytics::AskResponse,
    pinned_endpoints: &[EndpointSpec],
) -> GroundedHybridSynthesis {
    let raw_hybrid = if !data.abstained
        && !doc.abstained
        && !data.rows.is_empty()
        && !doc.citations.is_empty()
    {
        let mut prompt = String::from("### 结构化查询结果 [表]\n");
        let sample: Vec<_> = data.rows.iter().take(20).collect();
        let _fmt_ok = writeln!(
            prompt,
            "{}",
            serde_json::to_string(&sample).unwrap_or_default()
        );
        prompt.push_str("\n### 文档段落依据\n");
        for (i, c) in doc.citations.iter().take(6).enumerate() {
            let _fmt_ok = writeln!(
                prompt,
                "[文{}] 《{}》：{}",
                i.saturating_add(1),
                c.doc_title,
                c.snippet
            );
        }
        if !pinned_endpoints.is_empty() {
            prompt.push_str("\n### 已限定可用技能端点\n");
            for ep in pinned_endpoints {
                let _fmt_ok = writeln!(prompt, "- {} ({})", ep.slug, ep.title);
            }
        }
        let _fmt_ok = write!(prompt, "\n### 用户问题\n{}", question.trim());

        let scoped_ai = state
            .ai
            .for_tenant(&principal.tenant_id)
            .for_user(principal.user_id)
            .with_purpose_override(axpoint_domain::enums::Purpose::Summarize);
        scoped_ai.chat(HYBRID_SYSTEM_PROMPT, &prompt).await.ok()
    } else {
        None
    };

    sanitize_and_ground_hybrid_answer(&HybridGroundingInput {
        raw_hybrid: raw_hybrid.as_deref(),
        data_answer: &data.answer,
        data_rows: &data.rows,
        data_grounding_mode: data.grounding_mode,
        data_abstained: data.abstained,
        doc_answer: &doc.answer,
        doc_citations: &doc.citations,
        doc_abstained: doc.abstained,
    })
}

async fn precheck_chat_query_scope(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    scope: Option<&ChatQueryScope>,
) -> Result<Vec<EndpointSpec>, AppError> {
    let Some(s) = scope else {
        return Ok(Vec::new());
    };
    let subgraph_slug = s.subgraph_slug.as_deref().unwrap_or("main");
    let policy = axpoint_server::policy::build_policy_context_for_subgraph(
        &state.store,
        &state.cedar,
        principal,
        subgraph_slug,
        false,
        client_ip,
    )
    .await?;
    let (catalog, all_datasets, _, _) =
        load_tenant_catalog_and_live(state, &principal.tenant_id, false).await?;
    authorize_and_resolve_query_scope(
        state,
        principal,
        Some(s),
        &ScopeAuthContext {
            client_ip,
            catalog: &catalog,
            all_datasets: &all_datasets,
            policy: &policy,
        },
    )
    .await
}

async fn handle_doc_chat_query(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    req: &ChatQueryRequest,
    applied_scope: Option<ChatQueryScope>,
    started: Instant,
) -> Result<ChatQueryResponse, AppError> {
    let tenant_id = &principal.tenant_id;
    let doc_scope = build_doc_scope_from_chat_scope(applied_scope.as_ref());
    let doc =
        run_doc_rag_pipeline(state, principal, &req.query, req.top_k, doc_scope.as_ref()).await?;
    let session_id = req.session_id.unwrap_or_else(Uuid::new_v4);
    let turn = state
        .store
        .insert_ask_turn(
            tenant_id,
            NewAskTurnInput {
                session_id,
                user_id: Some(principal.user_id),
                question: req.query.trim().to_owned(),
                intent: AskIntent::Doc,
                linked_objects: LinkedObjects::default(),
                sql_text: None,
                result_preview: None,
                answer: doc.answer.clone(),
                citations: doc.citations.clone(),
                model_name: String::from("axpoint-llm"),
                grounding_mode: doc.grounding_mode,
                verified_numbers: 0,
                verified_citations: doc.verified_citations,
                abstained: doc.abstained,
                slot_payload: None,
            },
        )
        .await?;

    let subgraph_slug = applied_scope
        .as_ref()
        .and_then(|s| s.subgraph_slug.as_deref())
        .unwrap_or("main");
    let hit_para_ids: Vec<Uuid> = doc.hits.iter().map(|h| h.para_id).collect();
    let related_nodes = state
        .store
        .list_related_nodes_for_subgraph(tenant_id, subgraph_slug, &hit_para_ids, &[])
        .await?;
    state.audit.record(AuditRecord {
        tenant_id: tenant_id.clone(),
        user_id: Some(principal.user_id),
        username: principal.username.clone(),
        action: AuditAction::RagAsk,
        resource_ref: String::from("chat:doc"),
        status_code: 200,
        client_ip: String::from(client_ip),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "hit_count": doc.hits.len(),
            "abstained": doc.abstained,
            "grounding_mode": doc.grounding_mode,
        }),
        created_at: Utc::now(),
    });
    Ok(ChatQueryResponse {
        answer: doc.answer,
        hits: doc.hits,
        seed_entities: doc.seeds,
        elapsed_ms: started.elapsed().as_millis(),
        intent: AskIntent::Doc,
        session_id: Some(session_id),
        turn_id: Some(turn.id),
        citations: doc.citations,
        sql: None,
        columns: Vec::new(),
        rows: Vec::new(),
        chart: None,
        related_nodes,
        grounding_mode: doc.grounding_mode,
        verified_numbers: 0,
        verified_citations: doc.verified_citations,
        abstained: doc.abstained,
        slot_payload: None,
        applied_scope,
    })
}

struct HybridChatInput<'a> {
    client_ip: &'a str,
    req: &'a ChatQueryRequest,
    applied_scope: Option<ChatQueryScope>,
    pinned_endpoints: Vec<EndpointSpec>,
    started: Instant,
}

async fn persist_hybrid_turn(
    state: &AppState,
    principal: &AuthPrincipal,
    question: &str,
    doc: &DocRagOutcome,
    data: &axpoint_domain::analytics::AskResponse,
    hybrid: &GroundedHybridSynthesis,
) -> Result<Uuid, AppError> {
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
            &principal.tenant_id,
            NewAskTurnInput {
                session_id: data.session_id,
                user_id: Some(principal.user_id),
                question: question.trim().to_owned(),
                intent: AskIntent::Hybrid,
                linked_objects: data.linked_objects.clone(),
                sql_text: (!data.sql.is_empty()).then(|| data.sql.clone()),
                result_preview: preview,
                answer: hybrid.answer.clone(),
                citations: doc.citations.clone(),
                model_name: String::from("axpoint-llm"),
                grounding_mode: hybrid.grounding_mode,
                verified_numbers: hybrid.verified_numbers,
                verified_citations: hybrid.verified_citations,
                abstained: hybrid.abstained,
                slot_payload: data.slot_payload.clone(),
            },
        )
        .await?;
    Ok(turn.id)
}

async fn handle_hybrid_chat_query(
    state: &AppState,
    principal: &AuthPrincipal,
    inp: HybridChatInput<'_>,
) -> Result<ChatQueryResponse, AppError> {
    let doc_scope = build_doc_scope_from_chat_scope(inp.applied_scope.as_ref());
    let ask_req = AskRequest {
        question: inp.req.query.clone(),
        session_id: inp.req.session_id,
        context: ask_context_from_scope(inp.applied_scope.as_ref()),
    };
    let (doc_res, data_res) = tokio::join!(
        run_doc_rag_pipeline(
            state,
            principal,
            &inp.req.query,
            inp.req.top_k,
            doc_scope.as_ref()
        ),
        execute_ask_for_principal_with_ip(
            state,
            principal,
            inp.client_ip,
            ask_req,
            Some(AskIntent::Data),
            false,
        )
    );
    let doc = doc_res?;
    let data = data_res?;
    let hybrid = synthesize_hybrid_answer(
        state,
        principal,
        &inp.req.query,
        &doc,
        &data,
        &inp.pinned_endpoints,
    )
    .await;

    let turn_id =
        persist_hybrid_turn(state, principal, &inp.req.query, &doc, &data, &hybrid).await?;
    let subgraph_slug = inp
        .applied_scope
        .as_ref()
        .and_then(|s| s.subgraph_slug.as_deref())
        .unwrap_or("main");
    let hit_para_ids: Vec<Uuid> = doc.hits.iter().map(|h| h.para_id).collect();
    let related_nodes = state
        .store
        .list_related_nodes_for_subgraph(
            &principal.tenant_id,
            subgraph_slug,
            &hit_para_ids,
            &data.linked_objects.nodes,
        )
        .await?;

    Ok(ChatQueryResponse {
        answer: hybrid.answer,
        hits: doc.hits,
        seed_entities: doc.seeds,
        elapsed_ms: inp.started.elapsed().as_millis(),
        intent: AskIntent::Hybrid,
        session_id: Some(data.session_id),
        turn_id: Some(turn_id),
        citations: doc.citations,
        sql: (!data.sql.is_empty()).then_some(data.sql),
        columns: data.columns,
        rows: data.rows,
        chart: data.chart,
        related_nodes,
        grounding_mode: hybrid.grounding_mode,
        verified_numbers: hybrid.verified_numbers,
        verified_citations: hybrid.verified_citations,
        abstained: hybrid.abstained,
        slot_payload: data.slot_payload,
        applied_scope: inp.applied_scope,
    })
}

async fn handle_data_or_chitchat_query(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    req: ChatQueryRequest,
    intent: AskIntent,
    applied_scope: Option<ChatQueryScope>,
    started: Instant,
) -> Result<ChatQueryResponse, AppError> {
    let subgraph_slug = applied_scope
        .as_ref()
        .and_then(|s| s.subgraph_slug.as_deref())
        .unwrap_or("main")
        .to_owned();
    let ask_resp = Box::pin(execute_ask_for_principal_with_ip(
        state,
        principal,
        client_ip,
        AskRequest {
            question: req.query,
            session_id: req.session_id,
            context: ask_context_from_scope(applied_scope.as_ref()),
        },
        Some(intent),
        true,
    ))
    .await?;
    let related_nodes = state
        .store
        .list_related_nodes_for_subgraph(
            &principal.tenant_id,
            &subgraph_slug,
            &[],
            &ask_resp.linked_objects.nodes,
        )
        .await?;
    Ok(ChatQueryResponse {
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
        grounding_mode: ask_resp.grounding_mode,
        verified_numbers: ask_resp.verified_numbers,
        verified_citations: ask_resp.verified_citations,
        abstained: ask_resp.abstained,
        slot_payload: ask_resp.slot_payload,
        applied_scope,
    })
}

async fn chat_query_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
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

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let applied_scope = req.scope.as_ref().and_then(ChatQueryScope::normalized);
    let pinned_endpoints =
        precheck_chat_query_scope(&state, &principal, &client_ip, applied_scope.as_ref()).await?;

    let started = Instant::now();
    let intent = match req.mode {
        ChatQueryMode::Doc => AskIntent::Doc,
        ChatQueryMode::Data => AskIntent::Data,
        ChatQueryMode::Auto => {
            resolve_auto_chat_intent(
                &state,
                &principal,
                &req.query,
                req.session_id,
                applied_scope.as_ref(),
            )
            .await?
        }
    };

    match intent {
        AskIntent::Doc => Ok(Json(
            Box::pin(handle_doc_chat_query(
                &state,
                &principal,
                &client_ip,
                &req,
                applied_scope,
                started,
            ))
            .await?,
        )),
        AskIntent::Data | AskIntent::Chitchat => Ok(Json(
            handle_data_or_chitchat_query(
                &state,
                &principal,
                &client_ip,
                req,
                intent,
                applied_scope,
                started,
            )
            .await?,
        )),
        AskIntent::Hybrid => Ok(Json(
            Box::pin(handle_hybrid_chat_query(
                &state,
                &principal,
                HybridChatInput {
                    client_ip: &client_ip,
                    req: &req,
                    applied_scope,
                    pinned_endpoints,
                    started,
                },
            ))
            .await?,
        )),
    }
}
