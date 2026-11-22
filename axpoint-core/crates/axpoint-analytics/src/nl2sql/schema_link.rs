//! Hybrid FTS + vector schema linking across datasets, columns, metrics, and `BlueNode`s (`05-STEP04` §04.6.3).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use axpoint_domain::analytics::{
    AllowedDatasetJoinEdge, LinkedColumnHit, LinkedMetricHit, LinkedObjects, MetricDef,
    SchemaRecallBundle,
};
use axpoint_domain::enums::{DataType, PiiCategory};
use axpoint_domain::studio::{MappingSpec, SchemaDoc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::result::mask_pii_str;
use crate::semantic::is_metric_broken;
use crate::sql_guard::{CatalogDatasetEntry, TenantCatalog};

/// Maximum candidate datasets selected by schema linking (`§04.6.3`).
pub const MAX_LINKED_DATASETS: usize = 5;

/// Maximum candidate columns retained per linked dataset (`§04.6.3`).
pub const MAX_COLS_PER_DATASET: usize = 15;

/// Maximum candidate semantic metrics selected (`§04.6.3`).
pub const MAX_LINKED_METRICS: usize = 5;

/// Ranked column metadata included in a linked table DDL.
#[derive(Debug, Clone)]
pub struct LinkedTableColumn {
    /// Column SQL identifier.
    pub name: String,
    /// Column logical data type.
    pub data_type: DataType,
    /// Optional human description.
    pub description: String,
    /// Optional business glossary term.
    pub business_term: String,
    /// Optional measurement unit.
    pub unit: String,
    /// Combined hybrid relevance score.
    pub score: f64,
}

/// Ranked dataset table context injected into the NL2SQL user prompt.
#[derive(Debug, Clone)]
pub struct LinkedTableContext {
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Dataset SQL table name.
    pub dataset_name: String,
    /// Selected columns (`<= 15`).
    pub columns: Vec<LinkedTableColumn>,
    /// Up to 3 PII-masked sample rows formatted as compact JSON objects.
    pub sample_rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Aggregate dataset relevance score.
    pub score: f64,
}

/// Complete schema-linking output consumed by `nl2sql::prompt` and persisted to `an_ask_turns`.
#[derive(Debug, Clone, Default)]
pub struct LinkedSchemaContext {
    /// Top-ranked candidate tables (`<= 5`).
    pub tables: Vec<LinkedTableContext>,
    /// Top-ranked candidate semantic metrics (`<= 5`).
    pub metrics: Vec<MetricDef>,
    /// Matched `(type_name, node_key, title)` business nodes.
    pub matched_nodes: Vec<(String, String, String)>,
    /// Summary identifiers persisted in `an_ask_turns.linked_objects`.
    pub linked_objects: LinkedObjects,
    /// `true` if at least one column, metric, or ontology node had a positive match with the question.
    pub has_positive_hit: bool,
    /// `true` when `restrict_dataset_ids` is active and the question explicitly names an excluded dataset without naming any mounted dataset (`06-STEP20` §20.2.2).
    pub targeted_excluded_dataset: bool,
    /// Legal cross-dataset join edges compiled from published `SchemaDoc`s (`02-STEP16` §16.3.2).
    pub allowed_joins: Vec<AllowedDatasetJoinEdge>,
    /// Published `xBlueStudio` `SchemaDoc` snapshots available to Stage 0 ontology routing (`02-STEP16` §16.2).
    pub published_schemas: Vec<SchemaDoc>,
    /// Whether multi-table SQL must join exclusively along `allowed_joins` (`02-STEP16` §16.3.2).
    pub enforce_ontology_joins: bool,
}

/// Optional extra inputs for in-memory augmentation and sample-row masking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SchemaLinkExtras {
    /// Available semantic metrics in the tenant (used to supplement DB recall on CJK substrings).
    pub all_metrics: Vec<MetricDef>,
    /// Optional up-to-3 raw or masked sample rows keyed by dataset UUID.
    pub sample_rows_by_dataset: HashMap<Uuid, Vec<serde_json::Map<String, serde_json::Value>>>,
    /// Optional explicit dataset UUID whitelist (`AskContext.dataset_ids`).
    pub restrict_dataset_ids: Vec<Uuid>,
    /// Identifiers (`name` and `display_name`) of tenant datasets excluded by `restrict_dataset_ids` (`06-STEP20` §20.2.2).
    #[serde(default)]
    pub excluded_dataset_names: Vec<String>,
    /// Prior SQL text from the active multi-turn session (boosts tables referenced in prior turns).
    pub prior_session_sqls: Vec<String>,
    /// Published `xBlueStudio` `SchemaDoc` snapshots across tenant subgraphs (`02-STEP16` §16.3.2).
    pub published_schemas: Vec<SchemaDoc>,
    /// Enforce ontology join guard on multi-dataset SQL drafts (`02-STEP16` §16.3.2).
    pub enforce_ontology_joins: bool,
}

/// Minimum vector cosine similarity required before treating a vector-only recall as a positive hit.
const MIN_POSITIVE_VEC_SIM: f64 = 0.45;

/// Pure compiler deriving canonical undirected [`AllowedDatasetJoinEdge`] items from published
/// `xBlueStudio` [`SchemaDoc`]s (`02-STEP16` §16.3.1):
/// 1. Cross-object `MappingLinkSpec` foreign keys (`(D1, target_key_column) <-> (D2, key_column)`).
/// 2. Shared `NodeTypeSpec` primary keys across multiple datasets (`SAME_ENTITY:<NodeType>`).
#[must_use]
pub fn compile_ontology_join_edges(schemas: &[SchemaDoc]) -> Vec<AllowedDatasetJoinEdge> {
    let mappings: Vec<&MappingSpec> = schemas
        .iter()
        .flat_map(|s| s.mappings.iter())
        .filter(|m| {
            !m.dataset.trim().is_empty()
                && !m.node_type.trim().is_empty()
                && !m.key_column.trim().is_empty()
        })
        .collect();

    let mut edges = BTreeSet::new();

    for src in &mappings {
        let d1 = src.dataset.trim().to_ascii_lowercase();
        for link in &src.links {
            let fk1 = link.target_key_column.trim().to_ascii_lowercase();
            let target_type = link.target_type.trim();
            let rel_label = link.link_type.trim();
            if fk1.is_empty() || target_type.is_empty() || rel_label.is_empty() {
                continue;
            }
            for dst in &mappings {
                if !dst.node_type.trim().eq_ignore_ascii_case(target_type) {
                    continue;
                }
                let d2 = dst.dataset.trim().to_ascii_lowercase();
                let k2 = dst.key_column.trim().to_ascii_lowercase();
                if let Some(edge) = canonical_join_edge(&d1, &fk1, &d2, &k2, rel_label) {
                    edges.insert(edge);
                }
            }
        }
    }

    for (idx, ma) in mappings.iter().enumerate() {
        let tail = mappings.get(idx.saturating_add(1)..).unwrap_or(&[]);
        for mb in tail {
            if !ma
                .node_type
                .trim()
                .eq_ignore_ascii_case(mb.node_type.trim())
            {
                continue;
            }
            let d1 = ma.dataset.trim().to_ascii_lowercase();
            let k1 = ma.key_column.trim().to_ascii_lowercase();
            let d2 = mb.dataset.trim().to_ascii_lowercase();
            let k2 = mb.key_column.trim().to_ascii_lowercase();
            let label = format!("SAME_ENTITY:{}", ma.node_type.trim());
            if let Some(edge) = canonical_join_edge(&d1, &k1, &d2, &k2, &label) {
                edges.insert(edge);
            }
        }
    }

    edges.into_iter().collect()
}

fn canonical_join_edge(
    d1: &str,
    c1: &str,
    d2: &str,
    c2: &str,
    label: &str,
) -> Option<AllowedDatasetJoinEdge> {
    if d1.is_empty() || c1.is_empty() || d2.is_empty() || c2.is_empty() {
        return None;
    }
    if d1 == d2 && c1 == c2 {
        return None;
    }
    if (d1, c1) <= (d2, c2) {
        Some(AllowedDatasetJoinEdge {
            left_dataset: String::from(d1),
            left_column: String::from(c1),
            right_dataset: String::from(d2),
            right_column: String::from(c2),
            relationship_label: String::from(label),
        })
    } else {
        Some(AllowedDatasetJoinEdge {
            left_dataset: String::from(d2),
            left_column: String::from(c2),
            right_dataset: String::from(d1),
            right_column: String::from(c1),
            relationship_label: String::from(label),
        })
    }
}

/// Computes the canonical `05-STEP04` §04.6.3 hybrid score (`0.5 * fts + 0.5 * vec`, plus `0.1` if `certified`).
#[must_use]
pub fn hybrid_link_score(fts_score: f64, vec_score: f64, certified: bool) -> f64 {
    let fts = fts_score.clamp(0.0, 1.0);
    let vec_s = vec_score.clamp(0.0, 1.0);
    let base = 0.5 * fts + 0.5 * vec_s;
    if certified { base + 0.1 } else { base }
}

/// Builds a ranked [`LinkedSchemaContext`] by combining `db_recall` with lexical CJK bigram /
/// identifier matching against `catalog` (`05-STEP04` §04.6.3).
#[must_use]
pub fn build_linked_schema(
    question: &str,
    catalog: &TenantCatalog,
    db_recall: &SchemaRecallBundle,
    extras: &SchemaLinkExtras,
) -> LinkedSchemaContext {
    let restrict_set: BTreeSet<Uuid> = extras.restrict_dataset_ids.iter().copied().collect();
    let merged_cols = merge_column_candidates(
        question,
        catalog,
        &db_recall.columns,
        &restrict_set,
        &extras.prior_session_sqls,
    );
    let merged_metrics = merge_metric_candidates(
        question,
        catalog,
        &db_recall.metrics,
        &extras.all_metrics,
        &restrict_set,
    );

    let published_schemas = if extras.published_schemas.is_empty() {
        catalog.published_schemas.clone()
    } else {
        extras.published_schemas.clone()
    };

    if is_targeting_excluded_dataset(
        question,
        catalog,
        &restrict_set,
        &extras.excluded_dataset_names,
    ) {
        return LinkedSchemaContext {
            tables: Vec::new(),
            metrics: Vec::new(),
            matched_nodes: Vec::new(),
            linked_objects: LinkedObjects::default(),
            has_positive_hit: false,
            targeted_excluded_dataset: true,
            allowed_joins: Vec::new(),
            published_schemas,
            enforce_ontology_joins: extras.enforce_ontology_joins,
        };
    }

    let has_positive_hit = merged_cols
        .iter()
        .any(|(hit, s)| *s > 0.05 && is_substantive_column_hit(question, hit))
        || merged_metrics.iter().any(|(_, s)| *s > 0.12)
        || !db_recall.matched_nodes.is_empty()
        || question_matches_mapped_node_type(question, &published_schemas, catalog)
        || question_mentions_mounted_dataset(question, catalog, &restrict_set);

    let selected_metrics: Vec<MetricDef> = merged_metrics
        .into_iter()
        .filter(|(m, score)| {
            *score > 0.0
                || !restrict_set.is_empty()
                || extras.all_metrics.len() <= MAX_LINKED_METRICS && !is_metric_broken(m, catalog)
        })
        .take(MAX_LINKED_METRICS)
        .map(|(m, _)| m)
        .collect();

    let tables = select_top_tables(
        catalog,
        &merged_cols,
        &selected_metrics,
        &restrict_set,
        &extras.sample_rows_by_dataset,
    );

    let allowed_joins = resolve_context_allowed_joins(&published_schemas, catalog, &tables);
    let linked_objects =
        build_linked_objects_summary(&tables, &selected_metrics, &db_recall.matched_nodes);

    LinkedSchemaContext {
        tables,
        metrics: selected_metrics,
        matched_nodes: db_recall.matched_nodes.clone(),
        linked_objects,
        has_positive_hit,
        targeted_excluded_dataset: false,
        allowed_joins,
        published_schemas,
        enforce_ontology_joins: extras.enforce_ontology_joins,
    }
}

fn is_targeting_excluded_dataset(
    question: &str,
    catalog: &TenantCatalog,
    restrict_set: &BTreeSet<Uuid>,
    excluded_dataset_names: &[String],
) -> bool {
    if restrict_set.is_empty() || excluded_dataset_names.is_empty() {
        return false;
    }
    let q_lower = question.trim().to_lowercase();
    if q_lower.is_empty() {
        return false;
    }
    let mentions_excluded = excluded_dataset_names
        .iter()
        .any(|ident| mentions_dataset_identifier(&q_lower, ident));
    if !mentions_excluded {
        return false;
    }
    let mentions_mounted = catalog
        .datasets
        .values()
        .filter(|d| restrict_set.contains(&d.id))
        .any(|d| {
            mentions_dataset_identifier(&q_lower, &d.name)
                || mentions_dataset_identifier(&q_lower, &d.display_name)
        });
    !mentions_mounted
}

fn question_mentions_mounted_dataset(
    question: &str,
    catalog: &TenantCatalog,
    restrict_set: &BTreeSet<Uuid>,
) -> bool {
    let q_lower = question.trim().to_lowercase();
    if q_lower.is_empty() {
        return false;
    }
    catalog
        .datasets
        .values()
        .filter(|d| restrict_set.is_empty() || restrict_set.contains(&d.id))
        .any(|d| {
            mentions_dataset_identifier(&q_lower, &d.name)
                || mentions_dataset_identifier(&q_lower, &d.display_name)
        })
}

fn mentions_dataset_identifier(q_lower: &str, ident: &str) -> bool {
    let trimmed = ident.trim().to_lowercase();
    if trimmed.len() < 3 || trimmed.chars().count() < 2 {
        return false;
    }
    if !trimmed.is_ascii() {
        return q_lower.contains(&trimmed);
    }
    for (byte_idx, _) in q_lower.match_indices(&trimmed) {
        let prev_ok = q_lower
            .get(..byte_idx)
            .and_then(|s| s.chars().next_back())
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        let end_idx = byte_idx.saturating_add(trimmed.len());
        let next_ok = q_lower
            .get(end_idx..)
            .and_then(|s| s.chars().next())
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        if prev_ok && next_ok {
            return true;
        }
    }
    false
}

fn build_linked_objects_summary(
    tables: &[LinkedTableContext],
    selected_metrics: &[MetricDef],
    matched_nodes: &[(String, String, String)],
) -> LinkedObjects {
    let mut linked_objects = LinkedObjects::default();
    for t in tables {
        linked_objects.datasets.push(t.dataset_name.clone());
        for c in &t.columns {
            linked_objects
                .columns
                .push(format!("{}.{}", t.dataset_name, c.name));
        }
    }
    for m in selected_metrics {
        linked_objects.metrics.push(m.name.clone());
    }
    for (type_name, node_key, _) in matched_nodes {
        linked_objects.nodes.push(format!("{type_name}:{node_key}"));
    }
    linked_objects
}

fn resolve_context_allowed_joins(
    published_schemas: &[SchemaDoc],
    catalog: &TenantCatalog,
    tables: &[LinkedTableContext],
) -> Vec<AllowedDatasetJoinEdge> {
    let compiled = if published_schemas.is_empty() {
        catalog.allowed_joins.clone()
    } else {
        compile_ontology_join_edges(published_schemas)
    };
    let table_names: BTreeSet<String> = tables
        .iter()
        .map(|t| t.dataset_name.to_ascii_lowercase())
        .collect();
    compiled
        .into_iter()
        .filter(|e| {
            catalog.dataset(&e.left_dataset).is_some()
                && catalog.dataset(&e.right_dataset).is_some()
                && (table_names.is_empty()
                    || table_names.contains(&e.left_dataset)
                    || table_names.contains(&e.right_dataset))
        })
        .collect()
}

pub(crate) fn question_matches_mapped_node_type(
    question: &str,
    schemas: &[SchemaDoc],
    catalog: &TenantCatalog,
) -> bool {
    let q_lower = question.trim().to_lowercase();
    if q_lower.is_empty() {
        return false;
    }
    let mapped_types: BTreeSet<String> = schemas
        .iter()
        .flat_map(|s| s.mappings.iter())
        .filter(|m| catalog.dataset(&m.dataset).is_some())
        .map(|m| m.node_type.trim().to_ascii_lowercase())
        .collect();
    if mapped_types.is_empty() {
        return false;
    }
    for schema in schemas {
        for nt in &schema.node_types {
            if !mapped_types.contains(&nt.name.trim().to_ascii_lowercase()) {
                continue;
            }
            let n_low = nt.name.trim().to_lowercase();
            let d_low = nt.display_name.trim().to_lowercase();
            if (n_low.chars().count() >= 2 && q_lower.contains(&n_low))
                || (d_low.chars().count() >= 2 && q_lower.contains(&d_low))
            {
                return true;
            }
        }
    }
    false
}

fn merge_column_candidates(
    question: &str,
    catalog: &TenantCatalog,
    recalled: &[LinkedColumnHit],
    restrict_set: &BTreeSet<Uuid>,
    prior_sqls: &[String],
) -> Vec<(LinkedColumnHit, f64)> {
    let mut by_key: HashMap<(Uuid, String), (LinkedColumnHit, f64)> = HashMap::new();

    for hit in recalled {
        if !restrict_set.is_empty() && !restrict_set.contains(&hit.dataset_id) {
            continue;
        }
        let lexical = lexical_column_score(
            question,
            &hit.dataset_name,
            &hit.column_name,
            &hit.description,
            &hit.business_term,
        );
        let eff_fts = hit.fts_score.max(lexical);
        let eff_vec = if hit.vec_score >= MIN_POSITIVE_VEC_SIM {
            hit.vec_score
        } else {
            0.0
        };
        let score = hybrid_link_score(eff_fts, eff_vec, false);
        by_key.insert(
            (hit.dataset_id, hit.column_name.to_ascii_lowercase()),
            (hit.clone(), score),
        );
    }

    for entry in catalog.datasets.values() {
        if !restrict_set.is_empty() && !restrict_set.contains(&entry.id) {
            continue;
        }
        let prior_boost = if prior_sqls.iter().any(|s| {
            s.to_ascii_lowercase()
                .contains(&entry.name.to_ascii_lowercase())
        }) {
            0.35
        } else {
            0.0
        };

        for meta in entry.columns.values() {
            let key = (entry.id, meta.column_name.to_ascii_lowercase());
            let lex = lexical_column_score(
                question,
                &entry.name,
                &meta.column_name,
                &meta.description,
                &meta.business_term,
            );
            let combined_lex = (lex + prior_boost).clamp(0.0, 1.0);
            if let Some((existing_hit, existing_score)) = by_key.get_mut(&key) {
                let eff_vec = if existing_hit.vec_score >= MIN_POSITIVE_VEC_SIM {
                    existing_hit.vec_score
                } else {
                    0.0
                };
                let boosted =
                    hybrid_link_score(existing_hit.fts_score.max(combined_lex), eff_vec, false);
                if boosted > *existing_score {
                    *existing_score = boosted;
                }
            } else {
                let hit = LinkedColumnHit {
                    dataset_id: entry.id,
                    dataset_name: entry.name.clone(),
                    column_name: meta.column_name.clone(),
                    data_type: meta.data_type,
                    description: meta.description.clone(),
                    business_term: meta.business_term.clone(),
                    unit: meta.unit.clone(),
                    pii_category: meta.pii_category,
                    sample_values: Vec::new(),
                    fts_score: combined_lex,
                    vec_score: 0.0,
                };
                let score = hybrid_link_score(combined_lex, 0.0, false);
                by_key.insert(key, (hit, score));
            }
        }
    }

    let mut list: Vec<(LinkedColumnHit, f64)> = by_key.into_values().collect();
    list.sort_by(|(ha, sa), (hb, sb)| {
        sb.partial_cmp(sa)
            .unwrap_or(Ordering::Equal)
            .then_with(|| ha.dataset_name.cmp(&hb.dataset_name))
            .then_with(|| ha.column_name.cmp(&hb.column_name))
    });
    list
}

fn merge_metric_candidates(
    question: &str,
    catalog: &TenantCatalog,
    recalled: &[LinkedMetricHit],
    all_metrics: &[MetricDef],
    restrict_set: &BTreeSet<Uuid>,
) -> Vec<(MetricDef, f64)> {
    let mut by_name: HashMap<String, (MetricDef, f64)> = HashMap::new();

    for hit in recalled {
        if is_metric_broken(&hit.metric, catalog) {
            continue;
        }
        if !restrict_set.is_empty() && !restrict_set.contains(&hit.metric.dataset_id) {
            continue;
        }
        let lex = lexical_metric_score(question, &hit.metric);
        let eff_vec = if hit.vec_score >= MIN_POSITIVE_VEC_SIM {
            hit.vec_score
        } else {
            0.0
        };
        let score = hybrid_link_score(hit.fts_score.max(lex), eff_vec, hit.metric.certified);
        by_name.insert(
            hit.metric.name.to_ascii_lowercase(),
            (hit.metric.clone(), score),
        );
    }

    for m in all_metrics {
        if is_metric_broken(m, catalog) {
            continue;
        }
        if !restrict_set.is_empty() && !restrict_set.contains(&m.dataset_id) {
            continue;
        }
        let lex = lexical_metric_score(question, m);
        let score = hybrid_link_score(lex, 0.0, m.certified);
        let key = m.name.to_ascii_lowercase();
        if let Some((_, existing)) = by_name.get_mut(&key) {
            if score > *existing {
                *existing = score;
            }
        } else if lex > 0.0 || all_metrics.len() <= MAX_LINKED_METRICS {
            by_name.insert(key, (m.clone(), score));
        }
    }

    let mut list: Vec<(MetricDef, f64)> = by_name.into_values().collect();
    list.sort_by(|(ma, sa), (mb, sb)| {
        sb.partial_cmp(sa)
            .unwrap_or(Ordering::Equal)
            .then_with(|| ma.name.cmp(&mb.name))
    });
    list
}

fn select_top_tables(
    catalog: &TenantCatalog,
    scored_cols: &[(LinkedColumnHit, f64)],
    selected_metrics: &[MetricDef],
    restrict_set: &BTreeSet<Uuid>,
    sample_rows_map: &HashMap<Uuid, Vec<serde_json::Map<String, serde_json::Value>>>,
) -> Vec<LinkedTableContext> {
    let mut ds_scores: BTreeMap<Uuid, f64> = BTreeMap::new();
    for (hit, score) in scored_cols {
        let entry = ds_scores.entry(hit.dataset_id).or_insert(0.0);
        if *score > *entry {
            *entry = *score;
        }
    }
    // Ensure backing datasets of selected metrics are also included in table context.
    for m in selected_metrics {
        let entry = ds_scores.entry(m.dataset_id).or_insert(0.0);
        *entry = (*entry).max(0.4);
    }

    let mut ranked_ds: Vec<(Uuid, String, f64)> = catalog
        .datasets
        .values()
        .filter(|d| restrict_set.is_empty() || restrict_set.contains(&d.id))
        .map(|d| {
            (
                d.id,
                d.name.clone(),
                ds_scores.get(&d.id).copied().unwrap_or(0.0),
            )
        })
        .collect();

    ranked_ds.sort_by(|(_, na, sa), (_, nb, sb)| {
        sb.partial_cmp(sa)
            .unwrap_or(Ordering::Equal)
            .then_with(|| na.cmp(nb))
    });

    let mut out = Vec::new();
    for (ds_id, _ds_name, ds_score) in ranked_ds.into_iter().take(MAX_LINKED_DATASETS) {
        let Some(ds_entry) = catalog.dataset_by_id(ds_id) else {
            continue;
        };
        let cols = pick_columns_for_dataset(ds_entry, scored_cols);
        let masked_samples = mask_sample_rows_for_dataset(ds_entry, sample_rows_map.get(&ds_id));
        out.push(LinkedTableContext {
            dataset_id: ds_id,
            dataset_name: ds_entry.name.clone(),
            columns: cols,
            sample_rows: masked_samples,
            score: ds_score,
        });
    }
    out
}

fn pick_columns_for_dataset(
    ds_entry: &CatalogDatasetEntry,
    scored_cols: &[(LinkedColumnHit, f64)],
) -> Vec<LinkedTableColumn> {
    let mut col_scores: HashMap<String, f64> = HashMap::new();
    for (hit, s) in scored_cols {
        if hit.dataset_id == ds_entry.id {
            col_scores.insert(hit.column_name.to_ascii_lowercase(), *s);
        }
    }

    let mut candidates: Vec<(i32, LinkedTableColumn)> = ds_entry
        .columns
        .values()
        .map(|meta| {
            let mut s = col_scores
                .get(&meta.column_name.to_ascii_lowercase())
                .copied()
                .unwrap_or(0.0);
            // Slight structural tie-breaker for ID/foreign-key/date columns so JOINs/time filters work cleanly.
            if meta.column_name.ends_with("_id")
                || meta.column_name == "id"
                || matches!(meta.data_type, DataType::Date | DataType::Timestamp)
            {
                s += 0.01;
            }
            (
                meta.ordinal,
                LinkedTableColumn {
                    name: meta.column_name.clone(),
                    data_type: meta.data_type,
                    description: meta.description.clone(),
                    business_term: meta.business_term.clone(),
                    unit: meta.unit.clone(),
                    score: s,
                },
            )
        })
        .collect();

    if candidates.len() > MAX_COLS_PER_DATASET {
        candidates.sort_by(|(oa, ca), (ob, cb)| {
            cb.score
                .partial_cmp(&ca.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| oa.cmp(ob))
        });
        candidates.truncate(MAX_COLS_PER_DATASET);
    }
    candidates.sort_by(|(oa, ca), (ob, cb)| oa.cmp(ob).then_with(|| ca.name.cmp(&cb.name)));
    candidates.into_iter().map(|(_, c)| c).collect()
}

fn mask_sample_rows_for_dataset(
    ds_entry: &CatalogDatasetEntry,
    raw_rows: Option<&Vec<serde_json::Map<String, serde_json::Value>>>,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let Some(rows) = raw_rows else {
        return Vec::new();
    };
    rows.iter()
        .take(3)
        .map(|row| {
            let mut masked_map = serde_json::Map::new();
            for (k, v) in row {
                let pii = ds_entry
                    .columns
                    .get(&k.to_ascii_lowercase())
                    .map_or(PiiCategory::None, |m| m.pii_category);
                if pii != PiiCategory::None && !v.is_null() {
                    let raw_s = v.as_str().map_or_else(|| v.to_string(), String::from);
                    masked_map.insert(
                        k.clone(),
                        serde_json::Value::String(mask_pii_str(&raw_s, pii)),
                    );
                } else {
                    masked_map.insert(k.clone(), v.clone());
                }
            }
            masked_map
        })
        .collect()
}

fn lexical_column_score(
    question: &str,
    ds_name: &str,
    col_name: &str,
    desc: &str,
    term: &str,
) -> f64 {
    let q_lower = question.to_ascii_lowercase();
    let mut score: f64 = 0.0;
    for candidate in [col_name, term, desc, ds_name] {
        score = score.max(text_overlap_score(&q_lower, candidate));
    }
    // Common bilingual analytical concept bridges (e.g. 金额->amount/price, 订单->orders, 状态->status, 数量->qty/count)
    score = score.max(concept_synonym_score(&q_lower, ds_name, col_name));
    score.clamp(0.0, 1.0)
}

fn is_substantive_column_hit(question: &str, hit: &LinkedColumnHit) -> bool {
    if !matches!(hit.data_type, DataType::Date | DataType::Timestamp) {
        return true;
    }
    let q_lower = question.to_ascii_lowercase();
    hit.vec_score >= MIN_POSITIVE_VEC_SIM
        || text_overlap_score(&q_lower, &hit.column_name) > 0.0
        || text_overlap_score(&q_lower, &hit.dataset_name) > 0.0
        || text_overlap_score(&q_lower, &hit.business_term) > 0.0
        || text_overlap_score(&q_lower, &hit.description) > 0.0
}

pub(crate) fn lexical_metric_score(question: &str, metric: &MetricDef) -> f64 {
    let q_lower = question.to_ascii_lowercase();
    let mut best = text_overlap_score(&q_lower, &metric.name)
        .max(text_overlap_score(&q_lower, &metric.display_name))
        .max(text_overlap_score(&q_lower, &metric.description));
    for syn in &metric.synonyms {
        best = best.max(text_overlap_score(&q_lower, syn));
    }
    best.clamp(0.0, 1.0)
}

fn text_overlap_score(q_lower: &str, target: &str) -> f64 {
    let t_lower = target.trim().to_ascii_lowercase();
    if t_lower.is_empty() || q_lower.is_empty() {
        return 0.0;
    }
    if q_lower.contains(&t_lower) {
        return 0.95;
    }
    // Check underscore-split ASCII tokens (e.g. `demo_orders` -> `orders`, `order_date` -> `order`, `date`)
    for part in t_lower.split(|c: char| !c.is_alphanumeric()) {
        if part.len() >= 3 && q_lower.contains(part) {
            return 0.75;
        }
    }
    // CJK character bigram overlap
    let q_cjk: Vec<char> = q_lower.chars().filter(|c| !c.is_ascii()).collect();
    let t_cjk: Vec<char> = t_lower.chars().filter(|c| !c.is_ascii()).collect();
    if q_cjk.len() >= 2 && t_cjk.len() >= 2 {
        let mut q_bigrams = BTreeSet::new();
        for win in q_cjk.windows(2) {
            if let (Some(&a), Some(&b)) = (win.first(), win.get(1)) {
                q_bigrams.insert((a, b));
            }
        }
        let mut hits: u32 = 0;
        let total = u32::try_from(t_cjk.len().saturating_sub(1)).unwrap_or(1);
        for win in t_cjk.windows(2) {
            if let (Some(&a), Some(&b)) = (win.first(), win.get(1))
                && q_bigrams.contains(&(a, b))
            {
                hits = hits.saturating_add(1);
            }
        }
        if hits > 0 {
            return (f64::from(hits) / f64::from(total)).clamp(0.25, 0.9);
        }
    }
    0.0
}

fn concept_synonym_score(q_lower: &str, ds_name: &str, col_name: &str) -> f64 {
    let ds_l = ds_name.to_ascii_lowercase();
    let col_l = col_name.to_ascii_lowercase();
    let pairs: &[(&[&str], &[&str])] = &[
        (&["订单", "单据"], &["order", "orders"]),
        (
            &["金额", "销售额", "收入", "花费", "总价"],
            &["amount", "price", "revenue", "gmv", "total"],
        ),
        (
            &["支付", "已付", "状态", "退款", "完成"],
            &["status", "state", "paid"],
        ),
        (&["客户", "用户", "买家"], &["customer", "user", "buyer"]),
        (
            &["区域", "大区", "地区", "省份", "华东", "华南", "华北"],
            &["region", "area", "zone"],
        ),
        (&["城市", "上海", "北京", "杭州", "深圳", "广州"], &["city"]),
        (
            &["仓库", "库房", "工厂"],
            &["warehouse", "factory", "plant"],
        ),
        (
            &["库存", "存量"],
            &["inventory", "stock", "qty", "quantity"],
        ),
        (
            &["商品", "产品", "品类", "类别"],
            &["product", "item", "category", "sku"],
        ),
        (
            &["时间", "日期", "月", "年", "天", "季度"],
            &["date", "time", "ts", "created_at", "updated_at", "month"],
        ),
    ];

    for (cn_terms, en_terms) in pairs {
        if cn_terms.iter().any(|cn| q_lower.contains(cn))
            && en_terms
                .iter()
                .any(|en| matches_en_concept_token(&col_l, &ds_l, en))
        {
            return 0.65;
        }
    }
    0.0
}

fn matches_en_concept_token(col_l: &str, ds_l: &str, en: &str) -> bool {
    if en.len() <= 2 {
        return col_l.split('_').any(|p| p == en) || ds_l.split('_').any(|p| p == en);
    }
    col_l.contains(en) || ds_l.contains(en)
}

#[cfg(test)]
mod tests {
    use std::future::ready;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use axpoint_domain::AppError;
    use axpoint_domain::analytics::{AskContext, AskIntent, AskRequest, MetricDimension};
    use axpoint_domain::axhub::ColumnMeta;
    use axpoint_domain::enums::{GroundingMode, RollupGrain};
    use axpoint_domain::ports::AiPort;
    use axpoint_domain::{ExtractedTriple, FusedHit};
    use chrono::Utc;

    use super::*;
    use crate::nl2sql::{AskPipelineInput, run_ask_pipeline_conn};

    struct ZeroCallAi {
        calls: AtomicUsize,
    }

    impl AiPort for ZeroCallAi {
        fn embedding_model_name(&self) -> &'static str {
            "zero-call"
        }

        fn current_embedding_dim(&self) -> Option<usize> {
            Some(8)
        }

        fn embed_batch(
            &self,
            inputs: &[String],
        ) -> impl Future<Output = Result<(Vec<Vec<f32>>, usize), AppError>> + Send {
            ready(Ok((vec![vec![0.0_f32; 8]; inputs.len()], 8)))
        }

        fn embed_one(
            &self,
            _text: &str,
        ) -> impl Future<Output = Result<(Vec<f32>, usize), AppError>> + Send {
            ready(Ok((vec![0.0_f32; 8], 8)))
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
            ready(Ok(String::new()))
        }

        fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> impl Future<Output = Result<String, AppError>> + Send {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            ready(Err(AppError::Llm(String::from(
                "unexpected LLM call during deterministic abstention",
            ))))
        }
    }

    fn mk_col(ds_id: Uuid, name: &str, dt: DataType, term: &str, ordinal: i32) -> ColumnMeta {
        ColumnMeta {
            dataset_id: ds_id,
            column_name: String::from(name),
            data_type: dt,
            pii_category: PiiCategory::None,
            pii_confirmed: false,
            description: String::from(term),
            business_term: String::from(term),
            unit: String::new(),
            ordinal,
        }
    }

    fn sample_scoped_catalog() -> (TenantCatalog, Uuid, Uuid, Vec<MetricDef>) {
        let ds_first_id = Uuid::from_u128(0xA001);
        let ds_second_id = Uuid::from_u128(0xA002);

        let mut ds_first = CatalogDatasetEntry::simple(ds_first_id, "demo_orders", "demo_orders");
        ds_first.display_name = String::from("演示订单表");
        ds_first.columns.insert(
            String::from("order_id"),
            mk_col(ds_first_id, "order_id", DataType::String, "订单号", 0),
        );
        ds_first.columns.insert(
            String::from("warehouse_code"),
            mk_col(
                ds_first_id,
                "warehouse_code",
                DataType::String,
                "仓库编码",
                1,
            ),
        );
        ds_first.columns.insert(
            String::from("region"),
            mk_col(ds_first_id, "region", DataType::String, "区域", 2),
        );
        ds_first.columns.insert(
            String::from("amount"),
            mk_col(ds_first_id, "amount", DataType::Float64, "订单金额", 3),
        );

        let mut ds_second =
            CatalogDatasetEntry::simple(ds_second_id, "demo_warehouses", "demo_warehouses");
        ds_second.display_name = String::from("隔离对照仓");
        ds_second.columns.insert(
            String::from("warehouse_code"),
            mk_col(
                ds_second_id,
                "warehouse_code",
                DataType::String,
                "仓库编码",
                0,
            ),
        );
        ds_second.columns.insert(
            String::from("isolated_vault_qty"),
            mk_col(
                ds_second_id,
                "isolated_vault_qty",
                DataType::Int64,
                "隔离库存",
                1,
            ),
        );

        let catalog = TenantCatalog::new("default")
            .with_dataset(ds_first)
            .with_dataset(ds_second);
        let metrics = sample_scoped_metrics(ds_first_id, ds_second_id);
        (catalog, ds_first_id, ds_second_id, metrics)
    }

    fn sample_scoped_metrics(ds_first_id: Uuid, ds_second_id: Uuid) -> Vec<MetricDef> {
        let now = Utc::now();
        vec![
            MetricDef {
                id: Uuid::from_u128(0xB001),
                name: String::from("paid_amount"),
                display_name: String::from("已支付金额"),
                description: String::from("订单总金额"),
                dataset_id: ds_first_id,
                expression: String::from("SUM(amount)"),
                filter_sql: String::new(),
                time_column: None,
                default_grain: RollupGrain::Day,
                dimensions: vec![MetricDimension {
                    name: String::from("region"),
                    column: String::from("region"),
                    display_name: String::from("区域"),
                }],
                unit: String::from("CNY"),
                format: String::from("0.00"),
                synonyms: Vec::new(),
                certified: true,
                schema_version: 1,
                broken: false,
                owner_user_id: None,
                version: 1,
                created_at: now,
                updated_at: now,
            },
            MetricDef {
                id: Uuid::from_u128(0xB002),
                name: String::from("vault_stock"),
                display_name: String::from("隔离仓总库存"),
                description: String::from("隔离对照仓库存合计"),
                dataset_id: ds_second_id,
                expression: String::from("SUM(isolated_vault_qty)"),
                filter_sql: String::new(),
                time_column: None,
                default_grain: RollupGrain::Day,
                dimensions: Vec::new(),
                unit: String::from("件"),
                format: String::from("0"),
                synonyms: Vec::new(),
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

    #[test]
    fn restrict_dataset_ids_isolates_tables_and_metrics() {
        let (catalog, ds_first_id, _ds_second_id, metrics) = sample_scoped_catalog();
        let linked = build_linked_schema(
            "按区域统计已支付金额",
            &catalog,
            &SchemaRecallBundle::default(),
            &SchemaLinkExtras {
                all_metrics: metrics,
                restrict_dataset_ids: vec![ds_first_id],
                excluded_dataset_names: vec![
                    String::from("demo_warehouses"),
                    String::from("隔离对照仓"),
                ],
                ..SchemaLinkExtras::default()
            },
        );

        assert!(linked.has_positive_hit);
        assert!(!linked.targeted_excluded_dataset);
        assert_eq!(linked.tables.len(), 1);
        assert_eq!(
            linked.tables.first().map(|t| t.dataset_id),
            Some(ds_first_id)
        );
        assert_eq!(linked.metrics.len(), 1);
        assert_eq!(
            linked.metrics.first().map(|m| m.name.as_str()),
            Some("paid_amount")
        );
        assert_eq!(linked.linked_objects.datasets, vec!["demo_orders"]);
    }

    #[test]
    fn ungrounded_question_under_restricted_scope_does_not_set_positive_hit() {
        let (catalog, ds_first_id, _, metrics) = sample_scoped_catalog();
        let linked = build_linked_schema(
            "火星反物质反应堆堆芯温度是多少",
            &catalog,
            &SchemaRecallBundle::default(),
            &SchemaLinkExtras {
                all_metrics: metrics,
                restrict_dataset_ids: vec![ds_first_id],
                excluded_dataset_names: vec![String::from("demo_warehouses")],
                ..SchemaLinkExtras::default()
            },
        );

        assert!(!linked.has_positive_hit);
        assert!(!linked.targeted_excluded_dataset);
    }

    #[test]
    fn excluded_dataset_names_forces_abstention_even_with_shared_column_substrings() {
        let (catalog, ds_first_id, _, metrics) = sample_scoped_catalog();
        for q in [
            "统计表 demo_warehouses 中的记录总数",
            "统计表 step20_isolated_vault 中的记录总数",
            "查询隔离对照仓里的总行数",
        ] {
            let linked = build_linked_schema(
                q,
                &catalog,
                &SchemaRecallBundle::default(),
                &SchemaLinkExtras {
                    all_metrics: metrics.clone(),
                    restrict_dataset_ids: vec![ds_first_id],
                    excluded_dataset_names: vec![
                        String::from("demo_warehouses"),
                        String::from("step20_isolated_vault"),
                        String::from("隔离对照仓"),
                    ],
                    ..SchemaLinkExtras::default()
                },
            );
            assert!(
                !linked.has_positive_hit,
                "expected has_positive_hit == false for '{q}'"
            );
            assert!(
                linked.targeted_excluded_dataset,
                "expected targeted_excluded_dataset == true for '{q}'"
            );
            assert_eq!(linked.tables.len(), 0);
            assert_eq!(linked.metrics.len(), 0);
            assert_eq!(linked.linked_objects.datasets.len(), 0);
        }
    }

    #[test]
    fn mounted_dataset_name_and_display_name_match_normally_when_in_scope() {
        let (catalog, ds_first_id, _, metrics) = sample_scoped_catalog();
        for q in [
            "统计表 demo_orders 中的记录总数",
            "演示订单表总共有多少条记录",
        ] {
            let linked = build_linked_schema(
                q,
                &catalog,
                &SchemaRecallBundle::default(),
                &SchemaLinkExtras {
                    all_metrics: metrics.clone(),
                    restrict_dataset_ids: vec![ds_first_id],
                    excluded_dataset_names: vec![
                        String::from("orders"),
                        String::from("demo_warehouses"),
                        String::from("隔离对照仓"),
                    ],
                    ..SchemaLinkExtras::default()
                },
            );
            assert!(
                linked.has_positive_hit,
                "expected positive hit for mounted dataset query '{q}'"
            );
            assert!(!linked.targeted_excluded_dataset);
            assert_eq!(linked.linked_objects.datasets, vec!["demo_orders"]);
        }
    }

    #[tokio::test]
    async fn pipeline_abstains_with_zero_llm_calls_when_excluded_dataset_queried()
    -> Result<(), AppError> {
        let (mut catalog, ds_first_id, _, mut metrics) = sample_scoped_catalog();
        catalog.datasets.retain(|_, d| d.id == ds_first_id);
        metrics.retain(|m| m.dataset_id == ds_first_id);

        let conn = duckdb::Connection::open_in_memory()
            .map_err(|e| AppError::Database(format!("open duckdb: {e}")))?;
        let ai = ZeroCallAi {
            calls: AtomicUsize::new(0),
        };
        let req = AskRequest {
            question: String::from("统计表 step20_isolated_vault 中的记录总数"),
            session_id: None,
            context: Some(AskContext {
                dataset_ids: vec![ds_first_id],
                ..AskContext::default()
            }),
        };
        let extras = SchemaLinkExtras {
            all_metrics: metrics,
            restrict_dataset_ids: vec![ds_first_id],
            excluded_dataset_names: vec![
                String::from("step20_isolated_vault"),
                String::from("隔离对照仓"),
            ],
            ..SchemaLinkExtras::default()
        };

        let out = run_ask_pipeline_conn(
            &ai,
            &conn,
            &AskPipelineInput {
                request: &req,
                catalog: &catalog,
                db_recall: &SchemaRecallBundle::default(),
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

        assert_eq!(ai.calls.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(out.response.grounding_mode, GroundingMode::Abstained);
        assert!(out.response.abstained);
        assert_eq!(out.response.sql, "");
        assert_eq!(out.response.rows.len(), 0);
        Ok(())
    }
}
