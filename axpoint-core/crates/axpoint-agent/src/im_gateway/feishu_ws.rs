//! Feishu / Lark outbound WebSocket ticket exchange, `< 3s` JSON event ACK,
//! and bidirectional message codec (`07-STEP21` §21.2.1).

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

const DEFAULT_FEISHU_BASE: &str = "https://open.feishu.cn";
const DEFAULT_PING_SECS: u64 = 15;

/// Exchanges Feishu `AppID` / `AppSecret` for a dynamic WebSocket URL via `POST /callback/ws/endpoint`
/// (or uses a direct `ws://` / `wss://` `endpoint_override` after SSRF verification).
///
/// # Errors
/// Returns [`AppError`] on missing credentials, SSRF block, authentication failure, or invalid upstream JSON.
pub async fn resolve_feishu_dial_plan(
    config: &ImConnectorConfig,
    secrets: &BTreeMap<String, String>,
    url_guard: &dyn OutboundUrlGuardPort,
) -> Result<PlatformDialPlan, AppError> {
    let app_id = required_secret(secrets, "app_id")?;
    let app_secret = required_secret(secrets, "app_secret")?;

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
        .unwrap_or(DEFAULT_FEISHU_BASE)
        .trim_end_matches('/');
    let ticket_url = format!("{base}/callback/ws/endpoint");
    let _checked = url_guard
        .check_outbound_url(&ticket_url, config.allow_private_network)
        .await?;

    let client = build_im_http_client(config.allow_private_network)?;
    let resp = client
        .post(&ticket_url)
        .json(&serde_json::json!({
            "AppID": app_id,
            "AppSecret": app_secret,
        }))
        .send()
        .await
        .map_err(|err| AppError::Upstream(format!("Feishu ticket exchange failed: {err}")))?;

    let status = resp.status();
    let payload: serde_json::Value = resp.json().await.map_err(|err| {
        AppError::Upstream(format!(
            "Feishu ticket JSON decode failed ({status}): {err}"
        ))
    })?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(AppError::Unauthorized("IM_AUTH_FAILED"));
    }
    if !status.is_success() {
        return Err(AppError::Upstream(format!(
            "Feishu ticket endpoint HTTP {status}"
        )));
    }
    if let Some(code) = payload.get("code").and_then(serde_json::Value::as_i64)
        && code != 0
    {
        let msg = payload
            .get("msg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("ticket exchange rejected");
        return Err(AppError::Upstream(format!("Feishu error {code}: {msg}")));
    }

    let ws_url = payload
        .pointer("/data/URL")
        .or_else(|| payload.pointer("/data/url"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Upstream(String::from("Feishu response missing data.URL")))?;
    let ping_secs = payload
        .pointer("/data/ClientConfig/PingInterval")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(DEFAULT_PING_SECS)
        .clamp(5, 120);

    Ok(PlatformDialPlan {
        ws_url: String::from(ws_url),
        endpoint_preview: sanitize_endpoint_preview(ws_url),
        ping_interval: Duration::from_secs(ping_secs),
        subscribe_frame: None,
    })
}

/// Decodes a Feishu JSON WebSocket frame (`im.message.receive_v1` or `pong`), producing a `< 3s`
/// `{"code": 200, "headers": {"event_id": ...}}` ACK frame alongside any normalized user message.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when `raw_text` is not valid JSON.
pub fn decode_feishu_frame(raw_text: &str) -> Result<DecodedPlatformFrame, AppError> {
    let val: serde_json::Value = serde_json::from_str(raw_text)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid Feishu frame: {err}")))?;

    if val.get("type").and_then(serde_json::Value::as_str) == Some("pong")
        || (val.get("code").and_then(serde_json::Value::as_i64) == Some(200)
            && val.get("event").is_none())
    {
        return Ok(DecodedPlatformFrame::Ignore);
    }

    let event_type = val
        .pointer("/header/event_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let event_id = val
        .pointer("/header/event_id")
        .and_then(serde_json::Value::as_str)
        .map_or("", str::trim);

    let immediate_ack = (!event_id.is_empty()).then(|| {
        serde_json::json!({
            "code": 200,
            "headers": { "event_id": event_id }
        })
        .to_string()
    });

    if event_type != "im.message.receive_v1" {
        return Ok(match immediate_ack {
            Some(ack) => DecodedPlatformFrame::Handled {
                immediate_ack: Some(ack),
                inbound: None,
            },
            None => DecodedPlatformFrame::Ignore,
        });
    }

    let inbound = extract_feishu_message(&val, event_id);
    Ok(DecodedPlatformFrame::Handled {
        immediate_ack,
        inbound,
    })
}

fn extract_feishu_message(val: &serde_json::Value, event_id: &str) -> Option<ImInboundMessage> {
    let ext_uid = val
        .pointer("/event/sender/sender_id/open_id")
        .or_else(|| val.pointer("/event/sender/sender_id/user_id"))
        .or_else(|| val.pointer("/event/sender/sender_id/union_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let sender_name = val
        .pointer("/event/sender/sender_name")
        .and_then(serde_json::Value::as_str)
        .map_or_else(String::new, |s| String::from(s.trim()));

    let chat_id = val
        .pointer("/event/message/chat_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let chat_type = val
        .pointer("/event/message/chat_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("p2p");
    let message_id = val
        .pointer("/event/message/message_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let raw_content = val
        .pointer("/event/message/content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let raw_text = extract_feishu_text_content(raw_content);
    let cleaned = strip_leading_bot_mention(&raw_text);
    if cleaned.is_empty() {
        return None;
    }

    let resolved_event_id = if event_id.is_empty() {
        message_id
            .clone()
            .unwrap_or_else(|| format!("feishu_{}", Uuid::new_v4()))
    } else {
        String::from(event_id)
    };

    Some(ImInboundMessage {
        event_id: resolved_event_id,
        external_user_id: String::from(ext_uid),
        external_display_name: sender_name,
        chat_id: String::from(chat_id),
        text: cleaned,
        reply_token: message_id,
        is_group: chat_type != "p2p",
    })
}

fn extract_feishu_text_content(raw_content: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw_content)
        && let Some(text) = parsed.get("text").and_then(serde_json::Value::as_str)
    {
        return String::from(text);
    }
    String::from(raw_content)
}

/// Encodes an outbound reply or progress frame over an active Feishu WebSocket session.
#[must_use]
pub fn encode_feishu_reply(msg: &ImOutboundMessage) -> String {
    serde_json::json!({
        "type": "im.reply",
        "chat_id": msg.chat_id,
        "reply_to": msg.reply_token,
        "stream_id": msg.stream_id,
        "text": msg.text,
        "is_final": msg.is_final,
    })
    .to_string()
}

/// Encodes a Feishu JSON keepalive `ping` frame.
#[must_use]
pub fn encode_feishu_ping() -> String {
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
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", format!("missing Feishu '{key}'")))
}
