//! Rule condition evaluators for all 7 [`Condition`] variants and Step 06 `node_filter` (`07-STEP07-08` §08.2).

use std::collections::BTreeMap;
use std::num::FpCategory;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{QualityOnFail, QualityReport};
use axpoint_domain::expr::{EvalEnv, ExprValue, eval as eval_expr, parse as parse_expr};
use axpoint_domain::ports::RulesRepository;
use axpoint_domain::rules::{ComparisonOp, Condition, EventRuleTarget};
use axpoint_domain::topology::BlueNode;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::engine::eval_json_guard_expr;

/// Evaluation outcome of a rule [`Condition`] on a single candidate object.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionOutcome {
    /// Whether the rule condition evaluated to `true`.
    pub matched: bool,
    /// Primary observed numeric value (`{{value}}`), if applicable.
    pub value: Option<f64>,
    /// Diagnostic metadata merged into `ax_alerts.detail`.
    pub detail: serde_json::Value,
}

impl ConditionOutcome {
    /// Constructs a non-matching outcome with empty detail.
    #[must_use]
    pub fn not_matched() -> Self {
        Self {
            matched: false,
            value: None,
            detail: serde_json::json!({}),
        }
    }

    /// Constructs a non-matching outcome preserving an observed `value`.
    #[must_use]
    pub fn not_matched_with_value(value: f64) -> Self {
        Self {
            matched: false,
            value: value.is_finite().then_some(value),
            detail: serde_json::json!({}),
        }
    }
}

/// Builds a Step 06 [`EvalEnv`] from a [`BlueNode`]'s numeric `metrics` and `properties`.
#[must_use]
pub fn build_node_eval_env(node: &BlueNode) -> EvalEnv {
    let mut metrics = BTreeMap::new();
    for (k, &v) in &node.metrics {
        if v.is_finite() {
            metrics.insert(k.clone(), v);
        }
    }

    let mut properties = BTreeMap::new();
    properties.insert(String::from("key"), ExprValue::Text(node.node_key.clone()));
    properties.insert(
        String::from("node_key"),
        ExprValue::Text(node.node_key.clone()),
    );
    properties.insert(String::from("title"), ExprValue::Text(node.title.clone()));
    properties.insert(
        String::from("type_name"),
        ExprValue::Text(node.type_name.clone()),
    );
    for (k, val) in &node.properties {
        let ev = match val {
            serde_json::Value::Bool(b) => ExprValue::Bool(*b),
            serde_json::Value::String(s) => ExprValue::Text(s.clone()),
            serde_json::Value::Number(n) => n
                .as_f64()
                .filter(|f| f.is_finite())
                .map_or(ExprValue::Null, ExprValue::Number),
            serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => ExprValue::Null,
        };
        properties.insert(k.clone(), ev);
    }

    EvalEnv {
        metrics,
        properties,
    }
}

/// Evaluates an optional Step 06 `node_filter` boolean expression against `node` (`07-STEP07-08` §08.2).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`EXPR_INVALID`) if `filter_expr` fails to parse.
pub fn eval_node_filter(filter_expr: &str, node: &BlueNode) -> Result<bool, AppError> {
    let trimmed = filter_expr.trim();
    if trimmed.is_empty() {
        return Ok(true);
    }
    let ast = parse_expr(trimmed).map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;
    let env = build_node_eval_env(node);
    Ok(eval_expr(&ast, &env).value == ExprValue::Bool(true))
}

/// Evaluates `Condition::Threshold` over raw `(ts, value)` points and optional snapshot fallback.
#[must_use]
pub fn eval_threshold_points(
    op: ComparisonOp,
    threshold: f64,
    for_minutes: u32,
    raw_points: &[(DateTime<Utc>, f64)],
    snapshot_fallback: Option<f64>,
    now: DateTime<Utc>,
) -> ConditionOutcome {
    if for_minutes == 0 {
        let latest = raw_points
            .iter()
            .filter(|(_, v)| v.is_finite())
            .max_by_key(|(ts, _)| *ts)
            .map(|(_, v)| *v)
            .or_else(|| snapshot_fallback.filter(|v| v.is_finite()));
        let Some(val) = latest else {
            return ConditionOutcome::not_matched();
        };
        return ConditionOutcome {
            matched: op.compare(val, threshold),
            value: Some(val),
            detail: serde_json::json!({ "op": op.as_str(), "threshold": threshold }),
        };
    }

    let since = now - ChronoDuration::minutes(i64::from(for_minutes));
    let window: Vec<(DateTime<Utc>, f64)> = raw_points
        .iter()
        .copied()
        .filter(|(ts, v)| *ts >= since && *ts <= now && v.is_finite())
        .collect();
    let Some(&(_, latest_val)) = window.iter().max_by_key(|(ts, _)| *ts) else {
        return ConditionOutcome::not_matched();
    };
    let all_match = window.iter().all(|(_, v)| op.compare(*v, threshold));
    ConditionOutcome {
        matched: all_match,
        value: Some(latest_val),
        detail: serde_json::json!({
            "op": op.as_str(),
            "threshold": threshold,
            "for_minutes": for_minutes,
            "window_points": window.len(),
        }),
    }
}

/// Evaluates `Condition::ChangeRate` (`(latest - window_start) / |window_start|`) with `window_start == 0` guard.
#[must_use]
pub fn eval_change_rate_points(
    window_minutes: u32,
    op: ComparisonOp,
    pct: f64,
    raw_points: &[(DateTime<Utc>, f64)],
    now: DateTime<Utc>,
) -> ConditionOutcome {
    let mins = i64::from(window_minutes.max(1));
    let since = now - ChronoDuration::minutes(mins);
    let mut window: Vec<(DateTime<Utc>, f64)> = raw_points
        .iter()
        .copied()
        .filter(|(ts, v)| *ts >= since && *ts <= now && v.is_finite())
        .collect();
    window.sort_by_key(|(ts, _)| *ts);

    if window.len() < 2 {
        return ConditionOutcome::not_matched();
    }
    let (Some(&(_, v_past)), Some(&(_, v_latest))) = (window.first(), window.last()) else {
        return ConditionOutcome::not_matched();
    };
    if matches!(v_past.classify(), FpCategory::Zero) || v_past.abs() <= 1e-15 {
        return ConditionOutcome::not_matched();
    }

    let rate = (v_latest - v_past) / v_past.abs();
    if !rate.is_finite() {
        return ConditionOutcome::not_matched();
    }
    ConditionOutcome {
        matched: op.compare(rate, pct),
        value: Some(rate),
        detail: serde_json::json!({
            "window_minutes": window_minutes,
            "window_start": v_past,
            "latest": v_latest,
            "change_rate": rate,
            "pct": pct,
        }),
    }
}

/// Evaluates `Condition::Absence` (`true` when no raw points exist in `[now - minutes, now]`).
#[must_use]
pub fn eval_absence_points(
    minutes: u32,
    raw_points: &[(DateTime<Utc>, f64)],
    now: DateTime<Utc>,
) -> ConditionOutcome {
    let since = now - ChronoDuration::minutes(i64::from(minutes.max(1)));
    let has_recent = raw_points
        .iter()
        .any(|(ts, v)| *ts >= since && v.is_finite());
    ConditionOutcome {
        matched: !has_recent,
        value: None,
        detail: serde_json::json!({ "absence_minutes": minutes }),
    }
}

/// Evaluates `Condition::AnomalyZscore` (`|latest - mean| / sigma >= z`) with `min_points` and `sigma == 0` guards.
#[must_use]
pub fn eval_anomaly_zscore_points(
    lookback_points: usize,
    z_threshold: f64,
    min_points: usize,
    raw_points: &[(DateTime<Utc>, f64)],
) -> ConditionOutcome {
    let mut sorted: Vec<(DateTime<Utc>, f64)> = raw_points
        .iter()
        .copied()
        .filter(|(_, v)| v.is_finite())
        .collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.0));

    let Some(&(_, latest)) = sorted.first() else {
        return ConditionOutcome::not_matched();
    };
    let history: Vec<f64> = sorted
        .iter()
        .skip(1)
        .take(lookback_points.max(1))
        .map(|(_, v)| *v)
        .collect();
    let required = min_points.max(1);
    if history.len() < required {
        return ConditionOutcome::not_matched_with_value(latest);
    }

    let n = f64::from(u32::try_from(history.len()).unwrap_or(u32::MAX));
    let mean = history.iter().sum::<f64>() / n;
    let variance = history
        .iter()
        .map(|v| {
            let diff = *v - mean;
            diff * diff
        })
        .sum::<f64>()
        / n;
    let sigma = variance.sqrt();
    if !sigma.is_finite() || sigma <= 1e-12 {
        return ConditionOutcome::not_matched_with_value(latest);
    }

    let z_score = (latest - mean).abs() / sigma;
    ConditionOutcome {
        matched: z_score.is_finite() && z_score >= z_threshold,
        value: Some(latest),
        detail: serde_json::json!({
            "z_score": z_score,
            "z_threshold": z_threshold,
            "mean": mean,
            "std_dev": sigma,
            "sample_count": history.len(),
        }),
    }
}

/// Evaluates `Condition::Expr` against `node` metrics and `prop.*` attributes.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`EXPR_INVALID`) if `expression` fails to parse.
pub fn eval_expr_condition(
    expression: &str,
    node: &BlueNode,
    metric: &str,
    latest_raw: Option<f64>,
) -> Result<ConditionOutcome, AppError> {
    let ast =
        parse_expr(expression).map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;
    let mut env = build_node_eval_env(node);
    if let Some(v) = latest_raw.filter(|f| f.is_finite()) {
        env.metrics.insert(String::from(metric), v);
    }
    let observed = env.metrics.get(metric).copied();
    let report = eval_expr(&ast, &env);
    Ok(ConditionOutcome {
        matched: report.value == ExprValue::Bool(true),
        value: observed,
        detail: serde_json::json!({ "expression": expression }),
    })
}

/// Evaluates `Condition::QualityFailed` against a dataset synchronization [`QualityReport`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] if `condition` is not `Condition::QualityFailed`.
pub fn eval_quality_condition(
    condition: &Condition,
    report: Option<&QualityReport>,
) -> Result<ConditionOutcome, AppError> {
    let Condition::QualityFailed { min_severity } = condition else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "DATASET_QUALITY scope requires QUALITY_FAILED condition",
        ));
    };
    let Some(rep) = report else {
        return Ok(ConditionOutcome::not_matched());
    };
    let failed_rules: Vec<&str> = rep
        .rules
        .iter()
        .filter(|r| {
            !r.passed
                && match min_severity {
                    QualityOnFail::Block => r.on_fail == QualityOnFail::Block,
                    QualityOnFail::Warn => true,
                }
        })
        .map(|r| r.rule_kind.as_str())
        .collect();

    let matched = match min_severity {
        QualityOnFail::Block => rep.blocked || !failed_rules.is_empty(),
        QualityOnFail::Warn => !rep.passed || rep.blocked || !failed_rules.is_empty(),
    };
    let count_f64 = f64::from(u32::try_from(failed_rules.len()).unwrap_or(u32::MAX));
    Ok(ConditionOutcome {
        matched,
        value: matched.then_some(count_f64),
        detail: serde_json::json!({
            "min_severity": min_severity.as_str(),
            "blocked": rep.blocked,
            "failed_rules": failed_rules,
        }),
    })
}

/// Evaluates `Condition::EventMatch` against an outbox topic and payload.
///
/// # Errors
/// Returns [`AppError::BadRequest`] if `condition` is not `EVENT_MATCH` or `target.filter` is invalid.
pub fn eval_event_condition(
    target: &EventRuleTarget,
    condition: &Condition,
    event_topic: &str,
    event_payload: &serde_json::Value,
) -> Result<ConditionOutcome, AppError> {
    let Condition::EventMatch {} = condition else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "EVENT scope requires EVENT_MATCH condition",
        ));
    };
    if target.topic.trim() != event_topic.trim() {
        return Ok(ConditionOutcome::not_matched());
    }
    if let Some(filter_str) = target.filter.as_deref().filter(|s| !s.trim().is_empty()) {
        let passed = eval_json_guard_expr(filter_str, event_payload)?;
        if !passed {
            return Ok(ConditionOutcome::not_matched());
        }
    }
    Ok(ConditionOutcome {
        matched: true,
        value: None,
        detail: serde_json::json!({ "topic": event_topic }),
    })
}

/// Queries raw metric points from `repo` as needed and evaluates a `METRIC`-scope [`Condition`].
///
/// # Errors
/// Returns [`AppError`] if querying metric points fails or `condition` is invalid for `METRIC` scope.
pub async fn eval_metric_condition_for_node<R: RulesRepository>(
    repo: &R,
    tenant_id: &str,
    node: &BlueNode,
    metric: &str,
    condition: &Condition,
    now: DateTime<Utc>,
) -> Result<ConditionOutcome, AppError> {
    match *condition {
        Condition::Threshold {
            op,
            value,
            for_minutes,
        } => {
            let (since, limit) = if for_minutes == 0 {
                (None, 1)
            } else {
                (
                    Some(now - ChronoDuration::minutes(i64::from(for_minutes))),
                    5_000,
                )
            };
            let pts = repo
                .query_raw_metric_points(tenant_id, node.id, metric, since, limit)
                .await?;
            let snap = node.metrics.get(metric).copied();
            Ok(eval_threshold_points(
                op,
                value,
                for_minutes,
                &pts,
                snap,
                now,
            ))
        }
        Condition::ChangeRate {
            window_minutes,
            op,
            pct,
        } => {
            let since = now - ChronoDuration::minutes(i64::from(window_minutes.max(1)));
            let pts = repo
                .query_raw_metric_points(tenant_id, node.id, metric, Some(since), 5_000)
                .await?;
            Ok(eval_change_rate_points(window_minutes, op, pct, &pts, now))
        }
        Condition::Absence { minutes } => {
            let since = now - ChronoDuration::minutes(i64::from(minutes.max(1)));
            let pts = repo
                .query_raw_metric_points(tenant_id, node.id, metric, Some(since), 1)
                .await?;
            Ok(eval_absence_points(minutes, &pts, now))
        }
        Condition::AnomalyZscore {
            lookback_points,
            z,
            min_points,
        } => {
            let cap_usize = lookback_points.max(1).saturating_add(1).min(5_000);
            let limit = u32::try_from(cap_usize).unwrap_or(5_000);
            let pts = repo
                .query_raw_metric_points(tenant_id, node.id, metric, None, limit)
                .await?;
            Ok(eval_anomaly_zscore_points(
                lookback_points,
                z,
                min_points,
                &pts,
            ))
        }
        Condition::Expr { ref expression } => {
            let pts = repo
                .query_raw_metric_points(tenant_id, node.id, metric, None, 1)
                .await?;
            let latest_raw = pts.first().map(|(_, v)| *v);
            eval_expr_condition(expression, node, metric, latest_raw)
        }
        Condition::QualityFailed { .. } | Condition::EventMatch {} => Err(AppError::bad(
            "INVALID_ARGUMENT",
            "QUALITY_FAILED and EVENT_MATCH are not valid for METRIC scope",
        )),
    }
}
