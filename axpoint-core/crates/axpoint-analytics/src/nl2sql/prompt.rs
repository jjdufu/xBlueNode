//! System and user prompt templates for NL2SQL generation, self-repair, and summarization (`05-STEP04` §04.6.4–§04.6.6).

use std::fmt::Write as _;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{AskContext, AskTurnRecord, FewShotTurn, MetricDef, QueryColumn};
use serde::{Deserialize, Serialize};

use crate::nl2sql::intent::extract_json_object;
use crate::nl2sql::schema_link::{LinkedSchemaContext, LinkedTableContext};

/// Maximum historical turns from the same `session_id` included in the prompt (`§04.6.1`).
pub const MAX_SESSION_HISTORY_TURNS: usize = 6;

/// Maximum positive-feedback few-shot examples included in the prompt (`§04.6.4`).
pub const MAX_FEW_SHOT_TURNS: usize = 5;

/// System prompt for grounded 1–3 sentence Chinese table summarization (`§04.6.6`).
pub const SUMMARY_SYSTEM_PROMPT: &str = "你是严谨的数据分析助手。请根据用户问题和查询结果表，输出 1～3 句简洁中文结论。所有出现的数字必须逐字来自结果表单元格，严禁编造或推算未出现的数字。只输出纯文本摘要。";

/// System prompt for `CHITCHAT` conversational responses (`§04.6.2`).
pub const CHITCHAT_SYSTEM_PROMPT: &str = "你是 xBlueNode 智能分析助手。请简短礼貌地回应用户寒暄，并提示你可以协助查询分析业务数据或企业知识文档。";

/// Structured JSON payload returned by the NL2SQL generation/repair model (`§04.6.4`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmSqlDraft {
    /// Generated single `SELECT` statement, or `""` when user clarification is needed.
    #[serde(default)]
    pub sql: String,
    /// Explanation of the query logic or clarification question when `sql` is empty.
    #[serde(default)]
    pub explanation: String,
    /// Referenced semantic metric names.
    #[serde(default)]
    pub used_metrics: Vec<String>,
}

/// Builds the fixed NL2SQL system prompt with `now_rfc3339` and tenant `tz` (`05-STEP04` §04.6.4).
#[must_use]
pub fn build_nl2sql_system_prompt(now_rfc3339: &str, tz: &str) -> String {
    format!(
        "你是 DuckDB SQL 专家。只能使用下列表与列，只输出一个 JSON 对象：{{\"sql\": \"...\", \"explanation\": \"...\", \"used_metrics\": []}}\n\
         规则：只写单条 SELECT；表名直接使用给出的名称；指标必须使用给出的表达式；时间条件使用半开区间；\n\
         不确定时在 explanation 中说明需要澄清的问题，并令 sql 为空字符串。\n\
         当前时间：{now_rfc3339}（租户时区 {tz}）"
    )
}

/// Builds the deterministic NL2SQL user prompt containing candidate DDLs, masked samples,
/// semantic metrics, matched `BlueNode`s, few-shot turns, session history, and `question` (`§04.6.4`).
#[must_use]
pub fn build_nl2sql_user_prompt(
    question: &str,
    linked: &LinkedSchemaContext,
    few_shots: &[FewShotTurn],
    session_turns: &[AskTurnRecord],
    context: Option<&AskContext>,
) -> String {
    let mut out = String::from("### 候选数据表\n");
    if linked.tables.is_empty() {
        out.push_str("（无可用数据表）\n");
    } else {
        for tbl in &linked.tables {
            append_table_ddl_and_samples(&mut out, tbl);
        }
    }

    if !linked.metrics.is_empty() {
        out.push_str("\n### 候选认证/业务指标\n");
        for m in &linked.metrics {
            append_metric_prompt_line(&mut out, m);
        }
    }

    if !linked.matched_nodes.is_empty() {
        out.push_str("\n### 命中业务节点（过滤值候选）\n");
        for (type_name, node_key, title) in &linked.matched_nodes {
            let _ = writeln!(out, "- {type_name} key='{node_key}' ({title})");
        }
    }

    if !few_shots.is_empty() {
        out.push_str("\n### 参考示例\n");
        for fs in few_shots.iter().take(MAX_FEW_SHOT_TURNS) {
            let _ = writeln!(
                out,
                "问题: {}\nSQL: {}",
                fs.question.trim(),
                fs.sql_text.trim()
            );
        }
    }

    append_session_history(&mut out, session_turns);
    append_ask_context(&mut out, context);

    let _ = write!(out, "\n### 当前问题\n{}", question.trim());
    out
}

/// Builds the self-repair user prompt appending the failed SQL and guard/`EXPLAIN` error (`§04.6.5`).
#[must_use]
pub fn build_repair_user_prompt(
    original_user_prompt: &str,
    failed_sql: &str,
    error_first_line: &str,
    round: usize,
) -> String {
    format!(
        "{original_user_prompt}\n\n### 第 {round} 轮 SQL 修正请求\n\
         上一版 SQL:\n{failed_sql}\n\
         校验/试运行错误:\n{error_first_line}\n\
         请修正上述错误并重新只输出一个 JSON 对象：{{\"sql\": \"...\", \"explanation\": \"...\", \"used_metrics\": []}}"
    )
}

/// Builds the user prompt for `<= 20` row result summarization (`§04.6.6`).
#[must_use]
pub fn build_summary_user_prompt(
    question: &str,
    columns: &[QueryColumn],
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> String {
    let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    let rows_json = serde_json::to_string(rows).unwrap_or_else(|_| String::from("[]"));
    format!(
        "问题: {}\n结果列: {}\n结果数据 ({} 行): {}",
        question.trim(),
        col_names.join(", "),
        rows.len(),
        rows_json
    )
}

/// Parses the LLM JSON response (`{"sql": "...", "explanation": "...", "used_metrics": [...]}`),
/// stripping optional Markdown code fences (`§04.6.4`).
///
/// # Errors
/// Returns [`AppError::Llm`] if the model output is not valid JSON matching [`LlmSqlDraft`].
pub fn parse_llm_sql_response(raw: &str) -> Result<LlmSqlDraft, AppError> {
    let trimmed = raw.trim();
    let slice = extract_json_object(trimmed).ok_or_else(|| {
        AppError::Llm(String::from(
            "NL2SQL model response did not contain a JSON object",
        ))
    })?;
    let mut draft: LlmSqlDraft = serde_json::from_str(slice)
        .map_err(|err| AppError::Llm(format!("failed to parse NL2SQL JSON output: {err}")))?;
    draft.sql = strip_trailing_semicolons(draft.sql.trim());
    Ok(draft)
}

pub(crate) fn strip_trailing_semicolons(sql: &str) -> String {
    String::from(sql.trim_end_matches(|c: char| c == ';' || c.is_whitespace()))
}

fn append_table_ddl_and_samples(out: &mut String, tbl: &LinkedTableContext) {
    let _ = writeln!(out, "CREATE TABLE {} (", tbl.dataset_name);
    for (idx, col) in tbl.columns.iter().enumerate() {
        let comma = if idx.saturating_add(1) < tbl.columns.len() {
            ","
        } else {
            ""
        };
        let comment = build_col_comment(&col.description, &col.business_term, &col.unit);
        if comment.is_empty() {
            let _ = writeln!(out, "  {} {}{comma}", col.name, col.data_type);
        } else {
            let _ = writeln!(out, "  {} {}{comma} -- {comment}", col.name, col.data_type);
        }
    }
    out.push_str(");\n");

    if !tbl.sample_rows.is_empty() {
        let _ = writeln!(out, "-- {} 样例行（已脱敏）:", tbl.dataset_name);
        for row in tbl.sample_rows.iter().take(3) {
            if let Ok(line) = serde_json::to_string(row) {
                let _ = writeln!(out, "-- {line}");
            }
        }
    }
}

fn build_col_comment(desc: &str, term: &str, unit: &str) -> String {
    let mut parts = Vec::new();
    if !desc.trim().is_empty() {
        parts.push(desc.trim());
    }
    if !term.trim().is_empty() {
        parts.push(term.trim());
    }
    if !unit.trim().is_empty() {
        parts.push(unit.trim());
    }
    parts.join(" / ")
}

fn append_metric_prompt_line(out: &mut String, m: &MetricDef) {
    let dims: Vec<&str> = m.dimensions.iter().map(|d| d.column.as_str()).collect();
    let filter_note = if m.filter_sql.trim().is_empty() {
        String::from("无")
    } else {
        m.filter_sql.trim().to_owned()
    };
    let time_note = m.time_column.as_deref().unwrap_or("无");
    let _ = writeln!(
        out,
        "- {}（{}）: 表达式=({}) | 过滤=({}) | 时间列={} | 维度=[{}]",
        m.name,
        m.display_name,
        m.expression,
        filter_note,
        time_note,
        dims.join(", ")
    );
}

fn append_session_history(out: &mut String, session_turns: &[AskTurnRecord]) {
    if session_turns.is_empty() {
        return;
    }
    let start = session_turns
        .len()
        .saturating_sub(MAX_SESSION_HISTORY_TURNS);
    let recent = session_turns.get(start..).unwrap_or(session_turns);
    out.push_str("\n### 会话历史上下文\n");
    for (idx, t) in recent.iter().enumerate() {
        let turn_no = idx.saturating_add(1);
        let sql_s = t.sql_text.as_deref().unwrap_or("");
        let cols_s = extract_preview_col_names(t.result_preview.as_ref());
        let _ = writeln!(
            out,
            "轮次 {turn_no} 问题: {}\n轮次 {turn_no} SQL: {sql_s}\n轮次 {turn_no} 结果列: {cols_s}",
            t.question.trim()
        );
    }
}

fn extract_preview_col_names(preview: Option<&serde_json::Value>) -> String {
    let Some(cols_arr) = preview
        .and_then(|v| v.get("columns"))
        .and_then(serde_json::Value::as_array)
    else {
        return String::new();
    };
    let names: Vec<&str> = cols_arr
        .iter()
        .filter_map(|c| c.get("name").and_then(serde_json::Value::as_str))
        .collect();
    names.join(", ")
}

fn append_ask_context(out: &mut String, context: Option<&AskContext>) {
    let Some(ctx) = context else {
        return;
    };
    if ctx.filters.is_empty() && ctx.drill_path.is_empty() {
        return;
    }
    out.push_str("\n### 附加筛选与下钻约束\n");
    if !ctx.filters.is_empty()
        && let Ok(f_json) = serde_json::to_string(&ctx.filters)
    {
        let _ = writeln!(out, "必须满足筛选条件: {f_json}");
    }
    if !ctx.drill_path.is_empty() {
        let _ = writeln!(out, "当前下钻路径: {}", ctx.drill_path.join(" -> "));
    }
}
