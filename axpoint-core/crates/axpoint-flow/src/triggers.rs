//! `ax_outbox` subscribers for `OUTBOX_EVENT` flow triggers and `approval.decided` resumption (`07-STEP07-08` §07.4–§07.5).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, OutboxTopic, TriggerType};
use axpoint_domain::flow::StartRunParams;
use axpoint_domain::platform::{BoxFuture, OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{CredentialStore, FlowRepository};
use uuid::Uuid;

use crate::engine::{FlowEngine, eval_json_guard_expr};

/// Outbox subscriber starting `PROD`-deployed flows configured with `TriggerType::OutboxEvent` (`07-STEP07-08` §07.5).
pub struct FlowOutboxTriggerSubscriber<R> {
    repo: Arc<R>,
}

impl<R> FlowOutboxTriggerSubscriber<R>
where
    R: FlowRepository + 'static,
{
    /// Constructs a new [`FlowOutboxTriggerSubscriber`].
    #[must_use]
    pub const fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    /// Processes a single [`OutboxEvent`], matching tenant-scoped `PROD` flows and evaluating optional `filter` guards.
    ///
    /// Returns the number of newly created runs (`existing == false`).
    ///
    /// # Errors
    /// Returns [`AppError`] if querying event flows or starting a matching run fails.
    pub async fn dispatch_event(&self, event: &OutboxEvent) -> Result<usize, AppError> {
        let candidates = self.repo.list_event_flows(event.topic.as_str()).await?;
        if candidates.is_empty() {
            return Ok(0);
        }

        let origin_flow_id = event
            .payload
            .get("source_flow_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok());

        let mut started: usize = 0;
        for item in candidates {
            if item.tenant_id != event.tenant_id {
                continue;
            }
            if origin_flow_id == Some(item.flow_id) {
                continue;
            }

            if let Some(filter_str) = item
                .trigger_cfg
                .filter
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                let matched = eval_json_guard_expr(filter_str, &event.payload)?;
                if !matched {
                    continue;
                }
            }

            let idem_key = format!("outbox:{}", event.id);
            let (_run, existing) = self
                .repo
                .start_flow_run(
                    &event.tenant_id,
                    StartRunParams {
                        flow_id: item.flow_id,
                        environment: Environment::Prod,
                        explicit_version: Some(item.version),
                        trigger_source: TriggerType::OutboxEvent,
                        trigger_payload: event.payload.clone(),
                        idempotency_key: Some(idem_key),
                        started_by: None,
                    },
                )
                .await?;
            if !existing {
                started = started.saturating_add(1);
            }
        }

        Ok(started)
    }
}

impl<R> OutboxSubscriber for FlowOutboxTriggerSubscriber<R>
where
    R: FlowRepository + 'static,
{
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _ = self.dispatch_event(event).await?;
            Ok(())
        })
    }
}

/// Outbox subscriber resuming or failing `WAITING_APPROVAL` flow steps upon `approval.decided` events (`07-STEP07-08` §07.4).
pub struct ApprovalDecidedSubscriber<R> {
    engine: Arc<FlowEngine<R>>,
}

impl<R> ApprovalDecidedSubscriber<R> {
    /// Constructs a new [`ApprovalDecidedSubscriber`] backed by `engine`.
    #[must_use]
    pub const fn new(engine: Arc<FlowEngine<R>>) -> Self {
        Self { engine }
    }
}

impl<R> OutboxSubscriber for ApprovalDecidedSubscriber<R>
where
    R: FlowRepository + CredentialStore + 'static,
{
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != OutboxTopic::ApprovalDecided {
                return Ok(());
            }
            self.engine.handle_approval_decided_event(event).await
        })
    }
}
