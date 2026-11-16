//! `AGG_TRIGGER` operator executing dynamic `xBlueStudio` metric aggregation & derived rules (`07-STEP07-08` §07.4).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::StudioExecutionPort;
use axpoint_domain::rollup::GraphAggregateJobPayload;
use uuid::Uuid;

use super::{
    parse_optional_bool, parse_optional_string_any, parse_optional_string_vec,
    parse_optional_uuid_vec, templated_bool_schema, templated_uuid_schema,
};

/// Studio palette display name for `AGG_TRIGGER`.
pub const DISPLAY_NAME: &str = "动态指标聚合";

/// Studio palette short description for `AGG_TRIGGER`.
pub const DESCRIPTION: &str = "按指标依赖拓扑重算指定节点、节点类型或全图的聚合与派生指标";

/// Returns the JSON Schema validating `AGG_TRIGGER` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "full": templated_bool_schema(),
            "full_type": { "type": "string", "minLength": 1 },
            "type_name": { "type": "string", "minLength": 1 },
            "node_ids": {
                "oneOf": [
                    { "type": "array", "items": templated_uuid_schema() },
                    { "type": "string", "pattern": "^\\s*\\{\\{.+\\}\\}\\s*$" }
                ]
            },
            "metrics": {
                "type": "array",
                "items": { "type": "string", "minLength": 1 }
            }
        },
        "additionalProperties": false
    })
}

/// `AGG_TRIGGER` operator implementation generic over [`StudioExecutionPort`].
pub struct AggTriggerOperator<S> {
    studio_exec: Arc<S>,
}

impl<S> AggTriggerOperator<S>
where
    S: StudioExecutionPort + 'static,
{
    /// Constructs a new [`AggTriggerOperator`].
    #[must_use]
    pub const fn new(studio_exec: Arc<S>) -> Self {
        Self { studio_exec }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let full_type = parse_optional_string_any(&ctx.config, &["full_type", "type_name"])?;
        let node_ids: Vec<Uuid> = parse_optional_uuid_vec(&ctx.config, "node_ids")?;
        let metrics = parse_optional_string_vec(&ctx.config, "metrics")?;
        let explicit_full = parse_optional_bool(&ctx.config, "full")?;

        let full = explicit_full.unwrap_or(full_type.is_none() && node_ids.is_empty());
        let payload = GraphAggregateJobPayload {
            full,
            full_type,
            node_ids,
            metrics,
            schema_version: None,
        };

        let summary = self
            .studio_exec
            .execute_graph_aggregate(&ctx.tenant_id, &payload)
            .await?;

        let output = match summary {
            serde_json::Value::Object(mut map) => {
                map.entry(String::from("status"))
                    .or_insert_with(|| serde_json::Value::String(String::from("SUCCEEDED")));
                serde_json::Value::Object(map)
            }
            other => serde_json::json!({
                "status": "SUCCEEDED",
                "result": other,
            }),
        };

        Ok(OperatorOutcome::Completed(output))
    }
}

impl<S> Operator for AggTriggerOperator<S>
where
    S: StudioExecutionPort + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::AggTrigger
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
