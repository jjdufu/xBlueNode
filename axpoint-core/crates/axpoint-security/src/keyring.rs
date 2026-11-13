//! Versioned AES-256-GCM credential keyring (`AxGuard` / `AxHub` secret encryption at rest).

use std::collections::BTreeMap;
use std::fmt;

use aes_gcm::aead::{Aead, Generate, Nonce, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use axpoint_domain::AppError;
use axpoint_domain::axhub::SealedCredential;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Environment variable storing versioned 32-byte base64 keys (`"1:<b64>,2:<b64>"`).
pub const CREDENTIAL_KEYS_ENV_VAR: &str = "AXPOINT_CREDENTIAL_KEYS";

/// Required AES-256 key length in bytes.
const KEY_LEN: usize = 32;

/// Required AES-GCM nonce length in bytes (96-bit).
const NONCE_LEN: usize = 12;

/// Versioned AES-256-GCM keyring for sealing and opening connector/webhook credentials.
///
/// Keys are configured as `"<version>:<base64_32bytes>,..."`. The highest key version is
/// used as the active key for new [`CredentialKeyring::seal`] calls, while older versions
/// remain available for [`CredentialKeyring::open`] until `CREDENTIAL_ROTATE` completes.
#[derive(Clone)]
pub struct CredentialKeyring {
    keys: BTreeMap<i16, Zeroizing<[u8; KEY_LEN]>>,
    active_version: i16,
}

/// Alias matching Step 02 §02.2 documentation.
pub type Keyring = CredentialKeyring;

impl fmt::Debug for CredentialKeyring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let versions: Vec<i16> = self.keys.keys().copied().collect();
        f.debug_struct("CredentialKeyring")
            .field("active_version", &self.active_version)
            .field("key_versions", &versions)
            .field("keys", &"[REDACTED]")
            .finish()
    }
}

impl CredentialKeyring {
    /// Constructs a [`CredentialKeyring`] from an explicit map of `(key_version -> 32-byte key)`.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `keys` is empty or contains any non-positive version (`<= 0`).
    pub fn from_keys(keys: BTreeMap<i16, [u8; KEY_LEN]>) -> Result<Self, AppError> {
        if keys.is_empty() {
            return Err(AppError::Config(String::from(
                "credential keyring must contain at least one key",
            )));
        }

        let mut zeroizing_keys = BTreeMap::new();
        let mut max_version: i16 = 0;

        for (version, mut raw_key) in keys {
            if version <= 0 {
                zeroize::Zeroize::zeroize(&mut raw_key);
                return Err(AppError::Config(format!(
                    "credential key version must be positive, got {version}"
                )));
            }
            if version > max_version {
                max_version = version;
            }
            zeroizing_keys.insert(version, Zeroizing::new(raw_key));
        }

        Ok(Self {
            keys: zeroizing_keys,
            active_version: max_version,
        })
    }

    /// Parses a [`CredentialKeyring`] from a comma-separated `"<version>:<base64_32bytes>"` string.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `raw` is empty, contains malformed entries, duplicate or
    /// non-positive version numbers, invalid Base64, or any key whose decoded length is not 32 bytes.
    pub fn from_env_str(raw: &str) -> Result<Self, AppError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(AppError::Config(String::from(
                "AXPOINT_CREDENTIAL_KEYS cannot be empty",
            )));
        }

        let mut parsed_keys: BTreeMap<i16, [u8; KEY_LEN]> = BTreeMap::new();

        for segment in trimmed.split(',') {
            let entry = segment.trim();
            if entry.is_empty() {
                return Err(AppError::Config(String::from(
                    "AXPOINT_CREDENTIAL_KEYS contains an empty key entry",
                )));
            }

            let (ver_part, b64_part) = entry.split_once(':').ok_or_else(|| {
                AppError::Config(String::from(
                    "AXPOINT_CREDENTIAL_KEYS entry must have '<version>:<base64>' format",
                ))
            })?;

            let version: i16 = ver_part.trim().parse().map_err(|err| {
                AppError::Config(format!(
                    "invalid credential key version '{}': {err}",
                    ver_part.trim()
                ))
            })?;

            if version <= 0 {
                return Err(AppError::Config(format!(
                    "credential key version must be >= 1, got {version}"
                )));
            }

            if parsed_keys.contains_key(&version) {
                return Err(AppError::Config(format!(
                    "duplicate credential key version {version}"
                )));
            }

            let b64_clean = b64_part.trim();
            let decoded = Zeroizing::new(decode_base64_key(b64_clean)?);
            if decoded.len() != KEY_LEN {
                return Err(AppError::Config(format!(
                    "credential key v{version} decoded to {} bytes, expected {KEY_LEN} bytes",
                    decoded.len()
                )));
            }

            let mut key_arr = [0_u8; KEY_LEN];
            key_arr.copy_from_slice(decoded.as_slice());
            parsed_keys.insert(version, key_arr);
        }

        Self::from_keys(parsed_keys)
    }

    /// Alias for [`CredentialKeyring::from_env_str`] matching Step 02 §02.2 signature.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] on invalid key format, duplicate version, or wrong key length.
    pub fn from_env_value(raw: &str) -> Result<Self, AppError> {
        Self::from_env_str(raw)
    }

    /// Loads [`CredentialKeyring`] from environment variables (`AXPOINT_CREDENTIAL_KEYS` and `AXPOINT_ENV`).
    ///
    /// When `AXPOINT_CREDENTIAL_KEYS` is unset in `development` or `test` mode, generates an
    /// ephemeral single-key keyring and emits a `WARN` log; in `production`, fails with [`AppError::Config`].
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `AXPOINT_CREDENTIAL_KEYS` is malformed, or unset in `production`.
    pub fn from_env() -> Result<Self, AppError> {
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        let raw_keys = std::env::var(CREDENTIAL_KEYS_ENV_VAR).ok();
        Self::from_env_with_mode(&env_mode, raw_keys.as_deref())
    }

    /// Loads [`CredentialKeyring`] given an explicit `axpoint_env` mode and optional raw key string.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `raw_keys` is invalid, or missing when `axpoint_env == "production"`.
    pub fn from_env_with_mode(axpoint_env: &str, raw_keys: Option<&str>) -> Result<Self, AppError> {
        match raw_keys.map(str::trim).filter(|s| !s.is_empty()) {
            Some(configured) => Self::from_env_str(configured),
            None if axpoint_env.eq_ignore_ascii_case("production") => {
                Err(AppError::Config(String::from(
                    "AXPOINT_CREDENTIAL_KEYS must be configured when AXPOINT_ENV=production",
                )))
            }
            None => {
                tracing::warn!("credential keyring is ephemeral");
                Self::ephemeral()
            }
        }
    }

    /// Generates a fresh single-key (`version = 1`) ephemeral keyring backed by OS CSPRNG.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if the operating system CSPRNG fails.
    pub fn ephemeral() -> Result<Self, AppError> {
        let random_key = <[u8; KEY_LEN]>::try_generate()
            .map_err(|err| AppError::Config(format!("OS CSPRNG key generation failed: {err}")))?;
        Self::from_keys(BTreeMap::from([(1_i16, random_key)]))
    }

    /// Returns the highest configured key version used for all new encryptions.
    #[must_use]
    pub const fn active_version(&self) -> i16 {
        self.active_version
    }

    /// Returns the active key version widened to `i32` for database `INT` columns.
    #[must_use]
    pub fn active_version_i32(&self) -> i32 {
        i32::from(self.active_version)
    }

    /// Builds canonical AAD (`"{tenant_id}:{scope}:{resource_id}"`) binding ciphertext to tenant and resource.
    #[must_use]
    pub fn format_aad(tenant_id: &str, scope: &str, resource_id: Uuid) -> String {
        format!("{tenant_id}:{scope}:{resource_id}")
    }

    /// Builds canonical AAD (`"{tenant_id}:connection:{connection_id}"`) for a connector credential.
    #[must_use]
    pub fn aad_for_connection(tenant_id: &str, connection_id: Uuid) -> String {
        Self::format_aad(tenant_id, "connection", connection_id)
    }

    /// Builds canonical AAD (`"{tenant_id}:webhook:{webhook_id}"`) for an inbound webhook secret.
    #[must_use]
    pub fn aad_for_webhook(tenant_id: &str, webhook_id: Uuid) -> String {
        Self::format_aad(tenant_id, "webhook", webhook_id)
    }

    /// Builds canonical AAD (`"{tenant_id}:{credential_id}"`) for a credential row identifier.
    #[must_use]
    pub fn aad_for_credential(tenant_id: &str, credential_id: Uuid) -> String {
        format!("{tenant_id}:{credential_id}")
    }

    /// Encrypts `plaintext` with AES-256-GCM under [`Self::active_version`], a fresh 12-byte CSPRNG nonce,
    /// and `aad` as Additional Authenticated Data.
    ///
    /// Returns `(key_version, nonce_12bytes, ciphertext_with_tag)`.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if CSPRNG nonce generation or cipher encryption fails.
    pub fn seal(&self, aad: &str, plaintext: &[u8]) -> Result<(i16, Vec<u8>, Vec<u8>), AppError> {
        let key_bytes = self.keys.get(&self.active_version).ok_or_else(|| {
            AppError::Config(format!(
                "active credential key version {} missing from keyring",
                self.active_version
            ))
        })?;

        let cipher = Aes256Gcm::new_from_slice(key_bytes.as_slice())
            .map_err(|err| AppError::Config(format!("invalid AES-256 key length: {err}")))?;

        let nonce = Nonce::<Aes256Gcm>::try_generate()
            .map_err(|err| AppError::Config(format!("OS CSPRNG nonce generation failed: {err}")))?;

        let payload = Payload {
            msg: plaintext,
            aad: aad.as_bytes(),
        };
        let ciphertext = cipher
            .encrypt(&nonce, payload)
            .map_err(|err| AppError::Config(format!("AES-256-GCM encryption failed: {err}")))?;

        Ok((self.active_version, nonce.as_slice().to_vec(), ciphertext))
    }

    /// Decrypts and authenticates AES-256-GCM `ciphertext` using `key_version`, `nonce`, and `aad`.
    ///
    /// # Errors
    /// Returns `AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED")` if `key_version` is unknown,
    /// `nonce` is not 12 bytes, or AEAD tag verification fails (tampered ciphertext, nonce, or AAD).
    pub fn open(
        &self,
        key_version: i16,
        nonce: &[u8],
        aad: &str,
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        if nonce.len() != NONCE_LEN {
            return Err(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"));
        }

        let key_bytes = self
            .keys
            .get(&key_version)
            .ok_or(AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))?;

        let cipher = Aes256Gcm::new_from_slice(key_bytes.as_slice())
            .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))?;

        let nonce_arr = <&Nonce<Aes256Gcm>>::try_from(nonce)
            .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))?;

        let payload = Payload {
            msg: ciphertext,
            aad: aad.as_bytes(),
        };

        let plaintext = cipher
            .decrypt(nonce_arr, payload)
            .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))?;

        Ok(Zeroizing::new(plaintext))
    }

    /// Convenience helper sealing `plaintext` directly into a [`SealedCredential`] DTO.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if encryption fails.
    pub fn seal_credential(
        &self,
        credential_id: Uuid,
        aad: &str,
        plaintext: &[u8],
    ) -> Result<SealedCredential, AppError> {
        let (ver, nonce, ciphertext) = self.seal(aad, plaintext)?;
        Ok(SealedCredential {
            id: credential_id,
            key_version: i32::from(ver),
            nonce,
            ciphertext,
        })
    }

    /// Convenience helper decrypting a [`SealedCredential`] DTO with `aad`.
    ///
    /// # Errors
    /// Returns `AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED")` if `key_version` is out of range,
    /// unknown, or AEAD authentication fails.
    pub fn open_credential(
        &self,
        aad: &str,
        credential: &SealedCredential,
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        let ver = i16::try_from(credential.key_version)
            .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))?;
        self.open(ver, &credential.nonce, aad, &credential.ciphertext)
    }
}

fn decode_base64_key(encoded: &str) -> Result<Vec<u8>, AppError> {
    STANDARD
        .decode(encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(encoded))
        .map_err(|err| AppError::Config(format!("invalid base64 credential key: {err}")))
}
