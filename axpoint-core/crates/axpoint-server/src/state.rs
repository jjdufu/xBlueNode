use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_llm::AiGateway;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) store: PgStore,
    pub(crate) ai: AiGateway,
    pub(crate) parser_base_url: String,
    pub(crate) http: reqwest::Client,
}

#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    pub(crate) status: &'static str,
    pub(crate) service: &'static str,
    pub(crate) embedding_model: String,
    pub(crate) embedding_dim: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TenantQuery {
    #[serde(default = "default_tenant")]
    pub(crate) tenant_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ParserSidecarResponse {
    pub(crate) title: String,
    pub(crate) markdown: String,
}

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
            AppError::Llm(_) | AppError::Parser(_) | AppError::Upstream(_) => {
                StatusCode::BAD_GATEWAY
            }
            AppError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        };
        let trace_id = uuid::Uuid::new_v4().simple().to_string();
        let body = serde_json::json!({
            "error_code": self.0.error_code(),
            "message": self.0.client_message(),
            "trace_id": trace_id,
        });
        (status, Json(body)).into_response()
    }
}

pub(crate) fn default_tenant() -> String {
    String::from("default")
}
