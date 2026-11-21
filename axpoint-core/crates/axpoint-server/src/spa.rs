//! SPA static asset hosting, `Cache-Control` policy, share no-leak headers, and upload body limits.

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue, REFERRER_POLICY};
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
const NO_REFERRER: HeaderValue = HeaderValue::from_static("no-referrer");
const NOINDEX: HeaderValue = HeaderValue::from_static("noindex");
const X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");

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
    let path = req.uri().path();
    let is_immutable_asset = path.starts_with("/assets/");
    let is_share_route = path == "/share"
        || path.starts_with("/share/")
        || path.starts_with("/api/xblueview/share/");
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    if !headers.contains_key(CACHE_CONTROL) {
        let header_val = if is_immutable_asset {
            CACHE_IMMUTABLE
        } else {
            CACHE_NO_CACHE
        };
        headers.insert(CACHE_CONTROL, header_val);
    }
    if is_share_route {
        headers.insert(REFERRER_POLICY, NO_REFERRER);
        headers.insert(X_ROBOTS_TAG, NOINDEX);
    }
    resp
}
