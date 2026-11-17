//! `DingTalk` (`DINGTALK`) custom robot signed markdown notification sender (`07-STEP07-08` §08.4).

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
    append_query_param, lookup_secret_or_config, percent_encode_component,
    read_truncated_response_text,
};

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_DINGTALK_BASE_URL: &str = "https://oapi.dingtalk.com/robot/send";

/// Computes the URL-encoded Base64 `DingTalk` robot `sign` query parameter:
/// `urlencode(base64(HMAC_SHA256(key = secret, msg = "{timestamp_ms}\n{secret}")))`.
///
/// # Errors
/// Returns [`AppError::Config`] if HMAC key initialization fails.
pub fn compute_dingtalk_sign(timestamp_ms: i64, secret: &str) -> Result<String, AppError> {
    let string_to_sign = format!("{timestamp_ms}\n{secret}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|err| AppError::Config(format!("invalid DingTalk HMAC key: {err}")))?;
    mac.update(string_to_sign.as_bytes());
    let b64 = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    Ok(percent_encode_component(&b64))
}

/// Builds the signed `DingTalk` webhook URL for `timestamp_ms`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if neither `webhook_url` nor `access_token` is configured.
pub fn resolve_dingtalk_url(
    config: &serde_json::Value,
    secrets: &BTreeMap<String, String>,
    timestamp_ms: i64,
) -> Result<String, AppError> {
    let mut url =
        if let Some(explicit) = lookup_secret_or_config(secrets, config, &["webhook_url", "url"]) {
            let mut base = String::from(explicit);
            if let Some(token) = lookup_secret_or_config(secrets, config, &["access_token"])
                && !base.contains("access_token=")
            {
                base = append_query_param(&base, "access_token", &percent_encode_component(token));
            }
            base
        } else {
            let token =
                lookup_secret_or_config(secrets, config, &["access_token"]).ok_or_else(|| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        "DINGTALK channel requires 'access_token' or 'webhook_url'",
                    )
                })?;
            let base = lookup_secret_or_config(secrets, config, &["base_url"])
                .unwrap_or(DEFAULT_DINGTALK_BASE_URL);
            append_query_param(base, "access_token", &percent_encode_component(token))
        };

    if let Some(secret) = lookup_secret_or_config(
        secrets,
        config,
        &["sign_secret", "signing_secret", "secret"],
    ) {
        let sign = compute_dingtalk_sign(timestamp_ms, secret)?;
        url = append_query_param(&url, "timestamp", &timestamp_ms.to_string());
        url = append_query_param(&url, "sign", &sign);
    }

    Ok(url)
}

/// Sends a single `DINGTALK` markdown notification and verifies `errcode == 0`.
pub(crate) async fn send_dingtalk<U: OutboundUrlGuardPort>(
    channel: &NotifyChannelRecord,
    secrets: &BTreeMap<String, String>,
    view: &NotifyPayloadView<'_>,
    url_guard: &U,
    clients: HttpClientPair<'_>,
) -> Result<i32, ChannelFailure> {
    let ts_ms = view.now.timestamp_millis();
    let raw_url =
        resolve_dingtalk_url(&channel.config, secrets, ts_ms).map_err(ChannelFailure::permanent)?;
    let checked_url = url_guard
        .check_outbound_url(&raw_url, channel.allow_private_network)
        .await
        .map_err(ChannelFailure::from_guard_error)?;

    let at_mobiles: Vec<String> = channel
        .config
        .get("at_mobiles")
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        });

    let mut md_text = if view.body.is_empty() {
        format!("### {}", view.title)
    } else {
        format!("### {}\n{}", view.title, view.body)
    };
    if !at_mobiles.is_empty() {
        let mentions: Vec<String> = at_mobiles.iter().map(|m| format!("@{m}")).collect();
        md_text.push_str("\n\n");
        md_text.push_str(&mentions.join(" "));
    }

    let payload = serde_json::json!({
        "msgtype": "markdown",
        "markdown": {
            "title": view.title,
            "text": md_text,
        },
        "at": {
            "atMobiles": at_mobiles,
            "isAtAll": false,
        },
    });

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
            format!("DINGTALK HTTP {status_code}: {text}"),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        ChannelFailure::retryable(
            Some(status_code),
            format!("DINGTALK invalid JSON response: {err}"),
        )
    })?;
    let errcode = parsed.get("errcode").and_then(serde_json::Value::as_i64);
    if errcode != Some(0) {
        let errmsg = parsed
            .get("errmsg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Err(ChannelFailure::retryable(
            Some(status_code),
            format!("DINGTALK business error errcode={errcode:?}: {errmsg}"),
        ));
    }

    Ok(status_code)
}
