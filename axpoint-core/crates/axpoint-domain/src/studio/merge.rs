//! Cross-subgraph ontology alignment preview and atomic merge logic (`06-STEP05-06` §05.4.5).

use std::collections::{BTreeMap, BTreeSet};

use crate::AppError;
use crate::enums::{Cardinality, SubgraphMergeStrategy};
use crate::studio::doc::{
    EdgeTypeSpec, MappingSpec, MergeConflict, MergeConflictKind, MergePreview, MergePreviewRequest,
    MergeSubgraphRequest, NodeTypeSpec, SchemaDoc, TypeAlignmentCandidate, TypeMergeResolution,
};

/// Minimum composite similarity score (`[0.0, 1.0]`) for suggesting a synonymous entity alignment (`§05.4.5`).
pub const ALIGNMENT_THRESHOLD: f64 = 0.65;

/// Default bridge edge type name created under [`SubgraphMergeStrategy::BridgeLink`].
pub const DEFAULT_BRIDGE_EDGE_NAME: &str = "SAME_ENTITY_AS";

/// Computes cross-subgraph entity alignment candidates, conflicts, and net-new additions
/// against enterprise `main` (`06-STEP05-06` §05.4.5).
///
/// Composite similarity formula:
/// `0.4 * name_or_display_similarity + 0.4 * property_jaccard + 0.2 * cosine_similarity`
/// (exact `name` match always yields `score = 1.0`).
#[must_use]
pub fn preview_merge(
    subgraph_slug: &str,
    subgraph_version: i32,
    sub_doc: &SchemaDoc,
    main_version: i32,
    main_doc: &SchemaDoc,
    req: &MergePreviewRequest,
    desc_embeddings: Option<&BTreeMap<String, Vec<f32>>>,
) -> MergePreview {
    let alignments = compute_alignments(sub_doc, main_doc, desc_embeddings);
    let explicit_map: BTreeMap<&str, &TypeMergeResolution> = req
        .resolutions
        .iter()
        .map(|r| (r.sub_type.as_str(), r))
        .collect();
    let auto_map: BTreeMap<&str, &TypeAlignmentCandidate> = alignments
        .iter()
        .map(|c| (c.sub_type.as_str(), c))
        .collect();

    let mut rename_map: BTreeMap<String, String> = BTreeMap::new();
    let mut added_node_types = Vec::new();
    let mut bridge_edges: Vec<String> = Vec::new();

    for sub_nt in &sub_doc.node_types {
        if let Some(&res) = explicit_map.get(sub_nt.name.as_str()) {
            match res.strategy {
                SubgraphMergeStrategy::ReuseMain | SubgraphMergeStrategy::ExtendProps => {
                    rename_map.insert(sub_nt.name.clone(), res.main_type.clone());
                }
                SubgraphMergeStrategy::BridgeLink => {
                    let kept = resolve_bridge_type_name(subgraph_slug, &sub_nt.name, main_doc);
                    rename_map.insert(sub_nt.name.clone(), kept.clone());
                    added_node_types.push(kept);
                    let bname = res
                        .bridge_edge_name
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(DEFAULT_BRIDGE_EDGE_NAME);
                    bridge_edges.push(String::from(bname));
                }
            }
        } else if let Some(&cand) = auto_map.get(sub_nt.name.as_str()) {
            rename_map.insert(sub_nt.name.clone(), cand.main_type.clone());
        } else {
            added_node_types.push(sub_nt.name.clone());
        }
    }

    let conflicts = detect_all_conflicts(sub_doc, main_doc, &explicit_map, &auto_map, &rename_map);

    let mut added_edge_types = bridge_edges;
    for sub_e in &sub_doc.edge_types {
        let src = rename_map
            .get(&sub_e.src_type_name)
            .cloned()
            .unwrap_or_else(|| sub_e.src_type_name.clone());
        let dst = rename_map
            .get(&sub_e.dst_type_name)
            .cloned()
            .unwrap_or_else(|| sub_e.dst_type_name.clone());
        let exists_in_main = main_doc
            .edge_types
            .iter()
            .any(|me| me.name == sub_e.name && me.src_type_name == src && me.dst_type_name == dst);
        if !exists_in_main && !added_edge_types.contains(&sub_e.name) {
            added_edge_types.push(sub_e.name.clone());
        }
    }

    let mut added_mappings = Vec::new();
    for sub_m in &sub_doc.mappings {
        if !main_doc.mappings.iter().any(|mm| mm.name == sub_m.name) {
            added_mappings.push(sub_m.name.clone());
        }
    }

    MergePreview {
        subgraph_slug: String::from(subgraph_slug),
        subgraph_version,
        main_version,
        alignments,
        conflicts,
        added_node_types,
        added_edge_types,
        added_mappings,
    }
}

/// Applies a department subgraph merge into `main`'s [`SchemaDoc`] according to [`MergeSubgraphRequest`]
/// (`06-STEP05-06` §05.4.5).
///
/// # Errors
/// - Returns `AppError::BadRequest` (`TYPE_NOT_FOUND`) if an explicit resolution references a non-existent `main_type`.
/// - Returns `AppError::Conflict("SCHEMA_BREAKING_CHANGE")` when `EXTEND_PROPS` (or edge/mapping merge without `force`)
///   encounters unresolved property type, rule, edge, or mapping conflicts.
pub fn apply_merge(
    subgraph_slug: &str,
    sub_doc: &SchemaDoc,
    main_doc: &SchemaDoc,
    req: &MergeSubgraphRequest,
    desc_embeddings: Option<&BTreeMap<String, Vec<f32>>>,
) -> Result<SchemaDoc, AppError> {
    for res in &req.resolutions {
        if !main_doc
            .node_types
            .iter()
            .any(|nt| nt.name == res.main_type)
        {
            return Err(AppError::bad(
                "TYPE_NOT_FOUND",
                format!(
                    "merge resolution target main_type '{}' does not exist in main",
                    res.main_type
                ),
            ));
        }
    }

    let alignments = compute_alignments(sub_doc, main_doc, desc_embeddings);
    let mut merged = main_doc.clone();
    let (rename_map, strategy_by_sub, pending_bridges) =
        classify_and_append_sub_types(subgraph_slug, sub_doc, req, &alignments, &mut merged);

    extend_aligned_node_types(
        sub_doc,
        &mut merged,
        &rename_map,
        &strategy_by_sub,
        req.force,
    )?;
    rewrite_unaligned_agg_sources(&mut merged, &sub_doc.node_types, &rename_map);
    merge_edges(
        &sub_doc.edge_types,
        &pending_bridges,
        &mut merged,
        &rename_map,
        &strategy_by_sub,
        req.force,
    )?;
    merge_mappings(
        &sub_doc.mappings,
        &mut merged,
        &rename_map,
        &strategy_by_sub,
        req.force,
    )?;

    Ok(merged)
}

fn classify_and_append_sub_types(
    subgraph_slug: &str,
    sub_doc: &SchemaDoc,
    req: &MergeSubgraphRequest,
    alignments: &[TypeAlignmentCandidate],
    merged: &mut SchemaDoc,
) -> (
    BTreeMap<String, String>,
    BTreeMap<String, SubgraphMergeStrategy>,
    Vec<EdgeTypeSpec>,
) {
    let auto_map: BTreeMap<&str, &TypeAlignmentCandidate> = alignments
        .iter()
        .map(|c| (c.sub_type.as_str(), c))
        .collect();
    let explicit_map: BTreeMap<&str, &TypeMergeResolution> = req
        .resolutions
        .iter()
        .map(|r| (r.sub_type.as_str(), r))
        .collect();

    let mut rename_map: BTreeMap<String, String> = BTreeMap::new();
    let mut strategy_by_sub: BTreeMap<String, SubgraphMergeStrategy> = BTreeMap::new();
    let mut pending_bridges: Vec<EdgeTypeSpec> = Vec::new();

    for sub_nt in &sub_doc.node_types {
        let resolved: Option<(String, SubgraphMergeStrategy, Option<String>)> =
            if let Some(&res) = explicit_map.get(sub_nt.name.as_str()) {
                Some((
                    res.main_type.clone(),
                    res.strategy,
                    res.bridge_edge_name.clone(),
                ))
            } else {
                auto_map
                    .get(sub_nt.name.as_str())
                    .map(|cand| (cand.main_type.clone(), req.default_strategy, None))
            };

        match resolved {
            None => merged.node_types.push(sub_nt.clone()),
            Some((
                main_target,
                strategy @ (SubgraphMergeStrategy::ReuseMain | SubgraphMergeStrategy::ExtendProps),
                _,
            )) => {
                rename_map.insert(sub_nt.name.clone(), main_target);
                strategy_by_sub.insert(sub_nt.name.clone(), strategy);
            }
            Some((main_target, SubgraphMergeStrategy::BridgeLink, bridge_name_opt)) => {
                let kept_name = resolve_bridge_type_name(subgraph_slug, &sub_nt.name, merged);
                rename_map.insert(sub_nt.name.clone(), kept_name.clone());
                strategy_by_sub.insert(sub_nt.name.clone(), SubgraphMergeStrategy::BridgeLink);

                let mut cloned_nt = sub_nt.clone();
                cloned_nt.id = None;
                cloned_nt.name.clone_from(&kept_name);
                merged.node_types.push(cloned_nt);

                let edge_name = bridge_name_opt
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(DEFAULT_BRIDGE_EDGE_NAME);
                pending_bridges.push(EdgeTypeSpec {
                    id: None,
                    name: String::from(edge_name),
                    src_type_name: kept_name,
                    dst_type_name: main_target,
                    impact_weight: 1.0,
                    cardinality: Cardinality::OneToOne,
                    description: format!("Cross-subgraph bridge from '{subgraph_slug}'"),
                    properties: Vec::new(),
                    propagation: serde_json::Value::Object(serde_json::Map::new()),
                    ..EdgeTypeSpec::default()
                });
            }
        }
    }

    (rename_map, strategy_by_sub, pending_bridges)
}

fn extend_aligned_node_types(
    sub_doc: &SchemaDoc,
    merged: &mut SchemaDoc,
    rename_map: &BTreeMap<String, String>,
    strategy_by_sub: &BTreeMap<String, SubgraphMergeStrategy>,
    force: bool,
) -> Result<(), AppError> {
    for sub_nt in &sub_doc.node_types {
        if strategy_by_sub.get(&sub_nt.name).copied() != Some(SubgraphMergeStrategy::ExtendProps) {
            continue;
        }
        let Some(target_name) = rename_map.get(&sub_nt.name) else {
            continue;
        };
        let Some(main_nt) = merged
            .node_types
            .iter_mut()
            .find(|nt| nt.name == *target_name)
        else {
            continue;
        };

        let mut type_conflicts = Vec::new();
        collect_node_pair_conflicts(sub_nt, main_nt, rename_map, &mut type_conflicts);
        if !type_conflicts.is_empty() && !force {
            return Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"));
        }

        for sp in &sub_nt.properties {
            if !main_nt.properties.iter().any(|mp| mp.name == sp.name) {
                main_nt.properties.push(sp.clone());
            }
        }

        for sr in &sub_nt.agg_rules {
            let already_produced = main_nt
                .agg_rules
                .iter()
                .any(|mr| mr.target_metric == sr.target_metric)
                || main_nt
                    .derived_rules
                    .iter()
                    .any(|dr| dr.enabled && dr.target_metric == sr.target_metric);
            if !already_produced {
                let mut rewritten = sr.clone();
                if let Some(mapped_src) = rename_map.get(&rewritten.source_type_name) {
                    rewritten.source_type_name.clone_from(mapped_src);
                }
                main_nt.agg_rules.push(rewritten);
            }
        }

        for sdr in &sub_nt.derived_rules {
            let already_produced = main_nt
                .derived_rules
                .iter()
                .any(|mdr| mdr.target_metric == sdr.target_metric)
                || main_nt
                    .agg_rules
                    .iter()
                    .any(|mr| mr.target_metric == sdr.target_metric);
            if !already_produced {
                main_nt.derived_rules.push(sdr.clone());
            }
        }
    }
    Ok(())
}

fn rewrite_unaligned_agg_sources(
    merged: &mut SchemaDoc,
    sub_types: &[NodeTypeSpec],
    rename_map: &BTreeMap<String, String>,
) {
    let sub_names: BTreeSet<&str> = sub_types.iter().map(|nt| nt.name.as_str()).collect();
    for nt in &mut merged.node_types {
        if sub_names.contains(nt.name.as_str())
            || rename_map.values().any(|mapped| mapped == &nt.name)
        {
            for r in &mut nt.agg_rules {
                if let Some(mapped) = rename_map.get(&r.source_type_name) {
                    r.source_type_name.clone_from(mapped);
                }
            }
        }
    }
}

fn merge_edges(
    sub_edges: &[EdgeTypeSpec],
    pending_bridges: &[EdgeTypeSpec],
    merged: &mut SchemaDoc,
    rename_map: &BTreeMap<String, String>,
    strategy_by_sub: &BTreeMap<String, SubgraphMergeStrategy>,
    force: bool,
) -> Result<(), AppError> {
    for sub_e in sub_edges {
        let is_reuse = strategy_by_sub.get(&sub_e.src_type_name).copied()
            == Some(SubgraphMergeStrategy::ReuseMain)
            || strategy_by_sub.get(&sub_e.dst_type_name).copied()
                == Some(SubgraphMergeStrategy::ReuseMain);

        let mapped_src = rename_map
            .get(&sub_e.src_type_name)
            .cloned()
            .unwrap_or_else(|| sub_e.src_type_name.clone());
        let mapped_dst = rename_map
            .get(&sub_e.dst_type_name)
            .cloned()
            .unwrap_or_else(|| sub_e.dst_type_name.clone());

        if let Some(existing) = merged.edge_types.iter().find(|me| me.name == sub_e.name) {
            let compatible = existing.src_type_name == mapped_src
                && existing.dst_type_name == mapped_dst
                && existing.cardinality == sub_e.cardinality;
            if !compatible && !is_reuse && !force {
                return Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"));
            }
            continue;
        }

        let mut new_e = sub_e.clone();
        new_e.id = None;
        new_e.src_type_name = mapped_src;
        new_e.dst_type_name = mapped_dst;
        merged.edge_types.push(new_e);
    }

    for bridge in pending_bridges {
        let exists = merged.edge_types.iter().any(|e| {
            e.name == bridge.name
                && e.src_type_name == bridge.src_type_name
                && e.dst_type_name == bridge.dst_type_name
        });
        if !exists {
            merged.edge_types.push(bridge.clone());
        }
    }

    Ok(())
}

fn merge_mappings(
    sub_mappings: &[MappingSpec],
    merged: &mut SchemaDoc,
    rename_map: &BTreeMap<String, String>,
    strategy_by_sub: &BTreeMap<String, SubgraphMergeStrategy>,
    force: bool,
) -> Result<(), AppError> {
    for sub_m in sub_mappings {
        let is_reuse = strategy_by_sub.get(&sub_m.node_type).copied()
            == Some(SubgraphMergeStrategy::ReuseMain);

        let mut rewritten = sub_m.clone();
        if let Some(mapped_nt) = rename_map.get(&rewritten.node_type) {
            rewritten.node_type.clone_from(mapped_nt);
        }
        for link in &mut rewritten.links {
            if let Some(mapped_t) = rename_map.get(&link.target_type) {
                link.target_type.clone_from(mapped_t);
            }
        }

        if let Some(existing) = merged.mappings.iter().find(|mm| mm.name == rewritten.name) {
            if existing != &rewritten && !is_reuse && !force {
                return Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"));
            }
            continue;
        }
        merged.mappings.push(rewritten);
    }
    Ok(())
}

fn resolve_bridge_type_name(subgraph_slug: &str, sub_name: &str, target_doc: &SchemaDoc) -> String {
    if !target_doc.node_types.iter().any(|nt| nt.name == sub_name) {
        return String::from(sub_name);
    }
    let prefix = slug_to_pascal(subgraph_slug);
    let candidate = format!("{prefix}{sub_name}");
    if !target_doc.node_types.iter().any(|nt| nt.name == candidate) {
        return candidate;
    }
    format!("{candidate}Sub")
}

fn slug_to_pascal(slug: &str) -> String {
    let mut out = String::new();
    let mut cap_next = true;
    for ch in slug.chars() {
        if ch.is_ascii_alphanumeric() {
            if cap_next {
                out.push(ch.to_ascii_uppercase());
                cap_next = false;
            } else {
                out.push(ch);
            }
        } else {
            cap_next = true;
        }
    }
    if out.is_empty() {
        String::from("Sub")
    } else {
        out
    }
}

fn compute_alignments(
    sub_doc: &SchemaDoc,
    main_doc: &SchemaDoc,
    desc_embeddings: Option<&BTreeMap<String, Vec<f32>>>,
) -> Vec<TypeAlignmentCandidate> {
    let mut out = Vec::new();
    for sub_nt in &sub_doc.node_types {
        let mut best: Option<(String, f64, bool)> = None;
        for main_nt in &main_doc.node_types {
            let score = alignment_score(sub_nt, main_nt, desc_embeddings);
            if score >= ALIGNMENT_THRESHOLD {
                let replace = best
                    .as_ref()
                    .is_none_or(|(_, best_score, _)| score > *best_score);
                if replace {
                    let has_conflict = has_direct_node_conflict(sub_nt, main_nt);
                    best = Some((main_nt.name.clone(), score, has_conflict));
                }
            }
        }
        if let Some((main_name, score, has_conflict)) = best {
            let suggested_strategy = if has_conflict {
                SubgraphMergeStrategy::BridgeLink
            } else if sub_nt.name == main_name
                && main_doc
                    .node_types
                    .iter()
                    .find(|m| m.name == main_name)
                    .is_some_and(|m| !has_new_props_or_rules(sub_nt, m))
            {
                SubgraphMergeStrategy::ReuseMain
            } else {
                SubgraphMergeStrategy::ExtendProps
            };
            out.push(TypeAlignmentCandidate {
                sub_type: sub_nt.name.clone(),
                main_type: main_name,
                score,
                suggested_strategy,
            });
        }
    }
    out
}

fn has_new_props_or_rules(sub_nt: &NodeTypeSpec, main_nt: &NodeTypeSpec) -> bool {
    sub_nt
        .properties
        .iter()
        .any(|sp| !main_nt.properties.iter().any(|mp| mp.name == sp.name))
        || sub_nt.agg_rules.iter().any(|sr| {
            !main_nt
                .agg_rules
                .iter()
                .any(|mr| mr.target_metric == sr.target_metric)
        })
        || sub_nt.derived_rules.iter().any(|sr| {
            !main_nt
                .derived_rules
                .iter()
                .any(|mr| mr.target_metric == sr.target_metric)
        })
}

fn alignment_score(
    sub_nt: &NodeTypeSpec,
    main_nt: &NodeTypeSpec,
    desc_embeddings: Option<&BTreeMap<String, Vec<f32>>>,
) -> f64 {
    if sub_nt.name == main_nt.name {
        return 1.0;
    }

    let name_sim = f64::max(
        ngram_dice_similarity(
            &normalize_token(&sub_nt.name),
            &normalize_token(&main_nt.name),
        ),
        ngram_dice_similarity(
            &normalize_token(&sub_nt.display_name),
            &normalize_token(&main_nt.display_name),
        ),
    );
    let prop_sim = property_jaccard(sub_nt, main_nt);
    let cos_sim = description_cosine(sub_nt, main_nt, desc_embeddings);

    (0.4 * name_sim + 0.4 * prop_sim + 0.2 * cos_sim).clamp(0.0, 1.0)
}

fn normalize_token(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

fn ngram_dice_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let grams_a = extract_char_ngrams(a);
    let grams_b = extract_char_ngrams(b);
    let total_a: u32 = grams_a.values().copied().sum();
    let total_b: u32 = grams_b.values().copied().sum();
    let denom = total_a.saturating_add(total_b);
    if denom == 0 {
        return 0.0;
    }
    let mut inter: u32 = 0;
    for (g, &ca) in &grams_a {
        if let Some(&cb) = grams_b.get(g) {
            inter = inter.saturating_add(ca.min(cb));
        }
    }
    (2.0 * f64::from(inter) / f64::from(denom)).clamp(0.0, 1.0)
}

fn extract_char_ngrams(s: &str) -> BTreeMap<String, u32> {
    let chars: Vec<char> = s.chars().collect();
    let mut multiset: BTreeMap<String, u32> = BTreeMap::new();
    if chars.len() <= 2 {
        for &c in &chars {
            let entry = multiset.entry(c.to_string()).or_insert(0);
            *entry = entry.saturating_add(1);
        }
    }
    for pair in chars.windows(2) {
        let gram: String = pair.iter().collect();
        let entry = multiset.entry(gram).or_insert(0);
        *entry = entry.saturating_add(1);
    }
    multiset
}

fn property_jaccard(sub_nt: &NodeTypeSpec, main_nt: &NodeTypeSpec) -> f64 {
    let sub_set: BTreeSet<&str> = sub_nt.properties.iter().map(|p| p.name.as_str()).collect();
    let main_set: BTreeSet<&str> = main_nt.properties.iter().map(|p| p.name.as_str()).collect();
    let union_cnt = u32::try_from(sub_set.union(&main_set).count()).unwrap_or(u32::MAX);
    if union_cnt == 0 {
        return 0.0;
    }
    let inter_cnt = u32::try_from(sub_set.intersection(&main_set).count()).unwrap_or(u32::MAX);
    f64::from(inter_cnt) / f64::from(union_cnt)
}

fn description_cosine(
    sub_nt: &NodeTypeSpec,
    main_nt: &NodeTypeSpec,
    desc_embeddings: Option<&BTreeMap<String, Vec<f32>>>,
) -> f64 {
    if let Some(map) = desc_embeddings {
        let sub_vec = map
            .get(&format!("sub:{}", sub_nt.name))
            .or_else(|| map.get(&sub_nt.name));
        let main_vec = map
            .get(&format!("main:{}", main_nt.name))
            .or_else(|| map.get(&main_nt.name));
        if let (Some(va), Some(vb)) = (sub_vec, main_vec)
            && !va.is_empty()
            && va.len() == vb.len()
        {
            return vector_cosine(va, vb);
        }
    }

    let text_a = format!(
        "{} {} {}",
        sub_nt.name, sub_nt.display_name, sub_nt.description
    );
    let text_b = format!(
        "{} {} {}",
        main_nt.name, main_nt.display_name, main_nt.description
    );
    let tf_a = extract_char_ngrams(&normalize_token(&text_a));
    let tf_b = extract_char_ngrams(&normalize_token(&text_b));
    let norm_a: f64 = tf_a
        .values()
        .map(|&v| f64::from(v) * f64::from(v))
        .sum::<f64>()
        .sqrt();
    let norm_b: f64 = tf_b
        .values()
        .map(|&v| f64::from(v) * f64::from(v))
        .sum::<f64>()
        .sqrt();
    if norm_a <= f64::EPSILON || norm_b <= f64::EPSILON {
        return 0.0;
    }
    let mut dot = 0.0;
    for (k, &va) in &tf_a {
        if let Some(&vb) = tf_b.get(k) {
            dot += f64::from(va) * f64::from(vb);
        }
    }
    (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
}

fn vector_cosine(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (&xa, &xb) in a.iter().zip(b.iter()) {
        let fa = f64::from(xa);
        let fb = f64::from(xb);
        dot += fa * fb;
        na += fa * fa;
        nb += fb * fb;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom <= f64::EPSILON {
        0.0
    } else {
        (dot / denom).clamp(0.0, 1.0)
    }
}

fn has_direct_node_conflict(sub_nt: &NodeTypeSpec, main_nt: &NodeTypeSpec) -> bool {
    let empty_rename = BTreeMap::new();
    let mut tmp = Vec::new();
    collect_node_pair_conflicts(sub_nt, main_nt, &empty_rename, &mut tmp);
    !tmp.is_empty()
}

fn detect_all_conflicts(
    sub_doc: &SchemaDoc,
    main_doc: &SchemaDoc,
    explicit_map: &BTreeMap<&str, &TypeMergeResolution>,
    auto_map: &BTreeMap<&str, &TypeAlignmentCandidate>,
    rename_map: &BTreeMap<String, String>,
) -> Vec<MergeConflict> {
    let mut conflicts = Vec::new();

    for sub_nt in &sub_doc.node_types {
        let target_main_opt: Option<&str> =
            if let Some(&res) = explicit_map.get(sub_nt.name.as_str()) {
                if res.strategy == SubgraphMergeStrategy::BridgeLink {
                    None
                } else {
                    Some(res.main_type.as_str())
                }
            } else {
                auto_map
                    .get(sub_nt.name.as_str())
                    .map(|c| c.main_type.as_str())
            };

        if let Some(target_main) = target_main_opt
            && let Some(main_nt) = main_doc.node_types.iter().find(|m| m.name == target_main)
        {
            collect_node_pair_conflicts(sub_nt, main_nt, rename_map, &mut conflicts);
        }
    }

    for sub_e in &sub_doc.edge_types {
        let mapped_src = rename_map
            .get(&sub_e.src_type_name)
            .cloned()
            .unwrap_or_else(|| sub_e.src_type_name.clone());
        let mapped_dst = rename_map
            .get(&sub_e.dst_type_name)
            .cloned()
            .unwrap_or_else(|| sub_e.dst_type_name.clone());
        if let Some(main_e) = main_doc.edge_types.iter().find(|me| me.name == sub_e.name)
            && (main_e.src_type_name != mapped_src
                || main_e.dst_type_name != mapped_dst
                || main_e.cardinality != sub_e.cardinality)
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::EdgeConflict,
                sub_item: format!(
                    "{}({}->{} {})",
                    sub_e.name, sub_e.src_type_name, sub_e.dst_type_name, sub_e.cardinality
                ),
                main_item: format!(
                    "{}({}->{} {})",
                    main_e.name, main_e.src_type_name, main_e.dst_type_name, main_e.cardinality
                ),
                detail: format!(
                    "edge '{}' has conflicting endpoints or cardinality against main",
                    sub_e.name
                ),
            });
        }
    }

    for sub_m in &sub_doc.mappings {
        let mapped_nt = rename_map
            .get(&sub_m.node_type)
            .cloned()
            .unwrap_or_else(|| sub_m.node_type.clone());
        if let Some(main_m) = main_doc.mappings.iter().find(|mm| mm.name == sub_m.name)
            && (main_m.dataset != sub_m.dataset || main_m.node_type != mapped_nt)
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::MappingNameConflict,
                sub_item: sub_m.name.clone(),
                main_item: main_m.name.clone(),
                detail: format!(
                    "mapping '{}' targets ({}, {}) in subgraph vs ({}, {}) in main",
                    sub_m.name, sub_m.dataset, mapped_nt, main_m.dataset, main_m.node_type
                ),
            });
        }
    }

    conflicts
}

fn collect_node_pair_conflicts(
    sub_nt: &NodeTypeSpec,
    main_nt: &NodeTypeSpec,
    rename_map: &BTreeMap<String, String>,
    conflicts: &mut Vec<MergeConflict>,
) {
    for sp in &sub_nt.properties {
        if let Some(mp) = main_nt.properties.iter().find(|p| p.name == sp.name)
            && (sp.data_type != mp.data_type || sp.kind != mp.kind)
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::PropertyTypeConflict,
                sub_item: format!("{}.{}", sub_nt.name, sp.name),
                main_item: format!("{}.{}", main_nt.name, mp.name),
                detail: format!(
                    "property '{}' is ({:?}, {}) in subgraph vs ({:?}, {}) in main",
                    sp.name, sp.kind, sp.data_type, mp.kind, mp.data_type
                ),
            });
        }
    }

    for sr in &sub_nt.agg_rules {
        let mut normalized_sr = sr.clone();
        if let Some(mapped_src) = rename_map.get(&normalized_sr.source_type_name) {
            normalized_sr.source_type_name.clone_from(mapped_src);
        }
        if let Some(mr) = main_nt
            .agg_rules
            .iter()
            .find(|r| r.target_metric == sr.target_metric)
            && *mr != normalized_sr
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::RuleConflict,
                sub_item: format!("{}.{}", sub_nt.name, sr.target_metric),
                main_item: format!("{}.{}", main_nt.name, mr.target_metric),
                detail: format!(
                    "metric '{}' has conflicting aggregation rule definitions",
                    sr.target_metric
                ),
            });
        }
        if main_nt
            .derived_rules
            .iter()
            .any(|dr| dr.enabled && dr.target_metric == sr.target_metric)
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::RuleConflict,
                sub_item: format!("{}.{}", sub_nt.name, sr.target_metric),
                main_item: format!("{}.{}", main_nt.name, sr.target_metric),
                detail: format!(
                    "metric '{}' is an agg_rule in subgraph but a derived_rule in main",
                    sr.target_metric
                ),
            });
        }
    }

    for sdr in &sub_nt.derived_rules {
        if !sdr.enabled {
            continue;
        }
        if let Some(mdr) = main_nt
            .derived_rules
            .iter()
            .find(|r| r.enabled && r.target_metric == sdr.target_metric)
            && mdr.expression.trim() != sdr.expression.trim()
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::RuleConflict,
                sub_item: format!("{}.{}", sub_nt.name, sdr.target_metric),
                main_item: format!("{}.{}", main_nt.name, mdr.target_metric),
                detail: format!(
                    "metric '{}' has conflicting derived_rule expressions",
                    sdr.target_metric
                ),
            });
        }
        if main_nt
            .agg_rules
            .iter()
            .any(|mr| mr.target_metric == sdr.target_metric)
        {
            conflicts.push(MergeConflict {
                kind: MergeConflictKind::RuleConflict,
                sub_item: format!("{}.{}", sub_nt.name, sdr.target_metric),
                main_item: format!("{}.{}", main_nt.name, sdr.target_metric),
                detail: format!(
                    "metric '{}' is a derived_rule in subgraph but an agg_rule in main",
                    sdr.target_metric
                ),
            });
        }
    }
}
