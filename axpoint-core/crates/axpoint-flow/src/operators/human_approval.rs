//! `HUMAN_APPROVAL` operator creating an idempotent approval gate and suspending the branch (`07-STEP07-08` §07.4).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{
    ApprovalSourceKind, ApprovalStatus, NewApprovalInput, Operator, OperatorContext,
    OperatorOutcome,
};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::FlowRepository;
use chrono::{Duration, Utc};

use super::{
    parse_optional_i64_any, parse_optional_string_vec, parse_optional_uuid_vec,
    parse_required_string, templated_int_schema, templated_uuid_schema,
};

/// Studio palette display name for `HUMAN_APPROVAL`.
pub const DISPLAY_NAME: &str = "人工审批门禁";

/// Studio palette short description for `HUMAN_APPROVAL`.
pub const DESCRIPTION: &str = "挂起当前分支并创建待办审批单，审批通过后继续推进下游算子";

/// Returns the JSON Schema validating `HUMAN_APPROVAL` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["title"],
        "properties": {
            "title": { "type": "string", "minLength": 1 },
            "detail": { "type": "object" },
            "detail_template": { "type": "object" },
            "assignee_roles": {
                "type": "array",
                "items": { "type": "string", "minLength": 1 }
            },
            "assignee_users": {
                "type": "array",
                "items": templated_uuid_schema()
            },
            "expires_in_minutes": templated_int_schema(1, 43_200),
            "expires_in_s": templated_int_schema(1, 2_592_000),
            "timeout_s": templated_int_schema(1, 2_592_000),
            "ttl_s": templated_int_schema(1, 2_592_000)
        },
        "additionalProperties": false
    })
}

/// `HUMAN_APPROVAL` operator implementation generic over [`FlowRepository`].
pub struct HumanApprovalOperator<F> {
    flow_repo: Arc<F>,
}

impl<F> HumanApprovalOperator<F>
where
    F: FlowRepository + 'static,
{
    /// Constructs a new [`HumanApprovalOperator`].
    #[must_use]
    pub const fn new(flow_repo: Arc<F>) -> Self {
        Self { flow_repo }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let title = parse_required_string(&ctx.config, "title")?;
        let detail_raw = ctx
            .config
            .get("detail")
            .filter(|v| !v.is_null())
            .or_else(|| ctx.config.get("detail_template"));
        let detail = match detail_raw {
            Some(v @ serde_json::Value::Object(_)) => v.clone(),
            None | Some(serde_json::Value::Null) => serde_json::json!({}),
            Some(other) => serde_json::json!({ "value": other }),
        };

        let mut assignee_roles = parse_optional_string_vec(&ctx.config, "assignee_roles")?;
        let assignee_users = parse_optional_uuid_vec(&ctx.config, "assignee_users")?;
        if assignee_roles.is_empty() && assignee_users.is_empty() {
            assignee_roles = vec![String::from("ADMIN"), String::from("OPERATOR")];
        }

        let expires_in =
            match parse_optional_i64_any(&ctx.config, &["expires_in_s", "timeout_s", "ttl_s"])? {
                Some(secs) => Some(secs),
                None => parse_optional_i64_any(&ctx.config, &["expires_in_minutes"])?
                    .map(|mins| mins.saturating_mul(60)),
            };
        let expires_at = expires_in
            .filter(|&secs| secs > 0)
            .map(|secs| Utc::now() + Duration::seconds(secs));

        let approval = self
            .flow_repo
            .create_or_get_approval(
                &ctx.tenant_id,
                NewApprovalInput {
                    run_id: Some(ctx.run_id),
                    node_id: Some(ctx.node_id.clone()),
                    source_kind: ApprovalSourceKind::Flow,
                    source_ref: ctx.idempotency_key.clone(),
                    title,
                    detail,
                    assignee_roles,
                    assignee_users,
                    expires_at,
                },
            )
            .await?;

        match approval.status {
            ApprovalStatus::Pending => Ok(OperatorOutcome::Suspend {
                approval_id: approval.id,
            }),
            ApprovalStatus::Approved => Ok(OperatorOutcome::Completed(serde_json::json!({
                "approval_id": approval.id,
                "decision": "APPROVE",
                "status": "APPROVED",
                "decided_by": approval.decided_by,
                "decided_at": approval.decided_at,
                "comment": approval.comment,
            }))),
            ApprovalStatus::Rejected => Err(AppError::Forbidden("APPROVAL_REJECTED")),
            ApprovalStatus::Expired => Err(AppError::Timeout("APPROVAL_EXPIRED")),
        }
    }
}

impl<F> Operator for HumanApprovalOperator<F>
where
    F: FlowRepository + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::HumanApproval
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn config_schema(&self) -> serde_json::Value {
        config_schema()
    }

    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move { self.run(ctx).await })
    }
}
