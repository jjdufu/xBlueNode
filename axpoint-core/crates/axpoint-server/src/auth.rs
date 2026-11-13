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
