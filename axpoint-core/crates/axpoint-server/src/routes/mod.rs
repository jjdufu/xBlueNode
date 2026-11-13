//! `/api/*` router aggregation and JSON `404 ROUTE_NOT_FOUND` fallback.

mod auth;
mod axflow;
mod axops;
mod axsim;
mod documents;
mod system;
mod xblue;

use axpoint_domain::AppError;
use axum::Router;

use crate::state::{AppState, HttpError};

pub(crate) fn api_router(state: AppState) -> Router {
    let api = Router::new()
        .merge(system::router())
        .merge(auth::router())
        .merge(documents::router())
        .merge(xblue::router())
        .merge(axflow::router())
        .merge(axsim::router())
        .merge(axops::router())
        .fallback(|| async { HttpError(AppError::NotFound("ROUTE_NOT_FOUND")) });
    Router::new().nest("/api", api).with_state(state)
}
