//! Feishu / Lark (`FEISHU`) custom bot interactive card notification sender (`07-STEP07-08` §08.4).

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::ports::OutboundUrlGuardPort;
use axpoint_domain::rules::NotifyChannelRecord;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::{
    ChannelFailure, HttpClientPair, NOTIFY_HTTP_TIMEOUT_SECS, NotifyPayloadView,
    lookup_secret_or_config, read_truncated_response_text,
};

type HmacSha256 = Hmac<Sha256>;

/// Computes the Feishu custom bot signature (`base64(HMAC_SHA256(key = "{timestamp_secs}\n{secret}", msg = b""))`).
///
/// Notice Feishu uses `"{timestamp}\n{secret}"` as the HMAC **key** over an empty message (`07-STEP07-08` §08.4).
///
/// # Errors
/// Returns [`AppError::Config`] if HMAC key initialization fails.
pub fn compute_feishu_sign(timestamp_secs: i64, secret: &str) -> Result<String, AppError> {
    let key = format!("{timestamp_secs}\n{secret}");
    let mut mac = HmacSha256::new_from_slice(key.as_bytes())
        .map_err(|err| AppError::Config(format!("invalid Feishu HMAC key: {err}")))?;
    mac.update(b"");
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

/// Builds the Feishu `interactive` card JSON body (with optional `timestamp` + `sign`).
///
/// # Errors
/// Returns [`AppError::Config`] if signature computation fails.
pub fn build_feishu_payload(
    view: &NotifyPayloadView<'_>,
    sign_secret: Option<&str>,
) -> Result<serde_json::Value, AppError> {
    let md_content = if view.body.is_empty() {
        String::from(view.title)
    } else {
        String::from(view.body)
    };
    let mut obj = serde_json::Map::new();
    if let Some(secret) = sign_secret.filter(|s| !s.is_empty()) {
        let ts_secs = view.now.timestamp();
        let sign = compute_feishu_sign(ts_secs, secret)?;
        obj.insert(
            String::from("timestamp"),
            serde_json::Value::String(ts_secs.to_string()),
        );
        obj.insert(String::from("sign"), serde_json::Value::String(sign));
    }
    obj.insert(
        String::from("msg_type"),
        serde_json::Value::String(String::from("interactive")),
    );
    obj.insert(
        String::from("card"),
        serde_json::json!({
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": view.title,
                }
            },
            "elements": [
                {
                    "tag": "markdown",
                    "content": md_content,
                }
            ]
        }),
    );
    Ok(serde_json::Value::Object(obj))
}

/// Sends a single `FEISHU` interactive card notification and verifies `code == 0`.
pub(crate) async fn send_feishu<U: OutboundUrlGuardPort>(
    channel: &NotifyChannelRecord,
    secrets: &BTreeMap<String, String>,
    view: &NotifyPayloadView<'_>,
    url_guard: &U,
    clients: HttpClientPair<'_>,
) -> Result<i32, ChannelFailure> {
    let raw_url = lookup_secret_or_config(secrets, &channel.config, &["webhook_url", "url"])
        .ok_or_else(|| {
            ChannelFailure::permanent(AppError::bad(
                "INVALID_ARGUMENT",
                "FEISHU channel requires 'webhook_url'",
            ))
        })?;

    let checked_url = url_guard
        .check_outbound_url(raw_url, channel.allow_private_network)
        .await
        .map_err(ChannelFailure::from_guard_error)?;

    let sign_secret = lookup_secret_or_config(
        secrets,
        &channel.config,
        &["sign_secret", "signing_secret", "secret"],
    );
    let payload = build_feishu_payload(view, sign_secret).map_err(ChannelFailure::permanent)?;

    let client = clients.select(&checked_url, channel.allow_private_network);
    let resp = client
        .post(&checked_url)
        .timeout(Duration::from_secs(NOTIFY_HTTP_TIMEOUT_SECS))
        .json(&payload)
        .send()
        .await
        .map_err(|err| ChannelFailure::from_reqwest(&err))?;

    let status_code = i32::from(resp.status().as_u16());
    let text = read_truncated_response_text(resp).await;
    if !(200..300).contains(&status_code) {
        return Err(ChannelFailure::retryable(
            Some(status_code),
            format!("FEISHU HTTP {status_code}: {text}"),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        ChannelFailure::retryable(
            Some(status_code),
            format!("FEISHU invalid JSON response: {err}"),
        )
    })?;
    let code = parsed
        .get("code")
        .or_else(|| parsed.get("StatusCode"))
        .and_then(serde_json::Value::as_i64);
    if code != Some(0) {
        let msg = parsed
            .get("msg")
            .or_else(|| parsed.get("StatusMessage"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Err(ChannelFailure::retryable(
            Some(status_code),
            format!("FEISHU business error code={code:?}: {msg}"),
        ));
    }

    Ok(status_code)
}
