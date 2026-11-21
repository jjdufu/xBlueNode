//! Fast deterministic in-process `PII` span recognizer (`CN_PHONE`, `CN_ID_CARD` `GB 11643`,
//! `BANK_CARD` `Luhn`, `EMAIL`, `CN_ADDRESS`, `SECRET`) and `⟨CATEGORY_N⟩` `LLM` outbound egress
//! redactor/restorer (`11-STEP13` §13.8.3).

use std::collections::{BTreeMap, HashMap};

use axpoint_domain::enums::{MaskRule, PiiCategory};
use axpoint_domain::guard::{LlmEgressPreviewResponse, PiiSpan};

use crate::masking::apply_mask_rule;

/// Per-turn ephemeral placeholder session mapping `⟨CATEGORY_N⟩` tokens back to `(PiiCategory, raw_value)`.
///
/// Stored strictly in memory for the duration of a single `LLM` request/response turn (`§13.8.3`).
#[derive(Debug, Clone, Default)]
pub struct EgressSession {
    placeholder_to_raw: BTreeMap<String, (PiiCategory, String)>,
    raw_to_placeholder: HashMap<(PiiCategory, String), String>,
    counters: BTreeMap<PiiCategory, usize>,
}

impl EgressSession {
    /// Creates an empty [`EgressSession`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if no `PII` placeholders were emitted during outbound redaction.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.placeholder_to_raw.is_empty()
    }

    /// Returns the `⟨CATEGORY_N⟩ -> original_value` map (used by debug preview endpoints).
    #[must_use]
    pub fn placeholders_map(&self) -> BTreeMap<String, String> {
        self.placeholder_to_raw
            .iter()
            .map(|(k, (_, v))| (k.clone(), v.clone()))
            .collect()
    }

    /// Redacts detected `PII` spans in `text` into `⟨CATEGORY_N⟩` placeholders, deduplicating
    /// identical `(category, raw_value)` occurrences across messages in the same turn.
    pub fn redact_text(&mut self, text: &str) -> String {
        let spans = analyze_pii_local(text);
        if spans.is_empty() {
            return String::from(text);
        }

        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0_usize;

        for span in spans {
            if span.start < cursor || span.end > chars.len() {
                continue;
            }
            if let Some(prefix) = chars.get(cursor..span.start) {
                out.extend(prefix.iter().copied());
            }
            if let Some(matched) = chars.get(span.start..span.end) {
                let raw: String = matched.iter().copied().collect();
                let placeholder = self.intern_placeholder(span.category, raw);
                out.push_str(&placeholder);
            }
            cursor = span.end;
        }

        if let Some(tail) = chars.get(cursor..) {
            out.extend(tail.iter().copied());
        }
        out
    }

    /// Restores all `⟨CATEGORY_N⟩` placeholders in `text` to original values when
    /// `allow_raw_pii == true`, or masked values via [`apply_mask_rule`] when `false`.
    #[must_use]
    pub fn restore_text(
        &self,
        text: &str,
        allow_raw_pii: bool,
        tenant_hmac_key: &[u8; 32],
    ) -> String {
        if self.placeholder_to_raw.is_empty() || !text.contains('⟨') {
            return String::from(text);
        }
        let mut current = String::from(text);
        for (token, (category, raw_val)) in &self.placeholder_to_raw {
            if current.contains(token.as_str()) {
                let replacement = if allow_raw_pii {
                    raw_val.clone()
                } else {
                    apply_mask_rule(
                        MaskRule::from_pii_category(*category),
                        raw_val,
                        tenant_hmac_key,
                    )
                };
                current = current.replace(token.as_str(), &replacement);
            }
        }
        current
    }

    /// Recursively restores `⟨CATEGORY_N⟩` placeholders inside all string leaves of a `JSON` value.
    pub fn restore_json_value(
        &self,
        val: &mut serde_json::Value,
        allow_raw_pii: bool,
        tenant_hmac_key: &[u8; 32],
    ) {
        if self.placeholder_to_raw.is_empty() {
            return;
        }
        match val {
            serde_json::Value::String(s) => {
                if s.contains('⟨') {
                    *s = self.restore_text(s, allow_raw_pii, tenant_hmac_key);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    self.restore_json_value(item, allow_raw_pii, tenant_hmac_key);
                }
            }
            serde_json::Value::Object(map) => {
                for v in map.values_mut() {
                    self.restore_json_value(v, allow_raw_pii, tenant_hmac_key);
                }
            }
            _ => {}
        }
    }

    fn intern_placeholder(&mut self, category: PiiCategory, raw: String) -> String {
        let key = (category, raw.clone());
        if let Some(existing) = self.raw_to_placeholder.get(&key) {
            return existing.clone();
        }
        let next_idx = self
            .counters
            .get(&category)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.counters.insert(category, next_idx);
        let token = format!("⟨{}_{next_idx}⟩", category.as_str());
        self.raw_to_placeholder.insert(key, token.clone());
        self.placeholder_to_raw
            .insert(token.clone(), (category, raw));
        token
    }
}

/// Convenience helper redacting a single string into `(outbound_text, EgressSession)` (`§13.8.3`).
#[must_use]
pub fn redact_for_egress(text: &str) -> (String, EgressSession) {
    let mut session = EgressSession::new();
    let outbound = session.redact_text(text);
    (outbound, session)
}

/// Convenience helper restoring `text` from an [`EgressSession`] (`§13.8.3`).
#[must_use]
pub fn restore_from_egress(
    text: &str,
    session: &EgressSession,
    allow_raw_pii: bool,
    tenant_hmac_key: &[u8; 32],
) -> String {
    session.restore_text(text, allow_raw_pii, tenant_hmac_key)
}

/// Builds the full [`LlmEgressPreviewResponse`] for `POST /api/axguard/debug/llm-egress-preview` (`§13.8.3`).
#[must_use]
pub fn preview_llm_egress(text: &str, tenant_hmac_key: &[u8; 32]) -> LlmEgressPreviewResponse {
    let (outbound, session) = redact_for_egress(text);
    let restored = session.restore_text(&outbound, true, tenant_hmac_key);
    let masked = session.restore_text(&outbound, false, tenant_hmac_key);
    LlmEgressPreviewResponse {
        outbound,
        restored,
        masked,
        placeholders: session.placeholders_map(),
    }
}

/// Scans `text` deterministically in pure `Rust` and returns sorted, non-overlapping [`PiiSpan`]s
/// with Unicode scalar (`char`) offsets (`§13.8`).
#[must_use]
pub fn analyze_pii_local(text: &str) -> Vec<PiiSpan> {
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut candidates = Vec::new();

    scan_secrets(&chars, &mut candidates);
    scan_emails(&chars, &mut candidates);
    scan_id_cards(&chars, &mut candidates);
    scan_bank_cards(&chars, &mut candidates);
    scan_cn_phones(&chars, &mut candidates);
    scan_cn_addresses(&chars, &mut candidates);

    resolve_non_overlapping_spans(candidates)
}

fn resolve_non_overlapping_spans(mut candidates: Vec<PiiSpan>) -> Vec<PiiSpan> {
    // Higher score wins; on tie, earlier start and longer span win.
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.start.cmp(&b.start))
            .then_with(|| {
                b.end
                    .saturating_sub(b.start)
                    .cmp(&a.end.saturating_sub(a.start))
            })
    });

    let mut accepted: Vec<PiiSpan> = Vec::new();
    for cand in candidates {
        let overlaps = accepted
            .iter()
            .any(|ex| cand.start < ex.end && ex.start < cand.end);
        if !overlaps {
            accepted.push(cand);
        }
    }
    accepted.sort_by_key(|s| s.start);
    accepted
}

fn scan_secrets(chars: &[char], out: &mut Vec<PiiSpan>) {
    let text: String = chars.iter().copied().collect();
    // 1. PEM header blocks
    let mut search_from = 0_usize;
    while let Some(rel) = text.get(search_from..).and_then(|s| s.find("-----BEGIN ")) {
        let byte_start = search_from.saturating_add(rel);
        if let Some(after) = text.get(byte_start.saturating_add(11)..)
            && let Some(end_dashes) = after.find("-----")
        {
            let byte_end = byte_start
                .saturating_add(11)
                .saturating_add(end_dashes)
                .saturating_add(5);
            let char_start = text.get(..byte_start).map_or(0, |s| s.chars().count());
            let char_end = text
                .get(..byte_end)
                .map_or(char_start, |s| s.chars().count());
            if char_end > char_start {
                out.push(PiiSpan {
                    category: PiiCategory::Secret,
                    start: char_start,
                    end: char_end,
                    score: 0.99,
                });
            }
            search_from = byte_end;
            continue;
        }
        break;
    }

    // 2. Token prefixes (`sk-...`, `axp_...`, `axr_...`) and `password=...` / `api_key=...`
    let mut idx = 0_usize;
    while idx < chars.len() {
        if let Some(end) = match_prefixed_token(chars, idx).or_else(|| match_kv_secret(chars, idx))
        {
            out.push(PiiSpan {
                category: PiiCategory::Secret,
                start: idx,
                end,
                score: 0.95,
            });
            idx = end;
        } else {
            idx = idx.saturating_add(1);
        }
    }
}

fn match_prefixed_token(chars: &[char], start: usize) -> Option<usize> {
    if start > 0
        && chars
            .get(start.saturating_sub(1))
            .is_some_and(char::is_ascii_alphanumeric)
    {
        return None;
    }
    let slice = chars.get(start..)?;
    let prefix_len = if starts_with_ascii(slice, "sk-") {
        3
    } else if starts_with_ascii(slice, "axp_") || starts_with_ascii(slice, "axr_") {
        4
    } else {
        return None;
    };
    let mut len = prefix_len;
    while let Some(&c) = slice.get(len) {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            len = len.saturating_add(1);
        } else {
            break;
        }
    }
    (len >= prefix_len.saturating_add(10)).then_some(start.saturating_add(len))
}

fn match_kv_secret(chars: &[char], start: usize) -> Option<usize> {
    if start > 0
        && chars
            .get(start.saturating_sub(1))
            .is_some_and(char::is_ascii_alphanumeric)
    {
        return None;
    }
    let slice = chars.get(start..)?;
    let keywords = [
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "api-key",
        "access_token",
    ];
    let mut kw_len = 0_usize;
    for kw in keywords {
        if starts_with_ascii_ci(slice, kw) {
            kw_len = kw.len();
            break;
        }
    }
    if kw_len == 0 {
        return None;
    }
    let mut cur = kw_len;
    while slice.get(cur).is_some_and(|c| *c == ' ' || *c == '\t') {
        cur = cur.saturating_add(1);
    }
    if !slice.get(cur).is_some_and(|c| *c == '=' || *c == ':') {
        return None;
    }
    cur = cur.saturating_add(1);
    while slice.get(cur).is_some_and(|c| *c == ' ' || *c == '\t') {
        cur = cur.saturating_add(1);
    }
    let val_start = cur;
    while let Some(&c) = slice.get(cur) {
        if c.is_whitespace() || matches!(c, ',' | ';' | ')' | ']' | '}') {
            break;
        }
        cur = cur.saturating_add(1);
    }
    (cur.saturating_sub(val_start) >= 6).then_some(start.saturating_add(cur))
}

fn scan_emails(chars: &[char], out: &mut Vec<PiiSpan>) {
    let mut idx = 0_usize;
    while idx < chars.len() {
        if chars.get(idx) == Some(&'@') {
            let mut l = idx;
            while l > 0
                && chars
                    .get(l.saturating_sub(1))
                    .is_some_and(|c| is_email_local_char(*c))
            {
                l = l.saturating_sub(1);
            }
            let mut r = idx.saturating_add(1);
            while chars.get(r).is_some_and(|c| is_email_domain_char(*c)) {
                r = r.saturating_add(1);
            }
            while r > idx.saturating_add(1) && chars.get(r.saturating_sub(1)) == Some(&'.') {
                r = r.saturating_sub(1);
            }
            if l < idx && r > idx.saturating_add(3) {
                let dom: String = chars
                    .get(idx.saturating_add(1)..r)
                    .unwrap_or(&[])
                    .iter()
                    .copied()
                    .collect();
                if dom.contains('.') && !dom.starts_with('.') && !dom.ends_with('.') {
                    out.push(PiiSpan {
                        category: PiiCategory::Email,
                        start: l,
                        end: r,
                        score: 0.92,
                    });
                    idx = r;
                    continue;
                }
            }
        }
        idx = idx.saturating_add(1);
    }
}

fn scan_id_cards(chars: &[char], out: &mut Vec<PiiSpan>) {
    if chars.len() < 18 {
        return;
    }
    let limit = chars.len().saturating_sub(18);
    let mut idx = 0_usize;
    while idx <= limit {
        let prev_alnum = idx > 0
            && chars
                .get(idx.saturating_sub(1))
                .is_some_and(char::is_ascii_alphanumeric);
        let next_alnum = chars
            .get(idx.saturating_add(18))
            .is_some_and(char::is_ascii_alphanumeric);
        if !prev_alnum
            && !next_alnum
            && let Some(window) = chars.get(idx..idx.saturating_add(18))
            && let Some(score) = score_cn_id_card(window)
        {
            out.push(PiiSpan {
                category: PiiCategory::IdCard,
                start: idx,
                end: idx.saturating_add(18),
                score,
            });
            idx = idx.saturating_add(18);
            continue;
        }
        idx = idx.saturating_add(1);
    }
}

fn score_cn_id_card(w: &[char]) -> Option<f64> {
    let first = *w.first()?;
    if !('1'..='8').contains(&first) {
        return None;
    }
    if !w.get(..17)?.iter().all(char::is_ascii_digit) {
        return None;
    }
    let last = *w.get(17)?;
    if !(last.is_ascii_digit() || last == 'X' || last == 'x') {
        return None;
    }
    let year: u32 = w.get(6..10)?.iter().collect::<String>().parse().ok()?;
    let month: u32 = w.get(10..12)?.iter().collect::<String>().parse().ok()?;
    let day: u32 = w.get(12..14)?.iter().collect::<String>().parse().ok()?;
    if !(1900..=2099).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if gb11643_checksum_valid(w) {
        Some(0.98)
    } else {
        Some(0.82)
    }
}

fn gb11643_checksum_valid(w: &[char]) -> bool {
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK_CHARS: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    let mut sum = 0_u32;
    for (i, &wt) in WEIGHTS.iter().enumerate() {
        let Some(d) = w.get(i).and_then(|c| c.to_digit(10)) else {
            return false;
        };
        sum = sum.saturating_add(d.saturating_mul(wt));
    }
    let Some(&expected) = CHECK_CHARS.get(usize::try_from(sum % 11).unwrap_or(0)) else {
        return false;
    };
    let Some(&actual) = w.get(17) else {
        return false;
    };
    actual.to_ascii_uppercase() == expected
}

fn scan_bank_cards(chars: &[char], out: &mut Vec<PiiSpan>) {
    let mut idx = 0_usize;
    while idx < chars.len() {
        let prev_digit = idx > 0
            && chars
                .get(idx.saturating_sub(1))
                .is_some_and(char::is_ascii_digit);
        if !prev_digit
            && chars.get(idx).is_some_and(|c| matches!(*c, '3'..='6'))
            && let Some((end, digits)) = consume_card_run(chars, idx)
            && (16..=19).contains(&digits.len())
            && luhn_valid(&digits)
        {
            out.push(PiiSpan {
                category: PiiCategory::BankCard,
                start: idx,
                end,
                score: 0.90,
            });
            idx = end;
            continue;
        }
        idx = idx.saturating_add(1);
    }
}

fn consume_card_run(chars: &[char], start: usize) -> Option<(usize, Vec<u8>)> {
    let mut digits = Vec::with_capacity(19);
    let mut cur = start;
    while let Some(&ch) = chars.get(cur) {
        if ch.is_ascii_digit() {
            digits.push(u8::try_from(ch.to_digit(10).unwrap_or(0)).unwrap_or(0));
            cur = cur.saturating_add(1);
            if digits.len() > 19 {
                return None;
            }
        } else if (ch == ' ' || ch == '-')
            && chars
                .get(cur.saturating_add(1))
                .is_some_and(char::is_ascii_digit)
        {
            cur = cur.saturating_add(1);
        } else {
            break;
        }
    }
    if chars.get(cur).is_some_and(char::is_ascii_digit) {
        return None;
    }
    Some((cur, digits))
}

fn luhn_valid(digits: &[u8]) -> bool {
    let mut sum = 0_u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut v = u32::from(d);
        if double {
            v = v.saturating_mul(2);
            if v > 9 {
                v = v.saturating_sub(9);
            }
        }
        sum = sum.saturating_add(v);
        double = !double;
    }
    sum != 0 && sum.is_multiple_of(10)
}

fn scan_cn_phones(chars: &[char], out: &mut Vec<PiiSpan>) {
    let mut idx = 0_usize;
    while idx < chars.len() {
        let prev_digit = idx > 0
            && chars
                .get(idx.saturating_sub(1))
                .is_some_and(char::is_ascii_digit);
        if !prev_digit && let Some(end) = match_cn_mobile(chars, idx) {
            out.push(PiiSpan {
                category: PiiCategory::Phone,
                start: idx,
                end,
                score: 0.94,
            });
            idx = end;
            continue;
        }
        idx = idx.saturating_add(1);
    }
}

fn match_cn_mobile(chars: &[char], start: usize) -> Option<usize> {
    let mut cur = start;
    if chars.get(cur) == Some(&'+')
        && chars.get(cur.saturating_add(1)) == Some(&'8')
        && chars.get(cur.saturating_add(2)) == Some(&'6')
    {
        cur = cur.saturating_add(3);
        if chars.get(cur).is_some_and(|c| *c == '-' || *c == ' ') {
            cur = cur.saturating_add(1);
        }
    }
    let body = chars.get(cur..cur.saturating_add(11))?;
    if *body.first()? != '1' || !matches!(*body.get(1)?, '3'..='9') {
        return None;
    }
    if !body.iter().all(char::is_ascii_digit) {
        return None;
    }
    let end = cur.saturating_add(11);
    if chars.get(end).is_some_and(char::is_ascii_digit) {
        return None;
    }
    Some(end)
}

fn scan_cn_addresses(chars: &[char], out: &mut Vec<PiiSpan>) {
    let mut idx = 0_usize;
    while idx < chars.len() {
        if is_cjk(*chars.get(idx).unwrap_or(&' '))
            && let Some(end) = match_cn_address_at(chars, idx)
        {
            out.push(PiiSpan {
                category: PiiCategory::Address,
                start: idx,
                end,
                score: 0.80,
            });
            idx = end;
            continue;
        }
        idx = idx.saturating_add(1);
    }
}

fn match_cn_address_at(chars: &[char], start: usize) -> Option<usize> {
    let max_end = chars.len().min(start.saturating_add(48));
    let mut admin_pos: Option<usize> = None;
    for pos in start.saturating_add(2)..max_end.min(start.saturating_add(16)) {
        let ch = *chars.get(pos)?;
        if !is_cjk(ch) {
            break;
        }
        if matches!(ch, '省' | '市' | '区' | '县' | '旗') {
            admin_pos = Some(pos);
        }
    }
    let adm = admin_pos?;
    let mut last_street_marker: Option<usize> = None;
    let mut cur = adm.saturating_add(1);
    while cur < max_end {
        let ch = *chars.get(cur)?;
        if ch.is_whitespace() || matches!(ch, '，' | '。' | '；' | ',' | ';' | '(' | '（') {
            break;
        }
        if matches!(ch, '路' | '街' | '道' | '巷' | '号' | '室' | '楼' | '园') {
            last_street_marker = Some(cur.saturating_add(1));
        }
        cur = cur.saturating_add(1);
    }
    last_street_marker
}

const fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{4E00}'..='\u{9FFF}')
}

const fn is_email_local_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '%' | '+' | '-')
}

const fn is_email_domain_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-')
}

fn starts_with_ascii(chars: &[char], prefix: &str) -> bool {
    chars.len() >= prefix.len() && chars.iter().zip(prefix.chars()).all(|(&a, b)| a == b)
}

fn starts_with_ascii_ci(chars: &[char], prefix: &str) -> bool {
    chars.len() >= prefix.len()
        && chars
            .iter()
            .zip(prefix.chars())
            .all(|(&a, b)| a.eq_ignore_ascii_case(&b))
}
