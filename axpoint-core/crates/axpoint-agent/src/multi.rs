//! Multi-agent hierarchy validation (`<= 2` nesting depth, cycle detection) and sub-agent
//! delegation tool helpers (`08-STEP09` §09.5.3).

use std::collections::HashSet;

use axpoint_domain::AppError;
use axpoint_domain::agent::{AgentSpec, ToolRef};
use axpoint_domain::ports::AgentRepository;
use uuid::Uuid;

use crate::model::ModelToolDef;

/// Maximum permitted sub-agent delegation depth (`root=0 -> specialist=1 -> leaf=2`, `08-STEP09` §09.5.3).
pub const MAX_SUB_AGENT_DEPTH: usize = 2;

/// Formats a sub-agent's machine name into a default `delegate_to_<agent_name>` tool identifier (`08-STEP09` §09.5.3).
#[must_use]
pub fn sub_agent_tool_name(agent_name: &str) -> String {
    let mut sanitized = String::with_capacity(agent_name.len());
    for ch in agent_name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        sanitized.push_str("sub_agent");
    }
    let full = format!("delegate_to_{sanitized}");
    if full.len() > 64 {
        full.chars().take(64).collect()
    } else {
        full
    }
}

/// Builds the [`ModelToolDef`] advertising a published specialist sub-agent to a coordinator agent.
#[must_use]
pub fn build_sub_agent_tool_def(
    agent_name: &str,
    as_tool_name: &str,
    description: &str,
) -> ModelToolDef {
    let exposed_name = if as_tool_name.trim().is_empty() {
        sub_agent_tool_name(agent_name)
    } else {
        String::from(as_tool_name.trim())
    };
    let desc = if description.trim().is_empty() {
        format!("将专业子任务委托给子智能体 {agent_name} 独立分析并返回摘要结论")
    } else {
        format!("委托子智能体 {agent_name}：{}", description.trim())
    };
    ModelToolDef {
        name: exposed_name,
        description: desc,
        parameters: serde_json::json!({
            "type": "object",
            "required": ["task"],
            "properties": {
                "task": {
                    "type": "string",
                    "minLength": 1,
                    "description": "清晰描述需该子智能体独立调查或回答的子问题"
                },
                "context": {
                    "type": "object",
                    "description": "可选结构化背景上下文（不会泄漏主智能体完整对话历史）"
                }
            },
            "additionalProperties": false
        }),
    }
}

/// Formats the isolated single-turn user prompt passed into a delegated sub-agent (`08-STEP09` §09.5.3).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `arguments` lacks a non-empty `"task"` string.
pub fn format_sub_agent_prompt(arguments: &serde_json::Value) -> Result<String, AppError> {
    let task = arguments
        .get("task")
        .or_else(|| arguments.get("instruction"))
        .or_else(|| arguments.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "sub-agent delegation requires non-empty 'task' string",
            )
        })?;

    match arguments.get("context") {
        Some(ctx) if !ctx.is_null() => {
            let ctx_json = serde_json::to_string_pretty(ctx).unwrap_or_else(|_| String::from("{}"));
            Ok(format!("任务指令：\n{task}\n\n补充上下文：\n{ctx_json}"))
        }
        _ => Ok(String::from(task)),
    }
}

/// Validates a `ToolRef::Agent.as_tool_name` against `^[a-z][a-z0-9_]{0,63}$` when non-empty.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `as_tool_name` violates identifier rules.
pub fn validate_as_tool_name(as_tool_name: &str) -> Result<(), AppError> {
    let trimmed = as_tool_name.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let bytes = trimmed.as_bytes();
    let Some(&first) = bytes.first() else {
        return Ok(());
    };
    if bytes.len() > 64
        || !first.is_ascii_lowercase()
        || !bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "invalid sub-agent as_tool_name '{trimmed}': must match ^[a-z][a-z0-9_]{{0,63}}$"
            ),
        ));
    }
    Ok(())
}

/// Statically validates that `root_spec` (owned by `root_agent_id`, if already persisted) contains
/// no `ToolRef::Agent` reference cycles and that maximum delegation depth does not exceed [`MAX_SUB_AGENT_DEPTH`] (`2`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) on self-reference, cycle, missing sub-agent,
/// or delegation depth `> 2`.
pub async fn validate_sub_agent_graph<R>(
    store: &R,
    tenant_id: &str,
    root_agent_id: Option<Uuid>,
    root_spec: &AgentSpec,
) -> Result<(), AppError>
where
    R: AgentRepository,
{
    let mut visiting = HashSet::new();
    if let Some(id) = root_agent_id {
        visiting.insert(id);
    }
    validate_children_recursive(store, tenant_id, root_spec, 1, &mut visiting).await
}

fn validate_children_recursive<'a, R>(
    store: &'a R,
    tenant_id: &'a str,
    spec: &'a AgentSpec,
    current_depth: usize,
    visiting: &'a mut HashSet<Uuid>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>>
where
    R: AgentRepository,
{
    Box::pin(async move {
        for tool in &spec.tools {
            let ToolRef::Agent {
                agent_id,
                as_tool_name,
            } = tool
            else {
                continue;
            };
            validate_as_tool_name(as_tool_name)?;
            let next_depth = current_depth.saturating_add(1);
            if next_depth > MAX_SUB_AGENT_DEPTH {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!(
                        "SUB_AGENT nesting depth exceeds maximum allowed depth {MAX_SUB_AGENT_DEPTH}"
                    ),
                ));
            }
            if !visiting.insert(*agent_id) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("SUB_AGENT reference cycle detected involving agent {agent_id}"),
                ));
            }

            let child_agent = store
                .get_agent(tenant_id, *agent_id)
                .await?
                .ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("referenced SUB_AGENT {agent_id} does not exist"),
                    )
                })?;

            let child_spec = if child_agent.version >= 1
                && let Some(ver) = store
                    .get_agent_version(tenant_id, *agent_id, child_agent.version)
                    .await?
            {
                ver.spec
            } else {
                child_agent.spec
            };

            validate_children_recursive(store, tenant_id, &child_spec, next_depth, visiting)
                .await?;
            visiting.remove(agent_id);
        }
        Ok(())
    })
}
