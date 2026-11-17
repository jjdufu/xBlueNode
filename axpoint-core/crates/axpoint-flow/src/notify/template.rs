//! Safe single-pass variable interpolation for operational alert titles, bodies, and HTML emails (`07-STEP07-08` §08.3.2 & §08.4).

use std::num::FpCategory;

use axpoint_domain::enums::Severity;
use axpoint_domain::rules::{AlertRecord, RuleDef};

/// Structured variable bindings available when rendering `title_template` and `body_template`.
///
/// Supported placeholders (`07-STEP07-08` §08.3.2):
/// - `{{rule.name}}`
/// - `{{node.title}}`
/// - `{{node.type_name}}`
/// - `{{node.key}}` (alias `{{node.node_key}}`)
/// - `{{metric}}`
/// - `{{value}}`
/// - `{{threshold}}`
/// - `{{severity}}`
/// - `{{dataset.name}}`
/// - `{{link}}`
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertTemplateVars {
    /// Owning rule display name (`{{rule.name}}`).
    pub rule_name: String,
    /// Matched business node title (`{{node.title}}`).
    pub node_title: Option<String>,
    /// Matched business node type name (`{{node.type_name}}`).
    pub node_type_name: Option<String>,
    /// Matched business node business key (`{{node.key}}`).
    pub node_key: Option<String>,
    /// Target metric identifier (`{{metric}}`).
    pub metric: Option<String>,
    /// Observed numeric value (`{{value}}`).
    pub value: Option<f64>,
    /// Configured numeric threshold (`{{threshold}}`).
    pub threshold: Option<f64>,
    /// Alert severity (`{{severity}}`).
    pub severity: Option<Severity>,
    /// Matched dataset display name (`{{dataset.name}}`).
    pub dataset_name: Option<String>,
    /// Frontend alert detail link (`{{link}}`).
    pub link: Option<String>,
}

impl AlertTemplateVars {
    /// Reconstructs [`AlertTemplateVars`] from a persisted [`AlertRecord`] and optional [`RuleDef`].
    #[must_use]
    pub fn from_alert_and_rule(alert: &AlertRecord, rule: Option<&RuleDef>) -> Self {
        let detail_str = |keys: &[&str]| -> Option<String> {
            for &k in keys {
                if let Some(s) = alert.detail.get(k).and_then(serde_json::Value::as_str)
                    && !s.is_empty()
                {
                    return Some(String::from(s));
                }
            }
            None
        };
        let threshold = alert
            .detail
            .get("threshold")
            .and_then(serde_json::Value::as_f64)
            .filter(|v| v.is_finite())
            .or_else(|| rule.and_then(|r| r.condition.threshold_number()));

        let rule_name = rule
            .map(|r| r.name.clone())
            .or_else(|| detail_str(&["rule_name"]))
            .unwrap_or_default();

        let link = detail_str(&["link"]).or_else(|| Some(format!("/alerts?alert_id={}", alert.id)));

        Self {
            rule_name,
            node_title: detail_str(&["node_title", "title"]),
            node_type_name: detail_str(&["type_name", "node_type_name"]),
            node_key: detail_str(&["node_key", "key"]),
            metric: detail_str(&["metric"]),
            value: alert.last_value,
            threshold,
            severity: Some(alert.severity),
            dataset_name: detail_str(&["dataset_name"]),
            link,
        }
    }

    /// Resolves a trimmed placeholder key (without surrounding `{{` / `}}`) to its formatted string.
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<String> {
        match key {
            "rule.name" => Some(self.rule_name.clone()),
            "node.title" => Some(self.node_title.clone().unwrap_or_default()),
            "node.type_name" => Some(self.node_type_name.clone().unwrap_or_default()),
            "node.key" | "node.node_key" => Some(self.node_key.clone().unwrap_or_default()),
            "metric" => Some(self.metric.clone().unwrap_or_default()),
            "value" => Some(self.value.map_or_else(String::new, format_template_number)),
            "threshold" => Some(
                self.threshold
                    .map_or_else(String::new, format_template_number),
            ),
            "severity" => Some(
                self.severity
                    .map_or_else(String::new, |s| String::from(s.as_str())),
            ),
            "dataset.name" => Some(self.dataset_name.clone().unwrap_or_default()),
            "link" => Some(self.link.clone().unwrap_or_default()),
            _ => None,
        }
    }
}

/// Formats a finite `f64` for alert template interpolation without scientific-notation noise on integers.
#[must_use]
pub fn format_template_number(val: f64) -> String {
    if !val.is_finite() {
        return String::new();
    }
    if matches!(val.classify(), FpCategory::Zero) {
        return String::from("0");
    }
    if matches!(val.fract().classify(), FpCategory::Zero) && val.abs() < 1e15 {
        return format!("{val:.0}");
    }
    format!("{val}")
}

/// Renders `template` in a single non-recursive pass using `vars`.
///
/// Unknown placeholders resolve to an empty string so raw template syntax is never leaked,
/// and substituted values containing `{{...}}` are never re-evaluated.
#[must_use]
pub fn render_alert_template(template: &str, vars: &AlertTemplateVars) -> String {
    let mut rest = template;
    let mut out = String::with_capacity(template.len());
    while let Some((before, after_open)) = rest.split_once("{{") {
        out.push_str(before);
        if let Some((raw_key, after_close)) = after_open.split_once("}}") {
            if let Some(rendered) = vars.lookup(raw_key.trim()) {
                out.push_str(&rendered);
            }
            rest = after_close;
        } else {
            out.push_str("{{");
            out.push_str(after_open);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Escapes `&`, `<`, `>`, `"`, and `'` for safe HTML email rendering.
#[must_use]
pub fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}
