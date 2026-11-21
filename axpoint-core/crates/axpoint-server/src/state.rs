//! Shared application state, uniform three-field `HttpError`, and strict request extractors.

use std::path::PathBuf;
use std::sync::Arc;

use axpoint_analytics::SessionPool;
use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_llm::{AiGateway, LlmRouter};
use axpoint_security::{AuditWriter, Authenticator, CredentialKeyring};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::agent_bridge::AxAgentServices;
use crate::sidecar::HttpSidecarClient;
use crate::telemetry::current_trace_id;

/// Shared `AxPoint` Core application state injected into Axum handlers.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) store: PgStore,
    pub(crate) ai: AiGateway,
    pub(crate) llm_router: Arc<LlmRouter>,
    pub(crate) axagent: Arc<AxAgentServices>,
    pub(crate) authenticator: Arc<Authenticator<PgStore>>,
    pub(crate) jwt_keys: Arc<axpoint_security::jwt::JwtKeyManager>,
    pub(crate) oidc: Option<Arc<axpoint_security::oidc::OidcService>>,
    pub(crate) cedar: Arc<axpoint_security::cedar_authz::CedarAuthorizer>,
    pub(crate) mask_keys: Arc<axpoint_security::masking::MaskKeyRing>,
    pub(crate) auth_limiters: Arc<crate::routes::auth::AuthRateLimiters>,
    pub(crate) audit: AuditWriter,
    pub(crate) keyring: Arc<CredentialKeyring>,
    pub(crate) sidecar: Arc<HttpSidecarClient>,
    pub(crate) analytics_pool: Arc<SessionPool>,
    pub(crate) view: Arc<axpoint_server::view::ViewServices<PgStore>>,
    pub(crate) ops: Arc<axpoint_server::ops::OpsServices<PgStore, HttpSidecarClient>>,
    pub(crate) data_root: PathBuf,
    pub(crate) parser_base_url: String,
    pub(crate) http: reqwest::Client,
}

/// Public health-check response body for `GET /api/health`.
#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    pub(crate) status: &'static str,
    pub(crate) service: &'static str,
    pub(crate) embedding_model: String,
    pub(crate) embedding_dim: Option<usize>,
}

/// Minimal text-track response decoded from `axpoint-parser` `POST /parse`.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ParserSidecarResponse {
    pub(crate) title: String,
    pub(crate) markdown: String,
}

/// Uniform HTTP error wrapper rendering `{error_code, message, trace_id}`.
pub(crate) struct HttpError(pub(crate) AppError);

impl From<AppError> for HttpError {
    fn from(value: AppError) -> Self {
        Self(value)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AppError::BadRequest { .. } => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::Unsupported(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            AppError::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::Config(raw) | AppError::Database(raw) => {
                tracing::error!(error = %raw, "internal server error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AppError::Llm(raw) | AppError::Parser(raw) | AppError::Upstream(raw) => {
                tracing::warn!(error = %raw, "upstream dependency error");
                StatusCode::BAD_GATEWAY
            }
            AppError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        };
        let trace_id = current_trace_id();
        let body = serde_json::json!({
            "error_code": self.0.error_code(),
            "message": self.0.client_message(),
            "trace_id": trace_id,
        });
        (status, Json(body)).into_response()
    }
}

/// JSON body extractor mapping deserialization / `deny_unknown_fields` errors to `400 INVALID_ARGUMENT`.
pub(crate) struct ApiJson<T>(pub(crate) T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = HttpError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(val)) => Ok(Self(val)),
            Err(rejection) => Err(map_json_rejection(&rejection)),
        }
    }
}

fn map_json_rejection(rejection: &JsonRejection) -> HttpError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return HttpError(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE"));
    }
    HttpError(AppError::bad("INVALID_ARGUMENT", rejection.body_text()))
}

/// Query-string extractor mapping invalid or unknown parameters to `400 INVALID_ARGUMENT`.
pub(crate) struct ApiQuery<T>(pub(crate) T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(val)) => Ok(Self(val)),
            Err(rejection) => Err(map_query_rejection(&rejection)),
        }
    }
}

fn map_query_rejection(rejection: &QueryRejection) -> HttpError {
    HttpError(AppError::bad("INVALID_ARGUMENT", rejection.body_text()))
}

/// Path parameter extractor mapping malformed path segments to `400 INVALID_ARGUMENT`.
pub(crate) struct ApiPath<T>(pub(crate) T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(val)) => Ok(Self(val)),
            Err(rejection) => Err(map_path_rejection(&rejection)),
        }
    }
}

fn map_path_rejection(rejection: &PathRejection) -> HttpError {
    HttpError(AppError::bad("INVALID_ARGUMENT", rejection.body_text()))
}
