//! Enterprise `WeChat` (`WeCom`) intelligent bot outbound WebSocket `aibot_subscribe`
//! authentication, `ping` keepalive, and `aibot_respond_msg` stream codec (`07-STEP21` §21.2.1).

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::im::{ImConnectorConfig, ImInboundMessage, ImOutboundMessage};
use axpoint_domain::ports::OutboundUrlGuardPort;
use uuid::Uuid;

use super::protocol::{
    DecodedPlatformFrame, PlatformDialPlan, normalize_url_for_guard, sanitize_endpoint_preview,
    strip_leading_bot_mention,
};

const DEFAULT_WECOM_WSS: &str = "wss://openws.work.weixin.qq.com";
const DEFAULT_PING_SECS: u64 = 20;

/// Resolves the `WeCom` WebSocket endpoint, performs SSRF preflight via `url_guard`,
/// and builds the initial `aibot_subscribe` authentication frame.
///
/// # Errors
/// Returns [`AppError`] on missing `bot_id`/`secret`, malformed `endpoint_override`, or SSRF rejection.
pub async fn resolve_wecom_dial_plan(
    config: &ImConnectorConfig,
    secrets: &BTreeMap<String, String>,
    url_guard: &dyn OutboundUrlGuardPort,
) -> Result<PlatformDialPlan, AppError> {
    let bot_id = required_secret(secrets, "bot_id")?;
    let secret = required_secret(secrets, "secret")?;

    let ws_url = match config
        .endpoint_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => coerce_to_ws_url(raw)?,
        None => String::from(DEFAULT_WECOM_WSS),
    };

    let http_equiv = normalize_url_for_guard(&ws_url)?;
    let _checked = url_guard
        .check_outbound_url(&http_equiv, config.allow_private_network)
        .await?;

    let subscribe = serde_json::json!({
        "cmd": "aibot_subscribe",
        "headers": {
            "req_id": Uuid::new_v4().to_string(),
        },
        "body": {
            "bot_id": bot_id,
            "secret": secret,
        }
    })
    .to_string();

    Ok(PlatformDialPlan {
        endpoint_preview: sanitize_endpoint_preview(&ws_url),
        ws_url,
        ping_interval: Duration::from_secs(DEFAULT_PING_SECS),
        subscribe_frame: Some(subscribe),
    })
}

fn coerce_to_ws_url(raw: &str) -> Result<String, AppError> {
    if raw.starts_with("ws://") || raw.starts_with("wss://") {
        return Ok(String::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("http://") {
        let trimmed = rest.trim_end_matches('/');
        let has_path = trimmed.contains('/');
        return Ok(if has_path {
            format!("ws://{trimmed}")
        } else {
            format!("ws://{trimmed}/ws")
        });
    }
    if let Some(rest) = raw.strip_prefix("https://") {
        let trimmed = rest.trim_end_matches('/');
        let has_path = trimmed.contains('/');
        return Ok(if has_path {
            format!("wss://{trimmed}")
        } else {
            format!("wss://{trimmed}/ws")
        });
    }
    Err(AppError::bad(
        "INVALID_ARGUMENT",
        "WeCom endpoint_override must start with ws://, wss://, http://, or https://",
    ))
}

/// Decodes an inbound `WeCom` JSON WebSocket frame (`aibot_msg_callback` or subscribe/ping ack).
///
/// # Errors
/// Returns [`AppError::BadRequest`] on invalid JSON or [`AppError::Unauthorized`] when `errcode != 0`.
pub fn decode_wecom_frame(raw_text: &str) -> Result<DecodedPlatformFrame, AppError> {
    let val: serde_json::Value = serde_json::from_str(raw_text)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid WeCom frame: {err}")))?;

    if let Some(errcode) = val.get("errcode").and_then(serde_json::Value::as_i64) {
        if errcode != 0 {
            return Err(AppError::Unauthorized("IM_AUTH_FAILED"));
        }
        if val.get("cmd").is_none() {
            return Ok(DecodedPlatformFrame::Ignore);
        }
    }

    let cmd = val
        .get("cmd")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if cmd != "aibot_msg_callback" {
        return Ok(DecodedPlatformFrame::Ignore);
    }

    let inbound = extract_wecom_callback(&val);
    Ok(DecodedPlatformFrame::Handled {
        immediate_ack: None,
        inbound,
    })
}

fn extract_wecom_callback(val: &serde_json::Value) -> Option<ImInboundMessage> {
    let req_id = val
        .pointer("/headers/req_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let body = val.get("body")?;

    let ext_uid = body
        .pointer("/from/userid")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let sender_name = body
        .pointer("/from/name")
        .and_then(serde_json::Value::as_str)
        .map_or_else(String::new, |s| String::from(s.trim()));
    let chat_id = body
        .get("chatid")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(ext_uid);
    let chattype = body
        .get("chattype")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("single");
    let raw_text = body
        .pointer("/text/content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let cleaned = strip_leading_bot_mention(raw_text);
    if cleaned.is_empty() {
        return None;
    }

    let event_id = body
        .get("msgid")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(req_id)
        .map_or_else(|| format!("wecom_{}", Uuid::new_v4()), String::from);

    Some(ImInboundMessage {
        event_id,
        external_user_id: String::from(ext_uid),
        external_display_name: sender_name,
        chat_id: String::from(chat_id),
        text: cleaned,
        reply_token: req_id.map(String::from),
        is_group: chattype == "group",
    })
}

/// Encodes a `WeCom` `aibot_respond_msg` stream/final reply frame.
#[must_use]
pub fn encode_wecom_reply(msg: &ImOutboundMessage) -> String {
    let req_id = msg
        .reply_token
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let stream_id = msg.stream_id.clone().unwrap_or_else(|| req_id.clone());

    serde_json::json!({
        "cmd": "aibot_respond_msg",
        "headers": {
            "req_id": req_id,
        },
        "chat_id": msg.chat_id,
        "text": msg.text,
        "is_final": msg.is_final,
        "body": {
            "msgtype": "stream",
            "stream": {
                "id": stream_id,
                "finish": msg.is_final,
                "content": msg.text,
            }
        }
    })
    .to_string()
}

/// Encodes a `WeCom` `ping` heartbeat frame.
#[must_use]
pub fn encode_wecom_ping() -> String {
    serde_json::json!({
        "cmd": "ping",
        "headers": {
            "req_id": Uuid::new_v4().to_string(),
        }
    })
    .to_string()
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
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("missing WeCom '{key}'")))
}
