//! Static type checking and reference extraction for `Expr` ASTs (`06-STEP05-06` §06.1.2–§06.1.3).

use std::collections::BTreeSet;

use super::{BinaryOp, Expr, ExprIssue, ExprType, ExprTypeEnv, MAX_CALL_ARGS, UnaryOp};

fn tc_err(message: impl Into<String>) -> ExprIssue {
    ExprIssue {
        code: String::from("EXPR_INVALID"),
        message: message.into(),
        column: None,
    }
}

const fn is_numeric(t: ExprType) -> bool {
    matches!(t, ExprType::Number | ExprType::Null)
}

const fn is_bool(t: ExprType) -> bool {
    matches!(t, ExprType::Bool | ExprType::Null)
}

const fn is_text(t: ExprType) -> bool {
    matches!(t, ExprType::Text | ExprType::Null)
}

const fn unify_types(lhs: ExprType, rhs: ExprType) -> Option<ExprType> {
    match (lhs, rhs) {
        (ExprType::Null, other) | (other, ExprType::Null) => Some(other),
        (a, b) => {
            if matches!(
                (a, b),
                (ExprType::Number, ExprType::Number)
                    | (ExprType::Text, ExprType::Text)
                    | (ExprType::Bool, ExprType::Bool)
            ) {
                Some(a)
            } else {
                None
            }
        }
    }
}

/// Type-checking result pairing the inferred [`ExprType`] with referenced identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedExprInfo {
    /// Inferred output [`ExprType`].
    pub result_type: ExprType,
    /// Metric names referenced via `<ident>`.
    pub referenced_metrics: BTreeSet<String>,
    /// Attribute names referenced via `prop.<attr>`.
    pub referenced_properties: BTreeSet<String>,
}

/// Extracts `(referenced_metrics, referenced_properties)` from `expr` without requiring a schema scope.
#[must_use]
pub fn collect_references(expr: &Expr) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut metrics = BTreeSet::new();
    let mut props = BTreeSet::new();
    walk_refs(expr, &mut metrics, &mut props);
    (metrics, props)
}

fn walk_refs(expr: &Expr, metrics: &mut BTreeSet<String>, props: &mut BTreeSet<String>) {
    match expr {
        Expr::Literal(_) => {}
        Expr::MetricRef(m) => {
            metrics.insert(m.clone());
        }
        Expr::PropRef(p) => {
            props.insert(p.clone());
        }
        Expr::Unary { expr: inner, .. } => walk_refs(inner, metrics, props),
        Expr::Binary { left, right, .. } => {
            walk_refs(left, metrics, props);
            walk_refs(right, metrics, props);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                walk_refs(arg, metrics, props);
            }
        }
    }
}

/// Statically type-checks `expr` against `env` and collects referenced metrics and properties.
///
/// # Errors
/// Returns [`ExprIssue`] (`EXPR_INVALID`) on unknown metrics/attributes, unreferenceable attribute
/// `DataType`s, operator type mismatches, or invalid function calls.
pub fn typecheck_detailed(expr: &Expr, env: &ExprTypeEnv) -> Result<CheckedExprInfo, ExprIssue> {
    let mut referenced_metrics = BTreeSet::new();
    let mut referenced_properties = BTreeSet::new();
    let result_type = check_node(
        expr,
        env,
        &mut referenced_metrics,
        &mut referenced_properties,
    )?;
    Ok(CheckedExprInfo {
        result_type,
        referenced_metrics,
        referenced_properties,
    })
}

fn check_node(
    expr: &Expr,
    env: &ExprTypeEnv,
    metrics: &mut BTreeSet<String>,
    props: &mut BTreeSet<String>,
) -> Result<ExprType, ExprIssue> {
    match expr {
        Expr::Literal(val) => Ok(val.expr_type()),
        Expr::MetricRef(name) => {
            if !env.metrics.contains(name) {
                return Err(tc_err(format!("unknown metric '{name}'")));
            }
            metrics.insert(name.clone());
            Ok(ExprType::Number)
        }
        Expr::PropRef(attr) => {
            let Some(mapped_opt) = env.properties.get(attr) else {
                let label = if attr.contains('.') {
                    attr.clone()
                } else {
                    format!("prop.{attr}")
                };
                return Err(tc_err(format!("unknown attribute '{label}'")));
            };
            let Some(prop_type) = *mapped_opt else {
                let label = if attr.contains('.') {
                    attr.clone()
                } else {
                    format!("prop.{attr}")
                };
                return Err(tc_err(format!(
                    "attribute '{label}' has a non-primitive type and cannot be referenced in expressions"
                )));
            };
            props.insert(attr.clone());
            Ok(prop_type)
        }
        Expr::Unary { op, expr: inner } => {
            let inner_t = check_node(inner, env, metrics, props)?;
            match op {
                UnaryOp::Neg => {
                    if !is_numeric(inner_t) {
                        return Err(tc_err(format!(
                            "unary '-' requires NUMBER, got {inner_t:?}"
                        )));
                    }
                    Ok(ExprType::Number)
                }
                UnaryOp::Not => {
                    if !is_bool(inner_t) {
                        return Err(tc_err(format!(
                            "unary 'not'/'!' requires BOOL, got {inner_t:?}"
                        )));
                    }
                    Ok(ExprType::Bool)
                }
            }
        }
        Expr::Binary { op, left, right } => {
            let lt = check_node(left, env, metrics, props)?;
            let rt = check_node(right, env, metrics, props)?;
            check_binary(*op, lt, rt)
        }
        Expr::Call { name, args } => check_call(name, args, env, metrics, props),
    }
}

fn check_binary(op: BinaryOp, lt: ExprType, rt: ExprType) -> Result<ExprType, ExprIssue> {
    match op {
        BinaryOp::Or | BinaryOp::And => {
            if !is_bool(lt) || !is_bool(rt) {
                return Err(tc_err(format!(
                    "logical operator '{}' requires BOOL operands, got ({lt:?}, {rt:?})",
                    op.symbol()
                )));
            }
            Ok(ExprType::Bool)
        }
        BinaryOp::Add
        | BinaryOp::Sub
        | BinaryOp::Mul
        | BinaryOp::Div
        | BinaryOp::Mod
        | BinaryOp::Pow => {
            if !is_numeric(lt) || !is_numeric(rt) {
                return Err(tc_err(format!(
                    "arithmetic operator '{}' requires NUMBER operands, got ({lt:?}, {rt:?})",
                    op.symbol()
                )));
            }
            Ok(ExprType::Number)
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            if unify_types(lt, rt).is_none() {
                return Err(tc_err(format!(
                    "cannot compare incompatible types {lt:?} and {rt:?}"
                )));
            }
            Ok(ExprType::Bool)
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            let Some(unified) = unify_types(lt, rt) else {
                return Err(tc_err(format!(
                    "cannot compare incompatible types {lt:?} and {rt:?}"
                )));
            };
            if unified == ExprType::Bool {
                return Err(tc_err("ordering comparison is not supported on BOOL"));
            }
            Ok(ExprType::Bool)
        }
    }
}

fn check_call(
    name: &str,
    args: &[Expr],
    env: &ExprTypeEnv,
    metrics: &mut BTreeSet<String>,
    props: &mut BTreeSet<String>,
) -> Result<ExprType, ExprIssue> {
    if args.len() > MAX_CALL_ARGS {
        return Err(tc_err(format!(
            "function '{name}' exceeds maximum {MAX_CALL_ARGS} arguments"
        )));
    }
    let mut arg_types = Vec::with_capacity(args.len());
    for arg in args {
        arg_types.push(check_node(arg, env, metrics, props)?);
    }

    match name {
        "abs" | "sqrt" | "exp" | "ln" | "log10" | "floor" | "ceil" => {
            require_numeric_args(name, &arg_types, 1, 1)
        }
        "round" => require_numeric_args(name, &arg_types, 1, 2),
        "pow" => require_numeric_args(name, &arg_types, 2, 2),
        "min" | "max" => require_numeric_args(name, &arg_types, 2, MAX_CALL_ARGS),
        "clamp" => require_numeric_args(name, &arg_types, 3, 3),
        "safe_div" => require_numeric_args(name, &arg_types, 2, 3),
        "if" => {
            if arg_types.len() != 3 {
                return Err(tc_err(format!(
                    "'if' expects 3 arguments, got {}",
                    arg_types.len()
                )));
            }
            let cond = arg_types.first().copied().unwrap_or(ExprType::Null);
            let then_t = arg_types.get(1).copied().unwrap_or(ExprType::Null);
            let else_t = arg_types.get(2).copied().unwrap_or(ExprType::Null);
            if !is_bool(cond) {
                return Err(tc_err(format!("'if' condition must be BOOL, got {cond:?}")));
            }
            unify_types(then_t, else_t).ok_or_else(|| {
                tc_err(format!(
                    "'if' branches have incompatible types {then_t:?} and {else_t:?}"
                ))
            })
        }
        "coalesce" => {
            if arg_types.is_empty() {
                return Err(tc_err("'coalesce' expects at least 1 argument"));
            }
            let mut acc = ExprType::Null;
            for &t in &arg_types {
                acc = unify_types(acc, t).ok_or_else(|| {
                    tc_err(format!(
                        "'coalesce' arguments have incompatible types ({acc:?} vs {t:?})"
                    ))
                })?;
            }
            Ok(acc)
        }
        "is_null" => {
            if arg_types.len() != 1 {
                return Err(tc_err(format!(
                    "'is_null' expects 1 argument, got {}",
                    arg_types.len()
                )));
            }
            Ok(ExprType::Bool)
        }
        "contains" | "starts_with" => {
            if arg_types.len() != 2 {
                return Err(tc_err(format!(
                    "'{name}' expects 2 arguments, got {}",
                    arg_types.len()
                )));
            }
            if !arg_types.iter().all(|&t| is_text(t)) {
                return Err(tc_err(format!("'{name}' requires TEXT arguments")));
            }
            Ok(ExprType::Bool)
        }
        "lower" | "upper" => {
            if arg_types.len() != 1 {
                return Err(tc_err(format!(
                    "'{name}' expects 1 argument, got {}",
                    arg_types.len()
                )));
            }
            if !arg_types.iter().all(|&t| is_text(t)) {
                return Err(tc_err(format!("'{name}' requires a TEXT argument")));
            }
            Ok(ExprType::Text)
        }
        other => Err(tc_err(format!("unknown function '{other}'"))),
    }
}

fn require_numeric_args(
    name: &str,
    arg_types: &[ExprType],
    min_len: usize,
    max_len: usize,
) -> Result<ExprType, ExprIssue> {
    if arg_types.len() < min_len || arg_types.len() > max_len {
        return Err(tc_err(format!(
            "'{name}' expects {min_len}..={max_len} arguments, got {}",
            arg_types.len()
        )));
    }
    for &t in arg_types {
        if !is_numeric(t) {
            return Err(tc_err(format!(
                "'{name}' requires NUMBER arguments, got {t:?}"
            )));
        }
    }
    Ok(ExprType::Number)
}
