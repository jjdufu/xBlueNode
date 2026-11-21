//! Interactive `xBlueView` widget action invocation (`RUN_FLOW` and `WRITE_NODE`) (`10-STEP11-12` §11.6.2).

use axpoint_domain::AppError;
use axpoint_domain::enums::{AuditAction, Environment, Permission, TokenKind, TriggerType};
use axpoint_domain::flow::{
    ActionKind, ActionLogStatus, ActionSourceKind, NewActionLogInput, StartRunParams,
};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{AuditSink, FlowRepository, ViewRepository};
use axpoint_domain::view::{
    InvokeWidgetActionMode, InvokeWidgetActionRequest, InvokeWidgetActionResponse,
    ViewWriteNodeParams, WidgetActionType, WidgetDataSource, WriteNodeTargetKind,
};
use axpoint_security::AuthPrincipal;
use chrono::Utc;
use uuid::Uuid;

use super::cache::ViewCache;

/// Validates `inputs` against an action widget's JSON `input_schema` (`400 SCHEMA_INVALID`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`SCHEMA_INVALID`) when `input_schema` is malformed or
/// `inputs` fails schema validation.
pub fn validate_action_inputs(
    inputs: &serde_json::Value,
    input_schema: &serde_json::Value,
) -> Result<(), AppError> {
    let validator = jsonschema::validator_for(input_schema).map_err(|err| {
        AppError::bad(
            "SCHEMA_INVALID",
            format!("widget input_schema is invalid JSON Schema: {err}"),
        )
    })?;

    let errors: Vec<String> = validator
        .iter_errors(inputs)
        .take(5)
        .map(|err| {
            let path = err.instance_path().to_string();
            if path.is_empty() {
                err.to_string()
            } else {
                format!("{path}: {err}")
            }
        })
        .collect();

    if !errors.is_empty() {
        return Err(AppError::bad("SCHEMA_INVALID", errors.join("; ")));
    }
    Ok(())
}

/// Executes an interactive `ACTION_BUTTON` or `ACTION_FORM` widget invocation (`POST /api/xblueview/widgets/{id}/invoke`).
///
/// Rejects `TokenKind::Share` tokens with `403 PERMISSION_DENIED`, requires `Permission::FlowRun`,
/// and enforces published dashboard visibility when the caller lacks `Permission::ViewEdit`.
///
/// # Errors
/// Propagates `403 PERMISSION_DENIED`, `404 WIDGET_NOT_FOUND` / `404 NOT_FOUND`, `400 SCHEMA_INVALID`,
/// `400 INVALID_ARGUMENT`, or storage/workflow execution errors.
pub async fn invoke_widget_action<S>(
    store: &S,
    cache: &ViewCache,
    principal: &AuthPrincipal,
    widget_id: Uuid,
    req: InvokeWidgetActionRequest,
) -> Result<InvokeWidgetActionResponse, AppError>
where
    S: ViewRepository + FlowRepository + AuditSink + ?Sized,
{
    if principal.token_kind == TokenKind::Share {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    principal.require(Permission::FlowRun)?;

    let widget = store
        .get_widget(&principal.tenant_id, widget_id)
        .await?
        .ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;

    if !principal.has(Permission::ViewEdit) {
        let _published = store
            .get_dashboard(&principal.tenant_id, widget.dashboard_id, true)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
    }

    let WidgetDataSource::Action {
        action_type,
        flow_id,
        node_id,
        target_kind,
        key,
        input_schema,
        ..
    } = widget.data_source
    else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "target widget does not have an ACTION data_source",
        ));
    };

    validate_action_inputs(&req.inputs, &input_schema)?;

    let response = match action_type {
        WidgetActionType::RunFlow => {
            invoke_run_flow(store, principal, widget.id, flow_id, req).await?
        }
        WidgetActionType::WriteNode => {
            invoke_write_node(
                store,
                principal,
                widget.id,
                node_id,
                target_kind,
                key.as_deref(),
                req,
            )
            .await?
        }
    };

    cache
        .invalidate_widgets(&principal.tenant_id, &[widget.id])
        .await;
    Ok(response)
}

async fn invoke_run_flow<S: FlowRepository + AuditSink + ?Sized>(
    store: &S,
    principal: &AuthPrincipal,
    widget_id: Uuid,
    flow_id: Option<Uuid>,
    req: InvokeWidgetActionRequest,
) -> Result<InvokeWidgetActionResponse, AppError> {
    let target_flow_id = flow_id.ok_or_else(|| {
        AppError::bad("INVALID_ARGUMENT", "RUN_FLOW action widget missing flow_id")
    })?;

    let raw_inputs = req.inputs.clone();
    let mut payload_map = match req.inputs {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert(String::from("inputs"), other);
            map
        }
    };
    payload_map.insert(
        String::from("widget_id"),
        serde_json::Value::String(widget_id.to_string()),
    );
    if let Some(nid) = req.target_node_id {
        payload_map.insert(
            String::from("target_node_id"),
            serde_json::Value::String(nid.to_string()),
        );
    }

    let idempotency_key = req
        .idempotency_key
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());

    let (run, existing) = store
        .start_flow_run(
            &principal.tenant_id,
            StartRunParams {
                flow_id: target_flow_id,
                environment: Environment::Prod,
                explicit_version: None,
                trigger_source: TriggerType::Api,
                trigger_payload: serde_json::Value::Object(payload_map),
                idempotency_key: idempotency_key.clone(),
                started_by: (!principal.user_id.is_nil()).then_some(principal.user_id),
            },
        )
        .await?;

    let log_idem_key = idempotency_key.as_deref().map_or_else(
        || format!("view:run_flow:{widget_id}:{}", run.id),
        |k| format!("view:run_flow:{widget_id}:{k}"),
    );
    let (action_log, _) = store
        .record_action_log(
            &principal.tenant_id,
            NewActionLogInput {
                run_id: Some(run.id),
                node_id: req.target_node_id.map(|id| id.to_string()),
                source_kind: ActionSourceKind::View,
                action_kind: ActionKind::Http,
                target: widget_id.to_string(),
                payload: serde_json::json!({
                    "flow_id": target_flow_id,
                    "run_id": run.id,
                    "inputs": raw_inputs,
                }),
                status: ActionLogStatus::Done,
                idempotency_key: log_idem_key,
                error_message: None,
            },
        )
        .await?;

    record_flow_run_audit(
        store,
        principal,
        widget_id,
        target_flow_id,
        run.id,
        action_log.id,
        existing,
    )
    .await?;

    Ok(InvokeWidgetActionResponse {
        mode: InvokeWidgetActionMode::Flow,
        run_id: Some(run.id),
        status: String::from(run.status.as_str()),
        node_id: req.target_node_id,
        action_log_id: Some(action_log.id),
        idempotency_replay: existing,
    })
}

async fn record_flow_run_audit<S: AuditSink + ?Sized>(
    store: &S,
    principal: &AuthPrincipal,
    widget_id: Uuid,
    flow_id: Uuid,
    run_id: Uuid,
    action_log_id: i64,
    idempotency_replay: bool,
) -> Result<(), AppError> {
    store
        .write_batch(vec![AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: (!principal.user_id.is_nil()).then_some(principal.user_id),
            username: principal.username.clone(),
            action: AuditAction::FlowRun,
            resource_ref: format!("flow:{flow_id}"),
            status_code: 200,
            client_ip: String::new(),
            trace_id: String::new(),
            detail_json: serde_json::json!({
                "widget_id": widget_id,
                "flow_id": flow_id,
                "run_id": run_id,
                "action_log_id": action_log_id,
                "idempotency_replay": idempotency_replay,
            }),
            created_at: Utc::now(),
        }])
        .await
}

async fn invoke_write_node<S: ViewRepository + ?Sized>(
    store: &S,
    principal: &AuthPrincipal,
    widget_id: Uuid,
    default_node_id: Option<Uuid>,
    target_kind: Option<WriteNodeTargetKind>,
    key: Option<&str>,
    req: InvokeWidgetActionRequest,
) -> Result<InvokeWidgetActionResponse, AppError> {
    let target_node_id = req.target_node_id.or(default_node_id).ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            "WRITE_NODE action requires target_node_id or preset node_id",
        )
    })?;
    let t_kind = target_kind.ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            "WRITE_NODE action widget missing target_kind",
        )
    })?;
    let field_key = key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "WRITE_NODE action widget missing key"))?
        .to_owned();

    let extracted_val = extract_write_value(&req.inputs, &field_key)?;
    let numeric_value = if t_kind == WriteNodeTargetKind::Metric {
        let num = extracted_val.as_f64().ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "WRITE_NODE METRIC value must be a finite number",
            )
        })?;
        if !num.is_finite() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "WRITE_NODE METRIC value must be finite",
            ));
        }
        Some(num)
    } else {
        None
    };

    let idempotency_key = req
        .idempotency_key
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| format!("view:{widget_id}:{}", Uuid::new_v4()));

    let outcome = store
        .execute_view_write_node(
            &principal.tenant_id,
            ViewWriteNodeParams {
                widget_id,
                node_id: target_node_id,
                target_kind: t_kind,
                key: field_key,
                value: extracted_val,
                idempotency_key,
                caller_dept_id: principal.dept_id.clone(),
                numeric_value,
            },
        )
        .await?;

    Ok(InvokeWidgetActionResponse {
        mode: InvokeWidgetActionMode::WriteNode,
        run_id: None,
        status: String::from("DONE"),
        node_id: Some(outcome.node_id),
        action_log_id: Some(outcome.action_log_id),
        idempotency_replay: outcome.idempotency_replay,
    })
}

fn extract_write_value(
    inputs: &serde_json::Value,
    field_key: &str,
) -> Result<serde_json::Value, AppError> {
    if let Some(obj) = inputs.as_object() {
        if let Some(val) = obj.get("value") {
            return Ok(val.clone());
        }
        if let Some(val) = obj.get(field_key) {
            return Ok(val.clone());
        }
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("WRITE_NODE inputs must provide 'value' or '{field_key}'"),
        ));
    }
    Ok(inputs.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_action_inputs_rejects_missing_required_field() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["value"],
            "properties": { "value": { "type": "number" } }
        });
        let err = validate_action_inputs(&serde_json::json!({}), &schema);
        assert!(matches!(
            err,
            Err(AppError::BadRequest {
                code: "SCHEMA_INVALID",
                ..
            })
        ));
        assert!(validate_action_inputs(&serde_json::json!({"value": 620}), &schema).is_ok());
    }
}
