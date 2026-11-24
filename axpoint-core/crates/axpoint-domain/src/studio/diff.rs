//! Structural `SchemaDoc` diff engine (`06-STEP05-06` §05.4.2).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::studio::doc::{
    AggRuleSpec, ChangedItemDiff, DerivedRuleSpec, EdgeTypeSpec, FieldDiff, MappingSpec,
    NodeTypeSpec, PropertySpec, SchemaDiff, SchemaDoc, SectionDiff,
};

const FLOAT_EPSILON: f64 = 1e-9;

/// Computes a deterministic structural diff between `before` and `after` [`SchemaDoc`] snapshots
/// (`06-STEP05-06` §05.4.2).
///
/// Items are aligned by `name` (or `name:src->dst` when multiple edges share the same `name`),
/// and nested arrays (`properties`, `agg_rules`, `derived_rules`) are aligned by `name` / `target_metric`.
#[must_use]
pub fn diff_schema(before: &SchemaDoc, after: &SchemaDoc) -> SchemaDiff {
    SchemaDiff {
        node_types: diff_node_types(&before.node_types, &after.node_types),
        edge_types: diff_edge_types(&before.edge_types, &after.edge_types),
        mappings: diff_mappings(&before.mappings, &after.mappings),
    }
}

fn to_json_val<T: Serialize>(val: &T) -> Value {
    serde_json::to_value(val).unwrap_or(Value::Null)
}

fn push_if_eq_ne<T: PartialEq + Serialize>(
    fields: &mut Vec<FieldDiff>,
    path: &str,
    before: &T,
    after: &T,
) {
    if before != after {
        fields.push(FieldDiff {
            path: String::from(path),
            before: to_json_val(before),
            after: to_json_val(after),
        });
    }
}

fn push_if_f64_ne(fields: &mut Vec<FieldDiff>, path: &str, before: f64, after: f64) {
    if (before - after).abs() > FLOAT_EPSILON {
        fields.push(FieldDiff {
            path: String::from(path),
            before: to_json_val(&before),
            after: to_json_val(&after),
        });
    }
}

fn diff_node_types(before: &[NodeTypeSpec], after: &[NodeTypeSpec]) -> SectionDiff {
    let before_map: BTreeMap<&str, &NodeTypeSpec> =
        before.iter().map(|nt| (nt.name.as_str(), nt)).collect();
    let after_map: BTreeMap<&str, &NodeTypeSpec> =
        after.iter().map(|nt| (nt.name.as_str(), nt)).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for &name in after_map.keys() {
        if !before_map.contains_key(name) {
            added.push(String::from(name));
        }
    }
    for (&name, &old_nt) in &before_map {
        if let Some(&new_nt) = after_map.get(name) {
            let fields = diff_single_node_type(old_nt, new_nt);
            if !fields.is_empty() {
                changed.push(ChangedItemDiff {
                    name: String::from(name),
                    fields,
                });
            }
        } else {
            removed.push(String::from(name));
        }
    }

    SectionDiff {
        added,
        removed,
        changed,
    }
}

fn diff_single_node_type(old_nt: &NodeTypeSpec, new_nt: &NodeTypeSpec) -> Vec<FieldDiff> {
    let mut fields = Vec::new();
    push_if_eq_ne(
        &mut fields,
        "display_name",
        &old_nt.display_name,
        &new_nt.display_name,
    );
    push_if_eq_ne(
        &mut fields,
        "description",
        &old_nt.description,
        &new_nt.description,
    );
    push_if_eq_ne(&mut fields, "icon", &old_nt.icon, &new_nt.icon);
    push_if_eq_ne(&mut fields, "color", &old_nt.color, &new_nt.color);
    push_if_eq_ne(
        &mut fields,
        "title_template",
        &old_nt.title_template,
        &new_nt.title_template,
    );
    push_if_eq_ne(
        &mut fields,
        "sync_to_knowledge",
        &old_nt.sync_to_knowledge,
        &new_nt.sync_to_knowledge,
    );
    push_if_f64_ne(&mut fields, "pos_x", old_nt.pos_x, new_nt.pos_x);
    push_if_f64_ne(&mut fields, "pos_y", old_nt.pos_y, new_nt.pos_y);
    push_if_eq_ne(&mut fields, "maturity", &old_nt.maturity, &new_nt.maturity);
    push_if_eq_ne(
        &mut fields,
        "semantic_layer_tier",
        &old_nt.semantic_layer_tier,
        &new_nt.semantic_layer_tier,
    );
    push_if_eq_ne(
        &mut fields,
        "domain_group",
        &old_nt.domain_group,
        &new_nt.domain_group,
    );

    diff_properties(&old_nt.properties, &new_nt.properties, &mut fields);
    diff_agg_rules(&old_nt.agg_rules, &new_nt.agg_rules, &mut fields);
    diff_derived_rules(&old_nt.derived_rules, &new_nt.derived_rules, &mut fields);
    push_if_eq_ne(&mut fields, "actions", &old_nt.actions, &new_nt.actions);
    fields
}

fn diff_properties(
    old_props: &[PropertySpec],
    new_props: &[PropertySpec],
    fields: &mut Vec<FieldDiff>,
) {
    let old_map: BTreeMap<&str, &PropertySpec> =
        old_props.iter().map(|p| (p.name.as_str(), p)).collect();
    let new_map: BTreeMap<&str, &PropertySpec> =
        new_props.iter().map(|p| (p.name.as_str(), p)).collect();

    for (&name, &new_p) in &new_map {
        if !old_map.contains_key(name) {
            fields.push(FieldDiff {
                path: format!("properties[{name}]"),
                before: Value::Null,
                after: to_json_val(new_p),
            });
        }
    }
    for (&name, &old_p) in &old_map {
        if let Some(&new_p) = new_map.get(name) {
            if old_p != new_p {
                diff_single_property(name, old_p, new_p, fields);
            }
        } else {
            fields.push(FieldDiff {
                path: format!("properties[{name}]"),
                before: to_json_val(old_p),
                after: Value::Null,
            });
        }
    }
}

fn diff_single_property(
    name: &str,
    old_p: &PropertySpec,
    new_p: &PropertySpec,
    fields: &mut Vec<FieldDiff>,
) {
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].display_name"),
        &old_p.display_name,
        &new_p.display_name,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].description"),
        &old_p.description,
        &new_p.description,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].unit"),
        &old_p.unit,
        &new_p.unit,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].data_type"),
        &old_p.data_type,
        &new_p.data_type,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].kind"),
        &old_p.kind,
        &new_p.kind,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].required"),
        &old_p.required,
        &new_p.required,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].pii_category"),
        &old_p.pii_category,
        &new_p.pii_category,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].allowed_values"),
        &old_p.allowed_values,
        &new_p.allowed_values,
    );
    push_if_eq_ne(
        fields,
        &format!("properties[{name}].maturity"),
        &old_p.maturity,
        &new_p.maturity,
    );
}

fn agg_rule_key(rule: &AggRuleSpec, duplicate_targets: bool) -> String {
    if duplicate_targets {
        format!(
            "{}:{}:{}",
            rule.target_metric, rule.via_link_type, rule.source_metric
        )
    } else {
        rule.target_metric.clone()
    }
}

fn has_duplicate_agg_targets(rules: &[AggRuleSpec]) -> bool {
    let mut seen = BTreeMap::new();
    for r in rules {
        let c = seen.entry(r.target_metric.as_str()).or_insert(0usize);
        *c = c.saturating_add(1);
        if *c > 1 {
            return true;
        }
    }
    false
}

fn diff_agg_rules(
    old_rules: &[AggRuleSpec],
    new_rules: &[AggRuleSpec],
    fields: &mut Vec<FieldDiff>,
) {
    let dup = has_duplicate_agg_targets(old_rules) || has_duplicate_agg_targets(new_rules);
    let old_map: BTreeMap<String, &AggRuleSpec> = old_rules
        .iter()
        .map(|r| (agg_rule_key(r, dup), r))
        .collect();
    let new_map: BTreeMap<String, &AggRuleSpec> = new_rules
        .iter()
        .map(|r| (agg_rule_key(r, dup), r))
        .collect();

    for (key, &new_r) in &new_map {
        if !old_map.contains_key(key) {
            fields.push(FieldDiff {
                path: format!("agg_rules[{key}]"),
                before: Value::Null,
                after: to_json_val(new_r),
            });
        }
    }
    for (key, &old_r) in &old_map {
        if let Some(&new_r) = new_map.get(key) {
            if old_r != new_r {
                fields.push(FieldDiff {
                    path: format!("agg_rules[{key}]"),
                    before: to_json_val(old_r),
                    after: to_json_val(new_r),
                });
            }
        } else {
            fields.push(FieldDiff {
                path: format!("agg_rules[{key}]"),
                before: to_json_val(old_r),
                after: Value::Null,
            });
        }
    }
}

fn diff_derived_rules(
    old_rules: &[DerivedRuleSpec],
    new_rules: &[DerivedRuleSpec],
    fields: &mut Vec<FieldDiff>,
) {
    let old_map: BTreeMap<&str, &DerivedRuleSpec> = old_rules
        .iter()
        .map(|r| (r.target_metric.as_str(), r))
        .collect();
    let new_map: BTreeMap<&str, &DerivedRuleSpec> = new_rules
        .iter()
        .map(|r| (r.target_metric.as_str(), r))
        .collect();

    for (&key, &new_r) in &new_map {
        if !old_map.contains_key(key) {
            fields.push(FieldDiff {
                path: format!("derived_rules[{key}]"),
                before: Value::Null,
                after: to_json_val(new_r),
            });
        }
    }
    for (&key, &old_r) in &old_map {
        if let Some(&new_r) = new_map.get(key) {
            if old_r != new_r {
                fields.push(FieldDiff {
                    path: format!("derived_rules[{key}]"),
                    before: to_json_val(old_r),
                    after: to_json_val(new_r),
                });
            }
        } else {
            fields.push(FieldDiff {
                path: format!("derived_rules[{key}]"),
                before: to_json_val(old_r),
                after: Value::Null,
            });
        }
    }
}

fn diff_edge_types(before: &[EdgeTypeSpec], after: &[EdgeTypeSpec]) -> SectionDiff {
    let mut before_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in before {
        let c = before_counts.entry(e.name.as_str()).or_insert(0);
        *c = c.saturating_add(1);
    }
    let mut after_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in after {
        let c = after_counts.entry(e.name.as_str()).or_insert(0);
        *c = c.saturating_add(1);
    }

    let edge_key = |e: &EdgeTypeSpec| -> String {
        let bc = before_counts.get(e.name.as_str()).copied().unwrap_or(0);
        let ac = after_counts.get(e.name.as_str()).copied().unwrap_or(0);
        if bc <= 1 && ac <= 1 {
            e.name.clone()
        } else {
            format!("{}:{}->{}", e.name, e.src_type_name, e.dst_type_name)
        }
    };

    let before_map: BTreeMap<String, &EdgeTypeSpec> =
        before.iter().map(|e| (edge_key(e), e)).collect();
    let after_map: BTreeMap<String, &EdgeTypeSpec> =
        after.iter().map(|e| (edge_key(e), e)).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for key in after_map.keys() {
        if !before_map.contains_key(key) {
            added.push(key.clone());
        }
    }
    for (key, &old_e) in &before_map {
        if let Some(&new_e) = after_map.get(key) {
            let mut fields = Vec::new();
            push_if_eq_ne(
                &mut fields,
                "src_type_name",
                &old_e.src_type_name,
                &new_e.src_type_name,
            );
            push_if_eq_ne(
                &mut fields,
                "dst_type_name",
                &old_e.dst_type_name,
                &new_e.dst_type_name,
            );
            push_if_f64_ne(
                &mut fields,
                "impact_weight",
                old_e.impact_weight,
                new_e.impact_weight,
            );
            push_if_eq_ne(
                &mut fields,
                "cardinality",
                &old_e.cardinality,
                &new_e.cardinality,
            );
            push_if_eq_ne(
                &mut fields,
                "description",
                &old_e.description,
                &new_e.description,
            );
            diff_properties(&old_e.properties, &new_e.properties, &mut fields);
            push_if_eq_ne(
                &mut fields,
                "propagation",
                &old_e.propagation,
                &new_e.propagation,
            );
            push_if_eq_ne(&mut fields, "maturity", &old_e.maturity, &new_e.maturity);
            push_if_eq_ne(
                &mut fields,
                "derived_rules",
                &old_e.derived_rules,
                &new_e.derived_rules,
            );
            push_if_eq_ne(&mut fields, "agg_rules", &old_e.agg_rules, &new_e.agg_rules);
            if !fields.is_empty() {
                changed.push(ChangedItemDiff {
                    name: key.clone(),
                    fields,
                });
            }
        } else {
            removed.push(key.clone());
        }
    }

    SectionDiff {
        added,
        removed,
        changed,
    }
}

fn diff_mappings(before: &[MappingSpec], after: &[MappingSpec]) -> SectionDiff {
    let before_map: BTreeMap<&str, &MappingSpec> =
        before.iter().map(|m| (m.name.as_str(), m)).collect();
    let after_map: BTreeMap<&str, &MappingSpec> =
        after.iter().map(|m| (m.name.as_str(), m)).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for &name in after_map.keys() {
        if !before_map.contains_key(name) {
            added.push(String::from(name));
        }
    }
    for (&name, &old_m) in &before_map {
        if let Some(&new_m) = after_map.get(name) {
            let fields = diff_single_mapping(old_m, new_m);
            if !fields.is_empty() {
                changed.push(ChangedItemDiff {
                    name: String::from(name),
                    fields,
                });
            }
        } else {
            removed.push(String::from(name));
        }
    }

    SectionDiff {
        added,
        removed,
        changed,
    }
}

fn diff_single_mapping(old_m: &MappingSpec, new_m: &MappingSpec) -> Vec<FieldDiff> {
    let mut fields = Vec::new();
    push_if_eq_ne(&mut fields, "dataset", &old_m.dataset, &new_m.dataset);
    push_if_eq_ne(&mut fields, "node_type", &old_m.node_type, &new_m.node_type);
    push_if_eq_ne(
        &mut fields,
        "key_column",
        &old_m.key_column,
        &new_m.key_column,
    );
    push_if_eq_ne(&mut fields, "title", &old_m.title, &new_m.title);
    push_if_eq_ne(&mut fields, "filter", &old_m.filter, &new_m.filter);
    push_if_eq_ne(
        &mut fields,
        "attributes",
        &old_m.attributes,
        &new_m.attributes,
    );
    push_if_eq_ne(&mut fields, "metrics", &old_m.metrics, &new_m.metrics);
    push_if_eq_ne(
        &mut fields,
        "metric_time_column",
        &old_m.metric_time_column,
        &new_m.metric_time_column,
    );
    push_if_eq_ne(&mut fields, "geo", &old_m.geo, &new_m.geo);
    push_if_eq_ne(&mut fields, "links", &old_m.links, &new_m.links);
    push_if_eq_ne(
        &mut fields,
        "on_source_delete",
        &old_m.on_source_delete,
        &new_m.on_source_delete,
    );
    push_if_eq_ne(&mut fields, "mode", &old_m.mode, &new_m.mode);
    fields
}
