//! Scalar operand, state-field (`properties.*`, `metrics.*`, `node.prop.*`, `node.metric.*`),
//! and TTL active-window dual-dialect SQL compilation (`05-STEP27` §27.1, §27.3).

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::guard::PolicyTargetKind;
use sqlparser::ast::{Expr, Ident, UnaryOperator, Value};

use crate::row_policy::CompiledPredicate;
use crate::row_policy_ast::{
    PlaceholderKind, PlaceholderMode, ScalarContext, decode_placeholder, invalid_pred,
};

const MAX_TTL_HOURS: i32 = 87_600;

/// Anchored IEEE-754 decimal/scientific literal pattern guarding PostgreSQL `::double precision` casts.
const SAFE_NUM_REGEX: &str = r"^-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$";

/// Anchored calendar-valid ISO-8601 timestamp pattern guarding PostgreSQL `::timestamptz` casts.
const SAFE_ISO_TS_REGEX: &str = concat!(
    r"^(",
    r"[0-9]{4}-((0[13578]|1[02])-(0[1-9]|[12][0-9]|3[01])|(0[469]|11)-(0[1-9]|[12][0-9]|30)|02-(0[1-9]|1[0-9]|2[0-8]))",
    r"|(([0-9]{2}(0[48]|[2468][048]|[13579][26])|([02468][048]|[13579][26])00)-02-29)",
    r")",
    r"([T ]([01][0-9]|2[0-3]):[0-5][0-9](:[0-5][0-9](\.[0-9]{1,9})?)?(Z|[+-](0[0-9]|1[0-4])(:?[0-5][0-9])?)?)?$"
);

pub(crate) fn compile_scalar_operand(
    expr: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
    ctx: ScalarContext,
) -> Result<CompiledPredicate, AppError> {
    match expr {
        Expr::Identifier(ident) => {
            if ident.quote_style.is_none()
                && let Some(ph) = decode_placeholder(&ident.value)
            {
                return compile_scalar_placeholder(&ph, mode);
            }
            compile_column_ident(&ident.value, target_kind, allowed_columns, ctx)
        }
        Expr::CompoundIdentifier(idents) => {
            compile_compound_ident(idents, target_kind, allowed_columns, ctx)
        }
        Expr::Value(val_span) => compile_literal_value(&val_span.value),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => {
            if let Expr::Value(val_span) = inner.as_ref()
                && let Value::Number(num_str, false) = &val_span.value
            {
                if !num_str.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    return Err(invalid_pred(format!(
                        "invalid numeric literal '-{num_str}'"
                    )));
                }
                let lit = format!("-{num_str}");
                return Ok(CompiledPredicate {
                    duckdb_sql: lit.clone(),
                    pg_sql: lit,
                });
            }
            Err(invalid_pred(
                "unary minus is only permitted on numeric literals",
            ))
        }
        _ => Err(invalid_pred(
            "invalid operand in row predicate (functions and arbitrary expressions are forbidden)",
        )),
    }
}

fn compile_scalar_placeholder(
    ph: &PlaceholderKind,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    match ph {
        PlaceholderKind::Attr(key) => Err(invalid_pred(format!(
            "list placeholder '{{{{user.attr.{key}}}}}' is only allowed on the right-hand side of IN"
        ))),
        PlaceholderKind::Roles => Err(invalid_pred(
            "list placeholder '{{user.roles}}' is only allowed on the right-hand side of IN",
        )),
        PlaceholderKind::UserId => {
            let raw = match mode {
                PlaceholderMode::SyntaxValidationOnly => {
                    String::from("00000000-0000-0000-0000-000000000000")
                }
                PlaceholderMode::BindProfile(p) => p.user_id.to_string(),
            };
            let lit = quote_sql_string(&raw);
            Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            })
        }
        PlaceholderKind::Username => {
            let raw = match mode {
                PlaceholderMode::SyntaxValidationOnly => "__user__",
                PlaceholderMode::BindProfile(p) => p.username.as_str(),
            };
            let lit = quote_sql_string(raw);
            Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            })
        }
        PlaceholderKind::Dept => {
            let raw = match mode {
                PlaceholderMode::SyntaxValidationOnly => "default",
                PlaceholderMode::BindProfile(p) => p.dept_id.as_str(),
            };
            let lit = quote_sql_string(raw);
            Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            })
        }
    }
}

fn ensure_valid_ident_chars(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(invalid_pred(format!(
            "invalid column identifier '{trimmed}'"
        )));
    }
    Ok(trimmed)
}

fn check_whitelist(
    field: &str,
    is_core: bool,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<(), AppError> {
    if let Some(whitelist) = allowed_columns
        && !is_core
        && !whitelist.iter().any(|c| c.eq_ignore_ascii_case(field))
    {
        return Err(invalid_pred(format!(
            "column '{field}' does not exist on target resource"
        )));
    }
    Ok(())
}

fn compile_node_property_access(field: &str, ctx: ScalarContext) -> CompiledPredicate {
    let escaped = field.replace('\'', "''");
    let escaped_re = SAFE_NUM_REGEX.replace('\'', "''");
    match ctx {
        ScalarContext::General => CompiledPredicate {
            duckdb_sql: format!("json_extract_string(properties, '$.{escaped}')"),
            pg_sql: format!("(properties->>'{escaped}')"),
        },
        ScalarContext::NumericCompare => CompiledPredicate {
            duckdb_sql: format!(
                "TRY_CAST(json_extract_string(properties, '$.{escaped}') AS DOUBLE)"
            ),
            pg_sql: format!(
                "(CASE WHEN (properties->>'{escaped}') ~ '{escaped_re}' THEN (properties->>'{escaped}')::double precision END)"
            ),
        },
    }
}

fn compile_node_metric_access(field: &str) -> CompiledPredicate {
    let escaped = field.replace('\'', "''");
    let escaped_re = SAFE_NUM_REGEX.replace('\'', "''");
    CompiledPredicate {
        duckdb_sql: format!("TRY_CAST(json_extract_string(metrics, '$.{escaped}') AS DOUBLE)"),
        pg_sql: format!(
            "(CASE WHEN (metrics->>'{escaped}') ~ '{escaped_re}' THEN (metrics->>'{escaped}')::double precision END)"
        ),
    }
}

fn compile_compound_ident(
    idents: &[Ident],
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    ctx: ScalarContext,
) -> Result<CompiledPredicate, AppError> {
    if target_kind != PolicyTargetKind::NodeType {
        return Err(invalid_pred(
            "table-qualified columns are forbidden in dataset row predicates",
        ));
    }
    let parts: Vec<&str> = idents.iter().map(|i| i.value.trim()).collect();
    match parts.as_slice() {
        [ns, col] => {
            let ns_lower = ns.to_ascii_lowercase();
            let clean_col = ensure_valid_ident_chars(col)?;
            match ns_lower.as_str() {
                "properties" | "prop" => {
                    check_whitelist(clean_col, false, allowed_columns)?;
                    Ok(compile_node_property_access(clean_col, ctx))
                }
                "metrics" | "metric" => {
                    check_whitelist(clean_col, false, allowed_columns)?;
                    Ok(compile_node_metric_access(clean_col))
                }
                "node" => {
                    let lower = clean_col.to_ascii_lowercase();
                    if matches!(
                        lower.as_str(),
                        "node_key" | "title" | "type_name" | "updated_at"
                    ) {
                        Ok(CompiledPredicate {
                            duckdb_sql: format!("\"{lower}\""),
                            pg_sql: lower,
                        })
                    } else {
                        Err(invalid_pred(format!(
                            "unknown node core field 'node.{clean_col}'; use node.prop.<k> or node.metric.<k>"
                        )))
                    }
                }
                _ => Err(invalid_pred(format!(
                    "unsupported qualifier '{ns}' in node row predicate; use properties.<k> or metrics.<k>"
                ))),
            }
        }
        [root, ns, col] if root.eq_ignore_ascii_case("node") => {
            let ns_lower = ns.to_ascii_lowercase();
            let clean_col = ensure_valid_ident_chars(col)?;
            check_whitelist(clean_col, false, allowed_columns)?;
            match ns_lower.as_str() {
                "prop" | "properties" => Ok(compile_node_property_access(clean_col, ctx)),
                "metric" | "metrics" => Ok(compile_node_metric_access(clean_col)),
                _ => Err(invalid_pred(format!(
                    "unsupported node state namespace 'node.{ns}'; expected node.prop.<k> or node.metric.<k>"
                ))),
            }
        }
        _ => Err(invalid_pred(
            "invalid compound identifier in node row predicate",
        )),
    }
}

fn compile_column_ident(
    col_raw: &str,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    ctx: ScalarContext,
) -> Result<CompiledPredicate, AppError> {
    let trimmed = ensure_valid_ident_chars(col_raw)?;
    let lower = trimmed.to_ascii_lowercase();
    let is_core_node_col = target_kind == PolicyTargetKind::NodeType
        && matches!(lower.as_str(), "node_key" | "title" | "type_name");

    check_whitelist(trimmed, is_core_node_col, allowed_columns)?;

    match target_kind {
        PolicyTargetKind::Dataset => {
            let quoted = format!("\"{}\"", trimmed.replace('"', "\"\""));
            Ok(CompiledPredicate {
                duckdb_sql: quoted.clone(),
                pg_sql: quoted,
            })
        }
        PolicyTargetKind::NodeType => {
            if is_core_node_col {
                Ok(CompiledPredicate {
                    duckdb_sql: format!("\"{lower}\""),
                    pg_sql: lower,
                })
            } else {
                Ok(compile_node_property_access(trimmed, ctx))
            }
        }
    }
}

fn compile_literal_value(val: &Value) -> Result<CompiledPredicate, AppError> {
    let lit = match val {
        Value::SingleQuotedString(s) => quote_sql_string(s),
        Value::Number(n, false) => {
            if !n.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return Err(invalid_pred(format!("invalid numeric literal '{n}'")));
            }
            n.clone()
        }
        Value::Boolean(b) => {
            if *b {
                String::from("TRUE")
            } else {
                String::from("FALSE")
            }
        }
        _ => {
            return Err(invalid_pred(
                "only single-quoted strings, numbers, and booleans are allowed as literals",
            ));
        }
    };
    Ok(CompiledPredicate {
        duckdb_sql: lit.clone(),
        pg_sql: lit,
    })
}

pub(crate) fn compile_string_literal_only(expr: &Expr) -> Result<String, AppError> {
    if let Expr::Value(val_span) = expr
        && let Value::SingleQuotedString(s) = &val_span.value
    {
        return Ok(quote_sql_string(s));
    }
    Err(invalid_pred("LIKE pattern must be a string literal"))
}

pub(crate) fn compile_ttl_clause(
    target_kind: PolicyTargetKind,
    ttl_timestamp_field: Option<&str>,
    ttl_max_age_hours: Option<i32>,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<Option<CompiledPredicate>, AppError> {
    let clean_field = ttl_timestamp_field.map(str::trim).filter(|s| !s.is_empty());
    let Some(hours) = ttl_max_age_hours else {
        if clean_field.is_some() {
            return Err(invalid_pred(
                "ttl_max_age_hours is required when ttl_timestamp_field is specified",
            ));
        }
        return Ok(None);
    };
    if !(1..=MAX_TTL_HOURS).contains(&hours) {
        return Err(invalid_pred(format!(
            "ttl_max_age_hours must be between 1 and {MAX_TTL_HOURS}, got {hours}"
        )));
    }

    match target_kind {
        PolicyTargetKind::Dataset => {
            let Some(raw_col) = clean_field else {
                return Err(invalid_pred(
                    "ttl_timestamp_field is required when ttl_max_age_hours is set on a dataset policy",
                ));
            };
            let col = ensure_valid_ident_chars(raw_col)?;
            check_whitelist(col, false, allowed_columns)?;
            let quoted = format!("\"{}\"", col.replace('"', "\"\""));
            let ts_re = SAFE_ISO_TS_REGEX.replace('\'', "''");
            Ok(Some(CompiledPredicate {
                duckdb_sql: format!(
                    "TRY_CAST({quoted} AS TIMESTAMPTZ) >= (CURRENT_TIMESTAMP - INTERVAL '{hours} hours')"
                ),
                pg_sql: format!(
                    "(CASE WHEN ({quoted}::text) ~ '{ts_re}' THEN ({quoted}::text)::timestamptz END) >= (NOW() - INTERVAL '{hours} hours')"
                ),
            }))
        }
        PolicyTargetKind::NodeType => {
            compile_node_ttl_clause(clean_field, hours, allowed_columns).map(Some)
        }
    }
}

fn compile_node_ttl_clause(
    clean_field: Option<&str>,
    hours: i32,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<CompiledPredicate, AppError> {
    let field = clean_field.unwrap_or("updated_at");
    let lower = field.to_ascii_lowercase();
    if matches!(lower.as_str(), "updated_at" | "node.updated_at") {
        return Ok(CompiledPredicate {
            duckdb_sql: format!("\"updated_at\" >= (CURRENT_TIMESTAMP - INTERVAL '{hours} hours')"),
            pg_sql: format!("updated_at >= (NOW() - INTERVAL '{hours} hours')"),
        });
    }
    if matches!(lower.as_str(), "created_at" | "node.created_at") {
        return Ok(CompiledPredicate {
            duckdb_sql: format!("\"updated_at\" >= (CURRENT_TIMESTAMP - INTERVAL '{hours} hours')"),
            pg_sql: format!("created_at >= (NOW() - INTERVAL '{hours} hours')"),
        });
    }
    let stripped = field
        .strip_prefix("node.properties.")
        .or_else(|| field.strip_prefix("node.prop."))
        .or_else(|| field.strip_prefix("properties."))
        .or_else(|| field.strip_prefix("prop."))
        .unwrap_or(field);
    let prop_key = ensure_valid_ident_chars(stripped)?;
    check_whitelist(prop_key, false, allowed_columns)?;
    let escaped = prop_key.replace('\'', "''");
    let ts_re = SAFE_ISO_TS_REGEX.replace('\'', "''");
    Ok(CompiledPredicate {
        duckdb_sql: format!(
            "TRY_CAST(json_extract_string(properties, '$.{escaped}') AS TIMESTAMPTZ) >= (CURRENT_TIMESTAMP - INTERVAL '{hours} hours')"
        ),
        pg_sql: format!(
            "(CASE WHEN (properties->>'{escaped}') ~ '{ts_re}' THEN (properties->>'{escaped}')::timestamptz END) >= (NOW() - INTERVAL '{hours} hours')"
        ),
    })
}

pub(crate) fn quote_sql_string(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}
