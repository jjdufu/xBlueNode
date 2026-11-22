//! Strict Draft 2020-12 JSON Schema specifications with dynamic tenant ontology enum injection (`01-STEP15` §15.2.2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::AppError;
use crate::analytics::MetricDef;
use crate::studio::{EdgeTypeSpec, NodeTypeSpec};

/// Dynamic tenant ontology & semantic catalog identifiers injected as `enum` constraints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantOntologyEnums {
    /// Published `NodeTypeSpec.name` values (e.g., `["Factory", "Order", "Warehouse"]`).
    pub node_types: Vec<String>,
    /// Published `EdgeTypeSpec.name` values (e.g., `["FULFILLS", "SUPPLIES"]`).
    pub edge_types: Vec<String>,
    /// Active non-broken `MetricDef.name` identifiers (e.g., `["paid_amount", "on_time_rate"]`).
    pub metric_names: Vec<String>,
    /// Allowed dimension identifiers per `MetricDef.name`.
    pub metric_dimensions: BTreeMap<String, Vec<String>>,
    /// Authorized dataset SQL names in the active `TenantCatalog`.
    pub dataset_names: Vec<String>,
}

/// Self-contained strict JSON Schema contract bound to a structured LLM request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSchemaSpec {
    /// Machine identifier (`^[a-zA-Z0-9_-]{1,64}$`) sent as `json_schema.name`.
    pub name: String,
    /// Concise schema description sent as `json_schema.description`.
    pub description: String,
    /// Root JSON Schema object (`type == "object"`, `additionalProperties == false`, full `required`).
    pub schema: Value,
    /// Optional shallow fallback schema (nesting depth `<= 2`) used when `is_local_model` is `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_compact_schema: Option<Value>,
}

impl JsonSchemaSpec {
    /// Constructs a validated strict [`JsonSchemaSpec`], normalizing every nested object that
    /// declares fixed `"properties"` to carry `"additionalProperties": false` and deterministic
    /// `"required"` key arrays.
    #[must_use]
    pub fn new_strict(
        name: impl Into<String>,
        description: impl Into<String>,
        raw_schema: Value,
    ) -> Self {
        let mut normalized = raw_schema;
        enforce_strict_object_invariants(&mut normalized);
        Self {
            name: name.into(),
            description: description.into(),
            schema: normalized,
            local_compact_schema: None,
        }
    }

    /// Constructs a [`JsonSchemaSpec`] preserving caller-specified `"required"` subsets and
    /// dynamic map `"additionalProperties"` rules (used for extensible draft documents).
    #[must_use]
    pub fn new_flexible(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
            local_compact_schema: None,
        }
    }

    /// Attaches a flattened grammar-friendly schema variant for `is_local_model = true` (7B–32B models).
    #[must_use]
    pub fn with_local_compact(mut self, raw_compact: Value) -> Self {
        let mut normalized = raw_compact;
        enforce_strict_object_invariants(&mut normalized);
        self.local_compact_schema = Some(normalized);
        self
    }

    /// Returns the effective JSON Schema for a provider (`local_compact_schema` when `is_local_model` is `true`).
    #[must_use]
    pub fn effective_schema(&self, is_local_model: bool) -> &Value {
        if is_local_model && let Some(ref compact) = self.local_compact_schema {
            return compact;
        }
        &self.schema
    }

    /// Builds the strict `{ status: "OK" }` connectivity probe schema (`POST /api/axagent/providers/{id}/test`).
    #[must_use]
    pub fn connectivity_probe() -> Self {
        Self::new_strict(
            "axpoint_probe",
            "Connectivity and constrained decoding probe.",
            json!({
                "type": "object",
                "properties": {
                    "status": { "type": "string", "enum": ["OK"] }
                },
                "required": ["status"],
                "additionalProperties": false
            }),
        )
    }

    /// Builds the strict `{ sql, explanation }` schema for Stage 3 NL2SQL generation.
    #[must_use]
    pub fn nl2sql_draft() -> Self {
        Self::new_strict(
            "axpoint_nl2sql_draft",
            "Guarded DuckDB SELECT draft or clarification explanation when SQL is empty.",
            json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string" },
                    "explanation": { "type": "string" }
                },
                "required": ["sql", "explanation"],
                "additionalProperties": false
            }),
        )
    }

    /// Builds the Stage 0 semantic metric & domain-object slot extraction schema with dynamic enums (`Step 16`).
    #[must_use]
    pub fn ontology_slot_extraction(enums: &TenantOntologyEnums) -> Self {
        let full = build_full_ontology_slot_schema(enums);
        let compact = build_compact_ontology_slot_schema(enums);
        Self::new_strict(
            "axpoint_ontology_slots",
            "Deterministic ontology & certified metric slot extraction payload.",
            full,
        )
        .with_local_compact(compact)
    }

    /// Builds a single-metric compact slot schema pinned to `metric` and its declared `dimensions`.
    #[must_use]
    pub fn single_metric_slot(metric: &MetricDef) -> Self {
        let dim_names: Vec<String> = metric.dimensions.iter().map(|d| d.name.clone()).collect();
        let mut enums = TenantOntologyEnums::default();
        enums.metric_names.push(metric.name.clone());
        if !dim_names.is_empty() {
            enums
                .metric_dimensions
                .insert(metric.name.clone(), dim_names);
        }
        Self::ontology_slot_extraction(&enums)
    }

    /// Builds a domain-object query slot schema scoped to `node_type` and its incident `edge_types`.
    #[must_use]
    pub fn domain_object_slot(node_type: &NodeTypeSpec, edge_types: &[EdgeTypeSpec]) -> Self {
        let mut enums = TenantOntologyEnums::default();
        enums.node_types.push(node_type.name.clone());
        for edge in edge_types {
            if edge.src_type_name == node_type.name || edge.dst_type_name == node_type.name {
                enums.edge_types.push(edge.name.clone());
            }
        }
        Self::ontology_slot_extraction(&enums)
    }

    /// Builds a `SchemaDoc` JSON Schema used by `xBlueStudio` AI draft generation (`schema_draft.rs`).
    #[must_use]
    pub fn schema_doc_draft() -> Self {
        Self::new_flexible(
            "axpoint_schema_doc_draft",
            "xBlueStudio ontology draft containing node_types, edge_types, and dataset mappings.",
            json!({
                "type": "object",
                "properties": {
                    "node_types": { "type": "array", "items": { "type": "object" } },
                    "edge_types": { "type": "array", "items": { "type": "object" } },
                    "mappings": { "type": "array", "items": { "type": "object" } }
                },
                "required": ["node_types", "edge_types"]
            }),
        )
    }
}

fn build_full_ontology_slot_schema(enums: &TenantOntologyEnums) -> Value {
    let metric_item = string_or_enum(&enums.metric_names);
    let node_type_prop = nullable_string_or_enum(&enums.node_types);
    let dim_union = collect_all_dimensions(enums);
    let dim_item = string_or_enum(&dim_union);
    json!({
        "type": "object",
        "properties": {
            "route": {
                "type": "string",
                "enum": ["SINGLE_METRIC", "MULTI_METRIC", "DOMAIN_OBJECT", "CLARIFY", "FALLBACK_SQL"]
            },
            "metric_names": { "type": "array", "items": metric_item },
            "target_node_type": node_type_prop,
            "dimensions": { "type": "array", "items": dim_item },
            "filters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "field": { "type": "string" },
                        "op": {
                            "type": "string",
                            "enum": ["EQ", "NE", "GT", "GTE", "LT", "LTE", "IN", "NOT_IN", "LIKE", "IS_NULL", "IS_NOT_NULL"]
                        },
                        "value_text": { "type": "string" }
                    },
                    "required": ["field", "op", "value_text"],
                    "additionalProperties": false
                }
            },
            "time_from": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "time_to": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "grain": {
                "anyOf": [
                    { "type": "string", "enum": ["MINUTE", "HOUR", "DAY", "MONTH"] },
                    { "type": "null" }
                ]
            },
            "compare": {
                "anyOf": [
                    { "type": "string", "enum": ["PREVIOUS_PERIOD", "SAME_PERIOD_LAST_YEAR"] },
                    { "type": "null" }
                ]
            },
            "include_links": { "type": "array", "items": string_or_enum(&enums.edge_types) },
            "clarification": { "type": "string" }
        },
        "required": [
            "route", "metric_names", "target_node_type", "dimensions",
            "filters", "time_from", "time_to", "grain", "compare",
            "include_links", "clarification"
        ],
        "additionalProperties": false
    })
}

fn build_compact_ontology_slot_schema(enums: &TenantOntologyEnums) -> Value {
    let dim_union = collect_all_dimensions(enums);
    let target_node_schema = if enums.node_types.is_empty() {
        json!({ "type": "string" })
    } else {
        let mut allowed = vec![String::new()];
        for nt in &enums.node_types {
            if !allowed.contains(nt) {
                allowed.push(nt.clone());
            }
        }
        json!({ "type": "string", "enum": allowed })
    };
    json!({
        "type": "object",
        "properties": {
            "route": {
                "type": "string",
                "enum": ["SINGLE_METRIC", "MULTI_METRIC", "DOMAIN_OBJECT", "CLARIFY", "FALLBACK_SQL"]
            },
            "metric_names": { "type": "array", "items": string_or_enum(&enums.metric_names) },
            "target_node_type": target_node_schema,
            "dimensions": { "type": "array", "items": string_or_enum(&dim_union) },
            "filter_exprs": { "type": "array", "items": { "type": "string" } },
            "time_from": { "type": "string" },
            "time_to": { "type": "string" },
            "grain": { "type": "string", "enum": ["", "MINUTE", "HOUR", "DAY", "MONTH"] },
            "compare": { "type": "string", "enum": ["", "PREVIOUS_PERIOD", "SAME_PERIOD_LAST_YEAR"] },
            "include_links": { "type": "array", "items": string_or_enum(&enums.edge_types) },
            "clarification": { "type": "string" }
        },
        "required": [
            "route", "metric_names", "target_node_type", "dimensions",
            "filter_exprs", "time_from", "time_to", "grain", "compare",
            "include_links", "clarification"
        ],
        "additionalProperties": false
    })
}

fn collect_all_dimensions(enums: &TenantOntologyEnums) -> Vec<String> {
    let mut out = Vec::new();
    for dims in enums.metric_dimensions.values() {
        for dim in dims {
            if !out.contains(dim) {
                out.push(dim.clone());
            }
        }
    }
    out
}

fn string_or_enum(values: &[String]) -> Value {
    if values.is_empty() {
        json!({ "type": "string" })
    } else {
        let mut dedup: Vec<String> = Vec::with_capacity(values.len());
        for v in values {
            if !dedup.contains(v) {
                dedup.push(v.clone());
            }
        }
        json!({ "type": "string", "enum": dedup })
    }
}

fn nullable_string_or_enum(values: &[String]) -> Value {
    json!({
        "anyOf": [
            string_or_enum(values),
            { "type": "null" }
        ]
    })
}

fn enforce_strict_object_invariants(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        if let Some(arr) = node.as_array_mut() {
            for elem in arr {
                enforce_strict_object_invariants(elem);
            }
        }
        return;
    };

    let is_object_type = obj.get("type").and_then(Value::as_str) == Some("object");
    let has_dynamic_map = obj
        .get("additionalProperties")
        .is_some_and(Value::is_object);

    if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
        let mut keys: Vec<String> = props.keys().cloned().collect();
        keys.sort();
        for prop_schema in props.values_mut() {
            enforce_strict_object_invariants(prop_schema);
        }
        if is_object_type && !has_dynamic_map {
            obj.insert(String::from("additionalProperties"), Value::Bool(false));
            let req_vals: Vec<Value> = keys.into_iter().map(Value::String).collect();
            obj.insert(String::from("required"), Value::Array(req_vals));
        }
    }

    for nested_key in ["items", "additionalProperties"] {
        if let Some(child) = obj.get_mut(nested_key) {
            enforce_strict_object_invariants(child);
        }
    }
    for array_key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(arr) = obj.get_mut(array_key).and_then(Value::as_array_mut) {
            for elem in arr {
                enforce_strict_object_invariants(elem);
            }
        }
    }
    for defs_key in ["$defs", "definitions"] {
        if let Some(defs) = obj.get_mut(defs_key).and_then(Value::as_object_mut) {
            for def_schema in defs.values_mut() {
                enforce_strict_object_invariants(def_schema);
            }
        }
    }
}

/// Strips optional Markdown code fences (```` ```json ... ``` ````) around a JSON payload.
#[must_use]
pub fn strip_markdown_json_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(after_open) = trimmed.strip_prefix("```") {
        let body_start = after_open
            .find('\n')
            .map_or(after_open, |nl| after_open.get(nl + 1..).unwrap_or(""));
        if let Some(stripped) = body_start.trim_end().strip_suffix("```") {
            return stripped.trim();
        }
    }
    trimmed
}

/// Parses `raw` (tolerating surrounding Markdown code fences) and validates it against Draft 2020-12 `schema`.
///
/// # Errors
///
/// Returns [`AppError::Unprocessable`] (`STRUCTURED_OUTPUT_INVALID`) when `raw` is not valid JSON
/// or violates any type, `required`, `additionalProperties`, `enum`, `items`, or `anyOf` constraint in `schema`.
pub fn parse_and_validate_json(raw: &str, schema: &Value) -> Result<Value, AppError> {
    let cleaned = strip_markdown_json_fence(raw);
    let parsed: Value = serde_json::from_str(cleaned).map_err(|err| {
        AppError::unprocessable(
            "STRUCTURED_OUTPUT_INVALID",
            format!("invalid JSON output: {err}"),
        )
    })?;
    let errors = validate_json_against_schema(&parsed, schema);
    if let Some(first) = errors.first() {
        let joined = errors
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let _ = first;
        return Err(AppError::unprocessable("STRUCTURED_OUTPUT_INVALID", joined));
    }
    Ok(parsed)
}

/// Validates `instance` against a Draft 2020-12 strict-subset JSON Schema, returning human-readable path errors.
#[must_use]
pub fn validate_json_against_schema(instance: &Value, schema: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    validate_node(instance, schema, schema, "", &mut errors);
    errors
}

fn validate_node(
    instance: &Value,
    schema: &Value,
    root_schema: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    let Some(schema_obj) = schema.as_object() else {
        if schema == &Value::Bool(false) {
            errors.push(format!(
                "{}: value is disallowed by false schema",
                path_or_root(path)
            ));
        }
        return;
    };

    if let Some(ref_uri) = schema_obj.get("$ref").and_then(Value::as_str)
        && let Some(resolved) = resolve_local_ref(root_schema, ref_uri)
    {
        validate_node(instance, resolved, root_schema, path, errors);
        return;
    }

    if let Some(any_of) = schema_obj.get("anyOf").and_then(Value::as_array) {
        validate_any_of(instance, any_of, root_schema, path, errors);
        return;
    }

    if let Some(enum_vals) = schema_obj.get("enum").and_then(Value::as_array)
        && !enum_vals.iter().any(|allowed| allowed == instance)
    {
        errors.push(format!(
            "{}: {instance} is not one of {enum_vals:?}",
            path_or_root(path)
        ));
        return;
    }

    if let Some(expected_type) = schema_obj.get("type").and_then(Value::as_str)
        && !matches_json_type(instance, expected_type)
    {
        errors.push(format!(
            "{}: expected type '{expected_type}', got {}",
            path_or_root(path),
            json_type_label(instance)
        ));
        return;
    }

    if let Some(inst_obj) = instance.as_object() {
        validate_object_instance(inst_obj, schema_obj, root_schema, path, errors);
    } else if let Some(inst_arr) = instance.as_array()
        && let Some(items_schema) = schema_obj.get("items")
    {
        for (idx, elem) in inst_arr.iter().enumerate() {
            let elem_path = format!("{path}/{idx}");
            validate_node(elem, items_schema, root_schema, &elem_path, errors);
        }
    }
}

fn validate_any_of(
    instance: &Value,
    branches: &[Value],
    root_schema: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    let mut branch_msgs = Vec::new();
    for branch in branches {
        let mut sub_errs = Vec::new();
        validate_node(instance, branch, root_schema, path, &mut sub_errs);
        if sub_errs.is_empty() {
            return;
        }
        if let Some(first) = sub_errs.into_iter().next() {
            branch_msgs.push(first);
        }
    }
    errors.push(format!(
        "{}: value did not match anyOf ({})",
        path_or_root(path),
        branch_msgs.join(" | ")
    ));
}

fn validate_object_instance(
    inst_obj: &Map<String, Value>,
    schema_obj: &Map<String, Value>,
    root_schema: &Value,
    path: &str,
    errors: &mut Vec<String>,
) {
    if let Some(req_arr) = schema_obj.get("required").and_then(Value::as_array) {
        for req_key in req_arr.iter().filter_map(Value::as_str) {
            if !inst_obj.contains_key(req_key) {
                errors.push(format!(
                    "{}: missing required property '{req_key}'",
                    path_or_root(path)
                ));
            }
        }
    }

    let declared_props = schema_obj.get("properties").and_then(Value::as_object);
    if let Some(props) = declared_props {
        for (k, prop_schema) in props {
            if let Some(val) = inst_obj.get(k) {
                let child_path = format!("{path}/{k}");
                validate_node(val, prop_schema, root_schema, &child_path, errors);
            }
        }
    }

    match schema_obj.get("additionalProperties") {
        Some(Value::Bool(false)) => {
            for k in inst_obj.keys() {
                let known = declared_props.is_some_and(|p| p.contains_key(k));
                if !known {
                    errors.push(format!(
                        "{}: unexpected additional property '{k}'",
                        path_or_root(path)
                    ));
                }
            }
        }
        Some(extra_schema) if extra_schema.is_object() => {
            for (k, val) in inst_obj {
                let known = declared_props.is_some_and(|p| p.contains_key(k));
                if !known {
                    let child_path = format!("{path}/{k}");
                    validate_node(val, extra_schema, root_schema, &child_path, errors);
                }
            }
        }
        _ => {}
    }
}

fn resolve_local_ref<'a>(root_schema: &'a Value, ref_uri: &str) -> Option<&'a Value> {
    let pointer = ref_uri.strip_prefix('#')?;
    root_schema.pointer(pointer)
}

fn matches_json_type(val: &Value, expected: &str) -> bool {
    match expected {
        "object" => val.is_object(),
        "array" => val.is_array(),
        "string" => val.is_string(),
        "boolean" => val.is_boolean(),
        "null" => val.is_null(),
        "integer" => val.is_i64() || val.is_u64(),
        "number" => val.is_number(),
        _ => true,
    }
}

const fn json_type_label(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn path_or_root(path: &str) -> &str {
    if path.is_empty() { "/" } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_all_objects_strict(node: &Value) {
        if let Some(obj) = node.as_object() {
            if obj.get("type").and_then(Value::as_str) == Some("object")
                && let Some(props) = obj.get("properties").and_then(Value::as_object)
            {
                assert_eq!(obj.get("additionalProperties"), Some(&Value::Bool(false)));
                let req = obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map_or_default(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    });
                let mut expected: Vec<String> = props.keys().cloned().collect();
                expected.sort();
                assert_eq!(req, expected);
                for v in props.values() {
                    assert_all_objects_strict(v);
                }
            }
            for k in ["items", "additionalProperties"] {
                if let Some(child) = obj.get(k) {
                    assert_all_objects_strict(child);
                }
            }
            if let Some(arr) = obj.get("anyOf").and_then(Value::as_array) {
                for elem in arr {
                    assert_all_objects_strict(elem);
                }
            }
        }
    }

    #[test]
    fn strict_schema_enforces_additional_properties_and_required() {
        let raw = json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "alpha": { "type": "string" },
                        "beta": { "type": "integer" }
                    }
                },
                "list": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "item_key": { "type": "string" }
                        }
                    }
                }
            }
        });
        let spec = JsonSchemaSpec::new_strict("nested_test", "Strict normalization test", raw);
        assert_all_objects_strict(&spec.schema);

        let missing_err =
            parse_and_validate_json(r#"{"outer":{"alpha":"ok"},"list":[]}"#, &spec.schema);
        assert!(missing_err.is_err());

        let extra_err = parse_and_validate_json(
            r#"{"outer":{"alpha":"ok","beta":1,"rogue":true},"list":[]}"#,
            &spec.schema,
        );
        assert!(extra_err.is_err());

        let valid = parse_and_validate_json(
            "```json\n{\"outer\":{\"alpha\":\"ok\",\"beta\":1},\"list\":[{\"item_key\":\"k1\"}]}\n```",
            &spec.schema,
        );
        assert!(valid.is_ok());
    }

    #[test]
    fn ontology_slot_schema_injects_dynamic_enums() {
        let mut metric_dimensions = BTreeMap::new();
        metric_dimensions.insert(
            String::from("paid_amount"),
            vec![String::from("region"), String::from("channel")],
        );
        let enums = TenantOntologyEnums {
            node_types: vec![String::from("Warehouse"), String::from("Order")],
            edge_types: vec![String::from("FULFILLS")],
            metric_names: vec![String::from("paid_amount")],
            metric_dimensions,
            dataset_names: vec![String::from("demo_orders")],
        };

        let spec = JsonSchemaSpec::ontology_slot_extraction(&enums);
        assert_all_objects_strict(&spec.schema);
        let compact = spec.effective_schema(true);
        assert_all_objects_strict(compact);

        let full_metric_enum = spec
            .schema
            .pointer("/properties/metric_names/items/enum")
            .cloned();
        let compact_metric_enum = compact
            .pointer("/properties/metric_names/items/enum")
            .cloned();
        assert_eq!(full_metric_enum, Some(json!(["paid_amount"])));
        assert_eq!(compact_metric_enum, Some(json!(["paid_amount"])));

        let invalid_grain_payload = json!({
            "route": "SINGLE_METRIC",
            "metric_names": ["paid_amount"],
            "target_node_type": null,
            "dimensions": ["region"],
            "filters": [],
            "time_from": null,
            "time_to": null,
            "grain": "WEEKLY",
            "compare": null,
            "include_links": [],
            "clarification": ""
        });
        let errs = validate_json_against_schema(&invalid_grain_payload, &spec.schema);
        assert_ne!(errs.len(), 0);

        let valid_payload = json!({
            "route": "SINGLE_METRIC",
            "metric_names": ["paid_amount"],
            "target_node_type": "Order",
            "dimensions": ["region"],
            "filters": [],
            "time_from": "2026-01-01",
            "time_to": "2026-01-31",
            "grain": "DAY",
            "compare": "PREVIOUS_PERIOD",
            "include_links": ["FULFILLS"],
            "clarification": ""
        });
        assert_eq!(
            validate_json_against_schema(&valid_payload, &spec.schema),
            Vec::<String>::new()
        );
    }
}
