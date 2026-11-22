//! Entity normalization and `hg_entities <-> xb_nodes` linking (`xb_entity_links`) for Step 03 (`AxGraph`).

use std::collections::HashMap;

use axpoint_domain::AppError;
use axpoint_domain::graph::{
    EntityCandidatesResponse, EntityLink, EntityMatchMethod, NodeDocumentHit,
    NodeDocumentsResponse, ResolveEntitiesRequest, ResolveEntitiesResponse, normalize_name,
};
use pgvector::Vector;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

#[derive(Debug, Clone)]
struct EntityRow {
    id: Uuid,
    norm_name: String,
    norm_synonyms: Vec<String>,
}

#[derive(Debug, Clone)]
struct NodeSummaryRow {
    id: Uuid,
    norm_key: String,
    norm_title: String,
}

#[derive(Debug, Clone)]
struct ExistingLinkState {
    match_method: EntityMatchMethod,
    confirmed: bool,
    rejected: bool,
}

#[derive(Debug, Clone)]
struct ProposedLink {
    entity_id: Uuid,
    node_id: Uuid,
    method: EntityMatchMethod,
    confidence: f64,
    confirmed: bool,
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (&va, &vb) in a.iter().zip(b.iter()) {
        let fa = f64::from(va);
        let fb = f64::from(vb);
        dot += fa * fb;
        norm_a += fa * fa;
        norm_b += fb * fb;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom <= f64::EPSILON || !denom.is_finite() {
        return None;
    }
    Some((dot / denom).clamp(-1.0, 1.0))
}

fn decode_entity_link(row: &sqlx::postgres::PgRow) -> Result<EntityLink, AppError> {
    let method = EntityMatchMethod::parse(&row.get::<String, _>("match_method"))?;
    Ok(EntityLink {
        entity_id: row.get("entity_id"),
        entity_name: row.get("entity_name"),
        node_id: row.get("node_id"),
        node_key: row.get("node_key"),
        node_title: row.get("node_title"),
        match_method: method,
        confidence: row.get("confidence"),
        confirmed: row.get("confirmed"),
        rejected: row.get("rejected"),
        created_at: row.get("created_at"),
    })
}

pub(crate) async fn list_node_documents_impl(
    store: &PgStore,
    tenant_id: &str,
    node_id: Uuid,
) -> Result<NodeDocumentsResponse, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let node_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xb_nodes WHERE id = $1 AND subgraph_slug = 'main')",
    )
    .bind(node_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check xb_nodes exists failed: {err}")))?;

    if !node_exists {
        return Err(AppError::NotFound("NODE_NOT_FOUND"));
    }

    let rows = sqlx::query(
        "SELECT d.id AS doc_id, d.title, d.source_name,
                ent.id AS entity_id, ent.name AS entity_name,
                p.content AS snippet
           FROM xb_entity_links el
           JOIN hg_entities ent ON ent.id = el.entity_id
           JOIN hg_edges ed
             ON ed.dst_id = ent.id
            AND ed.rel_type = 'MENTIONS'
            AND ed.src_type = 'PARA'
            AND ed.dst_type = 'ENTITY'
           JOIN hg_paras p ON p.id = ed.src_id
           JOIN hg_docs d ON d.id = p.doc_id
          WHERE el.node_id = $1
            AND el.confirmed = TRUE
            AND el.rejected = FALSE
          ORDER BY d.created_at DESC, ent.name ASC, p.seq_no ASC",
    )
    .bind(node_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("query node documents failed: {err}")))?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit list_node_documents tx failed: {err}"))
    })?;

    let mut index_by_key: HashMap<(Uuid, Uuid), usize> = HashMap::new();
    let mut items: Vec<NodeDocumentHit> = Vec::new();

    for row in rows {
        let doc_id: Uuid = row.get("doc_id");
        let entity_id: Uuid = row.get("entity_id");
        let snippet: String = row.get("snippet");
        let key = (doc_id, entity_id);

        if let Some(&idx) = index_by_key.get(&key) {
            if let Some(hit) = items.get_mut(idx)
                && !hit.snippets.contains(&snippet)
                && hit.snippets.len() < 10
            {
                hit.snippets.push(snippet);
            }
        } else {
            let idx = items.len();
            index_by_key.insert(key, idx);
            items.push(NodeDocumentHit {
                doc_id,
                title: row.get("title"),
                source_name: row.get("source_name"),
                entity_id,
                entity_name: row.get("entity_name"),
                snippets: vec![snippet],
            });
        }
    }

    Ok(NodeDocumentsResponse { items })
}

pub(crate) async fn resolve_entity_links_impl(
    store: &PgStore,
    tenant_id: &str,
    req: ResolveEntitiesRequest,
) -> Result<ResolveEntitiesResponse, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    if let Some(target_nid) = req.node_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM xb_nodes WHERE id = $1 AND subgraph_slug = 'main')",
        )
        .bind(target_nid)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("verify target node failed: {err}")))?;
        if !exists {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }
    }

    let entities = load_entities_tx(&mut tx).await?;
    let nodes = load_nodes_summary_tx(&mut tx).await?;
    if entities.is_empty() || nodes.is_empty() {
        let pending_candidates = count_pending_candidates_tx(&mut tx).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit empty resolve_entity_links tx: {err}"))
        })?;
        return Ok(ResolveEntitiesResponse {
            resolved: 0,
            auto_confirmed: 0,
            pending_candidates,
        });
    }

    let existing_map = load_existing_links_tx(&mut tx).await?;
    let (entity_embs, node_embs) = load_entity_and_node_embeddings_tx(&mut tx).await?;

    let proposals = compute_proposals(
        &entities,
        &nodes,
        req.node_id,
        &existing_map,
        &entity_embs,
        &node_embs,
    );

    let mut resolved: usize = 0;
    let mut auto_confirmed: usize = 0;

    for prop in proposals {
        sqlx::query(
            "INSERT INTO xb_entity_links (
                 tenant_id, entity_id, node_id, match_method, confidence, confirmed, rejected
             )
             VALUES ($1, $2, $3, $4, $5, $6, FALSE)
             ON CONFLICT (entity_id, node_id) DO UPDATE SET
                 match_method = EXCLUDED.match_method,
                 confidence = EXCLUDED.confidence,
                 confirmed = xb_entity_links.confirmed OR EXCLUDED.confirmed
             WHERE NOT xb_entity_links.rejected
               AND xb_entity_links.match_method <> 'MANUAL'",
        )
        .bind(tenant_id)
        .bind(prop.entity_id)
        .bind(prop.node_id)
        .bind(prop.method.as_str())
        .bind(prop.confidence)
        .bind(prop.confirmed)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_entity_links failed: {err}")))?;

        resolved = resolved.saturating_add(1);
        if prop.confirmed {
            auto_confirmed = auto_confirmed.saturating_add(1);
        }
    }

    let pending_candidates = count_pending_candidates_tx(&mut tx).await?;
    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit resolve_entity_links tx failed: {err}"))
    })?;

    Ok(ResolveEntitiesResponse {
        resolved,
        auto_confirmed,
        pending_candidates,
    })
}

fn compute_proposals(
    entities: &[EntityRow],
    nodes: &[NodeSummaryRow],
    only_node_id: Option<Uuid>,
    existing_map: &HashMap<(Uuid, Uuid), ExistingLinkState>,
    entity_embs: &HashMap<(Uuid, i32), Vec<f32>>,
    node_embs: &HashMap<i32, Vec<(Uuid, Vec<f32>)>>,
) -> Vec<ProposedLink> {
    let mut proposals: HashMap<(Uuid, Uuid), ProposedLink> = HashMap::new();

    for ent in entities {
        for node in nodes {
            if let Some(filter_nid) = only_node_id
                && node.id != filter_nid
            {
                continue;
            }

            let pair = (ent.id, node.id);
            if let Some(ex) = existing_map.get(&pair)
                && (ex.rejected || ex.match_method == EntityMatchMethod::Manual)
            {
                continue;
            }

            let matched = if !ent.norm_name.is_empty() && ent.norm_name == node.norm_key {
                Some((EntityMatchMethod::ExactKey, 1.0))
            } else if !ent.norm_name.is_empty() && ent.norm_name == node.norm_title {
                Some((EntityMatchMethod::ExactTitle, 0.95))
            } else if ent
                .norm_synonyms
                .iter()
                .any(|syn| syn == &node.norm_key || syn == &node.norm_title)
            {
                Some((EntityMatchMethod::Synonym, 0.9))
            } else {
                None
            };

            if let Some((method, confidence)) = matched {
                let prev_confirmed = existing_map.get(&pair).is_some_and(|e| e.confirmed);
                proposals.insert(
                    pair,
                    ProposedLink {
                        entity_id: ent.id,
                        node_id: node.id,
                        method,
                        confidence,
                        confirmed: prev_confirmed || confidence >= 0.9,
                    },
                );
            }
        }
    }

    for ((ent_id, dim), ent_vec) in entity_embs {
        let Some(candidates) = node_embs.get(dim) else {
            continue;
        };
        let mut sims: Vec<(Uuid, f64)> = candidates
            .iter()
            .filter_map(|(nid, nvec)| cosine_similarity(ent_vec, nvec).map(|s| (*nid, s)))
            .collect();
        sims.sort_by(|a, b| b.1.total_cmp(&a.1));

        let Some(&(best_nid, best_sim)) = sims.first() else {
            continue;
        };
        let second_sim = sims.get(1).map_or(0.0, |x| x.1);
        let margin_ok = sims.len() == 1 || (best_sim - second_sim) >= 0.03;

        if best_sim < 0.88 || !margin_ok {
            continue;
        }
        if let Some(filter_nid) = only_node_id
            && best_nid != filter_nid
        {
            continue;
        }

        let pair = (*ent_id, best_nid);
        if proposals.contains_key(&pair) {
            continue;
        }
        if let Some(ex) = existing_map.get(&pair)
            && (ex.rejected || ex.match_method == EntityMatchMethod::Manual)
        {
            continue;
        }

        let confidence = best_sim.clamp(0.0, 1.0);
        let prev_confirmed = existing_map.get(&pair).is_some_and(|e| e.confirmed);
        proposals.insert(
            pair,
            ProposedLink {
                entity_id: *ent_id,
                node_id: best_nid,
                method: EntityMatchMethod::Embedding,
                confidence,
                confirmed: prev_confirmed || confidence >= 0.9,
            },
        );
    }

    proposals.into_values().collect()
}

pub(crate) async fn list_entity_candidates_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<EntityCandidatesResponse, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT el.entity_id, e.name AS entity_name,
                el.node_id, n.node_key, n.title AS node_title,
                el.match_method, el.confidence, el.confirmed, el.rejected, el.created_at
           FROM xb_entity_links el
           JOIN hg_entities e ON e.id = el.entity_id
           JOIN xb_nodes n ON n.id = el.node_id AND n.subgraph_slug = 'main'
          WHERE el.confirmed = FALSE AND el.rejected = FALSE
          ORDER BY el.confidence DESC, el.created_at DESC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list_entity_candidates failed: {err}")))?;
    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit list_entity_candidates tx failed: {err}"))
    })?;

    let items: Vec<EntityLink> = rows
        .iter()
        .map(decode_entity_link)
        .collect::<Result<_, _>>()?;
    Ok(EntityCandidatesResponse { items })
}

pub(crate) async fn confirm_or_reject_entity_link_impl(
    store: &PgStore,
    tenant_id: &str,
    entity_id: Uuid,
    node_id: Uuid,
    confirm: bool,
) -> Result<EntityLink, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing =
        sqlx::query("SELECT 1 FROM xb_entity_links WHERE entity_id = $1 AND node_id = $2")
            .bind(entity_id)
            .bind(node_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("check xb_entity_links failed: {err}")))?;

    if existing.is_some() {
        sqlx::query(
            "UPDATE xb_entity_links
                SET confirmed = $3,
                    rejected = $4
              WHERE entity_id = $1 AND node_id = $2",
        )
        .bind(entity_id)
        .bind(node_id)
        .bind(confirm)
        .bind(!confirm)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update xb_entity_links failed: {err}")))?;
    } else {
        let both_exist: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM hg_entities WHERE id = $1)
                AND EXISTS(SELECT 1 FROM xb_nodes WHERE id = $2 AND subgraph_slug = 'main')",
        )
        .bind(entity_id)
        .bind(node_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("check entity/node existence failed: {err}")))?;

        if !both_exist {
            return Err(AppError::NotFound("ENTITY_LINK_NOT_FOUND"));
        }

        let confidence = if confirm { 1.0 } else { 0.0 };
        sqlx::query(
            "INSERT INTO xb_entity_links (
                 tenant_id, entity_id, node_id, match_method, confidence, confirmed, rejected
             )
             VALUES ($1, $2, $3, 'MANUAL', $4, $5, $6)",
        )
        .bind(tenant_id)
        .bind(entity_id)
        .bind(node_id)
        .bind(confidence)
        .bind(confirm)
        .bind(!confirm)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("insert manual xb_entity_links failed: {err}"))
        })?;
    }

    let joined_row = sqlx::query(
        "SELECT el.entity_id, e.name AS entity_name,
                el.node_id, n.node_key, n.title AS node_title,
                el.match_method, el.confidence, el.confirmed, el.rejected, el.created_at
           FROM xb_entity_links el
           JOIN hg_entities e ON e.id = el.entity_id
           JOIN xb_nodes n ON n.id = el.node_id
          WHERE el.entity_id = $1 AND el.node_id = $2",
    )
    .bind(entity_id)
    .bind(node_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("reload xb_entity_links failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit confirm_or_reject link tx: {err}")))?;
    decode_entity_link(&joined_row)
}

async fn load_entities_tx(tx: &mut Transaction<'_, Postgres>) -> Result<Vec<EntityRow>, AppError> {
    let rows = sqlx::query("SELECT id, name, synonyms FROM hg_entities")
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load hg_entities failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let name: String = r.get("name");
            let syns: Vec<String> = r.get("synonyms");
            let norm_synonyms = syns
                .iter()
                .map(|s| normalize_name(s))
                .filter(|s| !s.is_empty())
                .collect();
            EntityRow {
                id: r.get("id"),
                norm_name: normalize_name(&name),
                norm_synonyms,
            }
        })
        .collect())
}

async fn load_nodes_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<NodeSummaryRow>, AppError> {
    let rows = sqlx::query("SELECT id, node_key, title FROM xb_nodes WHERE subgraph_slug = 'main'")
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load xb_nodes summary failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let node_key: String = r.get("node_key");
            let title: String = r.get("title");
            NodeSummaryRow {
                id: r.get("id"),
                norm_key: normalize_name(&node_key),
                norm_title: normalize_name(&title),
            }
        })
        .collect())
}

async fn load_existing_links_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<HashMap<(Uuid, Uuid), ExistingLinkState>, AppError> {
    let rows = sqlx::query(
        "SELECT entity_id, node_id, match_method, confirmed, rejected FROM xb_entity_links",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load xb_entity_links failed: {err}")))?;

    let mut map = HashMap::with_capacity(rows.len());
    for r in rows {
        let method = EntityMatchMethod::parse(&r.get::<String, _>("match_method"))?;
        map.insert(
            (r.get("entity_id"), r.get("node_id")),
            ExistingLinkState {
                match_method: method,
                confirmed: r.get("confirmed"),
                rejected: r.get("rejected"),
            },
        );
    }
    Ok(map)
}

type EmbeddingMaps = (
    HashMap<(Uuid, i32), Vec<f32>>,
    HashMap<i32, Vec<(Uuid, Vec<f32>)>>,
);

async fn load_entity_and_node_embeddings_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<EmbeddingMaps, AppError> {
    let rows = sqlx::query(
        "SELECT node_type, node_id, embedding_dim, embedding
           FROM hg_embeddings
          WHERE node_type IN ('ENTITY', 'XB_NODE')",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load hg_embeddings failed: {err}")))?;

    let mut entity_embs: HashMap<(Uuid, i32), Vec<f32>> = HashMap::new();
    let mut node_embs: HashMap<i32, Vec<(Uuid, Vec<f32>)>> = HashMap::new();

    for r in rows {
        let ntype: String = r.get("node_type");
        let nid: Uuid = r.get("node_id");
        let dim: i32 = r.get("embedding_dim");
        let vec_pg: Vector = r.get("embedding");
        let values = vec_pg.to_vec();
        if ntype == "ENTITY" {
            entity_embs.insert((nid, dim), values);
        } else if ntype == "XB_NODE" {
            node_embs.entry(dim).or_default().push((nid, values));
        }
    }

    Ok((entity_embs, node_embs))
}

async fn count_pending_candidates_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<usize, AppError> {
    let cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM xb_entity_links WHERE confirmed = FALSE AND rejected = FALSE",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("count pending candidates failed: {err}")))?;
    Ok(usize::try_from(cnt.max(0)).unwrap_or(0))
}

async fn collect_para_related_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    para_ids: &[Uuid],
    subgraph_slug: &str,
    seen: &mut std::collections::BTreeSet<Uuid>,
    out: &mut Vec<axpoint_domain::analytics::RelatedNodeSummary>,
) -> Result<(), AppError> {
    if para_ids.is_empty() {
        return Ok(());
    }
    let rows = sqlx::query(
        "SELECT DISTINCT n.id, n.type_name, n.title, n.metrics
           FROM hg_edges ed
           JOIN xb_entity_links el
             ON el.entity_id = ed.dst_id
            AND el.confirmed = TRUE
            AND el.rejected = FALSE
           JOIN xb_nodes n ON n.id = el.node_id AND n.subgraph_slug = $2
          WHERE ed.src_type = 'PARA'
            AND ed.dst_type = 'ENTITY'
            AND ed.rel_type = 'MENTIONS'
            AND ed.src_id = ANY($1)
          ORDER BY n.title ASC",
    )
    .bind(para_ids)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query related_nodes from paras failed: {err}")))?;

    for r in rows {
        let id: Uuid = r.get("id");
        if seen.insert(id) {
            out.push(axpoint_domain::analytics::RelatedNodeSummary {
                id,
                type_name: r.get("type_name"),
                title: r.get("title"),
                metrics: r.get("metrics"),
            });
        }
    }
    Ok(())
}

async fn collect_ref_related_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    linked_node_refs: &[String],
    subgraph_slug: &str,
    seen: &mut std::collections::BTreeSet<Uuid>,
    out: &mut Vec<axpoint_domain::analytics::RelatedNodeSummary>,
) -> Result<(), AppError> {
    for nref in linked_node_refs {
        let (maybe_type, key) = match nref.split_once(':') {
            Some((t, k)) => (Some(t.trim()), k.trim()),
            None => (None, nref.trim()),
        };
        if key.is_empty() {
            continue;
        }
        let rows = sqlx::query(
            "SELECT id, type_name, title, metrics
               FROM xb_nodes
              WHERE subgraph_slug = $1
                AND node_key = $2
                AND ($3::text IS NULL OR type_name = $3)
              ORDER BY title ASC",
        )
        .bind(subgraph_slug)
        .bind(key)
        .bind(maybe_type)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("query related_nodes by node_key failed: {err}"))
        })?;

        for r in rows {
            let id: Uuid = r.get("id");
            if seen.insert(id) {
                out.push(axpoint_domain::analytics::RelatedNodeSummary {
                    id,
                    type_name: r.get("type_name"),
                    title: r.get("title"),
                    metrics: r.get("metrics"),
                });
            }
        }
    }
    Ok(())
}

impl PgStore {
    /// Resolves distinct confirmed `BlueNode` summaries in `"main"` linked to recalled `hg_paras`
    /// (via `MENTIONS -> xb_entity_links`) or `type_name:node_key` references from NL2SQL schema linking (`05-STEP04` §04.8).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `xb_entity_links` or `xb_nodes` fails.
    pub async fn list_related_nodes(
        &self,
        tenant_id: &str,
        para_ids: &[Uuid],
        linked_node_refs: &[String],
    ) -> Result<Vec<axpoint_domain::analytics::RelatedNodeSummary>, AppError> {
        self.list_related_nodes_in_subgraph(tenant_id, para_ids, linked_node_refs, "main")
            .await
    }

    /// Resolves distinct confirmed `BlueNode` summaries in `subgraph_slug` linked to recalled `hg_paras`
    /// or `type_name:node_key` references (`06-STEP20` §20.2.2).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `xb_entity_links` or `xb_nodes` fails.
    pub async fn list_related_nodes_in_subgraph(
        &self,
        tenant_id: &str,
        para_ids: &[Uuid],
        linked_node_refs: &[String],
        subgraph_slug: &str,
    ) -> Result<Vec<axpoint_domain::analytics::RelatedNodeSummary>, AppError> {
        if para_ids.is_empty() && linked_node_refs.is_empty() {
            return Ok(Vec::new());
        }

        let clean_slug = {
            let trimmed = subgraph_slug.trim();
            if trimmed.is_empty() { "main" } else { trimmed }
        };

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();

        collect_para_related_nodes_tx(&mut tx, para_ids, clean_slug, &mut seen, &mut out).await?;
        collect_ref_related_nodes_tx(&mut tx, linked_node_refs, clean_slug, &mut seen, &mut out)
            .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_related_nodes tx failed: {err}"))
        })?;

        Ok(out)
    }
}
