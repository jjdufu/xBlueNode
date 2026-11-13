//! Document ingestion, Sidecar upload forwarding, and five-way hybrid RAG query routes.

use std::time::Instant;

use axpoint_db::{DocumentIngestPayload, ParaIngestInput};
use axpoint_domain::enums::Permission;
use axpoint_domain::{
    AppError, ChatQueryRequest, ChatQueryResponse, IngestResponse, IngestTextRequest, rrf_fuse,
    split_paragraphs,
};
use axpoint_security::AuthPrincipal;
use axum::extract::{Multipart, State};
use axum::routing::post;
use axum::{Json, Router};
use tracing::warn;
use uuid::Uuid;

use crate::state::{ApiJson, AppState, HttpError, ParserSidecarResponse};
use crate::telemetry::outbound_traceparent;

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

    let _ = axpoint_domain::ports::GraphStorePort::resolve_entity_links(
        store,
        tenant_id,
        axpoint_domain::graph::ResolveEntitiesRequest::default(),
    )
    .await;

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

    let (embed_res, seed_entities) = tokio::join!(
        state.ai.embed_one(&req.query),
        state.ai.extract_query_entities(&req.query)
    );

    let (q_vec, q_dim) = embed_res?;
    let seeds = seed_entities.unwrap_or_default();

    let recall_started = Instant::now();
    let (vec_res, bm25_res, ent_res, hop_res) = tokio::join!(
        state.store.recall_vector(tenant_id, q_vec, q_dim, 12),
        state.store.recall_bm25(tenant_id, &req.query, 12),
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

    let hits = rrf_fuse(all_candidates, req.top_k);
    let answer = state.ai.synthesize_answer(&req.query, &hits).await?;
    let citations = hits
        .iter()
        .map(|h| axpoint_domain::analytics::AskCitation {
            para_id: h.para_id,
            doc_id: h.doc_id,
            doc_title: h.doc_title.clone(),
            snippet: h.content.chars().take(240).collect(),
        })
        .collect();

    Ok(Json(ChatQueryResponse {
        answer,
        hits,
        seed_entities: seeds,
        elapsed_ms: started.elapsed().as_millis(),
        intent: axpoint_domain::analytics::AskIntent::Doc,
        session_id: req.session_id,
        turn_id: None,
        citations,
        sql: None,
        columns: Vec::new(),
        rows: Vec::new(),
        chart: None,
        related_nodes: Vec::new(),
    }))
}
