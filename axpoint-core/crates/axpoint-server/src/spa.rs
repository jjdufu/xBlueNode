//! SPA static asset hosting, `Cache-Control` policy, upload body limits, and router unit tests.

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::middleware::{self, Next};
use axum::response::Response;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::routes;
use crate::state::AppState;
use crate::telemetry::http_telemetry_middleware;

const CACHE_IMMUTABLE: HeaderValue =
    HeaderValue::from_static("public, max-age=31536000, immutable");
const CACHE_NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");

/// Builds the complete `AxPoint` Core HTTP application router with API routes, SPA fallback, and middleware.
pub(crate) fn build_app(state: AppState, web_dist_dir: &str, max_upload_bytes: usize) -> Router {
    let index_path = format!("{}/index.html", web_dist_dir.trim_end_matches('/'));
    let static_files = ServeDir::new(web_dist_dir).fallback(ServeFile::new(index_path));

    routes::api_router(state)
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::max(max_upload_bytes))
        .layer(middleware::from_fn(cache_control_middleware))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .layer(middleware::from_fn(http_telemetry_middleware))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

async fn cache_control_middleware(req: Request, next: Next) -> Response {
    let is_immutable_asset = req.uri().path().starts_with("/assets/");
    let mut resp = next.run(req).await;
    if !resp.headers().contains_key(CACHE_CONTROL) {
        let header_val = if is_immutable_asset {
            CACHE_IMMUTABLE
        } else {
            CACHE_NO_CACHE
        };
        resp.headers_mut().insert(CACHE_CONTROL, header_val);
    }
    resp
}
