//! Rule-first + LLM-fallback intent classifier for NL2SQL and `AxQuery` (`05-STEP04` §04.6.2, §04.8).

use axpoint_domain::AppError;
use axpoint_domain::analytics::AskIntent;
use axpoint_domain::ports::AiPort;
use serde::Deserialize;

const NUMERIC_KEYWORDS: &[&str] = &[
    "多少",
    "总和",
    "合计",
    "总金额",
    "总额",
    "总数",
    "数量",
    "几笔",
    "几个",
    "几条",
    "平均",
    "均值",
    "均价",
    "最大",
    "最高",
    "最小",
    "最低",
    "排名",
    "排行",
    "前十",
    "前五",
    "前三",
    "top",
    "同比",
    "环比",
    "占比",
    "比例",
    "趋势",
    "分布",
    "统计",
    "汇总",
    "明细",
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "median",
];

const DOC_KEYWORDS: &[&str] = &[
    "为什么",
    "依据",
    "制度",
    "规定",
    "文档",
    "原文",
    "条款",
    "规范",
    "政策",
    "办法",
    "手册",
    "说明文档",
];

const CHITCHAT_PHRASES: &[&str] = &[
    "你好",
    "您好",
    "早上好",
    "下午好",
    "晚上好",
    "你是谁",
    "谢谢",
    "再见",
    "讲个笑话",
    "今天天气",
    "hello",
    "hi",
];

const FOLLOWUP_PREFIXES: &[&str] = &[
    "那",
    "如果只看",
    "只看",
    "改成",
    "换成",
    "再按",
    "按",
    "其中",
    "排除",
    "去掉",
];

/// System prompt used when rule-based intent classification is inconclusive (`§04.6.2`).
pub const INTENT_SYSTEM_PROMPT: &str = "你是意图分类器。判断用户问题属于 DATA（结构化数据统计查询）、DOC（制度/文档知识检索）、HYBRID（同时需要数据统计与制度原因解释）还是 CHITCHAT（日常寒暄闲聊）。只输出一个 JSON 对象：{\"intent\": \"DATA|DOC|HYBRID|CHITCHAT\", \"confidence\": 0.0}";

#[derive(Debug, Deserialize)]
struct LlmIntentPayload {
    intent: AskIntent,
    #[serde(default)]
    confidence: f64,
}

/// Applies deterministic lexical rules from `05-STEP04` §04.6.2.
///
/// Returns `Some(intent)` when rules conclusively determine the intent, or `None` when LLM
/// fallback classification is required.
#[must_use]
pub fn classify_intent_by_rules(
    question: &str,
    has_schema_hit: bool,
    has_prior_data_turn: bool,
) -> Option<AskIntent> {
    let trimmed = question.trim();
    if trimmed.is_empty() {
        return Some(AskIntent::Chitchat);
    }

    let lower = trimmed.to_ascii_lowercase();
    let has_numeric = has_numeric_signal(&lower);
    let has_doc = DOC_KEYWORDS.iter().any(|kw| lower.contains(kw));

    if has_numeric && has_schema_hit && has_doc {
        return Some(AskIntent::Hybrid);
    }
    if has_numeric && has_schema_hit {
        return Some(AskIntent::Data);
    }
    if has_doc && !has_numeric {
        return Some(AskIntent::Doc);
    }
    if has_doc && has_numeric {
        return Some(AskIntent::Hybrid);
    }

    if !has_schema_hit && !has_numeric && !has_doc && is_pure_chitchat(&lower) {
        return Some(AskIntent::Chitchat);
    }

    if has_prior_data_turn
        && !has_doc
        && (has_schema_hit || FOLLOWUP_PREFIXES.iter().any(|p| trimmed.starts_with(p)))
    {
        return Some(AskIntent::Data);
    }

    None
}

/// Routes `question` to an [`AskIntent`] using deterministic rules first and `AiPort::chat` fallback
/// (`confidence < 0.6` defaults to [`AskIntent::Hybrid`], per `05-STEP04` §04.6.2).
///
/// # Errors
/// Propagates [`AppError`] only if unexpected fatal errors occur; upstream LLM classification
/// errors degrade safely to `DATA` (if `has_schema_hit`) or `HYBRID`.
pub async fn route_intent<A: AiPort>(
    ai: &A,
    question: &str,
    has_schema_hit: bool,
    has_prior_data_turn: bool,
) -> Result<AskIntent, AppError> {
    if let Some(rule_intent) =
        classify_intent_by_rules(question, has_schema_hit, has_prior_data_turn)
    {
        return Ok(rule_intent);
    }

    match ai.chat(INTENT_SYSTEM_PROMPT, question.trim()).await {
        Ok(raw) => {
            if let Some(parsed) = parse_intent_json(&raw) {
                if parsed.intent == AskIntent::Chitchat {
                    return Ok(AskIntent::Chitchat);
                }
                if parsed.confidence < 0.6 {
                    return Ok(AskIntent::Hybrid);
                }
                return Ok(parsed.intent);
            }
            Ok(if has_schema_hit {
                AskIntent::Data
            } else {
                AskIntent::Hybrid
            })
        }
        Err(_) => Ok(if has_schema_hit {
            AskIntent::Data
        } else {
            AskIntent::Hybrid
        }),
    }
}

fn has_numeric_signal(lower_q: &str) -> bool {
    if NUMERIC_KEYWORDS.iter().any(|kw| lower_q.contains(kw)) {
        return true;
    }
    if let Some(pos) = lower_q.find('按')
        && let Some(tail) = lower_q.get(pos..)
        && (tail.contains("统计")
            || tail.contains("分组")
            || tail.contains("汇总")
            || tail.contains("看"))
    {
        return true;
    }
    false
}

fn is_pure_chitchat(lower_q: &str) -> bool {
    let cleaned = lower_q.trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '!' | '！' | '?' | '？' | '。' | ',' | '，' | '~')
    });
    CHITCHAT_PHRASES.contains(&cleaned)
}

fn parse_intent_json(raw: &str) -> Option<LlmIntentPayload> {
    let trimmed = raw.trim();
    let json_slice = extract_json_object(trimmed).unwrap_or(trimmed);
    serde_json::from_str::<LlmIntentPayload>(json_slice).ok()
}

pub(crate) fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    raw.get(start..=end)
}
