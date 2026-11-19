//! `AGENT_TASK` operator (`08-STEP09` §09.6) delegating an `AxFlow` DAG step to a published `AxAgent`.

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::agent::{AgentTaskInput, AgentTaskOutcome};
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::AgentTaskPort;

use super::{
    parse_optional_i64_any, parse_optional_u32, parse_required_string, parse_required_uuid,
    templated_int_schema, templated_uuid_schema,
};

/// `FlowOperator::AgentTask` implementation delegating to [`AgentTaskPort::execute_flow_agent_task`].
pub struct AgentTaskOperator {
    agent_port: Arc<dyn AgentTaskPort>,
}

impl AgentTaskOperator {
    /// Human-readable operator label in `GET /api/axflow/operators`.
    pub const DISPLAY_NAME: &'static str = "智能体子任务";
    /// Human-readable operator description in `GET /api/axflow/operators`.
    pub const DESCRIPTION: &'static str =
        "将子任务委托给已发布 AxAgent 实例执行，支持结构化输出与审批门禁共享";

    /// Creates a new [`AgentTaskOperator`] backed by `agent_port`.
    #[must_use]
    pub fn new(agent_port: Arc<dyn AgentTaskPort>) -> Self {
        Self { agent_port }
    }

    /// JSON Schema validating `AGENT_TASK` node configuration (`08-STEP09` §09.6).
    #[must_use]
    pub fn config_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["agent_id", "instruction"],
            "properties": {
                "agent_id": templated_uuid_schema(),
                "version": templated_int_schema(1, 1_000_000),
                "input": {},
                "instruction": { "type": "string", "minLength": 1 },
                "output_schema": { "type": "object" },
                "max_steps": templated_int_schema(1, 20)
            },
            "additionalProperties": false
        })
    }
}

impl Operator for AgentTaskOperator {
    fn kind(&self) -> FlowOperator {
        FlowOperator::AgentTask
    }

    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }

    fn description(&self) -> &'static str {
        Self::DESCRIPTION
    }

    fn config_schema(&self) -> serde_json::Value {
        Self::config_schema()
    }

    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move {
            if ctx.is_canceled() {
                return Err(AppError::Conflict("RUN_CANCELED"));
            }

            let agent_id = parse_required_uuid(&ctx.config, "agent_id")?;
            let version = match parse_optional_i64_any(&ctx.config, &["version"])? {
                Some(v) => {
                    let parsed = i32::try_from(v).map_err(|_| {
                        AppError::bad("INVALID_ARGUMENT", "version out of i32 range")
                    })?;
                    if parsed < 1 {
                        return Err(AppError::bad("INVALID_ARGUMENT", "version must be >= 1"));
                    }
                    Some(parsed)
                }
                None => None,
            };
            let instruction = parse_required_string(&ctx.config, "instruction")?;
            let input = ctx
                .config
                .get("input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let output_schema = ctx
                .config
                .get("output_schema")
                .filter(|v| !v.is_null())
                .cloned();
            let max_steps = parse_optional_u32(&ctx.config, "max_steps")?;
            if let Some(steps) = max_steps
                && !(1..=20).contains(&steps)
            {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "max_steps must be in 1..=20",
                ));
            }

            let outcome = self
                .agent_port
                .execute_flow_agent_task(AgentTaskInput {
                    tenant_id: ctx.tenant_id.clone(),
                    user_id: ctx.principal.user_id,
                    username: ctx.principal.username.clone(),
                    dept_id: ctx.principal.dept_id.clone(),
                    roles: ctx.principal.roles.clone(),
                    permissions: ctx.principal.permissions.clone(),
                    run_id: ctx.run_id,
                    node_id: ctx.node_id.clone(),
                    idempotency_key: ctx.idempotency_key.clone(),
                    agent_id,
                    version,
                    input,
                    instruction,
                    output_schema,
                    max_steps,
                })
                .await?;

            match outcome {
                AgentTaskOutcome::Completed {
                    conversation_id,
                    answer,
                    structured,
                } => Ok(OperatorOutcome::Completed(serde_json::json!({
                    "conversation_id": conversation_id,
                    "answer": answer,
                    "structured": structured,
                }))),
                AgentTaskOutcome::WaitingApproval { approval_id, .. } => {
                    Ok(OperatorOutcome::Suspend { approval_id })
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axpoint_domain::AppError;
    use axpoint_domain::agent::{AgentTaskInput, AgentTaskOutcome};
    use axpoint_domain::enums::{Environment, FlowOperator};
    use axpoint_domain::flow::{FlowActorPrincipal, Operator, OperatorContext, OperatorOutcome};
    use axpoint_domain::platform::BoxFuture;
    use axpoint_domain::ports::AgentTaskPort;
    use uuid::Uuid;

    use super::AgentTaskOperator;

    struct StubAgentPort {
        calls: AtomicUsize,
        suspend_approval: Option<Uuid>,
    }

    impl AgentTaskPort for StubAgentPort {
        fn execute_flow_agent_task(
            &self,
            input: AgentTaskInput,
        ) -> BoxFuture<'_, AgentTaskOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if let Some(aid) = self.suspend_approval {
                    Ok(AgentTaskOutcome::WaitingApproval {
                        conversation_id: Uuid::new_v4(),
                        approval_id: aid,
                    })
                } else {
                    Ok(AgentTaskOutcome::Completed {
                        conversation_id: Uuid::nil(),
                        answer: format!("executed: {}", input.instruction),
                        structured: Some(serde_json::json!({ "ok": true })),
                    })
                }
            })
        }
    }

    #[tokio::test]
    async fn agent_task_operator_executes_and_suspends_on_approval() -> Result<(), AppError> {
        let agent_id = Uuid::new_v4();
        let port = Arc::new(StubAgentPort {
            calls: AtomicUsize::new(0),
            suspend_approval: None,
        });
        let op = AgentTaskOperator::new(port.clone());
        assert_eq!(op.kind(), FlowOperator::AgentTask);

        let ctx = OperatorContext::new(
            String::from("default"),
            Uuid::new_v4(),
            1,
            Uuid::new_v4(),
            String::from("agent_step"),
            1,
            Environment::Prod,
        )
        .with_execution_inputs(
            serde_json::json!({
                "agent_id": agent_id.to_string(),
                "instruction": "总结异常订单",
                "max_steps": 5
            }),
            FlowActorPrincipal::service(None),
            Arc::new(AtomicBool::new(false)),
        );

        let out = op.execute(&ctx).await?;
        let OperatorOutcome::Completed(val) = out else {
            return Err(AppError::Config(String::from("expected Completed")));
        };
        assert_eq!(
            val.get("answer").and_then(serde_json::Value::as_str),
            Some("executed: 总结异常订单")
        );
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);

        let approval_id = Uuid::new_v4();
        let suspend_op = AgentTaskOperator::new(Arc::new(StubAgentPort {
            calls: AtomicUsize::new(0),
            suspend_approval: Some(approval_id),
        }));
        let out2 = suspend_op.execute(&ctx).await?;
        assert_eq!(out2, OperatorOutcome::Suspend { approval_id });
        Ok(())
    }
}
