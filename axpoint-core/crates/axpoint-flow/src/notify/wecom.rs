//! Enterprise `WeChat` (`WECOM`) group bot markdown notification sender (`07-STEP07-08` §08.4).

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::ports::OutboundUrlGuardPort;
use axpoint_domain::rules::NotifyChannelRecord;

use super::{
    ChannelFailure, HttpClientPair, NOTIFY_HTTP_TIMEOUT_SECS, NotifyPayloadView,
    append_query_param, lookup_secret_or_config, percent_encode_component,
    read_truncated_response_text,
};

const DEFAULT_WECOM_BASE_URL: &str = "https://qyapi.weixin.qq.com/cgi-bin/webhook/send";

/// Builds the target `WECOM` webhook URL from `config` and `secrets`.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if neither `webhook_url` nor `webhook_key` is configured.
pub fn resolve_wecom_url(
    config: &serde_json::Value,
    secrets: &BTreeMap<String, String>,
) -> Result<String, AppError> {
    if let Some(explicit_url) = lookup_secret_or_config(secrets, config, &["webhook_url", "url"]) {
        if let Some(key) = lookup_secret_or_config(secrets, config, &["webhook_key", "key"])
            && !explicit_url.contains("key=")
        {
            return Ok(append_query_param(
                explicit_url,
                "key",
                &percent_encode_component(key),
            ));
        }
        return Ok(String::from(explicit_url));
    }

    let key =
        lookup_secret_or_config(secrets, config, &["webhook_key", "key"]).ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "WECOM channel requires 'webhook_key' secret",
            )
        })?;
    let base =
        lookup_secret_or_config(secrets, config, &["base_url"]).unwrap_or(DEFAULT_WECOM_BASE_URL);
    Ok(append_query_param(
        base,
        "key",
        &percent_encode_component(key),
    ))
}

/// Sends a single `WECOM` markdown bot notification and verifies `errcode == 0`.
pub(crate) async fn send_wecom<U: OutboundUrlGuardPort>(
    channel: &NotifyChannelRecord,
    secrets: &BTreeMap<String, String>,
    view: &NotifyPayloadView<'_>,
    url_guard: &U,
    clients: HttpClientPair<'_>,
) -> Result<i32, ChannelFailure> {
    let raw_url = resolve_wecom_url(&channel.config, secrets).map_err(ChannelFailure::permanent)?;
    let checked_url = url_guard
        .check_outbound_url(&raw_url, channel.allow_private_network)
        .await
        .map_err(ChannelFailure::from_guard_error)?;

    let content = if view.body.is_empty() {
        format!("### {}", view.title)
    } else {
        format!("### {}\n{}", view.title, view.body)
    };
    let payload = serde_json::json!({
        "msgtype": "markdown",
        "markdown": {
            "content": content,
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
            format!("WECOM HTTP {status_code}: {text}"),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        ChannelFailure::retryable(
            Some(status_code),
            format!("WECOM invalid JSON response: {err}"),
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
            format!("WECOM business error errcode={errcode:?}: {errmsg}"),
        ));
    }

    Ok(status_code)
}
