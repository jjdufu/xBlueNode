//! `xBlueView` full-format export document model, export job/template DTOs, and `axs_` secure share contracts (`10-STEP11-12` §12).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::enums::{DataType, ExportFormat, JobStatus, WidgetKind};
use crate::view::{DashboardDetail, DrillPathItem};

/// Threshold above which `SAVED_QUERY` exports spill tabular rows to a temporary `Parquet` file
/// instead of embedding hundreds of thousands of JSON arrays in memory (`D1`).
pub const EXPORT_INLINE_ROW_LIMIT: u32 = 50_000;

/// Maximum rows allowed when exporting a `SAVED_QUERY` to `CSV` or `XLSX` (`10-STEP11-12` §12.2).
pub const EXPORT_SAVED_QUERY_MAX_ROWS: u32 = 1_000_000;

/// Column metadata element inside an [`ExportEmbeddedTable`] or [`ExportSection::Table`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportColumnSpec {
    /// Column display or SQL identifier name.
    pub name: String,
    /// Logical column data type (`DECIMAL` values are transmitted as strings).
    pub data_type: DataType,
    /// Optional spreadsheet/report format pattern (e.g., `"#,##0.00"`, `"0.0%"`, `"yyyy-mm-dd"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Fixed `"TABLE"` discriminator tag for [`ExportEmbeddedTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportTableKind {
    /// Constant `"TABLE"` tag matching Python `ExportTable.kind`.
    #[default]
    #[serde(rename = "TABLE")]
    Table,
}

const fn default_chart_width() -> u32 {
    960
}

const fn default_chart_height() -> u32 {
    540
}

const fn default_heading_level() -> u8 {
    1
}

/// Embedded tabular dataset attached to [`ExportSection::Chart`] for native `PPTX` / `XLSX` charts
/// or standalone table serialization (`kind = "TABLE"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEmbeddedTable {
    /// Discriminator fixed to `"TABLE"` for Python `ExportTable` compatibility.
    #[serde(default)]
    pub kind: ExportTableKind,
    /// Optional table title.
    #[serde(default)]
    pub title: String,
    /// Ordered column specifications.
    pub columns: Vec<ExportColumnSpec>,
    /// Inline row values (`DECIMAL` encoded as strings).
    #[serde(default)]
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Optional summary/totals footer row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<Vec<serde_json::Value>>,
    /// Optional temporary `Parquet` path under `AXPOINT_DATA_ROOT` for streaming `> 50,000` rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows_parquet_path: Option<String>,
    /// Optional per-row kind tags (`DETAIL`, `SUBTOTAL`, `TOTAL`) for `PIVOT` styling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_kinds: Vec<String>,
}

/// Polymorphic section inside an [`ExportDocument`], isomorphic with Python `exporters/document.py` (`10-STEP11-12` §12.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExportSection {
    /// Tabular data section (`XLSX` sheet, `CSV` file, `PDF`/`DOCX`/`PPTX`/`MARKDOWN` table).
    Table {
        /// Section/worksheet title.
        #[serde(default)]
        title: String,
        /// Column specifications.
        columns: Vec<ExportColumnSpec>,
        /// Inline rows (`<= 50,000` rows; empty when `rows_parquet_path` is populated).
        #[serde(default)]
        rows: Vec<Vec<serde_json::Value>>,
        /// Optional totals row.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        totals: Option<Vec<serde_json::Value>>,
        /// Optional temporary `Parquet` file path for streaming large `SAVED_QUERY` exports.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows_parquet_path: Option<String>,
        /// Optional row kind tags (`DETAIL`, `SUBTOTAL`, `TOTAL`) from `PIVOT` queries.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        row_kinds: Vec<String>,
    },
    /// Visual chart section rendered via `ECharts` SSR or native `PPTX` `CategoryChartData`.
    Chart {
        /// Chart title.
        #[serde(default)]
        title: String,
        /// Originating `xBlueView` widget kind.
        widget_kind: WidgetKind,
        /// Pure-JSON `ECharts` option object (no JS functions permitted).
        echarts_option: serde_json::Value,
        /// Optional backing table used for native editable `PPTX` charts and `XLSX` data sheets.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_table: Option<ExportEmbeddedTable>,
        /// Render width in pixels (default `960`).
        #[serde(default = "default_chart_width")]
        width_px: u32,
        /// Render height in pixels (default `540`).
        #[serde(default = "default_chart_height")]
        height_px: u32,
    },
    /// Single KPI metric card section.
    Kpi {
        /// Metric display label.
        label: String,
        /// Formatted primary value string.
        value: String,
        /// Optional period-over-period comparison string.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compare: Option<String>,
    },
    /// Section heading text.
    Heading {
        /// Heading content.
        text: String,
        /// Heading level (`1..=6`).
        #[serde(default = "default_heading_level")]
        level: u8,
    },
    /// Plain paragraph text.
    Paragraph {
        /// Paragraph content.
        text: String,
        /// Nesting level (`1..=6`).
        #[serde(default = "default_heading_level")]
        level: u8,
    },
    /// Rich `GFM` Markdown note block.
    Markdown {
        /// Markdown source text.
        text: String,
        /// Heading offset level (`1..=6`).
        #[serde(default = "default_heading_level")]
        level: u8,
    },
}

/// PDF page size preset (`10-STEP11-12` §12.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportPageSize {
    /// ISO `A4` page size.
    #[default]
    A4,
    /// ISO `A3` page size.
    A3,
}

/// PDF page orientation (`10-STEP11-12` §12.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExportOrientation {
    /// Vertical portrait layout.
    #[default]
    Portrait,
    /// Horizontal landscape layout.
    Landscape,
}

/// Unified document payload sent from `Core` to `Sidecar` `POST /export/{format}` (`10-STEP11-12` §12.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportDocument {
    /// Document/report main title.
    pub title: String,
    /// Optional subtitle.
    #[serde(default)]
    pub subtitle: String,
    /// UTC generation timestamp.
    pub generated_at: DateTime<Utc>,
    /// Creator username (`"dev"` for static dev token).
    pub generated_by: String,
    /// IANA timezone name (e.g., `"Asia/Shanghai"` or `"UTC"`).
    pub timezone: String,
    /// Resolved watermark text (e.g., `"dev 2026-01-31 16:00"`), or `None` when disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<String>,
    /// Ordered report sections.
    #[serde(default)]
    pub sections: Vec<ExportSection>,
    /// Optional filesystem path to an uploaded `DOCX` / `PPTX` user template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_path: Option<String>,
    /// Key-value map replacing `{{key}}` placeholders inside user templates.
    #[serde(default)]
    pub placeholders: BTreeMap<String, String>,
    /// Whether `CSV` output should prepend a `UTF-8` BOM (`\xEF\xBB\xBF`).
    #[serde(default)]
    pub csv_bom: bool,
    /// Page size for `PDF` rendering.
    #[serde(default)]
    pub page_size: ExportPageSize,
    /// Page orientation for `PDF` rendering.
    #[serde(default)]
    pub orientation: ExportOrientation,
}

/// Source specification for `POST /api/xblueview/exports` (`10-STEP11-12` §12.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExportSource {
    /// Exports all static/analytical widgets of a dashboard in `(y, x)` reading order.
    Dashboard {
        /// Target dashboard UUID.
        dashboard_id: Uuid,
        /// Active global filter map keyed by `FilterDef.key`.
        #[serde(default)]
        filters: BTreeMap<String, serde_json::Value>,
    },
    /// Exports a single dashboard widget.
    Widget {
        /// Target widget UUID.
        widget_id: Uuid,
        /// Active global filter map keyed by `FilterDef.key`.
        #[serde(default)]
        filters: BTreeMap<String, serde_json::Value>,
        /// Active drill-down selection stack.
        #[serde(default)]
        drill_path: Vec<DrillPathItem>,
    },
    /// Exports a saved analytical query (`CSV`/`XLSX` up to `1,000,000` rows).
    SavedQuery {
        /// Saved query UUID (`an_saved_queries.id`).
        saved_query_id: Uuid,
    },
    /// Exports an ad-hoc `PivotSpec` preserving subtotal/total formatting.
    Pivot {
        /// Step 04 `PivotSpec` JSON object.
        spec: serde_json::Value,
    },
    /// Exports a side-by-side comparison of `AxSim` scenarios.
    Scenario {
        /// Scenario UUIDs (`1..=5`).
        scenario_ids: Vec<Uuid>,
    },
    /// Exports an `AxAgent` or `AxQuery` conversation transcript and result tables.
    Conversation {
        /// Conversation/session UUID.
        conversation_id: Uuid,
    },
}

/// Watermark option accepting either a boolean toggle or an explicit custom string (`10-STEP11-12` §12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExportWatermarkOption {
    /// `true` auto-generates `"{username} YYYY-MM-DD HH:MM"`; `false` disables watermarking.
    Toggle(bool),
    /// Non-empty string is used verbatim; empty string disables watermarking.
    Text(String),
}

impl Default for ExportWatermarkOption {
    fn default() -> Self {
        Self::Toggle(false)
    }
}

impl ExportWatermarkOption {
    /// Resolves the effective watermark string for `username` at `ts`.
    #[must_use]
    pub fn resolve(&self, username: &str, ts: DateTime<Utc>) -> Option<String> {
        match self {
            Self::Toggle(true) => Some(format!("{} {}", username, ts.format("%Y-%m-%d %H:%M"))),
            Self::Toggle(false) => None,
            Self::Text(raw) => {
                let trimmed = raw.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            }
        }
    }
}

/// Rendering options stored in `xv_export_jobs.options` (`10-STEP11-12` §12.1–§12.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportOptions {
    /// Optional override for report title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional report subtitle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Watermark toggle or custom text.
    #[serde(default)]
    pub watermark: ExportWatermarkOption,
    /// Prepend `UTF-8` BOM on `CSV` exports.
    #[serde(default)]
    pub csv_bom: bool,
    /// `PDF` page size (`A4` or `A3`).
    #[serde(default)]
    pub page_size: ExportPageSize,
    /// `PDF` page orientation (`portrait` or `landscape`).
    #[serde(default)]
    pub orientation: ExportOrientation,
    /// Optional template placeholder values (`{{key}}` -> value).
    #[serde(default)]
    pub placeholders: BTreeMap<String, String>,
    /// Optional IANA timezone string (defaults to `"UTC"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// Request payload for `POST /api/xblueview/exports`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateExportRequest {
    /// Target output format (`XLSX`, `CSV`, `PDF`, `DOCX`, `PPTX`, `MARKDOWN`).
    pub export_format: ExportFormat,
    /// Content source definition.
    pub source: ExportSource,
    /// Rendering and watermark options.
    #[serde(default)]
    pub options: ExportOptions,
    /// Optional `DOCX` / `PPTX` template UUID (`xv_export_templates.id`).
    #[serde(default)]
    pub template_id: Option<Uuid>,
}

/// Stored export template record (`xv_export_templates`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTemplateRecord {
    /// Template UUID.
    pub id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Tenant-unique template name.
    pub name: String,
    /// Supported template format (`DOCX` or `PPTX`).
    pub export_format: ExportFormat,
    /// Filesystem path under `AXPOINT_DATA_ROOT`.
    #[serde(skip_serializing)]
    pub file_path: String,
    /// Lowercase hex `SHA-256` checksum.
    pub sha256: String,
    /// Extracted `{{placeholder}}` keys discovered at upload time.
    pub placeholders: Vec<String>,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Input parameters for inserting an [`ExportTemplateRecord`].
#[derive(Debug, Clone)]
pub struct CreateExportTemplateParams {
    /// Pre-generated template UUID.
    pub id: Uuid,
    /// Unique template name.
    pub name: String,
    /// Template format (`DOCX` or `PPTX`).
    pub export_format: ExportFormat,
    /// Stored filesystem path.
    pub file_path: String,
    /// Hex `SHA-256` digest.
    pub sha256: String,
    /// Discovered placeholder names.
    pub placeholders: Vec<String>,
    /// Optional creator UUID.
    pub created_by: Option<Uuid>,
}

/// Stored export job record (`xv_export_jobs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportJobRecord {
    /// Export job UUID (`xv_export_jobs.id`).
    pub id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Output format.
    pub export_format: ExportFormat,
    /// Export source payload.
    pub source: ExportSource,
    /// Rendering options.
    pub options: ExportOptions,
    /// Optional template UUID.
    pub template_id: Option<Uuid>,
    /// Backing `ax_jobs.id` queue UUID.
    pub job_id: Option<Uuid>,
    /// Lifecycle status (`PENDING`, `RUNNING`, `SUCCEEDED`, `FAILED`, `CANCELED`, `EXPIRED`).
    pub status: JobStatus,
    /// Download filename (e.g., `"supply-overview.xlsx"`).
    pub file_name: Option<String>,
    /// Internal storage path (omitted from external JSON responses).
    #[serde(skip_serializing)]
    pub file_path: Option<String>,
    /// Artifact byte size.
    pub file_size: Option<i64>,
    /// Lowercase hex `SHA-256` digest of the generated file.
    pub sha256: Option<String>,
    /// Failure diagnostic message when `status = FAILED`.
    pub error_message: Option<String>,
    /// Creator user UUID (`00000000-0000-0000-0000-000000000000` for `dev`).
    pub created_by: Uuid,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Completion timestamp.
    pub finished_at: Option<DateTime<Utc>>,
    /// Retention expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Input parameters for creating a queued export job (`xv_export_jobs` + `ax_jobs`).
#[derive(Debug, Clone)]
pub struct CreateExportJobParams {
    /// Requested export format.
    pub export_format: ExportFormat,
    /// Validated export source.
    pub source: ExportSource,
    /// Rendering options.
    pub options: ExportOptions,
    /// Optional validated template UUID.
    pub template_id: Option<Uuid>,
    /// Creator user UUID (`NOT NULL`).
    pub created_by: Uuid,
}

/// Completion metadata written back when `Sidecar` finishes generating an export artifact.
#[derive(Debug, Clone)]
pub struct CompleteExportJobParams {
    /// User-facing attachment filename.
    pub file_name: String,
    /// Canonical filesystem path under `AXPOINT_DATA_ROOT`.
    pub file_path: String,
    /// File byte size.
    pub file_size: i64,
    /// Lowercase hex `SHA-256` digest.
    pub sha256: String,
    /// Computed retention expiration timestamp.
    pub expires_at: DateTime<Utc>,
}

/// Query parameters for `GET /api/xblueview/exports`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportListQuery {
    /// When `true` (default for non-admins), restricts results to `created_by = caller`.
    #[serde(default)]
    pub mine: Option<bool>,
    /// Maximum items per page (`1..=200`, default `50`).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Optional pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Shareable resource category (`xv_share_links.target_kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShareTargetKind {
    /// Published `xBlueView` dashboard.
    Dashboard,
    /// Completed `SUCCEEDED` export file.
    ExportFile,
    /// Completed `AxSim` scenario.
    Scenario,
}

impl ShareTargetKind {
    /// Returns the uppercase database representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dashboard => "DASHBOARD",
            Self::ExportFile => "EXPORT_FILE",
            Self::Scenario => "SCENARIO",
        }
    }
}

impl fmt::Display for ShareTargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ShareTargetKind {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "DASHBOARD" => Ok(Self::Dashboard),
            "EXPORT_FILE" => Ok(Self::ExportFile),
            "SCENARIO" => Ok(Self::Scenario),
            other => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown ShareTargetKind '{other}'"),
            )),
        }
    }
}

/// Request payload for `POST /api/xblueview/shares` (`10-STEP11-12` §12.5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShareRequest {
    /// Shared target category (`DASHBOARD`, `EXPORT_FILE`, `SCENARIO`).
    pub target_kind: ShareTargetKind,
    /// Target resource UUID.
    pub target_id: Uuid,
    /// Validity window in hours (`1..=720`).
    pub expires_in_hours: u32,
    /// Optional maximum view count (`>= 1`).
    #[serde(default)]
    pub max_views: Option<i32>,
    /// Optional plaintext access password (hashed with `argon2id` before storage).
    #[serde(default)]
    pub password: Option<String>,
    /// Filter values forcefully pinned on every shared query.
    #[serde(default)]
    pub locked_filters: BTreeMap<String, serde_json::Value>,
    /// Whether viewers may adjust non-locked filters.
    #[serde(default)]
    pub allow_filters: bool,
    /// Whether viewers may download/export data from the share page.
    #[serde(default)]
    pub allow_export: bool,
}

/// Response returned once by `POST /api/xblueview/shares`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareResponse {
    /// Share link UUID (`xv_share_links.id`).
    pub id: Uuid,
    /// One-time plaintext share token (`axs_<prefix8>_<secret32>`).
    pub token: String,
    /// Relative share page path (`"/share/<token>"`).
    pub url: String,
    /// Expiration timestamp.
    pub expires_at: DateTime<Utc>,
}

/// Stored share link metadata (`xv_share_links`, omitting `token_hash` and `password_hash`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareLinkRecord {
    /// Share link UUID.
    pub id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Public 8-char prefix (`<prefix8>`) for display and `share:<prefix>` audit attribution.
    pub token_prefix: String,
    /// Shared target category.
    pub target_kind: ShareTargetKind,
    /// Shared target UUID.
    pub target_id: Uuid,
    /// Locked filter map.
    pub locked_filters: BTreeMap<String, serde_json::Value>,
    /// Whether non-locked filters are adjustable.
    pub allow_filters: bool,
    /// Whether export download is permitted.
    pub allow_export: bool,
    /// Whether a password is set on this link.
    pub has_password: bool,
    /// Expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Optional view cap.
    pub max_views: Option<i32>,
    /// Consumed view count.
    pub view_count: i32,
    /// Creator user UUID.
    pub created_by: Uuid,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Revocation timestamp if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Input parameters for persisting a new row in `xv_share_links`.
#[derive(Debug, Clone)]
pub struct CreateShareLinkParams {
    /// Short 8-character token prefix.
    pub token_prefix: String,
    /// Lowercase hex `SHA-256` of full `axs_<prefix8>_<secret32>` token.
    pub token_hash: String,
    /// Target category.
    pub target_kind: ShareTargetKind,
    /// Target UUID.
    pub target_id: Uuid,
    /// Locked filter values.
    pub locked_filters: BTreeMap<String, serde_json::Value>,
    /// Allow non-locked filters.
    pub allow_filters: bool,
    /// Allow export/download.
    pub allow_export: bool,
    /// Optional `argon2id` PHC password hash string.
    pub password_hash: Option<String>,
    /// Expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Optional view cap (`> 0`).
    pub max_views: Option<i32>,
    /// Creator user UUID.
    pub created_by: Uuid,
}

/// Query parameters for `GET /api/xblueview/shares`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareListQuery {
    /// Optional filter by `target_id`.
    #[serde(default)]
    pub target_id: Option<Uuid>,
    /// Optional page size (`1..=200`, default `50`).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Optional pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Row returned by `SELECT * FROM ax_resolve_share($1)` (`0012_export_and_share.sql`).
#[derive(Debug, Clone)]
pub struct ResolvedShare {
    /// Share link UUID.
    pub share_id: Uuid,
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Shared target category.
    pub target_kind: ShareTargetKind,
    /// Shared target UUID.
    pub target_id: Uuid,
    /// Creator user UUID.
    pub created_by: Uuid,
    /// Locked filters JSON map.
    pub locked_filters: BTreeMap<String, serde_json::Value>,
    /// Whether visitor may change unlocked filters.
    pub allow_filters: bool,
    /// Whether visitor may download/export.
    pub allow_export: bool,
    /// Optional `argon2id` password hash.
    pub password_hash: Option<String>,
    /// Optional maximum view limit.
    pub max_views: Option<i32>,
    /// Current view count prior to increment.
    pub view_count: i32,
    /// Token prefix (`<prefix8>`) for audit logging (`"share:<prefix>"`).
    pub token_prefix: String,
    /// Expiration timestamp for display in `SharePage`.
    pub expires_at: DateTime<Utc>,
}

/// Metadata summary of a shared export file returned by `GET /api/xblueview/share/{token}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedExportFileMeta {
    /// Export job UUID.
    pub id: Uuid,
    /// Export format.
    pub export_format: ExportFormat,
    /// Attachment filename.
    pub file_name: String,
    /// File size in bytes.
    pub file_size: i64,
    /// Generation timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Public response returned by `GET /api/xblueview/share/{token}` (`10-STEP11-12` §12.5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharePublicResponse {
    /// `true` when the link is password-protected and no valid `X-Share-Session` header was supplied.
    pub password_required: bool,
    /// Shared target category (`None` while locked behind password).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<ShareTargetKind>,
    /// Link expiration timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Locked filters enforced by the share link.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locked_filters: BTreeMap<String, serde_json::Value>,
    /// Whether visitor may adjust unlocked filters.
    #[serde(default)]
    pub allow_filters: bool,
    /// Whether visitor may trigger file download.
    #[serde(default)]
    pub allow_export: bool,
    /// Populated when `target_kind = DASHBOARD` and unlocked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<DashboardDetail>,
    /// Populated when `target_kind = EXPORT_FILE` and unlocked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_file: Option<SharedExportFileMeta>,
    /// Populated when `target_kind = SCENARIO` and unlocked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario: Option<serde_json::Value>,
}

/// Request payload for `POST /api/xblueview/share/{token}/unlock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareUnlockRequest {
    /// Candidate plaintext password.
    pub password: String,
}

/// Response payload for `POST /api/xblueview/share/{token}/unlock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareUnlockResponse {
    /// HMAC-signed session token valid for 2 hours, passed via `X-Share-Session`.
    pub share_session: String,
    /// Session expiration timestamp (UTC).
    pub expires_at: DateTime<Utc>,
}

/// Chart image output format for `Sidecar` `POST /render/chart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChartImageFormat {
    /// Scalable Vector Graphics (`image/svg+xml`).
    Svg,
    /// 2x rasterized PNG (`image/png`).
    #[default]
    Png,
}

impl ChartImageFormat {
    /// Returns the lowercase wire string (`"svg"` or `"png"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
        }
    }
}

/// Request payload for `Sidecar` `POST /render/chart` (`10-STEP11-12` §12.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderChartRequest {
    /// Pure-JSON `ECharts` option object.
    pub option: serde_json::Value,
    /// Canvas width in CSS pixels.
    #[serde(default = "default_chart_width")]
    pub width: u32,
    /// Canvas height in CSS pixels.
    #[serde(default = "default_chart_height")]
    pub height: u32,
    /// Output format (`"svg"` or `"png"`).
    #[serde(default)]
    pub format: ChartImageFormat,
    /// Optional theme (`"light"` or `"dark"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// Request payload for `Sidecar` `POST /export/{format}` (`10-STEP11-12` §12.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarExportRequest {
    /// Owning tenant ID.
    pub tenant_id: String,
    /// Export job UUID (`xv_export_jobs.id`).
    pub job_id: Uuid,
    /// Unified export document model.
    pub document: ExportDocument,
}

/// Response payload returned by `Sidecar` `POST /export/{format}` (`10-STEP11-12` §12.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarExportResponse {
    /// Generated file path under `AXPOINT_DATA_ROOT/exports/<tenant>/<job_id>.<ext>`.
    pub file_path: String,
    /// Generated file size in bytes.
    pub file_size: i64,
    /// Lowercase hex `SHA-256` checksum.
    pub sha256: String,
    /// Suggested download filename (with `.zip` extension when multi-file `CSV`/`MARKDOWN`).
    pub file_name: String,
}

#[cfg(test)]
mod tests {
    use super::{
        ExportColumnSpec, ExportDocument, ExportEmbeddedTable, ExportOrientation, ExportPageSize,
        ExportSection, ExportTableKind, ExportWatermarkOption,
    };
    use crate::enums::{DataType, WidgetKind};
    use chrono::TimeZone as _;
    use std::collections::BTreeMap;

    #[test]
    fn watermark_option_resolves_bool_and_custom_text() -> Result<(), Box<dyn std::error::Error>> {
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 1, 31, 16, 5, 0)
            .single()
            .ok_or("invalid timestamp")?;
        let auto_wm: ExportWatermarkOption = serde_json::from_value(serde_json::json!(true))?;
        assert_eq!(
            auto_wm.resolve("dev", ts).as_deref(),
            Some("dev 2026-01-31 16:05")
        );

        let off_wm: ExportWatermarkOption = serde_json::from_value(serde_json::json!(false))?;
        assert_eq!(off_wm.resolve("dev", ts), None);

        let custom_wm: ExportWatermarkOption =
            serde_json::from_value(serde_json::json!("  内部资料 严禁外传  "))?;
        assert_eq!(
            custom_wm.resolve("dev", ts).as_deref(),
            Some("内部资料 严禁外传")
        );
        Ok(())
    }

    #[test]
    fn export_document_sections_serialize_isomorphic_kind_tags()
    -> Result<(), Box<dyn std::error::Error>> {
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 6, 1, 8, 0, 0)
            .single()
            .ok_or("invalid timestamp")?;
        let doc = ExportDocument {
            title: "供应概览".to_owned(),
            subtitle: String::new(),
            generated_at: ts,
            generated_by: "dev".to_owned(),
            timezone: "UTC".to_owned(),
            watermark: Some("dev 2026-06-01 08:00".to_owned()),
            sections: vec![
                ExportSection::Kpi {
                    label: "已支付金额".to_owned(),
                    value: "1,280.50".to_owned(),
                    compare: None,
                },
                ExportSection::Chart {
                    title: "按状态订单金额".to_owned(),
                    widget_kind: WidgetKind::BarChart,
                    echarts_option: serde_json::json!({"series": []}),
                    data_table: Some(ExportEmbeddedTable {
                        kind: ExportTableKind::Table,
                        title: "按状态订单金额".to_owned(),
                        columns: vec![ExportColumnSpec {
                            name: "status".to_owned(),
                            data_type: DataType::String,
                            format: None,
                        }],
                        rows: vec![vec![serde_json::json!("PAID")]],
                        totals: None,
                        rows_parquet_path: None,
                        row_kinds: Vec::new(),
                    }),
                    width_px: 960,
                    height_px: 540,
                },
            ],
            template_path: None,
            placeholders: BTreeMap::new(),
            csv_bom: true,
            page_size: ExportPageSize::A4,
            orientation: ExportOrientation::Portrait,
        };

        let val = serde_json::to_value(&doc)?;
        let sections = val
            .get("sections")
            .and_then(serde_json::Value::as_array)
            .ok_or("missing sections")?;
        assert_eq!(
            sections
                .first()
                .and_then(|s| s.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("KPI")
        );
        let chart = sections.get(1).ok_or("missing chart")?;
        assert_eq!(
            chart.get("kind").and_then(serde_json::Value::as_str),
            Some("CHART")
        );
        assert_eq!(
            chart
                .get("data_table")
                .and_then(|t| t.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("TABLE")
        );
        Ok(())
    }
}
