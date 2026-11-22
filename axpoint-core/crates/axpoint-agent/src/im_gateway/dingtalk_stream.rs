//! `DingTalk` Stream outbound ticket exchange (`POST /v1.0/gateway/connections/open`),
//! `SYSTEM/ping` keepalive responder, and `CALLBACK` bot message codec (`07-STEP21` §21.2.1).

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::im::{ImConnectorConfig, ImInboundMessage, ImOutboundMessage};
use axpoint_domain::ports::OutboundUrlGuardPort;
use uuid::Uuid;

use super::protocol::{
    DecodedPlatformFrame, PlatformDialPlan, build_im_http_client, normalize_url_for_guard,
    sanitize_endpoint_preview, strip_leading_bot_mention,
};

const DEFAULT_DINGTALK_BASE: &str = "https://api.dingtalk.com";
const DEFAULT_PING_SECS: u64 = 20;

/// Exchanges `DingTalk` `client_id` / `client_secret` for a Stream WebSocket endpoint + one-time `ticket`.
///
/// # Errors
/// Returns [`AppError`] on missing credentials, SSRF guard rejection, or upstream gateway error.
pub async fn resolve_dingtalk_dial_plan(
    config: &ImConnectorConfig,
    secrets: &BTreeMap<String, String>,
    url_guard: &dyn OutboundUrlGuardPort,
) -> Result<PlatformDialPlan, AppError> {
    let client_id = required_secret(secrets, "client_id")?;
    let client_secret = required_secret(secrets, "client_secret")?;

    if let Some(raw_override) = config
        .endpoint_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && (raw_override.starts_with("ws://") || raw_override.starts_with("wss://"))
    {
        let http_equiv = normalize_url_for_guard(raw_override)?;
        let _checked = url_guard
            .check_outbound_url(&http_equiv, config.allow_private_network)
            .await?;
        return Ok(PlatformDialPlan {
            ws_url: String::from(raw_override),
            endpoint_preview: sanitize_endpoint_preview(raw_override),
            ping_interval: Duration::from_secs(DEFAULT_PING_SECS),
            subscribe_frame: None,
        });
    }

    let base = config
        .endpoint_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_DINGTALK_BASE)
        .trim_end_matches('/');
    let open_url = format!("{base}/v1.0/gateway/connections/open");
    let _checked = url_guard
        .check_outbound_url(&open_url, config.allow_private_network)
        .await?;

    let client = build_im_http_client(config.allow_private_network)?;
    let resp = client
        .post(&open_url)
        .json(&serde_json::json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "subscriptions": [
                { "type": "CALLBACK", "topic": "/v1.0/im/bot/messages/get" }
            ]
        }))
        .send()
        .await
        .map_err(|err| AppError::Upstream(format!("DingTalk connection open failed: {err}")))?;

    let status = resp.status();
    let payload: serde_json::Value = resp.json().await.map_err(|err| {
        AppError::Upstream(format!(
            "DingTalk open JSON decode failed ({status}): {err}"
        ))
    })?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(AppError::Unauthorized("IM_AUTH_FAILED"));
    }
    if !status.is_success() {
        return Err(AppError::Upstream(format!(
            "DingTalk gateway open HTTP {status}"
        )));
    }

    let endpoint = payload
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Upstream(String::from("DingTalk response missing endpoint")))?;
    let ticket = payload
        .get("ticket")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Upstream(String::from("DingTalk response missing ticket")))?;

    let encoded_ticket: String = url::form_urlencoded::byte_serialize(ticket.as_bytes()).collect();
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    let ws_url = format!("{endpoint}{sep}ticket={encoded_ticket}");

    Ok(PlatformDialPlan {
        endpoint_preview: sanitize_endpoint_preview(endpoint),
        ws_url,
        ping_interval: Duration::from_secs(DEFAULT_PING_SECS),
        subscribe_frame: None,
    })
}

/// Decodes a `DingTalk` Stream JSON frame (`SYSTEM/ping` or `CALLBACK` bot message).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when `raw_text` is not valid JSON.
pub fn decode_dingtalk_frame(raw_text: &str) -> Result<DecodedPlatformFrame, AppError> {
    let val: serde_json::Value = serde_json::from_str(raw_text).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid DingTalk Stream frame: {err}"),
        )
    })?;

    if val.get("type").and_then(serde_json::Value::as_str) == Some("pong") {
        return Ok(DecodedPlatformFrame::Ignore);
    }

    let frame_type = val
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let topic = val
        .pointer("/headers/topic")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let msg_id = val
        .pointer("/headers/messageId")
        .and_then(serde_json::Value::as_str)
        .map_or("", str::trim);

    if frame_type == "SYSTEM" && topic == "ping" {
        let data_echo = val
            .get("data")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("{}");
        let pong_ack = serde_json::json!({
            "code": 200,
            "headers": {
                "contentType": "application/json",
                "messageId": msg_id,
            },
            "message": "OK",
            "data": data_echo,
        })
        .to_string();
        return Ok(DecodedPlatformFrame::Handled {
            immediate_ack: Some(pong_ack),
            inbound: None,
        });
    }

    if frame_type != "CALLBACK" {
        return Ok(DecodedPlatformFrame::Ignore);
    }

    let ack = serde_json::json!({
        "code": 200,
        "headers": {
            "contentType": "application/json",
            "messageId": msg_id,
        },
        "message": "OK",
        "data": "{}",
    })
    .to_string();

    let inbound = extract_dingtalk_callback(&val, msg_id);
    Ok(DecodedPlatformFrame::Handled {
        immediate_ack: Some(ack),
        inbound,
    })
}

fn extract_dingtalk_callback(
    val: &serde_json::Value,
    header_msg_id: &str,
) -> Option<ImInboundMessage> {
    let data_obj: serde_json::Value = match val.get("data") {
        Some(serde_json::Value::String(s)) => serde_json::from_str(s).ok()?,
        Some(obj @ serde_json::Value::Object(_)) => obj.clone(),
        _ => return None,
    };

    let ext_uid = data_obj
        .get("senderStaffId")
        .or_else(|| data_obj.get("senderId"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let sender_nick = data_obj
        .get("senderNick")
        .and_then(serde_json::Value::as_str)
        .map_or_else(String::new, |s| String::from(s.trim()));
    let chat_id = data_obj
        .get("conversationId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let raw_text = data_obj
        .pointer("/text/content")
        .or_else(|| data_obj.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let cleaned = strip_leading_bot_mention(raw_text);
    if cleaned.is_empty() {
        return None;
    }

    let event_id = data_obj
        .get("msgId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| (!header_msg_id.is_empty()).then_some(header_msg_id))
        .map_or_else(|| format!("dt_{}", Uuid::new_v4()), String::from);
    let session_webhook = data_obj
        .get("sessionWebhook")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let conv_type = data_obj
        .get("conversationType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("1");

    Some(ImInboundMessage {
        event_id,
        external_user_id: String::from(ext_uid),
        external_display_name: sender_nick,
        chat_id: String::from(chat_id),
        text: cleaned,
        reply_token: session_webhook,
        is_group: conv_type == "2",
    })
}

/// Encodes an outbound `DingTalk` reply frame over the Stream WebSocket session.
#[must_use]
pub fn encode_dingtalk_reply(msg: &ImOutboundMessage) -> String {
    serde_json::json!({
        "type": "im.reply",
        "chat_id": msg.chat_id,
        "session_webhook": msg.reply_token,
        "stream_id": msg.stream_id,
        "text": msg.text,
        "is_final": msg.is_final,
    })
    .to_string()
}

/// Encodes an application-level `ping` frame for `DingTalk` Stream sessions.
#[must_use]
pub fn encode_dingtalk_ping() -> String {
    String::from(r#"{"type":"ping"}"#)
}

fn required_secret<'a>(
    secrets: &'a BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, AppError> {
    secrets
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("missing DingTalk '{key}'")))
}
