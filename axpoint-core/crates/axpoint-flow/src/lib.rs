//! `AxFlow` persistent DAG orchestration engine, static validator, template renderer,
//! Step 07 operator registry, cron scheduler, outbox triggers, Step 08 operational rules
//! evaluator, and 5-channel notification dispatcher (`07-STEP07-08` §07.1–§08.4).
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub mod engine;
pub mod graph;
pub mod notify;
pub mod operators;
pub mod rules;
pub mod scheduler;
pub mod template;
pub mod triggers;
pub mod validate;

#[cfg(test)]
mod tests;

pub use engine::{
    FlowEngine, FlowStepJobHandler, INLINE_OUTPUT_MAX_BYTES, compute_retry_backoff_secs,
    eval_json_guard_expr, plan_progression, signal_run_canceled,
};
pub use graph::{
    ancestors, leaf_nodes, node_map, predecessors, root_nodes, strict_descendants, successors,
    topological_order_or_cycle,
};
pub use notify::{
    AlertEscalationNotifier, AlertNotifySubscriber, MAX_NOTIFY_ATTEMPTS, NOTIFY_HTTP_TIMEOUT_SECS,
    NoopCredentialUnsealer, NotificationDispatcher, NotifyPayloadView, delivery_retry_delay,
    is_loopback_url,
};
pub use operators::{
    ActionEmitOperator, AgentTaskOperator, AggTriggerOperator, CausalSimOperator, HttpCallOperator,
    HumanApprovalOperator, LlmExtractOperator, SourceIngestOperator, SqlTransformOperator,
    Step07OperatorDeps, Step09OperatorDeps, Step10OperatorDeps, XblueMapOperator,
    build_step07_operator_registry, build_step09_operator_registry, build_step10_operator_registry,
    default_operator_specs, step07_schema_only_registry, step09_schema_only_registry,
};
pub use rules::{
    AbsenceRuleScanner, ConditionOutcome, FlowEngineLauncher, RuleEvalJobHandler, RuleEvaluator,
    RuleFlowLauncherPort, RuleTriggerSubscriber, eval_node_filter,
};
pub use scheduler::{
    FlowCronScheduler, cron_idempotency_key, due_cron_minute_slot, parse_timezone_offset,
    to_six_field_cron,
};
pub use template::{
    TemplateContext, TemplateSegment, collect_config_template_errors, parse_template_segments,
    render_config_value, validate_placeholder_expr,
};
pub use triggers::{ApprovalDecidedSubscriber, FlowOutboxTriggerSubscriber};
pub use validate::{validate_flow, validate_flow_default};
