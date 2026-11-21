//! Federated `OpenID` Connect (`OIDC`) discovery, `JWKS` caching (`3600s`), `PKCE` (`S256`)
//! authorization flow, and `ID Token` verification (`11-STEP13` §13.5).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::guard::{OidcLoginState, UpsertOidcUserParams};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use moka::future::Cache;
use rand::RngCore as _;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;

/// Default display label shown on the login page when `AXPOINT_OIDC_DISPLAY_NAME` is unset.
pub const DEFAULT_OIDC_DISPLAY_NAME: &str = "企业单点登录 (OIDC)";

/// Discovery and `JWKS` cache TTL in seconds (`3600s = 1 hour`, `§13.5`).
const DISCOVERY_CACHE_TTL_SECS: u64 = 3_600;

/// Ephemeral `sec_login_states` validity window in minutes (`10` minutes, `§13.5`).
const LOGIN_STATE_TTL_MINUTES: i64 = 10;

/// Supported asymmetric `ID Token` signing algorithms (`HS*` and `none` are strictly rejected).
const ALLOWED_ID_TOKEN_ALGS: &[Algorithm] = &[
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

/// Claim-to-role and department mapping configuration parsed from `AXPOINT_OIDC_ROLE_MAP` (`§13.5`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcRoleMapConfig {
    /// Claim holding external group names (defaults to `"groups"`).
    #[serde(default = "default_groups_claim")]
    pub groups_claim: String,
    /// Claim holding department slug (defaults to `"department"`).
    #[serde(default = "default_dept_claim")]
    pub dept_claim: String,
    /// External group name -> `AxPoint` role name mapping.
    #[serde(default)]
    pub map: BTreeMap<String, String>,
}

fn default_groups_claim() -> String {
    String::from("groups")
}

fn default_dept_claim() -> String {
    String::from("department")
}

/// Configuration parameters for [`OidcService`].
#[derive(Clone)]
pub struct OidcConfig {
    /// Expected `OIDC` issuer URL (`AXPOINT_OIDC_ISSUER`).
    pub issuer: String,
    /// Relying-party `client_id` (`AXPOINT_OIDC_CLIENT_ID`).
    pub client_id: String,
    /// Optional `client_secret` (`AXPOINT_OIDC_CLIENT_SECRET`).
    pub client_secret: Option<String>,
    /// Callback redirect URI (`AXPOINT_OIDC_REDIRECT_URI`).
    pub redirect_uri: String,
    /// UI button label (`AXPOINT_OIDC_DISPLAY_NAME`).
    pub display_name: String,
    /// Claim used for initial username hint (`AXPOINT_OIDC_USERNAME_CLAIM`, default `"preferred_username"`).
    pub username_claim: String,
    /// Group/department claim mapping (`AXPOINT_OIDC_ROLE_MAP`).
    pub role_map: OidcRoleMapConfig,
    /// Whether unknown `(tenant, sub)` accounts are provisioned on first login (`AXPOINT_OIDC_AUTO_PROVISION`).
    pub auto_provision: bool,
}

impl fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("has_client_secret", &self.client_secret.is_some())
            .field("redirect_uri", &self.redirect_uri)
            .field("display_name", &self.display_name)
            .field("auto_provision", &self.auto_provision)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct TokenEndpointResponse {
    id_token: Option<String>,
}

/// Federated `OpenID` Connect relying-party service (`11-STEP13` §13.5).
#[derive(Clone)]
pub struct OidcService {
    cfg: Arc<OidcConfig>,
    http: Client,
    discovery_cache: Cache<String, DiscoveryDocument>,
    jwks_cache: Cache<String, JwkSet>,
}

impl fmt::Debug for OidcService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcService")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl OidcService {
    /// Constructs an [`OidcService`] from explicit [`OidcConfig`].
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if required URLs or identifiers are empty or HTTP client setup fails.
    pub fn new(mut cfg: OidcConfig) -> Result<Self, AppError> {
        cfg.issuer = String::from(cfg.issuer.trim().trim_end_matches('/'));
        cfg.client_id = String::from(cfg.client_id.trim());
        cfg.redirect_uri = String::from(cfg.redirect_uri.trim());
        if cfg.issuer.is_empty() || cfg.client_id.is_empty() || cfg.redirect_uri.is_empty() {
            return Err(AppError::Config(String::from(
                "OIDC issuer, client_id, and redirect_uri must be non-empty",
            )));
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|err| AppError::Config(format!("failed building OIDC HTTP client: {err}")))?;
        let discovery_cache = Cache::builder()
            .max_capacity(4)
            .time_to_live(Duration::from_secs(DISCOVERY_CACHE_TTL_SECS))
            .build();
        let jwks_cache = Cache::builder()
            .max_capacity(4)
            .time_to_live(Duration::from_secs(DISCOVERY_CACHE_TTL_SECS))
            .build();
        Ok(Self {
            cfg: Arc::new(cfg),
            http,
            discovery_cache,
            jwks_cache,
        })
    }

    /// Loads [`OidcService`] from `AXPOINT_OIDC_*` environment variables, returning `Ok(None)` when
    /// `AXPOINT_OIDC_ISSUER`, `AXPOINT_OIDC_CLIENT_ID`, or `AXPOINT_OIDC_REDIRECT_URI` is unset/empty.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `AXPOINT_OIDC_ROLE_MAP` is malformed JSON or client setup fails.
    pub fn from_env() -> Result<Option<Self>, AppError> {
        let issuer = env_non_empty("AXPOINT_OIDC_ISSUER");
        let client_id = env_non_empty("AXPOINT_OIDC_CLIENT_ID");
        let redirect_uri = env_non_empty("AXPOINT_OIDC_REDIRECT_URI");
        let (Some(issuer), Some(client_id), Some(redirect_uri)) = (issuer, client_id, redirect_uri)
        else {
            return Ok(None);
        };

        let client_secret = env_non_empty("AXPOINT_OIDC_CLIENT_SECRET");
        let display_name = env_non_empty("AXPOINT_OIDC_DISPLAY_NAME")
            .unwrap_or_else(|| String::from(DEFAULT_OIDC_DISPLAY_NAME));
        let username_claim = env_non_empty("AXPOINT_OIDC_USERNAME_CLAIM")
            .unwrap_or_else(|| String::from("preferred_username"));

        let role_map = match env_non_empty("AXPOINT_OIDC_ROLE_MAP") {
            Some(raw) => serde_json::from_str::<OidcRoleMapConfig>(&raw).map_err(|err| {
                AppError::Config(format!("invalid AXPOINT_OIDC_ROLE_MAP JSON: {err}"))
            })?,
            None => OidcRoleMapConfig {
                groups_claim: default_groups_claim(),
                dept_claim: default_dept_claim(),
                map: BTreeMap::new(),
            },
        };

        let auto_provision = env_non_empty("AXPOINT_OIDC_AUTO_PROVISION").is_none_or(|val| {
            !matches!(
                val.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        });

        Self::new(OidcConfig {
            issuer,
            client_id,
            client_secret,
            redirect_uri,
            display_name,
            username_claim,
            role_map,
            auto_provision,
        })
        .map(Some)
    }

    /// Returns the human-readable `OIDC` login button label.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.cfg.display_name
    }

    /// Validates that `return_to` is a safe internal relative path (`/`-prefixed, rejecting
    /// protocol-relative `//` and `/\` open-redirect vectors, `§13.5`).
    ///
    /// # Errors
    /// Returns `AppError::BadRequest` (`INVALID_REDIRECT`) if `return_to` is an external or malformed URI.
    pub fn validate_return_to(return_to: Option<&str>) -> Result<String, AppError> {
        let Some(raw) = return_to.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(String::from("/"));
        };
        if !raw.starts_with('/')
            || raw.starts_with("//")
            || raw.starts_with("/\\")
            || raw.contains('\r')
            || raw.contains('\n')
            || raw.contains('\0')
        {
            return Err(AppError::bad(
                "INVALID_REDIRECT",
                "return_to must be a relative in-app path starting with a single '/'",
            ));
        }
        Ok(String::from(raw))
    }

    /// Initiates an `OIDC` authorization-code + `PKCE` (`S256`) login flow, returning
    /// `(authorization_redirect_url, OidcLoginState)` (`§13.5`).
    ///
    /// # Errors
    /// Returns `INVALID_REDIRECT` if `return_to` is invalid, or [`AppError::Upstream`] if
    /// `OIDC` discovery fails.
    pub async fn start_login(
        &self,
        tenant_id: &str,
        return_to: Option<&str>,
    ) -> Result<(String, OidcLoginState), AppError> {
        let safe_return_to = Self::validate_return_to(return_to)?;
        let discovery = self.fetch_discovery().await?;

        let state = random_b64url_32();
        let nonce = random_b64url_32();
        let pkce_verifier = random_b64url_32();
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce_verifier.as_bytes()));

        let sep = if discovery.authorization_endpoint.contains('?') {
            '&'
        } else {
            '?'
        };
        let auth_url = format!(
            "{}{sep}response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
            discovery.authorization_endpoint,
            percent_encode(&self.cfg.client_id),
            percent_encode(&self.cfg.redirect_uri),
            percent_encode("openid profile email"),
            percent_encode(&state),
            percent_encode(&nonce),
            percent_encode(&code_challenge),
        );

        let login_state = OidcLoginState {
            state,
            tenant_id: String::from(tenant_id),
            nonce,
            pkce_verifier,
            return_to: safe_return_to,
            expires_at: Utc::now() + ChronoDuration::minutes(LOGIN_STATE_TTL_MINUTES),
        };
        Ok((auth_url, login_state))
    }

    /// Exchanges an authorization `code` using `login_state.pkce_verifier`, verifies the `ID Token`
    /// signature and claims (`iss`, `aud`, `exp`, `nonce`), and maps external claims into
    /// [`UpsertOidcUserParams`] (`§13.5`).
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`], [`AppError::Unauthorized`], or [`AppError::Upstream`] on
    /// expired state, token exchange failure, or `ID Token` validation failure.
    pub async fn exchange_code(
        &self,
        code: &str,
        login_state: &OidcLoginState,
    ) -> Result<UpsertOidcUserParams, AppError> {
        if code.trim().is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "OIDC code cannot be empty",
            ));
        }
        if login_state.expires_at <= Utc::now() {
            return Err(AppError::bad(
                "INVALID_STATE",
                "OIDC login state has expired",
            ));
        }

        let discovery = self.fetch_discovery().await?;
        let raw_id_token = self
            .exchange_code_for_id_token(&discovery.token_endpoint, code, &login_state.pkce_verifier)
            .await?;

        let claims = self
            .verify_id_token(&raw_id_token, &discovery.jwks_uri, &login_state.nonce)
            .await?;

        self.extract_user_params(&claims)
    }

    async fn fetch_discovery(&self) -> Result<DiscoveryDocument, AppError> {
        if let Some(cached) = self.discovery_cache.get("default").await {
            return Ok(cached);
        }
        let url = format!("{}/.well-known/openid-configuration", self.cfg.issuer);
        let resp =
            self.http.get(&url).send().await.map_err(|err| {
                AppError::Upstream(format!("OIDC discovery request failed: {err}"))
            })?;
        if !resp.status().is_success() {
            return Err(AppError::Upstream(format!(
                "OIDC discovery returned HTTP {}",
                resp.status()
            )));
        }
        let doc: DiscoveryDocument = resp
            .json()
            .await
            .map_err(|err| AppError::Upstream(format!("invalid OIDC discovery JSON: {err}")))?;
        if doc.issuer.trim_end_matches('/') != self.cfg.issuer {
            return Err(AppError::Upstream(String::from(
                "OIDC discovery issuer mismatch",
            )));
        }
        self.discovery_cache
            .insert(String::from("default"), doc.clone())
            .await;
        Ok(doc)
    }

    async fn fetch_jwks(&self, jwks_uri: &str, force_refresh: bool) -> Result<JwkSet, AppError> {
        if !force_refresh && let Some(cached) = self.jwks_cache.get(jwks_uri).await {
            return Ok(cached);
        }
        let resp = self
            .http
            .get(jwks_uri)
            .send()
            .await
            .map_err(|err| AppError::Upstream(format!("OIDC JWKS request failed: {err}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Upstream(format!(
                "OIDC JWKS returned HTTP {}",
                resp.status()
            )));
        }
        let jwks: JwkSet = resp
            .json()
            .await
            .map_err(|err| AppError::Upstream(format!("invalid OIDC JWKS JSON: {err}")))?;
        self.jwks_cache
            .insert(String::from(jwks_uri), jwks.clone())
            .await;
        Ok(jwks)
    }

    async fn exchange_code_for_id_token(
        &self,
        token_endpoint: &str,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<String, AppError> {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.cfg.redirect_uri),
            ("client_id", &self.cfg.client_id),
            ("code_verifier", pkce_verifier),
        ];
        if let Some(ref secret) = self.cfg.client_secret {
            form.push(("client_secret", secret.as_str()));
        }

        let resp = self
            .http
            .post(token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|err| AppError::Upstream(format!("OIDC token request failed: {err}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Unauthorized("OIDC_TOKEN_EXCHANGE_FAILED"));
        }
        let parsed: TokenEndpointResponse = resp
            .json()
            .await
            .map_err(|err| AppError::Upstream(format!("invalid OIDC token JSON: {err}")))?;
        parsed
            .id_token
            .filter(|t| !t.is_empty())
            .ok_or(AppError::Unauthorized("OIDC_MISSING_ID_TOKEN"))
    }

    async fn verify_id_token(
        &self,
        id_token: &str,
        jwks_uri: &str,
        expected_nonce: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
        let header =
            decode_header(id_token).map_err(|_| AppError::Unauthorized("TOKEN_INVALID"))?;
        if !ALLOWED_ID_TOKEN_ALGS.contains(&header.alg) {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }

        let mut jwks = self.fetch_jwks(jwks_uri, false).await?;
        let jwk = if let Some(found) = select_jwk(&jwks, header.kid.as_deref()) {
            found
        } else {
            jwks = self.fetch_jwks(jwks_uri, true).await?;
            select_jwk(&jwks, header.kid.as_deref())
                .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?
        };

        let decoding_key =
            DecodingKey::from_jwk(jwk).map_err(|_| AppError::Unauthorized("TOKEN_INVALID"))?;

        let mut validation = Validation::new(header.alg);
        validation.leeway = 30;
        validation.set_issuer(&[&self.cfg.issuer, &format!("{}/", self.cfg.issuer)]);
        validation.set_audience(&[&self.cfg.client_id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        let token_data = decode::<serde_json::Map<String, serde_json::Value>>(
            id_token,
            &decoding_key,
            &validation,
        )
        .map_err(|_| AppError::Unauthorized("TOKEN_INVALID"))?;

        let token_nonce = token_data
            .claims
            .get("nonce")
            .and_then(serde_json::Value::as_str)
            .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
        let nonce_ok = bool::from(
            Sha256::digest(token_nonce.as_bytes())
                .as_slice()
                .ct_eq(Sha256::digest(expected_nonce.as_bytes()).as_slice()),
        );
        if !nonce_ok {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }

        Ok(token_data.claims)
    }

    fn extract_user_params(
        &self,
        claims: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<UpsertOidcUserParams, AppError> {
        let sub = claims
            .get("sub")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;

        let email = String::from(
            claims
                .get("email")
                .and_then(serde_json::Value::as_str)
                .map_or("", str::trim),
        );

        let username_hint = claims
            .get(&self.cfg.username_claim)
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                claims
                    .get("preferred_username")
                    .and_then(serde_json::Value::as_str)
            })
            .or_else(|| {
                email
                    .split_once('@')
                    .map(|(local, _)| local)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or(sub)
            .trim()
            .to_owned();

        let mapped_dept = claims
            .get(&self.cfg.role_map.dept_claim)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let mut mapped_roles = Vec::new();
        if let Some(groups_val) = claims.get(&self.cfg.role_map.groups_claim) {
            match groups_val {
                serde_json::Value::Array(items) => {
                    for item in items {
                        if let Some(g) = item.as_str()
                            && let Some(role) = self.cfg.role_map.map.get(g)
                            && !mapped_roles.contains(role)
                        {
                            mapped_roles.push(role.clone());
                        }
                    }
                }
                serde_json::Value::String(single) => {
                    if let Some(role) = self.cfg.role_map.map.get(single) {
                        mapped_roles.push(role.clone());
                    }
                }
                _ => {}
            }
        }

        Ok(UpsertOidcUserParams {
            oidc_subject: String::from(sub),
            username_hint,
            email,
            mapped_dept,
            mapped_roles,
            auto_provision: self.cfg.auto_provision,
        })
    }
}

fn select_jwk<'a>(jwks: &'a JwkSet, kid: Option<&str>) -> Option<&'a jsonwebtoken::jwk::Jwk> {
    if let Some(target_kid) = kid {
        return jwks.find(target_kid);
    }
    if jwks.keys.len() == 1 {
        return jwks.keys.first();
    }
    None
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn random_b64url_32() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let hi = char::from(HEX_UPPER.get(usize::from(b >> 4)).copied().unwrap_or(b'0'));
            let lo = char::from(
                HEX_UPPER
                    .get(usize::from(b & 0x0F))
                    .copied()
                    .unwrap_or(b'0'),
            );
            out.push('%');
            out.push(hi);
            out.push(lo);
        }
    }
    out
}

const HEX_UPPER: [u8; 16] = *b"0123456789ABCDEF";
