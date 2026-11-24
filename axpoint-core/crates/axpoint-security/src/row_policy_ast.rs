//! `EBNF` predicate parser, placeholder expander (`{{user.*}}`, `{{user.roles}}`), and boolean
//! expression compiler (`11-STEP13` §13.6.2, `05-STEP27` §27.3).

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::guard::{GuardPrincipalProfile, PolicyTargetKind};
use sqlparser::ast::{BinaryOperator, Expr, InTarget, Statement, UnaryOperator, Value};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::row_policy::CompiledPredicate;
pub(crate) use crate::row_policy_state::compile_ttl_clause;
use crate::row_policy_state::{
    compile_scalar_operand, compile_string_literal_only, quote_sql_string,
};

const PLACEHOLDER_USER_ID: &str = "__ax_ph_user_id__";
const PLACEHOLDER_USER_NAME: &str = "__ax_ph_user_username__";
const PLACEHOLDER_USER_DEPT: &str = "__ax_ph_user_dept__";
const PLACEHOLDER_USER_ROLES: &str = "__ax_ph_user_roles__";
const PLACEHOLDER_ATTR_PREFIX: &str = "__ax_ph_user_attr_";
const PLACEHOLDER_SUFFIX: &str = "__";

#[derive(Clone, Copy)]
pub(crate) enum PlaceholderMode<'a> {
    SyntaxValidationOnly,
    BindProfile(&'a GuardPrincipalProfile),
}

pub(crate) enum PlaceholderKind {
    UserId,
    Username,
    Dept,
    Roles,
    Attr(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarContext {
    General,
    NumericCompare,
}

pub(crate) fn invalid_pred(msg: impl Into<String>) -> AppError {
    AppError::bad("INVALID_PREDICATE", msg.into())
}

pub(crate) fn parse_predicate_expr(predicate: &str) -> Result<Expr, AppError> {
    let trimmed = predicate.trim();
    if trimmed.is_empty() {
        return Err(invalid_pred("row policy predicate cannot be empty"));
    }
    if trimmed.len() > 8_192 {
        return Err(invalid_pred("row policy predicate exceeds 8 KiB"));
    }
    let preprocessed = preprocess_placeholders_and_scan_hazards(trimmed)?;
    let wrapper_sql = format!("SELECT 1 WHERE {preprocessed}");
    let stmts = Parser::parse_sql(&GenericDialect {}, &wrapper_sql)
        .map_err(|err| invalid_pred(format!("predicate syntax error: {err}")))?;
    let [Statement::Query(query)] = stmts.as_slice() else {
        return Err(invalid_pred(
            "predicate must parse as a single WHERE expression",
        ));
    };
    let sqlparser::ast::SetExpr::Select(select) = query.body.as_ref() else {
        return Err(invalid_pred("invalid predicate SELECT wrapper"));
    };
    select
        .selection
        .clone()
        .ok_or_else(|| invalid_pred("missing predicate expression"))
}

fn scan_quoted_literal(
    chars: &[char],
    start_idx: usize,
    quote: char,
    out: &mut String,
) -> Result<usize, AppError> {
    out.push(quote);
    let mut idx = start_idx.saturating_add(1);
    while let Some(&c) = chars.get(idx) {
        out.push(c);
        idx = idx.saturating_add(1);
        if c == quote {
            if chars.get(idx) == Some(&quote) {
                out.push(quote);
                idx = idx.saturating_add(1);
            } else {
                return Ok(idx);
            }
        }
    }
    if quote == '\'' {
        Err(invalid_pred("unclosed single-quoted string literal"))
    } else {
        Err(invalid_pred("unclosed double-quoted identifier"))
    }
}

fn preprocess_placeholders_and_scan_hazards(raw: &str) -> Result<String, AppError> {
    if raw.contains('\0') {
        return Err(invalid_pred("NUL byte in predicate is forbidden"));
    }
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len().saturating_add(32));
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if ch == '\'' || ch == '"' {
            idx = scan_quoted_literal(&chars, idx, ch, &mut out)?;
            continue;
        }
        if ch == ';' {
            return Err(invalid_pred("semicolons are forbidden in row predicates"));
        }
        if ch == '-' && chars.get(idx.saturating_add(1)) == Some(&'-') {
            return Err(invalid_pred(
                "SQL line comments are forbidden in row predicates",
            ));
        }
        if ch == '/' && chars.get(idx.saturating_add(1)) == Some(&'*') {
            return Err(invalid_pred(
                "SQL block comments are forbidden in row predicates",
            ));
        }
        if ch == '=' && chars.get(idx.saturating_add(1)) == Some(&'=') {
            if chars.get(idx.saturating_add(2)) == Some(&'=') {
                return Err(invalid_pred(
                    "operator '===' is forbidden in row predicates",
                ));
            }
            out.push('=');
            idx = idx.saturating_add(2);
            continue;
        }
        if ch == '{' && chars.get(idx.saturating_add(1)) == Some(&'{') {
            let (token_ident, next_idx) = extract_placeholder(&chars, idx.saturating_add(2))?;
            if previous_upper_keyword(&out) == "IN" {
                out.push('(');
                out.push_str(&token_ident);
                out.push(')');
            } else {
                out.push_str(&token_ident);
            }
            idx = next_idx;
            continue;
        }
        if ch == '}' && chars.get(idx.saturating_add(1)) == Some(&'}') {
            return Err(invalid_pred("unmatched '}}' in row predicate"));
        }
        out.push(ch);
        idx = idx.saturating_add(1);
    }

    Ok(out)
}

fn previous_upper_keyword(prefix: &str) -> String {
    let trimmed = prefix.trim_end();
    let word: String = trimmed
        .chars()
        .rev()
        .take_while(char::is_ascii_alphabetic)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    word.to_ascii_uppercase()
}

fn extract_placeholder(chars: &[char], start: usize) -> Result<(String, usize), AppError> {
    let mut cursor = start;
    let mut body = String::new();
    while let Some(&c) = chars.get(cursor) {
        if c == '}' && chars.get(cursor.saturating_add(1)) == Some(&'}') {
            let token = validate_and_encode_placeholder(body.trim())?;
            return Ok((token, cursor.saturating_add(2)));
        }
        body.push(c);
        cursor = cursor.saturating_add(1);
    }
    Err(invalid_pred("unclosed '{{' placeholder in row predicate"))
}

fn validate_and_encode_placeholder(spec: &str) -> Result<String, AppError> {
    match spec {
        "user.id" => Ok(String::from(PLACEHOLDER_USER_ID)),
        "user.username" => Ok(String::from(PLACEHOLDER_USER_NAME)),
        "user.dept" | "user.dept_id" => Ok(String::from(PLACEHOLDER_USER_DEPT)),
        "user.roles" => Ok(String::from(PLACEHOLDER_USER_ROLES)),
        other => {
            let Some(attr_key) = other.strip_prefix("user.attr.") else {
                return Err(invalid_pred(format!(
                    "unknown placeholder '{{{{{other}}}}}'"
                )));
            };
            if attr_key.is_empty()
                || !attr_key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(invalid_pred(format!(
                    "invalid user attribute placeholder '{{{{{other}}}}}'"
                )));
            }
            Ok(format!(
                "{PLACEHOLDER_ATTR_PREFIX}{attr_key}{PLACEHOLDER_SUFFIX}"
            ))
        }
    }
}

pub(crate) fn decode_placeholder(ident: &str) -> Option<PlaceholderKind> {
    if ident == PLACEHOLDER_USER_ID {
        return Some(PlaceholderKind::UserId);
    }
    if ident == PLACEHOLDER_USER_NAME {
        return Some(PlaceholderKind::Username);
    }
    if ident == PLACEHOLDER_USER_DEPT {
        return Some(PlaceholderKind::Dept);
    }
    if ident == PLACEHOLDER_USER_ROLES {
        return Some(PlaceholderKind::Roles);
    }
    let rest = ident.strip_prefix(PLACEHOLDER_ATTR_PREFIX)?;
    let key = rest.strip_suffix(PLACEHOLDER_SUFFIX)?;
    (!key.is_empty()).then(|| PlaceholderKind::Attr(String::from(key)))
}

fn expr_is_numeric_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Value(v) => matches!(v.value, Value::Number(_, false)),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => matches!(
            inner.as_ref(),
            Expr::Value(v) if matches!(v.value, Value::Number(_, false))
        ),
        _ => false,
    }
}

pub(crate) fn compile_expr(
    expr: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    match expr {
        Expr::Nested(inner) => {
            let inner_compiled = compile_expr(inner, target_kind, allowed_columns, mode)?;
            Ok(CompiledPredicate {
                duckdb_sql: wrap_if_unparenthesized(inner_compiled.duckdb_sql),
                pg_sql: wrap_if_unparenthesized(inner_compiled.pg_sql),
            })
        }
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr: inner,
        } => {
            let sub = compile_expr(inner, target_kind, allowed_columns, mode)?;
            Ok(CompiledPredicate {
                duckdb_sql: format!("NOT {}", sub.duckdb_sql),
                pg_sql: format!("NOT {}", sub.pg_sql),
            })
        }
        Expr::BinaryOp { left, op, right } => {
            compile_binary_op(left, op, right, target_kind, allowed_columns, mode)
        }
        Expr::IsNull(inner) => {
            let opd = compile_scalar_operand(
                inner,
                target_kind,
                allowed_columns,
                mode,
                ScalarContext::General,
            )?;
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} IS NULL", opd.duckdb_sql),
                pg_sql: format!("{} IS NULL", opd.pg_sql),
            })
        }
        Expr::IsNotNull(inner) => {
            let opd = compile_scalar_operand(
                inner,
                target_kind,
                allowed_columns,
                mode,
                ScalarContext::General,
            )?;
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} IS NOT NULL", opd.duckdb_sql),
                pg_sql: format!("{} IS NOT NULL", opd.pg_sql),
            })
        }
        Expr::Between {
            expr: target,
            negated,
            low,
            high,
        } => compile_between(
            target,
            *negated,
            low,
            high,
            target_kind,
            allowed_columns,
            mode,
        ),
        Expr::Like {
            negated,
            any: false,
            expr: target,
            pattern,
            escape_char: None,
        } => {
            let lhs = compile_scalar_operand(
                target,
                target_kind,
                allowed_columns,
                mode,
                ScalarContext::General,
            )?;
            let rhs = compile_string_literal_only(pattern)?;
            let kw = if *negated { "NOT LIKE" } else { "LIKE" };
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} {kw} {rhs}", lhs.duckdb_sql),
                pg_sql: format!("{} {kw} {rhs}", lhs.pg_sql),
            })
        }
        Expr::In {
            expr: lhs_expr,
            target: InTarget::List(list),
            negated,
            ..
        } => compile_in_list(lhs_expr, list, *negated, target_kind, allowed_columns, mode),
        _ => Err(invalid_pred(
            "unsupported SQL construct in row predicate (functions, subqueries, and CASE are forbidden)",
        )),
    }
}

fn compile_binary_op(
    left: &Expr,
    op: &BinaryOperator,
    right: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    match op {
        BinaryOperator::And | BinaryOperator::Or => {
            let l = compile_expr(left, target_kind, allowed_columns, mode)?;
            let r = compile_expr(right, target_kind, allowed_columns, mode)?;
            let kw = if *op == BinaryOperator::And {
                "AND"
            } else {
                "OR"
            };
            Ok(CompiledPredicate {
                duckdb_sql: format!("({} {kw} {})", l.duckdb_sql, r.duckdb_sql),
                pg_sql: format!("({} {kw} {})", l.pg_sql, r.pg_sql),
            })
        }
        BinaryOperator::Eq
        | BinaryOperator::NotEq
        | BinaryOperator::Lt
        | BinaryOperator::LtEq
        | BinaryOperator::Gt
        | BinaryOperator::GtEq => {
            let left_ctx = if expr_is_numeric_literal(right) {
                ScalarContext::NumericCompare
            } else {
                ScalarContext::General
            };
            let right_ctx = if expr_is_numeric_literal(left) {
                ScalarContext::NumericCompare
            } else {
                ScalarContext::General
            };
            let l = compile_scalar_operand(left, target_kind, allowed_columns, mode, left_ctx)?;
            let r = compile_scalar_operand(right, target_kind, allowed_columns, mode, right_ctx)?;
            let op_str = match op {
                BinaryOperator::Eq => "=",
                BinaryOperator::NotEq => "<>",
                BinaryOperator::Lt => "<",
                BinaryOperator::LtEq => "<=",
                BinaryOperator::Gt => ">",
                _ => ">=",
            };
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} {op_str} {}", l.duckdb_sql, r.duckdb_sql),
                pg_sql: format!("{} {op_str} {}", l.pg_sql, r.pg_sql),
            })
        }
        _ => Err(invalid_pred(format!(
            "operator '{op}' is forbidden in row predicates"
        ))),
    }
}

fn compile_between(
    target: &Expr,
    negated: bool,
    low: &Expr,
    high: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    let ctx = if expr_is_numeric_literal(low) || expr_is_numeric_literal(high) {
        ScalarContext::NumericCompare
    } else {
        ScalarContext::General
    };
    let lhs = compile_scalar_operand(target, target_kind, allowed_columns, mode, ctx)?;
    let lo = compile_scalar_operand(low, target_kind, allowed_columns, mode, ctx)?;
    let hi = compile_scalar_operand(high, target_kind, allowed_columns, mode, ctx)?;
    let kw = if negated { "NOT BETWEEN" } else { "BETWEEN" };
    Ok(CompiledPredicate {
        duckdb_sql: format!(
            "{} {kw} {} AND {}",
            lhs.duckdb_sql, lo.duckdb_sql, hi.duckdb_sql
        ),
        pg_sql: format!("{} {kw} {} AND {}", lhs.pg_sql, lo.pg_sql, hi.pg_sql),
    })
}

fn compile_in_lhs_for_list_placeholder(
    lhs_expr: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    if let Expr::Identifier(ident) = lhs_expr
        && ident.quote_style == Some('"')
    {
        let is_known_col =
            allowed_columns.is_some_and(|w| w.iter().any(|c| c.eq_ignore_ascii_case(&ident.value)));
        if !is_known_col {
            let lit = quote_sql_string(&ident.value);
            return Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            });
        }
    }
    compile_scalar_operand(
        lhs_expr,
        target_kind,
        allowed_columns,
        mode,
        ScalarContext::General,
    )
}

fn compile_in_list(
    lhs_expr: &Expr,
    list: &[Expr],
    negated: bool,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    if let [Expr::Identifier(single_ident)] = list
        && let Some(ph) = decode_placeholder(&single_ident.value)
    {
        match ph {
            PlaceholderKind::Attr(attr_key) => {
                let lhs = compile_in_lhs_for_list_placeholder(
                    lhs_expr,
                    target_kind,
                    allowed_columns,
                    mode,
                )?;
                return Ok(compile_in_string_slice(
                    &lhs,
                    negated,
                    mode,
                    "__validation__",
                    |p| p.attributes.get(&attr_key).map_or(&[][..], Vec::as_slice),
                ));
            }
            PlaceholderKind::Roles => {
                let lhs = compile_in_lhs_for_list_placeholder(
                    lhs_expr,
                    target_kind,
                    allowed_columns,
                    mode,
                )?;
                return Ok(compile_in_string_slice(
                    &lhs,
                    negated,
                    mode,
                    "__role__",
                    |p| p.roles.as_slice(),
                ));
            }
            PlaceholderKind::UserId | PlaceholderKind::Username | PlaceholderKind::Dept => {}
        }
    }

    let lhs = compile_scalar_operand(
        lhs_expr,
        target_kind,
        allowed_columns,
        mode,
        ScalarContext::General,
    )?;
    if list.is_empty() {
        let empty_bool_sql = if negated { "TRUE" } else { "FALSE" };
        return Ok(CompiledPredicate {
            duckdb_sql: String::from(empty_bool_sql),
            pg_sql: String::from(empty_bool_sql),
        });
    }

    let mut duck_items = Vec::with_capacity(list.len());
    let mut pg_items = Vec::with_capacity(list.len());
    for item in list {
        let compiled = compile_scalar_operand(
            item,
            target_kind,
            allowed_columns,
            mode,
            ScalarContext::General,
        )?;
        duck_items.push(compiled.duckdb_sql);
        pg_items.push(compiled.pg_sql);
    }
    let kw = if negated { "NOT IN" } else { "IN" };
    Ok(CompiledPredicate {
        duckdb_sql: format!("{} {kw} ({})", lhs.duckdb_sql, duck_items.join(", ")),
        pg_sql: format!("{} {kw} ({})", lhs.pg_sql, pg_items.join(", ")),
    })
}

fn compile_in_string_slice<'a, F>(
    lhs: &CompiledPredicate,
    negated: bool,
    mode: PlaceholderMode<'a>,
    validation_sentinel: &str,
    extract: F,
) -> CompiledPredicate
where
    F: FnOnce(&'a GuardPrincipalProfile) -> &'a [String],
{
    let kw = if negated { "NOT IN" } else { "IN" };
    match mode {
        PlaceholderMode::SyntaxValidationOnly => {
            let sentinel = quote_sql_string(validation_sentinel);
            CompiledPredicate {
                duckdb_sql: format!("{} {kw} ({sentinel})", lhs.duckdb_sql),
                pg_sql: format!("{} {kw} ({sentinel})", lhs.pg_sql),
            }
        }
        PlaceholderMode::BindProfile(profile) => {
            let vals = extract(profile);
            if vals.is_empty() {
                let constant = if negated { "TRUE" } else { "FALSE" };
                return CompiledPredicate {
                    duckdb_sql: String::from(constant),
                    pg_sql: String::from(constant),
                };
            }
            let quoted: Vec<String> = vals.iter().map(|v| quote_sql_string(v)).collect();
            let joined = quoted.join(", ");
            CompiledPredicate {
                duckdb_sql: format!("{} {kw} ({joined})", lhs.duckdb_sql),
                pg_sql: format!("{} {kw} ({joined})", lhs.pg_sql),
            }
        }
    }
}

fn wrap_if_unparenthesized(s: String) -> String {
    if s.starts_with('(') && s.ends_with(')') {
        s
    } else {
        format!("({s})")
    }
}
