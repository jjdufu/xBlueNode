use std::time::Instant;

use axpoint_db::{DocumentIngestPayload, ParaIngestInput};
use axpoint_domain::{
    AppError, ChatQueryRequest, ChatQueryResponse, IngestResponse, IngestTextRequest, rrf_fuse,
    split_paragraphs,
};
use axum::extract::{Multipart, State};
use axum::routing::post;
use axum::{Json, Router};
use tracing::warn;
use uuid::Uuid;

use crate::state::{AppState, HttpError, ParserSidecarResponse, default_tenant};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/documents/ingest", post(ingest_text_handler))
        .route("/api/documents/upload", post(upload_document_handler))
        .route("/api/chat/query", post(chat_query_handler))
}

async fn ingest_text_handler(
    State(state): State<AppState>,
    Json(req): Json<IngestTextRequest>,
) -> Result<Json<IngestResponse>, HttpError> {
    let resp = run_ingest_pipeline(&state, req).await?;
    Ok(Json(resp))
}

async fn upload_document_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<IngestResponse>, HttpError> {
    let mut tenant_id = default_tenant();
    let mut file_name = String::from("uploaded.pdf");
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| HttpError(AppError::BadRequest(format!("multipart error: {err}"))))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "tenant_id" {
            tenant_id = field.text().await.map_err(|err| {
                HttpError(AppError::BadRequest(format!("invalid tenant_id: {err}")))
            })?;
        } else if name == "file" {
            if let Some(orig) = field.file_name() {
                file_name = orig.to_owned();
            }
            let bytes = field.bytes().await.map_err(|err| {
                HttpError(AppError::BadRequest(format!("read file failed: {err}")))
            })?;
            file_bytes = Some(bytes.to_vec());
        }
    }

    let raw_bytes = file_bytes
        .ok_or_else(|| HttpError(AppError::BadRequest(String::from("missing 'file' field"))))?;

    let part = reqwest::multipart::Part::bytes(raw_bytes).file_name(file_name.clone());
    let form = reqwest::multipart::Form::new().part("file", part);
    let parse_url = format!("{}/parse", state.parser_base_url.trim_end_matches('/'));

    let parser_resp = state
        .http
        .post(&parse_url)
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
        IngestTextRequest {
            tenant_id,
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
    req: IngestTextRequest,
) -> Result<IngestResponse, AppError> {
    if req.content.trim().is_empty() {
        return Err(AppError::BadRequest(String::from(
            "document content is empty",
        )));
    }

    let chunks = split_paragraphs(&req.content, 900);
    if chunks.is_empty() {
        return Err(AppError::BadRequest(String::from(
            "no valid paragraphs found",
        )));
    }

    let (vectors, embed_dim) = state.ai.embed_batch(&chunks).await?;
    let mut para_inputs = Vec::with_capacity(chunks.len());

    for (idx, (chunk, vec)) in chunks.into_iter().zip(vectors).enumerate() {
        let triples = match state.ai.extract_triples(&chunk).await {
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
    let commit_res = state
        .store
        .ingest_document(DocumentIngestPayload {
            tenant_id: req.tenant_id,
            title: req.title,
            source_name: req.source_name,
            summary,
            doc_embedding: None,
            embed_model: state.ai.embedding_model_name().to_owned(),
            embed_dim,
            paras: para_inputs,
        })
        .await?;

    Ok(IngestResponse {
        doc_id: commit_res.doc_id,
        title: title_clone,
        para_count: commit_res.para_count,
        entity_count: commit_res.entity_count,
        edge_count: commit_res.edge_count,
        embedding_model: state.ai.embedding_model_name().to_owned(),
        embedding_dim: embed_dim,
    })
}

async fn chat_query_handler(
    State(state): State<AppState>,
    Json(req): Json<ChatQueryRequest>,
) -> Result<Json<ChatQueryResponse>, HttpError> {
    if req.query.trim().is_empty() {
        return Err(HttpError(AppError::BadRequest(String::from(
            "query cannot be empty",
        ))));
    }

    let started = Instant::now();

    let (embed_res, seed_entities) = tokio::join!(
        state.ai.embed_one(&req.query),
        state.ai.extract_query_entities(&req.query)
    );

    let (q_vec, q_dim) = embed_res?;
    let seeds = seed_entities.unwrap_or_default();

    let (vec_res, bm25_res, ent_res, hop_res) = tokio::join!(
        state.store.recall_vector(&req.tenant_id, q_vec, q_dim, 12),
        state.store.recall_bm25(&req.tenant_id, &req.query, 12),
        state.store.recall_entity_link(&req.tenant_id, &seeds, 10),
        state.store.recall_multi_hop(&req.tenant_id, &seeds, 10)
    );

    let mut all_candidates = Vec::new();
    all_candidates.extend(vec_res?);
    all_candidates.extend(bm25_res?);
    all_candidates.extend(ent_res?);
    all_candidates.extend(hop_res?);

    let seed_para_ids: Vec<Uuid> = all_candidates.iter().take(4).map(|c| c.para_id).collect();
    let ctx_res = state
        .store
        .recall_context(&req.tenant_id, &seed_para_ids, 6)
        .await?;
    all_candidates.extend(ctx_res);

    let hits = rrf_fuse(all_candidates, req.top_k);
    let answer = state.ai.synthesize_answer(&req.query, &hits).await?;

    Ok(Json(ChatQueryResponse {
        answer,
        hits,
        seed_entities: seeds,
        elapsed_ms: started.elapsed().as_millis(),
    }))
}
