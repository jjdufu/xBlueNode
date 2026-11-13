mod axflow;
mod axsim;
mod documents;
mod system;
mod xblue;

use axum::Router;

use crate::state::AppState;

pub(crate) fn api_router(state: AppState) -> Router {
    Router::new()
        .merge(system::router())
        .merge(documents::router())
        .merge(xblue::router())
        .merge(axflow::router())
        .merge(axsim::router())
        .with_state(state)
}
