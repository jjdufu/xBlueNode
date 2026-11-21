//! Dynamic column, JSON property, and text-span `PII` masking engine with `HKDF-SHA256`
//! per-tenant `HMAC` key derivation (`11-STEP13` §13.7).

use std::collections::BTreeMap;
use std::fmt;

use axpoint_domain::AppError;
use axpoint_domain::enums::{MaskRule, PiiCategory};
use axpoint_domain::guard::PiiSpan;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use hmac::{Hmac, Mac as _};
use sha2::Sha256;
use zeroize::Zeroizing;

/// Environment variable storing versioned 32-byte Base64 root masking keys (`"1:<b64>,2:<b64>"`).
pub const MASK_KEYS_ENV_VAR: &str = "AXPOINT_MASK_KEYS";

/// Deterministic non-production fallback 32-byte key (`"0123456789abcdef0123456789abcdef"`).
const DEV_FALLBACK_MASK_KEY: [u8; 32] = *b"0123456789abcdef0123456789abcdef";

type HmacSha256 = Hmac<Sha256>;

/// The 81 standard two-character Chinese compound surnames (`复姓`, `§13.7`).
const COMPOUND_SURNAMES: &[&str] = &[
    "欧阳", "太史", "端木", "上官", "司马", "东方", "独孤", "南宫", "万俟", "闻人", "夏侯", "诸葛",
    "尉迟", "公羊", "赫连", "澹台", "皇甫", "宗政", "濮阳", "公冶", "太叔", "申屠", "公孙", "慕容",
    "仲孙", "钟离", "长孙", "宇文", "司徒", "鲜于", "司空", "闾丘", "子车", "亓官", "司寇", "巫马",
    "公西", "颛孙", "壤驷", "公良", "漆雕", "乐正", "宰父", "谷梁", "拓跋", "夹谷", "轩辕", "令狐",
    "段干", "百里", "呼延", "东郭", "南门", "羊舌", "微生", "公户", "公玉", "公仪", "梁丘", "公仲",
    "公上", "公门", "公山", "公坚", "左丘", "公伯", "西门", "公祖", "第五", "公乘", "贯丘", "公皙",
    "南荣", "东里", "东宫", "仲长", "子书", "子桑", "即墨", "达奚", "褚师",
];

/// Versioned root keyring for deterministic `HMAC_HASH` masking (`AXPOINT_MASK_KEYS`, `§13.7`).
#[derive(Clone)]
pub struct MaskKeyRing {
    keys: BTreeMap<i16, Zeroizing<[u8; 32]>>,
    active_version: i16,
}

impl fmt::Debug for MaskKeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let versions: Vec<i16> = self.keys.keys().copied().collect();
        f.debug_struct("MaskKeyRing")
            .field("active_version", &self.active_version)
            .field("key_versions", &versions)
            .field("keys", &"[REDACTED]")
            .finish()
    }
}

impl MaskKeyRing {
    /// Constructs a [`MaskKeyRing`] from an explicit `(version -> 32-byte key)` map.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `keys` is empty or contains a non-positive version.
    pub fn from_keys(keys: BTreeMap<i16, [u8; 32]>) -> Result<Self, AppError> {
        if keys.is_empty() {
            return Err(AppError::Config(String::from(
                "MaskKeyRing must contain at least one key",
            )));
        }
        let mut wrapped = BTreeMap::new();
        let mut max_ver: i16 = 0;
        for (ver, mut raw_key) in keys {
            if ver <= 0 {
                zeroize::Zeroize::zeroize(&mut raw_key);
                return Err(AppError::Config(format!(
                    "mask key version must be >= 1, got {ver}"
                )));
            }
            if ver > max_ver {
                max_ver = ver;
            }
            wrapped.insert(ver, Zeroizing::new(raw_key));
        }
        Ok(Self {
            keys: wrapped,
            active_version: max_ver,
        })
    }

    /// Parses a [`MaskKeyRing`] from `"<version>:<base64_32bytes>,..."`.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] on empty input, malformed entries, duplicate versions, or
    /// decoded key lengths other than `32` bytes.
    pub fn from_env_str(raw: &str) -> Result<Self, AppError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(AppError::Config(String::from(
                "AXPOINT_MASK_KEYS cannot be empty",
            )));
        }

        let mut parsed = BTreeMap::new();
        for part in trimmed.split(',') {
            let entry = part.trim();
            let (ver_s, b64_s) = entry.split_once(':').ok_or_else(|| {
                AppError::Config(String::from(
                    "AXPOINT_MASK_KEYS entry must match '<version>:<base64>'",
                ))
            })?;
            let version: i16 = ver_s.trim().parse().map_err(|err| {
                AppError::Config(format!(
                    "invalid mask key version '{}': {err}",
                    ver_s.trim()
                ))
            })?;
            if version <= 0 || parsed.contains_key(&version) {
                return Err(AppError::Config(format!(
                    "invalid or duplicate mask key version {version}"
                )));
            }
            let decoded = Zeroizing::new(decode_b64_any(b64_s.trim())?);
            let key_arr: [u8; 32] = decoded.as_slice().try_into().map_err(|_| {
                AppError::Config(format!(
                    "mask key v{version} decoded to {} bytes, expected 32 bytes",
                    decoded.len()
                ))
            })?;
            parsed.insert(version, key_arr);
        }
        Self::from_keys(parsed)
    }

    /// Loads [`MaskKeyRing`] from `AXPOINT_MASK_KEYS` and `AXPOINT_ENV`.
    ///
    /// In non-production modes when `AXPOINT_MASK_KEYS` is unset, falls back to the deterministic
    /// development key; in `production`, returns [`AppError::Config`].
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `AXPOINT_MASK_KEYS` is invalid or unset in `production`.
    pub fn from_env() -> Result<Self, AppError> {
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        let raw_keys = std::env::var(MASK_KEYS_ENV_VAR).ok();
        Self::from_env_with_mode(&env_mode, raw_keys.as_deref())
    }

    /// Loads [`MaskKeyRing`] given an explicit `axpoint_env` mode and optional raw key string.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] if `raw_keys` is malformed or missing in `production`.
    pub fn from_env_with_mode(axpoint_env: &str, raw_keys: Option<&str>) -> Result<Self, AppError> {
        match raw_keys.map(str::trim).filter(|s| !s.is_empty()) {
            Some(configured) => Self::from_env_str(configured),
            None if axpoint_env.eq_ignore_ascii_case("production") => Err(AppError::Config(
                String::from("AXPOINT_MASK_KEYS must be configured when AXPOINT_ENV=production"),
            )),
            None => Ok(Self::dev_default()),
        }
    }

    /// Returns the deterministic single-key development [`MaskKeyRing`] (`version = 1`).
    #[must_use]
    pub fn dev_default() -> Self {
        Self {
            keys: BTreeMap::from([(1_i16, Zeroizing::new(DEV_FALLBACK_MASK_KEY))]),
            active_version: 1,
        }
    }

    /// Returns the active key version number.
    #[must_use]
    pub const fn active_version(&self) -> i16 {
        self.active_version
    }

    /// Derives the 32-byte `HKDF-SHA256` tenant masking key (`info = tenant_id`, `§13.7`).
    #[must_use]
    pub fn tenant_key(&self, tenant_id: &str) -> [u8; 32] {
        let root = self
            .keys
            .get(&self.active_version)
            .map_or(&DEV_FALLBACK_MASK_KEY, |z| &**z);
        derive_tenant_hmac_key(root, tenant_id)
    }

    /// Alias for [`MaskKeyRing::tenant_key`].
    #[must_use]
    pub fn derive_tenant_key(&self, tenant_id: &str) -> [u8; 32] {
        self.tenant_key(tenant_id)
    }
}

/// Derives `k_tenant = HKDF-SHA256(ikm = root_key, salt = None, info = tenant_id)` (`§13.7`).
#[must_use]
pub fn derive_tenant_hmac_key(root_key: &[u8; 32], tenant_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, root_key);
    let mut okm = [0_u8; 32];
    let _ok = hk.expand(tenant_id.as_bytes(), &mut okm);
    okm
}

/// Applies `rule` to a non-null string `value` using `tenant_hmac_key` for `HMAC_HASH` (`§13.7`).
#[must_use]
pub fn apply_mask_rule(rule: MaskRule, value: &str, tenant_hmac_key: &[u8; 32]) -> String {
    match rule {
        MaskRule::None => String::from(value),
        MaskRule::MaskPhone => mask_phone(value),
        MaskRule::MaskIdCard => mask_id_card(value),
        MaskRule::MaskEmail => mask_email(value),
        MaskRule::MaskBankCard => mask_bank_card(value),
        MaskRule::MaskName => mask_name(value),
        MaskRule::MaskAddress => mask_address(value),
        MaskRule::HmacHash => mask_hmac_hash(value, tenant_hmac_key),
        MaskRule::Redact => String::from("[REDACTED]"),
    }
}

/// Masks in-place any property in `props` matching an entry in `masks` (`null` JSON values stay `null`).
pub fn mask_json_properties(
    props: &mut serde_json::Map<String, serde_json::Value>,
    masks: &BTreeMap<String, MaskRule>,
    tenant_hmac_key: &[u8; 32],
) {
    if masks.is_empty() {
        return;
    }
    for (prop_key, prop_val) in props.iter_mut() {
        if prop_val.is_null() {
            continue;
        }
        let rule_opt = masks.get(prop_key).copied().or_else(|| {
            masks
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(prop_key))
                .map(|(_, &r)| r)
        });
        let Some(rule) = rule_opt else {
            continue;
        };
        if rule == MaskRule::None {
            continue;
        }
        let raw_text = match &*prop_val {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        *prop_val = serde_json::Value::String(apply_mask_rule(rule, &raw_text, tenant_hmac_key));
    }
}

/// Masks non-overlapping Unicode character `spans` in `text` according to `rules` (falling back
/// to [`MaskRule::from_pii_category`], `§13.8.2`).
#[must_use]
pub fn mask_text_spans(
    text: &str,
    spans: &[PiiSpan],
    rules: &BTreeMap<PiiCategory, MaskRule>,
    tenant_hmac_key: &[u8; 32],
) -> String {
    if text.is_empty() || spans.is_empty() {
        return String::from(text);
    }

    let chars: Vec<char> = text.chars().collect();
    let total_chars = chars.len();
    let mut sorted: Vec<&PiiSpan> = spans
        .iter()
        .filter(|s| s.start < s.end && s.end <= total_chars && s.category != PiiCategory::None)
        .collect();
    sorted.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0_usize;

    for span in sorted {
        if span.start < cursor {
            continue;
        }
        let rule = rules
            .get(&span.category)
            .copied()
            .unwrap_or_else(|| MaskRule::from_pii_category(span.category));
        if rule == MaskRule::None {
            continue;
        }
        if let Some(prefix_chars) = chars.get(cursor..span.start) {
            out.extend(prefix_chars.iter().copied());
        }
        if let Some(matched_chars) = chars.get(span.start..span.end) {
            let raw: String = matched_chars.iter().copied().collect();
            out.push_str(&apply_mask_rule(rule, &raw, tenant_hmac_key));
        }
        cursor = span.end;
    }

    if let Some(tail) = chars.get(cursor..) {
        out.extend(tail.iter().copied());
    }
    out
}

fn normalize_fullwidth(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch == '\u{3000}' {
            out.push(' ');
        } else if ('\u{FF01}'..='\u{FF5E}').contains(&ch) {
            let code = u32::from(ch).saturating_sub(0xFEE0);
            out.push(char::from_u32(code).unwrap_or(ch));
        } else {
            out.push(ch);
        }
    }
    out
}

fn mask_phone(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let norm = normalize_fullwidth(value);
    let trimmed = norm.trim();

    let (prefix, rest) = if let Some(r) = trimmed.strip_prefix("+86-") {
        ("+86-", r)
    } else if let Some(r) = trimmed.strip_prefix("+86 ") {
        ("+86 ", r)
    } else if let Some(r) = trimmed.strip_prefix("+86") {
        ("+86", r)
    } else {
        ("", trimmed)
    };

    let digits: String = rest.chars().filter(|&c| c != '-' && c != ' ').collect();
    if digits.len() < 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return String::from("****");
    }

    let head = digits.get(..3).unwrap_or("");
    let tail = digits.get(digits.len().saturating_sub(4)..).unwrap_or("");
    let star_count = digits.len().saturating_sub(7).max(4);
    format!("{prefix}{head}{}{tail}", "*".repeat(star_count))
}

fn mask_id_card(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let norm = normalize_fullwidth(value);
    let trimmed = norm.trim();
    if trimmed.len() < 15 || !trimmed.is_ascii() {
        return String::from("****");
    }
    let body_len = trimmed.len().saturating_sub(1);
    let Some(body) = trimmed.get(..body_len) else {
        return String::from("****");
    };
    let Some(last) = trimmed.bytes().last() else {
        return String::from("****");
    };
    if !body.bytes().all(|b| b.is_ascii_digit())
        || !(last.is_ascii_digit() || last == b'X' || last == b'x')
    {
        return String::from("****");
    }

    let head = trimmed.get(..6).unwrap_or("");
    let tail = trimmed.get(trimmed.len().saturating_sub(4)..).unwrap_or("");
    let star_count = trimmed.len().saturating_sub(10);
    format!("{head}{}{tail}", "*".repeat(star_count))
}

fn mask_email(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let norm = normalize_fullwidth(value);
    let trimmed = norm.trim();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return String::from("****");
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') || !domain.contains('.') {
        return String::from("****");
    }
    let mut local_chars = local.chars();
    let Some(first) = local_chars.next() else {
        return String::from("****");
    };
    let rest_len = local_chars.count();
    let stars = "*".repeat(rest_len.max(1));
    format!("{first}{stars}@{domain}")
}

fn mask_bank_card(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let norm = normalize_fullwidth(value);
    let digits: String = norm
        .trim()
        .chars()
        .filter(|&c| c != ' ' && c != '-')
        .collect();
    if digits.len() < 12 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return String::from("****");
    }
    let tail = digits.get(digits.len().saturating_sub(4)..).unwrap_or("");
    let stars = "*".repeat(digits.len().saturating_sub(4));
    format!("{stars}{tail}")
}

fn mask_name(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 1 {
        return String::from("*");
    }
    if chars.len() >= 3
        && let Some(first_two) = chars.get(..2)
    {
        let cand: String = first_two.iter().copied().collect();
        if COMPOUND_SURNAMES.contains(&cand.as_str()) {
            let stars = "*".repeat(chars.len().saturating_sub(2));
            return format!("{cand}{stars}");
        }
    }
    let first = chars.first().copied().unwrap_or('*');
    let stars = "*".repeat(chars.len().saturating_sub(1));
    format!("{first}{stars}")
}

fn mask_address(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let mut end_byte = 0_usize;
    if let Some(prov_end) = match_province_prefix(value) {
        end_byte = prov_end;
    }
    if let Some(rest) = value.get(end_byte..)
        && let Some(city_len) = match_city_prefix(rest)
    {
        end_byte = end_byte.saturating_add(city_len);
    }
    if let Some(rest) = value.get(end_byte..)
        && let Some(county_len) = match_county_prefix(rest)
    {
        end_byte = end_byte.saturating_add(county_len);
    }
    if end_byte == 0 {
        return String::from("****");
    }
    let prefix = value.get(..end_byte).unwrap_or("");
    format!("{prefix}****")
}

fn match_province_prefix(s: &str) -> Option<usize> {
    let mut chars_seen = 0_usize;
    for (byte_idx, ch) in s.char_indices() {
        chars_seen = chars_seen.saturating_add(1);
        if chars_seen > 10 || is_street_delimiter(ch) {
            break;
        }
        let after = byte_idx.saturating_add(ch.len_utf8());
        let head = s.get(..after)?;
        if chars_seen >= 2
            && (head.ends_with("自治区") || head.ends_with("特别行政区") || ch == '省')
        {
            return Some(after);
        }
    }
    None
}

fn match_city_prefix(s: &str) -> Option<usize> {
    let mut chars_seen = 0_usize;
    for (byte_idx, ch) in s.char_indices() {
        chars_seen = chars_seen.saturating_add(1);
        if chars_seen > 12 || is_street_delimiter(ch) || matches!(ch, '县' | '旗') {
            break;
        }
        let after = byte_idx.saturating_add(ch.len_utf8());
        let head = s.get(..after)?;
        if chars_seen >= 2
            && (head.ends_with("自治州") || head.ends_with("地区") || ch == '市' || ch == '盟')
        {
            return Some(after);
        }
    }
    None
}

fn match_county_prefix(s: &str) -> Option<usize> {
    let mut chars_seen = 0_usize;
    for (byte_idx, ch) in s.char_indices() {
        chars_seen = chars_seen.saturating_add(1);
        if chars_seen > 12 || is_street_delimiter(ch) {
            break;
        }
        let after = byte_idx.saturating_add(ch.len_utf8());
        let head = s.get(..after)?;
        if chars_seen >= 2 && matches!(ch, '区' | '县' | '旗') {
            let is_non_admin_zone = head.ends_with("园区")
                || head.ends_with("小区")
                || head.ends_with("校区")
                || head.ends_with("厂区")
                || head.ends_with("景区");
            if !is_non_admin_zone {
                return Some(after);
            }
        }
    }
    None
}

const fn is_street_delimiter(ch: char) -> bool {
    matches!(ch, '镇' | '乡' | '街' | '路' | '道' | '巷' | '弄' | '号')
}

fn mask_hmac_hash(value: &str, tenant_hmac_key: &[u8; 32]) -> String {
    if value.is_empty() {
        return String::new();
    }
    let Ok(mut mac) = HmacSha256::new_from_slice(tenant_hmac_key) else {
        return String::from("[REDACTED]");
    };
    mac.update(value.as_bytes());
    let digest = mac.finalize().into_bytes();
    let first8 = digest.get(..8).unwrap_or(&[]);
    hex::encode(first8)
}

fn decode_b64_any(encoded: &str) -> Result<Vec<u8>, AppError> {
    STANDARD
        .decode(encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(encoded))
        .or_else(|_| URL_SAFE.decode(encoded))
        .or_else(|_| URL_SAFE_NO_PAD.decode(encoded))
        .map_err(|err| AppError::Config(format!("invalid base64 mask key: {err}")))
}
