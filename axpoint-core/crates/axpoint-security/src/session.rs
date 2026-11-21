//! Refresh token generation (`axr_<32 base62>`), `SHA-256` hashing, and `HttpOnly` cookie
//! helpers (`11-STEP13` §13.4.2).

use rand::RngCore as _;

use crate::authenticator::sha256_hex;

/// `HttpOnly` cookie name carrying the opaque refresh token (`§13.4.2`).
pub const REFRESH_COOKIE_NAME: &str = "axp_rt";

/// Legacy alias accepted when extracting incoming refresh cookies.
const LEGACY_REFRESH_COOKIE_NAME: &str = "axp_refresh";

/// Refresh session time-to-live in days (`14` days, `§13.4.2`).
pub const REFRESH_TTL_DAYS: i64 = 14;

/// Refresh cookie `Max-Age` in seconds (`14 * 86,400 = 1,209,600`).
pub const REFRESH_MAX_AGE_SECS: i64 = REFRESH_TTL_DAYS * 86_400;

/// Number of Base62 random characters appended after `"axr_"`.
const REFRESH_SECRET_CHARS: usize = 32;

const BASE62_ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Generates a fresh opaque refresh token `("axr_<32 base62>", sha256_hex)` using unbiased
/// rejection sampling over OS CSPRNG bytes (`§13.4.2`).
#[must_use]
pub fn generate_refresh_token() -> (String, String) {
    let mut rng = rand::rng();
    let mut raw = String::with_capacity(4 + REFRESH_SECRET_CHARS);
    raw.push_str("axr_");

    // Largest multiple of 62 <= 255 is 62 * 4 = 248 (0..248 is unbiased mod 62).
    let mut buf = [0_u8; 64];
    while raw.len() < 4 + REFRESH_SECRET_CHARS {
        rng.fill_bytes(&mut buf);
        for &byte in &buf {
            if byte < 248 {
                let idx = usize::from(byte % 62);
                if let Some(&ch) = BASE62_ALPHABET.get(idx) {
                    raw.push(char::from(ch));
                    if raw.len() == 4 + REFRESH_SECRET_CHARS {
                        break;
                    }
                }
            }
        }
    }

    let hash = hash_refresh_token(&raw);
    (raw, hash)
}

/// Computes the lowercase hexadecimal `SHA-256` digest of a raw refresh token (`§13.4.2`).
#[must_use]
pub fn hash_refresh_token(raw: &str) -> String {
    sha256_hex(raw)
}

/// Extracts the raw refresh token from an HTTP `Cookie` request header (`axp_rt=...`).
#[must_use]
pub fn extract_refresh_cookie(cookie_header: Option<&str>) -> Option<String> {
    let header = cookie_header?;
    let mut legacy_fallback: Option<String> = None;

    for part in header.split(';') {
        let trimmed = part.trim();
        let Some((name, val)) = trimmed.split_once('=') else {
            continue;
        };
        let key = name.trim();
        let value = val.trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        if key == REFRESH_COOKIE_NAME {
            return Some(String::from(value));
        }
        if key == LEGACY_REFRESH_COOKIE_NAME && legacy_fallback.is_none() {
            legacy_fallback = Some(String::from(value));
        }
    }

    legacy_fallback
}

/// Formats the `Set-Cookie` header value issuing a rotated or newly created refresh token (`§13.4.2`).
#[must_use]
pub fn build_set_refresh_cookie(raw_token: &str, max_age_secs: i64, secure: bool) -> String {
    let clamped_age = max_age_secs.max(0);
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{REFRESH_COOKIE_NAME}={raw_token}; HttpOnly; SameSite=Strict; Path=/api/auth; Max-Age={clamped_age}{secure_attr}"
    )
}

/// Formats the `Set-Cookie` header value clearing the client's refresh cookie on logout (`§13.4.2`).
#[must_use]
pub fn build_clear_refresh_cookie(secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{REFRESH_COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/api/auth; Max-Age=0{secure_attr}"
    )
}
