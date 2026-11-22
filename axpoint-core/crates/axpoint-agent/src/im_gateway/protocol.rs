//! Outbound `ws://` / `wss://` WebSocket transport dialer with `NetGuard` SSRF preflight,
//! `rustls` TLS wrapping, and platform-neutral frame envelopes (`07-STEP21` §21.2.1).

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::im::ImInboundMessage;
use axpoint_domain::ports::OutboundUrlGuardPort;
use axpoint_security::net_guard::safe_redirect_policy;
use rustls_pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;

const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TICKET_TIMEOUT: Duration = Duration::from_secs(10);

/// Underlying TCP or `rustls` TLS socket backing an outbound [`ImWsStream`].
pub enum ImSocketStream {
    /// Unencrypted `ws://` TCP stream (used for private intranet or loopback gate tests).
    Plain(TcpStream),
    /// TLS-encrypted `wss://` stream verified against `webpki_roots`.
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for ImSocketStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_read(cx, buf),
            Self::Tls(tls) => Pin::new(tls.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ImSocketStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_write(cx, buf),
            Self::Tls(tls) => Pin::new(tls.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_flush(cx),
            Self::Tls(tls) => Pin::new(tls.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(tcp) => Pin::new(tcp).poll_shutdown(cx),
            Self::Tls(tls) => Pin::new(tls.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Type alias for an established outbound RFC-6455 [`WebSocketStream`].
pub type ImWsStream = WebSocketStream<ImSocketStream>;

/// Resolved WebSocket dial parameters produced after optional ticket exchange.
#[derive(Debug, Clone)]
pub struct PlatformDialPlan {
    /// Target `ws://` or `wss://` URL.
    pub ws_url: String,
    /// Sanitized `scheme://host[:port]/path` preview safe for UI/logging.
    pub endpoint_preview: String,
    /// Application-level ping interval.
    pub ping_interval: Duration,
    /// Optional initial JSON auth frame sent immediately upon WebSocket upgrade.
    pub subscribe_frame: Option<String>,
}

/// Decoded action from a single inbound WebSocket/Stream frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedPlatformFrame {
    /// Frame parsed successfully with optional immediate ACK and optional inbound chat turn.
    Handled {
        /// Protocol-level ACK/pong JSON frame to write back within `< 3s`.
        immediate_ack: Option<String>,
        /// Normalized inbound chat/mention message for `AgentRuntime`.
        inbound: Option<ImInboundMessage>,
    },
    /// Heartbeat `pong` or non-message notification requiring no action.
    Ignore,
}

/// Normalizes a `ws://`, `wss://`, `http://`, or `https://` URL into an `http://` or `https://`
/// equivalent accepted by [`OutboundUrlGuardPort`].
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when the URL uses an unsupported scheme.
pub fn normalize_url_for_guard(raw_url: &str) -> Result<String, AppError> {
    let trimmed = raw_url.trim();
    if let Some(rest) = trimmed.strip_prefix("ws://") {
        return Ok(format!("http://{rest}"));
    }
    if let Some(rest) = trimmed.strip_prefix("wss://") {
        return Ok(format!("https://{rest}"));
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(String::from(trimmed));
    }
    Err(AppError::bad(
        "INVALID_ARGUMENT",
        "IM endpoint URL must start with http://, https://, ws://, or wss://",
    ))
}

/// Formats a sanitized `scheme://host[:port]/path` endpoint preview with query parameters stripped.
#[must_use]
pub fn sanitize_endpoint_preview(raw_url: &str) -> String {
    let Ok(parsed) = url::Url::parse(raw_url.trim()) else {
        return String::from("<invalid-url>");
    };
    let host = parsed.host_str().unwrap_or("unknown");
    match parsed.port() {
        Some(port) => format!("{}://{host}:{port}{}", parsed.scheme(), parsed.path()),
        None => format!("{}://{host}{}", parsed.scheme(), parsed.path()),
    }
}

/// Builds an SSRF-safe `reqwest::Client` for IM ticket exchange requests.
///
/// # Errors
/// Returns [`AppError::Config`] if constructing the TLS HTTP client fails.
pub fn build_im_http_client(allow_private_network: bool) -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(HTTP_TICKET_TIMEOUT)
        .redirect(safe_redirect_policy(allow_private_network, 3))
        .build()
        .map_err(|err| AppError::Config(format!("build IM HTTP client failed: {err}")))
}

/// Validates `ws_url` through `url_guard`, establishes a TCP (+ optional `rustls` TLS) connection,
/// and performs the RFC-6455 WebSocket client handshake.
///
/// # Errors
/// Returns [`AppError::Forbidden`] if `url_guard` blocks the target host/IP, [`AppError::BadRequest`]
/// on malformed URLs, [`AppError::Timeout`] on connect timeout, or [`AppError::Upstream`] on handshake error.
pub async fn dial_websocket(
    ws_url: &str,
    url_guard: &dyn OutboundUrlGuardPort,
    allow_private_network: bool,
) -> Result<ImWsStream, AppError> {
    let trimmed = ws_url.trim();
    let http_equiv = normalize_url_for_guard(trimmed)?;
    let _checked = url_guard
        .check_outbound_url(&http_equiv, allow_private_network)
        .await?;

    let parsed = url::Url::parse(trimmed)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid WS URL: {err}")))?;
    let is_tls = match parsed.scheme() {
        "ws" => false,
        "wss" => true,
        other => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("WebSocket URL scheme must be ws or wss, got '{other}'"),
            ));
        }
    };
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "WebSocket URL missing host"))?;
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if is_tls { 443 } else { 80 });

    let unbracketed = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);

    let tcp = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect((unbracketed, port)))
        .await
        .map_err(|_| AppError::Timeout("IM_WS_CONNECT_TIMEOUT"))?
        .map_err(|err| AppError::Upstream(format!("IM TCP connect failed: {err}")))?;
    if let Err(err) = tcp.set_nodelay(true) {
        tracing::debug!(error = %err, "TCP_NODELAY skipped on IM socket");
    }

    let socket = if is_tls {
        let tls = wrap_rustls_stream(unbracketed, tcp).await?;
        ImSocketStream::Tls(Box::new(tls))
    } else {
        ImSocketStream::Plain(tcp)
    };

    let (ws_stream, _resp) = tokio::time::timeout(
        DIAL_TIMEOUT,
        tokio_tungstenite::client_async(trimmed, socket),
    )
    .await
    .map_err(|_| AppError::Timeout("IM_WS_HANDSHAKE_TIMEOUT"))?
    .map_err(|err| AppError::Upstream(format!("IM WebSocket handshake failed: {err}")))?;

    Ok(ws_stream)
}

async fn wrap_rustls_stream(host: &str, tcp: TcpStream) -> Result<TlsStream<TcpStream>, AppError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls_cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| AppError::Config(format!("rustls protocol config failed: {err}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();

    let server_name = ServerName::try_from(host.to_owned()).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid TLS SNI server name '{host}': {err}"),
        )
    })?;
    let connector = TlsConnector::from(Arc::new(tls_cfg));
    tokio::time::timeout(DIAL_TIMEOUT, connector.connect(server_name, tcp))
        .await
        .map_err(|_| AppError::Timeout("IM_TLS_HANDSHAKE_TIMEOUT"))?
        .map_err(|err| AppError::Upstream(format!("IM TLS handshake failed: {err}")))
}

/// Strips a leading `@mention` token (e.g. `@_user_1` or `@机器人`) from an inbound group/DM prompt.
#[must_use]
pub fn strip_leading_bot_mention(raw: &str) -> String {
    let mut rest = raw.trim();
    while let Some(after_at) = rest.strip_prefix('@') {
        let Some((mention_tok, tail)) = after_at.split_once(char::is_whitespace) else {
            return String::new();
        };
        if mention_tok.is_empty() || mention_tok.contains('@') {
            break;
        }
        rest = tail.trim_start();
    }
    String::from(rest)
}
