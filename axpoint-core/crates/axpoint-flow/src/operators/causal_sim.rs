//! `CAUSAL_SIM` operator running Copy-on-Write causal propagation scenarios (`07-STEP07-08` §07.4).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, FlowOperator};
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::TopologyRepository;
use axpoint_domain::topology::{AxSimAction, simulate_causal_propagation};

use super::{
    parse_optional_uuid, parse_required_f64, parse_required_string_any, parse_required_uuid_any,
    templated_number_schema, templated_uuid_schema,
};

/// Studio palette display name for `CAUSAL_SIM`.
pub const DISPLAY_NAME: &str = "因果沙盘推演";

/// Studio palette short description for `CAUSAL_SIM`.
pub const DESCRIPTION: &str = "在写时复制沙盘中对节点指标施加假设扰动并计算多跳因果传导影响";

/// Returns the JSON Schema validating `CAUSAL_SIM` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    let action_item = serde_json::json!({
        "type": "object",
        "properties": {
            "target_node_id": templated_uuid_schema(),
            "node_id": templated_uuid_schema(),
            "metric_key": { "type": "string", "minLength": 1 },
            "metric": { "type": "string", "minLength": 1 },
            "delta_ratio": templated_number_schema()
        },
        "required": ["delta_ratio"],
        "additionalProperties": false
    });

    serde_json::json!({
        "type": "object",
        "properties": {
            "sandbox_id": templated_uuid_schema(),
            "actions": {
                "oneOf": [
                    { "type": "array", "items": action_item.clone() },
                    { "type": "string", "pattern": "^\\s*\\{\\{.+\\}\\}\\s*$" }
                ]
            },
            "interventions": {
                "oneOf": [
                    { "type": "array", "items": action_item },
                    { "type": "string", "pattern": "^\\s*\\{\\{.+\\}\\}\\s*$" }
                ]
            }
        },
        "additionalProperties": false
    })
}

/// `CAUSAL_SIM` operator implementation generic over [`TopologyRepository`].
pub struct CausalSimOperator<T> {
    topology: Arc<T>,
}

impl<T> CausalSimOperator<T>
where
    T: TopologyRepository + 'static,
{
    /// Constructs a new [`CausalSimOperator`].
    #[must_use]
    pub const fn new(topology: Arc<T>) -> Self {
        Self { topology }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let sandbox_id = parse_optional_uuid(&ctx.config, "sandbox_id")?;
        let actions = parse_sim_actions(&ctx.config)?;

        let sim_res = if let (Some(sb_id), Environment::Prod) = (sandbox_id, ctx.environment) {
            self.topology
                .simulate_ax_sandbox(&ctx.tenant_id, sb_id, actions)
                .await?
        } else {
            let snapshot = self.topology.get_runtime_snapshot(&ctx.tenant_id).await?;
            simulate_causal_propagation(&snapshot, &actions)
        };

        let max_abs_delta = sim_res
            .impacts
            .iter()
            .map(|r| r.delta.abs())
            .fold(0.0_f64, f64::max);

        Ok(OperatorOutcome::Completed(serde_json::json!({
            "affected_count": sim_res.affected_count,
            "impacts": sim_res.impacts,
            "max_abs_delta": max_abs_delta,
        })))
    }
}

fn parse_sim_actions(config: &serde_json::Value) -> Result<Vec<AxSimAction>, AppError> {
    let raw_arr = config
        .get("actions")
        .or_else(|| config.get("interventions"))
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "CAUSAL_SIM requires 'actions' or 'interventions' array",
            )
        })?
        .as_array()
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "CAUSAL_SIM 'actions' must be a JSON array",
            )
        })?;

    let mut out = Vec::with_capacity(raw_arr.len());
    for item in raw_arr {
        let target_node_id = parse_required_uuid_any(item, &["target_node_id", "node_id"])?;
        let metric_key = parse_required_string_any(item, &["metric_key", "metric"])?;
        let delta_ratio = parse_required_f64(item, "delta_ratio")?;
        out.push(AxSimAction {
            target_node_id,
            metric_key,
            delta_ratio,
        });
    }
    Ok(out)
}

impl<T> Operator for CausalSimOperator<T>
where
    T: TopologyRepository + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::CausalSim
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
