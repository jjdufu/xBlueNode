//! Hybrid FTS + vector schema linking across datasets, columns, metrics, and `BlueNode`s (`05-STEP04` §04.6.3).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use axpoint_domain::analytics::{
    LinkedColumnHit, LinkedMetricHit, LinkedObjects, MetricDef, SchemaRecallBundle,
};
use axpoint_domain::enums::{DataType, PiiCategory};
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
    /// `true` if at least one column or metric had a positive lexical/vector match with the question.
    pub has_positive_hit: bool,
}

/// Optional extra inputs for in-memory augmentation and sample-row masking.
#[derive(Debug, Clone, Default)]
pub struct SchemaLinkExtras {
    /// Available semantic metrics in the tenant (used to supplement DB recall on CJK substrings).
    pub all_metrics: Vec<MetricDef>,
    /// Optional up-to-3 raw or masked sample rows keyed by dataset UUID.
    pub sample_rows_by_dataset: HashMap<Uuid, Vec<serde_json::Map<String, serde_json::Value>>>,
    /// Optional explicit dataset UUID whitelist (`AskContext.dataset_ids`).
    pub restrict_dataset_ids: Vec<Uuid>,
    /// Prior SQL text from the active multi-turn session (boosts tables referenced in prior turns).
    pub prior_session_sqls: Vec<String>,
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

    let has_positive_hit = merged_cols.iter().any(|(_, s)| *s > 0.05)
        || merged_metrics.iter().any(|(_, s)| *s > 0.12)
        || (!restrict_set.is_empty() && !merged_cols.is_empty());

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

    let mut linked_objects = LinkedObjects::default();
    for t in &tables {
        linked_objects.datasets.push(t.dataset_name.clone());
        for c in &t.columns {
            linked_objects
                .columns
                .push(format!("{}.{}", t.dataset_name, c.name));
        }
    }
    for m in &selected_metrics {
        linked_objects.metrics.push(m.name.clone());
    }
    for (type_name, node_key, _) in &db_recall.matched_nodes {
        linked_objects.nodes.push(format!("{type_name}:{node_key}"));
    }

    LinkedSchemaContext {
        tables,
        metrics: selected_metrics,
        matched_nodes: db_recall.matched_nodes.clone(),
        linked_objects,
        has_positive_hit,
    }
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
        let score = hybrid_link_score(eff_fts, hit.vec_score, false);
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
                let boosted = hybrid_link_score(
                    existing_hit.fts_score.max(combined_lex),
                    existing_hit.vec_score,
                    false,
                );
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
        let score = hybrid_link_score(hit.fts_score.max(lex), hit.vec_score, hit.metric.certified);
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

fn lexical_metric_score(question: &str, metric: &MetricDef) -> f64 {
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
                .any(|en| col_l.contains(en) || ds_l.contains(en))
        {
            return 0.65;
        }
    }
    0.0
}
