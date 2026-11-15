//! Bi-directional `BlueNode` ↔ Hypergraph dossier synchronization and node candidate promotion (`06-STEP05-06` §06.5).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use axpoint_domain::enums::{JobKind, OutboxTopic, PiiCategory};
use axpoint_domain::graph::normalize_name;
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::ports::TextEmbedderPort;
use axpoint_domain::rollup::{
    AcceptNodeCandidateRequest, AcceptNodeCandidateResponse, KnowledgeNodeCandidate,
    KnowledgeNodeCandidatesResponse, KnowledgeSyncJobPayload,
};
use axpoint_domain::studio::{NodeTypeSpec, PropertyKind, PropertySpec};
use axpoint_domain::topology::BlueNode;
use axpoint_domain::{AppError, split_paragraphs};
use pgvector::Vector;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::nodes::decode_blue_node;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

const MAX_DOSSIER_NEIGHBORS: i64 = 50;
const MAX_DOSSIER_DOCS: i64 = 20;
const MIN_CANDIDATE_OCCURRENCES: i64 = 3;

struct DossierNeighbor {
    link_type: String,
    arrow: &'static str,
    neighbor_type: String,
    neighbor_key: String,
    neighbor_title: String,
}

struct PreparedDossier {
    node: BlueNode,
    nt: NodeTypeSpec,
    title: String,
    summary: String,
    paras: Vec<String>,
    doc_embedding: Vec<f32>,
    para_embeddings: Vec<Vec<f32>>,
    model_name: String,
    dim_i32: i32,
}

/// Executes `KNOWLEDGE_SYNC` for upserted/deleted nodes or newly enabled types (`06-STEP05-06` §06.5).
///
/// # Errors
/// Returns [`AppError`] if embedding generation or hypergraph persistence fails.
pub async fn execute_knowledge_sync_impl<E: TextEmbedderPort>(
    store: &PgStore,
    tenant_id: &str,
    payload: &KnowledgeSyncJobPayload,
    embedder: &E,
) -> Result<serde_json::Value, AppError> {
    let mut docs_deleted = 0_usize;
    let mut docs_synced = 0_usize;

    if let Some(del_id) = payload.deleted_node_id {
        let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
        docs_deleted =
            docs_deleted.saturating_add(delete_node_dossier_tx(&mut tx, tenant_id, del_id).await?);
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit knowledge_sync delete tx failed: {err}"))
        })?;
    }

    let (target_ids, sync_types) = resolve_sync_targets(store, tenant_id, payload).await?;
    for nid in target_ids {
        let Some(bundle) = load_node_dossier_inputs(store, tenant_id, nid, &sync_types).await?
        else {
            let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
            docs_deleted =
                docs_deleted.saturating_add(delete_node_dossier_tx(&mut tx, tenant_id, nid).await?);
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit knowledge_sync stale cleanup failed: {err}"))
            })?;
            continue;
        };

        let prepared =
            build_and_embed_dossier(bundle.0, bundle.1, &bundle.2, &bundle.3, embedder).await?;
        let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
        let _ = delete_node_dossier_tx(&mut tx, tenant_id, nid).await?;
        persist_node_dossier_tx(&mut tx, tenant_id, &prepared).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit knowledge_sync upsert tx failed: {err}"))
        })?;
        docs_synced = docs_synced.saturating_add(1);
    }

    Ok(serde_json::json!({
        "docs_synced": docs_synced,
        "docs_deleted": docs_deleted,
    }))
}

async fn resolve_sync_targets(
    store: &PgStore,
    tenant_id: &str,
    payload: &KnowledgeSyncJobPayload,
) -> Result<(BTreeSet<Uuid>, BTreeMap<String, NodeTypeSpec>), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let nt_rows = sqlx::query(
        "SELECT id, name, display_name, description, icon, color, title_template,
                properties, agg_rules, derived_rules, sync_to_knowledge, pos_x, pos_y
           FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main'",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load main node types for ksync failed: {err}")))?;

    let mut sync_types = BTreeMap::new();
    for r in nt_rows {
        let sync_flag: bool = r.get("sync_to_knowledge");
        if !sync_flag {
            continue;
        }
        let nt = NodeTypeSpec {
            id: Some(r.get("id")),
            name: r.get("name"),
            display_name: r.get("display_name"),
            description: r.get("description"),
            icon: r.get("icon"),
            color: r.get("color"),
            title_template: r.get("title_template"),
            properties: serde_json::from_value(r.get("properties")).unwrap_or_default(),
            agg_rules: serde_json::from_value(r.get("agg_rules")).unwrap_or_default(),
            derived_rules: serde_json::from_value(r.get("derived_rules")).unwrap_or_default(),
            sync_to_knowledge: true,
            pos_x: r.get("pos_x"),
            pos_y: r.get("pos_y"),
        };
        sync_types.insert(nt.name.clone(), nt);
    }

    let mut ids = BTreeSet::new();
    if let Some(single) = payload.node_id {
        ids.insert(single);
    }
    ids.extend(payload.node_ids.iter().copied());

    if !payload.type_names.is_empty() {
        let rows = sqlx::query(
            "SELECT id FROM xb_nodes WHERE tenant_id = $1 AND type_name = ANY($2) ORDER BY id ASC",
        )
        .bind(tenant_id)
        .bind(&payload.type_names)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list nodes by type for ksync failed: {err}")))?;
        for r in rows {
            ids.insert(r.get("id"));
        }
    }

    if let Some(del_id) = payload.deleted_node_id {
        let _ = ids.remove(&del_id);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit resolve_sync_targets failed: {err}")))?;
    Ok((ids, sync_types))
}

async fn load_node_dossier_inputs(
    store: &PgStore,
    tenant_id: &str,
    node_id: Uuid,
    sync_types: &BTreeMap<String, NodeTypeSpec>,
) -> Result<Option<(BlueNode, NodeTypeSpec, Vec<DossierNeighbor>, Vec<String>)>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let Some(nrow) = sqlx::query(
        "SELECT id, tenant_id, type_name, node_key, title, properties, metrics,
                geo_lng, geo_lat, source_dataset_id, source_row_key, row_version,
                created_at, updated_at
           FROM xb_nodes
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(node_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load node for ksync failed: {err}")))?
    else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit missing node ksync lookup failed: {err}"))
        })?;
        return Ok(None);
    };

    let node = decode_blue_node(&nrow);
    let Some(nt) = sync_types.get(&node.type_name).cloned() else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit unsynced type ksync lookup failed: {err}"))
        })?;
        return Ok(None);
    };

    let neighbors = load_dossier_neighbors_tx(&mut tx, tenant_id, node_id).await?;
    let related_docs = load_related_doc_titles_tx(&mut tx, tenant_id, node_id).await?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit load_node_dossier_inputs failed: {err}"))
    })?;
    Ok(Some((node, nt, neighbors, related_docs)))
}

async fn load_dossier_neighbors_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_id: Uuid,
) -> Result<Vec<DossierNeighbor>, AppError> {
    let rows = sqlx::query(
        "SELECT l.link_type, l.src_node_id,
                other.type_name AS neighbor_type,
                other.node_key AS neighbor_key,
                other.title AS neighbor_title
           FROM xb_links l
           JOIN xb_nodes other
             ON other.tenant_id = l.tenant_id
            AND other.id = CASE WHEN l.src_node_id = $2 THEN l.dst_node_id ELSE l.src_node_id END
          WHERE l.tenant_id = $1
            AND (l.src_node_id = $2 OR l.dst_node_id = $2)
          ORDER BY l.link_type ASC, other.node_key ASC
          LIMIT $3",
    )
    .bind(tenant_id)
    .bind(node_id)
    .bind(MAX_DOSSIER_NEIGHBORS)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load dossier 1-hop neighbors failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let src_id: Uuid = r.get("src_node_id");
            DossierNeighbor {
                link_type: r.get("link_type"),
                arrow: if src_id == node_id { "→" } else { "←" },
                neighbor_type: r.get("neighbor_type"),
                neighbor_key: r.get("neighbor_key"),
                neighbor_title: r.get("neighbor_title"),
            }
        })
        .collect())
}

async fn load_related_doc_titles_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        "SELECT DISTINCT d.title
           FROM xb_entity_links el
           JOIN hg_edges e ON e.tenant_id = el.tenant_id AND e.dst_id = el.entity_id
                          AND e.src_type = 'PARA' AND e.dst_type = 'ENTITY' AND e.rel_type = 'MENTIONS'
           JOIN hg_paras p ON p.tenant_id = el.tenant_id AND p.id = e.src_id
           JOIN hg_docs d ON d.tenant_id = el.tenant_id AND d.id = p.doc_id
          WHERE el.tenant_id = $1
            AND el.node_id = $2
            AND el.rejected = FALSE
            AND d.source_name NOT LIKE 'xb_node:%'
          ORDER BY d.title ASC
          LIMIT $3",
    )
    .bind(tenant_id)
    .bind(node_id)
    .bind(MAX_DOSSIER_DOCS)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load related doc titles for ksync failed: {err}")))?;

    Ok(rows.into_iter().map(|r| r.get("title")).collect())
}

async fn build_and_embed_dossier<E: TextEmbedderPort>(
    node: BlueNode,
    nt: NodeTypeSpec,
    neighbors: &[DossierNeighbor],
    related_docs: &[String],
    embedder: &E,
) -> Result<PreparedDossier, AppError> {
    let doc_title = format!("{}（{} · {}）", node.title, nt.display_name, node.node_key);
    let summary = format!(
        "业务节点档案：{}（类型 {} / {}，主键 {}）",
        node.title, nt.display_name, nt.name, node.node_key
    );
    let markdown = render_node_dossier_markdown(&node, &nt, neighbors, related_docs);
    let mut paras = split_paragraphs(&markdown, 900);
    if paras.is_empty() {
        paras.push(markdown);
    }

    let mut texts_to_embed = Vec::with_capacity(paras.len().saturating_add(1));
    texts_to_embed.push(summary.clone());
    texts_to_embed.extend(paras.iter().cloned());

    let (mut vecs, dim) = embedder.embed_texts(&texts_to_embed).await?;
    let model_name = String::from(embedder.embedding_model_name());
    if vecs.len() != texts_to_embed.len() {
        return Err(AppError::Database(format!(
            "embedder returned {} vectors for {} texts",
            vecs.len(),
            texts_to_embed.len()
        )));
    }
    let dim_i32 = i32::try_from(dim)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid embed dim: {err}")))?;

    let mut iter = vecs.drain(..);
    let doc_embedding = iter.next().unwrap_or_default();
    let para_embeddings: Vec<Vec<f32>> = iter.collect();

    Ok(PreparedDossier {
        node,
        nt,
        title: doc_title,
        summary,
        paras,
        doc_embedding,
        para_embeddings,
        model_name,
        dim_i32,
    })
}

fn render_node_dossier_markdown(
    node: &BlueNode,
    nt: &NodeTypeSpec,
    neighbors: &[DossierNeighbor],
    related_docs: &[String],
) -> String {
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# {}（{} · {}）\n\n- **节点类型**：{} (`{}`)\n- **业务主键**：`{}`",
        node.title, nt.display_name, node.node_key, nt.display_name, nt.name, node.node_key
    );
    if let Some(geo) = node.geo {
        let _ = writeln!(md, "- **地理坐标**：`{}, {}`", geo.lng, geo.lat);
    }

    let prop_specs: BTreeMap<&str, &PropertySpec> =
        nt.properties.iter().map(|p| (p.name.as_str(), p)).collect();

    md.push_str("\n## 属性表\n\n| 属性 | 键名 | 值 |\n| :--- | :--- | :--- |\n");
    let mut attr_count = 0_usize;
    for (k, v) in &node.properties {
        if k.starts_with("__") {
            continue;
        }
        let spec_opt = prop_specs.get(k.as_str()).copied();
        let label = spec_opt.map_or(k.as_str(), |p| p.display_name.as_str());
        let rendered_val = if spec_opt.is_some_and(|p| p.pii_category == PiiCategory::Secret) {
            String::from("[REDACTED]")
        } else {
            format_json_cell(v)
        };
        let _ = writeln!(md, "| {label} | `{k}` | {rendered_val} |");
        attr_count = attr_count.saturating_add(1);
    }
    if attr_count == 0 {
        md.push_str("| - | - | 暂无属性 |\n");
    }

    md.push_str(
        "\n## 最新指标\n\n| 指标 | 键名 | 最新值 | 单位 |\n| :--- | :--- | :--- | :--- |\n",
    );
    if node.metrics.is_empty() {
        md.push_str("| - | - | 暂无指标 | - |\n");
    } else {
        for (mk, &mv) in &node.metrics {
            let spec_opt = prop_specs
                .get(mk.as_str())
                .copied()
                .filter(|p| p.kind == PropertyKind::Metric);
            let label = spec_opt.map_or(mk.as_str(), |p| p.display_name.as_str());
            let unit = spec_opt
                .and_then(|p| p.unit.as_deref())
                .filter(|u| !u.is_empty())
                .unwrap_or("-");
            let _ = writeln!(md, "| {label} | `{mk}` | {mv:.4} | {unit} |");
        }
    }

    md.push_str("\n## 一跳关联节点\n\n");
    if neighbors.is_empty() {
        md.push_str("- 暂无关联节点\n");
    } else {
        for nb in neighbors {
            let _ = writeln!(
                md,
                "- `{}` {} {}（`{}:{}`）",
                nb.link_type, nb.arrow, nb.neighbor_title, nb.neighbor_type, nb.neighbor_key
            );
        }
    }

    if !related_docs.is_empty() {
        md.push_str("\n## 关联知识文档\n\n");
        for dt in related_docs {
            let _ = writeln!(md, "- {dt}");
        }
    }

    md
}

fn format_json_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::from("-"),
        serde_json::Value::String(s) => s.replace('|', "\\|"),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            v.to_string().replace('|', "\\|")
        }
    }
}

async fn delete_node_dossier_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_id: Uuid,
) -> Result<usize, AppError> {
    let source_name = format!("xb_node:{node_id}");
    let doc_rows = sqlx::query("SELECT id FROM hg_docs WHERE tenant_id = $1 AND source_name = $2")
        .bind(tenant_id)
        .bind(&source_name)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("lookup existing node dossier failed: {err}")))?;

    let doc_ids: Vec<Uuid> = doc_rows.into_iter().map(|r| r.get("id")).collect();
    if doc_ids.is_empty() {
        return Ok(0);
    }

    let para_rows =
        sqlx::query("SELECT id FROM hg_paras WHERE tenant_id = $1 AND doc_id = ANY($2)")
            .bind(tenant_id)
            .bind(&doc_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("lookup dossier paras failed: {err}")))?;
    let para_ids: Vec<Uuid> = para_rows.into_iter().map(|r| r.get("id")).collect();

    sqlx::query(
        "DELETE FROM hg_embeddings
          WHERE tenant_id = $1
            AND ((node_type = 'DOC' AND node_id = ANY($2))
              OR (node_type = 'PARA' AND node_id = ANY($3)))",
    )
    .bind(tenant_id)
    .bind(&doc_ids)
    .bind(&para_ids)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete dossier embeddings failed: {err}")))?;

    sqlx::query(
        "DELETE FROM hg_edges
          WHERE tenant_id = $1
            AND (src_id = ANY($2) OR dst_id = ANY($2) OR src_id = ANY($3) OR dst_id = ANY($3))",
    )
    .bind(tenant_id)
    .bind(&doc_ids)
    .bind(&para_ids)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete dossier hg_edges failed: {err}")))?;

    let del = sqlx::query("DELETE FROM hg_docs WHERE tenant_id = $1 AND id = ANY($2)")
        .bind(tenant_id)
        .bind(&doc_ids)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("delete dossier hg_docs failed: {err}")))?;

    Ok(usize::try_from(del.rows_affected()).unwrap_or(0))
}

async fn persist_node_dossier_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    prep: &PreparedDossier,
) -> Result<(), AppError> {
    let source_name = format!("xb_node:{}", prep.node.id);
    let doc_row = sqlx::query(
        "INSERT INTO hg_docs (tenant_id, title, source_name, summary)
         VALUES ($1, $2, $3, $4)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(&prep.title)
    .bind(&source_name)
    .bind(&prep.summary)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert dossier hg_docs failed: {err}")))?;
    let doc_id: Uuid = doc_row.get("id");

    sqlx::query(
        "INSERT INTO hg_embeddings (tenant_id, node_type, node_id, model_name, embedding_dim, embedding)
         VALUES ($1, 'DOC', $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(doc_id)
    .bind(&prep.model_name)
    .bind(prep.dim_i32)
    .bind(Vector::from(prep.doc_embedding.clone()))
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert dossier DOC embedding failed: {err}")))?;

    let ent_name = prep.node.title.trim();
    let ent_norm = normalize_name(ent_name);
    let synonyms: Vec<String> = if prep.node.node_key == ent_name {
        Vec::new()
    } else {
        vec![prep.node.node_key.clone()]
    };
    let ent_row = sqlx::query(
        "INSERT INTO hg_entities (tenant_id, name, name_norm, entity_type, synonyms, occur_count, confidence)
         VALUES ($1, $2, $3, $4, $5, 1, 1.0)
         ON CONFLICT (tenant_id, name, entity_type) DO UPDATE SET
             name_norm = EXCLUDED.name_norm,
             confidence = 1.0,
             updated_at = NOW()
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(ent_name)
    .bind(&ent_norm)
    .bind(&prep.nt.name)
    .bind(&synonyms)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert dossier canonical entity failed: {err}")))?;
    let entity_id: Uuid = ent_row.get("id");

    sqlx::query(
        "INSERT INTO xb_entity_links (
             tenant_id, entity_id, node_id, match_method, confidence, confirmed, rejected
         )
         VALUES ($1, $2, $3, 'EXACT_KEY', 1.0, TRUE, FALSE)
         ON CONFLICT (entity_id, node_id) DO UPDATE SET
             match_method = 'EXACT_KEY',
             confidence = 1.0,
             confirmed = TRUE,
             rejected = FALSE",
    )
    .bind(tenant_id)
    .bind(entity_id)
    .bind(prep.node.id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert EXACT_KEY xb_entity_links failed: {err}")))?;

    persist_dossier_paragraphs_tx(tx, tenant_id, doc_id, entity_id, prep).await
}

async fn persist_dossier_paragraphs_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    doc_id: Uuid,
    entity_id: Uuid,
    prep: &PreparedDossier,
) -> Result<(), AppError> {
    let mut prev_para_id: Option<Uuid> = None;
    for (idx, (content, emb)) in prep.paras.iter().zip(&prep.para_embeddings).enumerate() {
        let seq_no = i32::try_from(idx).unwrap_or(i32::MAX);
        let prow = sqlx::query(
            "INSERT INTO hg_paras (doc_id, tenant_id, seq_no, content, entity_density)
             VALUES ($1, $2, $3, $4, 1)
             RETURNING id",
        )
        .bind(doc_id)
        .bind(tenant_id)
        .bind(seq_no)
        .bind(content)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert dossier hg_paras failed: {err}")))?;
        let para_id: Uuid = prow.get("id");

        sqlx::query(
            "INSERT INTO hg_embeddings (tenant_id, node_type, node_id, model_name, embedding_dim, embedding)
             VALUES ($1, 'PARA', $2, $3, $4, $5)",
        )
        .bind(tenant_id)
        .bind(para_id)
        .bind(&prep.model_name)
        .bind(prep.dim_i32)
        .bind(Vector::from(emb.clone()))
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("insert dossier PARA embedding failed: {err}")))?;

        for (s_type, s_id, d_type, d_id, rel) in [
            ("DOC", doc_id, "PARA", para_id, "CONTAINS"),
            ("PARA", para_id, "ENTITY", entity_id, "MENTIONS"),
        ] {
            sqlx::query(
                "INSERT INTO hg_edges (tenant_id, src_type, src_id, dst_type, dst_id, rel_type)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(tenant_id)
            .bind(s_type)
            .bind(s_id)
            .bind(d_type)
            .bind(d_id)
            .bind(rel)
            .execute(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("insert dossier {rel} edge failed: {err}"))
            })?;
        }

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
            .map_err(|err| {
                AppError::Database(format!("insert dossier NEXT_PARA edge failed: {err}"))
            })?;
        }
        prev_para_id = Some(para_id);
    }
    Ok(())
}

/// Lists unlinked hypergraph entities (`occur_count >= 3`) eligible to become business nodes (`06-STEP05-06` §06.5).
///
/// # Errors
/// Returns [`AppError::Database`] if querying `hg_entities` fails.
pub async fn list_knowledge_node_candidates_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<KnowledgeNodeCandidatesResponse, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let type_rows = sqlx::query(
        "SELECT name, display_name
           FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main'
          ORDER BY name ASC",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load main node types for candidates: {err}")))?;

    let main_types: Vec<(String, String)> = type_rows
        .into_iter()
        .map(|r| (r.get("name"), r.get("display_name")))
        .collect();
    if main_types.is_empty() {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit empty node candidates tx: {err}")))?;
        return Ok(KnowledgeNodeCandidatesResponse { items: Vec::new() });
    }

    let ent_rows = sqlx::query(
        "SELECT e.id, e.name, e.entity_type, e.occur_count, e.confidence
           FROM hg_entities e
          WHERE e.tenant_id = $1
            AND e.occur_count >= $2
            AND NOT EXISTS (
                SELECT 1 FROM xb_entity_links el
                 WHERE el.tenant_id = $1
                   AND el.entity_id = e.id
                   AND (el.confirmed = TRUE OR el.rejected = TRUE)
            )
          ORDER BY e.occur_count DESC, e.confidence DESC, e.name ASC
          LIMIT 200",
    )
    .bind(tenant_id)
    .bind(MIN_CANDIDATE_OCCURRENCES)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("query knowledge node candidates failed: {err}")))?;

    let mut matched = Vec::new();
    let mut ent_ids = Vec::new();
    for r in ent_rows {
        let ent_type: String = r.get("entity_type");
        if let Some((nt_name, _)) = match_entity_to_node_type(&ent_type, &main_types) {
            let eid: Uuid = r.get("id");
            ent_ids.push(eid);
            matched.push(KnowledgeNodeCandidate {
                entity_id: eid,
                name: r.get("name"),
                entity_type: ent_type,
                suggested_type_name: nt_name,
                occur_count: r.get("occur_count"),
                confidence: r.get("confidence"),
                sample_docs: Vec::new(),
            });
        }
    }

    if !ent_ids.is_empty() {
        let doc_map = load_candidate_sample_docs_tx(&mut tx, tenant_id, &ent_ids).await?;
        for item in &mut matched {
            if let Some(docs) = doc_map.get(&item.entity_id) {
                item.sample_docs.clone_from(docs);
            }
        }
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list node candidates failed: {err}")))?;
    Ok(KnowledgeNodeCandidatesResponse { items: matched })
}

fn match_entity_to_node_type(
    entity_type: &str,
    main_types: &[(String, String)],
) -> Option<(String, String)> {
    let et_trim = entity_type.trim();
    let et_norm = normalize_name(et_trim);
    for (name, disp) in main_types {
        if et_trim.eq_ignore_ascii_case(name.trim())
            || et_trim.eq_ignore_ascii_case(disp.trim())
            || (!et_norm.is_empty() && et_norm == normalize_name(disp))
        {
            return Some((name.clone(), disp.clone()));
        }
    }
    None
}

async fn load_candidate_sample_docs_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    entity_ids: &[Uuid],
) -> Result<BTreeMap<Uuid, Vec<String>>, AppError> {
    let rows = sqlx::query(
        "SELECT DISTINCT e.dst_id AS entity_id, d.title
           FROM hg_edges e
           JOIN hg_paras p ON p.tenant_id = e.tenant_id AND p.id = e.src_id
           JOIN hg_docs d ON d.tenant_id = e.tenant_id AND d.id = p.doc_id
          WHERE e.tenant_id = $1
            AND e.src_type = 'PARA'
            AND e.dst_type = 'ENTITY'
            AND e.rel_type = 'MENTIONS'
            AND e.dst_id = ANY($2)
            AND d.source_name NOT LIKE 'xb_node:%'
          ORDER BY e.dst_id ASC, d.title ASC",
    )
    .bind(tenant_id)
    .bind(entity_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load candidate sample docs failed: {err}")))?;

    let mut out: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
    for r in rows {
        let eid: Uuid = r.get("entity_id");
        let entry = out.entry(eid).or_default();
        if entry.len() < 5 {
            entry.push(r.get("title"));
        }
    }
    Ok(out)
}

/// Accepts an unlinked hypergraph entity candidate, creating/upserting its `BlueNode` and confirming its `xb_entity_links` entry (`06-STEP05-06` §06.5).
///
/// # Errors
/// Returns [`AppError::NotFound`] if `entity_id` or `req.type_name` does not exist, [`AppError::BadRequest`] on invalid `node_key`, or [`AppError::Database`] on SQL failure.
pub async fn accept_knowledge_node_candidate_impl(
    store: &PgStore,
    tenant_id: &str,
    entity_id: Uuid,
    req: AcceptNodeCandidateRequest,
) -> Result<AcceptNodeCandidateResponse, AppError> {
    let type_name = req.type_name.trim();
    let node_key = req.node_key.trim();
    if type_name.is_empty() || node_key.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "type_name and node_key must not be empty",
        ));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let type_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM xb_node_types
              WHERE tenant_id = $1 AND subgraph_slug = 'main' AND name = $2
         )",
    )
    .bind(tenant_id)
    .bind(type_name)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check accept candidate node type: {err}")))?;
    if !type_exists {
        return Err(AppError::NotFound("TYPE_NOT_FOUND"));
    }

    let ent_row = sqlx::query("SELECT name FROM hg_entities WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(entity_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lookup hg_entities for accept: {err}")))?
        .ok_or(AppError::NotFound("ENTITY_NOT_FOUND"))?;
    let ent_name: String = ent_row.get("name");

    let resolved_title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&ent_name);

    let nrow = sqlx::query(
        "INSERT INTO xb_nodes (tenant_id, type_name, node_key, title)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id, type_name, node_key) DO UPDATE SET
             title = EXCLUDED.title,
             row_version = xb_nodes.row_version + 1,
             updated_at = NOW()
         RETURNING id, tenant_id, type_name, node_key, title, properties, metrics,
                   geo_lng, geo_lat, source_dataset_id, source_row_key, row_version,
                   created_at, updated_at",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(node_key)
    .bind(resolved_title)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert accepted candidate xb_nodes: {err}")))?;

    let node = decode_blue_node(&nrow);

    sqlx::query(
        "INSERT INTO xb_entity_links (
             tenant_id, entity_id, node_id, match_method, confidence, confirmed, rejected
         )
         VALUES ($1, $2, $3, 'MANUAL', 1.0, TRUE, FALSE)
         ON CONFLICT (entity_id, node_id) DO UPDATE SET
             match_method = 'MANUAL',
             confidence = 1.0,
             confirmed = TRUE,
             rejected = FALSE",
    )
    .bind(tenant_id)
    .bind(entity_id)
    .bind(node.id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("confirm accepted xb_entity_links: {err}")))?;

    let up_event = NewOutboxEvent {
        topic: OutboxTopic::NodeUpserted,
        payload: serde_json::json!({
            "node_id": node.id,
            "node_ids": [node.id],
            "type_name": node.type_name,
        }),
    };
    let _ = insert_outbox_tx(&mut tx, tenant_id, &up_event).await?;

    let embed_job = NewJob::new(
        JobKind::EmbedNodes,
        serde_json::json!({ "node_ids": [node.id] }),
    );
    let _ = enqueue_job_tx(&mut tx, tenant_id, &embed_job).await?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit accept candidate tx failed: {err}")))?;

    Ok(AcceptNodeCandidateResponse {
        node,
        entity_id,
        confirmed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::enums::DataType;
    use chrono::Utc;

    #[test]
    fn knowledge_sync_renders_dossier_and_redacts_secret_properties() {
        let mut props = serde_json::Map::new();
        props.insert(String::from("city"), serde_json::json!("苏州"));
        props.insert(String::from("api_secret"), serde_json::json!("s3cr3t"));

        let node = BlueNode {
            id: Uuid::new_v4(),
            type_name: String::from("Warehouse"),
            node_key: String::from("W-1"),
            title: String::from("华东中心仓"),
            properties: props,
            metrics: BTreeMap::from([(String::from("inventory"), 500.0)]),
            geo: None,
            source_dataset_id: None,
            source_row_key: None,
            row_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let nt = NodeTypeSpec {
            id: None,
            name: String::from("Warehouse"),
            display_name: String::from("仓库"),
            description: String::new(),
            icon: String::from("warehouse"),
            color: String::from("#2563eb"),
            title_template: String::from("{node_key}"),
            properties: vec![
                PropertySpec {
                    name: String::from("city"),
                    display_name: String::from("城市"),
                    description: String::new(),
                    unit: None,
                    data_type: DataType::String,
                    kind: PropertyKind::Attribute,
                    required: false,
                    pii_category: PiiCategory::None,
                    allowed_values: None,
                },
                PropertySpec {
                    name: String::from("api_secret"),
                    display_name: String::from("密钥"),
                    description: String::new(),
                    unit: None,
                    data_type: DataType::String,
                    kind: PropertyKind::Attribute,
                    required: false,
                    pii_category: PiiCategory::Secret,
                    allowed_values: None,
                },
            ],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: true,
            pos_x: 0.0,
            pos_y: 0.0,
        };

        let md = render_node_dossier_markdown(&node, &nt, &[], &[String::from("2026履约SOP")]);
        assert!(md.contains("华东中心仓（仓库 · W-1）"));
        assert!(md.contains("苏州"));
        assert!(!md.contains("s3cr3t"));
        assert!(md.contains("[REDACTED]"));

        let types = vec![(String::from("Warehouse"), String::from("仓库"))];
        assert_eq!(
            match_entity_to_node_type("仓库", &types).map(|t| t.0),
            Some(String::from("Warehouse"))
        );
    }
}
