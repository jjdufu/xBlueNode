//! Public PAT-authenticated capability invocation routes (`/api/published/{env}/{tenant_id}/{slug}*`, `08-STEP09` §09.3.2).

use std::convert::Infallible;

use axpoint_agent::{PublishedCallContext, PublishedInvokeOutcome, PublishedRouteTarget};
use axpoint_domain::AppError;
use axpoint_domain::agent::AgentEvent;
use axpoint_domain::enums::{EndpointKind, Environment};
use axpoint_domain::flow::RunRecord;
use axpoint_domain::ports::AgentRepository as _;
use axpoint_llm::sha256_hex;
use axpoint_security::AuthPrincipal;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::state::{ApiPath, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Builds the `/published/*` sub-router mounted under `/api`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/published/{env}/{tenant_id}/{slug}",
            post(invoke_published_handler),
        )
        .route(
            "/published/{env}/{tenant_id}/{slug}/runs/{run_id}",
            get(get_published_run_handler),
        )
}

fn parse_env_segment(raw: &str) -> Result<Environment, AppError> {
    match raw.trim() {
        s if s.eq_ignore_ascii_case("test") => Ok(Environment::Test),
        s if s.eq_ignore_ascii_case("prod") => Ok(Environment::Prod),
        other => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid environment segment '{other}': expected 'test' or 'prod'"),
        )),
    }
}

fn parse_json_or_empty(body: &Bytes) -> Result<serde_json::Value, AppError> {
    if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid JSON payload: {err}")))
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| String::from("127.0.0.1"), String::from)
}

fn extract_token_rate_key(headers: &HeaderMap, user_id: Uuid) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| user_id.to_string(), sha256_hex)
}

fn wants_sse(headers: &HeaderMap) -> bool {
    headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.to_ascii_lowercase().contains("text/event-stream"))
}

pub(crate) fn agent_events_to_sse(rx: mpsc::Receiver<AgentEvent>) -> Response {
    let stream = futures::stream::unfold(rx, |mut receiver| async move {
        let ev = receiver.recv().await?;
        let name = ev.sse_event_name();
        let data = serde_json::to_string(&ev).unwrap_or_else(|_| String::from("{}"));
        let item: Result<Event, Infallible> = Ok(Event::default().event(name).data(data));
        Some((item, receiver))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn invoke_published_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((env_raw, tenant_id, slug)): ApiPath<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, HttpError> {
    let environment = parse_env_segment(&env_raw)?;
    let payload = parse_json_or_empty(&body)?;
    let ctx = PublishedCallContext {
        rate_limit_key: extract_token_rate_key(&headers, principal.user_id),
        idempotency_key: headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        client_ip: extract_client_ip(&headers),
        trace_id: current_trace_id(),
    };

    if wants_sse(&headers)
        && principal.tenant_id == tenant_id
        && let Some(ep) = state
            .store
            .get_endpoint_by_slug(&tenant_id, environment, &slug)
            .await?
        && ep.enabled
        && ep.kind == EndpointKind::Agent
    {
        principal.require(ep.required_scope)?;
        let (tx, rx) = mpsc::channel(64);
        let publisher = std::sync::Arc::clone(&state.axagent.publisher);
        let audit = state.audit.clone();
        tokio::spawn(async move {
            let target = PublishedRouteTarget {
                tenant_id: &tenant_id,
                environment,
                slug: &slug,
            };
            if let Err(err) = publisher
                .invoke_published_with_stream(&principal, target, payload, ctx, Some(tx.clone()))
                .await
            {
                let _closed = tx
                    .send(AgentEvent::Error {
                        error_code: String::from(err.error_code()),
                        message: err.client_message(),
                    })
                    .await
                    .is_err();
            }
            if let Err(err) = audit.flush().await {
                tracing::warn!(error = %err, "published SSE audit flush failed");
            }
        });
        return Ok(agent_events_to_sse(rx));
    }

    let outcome = state
        .axagent
        .publisher
        .invoke_published(&principal, &tenant_id, environment, &slug, payload, ctx)
        .await?;
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "published invoke audit flush failed");
    }

    match outcome {
        PublishedInvokeOutcome::Json(val) => Ok((StatusCode::OK, Json(val)).into_response()),
        start @ PublishedInvokeOutcome::StartedRun { .. } => {
            Ok((StatusCode::ACCEPTED, Json(start.into_json_value())).into_response())
        }
    }
}

async fn get_published_run_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((env_raw, tenant_id, slug, run_id)): ApiPath<(String, String, String, Uuid)>,
) -> Result<Json<RunRecord>, HttpError> {
    let environment = parse_env_segment(&env_raw)?;
    let run = state
        .axagent
        .publisher
        .get_published_flow_run(&principal, &tenant_id, environment, &slug, run_id)
        .await?;
    Ok(Json(run))
}
