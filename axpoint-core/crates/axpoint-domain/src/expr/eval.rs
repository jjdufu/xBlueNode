//! Deterministic, side-effect-free expression evaluator (`06-STEP05-06` §06.1.2–§06.1.3).

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::num::FpCategory;

use super::{BinaryOp, EvalEnv, EvalReport, EvalWarningCode, Expr, ExprValue, UnaryOp};

const fn is_zero(v: f64) -> bool {
    matches!(v.classify(), FpCategory::Zero)
}

const fn normalize_zero(v: f64) -> f64 {
    if is_zero(v) { 0.0 } else { v }
}

fn finish_number(v: f64, warnings: &mut BTreeSet<EvalWarningCode>) -> ExprValue {
    if v.is_finite() {
        ExprValue::Number(normalize_zero(v))
    } else {
        warnings.insert(EvalWarningCode::DomainError);
        ExprValue::Null
    }
}

/// Evaluates `expr` against `env`, returning an [`EvalReport`] with deduplicated [`EvalWarningCode`]s.
///
/// Guaranteed never to panic or return non-finite `NaN` / `±Infinity` numbers.
#[must_use]
pub fn eval_ast(expr: &Expr, env: &EvalEnv) -> EvalReport {
    let mut warnings = BTreeSet::new();
    let value = eval_node(expr, env, &mut warnings);
    EvalReport {
        value,
        warnings: warnings.into_iter().collect(),
    }
}

fn eval_node(expr: &Expr, env: &EvalEnv, warnings: &mut BTreeSet<EvalWarningCode>) -> ExprValue {
    match expr {
        Expr::Literal(val) => match val {
            ExprValue::Number(v) => finish_number(*v, warnings),
            other => other.clone(),
        },
        Expr::MetricRef(name) => env
            .metrics
            .get(name)
            .copied()
            .filter(|v| v.is_finite())
            .map_or(ExprValue::Null, |v| ExprValue::Number(normalize_zero(v))),
        Expr::PropRef(name) => env.properties.get(name).cloned().unwrap_or(ExprValue::Null),
        Expr::Unary { op, expr: inner } => {
            let val = eval_node(inner, env, warnings);
            match op {
                UnaryOp::Neg => match val {
                    ExprValue::Null => ExprValue::Null,
                    ExprValue::Number(v) => finish_number(-v, warnings),
                    _ => {
                        warnings.insert(EvalWarningCode::DomainError);
                        ExprValue::Null
                    }
                },
                UnaryOp::Not => match val {
                    ExprValue::Null => ExprValue::Null,
                    ExprValue::Bool(b) => ExprValue::Bool(!b),
                    _ => {
                        warnings.insert(EvalWarningCode::DomainError);
                        ExprValue::Null
                    }
                },
            }
        }
        Expr::Binary { op, left, right } => eval_binary(*op, left, right, env, warnings),
        Expr::Call { name, args } => eval_call(name, args, env, warnings),
    }
}

fn eval_binary(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    match op {
        BinaryOp::And => eval_three_valued_and(left, right, env, warnings),
        BinaryOp::Or => eval_three_valued_or(left, right, env, warnings),
        BinaryOp::Add
        | BinaryOp::Sub
        | BinaryOp::Mul
        | BinaryOp::Div
        | BinaryOp::Mod
        | BinaryOp::Pow => {
            let lv = eval_node(left, env, warnings);
            let rv = eval_node(right, env, warnings);
            eval_arith(op, lv, rv, warnings)
        }
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            let lv = eval_node(left, env, warnings);
            let rv = eval_node(right, env, warnings);
            eval_compare(op, &lv, &rv, warnings)
        }
    }
}

fn eval_three_valued_and(
    left: &Expr,
    right: &Expr,
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    match eval_node(left, env, warnings) {
        ExprValue::Bool(false) => ExprValue::Bool(false),
        ExprValue::Bool(true) => match eval_node(right, env, warnings) {
            ExprValue::Bool(b) => ExprValue::Bool(b),
            ExprValue::Null => ExprValue::Null,
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            }
        },
        ExprValue::Null => match eval_node(right, env, warnings) {
            ExprValue::Bool(false) => ExprValue::Bool(false),
            ExprValue::Bool(true) | ExprValue::Null => ExprValue::Null,
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            }
        },
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn eval_three_valued_or(
    left: &Expr,
    right: &Expr,
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    match eval_node(left, env, warnings) {
        ExprValue::Bool(true) => ExprValue::Bool(true),
        ExprValue::Bool(false) => match eval_node(right, env, warnings) {
            ExprValue::Bool(b) => ExprValue::Bool(b),
            ExprValue::Null => ExprValue::Null,
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            }
        },
        ExprValue::Null => match eval_node(right, env, warnings) {
            ExprValue::Bool(true) => ExprValue::Bool(true),
            ExprValue::Bool(false) | ExprValue::Null => ExprValue::Null,
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            }
        },
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn eval_arith(
    op: BinaryOp,
    lv: ExprValue,
    rv: ExprValue,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    if matches!(lv, ExprValue::Null) || matches!(rv, ExprValue::Null) {
        return ExprValue::Null;
    }
    let (ExprValue::Number(a), ExprValue::Number(b)) = (lv, rv) else {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    };

    match op {
        BinaryOp::Add => finish_number(a + b, warnings),
        BinaryOp::Sub => finish_number(a - b, warnings),
        BinaryOp::Mul => finish_number(a * b, warnings),
        BinaryOp::Div => {
            if is_zero(b) {
                warnings.insert(EvalWarningCode::DivByZero);
                ExprValue::Null
            } else {
                finish_number(a / b, warnings)
            }
        }
        BinaryOp::Mod => {
            if is_zero(b) {
                warnings.insert(EvalWarningCode::DivByZero);
                ExprValue::Null
            } else {
                finish_number(a % b, warnings)
            }
        }
        BinaryOp::Pow => finish_number(a.powf(b), warnings),
        _ => ExprValue::Null,
    }
}

fn eval_compare(
    op: BinaryOp,
    lv: &ExprValue,
    rv: &ExprValue,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    if matches!(lv, ExprValue::Null) || matches!(rv, ExprValue::Null) {
        return ExprValue::Null;
    }
    match (lv, rv) {
        (ExprValue::Number(a), ExprValue::Number(b)) => {
            let res = match op {
                BinaryOp::Eq => a.partial_cmp(b) == Some(Ordering::Equal),
                BinaryOp::Ne => a.partial_cmp(b) != Some(Ordering::Equal),
                BinaryOp::Lt => a < b,
                BinaryOp::Le => a <= b,
                BinaryOp::Gt => a > b,
                BinaryOp::Ge => a >= b,
                _ => return ExprValue::Null,
            };
            ExprValue::Bool(res)
        }
        (ExprValue::Text(a), ExprValue::Text(b)) => {
            let res = match op {
                BinaryOp::Eq => a == b,
                BinaryOp::Ne => a != b,
                BinaryOp::Lt => a < b,
                BinaryOp::Le => a <= b,
                BinaryOp::Gt => a > b,
                BinaryOp::Ge => a >= b,
                _ => return ExprValue::Null,
            };
            ExprValue::Bool(res)
        }
        (ExprValue::Bool(a), ExprValue::Bool(b)) => match op {
            BinaryOp::Eq => ExprValue::Bool(a == b),
            BinaryOp::Ne => ExprValue::Bool(a != b),
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            }
        },
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn eval_call(
    name: &str,
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    match name {
        "if" => eval_if(args, env, warnings),
        "coalesce" => eval_coalesce(args, env, warnings),
        "is_null" => {
            let Some(first) = args.first() else {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            };
            let v = eval_node(first, env, warnings);
            ExprValue::Bool(matches!(v, ExprValue::Null))
        }
        "safe_div" => eval_safe_div(args, env, warnings),
        "min" => eval_min_max(args, env, warnings, true),
        "max" => eval_min_max(args, env, warnings, false),
        "contains" | "starts_with" | "lower" | "upper" => eval_text_fn(name, args, env, warnings),
        _ => eval_numeric_fn(name, args, env, warnings),
    }
}

fn eval_if(
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    let (Some(cond_expr), Some(then_expr), Some(else_expr)) =
        (args.first(), args.get(1), args.get(2))
    else {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    };
    if args.len() != 3 {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    }

    match eval_node(cond_expr, env, warnings) {
        ExprValue::Bool(true) => eval_node(then_expr, env, warnings),
        ExprValue::Bool(false) | ExprValue::Null => eval_node(else_expr, env, warnings),
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn eval_coalesce(
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    for arg in args {
        let val = eval_node(arg, env, warnings);
        if !matches!(val, ExprValue::Null) {
            return val;
        }
    }
    ExprValue::Null
}

fn eval_safe_div(
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    let (Some(num_expr), Some(den_expr)) = (args.first(), args.get(1)) else {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    };
    if args.len() > 3 {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    }

    let num_val = eval_node(num_expr, env, warnings);
    let den_val = eval_node(den_expr, env, warnings);

    match den_val {
        ExprValue::Number(d) if is_zero(d) => {
            if let Some(fallback_expr) = args.get(2) {
                eval_node(fallback_expr, env, warnings)
            } else {
                ExprValue::Null
            }
        }
        ExprValue::Null => ExprValue::Null,
        ExprValue::Number(d) => match num_val {
            ExprValue::Null => ExprValue::Null,
            ExprValue::Number(n) => finish_number(n / d, warnings),
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            }
        },
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn eval_min_max(
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
    is_min: bool,
) -> ExprValue {
    if args.len() < 2 {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    }
    let mut acc: Option<f64> = None;
    for arg in args {
        match eval_node(arg, env, warnings) {
            ExprValue::Null => {}
            ExprValue::Number(v) => {
                acc = Some(match acc {
                    None => v,
                    Some(cur) => {
                        if is_min {
                            cur.min(v)
                        } else {
                            cur.max(v)
                        }
                    }
                });
            }
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            }
        }
    }
    acc.map_or(ExprValue::Null, |v| ExprValue::Number(normalize_zero(v)))
}

fn eval_text_fn(
    name: &str,
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    match name {
        "lower" | "upper" => {
            let Some(first) = args.first() else {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            };
            if args.len() != 1 {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            }
            match eval_node(first, env, warnings) {
                ExprValue::Null => ExprValue::Null,
                ExprValue::Text(s) => {
                    if name == "lower" {
                        ExprValue::Text(s.to_lowercase())
                    } else {
                        ExprValue::Text(s.to_uppercase())
                    }
                }
                _ => {
                    warnings.insert(EvalWarningCode::DomainError);
                    ExprValue::Null
                }
            }
        }
        "contains" | "starts_with" => {
            let (Some(a0), Some(a1)) = (args.first(), args.get(1)) else {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            };
            if args.len() != 2 {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            }
            let v0 = eval_node(a0, env, warnings);
            let v1 = eval_node(a1, env, warnings);
            if matches!(v0, ExprValue::Null) || matches!(v1, ExprValue::Null) {
                return ExprValue::Null;
            }
            let (ExprValue::Text(haystack), ExprValue::Text(needle)) = (v0, v1) else {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            };
            if name == "contains" {
                ExprValue::Bool(haystack.contains(needle.as_str()))
            } else {
                ExprValue::Bool(haystack.starts_with(needle.as_str()))
            }
        }
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn eval_numeric_fn(
    name: &str,
    args: &[Expr],
    env: &EvalEnv,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    let mut nums = Vec::with_capacity(args.len());
    for arg in args {
        match eval_node(arg, env, warnings) {
            ExprValue::Null => return ExprValue::Null,
            ExprValue::Number(v) => nums.push(v),
            _ => {
                warnings.insert(EvalWarningCode::DomainError);
                return ExprValue::Null;
            }
        }
    }

    match (name, nums.as_slice()) {
        ("abs", &[x]) => finish_number(x.abs(), warnings),
        ("floor", &[x]) => finish_number(x.floor(), warnings),
        ("ceil", &[x]) => finish_number(x.ceil(), warnings),
        ("exp", &[x]) => finish_number(x.exp(), warnings),
        ("sqrt", &[x]) => {
            if x < 0.0 {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            } else {
                finish_number(x.sqrt(), warnings)
            }
        }
        ("ln", &[x]) => {
            if x <= 0.0 {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            } else {
                finish_number(x.ln(), warnings)
            }
        }
        ("log10", &[x]) => {
            if x <= 0.0 {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            } else {
                finish_number(x.log10(), warnings)
            }
        }
        ("round", &[x]) => finish_number(x.round(), warnings),
        ("round", &[x, digits]) => round_half_away_from_zero(x, digits, warnings),
        ("pow", &[x, y]) => finish_number(x.powf(y), warnings),
        ("clamp", &[x, lo, hi]) => {
            if lo > hi {
                warnings.insert(EvalWarningCode::DomainError);
                ExprValue::Null
            } else if x < lo {
                finish_number(lo, warnings)
            } else if x > hi {
                finish_number(hi, warnings)
            } else {
                finish_number(x, warnings)
            }
        }
        _ => {
            warnings.insert(EvalWarningCode::DomainError);
            ExprValue::Null
        }
    }
}

fn round_half_away_from_zero(
    x: f64,
    digits: f64,
    warnings: &mut BTreeSet<EvalWarningCode>,
) -> ExprValue {
    let d_trunc = digits.trunc();
    if is_zero(d_trunc) {
        return finish_number(x.round(), warnings);
    }
    if d_trunc > 300.0 {
        return finish_number(x, warnings);
    }
    if d_trunc < -308.0 {
        return ExprValue::Number(0.0);
    }
    let Ok(exp) = format!("{d_trunc:.0}").parse::<i32>() else {
        warnings.insert(EvalWarningCode::DomainError);
        return ExprValue::Null;
    };
    if exp >= 0 {
        let factor = 10_f64.powi(exp);
        let scaled = x * factor;
        if !scaled.is_finite() {
            return finish_number(x, warnings);
        }
        finish_number(scaled.round() / factor, warnings)
    } else {
        let factor = 10_f64.powi(exp.saturating_neg());
        if !factor.is_finite() || is_zero(factor) {
            return ExprValue::Number(0.0);
        }
        finish_number((x / factor).round() * factor, warnings)
    }
}
