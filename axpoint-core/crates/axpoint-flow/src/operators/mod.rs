//! Built-in Step 07 `AxFlow` operators, shared config helpers, and registry constructors (`07-STEP07-08` §07.4).

pub mod action_emit;
pub mod agent_task;
pub mod agg_trigger;
pub mod causal_sim;
pub mod http_call;
pub mod human_approval;
pub mod llm_extract;
pub mod source_ingest;
pub mod sql_transform;
pub mod xblue_map;

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{
    Operator, OperatorContext, OperatorOutcome, OperatorRegistry, OperatorSpecDto,
};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{
    AgentTaskPort, AiPort, AnalyticsQueryPort, DatasetRepository, FlowCredentialUnsealPort,
    FlowRepository, FlowSqlMaterializePort, GraphStorePort, JobQueuePort, LakeInternalQueryPort,
    MetricStorePort, OutboundUrlGuardPort, StudioExecutionPort, StudioRepository,
    TopologyRepository,
};
use uuid::Uuid;

pub use action_emit::ActionEmitOperator;
pub use agent_task::AgentTaskOperator;
pub use agg_trigger::AggTriggerOperator;
pub use causal_sim::CausalSimOperator;
pub use http_call::HttpCallOperator;
pub use human_approval::HumanApprovalOperator;
pub use llm_extract::LlmExtractOperator;
pub use source_ingest::SourceIngestOperator;
pub use sql_transform::SqlTransformOperator;
pub use xblue_map::XblueMapOperator;

const TEMPLATE_EXPR_PATTERN: &str = "^\\s*\\{\\{.+\\}\\}\\s*$";
const UUID_PATTERN: &str =
    "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

struct SchemaOnlyOperator {
    kind: FlowOperator,
    display_name: &'static str,
    description: &'static str,
    schema_fn: fn() -> serde_json::Value,
}

impl Operator for SchemaOnlyOperator {
    fn kind(&self) -> FlowOperator {
        self.kind
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn config_schema(&self) -> serde_json::Value {
        (self.schema_fn)()
    }

    fn execute<'a>(&'a self, _ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async {
            Err(AppError::Config(String::from(
                "schema-only operator registry cannot execute steps",
            )))
        })
    }
}

/// Port dependency bundle passed to [`build_step07_operator_registry`].
///
/// Notice `AGENT_TASK` is intentionally omitted in Step 07 (registered in Step 09).
pub struct Step07OperatorDeps<R, Q, M, A, L, U, C> {
    /// Storage repository implementing flow, dataset, studio, topology, graph, metric, and job ports.
    pub store: Arc<R>,
    /// `AxAnalytics` guarded SQL execution port.
    pub analytics: Arc<Q>,
    /// Optional `DERIVED_VIEW` SQL materialization port for `SQL_TRANSFORM`.
    pub sql_materializer: Option<Arc<M>>,
    /// AI gateway port for `LLM_EXTRACT`.
    pub ai: Arc<A>,
    /// Trusted internal `DuckLake` query port for `XBLUE_MAP`.
    pub lake: Arc<L>,
    /// Outbound URL & DNS SSRF guard for `HTTP_CALL`.
    pub url_guard: Arc<U>,
    /// Optional credential unsealer for `HTTP_CALL`.
    pub cred_unsealer: Option<Arc<C>>,
}

/// Builds an [`OperatorRegistry`] populated with all 9 executable Step 07 operators (`AGENT_TASK` excluded).
#[must_use]
pub fn build_step07_operator_registry<R, Q, M, A, L, U, C>(
    deps: Step07OperatorDeps<R, Q, M, A, L, U, C>,
) -> OperatorRegistry
where
    R: FlowRepository
        + DatasetRepository
        + JobQueuePort
        + StudioRepository
        + StudioExecutionPort
        + TopologyRepository
        + GraphStorePort
        + MetricStorePort
        + 'static,
    Q: AnalyticsQueryPort + 'static,
    M: FlowSqlMaterializePort + 'static,
    A: AiPort + 'static,
    L: LakeInternalQueryPort + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    let mut reg = OperatorRegistry::new();
    reg.register(Arc::new(SourceIngestOperator::new(
        Arc::clone(&deps.store),
        Arc::clone(&deps.store),
    )));
    reg.register(Arc::new(SqlTransformOperator::new(
        deps.analytics,
        deps.sql_materializer,
    )));
    reg.register(Arc::new(LlmExtractOperator::new(
        deps.ai,
        Arc::clone(&deps.store),
    )));
    reg.register(Arc::new(XblueMapOperator::new(
        Arc::clone(&deps.store),
        deps.lake,
    )));
    reg.register(Arc::new(AggTriggerOperator::new(Arc::clone(&deps.store))));
    reg.register(Arc::new(CausalSimOperator::new(Arc::clone(&deps.store))));
    reg.register(Arc::new(HumanApprovalOperator::new(Arc::clone(
        &deps.store,
    ))));
    reg.register(Arc::new(ActionEmitOperator::new(
        Arc::clone(&deps.store),
        Arc::clone(&deps.store),
        Arc::clone(&deps.store),
    )));
    reg.register(Arc::new(HttpCallOperator::new(
        deps.store,
        deps.url_guard,
        deps.cred_unsealer,
    )));
    reg
}

type SchemaOnlyEntry = (
    FlowOperator,
    &'static str,
    &'static str,
    fn() -> serde_json::Value,
);

/// Builds a lightweight [`OperatorRegistry`] exposing the 9 Step 07 operator metadata & JSON Schemas
/// (`AGENT_TASK` excluded) for static validation and `GET /api/axflow/operators`.
#[must_use]
pub fn step07_schema_only_registry() -> OperatorRegistry {
    let entries: [SchemaOnlyEntry; 9] = [
        (
            FlowOperator::SourceIngest,
            source_ingest::DISPLAY_NAME,
            source_ingest::DESCRIPTION,
            source_ingest::config_schema,
        ),
        (
            FlowOperator::SqlTransform,
            sql_transform::DISPLAY_NAME,
            sql_transform::DESCRIPTION,
            sql_transform::config_schema,
        ),
        (
            FlowOperator::LlmExtract,
            llm_extract::DISPLAY_NAME,
            llm_extract::DESCRIPTION,
            llm_extract::config_schema,
        ),
        (
            FlowOperator::XblueMap,
            xblue_map::DISPLAY_NAME,
            xblue_map::DESCRIPTION,
            xblue_map::config_schema,
        ),
        (
            FlowOperator::AggTrigger,
            agg_trigger::DISPLAY_NAME,
            agg_trigger::DESCRIPTION,
            agg_trigger::config_schema,
        ),
        (
            FlowOperator::CausalSim,
            causal_sim::DISPLAY_NAME,
            causal_sim::DESCRIPTION,
            causal_sim::config_schema,
        ),
        (
            FlowOperator::HumanApproval,
            human_approval::DISPLAY_NAME,
            human_approval::DESCRIPTION,
            human_approval::config_schema,
        ),
        (
            FlowOperator::ActionEmit,
            action_emit::DISPLAY_NAME,
            action_emit::DESCRIPTION,
            action_emit::config_schema,
        ),
        (
            FlowOperator::HttpCall,
            http_call::DISPLAY_NAME,
            http_call::DESCRIPTION,
            http_call::config_schema,
        ),
    ];

    let mut reg = OperatorRegistry::new();
    for (kind, display_name, description, schema_fn) in entries {
        reg.register(Arc::new(SchemaOnlyOperator {
            kind,
            display_name,
            description,
            schema_fn,
        }));
    }
    reg
}

/// Port dependency bundle passed to [`build_step09_operator_registry`].
pub struct Step09OperatorDeps<R, Q, M, A, L, U, C> {
    /// Step 07 core operator dependencies.
    pub step07: Step07OperatorDeps<R, Q, M, A, L, U, C>,
    /// `AxAgent` task execution port (`FlowOperator::AgentTask`).
    pub agent_task: Arc<dyn AgentTaskPort>,
}

/// Port dependency bundle passed to [`build_step10_operator_registry`].
pub struct Step10OperatorDeps<R, Q, M, A, L, U, C> {
    /// Step 09 operator dependencies.
    pub step09: Step09OperatorDeps<R, Q, M, A, L, U, C>,
    /// Full `AxSim` simulation execution port (`FlowOperator::CausalSim`).
    pub sim_port: Arc<dyn axpoint_domain::ports::SimExecutionPort>,
}

/// Builds an [`OperatorRegistry`] populated with all 10 executable Step 09 operators (`9` Step 07 + `AGENT_TASK`).
#[must_use]
pub fn build_step09_operator_registry<R, Q, M, A, L, U, C>(
    deps: Step09OperatorDeps<R, Q, M, A, L, U, C>,
) -> OperatorRegistry
where
    R: FlowRepository
        + DatasetRepository
        + JobQueuePort
        + StudioRepository
        + StudioExecutionPort
        + TopologyRepository
        + GraphStorePort
        + MetricStorePort
        + 'static,
    Q: AnalyticsQueryPort + 'static,
    M: FlowSqlMaterializePort + 'static,
    A: AiPort + 'static,
    L: LakeInternalQueryPort + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    let mut reg = build_step07_operator_registry(deps.step07);
    reg.register(Arc::new(AgentTaskOperator::new(deps.agent_task)));
    reg
}

/// Builds an [`OperatorRegistry`] populated with all 10 executable Step 10 operators
/// (wiring [`axpoint_domain::ports::SimExecutionPort`] into `CAUSAL_SIM`).
#[must_use]
pub fn build_step10_operator_registry<R, Q, M, A, L, U, C>(
    deps: Step10OperatorDeps<R, Q, M, A, L, U, C>,
) -> OperatorRegistry
where
    R: FlowRepository
        + DatasetRepository
        + JobQueuePort
        + StudioRepository
        + StudioExecutionPort
        + TopologyRepository
        + GraphStorePort
        + MetricStorePort
        + 'static,
    Q: AnalyticsQueryPort + 'static,
    M: FlowSqlMaterializePort + 'static,
    A: AiPort + 'static,
    L: LakeInternalQueryPort + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    let store_for_sim = Arc::clone(&deps.step09.step07.store);
    let mut reg = build_step09_operator_registry(deps.step09);
    reg.register(Arc::new(CausalSimOperator::with_sim_port(
        store_for_sim,
        deps.sim_port,
    )));
    reg
}

/// Builds a lightweight [`OperatorRegistry`] exposing all 10 Step 09 operator metadata & JSON Schemas
/// (`9` Step 07 + `AGENT_TASK`) for static validation and `GET /api/axflow/operators`.
#[must_use]
pub fn step09_schema_only_registry() -> OperatorRegistry {
    let mut reg = step07_schema_only_registry();
    reg.register(Arc::new(SchemaOnlyOperator {
        kind: FlowOperator::AgentTask,
        display_name: AgentTaskOperator::DISPLAY_NAME,
        description: AgentTaskOperator::DESCRIPTION,
        schema_fn: AgentTaskOperator::config_schema,
    }));
    reg
}

/// Returns the canonical 10 Step 09 [`OperatorSpecDto`] descriptors (including `AGENT_TASK`)
/// for `GET /api/axflow/operators`.
#[must_use]
pub fn default_operator_specs() -> Vec<OperatorSpecDto> {
    step09_schema_only_registry().specs()
}

pub(crate) fn templated_uuid_schema() -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            { "type": "string", "pattern": UUID_PATTERN },
            { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
        ]
    })
}

pub(crate) fn templated_bool_schema() -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            { "type": "boolean" },
            { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
        ]
    })
}

pub(crate) fn templated_int_schema(min: i64, max: i64) -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            { "type": "integer", "minimum": min, "maximum": max },
            { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
        ]
    })
}

pub(crate) fn templated_number_schema() -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            { "type": "number" },
            { "type": "string", "pattern": TEMPLATE_EXPR_PATTERN }
        ]
    })
}

pub(crate) fn parse_required_string(
    obj: &serde_json::Value,
    key: &str,
) -> Result<String, AppError> {
    parse_required_string_any(obj, &[key])
}

pub(crate) fn parse_required_string_any(
    obj: &serde_json::Value,
    keys: &[&str],
) -> Result<String, AppError> {
    parse_optional_string_any(obj, keys)?.ok_or_else(|| {
        let label = keys.first().copied().unwrap_or("field");
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("missing required string '{label}'"),
        )
    })
}

pub(crate) fn parse_optional_string_any(
    obj: &serde_json::Value,
    keys: &[&str],
) -> Result<Option<String>, AppError> {
    for &k in keys {
        if let Some(val) = obj.get(k) {
            if val.is_null() {
                continue;
            }
            let s = val.as_str().ok_or_else(|| {
                AppError::bad("INVALID_ARGUMENT", format!("'{k}' must be a string"))
            })?;
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Ok(Some(String::from(trimmed)));
            }
        }
    }
    Ok(None)
}

pub(crate) fn parse_required_uuid(obj: &serde_json::Value, key: &str) -> Result<Uuid, AppError> {
    parse_required_uuid_any(obj, &[key])
}

pub(crate) fn parse_required_uuid_any(
    obj: &serde_json::Value,
    keys: &[&str],
) -> Result<Uuid, AppError> {
    let raw = parse_required_string_any(obj, keys)?;
    Uuid::parse_str(&raw)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid UUID '{raw}': {err}")))
}

pub(crate) fn parse_optional_uuid(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Option<Uuid>, AppError> {
    parse_optional_uuid_any(obj, &[key])
}

pub(crate) fn parse_optional_uuid_any(
    obj: &serde_json::Value,
    keys: &[&str],
) -> Result<Option<Uuid>, AppError> {
    match parse_optional_string_any(obj, keys)? {
        Some(raw) => Uuid::parse_str(&raw).map(Some).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid UUID '{raw}': {err}"))
        }),
        None => Ok(None),
    }
}

pub(crate) fn parse_optional_bool(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Option<bool>, AppError> {
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
        Some(serde_json::Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Some(true)),
            "false" | "0" => Ok(Some(false)),
            other => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("'{key}' must be boolean, got '{other}'"),
            )),
        },
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("'{key}' must be boolean"),
        )),
    }
}

pub(crate) fn parse_optional_u64(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Option<u64>, AppError> {
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("'{key}' must be >= 0"))),
        Some(serde_json::Value::String(s)) => s
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid '{key}': {err}"))),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("'{key}' must be an integer"),
        )),
    }
}

pub(crate) fn parse_optional_u32(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Option<u32>, AppError> {
    match parse_optional_u64(obj, key)? {
        Some(v) => u32::try_from(v)
            .map(Some)
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", format!("'{key}' out of u32 range"))),
        None => Ok(None),
    }
}

pub(crate) fn parse_optional_i64_any(
    obj: &serde_json::Value,
    keys: &[&str],
) -> Result<Option<i64>, AppError> {
    for &k in keys {
        match obj.get(k) {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Number(n)) => {
                return n.as_i64().map(Some).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", format!("'{k}' must be an integer"))
                });
            }
            Some(serde_json::Value::String(s)) => {
                return s.trim().parse::<i64>().map(Some).map_err(|err| {
                    AppError::bad("INVALID_ARGUMENT", format!("invalid '{k}': {err}"))
                });
            }
            _ => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("'{k}' must be an integer"),
                ));
            }
        }
    }
    Ok(None)
}

pub(crate) fn parse_required_f64(obj: &serde_json::Value, key: &str) -> Result<f64, AppError> {
    parse_optional_f64(obj, key)?
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("missing number '{key}'")))
}

pub(crate) fn parse_optional_f64(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Option<f64>, AppError> {
    let val_opt =
        match obj.get(key) {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Number(n)) => Some(n.as_f64().ok_or_else(|| {
                AppError::bad("INVALID_ARGUMENT", format!("'{key}' must be finite number"))
            })?),
            Some(serde_json::Value::String(s)) => Some(s.trim().parse::<f64>().map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid '{key}': {err}"))
            })?),
            _ => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("'{key}' must be a number"),
                ));
            }
        };
    if let Some(v) = val_opt
        && !v.is_finite()
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("'{key}' must be a finite number"),
        ));
    }
    Ok(val_opt)
}

pub(crate) fn parse_optional_string_vec(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Vec<String>, AppError> {
    let Some(val) = obj.get(key) else {
        return Ok(Vec::new());
    };
    if val.is_null() {
        return Ok(Vec::new());
    }
    let arr = val.as_array().ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("'{key}' must be a string array"),
        )
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            AppError::bad("INVALID_ARGUMENT", format!("'{key}' items must be strings"))
        })?;
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            out.push(String::from(trimmed));
        }
    }
    Ok(out)
}

pub(crate) fn parse_optional_uuid_vec(
    obj: &serde_json::Value,
    key: &str,
) -> Result<Vec<Uuid>, AppError> {
    let strs = parse_optional_string_vec(obj, key)?;
    let mut out = Vec::with_capacity(strs.len());
    for raw in strs {
        let parsed = Uuid::parse_str(&raw).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid UUID in '{key}': {err}"),
            )
        })?;
        out.push(parsed);
    }
    Ok(out)
}
