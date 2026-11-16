//! Parameter template parsing, static ancestor validation, and JSON type-preserving rendering (`07-STEP07-08` §07.2.1).

use std::collections::{BTreeSet, HashMap};

use axpoint_domain::AppError;
use axpoint_domain::enums::Environment;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Parsed segment inside a string containing `{{ ... }}` parameter placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSegment {
    /// Static literal substring.
    Literal(String),
    /// Trimmed placeholder expression inside `{{ ... }}`.
    Expr(String),
}

/// Runtime variable bindings used to render operator `config` JSON before execution (`07-STEP07-08` §07.2.1).
#[derive(Debug, Clone)]
pub struct TemplateContext<'a> {
    /// Run `trigger_payload` JSON (`{{ trigger.* }}`).
    pub trigger: &'a serde_json::Value,
    /// Map from ancestor `node_id` to its `SUCCEEDED` JSON output (`{{ nodes.<id>.output.* }}`).
    pub step_outputs: &'a HashMap<String, serde_json::Value>,
    /// Current `ax_flow_runs.id` (`{{ run.id }}`).
    pub run_id: Uuid,
    /// Current execution environment (`{{ run.environment }}`).
    pub run_environment: Environment,
    /// Run start timestamp (`{{ run.started_at }}`).
    pub run_started_at: Option<DateTime<Utc>>,
    /// Owning `ax_flows.id` (`{{ flow.id }}`).
    pub flow_id: Uuid,
    /// Pinned `FlowVersionRecord.version` (`{{ flow.version }}`).
    pub flow_version: i32,
}

/// Splits a string into [`TemplateSegment`]s, rejecting unclosed, nested, or empty `{{ ... }}` placeholders.
///
/// # Errors
/// Returns a human-readable diagnostic string when `{{ ... }}` syntax is malformed.
pub fn parse_template_segments(input: &str) -> Result<Vec<TemplateSegment>, String> {
    if !input.contains("{{") {
        return Ok(vec![TemplateSegment::Literal(String::from(input))]);
    }

    let mut segments = Vec::new();
    let mut rest = input;

    while let Some(open_pos) = rest.find("{{") {
        let (before, after_open_inclusive) = rest
            .split_at_checked(open_pos)
            .ok_or_else(|| String::from("invalid UTF-8 boundary at '{{'"))?;
        if !before.is_empty() {
            segments.push(TemplateSegment::Literal(String::from(before)));
        }

        let after_open = after_open_inclusive
            .strip_prefix("{{")
            .ok_or_else(|| String::from("missing '{{' prefix"))?;
        let Some(close_pos) = after_open.find("}}") else {
            return Err(String::from("unclosed '{{' in template string"));
        };
        let (inner, after_close_inclusive) = after_open
            .split_at_checked(close_pos)
            .ok_or_else(|| String::from("invalid UTF-8 boundary at '}}'"))?;

        if inner.contains("{{") {
            return Err(String::from("nested '{{' inside template placeholder"));
        }
        let expr = inner.trim();
        if expr.is_empty() {
            return Err(String::from("empty '{{ }}' placeholder is not allowed"));
        }
        segments.push(TemplateSegment::Expr(String::from(expr)));

        rest = after_close_inclusive
            .strip_prefix("}}")
            .ok_or_else(|| String::from("missing '}}' suffix"))?;
    }

    if !rest.is_empty() {
        segments.push(TemplateSegment::Literal(String::from(rest)));
    }

    Ok(segments)
}

/// Statically validates a single trimmed placeholder path (`trigger.*`, `nodes.<id>.output.*`, `run.*`, `flow.*`)
/// against the strict `ancestors` of the consuming node (`07-STEP07-08` §07.2.1).
///
/// # Errors
/// Returns a diagnostic message when the root namespace is unknown, a path segment is empty,
/// or `nodes.<id>.output` references a node that is not a strict ancestor.
pub fn validate_placeholder_expr(expr: &str, ancestors: &BTreeSet<String>) -> Result<(), String> {
    let parts: Vec<&str> = expr.split('.').collect();
    if parts
        .iter()
        .any(|seg| seg.is_empty() || seg.chars().any(char::is_whitespace))
    {
        return Err(format!("invalid placeholder path '{expr}'"));
    }

    let Some(&root) = parts.first() else {
        return Err(String::from("empty placeholder path"));
    };

    match root {
        "trigger" => Ok(()),
        "nodes" => validate_nodes_placeholder(&parts, expr, ancestors),
        "run" => match (parts.get(1).copied(), parts.len()) {
            (Some("id" | "environment" | "started_at"), 2) => Ok(()),
            _ => Err(format!(
                "unsupported run variable '{expr}'; expected run.id, run.environment, or run.started_at"
            )),
        },
        "flow" => match (parts.get(1).copied(), parts.len()) {
            (Some("id" | "version"), 2) => Ok(()),
            _ => Err(format!(
                "unsupported flow variable '{expr}'; expected flow.id or flow.version"
            )),
        },
        other => Err(format!(
            "unknown template namespace '{other}' in '{{{{ {expr} }}}}'; allowed: trigger, nodes, run, flow"
        )),
    }
}

fn validate_nodes_placeholder(
    parts: &[&str],
    expr: &str,
    ancestors: &BTreeSet<String>,
) -> Result<(), String> {
    let Some(&ref_node) = parts.get(1) else {
        return Err(format!(
            "placeholder '{expr}' must specify nodes.<node_id>.output"
        ));
    };
    let Some(&slot) = parts.get(2) else {
        return Err(format!(
            "placeholder '{expr}' must specify nodes.{ref_node}.output"
        ));
    };
    if slot != "output" {
        return Err(format!(
            "placeholder '{expr}' can only access '.output' on node '{ref_node}'"
        ));
    }
    if !ancestors.contains(ref_node) {
        return Err(format!(
            "node '{ref_node}' referenced in '{{{{ {expr} }}}}' is not a strict upstream ancestor"
        ));
    }
    Ok(())
}

/// Recursively collects all static template errors inside `config` for a node with `ancestors`.
#[must_use]
pub fn collect_config_template_errors(
    config: &serde_json::Value,
    ancestors: &BTreeSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    collect_value_errors(config, ancestors, &mut errors);
    errors
}

fn collect_value_errors(
    value: &serde_json::Value,
    ancestors: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(s) => match parse_template_segments(s) {
            Ok(segments) => {
                for seg in segments {
                    if let TemplateSegment::Expr(expr) = seg
                        && let Err(msg) = validate_placeholder_expr(&expr, ancestors)
                    {
                        errors.push(msg);
                    }
                }
            }
            Err(msg) => errors.push(msg),
        },
        serde_json::Value::Array(items) => {
            for item in items {
                collect_value_errors(item, ancestors, errors);
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                collect_value_errors(val, ancestors, errors);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

/// Renders all `{{ ... }}` templates inside `value` against `ctx`.
///
/// - Whole-string single placeholders (`"{{ nodes.a.output.count }}"`) preserve the native JSON type
///   (`Number`, `Bool`, `Object`, `Array`, `Null`, or `String`).
/// - Embedded placeholders (`"rows={{ nodes.a.output.count }}"`) concatenate into a JSON `String`,
///   rendering `null` / missing fields as empty string `""` (`07-STEP07-08` §07.2.1).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`EXPR_INVALID`) if a template expression has invalid syntax or namespace.
pub fn render_config_value(
    value: &serde_json::Value,
    ctx: &TemplateContext<'_>,
) -> Result<serde_json::Value, AppError> {
    match value {
        serde_json::Value::String(s) => render_string_template(s, ctx),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(render_config_value(item, ctx)?);
            }
            Ok(serde_json::Value::Array(out))
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), render_config_value(v, ctx)?);
            }
            Ok(serde_json::Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn render_string_template(
    raw: &str,
    ctx: &TemplateContext<'_>,
) -> Result<serde_json::Value, AppError> {
    if !raw.contains("{{") {
        return Ok(serde_json::Value::String(String::from(raw)));
    }

    let trimmed = raw.trim();
    if trimmed.starts_with("{{") && trimmed.ends_with("}}") {
        let trimmed_segments =
            parse_template_segments(trimmed).map_err(|msg| AppError::bad("EXPR_INVALID", msg))?;
        if trimmed_segments.len() == 1
            && let Some(TemplateSegment::Expr(expr)) = trimmed_segments.first()
        {
            return resolve_placeholder_value(expr, ctx);
        }
    }

    let segments =
        parse_template_segments(raw).map_err(|msg| AppError::bad("EXPR_INVALID", msg))?;
    let mut buf = String::with_capacity(raw.len());
    for seg in segments {
        match seg {
            TemplateSegment::Literal(lit) => buf.push_str(&lit),
            TemplateSegment::Expr(expr) => {
                let val = resolve_placeholder_value(&expr, ctx)?;
                append_inline_value(&mut buf, &val);
            }
        }
    }
    Ok(serde_json::Value::String(buf))
}

fn append_inline_value(buf: &mut String, val: &serde_json::Value) {
    match val {
        serde_json::Value::Null => {}
        serde_json::Value::String(s) => buf.push_str(s),
        serde_json::Value::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => buf.push_str(&n.to_string()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            if let Ok(encoded) = serde_json::to_string(val) {
                buf.push_str(&encoded);
            }
        }
    }
}

fn resolve_placeholder_value(
    expr: &str,
    ctx: &TemplateContext<'_>,
) -> Result<serde_json::Value, AppError> {
    let parts: Vec<&str> = expr.split('.').collect();
    if parts
        .iter()
        .any(|seg| seg.is_empty() || seg.chars().any(char::is_whitespace))
    {
        return Err(AppError::bad(
            "EXPR_INVALID",
            format!("invalid placeholder path '{expr}'"),
        ));
    }

    let Some(&root) = parts.first() else {
        return Err(AppError::bad("EXPR_INVALID", "empty placeholder path"));
    };

    match root {
        "trigger" => Ok(walk_json_path(
            ctx.trigger,
            parts.get(1..).unwrap_or_default(),
        )),
        "nodes" => {
            let Some(&node_id) = parts.get(1) else {
                return Err(AppError::bad(
                    "EXPR_INVALID",
                    format!("invalid node reference '{expr}'"),
                ));
            };
            if parts.get(2).copied() != Some("output") {
                return Err(AppError::bad(
                    "EXPR_INVALID",
                    format!("invalid node output reference '{expr}'"),
                ));
            }
            let Some(out_val) = ctx.step_outputs.get(node_id) else {
                return Ok(serde_json::Value::Null);
            };
            Ok(walk_json_path(out_val, parts.get(3..).unwrap_or_default()))
        }
        "run" => match (parts.get(1).copied(), parts.len()) {
            (Some("id"), 2) => Ok(serde_json::Value::String(ctx.run_id.to_string())),
            (Some("environment"), 2) => Ok(serde_json::Value::String(String::from(
                ctx.run_environment.as_str(),
            ))),
            (Some("started_at"), 2) => {
                Ok(ctx.run_started_at.map_or(serde_json::Value::Null, |dt| {
                    serde_json::Value::String(dt.to_rfc3339())
                }))
            }
            _ => Err(AppError::bad(
                "EXPR_INVALID",
                format!("unsupported run variable '{expr}'"),
            )),
        },
        "flow" => match (parts.get(1).copied(), parts.len()) {
            (Some("id"), 2) => Ok(serde_json::Value::String(ctx.flow_id.to_string())),
            (Some("version"), 2) => Ok(serde_json::Value::Number(serde_json::Number::from(
                ctx.flow_version,
            ))),
            _ => Err(AppError::bad(
                "EXPR_INVALID",
                format!("unsupported flow variable '{expr}'"),
            )),
        },
        other => Err(AppError::bad(
            "EXPR_INVALID",
            format!("unknown template namespace '{other}'"),
        )),
    }
}

fn walk_json_path(root: &serde_json::Value, path: &[&str]) -> serde_json::Value {
    let mut curr = root;
    for &seg in path {
        match curr {
            serde_json::Value::Object(map) => {
                let Some(next) = map.get(seg) else {
                    return serde_json::Value::Null;
                };
                curr = next;
            }
            serde_json::Value::Array(items) => {
                let Ok(idx) = seg.parse::<usize>() else {
                    return serde_json::Value::Null;
                };
                let Some(next) = items.get(idx) else {
                    return serde_json::Value::Null;
                };
                curr = next;
            }
            _ => return serde_json::Value::Null,
        }
    }
    curr.clone()
}
