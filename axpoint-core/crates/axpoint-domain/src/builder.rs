//! Contracts and pure helpers for `AxBuilder` build-plane MCP tools and dev subgraph sandboxes (`03-STEP17` §17.2).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppError;
use crate::analytics::AxqlCompilePlan;
use crate::enums::{
    GroundingMode, HarnessGapKind, HarnessRunStatus, Permission, Sensitivity, StructuredOutputMode,
    TokenProfile, WidgetKind,
};
use crate::ops::TemplateManifest;
use crate::studio::{
    MappingRejectSample, RuntimeCertReport, SchemaDoc, SchemaDraft, SubgraphInfo, ValidationIssue,
};
use crate::topology::{AxSimAction, AxSimResult, BlueNode};
use crate::view::{DashboardPreflightReport, FilterDef, WidgetDataSource, WidgetPosition};

/// Default permissions granted to a `TokenProfile::Builder` personal access token when `scopes` is omitted.
pub const BUILDER_DEFAULT_SCOPES: &[Permission] = &[
    Permission::DataRead,
    Permission::StudioEdit,
    Permission::FlowEdit,
    Permission::ViewEdit,
    Permission::AgentRun,
    Permission::SimRun,
];

/// Allowed permissions on a `TokenProfile::Builder` personal access token.
pub const BUILDER_ALLOWED_PERMISSIONS: &[Permission] = &[
    Permission::DataRead,
    Permission::DataSync,
    Permission::StudioEdit,
    Permission::FlowEdit,
    Permission::FlowRun,
    Permission::RuleEdit,
    Permission::AgentEdit,
    Permission::AgentRun,
    Permission::SimRun,
    Permission::ViewEdit,
];

/// Permissions strictly stripped and forbidden when `token_profile == BUILDER`.
pub const BUILDER_FORBIDDEN_PROD_PERMISSIONS: &[Permission] = &[
    Permission::StudioPublish,
    Permission::FlowPublish,
    Permission::SimApply,
    Permission::GuardAdmin,
    Permission::OpsAdmin,
    Permission::PiiRawRead,
];

/// Validates requested `scopes` when issuing a token under `profile`, returning effective scopes.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] (`BUILDER_SCOPE_FORBIDDEN`) if `profile` is [`TokenProfile::Builder`]
/// and `requested_scopes` includes any permission in [`BUILDER_FORBIDDEN_PROD_PERMISSIONS`].
pub fn validate_and_resolve_token_scopes(
    profile: TokenProfile,
    requested_scopes: &[Permission],
    caller_permissions: &BTreeSet<Permission>,
) -> Result<Vec<Permission>, AppError> {
    if profile == TokenProfile::Builder {
        for perm in requested_scopes {
            if BUILDER_FORBIDDEN_PROD_PERMISSIONS.contains(perm) {
                return Err(AppError::bad(
                    "BUILDER_SCOPE_FORBIDDEN",
                    format!(
                        "permission {} is forbidden on BUILDER tokens",
                        perm.as_str()
                    ),
                ));
            }
        }
        if requested_scopes.is_empty() {
            return Ok(BUILDER_DEFAULT_SCOPES
                .iter()
                .copied()
                .filter(|perm| caller_permissions.contains(perm))
                .collect());
        }
    }
    let mut dedup = Vec::with_capacity(requested_scopes.len());
    for &perm in requested_scopes {
        if !dedup.contains(&perm) {
            dedup.push(perm);
        }
    }
    Ok(dedup)
}

/// Strips all [`BUILDER_FORBIDDEN_PROD_PERMISSIONS`] from `permissions` when `profile` is [`TokenProfile::Builder`].
pub fn strip_forbidden_builder_permissions(
    profile: TokenProfile,
    permissions: &mut BTreeSet<Permission>,
) {
    if profile == TokenProfile::Builder {
        for forbidden in BUILDER_FORBIDDEN_PROD_PERMISSIONS {
            permissions.remove(forbidden);
        }
    }
}

/// Validates an optional `web_base_url` supplied when issuing a personal access token.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT`) when the URL is non-empty and not a valid `http://` or `https://` origin.
pub fn normalize_web_base_url(raw: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(val) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if val.len() > 256 || !is_valid_http_origin(val) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "web_base_url must start with http:// or https:// without whitespace",
        ));
    }
    Ok(Some(val.trim_end_matches('/').to_owned()))
}

/// Frontend deep links returned alongside every `AxBuilder` MCP tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepLinks {
    /// Direct URL to `xBlueStudio` positioned on the target subgraph and optional draft.
    pub studio_url: String,
    /// Direct URL to `AxFlow` positioned on the target flow draft (or `/flow` root).
    pub flow_url: String,
    /// Direct URL to `xBlueView` positioned on the target dashboard draft (or `/view` root).
    pub view_url: String,
}

/// Resolves deterministic frontend URLs from token `web_base_url`, `AXPOINT_PUBLIC_HOST`, or localhost fallback.
#[must_use]
pub fn resolve_deep_links(
    token_web_base_url: Option<&str>,
    env_public_host: Option<&str>,
    subgraph_slug: &str,
    draft_id: Option<Uuid>,
    flow_id: Option<Uuid>,
    dashboard_id: Option<Uuid>,
) -> DeepLinks {
    let base = token_web_base_url
        .filter(|u| is_valid_http_origin(u))
        .or_else(|| env_public_host.filter(|u| is_valid_http_origin(u)))
        .unwrap_or("http://127.0.0.1:5173")
        .trim_end_matches('/');

    let studio_url = match draft_id {
        Some(did) => format!("{base}/studio?subgraph={subgraph_slug}&draft={did}"),
        None => format!("{base}/studio?subgraph={subgraph_slug}"),
    };
    let flow_url = match flow_id {
        Some(fid) => format!("{base}/flow?flow={fid}"),
        None => format!("{base}/flow"),
    };
    let view_url = match dashboard_id {
        Some(vid) => format!("{base}/view?dashboard={vid}"),
        None => format!("{base}/view"),
    };
    DeepLinks {
        studio_url,
        flow_url,
        view_url,
    }
}

fn is_valid_http_origin(raw: &str) -> bool {
    let trimmed = raw.trim();
    (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.contains(char::is_whitespace)
}

/// Summary of tenant LLM structured decoding capabilities returned by `axbuilder_preflight`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderLlmCapabilitySummary {
    /// Number of active LLM providers in the tenant.
    pub active_provider_count: usize,
    /// Highest structured output decoding tier available across active providers.
    pub highest_structured_mode: StructuredOutputMode,
    /// Whether at least one tool-calling provider is enabled.
    pub supports_tools: bool,
}

/// Response payload of `axbuilder_preflight` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuilderPreflightResponse {
    /// Overall build-plane readiness indicator.
    pub healthy: bool,
    /// Active token profile (`GENERAL` or `BUILDER`).
    pub token_profile: TokenProfile,
    /// Effective permissions held by the caller.
    pub permissions: Vec<Permission>,
    /// Visible business subgraphs in the tenant.
    pub subgraphs: Vec<SubgraphInfo>,
    /// Available built-in industry template packs.
    pub templates: Vec<TemplateManifest>,
    /// Active LLM structured decoding capability summary.
    pub llm_capabilities: BuilderLlmCapabilitySummary,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Lightweight flow summary included in `WorkspacePullResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderFlowBrief {
    /// Flow UUID.
    pub id: Uuid,
    /// Flow name.
    pub name: String,
    /// Optional flow slug.
    pub slug: Option<String>,
    /// Flow status (`DRAFT` / `ACTIVE` / `PAUSED`).
    pub status: String,
    /// Latest published version (`0` when draft-only).
    pub latest_version: i32,
}

/// Lightweight dashboard summary included in `WorkspacePullResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderDashboardBrief {
    /// Dashboard UUID.
    pub id: Uuid,
    /// Dashboard name.
    pub name: String,
    /// Dashboard URL slug.
    pub slug: String,
    /// Dashboard status (`DRAFT` / `PUBLISHED`).
    pub status: String,
}

/// Dataset column profile summary (masked of raw sensitive samples) in `WorkspacePullResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderDatasetSummary {
    /// Dataset UUID.
    pub id: Uuid,
    /// SQL-addressable dataset name.
    pub name: String,
    /// Row count in `DuckLake`.
    pub row_count: i64,
    /// Column descriptors (`name`, `data_type`, `pii_category`).
    pub columns: Vec<serde_json::Value>,
}

/// Response payload of `axbuilder_pull_workspace` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacePullResponse {
    /// Target subgraph metadata.
    pub subgraph: SubgraphInfo,
    /// Current published head version (`0` if unpublished).
    pub published_version: i32,
    /// Published `SchemaDoc` for the subgraph.
    pub published_doc: SchemaDoc,
    /// Latest open `DRAFT` on the subgraph, if any.
    pub active_draft: Option<SchemaDraft>,
    /// Dataset column profiles (included when `include_datasets = true`).
    pub datasets: Vec<BuilderDatasetSummary>,
    /// Flow summaries (included when `include_flows = true`).
    pub flows: Vec<BuilderFlowBrief>,
    /// Dashboard summaries (included when `include_dashboards = true`).
    pub dashboards: Vec<BuilderDashboardBrief>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Response payload of `axbuilder_validate_schema` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateSchemaDraftResponse {
    /// `true` when `error_count == 0`.
    pub valid: bool,
    /// Blocking validation error count.
    pub error_count: usize,
    /// Non-blocking warning count.
    pub warning_count: usize,
    /// Detailed validation diagnostics.
    pub issues: Vec<ValidationIssue>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Response payload of `axbuilder_push_schema_draft` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSchemaDraftResponse {
    /// Created or updated draft UUID.
    pub draft_id: Uuid,
    /// Target subgraph slug.
    pub subgraph_slug: String,
    /// Base published head version.
    pub base_version: i32,
    /// Fixed draft status (`"DRAFT"`).
    pub status: String,
    /// Blocking validation error count.
    pub error_count: usize,
    /// Non-blocking warning count.
    pub warning_count: usize,
    /// Validation diagnostics attached to the draft.
    pub issues: Vec<ValidationIssue>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Response payload of `axbuilder_clone_template` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneTemplateSandboxResponse {
    /// Created dev sandbox subgraph slug.
    pub subgraph_slug: String,
    /// Always `true` for cloned sandbox subgraphs.
    pub is_dev_sandbox: bool,
    /// Originating template pack identifier.
    pub source_template_id: String,
    /// Count of node types loaded into the sandbox draft.
    pub node_type_count: usize,
    /// Count of edge types loaded into the sandbox draft.
    pub edge_type_count: usize,
    /// Created `DRAFT` UUID holding the cloned `SchemaDoc` and dataset mappings.
    pub draft_id: Uuid,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Response payload of `axbuilder_upsert_flow_draft` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertFlowDraftResponse {
    /// Created or updated flow UUID.
    pub flow_id: Uuid,
    /// Flow display name.
    pub name: String,
    /// Fixed draft status (`"DRAFT"`).
    pub status: String,
    /// DAG node count.
    pub node_count: usize,
    /// DAG edge count.
    pub edge_count: usize,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Request payload for dry-run materialization inside a dev sandbox subgraph (`03-STEP17` §17.2.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevSubgraphRunRequest {
    /// Optional draft UUID whose `SchemaDoc` and `mappings` should be materialized; uses latest draft or head if `None`.
    #[serde(default)]
    pub draft_id: Option<Uuid>,
    /// Maximum rows read per mapped dataset (`1..=5_000`, defaults to `200`).
    #[serde(default = "default_limit_rows")]
    pub limit_rows: u32,
}

const fn default_limit_rows() -> u32 {
    200
}

impl Default for DevSubgraphRunRequest {
    fn default() -> Self {
        Self {
            draft_id: None,
            limit_rows: default_limit_rows(),
        }
    }
}

/// Execution summary of `axbuilder_run_dev_subgraph` and `POST /api/xblue/subgraphs/{slug}/dry-run-materialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevSubgraphRunResponse {
    /// Target dev sandbox subgraph slug.
    pub subgraph_slug: String,
    /// Number of nodes upserted inside the sandbox namespace.
    pub nodes_upserted: u64,
    /// Number of links upserted inside the sandbox namespace.
    pub links_upserted: u64,
    /// Number of derived/aggregated metric points computed.
    pub points_computed: u64,
    /// Up to 10 materialized sample nodes for immediate inspection.
    pub sample_nodes: Vec<BlueNode>,
    /// Up to 20 rejected row diagnostics.
    pub reject_samples: Vec<MappingRejectSample>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Execution summary of `POST /api/xblue/subgraphs/{slug}/reset-nodes` and `axbuilder_reset_dev_subgraph`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevSubgraphResetResponse {
    /// Target dev sandbox subgraph slug.
    pub subgraph_slug: String,
    /// Deleted runtime node count in `xb_nodes`.
    pub nodes_deleted: u64,
    /// Deleted runtime link count in `xb_links`.
    pub links_deleted: u64,
    /// Deleted time-series point count in `xb_metric_series`.
    pub series_points_deleted: u64,
    /// Updated reset timestamp recorded on `xb_subgraphs.last_reset_at`.
    pub last_reset_at: DateTime<Utc>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Single widget item inside [`UpsertDashboardDraftInput`] (`axbuilder_upsert_dashboard_draft`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetDraftSpec {
    /// Widget display title.
    pub title: String,
    /// Widget visualization kind.
    pub widget_kind: WidgetKind,
    /// 24-column grid coordinates (`x + w <= 24`).
    pub position: WidgetPosition,
    /// Typed widget data source binding.
    pub data_source: WidgetDataSource,
    /// Optional visual encoding configuration.
    #[serde(default)]
    pub encoding: serde_json::Value,
    /// Optional widget-specific options.
    #[serde(default)]
    pub options: serde_json::Value,
}

/// Input specification passed to `BuilderDashboardPort::upsert_dashboard_draft`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertDashboardDraftInput {
    /// Optional existing dashboard UUID to update (`None` creates or matches by `slug`).
    #[serde(default)]
    pub dashboard_id: Option<Uuid>,
    /// Dashboard display name.
    pub name: String,
    /// Dashboard URL slug (`^[a-z0-9][a-z0-9-]{0,62}$`).
    pub slug: String,
    /// Optional visual theme (`"light"`, `"dark"`, or `"screen"`).
    #[serde(default)]
    pub theme: Option<String>,
    /// Optional dashboard-level filter definitions.
    #[serde(default)]
    pub filters: Vec<FilterDef>,
    /// Draft widgets to replace/upsert on the dashboard.
    #[serde(default)]
    pub widgets: Vec<WidgetDraftSpec>,
}

/// Response payload of `axbuilder_upsert_dashboard_draft` (`03-STEP17` §17.3.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertDashboardDraftResponse {
    /// Created or updated dashboard UUID.
    pub dashboard_id: Uuid,
    /// Dashboard URL slug.
    pub slug: String,
    /// Fixed draft status (`"DRAFT"`).
    pub status: String,
    /// Number of widgets persisted on the draft dashboard.
    pub widget_count: usize,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Full template resource payload returned by `resources/read` for `axpoint://templates/{id}` (`03-STEP17` §17.3.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateResourcePayload {
    /// Parsed `manifest.json` value.
    pub manifest: serde_json::Value,
    /// `README.md` content.
    pub readme_markdown: String,
    /// Parsed `SchemaDoc` (`schema.json` with resolved `mappings.json`).
    pub schema_doc: SchemaDoc,
    /// Semantic metrics array (`metrics.json`).
    pub metrics: serde_json::Value,
    /// Raw mappings array (`mappings.json`).
    pub mappings: serde_json::Value,
}

/// Golden competency question specification stored in `xb_competency_suites.questions` (`07-STEP29` §29.1 & §29.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompetencyQuestionSpec {
    /// Stable question identifier within the suite (e.g., `"cq_east_inv"`).
    pub id: String,
    /// Natural-language business competency question to verify against the subgraph.
    pub question: String,
    /// Optional expected `GroundingMode` (`SEMANTIC_METRIC`, `DOMAIN_OBJECT`, `GUARDED_SQL`, `GROUNDED_DOC`, or `ABSTAINED`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_grounding_mode: Option<GroundingMode>,
    /// Expected `NodeTypeSpec` names referenced by the grounded query/slot plan.
    #[serde(default)]
    pub expected_node_types: Vec<String>,
    /// Expected metric or property names referenced by the grounded query/slot plan.
    #[serde(default)]
    pub expected_metrics: Vec<String>,
    /// Optional expected numeric scalar answer verified within `tolerance`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_number: Option<f64>,
    /// Relative/absolute tolerance for `expected_number` matching (default `1e-3`).
    #[serde(default = "default_competency_tolerance")]
    pub tolerance: f64,
    /// When `true` (default), `ABSTAINED` is treated as a coverage gap unless `expected_grounding_mode == Some(Abstained)`.
    #[serde(default = "default_must_not_abstain")]
    pub must_not_abstain: bool,
}

const fn default_competency_tolerance() -> f64 {
    1e-3
}

const fn default_must_not_abstain() -> bool {
    true
}

/// Persisted competency question suite for a subgraph (`xb_competency_suites`, `07-STEP29` §29.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompetencySuiteRecord {
    /// Target subgraph slug.
    pub subgraph_slug: String,
    /// Ordered competency question specifications.
    pub questions: Vec<CompetencyQuestionSpec>,
    /// Last editor user UUID.
    pub updated_by: Option<Uuid>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Request payload for `PUT /api/xblue/subgraphs/{slug}/competency-suite` and `axbuilder_upsert_competency_suite`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertCompetencySuiteRequest {
    /// Competency questions replacing the suite for `subgraph_slug`.
    #[serde(default)]
    pub questions: Vec<CompetencyQuestionSpec>,
}

/// Request payload for `POST /api/xblue/subgraphs/{slug}/competency-verify` and `axbuilder_verify_competency_suite` (`07-STEP29` §29.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyCompetencySuiteRequest {
    /// Optional active `SchemaDraft` UUID to materialize/verify before publication.
    #[serde(default)]
    pub draft_id: Option<Uuid>,
    /// When `true` on a dev sandbox subgraph, materializes mapped rows and evaluates 4-path rules before running questions.
    #[serde(default = "default_run_dev_materialize")]
    pub run_dev_materialize: bool,
    /// Optional row cap when `run_dev_materialize == true` (`1..=5_000`, default `500`).
    #[serde(default)]
    pub limit_rows: Option<u32>,
    /// Optional ad-hoc question list overriding persisted `xb_competency_suites` for this run.
    #[serde(default)]
    pub questions_override: Option<Vec<CompetencyQuestionSpec>>,
}

const fn default_run_dev_materialize() -> bool {
    true
}

/// Structured ontology, mapping, metric, or data gap diagnostic emitted for a failed competency question (`07-STEP29` §29.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessGapDiagnostic {
    /// Failed `CompetencyQuestionSpec.id`.
    pub question_id: String,
    /// Gap classification (`MISSING_METRIC`, `UNMAPPED_JOIN_EDGE`, `EMPTY_DEV_DATA`, ...).
    pub gap_kind: HarnessGapKind,
    /// Missing or problematic symbol/entity/edge identifier.
    pub target_ref: String,
    /// Actionable remediation guidance for the builder agent or human modeler.
    pub message: String,
    /// Recommended `axbuilder_*` MCP tool to remediate this gap.
    pub suggested_tool: String,
}

/// Per-question verification outcome stored in `xb_harness_runs.results_json` (`07-STEP29` §29.1 & §29.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompetencyQuestionOutcome {
    /// `CompetencyQuestionSpec.id`.
    pub question_id: String,
    /// Question text.
    pub question: String,
    /// Whether all grounding, reference, and numeric tolerance assertions passed.
    pub passed: bool,
    /// Observed `GroundingMode`.
    pub grounding_mode: GroundingMode,
    /// Whether the grounded pipeline abstained.
    pub abstained: bool,
    /// Synthesized grounded answer text.
    pub answer: String,
    /// Verified numeric scalars extracted from deterministic execution results.
    #[serde(default)]
    pub observed_numbers: Vec<f64>,
    /// Compiled SQL or `AxQL` plan executed, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Concise failure explanation when `passed == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

/// Persisted competency harness run record (`xb_harness_runs`, `07-STEP29` §29.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessRunRecord {
    /// Harness run UUID.
    pub id: Uuid,
    /// Evaluated subgraph slug.
    pub subgraph_slug: String,
    /// Total evaluated competency questions.
    pub total_questions: i32,
    /// Count of questions with `passed == true`.
    pub passed_count: i32,
    /// Overall status (`PASS`, `GAP_DETECTED`, or `FAILED`).
    pub status: HarnessRunStatus,
    /// Actionable gap diagnostics for failed questions.
    #[serde(default)]
    pub gap_diagnostics: Vec<HarnessGapDiagnostic>,
    /// Detailed per-question outcomes.
    #[serde(default)]
    pub results: Vec<CompetencyQuestionOutcome>,
    /// Initiating user UUID.
    pub triggered_by: Option<Uuid>,
    /// Execution timestamp.
    pub created_at: DateTime<Utc>,
}

/// Input payload for `axbuilder_upsert_chart_draft` (`07-STEP29` §29.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderUpsertChartDraftInput {
    /// Optional existing `xv_charts.id` to update; `None` creates a new reusable chart asset.
    #[serde(default)]
    pub chart_id: Option<Uuid>,
    /// Chart title.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Visualization kind (`BAR_CHART`, `LINE_CHART`, `PIVOT_TABLE`, etc.).
    pub widget_kind: WidgetKind,
    /// Bound `WidgetDataSource` (`SQL`, `METRIC`, `PIVOT`, etc.).
    pub data_source: WidgetDataSource,
    /// Field encoding map.
    #[serde(default)]
    pub encoding: serde_json::Value,
    /// Chart visual options.
    #[serde(default)]
    pub options: serde_json::Value,
    /// Resource sensitivity (defaults to `INTERNAL`).
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
}

/// Response payload for `axbuilder_upsert_chart_draft` (`07-STEP29` §29.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuilderUpsertChartDraftResponse {
    /// Created or updated `xv_charts.id`.
    pub chart_id: Uuid,
    /// Chart display name.
    pub name: String,
    /// Chart widget kind.
    pub widget_kind: WidgetKind,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Response payload for `axbuilder_sample_dataset_rows` with mandatory PII masking (`07-STEP29` §29.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuilderDatasetSampleResponse {
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Dataset machine name.
    pub dataset_name: String,
    /// Column descriptors (`name`, `data_type`, `pii_category`).
    pub columns: Vec<serde_json::Value>,
    /// Masked sample rows (strictly capped at `<= 20` rows, `PII_RAW_READ` never bypassed).
    pub rows: Vec<serde_json::Value>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Input payload for `axbuilder_sim_trial_run` (`07-STEP29` §29.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderSimTrialRunInput {
    /// Target dev sandbox subgraph slug (must be `is_dev_sandbox == true` or non-`main` trial subgraph).
    pub subgraph_slug: String,
    /// Interventions to apply in the ephemeral trial simulation.
    pub actions: Vec<AxSimAction>,
    /// Optional simulation parameters (`STEADY` or `STEPPED`).
    #[serde(default)]
    pub params: Option<crate::sim::SimParams>,
}

/// Response payload for `axbuilder_sim_trial_run` (`07-STEP29` §29.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuilderSimTrialRunResponse {
    /// Evaluated dev subgraph slug.
    pub subgraph_slug: String,
    /// Ephemeral simulation result (`impacts`, `kpis`, `fired_events`, `deadline_report`).
    pub sim_result: AxSimResult,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Unused-import anchor helper keeping `AxqlCompilePlan`, `RuntimeCertReport`, and `DashboardPreflightReport` re-exported for `BuilderHarnessPort` consumers.
pub type BuilderAxqlPlan = AxqlCompilePlan;
/// Re-exported runtime certification report type for `axbuilder_certify_subgraph`.
pub type BuilderCertReport = RuntimeCertReport;
/// Re-exported dashboard preflight report type for `axbuilder_preflight_dashboard`.
pub type BuilderDashboardPreflight = DashboardPreflightReport;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_links_follow_token_then_env_then_localhost_precedence() {
        let draft_id = Uuid::nil();
        let flow_id = Uuid::nil();
        let dash_id = Uuid::nil();

        let from_token = resolve_deep_links(
            Some("https://studio.xblue.example/"),
            Some("https://public.xblue.example"),
            "dev_sbx",
            Some(draft_id),
            Some(flow_id),
            Some(dash_id),
        );
        assert_eq!(
            from_token.studio_url,
            format!("https://studio.xblue.example/studio?subgraph=dev_sbx&draft={draft_id}")
        );
        assert_eq!(
            from_token.flow_url,
            format!("https://studio.xblue.example/flow?flow={flow_id}")
        );
        assert_eq!(
            from_token.view_url,
            format!("https://studio.xblue.example/view?dashboard={dash_id}")
        );

        let from_env = resolve_deep_links(
            Some("not-an-http-origin"),
            Some("https://public.xblue.example/"),
            "main",
            None,
            None,
            None,
        );
        assert_eq!(
            from_env.studio_url,
            "https://public.xblue.example/studio?subgraph=main"
        );
        assert_eq!(from_env.flow_url, "https://public.xblue.example/flow");
        assert_eq!(from_env.view_url, "https://public.xblue.example/view");

        let fallback = resolve_deep_links(None, None, "main", None, None, None);
        assert_eq!(
            fallback.studio_url,
            "http://127.0.0.1:5173/studio?subgraph=main"
        );
    }

    #[test]
    fn builder_token_rejects_prod_scopes_and_strips_forbidden_permissions() {
        let caller = Permission::all_set();
        let rejected = validate_and_resolve_token_scopes(
            TokenProfile::Builder,
            &[Permission::StudioEdit, Permission::StudioPublish],
            &caller,
        );
        assert!(rejected.is_err());

        let defaults = validate_and_resolve_token_scopes(TokenProfile::Builder, &[], &caller);
        assert!(defaults.is_ok());
        if let Ok(list) = defaults {
            assert!(list.contains(&Permission::StudioEdit));
            assert!(!list.contains(&Permission::StudioPublish));
            assert!(!list.contains(&Permission::PiiRawRead));
        }

        let mut perms = caller;
        strip_forbidden_builder_permissions(TokenProfile::Builder, &mut perms);
        for forbidden in BUILDER_FORBIDDEN_PROD_PERMISSIONS {
            assert!(!perms.contains(forbidden));
        }
    }
}
