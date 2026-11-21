//! `xBlueView` digital-twin dashboard, widget data source, spatial binding, and live event contracts (`10-STEP11-12`).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::analytics::QueryColumn;
use crate::enums::{AssetKind, DataType, WidgetKind};

/// Dashboard visual theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DashboardTheme {
    /// Light minimalist theme.
    #[default]
    Light,
    /// Dark minimalist theme.
    Dark,
    /// Full-bleed dark big-screen mode.
    Screen,
}

impl DashboardTheme {
    /// Returns the PostgreSQL CHECK-constraint string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Screen => "screen",
        }
    }

    /// Parses a stored dashboard theme string.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Self {
        match raw {
            "dark" => Self::Dark,
            "screen" => Self::Screen,
            _ => Self::Light,
        }
    }
}

/// Dashboard lifecycle publication status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DashboardStatus {
    /// Editable draft dashboard.
    #[default]
    Draft,
    /// Published dashboard visible to viewers.
    Published,
}

impl DashboardStatus {
    /// Returns the PostgreSQL CHECK-constraint string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Published => "PUBLISHED",
        }
    }

    /// Parses a stored dashboard status string.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Self {
        match raw {
            "PUBLISHED" => Self::Published,
            _ => Self::Draft,
        }
    }
}

/// Widget refresh policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RefreshMode {
    /// Re-fetches automatically on SSE invalidation events.
    #[default]
    Live,
    /// Polls periodically every `refresh_seconds`.
    Interval,
    /// Refreshes only on explicit user action.
    Manual,
}

impl RefreshMode {
    /// Returns the PostgreSQL CHECK-constraint string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "LIVE",
            Self::Interval => "INTERVAL",
            Self::Manual => "MANUAL",
        }
    }

    /// Parses a stored refresh mode string.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Self {
        match raw {
            "INTERVAL" => Self::Interval,
            "MANUAL" => Self::Manual,
            _ => Self::Live,
        }
    }
}

/// Global dashboard filter input control type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FilterKind {
    /// Single-select dropdown filter.
    Select,
    /// Multi-select dropdown filter.
    MultiSelect,
    /// Closed date range (`[start, end]`).
    DateRange,
    /// Closed numeric range (`[min, max]`).
    NumberRange,
    /// Substring text filter.
    Text,
}

/// Action mode supported by `ACTION_BUTTON` and `ACTION_FORM` widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WidgetActionType {
    /// Triggers the `PROD` deployment of an `AxFlow` workflow.
    RunFlow,
    /// Writes a property or metric back onto a business node (`xb_nodes`).
    WriteNode,
}

/// Target field category when `action_type = WRITE_NODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WriteNodeTargetKind {
    /// Updates a static property in `xb_nodes.properties`.
    Property,
    /// Updates a metric in `xb_nodes.metrics` and `xb_metric_series`.
    Metric,
}

/// Response mode returned by `POST /api/xblueview/widgets/{id}/invoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InvokeWidgetActionMode {
    /// Started an `AxFlow` run.
    Flow,
    /// Executed an atomic node property/metric writeback.
    WriteNode,
}

/// Dashboard grid canvas geometry and big-screen reference resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasConfig {
    /// Grid column count (fixed 24 for standard dashboards).
    #[serde(default = "default_canvas_cols")]
    pub cols: i32,
    /// Grid row unit height in pixels.
    #[serde(default = "default_row_height")]
    pub row_height: i32,
    /// Reference width in pixels for `?screen=1` scaled view.
    #[serde(default = "default_screen_width")]
    pub screen_width: i32,
    /// Reference height in pixels for `?screen=1` scaled view.
    #[serde(default = "default_screen_height")]
    pub screen_height: i32,
}

const fn default_canvas_cols() -> i32 {
    24
}

const fn default_row_height() -> i32 {
    30
}

const fn default_screen_width() -> i32 {
    1920
}

const fn default_screen_height() -> i32 {
    1080
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            cols: default_canvas_cols(),
            row_height: default_row_height(),
            screen_width: default_screen_width(),
            screen_height: default_screen_height(),
        }
    }
}

/// Widget grid placement coordinates (`{x, y, w, h}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetPosition {
    /// Zero-based horizontal column offset (`0..24`).
    pub x: i32,
    /// Zero-based vertical row offset (`>= 0`).
    pub y: i32,
    /// Column span (`>= 1` and `x + w <= 24`).
    pub w: i32,
    /// Row span (`>= 1`).
    pub h: i32,
}

/// Single drill-down hierarchy level configured on a widget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrillLevel {
    /// Grouping column name for this drill depth.
    pub field: String,
    /// Optional display label for the breadcrumb.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional business object type opened in `NodeDrawer` at leaf click.
    #[serde(default)]
    pub node_type: Option<String>,
    /// Optional result column supplying the `node_key` for `NodeDrawer`.
    #[serde(default)]
    pub node_key_field: Option<String>,
}

/// Global dashboard filter definition (`xv_dashboards.filters`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterDef {
    /// Unique filter key within the dashboard.
    pub key: String,
    /// Human-readable label shown in the filter bar.
    pub label: String,
    /// Input control type.
    pub kind: FilterKind,
    /// Mapping from widget UUID string to target column name.
    #[serde(default)]
    pub field_map: BTreeMap<String, String>,
    /// Optional dynamic options query source.
    #[serde(default)]
    pub options_source: Option<Box<WidgetDataSource>>,
    /// Optional default filter value.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// Precomputed dependency graph used for GIN-indexed SSE cache invalidation (`xv_widgets.deps`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetDeps {
    /// Referenced `ds_datasets` UUIDs (from SQL/saved-query/metric lake tables).
    #[serde(default)]
    pub dataset_ids: Vec<Uuid>,
    /// Referenced business node `type_name`s.
    #[serde(default)]
    pub type_names: Vec<String>,
    /// Referenced metric keys.
    #[serde(default)]
    pub metrics: Vec<String>,
    /// Whether this widget subscribes to alert open/resolve events.
    #[serde(default)]
    pub alerts: bool,
    /// Referenced `AxSim` scenario UUIDs.
    #[serde(default)]
    pub scenario_ids: Vec<Uuid>,
}

fn default_action_schema() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

fn deserialize_opt_uuid<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Uuid::parse_str(trimmed)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

/// Declarative widget data source union (`xv_widgets.data_source`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WidgetDataSource {
    /// Executes a saved analytical query (`an_saved_queries`).
    SavedQuery {
        /// Saved query UUID.
        saved_query_id: Uuid,
    },
    /// Executes a semantic-layer metric query (`an_metrics`).
    Metric {
        /// Certified metric name.
        metric: String,
        /// Grouping dimension columns.
        #[serde(default)]
        dimensions: Vec<String>,
        /// Optional time bucket grain (`DAY`, `WEEK`, `MONTH`, `YEAR`).
        #[serde(default)]
        time_grain: Option<String>,
        /// Optional relative time range preset (e.g., `LAST_30_DAYS`).
        #[serde(default)]
        time_range: Option<String>,
    },
    /// Executes a read-only `DuckDB` SQL query via `sql_guard`.
    Sql {
        /// SQL `SELECT` statement.
        sql: String,
    },
    /// Queries time-series node metric rollups (`/api/xblue/metrics/window`).
    NodeMetric {
        /// Business node type name.
        type_name: String,
        /// Metric key.
        metric: String,
        /// Aggregation function (`SUM`, `AVG`, `MIN`, `MAX`, `LAST`, `COUNT`).
        #[serde(default)]
        agg: Option<String>,
        /// Time rollup grain (`MINUTE`, `HOUR`, `DAY`, `MONTH`).
        #[serde(default)]
        grain: Option<String>,
        /// Relative range preset (e.g., `LAST_7_DAYS`).
        #[serde(default)]
        time_range: Option<String>,
        /// Grouping dimension (`node` or `time`).
        #[serde(default)]
        group_by: Option<String>,
    },
    /// Executes an OLAP pivot query (`PivotSpec`).
    Pivot {
        /// Step 04 `PivotSpec` JSON payload.
        spec: serde_json::Value,
    },
    /// Loads a business graph subgraph with optional metric coloring.
    Graph {
        /// Filter to specific node types.
        #[serde(default)]
        type_names: Vec<String>,
        /// Filter to specific relationship types.
        #[serde(default)]
        link_types: Vec<String>,
        /// Node metric key used for color encoding.
        #[serde(default)]
        color_metric: Option<String>,
        /// Maximum nodes returned (default 2000).
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Lists operational alerts (`ax_alerts`).
    Alerts {
        /// Alert statuses (`OPEN`, `ACK`, `RESOLVED`).
        #[serde(default)]
        status: Vec<String>,
        /// Severities (`INFO`, `WARN`, `CRITICAL`).
        #[serde(default)]
        severity: Vec<String>,
        /// Maximum alert rows returned.
        #[serde(default)]
        limit: Option<i64>,
    },
    /// Compares multiple `AxSim` scenarios side by side.
    SimScenarios {
        /// Scenario UUIDs (`2..=5`).
        #[serde(default)]
        scenario_ids: Vec<Uuid>,
        /// Optional metric filter list.
        #[serde(default)]
        metrics: Vec<String>,
    },
    /// Loads geo-located business nodes (`geo_lng`, `geo_lat`) and optional links.
    NodesGeo {
        /// Node types to include.
        #[serde(default)]
        type_names: Vec<String>,
        /// Node metric key used for marker color.
        #[serde(default)]
        color_metric: Option<String>,
        /// Optional link types for fly-lines.
        #[serde(default)]
        link_types: Vec<String>,
    },
    /// Loads 2D floor plan node placements joined with live metric values.
    FloorPlan {
        /// Floor plan image asset UUID (`FLOOR_PLAN_IMAGE`).
        asset_id: Uuid,
        /// Metric key joined onto placed nodes.
        #[serde(default)]
        color_metric: Option<String>,
    },
    /// Loads 3D twin bindings joined with live node metric values and open alert counts.
    #[serde(rename = "SCENE_3D", alias = "SCENE3D")]
    Scene3d {
        /// 3D model asset UUID (`MODEL_3D`).
        asset_id: Uuid,
        /// Metric key joined onto bound nodes/meshes.
        #[serde(default)]
        color_metric: Option<String>,
    },
    /// Interactive action widget (`ACTION_BUTTON` / `ACTION_FORM`); `/data` returns recent logs.
    Action {
        /// Action mode (`RUN_FLOW` or `WRITE_NODE`).
        action_type: WidgetActionType,
        /// Target workflow UUID when `action_type = RUN_FLOW`.
        #[serde(default, deserialize_with = "deserialize_opt_uuid")]
        flow_id: Option<Uuid>,
        /// Default target node UUID when `action_type = WRITE_NODE`.
        #[serde(default, deserialize_with = "deserialize_opt_uuid")]
        node_id: Option<Uuid>,
        /// Node field target (`PROPERTY` or `METRIC`) when `action_type = WRITE_NODE`.
        #[serde(default)]
        target_kind: Option<WriteNodeTargetKind>,
        /// Target property or metric key when `action_type = WRITE_NODE`.
        #[serde(default)]
        key: Option<String>,
        /// JSON Schema validating invocation `inputs`.
        #[serde(default = "default_action_schema")]
        input_schema: serde_json::Value,
        /// Optional confirmation prompt before execution.
        #[serde(default)]
        confirm_text: Option<String>,
    },
    /// Static inline rows or Markdown content without upstream query.
    Static {
        /// Optional static column metadata.
        #[serde(default)]
        columns: Vec<QueryColumn>,
        /// Inline tabular rows.
        #[serde(default)]
        rows: Vec<Vec<serde_json::Value>>,
    },
}

impl Default for WidgetDataSource {
    fn default() -> Self {
        Self::Static {
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }
}

/// Stored dashboard record (`xv_dashboards`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardRecord {
    /// Dashboard UUID.
    pub id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Human-readable dashboard title.
    pub name: String,
    /// URL-safe slug (`^[a-z0-9][a-z0-9-]{0,62}$`).
    pub slug: String,
    /// Dashboard description.
    pub description: String,
    /// Global filter definitions.
    pub filters: Vec<FilterDef>,
    /// Visual theme (`light`, `dark`, `screen`).
    pub theme: DashboardTheme,
    /// Grid & big-screen canvas settings.
    pub canvas: CanvasConfig,
    /// Lifecycle publication status (`DRAFT`, `PUBLISHED`).
    pub status: DashboardStatus,
    /// Published version counter.
    pub version: i32,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Stored widget record (`xv_widgets`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetRecord {
    /// Widget UUID.
    pub id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Parent dashboard UUID.
    pub dashboard_id: Uuid,
    /// Visual widget category (`WidgetKind`).
    pub widget_kind: WidgetKind,
    /// Widget header title.
    pub title: String,
    /// Grid position `{x, y, w, h}`.
    pub position: WidgetPosition,
    /// Declarative data source definition.
    pub data_source: WidgetDataSource,
    /// Field-to-visual-channel encoding map.
    pub encoding: serde_json::Value,
    /// Thresholds, formatting, `ECharts` overrides, cross-filter config.
    pub options: serde_json::Value,
    /// Ordered drill-down levels.
    pub drill: Vec<DrillLevel>,
    /// Refresh trigger policy (`LIVE`, `INTERVAL`, `MANUAL`).
    pub refresh_mode: RefreshMode,
    /// Polling interval in seconds (`5..=86400`) for `INTERVAL` mode.
    pub refresh_seconds: i32,
    /// Precomputed dependency metadata for event invalidation.
    pub deps: WidgetDeps,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Hydrated dashboard detail including ordered widgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardDetail {
    /// Dashboard metadata.
    #[serde(flatten)]
    pub dashboard: DashboardRecord,
    /// Ordered widgets belonging to this dashboard.
    pub widgets: Vec<WidgetRecord>,
}

/// Request payload for `POST /api/xblueview/dashboards`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDashboardRequest {
    /// Dashboard display name.
    pub name: String,
    /// URL slug (`^[a-z0-9][a-z0-9-]{0,62}$`).
    pub slug: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Optional global filters.
    #[serde(default)]
    pub filters: Vec<FilterDef>,
    /// Optional visual theme.
    #[serde(default)]
    pub theme: DashboardTheme,
    /// Optional grid canvas geometry.
    #[serde(default)]
    pub canvas: CanvasConfig,
}

/// Request payload for `PUT /api/xblueview/dashboards/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDashboardRequest {
    /// Updated display name.
    pub name: Option<String>,
    /// Updated URL slug.
    pub slug: Option<String>,
    /// Updated description.
    pub description: Option<String>,
    /// Replaced global filter definitions.
    pub filters: Option<Vec<FilterDef>>,
    /// Updated visual theme.
    pub theme: Option<DashboardTheme>,
    /// Updated canvas configuration.
    pub canvas: Option<CanvasConfig>,
}

/// Request payload for `POST /api/xblueview/dashboards/{id}/widgets`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWidgetRequest {
    /// Visual widget kind.
    pub widget_kind: WidgetKind,
    /// Display title.
    #[serde(default)]
    pub title: String,
    /// Grid layout coordinates `{x, y, w, h}`.
    pub position: WidgetPosition,
    /// Data source configuration.
    #[serde(default)]
    pub data_source: WidgetDataSource,
    /// Visual channel mapping JSON.
    #[serde(default = "default_empty_obj")]
    pub encoding: serde_json::Value,
    /// Thresholds/formatting/options JSON.
    #[serde(default = "default_empty_obj")]
    pub options: serde_json::Value,
    /// Drill-down hierarchy.
    #[serde(default)]
    pub drill: Vec<DrillLevel>,
    /// Refresh mode.
    #[serde(default)]
    pub refresh_mode: RefreshMode,
    /// Polling interval seconds (`5..=86400`).
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: i32,
}

/// Request payload for `PUT /api/xblueview/widgets/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateWidgetRequest {
    /// Updated widget kind.
    pub widget_kind: Option<WidgetKind>,
    /// Updated title.
    pub title: Option<String>,
    /// Updated position.
    pub position: Option<WidgetPosition>,
    /// Updated data source.
    pub data_source: Option<WidgetDataSource>,
    /// Updated visual encoding.
    pub encoding: Option<serde_json::Value>,
    /// Updated options.
    pub options: Option<serde_json::Value>,
    /// Updated drill-down hierarchy.
    pub drill: Option<Vec<DrillLevel>>,
    /// Updated refresh mode.
    pub refresh_mode: Option<RefreshMode>,
    /// Updated polling interval seconds.
    pub refresh_seconds: Option<i32>,
}

fn default_empty_obj() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

const fn default_refresh_seconds() -> i32 {
    60
}

/// Single widget layout adjustment item (`PUT /api/xblueview/dashboards/{id}/layout`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutUpdateItem {
    /// Target widget UUID.
    pub id: Uuid,
    /// New grid coordinates.
    pub position: WidgetPosition,
}

/// Batch widget layout update request (`PUT /api/xblueview/dashboards/{id}/layout`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateLayoutRequest {
    /// Ordered position updates for widgets in the dashboard.
    pub widgets: Vec<LayoutUpdateItem>,
}

/// Request payload for `POST /api/xblueview/dashboards/ai-generate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiGenerateDashboardRequest {
    /// Natural-language prompt describing desired monitoring cockpit.
    pub prompt: String,
    /// Optional explicit title override.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional explicit slug override.
    #[serde(default)]
    pub slug: Option<String>,
    /// Optional theme override.
    #[serde(default)]
    pub theme: Option<DashboardTheme>,
}

/// Widget draft item emitted by AI dashboard generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiWidgetDraft {
    /// Visual widget kind.
    pub widget_kind: WidgetKind,
    /// Widget header title.
    pub title: String,
    /// Grid coordinates within 24-column layout.
    pub position: WidgetPosition,
    /// Widget data source definition.
    pub data_source: WidgetDataSource,
    /// Visual encoding map.
    #[serde(default = "default_empty_obj")]
    pub encoding: serde_json::Value,
    /// Display options map.
    #[serde(default = "default_empty_obj")]
    pub options: serde_json::Value,
    /// Drill-down hierarchy.
    #[serde(default)]
    pub drill: Vec<DrillLevel>,
}

/// Complete dashboard draft schema emitted by `LlmRouter` for AI generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiDashboardDraft {
    /// Suggested dashboard title.
    pub name: String,
    /// Suggested URL slug (`^[a-z0-9][a-z0-9-]{0,62}$`).
    pub slug: String,
    /// Suggested theme.
    #[serde(default)]
    pub theme: DashboardTheme,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Global filters.
    #[serde(default)]
    pub filters: Vec<FilterDef>,
    /// Drafted widgets.
    #[serde(default)]
    pub widgets: Vec<AiWidgetDraft>,
}

/// Active drill-down breadcrumb predicate passed to widget execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrillPathItem {
    /// Grouping column name clicked at that level.
    pub field: String,
    /// Selected key value.
    pub value: serde_json::Value,
}

/// Request payload for `POST /api/xblueview/widgets/{id}/data`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetDataRequest {
    /// Active global filter values keyed by dashboard filter key.
    #[serde(default)]
    pub filters: BTreeMap<String, serde_json::Value>,
    /// Active drill-down selection stack.
    #[serde(default)]
    pub drill_path: Vec<DrillPathItem>,
}

/// Request payload for `POST /api/xblueview/widgets/preview` (unsaved widget).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetPreviewRequest {
    /// Visual widget kind.
    pub widget_kind: WidgetKind,
    /// Candidate data source.
    pub data_source: WidgetDataSource,
    /// Visual encoding options.
    #[serde(default = "default_empty_obj")]
    pub encoding: serde_json::Value,
    /// Display options.
    #[serde(default = "default_empty_obj")]
    pub options: serde_json::Value,
    /// Drill-down hierarchy.
    #[serde(default)]
    pub drill: Vec<DrillLevel>,
    /// Active filters map (treated as direct column filters in preview).
    #[serde(default)]
    pub filters: BTreeMap<String, serde_json::Value>,
    /// Active drill path.
    #[serde(default)]
    pub drill_path: Vec<DrillPathItem>,
}

/// Column metadata element returned inside `WidgetDataResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetColumn {
    /// Column name.
    pub name: String,
    /// Logical data type.
    pub data_type: DataType,
}

/// Unified tabular response payload returned by widget execution endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetDataResponse {
    /// Output column definitions.
    pub columns: Vec<WidgetColumn>,
    /// Tabular row values aligned with `columns`.
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Number of returned rows.
    pub row_count: usize,
    /// Whether server-side row cap truncated additional rows.
    pub truncated: bool,
    /// Generation timestamp (UTC).
    pub generated_at: DateTime<Utc>,
    /// Version fingerprint for cache/refresh diagnostics.
    pub data_version: String,
    /// Current active drill depth (`drill_path.len()`).
    pub drill_level: usize,
}

/// Request payload for `POST /api/xblueview/widgets/{id}/invoke`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeWidgetActionRequest {
    /// Action input arguments validated against `data_source.input_schema`.
    #[serde(default = "default_empty_obj")]
    pub inputs: serde_json::Value,
    /// Optional target node override (`WRITE_NODE` or `RUN_FLOW` context).
    #[serde(default, deserialize_with = "deserialize_opt_uuid")]
    pub target_node_id: Option<Uuid>,
    /// Optional client idempotency key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Response payload for `POST /api/xblueview/widgets/{id}/invoke`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWidgetActionResponse {
    /// Execution branch (`FLOW` or `WRITE_NODE`).
    pub mode: InvokeWidgetActionMode,
    /// Started workflow run UUID when `mode = FLOW`.
    pub run_id: Option<Uuid>,
    /// Execution status (`PENDING`, `RUNNING`, `DONE`, etc.).
    pub status: String,
    /// Updated node UUID when `mode = WRITE_NODE`.
    pub node_id: Option<Uuid>,
    /// Recorded `ax_action_log.id` when `mode = WRITE_NODE`.
    pub action_log_id: Option<i64>,
    /// `true` when replayed from an existing `(tenant_id, idempotency_key)` log.
    pub idempotency_replay: bool,
}

/// Stored visual asset record (`xv_assets`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRecord {
    /// Asset UUID.
    pub id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Asset category (`FLOOR_PLAN_IMAGE`, `GEOJSON`, `ICON`, `MODEL_3D`).
    pub asset_kind: AssetKind,
    /// Unique asset name within the tenant.
    pub name: String,
    /// Stored filesystem path relative/absolute under `AXPOINT_DATA_ROOT`.
    pub file_path: String,
    /// Validated MIME content type.
    pub mime_type: String,
    /// Lowercase hex SHA-256 checksum.
    pub sha256: String,
    /// Optional image width in pixels.
    pub width_px: Option<i32>,
    /// Optional image height in pixels.
    pub height_px: Option<i32>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// 2D floor-plan node placement record (`xv_floor_placements`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloorPlacementRecord {
    /// Parent floor-plan asset UUID.
    pub asset_id: Uuid,
    /// Placed business node UUID.
    pub node_id: Uuid,
    /// Normalized horizontal coordinate (`0.0..=1.0`).
    pub x_ratio: f64,
    /// Normalized vertical coordinate (`0.0..=1.0`).
    pub y_ratio: f64,
}

/// Placement item in `PUT /api/xblueview/assets/{id}/placements`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutFloorPlacementItem {
    /// Placed business node UUID.
    pub node_id: Uuid,
    /// Normalized horizontal coordinate (`0.0..=1.0`).
    pub x_ratio: f64,
    /// Normalized vertical coordinate (`0.0..=1.0`).
    pub y_ratio: f64,
}

/// Request payload for `PUT /api/xblueview/assets/{id}/placements`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutFloorPlacementsRequest {
    /// Complete replacement list of node placements for this asset.
    pub placements: Vec<PutFloorPlacementItem>,
}

/// 3D twin scene node binding record (`xv_scene3d_bindings`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene3dBindingRecord {
    /// Parent `MODEL_3D` asset UUID.
    pub asset_id: Uuid,
    /// Bound business node UUID.
    pub node_id: Uuid,
    /// Named `glTF` mesh/node name (empty string uses marker stake at `(pos_x, pos_y, pos_z)`).
    pub mesh_name: String,
    /// X offset in scene coordinates.
    pub pos_x: f64,
    /// Y offset in scene coordinates.
    pub pos_y: f64,
    /// Z offset in scene coordinates.
    pub pos_z: f64,
    /// Uniform marker scale (`> 0`).
    pub scale: f64,
}

/// Binding item in `PUT /api/xblueview/assets/{id}/bindings3d`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutScene3dBindingItem {
    /// Bound business node UUID.
    pub node_id: Uuid,
    /// Named `glTF` mesh/node name (empty string for stake coordinates).
    #[serde(default)]
    pub mesh_name: String,
    /// X position.
    #[serde(default)]
    pub pos_x: f64,
    /// Y position.
    #[serde(default)]
    pub pos_y: f64,
    /// Z position.
    #[serde(default)]
    pub pos_z: f64,
    /// Marker scale (`> 0`, defaults to `1.0`).
    #[serde(default = "default_scene_scale")]
    pub scale: f64,
}

const fn default_scene_scale() -> f64 {
    1.0
}

/// Request payload for `PUT /api/xblueview/assets/{id}/bindings3d`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutScene3dBindingsRequest {
    /// Complete replacement list of 3D node bindings for this asset.
    pub bindings: Vec<PutScene3dBindingItem>,
}

/// Geo-node row returned for `NODES_GEO` map widgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodesGeoRow {
    /// Business node UUID.
    pub node_id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Node title.
    pub title: String,
    /// Longitude.
    pub lng: f64,
    /// Latitude.
    pub lat: f64,
    /// Optional metric value for `color_metric`.
    pub metric_value: Option<f64>,
    /// Active open alert count on this node.
    pub open_alerts: i64,
}

/// Floor-plan node row returned for `FLOOR_PLAN` widgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloorPlanRow {
    /// Business node UUID.
    pub node_id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Node title.
    pub title: String,
    /// Normalized X coordinate (`0.0..=1.0`).
    pub x_ratio: f64,
    /// Normalized Y coordinate (`0.0..=1.0`).
    pub y_ratio: f64,
    /// Optional metric value for `color_metric`.
    pub metric_value: Option<f64>,
    /// Active open alert count on this node.
    pub open_alerts: i64,
}

/// 3D twin row returned for `SCENE_3D` widgets (`10-STEP11-12` §11.2.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene3dRow {
    /// Bound business node UUID.
    pub node_id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business key.
    pub node_key: String,
    /// Node title.
    pub title: String,
    /// Bound `glTF` mesh name.
    pub mesh_name: String,
    /// Scene X position.
    pub pos_x: f64,
    /// Scene Y position.
    pub pos_y: f64,
    /// Scene Z position.
    pub pos_z: f64,
    /// Marker scale.
    pub scale: f64,
    /// Optional metric value for `color_metric`.
    pub metric_value: Option<f64>,
    /// Active open alert count on this node.
    pub open_alerts: i64,
}

/// Input parameters for transactional node writeback from an `xBlueView` action widget.
#[derive(Debug, Clone)]
pub struct ViewWriteNodeParams {
    /// Target widget UUID (recorded in `ax_action_log.target`).
    pub widget_id: Uuid,
    /// Target business node UUID.
    pub node_id: Uuid,
    /// Write target category (`PROPERTY` or `METRIC`).
    pub target_kind: WriteNodeTargetKind,
    /// Property or metric key declared in the published main `NodeTypeSpec`.
    pub key: String,
    /// Value to write (numeric for `METRIC`, typed JSON value for `PROPERTY`).
    pub value: serde_json::Value,
    /// Idempotency key unique within the tenant.
    pub idempotency_key: String,
    /// Optional caller department for ABAC validation (`dept_id`).
    pub caller_dept_id: String,
    /// Optional numeric value extracted when `target_kind = METRIC`.
    pub numeric_value: Option<f64>,
}

/// Transactional outcome returned by `execute_view_write_node`.
#[derive(Debug, Clone)]
pub struct ViewWriteNodeOutcome {
    /// Updated node UUID.
    pub node_id: Uuid,
    /// Node `type_name`.
    pub type_name: String,
    /// Recorded `ax_action_log.id`.
    pub action_log_id: i64,
    /// `true` if replayed idempotently without re-applying side effects.
    pub idempotency_replay: bool,
}

/// Real-time `xBlueView` event distributed across instances via `pg_notify('ax_view_events', ...)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewEvent {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Triggering outbox topic string (e.g., `dataset.synced`, `alert.opened`).
    pub topic: String,
    /// Affected widget UUIDs (may be empty if compacted to `dashboard_ids` due to payload size).
    #[serde(default)]
    pub widget_ids: Vec<Uuid>,
    /// Affected dashboard UUIDs.
    #[serde(default)]
    pub dashboard_ids: Vec<Uuid>,
    /// Optional alert payload forwarded on `event: alert` SSE frames.
    #[serde(default)]
    pub alert: Option<serde_json::Value>,
}

/// Result of looking up affected widgets from GIN-indexed `xv_widgets.deps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffectedWidgetRef {
    /// Widget UUID.
    pub widget_id: Uuid,
    /// Parent dashboard UUID.
    pub dashboard_id: Uuid,
}

/// Input parameters for creating a visual asset record in `xv_assets`.
#[derive(Debug, Clone)]
pub struct CreateAssetParams {
    /// Asset UUID (pre-generated to name the file on disk cleanly).
    pub id: Uuid,
    /// Asset category.
    pub asset_kind: AssetKind,
    /// Tenant-unique asset name.
    pub name: String,
    /// Relative or canonical storage path under `AXPOINT_DATA_ROOT`.
    pub file_path: String,
    /// Validated MIME type.
    pub mime_type: String,
    /// Hex SHA-256 digest.
    pub sha256: String,
    /// Optional raster width.
    pub width_px: Option<i32>,
    /// Optional raster height.
    pub height_px: Option<i32>,
}

/// Input parameters for persisting a new widget with computed `deps`.
#[derive(Debug, Clone)]
pub struct CreateWidgetRecordParams {
    /// Parent dashboard UUID.
    pub dashboard_id: Uuid,
    /// Visual widget kind.
    pub widget_kind: WidgetKind,
    /// Display title.
    pub title: String,
    /// Grid position.
    pub position: WidgetPosition,
    /// Data source definition.
    pub data_source: WidgetDataSource,
    /// Visual encoding map.
    pub encoding: serde_json::Value,
    /// Display options map.
    pub options: serde_json::Value,
    /// Drill-down hierarchy.
    pub drill: Vec<DrillLevel>,
    /// Refresh mode.
    pub refresh_mode: RefreshMode,
    /// Refresh interval in seconds.
    pub refresh_seconds: i32,
    /// Precomputed dependency sets.
    pub deps: WidgetDeps,
}

/// Input parameters for updating an existing widget.
#[derive(Debug, Clone, Default)]
pub struct UpdateWidgetRecordParams {
    /// Updated widget kind.
    pub widget_kind: Option<WidgetKind>,
    /// Updated title.
    pub title: Option<String>,
    /// Updated grid position.
    pub position: Option<WidgetPosition>,
    /// Updated data source.
    pub data_source: Option<WidgetDataSource>,
    /// Updated visual encoding.
    pub encoding: Option<serde_json::Value>,
    /// Updated options.
    pub options: Option<serde_json::Value>,
    /// Updated drill-down hierarchy.
    pub drill: Option<Vec<DrillLevel>>,
    /// Updated refresh mode.
    pub refresh_mode: Option<RefreshMode>,
    /// Updated polling interval seconds.
    pub refresh_seconds: Option<i32>,
    /// Recomputed dependencies when `data_source` changes.
    pub deps: Option<WidgetDeps>,
}

/// Directed geo-located relationship row used to render map fly-lines (`NODES_GEO.link_types`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodesGeoLinkRow {
    /// Link UUID (`xb_links.id`).
    pub link_id: Uuid,
    /// Relationship type name (`xb_links.link_type`).
    pub link_type: String,
    /// Source business node UUID.
    pub src_node_id: Uuid,
    /// Destination business node UUID.
    pub dst_node_id: Uuid,
    /// Source node longitude.
    pub src_lng: f64,
    /// Source node latitude.
    pub src_lat: f64,
    /// Destination node longitude.
    pub dst_lng: f64,
    /// Destination node latitude.
    pub dst_lat: f64,
    /// Relationship weight.
    pub weight: f64,
}

#[cfg(test)]
mod tests {
    use super::{InvokeWidgetActionRequest, WidgetDataSource};
    use uuid::Uuid;

    #[test]
    fn scene3d_data_source_serializes_as_scene_3d_and_accepts_legacy_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        let asset_id = Uuid::nil();
        let ds = WidgetDataSource::Scene3d {
            asset_id,
            color_metric: Some("inventory".to_owned()),
        };
        let encoded = serde_json::to_value(&ds)?;
        assert_eq!(
            encoded.get("kind").and_then(serde_json::Value::as_str),
            Some("SCENE_3D")
        );

        let from_canonical: WidgetDataSource = serde_json::from_value(serde_json::json!({
            "kind": "SCENE_3D",
            "asset_id": asset_id,
            "color_metric": "inventory"
        }))?;
        assert!(matches!(from_canonical, WidgetDataSource::Scene3d { .. }));

        let from_legacy: WidgetDataSource = serde_json::from_value(serde_json::json!({
            "kind": "SCENE3D",
            "asset_id": asset_id
        }))?;
        assert!(matches!(from_legacy, WidgetDataSource::Scene3d { .. }));
        Ok(())
    }

    #[test]
    fn action_optional_uuids_treat_empty_strings_as_none() -> Result<(), Box<dyn std::error::Error>>
    {
        let ds: WidgetDataSource = serde_json::from_value(serde_json::json!({
            "kind": "ACTION",
            "action_type": "WRITE_NODE",
            "flow_id": "",
            "node_id": "   "
        }))?;
        assert!(matches!(
            ds,
            WidgetDataSource::Action {
                flow_id: None,
                node_id: None,
                ..
            }
        ));

        let invoke_req: InvokeWidgetActionRequest = serde_json::from_value(serde_json::json!({
            "inputs": {"value": 42},
            "target_node_id": ""
        }))?;
        assert_eq!(invoke_req.target_node_id, None);
        Ok(())
    }
}
