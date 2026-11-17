//! Operational alerting rules, conditions, alert state machine DTOs, notification channels,
//! delivery records, and alert silences (`07-STEP07-08` §08).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::AppError;
use crate::axhub::{QualityOnFail, QualityReport};
use crate::enums::{AlertStatus, NotifyKind, Severity};

const fn default_true() -> bool {
    true
}

const fn default_cooldown_minutes() -> i32 {
    30
}

const fn default_resolve_after_minutes() -> i32 {
    10
}

const fn default_lookback_points() -> usize {
    96
}

const fn default_z_threshold() -> f64 {
    3.0
}

const fn default_min_points() -> usize {
    30
}

fn default_empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Three-state patch field distinguishing omitted (`Unchanged`), explicit `null` (`Clear`),
/// and an updated value (`Set(T)`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NullablePatch<T> {
    /// Field was omitted from the request payload; preserve existing persisted value.
    #[default]
    Unchanged,
    /// Field was explicitly set to JSON `null`; clear persisted value to `None`.
    Clear,
    /// Field was explicitly set to a concrete value.
    Set(T),
}

impl<T> NullablePatch<T> {
    /// Returns `true` when the patch leaves the field unchanged.
    #[must_use]
    pub const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    /// Applies this patch onto an existing `Option<T>`.
    #[must_use]
    pub fn apply_to(self, current: Option<T>) -> Option<T> {
        match self {
            Self::Unchanged => current,
            Self::Clear => None,
            Self::Set(val) => Some(val),
        }
    }
}

impl<T: Serialize> Serialize for NullablePatch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unchanged | Self::Clear => serializer.serialize_none(),
            Self::Set(val) => val.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NullablePatch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|opt| opt.map_or(Self::Clear, Self::Set))
    }
}

macro_rules! define_rule_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident => $str_val:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(
                $(#[$vmeta])*
                #[serde(rename = $str_val)]
                $variant,
            )+
        }

        impl $name {
            /// All variants of this enumeration in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Returns the canonical wire and database string representation.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $str_val,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = AppError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($str_val => Ok(Self::$variant),)+
                    other => Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("unknown {} value: {other}", stringify!($name)),
                    )),
                }
            }
        }
    };
}

define_rule_enum! {
    /// Target scope category of an operational rule (`07-STEP07-08` §08.1).
    pub enum RuleScopeKind {
        /// Business node time-series or snapshot metric rule.
        Metric => "METRIC",
        /// `AxHub` dataset synchronization quality assertion rule.
        DatasetQuality => "DATASET_QUALITY",
        /// Transactional `ax_outbox` event rule.
        Event => "EVENT",
    }
}

define_rule_enum! {
    /// Numeric comparison operator used by `THRESHOLD` and `CHANGE_RATE` conditions (`07-STEP07-08` §08.2).
    pub enum ComparisonOp {
        /// Strictly greater than (`>`).
        Gt => "GT",
        /// Greater than or equal (`>=`).
        Gte => "GTE",
        /// Strictly less than (`<`).
        Lt => "LT",
        /// Less than or equal (`<=`).
        Lte => "LTE",
        /// Equal within floating-point tolerance (`==`).
        Eq => "EQ",
        /// Not equal outside floating-point tolerance (`!=`).
        Ne => "NE",
    }
}

impl ComparisonOp {
    /// Evaluates `actual <op> expected` (`1e-9` tolerance for `EQ`/`NE`).
    #[must_use]
    pub fn compare(self, actual: f64, expected: f64) -> bool {
        if !actual.is_finite() || !expected.is_finite() {
            return false;
        }
        match self {
            Self::Gt => actual > expected,
            Self::Gte => actual >= expected,
            Self::Lt => actual < expected,
            Self::Lte => actual <= expected,
            Self::Eq => (actual - expected).abs() <= 1e-9,
            Self::Ne => (actual - expected).abs() > 1e-9,
        }
    }
}

define_rule_enum! {
    /// Reason recorded when an operational alert transitions to `RESOLVED` (`07-STEP07-08` §08.1).
    pub enum AlertResolveReason {
        /// Automatically resolved after condition remained false for `resolve_after_minutes`.
        Auto => "AUTO",
        /// Manually resolved by an operator via REST API.
        Manual => "MANUAL",
        /// Automatically resolved because the owning rule was disabled or deleted.
        RuleDisabled => "RULE_DISABLED",
    }
}

define_rule_enum! {
    /// Lifecycle timeline event kind recorded in `ax_alert_events` (`07-STEP07-08` §08.1).
    pub enum AlertEventKind {
        /// Alert newly opened.
        Opened => "OPENED",
        /// Active or cooling-down alert retriggered.
        Retriggered => "RETRIGGERED",
        /// Alert acknowledged by an operator.
        Acked => "ACKED",
        /// Unacknowledged alert escalated after `escalate_after_minutes`.
        Escalated => "ESCALATED",
        /// Alert transitioned to `RESOLVED`.
        Resolved => "RESOLVED",
        /// Notification successfully delivered to a channel.
        Notified => "NOTIFIED",
        /// Notification exhausted all retry attempts and failed.
        NotifyFailed => "NOTIFY_FAILED",
        /// Notification suppressed by an active [`AlertSilenceRecord`].
        Silenced => "SILENCED",
        /// Linked `AxFlow` `PROD` deployment started (`trigger_source = RULE`).
        FlowStarted => "FLOW_STARTED",
    }
}

define_rule_enum! {
    /// Event category recorded in `ax_notify_deliveries` (`07-STEP07-08` §08.1).
    pub enum NotifyDeliveryEvent {
        /// Initial alert open notification.
        Opened => "OPENED",
        /// Escalation notification sent to `escalation_channel_ids`.
        Escalated => "ESCALATED",
        /// Alert resolution notification.
        Resolved => "RESOLVED",
        /// Synchronous channel connectivity test (`POST /api/axflow/channels/{id}/test`).
        Test => "TEST",
        /// Notification emitted by an `AxFlow` `ACTION_EMIT(NOTIFY)` node.
        Action => "ACTION",
    }
}

define_rule_enum! {
    /// Delivery status recorded in `ax_notify_deliveries` (`07-STEP07-08` §08.1).
    pub enum NotifyDeliveryStatus {
        /// Queued or awaiting backoff retry.
        Pending => "PENDING",
        /// Successfully accepted by remote channel.
        Sent => "SENT",
        /// Permanently failed after retries.
        Failed => "FAILED",
    }
}

/// Rule evaluation condition specification (`07-STEP07-08` §08.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Condition {
    /// Latest value (`for_minutes == 0`) or all raw points in `[now - for_minutes, now]` satisfy `op(v, value)`.
    #[serde(rename = "THRESHOLD")]
    Threshold {
        /// Comparison operator.
        op: ComparisonOp,
        /// Threshold numeric value.
        value: f64,
        /// Required continuous duration in minutes (`0` checks latest value only).
        #[serde(default)]
        for_minutes: u32,
    },
    /// Relative change `(latest - window_start) / |window_start|` over `window_minutes` satisfies `op(rate, pct)`.
    #[serde(rename = "CHANGE_RATE")]
    ChangeRate {
        /// Lookback window in minutes (`>= 1`).
        window_minutes: u32,
        /// Comparison operator.
        op: ComparisonOp,
        /// Target fractional change (e.g. `0.2` for +20%).
        pct: f64,
    },
    /// No raw points exist for the target metric in `[now - minutes, now]`.
    #[serde(rename = "ABSENCE")]
    Absence {
        /// Absence window in minutes (`>= 1`).
        minutes: u32,
    },
    /// Statistical Z-score anomaly `|x - mean| / std_dev >= z` over preceding `lookback_points` (`>= min_points`).
    #[serde(rename = "ANOMALY_ZSCORE")]
    AnomalyZscore {
        /// Number of historical points before the latest point (`default = 96`).
        #[serde(default = "default_lookback_points")]
        lookback_points: usize,
        /// Z-score threshold (`default = 3.0`).
        #[serde(default = "default_z_threshold")]
        z: f64,
        /// Minimum historical points required before evaluating (`default = 30`).
        #[serde(default = "default_min_points")]
        min_points: usize,
    },
    /// Boolean Step 06 expression over node metrics and `prop.*` attributes.
    #[serde(rename = "EXPR")]
    Expr {
        /// Expression source string.
        expression: String,
    },
    /// Dataset sync quality check failure at or above `min_severity` (`DATASET_QUALITY` scope).
    #[serde(rename = "QUALITY_FAILED")]
    QualityFailed {
        /// Minimum failing quality rule severity (`WARN` or `BLOCK`).
        min_severity: QualityOnFail,
    },
    /// Outbox event topic and optional filter match (`EVENT` scope).
    #[serde(rename = "EVENT_MATCH")]
    EventMatch {},
}

impl Condition {
    /// Returns the primary numeric threshold parameter (`value`, `pct`, or `z`) for `{{threshold}}` template rendering.
    #[must_use]
    pub const fn threshold_number(&self) -> Option<f64> {
        match self {
            Self::Threshold { value, .. } => Some(*value),
            Self::ChangeRate { pct, .. } => Some(*pct),
            Self::AnomalyZscore { z, .. } => Some(*z),
            Self::Absence { .. }
            | Self::Expr { .. }
            | Self::QualityFailed { .. }
            | Self::EventMatch {} => None,
        }
    }
}

/// Typed `METRIC` scope target (`{type_name, metric, node_filter?}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricRuleTarget {
    /// Target `BlueNode` `type_name` (e.g. `"Warehouse"`).
    pub type_name: String,
    /// Target metric identifier (e.g. `"inventory"`).
    pub metric: String,
    /// Optional Step 06 boolean expression filtering candidate nodes (e.g. `prop.region == '华东'`).
    #[serde(default)]
    pub node_filter: Option<String>,
}

/// Typed `DATASET_QUALITY` scope target (`{dataset_id?}`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetQualityRuleTarget {
    /// Optional specific dataset UUID; `None` matches all datasets in the tenant.
    #[serde(default)]
    pub dataset_id: Option<Uuid>,
}

/// Typed `EVENT` scope target (`{topic, filter?}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRuleTarget {
    /// Target `OutboxTopic` wire name (e.g. `"dataset.synced"`).
    pub topic: String,
    /// Optional Step 06 boolean expression over top-level event payload fields.
    #[serde(default)]
    pub filter: Option<String>,
}

/// Validated `ax_rules.target` parsed according to [`RuleScopeKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedRuleTarget {
    /// Metric rule target.
    Metric(MetricRuleTarget),
    /// Dataset quality rule target.
    DatasetQuality(DatasetQualityRuleTarget),
    /// Outbox event rule target.
    Event(EventRuleTarget),
}

impl ParsedRuleTarget {
    /// Parses and validates `raw` against `scope_kind`.
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) if `raw` does not conform to `scope_kind`.
    pub fn parse(scope_kind: RuleScopeKind, raw: &serde_json::Value) -> Result<Self, AppError> {
        match scope_kind {
            RuleScopeKind::Metric => {
                let target: MetricRuleTarget =
                    serde_json::from_value(raw.clone()).map_err(|err| {
                        AppError::bad("INVALID_ARGUMENT", format!("invalid METRIC target: {err}"))
                    })?;
                if target.type_name.trim().is_empty() || target.metric.trim().is_empty() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "METRIC target requires non-empty `type_name` and `metric`",
                    ));
                }
                Ok(Self::Metric(target))
            }
            RuleScopeKind::DatasetQuality => {
                let target: DatasetQualityRuleTarget = serde_json::from_value(raw.clone())
                    .map_err(|err| {
                        AppError::bad(
                            "INVALID_ARGUMENT",
                            format!("invalid DATASET_QUALITY target: {err}"),
                        )
                    })?;
                Ok(Self::DatasetQuality(target))
            }
            RuleScopeKind::Event => {
                let target: EventRuleTarget =
                    serde_json::from_value(raw.clone()).map_err(|err| {
                        AppError::bad("INVALID_ARGUMENT", format!("invalid EVENT target: {err}"))
                    })?;
                if target.topic.trim().is_empty() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "EVENT target requires non-empty `topic`",
                    ));
                }
                Ok(Self::Event(target))
            }
        }
    }
}

/// Persisted alerting rule definition (`ax_rules`, `07-STEP07-08` §08.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDef {
    /// Rule UUID.
    pub id: Uuid,
    /// Tenant-unique rule name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Rule scope (`METRIC`, `DATASET_QUALITY`, `EVENT`).
    pub scope_kind: RuleScopeKind,
    /// Scope-specific target configuration JSON.
    pub target: serde_json::Value,
    /// Evaluation condition.
    pub condition: Condition,
    /// Alert severity emitted when triggered.
    pub severity: Severity,
    /// Alert title template (`{{rule.name}}`, `{{node.title}}`, `{{value}}`, etc.).
    pub title_template: String,
    /// Alert body template.
    pub body_template: String,
    /// Cooldown window after resolution in minutes (`>= 0`).
    pub cooldown_minutes: i32,
    /// Continuous false duration required before auto-resolving in minutes (`>= 0`).
    pub resolve_after_minutes: i32,
    /// Optional minutes before an unacknowledged `OPEN` alert escalates (`> 0`).
    pub escalate_after_minutes: Option<i32>,
    /// Notification channel IDs notified on `OPENED` and `RESOLVED`.
    pub channel_ids: Vec<Uuid>,
    /// Escalation channel IDs notified on `ESCALATED`.
    pub escalation_channel_ids: Vec<Uuid>,
    /// Optional `AxFlow` UUID started (`PROD`, `trigger_source = RULE`) when an alert opens.
    pub run_flow_id: Option<Uuid>,
    /// Whether this rule is enabled.
    pub enabled: bool,
    /// Monotonically increasing rule revision number.
    pub version: i32,
    /// Creator user UUID, if created interactively.
    pub created_by: Option<Uuid>,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp (UTC).
    pub updated_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axflow/rules` (`07-STEP07-08` §08.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRuleRequest {
    /// Tenant-unique rule name.
    pub name: String,
    /// Optional rule description.
    #[serde(default)]
    pub description: String,
    /// Rule scope kind.
    pub scope_kind: RuleScopeKind,
    /// Scope target configuration JSON.
    pub target: serde_json::Value,
    /// Trigger condition.
    pub condition: Condition,
    /// Alert severity.
    pub severity: Severity,
    /// Alert title template.
    pub title_template: String,
    /// Optional alert body template.
    #[serde(default)]
    pub body_template: String,
    /// Cooldown duration in minutes (`default = 30`).
    #[serde(default = "default_cooldown_minutes")]
    pub cooldown_minutes: i32,
    /// Auto-resolve duration in minutes (`default = 10`).
    #[serde(default = "default_resolve_after_minutes")]
    pub resolve_after_minutes: i32,
    /// Optional escalation duration in minutes (`> 0`).
    #[serde(default)]
    pub escalate_after_minutes: Option<i32>,
    /// Notification channel UUIDs.
    #[serde(default)]
    pub channel_ids: Vec<Uuid>,
    /// Escalation notification channel UUIDs.
    #[serde(default)]
    pub escalation_channel_ids: Vec<Uuid>,
    /// Optional linked `AxFlow` UUID.
    #[serde(default)]
    pub run_flow_id: Option<Uuid>,
    /// Initial enabled flag (`default = true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Request payload for `PUT /api/axflow/rules/{id}` (`07-STEP07-08` §08.6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRuleRequest {
    /// Updated rule name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated scope kind.
    #[serde(default)]
    pub scope_kind: Option<RuleScopeKind>,
    /// Updated scope target JSON.
    #[serde(default)]
    pub target: Option<serde_json::Value>,
    /// Updated condition.
    #[serde(default)]
    pub condition: Option<Condition>,
    /// Updated severity.
    #[serde(default)]
    pub severity: Option<Severity>,
    /// Updated title template.
    #[serde(default)]
    pub title_template: Option<String>,
    /// Updated body template.
    #[serde(default)]
    pub body_template: Option<String>,
    /// Updated cooldown minutes.
    #[serde(default)]
    pub cooldown_minutes: Option<i32>,
    /// Updated auto-resolve minutes.
    #[serde(default)]
    pub resolve_after_minutes: Option<i32>,
    /// Updated escalation minutes (`null` clears escalation).
    #[serde(default, skip_serializing_if = "NullablePatch::is_unchanged")]
    pub escalate_after_minutes: NullablePatch<i32>,
    /// Updated primary channel IDs.
    #[serde(default)]
    pub channel_ids: Option<Vec<Uuid>>,
    /// Updated escalation channel IDs.
    #[serde(default)]
    pub escalation_channel_ids: Option<Vec<Uuid>>,
    /// Updated linked `AxFlow` UUID (`null` clears flow linkage).
    #[serde(default, skip_serializing_if = "NullablePatch::is_unchanged")]
    pub run_flow_id: NullablePatch<Uuid>,
    /// Updated enabled flag (setting `false` resolves active alerts with `RULE_DISABLED`).
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Query parameters for `GET /api/axflow/rules`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleListQuery {
    /// Filter by scope kind.
    pub scope_kind: Option<RuleScopeKind>,
    /// Filter by enabled flag.
    pub enabled: Option<bool>,
    /// Case-insensitive substring search on `name` or `description`.
    pub q: Option<String>,
    /// Page size (`1..=500`, default `50`).
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    pub cursor: Option<String>,
}

/// Single preview alert candidate returned by `POST /api/axflow/rules/{id}/dry-run` (`07-STEP07-08` §08.5–§08.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDryRunHit {
    /// Deduplication key `<rule_id>:<object_id>`.
    pub dedup_key: String,
    /// Alert severity.
    pub severity: Severity,
    /// Rendered alert title.
    pub title: String,
    /// Rendered alert body.
    pub body: String,
    /// Matched business node ID, if `METRIC` scope.
    pub node_id: Option<Uuid>,
    /// Matched dataset ID, if `DATASET_QUALITY` scope.
    pub dataset_id: Option<Uuid>,
    /// Observed numeric value, if applicable.
    pub value: Option<f64>,
    /// Evaluation diagnostic detail.
    pub detail: serde_json::Value,
}

/// Response body for `POST /api/axflow/rules/{id}/dry-run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDryRunResponse {
    /// Evaluated rule ID.
    pub rule_id: Uuid,
    /// Total candidate objects inspected.
    pub evaluated_objects: usize,
    /// Number of objects where the condition evaluated to `true`.
    pub matched_count: usize,
    /// Non-persisted alerts that would be emitted right now.
    pub would_trigger: Vec<RuleDryRunHit>,
}

/// Persisted operational alert record (`ax_alerts`, `07-STEP07-08` §08.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRecord {
    /// Alert UUID.
    pub id: Uuid,
    /// Owning rule UUID.
    pub rule_id: Uuid,
    /// Deduplication key `<rule_id>:<object_id>`.
    pub dedup_key: String,
    /// Alert severity.
    pub severity: Severity,
    /// Lifecycle status (`OPEN`, `ACK`, `RESOLVED`).
    pub status: AlertStatus,
    /// Rendered alert title.
    pub title: String,
    /// Rendered alert body.
    pub body: String,
    /// Associated business node UUID, if any.
    pub node_id: Option<Uuid>,
    /// Associated dataset UUID, if any.
    pub dataset_id: Option<Uuid>,
    /// Latest observed numeric metric value, if any.
    pub last_value: Option<f64>,
    /// Structured alert context (node metadata, metric name, linked `flow_run_id`, etc.).
    pub detail: serde_json::Value,
    /// Number of times this alert has triggered while active.
    pub occurrences: i32,
    /// First trigger timestamp.
    pub first_seen_at: DateTime<Utc>,
    /// Most recent trigger timestamp.
    pub last_seen_at: DateTime<Utc>,
    /// Timestamp when the condition first became continuously false (`None` while true).
    pub last_false_at: Option<DateTime<Utc>>,
    /// Operator user UUID who acknowledged the alert.
    pub acked_by: Option<Uuid>,
    /// Acknowledgment timestamp.
    pub acked_at: Option<DateTime<Utc>>,
    /// Escalation timestamp, if escalated.
    pub escalated_at: Option<DateTime<Utc>>,
    /// Resolution timestamp, if `RESOLVED`.
    pub resolved_at: Option<DateTime<Utc>>,
    /// Resolution reason (`AUTO`, `MANUAL`, `RULE_DISABLED`).
    pub resolve_reason: Option<AlertResolveReason>,
    /// Associated notification delivery records (populated on `GET /api/axflow/alerts/{id}`).
    #[serde(default)]
    pub deliveries: Vec<NotifyDeliveryRecord>,
}

/// Query parameters for `GET /api/axflow/alerts` (`07-STEP07-08` §08.5–§08.6).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertListQuery {
    /// Filter by lifecycle status (`OPEN`, `ACK`, `RESOLVED`).
    pub status: Option<AlertStatus>,
    /// Filter by severity (`INFO`, `WARN`, `CRITICAL`).
    pub severity: Option<Severity>,
    /// Filter by owning rule UUID.
    pub rule_id: Option<Uuid>,
    /// Filter by business node UUID.
    pub node_id: Option<Uuid>,
    /// Filter by business node `type_name`.
    pub type_name: Option<String>,
    /// Page size (`1..=500`, default `50`).
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    pub cursor: Option<String>,
}

/// Query parameters for `GET /api/axflow/alerts/{id}/events`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertEventsQuery {
    /// Page size (`1..=500`, default `100`).
    pub limit: Option<u32>,
    /// Opaque pagination cursor (`id` string).
    pub cursor: Option<String>,
}

/// Optional JSON body for `POST /api/axflow/alerts/{id}/ack` and `POST /api/axflow/alerts/{id}/resolve`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertActionCommentRequest {
    /// Optional operator note stored in `ax_alert_events.detail`.
    #[serde(default)]
    pub comment: String,
}

/// Request payload for `POST /api/axflow/alerts/ack-batch` (`07-STEP07-08` §08.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckBatchRequest {
    /// Alert UUIDs to acknowledge (`1..=500`).
    pub alert_ids: Vec<Uuid>,
    /// Optional operator note stored in `ax_alert_events.detail`.
    #[serde(default)]
    pub comment: String,
}

/// Response payload for `POST /api/axflow/alerts/ack-batch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckBatchResponse {
    /// Number of alerts transitioned to `ACK`.
    pub acked_count: usize,
    /// Updated alert records.
    pub items: Vec<AlertRecord>,
}

/// Timeline event record (`ax_alert_events`, `07-STEP07-08` §08.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEventRecord {
    /// Monotonic `BIGSERIAL` event ID.
    pub id: i64,
    /// Owning alert UUID.
    pub alert_id: Uuid,
    /// Event kind.
    pub event: AlertEventKind,
    /// Structured event detail.
    pub detail: serde_json::Value,
    /// Optional user UUID who initiated the action.
    pub actor: Option<Uuid>,
    /// Event creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Persisted notification channel (`ax_notify_channels`, `07-STEP07-08` §08.1 & §08.4).
///
/// Encrypted secrets stored in `ds_credentials` are never serialized back to callers; only
/// `secret_fields_set` key names are exposed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyChannelRecord {
    /// Channel UUID.
    pub id: Uuid,
    /// Tenant-unique channel name.
    pub name: String,
    /// Channel kind (`WEBHOOK`, `EMAIL`, `WECOM`, `DINGTALK`, `FEISHU`).
    pub kind: NotifyKind,
    /// Non-secret channel configuration JSON.
    pub config: serde_json::Value,
    /// Optional reference to sealed `ds_credentials.id`.
    #[serde(skip_serializing)]
    pub credential_id: Option<Uuid>,
    /// Sorted list of configured secret field names (e.g. `["signing_secret"]`).
    #[serde(default)]
    pub secret_fields_set: Vec<String>,
    /// Whether RFC1918 / loopback targets are permitted (requires `GUARD_ADMIN` to enable).
    pub allow_private_network: bool,
    /// Whether deliveries through this channel are enabled.
    pub enabled: bool,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axflow/channels` (`07-STEP07-08` §08.4 & §08.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateNotifyChannelRequest {
    /// Tenant-unique channel name.
    pub name: String,
    /// Channel kind.
    pub kind: NotifyKind,
    /// Non-secret configuration object.
    #[serde(default = "default_empty_object")]
    pub config: serde_json::Value,
    /// Plaintext secret key-values to seal into `ds_credentials` (e.g. `signing_secret`, `webhook_key`, `smtp_password`).
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// Optional pre-existing `ds_credentials` UUID (if `secrets` is empty).
    #[serde(default)]
    pub credential_id: Option<Uuid>,
    /// Allow private/loopback target addresses (`GUARD_ADMIN` required when `true`).
    #[serde(default)]
    pub allow_private_network: bool,
    /// Whether the channel starts enabled (`default = true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Request payload for `PUT /api/axflow/channels/{id}` (`07-STEP07-08` §08.6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateNotifyChannelRequest {
    /// Updated channel name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated channel kind.
    #[serde(default)]
    pub kind: Option<NotifyKind>,
    /// Updated non-secret configuration object.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    /// Updated secret key-values (`Some(empty)` clears stored credential; `Some(non_empty)` replaces/merges).
    #[serde(default)]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Updated private-network flag (`GUARD_ADMIN` required when setting `true`).
    #[serde(default)]
    pub allow_private_network: Option<bool>,
    /// Updated enabled flag.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Pagination query parameters for `GET /api/axflow/channels` and `GET /api/axflow/silences`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimplePageQuery {
    /// Page size (`1..=500`, default `50`).
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    pub cursor: Option<String>,
}

/// Response body for `POST /api/axflow/channels/{id}/test` (`07-STEP07-08` §08.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestChannelResponse {
    /// Whether the synchronous test delivery succeeded.
    pub ok: bool,
    /// Recorded `TEST` delivery row.
    pub delivery: NotifyDeliveryRecord,
}

/// Persisted notification delivery attempt row (`ax_notify_deliveries`, `07-STEP07-08` §08.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyDeliveryRecord {
    /// Delivery UUID.
    pub id: Uuid,
    /// Associated alert UUID (`None` for `TEST` or `ACTION` deliveries).
    pub alert_id: Option<Uuid>,
    /// Target channel UUID.
    pub channel_id: Uuid,
    /// Delivery trigger event (`OPENED`, `ESCALATED`, `RESOLVED`, `TEST`, `ACTION`).
    pub event: NotifyDeliveryEvent,
    /// Current status (`PENDING`, `SENT`, `FAILED`).
    pub status: NotifyDeliveryStatus,
    /// Completed delivery attempts count (`0..=5`).
    pub attempt: i32,
    /// Remote HTTP or SMTP status code, if received.
    pub response_code: Option<i32>,
    /// Last error diagnostic message, if failed.
    pub error_message: Option<String>,
    /// Next scheduled retry timestamp for `PENDING` rows.
    pub next_retry_at: DateTime<Utc>,
    /// Row creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Timestamp when delivery succeeded (`SENT`).
    pub sent_at: Option<DateTime<Utc>>,
}

/// Matcher criteria on an [`AlertSilenceRecord`] (`07-STEP07-08` §08.1 & §08.4).
///
/// Each non-empty vector acts as an `IN (...)` filter; all non-empty filters are combined with logical `AND`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SilenceMatcher {
    /// Restrict to specific rule UUIDs.
    #[serde(default)]
    pub rule_ids: Vec<Uuid>,
    /// Restrict to specific alert severities.
    #[serde(default)]
    pub severities: Vec<Severity>,
    /// Restrict to specific business node UUIDs.
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Restrict to specific business node `type_name`s.
    #[serde(default)]
    pub type_names: Vec<String>,
}

impl SilenceMatcher {
    /// Returns `true` if the given alert context satisfies every non-empty filter on this matcher.
    #[must_use]
    pub fn matches(
        &self,
        rule_id: Uuid,
        severity: Severity,
        node_id: Option<Uuid>,
        node_type_name: Option<&str>,
    ) -> bool {
        if !self.rule_ids.is_empty() && !self.rule_ids.contains(&rule_id) {
            return false;
        }
        if !self.severities.is_empty() && !self.severities.contains(&severity) {
            return false;
        }
        if !self.node_ids.is_empty() && !node_id.is_some_and(|nid| self.node_ids.contains(&nid)) {
            return false;
        }
        if !self.type_names.is_empty()
            && !node_type_name.is_some_and(|tn| self.type_names.iter().any(|cand| cand == tn))
        {
            return false;
        }
        true
    }
}

/// Persisted alert silence window (`ax_alert_silences`, `07-STEP07-08` §08.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertSilenceRecord {
    /// Silence UUID.
    pub id: Uuid,
    /// Alert matching filter.
    pub matcher: SilenceMatcher,
    /// Silence window start (inclusive).
    pub starts_at: DateTime<Utc>,
    /// Silence window end (exclusive; `ends_at > starts_at`).
    pub ends_at: DateTime<Utc>,
    /// Operator reason for silencing notifications.
    pub reason: String,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axflow/silences` (`07-STEP07-08` §08.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSilenceRequest {
    /// Alert matching criteria.
    pub matcher: SilenceMatcher,
    /// Silence window start timestamp.
    pub starts_at: DateTime<Utc>,
    /// Silence window end timestamp (`> starts_at`).
    pub ends_at: DateTime<Utc>,
    /// Operator reason.
    pub reason: String,
}

/// Background `ax_jobs` payload for `JobKind::RuleEval` (`07-STEP07-08` §08.3.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleEvalJobPayload {
    /// Target rule UUID to evaluate.
    pub rule_id: Uuid,
    /// Specific business node IDs to evaluate (empty evaluates all matching nodes for `METRIC` / `ABSENCE`).
    #[serde(default)]
    pub node_ids: Vec<Uuid>,
    /// Dataset UUID when triggered by `dataset.synced`.
    #[serde(default)]
    pub dataset_id: Option<Uuid>,
    /// Inline quality report from `dataset.synced`, if present.
    #[serde(default)]
    pub quality: Option<QualityReport>,
    /// Outbox event topic when triggered by an `EVENT` rule.
    #[serde(default)]
    pub event_topic: Option<String>,
    /// Outbox event payload when triggered by an `EVENT` rule.
    #[serde(default)]
    pub event_payload: Option<serde_json::Value>,
    /// Stable event deduplication key (e.g. outbox `id` string) for `EVENT` rules.
    #[serde(default)]
    pub event_key: Option<String>,
    /// Whether this invocation comes from the 60s `ax_absence_rules()` lifecycle/escalation scanner.
    #[serde(default)]
    pub lifecycle_scan: bool,
}

/// Cross-tenant item returned by `ax_absence_rules()` (`07-STEP07-08` §08.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbsenceRuleItem {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Rule UUID requiring absence, auto-resolve, or escalation evaluation.
    pub rule_id: Uuid,
}

/// Cross-tenant due delivery item returned by `ax_due_notify_deliveries()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DueDeliveryItem {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Pending `ax_notify_deliveries.id` ready for attempt execution.
    pub delivery_id: Uuid,
}

/// Repository input for opening or retriggering an alert when a rule condition evaluates to `true` (`07-STEP07-08` §08.3.2).
#[derive(Debug, Clone)]
pub struct TrueConditionInput {
    /// Owning rule UUID.
    pub rule_id: Uuid,
    /// Deduplication key `<rule_id>:<object_id>`.
    pub dedup_key: String,
    /// Alert severity.
    pub severity: Severity,
    /// Rendered title.
    pub title: String,
    /// Rendered body.
    pub body: String,
    /// Optional business node UUID.
    pub node_id: Option<Uuid>,
    /// Optional dataset UUID.
    pub dataset_id: Option<Uuid>,
    /// Optional observed numeric value.
    pub last_value: Option<f64>,
    /// Structured alert detail JSON.
    pub detail: serde_json::Value,
    /// Rule `cooldown_minutes` (`>= 0`).
    pub cooldown_minutes: i32,
    /// Evaluation timestamp.
    pub now: DateTime<Utc>,
}

/// Outcome of [`crate::ports::RulesRepository::record_true_condition`].
#[derive(Debug, Clone)]
pub enum TrueConditionOutcome {
    /// Newly inserted `OPEN` alert (with `OPENED` event and `alert.opened` outbox event emitted).
    Opened(AlertRecord),
    /// Existing active (`OPEN`/`ACK`) alert updated (`occurrences + 1`, `last_false_at = None`, 60s-merged `RETRIGGERED` event).
    RetriggeredActive(AlertRecord),
    /// Suppressed by `cooldown_minutes` on a recently `RESOLVED` alert (`RETRIGGERED` event recorded on resolved alert).
    SuppressedByCooldown(AlertRecord),
}

/// Outcome of [`crate::ports::RulesRepository::record_false_condition`].
#[derive(Debug, Clone)]
pub enum FalseConditionOutcome {
    /// No active alert existed for `dedup_key`.
    NoActiveAlert,
    /// Active alert marked with `last_false_at` awaiting `resolve_after_minutes`.
    PendingAutoResolve(AlertRecord),
    /// Active alert transitioned to `RESOLVED` (`resolve_reason = AUTO`, `RESOLVED` event & `alert.resolved` outbox emitted).
    AutoResolved(AlertRecord),
}

/// Persistence input for creating or updating a notification channel alongside optional sealed credentials.
#[derive(Debug, Clone)]
pub struct SaveNotifyChannelInput {
    /// Tenant-unique channel name.
    pub name: String,
    /// Channel kind.
    pub kind: NotifyKind,
    /// Non-secret JSON configuration.
    pub config: serde_json::Value,
    /// Pre-existing credential ID when keeping or referencing an existing row.
    pub credential_id: Option<Uuid>,
    /// Newly sealed credential row `(key_version, nonce, ciphertext)` to insert/update in `ds_credentials`, if provided.
    pub new_sealed_secret: Option<(i32, Vec<u8>, Vec<u8>)>,
    /// Whether to clear `credential_id` (`true` removes secret binding).
    pub clear_credential: bool,
    /// Allow private/loopback addresses.
    pub allow_private_network: bool,
    /// Enabled flag.
    pub enabled: bool,
}

/// Persistence input for enqueueing a notification delivery row (`ax_notify_deliveries`).
#[derive(Debug, Clone)]
pub struct EnqueueDeliveryInput {
    /// Optional alert UUID (`None` for `TEST` or `ACTION`).
    pub alert_id: Option<Uuid>,
    /// Target channel UUID.
    pub channel_id: Uuid,
    /// Delivery event kind.
    pub event: NotifyDeliveryEvent,
    /// Initial `next_retry_at` timestamp (`Utc::now()`).
    pub now: DateTime<Utc>,
}

/// Persistence input for recording a failed delivery attempt (either scheduling a retry or marking terminal `FAILED`).
#[derive(Debug, Clone)]
pub struct DeliveryFailureUpdate {
    /// Completed attempt number (`1..=5`).
    pub attempt: i32,
    /// Optional HTTP or SMTP status code received.
    pub response_code: Option<i32>,
    /// Diagnostic error message.
    pub error_message: String,
    /// `Some(ts)` keeps status `PENDING` with `next_retry_at = ts`; `None` marks terminal `FAILED`.
    pub next_retry_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_op_evaluates_all_variants() {
        assert!(ComparisonOp::Gt.compare(10.1, 10.0));
        assert!(!ComparisonOp::Gt.compare(10.0, 10.0));
        assert!(ComparisonOp::Gte.compare(10.0, 10.0));
        assert!(ComparisonOp::Lt.compare(9.9, 10.0));
        assert!(ComparisonOp::Lte.compare(10.0, 10.0));
        assert!(ComparisonOp::Eq.compare(42.0, 42.0));
        assert!(ComparisonOp::Ne.compare(42.0, 42.1));
        assert!(!ComparisonOp::Eq.compare(f64::NAN, 1.0));
    }

    #[test]
    fn silence_matcher_combines_non_empty_axes_with_and() {
        let r1 = Uuid::new_v4();
        let n1 = Uuid::new_v4();
        let matcher = SilenceMatcher {
            rule_ids: vec![r1],
            severities: vec![Severity::Critical, Severity::Warn],
            node_ids: Vec::new(),
            type_names: vec![String::from("Warehouse")],
        };
        assert!(matcher.matches(r1, Severity::Warn, Some(n1), Some("Warehouse")));
        assert!(!matcher.matches(r1, Severity::Info, Some(n1), Some("Warehouse")));
        assert!(!matcher.matches(r1, Severity::Warn, Some(n1), Some("Factory")));
    }

    #[test]
    fn nullable_patch_distinguishes_omitted_null_and_set() -> Result<(), serde_json::Error> {
        let omitted: UpdateRuleRequest = serde_json::from_str("{}")?;
        assert_eq!(omitted.escalate_after_minutes, NullablePatch::Unchanged);
        assert_eq!(omitted.escalate_after_minutes.apply_to(Some(15)), Some(15));

        let cleared: UpdateRuleRequest =
            serde_json::from_str(r#"{"escalate_after_minutes": null}"#)?;
        assert_eq!(cleared.escalate_after_minutes, NullablePatch::Clear);
        assert_eq!(cleared.escalate_after_minutes.apply_to(Some(15)), None);

        let updated: UpdateRuleRequest = serde_json::from_str(r#"{"escalate_after_minutes": 45}"#)?;
        assert_eq!(updated.escalate_after_minutes, NullablePatch::Set(45));
        assert_eq!(updated.escalate_after_minutes.apply_to(Some(15)), Some(45));
        Ok(())
    }
}
