//! Pure `xBlueView` 12-column five-zone preset generator and pre-publish widget health preflight kernel (`06-STEP20` §20.3).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::analytics::{PivotSpec, QueryColumn};
use crate::enums::{
    AssetKind, CockpitZoneLayout, DataType, PiiCategory, SemanticZoneKind, WidgetKind,
};
use crate::sim::ScenarioStatus;
use crate::studio::{PropertyKind, SchemaDoc};
use crate::view::{
    DashboardDetail, RefreshMode, WidgetActionType, WidgetDataSource, WidgetPosition, WidgetRecord,
    WriteNodeTargetKind,
};

/// Request payload for `POST /api/xblueview/dashboards/{id}/apply-zone-preset` (`06-STEP20` §20.3.1, §20.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyZonePresetRequest {
    /// Target cockpit zone layout preset.
    pub zone_layout: CockpitZoneLayout,
}

/// Canonical 12-column semantic region descriptor (`06-STEP20` §20.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CockpitZoneSlot {
    /// Semantic region kind.
    pub zone: SemanticZoneKind,
    /// Product-grade zone header label.
    pub title: String,
    /// Bounding 12-column grid region `{x, y, w, h}`.
    pub position: WidgetPosition,
}

/// Generated widget blueprint inside [`CockpitZoneLayout::StandardFiveZone`] (`06-STEP20` §20.3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZonePresetSlotSpec {
    /// Semantic zone assigned to this widget.
    pub semantic_zone: SemanticZoneKind,
    /// Visual widget kind.
    pub widget_kind: WidgetKind,
    /// Concise header title.
    pub title: String,
    /// 12-column placement (`0 <= x` and `x + w <= 12`).
    pub position: WidgetPosition,
    /// Initial declarative data source (falls back to non-blocking `Static` placeholder when live refs are absent).
    pub data_source: WidgetDataSource,
    /// Visual encoding JSON object.
    pub encoding: serde_json::Value,
    /// Options JSON object containing `"semantic_zone"`.
    pub options: serde_json::Value,
    /// Refresh mode.
    pub refresh_mode: RefreshMode,
    /// Refresh interval seconds (`5..=86400`).
    pub refresh_seconds: i32,
}

/// Optional tenant assets supplied when generating a 12-column five-zone preset (`06-STEP20` §20.3.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FiveZonePresetContext {
    /// Certified or available semantic metric machine names (`an_metrics.name`).
    #[serde(default)]
    pub metric_names: Vec<String>,
    /// Optional human-readable display names keyed by `an_metrics.name`.
    #[serde(default)]
    pub metric_labels: BTreeMap<String, String>,
    /// Optional `(type_name, metric)` fallback pairs from the published `main` `SchemaDoc`.
    #[serde(default)]
    pub node_metrics: Vec<(String, String)>,
    /// Available `SUCCEEDED` `AxSim` scenario UUIDs (binds `SIM_COMPARE` only when `>= 2`).
    #[serde(default)]
    pub succeeded_scenario_ids: Vec<Uuid>,
}

/// Aggregated preflight status for a dashboard prior to publish or external share (`06-STEP20` §20.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DashboardPreflightStatus {
    /// Zero blocking issues and zero warnings.
    Pass,
    /// Zero blocking issues and at least one non-blocking warning.
    Warn,
    /// One or more blocking issues; publish and external share are rejected with HTTP 422.
    Blocked,
}

impl DashboardPreflightStatus {
    /// Returns the canonical `SCREAMING_SNAKE_CASE` wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Blocked => "BLOCKED",
        }
    }
}

/// Machine-readable widget validation issue code (`06-STEP20` §20.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WidgetIssueCode {
    /// Backing dataset was deleted or does not exist in the tenant.
    MissingDataset,
    /// Semantic or node metric is missing, broken, uncertified (`WARNING`), or lacks runtime instances (`WARNING`).
    BrokenMetric,
    /// Referenced `an_saved_queries` row does not exist.
    MissingSavedQuery,
    /// `Sql` or `Pivot` query failed AST / specification validation.
    InvalidSqlAst,
    /// Referenced `xv_assets` row is missing, has wrong `AssetKind`, or has zero spatial bindings (`WARNING`).
    MissingAsset,
    /// `SimScenarios` widget has fewer than 2 `SUCCEEDED` scenarios (`BLOCKING`) or references a stale scenario (`WARNING`).
    InsufficientScenarios,
    /// `Action` widget lacks target flow/node, `PROD` deployment, valid target field, or object `input_schema`.
    UnconfiguredAction,
    /// Publisher lacks required permission (`DATA_READ`, `FLOW_RUN`, or ABAC) on the referenced target.
    UnauthorizedTarget,
    /// Non-note widget still uses a `STATIC` placeholder data source (`WARNING`).
    StaticPlaceholder,
    /// Widget grid coordinates exceed canvas bounds (`x < 0`, `w < 1`, or `x + w > cols`).
    LayoutOutOfBounds,
}

/// Single preflight finding tied to a widget or global filter (`06-STEP20` §20.3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetPreflightIssue {
    /// Target widget UUID (`None` when the finding originates from a global `FilterDef`).
    pub widget_id: Option<Uuid>,
    /// Widget or filter title.
    pub widget_title: String,
    /// Semantic zone extracted from `widget.options.semantic_zone`, if present.
    pub semantic_zone: Option<SemanticZoneKind>,
    /// Severity string (`"BLOCKING"` or `"WARNING"`).
    pub severity: String,
    /// Machine-readable issue code.
    pub code: WidgetIssueCode,
    /// Concise human-readable diagnostic message.
    pub message: String,
}

/// Response returned by `POST /api/xblueview/dashboards/{id}/preflight` (`06-STEP20` §20.3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardPreflightReport {
    /// Evaluated dashboard UUID.
    pub dashboard_id: Uuid,
    /// Aggregated status (`PASS`, `WARN`, or `BLOCKED`).
    pub status: DashboardPreflightStatus,
    /// Total number of widgets inspected.
    pub checked_widgets: usize,
    /// Number of `"BLOCKING"` issues.
    pub blocking_count: usize,
    /// Number of `"WARNING"` issues.
    pub warning_count: usize,
    /// Ordered list of findings.
    pub issues: Vec<WidgetPreflightIssue>,
}

impl DashboardPreflightReport {
    /// Converts a blocking report into [`AppError::UnprocessableDetails`] (`422 VIEW_PREFLIGHT_BLOCKED`)
    /// with structured `issues` details, or returns `Ok(self)` when `blocking_count == 0`.
    ///
    /// # Errors
    /// Returns [`AppError::UnprocessableDetails`] when `self.blocking_count > 0`.
    pub fn ensure_publishable(self) -> Result<Self, AppError> {
        if self.blocking_count > 0 {
            let details = serde_json::json!({
                "dashboard_id": self.dashboard_id,
                "status": self.status,
                "checked_widgets": self.checked_widgets,
                "blocking_count": self.blocking_count,
                "warning_count": self.warning_count,
                "issues": self.issues,
            });
            return Err(AppError::unprocessable_details(
                "VIEW_PREFLIGHT_BLOCKED",
                format!(
                    "dashboard {} failed preflight with {} blocking issue(s)",
                    self.dashboard_id, self.blocking_count
                ),
                details,
            ));
        }
        Ok(self)
    }
}

/// Saved query dependency state supplied to [`evaluate_dashboard_preflight`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQueryPreflightState {
    /// Backing dataset UUIDs referenced by the saved query.
    #[serde(default)]
    pub referenced_dataset_ids: Vec<Uuid>,
    /// Explicit flag when a referenced dataset or metric could not be resolved.
    #[serde(default)]
    pub dataset_missing: bool,
}

/// Semantic metric dependency state supplied to [`evaluate_dashboard_preflight`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricPreflightState {
    /// Backing dataset UUID.
    pub dataset_id: Uuid,
    /// Whether `MetricDef.broken` (or live column check) detected a broken schema reference.
    pub broken: bool,
    /// Whether `MetricDef.certified` is `true`.
    pub certified: bool,
}

/// Pre-evaluated SQL / `sql_guard` AST check result supplied by the analytics/server layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SqlPreflightOutcome {
    /// SQL passed `sql_guard` AST validation against existing, authorized datasets.
    Valid {
        /// Dataset UUIDs referenced by the guarded SQL statement.
        #[serde(default)]
        referenced_dataset_ids: Vec<Uuid>,
    },
    /// SQL was rejected by `sql_guard` AST rules.
    InvalidAst {
        /// Diagnostic message.
        message: String,
    },
    /// SQL referenced an unknown/deleted lake table.
    MissingDataset {
        /// Diagnostic message.
        message: String,
    },
    /// Publisher lacks `DATA_READ` / policy access to a referenced dataset.
    UnauthorizedDataset {
        /// Diagnostic message.
        message: String,
    },
}

/// Visual asset dependency state supplied to [`evaluate_dashboard_preflight`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetPreflightState {
    /// Stored asset kind (`FLOOR_PLAN_IMAGE`, `MODEL_3D`, etc.).
    pub asset_kind: AssetKind,
    /// Count of bound node placements (`xv_floor_placements`) or 3D bindings (`xv_scene3d_bindings`).
    pub binding_count: usize,
}

/// `AxSim` scenario dependency state supplied to [`evaluate_dashboard_preflight`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioPreflightState {
    /// Execution status.
    pub status: ScenarioStatus,
    /// Whether the sandbox overlay drifted since execution.
    pub is_stale: bool,
}

/// `AxFlow` workflow dependency state supplied to [`evaluate_dashboard_preflight`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowPreflightState {
    /// Whether the flow currently has an active `PROD` deployment.
    pub has_prod_deployment: bool,
}

/// Business node dependency state supplied to [`evaluate_dashboard_preflight`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodePreflightState {
    /// Node `type_name` in `xb_nodes`.
    pub type_name: String,
    /// Whether the publisher is authorized to mutate/access this node under ABAC.
    pub authorized: bool,
}

/// Pure dependency snapshot consumed by [`evaluate_dashboard_preflight`] (`06-STEP20` §20.3.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DashboardPreflightContext {
    /// Existing dataset UUIDs in the tenant.
    #[serde(default)]
    pub existing_dataset_ids: BTreeSet<Uuid>,
    /// Dataset machine name -> UUID lookup for `Pivot` dataset resolution.
    #[serde(default)]
    pub dataset_ids_by_name: BTreeMap<String, Uuid>,
    /// Dataset UUIDs for which the publisher lacks `DATA_READ` or ABAC access.
    #[serde(default)]
    pub unauthorized_dataset_ids: BTreeSet<Uuid>,
    /// Existing saved queries keyed by `an_saved_queries.id`.
    #[serde(default)]
    pub saved_queries: BTreeMap<Uuid, SavedQueryPreflightState>,
    /// Existing semantic metrics keyed by `an_metrics.name`.
    #[serde(default)]
    pub metrics: BTreeMap<String, MetricPreflightState>,
    /// Pre-evaluated `sql_guard` outcomes for `Sql` / `Pivot` widgets keyed by `widget.id`.
    #[serde(default)]
    pub sql_outcomes: BTreeMap<Uuid, SqlPreflightOutcome>,
    /// Pre-evaluated `sql_guard` outcomes for global `FilterDef.options_source` keyed by `filter.key`.
    #[serde(default)]
    pub filter_sql_outcomes: BTreeMap<String, SqlPreflightOutcome>,
    /// Published `main` `SchemaDoc` for `NodeMetric` and `WRITE_NODE` validation.
    #[serde(default)]
    pub main_schema: SchemaDoc,
    /// Node `type_name`s that currently have at least one runtime row in `xb_nodes`.
    #[serde(default)]
    pub populated_node_types: BTreeSet<String>,
    /// Existing visual assets keyed by `xv_assets.id`.
    #[serde(default)]
    pub assets: BTreeMap<Uuid, AssetPreflightState>,
    /// Existing `AxSim` scenarios keyed by `ax_sim_scenarios.id`.
    #[serde(default)]
    pub scenarios: BTreeMap<Uuid, ScenarioPreflightState>,
    /// Existing `AxFlow` definitions keyed by `ax_flows.id`.
    #[serde(default)]
    pub flows: BTreeMap<Uuid, FlowPreflightState>,
    /// Existing target business nodes keyed by `xb_nodes.id`.
    #[serde(default)]
    pub nodes: BTreeMap<Uuid, NodePreflightState>,
    /// Whether the publisher holds `Permission::FlowRun` (required for `RUN_FLOW` action widgets).
    #[serde(default = "default_true_flag")]
    pub caller_has_flow_run: bool,
}

const fn default_true_flag() -> bool {
    true
}

/// Returns the five canonical 12-column semantic zone bounding boxes (`06-STEP20` §20.3.1).
#[must_use]
pub fn standard_five_zone_slots() -> Vec<CockpitZoneSlot> {
    vec![
        CockpitZoneSlot {
            zone: SemanticZoneKind::Control,
            title: String::from("全局控制"),
            position: WidgetPosition {
                x: 0,
                y: 0,
                w: 12,
                h: 2,
            },
        },
        CockpitZoneSlot {
            zone: SemanticZoneKind::Kpi,
            title: String::from("核心指标"),
            position: WidgetPosition {
                x: 0,
                y: 2,
                w: 12,
                h: 3,
            },
        },
        CockpitZoneSlot {
            zone: SemanticZoneKind::Scene,
            title: String::from("孪生态势"),
            position: WidgetPosition {
                x: 0,
                y: 5,
                w: 7,
                h: 9,
            },
        },
        CockpitZoneSlot {
            zone: SemanticZoneKind::Insight,
            title: String::from("归因洞察"),
            position: WidgetPosition {
                x: 7,
                y: 5,
                w: 5,
                h: 9,
            },
        },
        CockpitZoneSlot {
            zone: SemanticZoneKind::Decision,
            title: String::from("协同决策"),
            position: WidgetPosition {
                x: 0,
                y: 14,
                w: 12,
                h: 5,
            },
        },
    ]
}

fn static_placeholder_source() -> WidgetDataSource {
    WidgetDataSource::Static {
        columns: vec![QueryColumn {
            name: String::from("value"),
            data_type: DataType::Float64,
            pii_category: PiiCategory::None,
            masked: false,
        }],
        rows: vec![vec![serde_json::json!(0)]],
    }
}

fn zone_options(zone: SemanticZoneKind) -> serde_json::Value {
    serde_json::json!({ "semantic_zone": zone.as_str() })
}

fn build_kpi_slot(idx: usize, ctx: &FiveZonePresetContext) -> ZonePresetSlotSpec {
    let x = i32::try_from(idx.saturating_mul(3)).unwrap_or(0);
    let display_idx = idx.saturating_add(1);
    let (title, data_source) = if let Some(metric_name) = ctx.metric_names.get(idx) {
        let label = ctx
            .metric_labels
            .get(metric_name)
            .cloned()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| metric_name.clone());
        (
            label,
            WidgetDataSource::Metric {
                metric: metric_name.clone(),
                dimensions: Vec::new(),
                time_grain: None,
                time_range: None,
            },
        )
    } else if let Some((type_name, metric)) = ctx
        .node_metrics
        .get(idx.saturating_sub(ctx.metric_names.len()))
    {
        (
            format!("{type_name} · {metric}"),
            WidgetDataSource::NodeMetric {
                type_name: type_name.clone(),
                metric: metric.clone(),
                agg: Some(String::from("AVG")),
                grain: Some(String::from("DAY")),
                time_range: Some(String::from("LAST_7_DAYS")),
                group_by: None,
            },
        )
    } else {
        (
            format!("核心指标 {display_idx}"),
            static_placeholder_source(),
        )
    };

    ZonePresetSlotSpec {
        semantic_zone: SemanticZoneKind::Kpi,
        widget_kind: WidgetKind::KpiCard,
        title,
        position: WidgetPosition {
            x,
            y: 2,
            w: 3,
            h: 3,
        },
        data_source,
        encoding: serde_json::json!({}),
        options: zone_options(SemanticZoneKind::Kpi),
        refresh_mode: RefreshMode::Live,
        refresh_seconds: 60,
    }
}

fn build_insight_slots(ctx: &FiveZonePresetContext) -> [ZonePresetSlotSpec; 2] {
    let line_ds = ctx
        .metric_names
        .first()
        .map_or_else(static_placeholder_source, |m| WidgetDataSource::Metric {
            metric: m.clone(),
            dimensions: Vec::new(),
            time_grain: Some(String::from("DAY")),
            time_range: Some(String::from("LAST_30_DAYS")),
        });
    let bar_ds = ctx
        .metric_names
        .get(1)
        .or_else(|| ctx.metric_names.first())
        .map_or_else(static_placeholder_source, |m| WidgetDataSource::Metric {
            metric: m.clone(),
            dimensions: Vec::new(),
            time_grain: None,
            time_range: None,
        });

    [
        ZonePresetSlotSpec {
            semantic_zone: SemanticZoneKind::Insight,
            widget_kind: WidgetKind::LineChart,
            title: String::from("趋势分析"),
            position: WidgetPosition {
                x: 7,
                y: 5,
                w: 5,
                h: 4,
            },
            data_source: line_ds,
            encoding: serde_json::json!({}),
            options: zone_options(SemanticZoneKind::Insight),
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
        },
        ZonePresetSlotSpec {
            semantic_zone: SemanticZoneKind::Insight,
            widget_kind: WidgetKind::BarChart,
            title: String::from("结构对比"),
            position: WidgetPosition {
                x: 7,
                y: 9,
                w: 5,
                h: 5,
            },
            data_source: bar_ds,
            encoding: serde_json::json!({}),
            options: zone_options(SemanticZoneKind::Insight),
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
        },
    ]
}

fn build_decision_slots(ctx: &FiveZonePresetContext) -> [ZonePresetSlotSpec; 2] {
    let decision_ds = if ctx.succeeded_scenario_ids.len() >= 2 {
        WidgetDataSource::SimScenarios {
            scenario_ids: ctx.succeeded_scenario_ids.iter().take(5).copied().collect(),
            metrics: Vec::new(),
        }
    } else {
        WidgetDataSource::Static {
            columns: Vec::new(),
            rows: Vec::new(),
        }
    };

    [
        ZonePresetSlotSpec {
            semantic_zone: SemanticZoneKind::Decision,
            widget_kind: WidgetKind::AlertList,
            title: String::from("活跃告警"),
            position: WidgetPosition {
                x: 0,
                y: 14,
                w: 8,
                h: 5,
            },
            data_source: WidgetDataSource::Alerts {
                status: vec![String::from("OPEN")],
                severity: Vec::new(),
                limit: Some(20),
            },
            encoding: serde_json::json!({}),
            options: zone_options(SemanticZoneKind::Decision),
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
        },
        ZonePresetSlotSpec {
            semantic_zone: SemanticZoneKind::Decision,
            widget_kind: WidgetKind::SimCompare,
            title: String::from("推演对比"),
            position: WidgetPosition {
                x: 8,
                y: 14,
                w: 4,
                h: 5,
            },
            data_source: decision_ds,
            encoding: serde_json::json!({}),
            options: zone_options(SemanticZoneKind::Decision),
            refresh_mode: RefreshMode::Manual,
            refresh_seconds: 60,
        },
    ]
}

/// Generates the canonical 12-column five-zone widget skeleton covering `CONTROL`, `KPI`, `SCENE`,
/// `INSIGHT`, and `DECISION` (`06-STEP20` §20.3.1).
///
/// Uses non-blocking [`WidgetDataSource::Static`] placeholders whenever certified metrics or `< 2`
/// `SUCCEEDED` simulation scenarios are available so newly created cockpits never fail publish with
/// broken references out of the box.
#[must_use]
pub fn generate_five_zone_preset(ctx: &FiveZonePresetContext) -> Vec<ZonePresetSlotSpec> {
    let mut slots = Vec::with_capacity(10);

    slots.push(ZonePresetSlotSpec {
        semantic_zone: SemanticZoneKind::Control,
        widget_kind: WidgetKind::MarkdownNote,
        title: String::from("全局控制"),
        position: WidgetPosition {
            x: 0,
            y: 0,
            w: 12,
            h: 2,
        },
        data_source: WidgetDataSource::Static {
            columns: Vec::new(),
            rows: Vec::new(),
        },
        encoding: serde_json::json!({}),
        options: serde_json::json!({
            "semantic_zone": SemanticZoneKind::Control.as_str(),
            "markdown": "全局时间窗与业务子图控制栏"
        }),
        refresh_mode: RefreshMode::Manual,
        refresh_seconds: 60,
    });

    for idx in 0..4 {
        slots.push(build_kpi_slot(idx, ctx));
    }

    slots.push(ZonePresetSlotSpec {
        semantic_zone: SemanticZoneKind::Scene,
        widget_kind: WidgetKind::GraphTopology,
        title: String::from("孪生态势"),
        position: WidgetPosition {
            x: 0,
            y: 5,
            w: 7,
            h: 9,
        },
        data_source: WidgetDataSource::Graph {
            type_names: Vec::new(),
            link_types: Vec::new(),
            color_metric: None,
            limit: Some(500),
        },
        encoding: serde_json::json!({}),
        options: zone_options(SemanticZoneKind::Scene),
        refresh_mode: RefreshMode::Live,
        refresh_seconds: 60,
    });

    slots.extend(build_insight_slots(ctx));
    slots.extend(build_decision_slots(ctx));
    slots
}

struct IssueTarget<'a> {
    widget_id: Option<Uuid>,
    title: &'a str,
    zone: Option<SemanticZoneKind>,
    widget_kind: Option<WidgetKind>,
    deps_dataset_ids: &'a [Uuid],
}

fn push_issue(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    severity: &str,
    code: WidgetIssueCode,
    message: impl Into<String>,
) {
    issues.push(WidgetPreflightIssue {
        widget_id: target.widget_id,
        widget_title: target.title.to_owned(),
        semantic_zone: target.zone,
        severity: String::from(severity),
        code,
        message: message.into(),
    });
}

fn extract_semantic_zone(options: &serde_json::Value) -> Option<SemanticZoneKind> {
    options
        .get("semantic_zone")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<SemanticZoneKind>().ok())
}

fn check_dataset_refs(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    dataset_ids: &[Uuid],
    ctx: &DashboardPreflightContext,
) {
    for ds_id in dataset_ids {
        if !ctx.existing_dataset_ids.contains(ds_id) {
            push_issue(
                issues,
                target,
                "BLOCKING",
                WidgetIssueCode::MissingDataset,
                format!("referenced dataset {ds_id} does not exist"),
            );
            return;
        }
        if ctx.unauthorized_dataset_ids.contains(ds_id) {
            push_issue(
                issues,
                target,
                "BLOCKING",
                WidgetIssueCode::UnauthorizedTarget,
                format!("publisher lacks DATA_READ access to dataset {ds_id}"),
            );
            return;
        }
    }
}

fn check_sql_outcome(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    outcome: Option<&SqlPreflightOutcome>,
    raw_sql_non_empty: bool,
    ctx: &DashboardPreflightContext,
) {
    if !raw_sql_non_empty {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::InvalidSqlAst,
            "SQL query cannot be empty",
        );
        return;
    }
    match outcome {
        Some(SqlPreflightOutcome::Valid {
            referenced_dataset_ids,
        }) => check_dataset_refs(issues, target, referenced_dataset_ids, ctx),
        Some(SqlPreflightOutcome::InvalidAst { message }) => push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::InvalidSqlAst,
            message.clone(),
        ),
        Some(SqlPreflightOutcome::MissingDataset { message }) => push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::MissingDataset,
            message.clone(),
        ),
        Some(SqlPreflightOutcome::UnauthorizedDataset { message }) => push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnauthorizedTarget,
            message.clone(),
        ),
        None => check_dataset_refs(issues, target, target.deps_dataset_ids, ctx),
    }
}

fn check_pivot_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    spec: &serde_json::Value,
    outcome: Option<&SqlPreflightOutcome>,
    ctx: &DashboardPreflightContext,
) {
    let Ok(parsed) = serde_json::from_value::<PivotSpec>(spec.clone()) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::InvalidSqlAst,
            "invalid PivotSpec configuration",
        );
        return;
    };
    let ds_name = parsed.dataset.trim();
    if ds_name.is_empty() || parsed.values.is_empty() {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::InvalidSqlAst,
            "PivotSpec requires a non-empty dataset and at least one value measure",
        );
        return;
    }
    if !ds_name.starts_with("ax.") {
        let Some(&ds_id) = ctx.dataset_ids_by_name.get(ds_name) else {
            push_issue(
                issues,
                target,
                "BLOCKING",
                WidgetIssueCode::MissingDataset,
                format!("pivot dataset '{ds_name}' does not exist"),
            );
            return;
        };
        check_dataset_refs(issues, target, &[ds_id], ctx);
        return;
    }
    if let Some(out) = outcome {
        check_sql_outcome(issues, target, Some(out), true, ctx);
    }
}

fn check_node_metric_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    type_name: &str,
    metric: &str,
    ctx: &DashboardPreflightContext,
) {
    let t_name = type_name.trim();
    let m_name = metric.trim();
    let declared = ctx
        .main_schema
        .node_types
        .iter()
        .find(|nt| nt.name == t_name)
        .is_some_and(|nt| {
            nt.properties
                .iter()
                .any(|p| p.kind == PropertyKind::Metric && p.name == m_name)
                || nt.agg_rules.iter().any(|r| r.target_metric == m_name)
                || nt
                    .derived_rules
                    .iter()
                    .any(|d| d.enabled && d.target_metric == m_name)
        });
    if !declared {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::BrokenMetric,
            format!("node metric '{t_name}.{m_name}' is not declared in published main schema"),
        );
    } else if !ctx.populated_node_types.contains(t_name) {
        push_issue(
            issues,
            target,
            "WARNING",
            WidgetIssueCode::BrokenMetric,
            format!("node type '{t_name}' currently has zero runtime instances"),
        );
    }
}

fn is_valid_action_object_schema(schema: &serde_json::Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if obj.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return false;
    }
    obj.get("properties")
        .is_none_or(serde_json::Value::is_object)
}

struct ActionSourceRef<'a> {
    action_type: WidgetActionType,
    flow_id: Option<Uuid>,
    node_id: Option<Uuid>,
    target_kind: Option<WriteNodeTargetKind>,
    key: Option<&'a str>,
    input_schema: &'a serde_json::Value,
}

fn check_run_flow_action(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    flow_id: Option<Uuid>,
    ctx: &DashboardPreflightContext,
) {
    let Some(fid) = flow_id else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            "RUN_FLOW action widget has no bound flow_id",
        );
        return;
    };
    let Some(flow) = ctx.flows.get(&fid) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            format!("target workflow {fid} does not exist"),
        );
        return;
    };
    if !flow.has_prod_deployment {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            format!("target workflow {fid} has no PROD deployment"),
        );
    } else if !ctx.caller_has_flow_run {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnauthorizedTarget,
            "publisher lacks FLOW_RUN permission for RUN_FLOW action",
        );
    }
}

fn check_write_node_action(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    node_id: Option<Uuid>,
    target_kind: Option<WriteNodeTargetKind>,
    key: Option<&str>,
    ctx: &DashboardPreflightContext,
) {
    let clean_key = key.map(str::trim).filter(|s| !s.is_empty());
    let (Some(nid), Some(t_kind), Some(field_key)) = (node_id, target_kind, clean_key) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            "WRITE_NODE action widget requires node_id, target_kind, and non-empty key",
        );
        return;
    };
    let Some(node_state) = ctx.nodes.get(&nid) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            format!("target node {nid} does not exist"),
        );
        return;
    };
    if !node_state.authorized {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnauthorizedTarget,
            format!("publisher is not authorized to modify node {nid}"),
        );
        return;
    }
    let expected_prop_kind = match t_kind {
        WriteNodeTargetKind::Property => PropertyKind::Attribute,
        WriteNodeTargetKind::Metric => PropertyKind::Metric,
    };
    let has_field = ctx
        .main_schema
        .node_types
        .iter()
        .find(|nt| nt.name == node_state.type_name)
        .is_some_and(|nt| {
            nt.properties
                .iter()
                .any(|p| p.kind == expected_prop_kind && p.name == field_key)
        });
    if !has_field {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            format!(
                "field '{field_key}' ({expected_prop_kind:?}) does not exist on '{}'",
                node_state.type_name
            ),
        );
    }
}

fn check_action_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    act: &ActionSourceRef<'_>,
    ctx: &DashboardPreflightContext,
) {
    if !is_valid_action_object_schema(act.input_schema) {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnconfiguredAction,
            "action input_schema must be a JSON Schema with type = 'object'",
        );
        return;
    }
    match act.action_type {
        WidgetActionType::RunFlow => check_run_flow_action(issues, target, act.flow_id, ctx),
        WidgetActionType::WriteNode => {
            check_write_node_action(issues, target, act.node_id, act.target_kind, act.key, ctx);
        }
    }
}

fn check_saved_query_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    saved_query_id: Uuid,
    ctx: &DashboardPreflightContext,
) {
    let Some(sq) = ctx.saved_queries.get(&saved_query_id) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::MissingSavedQuery,
            format!("saved query {saved_query_id} does not exist"),
        );
        return;
    };
    if sq.dataset_missing {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::MissingDataset,
            format!("saved query {saved_query_id} references a deleted dataset"),
        );
    } else {
        check_dataset_refs(issues, target, &sq.referenced_dataset_ids, ctx);
    }
}

fn check_metric_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    metric: &str,
    ctx: &DashboardPreflightContext,
) {
    let name = metric.trim();
    let Some(m) = ctx.metrics.get(name) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::BrokenMetric,
            format!("metric '{name}' does not exist"),
        );
        return;
    };
    if m.broken {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::BrokenMetric,
            format!("metric '{name}' has broken column references"),
        );
    } else if !ctx.existing_dataset_ids.contains(&m.dataset_id) {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::MissingDataset,
            format!(
                "backing dataset {} for metric '{name}' is missing",
                m.dataset_id
            ),
        );
    } else if ctx.unauthorized_dataset_ids.contains(&m.dataset_id) {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::UnauthorizedTarget,
            format!("publisher lacks access to metric '{name}' dataset"),
        );
    } else if !m.certified {
        push_issue(
            issues,
            target,
            "WARNING",
            WidgetIssueCode::BrokenMetric,
            format!("metric '{name}' is not marked certified"),
        );
    }
}

fn check_sim_scenarios_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    scenario_ids: &[Uuid],
    ctx: &DashboardPreflightContext,
) {
    let mut unique_ok = BTreeSet::new();
    let mut any_stale = false;
    for sid in scenario_ids {
        if let Some(sc) = ctx.scenarios.get(sid) {
            if sc.status == ScenarioStatus::Succeeded {
                unique_ok.insert(*sid);
            }
            if sc.is_stale {
                any_stale = true;
            }
        }
    }
    if unique_ok.len() < 2 {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::InsufficientScenarios,
            "SIM_COMPARE widget requires at least 2 SUCCEEDED scenarios",
        );
    } else if any_stale {
        push_issue(
            issues,
            target,
            "WARNING",
            WidgetIssueCode::InsufficientScenarios,
            "one or more referenced simulation scenarios are marked stale",
        );
    }
}

fn check_data_source(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    source: &WidgetDataSource,
    sql_outcome: Option<&SqlPreflightOutcome>,
    ctx: &DashboardPreflightContext,
) {
    match source {
        WidgetDataSource::SavedQuery { saved_query_id } => {
            check_saved_query_source(issues, target, *saved_query_id, ctx);
        }
        WidgetDataSource::Metric { metric, .. } => {
            check_metric_source(issues, target, metric, ctx);
        }
        WidgetDataSource::Sql { sql } => {
            check_sql_outcome(issues, target, sql_outcome, !sql.trim().is_empty(), ctx);
        }
        WidgetDataSource::Pivot { spec } => {
            check_pivot_source(issues, target, spec, sql_outcome, ctx);
        }
        WidgetDataSource::NodeMetric {
            type_name, metric, ..
        } => {
            check_node_metric_source(issues, target, type_name, metric, ctx);
        }
        WidgetDataSource::FloorPlan { asset_id, .. } => {
            check_spatial_asset(issues, target, *asset_id, AssetKind::FloorPlanImage, ctx);
        }
        WidgetDataSource::Scene3d { asset_id, .. } => {
            check_spatial_asset(issues, target, *asset_id, AssetKind::Model3d, ctx);
        }
        WidgetDataSource::SimScenarios { scenario_ids, .. } => {
            check_sim_scenarios_source(issues, target, scenario_ids, ctx);
        }
        WidgetDataSource::Action {
            action_type,
            flow_id,
            node_id,
            target_kind,
            key,
            input_schema,
            ..
        } => {
            let act = ActionSourceRef {
                action_type: *action_type,
                flow_id: *flow_id,
                node_id: *node_id,
                target_kind: *target_kind,
                key: key.as_deref(),
                input_schema,
            };
            check_action_source(issues, target, &act, ctx);
        }
        WidgetDataSource::Static { .. } => {
            if let Some(kind) = target.widget_kind
                && kind != WidgetKind::MarkdownNote
            {
                push_issue(
                    issues,
                    target,
                    "WARNING",
                    WidgetIssueCode::StaticPlaceholder,
                    "widget uses an unbound STATIC placeholder data source",
                );
            }
        }
        WidgetDataSource::Graph { .. }
        | WidgetDataSource::Alerts { .. }
        | WidgetDataSource::NodesGeo { .. } => {}
    }
}

fn check_spatial_asset(
    issues: &mut Vec<WidgetPreflightIssue>,
    target: &IssueTarget<'_>,
    asset_id: Uuid,
    expected_kind: AssetKind,
    ctx: &DashboardPreflightContext,
) {
    let Some(asset) = ctx.assets.get(&asset_id) else {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::MissingAsset,
            format!("visual asset {asset_id} does not exist"),
        );
        return;
    };
    if asset.asset_kind != expected_kind {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::MissingAsset,
            format!(
                "visual asset {asset_id} has kind {}, expected {}",
                asset.asset_kind.as_str(),
                expected_kind.as_str()
            ),
        );
    } else if asset.binding_count == 0 {
        push_issue(
            issues,
            target,
            "WARNING",
            WidgetIssueCode::MissingAsset,
            format!("visual asset {asset_id} has zero bound node placements/meshes"),
        );
    }
}

fn check_widget_bounds(
    issues: &mut Vec<WidgetPreflightIssue>,
    widget: &WidgetRecord,
    target: &IssueTarget<'_>,
    max_cols: i32,
) {
    let pos = &widget.position;
    if pos.x < 0 || pos.y < 0 || pos.w < 1 || pos.h < 1 || pos.x.saturating_add(pos.w) > max_cols {
        push_issue(
            issues,
            target,
            "BLOCKING",
            WidgetIssueCode::LayoutOutOfBounds,
            format!(
                "widget position (x={}, y={}, w={}, h={}) exceeds {max_cols}-column grid",
                pos.x, pos.y, pos.w, pos.h
            ),
        );
    }
}

/// Evaluates all widgets and global filter `options_source` bindings on a [`DashboardDetail`]
/// prior to publishing or external sharing (`06-STEP20` §20.3.2).
#[must_use]
pub fn evaluate_dashboard_preflight(
    detail: &DashboardDetail,
    ctx: &DashboardPreflightContext,
) -> DashboardPreflightReport {
    let mut issues = Vec::new();
    let max_cols = if detail.dashboard.canvas.zone_layout == CockpitZoneLayout::StandardFiveZone {
        detail.dashboard.canvas.cols.clamp(1, 12)
    } else {
        detail.dashboard.canvas.cols.max(1)
    };

    for filter in &detail.dashboard.filters {
        if let Some(ref src) = filter.options_source {
            let target = IssueTarget {
                widget_id: None,
                title: &filter.label,
                zone: Some(SemanticZoneKind::Control),
                widget_kind: None,
                deps_dataset_ids: &[],
            };
            let sql_out = ctx.filter_sql_outcomes.get(&filter.key);
            check_data_source(&mut issues, &target, src, sql_out, ctx);
        }
    }

    for widget in &detail.widgets {
        let target = IssueTarget {
            widget_id: Some(widget.id),
            title: &widget.title,
            zone: extract_semantic_zone(&widget.options),
            widget_kind: Some(widget.widget_kind),
            deps_dataset_ids: &widget.deps.dataset_ids,
        };
        check_widget_bounds(&mut issues, widget, &target, max_cols);
        let sql_out = ctx.sql_outcomes.get(&widget.id);
        check_data_source(&mut issues, &target, &widget.data_source, sql_out, ctx);
    }

    let blocking_count = issues.iter().filter(|i| i.severity == "BLOCKING").count();
    let warning_count = issues.iter().filter(|i| i.severity == "WARNING").count();
    let status = if blocking_count > 0 {
        DashboardPreflightStatus::Blocked
    } else if warning_count > 0 {
        DashboardPreflightStatus::Warn
    } else {
        DashboardPreflightStatus::Pass
    };

    DashboardPreflightReport {
        dashboard_id: detail.dashboard.id,
        status,
        checked_widgets: detail.widgets.len(),
        blocking_count,
        warning_count,
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{CanvasConfig, DashboardRecord, DashboardStatus, DashboardTheme, WidgetDeps};
    use chrono::Utc;

    fn make_dashboard(canvas: CanvasConfig, widgets: Vec<WidgetRecord>) -> DashboardDetail {
        let now = Utc::now();
        DashboardDetail {
            dashboard: DashboardRecord {
                id: Uuid::new_v4(),
                tenant_id: String::from("default"),
                name: String::from("Five Zone Board"),
                slug: String::from("five-zone-board"),
                description: String::new(),
                filters: Vec::new(),
                theme: DashboardTheme::Dark,
                canvas,
                status: DashboardStatus::Draft,
                version: 1,
                created_by: None,
                created_at: now,
                updated_at: now,
            },
            widgets,
        }
    }

    fn slot_to_widget(dashboard_id: Uuid, slot: ZonePresetSlotSpec) -> WidgetRecord {
        let now = Utc::now();
        WidgetRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from("default"),
            dashboard_id,
            widget_kind: slot.widget_kind,
            title: slot.title,
            position: slot.position,
            data_source: slot.data_source,
            encoding: slot.encoding,
            options: slot.options,
            drill: Vec::new(),
            refresh_mode: slot.refresh_mode,
            refresh_seconds: slot.refresh_seconds,
            deps: WidgetDeps::default(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn five_zone_preset_covers_all_zones_within_12_cols_and_warns_without_blocking() {
        let slots = generate_five_zone_preset(&FiveZonePresetContext::default());
        assert_eq!(slots.len(), 10);

        let mut zones = BTreeSet::new();
        for slot in &slots {
            zones.insert(slot.semantic_zone);
            assert!(slot.position.x >= 0);
            assert!(slot.position.w >= 1);
            assert!(slot.position.x + slot.position.w <= 12);
            assert_eq!(
                slot.options
                    .get("semantic_zone")
                    .and_then(serde_json::Value::as_str),
                Some(slot.semantic_zone.as_str())
            );
        }
        assert_eq!(zones.len(), 5);

        let dash_id = Uuid::new_v4();
        let widgets: Vec<WidgetRecord> = slots
            .into_iter()
            .map(|s| slot_to_widget(dash_id, s))
            .collect();
        let detail = make_dashboard(CanvasConfig::standard_five_zone(), widgets);
        let report = evaluate_dashboard_preflight(&detail, &DashboardPreflightContext::default());
        assert_eq!(report.blocking_count, 0);
        assert_eq!(report.status, DashboardPreflightStatus::Warn);
        assert!(report.ensure_publishable().is_ok());
    }

    #[test]
    fn five_zone_preset_passes_cleanly_when_certified_metrics_and_scenarios_bound() {
        let ds_id = Uuid::new_v4();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let preset_ctx = FiveZonePresetContext {
            metric_names: vec![
                String::from("m1"),
                String::from("m2"),
                String::from("m3"),
                String::from("m4"),
            ],
            metric_labels: BTreeMap::new(),
            node_metrics: Vec::new(),
            succeeded_scenario_ids: vec![s1, s2],
        };
        let slots = generate_five_zone_preset(&preset_ctx);
        let dash_id = Uuid::new_v4();
        let widgets: Vec<WidgetRecord> = slots
            .into_iter()
            .map(|s| slot_to_widget(dash_id, s))
            .collect();
        let detail = make_dashboard(CanvasConfig::standard_five_zone(), widgets);

        let mut metrics = BTreeMap::new();
        for name in ["m1", "m2", "m3", "m4"] {
            metrics.insert(
                String::from(name),
                MetricPreflightState {
                    dataset_id: ds_id,
                    broken: false,
                    certified: true,
                },
            );
        }
        let mut scenarios = BTreeMap::new();
        scenarios.insert(
            s1,
            ScenarioPreflightState {
                status: ScenarioStatus::Succeeded,
                is_stale: false,
            },
        );
        scenarios.insert(
            s2,
            ScenarioPreflightState {
                status: ScenarioStatus::Succeeded,
                is_stale: false,
            },
        );
        let check_ctx = DashboardPreflightContext {
            existing_dataset_ids: BTreeSet::from([ds_id]),
            metrics,
            scenarios,
            ..DashboardPreflightContext::default()
        };

        let report = evaluate_dashboard_preflight(&detail, &check_ctx);
        assert_eq!(report.status, DashboardPreflightStatus::Pass);
        assert_eq!(report.blocking_count, 0);
        assert_eq!(report.warning_count, 0);
    }

    #[test]
    fn unconfigured_run_flow_action_blocks_preflight_and_emits_422_details() {
        let now = Utc::now();
        let dash_id = Uuid::new_v4();
        let broken_action = WidgetRecord {
            id: Uuid::new_v4(),
            tenant_id: String::from("default"),
            dashboard_id: dash_id,
            widget_kind: WidgetKind::ActionButton,
            title: String::from("下发调拨"),
            position: WidgetPosition {
                x: 8,
                y: 14,
                w: 4,
                h: 2,
            },
            data_source: WidgetDataSource::Action {
                action_type: WidgetActionType::RunFlow,
                flow_id: None,
                node_id: None,
                target_kind: None,
                key: None,
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
                confirm_text: None,
            },
            encoding: serde_json::json!({}),
            options: serde_json::json!({"semantic_zone": "DECISION"}),
            drill: Vec::new(),
            refresh_mode: RefreshMode::Manual,
            refresh_seconds: 60,
            deps: WidgetDeps::default(),
            created_at: now,
            updated_at: now,
        };
        let detail = make_dashboard(CanvasConfig::standard_five_zone(), vec![broken_action]);
        let report = evaluate_dashboard_preflight(&detail, &DashboardPreflightContext::default());
        assert_eq!(report.status, DashboardPreflightStatus::Blocked);
        assert_eq!(report.blocking_count, 1);
        assert_eq!(
            report.issues.first().map(|i| i.code),
            Some(WidgetIssueCode::UnconfiguredAction)
        );
        assert_eq!(
            report.issues.first().and_then(|i| i.semantic_zone),
            Some(SemanticZoneKind::Decision)
        );

        let err_opt = report.ensure_publishable().err();
        assert!(err_opt.is_some());
        if let Some(err) = err_opt {
            assert_eq!(err.error_code(), "VIEW_PREFLIGHT_BLOCKED");
            assert!(err.details().is_some());
        }
    }

    #[test]
    fn broken_metric_missing_dataset_and_insufficient_scenarios_are_detected() {
        let now = Utc::now();
        let dash_id = Uuid::new_v4();
        let s1 = Uuid::new_v4();
        let widgets = vec![
            WidgetRecord {
                id: Uuid::new_v4(),
                tenant_id: String::from("default"),
                dashboard_id: dash_id,
                widget_kind: WidgetKind::KpiCard,
                title: String::from("失效指标"),
                position: WidgetPosition {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 3,
                },
                data_source: WidgetDataSource::Metric {
                    metric: String::from("missing_gmv"),
                    dimensions: Vec::new(),
                    time_grain: None,
                    time_range: None,
                },
                encoding: serde_json::json!({}),
                options: serde_json::json!({}),
                drill: Vec::new(),
                refresh_mode: RefreshMode::Live,
                refresh_seconds: 60,
                deps: WidgetDeps::default(),
                created_at: now,
                updated_at: now,
            },
            WidgetRecord {
                id: Uuid::new_v4(),
                tenant_id: String::from("default"),
                dashboard_id: dash_id,
                widget_kind: WidgetKind::SimCompare,
                title: String::from("单场景对比"),
                position: WidgetPosition {
                    x: 4,
                    y: 0,
                    w: 10,
                    h: 4,
                },
                data_source: WidgetDataSource::SimScenarios {
                    scenario_ids: vec![s1],
                    metrics: Vec::new(),
                },
                encoding: serde_json::json!({}),
                options: serde_json::json!({}),
                drill: Vec::new(),
                refresh_mode: RefreshMode::Manual,
                refresh_seconds: 60,
                deps: WidgetDeps::default(),
                created_at: now,
                updated_at: now,
            },
        ];

        let mut scenarios = BTreeMap::new();
        scenarios.insert(
            s1,
            ScenarioPreflightState {
                status: ScenarioStatus::Succeeded,
                is_stale: false,
            },
        );
        let detail = make_dashboard(CanvasConfig::standard_five_zone(), widgets);
        let report = evaluate_dashboard_preflight(
            &detail,
            &DashboardPreflightContext {
                scenarios,
                ..DashboardPreflightContext::default()
            },
        );
        assert_eq!(report.status, DashboardPreflightStatus::Blocked);
        let codes: Vec<WidgetIssueCode> = report.issues.iter().map(|i| i.code).collect();
        assert!(codes.contains(&WidgetIssueCode::BrokenMetric));
        assert!(codes.contains(&WidgetIssueCode::LayoutOutOfBounds));
        assert!(codes.contains(&WidgetIssueCode::InsufficientScenarios));
    }

    #[test]
    fn unauthorized_dataset_and_missing_saved_query_are_blocked() {
        let now = Utc::now();
        let dash_id = Uuid::new_v4();
        let secret_ds = Uuid::new_v4();
        let sq_id = Uuid::new_v4();
        let sql_wid = Uuid::new_v4();
        let widgets = vec![
            WidgetRecord {
                id: Uuid::new_v4(),
                tenant_id: String::from("default"),
                dashboard_id: dash_id,
                widget_kind: WidgetKind::Table,
                title: String::from("缺失查询"),
                position: WidgetPosition {
                    x: 0,
                    y: 0,
                    w: 6,
                    h: 4,
                },
                data_source: WidgetDataSource::SavedQuery {
                    saved_query_id: sq_id,
                },
                encoding: serde_json::json!({}),
                options: serde_json::json!({}),
                drill: Vec::new(),
                refresh_mode: RefreshMode::Live,
                refresh_seconds: 60,
                deps: WidgetDeps::default(),
                created_at: now,
                updated_at: now,
            },
            WidgetRecord {
                id: sql_wid,
                tenant_id: String::from("default"),
                dashboard_id: dash_id,
                widget_kind: WidgetKind::BarChart,
                title: String::from("越权查询"),
                position: WidgetPosition {
                    x: 6,
                    y: 0,
                    w: 6,
                    h: 4,
                },
                data_source: WidgetDataSource::Sql {
                    sql: String::from("SELECT * FROM secret_payroll"),
                },
                encoding: serde_json::json!({}),
                options: serde_json::json!({}),
                drill: Vec::new(),
                refresh_mode: RefreshMode::Live,
                refresh_seconds: 60,
                deps: WidgetDeps {
                    dataset_ids: vec![secret_ds],
                    ..WidgetDeps::default()
                },
                created_at: now,
                updated_at: now,
            },
        ];

        let detail = make_dashboard(CanvasConfig::default(), widgets);
        let report = evaluate_dashboard_preflight(
            &detail,
            &DashboardPreflightContext {
                existing_dataset_ids: BTreeSet::from([secret_ds]),
                unauthorized_dataset_ids: BTreeSet::from([secret_ds]),
                ..DashboardPreflightContext::default()
            },
        );
        assert_eq!(report.status, DashboardPreflightStatus::Blocked);
        let codes: Vec<WidgetIssueCode> = report.issues.iter().map(|i| i.code).collect();
        assert!(codes.contains(&WidgetIssueCode::MissingSavedQuery));
        assert!(codes.contains(&WidgetIssueCode::UnauthorizedTarget));
    }

    #[test]
    fn action_data_source_accepts_action_schema_alias() {
        let parsed = serde_json::from_value::<WidgetDataSource>(serde_json::json!({
            "kind": "ACTION",
            "action_type": "RUN_FLOW",
            "flow_id": null,
            "action_schema": { "type": "string" }
        }));
        assert!(parsed.is_ok());
        if let Ok(WidgetDataSource::Action { input_schema, .. }) = parsed {
            assert!(!is_valid_action_object_schema(&input_schema));
        }
    }
}
