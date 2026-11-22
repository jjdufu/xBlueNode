//! Single-leader PostgreSQL lease supervisor (`ImGatewaySupervisor`), split-brain fencing,
//! inbound deduplication, one-time `axim_` challenge emission, principal hydration,
//! and multi-turn `AgentRuntime` execution (`07-STEP21` §21.2–§21.3).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axpoint_domain::AppError;
use axpoint_domain::agent::{AgentChatRequest, AgentChatResponse, AgentEvent, ConversationRecord};
use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, ImConnectorStatus, ImIdentityPolicy, ImPlatform,
    Permission, TokenKind, TokenProfile,
};
use axpoint_domain::im::{
    IM_BIND_CHALLENGE_TTL_SECS, IM_BIND_TOKEN_PREFIX, IM_MARKDOWN_MAX_CHUNK_BYTES,
    ImConnectorConfig, ImConnectorRecord, ImConnectorTestResponse, ImInboundMessage,
    ImOutboundMessage, NewImBindChallengeInput, hash_im_bind_token, split_markdown_chunks,
};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, ConnectionRepository, CredentialStore, FlowRepository,
    GraphQueryPort, GraphStorePort, ImGatewayRepository, MetricStorePort, OutboundUrlGuardPort,
    PrincipalHydrationPort, RulesRepository, StudioRepository,
};
use axpoint_security::{AuthPrincipal, CredentialKeyring};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{TimeDelta, Utc};
use futures::{SinkExt, StreamExt};
use tokio::sync::{Mutex, RwLock, Semaphore, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::chunker::ProgressThrottler;
use super::dingtalk_stream::{
    decode_dingtalk_frame, encode_dingtalk_ping, encode_dingtalk_reply, resolve_dingtalk_dial_plan,
};
use super::feishu_ws::{
    decode_feishu_frame, encode_feishu_ping, encode_feishu_reply, resolve_feishu_dial_plan,
};
use super::protocol::{DecodedPlatformFrame, ImWsStream, PlatformDialPlan, dial_websocket};
use super::wecom_bot::{
    decode_wecom_frame, encode_wecom_ping, encode_wecom_reply, resolve_wecom_dial_plan,
};
use crate::runtime::{AgentRuntime, ChannelChatCall, ImApprovalReplyPort};

/// Single-leader PostgreSQL lease duration in seconds (`30s`, `07-STEP21` §21.2.2).
pub const IM_LEASE_DURATION_SECS: i32 = axpoint_domain::im::IM_LEASE_DURATION_SECS;

/// Worker lease heartbeat and status report interval (`10s`, `07-STEP21` §21.2.2).
pub const IM_HEARTBEAT_INTERVAL: Duration =
    Duration::from_secs(axpoint_domain::im::IM_HEARTBEAT_INTERVAL_SECS);

/// Supervisor scan interval for unclaimed or expired connector leases (`3s` -> `< 45s` failover).
pub const IM_SUPERVISOR_SCAN_INTERVAL: Duration = Duration::from_secs(3);

/// Default maximum outbound IM connectors leased by a single Core replica.
pub const DEFAULT_MAX_IM_SLOTS: i32 = 64;

const MAX_INBOUND_CONCURRENCY_PER_CONNECTOR: usize = 8;

/// Dependency bundle passed to [`ImGatewaySupervisor::new`].
pub struct ImGatewayDeps<R> {
    /// Tenant-scoped storage repository (`PgStore` in production).
    pub store: Arc<R>,
    /// Autonomous multi-turn `AxAgent` runtime.
    pub runtime: Arc<AgentRuntime<R>>,
    /// AES-256-GCM credential keyring used to decrypt `ax_im_connectors.credential_id`.
    pub keyring: Arc<CredentialKeyring>,
    /// Outbound URL & DNS SSRF guard (`NetGuard` in production).
    pub url_guard: Arc<dyn OutboundUrlGuardPort>,
    /// Optional explicit replica holder identifier (defaults to `core-<uuid>`).
    pub holder_id: Option<String>,
    /// Optional default workbench base URL for `/agents?im_bind=...` and `/flow?approval_id=...` links.
    pub default_web_base_url: Option<String>,
}

struct ActiveWorkerHandle {
    cancel: CancellationToken,
    fingerprint: String,
    handle: JoinHandle<()>,
}

/// Outbound enterprise IM WebSocket/Stream single-leader supervisor (`07-STEP21` §21.2).
pub struct ImGatewaySupervisor<R> {
    store: Arc<R>,
    runtime: Arc<AgentRuntime<R>>,
    keyring: Arc<CredentialKeyring>,
    url_guard: Arc<dyn OutboundUrlGuardPort>,
    holder_id: String,
    max_slots: i32,
    default_web_base_url: Option<String>,
    workers: Mutex<HashMap<Uuid, ActiveWorkerHandle>>,
    active_out_senders: RwLock<HashMap<Uuid, mpsc::Sender<ImOutboundMessage>>>,
}

impl<R> ImGatewaySupervisor<R> {
    /// Returns this replica's unique lease holder identifier.
    #[must_use]
    pub fn holder_id(&self) -> &str {
        &self.holder_id
    }
}

impl<R> ImApprovalReplyPort for ImGatewaySupervisor<R>
where
    R: Send + Sync + 'static,
{
    fn push_resumed_im_reply<'a>(
        &'a self,
        conversation: &'a ConversationRecord,
        response: &'a AgentChatResponse,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (Some(conn_id), Some(chat_id)) = (
                conversation.im_connector_id,
                conversation.external_chat_id.as_deref(),
            ) else {
                return Ok(());
            };
            let sender_opt = {
                let guard = self.active_out_senders.read().await;
                guard.get(&conn_id).cloned()
            };
            let Some(out_tx) = sender_opt else {
                return Ok(());
            };
            let chunks = format_response_chunks(response, self.default_web_base_url.as_deref());
            for chunk in chunks {
                let _closed = out_tx
                    .send(ImOutboundMessage {
                        chat_id: String::from(chat_id),
                        text: chunk,
                        reply_token: None,
                        stream_id: None,
                        is_final: true,
                    })
                    .await
                    .is_err();
            }
            Ok(())
        })
    }
}

impl<R> ImGatewaySupervisor<R>
where
    R: ImGatewayRepository
        + PrincipalHydrationPort
        + ConnectionRepository
        + AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + CredentialStore
        + 'static,
{
    /// Constructs a new [`ImGatewaySupervisor`] and registers it as the `approval.decided`
    /// IM reply sink on `deps.runtime`.
    #[must_use]
    pub fn new(deps: ImGatewayDeps<R>) -> Arc<Self> {
        let holder_id = deps
            .holder_id
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("core-{}", Uuid::new_v4()));
        let default_web_base_url = deps
            .default_web_base_url
            .map(|s| s.trim().trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty());

        let sup = Arc::new(Self {
            store: deps.store,
            runtime: deps.runtime,
            keyring: deps.keyring,
            url_guard: deps.url_guard,
            holder_id,
            max_slots: DEFAULT_MAX_IM_SLOTS,
            default_web_base_url,
            workers: Mutex::new(HashMap::new()),
            active_out_senders: RwLock::new(HashMap::new()),
        });
        let sink_concrete = Arc::clone(&sup);
        let sink_arc: Arc<dyn ImApprovalReplyPort> = sink_concrete;
        sup.runtime
            .register_im_reply_sink(Arc::downgrade(&sink_arc));
        sup
    }

    /// Spawns the supervisor background loop bound to `shutdown_rx`.
    #[must_use]
    pub fn spawn(self: Arc<Self>, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run(shutdown_rx).await;
        })
    }

    /// Runs the periodic lease-claim, fencing, and graceful shutdown loop until `shutdown_rx` signals `true`.
    pub async fn run(self: Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) {
        if let Err(err) = self.scan_once().await {
            tracing::warn!(holder = %self.holder_id, error = %err, "initial IM lease scan failed");
        }

        let mut ticker = tokio::time::interval(IM_SUPERVISOR_SCAN_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let _first = ticker.tick().await;

        while !*shutdown_rx.borrow() {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                _ = ticker.tick() => {
                    if let Err(err) = self.scan_once().await {
                        tracing::warn!(
                            holder = %self.holder_id,
                            error = %err,
                            "IM connector lease scan failed"
                        );
                    }
                }
            }
        }

        self.shutdown_all_workers().await;
    }

    /// Performs one lease claim/renewal sweep (`ax_claim_im_connectors`), fencing lost connectors
    /// and spawning workers for newly acquired or reconfigured connectors.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if `claim_im_connectors` fails.
    pub async fn scan_once(self: &Arc<Self>) -> Result<usize, AppError> {
        let claimed = self
            .store
            .claim_im_connectors(&self.holder_id, self.max_slots, IM_LEASE_DURATION_SECS)
            .await?;
        let claimed_ids: HashSet<Uuid> = claimed.iter().map(|c| c.id).collect();

        let mut workers = self.workers.lock().await;
        workers.retain(|_, entry| !entry.handle.is_finished());

        let stale_ids: Vec<Uuid> = workers
            .keys()
            .copied()
            .filter(|id| !claimed_ids.contains(id))
            .collect();
        for stale_id in stale_ids {
            if let Some(removed) = workers.remove(&stale_id) {
                removed.cancel.cancel();
            }
        }

        let count = claimed.len();
        for conn in claimed {
            let fp = connector_fingerprint(&conn);
            let needs_respawn = match workers.get(&conn.id) {
                Some(existing) => existing.fingerprint != fp,
                None => true,
            };
            if needs_respawn {
                if let Some(old) = workers.remove(&conn.id) {
                    old.cancel.cancel();
                }
                let cancel = CancellationToken::new();
                let sup_clone = Arc::clone(self);
                let worker_cancel = cancel.clone();
                let conn_id = conn.id;
                let handle = tokio::spawn(async move {
                    sup_clone.run_connector_worker(conn, worker_cancel).await;
                });
                workers.insert(
                    conn_id,
                    ActiveWorkerHandle {
                        cancel,
                        fingerprint: fp,
                        handle,
                    },
                );
            }
        }
        Ok(count)
    }

    async fn shutdown_all_workers(&self) {
        let drained: Vec<ActiveWorkerHandle> = {
            let mut workers = self.workers.lock().await;
            workers.drain().map(|(_, v)| v).collect()
        };
        for entry in &drained {
            entry.cancel.cancel();
        }
        for entry in drained {
            if tokio::time::timeout(Duration::from_secs(2), entry.handle)
                .await
                .is_err()
            {
                tracing::debug!("IM worker shutdown timed out after 2s");
            }
        }
        self.active_out_senders.write().await.clear();
        if let Err(err) = self.store.release_im_connectors(&self.holder_id).await {
            tracing::warn!(
                holder = %self.holder_id,
                error = %err,
                "ax_release_im_connectors failed on shutdown"
            );
        }
    }

    async fn run_connector_worker(
        self: Arc<Self>,
        connector: ImConnectorRecord,
        cancel: CancellationToken,
    ) {
        let mut backoff = Duration::from_secs(1);

        while !cancel.is_cancelled() {
            if !self
                .report_status(&connector, ImConnectorStatus::Connecting, "")
                .await
            {
                cancel.cancel();
                break;
            }

            match self.establish_connector_session(&connector).await {
                Ok((ws_stream, ping_interval)) => {
                    backoff = Duration::from_secs(1);
                    if !self
                        .report_status(&connector, ImConnectorStatus::Connected, "")
                        .await
                    {
                        cancel.cancel();
                        break;
                    }
                    self.drive_ws_session(&connector, ws_stream, ping_interval, &cancel)
                        .await;
                    if cancel.is_cancelled() {
                        break;
                    }
                    let _ = self
                        .report_status(&connector, ImConnectorStatus::Degraded, "connection closed")
                        .await;
                    wait_or_cancel(&cancel, Duration::from_millis(500)).await;
                }
                Err(err) => {
                    let msg = err.client_message();
                    if !self
                        .report_status(&connector, ImConnectorStatus::Error, &msg)
                        .await
                    {
                        cancel.cancel();
                        break;
                    }
                    wait_or_cancel(&cancel, backoff).await;
                    backoff = (backoff.saturating_mul(2)).min(Duration::from_secs(10));
                }
            }
        }

        self.active_out_senders.write().await.remove(&connector.id);
    }

    async fn establish_connector_session(
        &self,
        connector: &ImConnectorRecord,
    ) -> Result<(ImWsStream, Duration), AppError> {
        let secrets = self.unseal_connector_secrets(connector).await?;
        let plan = resolve_platform_dial_plan(
            connector.platform,
            &connector.config,
            &secrets,
            self.url_guard.as_ref(),
        )
        .await?;
        let mut ws_stream = dial_websocket(
            &plan.ws_url,
            self.url_guard.as_ref(),
            connector.config.allow_private_network,
        )
        .await?;
        if let Some(sub_frame) = plan.subscribe_frame {
            ws_stream
                .send(Message::Text(sub_frame.into()))
                .await
                .map_err(|err| AppError::Upstream(format!("IM subscribe send failed: {err}")))?;
        }
        Ok((ws_stream, plan.ping_interval))
    }

    async fn unseal_connector_secrets(
        &self,
        connector: &ImConnectorRecord,
    ) -> Result<BTreeMap<String, String>, AppError> {
        let sealed = self
            .store
            .get_credential(&connector.tenant_id, connector.credential_id)
            .await?
            .ok_or(AppError::NotFound("CREDENTIAL_NOT_FOUND"))?;
        let aad = CredentialKeyring::aad_for_credential(&connector.tenant_id, sealed.id);
        let plain = self.keyring.open_credential(&aad, &sealed)?;
        serde_json::from_slice(plain.as_slice())
            .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
    }

    async fn report_status(
        &self,
        connector: &ImConnectorRecord,
        status: ImConnectorStatus,
        last_error: &str,
    ) -> bool {
        match self
            .store
            .update_im_connector_runtime_state(
                &connector.tenant_id,
                connector.id,
                &self.holder_id,
                status,
                last_error,
            )
            .await
        {
            Ok(still_leader) => still_leader,
            Err(err) => {
                tracing::warn!(
                    connector_id = %connector.id,
                    error = %err,
                    "update_im_connector_runtime_state failed"
                );
                true
            }
        }
    }

    async fn drive_ws_session(
        self: &Arc<Self>,
        connector: &ImConnectorRecord,
        mut ws_stream: ImWsStream,
        ping_interval: Duration,
        cancel: &CancellationToken,
    ) {
        let (out_tx, mut out_rx) = mpsc::channel::<ImOutboundMessage>(64);
        self.active_out_senders
            .write()
            .await
            .insert(connector.id, out_tx.clone());
        let sem = Arc::new(Semaphore::new(MAX_INBOUND_CONCURRENCY_PER_CONNECTOR));

        let mut hb_ticker = tokio::time::interval(IM_HEARTBEAT_INTERVAL);
        hb_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let _first_hb = hb_ticker.tick().await;

        let mut ping_ticker = tokio::time::interval(ping_interval.max(Duration::from_secs(5)));
        ping_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let _first_ping = ping_ticker.tick().await;

        while !cancel.is_cancelled() {
            tokio::select! {
                () = cancel.cancelled() => {
                    let _closed = ws_stream.close(None).await.is_err();
                    break;
                }
                maybe_frame = ws_stream.next() => {
                    let Some(Ok(frame)) = maybe_frame else {
                        break;
                    };
                    if !self
                        .handle_raw_ws_frame(connector, frame, &mut ws_stream, &out_tx, &sem, cancel)
                        .await
                    {
                        break;
                    }
                }
                Some(outbound) = out_rx.recv() => {
                    if !self
                        .send_fenced_outbound(connector, &outbound, &mut ws_stream, cancel)
                        .await
                    {
                        break;
                    }
                }
                _ = hb_ticker.tick() => {
                    if !self.report_status(connector, ImConnectorStatus::Connected, "").await {
                        cancel.cancel();
                        break;
                    }
                }
                _ = ping_ticker.tick() => {
                    let ping_json = encode_platform_ping(connector.platform);
                    if ws_stream.send(Message::Text(ping_json.into())).await.is_err() {
                        break;
                    }
                }
            }
        }

        self.active_out_senders.write().await.remove(&connector.id);
    }

    async fn handle_raw_ws_frame(
        self: &Arc<Self>,
        connector: &ImConnectorRecord,
        frame: Message,
        ws_stream: &mut ImWsStream,
        out_tx: &mpsc::Sender<ImOutboundMessage>,
        sem: &Arc<Semaphore>,
        cancel: &CancellationToken,
    ) -> bool {
        let raw_text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => match String::from_utf8(b.to_vec()) {
                Ok(s) => s,
                Err(_) => return true,
            },
            Message::Ping(payload) => {
                return ws_stream.send(Message::Pong(payload)).await.is_ok();
            }
            Message::Pong(_) | Message::Frame(_) => return true,
            Message::Close(_) => return false,
        };

        let decoded = match decode_platform_frame(connector.platform, &raw_text) {
            Ok(d) => d,
            Err(AppError::Unauthorized(_)) => return false,
            Err(err) => {
                tracing::debug!(connector_id = %connector.id, error = %err, "ignored malformed IM frame");
                return true;
            }
        };

        if let DecodedPlatformFrame::Handled {
            immediate_ack,
            inbound,
        } = decoded
        {
            if let Some(ack_json) = immediate_ack
                && ws_stream
                    .send(Message::Text(ack_json.into()))
                    .await
                    .is_err()
            {
                return false;
            }
            if let Some(msg) = inbound {
                self.spawn_inbound_turn(
                    connector.clone(),
                    msg,
                    out_tx.clone(),
                    Arc::clone(sem),
                    cancel.clone(),
                );
            }
        }
        true
    }

    async fn send_fenced_outbound(
        &self,
        connector: &ImConnectorRecord,
        outbound: &ImOutboundMessage,
        ws_stream: &mut ImWsStream,
        cancel: &CancellationToken,
    ) -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        if outbound.is_final
            && !self
                .report_status(connector, ImConnectorStatus::Connected, "")
                .await
        {
            cancel.cancel();
            return false;
        }
        let encoded = encode_platform_reply(connector.platform, outbound);
        ws_stream.send(Message::Text(encoded.into())).await.is_ok()
    }

    fn spawn_inbound_turn(
        self: &Arc<Self>,
        connector: ImConnectorRecord,
        msg: ImInboundMessage,
        out_tx: mpsc::Sender<ImOutboundMessage>,
        sem: Arc<Semaphore>,
        cancel: CancellationToken,
    ) {
        let sup = Arc::clone(self);
        tokio::spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return;
            };
            if cancel.is_cancelled() {
                return;
            }
            if let Err(err) = sup
                .process_inbound_message(&connector, &msg, &out_tx, &cancel)
                .await
            {
                tracing::warn!(
                    connector_id = %connector.id,
                    event_id = %msg.event_id,
                    error = %err,
                    "IM inbound turn failed"
                );
            }
        });
    }

    async fn process_inbound_message(
        &self,
        connector: &ImConnectorRecord,
        msg: &ImInboundMessage,
        out_tx: &mpsc::Sender<ImOutboundMessage>,
        cancel: &CancellationToken,
    ) -> Result<(), AppError> {
        if msg.text.trim().is_empty() || msg.event_id.trim().is_empty() {
            return Ok(());
        }
        let newly_recorded = self
            .store
            .record_im_event_dedup(&connector.tenant_id, connector.id, &msg.event_id)
            .await?;
        if !newly_recorded {
            return Ok(());
        }

        let Some(target_uid) = self
            .resolve_or_challenge_user_id(connector, msg, out_tx)
            .await?
        else {
            return Ok(());
        };

        let Some(hydrated) = self
            .store
            .hydrate_im_principal(&connector.tenant_id, target_uid, connector.identity_policy)
            .await?
        else {
            send_final_text(
                out_tx,
                msg,
                String::from("当前绑定账号已停用或锁定，无法执行查询。"),
            )
            .await;
            return Ok(());
        };

        let auth =
            build_im_auth_principal(hydrated, connector, self.default_web_base_url.as_deref());
        self.execute_agent_turn_and_reply(connector, &auth, msg, out_tx, cancel)
            .await
    }

    async fn resolve_or_challenge_user_id(
        &self,
        connector: &ImConnectorRecord,
        msg: &ImInboundMessage,
        out_tx: &mpsc::Sender<ImOutboundMessage>,
    ) -> Result<Option<Uuid>, AppError> {
        match connector.identity_policy {
            ImIdentityPolicy::RequireUserBind => {
                if let Some(bound) = self
                    .store
                    .get_im_user_binding(
                        &connector.tenant_id,
                        connector.platform,
                        &msg.external_user_id,
                    )
                    .await?
                {
                    return Ok(Some(bound.user_id));
                }

                let raw_token = generate_im_bind_token();
                let token_hash = hash_im_bind_token(&raw_token)?;
                let expires_at = Utc::now() + TimeDelta::seconds(IM_BIND_CHALLENGE_TTL_SECS);
                self.store
                    .create_im_bind_challenge(
                        &connector.tenant_id,
                        NewImBindChallengeInput {
                            token_hash,
                            connector_id: connector.id,
                            platform: connector.platform,
                            external_user_id: msg.external_user_id.clone(),
                            external_display_name: msg.external_display_name.clone(),
                            expires_at,
                        },
                    )
                    .await?;

                let base = effective_web_base(connector, self.default_web_base_url.as_deref());
                let bind_text = format!(
                    "请完成平台账号绑定后继续提问：{base}/agents?im_bind={raw_token}（10 分钟内单次有效）"
                );
                send_final_text(out_tx, msg, bind_text).await;
                Ok(None)
            }
            ImIdentityPolicy::CreatorDelegated => {
                let Some(uid) = connector.delegated_user_id else {
                    send_final_text(
                        out_tx,
                        msg,
                        String::from("连接器未配置有效的委托账号，无法执行查询。"),
                    )
                    .await;
                    return Ok(None);
                };
                Ok(Some(uid))
            }
        }
    }

    async fn execute_agent_turn_and_reply(
        &self,
        connector: &ImConnectorRecord,
        auth: &AuthPrincipal,
        msg: &ImInboundMessage,
        out_tx: &mpsc::Sender<ImOutboundMessage>,
        cancel: &CancellationToken,
    ) -> Result<(), AppError> {
        let existing = self
            .store
            .find_active_im_conversation(
                &connector.tenant_id,
                connector.id,
                &msg.chat_id,
                Some(auth.user_id),
            )
            .await?;

        let (ev_tx, mut ev_rx) = mpsc::channel::<AgentEvent>(64);
        let progress_out = out_tx.clone();
        let chat_id = msg.chat_id.clone();
        let reply_token = msg.reply_token.clone();
        let progress_cancel = cancel.clone();
        let stream_id = format!("im_stream_{}", Uuid::new_v4().simple());

        let forwarder = tokio::spawn(async move {
            let mut throttler = ProgressThrottler::default();
            while let Some(ev) = ev_rx.recv().await {
                if progress_cancel.is_cancelled() {
                    break;
                }
                if let Some(hint_msg) = throttler.on_event(
                    &ev,
                    &chat_id,
                    reply_token.as_deref(),
                    Some(&stream_id),
                    Instant::now(),
                ) && progress_out.send(hint_msg).await.is_err()
                {
                    break;
                }
            }
        });

        let call = ChannelChatCall {
            agent_id: connector.agent_id,
            explicit_version: None,
            explicit_spec: None,
            request: AgentChatRequest {
                message: msg.text.clone(),
                conversation_id: existing.map(|c| c.id),
                draft: false,
            },
            channel: ConversationChannel::Im,
            embed_id: None,
            im_connector_id: Some(connector.id),
            external_chat_id: Some(msg.chat_id.clone()),
        };

        let turn_res = self.runtime.chat_on_channel(auth, call, Some(ev_tx)).await;
        let _joined = forwarder.await.is_err();

        if cancel.is_cancelled() {
            return Ok(());
        }

        match turn_res {
            Ok(resp) => {
                let base = effective_web_base(connector, self.default_web_base_url.as_deref());
                for chunk in format_response_chunks(&resp, Some(base)) {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let _closed = out_tx
                        .send(ImOutboundMessage {
                            chat_id: msg.chat_id.clone(),
                            text: chunk,
                            reply_token: msg.reply_token.clone(),
                            stream_id: None,
                            is_final: true,
                        })
                        .await
                        .is_err();
                }
                Ok(())
            }
            Err(err) => {
                send_final_text(
                    out_tx,
                    msg,
                    format!("请求未能完成：{}", err.client_message()),
                )
                .await;
                Err(err)
            }
        }
    }
}

/// Executes an outbound ticket/WebSocket connectivity probe for `POST /api/axagent/im-connectors/{id}/test` (`07-STEP21` §21.4.1).
///
/// # Errors
/// Propagates [`AppError::Forbidden`] on SSRF guard blocks and [`AppError::BadRequest`] on invalid inputs.
pub async fn probe_im_connector(
    connector: &ImConnectorRecord,
    secrets: &BTreeMap<String, String>,
    url_guard: &dyn OutboundUrlGuardPort,
) -> Result<ImConnectorTestResponse, AppError> {
    let start = Instant::now();
    let plan =
        match resolve_platform_dial_plan(connector.platform, &connector.config, secrets, url_guard)
            .await
        {
            Ok(p) => p,
            Err(err @ (AppError::Forbidden(_) | AppError::BadRequest { .. })) => return Err(err),
            Err(err) => {
                return Ok(ImConnectorTestResponse {
                    ok: false,
                    platform: connector.platform,
                    latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                    endpoint_preview: String::new(),
                    message: err.client_message(),
                });
            }
        };

    match dial_websocket(
        &plan.ws_url,
        url_guard,
        connector.config.allow_private_network,
    )
    .await
    {
        Ok(mut ws) => {
            if let Some(sub) = plan.subscribe_frame {
                let _sent = ws.send(Message::Text(sub.into())).await.is_ok();
            }
            let _closed = ws.close(None).await.is_ok();
            Ok(ImConnectorTestResponse {
                ok: true,
                platform: connector.platform,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                endpoint_preview: plan.endpoint_preview,
                message: String::from("已连接"),
            })
        }
        Err(err @ (AppError::Forbidden(_) | AppError::BadRequest { .. })) => Err(err),
        Err(err) => Ok(ImConnectorTestResponse {
            ok: false,
            platform: connector.platform,
            latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            endpoint_preview: plan.endpoint_preview,
            message: err.client_message(),
        }),
    }
}

/// Generates a 32-byte CSPRNG `axim_<base64url>` one-time identity bind challenge token (`07-STEP21` §21.3).
#[must_use]
pub fn generate_im_bind_token() -> String {
    let mut bytes = [0_u8; 32];
    let first = Uuid::new_v4().into_bytes();
    let second = Uuid::new_v4().into_bytes();
    if let Some(left) = bytes.get_mut(..16) {
        left.copy_from_slice(&first);
    }
    if let Some(right) = bytes.get_mut(16..) {
        right.copy_from_slice(&second);
    }
    format!("{IM_BIND_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

async fn resolve_platform_dial_plan(
    platform: ImPlatform,
    config: &ImConnectorConfig,
    secrets: &BTreeMap<String, String>,
    url_guard: &dyn OutboundUrlGuardPort,
) -> Result<PlatformDialPlan, AppError> {
    match platform {
        ImPlatform::Feishu => resolve_feishu_dial_plan(config, secrets, url_guard).await,
        ImPlatform::Dingtalk => resolve_dingtalk_dial_plan(config, secrets, url_guard).await,
        ImPlatform::Wecom => resolve_wecom_dial_plan(config, secrets, url_guard).await,
    }
}

fn decode_platform_frame(
    platform: ImPlatform,
    raw_text: &str,
) -> Result<DecodedPlatformFrame, AppError> {
    match platform {
        ImPlatform::Feishu => decode_feishu_frame(raw_text),
        ImPlatform::Dingtalk => decode_dingtalk_frame(raw_text),
        ImPlatform::Wecom => decode_wecom_frame(raw_text),
    }
}

fn encode_platform_reply(platform: ImPlatform, msg: &ImOutboundMessage) -> String {
    match platform {
        ImPlatform::Feishu => encode_feishu_reply(msg),
        ImPlatform::Dingtalk => encode_dingtalk_reply(msg),
        ImPlatform::Wecom => encode_wecom_reply(msg),
    }
}

fn encode_platform_ping(platform: ImPlatform) -> String {
    match platform {
        ImPlatform::Feishu => encode_feishu_ping(),
        ImPlatform::Dingtalk => encode_dingtalk_ping(),
        ImPlatform::Wecom => encode_wecom_ping(),
    }
}

fn build_im_auth_principal(
    hydrated: axpoint_domain::im::HydratedImPrincipal,
    connector: &ImConnectorRecord,
    default_web_base: Option<&str>,
) -> AuthPrincipal {
    let mut permissions = hydrated.principal.permissions;
    permissions.remove(&Permission::PiiRawRead);
    let base = effective_web_base(connector, default_web_base);
    AuthPrincipal {
        user_id: hydrated.principal.user_id,
        tenant_id: hydrated.principal.tenant_id,
        username: hydrated.principal.username,
        dept_id: hydrated.principal.dept_id,
        roles: hydrated.principal.roles,
        permissions,
        token_kind: TokenKind::Access,
        token_profile: TokenProfile::General,
        web_base_url: (!base.is_empty()).then(|| String::from(base)),
    }
}

fn effective_web_base<'a>(connector: &'a ImConnectorRecord, fallback: Option<&'a str>) -> &'a str {
    connector
        .config
        .web_base_url
        .as_deref()
        .map(str::trim)
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .or(fallback)
        .unwrap_or("")
}

fn format_response_chunks(resp: &AgentChatResponse, web_base: Option<&str>) -> Vec<String> {
    let base = web_base.map_or("", |s| s.trim().trim_end_matches('/'));
    if resp.status == ConversationStatus::WaitingApproval {
        let approval_id = resp.approval_id.unwrap_or_else(Uuid::nil);
        return vec![format!(
            "该操作需要人工审批后继续执行：{base}/flow?approval_id={approval_id}"
        )];
    }
    let chunks = split_markdown_chunks(&resp.answer, IM_MARKDOWN_MAX_CHUNK_BYTES);
    if chunks.is_empty() {
        vec![String::from("已完成处理。")]
    } else {
        chunks
    }
}

fn connector_fingerprint(c: &ImConnectorRecord) -> String {
    format!(
        "{}:{}:{:?}:{}:{:?}",
        c.agent_id, c.credential_id, c.config, c.identity_policy, c.delegated_user_id
    )
}

async fn send_final_text(
    out_tx: &mpsc::Sender<ImOutboundMessage>,
    inbound: &ImInboundMessage,
    text: String,
) {
    let _closed = out_tx
        .send(ImOutboundMessage {
            chat_id: inbound.chat_id.clone(),
            text,
            reply_token: inbound.reply_token.clone(),
            stream_id: None,
            is_final: true,
        })
        .await
        .is_err();
}

async fn wait_or_cancel(cancel: &CancellationToken, delay: Duration) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = tokio::time::sleep(delay) => {}
    }
}
