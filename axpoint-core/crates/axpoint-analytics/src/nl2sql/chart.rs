//! Chart type recommendation and numeric-grounding summary verifier (`05-STEP04` §04.6.6).

use axpoint_domain::analytics::{ChartSuggestion, QueryColumn};
use axpoint_domain::enums::{DataType, WidgetKind};

const SHARE_KEYWORDS: &[&str] = &[
    "占比",
    "比例",
    "份额",
    "构成",
    "比重",
    "百分比",
    "share",
    "proportion",
    "ratio",
    "percentage",
];

/// Recommends a [`ChartSuggestion`] from output columns, row count, and question phrasing (`§04.6.6`).
///
/// Rules:
/// - Single row & single column (`row_count == 1 && columns.len() == 1`) → [`WidgetKind::KpiCard`]
/// - 1 temporal (`DATE` / `TIMESTAMP`) column + `>= 1` numeric column (and no extra categorical columns) → [`WidgetKind::LineChart`]
/// - 1 categorical column + 1 numeric column with `1 <= row_count <= 12` → [`WidgetKind::PieChart`] for share/proportion questions, else [`WidgetKind::BarChart`]
/// - All other shapes → [`WidgetKind::Table`]
#[must_use]
pub fn recommend_chart(
    question: &str,
    columns: &[QueryColumn],
    row_count: usize,
) -> ChartSuggestion {
    if row_count == 1
        && columns.len() == 1
        && let Some(only_col) = columns.first()
    {
        return ChartSuggestion {
            kind: WidgetKind::KpiCard,
            x_field: None,
            y_fields: vec![only_col.name.clone()],
            title: None,
        };
    }

    let mut time_cols = Vec::new();
    let mut num_cols = Vec::new();
    let mut cat_cols = Vec::new();

    for col in columns {
        match col.data_type {
            DataType::Date | DataType::Timestamp => time_cols.push(col.name.clone()),
            DataType::Int64 | DataType::Float64 | DataType::Decimal => {
                num_cols.push(col.name.clone());
            }
            DataType::String
            | DataType::Boolean
            | DataType::Binary
            | DataType::Json
            | DataType::List
            | DataType::Struct => {
                cat_cols.push(col.name.clone());
            }
        }
    }

    if time_cols.len() == 1
        && !num_cols.is_empty()
        && cat_cols.is_empty()
        && row_count >= 1
        && let Some(t_col) = time_cols.first()
    {
        return ChartSuggestion {
            kind: WidgetKind::LineChart,
            x_field: Some(t_col.clone()),
            y_fields: num_cols,
            title: None,
        };
    }

    if cat_cols.len() == 1
        && num_cols.len() == 1
        && time_cols.is_empty()
        && (1..=12).contains(&row_count)
        && let (Some(c_col), Some(n_col)) = (cat_cols.first(), num_cols.first())
    {
        let kind = if is_share_question(question) {
            WidgetKind::PieChart
        } else {
            WidgetKind::BarChart
        };
        return ChartSuggestion {
            kind,
            x_field: Some(c_col.clone()),
            y_fields: vec![n_col.clone()],
            title: None,
        };
    }

    ChartSuggestion {
        kind: WidgetKind::Table,
        x_field: None,
        y_fields: num_cols,
        title: None,
    }
}

/// Fallback deterministic summary sentence when `rows.len() > 20` or numeric grounding fails (`§04.6.6`).
#[must_use]
pub fn fallback_table_summary(row_count: usize) -> String {
    format!("查询返回 {row_count} 行，主要结果见表格")
}

/// Verifies that every numeric literal in `candidate_summary` appears in `rows` (allowing comma
/// thousands separators and percentage scaling), returning [`fallback_table_summary`] if any
/// ungrounded number is detected (`05-STEP04` §04.6.6).
#[must_use]
pub fn verify_or_fallback_summary(
    candidate_summary: &str,
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> String {
    let trimmed = candidate_summary.trim();
    if trimmed.is_empty() {
        return fallback_table_summary(rows.len());
    }

    let extracted = extract_number_tokens(trimmed);
    if extracted.is_empty() {
        return String::from(trimmed);
    }

    let cell_pool = collect_grounded_cell_values(rows);
    for token in &extracted {
        if !is_token_grounded(token, &cell_pool, rows.len()) {
            return fallback_table_summary(rows.len());
        }
    }
    String::from(trimmed)
}

/// Verifies `candidate_summary` against `rows` and returns `(verified_summary, verified_number_count)` (`02-STEP16` §16.4.2).
#[must_use]
pub fn verify_or_fallback_summary_with_count(
    candidate_summary: &str,
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> (String, u32) {
    let verified = verify_or_fallback_summary(candidate_summary, rows);
    let count = axpoint_domain::grounding::count_grounded_numbers(&verified);
    (verified, count)
}

/// Verifies that all standalone numeric tokens in a citation-sanitized `HYBRID` answer appear in
/// either `rows` or one of the cited document `snippets` (`02-STEP16` §16.4.2).
#[must_use]
pub fn verify_hybrid_numbers_grounded(
    sanitized_answer: &str,
    rows: &[serde_json::Map<String, serde_json::Value>],
    cited_snippets: &[String],
) -> bool {
    let stripped = axpoint_domain::grounding::strip_citation_and_table_markers(sanitized_answer);
    let extracted = extract_number_tokens(stripped.trim());
    if extracted.is_empty() {
        return true;
    }

    let mut cell_pool = collect_grounded_cell_values(rows);
    for snippet in cited_snippets {
        cell_pool.strings.push(snippet.clone());
        for tok in extract_number_tokens(snippet) {
            cell_pool.numbers.push(tok.value);
            if tok.is_percent {
                cell_pool.numbers.push(tok.value / 100.0);
            }
            cell_pool.strings.push(tok.raw_digits);
        }
    }

    extracted
        .iter()
        .all(|tok| is_token_grounded(tok, &cell_pool, rows.len()))
}

fn is_share_question(question: &str) -> bool {
    let lower = question.to_ascii_lowercase();
    SHARE_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

#[derive(Debug, Clone)]
struct ExtractedNum {
    value: f64,
    is_percent: bool,
    raw_digits: String,
}

fn extract_number_tokens(text: &str) -> Vec<ExtractedNum> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut idx = 0;
    let mut out = Vec::new();

    while idx < len {
        let Some(&ch) = chars.get(idx) else {
            break;
        };
        let starts_neg = ch == '-'
            && chars
                .get(idx.saturating_add(1))
                .is_some_and(char::is_ascii_digit);
        if !ch.is_ascii_digit() && !starts_neg {
            idx = idx.saturating_add(1);
            continue;
        }

        // Skip numbers embedded inside identifiers like `order_1`, `F-1`, or `[文1]`.
        let prev = idx.checked_sub(1).and_then(|p| chars.get(p)).copied();
        if prev.is_some_and(|p| p.is_ascii_alphabetic() || p == '_' || p == '文') {
            if ch == '-' {
                idx = idx.saturating_add(1);
            }
            while chars
                .get(idx)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                idx = idx.saturating_add(1);
            }
            continue;
        }

        let (token_str, next_idx) = scan_single_number(&chars, idx);
        idx = next_idx;
        let is_percent = chars.get(idx).copied() == Some('%');
        if is_percent {
            idx = idx.saturating_add(1);
        }

        let cleaned = token_str.replace(',', "");
        if let Ok(val) = cleaned.parse::<f64>()
            && val.is_finite()
        {
            out.push(ExtractedNum {
                value: val,
                is_percent,
                raw_digits: cleaned,
            });
        }
    }
    out
}

fn scan_single_number(chars: &[char], start: usize) -> (String, usize) {
    let mut buf = String::new();
    let mut i = start;
    if chars.get(i) == Some(&'-') {
        buf.push('-');
        i = i.saturating_add(1);
    }
    let mut seen_dot = false;
    while let Some(&c) = chars.get(i) {
        let is_grouping_comma = c == ','
            && !seen_dot
            && chars
                .get(i.saturating_add(1))
                .is_some_and(char::is_ascii_digit);
        if c.is_ascii_digit() || is_grouping_comma {
            buf.push(c);
            i = i.saturating_add(1);
        } else if c == '.'
            && !seen_dot
            && chars
                .get(i.saturating_add(1))
                .is_some_and(char::is_ascii_digit)
        {
            seen_dot = true;
            buf.push(c);
            i = i.saturating_add(1);
        } else {
            break;
        }
    }
    (buf, i)
}

#[derive(Debug, Default)]
struct GroundedCellPool {
    numbers: Vec<f64>,
    strings: Vec<String>,
}

fn collect_grounded_cell_values(
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> GroundedCellPool {
    let mut pool = GroundedCellPool::default();
    for row in rows {
        for val in row.values() {
            match val {
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64()
                        && f.is_finite()
                    {
                        pool.numbers.push(f);
                    }
                    pool.strings.push(n.to_string());
                }
                serde_json::Value::String(s) => {
                    let clean = s.trim().replace(',', "");
                    if let Some(pct_stripped) = clean.strip_suffix('%') {
                        if let Ok(f) = pct_stripped.trim().parse::<f64>()
                            && f.is_finite()
                        {
                            pool.numbers.push(f);
                            pool.numbers.push(f / 100.0);
                        }
                    } else if let Ok(f) = clean.parse::<f64>()
                        && f.is_finite()
                    {
                        pool.numbers.push(f);
                    }
                    pool.strings.push(s.clone());
                }
                _ => {}
            }
        }
    }
    pool
}

fn is_token_grounded(token: &ExtractedNum, pool: &GroundedCellPool, row_count: usize) -> bool {
    // Allow exact mention of returned row count when rows are non-empty (e.g. "共 3 行").
    if !token.is_percent
        && row_count > 0
        && u64::try_from(row_count)
            .ok()
            .and_then(|rc| f64::from(u32::try_from(rc).ok()?).into())
            .is_some_and(|rc_f: f64| (token.value - rc_f).abs() < 1e-9)
        && pool.strings.iter().any(|s| s == &token.raw_digits)
    {
        return true;
    }

    for &cell_num in &pool.numbers {
        if approx_matches(token.value, cell_num) {
            return true;
        }
        if token.is_percent && approx_matches(token.value, cell_num * 100.0) {
            return true;
        }
    }

    // Allow date/code substrings verbatim from string/timestamp cells (e.g. "2026" or "01" in "2026-01-01").
    if !token.is_percent && !token.raw_digits.contains('.') {
        for s in &pool.strings {
            if contains_standalone_digits(s, &token.raw_digits) {
                return true;
            }
        }
    }
    false
}

fn approx_matches(summary_val: f64, cell_val: f64) -> bool {
    let diff = (summary_val - cell_val).abs();
    if diff <= 1e-6 {
        return true;
    }
    // Allow standard 2-decimal rounding when cell has higher float precision (e.g. 33.33% for 0.333333...).
    let rounded_2 = (cell_val * 100.0).round() / 100.0;
    (summary_val - rounded_2).abs() <= 1e-6
}

fn contains_standalone_digits(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    for part in haystack.split(|c: char| !c.is_ascii_digit()) {
        if part == needle {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::verify_or_fallback_summary;

    #[test]
    fn verify_summary_skips_hyphenated_identifiers_without_looping() {
        let mut row = serde_json::Map::new();
        row.insert(String::from("delta_ratio"), serde_json::json!(-0.3));
        let rows = vec![row];

        let summary = "相对降幅最大的指标为 F-1 产能 output（-30%），关联订单 O-2。";
        let out = verify_or_fallback_summary(summary, &rows);
        assert_eq!(out, summary);
    }
}
