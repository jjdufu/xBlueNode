//! SMTP `EMAIL` notification channel sender backed by `lettre` (`07-STEP07-08` §08.4).

use std::collections::BTreeMap;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::ports::OutboundUrlGuardPort;
use axpoint_domain::rules::NotifyChannelRecord;
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::template::escape_html;
use super::{ChannelFailure, NOTIFY_HTTP_TIMEOUT_SECS, NotifyPayloadView, lookup_secret_or_config};

struct ParsedEmailConfig {
    smtp_host: String,
    smtp_port: u16,
    starttls: bool,
    from: Mailbox,
    recipients: Vec<Mailbox>,
    subject_prefix: Option<String>,
}

fn parse_email_config(
    channel: &NotifyChannelRecord,
    secrets: &BTreeMap<String, String>,
) -> Result<ParsedEmailConfig, AppError> {
    let smtp_host = lookup_secret_or_config(secrets, &channel.config, &["smtp_host", "host"])
        .map(str::trim)
        .filter(|h| !h.is_empty() && !h.contains('/') && !h.chars().any(char::is_whitespace))
        .map(String::from)
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "EMAIL channel requires valid 'smtp_host'",
            )
        })?;

    let starttls = channel
        .config
        .get("starttls")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    let default_port: u16 = if starttls { 587 } else { 25 };
    let smtp_port = match channel
        .config
        .get("smtp_port")
        .or_else(|| channel.config.get("port"))
    {
        None | Some(serde_json::Value::Null) => default_port,
        Some(val) => val
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|&p| p > 0)
            .ok_or_else(|| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    "EMAIL channel 'smtp_port' must be in 1..=65535",
                )
            })?,
    };

    let from_raw = lookup_secret_or_config(secrets, &channel.config, &["from", "sender"])
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "EMAIL channel requires 'from'"))?;
    let from: Mailbox = from_raw.parse().map_err(|err| {
        AppError::bad("INVALID_ARGUMENT", format!("invalid 'from' mailbox: {err}"))
    })?;

    let recipients = parse_recipient_mailboxes(&channel.config)?;
    let subject_prefix = channel
        .config
        .get("subject_prefix")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    Ok(ParsedEmailConfig {
        smtp_host,
        smtp_port,
        starttls,
        from,
        recipients,
        subject_prefix,
    })
}

fn parse_recipient_mailboxes(config: &serde_json::Value) -> Result<Vec<Mailbox>, AppError> {
    let mut raw_addrs: Vec<String> = Vec::new();
    match config.get("to") {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    raw_addrs.push(String::from(s));
                }
            }
        }
        Some(serde_json::Value::String(csv)) => {
            for part in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                raw_addrs.push(String::from(part));
            }
        }
        _ => {}
    }
    if raw_addrs.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "EMAIL channel requires at least one recipient in 'to'",
        ));
    }

    let mut out = Vec::with_capacity(raw_addrs.len());
    for addr in raw_addrs {
        let mb: Mailbox = addr.parse().map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid recipient '{addr}': {err}"),
            )
        })?;
        out.push(mb);
    }
    Ok(out)
}

/// Builds the multipart `plain` + `html` [`Message`] for `view`.
fn build_email_message(
    cfg: &ParsedEmailConfig,
    view: &NotifyPayloadView<'_>,
) -> Result<Message, AppError> {
    let clean_title: String = view
        .title
        .chars()
        .filter(|&c| c != '\r' && c != '\n')
        .collect();
    let subject = match &cfg.subject_prefix {
        Some(prefix) => format!("{prefix} {clean_title}"),
        None => clean_title.clone(),
    };

    let html_title = escape_html(&clean_title);
    let html_body = escape_html(view.body).replace('\n', "<br/>");
    let html_doc = format!(
        "<!DOCTYPE html><html><body><h3>{html_title}</h3><div>{html_body}</div></body></html>"
    );
    let plain_doc = if view.body.is_empty() {
        clean_title
    } else {
        format!("{clean_title}\n\n{}", view.body)
    };

    let mut builder = Message::builder().from(cfg.from.clone()).subject(subject);
    for rcpt in &cfg.recipients {
        builder = builder.to(rcpt.clone());
    }
    builder
        .multipart(MultiPart::alternative_plain_html(plain_doc, html_doc))
        .map_err(|err| AppError::Config(format!("build EMAIL message failed: {err}")))
}

/// Sends an `EMAIL` notification via `AsyncSmtpTransport::<Tokio1Executor>` with a `10s` timeout.
pub(crate) async fn send_email<U: OutboundUrlGuardPort>(
    channel: &NotifyChannelRecord,
    secrets: &BTreeMap<String, String>,
    view: &NotifyPayloadView<'_>,
    url_guard: &U,
) -> Result<i32, ChannelFailure> {
    let cfg = parse_email_config(channel, secrets).map_err(ChannelFailure::permanent)?;

    let synthetic_url = format!("http://{}:{}", cfg.smtp_host, cfg.smtp_port);
    let _checked = url_guard
        .check_outbound_url(&synthetic_url, channel.allow_private_network)
        .await
        .map_err(ChannelFailure::from_guard_error)?;

    let message = build_email_message(&cfg, view).map_err(ChannelFailure::permanent)?;
    let timeout_dur = Duration::from_secs(NOTIFY_HTTP_TIMEOUT_SECS);

    let mut transport_builder = if cfg.starttls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host).map_err(|err| {
            ChannelFailure::permanent(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid SMTP relay host '{}': {err}", cfg.smtp_host),
            ))
        })?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
    };

    transport_builder = transport_builder
        .port(cfg.smtp_port)
        .timeout(Some(timeout_dur));

    if let Some(username) =
        lookup_secret_or_config(secrets, &channel.config, &["smtp_username", "username"])
    {
        let password =
            lookup_secret_or_config(secrets, &channel.config, &["smtp_password", "password"])
                .unwrap_or_default();
        transport_builder = transport_builder.credentials(Credentials::new(
            String::from(username),
            String::from(password),
        ));
    }

    let mailer = transport_builder.build();
    let send_res = tokio::time::timeout(timeout_dur, mailer.send(message)).await;
    match send_res {
        Ok(Ok(_resp)) => Ok(250),
        Ok(Err(err)) => Err(ChannelFailure::retryable(
            None,
            format!("SMTP delivery failed: {err}"),
        )),
        Err(_) => Err(ChannelFailure::retryable(
            None,
            String::from("SMTP delivery timed out after 10s"),
        )),
    }
}
