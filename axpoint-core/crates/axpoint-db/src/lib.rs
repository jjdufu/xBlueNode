//! PostgreSQL (`pgvector` + hypergraph) persistence, five-way retrieval, and platform ports.
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub mod agent_repo;
pub mod aggregate_exec;
pub mod analytics_repo;
pub mod audit;
pub mod axhub_repo;
pub mod entity_link;
pub mod flow_repo;
pub mod graph_query;
pub mod guard_repo;
pub mod im_repo;
pub mod jobs;
pub mod knowledge_sync;
pub mod mapping_exec;
pub mod metrics;
pub mod nodes;
pub mod ops_repo;
pub mod outbox;
pub mod rollup;
pub mod rules_repo;
pub mod sim_repo;
pub mod studio_repo;
pub mod tenant;
pub mod tokens;
pub mod topology;
pub mod view_repo;

#[cfg(test)]
mod tests;

pub use jobs::enqueue_job_tx;
pub use outbox::insert_outbox_tx;
pub use tenant::begin_tenant_tx;

use axpoint_domain::guard::PiiSpan;
use axpoint_domain::{
    AppError, ChannelCandidate, DocumentListResponse, DocumentSummary, ExtractedTriple,
    RecallChannel, SystemStats, TopologyEdge, TopologyGraph, TopologyNode,
};
use chrono::Utc;
use pgvector::Vector;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Input paragraph bundle prepared for transactional hypergraph ingestion.
#[derive(Debug, Clone)]
pub struct ParaIngestInput {
    /// Zero-based sequence index within the document.
    pub seq_no: i32,
    /// Paragraph text content.
    pub content: String,
    /// Extracted knowledge triples from this paragraph.
    pub triples: Vec<ExtractedTriple>,
    /// Embedding vector for this paragraph.
    pub embedding: Vec<f32>,
    /// Detected PII spans (`11-STEP13` §13.8.2).
    pub pii_spans: Vec<PiiSpan>,
}

/// Ingestion counters returned after committing a document.
#[derive(Debug, Clone)]
pub struct IngestCommitStats {
    /// Created document UUID.
    pub doc_id: Uuid,
    /// Number of stored paragraphs.
    pub para_count: usize,
    /// Number of upserted entities.
    pub entity_count: usize,
    /// Number of created hypergraph edges.
    pub edge_count: usize,
}

/// Complete document bundle passed to [`PgStore::ingest_document`].
#[derive(Debug, Clone)]
pub struct DocumentIngestPayload {
    /// Tenant or subgraph isolation identifier.
    pub tenant_id: String,
    /// Document title.
    pub title: String,
    /// Original file or source name.
    pub source_name: String,
    /// Short document summary.
    pub summary: String,
    /// Optional document-level embedding vector.
    pub doc_embedding: Option<Vec<f32>>,
    /// Embedding model identifier.
    pub embed_model: String,
    /// Embedding vector dimension.
    pub embed_dim: usize,
    /// Paragraphs and extracted triples.
    pub paras: Vec<ParaIngestInput>,
}

/// PostgreSQL hypergraph and multi-dimension vector store.
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Creates a new [`PgStore`] wrapping a [`PgPool`].
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns a reference to the underlying PostgreSQL connection pool.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Checks database connectivity.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the health query fails.
    pub async fn ping(&self) -> Result<(), AppError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::Database(format!("database ping failed: {err}")))?;
        Ok(())
    }

    /// Persists a document, its paragraphs, entities, edges, and embeddings atomically.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if any SQL statement or transaction commit fails.
    pub async fn ingest_document(
        &self,
        payload: DocumentIngestPayload,
    ) -> Result<IngestCommitStats, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, &payload.tenant_id).await?;

        let dim_i32 = i32::try_from(payload.embed_dim).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid embedding dim: {err}"))
        })?;

        let doc_row = sqlx::query(
            "INSERT INTO hg_docs (tenant_id, title, source_name, summary)
             VALUES ($1, $2, $3, $4)
             RETURNING id",
        )
        .bind(&payload.tenant_id)
        .bind(&payload.title)
        .bind(&payload.source_name)
        .bind(&payload.summary)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert doc failed: {err}")))?;

        let doc_id: Uuid = doc_row.get("id");

        if let Some(vec_data) = payload.doc_embedding {
            sqlx::query(
                "INSERT INTO hg_embeddings (tenant_id, node_type, node_id, model_name, embedding_dim, embedding)
                 VALUES ($1, 'DOC', $2, $3, $4, $5)",
            )
            .bind(&payload.tenant_id)
            .bind(doc_id)
            .bind(&payload.embed_model)
            .bind(dim_i32)
            .bind(Vector::from(vec_data))
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("insert doc embedding failed: {err}")))?;
        }

        let mut prev_para_id: Option<Uuid> = None;
        let mut entity_count: usize = 0;
        let mut edge_count: usize = 0;
        let para_count = payload.paras.len();

        for para in payload.paras {
            let (para_id, added_entities, added_edges) = Self::insert_paragraph_tx(
                &mut tx,
                &payload.tenant_id,
                doc_id,
                prev_para_id,
                &payload.embed_model,
                dim_i32,
                para,
            )
            .await?;
            prev_para_id = Some(para_id);
            entity_count = entity_count.saturating_add(added_entities);
            edge_count = edge_count.saturating_add(added_edges);
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit tx failed: {err}")))?;

        Ok(IngestCommitStats {
            doc_id,
            para_count,
            entity_count,
            edge_count,
        })
    }

    async fn insert_paragraph_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant_id: &str,
        doc_id: Uuid,
        prev_para_id: Option<Uuid>,
        embed_model: &str,
        dim_i32: i32,
        para: ParaIngestInput,
    ) -> Result<(Uuid, usize, usize), AppError> {
        let density = i32::try_from(para.triples.len()).unwrap_or(i32::MAX);
        let pii_spans_json = serde_json::to_value(&para.pii_spans).map_err(|err| {
            AppError::Database(format!("serialize hg_paras.pii_spans failed: {err}"))
        })?;
        let para_row = sqlx::query(
            "INSERT INTO hg_paras (doc_id, tenant_id, seq_no, content, entity_density, pii_spans)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id",
        )
        .bind(doc_id)
        .bind(tenant_id)
        .bind(para.seq_no)
        .bind(&para.content)
        .bind(density)
        .bind(pii_spans_json)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert para failed: {err}")))?;

        let para_id: Uuid = para_row.get("id");

        sqlx::query(
            "INSERT INTO hg_embeddings (tenant_id, node_type, node_id, model_name, embedding_dim, embedding)
             VALUES ($1, 'PARA', $2, $3, $4, $5)",
        )
        .bind(tenant_id)
        .bind(para_id)
        .bind(embed_model)
        .bind(dim_i32)
        .bind(Vector::from(para.embedding))
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert para embedding failed: {err}")))?;

        sqlx::query(
            "INSERT INTO hg_edges (tenant_id, src_type, src_id, dst_type, dst_id, rel_type)
             VALUES ($1, 'DOC', $2, 'PARA', $3, 'CONTAINS')",
        )
        .bind(tenant_id)
        .bind(doc_id)
        .bind(para_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert CONTAINS edge failed: {err}")))?;

        let mut edge_count: usize = 1;
        if let Some(prev_id) = prev_para_id {
            sqlx::query(
                "INSERT INTO hg_edges (tenant_id, src_type, src_id, dst_type, dst_id, rel_type)
                 VALUES ($1, 'PARA', $2, 'PARA', $3, 'NEXT_PARA')",
            )
            .bind(tenant_id)
            .bind(prev_id)
            .bind(para_id)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("insert NEXT_PARA edge failed: {err}")))?;
            edge_count = edge_count.saturating_add(1);
        }

        let (entity_count, rel_edges) =
            Self::insert_para_triples_tx(tx, tenant_id, para_id, para.triples).await?;
        Ok((para_id, entity_count, edge_count.saturating_add(rel_edges)))
    }

    async fn insert_para_triples_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant_id: &str,
        para_id: Uuid,
        triples: Vec<ExtractedTriple>,
    ) -> Result<(usize, usize), AppError> {
        let mut entity_count: usize = 0;
        let mut edge_count: usize = 0;
        for triple in triples {
            let subj_id = Self::upsert_entity(
                tx,
                tenant_id,
                &triple.subject,
                &triple.subject_type,
                triple.confidence,
            )
            .await?;
            let obj_id = Self::upsert_entity(
                tx,
                tenant_id,
                &triple.object,
                &triple.object_type,
                triple.confidence,
            )
            .await?;
            entity_count = entity_count.saturating_add(2);

            for eid in [subj_id, obj_id] {
                sqlx::query(
                    "INSERT INTO hg_edges (tenant_id, src_type, src_id, dst_type, dst_id, rel_type)
                     VALUES ($1, 'PARA', $2, 'ENTITY', $3, 'MENTIONS')",
                )
                .bind(tenant_id)
                .bind(para_id)
                .bind(eid)
                .execute(&mut **tx)
                .await
                .map_err(|err| AppError::Database(format!("insert MENTIONS edge failed: {err}")))?;
                edge_count = edge_count.saturating_add(1);
            }

            sqlx::query(
                "INSERT INTO hg_edges (tenant_id, src_type, src_id, dst_type, dst_id, rel_type, fact, weight)
                 VALUES ($1, 'ENTITY', $2, 'ENTITY', $3, 'RELATES', $4, $5)",
            )
            .bind(tenant_id)
            .bind(subj_id)
            .bind(obj_id)
            .bind(&triple.fact)
            .bind(triple.confidence)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("insert RELATES edge failed: {err}")))?;
            edge_count = edge_count.saturating_add(1);
        }

        Ok((entity_count, edge_count))
    }

    async fn upsert_entity(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tenant_id: &str,
        name: &str,
        entity_type: &str,
        confidence: f64,
    ) -> Result<Uuid, AppError> {
        let trimmed_name = name.trim();
        let name_norm = axpoint_domain::graph::normalize_name(trimmed_name);
        let row = sqlx::query(
            "INSERT INTO hg_entities (tenant_id, name, name_norm, entity_type, occur_count, confidence)
             VALUES ($1, $2, $3, $4, 1, $5)
             ON CONFLICT (tenant_id, name, entity_type)
             DO UPDATE SET
                name_norm = EXCLUDED.name_norm,
                occur_count = hg_entities.occur_count + 1,
                confidence = GREATEST(hg_entities.confidence, EXCLUDED.confidence),
                updated_at = NOW()
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(name_norm)
        .bind(entity_type.trim())
        .bind(confidence)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert entity failed: {err}")))?;

        Ok(row.get("id"))
    }

    /// Channel 1: Dense vector similarity recall filtered by `embedding_dim`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the vector query fails.
    pub async fn recall_vector(
        &self,
        tenant_id: &str,
        query_vec: Vec<f32>,
        dim: usize,
        limit: i64,
    ) -> Result<Vec<ChannelCandidate>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let dim_i32 = i32::try_from(dim).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid embedding dim: {err}"))
        })?;

        let rows = sqlx::query(
            "SELECT p.id AS para_id, p.doc_id, d.title AS doc_title, p.content,
                    1.0 - (e.embedding <=> $2) AS sim
             FROM hg_embeddings e
             JOIN hg_paras p ON p.id = e.node_id
             JOIN hg_docs d ON d.id = p.doc_id
             WHERE e.tenant_id = $1
               AND e.node_type = 'PARA'
               AND e.embedding_dim = $3
             ORDER BY e.embedding <=> $2 ASC
             LIMIT $4",
        )
        .bind(tenant_id)
        .bind(Vector::from(query_vec))
        .bind(dim_i32)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("vector recall failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit vector recall tx failed: {err}")))?;

        Ok(rows_to_candidates(&rows, RecallChannel::Vector, |r| {
            let sim: f64 = r.try_get("sim").unwrap_or(0.0);
            Some(format!("cosine_sim={sim:.4}"))
        }))
    }

    /// Channel 2: Lexical BM25 / PostgreSQL `tsvector` + CJK `ILIKE` recall (including document titles and subword bigrams).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the full-text query fails.
    pub async fn recall_bm25(
        &self,
        tenant_id: &str,
        query_text: &str,
        limit: i64,
    ) -> Result<Vec<ChannelCandidate>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let trimmed = query_text.trim();
        let like_pattern = format!("%{trimmed}%");
        let sub_patterns = build_lexical_subpatterns(trimmed);
        let rows = sqlx::query(
            "SELECT p.id AS para_id, p.doc_id, d.title AS doc_title, p.content,
                    (
                        ts_rank_cd(p.fts, websearch_to_tsquery('simple', $2))
                        + ts_rank_cd(d.fts, websearch_to_tsquery('simple', $2)) * 1.5
                        + CASE WHEN d.title ILIKE $3 THEN 4.0::real ELSE 0.0::real END
                        + CASE WHEN p.content ILIKE $3 THEN 2.0::real ELSE 0.0::real END
                        + COALESCE((
                            SELECT COUNT(*)::real * 0.6::real
                            FROM unnest($4::text[]) AS pat
                            WHERE d.title ILIKE pat OR p.content ILIKE pat
                        ), 0.0::real)
                    ) AS score
             FROM hg_paras p
             JOIN hg_docs d ON d.id = p.doc_id
             WHERE p.tenant_id = $1
               AND (
                   p.fts @@ websearch_to_tsquery('simple', $2)
                   OR d.fts @@ websearch_to_tsquery('simple', $2)
                   OR p.content ILIKE $3
                   OR d.title ILIKE $3
                   OR (cardinality($4::text[]) > 0 AND (p.content ILIKE ANY($4) OR d.title ILIKE ANY($4)))
               )
             ORDER BY score DESC, p.entity_density DESC, p.seq_no ASC
             LIMIT $5",
        )
        .bind(tenant_id)
        .bind(trimmed)
        .bind(like_pattern)
        .bind(&sub_patterns)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("bm25 recall failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit bm25 recall tx failed: {err}")))?;

        Ok(rows_to_candidates(&rows, RecallChannel::Bm25, |r| {
            let score: f32 = r.try_get("score").unwrap_or(0.0);
            Some(format!("bm25_rank={score:.4}"))
        }))
    }

    /// Channel 3: Cross-document entity-bridge recall.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the entity link query fails.
    pub async fn recall_entity_link(
        &self,
        tenant_id: &str,
        seed_entities: &[String],
        limit: i64,
    ) -> Result<Vec<ChannelCandidate>, AppError> {
        if seed_entities.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT DISTINCT ON (p.id) p.id AS para_id, p.doc_id, d.title AS doc_title, p.content,
                    ent.name AS matched_entity, ent.occur_count
             FROM hg_entities ent
             JOIN hg_edges ed ON ed.dst_id = ent.id AND ed.rel_type = 'MENTIONS'
             JOIN hg_paras p ON p.id = ed.src_id
             JOIN hg_docs d ON d.id = p.doc_id
             WHERE ent.tenant_id = $1
               AND ent.name = ANY($2)
             ORDER BY p.id, ent.occur_count DESC
             LIMIT $3",
        )
        .bind(tenant_id)
        .bind(seed_entities)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("entity_link recall failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit entity_link recall tx failed: {err}"))
        })?;

        Ok(rows_to_candidates(&rows, RecallChannel::EntityLink, |r| {
            let name: String = r.try_get("matched_entity").unwrap_or_default();
            Some(format!("seed_entity={name}"))
        }))
    }

    /// Channel 4: Adjacent paragraph (`NEXT_PARA`) context expansion.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the context expansion query fails.
    pub async fn recall_context(
        &self,
        tenant_id: &str,
        seed_para_ids: &[Uuid],
        limit: i64,
    ) -> Result<Vec<ChannelCandidate>, AppError> {
        if seed_para_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT DISTINCT p.id AS para_id, p.doc_id, d.title AS doc_title, p.content
             FROM hg_edges ed
             JOIN hg_paras p ON (p.id = ed.dst_id OR p.id = ed.src_id)
             JOIN hg_docs d ON d.id = p.doc_id
             WHERE ed.tenant_id = $1
               AND ed.rel_type = 'NEXT_PARA'
               AND (ed.src_id = ANY($2) OR ed.dst_id = ANY($2))
               AND NOT (p.id = ANY($2))
             LIMIT $3",
        )
        .bind(tenant_id)
        .bind(seed_para_ids)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("context recall failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit context recall tx failed: {err}")))?;

        Ok(rows_to_candidates(&rows, RecallChannel::Context, |_| {
            Some(String::from("adjacent_para_context"))
        }))
    }

    /// Channel 5: Multi-hop (`RELATES`) graph path reasoning recall.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the multi-hop traversal query fails.
    pub async fn recall_multi_hop(
        &self,
        tenant_id: &str,
        seed_entities: &[String],
        limit: i64,
    ) -> Result<Vec<ChannelCandidate>, AppError> {
        if seed_entities.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT DISTINCT ON (p.id) p.id AS para_id, p.doc_id, d.title AS doc_title, p.content,
                    e1.name AS src_name, rel.fact, e2.name AS dst_name
             FROM hg_entities e1
             JOIN hg_edges rel ON (rel.src_id = e1.id OR rel.dst_id = e1.id) AND rel.rel_type = 'RELATES'
             JOIN hg_entities e2 ON e2.id = CASE WHEN rel.src_id = e1.id THEN rel.dst_id ELSE rel.src_id END
             JOIN hg_edges m ON m.dst_id = e2.id AND m.rel_type = 'MENTIONS'
             JOIN hg_paras p ON p.id = m.src_id
             JOIN hg_docs d ON d.id = p.doc_id
             WHERE e1.tenant_id = $1
               AND e1.name = ANY($2)
             ORDER BY p.id, rel.weight DESC
             LIMIT $3",
        )
        .bind(tenant_id)
        .bind(seed_entities)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("multi_hop recall failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit multi_hop recall tx failed: {err}"))
        })?;

        Ok(rows_to_candidates(&rows, RecallChannel::MultiHop, |r| {
            let s: String = r.try_get("src_name").unwrap_or_default();
            let o: String = r.try_get("dst_name").unwrap_or_default();
            let f: String = r.try_get("fact").unwrap_or_default();
            Some(format!("{s} -> {o}: {f}"))
        }))
    }

    /// Loads stored `hg_paras.pii_spans` JSONB arrays for a batch of paragraph IDs (`11-STEP13` §13.6.3, §13.8.2).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `hg_paras` fails.
    pub async fn load_para_pii_spans(
        &self,
        tenant_id: &str,
        para_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, Vec<PiiSpan>>, AppError> {
        if para_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, pii_spans
               FROM hg_paras
              WHERE tenant_id = $1
                AND id = ANY($2)",
        )
        .bind(tenant_id)
        .bind(para_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load hg_paras.pii_spans failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit load_para_pii_spans tx failed: {err}"))
        })?;

        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.get("id");
            let raw: serde_json::Value = row.get("pii_spans");
            let spans: Vec<PiiSpan> = serde_json::from_value(raw).unwrap_or_default();
            if !spans.is_empty() {
                map.insert(id, spans);
            }
        }
        Ok(map)
    }

    /// Returns hypergraph counts and active embedding dimensions for the dashboard.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if any aggregation query fails.
    pub async fn get_stats(&self, tenant_id: &str) -> Result<SystemStats, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let counts = sqlx::query(
            "SELECT
                (SELECT COUNT(*) FROM hg_docs WHERE tenant_id = $1) AS doc_count,
                (SELECT COUNT(*) FROM hg_paras WHERE tenant_id = $1) AS para_count,
                (SELECT COUNT(*) FROM hg_entities WHERE tenant_id = $1) AS entity_count,
                (SELECT COUNT(*) FROM hg_edges WHERE tenant_id = $1) AS edge_count,
                (SELECT COUNT(*) FROM hg_embeddings WHERE tenant_id = $1) AS embedding_count",
        )
        .bind(tenant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("stats query failed: {err}")))?;

        let dim_rows = sqlx::query(
            "SELECT DISTINCT embedding_dim FROM hg_embeddings WHERE tenant_id = $1 ORDER BY embedding_dim",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("dim query failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit stats tx failed: {err}")))?;

        let embedding_dims = dim_rows.iter().map(|r| r.get("embedding_dim")).collect();

        Ok(SystemStats {
            doc_count: counts.get("doc_count"),
            para_count: counts.get("para_count"),
            entity_count: counts.get("entity_count"),
            edge_count: counts.get("edge_count"),
            embedding_count: counts.get("embedding_count"),
            embedding_dims,
            timestamp: Utc::now(),
        })
    }

    /// Returns a subgraph of nodes and edges for the `xBlueNode` topology explorer.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the topology query fails.
    pub async fn get_topology(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> Result<TopologyGraph, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let doc_rows = sqlx::query(
            "SELECT id, title FROM hg_docs WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 20",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("topology docs failed: {err}")))?;

        let ent_rows = sqlx::query(
            "SELECT id, name, occur_count FROM hg_entities WHERE tenant_id = $1 ORDER BY occur_count DESC LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("topology entities failed: {err}")))?;

        let edge_rows = sqlx::query(
            "SELECT id, src_id, dst_id, rel_type, fact FROM hg_edges
             WHERE tenant_id = $1 AND rel_type = 'RELATES'
             ORDER BY occur_count DESC LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("topology edges failed: {err}")))?;

        let doc_ids: Vec<Uuid> = doc_rows.iter().map(|r| r.get("id")).collect();
        let ent_ids: Vec<Uuid> = ent_rows.iter().map(|r| r.get("id")).collect();
        let mention_rows = sqlx::query(
            "SELECT DISTINCT ON (p.doc_id, e.dst_id)
                    e.id, p.doc_id AS src_id, e.dst_id, 'MENTIONS'::text AS rel_type, d.title AS fact
             FROM hg_paras p
             JOIN hg_docs d ON d.id = p.doc_id
             JOIN hg_edges e ON e.tenant_id = p.tenant_id AND e.src_id = p.id AND e.rel_type = 'MENTIONS'
             WHERE p.tenant_id = $1
               AND p.doc_id = ANY($2)
               AND e.dst_id = ANY($3)
             LIMIT $4",
        )
        .bind(tenant_id)
        .bind(&doc_ids)
        .bind(&ent_ids)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("topology mention edges failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit topology tx failed: {err}")))?;

        let mut nodes = Vec::new();
        for r in doc_rows {
            nodes.push(TopologyNode {
                id: r.get("id"),
                node_type: String::from("DOC"),
                label: r.get("title"),
                weight: 10,
            });
        }
        for r in ent_rows {
            nodes.push(TopologyNode {
                id: r.get("id"),
                node_type: String::from("ENTITY"),
                label: r.get("name"),
                weight: r.get("occur_count"),
            });
        }

        let mut edges: Vec<TopologyEdge> = edge_rows
            .into_iter()
            .chain(mention_rows)
            .map(|r| TopologyEdge {
                id: r.get("id"),
                source: r.get("src_id"),
                target: r.get("dst_id"),
                rel_type: r.get("rel_type"),
                fact: r.get("fact"),
            })
            .collect();
        edges.truncate(usize::try_from(limit.saturating_mul(2)).unwrap_or(120));

        Ok(TopologyGraph { nodes, edges })
    }

    /// Lists ingested knowledge documents (`hg_docs`) with paragraph and entity counts.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the query fails.
    pub async fn list_documents(
        &self,
        tenant_id: &str,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<DocumentListResponse, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let pattern = query
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"));

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hg_docs
             WHERE tenant_id = $1
               AND ($2::text IS NULL OR title ILIKE $2 OR source_name ILIKE $2 OR summary ILIKE $2)",
        )
        .bind(tenant_id)
        .bind(pattern.as_deref())
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("count hg_docs failed: {err}")))?;

        let rows = sqlx::query(
            "SELECT d.id, d.title, d.source_name, d.summary, d.created_at, d.updated_at,
                    (SELECT COUNT(*) FROM hg_paras p WHERE p.tenant_id = d.tenant_id AND p.doc_id = d.id) AS para_count,
                    (SELECT COUNT(DISTINCT e.dst_id)
                     FROM hg_paras p
                     JOIN hg_edges e ON e.tenant_id = p.tenant_id AND e.src_id = p.id AND e.rel_type = 'MENTIONS'
                     WHERE p.tenant_id = d.tenant_id AND p.doc_id = d.id) AS entity_count
             FROM hg_docs d
             WHERE d.tenant_id = $1
               AND ($2::text IS NULL OR d.title ILIKE $2 OR d.source_name ILIKE $2 OR d.summary ILIKE $2)
             ORDER BY d.created_at DESC, d.id DESC
             LIMIT $3 OFFSET $4",
        )
        .bind(tenant_id)
        .bind(pattern.as_deref())
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list hg_docs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list hg_docs tx failed: {err}")))?;

        let items: Vec<DocumentSummary> = rows
            .into_iter()
            .map(|r| DocumentSummary {
                id: r.get("id"),
                title: r.get("title"),
                source_name: r.get("source_name"),
                summary: r.get("summary"),
                para_count: r.get("para_count"),
                entity_count: r.get("entity_count"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect();

        let next_offset = offset.saturating_add(i64::try_from(items.len()).unwrap_or(0));
        let next_cursor = if next_offset < total && !items.is_empty() {
            Some(next_offset.to_string())
        } else {
            None
        };

        Ok(DocumentListResponse {
            items,
            total,
            next_cursor,
        })
    }

    /// Deletes an ingested knowledge document (`hg_docs`) along with its paragraph embeddings and edges.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] if the document does not exist in the tenant, or [`AppError::Database`] on SQL failure.
    pub async fn delete_document(&self, tenant_id: &str, doc_id: Uuid) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let para_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM hg_paras WHERE tenant_id = $1 AND doc_id = $2")
                .bind(tenant_id)
                .bind(doc_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("fetch doc paras failed: {err}")))?;

        sqlx::query(
            "DELETE FROM hg_embeddings
             WHERE tenant_id = $1 AND (node_id = $2 OR node_id = ANY($3))",
        )
        .bind(tenant_id)
        .bind(doc_id)
        .bind(&para_ids)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete doc embeddings failed: {err}")))?;

        sqlx::query(
            "DELETE FROM hg_edges
             WHERE tenant_id = $1
               AND (src_id = $2 OR dst_id = $2 OR src_id = ANY($3) OR dst_id = ANY($3))",
        )
        .bind(tenant_id)
        .bind(doc_id)
        .bind(&para_ids)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete doc edges failed: {err}")))?;

        let deleted = sqlx::query("DELETE FROM hg_docs WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(doc_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete hg_doc failed: {err}")))?;

        if deleted.rows_affected() == 0 {
            return Err(AppError::NotFound("NOT_FOUND"));
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete hg_doc tx failed: {err}")))?;
        Ok(())
    }
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
    )
}

fn build_lexical_subpatterns(query_text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push_unique = |term: &str| {
        if out.len() >= 24 {
            return;
        }
        let pat = format!("%{term}%");
        if !out.contains(&pat) {
            out.push(pat);
        }
    };

    for token in query_text.split(|c: char| !c.is_alphanumeric() && !is_cjk_char(c)) {
        let chars: Vec<char> = token.chars().collect();
        if chars.len() >= 2 && chars.iter().all(char::is_ascii_alphanumeric) {
            push_unique(token);
        }
        let cjk_run: Vec<char> = chars.into_iter().filter(|c| is_cjk_char(*c)).collect();
        for pair in cjk_run.windows(2) {
            let bi: String = pair.iter().collect();
            push_unique(&bi);
        }
    }
    out
}

fn rows_to_candidates<F>(
    rows: &[sqlx::postgres::PgRow],
    channel: RecallChannel,
    trace_fn: F,
) -> Vec<ChannelCandidate>
where
    F: Fn(&sqlx::postgres::PgRow) -> Option<String>,
{
    rows.iter()
        .enumerate()
        .map(|(idx, row)| {
            let rank = u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX);
            ChannelCandidate {
                para_id: row.get("para_id"),
                doc_id: row.get("doc_id"),
                doc_title: row.get("doc_title"),
                content: row.get("content"),
                channel,
                rank,
                trace: trace_fn(row),
            }
        })
        .collect()
}

impl axpoint_domain::ports::HypergraphQueryPort for PgStore {
    async fn ping(&self) -> Result<(), AppError> {
        Self::ping(self).await
    }

    async fn get_stats(&self, tenant_id: &str) -> Result<SystemStats, AppError> {
        Self::get_stats(self, tenant_id).await
    }

    async fn get_topology(&self, tenant_id: &str, limit: i64) -> Result<TopologyGraph, AppError> {
        Self::get_topology(self, tenant_id, limit).await
    }
}

impl axpoint_domain::ports::StudioExecutionPort for PgStore {
    async fn execute_mapping_materialize<L: axpoint_domain::ports::LakeInternalQueryPort>(
        &self,
        tenant_id: &str,
        job_id: Uuid,
        payload: &axpoint_domain::rollup::MapMaterializeJobPayload,
        lake: &L,
    ) -> Result<axpoint_domain::studio::MappingRunRecord, AppError> {
        mapping_exec::execute_mapping_materialize_impl(self, tenant_id, job_id, payload, lake).await
    }

    async fn list_enabled_main_mappings_for_dataset(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<Vec<axpoint_domain::studio::MappingRecord>, AppError> {
        mapping_exec::list_enabled_main_mappings_for_dataset_impl(self, tenant_id, dataset_id).await
    }

    async fn execute_graph_aggregate(
        &self,
        tenant_id: &str,
        payload: &axpoint_domain::rollup::GraphAggregateJobPayload,
    ) -> Result<serde_json::Value, AppError> {
        aggregate_exec::execute_graph_aggregate_impl(self, tenant_id, payload).await
    }

    async fn enqueue_graph_aggregate(
        &self,
        tenant_id: &str,
        payload: axpoint_domain::rollup::GraphAggregateJobPayload,
        idempotency_key: Option<String>,
    ) -> Result<Uuid, AppError> {
        aggregate_exec::enqueue_graph_aggregate_impl(self, tenant_id, payload, idempotency_key)
            .await
    }

    async fn explain_node_metric(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        metric: &str,
    ) -> Result<axpoint_domain::rollup::MetricExplainResponse, AppError> {
        aggregate_exec::explain_node_metric_impl(self, tenant_id, node_id, metric).await
    }

    async fn execute_knowledge_sync<E: axpoint_domain::ports::TextEmbedderPort>(
        &self,
        tenant_id: &str,
        payload: &axpoint_domain::rollup::KnowledgeSyncJobPayload,
        embedder: &E,
    ) -> Result<serde_json::Value, AppError> {
        knowledge_sync::execute_knowledge_sync_impl(self, tenant_id, payload, embedder).await
    }

    async fn list_knowledge_node_candidates(
        &self,
        tenant_id: &str,
    ) -> Result<axpoint_domain::rollup::KnowledgeNodeCandidatesResponse, AppError> {
        knowledge_sync::list_knowledge_node_candidates_impl(self, tenant_id).await
    }

    async fn accept_knowledge_node_candidate(
        &self,
        tenant_id: &str,
        entity_id: Uuid,
        req: axpoint_domain::rollup::AcceptNodeCandidateRequest,
    ) -> Result<axpoint_domain::rollup::AcceptNodeCandidateResponse, AppError> {
        knowledge_sync::accept_knowledge_node_candidate_impl(self, tenant_id, entity_id, req).await
    }
}
