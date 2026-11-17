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
pub mod jobs;
pub mod knowledge_sync;
pub mod mapping_exec;
pub mod metrics;
pub mod nodes;
pub mod outbox;
pub mod rollup;
pub mod rules_repo;
pub mod studio_repo;
pub mod tenant;
pub mod tokens;
pub mod topology;

#[cfg(test)]
mod tests;

pub use jobs::enqueue_job_tx;
pub use outbox::insert_outbox_tx;
pub use tenant::begin_tenant_tx;

use axpoint_domain::{
    AppError, ChannelCandidate, ExtractedTriple, RecallChannel, SystemStats, TopologyEdge,
    TopologyGraph, TopologyNode,
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
        let para_row = sqlx::query(
            "INSERT INTO hg_paras (doc_id, tenant_id, seq_no, content, entity_density)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id",
        )
        .bind(doc_id)
        .bind(tenant_id)
        .bind(para.seq_no)
        .bind(&para.content)
        .bind(density)
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

        let mut entity_count: usize = 0;
        for triple in para.triples {
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

        Ok((para_id, entity_count, edge_count))
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

    /// Channel 2: Lexical BM25 / PostgreSQL `tsvector` + CJK `ILIKE` recall.
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
        let like_pattern = format!("%{}%", query_text.trim());
        let rows = sqlx::query(
            "SELECT p.id AS para_id, p.doc_id, d.title AS doc_title, p.content,
                    ts_rank_cd(p.fts, websearch_to_tsquery('simple', $2)) AS score
             FROM hg_paras p
             JOIN hg_docs d ON d.id = p.doc_id
             WHERE p.tenant_id = $1
               AND (p.fts @@ websearch_to_tsquery('simple', $2) OR p.content ILIKE $3)
             ORDER BY score DESC, p.entity_density DESC
             LIMIT $4",
        )
        .bind(tenant_id)
        .bind(query_text)
        .bind(like_pattern)
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

        let edges = edge_rows
            .into_iter()
            .map(|r| TopologyEdge {
                id: r.get("id"),
                source: r.get("src_id"),
                target: r.get("dst_id"),
                rel_type: r.get("rel_type"),
                fact: r.get("fact"),
            })
            .collect();

        Ok(TopologyGraph { nodes, edges })
    }
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
