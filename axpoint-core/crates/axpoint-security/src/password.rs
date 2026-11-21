//! Local account password policy validation, `Argon2id` PHC hashing, verification,
//! constant-time dummy verification, and parameter upgrade checks (`11-STEP13` §13.3).

use std::sync::LazyLock;

use argon2::password_hash::phc::PasswordHash;
use argon2::{
    ARGON2ID_IDENT, Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
};
use axpoint_domain::AppError;
use rand::RngCore as _;

/// `Argon2id` memory cost in KiB (`19 MiB = 19,456 KiB`, OWASP minimum baseline, `§13.3`).
pub const ARGON2_M_COST_KIB: u32 = 19_456;

/// `Argon2id` time/iteration cost (`t = 2`, `§13.3`).
pub const ARGON2_T_COST: u32 = 2;

/// `Argon2id` degree of parallelism (`p = 1`, `§13.3`).
pub const ARGON2_P_COST: u32 = 1;

/// Minimum required password length in Unicode scalar values (`§13.3`).
pub const MIN_PASSWORD_CHARS: usize = 12;

/// Maximum accepted password byte length (`1024` bytes) to prevent CPU `DoS`.
const MAX_PASSWORD_BYTES: usize = 1_024;

/// Salt byte length (`128-bit = 16 bytes`).
const SALT_LEN: usize = 16;

/// Lazily initialized valid `Argon2id` PHC hash used by [`verify_dummy_password`] so login attempts
/// against nonexistent or locked accounts spend indistinguishable CPU work (`§13.3`).
static DUMMY_PASSWORD_HASH: LazyLock<String> = LazyLock::new(|| {
    let salt = [0xA5_u8; SALT_LEN];
    if let Ok(ctx) = build_argon2()
        && let Ok(phc) = ctx.hash_password_with_salt(b"axpoint-dummy- timing-guard-pw!", &salt)
    {
        return phc.to_string();
    }
    String::from(
        "$argon2id$v=19$m=19456,t=2,p=1$pKWkpKWkpKWkpKWkpKWkpA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
});

fn build_argon2() -> Result<Argon2<'static>, AppError> {
    let params = Params::new(ARGON2_M_COST_KIB, ARGON2_T_COST, ARGON2_P_COST, None)
        .map_err(|err| AppError::Config(format!("invalid Argon2id parameters: {err}")))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Validates `password` against the platform password policy (`>= 12` characters, must not
/// contain `username` case-insensitively, and no NUL bytes, `11-STEP13` §13.3).
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PASSWORD`) if `password` violates length, content, or
/// username-substring constraints.
pub fn validate_password_policy(username: &str, password: &str) -> Result<(), AppError> {
    if password.contains('\0') {
        return Err(AppError::bad(
            "INVALID_PASSWORD",
            "password must not contain NUL bytes",
        ));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(AppError::bad(
            "INVALID_PASSWORD",
            "password exceeds maximum byte length",
        ));
    }
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(AppError::bad(
            "INVALID_PASSWORD",
            format!("password must be at least {MIN_PASSWORD_CHARS} characters"),
        ));
    }

    let trimmed_user = username.trim();
    if !trimmed_user.is_empty() {
        let lower_pw = password.to_lowercase();
        let lower_user = trimmed_user.to_lowercase();
        if lower_pw.contains(&lower_user) {
            return Err(AppError::bad(
                "INVALID_PASSWORD",
                "password must not contain the username",
            ));
        }
    }

    Ok(())
}

/// Hashes `password` using `Argon2id` (`v=19`, `m=19456`, `t=2`, `p=1`) with a fresh 16-byte
/// CSPRNG salt and returns the encoded PHC string (`$argon2id$v=19$m=19456,t=2,p=1$...`).
///
/// # Errors
/// Returns [`AppError::Config`] if parameter construction or `Argon2id` hashing fails.
pub fn hash_password(password: &str) -> Result<String, AppError> {
    let mut salt = [0_u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    let ctx = build_argon2()?;
    let phc: PasswordHash = ctx
        .hash_password_with_salt(password.as_bytes(), &salt)
        .map_err(|err| AppError::Config(format!("Argon2id password hashing failed: {err}")))?;
    Ok(phc.to_string())
}

/// Verifies `password` in constant time against a stored `Argon2id` PHC `password_hash`.
#[must_use]
pub fn verify_password(password: &str, password_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(password_hash) else {
        return false;
    };
    let Ok(ctx) = build_argon2() else {
        return false;
    };
    ctx.verify_password(password.as_bytes(), &parsed).is_ok()
}

/// Performs a full `Argon2id` verification pass against a lazily computed static PHC hash so
/// authentication failures for unknown or locked users exhibit uniform latency (`§13.3`).
pub fn verify_dummy_password(password: &str) {
    let _matched = verify_password(password, DUMMY_PASSWORD_HASH.as_str());
}

/// Returns `true` when `password_hash` does not use `Argon2id` `v=19` with exact `(m=19456, t=2, p=1)`
/// parameters and should be transparently re-hashed after a successful login (`§13.3`).
#[must_use]
pub fn needs_rehash(password_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(password_hash) else {
        return true;
    };
    if parsed.algorithm != ARGON2ID_IDENT {
        return true;
    }
    if parsed.version != Some(Version::V0x13.into()) {
        return true;
    }
    let Ok(params) = Params::try_from(&parsed) else {
        return true;
    };
    params.m_cost() != ARGON2_M_COST_KIB
        || params.t_cost() != ARGON2_T_COST
        || params.p_cost() != ARGON2_P_COST
}
