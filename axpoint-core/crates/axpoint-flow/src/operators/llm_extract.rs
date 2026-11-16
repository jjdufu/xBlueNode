//! `LLM_EXTRACT` operator extracting structured JSON or knowledge triples with retry input-caching (`07-STEP07-08` §07.4).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{AiPort, FlowRepository};

use super::{parse_optional_string_any, parse_required_string};

/// Studio palette display name for `LLM_EXTRACT`.
pub const DISPLAY_NAME: &str = "大模型结构化抽取";

/// Studio palette short description for `LLM_EXTRACT`.
pub const DESCRIPTION: &str =
    "调用大模型按 JSON Schema 抽取结构化字段或知识三元组，重试复用相同输入结果";

/// Returns the JSON Schema validating `LLM_EXTRACT` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["text"],
        "properties": {
            "text": { "type": "string", "minLength": 1 },
            "instruction": { "type": "string" },
            "system_prompt": { "type": "string" },
            "output_schema": { "type": "object" },
            "mode": { "type": "string", "enum": ["JSON", "TRIPLES"] }
        },
        "additionalProperties": false
    })
}

/// `LLM_EXTRACT` operator implementation generic over [`AiPort`] and [`FlowRepository`].
pub struct LlmExtractOperator<A, F> {
    ai: Arc<A>,
    flow_repo: Arc<F>,
}

impl<A, F> LlmExtractOperator<A, F>
where
    A: AiPort + 'static,
    F: FlowRepository + 'static,
{
    /// Constructs a new [`LlmExtractOperator`].
    #[must_use]
    pub const fn new(ai: Arc<A>, flow_repo: Arc<F>) -> Self {
        Self { ai, flow_repo }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        if ctx.attempt > 1
            && let Some(cached) = self
                .flow_repo
                .find_cached_step_output(&ctx.tenant_id, ctx.run_id, &ctx.node_id, &ctx.config)
                .await?
        {
            return Ok(OperatorOutcome::Completed(cached));
        }

        let text = parse_required_string(&ctx.config, "text")?;
        let mode = parse_optional_string_any(&ctx.config, &["mode"])?
            .unwrap_or_else(|| String::from("JSON"));

        if mode.eq_ignore_ascii_case("TRIPLES") && ctx.config.get("output_schema").is_none() {
            let triples = self.ai.extract_triples(&text).await?;
            let count = triples.len();
            return Ok(OperatorOutcome::Completed(serde_json::json!({
                "triples": triples,
                "count": count,
            })));
        }

        let schema_opt = ctx.config.get("output_schema");
        let sys_prompt = build_extraction_system_prompt(&ctx.config, schema_opt);
        let raw_reply = self.ai.chat(&sys_prompt, &text).await?;
        let parsed = parse_json_reply(&raw_reply)?;

        if let Some(schema_val) = schema_opt {
            validate_against_schema(&parsed, schema_val)?;
        }

        let output = match parsed {
            serde_json::Value::Object(mut obj) => {
                let data_copy = serde_json::Value::Object(obj.clone());
                obj.entry(String::from("data")).or_insert(data_copy);
                serde_json::Value::Object(obj)
            }
            other => serde_json::json!({ "data": other }),
        };

        Ok(OperatorOutcome::Completed(output))
    }
}

fn build_extraction_system_prompt(
    config: &serde_json::Value,
    schema_opt: Option<&serde_json::Value>,
) -> String {
    if let Some(Some(custom)) = config
        .get("system_prompt")
        .map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
    {
        return String::from(custom);
    }

    let instruction = config
        .get("instruction")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Extract structured JSON fields from the user text.");

    match schema_opt {
        Some(schema) => format!(
            "{instruction}\nReturn ONLY valid JSON conforming strictly to this JSON Schema (no markdown prose): {schema}"
        ),
        None => format!("{instruction}\nReturn ONLY a valid JSON object without markdown prose."),
    }
}

fn parse_json_reply(raw: &str) -> Result<serde_json::Value, AppError> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |s| s.trim_end_matches("```").trim());

    serde_json::from_str(stripped).map_err(|err| {
        AppError::unprocessable(
            "PARSE_FAILED",
            format!("LLM_EXTRACT reply is not valid JSON: {err}"),
        )
    })
}

fn validate_against_schema(
    instance: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), AppError> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|err| AppError::bad("SCHEMA_INVALID", format!("invalid output_schema: {err}")))?;
    let msgs: Vec<String> = validator
        .iter_errors(instance)
        .take(5)
        .map(|e| e.to_string())
        .collect();
    if !msgs.is_empty() {
        return Err(AppError::bad(
            "SCHEMA_INVALID",
            format!(
                "LLM_EXTRACT output failed output_schema validation: {}",
                msgs.join("; ")
            ),
        ));
    }
    Ok(())
}

impl<A, F> Operator for LlmExtractOperator<A, F>
where
    A: AiPort + 'static,
    F: FlowRepository + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::LlmExtract
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn config_schema(&self) -> serde_json::Value {
        config_schema()
    }

    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move { self.run(ctx).await })
    }
}
