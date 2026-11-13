//! Bearer credential verifier (`Authenticator<C>`) resolving `AuthPrincipal` instances.

use std::fmt;

use axpoint_domain::AppError;
use axpoint_domain::enums::TokenKind;
use axpoint_domain::ports::CredentialStore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::principal::AuthPrincipal;

/// Minimum byte length required for `AXPOINT_DEV_TOKEN` when configured.
const MIN_DEV_TOKEN_LEN: usize = 32;

/// Bearer token authenticator backed by a [`CredentialStore`] port.
#[derive(Clone)]
pub struct Authenticator<C> {
    store: C,
    env_mode: String,
    dev_token: Option<String>,
}

impl<C> fmt::Debug for Authenticator<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authenticator")
            .field("env_mode", &self.env_mode)
            .field("dev_token_configured", &self.dev_token.is_some())
            .finish_non_exhaustive()
    }
}

impl<C> Authenticator<C> {
    /// Constructs a new [`Authenticator`] and validates environment safety invariants.
    ///
    /// # Errors
    /// - Returns [`AppError::Config`] if `axpoint_env` is `"production"` and `dev_token` is non-empty.
    /// - Returns [`AppError::Config`] if `dev_token` is configured in non-production with fewer than 32 bytes.
    pub fn new(
        store: C,
        axpoint_env: impl Into<String>,
        dev_token: Option<String>,
    ) -> Result<Self, AppError> {
        let env_mode = axpoint_env.into();
        let normalized_dev_token = dev_token.filter(|tok| !tok.is_empty());

        if env_mode.eq_ignore_ascii_case("production") && normalized_dev_token.is_some() {
            return Err(AppError::Config(String::from(
                "AXPOINT_DEV_TOKEN cannot be configured when AXPOINT_ENV=production",
            )));
        }

        if let Some(ref tok) = normalized_dev_token
            && tok.len() < MIN_DEV_TOKEN_LEN
        {
            return Err(AppError::Config(format!(
                "AXPOINT_DEV_TOKEN must be at least {MIN_DEV_TOKEN_LEN} bytes"
            )));
        }

        Ok(Self {
            store,
            env_mode,
            dev_token: normalized_dev_token,
        })
    }

    /// Constructs an [`Authenticator`] reading `AXPOINT_ENV` and `AXPOINT_DEV_TOKEN` from process environment.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if the environment variables violate production or minimum-length policies.
    pub fn from_env(store: C) -> Result<Self, AppError> {
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        let dev_token = std::env::var("AXPOINT_DEV_TOKEN").ok();
        Self::new(store, env_mode, dev_token)
    }

    /// Returns the configured environment mode string (e.g., `"development"`, `"test"`, `"production"`).
    #[must_use]
    pub fn env_mode(&self) -> &str {
        &self.env_mode
    }
}

impl<C> Authenticator<C>
where
    C: CredentialStore + Clone + 'static,
{
    /// Authenticates an HTTP `Authorization` header value (`Bearer <token>`) into an [`AuthPrincipal`].
    ///
    /// # Errors
    /// - Returns `AppError::Unauthorized("TOKEN_MISSING")` if the header is absent, malformed, or empty.
    /// - Returns `AppError::Unauthorized("TOKEN_INVALID")` if the token is unknown, revoked, expired, or its user is inactive.
    /// - Returns [`AppError::Database`] if underlying [`CredentialStore`] queries fail.
    pub async fn authenticate(&self, auth_header: Option<&str>) -> Result<AuthPrincipal, AppError> {
        let raw_header = auth_header
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or(AppError::Unauthorized("TOKEN_MISSING"))?;

        let token = raw_header
            .strip_prefix("Bearer ")
            .or_else(|| raw_header.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(AppError::Unauthorized("TOKEN_MISSING"))?;

        if !self.env_mode.eq_ignore_ascii_case("production")
            && let Some(ref expected_dev) = self.dev_token
            && constant_time_token_eq(token, expected_dev)
        {
            return Ok(AuthPrincipal::dev_static());
        }

        if token.starts_with("axp_") {
            let token_hash = sha256_hex(token);
            let tok = self
                .store
                .resolve_api_token(&token_hash)
                .await?
                .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;

            let principal = self
                .store
                .load_principal(&tok.tenant_id, tok.user_id)
                .await?
                .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;

            let permissions = if tok.scopes.is_empty() {
                principal.permissions
            } else {
                principal
                    .permissions
                    .intersection(&tok.scopes)
                    .copied()
                    .collect()
            };

            let store = self.store.clone();
            let touch_tenant = tok.tenant_id;
            let token_id = tok.token_id;
            tokio::spawn(async move {
                if let Err(err) = store.touch_api_token(&touch_tenant, token_id).await {
                    tracing::warn!(
                        error = %err,
                        %token_id,
                        "failed to refresh personal access token last_used_at"
                    );
                }
            });

            return Ok(AuthPrincipal {
                user_id: principal.user_id,
                tenant_id: principal.tenant_id,
                username: principal.username,
                dept_id: principal.dept_id,
                roles: principal.roles,
                permissions,
                token_kind: TokenKind::ApiToken,
            });
        }

        Err(AppError::Unauthorized("TOKEN_INVALID"))
    }
}

/// Compares two secret strings in constant time by comparing their fixed-width SHA-256 digests,
/// preventing both byte-value and token-length timing side channels.
#[must_use]
fn constant_time_token_eq(supplied: &str, expected: &str) -> bool {
    let supplied_digest = Sha256::digest(supplied.as_bytes());
    let expected_digest = Sha256::digest(expected.as_bytes());
    bool::from(supplied_digest.as_slice().ct_eq(expected_digest.as_slice()))
}

/// Computes the lowercase hexadecimal SHA-256 digest of a plaintext API token.
#[must_use]
pub fn sha256_hex(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}
