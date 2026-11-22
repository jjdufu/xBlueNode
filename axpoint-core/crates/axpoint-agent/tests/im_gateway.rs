//! Step 21 (`im_gateway`) unit and live local RFC-6455 WebSocket integration tests.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axpoint_agent::im_gateway::{
    DecodedPlatformFrame, ImGatewayDeps, ImGatewaySupervisor, ProgressThrottler,
    decode_dingtalk_frame, decode_feishu_frame, decode_wecom_frame, dial_websocket,
    encode_dingtalk_reply, encode_feishu_reply, encode_wecom_reply, probe_im_connector,
    strip_leading_bot_mention,
};
use axpoint_agent::{AgentModelPort, AgentModelRequest, AgentModelTurn, ChannelChatCall};
use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentEvent, AgentSpec, CreateAgentRequest, ToolCallSpec, ToolRef,
};
use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, ImConnectorStatus, ImIdentityPolicy, ImPlatform,
    Permission, Sensitivity, TokenKind,
};
use axpoint_domain::flow::{ApprovalDecision, DecideApprovalParams};
use axpoint_domain::im::{
    ImConnectorConfig, ImConnectorSecretInput, ImOutboundMessage, NewImConnectorInput,
    hash_im_bind_token,
};
use axpoint_domain::platform::{BoxFuture, PrincipalRecord};
use axpoint_domain::ports::{
    AgentRepository as _, FlowRepository as _, ImGatewayRepository as _, SimExecutionPort,
};
use axpoint_security::{CredentialKeyring, NetGuard};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use support::{HarnessStack, MockSimPort, MockStep09Store, WAREHOUSE_W1_ID, make_principal};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, watch};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct CountingScriptedModel {
    calls: AtomicUsize,
    last_raw_pii_flag: Mutex<Option<bool>>,
}

impl CountingScriptedModel {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            last_raw_pii_flag: Mutex::new(None),
        })
    }
}

impl AgentModelPort for CountingScriptedModel {
    fn complete_turn(
        &self,
        req: AgentModelRequest,
        _token_tx: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> BoxFuture<'_, AgentModelTurn> {
        Box::pin(async move {
            let _ = self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_raw_pii_flag.lock().await = Some(req.caller_can_read_raw_pii);

            let tool_outputs: Vec<String> = req
                .messages
                .iter()
                .filter_map(|m| match m {
                    axpoint_agent::ModelMessage::Tool { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .collect();

            if tool_outputs.is_empty() && !req.tools.is_empty() {
                let first_tool = req
                    .tools
                    .first()
                    .map_or_else(|| String::from("ask_analytics"), |t| t.name.clone());
                let args = if first_tool == "get_node" {
                    serde_json::json!({ "node_id": WAREHOUSE_W1_ID })
                } else if first_tool == "ack_alert" {
                    serde_json::json!({ "alert_id": support::ALERT_A1_ID, "comment": "ok" })
                } else {
                    serde_json::json!({ "question": "查询我可见的部门订单明细" })
                };
                return Ok(AgentModelTurn {
                    content: String::new(),
                    tool_calls: vec![ToolCallSpec {
                        id: String::from("call_step21_1"),
                        name: first_tool,
                        arguments: args,
                    }],
                    prompt_tokens: 32,
                    completion_tokens: 16,
                });
            }

            let joined = tool_outputs.join(" | ");
            Ok(AgentModelTurn {
                content: format!("受控核验结果：{joined}"),
                tool_calls: Vec::new(),
                prompt_tokens: 48,
                completion_tokens: 24,
            })
        })
    }
}

#[test]
fn im_gateway_feishu_dingtalk_wecom_codecs_and_mention_stripping() -> TestResult {
    assert_eq!(
        strip_leading_bot_mention("@_user_1   @经营助手 查询华西订单"),
        "查询华西订单"
    );

    let feishu_raw = serde_json::json!({
        "schema": "2.0",
        "header": { "event_id": "evt_f1", "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_123" }, "sender_name": "张三" },
            "message": {
                "message_id": "om_1", "chat_id": "oc_1", "chat_type": "group",
                "content": "{\"text\":\"@_user_1 订单汇总\"}"
            }
        }
    })
    .to_string();
    let DecodedPlatformFrame::Handled {
        immediate_ack: Some(f_ack),
        inbound: Some(f_msg),
    } = decode_feishu_frame(&feishu_raw)?
    else {
        return Err("expected Feishu Handled frame".into());
    };
    assert!(f_ack.contains("\"code\":200") && f_ack.contains("evt_f1"));
    assert_eq!(f_msg.external_user_id, "ou_123");
    assert_eq!(f_msg.text, "订单汇总");
    assert!(f_msg.is_group);

    let dingtalk_sys_frame = r#"{"type":"SYSTEM","headers":{"topic":"ping","messageId":"m_ping"},"data":"{\"opaque\":1}"}"#;
    let DecodedPlatformFrame::Handled {
        immediate_ack: Some(dingtalk_ack_frame),
        inbound: None,
    } = decode_dingtalk_frame(dingtalk_sys_frame)?
    else {
        return Err("expected DingTalk SYSTEM/ping ack".into());
    };
    assert!(dingtalk_ack_frame.contains("m_ping"));

    let dt_cb = serde_json::json!({
        "type": "CALLBACK",
        "headers": { "topic": "/v1.0/im/bot/messages/get", "messageId": "m_cb_1" },
        "data": serde_json::json!({
            "msgId": "dt_msg_1", "senderStaffId": "staff_09", "senderNick": "李四",
            "conversationId": "cid_9", "conversationType": "1",
            "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=abc",
            "text": { "content": " 库存预警 " }
        }).to_string()
    })
    .to_string();
    let DecodedPlatformFrame::Handled {
        immediate_ack: Some(_),
        inbound: Some(dt_msg),
    } = decode_dingtalk_frame(&dt_cb)?
    else {
        return Err("expected DingTalk CALLBACK message".into());
    };
    assert_eq!(dt_msg.external_user_id, "staff_09");
    assert_eq!(dt_msg.text, "库存预警");

    let wc_cb = serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": { "req_id": "req_77" },
        "body": {
            "msgid": "wc_1", "chatid": "wrk_1", "chattype": "group",
            "from": { "userid": "zhangsan", "name": "张三" },
            "text": { "content": "@bot 营收趋势" }
        }
    })
    .to_string();
    let DecodedPlatformFrame::Handled {
        inbound: Some(wc_msg),
        ..
    } = decode_wecom_frame(&wc_cb)?
    else {
        return Err("expected WeCom callback".into());
    };
    assert_eq!(wc_msg.external_user_id, "zhangsan");
    assert_eq!(wc_msg.text, "营收趋势");

    let sample_out = ImOutboundMessage {
        chat_id: String::from("c1"),
        text: String::from("结果正常"),
        reply_token: Some(String::from("r1")),
        stream_id: Some(String::from("s1")),
        is_final: true,
    };
    assert!(encode_feishu_reply(&sample_out).contains("结果正常"));
    assert!(encode_dingtalk_reply(&sample_out).contains("结果正常"));
    assert!(encode_wecom_reply(&sample_out).contains("aibot_respond_msg"));
    Ok(())
}

#[test]
fn im_gateway_progress_throttler_enforces_800ms_interval() {
    let mut throttler = ProgressThrottler::default();
    let t0 = Instant::now();
    let ev_ask = AgentEvent::ToolCall {
        id: String::from("c1"),
        name: String::from("ask_analytics"),
        arguments: serde_json::json!({}),
        agent_name: None,
    };
    let ev_kb = AgentEvent::ToolCall {
        id: String::from("c2"),
        name: String::from("search_knowledge"),
        arguments: serde_json::json!({}),
        agent_name: None,
    };

    let first = throttler.on_event(&ev_ask, "oc_1", None, None, t0);
    assert_eq!(
        first.as_ref().map(|m| m.text.as_str()),
        Some("执行受控问数…")
    );
    assert_eq!(first.as_ref().map(|m| m.is_final), Some(false));

    assert!(
        throttler
            .on_event(&ev_kb, "oc_1", None, None, t0 + Duration::from_millis(300))
            .is_none()
    );
    let second = throttler.on_event(&ev_kb, "oc_1", None, None, t0 + Duration::from_millis(850));
    assert_eq!(
        second.as_ref().map(|m| m.text.as_str()),
        Some("检索知识库…")
    );

    let ev_approval = AgentEvent::ApprovalRequired {
        approval_id: Uuid::new_v4(),
        conversation_id: Uuid::new_v4(),
        tool_call_id: String::from("c3"),
        tool_name: String::from("ack_alert"),
        arguments: serde_json::json!({}),
    };
    let third = throttler.on_event(
        &ev_approval,
        "oc_1",
        None,
        None,
        t0 + Duration::from_millis(900),
    );
    assert_eq!(
        third.as_ref().map(|m| m.text.as_str()),
        Some("等待人工审批…")
    );
}

#[tokio::test]
async fn im_gateway_ssrf_guard_blocks_loopback_when_private_network_disallowed() -> TestResult {
    let guard = NetGuard::new();
    let res = dial_websocket("ws://127.0.0.1:19821/ws", &guard, false).await;
    assert!(matches!(res, Err(AppError::Forbidden("PERMISSION_DENIED"))));

    let store = MockStep09Store::seeded().await?;
    let cred_id = Uuid::new_v4();
    let keyring = CredentialKeyring::ephemeral()?;
    let secrets = ImConnectorSecretInput {
        app_id: Some(String::from("cli_1")),
        app_secret: Some(String::from("sec_1")),
        ..ImConnectorSecretInput::default()
    }
    .normalize_for_platform(ImPlatform::Feishu)?;
    let aad = CredentialKeyring::aad_for_credential("default", cred_id);
    let sealed = keyring.seal_credential(cred_id, &aad, &serde_json::to_vec(&secrets)?)?;
    let conn = store
        .create_im_connector(
            "default",
            NewImConnectorInput {
                name: String::from("ssrf_blocked_bot"),
                platform: ImPlatform::Feishu,
                agent_id: Uuid::new_v4(),
                credential_id: cred_id,
                credential: sealed,
                config: ImConnectorConfig {
                    endpoint_override: Some(String::from("ws://127.0.0.1:19821/ws")),
                    allow_private_network: false,
                    bot_code: None,
                    web_base_url: None,
                },
                identity_policy: ImIdentityPolicy::RequireUserBind,
                delegated_user_id: None,
                enabled: false,
                created_by: None,
            },
        )
        .await?;

    let probe_res = probe_im_connector(&conn, &secrets, &guard).await;
    assert!(matches!(
        probe_res,
        Err(AppError::Forbidden("PERMISSION_DENIED"))
    ));
    Ok(())
}

struct LocalWsHub {
    ws_url: String,
    inbound_tx: tokio::sync::mpsc::Sender<String>,
    outbound_frames: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn spawn_local_ws_hub() -> Result<LocalWsHub, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel::<String>(32);
    let shared_rx = Arc::new(Mutex::new(inbound_rx));
    let outbound_frames: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let out_clone = Arc::clone(&outbound_frames);

    tokio::spawn(async move {
        for _ in 0..8_u8 {
            let Ok((tcp, _)) = listener.accept().await else {
                break;
            };
            let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else {
                continue;
            };
            let rx_arc = Arc::clone(&shared_rx);
            let frames_arc = Arc::clone(&out_clone);
            let mut active = true;
            while active {
                let mut rx_guard = rx_arc.lock().await;
                tokio::select! {
                    Some(server_push) = rx_guard.recv() => {
                        drop(rx_guard);
                        if ws.send(Message::Text(server_push.into())).await.is_err() {
                            active = false;
                        }
                    }
                    client_msg = ws.next() => {
                        drop(rx_guard);
                        match client_msg {
                            Some(Ok(Message::Text(txt))) => {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                                    frames_arc.lock().await.push(v);
                                }
                            }
                            Some(Ok(Message::Close(_)) | Err(_)) | None => {
                                active = false;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    Ok(LocalWsHub {
        ws_url: format!("ws://{addr}/ws"),
        inbound_tx,
        outbound_frames,
    })
}

async fn build_harness(model: &Arc<CountingScriptedModel>) -> Result<HarnessStack, AppError> {
    let model_concrete = Arc::clone(model);
    let model_port: Arc<dyn AgentModelPort> = model_concrete;
    let sim_port: Arc<dyn SimExecutionPort> = Arc::new(MockSimPort);
    let stack = HarnessStack::build(model_port, Some(sim_port)).await?;
    let _touch = (
        &stack.publisher,
        &stack.mcp_hub,
        &stack.mcp_clients,
        &stack.embed,
        &stack.audit_sink,
    );
    Ok(stack)
}

async fn seed_im_agent_and_connector(
    store: &Arc<MockStep09Store>,
    keyring: &CredentialKeyring,
    ws_url: &str,
    tool_name: &str,
    approval_tools: Vec<String>,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    let admin = make_principal(TokenKind::Access, &[]);
    let agent = store
        .create_agent(
            "default",
            Some(admin.user_id),
            CreateAgentRequest {
                name: format!("im_bot_{}", Uuid::new_v4().simple()),
                description: String::from("IM test bot"),
                spec: AgentSpec {
                    system_prompt: String::from("严格按工具返回作答"),
                    tools: vec![ToolRef::Builtin {
                        name: String::from(tool_name),
                    }],
                    approval_required_tools: approval_tools,
                    max_steps: 4,
                    ..AgentSpec::default()
                },
            },
        )
        .await?;
    let _ver = store
        .publish_agent_version("default", agent.id, Some(admin.user_id))
        .await?;

    let secrets = ImConnectorSecretInput {
        app_id: Some(String::from("cli_live")),
        app_secret: Some(String::from("sec_live")),
        ..ImConnectorSecretInput::default()
    }
    .normalize_for_platform(ImPlatform::Feishu)?;
    let cred_id = Uuid::new_v4();
    let aad = CredentialKeyring::aad_for_credential("default", cred_id);
    let sealed = keyring.seal_credential(cred_id, &aad, &serde_json::to_vec(&secrets)?)?;
    let conn = store
        .create_im_connector(
            "default",
            NewImConnectorInput {
                name: format!("conn_{}", Uuid::new_v4().simple()),
                platform: ImPlatform::Feishu,
                agent_id: agent.id,
                credential_id: cred_id,
                credential: sealed,
                config: ImConnectorConfig {
                    endpoint_override: Some(String::from(ws_url)),
                    allow_private_network: true,
                    bot_code: None,
                    web_base_url: None,
                },
                identity_policy: ImIdentityPolicy::RequireUserBind,
                delegated_user_id: None,
                enabled: true,
                created_by: Some(admin.user_id),
            },
        )
        .await?;
    Ok((agent.id, conn.id))
}

fn feishu_inbound_json(event_id: &str, open_id: &str, chat_id: &str, text: &str) -> String {
    serde_json::json!({
        "schema": "2.0",
        "header": { "event_id": event_id, "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": open_id }, "sender_name": "王西区" },
            "message": {
                "message_id": format!("om_{event_id}"),
                "chat_id": chat_id,
                "chat_type": "p2p",
                "content": serde_json::json!({ "text": text }).to_string()
            }
        }
    })
    .to_string()
}

async fn wait_for_connected(store: &MockStep09Store, conn_id: Uuid) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if store
            .get_im_connector("default", conn_id)
            .await?
            .is_some_and(|c| c.status == ImConnectorStatus::Connected)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    Err("timed out waiting for ImConnectorStatus::Connected".into())
}

async fn wait_for_reply_count(
    frames: &Arc<Mutex<Vec<serde_json::Value>>>,
    min_replies: usize,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let replies: Vec<serde_json::Value> = frames
            .lock()
            .await
            .iter()
            .filter(|v| {
                v.get("type").and_then(serde_json::Value::as_str) == Some("im.reply")
                    && v.get("is_final").and_then(serde_json::Value::as_bool) == Some(true)
            })
            .cloned()
            .collect();
        if replies.len() >= min_replies {
            return Ok(replies);
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    Err(format!("timed out waiting for {min_replies} IM final replies").into())
}

#[tokio::test]
async fn im_gateway_live_ws_challenge_bind_pii_guardrail_dedup_and_fencing() -> TestResult {
    let hub = spawn_local_ws_hub().await?;
    let model = CountingScriptedModel::new();
    let stack = build_harness(&model).await?;
    let keyring = Arc::new(CredentialKeyring::ephemeral()?);
    let (_agent_id, conn_id) =
        seed_im_agent_and_connector(&stack.store, &keyring, &hub.ws_url, "get_node", Vec::new())
            .await?;

    let west_uid = Uuid::new_v4();
    stack.store.inner.lock().await.principals.insert(
        (String::from("default"), west_uid),
        PrincipalRecord {
            user_id: west_uid,
            tenant_id: String::from("default"),
            username: String::from("analyst_west"),
            display_name: String::from("王西区"),
            dept_id: String::from("west"),
            roles: vec![String::from("ADMIN")],
            permissions: Permission::all_set(),
            clearance: Sensitivity::Secret,
            tokens_valid_after: Utc::now(),
        },
    );

    let sup = ImGatewaySupervisor::new(ImGatewayDeps {
        store: Arc::clone(&stack.store),
        runtime: Arc::clone(&stack.runtime),
        keyring,
        url_guard: Arc::new(NetGuard::new()),
        holder_id: Some(String::from("replica-test-1")),
        default_web_base_url: None,
    });
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let sup_task = Arc::clone(&sup).spawn(shutdown_rx);
    wait_for_connected(&stack.store, conn_id).await?;

    // 1. Unbound user -> challenge emitted, ZERO LLM calls!
    hub.inbound_tx
        .send(feishu_inbound_json(
            "evt_1",
            "ou_west_1",
            "oc_dm_1",
            "查仓库",
        ))
        .await?;
    let replies_1 = wait_for_reply_count(&hub.outbound_frames, 1).await?;
    let challenge_text = replies_1
        .first()
        .and_then(|v| v.get("text"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(challenge_text.contains("/agents?im_bind=axim_"));
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);

    let raw_token = challenge_text
        .split("im_bind=")
        .nth(1)
        .and_then(|s| s.split('（').next())
        .ok_or("missing axim token")?;
    let token_hash = hash_im_bind_token(raw_token)?;
    assert!(
        stack
            .store
            .consume_im_bind_challenge("default", &token_hash, west_uid)
            .await?
            .is_some()
    );

    // 2. Bound user + duplicate event_id -> 1 reply, PII_RAW_READ stripped, phone masked!
    hub.inbound_tx
        .send(feishu_inbound_json(
            "evt_2",
            "ou_west_1",
            "oc_dm_1",
            "查一号仓",
        ))
        .await?;
    hub.inbound_tx
        .send(feishu_inbound_json(
            "evt_2",
            "ou_west_1",
            "oc_dm_1",
            "查一号仓",
        ))
        .await?;
    let replies_2 = wait_for_reply_count(&hub.outbound_frames, 2).await?;
    let answer_text = replies_2
        .get(1)
        .and_then(|v| v.get("text"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(!answer_text.contains("13812345678"));
    assert_eq!(*model.last_raw_pii_flag.lock().await, Some(false));
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);

    // 3. Steal lease to simulate split-brain fencing, then graceful shutdown.
    if let Some(c) = stack.store.inner.lock().await.im_connectors.first_mut() {
        c.leader_holder = Some(String::from("replica-test-2"));
    }
    let _ = sup.scan_once().await?;
    let _sent = shutdown_tx.send(true).is_ok();
    sup_task.await?;
    Ok(())
}

#[tokio::test]
async fn im_gateway_approval_resumption_strips_pii_and_pushes_to_im_thread() -> TestResult {
    let hub = spawn_local_ws_hub().await?;
    let model = CountingScriptedModel::new();
    let stack = build_harness(&model).await?;
    let keyring = Arc::new(CredentialKeyring::ephemeral()?);
    let (agent_id, conn_id) = seed_im_agent_and_connector(
        &stack.store,
        &keyring,
        &hub.ws_url,
        "ack_alert",
        vec![String::from("ack_alert")],
    )
    .await?;

    let uid = Uuid::from_u128(1);
    stack.store.inner.lock().await.principals.insert(
        (String::from("default"), uid),
        PrincipalRecord {
            user_id: uid,
            tenant_id: String::from("default"),
            username: String::from("ops_admin"),
            display_name: String::from("Operator"),
            dept_id: String::from("east_china"),
            roles: vec![String::from("ADMIN")],
            permissions: Permission::all_set(),
            clearance: Sensitivity::Secret,
            tokens_valid_after: Utc::now(),
        },
    );

    let sup = ImGatewaySupervisor::new(ImGatewayDeps {
        store: Arc::clone(&stack.store),
        runtime: Arc::clone(&stack.runtime),
        keyring,
        url_guard: Arc::new(NetGuard::new()),
        holder_id: Some(String::from("replica-approval")),
        default_web_base_url: None,
    });
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let sup_task = Arc::clone(&sup).spawn(shutdown_rx);
    wait_for_connected(&stack.store, conn_id).await?;

    let im_principal = make_principal(
        TokenKind::Access,
        &[
            Permission::DataRead,
            Permission::AgentRun,
            Permission::AlertAck,
        ],
    );
    let paused = stack
        .runtime
        .chat_on_channel(
            &im_principal,
            ChannelChatCall {
                agent_id,
                explicit_version: None,
                explicit_spec: None,
                request: AgentChatRequest {
                    message: String::from("确认告警"),
                    conversation_id: None,
                    draft: false,
                },
                channel: ConversationChannel::Im,
                embed_id: None,
                im_connector_id: Some(conn_id),
                external_chat_id: Some(String::from("oc_approval_1")),
            },
            None,
        )
        .await?;
    assert_eq!(paused.status, ConversationStatus::WaitingApproval);
    let approval_id = paused.approval_id.ok_or("missing approval_id")?;

    let _decided = stack
        .store
        .decide_approval(
            "default",
            DecideApprovalParams {
                approval_id,
                decision: ApprovalDecision::Approve,
                actor_user_id: uid,
                actor_roles: vec![String::from("ADMIN")],
                comment: String::from("approved"),
            },
        )
        .await?;

    let resumed = stack
        .runtime
        .resume_after_approval("default", approval_id)
        .await?
        .ok_or("expected resumed response")?;
    assert_eq!(resumed.status, ConversationStatus::Active);
    assert_eq!(*model.last_raw_pii_flag.lock().await, Some(false));

    let pushed = wait_for_reply_count(&hub.outbound_frames, 1).await?;
    assert_eq!(
        pushed
            .first()
            .and_then(|v| v.get("chat_id"))
            .and_then(serde_json::Value::as_str),
        Some("oc_approval_1")
    );

    let _sent = shutdown_tx.send(true).is_ok();
    sup_task.await?;
    Ok(())
}
