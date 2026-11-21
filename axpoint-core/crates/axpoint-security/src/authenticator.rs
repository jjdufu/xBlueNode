//! Bearer credential verifier (`Authenticator<C>`) resolving `AuthPrincipal` instances across
//! static dev tokens, `axp_` personal access tokens, and `EdDSA` access `JWT`s (`11-STEP13` §13.4.1).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::TokenKind;
use axpoint_domain::platform::PrincipalRecord;
use axpoint_domain::ports::CredentialStore;
use moka::future::Cache;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::jwt::JwtKeyManager;
use crate::principal::AuthPrincipal;

/// Minimum byte length required for `AXPOINT_DEV_TOKEN` when configured.
const MIN_DEV_TOKEN_LEN: usize = 32;

/// Principal cache time-to-live in seconds (`30s`, `§13.4.1`).
const PRINCIPAL_CACHE_TTL_SECS: u64 = 30;

/// Maximum cached `(tenant_id, user_id)` principal entries (`10,000`).
const PRINCIPAL_CACHE_CAPACITY: u64 = 10_000;

#[derive(Default)]
struct CacheEpochs {
    tenant_gen: HashMap<String, u64>,
    user_gen: HashMap<(String, Uuid), u64>,
}

/// Bearer token authenticator backed by a [`CredentialStore`] port and optional [`JwtKeyManager`].
#[derive(Clone)]
pub struct Authenticator<C> {
    store: C,
    env_mode: String,
    dev_token: Option<String>,
    jwt: Option<Arc<JwtKeyManager>>,
    principal_cache: Cache<(String, Uuid, u64, u64), PrincipalRecord>,
    epochs: Arc<Mutex<CacheEpochs>>,
}

impl<C> fmt::Debug for Authenticator<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authenticator")
            .field("env_mode", &self.env_mode)
            .field("dev_token_configured", &self.dev_token.is_some())
            .field("jwt_configured", &self.jwt.is_some())
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

        let principal_cache = Cache::builder()
            .max_capacity(PRINCIPAL_CACHE_CAPACITY)
            .time_to_live(Duration::from_secs(PRINCIPAL_CACHE_TTL_SECS))
            .build();

        Ok(Self {
            store,
            env_mode,
            dev_token: normalized_dev_token,
            jwt: None,
            principal_cache,
            epochs: Arc::new(Mutex::new(CacheEpochs::default())),
        })
    }

    /// Constructs an [`Authenticator`] reading `AXPOINT_ENV`, `AXPOINT_DEV_TOKEN`, and
    /// [`JwtKeyManager::from_env`] from process environment.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if the environment variables violate production or minimum-length policies.
    pub fn from_env(store: C) -> Result<Self, AppError> {
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        let dev_token = std::env::var("AXPOINT_DEV_TOKEN").ok();
        let jwt = Arc::new(JwtKeyManager::from_env().with_env_mode(&env_mode));
        Ok(Self::new(store, env_mode, dev_token)?.with_jwt(jwt))
    }

    /// Attaches an `EdDSA` [`JwtKeyManager`] enabling Branch 4 access-token verification (`§13.4.1`).
    #[must_use]
    pub fn with_jwt(mut self, jwt: Arc<JwtKeyManager>) -> Self {
        self.jwt = Some(jwt);
        self
    }

    /// Returns a reference to the attached [`JwtKeyManager`], if configured.
    #[must_use]
    pub const fn jwt_manager(&self) -> Option<&Arc<JwtKeyManager>> {
        self.jwt.as_ref()
    }

    /// Returns the configured environment mode string (e.g., `"development"`, `"test"`, `"production"`).
    #[must_use]
    pub fn env_mode(&self) -> &str {
        &self.env_mode
    }

    /// Immediately invalidates all cached [`PrincipalRecord`] entries for `tenant_id`
    /// (invoked on `ax_policy_changed` notifications).
    pub fn invalidate_tenant(&self, tenant_id: &str) {
        let mut guard = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard.tenant_gen.entry(String::from(tenant_id)).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// Immediately invalidates the cached [`PrincipalRecord`] for `(tenant_id, user_id)`
    /// (invoked on password change, logout-all, or user role/status patch).
    pub fn invalidate_user(&self, tenant_id: &str, user_id: Uuid) {
        let mut guard = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard
            .user_gen
            .entry((String::from(tenant_id), user_id))
            .or_insert(0);
        *entry = entry.saturating_add(1);
    }

    fn current_epochs(&self, tenant_id: &str, user_id: Uuid) -> (u64, u64) {
        let guard = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);
        let t_gen = guard.tenant_gen.get(tenant_id).copied().unwrap_or(0);
        let u_gen = guard
            .user_gen
            .get(&(String::from(tenant_id), user_id))
            .copied()
            .unwrap_or(0);
        (t_gen, u_gen)
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
    /// - Returns `AppError::Unauthorized("TOKEN_EXPIRED")` if an `EdDSA` access `JWT` has expired.
    /// - Returns `AppError::Unauthorized("TOKEN_INVALID")` if the token is unknown, revoked, or its user is inactive.
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

        if token.starts_with("axs_") || token.starts_with("axe_") || token.starts_with("axr_") {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }

        if !self.env_mode.eq_ignore_ascii_case("production")
            && let Some(ref expected_dev) = self.dev_token
            && constant_time_token_eq(token, expected_dev)
        {
            return Ok(AuthPrincipal::dev_static());
        }

        if token.starts_with("axp_") {
            return self.authenticate_api_token(token).await;
        }

        if token.contains('.')
            && let Some(ref jwt) = self.jwt
        {
            return self.authenticate_jwt(jwt, token).await;
        }

        Err(AppError::Unauthorized("TOKEN_INVALID"))
    }

    async fn authenticate_api_token(&self, token: &str) -> Result<AuthPrincipal, AppError> {
        let token_hash = sha256_hex(token);
        let tok = self
            .store
            .resolve_api_token(&token_hash)
            .await?
            .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;

        let principal = self
            .load_principal_cached(&tok.tenant_id, tok.user_id)
            .await?;

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

        Ok(AuthPrincipal {
            user_id: principal.user_id,
            tenant_id: principal.tenant_id,
            username: principal.username,
            dept_id: principal.dept_id,
            roles: principal.roles,
            permissions,
            token_kind: TokenKind::ApiToken,
        })
    }

    async fn authenticate_jwt(
        &self,
        jwt: &JwtKeyManager,
        token: &str,
    ) -> Result<AuthPrincipal, AppError> {
        let claims = jwt.verify_access_token(token)?;
        let user_id =
            Uuid::parse_str(&claims.sub).map_err(|_| AppError::Unauthorized("TOKEN_INVALID"))?;

        let principal = self.load_principal_cached(&claims.tid, user_id).await?;
        if claims.iat < principal.tokens_valid_after.timestamp() {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }

        Ok(AuthPrincipal {
            user_id: principal.user_id,
            tenant_id: principal.tenant_id,
            username: principal.username,
            dept_id: principal.dept_id,
            roles: principal.roles,
            permissions: principal.permissions,
            token_kind: TokenKind::Access,
        })
    }

    async fn load_principal_cached(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<PrincipalRecord, AppError> {
        let (t_gen, u_gen) = self.current_epochs(tenant_id, user_id);
        let cache_key = (String::from(tenant_id), user_id, t_gen, u_gen);
        if let Some(hit) = self.principal_cache.get(&cache_key).await {
            return Ok(hit);
        }

        let loaded = self
            .store
            .load_principal(tenant_id, user_id)
            .await?
            .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
        self.principal_cache.insert(cache_key, loaded.clone()).await;
        Ok(loaded)
    }
}

/// Compares two secret strings in constant time by comparing their fixed-width `SHA-256` digests,
/// preventing both byte-value and token-length timing side channels.
#[must_use]
fn constant_time_token_eq(supplied: &str, expected: &str) -> bool {
    let supplied_digest = Sha256::digest(supplied.as_bytes());
    let expected_digest = Sha256::digest(expected.as_bytes());
    bool::from(supplied_digest.as_slice().ct_eq(expected_digest.as_slice()))
}

/// Computes the lowercase hexadecimal `SHA-256` digest of a plaintext token.
#[must_use]
pub fn sha256_hex(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use axpoint_domain::AppError;
    use axpoint_domain::platform::{ApiTokenRecord, PrincipalRecord};
    use axpoint_domain::ports::CredentialStore;
    use uuid::Uuid;

    use super::Authenticator;

    #[derive(Clone, Default)]
    struct StubCredentialStore;

    impl CredentialStore for StubCredentialStore {
        fn resolve_api_token(
            &self,
            _token_hash: &str,
        ) -> impl Future<Output = Result<Option<ApiTokenRecord>, AppError>> + Send {
            std::future::ready(Ok(None))
        }

        fn load_principal(
            &self,
            _tenant_id: &str,
            _user_id: Uuid,
        ) -> impl Future<Output = Result<Option<PrincipalRecord>, AppError>> + Send {
            std::future::ready(Ok(None))
        }

        fn touch_api_token(
            &self,
            _tenant_id: &str,
            _token_id: Uuid,
        ) -> impl Future<Output = Result<(), AppError>> + Send {
            std::future::ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn share_embed_and_refresh_tokens_are_rejected_as_api_bearer_tokens()
    -> Result<(), AppError> {
        let auth = Authenticator::new(
            StubCredentialStore,
            "development",
            Some(String::from("axs_abcd1234_0123456789abcdef0123456789ab")),
        )?;

        let share_res = auth
            .authenticate(Some("Bearer axs_abcd1234_0123456789abcdef0123456789ab"))
            .await;
        assert!(matches!(
            share_res,
            Err(AppError::Unauthorized("TOKEN_INVALID"))
        ));

        let embed_res = auth
            .authenticate(Some("Bearer axe_0123456789abcdef0123456789abcdef"))
            .await;
        assert!(matches!(
            embed_res,
            Err(AppError::Unauthorized("TOKEN_INVALID"))
        ));

        let refresh_res = auth
            .authenticate(Some("Bearer axr_0123456789abcdef0123456789abcdef"))
            .await;
        assert!(matches!(
            refresh_res,
            Err(AppError::Unauthorized("TOKEN_INVALID"))
        ));
        Ok(())
    }
}
