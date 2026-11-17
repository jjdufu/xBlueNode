//! Signed HTTP `WEBHOOK` notification channel sender (`07-STEP07-08` §08.4).

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::ports::OutboundUrlGuardPort;
use axpoint_domain::rules::NotifyChannelRecord;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::{
    ChannelFailure, HttpClientPair, NOTIFY_HTTP_TIMEOUT_SECS, NotifyPayloadView,
    lookup_secret_or_config, read_truncated_response_text,
};

type HmacSha256 = Hmac<Sha256>;

/// Computes `X-AxPoint-Signature` (`sha256=<hex(HMAC_SHA256(secret, "{timestamp}." + raw_body))>`)
/// over the exact raw HTTP body bytes (`07-STEP07-08` §08.4).
///
/// # Errors
/// Returns [`AppError::Config`] if the HMAC key initialization fails.
pub fn compute_webhook_signature(
    secret: &str,
    timestamp: &str,
    raw_body: &[u8],
) -> Result<String, AppError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|err| AppError::Config(format!("invalid webhook HMAC key: {err}")))?;
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    let digest = mac.finalize().into_bytes();
    Ok(format!("sha256={}", hex::encode(digest)))
}

/// Serializes the canonical `WEBHOOK` JSON payload bytes for `view`.
///
/// # Errors
/// Returns [`AppError::Config`] if JSON serialization fails.
pub fn build_webhook_body_bytes(view: &NotifyPayloadView<'_>) -> Result<Vec<u8>, AppError> {
    let payload = serde_json::json!({
        "event": view.event.as_str(),
        "tenant_id": view.tenant_id,
        "timestamp": view.now.timestamp(),
        "sent_at": view.now.to_rfc3339(),
        "delivery_id": view.delivery_id,
        "title": view.title,
        "body": view.body,
        "alert": view.alert,
        "rule": view.rule,
    });
    serde_json::to_vec(&payload)
        .map_err(|err| AppError::Config(format!("serialize webhook notification payload: {err}")))
}

/// Sends a single `WEBHOOK` notification after validating the target URL via `url_guard`.
pub(crate) async fn send_webhook<U: OutboundUrlGuardPort>(
    channel: &NotifyChannelRecord,
    secrets: &BTreeMap<String, String>,
    view: &NotifyPayloadView<'_>,
    url_guard: &U,
    clients: HttpClientPair<'_>,
) -> Result<i32, ChannelFailure> {
    let raw_url = lookup_secret_or_config(secrets, &channel.config, &["url", "webhook_url"])
        .ok_or_else(|| {
            ChannelFailure::permanent(AppError::bad(
                "INVALID_ARGUMENT",
                "WEBHOOK channel requires non-empty 'url'",
            ))
        })?;

    let checked_url = url_guard
        .check_outbound_url(raw_url, channel.allow_private_network)
        .await
        .map_err(ChannelFailure::from_guard_error)?;

    let raw_body = build_webhook_body_bytes(view).map_err(ChannelFailure::permanent)?;
    let ts_str = view.now.timestamp().to_string();
    let delivery_str = view.delivery_id.to_string();
    let client = clients.select(&checked_url, channel.allow_private_network);

    let mut req = client
        .post(&checked_url)
        .timeout(Duration::from_secs(NOTIFY_HTTP_TIMEOUT_SECS));

    let mut has_custom_auth = false;
    if let Some(serde_json::Value::Object(hdrs)) = channel.config.get("headers") {
        for (name, val) in hdrs {
            if let Some(val_str) = val.as_str() {
                if name.eq_ignore_ascii_case("authorization") {
                    has_custom_auth = true;
                }
                req = req.header(name.as_str(), val_str);
            }
        }
    }

    if !has_custom_auth
        && let Some(token) = lookup_secret_or_config(
            secrets,
            &channel.config,
            &["bearer_token", "auth_token", "token"],
        )
    {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    req = req
        .header("Content-Type", "application/json")
        .header("X-AxPoint-Event", view.event.as_str())
        .header("X-AxPoint-Timestamp", &ts_str)
        .header("X-AxPoint-Delivery", &delivery_str)
        .header("Idempotency-Key", &delivery_str);

    if let Some(secret) = lookup_secret_or_config(
        secrets,
        &channel.config,
        &["signing_secret", "sign_secret", "secret"],
    ) {
        let signature = compute_webhook_signature(secret, &ts_str, &raw_body)
            .map_err(ChannelFailure::permanent)?;
        req = req.header("X-AxPoint-Signature", signature);
    }

    let resp = req
        .body(raw_body)
        .send()
        .await
        .map_err(|err| ChannelFailure::from_reqwest(&err))?;

    let status_u16 = resp.status().as_u16();
    let status_code = i32::from(status_u16);
    if !resp.status().is_success() {
        let snippet = read_truncated_response_text(resp).await;
        return Err(ChannelFailure::retryable(
            Some(status_code),
            format!("WEBHOOK returned HTTP {status_code}: {snippet}"),
        ));
    }

    Ok(status_code)
}
