//! Axum `FromRequestParts` extractor for [`AuthPrincipal`].

use axpoint_domain::AppError;
use axpoint_security::AuthPrincipal;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::state::{AppState, HttpError};

impl FromRequestParts<AppState> for AuthPrincipal {
    type Rejection = HttpError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(raw_query) = parts.uri.query() {
            for pair in raw_query.split('&') {
                let key = pair.split('=').next().unwrap_or_default();
                if key == "tenant_id" {
                    return Err(HttpError(AppError::bad(
                        "INVALID_ARGUMENT",
                        "tenant_id query parameter is forbidden",
                    )));
                }
            }
        }

        let auth_header = match parts.headers.get(AUTHORIZATION) {
            Some(raw) => {
                let s = raw
                    .to_str()
                    .map_err(|_| HttpError(AppError::Unauthorized("TOKEN_INVALID")))?;
                Some(s)
            }
            None => None,
        };

        let principal = state.authenticator.authenticate(auth_header).await?;
        let span = tracing::Span::current();
        span.record("tenant_id", principal.tenant_id.as_str());
        span.record("username", principal.username.as_str());
        Ok(principal)
    }
}
