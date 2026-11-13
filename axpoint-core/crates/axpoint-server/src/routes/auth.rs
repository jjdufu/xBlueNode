//! Authentication identity routes (`/api/auth/*`).

use axpoint_domain::platform::MeResponse;
use axpoint_security::AuthPrincipal;
use axum::routing::get;
use axum::{Json, Router};

use crate::state::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/auth/me", get(me_handler))
}

async fn me_handler(principal: AuthPrincipal) -> Json<MeResponse> {
    Json(principal.to_me_response())
}
