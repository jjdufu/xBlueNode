//! `ACTION_EMIT` operator executing idempotent node writebacks (`DRY_RUN` in `TEST`) (`07-STEP07-08` §07.3.3 & §07.4).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, FlowOperator, MetricSourceTag};
use axpoint_domain::flow::{
    ActionKind, ActionLogStatus, ActionSourceKind, NewActionLogInput, Operator, OperatorContext,
    OperatorOutcome,
};
use axpoint_domain::graph::{
    MetricPointInput, NodeListQuery, NodeListResponse, NodeRef, PatchNodeRequest,
    WriteMetricsRequest,
};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{FlowRepository, GraphStorePort, MetricStorePort};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    parse_optional_bool, parse_optional_f64, parse_optional_string_any, parse_optional_uuid_any,
    parse_required_string_any, templated_bool_schema, templated_uuid_schema,
};

/// Studio palette display name for `ACTION_EMIT`.
pub const DISPLAY_NAME: &str = "动作回写下发";

/// Studio palette short description for `ACTION_EMIT`.
pub const DESCRIPTION: &str =
    "幂等回写业务节点指标、节点属性或通知流水；TEST 环境自动记录为 DRY_RUN";

/// Returns the JSON Schema validating `ACTION_EMIT` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    let node_obj = node_object_schema();
    let node_ref = node_ref_schema();
    let action_item = serde_json::json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string", "minLength": 1 },
            "action_kind": { "type": "string", "minLength": 1 },
            "node_id": templated_uuid_schema(),
            "target_node_id": templated_uuid_schema(),
            "node_ref": node_ref.clone(),
            "node": node_obj.clone(),
            "metric": { "type": "string", "minLength": 1 },
            "metric_key": { "type": "string", "minLength": 1 },
            "value": {},
            "value_text": { "type": "string" },
            "ts": { "type": "string" },
            "properties": { "type": "object" },
            "property": { "type": "string", "minLength": 1 },
            "key": { "type": "string", "minLength": 1 },
            "target": { "type": "string" },
            "channel": { "type": "string" },
            "payload": {},
            "message": { "type": "string" },
            "dry_run": templated_bool_schema()
        }
    });

    serde_json::json!({
        "type": "object",
        "properties": {
            "actions": {
                "type": "array",
                "minItems": 1,
                "items": action_item
            },
            "kind": { "type": "string", "minLength": 1 },
            "action_kind": { "type": "string", "minLength": 1 },
            "node_id": templated_uuid_schema(),
            "target_node_id": templated_uuid_schema(),
            "node_ref": node_ref,
            "node": node_obj,
            "metric": { "type": "string", "minLength": 1 },
            "metric_key": { "type": "string", "minLength": 1 },
            "value": {},
            "value_text": { "type": "string" },
            "ts": { "type": "string" },
            "properties": { "type": "object" },
            "property": { "type": "string", "minLength": 1 },
            "key": { "type": "string", "minLength": 1 },
            "target": { "type": "string" },
            "channel": { "type": "string" },
            "payload": {},
            "message": { "type": "string" },
            "dry_run": templated_bool_schema()
        },
        "anyOf": [
            { "required": ["actions"] },
            { "required": ["kind"] },
            { "required": ["action_kind"] },
            { "required": ["dry_run"] }
        ]
    })
}

fn node_object_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["type_name", "node_key"],
        "properties": {
            "type_name": { "type": "string", "minLength": 1 },
            "node_key": { "type": "string", "minLength": 1 }
        },
        "additionalProperties": false
    })
}

fn node_ref_schema() -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            { "type": "string", "minLength": 1 },
            node_object_schema()
        ]
    })
}

/// Checks whether `config` contains any `WORK_ITEM` action (deferred to Step 14 per `07-STEP07-08` §07.4).
#[must_use]
pub fn contains_unsupported_work_item(config: &serde_json::Value) -> bool {
    let check_item = |item: &serde_json::Value| -> bool {
        item.get("kind")
            .or_else(|| item.get("action_kind"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("WORK_ITEM"))
    };

    if check_item(config) {
        return true;
    }
    config
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|arr| arr.iter().any(check_item))
}

/// `ACTION_EMIT` operator implementation generic over [`FlowRepository`], [`GraphStorePort`], and [`MetricStorePort`].
pub struct ActionEmitOperator<F, G, M> {
    flow_repo: Arc<F>,
    graph_store: Arc<G>,
    metric_store: Arc<M>,
}

impl<F, G, M> ActionEmitOperator<F, G, M>
where
    F: FlowRepository + 'static,
    G: GraphStorePort + 'static,
    M: MetricStorePort + 'static,
{
    /// Constructs a new [`ActionEmitOperator`].
    #[must_use]
    pub const fn new(flow_repo: Arc<F>, graph_store: Arc<G>, metric_store: Arc<M>) -> Self {
        Self {
            flow_repo,
            graph_store,
            metric_store,
        }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }
        if contains_unsupported_work_item(&ctx.config) {
            return Err(AppError::Unsupported("UNSUPPORTED_FORMAT"));
        }

        let node_dry_run = parse_optional_bool(&ctx.config, "dry_run")?.unwrap_or(false);
        let action_items = extract_action_items(&ctx.config)?;

        let mut action_ids = Vec::with_capacity(action_items.len());
        let mut statuses = Vec::with_capacity(action_items.len());

        for (index, item) in action_items.iter().enumerate() {
            if ctx.is_canceled() {
                return Err(AppError::Conflict("RUN_CANCELED"));
            }
            let idem_key = format!("{}:{}:{index}", ctx.run_id, ctx.node_id);

            if let Some(existing) = self
                .flow_repo
                .get_action_log_by_key(&ctx.tenant_id, &idem_key)
                .await?
                && matches!(
                    existing.status,
                    ActionLogStatus::Done | ActionLogStatus::DryRun
                )
            {
                action_ids.push(existing.id);
                statuses.push(existing.status);
                continue;
            }

            let item_dry = parse_optional_bool(item, "dry_run")?.unwrap_or(false);
            let is_dry_run = ctx.environment == Environment::Test || node_dry_run || item_dry;
            let rec = self
                .execute_single_action(ctx, item, &idem_key, is_dry_run)
                .await?;
            action_ids.push(rec.id);
            statuses.push(rec.status);
        }

        Ok(OperatorOutcome::Completed(serde_json::json!({
            "actions_total": action_ids.len(),
            "dry_run": ctx.environment == Environment::Test || node_dry_run,
            "action_ids": action_ids,
            "statuses": statuses,
        })))
    }

    async fn execute_single_action(
        &self,
        ctx: &OperatorContext,
        item: &serde_json::Value,
        idem_key: &str,
        is_dry_run: bool,
    ) -> Result<axpoint_domain::flow::ActionLogRecord, AppError> {
        let kind_str = parse_optional_string_any(item, &["kind", "action_kind"])?
            .unwrap_or_else(|| String::from("NOTIFY"));
        let (action_kind, target) = classify_action_and_target(&kind_str, item)?;

        if is_dry_run {
            return self
                .flow_repo
                .record_action_log(
                    &ctx.tenant_id,
                    NewActionLogInput {
                        run_id: Some(ctx.run_id),
                        node_id: Some(ctx.node_id.clone()),
                        source_kind: ActionSourceKind::Flow,
                        action_kind,
                        target,
                        payload: item.clone(),
                        status: ActionLogStatus::DryRun,
                        idempotency_key: String::from(idem_key),
                        error_message: None,
                    },
                )
                .await
                .map(|(rec, _)| rec);
        }

        let exec_res = match action_kind {
            ActionKind::WriteNodeMetric => self.apply_write_metric(&ctx.tenant_id, item).await,
            ActionKind::WriteNodeProperty => self.apply_write_property(&ctx.tenant_id, item).await,
            ActionKind::Notify => Ok(()),
            ActionKind::WorkItem | ActionKind::Http => {
                Err(AppError::Unsupported("UNSUPPORTED_FORMAT"))
            }
        };

        match exec_res {
            Ok(()) => self
                .flow_repo
                .record_action_log(
                    &ctx.tenant_id,
                    NewActionLogInput {
                        run_id: Some(ctx.run_id),
                        node_id: Some(ctx.node_id.clone()),
                        source_kind: ActionSourceKind::Flow,
                        action_kind,
                        target,
                        payload: item.clone(),
                        status: ActionLogStatus::Done,
                        idempotency_key: String::from(idem_key),
                        error_message: None,
                    },
                )
                .await
                .map(|(rec, _)| rec),
            Err(err) => {
                if let Err(log_err) = self
                    .flow_repo
                    .record_action_log(
                        &ctx.tenant_id,
                        NewActionLogInput {
                            run_id: Some(ctx.run_id),
                            node_id: Some(ctx.node_id.clone()),
                            source_kind: ActionSourceKind::Flow,
                            action_kind,
                            target,
                            payload: item.clone(),
                            status: ActionLogStatus::Failed,
                            idempotency_key: String::from(idem_key),
                            error_message: Some(err.client_message()),
                        },
                    )
                    .await
                {
                    tracing::warn!(error = %log_err, "failed to record FAILED ax_action_log row");
                }
                Err(err)
            }
        }
    }

    async fn apply_write_metric(
        &self,
        tenant_id: &str,
        item: &serde_json::Value,
    ) -> Result<(), AppError> {
        let (mut node_id, mut node_ref) = parse_node_selector(item)?;
        if node_id.is_none() && node_ref.is_none() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "WRITE_NODE_METRIC requires 'node_id', 'node_ref', or 'node'",
            ));
        }
        if node_id.is_none()
            && let Some(ref nr) = node_ref
            && let Ok(resolved_id) = self.resolve_node_ref(tenant_id, nr).await
        {
            node_id = Some(resolved_id);
            node_ref = None;
        }

        let metric = parse_required_string_any(item, &["metric", "metric_key"])?;
        let value = parse_optional_f64(item, "value")?;
        let value_text = parse_optional_string_any(item, &["value_text"])?;
        if value.is_none() && value_text.is_none() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "WRITE_NODE_METRIC requires 'value' or 'value_text'",
            ));
        }

        let ts = match parse_optional_string_any(item, &["ts"])? {
            Some(raw_ts) => DateTime::parse_from_rfc3339(&raw_ts)
                .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid ts: {err}")))?
                .with_timezone(&Utc),
            None => Utc::now(),
        };

        let _ = self
            .metric_store
            .write_metrics(
                tenant_id,
                WriteMetricsRequest {
                    points: vec![MetricPointInput {
                        node_id,
                        node: node_ref,
                        metric,
                        ts,
                        value,
                        value_text,
                    }],
                    source_tag: MetricSourceTag::Action,
                    batch_id: None,
                },
            )
            .await?;
        Ok(())
    }

    async fn apply_write_property(
        &self,
        tenant_id: &str,
        item: &serde_json::Value,
    ) -> Result<(), AppError> {
        let (node_id_opt, node_ref_opt) = parse_node_selector(item)?;
        let node_id = match (node_id_opt, node_ref_opt) {
            (Some(id), _) => id,
            (None, Some(nr)) => self.resolve_node_ref(tenant_id, &nr).await?,
            (None, None) => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "WRITE_NODE_PROPERTY requires 'node_id', 'node_ref', or 'node'",
                ));
            }
        };

        let props = if let Some(serde_json::Value::Object(map)) = item.get("properties") {
            map.clone()
        } else {
            let key = parse_required_string_any(item, &["property", "key"])?;
            let val = item
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let mut map = serde_json::Map::new();
            map.insert(key, val);
            map
        };

        let _ = self
            .graph_store
            .patch_node(
                tenant_id,
                node_id,
                PatchNodeRequest {
                    properties: Some(props),
                    ..PatchNodeRequest::default()
                },
            )
            .await?;
        Ok(())
    }

    async fn resolve_node_ref(&self, tenant_id: &str, nr: &NodeRef) -> Result<Uuid, AppError> {
        if let Ok(resp) = self
            .graph_store
            .list_nodes(
                tenant_id,
                NodeListQuery {
                    type_name: Some(nr.type_name.clone()),
                    q: Some(nr.node_key.clone()),
                    bbox: None,
                    limit: Some(200),
                    cursor: None,
                    reveal_pii: false,
                },
            )
            .await
            && let Some(found_id) = find_matching_node(&resp, nr)
        {
            return Ok(found_id);
        }

        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let resp = self
                .graph_store
                .list_nodes(
                    tenant_id,
                    NodeListQuery {
                        type_name: Some(nr.type_name.clone()),
                        q: None,
                        bbox: None,
                        limit: Some(500),
                        cursor: cursor.clone(),
                        reveal_pii: false,
                    },
                )
                .await?;
            if let Some(found_id) = find_matching_node(&resp, nr) {
                return Ok(found_id);
            }
            match resp.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => break,
            }
        }

        Err(AppError::NotFound("NODE_NOT_FOUND"))
    }
}

fn find_matching_node(resp: &NodeListResponse, nr: &NodeRef) -> Option<Uuid> {
    resp.items
        .iter()
        .chain(resp.nodes.iter())
        .find(|n| n.type_name == nr.type_name && n.node_key == nr.node_key)
        .map(|n| n.id)
}

enum FlexibleNodeRef {
    Id(Uuid),
    Key(NodeRef),
    Empty,
}

fn parse_node_selector(
    item: &serde_json::Value,
) -> Result<(Option<Uuid>, Option<NodeRef>), AppError> {
    let mut node_id = parse_optional_uuid_any(item, &["node_id", "target_node_id"])?;
    let mut node_ref: Option<NodeRef> = match item.get("node") {
        Some(v) if !v.is_null() => Some(parse_node_ref_object(v, "node")?),
        _ => None,
    };

    if let Some(raw_ref) = item.get("node_ref")
        && !raw_ref.is_null()
    {
        match parse_flexible_node_ref(raw_ref)? {
            FlexibleNodeRef::Id(uid) => {
                if node_id.is_none() {
                    node_id = Some(uid);
                }
            }
            FlexibleNodeRef::Key(nr) => {
                if node_ref.is_none() {
                    node_ref = Some(nr);
                }
            }
            FlexibleNodeRef::Empty => {}
        }
    }

    Ok((node_id, node_ref))
}

fn parse_flexible_node_ref(val: &serde_json::Value) -> Result<FlexibleNodeRef, AppError> {
    match val {
        serde_json::Value::Null => Ok(FlexibleNodeRef::Empty),
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(FlexibleNodeRef::Empty);
            }
            if let Ok(uid) = Uuid::parse_str(trimmed) {
                return Ok(FlexibleNodeRef::Id(uid));
            }
            let Some((raw_type, raw_key)) = trimmed.split_once(':') else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid node_ref '{trimmed}'; expected UUID or 'TypeName:node_key'"),
                ));
            };
            let type_name = raw_type.trim();
            let node_key = raw_key.trim();
            if type_name.is_empty() || node_key.is_empty() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!(
                        "invalid node_ref '{trimmed}'; type_name and node_key must be non-empty"
                    ),
                ));
            }
            Ok(FlexibleNodeRef::Key(NodeRef {
                type_name: String::from(type_name),
                node_key: String::from(node_key),
            }))
        }
        serde_json::Value::Object(_) => {
            parse_node_ref_object(val, "node_ref").map(FlexibleNodeRef::Key)
        }
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            "node_ref must be a UUID string, 'TypeName:node_key', or {type_name, node_key} object",
        )),
    }
}

fn parse_node_ref_object(val: &serde_json::Value, field: &str) -> Result<NodeRef, AppError> {
    let parsed: NodeRef = serde_json::from_value(val.clone())
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid {field}: {err}")))?;
    let type_name = parsed.type_name.trim();
    let node_key = parsed.node_key.trim();
    if type_name.is_empty() || node_key.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("{field}.type_name and {field}.node_key must be non-empty"),
        ));
    }
    Ok(NodeRef {
        type_name: String::from(type_name),
        node_key: String::from(node_key),
    })
}

fn extract_action_items(config: &serde_json::Value) -> Result<Vec<serde_json::Value>, AppError> {
    if let Some(arr) = config.get("actions").and_then(serde_json::Value::as_array) {
        if arr.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "ACTION_EMIT actions array cannot be empty",
            ));
        }
        return Ok(arr.clone());
    }
    if config.is_object() {
        return Ok(vec![config.clone()]);
    }
    Err(AppError::bad(
        "INVALID_ARGUMENT",
        "ACTION_EMIT config must be an object",
    ))
}

fn extract_target(item: &serde_json::Value, fallback: &str) -> Result<String, AppError> {
    let (node_id, node_ref) = parse_node_selector(item)?;
    if let Some(id) = node_id {
        return Ok(id.to_string());
    }
    if let Some(nr) = node_ref {
        return Ok(format!("{}:{}", nr.type_name, nr.node_key));
    }
    Ok(parse_optional_string_any(item, &["target"])?.unwrap_or_else(|| String::from(fallback)))
}

fn classify_action_and_target(
    kind_str: &str,
    item: &serde_json::Value,
) -> Result<(ActionKind, String), AppError> {
    match kind_str.trim().to_ascii_uppercase().as_str() {
        "WRITE_NODE_METRIC" => {
            let target = extract_target(item, "node_ref")?;
            Ok((ActionKind::WriteNodeMetric, target))
        }
        "WRITE_NODE_PROPERTY" => {
            let target = extract_target(item, "node")?;
            Ok((ActionKind::WriteNodeProperty, target))
        }
        "NOTIFY" => {
            let target = parse_optional_string_any(item, &["target", "channel"])?
                .unwrap_or_else(|| String::from("notify"));
            Ok((ActionKind::Notify, target))
        }
        "WORK_ITEM" => Err(AppError::Unsupported("UNSUPPORTED_FORMAT")),
        other => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unsupported ACTION_EMIT kind '{other}'"),
        )),
    }
}

impl<F, G, M> Operator for ActionEmitOperator<F, G, M>
where
    F: FlowRepository + 'static,
    G: GraphStorePort + 'static,
    M: MetricStorePort + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::ActionEmit
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
