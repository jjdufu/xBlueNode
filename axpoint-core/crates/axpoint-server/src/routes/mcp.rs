//! Streamable HTTP Model Context Protocol (`MCP`) server routes (`/api/mcp`, `08-STEP09` §09.4.1).

use std::convert::Infallible;

use axpoint_domain::AppError;
use axpoint_security::AuthPrincipal;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use crate::state::{AppState, HttpError};
use crate::telemetry::current_trace_id;

const MCP_SESSION_HEADER: &str = "mcp-session-id";

/// Builds the `/mcp` sub-router mounted under `/api`.
pub(crate) fn router() -> Router<AppState> {
    Router::new().route(
        "/mcp",
        post(mcp_post_handler)
            .get(mcp_get_sse_handler)
            .delete(mcp_delete_handler),
    )
}

fn read_session_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(MCP_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
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

async fn mcp_post_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, HttpError> {
    let raw_body: serde_json::Value = serde_json::from_slice(&body).map_err(|err| {
        HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid JSON-RPC body: {err}"),
        ))
    })?;
    let session_id = read_session_header(&headers);
    let client_ip = extract_client_ip(&headers);
    let trace_id = current_trace_id();

    let out = state
        .axagent
        .mcp_hub
        .handle_post(&principal, session_id, raw_body, &client_ip, &trace_id)
        .await?;

    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "mcp audit flush failed");
    }

    let status = StatusCode::from_u16(out.status_code).unwrap_or(StatusCode::OK);
    let mut response = match out.body {
        Some(json_val) => (status, Json(json_val)).into_response(),
        None => status.into_response(),
    };

    if let Some(sid) = out.session_id
        && let Ok(hval) = HeaderValue::from_str(&sid)
    {
        response.headers_mut().insert("Mcp-Session-Id", hval);
    }
    Ok(response)
}

async fn mcp_get_sse_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    let session_id = read_session_header(&headers);
    let rx = state
        .axagent
        .mcp_hub
        .open_sse_stream(&principal, session_id)
        .await?;

    let stream = futures::stream::unfold(rx, |mut receiver| async move {
        let msg = receiver.recv().await?;
        let text = serde_json::to_string(&msg).unwrap_or_else(|_| String::from("{}"));
        let item: Result<Event, Infallible> = Ok(Event::default().event("message").data(text));
        Some((item, receiver))
    });
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn mcp_delete_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
) -> Result<StatusCode, HttpError> {
    let session_id = read_session_header(&headers);
    state
        .axagent
        .mcp_hub
        .close_session(&principal, session_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
