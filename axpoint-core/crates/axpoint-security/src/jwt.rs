//! `EdDSA` (`Ed25519`) access token signer, live-reloading key manager, verifier, and `JWKS`
//! publisher (`11-STEP13` §13.4.1, §13.4.3).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use axpoint_domain::AppError;
use axpoint_domain::enums::Sensitivity;
use axpoint_domain::guard::{AccessTokenResponse, GuardPrincipalProfile};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::EllipticCurve;
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Access token issuer claim (`"axpoint"`, `§13.4.1`).
pub const JWT_ISSUER: &str = "axpoint";

/// Access token audience claim (`"axpoint-api"`, `§13.4.1`).
pub const JWT_AUDIENCE: &str = "axpoint-api";

/// Access token lifetime in seconds (`900s = 15 minutes`, `§13.4.1`).
pub const ACCESS_TOKEN_TTL_SECS: i64 = 900;

/// Clock skew leeway in seconds during `exp` verification (`30s`, `§13.4.1`).
pub const JWT_VERIFY_LEEWAY_SECS: u64 = 30;

/// Public-key directory reload interval (`60s`, `§13.4.1`).
const PUBLIC_KEYS_RELOAD_INTERVAL: Duration = Duration::from_secs(60);

/// RFC 8410 §7 `PKCS#8` v1 `PrivateKeyInfo` ASN.1 DER prefix preceding a 32-byte `Ed25519` seed.
const ED25519_PKCS8_V1_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// RFC 8410 §4 `SubjectPublicKeyInfo` ASN.1 DER prefix preceding a 32-byte `Ed25519` public key.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Signed `EdDSA` access token claims (`11-STEP13` §13.4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessClaims {
    /// Token issuer (`"axpoint"`).
    pub iss: String,
    /// Token audience (`"axpoint-api"`).
    pub aud: String,
    /// Subject user UUID string.
    pub sub: String,
    /// Tenant identifier.
    pub tid: String,
    /// Username.
    pub unm: String,
    /// Primary department identifier.
    pub dept: String,
    /// Security clearance classification.
    pub clr: Sensitivity,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// Unique token identifier (`UUIDv4`).
    pub jti: String,
    /// Issued-at Unix timestamp in seconds.
    pub iat: i64,
    /// Expiration Unix timestamp in seconds (`iat + 900`).
    pub exp: i64,
}

#[derive(Clone)]
struct LoadedSigningKey {
    kid: String,
    encoding_key: EncodingKey,
    public_bytes: [u8; 32],
    source_path: Option<PathBuf>,
    source_mtime: Option<SystemTime>,
    dir_mtime_at_resolve: Option<SystemTime>,
}

#[derive(Clone)]
struct LoadedPublicKey {
    decoding_key: DecodingKey,
    public_bytes: [u8; 32],
}

#[derive(Default)]
struct KeyCacheState {
    signer: Option<LoadedSigningKey>,
    ephemeral_signer: Option<LoadedSigningKey>,
    public_keys: BTreeMap<String, LoadedPublicKey>,
    public_dir_loaded_at: Option<Instant>,
    public_dir_mtime: Option<SystemTime>,
}

/// Live-reloading `Ed25519` (`EdDSA`) signing and verification key manager (`§13.4`).
#[derive(Clone)]
pub struct JwtKeyManager {
    private_key_path: Option<PathBuf>,
    public_keys_dir: Option<PathBuf>,
    env_mode: String,
    state: Arc<RwLock<KeyCacheState>>,
}

impl fmt::Debug for JwtKeyManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtKeyManager")
            .field("private_key_path", &self.private_key_path)
            .field("public_keys_dir", &self.public_keys_dir)
            .field("env_mode", &self.env_mode)
            .finish_non_exhaustive()
    }
}

impl JwtKeyManager {
    /// Creates a [`JwtKeyManager`] watching `private_key_path` and `public_keys_dir`.
    #[must_use]
    pub fn new(private_key_path: Option<PathBuf>, public_keys_dir: Option<PathBuf>) -> Self {
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        Self {
            private_key_path,
            public_keys_dir,
            env_mode,
            state: Arc::new(RwLock::new(KeyCacheState::default())),
        }
    }

    /// Creates a [`JwtKeyManager`] configured from `AXPOINT_JWT_PRIVATE_KEY_PATH` and
    /// `AXPOINT_JWT_PUBLIC_KEYS_DIR` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let priv_path = std::env::var("AXPOINT_JWT_PRIVATE_KEY_PATH")
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let pub_dir = std::env::var("AXPOINT_JWT_PUBLIC_KEYS_DIR")
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        Self::new(priv_path, pub_dir)
    }

    /// Overrides the environment mode (`"development"`, `"test"`, `"production"`).
    #[must_use]
    pub fn with_env_mode(mut self, env_mode: impl Into<String>) -> Self {
        self.env_mode = env_mode.into();
        self
    }

    /// Signs a `900s` `EdDSA` access token for `profile` (`§13.4.1`).
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if no valid `Ed25519` private key is available or signing fails.
    pub fn sign_access_token(
        &self,
        profile: &GuardPrincipalProfile,
    ) -> Result<AccessTokenResponse, AppError> {
        let signer = self.ensure_signing_key()?;
        let now = Utc::now().timestamp();
        let exp = now.saturating_add(ACCESS_TOKEN_TTL_SECS);
        let claims = AccessClaims {
            iss: String::from(JWT_ISSUER),
            aud: String::from(JWT_AUDIENCE),
            sub: profile.user_id.to_string(),
            tid: profile.tenant_id.clone(),
            unm: profile.username.clone(),
            dept: profile.dept_id.clone(),
            clr: profile.clearance,
            roles: profile.roles.clone(),
            jti: Uuid::new_v4().to_string(),
            iat: now,
            exp,
        };

        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(signer.kid);
        let access_token = encode(&header, &claims, &signer.encoding_key)
            .map_err(|err| AppError::Config(format!("EdDSA JWT signing failed: {err}")))?;

        Ok(AccessTokenResponse {
            access_token,
            token_type: String::from("Bearer"),
            expires_in: ACCESS_TOKEN_TTL_SECS,
        })
    }

    /// Verifies an `EdDSA` access token and returns its validated [`AccessClaims`] (`§13.4.1`).
    ///
    /// Rejects any non-`EdDSA` algorithm (`none`, `HS256`, `RS256`, etc.), wrong `iss`/`aud`,
    /// unknown `kid`, invalid signature, or expired `exp` (beyond `30s` leeway).
    ///
    /// # Errors
    /// - Returns `AppError::Unauthorized("TOKEN_EXPIRED")` if the signature is valid and `exp` is past leeway.
    /// - Returns `AppError::Unauthorized("TOKEN_INVALID")` on any header, `alg`, `kid`, signature, or claim error.
    pub fn verify_access_token(&self, token: &str) -> Result<AccessClaims, AppError> {
        let header = decode_header(token).map_err(|_| AppError::Unauthorized("TOKEN_INVALID"))?;
        if header.alg != Algorithm::EdDSA {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }

        let decoding_key = self.resolve_decoding_key(header.kid.as_deref())?;
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.leeway = JWT_VERIFY_LEEWAY_SECS;
        validation.set_issuer(&[JWT_ISSUER]);
        validation.set_audience(&[JWT_AUDIENCE]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        let data = decode::<AccessClaims>(token, &decoding_key, &validation).map_err(|err| {
            if matches!(err.kind(), ErrorKind::ExpiredSignature) {
                AppError::Unauthorized("TOKEN_EXPIRED")
            } else {
                AppError::Unauthorized("TOKEN_INVALID")
            }
        })?;

        if data.claims.tid.trim().is_empty() || data.claims.sub.trim().is_empty() {
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }
        Ok(data.claims)
    }

    /// Renders all active `Ed25519` public keys in RFC 8037 `JWK Set` (`OKP` / `Ed25519`) JSON format
    /// for `GET /api/auth/jwks.json` (`§13.4.1`).
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if key loading fails in production.
    pub fn jwks_json(&self) -> Result<serde_json::Value, AppError> {
        let map = self.collect_all_public_keys()?;
        let mut keys = Vec::with_capacity(map.len());
        for (kid, pub_bytes) in map {
            let x = URL_SAFE_NO_PAD.encode(pub_bytes);
            keys.push(serde_json::json!({
                "kty": "OKP",
                "crv": "Ed25519",
                "alg": "EdDSA",
                "use": "sig",
                "kid": kid,
                "x": x,
            }));
        }
        Ok(serde_json::json!({ "keys": keys }))
    }

    fn effective_private_key_path(&self) -> Option<PathBuf> {
        if let Some(ref explicit) = self.private_key_path {
            return Some(explicit.clone());
        }
        if self.env_mode.eq_ignore_ascii_case("production") {
            return None;
        }
        for candidate in [".gate/jwt/private.pem", "../.gate/jwt/private.pem"] {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    fn effective_public_keys_dir(&self) -> Option<PathBuf> {
        if let Some(ref explicit) = self.public_keys_dir {
            return Some(explicit.clone());
        }
        if self.env_mode.eq_ignore_ascii_case("production") {
            return None;
        }
        for candidate in [".gate/jwt/public", "../.gate/jwt/public"] {
            let p = PathBuf::from(candidate);
            if p.is_dir() {
                return Some(p);
            }
        }
        None
    }

    fn ensure_signing_key(&self) -> Result<LoadedSigningKey, AppError> {
        let pub_map = self.refresh_public_keys_if_needed(true)?;
        let pub_dir_mtime = self
            .effective_public_keys_dir()
            .and_then(|d| file_mtime(&d));

        if let Some(priv_path) = self.effective_private_key_path() {
            let current_mtime = file_mtime(&priv_path);
            {
                let guard = self.state.read().unwrap_or_else(PoisonError::into_inner);
                if let Some(ref cached) = guard.signer
                    && cached.source_path.as_deref() == Some(priv_path.as_path())
                    && current_mtime.is_some()
                    && cached.source_mtime == current_mtime
                    && cached.dir_mtime_at_resolve == pub_dir_mtime
                {
                    return Ok(cached.clone());
                }
            }

            if priv_path.is_file() {
                let pem_bytes = std::fs::read(&priv_path).map_err(|err| {
                    AppError::Config(format!(
                        "failed reading Ed25519 private key '{}': {err}",
                        priv_path.display()
                    ))
                })?;
                let (encoding_key, public_bytes) = parse_ed25519_private_pem(&pem_bytes)?;
                let kid = resolve_kid_for_public_bytes(&public_bytes, &pub_map, Some(&priv_path));
                let loaded = LoadedSigningKey {
                    kid,
                    encoding_key,
                    public_bytes,
                    source_path: Some(priv_path),
                    source_mtime: current_mtime,
                    dir_mtime_at_resolve: pub_dir_mtime,
                };
                let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
                guard.signer = Some(loaded.clone());
                return Ok(loaded);
            }

            if self.env_mode.eq_ignore_ascii_case("production") {
                return Err(AppError::Config(format!(
                    "AXPOINT_JWT_PRIVATE_KEY_PATH '{}' does not exist in production",
                    priv_path.display()
                )));
            }
        } else if self.env_mode.eq_ignore_ascii_case("production") {
            return Err(AppError::Config(String::from(
                "AXPOINT_JWT_PRIVATE_KEY_PATH must be configured when AXPOINT_ENV=production",
            )));
        }

        self.get_or_init_ephemeral_signer()
    }

    fn get_or_init_ephemeral_signer(&self) -> Result<LoadedSigningKey, AppError> {
        {
            let guard = self.state.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(ref eph) = guard.ephemeral_signer {
                return Ok(eph.clone());
            }
        }
        let (priv_pem, _) = generate_ed25519_keypair_pem()?;
        let (encoding_key, public_bytes) = parse_ed25519_private_pem(priv_pem.as_bytes())?;
        let eph = LoadedSigningKey {
            kid: String::from("1"),
            encoding_key,
            public_bytes,
            source_path: None,
            source_mtime: None,
            dir_mtime_at_resolve: None,
        };
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(ref raced) = guard.ephemeral_signer {
            return Ok(raced.clone());
        }
        guard.ephemeral_signer = Some(eph.clone());
        Ok(eph)
    }

    fn refresh_public_keys_if_needed(
        &self,
        force_check_mtime: bool,
    ) -> Result<BTreeMap<String, LoadedPublicKey>, AppError> {
        let Some(dir) = self.effective_public_keys_dir() else {
            return Ok(BTreeMap::new());
        };
        if !dir.is_dir() {
            return Ok(BTreeMap::new());
        }

        let now = Instant::now();
        let current_dir_mtime = file_mtime(&dir);
        {
            let guard = self.state.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(loaded_at) = guard.public_dir_loaded_at {
                let within_ttl =
                    now.saturating_duration_since(loaded_at) < PUBLIC_KEYS_RELOAD_INTERVAL;
                let mtime_same = !force_check_mtime || guard.public_dir_mtime == current_dir_mtime;
                if within_ttl && mtime_same {
                    return Ok(guard.public_keys.clone());
                }
            }
        }

        let scanned = scan_public_keys_dir(&dir)?;
        let mut guard = self.state.write().unwrap_or_else(PoisonError::into_inner);
        guard.public_keys.clone_from(&scanned);
        guard.public_dir_loaded_at = Some(now);
        guard.public_dir_mtime = current_dir_mtime;
        Ok(scanned)
    }

    fn resolve_decoding_key(&self, kid_opt: Option<&str>) -> Result<DecodingKey, AppError> {
        let mut pub_map = self.refresh_public_keys_if_needed(false)?;

        if let Some(kid) = kid_opt {
            if let Some(found) = pub_map.get(kid) {
                return Ok(found.decoding_key.clone());
            }
            // Reload immediately on unknown `kid` if public_keys_dir mtime changed (§13.4.1).
            pub_map = self.refresh_public_keys_if_needed(true)?;
            if let Some(found) = pub_map.get(kid) {
                return Ok(found.decoding_key.clone());
            }
            if let Ok(signer) = self.ensure_signing_key()
                && signer.kid == kid
            {
                return Ok(DecodingKey::from_ed_der(&signer.public_bytes));
            }
            return Err(AppError::Unauthorized("TOKEN_INVALID"));
        }

        if pub_map.len() == 1
            && let Some(only) = pub_map.values().next()
        {
            return Ok(only.decoding_key.clone());
        }
        if pub_map.is_empty()
            && let Ok(signer) = self.ensure_signing_key()
        {
            return Ok(DecodingKey::from_ed_der(&signer.public_bytes));
        }
        Err(AppError::Unauthorized("TOKEN_INVALID"))
    }

    fn collect_all_public_keys(&self) -> Result<BTreeMap<String, [u8; 32]>, AppError> {
        let pub_map = self.refresh_public_keys_if_needed(true)?;
        let mut out: BTreeMap<String, [u8; 32]> = pub_map
            .into_iter()
            .map(|(k, v)| (k, v.public_bytes))
            .collect();

        if let Ok(signer) = self.ensure_signing_key() {
            out.entry(signer.kid).or_insert(signer.public_bytes);
        } else if self.env_mode.eq_ignore_ascii_case("production") && out.is_empty() {
            return Err(AppError::Config(String::from(
                "no Ed25519 JWT keys configured in production",
            )));
        }
        Ok(out)
    }
}

/// Generates a fresh `Ed25519` `(private_pkcs8_pem, public_spki_pem)` keypair using OS CSPRNG.
///
/// # Errors
/// Returns [`AppError::Config`] if `Ed25519` public key derivation fails.
pub fn generate_ed25519_keypair_pem() -> Result<(String, String), AppError> {
    let mut seed = [0_u8; 32];
    rand::rng().fill_bytes(&mut seed);

    let mut pkcs8_der = Vec::with_capacity(48);
    pkcs8_der.extend_from_slice(&ED25519_PKCS8_V1_PREFIX);
    pkcs8_der.extend_from_slice(&seed);

    let pub_vec = (DEFAULT_PROVIDER
        .key_utils
        .ed_pub_components_from_private_key)(&pkcs8_der, &EllipticCurve::Ed25519)
    .map_err(|err| AppError::Config(format!("Ed25519 public key derivation failed: {err}")))?;

    let pub_arr: [u8; 32] = pub_vec.as_slice().try_into().map_err(|_| {
        AppError::Config(String::from("derived Ed25519 public key was not 32 bytes"))
    })?;

    let mut spki_der = Vec::with_capacity(44);
    spki_der.extend_from_slice(&ED25519_SPKI_PREFIX);
    spki_der.extend_from_slice(&pub_arr);

    let priv_pem = encode_pem_block("PRIVATE KEY", &pkcs8_der);
    let pub_pem = encode_pem_block("PUBLIC KEY", &spki_der);
    Ok((priv_pem, pub_pem))
}

fn encode_pem_block(label: &str, der: &[u8]) -> String {
    let b64 = STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    let mut offset = 0_usize;
    while let Some(chunk) = b64.get(offset..) {
        if chunk.is_empty() {
            break;
        }
        let take = chunk.len().min(64);
        if let Some(line) = chunk.get(..take) {
            out.push_str(line);
            out.push('\n');
        }
        offset = offset.saturating_add(take);
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\n");
    out
}

fn parse_ed25519_private_pem(pem_bytes: &[u8]) -> Result<(EncodingKey, [u8; 32]), AppError> {
    let encoding_key = EncodingKey::from_ed_pem(pem_bytes)
        .map_err(|err| AppError::Config(format!("invalid Ed25519 PKCS#8 private PEM: {err}")))?;
    let pub_vec = (DEFAULT_PROVIDER
        .key_utils
        .ed_pub_components_from_private_key)(
        encoding_key.as_bytes(), &EllipticCurve::Ed25519
    )
    .map_err(|err| AppError::Config(format!("failed deriving Ed25519 public key: {err}")))?;
    let pub_bytes: [u8; 32] = pub_vec
        .as_slice()
        .try_into()
        .map_err(|_| AppError::Config(String::from("Ed25519 public key must be 32 bytes")))?;
    Ok((encoding_key, pub_bytes))
}

fn parse_ed25519_public_pem(pem_bytes: &[u8]) -> Result<LoadedPublicKey, AppError> {
    let decoding_key = DecodingKey::from_ed_pem(pem_bytes)
        .map_err(|err| AppError::Config(format!("invalid Ed25519 public PEM: {err}")))?;
    let raw = decoding_key
        .try_get_as_bytes()
        .map_err(|err| AppError::Config(format!("invalid Ed25519 public key bytes: {err}")))?;
    let first32 = raw.get(..32).ok_or_else(|| {
        AppError::Config(String::from("Ed25519 public key shorter than 32 bytes"))
    })?;
    let pub_bytes: [u8; 32] = first32
        .try_into()
        .map_err(|_| AppError::Config(String::from("Ed25519 public key must be 32 bytes")))?;
    Ok(LoadedPublicKey {
        decoding_key,
        public_bytes: pub_bytes,
    })
}

fn scan_public_keys_dir(dir: &Path) -> Result<BTreeMap<String, LoadedPublicKey>, AppError> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        AppError::Config(format!(
            "failed reading JWT public keys directory '{}': {err}",
            dir.display()
        ))
    })?;
    let mut map = BTreeMap::new();
    for entry_res in entries {
        let Ok(entry) = entry_res else {
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_pem = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pem"));
        if !is_pem {
            continue;
        }
        let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        // Skip private keys accidentally placed in the same directory (`*.key.pem`).
        let is_key_stem = Path::new(stem)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("key"));
        if is_key_stem || stem.eq_ignore_ascii_case("private") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(loaded) = parse_ed25519_public_pem(&bytes) {
            map.insert(String::from(stem), loaded);
        }
    }
    Ok(map)
}

fn resolve_kid_for_public_bytes(
    public_bytes: &[u8; 32],
    pub_map: &BTreeMap<String, LoadedPublicKey>,
    priv_path: Option<&Path>,
) -> String {
    for (kid, loaded) in pub_map {
        if &loaded.public_bytes == public_bytes {
            return kid.clone();
        }
    }
    if let Some(path) = priv_path
        && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
    {
        let clean = stem.strip_suffix(".key").unwrap_or(stem).trim();
        if !clean.is_empty() && !clean.eq_ignore_ascii_case("private") {
            return String::from(clean);
        }
    }
    String::from("1")
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}
