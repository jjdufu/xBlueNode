//! Structured JSON logging, optional OTLP tracing, request IDs, and Prometheus HTTP metrics.

use std::env;
use std::net::SocketAddr;
use std::time::Instant;

use axpoint_domain::AppError;
use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use metrics_exporter_prometheus::PrometheusBuilder;
use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use uuid::Uuid;

tokio::task_local! {
    static CURRENT_REQUEST_ID: String;
}

/// Initializes structured JSON logging, optional OTLP distributed tracing, and Prometheus `/metrics` listener.
///
/// # Errors
/// Returns [`AppError::Config`] if `AXPOINT_METRICS_ADDR`, OTLP exporter setup, or the Prometheus listener fails.
pub(crate) fn init_telemetry() -> Result<Option<SdkTracerProvider>, AppError> {
    let env_filter = EnvFilter::new(
        env::var("RUST_LOG").unwrap_or_else(|_| String::from("axpoint_server=info,info")),
    );

    let otlp_endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let provider = if let Some(endpoint) = otlp_endpoint {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|err| AppError::Config(format!("failed to build OTLP exporter: {err}")))?;
        let sdk_provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .build();
        let tracer = sdk_provider.tracer("axpoint-server");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().json())
            .with(otel_layer)
            .try_init()
            .map_err(|err| {
                AppError::Config(format!("failed to initialize tracing subscriber: {err}"))
            })?;
        Some(sdk_provider)
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()
            .map_err(|err| {
                AppError::Config(format!("failed to initialize tracing subscriber: {err}"))
            })?;
        None
    };

    let metrics_addr_str =
        env::var("AXPOINT_METRICS_ADDR").unwrap_or_else(|_| String::from("127.0.0.1:9464"));
    let metrics_addr: SocketAddr = metrics_addr_str
        .parse()
        .map_err(|err| AppError::Config(format!("invalid AXPOINT_METRICS_ADDR: {err}")))?;

    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .map_err(|err| {
            AppError::Config(format!(
                "failed to install Prometheus exporter on {metrics_addr}: {err}"
            ))
        })?;

    Ok(provider)
}

/// Returns the active OpenTelemetry trace ID, task-local `x-request-id`, or a fresh UUID hex string.
pub(crate) fn current_trace_id() -> String {
    let cx = tracing::Span::current().context();
    let span_ref = cx.span();
    let span_cx = span_ref.span_context();
    if span_cx.is_valid() {
        return span_cx.trace_id().to_string();
    }
    if let Ok(req_id) = CURRENT_REQUEST_ID.try_with(Clone::clone)
        && !req_id.is_empty()
    {
        return req_id;
    }
    Uuid::new_v4().simple().to_string()
}

/// Returns a W3C `traceparent` header value for outbound calls to `axpoint-parser`.
pub(crate) fn outbound_traceparent() -> String {
    let cx = tracing::Span::current().context();
    let span_ref = cx.span();
    let span_cx = span_ref.span_context();
    if span_cx.is_valid() {
        return format!(
            "00-{}-{}-{:02x}",
            span_cx.trace_id(),
            span_cx.span_id(),
            span_cx.trace_flags().to_u8()
        );
    }
    let trace_hex = Uuid::new_v4().simple().to_string();
    let span_full = Uuid::new_v4().simple().to_string();
    let span_hex: String = span_full.chars().take(16).collect();
    format!("00-{trace_hex}-{span_hex}-01")
}

/// Axum middleware recording structured request spans, `trace_id`, and Prometheus HTTP metrics.
pub(crate) async fn http_telemetry_middleware(req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().as_str().to_owned();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| req.uri().path().to_owned(), |mp| mp.as_str().to_owned());
    let req_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map_or_else(|| Uuid::new_v4().simple().to_string(), str::to_owned);

    let span = tracing::info_span!(
        "http_request",
        trace_id = %req_id,
        tenant_id = tracing::field::Empty,
        username = tracing::field::Empty,
        method = %method,
        route = %route,
        status = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
    );

    let resp = CURRENT_REQUEST_ID
        .scope(
            req_id,
            tracing::Instrument::instrument(next.run(req), span.clone()),
        )
        .await;

    let elapsed = started.elapsed();
    let status_code = resp.status().as_u16();
    let status_str = status_code.to_string();
    let latency_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);

    span.record("status", status_code);
    span.record("latency_ms", latency_ms);

    metrics::counter!(
        "axpoint_http_requests_total",
        "route" => route.clone(),
        "status" => status_str
    )
    .increment(1);
    metrics::histogram!(
        "axpoint_http_request_duration_seconds",
        "route" => route
    )
    .record(elapsed.as_secs_f64());

    resp
}
