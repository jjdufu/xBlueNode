//! Operational rules condition engine, alert lifecycle evaluator, outbox triggers,
//! and periodic absence/escalation scanner (`07-STEP07-08` §08.2–§08.3).

pub mod condition;
pub mod evaluator;
pub mod subscribers;

pub use condition::{
    ConditionOutcome, build_node_eval_env, eval_absence_points, eval_anomaly_zscore_points,
    eval_change_rate_points, eval_event_condition, eval_expr_condition,
    eval_metric_condition_for_node, eval_node_filter, eval_quality_condition,
    eval_threshold_points,
};
pub use evaluator::{FlowEngineLauncher, RuleEvaluator, RuleFlowLauncherPort};
pub use subscribers::{AbsenceRuleScanner, RuleEvalJobHandler, RuleTriggerSubscriber};
