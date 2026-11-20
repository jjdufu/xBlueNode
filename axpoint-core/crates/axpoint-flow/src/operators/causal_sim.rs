//! `CAUSAL_SIM` operator running Copy-on-Write causal propagation scenarios (`07-STEP07-08` §07.4, `09-STEP10` §10.3–§10.9).

use std::str::FromStr;
use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::agent::{SimRunToolInput, SimRunToolOutput};
use axpoint_domain::enums::{Environment, FlowOperator, InterventionMode};
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{SimExecutionPort, TopologyRepository};
use axpoint_domain::sim::{
    DEFAULT_SIM_CHECKPOINT_EVERY, DEFAULT_SIM_DAMPING, DEFAULT_SIM_HORIZON_STEPS,
    DEFAULT_SIM_MAX_ITERATIONS, DEFAULT_SIM_MIN_EFFECT, DEFAULT_SIM_TOLERANCE, SimMode, SimParams,
    UncertaintyConfig,
};
use axpoint_domain::topology::{AxSimAction, simulate_causal_propagation};

use super::{
    parse_optional_bool, parse_optional_f64, parse_optional_string_any, parse_optional_u32,
    parse_optional_u64, parse_optional_uuid, parse_required_f64, parse_required_string_any,
    parse_required_uuid_any, templated_bool_schema, templated_int_schema, templated_number_schema,
    templated_uuid_schema,
};

/// Studio palette display name for `CAUSAL_SIM`.
pub const DISPLAY_NAME: &str = "因果沙盘推演";

/// Studio palette short description for `CAUSAL_SIM`.
pub const DESCRIPTION: &str = "在写时复制沙盘中对节点指标施加假设扰动并计算多跳因果传导影响";

const TEMPLATE_EXPR_PATTERN: &str = "^\\s*\\{\\{.+\\}\\}\\s*$";

/// Returns the JSON Schema validating `CAUSAL_SIM` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    let action_item = action_item_schema();
    let params_schema = params_object_schema();

    serde_json::json!({
        "type": "object",
        "properties": {
            "sandbox_id": templated_uuid_schema(),
            "scenario_name": { "type": "string", "minLength": 1 },
            "actions": {
                "oneOf": [
                    { "type": "array", "items": action_item.clone() },
                    { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
                ]
            },
            "interventions": {
                "oneOf": [
                    { "type": "array", "items": action_item },
                    { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
                ]
            },
            "max_hops": templated_int_schema(1, 100),
            "params": params_schema
        },
        "anyOf": [
            { "required": ["actions"] },
            { "required": ["interventions"] }
        ],
        "additionalProperties": false
    })
}

fn action_item_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "target_node_id": templated_uuid_schema(),
            "node_id": templated_uuid_schema(),
            "metric_key": { "type": "string", "minLength": 1 },
            "metric": { "type": "string", "minLength": 1 },
            "mode": {
                "oneOf": [
                    { "type": "string", "enum": ["RATIO", "ADD", "ABSOLUTE"] },
                    { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
                ]
            },
            "value": templated_number_schema(),
            "delta_ratio": templated_number_schema(),
            "at_step": templated_int_schema(0, 104)
        },
        "allOf": [
            { "anyOf": [{ "required": ["target_node_id"] }, { "required": ["node_id"] }] },
            { "anyOf": [{ "required": ["metric_key"] }, { "required": ["metric"] }] },
            { "anyOf": [{ "required": ["delta_ratio"] }, { "required": ["mode", "value"] }] }
        ],
        "additionalProperties": false
    })
}

fn params_object_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "mode": {
                "oneOf": [
                    { "type": "string", "enum": ["STEADY", "STEPPED"] },
                    { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
                ]
            },
            "horizon_steps": templated_int_schema(1, 104),
            "damping": templated_number_schema(),
            "max_iters": templated_int_schema(1, 100_000),
            "max_iterations": templated_int_schema(1, 100_000),
            "tol": templated_number_schema(),
            "tolerance": templated_number_schema(),
            "min_effect": templated_number_schema(),
            "max_hops": templated_int_schema(1, 100),
            "checkpoint_every": templated_int_schema(1, 1_000),
            "uncertainty": {
                "type": "object",
                "properties": {
                    "enabled": templated_bool_schema(),
                    "samples": templated_int_schema(1, 500),
                    "sigma_rel": templated_number_schema(),
                    "coefficient_cv": templated_number_schema(),
                    "seed": templated_int_schema(0, 9_007_199_254_740_991)
                },
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

/// `CAUSAL_SIM` operator implementation generic over [`TopologyRepository`].
pub struct CausalSimOperator<T> {
    topology: Arc<T>,
    sim_port: Option<Arc<dyn SimExecutionPort>>,
}

impl<T> CausalSimOperator<T>
where
    T: TopologyRepository + 'static,
{
    /// Constructs a new [`CausalSimOperator`] using the fallback topology snapshot path.
    #[must_use]
    pub const fn new(topology: Arc<T>) -> Self {
        Self {
            topology,
            sim_port: None,
        }
    }

    /// Constructs a [`CausalSimOperator`] backed by the full Step 10 [`SimExecutionPort`].
    #[must_use]
    pub const fn with_sim_port(topology: Arc<T>, sim_port: Arc<dyn SimExecutionPort>) -> Self {
        Self {
            topology,
            sim_port: Some(sim_port),
        }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let sandbox_id = parse_optional_uuid(&ctx.config, "sandbox_id")?;
        let scenario_name = parse_optional_string_any(&ctx.config, &["scenario_name"])?;
        let actions = parse_sim_actions(&ctx.config)?;
        let params = parse_sim_params(&ctx.config)?;
        let has_params_cfg = ctx.config.get("params").is_some_and(|v| !v.is_null());

        if let Some(port) = &self.sim_port {
            let interventions = if has_params_cfg {
                serde_json::json!({
                    "actions": actions,
                    "params": params,
                })
            } else {
                serde_json::to_value(&actions)
                    .map_err(|err| AppError::Config(format!("serialize AxSimAction list: {err}")))?
            };
            let input = SimRunToolInput {
                sandbox_id,
                scenario_name,
                interventions,
                max_hops: params.max_hops,
            };
            let out = port
                .run_simulation_tool(&ctx.tenant_id, ctx.principal.user_id, input)
                .await?;
            return Ok(OperatorOutcome::Completed(build_sim_tool_outcome(&out)));
        }

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
            "kpis": sim_res.kpis,
            "warnings": sim_res.warnings,
            "converged": sim_res.converged,
            "max_abs_delta": max_abs_delta,
        })))
    }
}

fn build_sim_tool_outcome(out: &SimRunToolOutput) -> serde_json::Value {
    let mut map = match &out.summary {
        serde_json::Value::Object(obj) => obj.clone(),
        other => {
            let mut m = serde_json::Map::new();
            m.insert(String::from("summary"), other.clone());
            m
        }
    };

    map.insert(
        String::from("sandbox_id"),
        serde_json::json!(out.sandbox_id),
    );
    if let Some(scenario_id) = out.scenario_id {
        map.insert(String::from("scenario_id"), serde_json::json!(scenario_id));
    }

    if !map.contains_key("impacts") {
        let fallback_impacts = map
            .get("top_impacts")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        map.insert(
            String::from("impacts"),
            serde_json::Value::Array(fallback_impacts),
        );
    }

    if !map.contains_key("affected_count") {
        let count_from_impacts = map
            .get("impacts")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        let derived_count = if count_from_impacts > 0 {
            serde_json::json!(count_from_impacts)
        } else if let Some(n) = map
            .get("impacted_nodes")
            .and_then(serde_json::Value::as_u64)
        {
            serde_json::json!(n)
        } else {
            serde_json::json!(0)
        };
        map.insert(String::from("affected_count"), derived_count);
    }

    if !map.contains_key("max_abs_delta") {
        let max_abs_delta = map
            .get("impacts")
            .and_then(serde_json::Value::as_array)
            .map_or(0.0, |rows| {
                rows.iter()
                    .filter_map(|row| row.get("delta").and_then(serde_json::Value::as_f64))
                    .map(f64::abs)
                    .fold(0.0_f64, f64::max)
            });
        map.insert(
            String::from("max_abs_delta"),
            serde_json::json!(max_abs_delta),
        );
    }

    serde_json::Value::Object(map)
}

/// Parses `CAUSAL_SIM` `actions` / `interventions` into canonical [`AxSimAction`] entries.
///
/// Supports Step 10 `mode` (`RATIO`, `ADD`, `ABSOLUTE`), `value`, `at_step`, and legacy `delta_ratio` configs.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `actions`/`interventions` is missing,
/// not an array, or any entry has invalid UUIDs, missing metrics, conflicting `delta_ratio` vs
/// `mode`/`value`, or `RATIO <= -1.0`.
pub fn parse_sim_actions(config: &serde_json::Value) -> Result<Vec<AxSimAction>, AppError> {
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
        out.push(parse_single_action(item)?);
    }
    Ok(out)
}

fn parse_single_action(item: &serde_json::Value) -> Result<AxSimAction, AppError> {
    let target_node_id = parse_required_uuid_any(item, &["target_node_id", "node_id"])?;
    let metric_key = parse_required_string_any(item, &["metric_key", "metric"])?;
    let mode = match parse_optional_string_any(item, &["mode"])? {
        Some(raw) => Some(InterventionMode::from_str(&raw.to_ascii_uppercase())?),
        None => None,
    };
    let delta_ratio = parse_optional_f64(item, "delta_ratio")?;
    let at_step = parse_optional_u32(item, "at_step")?.unwrap_or(0);

    let value = match (delta_ratio, mode) {
        (Some(_), Some(_)) => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "CAUSAL_SIM action cannot specify both 'delta_ratio' and 'mode'/'value'",
            ));
        }
        (Some(dr), None) => {
            if parse_optional_f64(item, "value")?.is_some() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "CAUSAL_SIM action cannot specify both 'delta_ratio' and 'mode'/'value'",
                ));
            }
            if dr <= -1.0 {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "RATIO delta_ratio must be > -1.0",
                ));
            }
            None
        }
        (None, Some(m)) => {
            let v = parse_required_f64(item, "value")?;
            if m == InterventionMode::Ratio && v <= -1.0 {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "RATIO intervention value must be > -1.0",
                ));
            }
            Some(v)
        }
        (None, None) => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "CAUSAL_SIM action requires either 'delta_ratio' or 'mode' + 'value'",
            ));
        }
    };

    Ok(AxSimAction {
        target_node_id,
        metric_key,
        mode,
        value,
        delta_ratio,
        at_step,
    })
}

/// Parses optional Step 10 `params` and top-level `max_hops` into [`SimParams`].
///
/// Supports both short (`tol`, `max_iters`, `uncertainty.sigma_rel`) and canonical
/// (`tolerance`, `max_iterations`, `uncertainty.coefficient_cv`) field names.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if any parameter has an invalid type or
/// falls outside allowed Step 10 bounds.
pub fn parse_sim_params(config: &serde_json::Value) -> Result<SimParams, AppError> {
    let top_max_hops = parse_optional_u32(config, "max_hops")?;
    let Some(raw_params) = config.get("params").filter(|v| !v.is_null()) else {
        return Ok(SimParams {
            max_hops: top_max_hops,
            ..SimParams::default()
        });
    };
    if !raw_params.is_object() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "CAUSAL_SIM 'params' must be a JSON object",
        ));
    }

    let mode = parse_sim_mode(raw_params)?;
    let damping = parse_optional_f64(raw_params, "damping")?.unwrap_or(DEFAULT_SIM_DAMPING);
    if !(damping > 0.0 && damping <= 1.0) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.damping must be in (0.0, 1.0]",
        ));
    }

    let tolerance =
        parse_optional_f64_any(raw_params, &["tolerance", "tol"])?.unwrap_or(DEFAULT_SIM_TOLERANCE);
    if tolerance <= 0.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.tolerance must be > 0.0",
        ));
    }

    let max_iterations = parse_optional_u32_any(raw_params, &["max_iterations", "max_iters"])?
        .unwrap_or(DEFAULT_SIM_MAX_ITERATIONS);
    if max_iterations < 1 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.max_iterations must be >= 1",
        ));
    }

    let min_effect =
        parse_optional_f64(raw_params, "min_effect")?.unwrap_or(DEFAULT_SIM_MIN_EFFECT);
    if min_effect < 0.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.min_effect must be >= 0.0",
        ));
    }

    let horizon_steps =
        parse_optional_u32(raw_params, "horizon_steps")?.unwrap_or(DEFAULT_SIM_HORIZON_STEPS);
    if !(1..=104).contains(&horizon_steps) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.horizon_steps must be in 1..=104",
        ));
    }

    let checkpoint_every =
        parse_optional_u32(raw_params, "checkpoint_every")?.unwrap_or(DEFAULT_SIM_CHECKPOINT_EVERY);
    if checkpoint_every < 1 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.checkpoint_every must be >= 1",
        ));
    }

    let max_hops = parse_optional_u32(raw_params, "max_hops")?.or(top_max_hops);
    let uncertainty = parse_uncertainty_config(raw_params)?;

    Ok(SimParams {
        mode,
        damping,
        tolerance,
        max_iterations,
        min_effect,
        max_hops,
        horizon_steps,
        uncertainty,
        checkpoint_every,
    })
}

fn parse_sim_mode(raw_params: &serde_json::Value) -> Result<SimMode, AppError> {
    let Some(raw) = parse_optional_string_any(raw_params, &["mode"])? else {
        return Ok(SimMode::Steady);
    };
    match raw.to_ascii_uppercase().as_str() {
        "STEADY" => Ok(SimMode::Steady),
        "STEPPED" => Ok(SimMode::Stepped),
        other => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown SimMode '{other}', expected STEADY or STEPPED"),
        )),
    }
}

fn parse_uncertainty_config(
    raw_params: &serde_json::Value,
) -> Result<Option<UncertaintyConfig>, AppError> {
    let Some(raw_unc) = raw_params.get("uncertainty").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    if !raw_unc.is_object() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.uncertainty must be a JSON object",
        ));
    }

    if parse_optional_bool(raw_unc, "enabled")? == Some(false) {
        return Ok(None);
    }

    let defaults = UncertaintyConfig::default();
    let samples = parse_optional_u32(raw_unc, "samples")?.unwrap_or(defaults.samples);
    if !(1..=500).contains(&samples) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.uncertainty.samples must be in 1..=500",
        ));
    }

    let coefficient_cv = parse_optional_f64_any(raw_unc, &["coefficient_cv", "sigma_rel"])?
        .unwrap_or(defaults.coefficient_cv);
    if coefficient_cv < 0.0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "params.uncertainty.sigma_rel must be >= 0.0",
        ));
    }

    let seed = parse_optional_u64(raw_unc, "seed")?.unwrap_or(defaults.seed);
    Ok(Some(UncertaintyConfig {
        samples,
        coefficient_cv,
        seed,
    }))
}

fn parse_optional_f64_any(obj: &serde_json::Value, keys: &[&str]) -> Result<Option<f64>, AppError> {
    for &key in keys {
        if let Some(val) = parse_optional_f64(obj, key)? {
            return Ok(Some(val));
        }
    }
    Ok(None)
}

fn parse_optional_u32_any(obj: &serde_json::Value, keys: &[&str]) -> Result<Option<u32>, AppError> {
    for &key in keys {
        if let Some(val) = parse_optional_u32(obj, key)? {
            return Ok(Some(val));
        }
    }
    Ok(None)
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
