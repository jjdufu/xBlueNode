//! JSON Schema (`inputSchema`) descriptors for the 13 Step 29 `AxBuilder` MCP tools (`07-STEP29` §29.4).

/// Returns the JSON Schema (`inputSchema`) for one of the 13 Step 29 `axbuilder_*` tools (`07-STEP29` §29.4).
#[must_use]
pub fn step29_axbuilder_tool_schema(name: &str) -> Option<serde_json::Value> {
    step29_data_and_model_schema(name).or_else(|| step29_sandbox_and_qa_schema(name))
}

fn step29_data_and_model_schema(name: &str) -> Option<serde_json::Value> {
    match name {
        "axbuilder_list_datasets" => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "q": { "type": "string" },
                "tag": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            },
            "additionalProperties": false
        })),
        "axbuilder_inspect_dataset_profile" => Some(serde_json::json!({
            "type": "object",
            "required": ["dataset"],
            "properties": {
                "dataset": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        })),
        "axbuilder_sample_dataset_rows" => Some(serde_json::json!({
            "type": "object",
            "required": ["dataset"],
            "properties": {
                "dataset": { "type": "string", "minLength": 1 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 20 }
            },
            "additionalProperties": false
        })),
        "axbuilder_diff_schema_draft" => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" }
            },
            "additionalProperties": false
        })),
        "axbuilder_eval_expression" => Some(serde_json::json!({
            "type": "object",
            "required": ["expression"],
            "properties": {
                "expression": { "type": "string", "minLength": 1 },
                "metrics": { "type": "object" },
                "properties": { "type": "object" },
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "node_id": { "type": "string", "format": "uuid" }
            },
            "additionalProperties": false
        })),
        "axbuilder_dryrun_action_spec" => Some(serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug", "type_name", "node_key", "action_name"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "type_name": { "type": "string", "minLength": 1 },
                "node_key": { "type": "string", "minLength": 1 },
                "action_name": { "type": "string", "minLength": 1 },
                "inputs": { "type": "object" }
            },
            "additionalProperties": false
        })),
        _ => None,
    }
}

fn step29_sandbox_and_qa_schema(name: &str) -> Option<serde_json::Value> {
    match name {
        "axbuilder_certify_subgraph" => Some(serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "sample_limit": { "type": "integer", "minimum": 1, "maximum": 500 }
            },
            "additionalProperties": false
        })),
        "axbuilder_compile_axql" => Some(serde_json::json!({
            "type": "object",
            "required": ["axql"],
            "properties": {
                "axql": { "type": "string", "minLength": 1 },
                "subgraph_slug": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        })),
        "axbuilder_sim_trial_run" => Some(serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug", "actions"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "actions": { "type": "array", "items": { "type": "object" } },
                "params": { "type": "object" }
            },
            "additionalProperties": false
        })),
        "axbuilder_upsert_chart_draft" => Some(serde_json::json!({
            "type": "object",
            "required": ["name", "widget_kind", "data_source"],
            "properties": {
                "chart_id": { "type": "string", "format": "uuid" },
                "name": { "type": "string", "minLength": 1 },
                "description": { "type": "string" },
                "widget_kind": { "type": "string", "minLength": 1 },
                "data_source": { "type": "object" },
                "encoding": { "type": "object" },
                "options": { "type": "object" },
                "sensitivity": { "type": "string", "enum": ["PUBLIC", "INTERNAL", "CONFIDENTIAL", "RESTRICTED"] }
            },
            "additionalProperties": false
        })),
        "axbuilder_preflight_dashboard" => Some(serde_json::json!({
            "type": "object",
            "required": ["dashboard_id"],
            "properties": {
                "dashboard_id": { "type": "string", "format": "uuid" }
            },
            "additionalProperties": false
        })),
        "axbuilder_upsert_competency_suite" => Some(serde_json::json!({
            "type": "object",
            "required": ["questions"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "questions": { "type": "array", "items": { "type": "object" } }
            },
            "additionalProperties": false
        })),
        "axbuilder_verify_competency_suite" => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "run_dev_materialize": { "type": "boolean" },
                "limit_rows": { "type": "integer", "minimum": 1, "maximum": 5000 },
                "questions_override": { "type": "array", "items": { "type": "object" } }
            },
            "additionalProperties": false
        })),
        _ => None,
    }
}
