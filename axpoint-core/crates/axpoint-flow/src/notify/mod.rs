//! 5-channel notification dispatcher (`WEBHOOK`, `EMAIL`, `WECOM`, `DINGTALK`, `FEISHU`),
//! silence suppression, SSRF guard, dual proxy/loopback HTTP clients, and 5-attempt backoff (`07-STEP07-08` §08.4).

pub mod dingtalk;
pub mod email;
pub mod feishu;
pub mod template;
pub mod webhook;
pub mod wecom;

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::{NotifyKind, OutboxTopic};
use axpoint_domain::platform::{BoxFuture, OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{FlowCredentialUnsealPort, OutboundUrlGuardPort, RulesRepository};
use axpoint_domain::rules::{
    AlertEventKind, AlertRecord, DeliveryFailureUpdate, EnqueueDeliveryInput, NotifyChannelRecord,
    NotifyDeliveryEvent, NotifyDeliveryRecord, NotifyDeliveryStatus, RuleDef, TestChannelResponse,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

/// Per-attempt HTTP and SMTP network timeout in seconds (`07-STEP07-08` §08.4).
pub const NOTIFY_HTTP_TIMEOUT_SECS: u64 = 10;

/// Maximum notification delivery attempts (`1` initial + `4` retries = `5` attempts total).
pub const MAX_NOTIFY_ATTEMPTS: i32 = 5;

/// Borrowed notification payload and delivery metadata passed to concrete channel senders.
#[derive(Debug, Clone, Copy)]
pub struct NotifyPayloadView<'a> {
    /// Owning tenant identifier.
    pub tenant_id: &'a str,
    /// Persisted `ax_notify_deliveries.id`.
    pub delivery_id: Uuid,
    /// Delivery event kind (`OPENED`, `ESCALATED`, `RESOLVED`, `TEST`, `ACTION`).
    pub event: NotifyDeliveryEvent,
    /// Rendered notification title.
    pub title: &'a str,
    /// Rendered notification body.
    pub body: &'a str,
    /// Associated alert record, if bound to an alert.
    pub alert: Option<&'a AlertRecord>,
    /// Associated rule definition, if bound to a rule.
    pub rule: Option<&'a RuleDef>,
    /// Timestamp used for headers and cryptographic signatures.
    pub now: DateTime<Utc>,
}

#[derive(Debug)]
pub(crate) struct ChannelFailure {
    pub(crate) response_code: Option<i32>,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    pub(crate) guard_error: Option<AppError>,
}

impl ChannelFailure {
    pub(crate) fn retryable(response_code: Option<i32>, message: String) -> Self {
        Self {
            response_code,
            message,
            retryable: true,
            guard_error: None,
        }
    }

    pub(crate) fn permanent(err: AppError) -> Self {
        Self {
            response_code: None,
            message: err.to_string(),
            retryable: false,
            guard_error: Some(err),
        }
    }

    pub(crate) fn from_guard_error(err: AppError) -> Self {
        Self::permanent(err)
    }

    pub(crate) fn from_reqwest(err: &reqwest::Error) -> Self {
        let code = err.status().map(|s| i32::from(s.as_u16()));
        let msg = if err.is_timeout() {
            String::from("notification HTTP request timed out after 10s")
        } else {
            format!("notification HTTP request failed: {err}")
        };
        Self::retryable(code, msg)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HttpClientPair<'a> {
    pub(crate) external: &'a reqwest::Client,
    pub(crate) loopback: &'a reqwest::Client,
}

impl<'a> HttpClientPair<'a> {
    pub(crate) fn select(self, url: &str, allow_private_network: bool) -> &'a reqwest::Client {
        if allow_private_network && is_loopback_url(url) {
            self.loopback
        } else {
            self.external
        }
    }
}

/// Computes the retry delay after `completed_attempt` (`1 -> 30s`, `2 -> 2m`, `3 -> 10m`, `4 -> 30m`, `5+ -> None`).
#[must_use]
pub const fn delivery_retry_delay(completed_attempt: i32) -> Option<ChronoDuration> {
    match completed_attempt {
        1 => Some(ChronoDuration::seconds(30)),
        2 => Some(ChronoDuration::minutes(2)),
        3 => Some(ChronoDuration::minutes(10)),
        4 => Some(ChronoDuration::minutes(30)),
        _ => None,
    }
}

/// Returns `true` if `url_str` targets `localhost` or a loopback IP (`127.0.0.0/8`, `::1`).
#[must_use]
pub fn is_loopback_url(url_str: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url_str) else {
        return false;
    };
    let Some(raw_host) = parsed.host_str() else {
        return false;
    };
    let host = raw_host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// No-op [`FlowCredentialUnsealPort`] returning an empty JSON object when no secret keyring is wired.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopCredentialUnsealer;

impl FlowCredentialUnsealPort for NoopCredentialUnsealer {
    fn unseal_credential_json<'a>(
        &'a self,
        _tenant_id: &'a str,
        _credential_id: Uuid,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async { Ok(serde_json::json!({})) })
    }
}

/// Object-safe port notifying `ESCALATED` alerts immediately from [`crate::rules::RuleEvaluator`].
pub trait AlertEscalationNotifier: Send + Sync {
    /// Enqueues and dispatches `ESCALATED` notifications for `alert` and `rule`.
    fn notify_escalated<'a>(
        &'a self,
        tenant_id: &'a str,
        alert: &'a AlertRecord,
        rule: &'a RuleDef,
    ) -> BoxFuture<'a, Vec<NotifyDeliveryRecord>>;
}

/// Multi-channel notification dispatcher with silence suppression, SSRF protection, and 5-attempt backoff.
pub struct NotificationDispatcher<R, U, C = NoopCredentialUnsealer> {
    repo: Arc<R>,
    url_guard: Arc<U>,
    cred_unsealer: Arc<C>,
    external_http: reqwest::Client,
    loopback_http: reqwest::Client,
}

impl<R, U, C> NotificationDispatcher<R, U, C>
where
    R: RulesRepository + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    /// Constructs a new [`NotificationDispatcher`] with dual proxy-aware external and `.no_proxy()` loopback HTTP clients.
    #[must_use]
    pub fn new(repo: Arc<R>, url_guard: Arc<U>, cred_unsealer: Arc<C>) -> Self {
        let external_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(NOTIFY_HTTP_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let loopback_http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(NOTIFY_HTTP_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            repo,
            url_guard,
            cred_unsealer,
            external_http,
            loopback_http,
        }
    }

    /// Selects the `.no_proxy()` client for loopback targets when `allow_private_network` is `true`,
    /// or the standard proxy-aware client for external endpoints.
    #[must_use]
    pub fn select_http_client(&self, url: &str, allow_private_network: bool) -> &reqwest::Client {
        if allow_private_network && is_loopback_url(url) {
            &self.loopback_http
        } else {
            &self.external_http
        }
    }

    /// Enqueues and executes initial delivery attempts for an alert lifecycle event (`OPENED`, `ESCALATED`, `RESOLVED`).
    ///
    /// Suppresses `OPENED` and `ESCALATED` notifications when an active silence matches, recording `SILENCED`.
    ///
    /// # Errors
    /// Returns [`AppError`] if silence lookup or delivery persistence fails.
    pub async fn dispatch_alert_event(
        &self,
        tenant_id: &str,
        alert: &AlertRecord,
        rule: &RuleDef,
        event: NotifyDeliveryEvent,
    ) -> Result<Vec<NotifyDeliveryRecord>, AppError> {
        let channel_ids = match event {
            NotifyDeliveryEvent::Escalated => &rule.escalation_channel_ids,
            NotifyDeliveryEvent::Opened | NotifyDeliveryEvent::Resolved => &rule.channel_ids,
            NotifyDeliveryEvent::Test | NotifyDeliveryEvent::Action => return Ok(Vec::new()),
        };
        if channel_ids.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now();
        if matches!(
            event,
            NotifyDeliveryEvent::Opened | NotifyDeliveryEvent::Escalated
        ) && let Some(sil) = self.find_matching_silence(tenant_id, alert, now).await?
        {
            let _ = self
                .repo
                .append_alert_event(
                    tenant_id,
                    alert.id,
                    AlertEventKind::Silenced,
                    serde_json::json!({
                        "silence_id": sil.id,
                        "event": event.as_str(),
                        "reason": sil.reason,
                    }),
                    None,
                )
                .await?;
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(channel_ids.len());
        for &channel_id in channel_ids {
            let (rec, existing) = self
                .repo
                .enqueue_delivery(
                    tenant_id,
                    EnqueueDeliveryInput {
                        alert_id: Some(alert.id),
                        channel_id,
                        event,
                        now,
                    },
                )
                .await?;
            if existing && rec.status != NotifyDeliveryStatus::Pending {
                results.push(rec);
                continue;
            }
            if let Some(updated) = self.deliver_single(tenant_id, rec.id).await? {
                results.push(updated);
            }
        }
        Ok(results)
    }

    /// Executes one attempt for a single `PENDING` `delivery_id`, updating its persistence status and backoff schedule.
    ///
    /// # Errors
    /// Returns [`AppError`] on storage errors; remote channel errors are recorded on [`NotifyDeliveryRecord`].
    pub async fn deliver_single(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
    ) -> Result<Option<NotifyDeliveryRecord>, AppError> {
        let Some(delivery) = self.repo.get_delivery(tenant_id, delivery_id).await? else {
            return Ok(None);
        };
        if delivery.status != NotifyDeliveryStatus::Pending {
            return Ok(Some(delivery));
        }

        let next_attempt = delivery.attempt.saturating_add(1);
        let Some((channel, _sealed)) = self
            .repo
            .get_channel(tenant_id, delivery.channel_id)
            .await?
        else {
            let failed = self
                .repo
                .mark_delivery_failed(
                    tenant_id,
                    delivery.id,
                    DeliveryFailureUpdate {
                        attempt: next_attempt,
                        response_code: None,
                        error_message: String::from("notification channel not found"),
                        next_retry_at: None,
                    },
                )
                .await?;
            return Ok(Some(failed));
        };

        if !channel.enabled {
            let failed = self
                .repo
                .mark_delivery_failed(
                    tenant_id,
                    delivery.id,
                    DeliveryFailureUpdate {
                        attempt: next_attempt,
                        response_code: None,
                        error_message: String::from("notification channel is disabled"),
                        next_retry_at: None,
                    },
                )
                .await?;
            return Ok(Some(failed));
        }

        let (alert_opt, rule_opt) = self
            .load_alert_and_rule(tenant_id, delivery.alert_id)
            .await?;
        let (title, body) = build_delivery_title_body(delivery.event, alert_opt.as_ref());
        let now = Utc::now();
        let view = NotifyPayloadView {
            tenant_id,
            delivery_id: delivery.id,
            event: delivery.event,
            title: &title,
            body: &body,
            alert: alert_opt.as_ref(),
            rule: rule_opt.as_ref(),
            now,
        };

        match self.send_via_channel(tenant_id, &channel, &view).await {
            Ok(code) => {
                let sent = self
                    .repo
                    .mark_delivery_sent(tenant_id, delivery.id, next_attempt, Some(code))
                    .await?;
                Ok(Some(sent))
            }
            Err(fail) => {
                let next_retry_at = if fail.retryable && next_attempt < MAX_NOTIFY_ATTEMPTS {
                    delivery_retry_delay(next_attempt).map(|d| now + d)
                } else {
                    None
                };
                let updated = self
                    .repo
                    .mark_delivery_failed(
                        tenant_id,
                        delivery.id,
                        DeliveryFailureUpdate {
                            attempt: next_attempt,
                            response_code: fail.response_code,
                            error_message: fail.message,
                            next_retry_at,
                        },
                    )
                    .await?;
                Ok(Some(updated))
            }
        }
    }

    /// Polls `ax_due_notify_deliveries(limit)` across tenants and executes due retry attempts.
    ///
    /// # Errors
    /// Returns [`AppError`] if querying due deliveries fails.
    pub async fn poll_due_deliveries(&self, limit: i32) -> Result<usize, AppError> {
        let due = self.repo.list_due_deliveries(limit).await?;
        let mut processed: usize = 0;
        for item in due {
            match self.deliver_single(&item.tenant_id, item.delivery_id).await {
                Ok(_) => processed = processed.saturating_add(1),
                Err(err) => tracing::warn!(
                    tenant_id = %item.tenant_id,
                    delivery_id = %item.delivery_id,
                    error = %err,
                    "due notification delivery attempt failed"
                ),
            }
        }
        Ok(processed)
    }

    /// Synchronously sends a `TEST` probe notification to `channel_id` (`POST /api/axflow/channels/{id}/test`).
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] if the channel does not exist, [`AppError::Forbidden`] if blocked by `NetGuard`,
    /// or [`AppError::BadRequest`] if channel configuration is invalid.
    pub async fn test_channel(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
    ) -> Result<TestChannelResponse, AppError> {
        let (channel, _sealed) = self
            .repo
            .get_channel(tenant_id, channel_id)
            .await?
            .ok_or(AppError::NotFound("CHANNEL_NOT_FOUND"))?;

        let now = Utc::now();
        let (rec, _) = self
            .repo
            .enqueue_delivery(
                tenant_id,
                EnqueueDeliveryInput {
                    alert_id: None,
                    channel_id,
                    event: NotifyDeliveryEvent::Test,
                    now,
                },
            )
            .await?;

        let title = format!("[AxPoint] 通知渠道连通性测试 ({})", channel.name);
        let body = format!(
            "渠道 `{}` ({}) 连通性测试消息，发送时间：{}",
            channel.name,
            channel.kind.as_str(),
            now.to_rfc3339()
        );
        let view = NotifyPayloadView {
            tenant_id,
            delivery_id: rec.id,
            event: NotifyDeliveryEvent::Test,
            title: &title,
            body: &body,
            alert: None,
            rule: None,
            now,
        };

        match self.send_via_channel(tenant_id, &channel, &view).await {
            Ok(code) => {
                let delivery = self
                    .repo
                    .mark_delivery_sent(tenant_id, rec.id, 1, Some(code))
                    .await?;
                Ok(TestChannelResponse { ok: true, delivery })
            }
            Err(fail) => {
                let delivery = self
                    .repo
                    .mark_delivery_failed(
                        tenant_id,
                        rec.id,
                        DeliveryFailureUpdate {
                            attempt: 1,
                            response_code: fail.response_code,
                            error_message: fail.message,
                            next_retry_at: None,
                        },
                    )
                    .await?;
                if let Some(guard_err) = fail.guard_error {
                    return Err(guard_err);
                }
                Ok(TestChannelResponse {
                    ok: false,
                    delivery,
                })
            }
        }
    }

    /// Sends an `ACTION` notification emitted by an `AxFlow` `ACTION_EMIT(NOTIFY)` node.
    ///
    /// # Errors
    /// Returns [`AppError`] if the channel is missing, SSRF-blocked, or persistence fails.
    pub async fn send_action_notify(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
        title: &str,
        body: &str,
    ) -> Result<NotifyDeliveryRecord, AppError> {
        let (channel, _sealed) = self
            .repo
            .get_channel(tenant_id, channel_id)
            .await?
            .ok_or(AppError::NotFound("CHANNEL_NOT_FOUND"))?;

        let now = Utc::now();
        let (rec, _) = self
            .repo
            .enqueue_delivery(
                tenant_id,
                EnqueueDeliveryInput {
                    alert_id: None,
                    channel_id,
                    event: NotifyDeliveryEvent::Action,
                    now,
                },
            )
            .await?;

        let view = NotifyPayloadView {
            tenant_id,
            delivery_id: rec.id,
            event: NotifyDeliveryEvent::Action,
            title,
            body,
            alert: None,
            rule: None,
            now,
        };

        match self.send_via_channel(tenant_id, &channel, &view).await {
            Ok(code) => {
                self.repo
                    .mark_delivery_sent(tenant_id, rec.id, 1, Some(code))
                    .await
            }
            Err(fail) => {
                let next_retry_at = if fail.retryable {
                    delivery_retry_delay(1).map(|d| now + d)
                } else {
                    None
                };
                let updated = self
                    .repo
                    .mark_delivery_failed(
                        tenant_id,
                        rec.id,
                        DeliveryFailureUpdate {
                            attempt: 1,
                            response_code: fail.response_code,
                            error_message: fail.message,
                            next_retry_at,
                        },
                    )
                    .await?;
                if let Some(guard_err) = fail.guard_error {
                    return Err(guard_err);
                }
                Ok(updated)
            }
        }
    }

    /// Runs a periodic due-delivery retry worker until `shutdown_rx` signals `true`.
    pub async fn run_retry_loop(
        &self,
        poll_interval: Duration,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut ticker = tokio::time::interval(poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while !*shutdown_rx.borrow_and_update() {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shutdown_rx.changed() => break,
            }
            if let Err(err) = self.poll_due_deliveries(100).await {
                tracing::warn!(error = %err, "NotificationDispatcher retry poll failed");
            }
        }
    }

    async fn find_matching_silence(
        &self,
        tenant_id: &str,
        alert: &AlertRecord,
        at: DateTime<Utc>,
    ) -> Result<Option<axpoint_domain::rules::AlertSilenceRecord>, AppError> {
        let silences = self.repo.list_active_silences(tenant_id, at).await?;
        let type_name = alert
            .detail
            .get("type_name")
            .or_else(|| alert.detail.get("node_type_name"))
            .and_then(serde_json::Value::as_str);
        Ok(silences.into_iter().find(|s| {
            s.matcher
                .matches(alert.rule_id, alert.severity, alert.node_id, type_name)
        }))
    }

    async fn load_alert_and_rule(
        &self,
        tenant_id: &str,
        alert_id: Option<Uuid>,
    ) -> Result<(Option<AlertRecord>, Option<RuleDef>), AppError> {
        let Some(aid) = alert_id else {
            return Ok((None, None));
        };
        let Some(alert) = self.repo.get_alert(tenant_id, aid).await? else {
            return Ok((None, None));
        };
        let rule = self.repo.get_rule(tenant_id, alert.rule_id).await?;
        Ok((Some(alert), rule))
    }

    async fn send_via_channel(
        &self,
        tenant_id: &str,
        channel: &NotifyChannelRecord,
        view: &NotifyPayloadView<'_>,
    ) -> Result<i32, ChannelFailure> {
        let secrets = self
            .load_channel_secrets(tenant_id, channel.credential_id)
            .await?;
        let clients = HttpClientPair {
            external: &self.external_http,
            loopback: &self.loopback_http,
        };
        match channel.kind {
            NotifyKind::Webhook => {
                webhook::send_webhook(channel, &secrets, view, &*self.url_guard, clients).await
            }
            NotifyKind::Email => email::send_email(channel, &secrets, view, &*self.url_guard).await,
            NotifyKind::Wecom => {
                wecom::send_wecom(channel, &secrets, view, &*self.url_guard, clients).await
            }
            NotifyKind::Dingtalk => {
                dingtalk::send_dingtalk(channel, &secrets, view, &*self.url_guard, clients).await
            }
            NotifyKind::Feishu => {
                feishu::send_feishu(channel, &secrets, view, &*self.url_guard, clients).await
            }
        }
    }

    async fn load_channel_secrets(
        &self,
        tenant_id: &str,
        credential_id: Option<Uuid>,
    ) -> Result<BTreeMap<String, String>, ChannelFailure> {
        let Some(cred_id) = credential_id else {
            return Ok(BTreeMap::new());
        };
        let val = self
            .cred_unsealer
            .unseal_credential_json(tenant_id, cred_id)
            .await
            .map_err(ChannelFailure::permanent)?;
        let mut map = BTreeMap::new();
        if let Some(obj) = val.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    map.insert(k.clone(), String::from(s));
                }
            }
        }
        Ok(map)
    }
}

impl<R, U, C> AlertEscalationNotifier for NotificationDispatcher<R, U, C>
where
    R: RulesRepository + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    fn notify_escalated<'a>(
        &'a self,
        tenant_id: &'a str,
        alert: &'a AlertRecord,
        rule: &'a RuleDef,
    ) -> BoxFuture<'a, Vec<NotifyDeliveryRecord>> {
        Box::pin(async move {
            self.dispatch_alert_event(tenant_id, alert, rule, NotifyDeliveryEvent::Escalated)
                .await
        })
    }
}

/// Outbox subscriber listening to `alert.opened` and `alert.resolved` events (`07-STEP07-08` §08.4).
pub struct AlertNotifySubscriber<R, U, C = NoopCredentialUnsealer> {
    dispatcher: Arc<NotificationDispatcher<R, U, C>>,
}

impl<R, U, C> AlertNotifySubscriber<R, U, C>
where
    R: RulesRepository + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    /// Constructs a new [`AlertNotifySubscriber`] wrapping `dispatcher`.
    #[must_use]
    pub const fn new(dispatcher: Arc<NotificationDispatcher<R, U, C>>) -> Self {
        Self { dispatcher }
    }

    /// Dispatches notifications for a single `alert.opened` or `alert.resolved` [`OutboxEvent`].
    ///
    /// # Errors
    /// Returns [`AppError`] if loading alert/rule metadata or persisting delivery records fails.
    pub async fn handle_event(
        &self,
        event: &OutboxEvent,
    ) -> Result<Vec<NotifyDeliveryRecord>, AppError> {
        let delivery_event = match event.topic {
            OutboxTopic::AlertOpened => NotifyDeliveryEvent::Opened,
            OutboxTopic::AlertResolved => NotifyDeliveryEvent::Resolved,
            _ => return Ok(Vec::new()),
        };
        let Some(alert_id) = event
            .payload
            .get("alert_id")
            .or_else(|| event.payload.get("id"))
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            return Ok(Vec::new());
        };
        let Some(alert) = self
            .dispatcher
            .repo
            .get_alert(&event.tenant_id, alert_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        let Some(rule) = self
            .dispatcher
            .repo
            .get_rule(&event.tenant_id, alert.rule_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        self.dispatcher
            .dispatch_alert_event(&event.tenant_id, &alert, &rule, delivery_event)
            .await
    }
}

impl<R, U, C> OutboxSubscriber for AlertNotifySubscriber<R, U, C>
where
    R: RulesRepository + 'static,
    U: OutboundUrlGuardPort + 'static,
    C: FlowCredentialUnsealPort + 'static,
{
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _ = self.handle_event(event).await?;
            Ok(())
        })
    }
}

fn build_delivery_title_body(
    event: NotifyDeliveryEvent,
    alert: Option<&AlertRecord>,
) -> (String, String) {
    let Some(a) = alert else {
        return (String::from("[AxPoint] Notification"), String::new());
    };
    let title = match event {
        NotifyDeliveryEvent::Resolved => format!("[已恢复] {}", a.title),
        NotifyDeliveryEvent::Escalated => format!("[已升级] {}", a.title),
        NotifyDeliveryEvent::Opened | NotifyDeliveryEvent::Test | NotifyDeliveryEvent::Action => {
            a.title.clone()
        }
    };
    (title, a.body.clone())
}

pub(crate) fn lookup_secret_or_config<'a>(
    secrets: &'a BTreeMap<String, String>,
    config: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a str> {
    for &key in keys {
        if let Some(val) = secrets.get(key).map(String::as_str).map(str::trim)
            && !val.is_empty()
        {
            return Some(val);
        }
    }
    for &key in keys {
        if let Some(val) = config
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            && !val.is_empty()
        {
            return Some(val);
        }
    }
    None
}

pub(crate) fn percent_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => {
                out.push('%');
                if let Some(hi) = char::from_digit(u32::from(other >> 4), 16) {
                    out.push(hi.to_ascii_uppercase());
                }
                if let Some(lo) = char::from_digit(u32::from(other & 0x0F), 16) {
                    out.push(lo.to_ascii_uppercase());
                }
            }
        }
    }
    out
}

pub(crate) fn append_query_param(base_url: &str, key: &str, encoded_val: &str) -> String {
    let sep = if base_url.contains('?') { '&' } else { '?' };
    format!("{base_url}{sep}{key}={encoded_val}")
}

pub(crate) async fn read_truncated_response_text(resp: reqwest::Response) -> String {
    match resp.bytes().await {
        Ok(bytes) => {
            let prefix: Vec<u8> = bytes.iter().take(512).copied().collect();
            String::from_utf8_lossy(&prefix).into_owned()
        }
        Err(err) => format!("<failed reading response body: {err}>"),
    }
}
