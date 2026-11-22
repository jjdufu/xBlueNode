//! Pure citation-bounds verification, grounded numeric counting, and zero-recall abstention rules (`02-STEP16` §16.4.1).

use std::collections::BTreeSet;

/// Deterministic abstention message when 5-way knowledge recall returns zero paragraphs.
pub const ABSTAIN_NO_DOC_EVIDENCE_ZH: &str = "未在当前授权知识库中检索到相关制度或文档依据，无法给出确定结论。请确认文档已入库或调整检索关键词。";

/// Deterministic abstention message when both schema linking and knowledge recall have zero positive hits.
pub const ABSTAIN_NO_DATA_OR_DOC_ZH: &str = "未在当前授权数据集、语义指标或知识库中匹配到相关依据，无法生成回答。请核对表名、指标名或补充上下文范围。";

/// Verification outcome for `[文N]` and `[Doc N]` inline paragraph citations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationVerifyOutcome {
    /// Answer text with all out-of-bounds (`0` or `> recalled_count`) citation tags stripped.
    pub sanitized_answer: String,
    /// Sorted unique 1-based indices (`1..=recalled_count`) validly referenced in `sanitized_answer`.
    pub verified_indices: Vec<usize>,
    /// Out-of-bounds indices (`0` or `> recalled_count`) intercepted and stripped.
    pub stripped_phantom_indices: Vec<usize>,
}

impl CitationVerifyOutcome {
    /// Number of unique verified in-bounds citations.
    #[must_use]
    pub fn verified_count(&self) -> u32 {
        u32::try_from(self.verified_indices.len()).unwrap_or(u32::MAX)
    }
}

/// Scans `raw_answer` for `[文N]` and `[Doc N]` (case-insensitive) markers against `1..=recalled_count`.
///
/// - In-bounds markers (`1 <= N <= recalled_count`) are normalized to `[文N]` and recorded in `verified_indices`.
/// - Out-of-bounds markers (`N == 0` or `N > recalled_count`) are removed cleanly without leaving orphan whitespace.
///
/// # Examples
///
/// ```
/// use axpoint_domain::grounding::verify_and_sanitize_citations;
///
/// let res = verify_and_sanitize_citations("补货阈值为 200 件 [文1] 且需审批 [文99][Doc 2]。", 2);
/// assert_eq!(res.sanitized_answer, "补货阈值为 200 件 [文1] 且需审批 [文2]。");
/// assert_eq!(res.verified_indices, vec![1, 2]);
/// assert_eq!(res.stripped_phantom_indices, vec![99]);
/// assert_eq!(res.verified_count(), 2);
/// ```
#[must_use]
pub fn verify_and_sanitize_citations(
    raw_answer: &str,
    recalled_count: usize,
) -> CitationVerifyOutcome {
    let chars: Vec<char> = raw_answer.chars().collect();
    let mut out = String::with_capacity(raw_answer.len());
    let mut verified = BTreeSet::new();
    let mut phantoms = Vec::new();
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if ch == '['
            && let Some((parsed_num, next_idx)) = try_parse_citation_tag(&chars, idx)
        {
            if parsed_num >= 1 && parsed_num <= recalled_count {
                verified.insert(parsed_num);
                out.push_str("[文");
                out.push_str(&parsed_num.to_string());
                out.push(']');
            } else {
                phantoms.push(parsed_num);
            }
            idx = next_idx;
            continue;
        }
        out.push(ch);
        idx = idx.saturating_add(1);
    }

    CitationVerifyOutcome {
        sanitized_answer: collapse_orphan_spaces(&out),
        verified_indices: verified.into_iter().collect(),
        stripped_phantom_indices: phantoms,
    }
}

/// Strips `[文N]`, `[Doc N]`, and `[表]` markers from `text` so citation indices are not misclassified as business numbers.
#[must_use]
pub fn strip_citation_and_table_markers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if ch == '[' {
            if let Some((_num, next_idx)) = try_parse_citation_tag(&chars, idx) {
                idx = next_idx;
                continue;
            }
            if chars.get(idx.saturating_add(1)) == Some(&'表')
                && chars.get(idx.saturating_add(2)) == Some(&']')
            {
                idx = idx.saturating_add(3);
                continue;
            }
        }
        out.push(ch);
        idx = idx.saturating_add(1);
    }

    collapse_orphan_spaces(&out)
}

/// Counts standalone numeric tokens in `summary_text` (excluding `[文N]` / `[Doc N]` citation markers).
#[must_use]
pub fn count_grounded_numbers(summary_text: &str) -> u32 {
    let stripped = strip_citation_and_table_markers(summary_text);
    let chars: Vec<char> = stripped.chars().collect();
    let mut count = 0_u32;
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if ch.is_ascii_digit() {
            let prev = idx.checked_sub(1).and_then(|p| chars.get(p)).copied();
            let attached_ident =
                prev.is_some_and(|p| p.is_ascii_alphabetic() || p == '_' || p == '-');
            idx = consume_numeric_literal(&chars, idx);
            let next = chars.get(idx).copied();
            let trailing_ident = next.is_some_and(|n| n.is_ascii_alphabetic() || n == '_');
            if !attached_ident && !trailing_ident {
                count = count.saturating_add(1);
            }
            continue;
        }
        idx = idx.saturating_add(1);
    }

    count
}

/// Returns `true` when retrieval and schema linking produced zero actionable evidence and the pipeline must abstain.
#[must_use]
pub const fn should_abstain_zero_evidence(
    has_schema_hit: bool,
    has_session_context: bool,
    recalled_doc_count: usize,
    is_chitchat: bool,
) -> bool {
    !is_chitchat && !has_schema_hit && !has_session_context && recalled_doc_count == 0
}

fn try_parse_citation_tag(chars: &[char], start_idx: usize) -> Option<(usize, usize)> {
    let mut cur = start_idx.saturating_add(1);
    while chars.get(cur) == Some(&' ') {
        cur = cur.saturating_add(1);
    }

    if chars.get(cur) == Some(&'文') {
        cur = cur.saturating_add(1);
    } else if matches_ascii_doc_prefix(chars, cur) {
        cur = cur.saturating_add(3);
    } else {
        return None;
    }

    while chars.get(cur) == Some(&' ') || chars.get(cur) == Some(&'#') {
        cur = cur.saturating_add(1);
    }

    let digit_start = cur;
    let mut digits = String::new();
    while let Some(&d) = chars.get(cur) {
        if d.is_ascii_digit() {
            digits.push(d);
            cur = cur.saturating_add(1);
        } else {
            break;
        }
    }
    if cur == digit_start {
        return None;
    }

    while chars.get(cur) == Some(&' ') {
        cur = cur.saturating_add(1);
    }
    if chars.get(cur) != Some(&']') {
        return None;
    }

    let parsed = digits.parse::<usize>().ok()?;
    Some((parsed, cur.saturating_add(1)))
}

fn matches_ascii_doc_prefix(chars: &[char], idx: usize) -> bool {
    let c0 = chars.get(idx).copied();
    let c1 = chars.get(idx.saturating_add(1)).copied();
    let c2 = chars.get(idx.saturating_add(2)).copied();
    matches!(
        (c0, c1, c2),
        (Some('d' | 'D'), Some('o' | 'O'), Some('c' | 'C'))
    )
}

fn consume_numeric_literal(chars: &[char], start_idx: usize) -> usize {
    let mut cur = start_idx;
    while let Some(&ch) = chars.get(cur) {
        if ch.is_ascii_digit() {
            cur = cur.saturating_add(1);
            continue;
        }
        if (ch == ',' || ch == '.')
            && chars
                .get(cur.saturating_add(1))
                .is_some_and(char::is_ascii_digit)
        {
            cur = cur.saturating_add(1);
            continue;
        }
        break;
    }
    if chars.get(cur) == Some(&'%') {
        cur = cur.saturating_add(1);
    }
    cur
}

fn collapse_orphan_spaces(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = false;
    for ch in raw.chars() {
        if ch == ' ' {
            if !prev_space && !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
                prev_space = true;
            }
            continue;
        }
        if prev_space && (is_closing_punctuation(ch) || ch == '\n') {
            out.pop();
        }
        prev_space = false;
        out.push(ch);
    }
    out.trim().to_owned()
}

const fn is_closing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '。' | '，'
            | '；'
            | '：'
            | '！'
            | '？'
            | '、'
            | '）'
            | '.'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_zero_and_out_of_bounds_citations_across_zh_and_en_tags() {
        let input = "依据条款 [文0] 安全库存低于 500 件触发补货 [文1]，详见附录 [文5] 与 [Doc 9] 及 [doc 2]。";
        let outcome = verify_and_sanitize_citations(input, 2);
        assert_eq!(
            outcome.sanitized_answer,
            "依据条款 安全库存低于 500 件触发补货 [文1]，详见附录 与 及 [文2]。"
        );
        assert_eq!(outcome.verified_indices, vec![1, 2]);
        assert_eq!(outcome.stripped_phantom_indices, vec![0, 5, 9]);
        assert_eq!(outcome.verified_count(), 2);
    }

    #[test]
    fn zero_recalled_chunks_strips_every_citation() {
        let outcome = verify_and_sanitize_citations("凭空捏造引用 [文1] [Doc 1]。", 0);
        assert_eq!(outcome.sanitized_answer, "凭空捏造引用。");
        assert_eq!(outcome.verified_indices, Vec::<usize>::new());
        assert_eq!(outcome.stripped_phantom_indices, vec![1, 1]);
    }

    #[test]
    fn counts_grounded_numbers_ignoring_citations_and_node_keys() {
        let text = "[表] 华东仓 W-1 在库量 500 件，占比 62.5%，金额 33,239.25 元 [文1][Doc 2]。";
        assert_eq!(count_grounded_numbers(text), 3);
    }

    #[test]
    fn zero_evidence_abstention_respects_context_and_chitchat() {
        assert!(should_abstain_zero_evidence(false, false, 0, false));
        assert!(!should_abstain_zero_evidence(true, false, 0, false));
        assert!(!should_abstain_zero_evidence(false, true, 0, false));
        assert!(!should_abstain_zero_evidence(false, false, 1, false));
        assert!(!should_abstain_zero_evidence(false, false, 0, true));
    }
}
