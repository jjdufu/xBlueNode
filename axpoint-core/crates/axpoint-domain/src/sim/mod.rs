//! `AxSim` Copy-on-Write causal simulation kernel, scenarios, checkpoints, and decision contracts (`09-STEP10`).

pub mod action;
pub mod explain;
pub mod graph;
pub mod scc;
pub mod steady;
pub mod stepped;
pub mod uncertainty;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::TraversalDirection;
use crate::topology::{AxSandboxRecord, AxSimAction};

pub use action::{NormalizedAction, normalize_actions, normalize_actions_indexed};
pub use explain::{
    ImpactPathExplanation, PathStepDetail, compute_hop_distances, compute_top_paths,
    explain_impact_path,
};
pub use graph::{
    CompiledEdge, EdgeSaturation, IndexedSimGraph, IndexedSimNode, NodeIdx, PropagationClamp,
    PropagationCurve, PropagationRule, PropagationSpec, SimGraph, SimLink, SimNode, aggregate_kpis,
    apply_clamp, eval_edge_delta, merge_clamps,
};
pub use scc::{SccGraph, is_cyclic_component, tarjan_scc_topo};

/// Default maximum node count permitted when cloning an `AxSim` sandbox without explicit truncation.
pub const DEFAULT_SANDBOX_MAX_NODES: usize = 50_000;

/// Maximum recursive traversal depth permitted when scoping an `AxSim` sandbox clone.
pub const MAX_SANDBOX_SCOPE_DEPTH: u32 = 6;

/// Default `SimParams::damping` factor (`α ∈ (0, 1]`).
pub const DEFAULT_SIM_DAMPING: f64 = 0.85;

/// Default `SimParams::tolerance` infinity-norm convergence threshold.
pub const DEFAULT_SIM_TOLERANCE: f64 = 1e-9;

/// Default `SimParams::max_iterations` inside cyclic strongly connected components.
pub const DEFAULT_SIM_MAX_ITERATIONS: u32 = 500;

/// Default `SimParams::min_effect` relative change filter below which impacts are omitted.
pub const DEFAULT_SIM_MIN_EFFECT: f64 = 1e-6;

/// Default `SimParams::horizon_steps` for stepped temporal simulation (`1..=104`).
pub const DEFAULT_SIM_HORIZON_STEPS: u32 = 12;

/// Default `SimParams::checkpoint_every` interval.
pub const DEFAULT_SIM_CHECKPOINT_EVERY: u32 = 1;

/// Sandbox provisioning lifecycle status (`ax_sandboxes.status`).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum SandboxStatus {
    /// Background scope clone is currently in progress.
    #[serde(rename = "CLONING")]
    Cloning,
    /// Baseline snapshot is captured and ready for overlay edits and scenarios.
    #[default]
    #[serde(rename = "READY")]
    Ready,
    /// Sandbox has been archived and is read-only.
    #[serde(rename = "ARCHIVED")]
    Archived,
}

impl SandboxStatus {
    /// Returns the canonical `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cloning => "CLONING",
            Self::Ready => "READY",
            Self::Archived => "ARCHIVED",
        }
    }
}

/// Simulation execution mode (`STEADY` vs `STEPPED`).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum SimMode {
    /// Solve steady-state equilibrium across SCCs using damped Jacobi iteration.
    #[default]
    #[serde(rename = "STEADY")]
    Steady,
    /// Solve step-by-step propagation across `t = 0..=horizon_steps` with lag support.
    #[serde(rename = "STEPPED")]
    Stepped,
}

impl SimMode {
    /// Returns the canonical `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steady => "STEADY",
            Self::Stepped => "STEPPED",
        }
    }
}

/// Scenario execution lifecycle status (`ax_sim_scenarios.status`).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum ScenarioStatus {
    /// Queued for execution.
    #[default]
    #[serde(rename = "PENDING")]
    Pending,
    /// Currently solving propagation.
    #[serde(rename = "RUNNING")]
    Running,
    /// Completed and impacts persisted.
    #[serde(rename = "SUCCEEDED")]
    Succeeded,
    /// Execution failed (e.g., `SIM_DIVERGENT` or invalid action).
    #[serde(rename = "FAILED")]
    Failed,
}

impl ScenarioStatus {
    /// Returns the canonical `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Running => "RUNNING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

/// Decision writeback lifecycle status (`ax_sim_decisions.status`).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum DecisionStatus {
    /// Waiting for `ax_approvals` decision (`SIM_APPLY`).
    #[default]
    #[serde(rename = "PENDING_APPROVAL")]
    PendingApproval,
    /// Approval granted; awaiting conflict check and apply.
    #[serde(rename = "APPROVED")]
    Approved,
    /// Atomically applied to production nodes, time series, action log, and outbox.
    #[serde(rename = "APPLIED")]
    Applied,
    /// Rejected or expired during approval.
    #[serde(rename = "REJECTED")]
    Rejected,
    /// Aborted because production values drifted beyond `tolerance_ratio`.
    #[serde(rename = "CONFLICT")]
    Conflict,
    /// Execution failed unexpectedly during apply.
    #[serde(rename = "FAILED")]
    Failed,
}

impl DecisionStatus {
    /// Returns the canonical `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingApproval => "PENDING_APPROVAL",
            Self::Approved => "APPROVED",
            Self::Applied => "APPLIED",
            Self::Rejected => "REJECTED",
            Self::Conflict => "CONFLICT",
            Self::Failed => "FAILED",
        }
    }
}

/// Natural-language intent for `POST /api/axsim/ask` (`09-STEP10` §10.7.3).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum SimAskIntent {
    /// Parse a hypothetical "what-if" intervention, create/run a scenario, and explain impacts.
    #[default]
    #[serde(rename = "WHAT_IF")]
    WhatIf,
    /// Query existing scenario `ax_sim_impacts` via in-memory `DuckDB` NL2SQL.
    #[serde(rename = "QUERY_RESULT")]
    QueryResult,
}

/// Subgraph extraction scope for cloning a production slice into an `AxSim` sandbox (`§10.2.1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxScope {
    /// Optional seed node UUIDs to expand from; empty clones all matching tenant nodes.
    #[serde(default)]
    pub root_node_ids: Vec<Uuid>,
    /// BFS expansion hop depth (`0..=6`, default `3` when `root_node_ids` is non-empty).
    #[serde(default = "default_scope_depth")]
    pub depth: u32,
    /// Link traversal direction (`OUT`, `IN`, or `BOTH`).
    #[serde(default = "default_scope_direction")]
    pub direction: TraversalDirection,
    /// Optional node `type_name` whitelist.
    #[serde(default)]
    pub type_names: Vec<String>,
    /// Optional link `link_type` whitelist.
    #[serde(default)]
    pub link_types: Vec<String>,
    /// Hard maximum node cap (default `50_000`); exceeding returns `400 LIMIT_OUT_OF_RANGE`.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
}

impl Default for SandboxScope {
    fn default() -> Self {
        Self {
            root_node_ids: Vec::new(),
            depth: default_scope_depth(),
            direction: default_scope_direction(),
            type_names: Vec::new(),
            link_types: Vec::new(),
            max_nodes: DEFAULT_SANDBOX_MAX_NODES,
        }
    }
}

/// Monte-Carlo coefficient perturbation configuration (`§10.6`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UncertaintyConfig {
    /// Number of Monte-Carlo draws (`1..=500`).
    #[serde(default = "default_uncertainty_samples")]
    pub samples: u32,
    /// Coefficient of variation `σ` applied multiplicatively as `max(0, 1 + N(0, cv))`.
    #[serde(default = "default_uncertainty_cv")]
    pub coefficient_cv: f64,
    /// Deterministic PRNG seed for `ChaCha8Rng`.
    #[serde(default = "default_uncertainty_seed")]
    pub seed: u64,
}

impl Default for UncertaintyConfig {
    fn default() -> Self {
        Self {
            samples: default_uncertainty_samples(),
            coefficient_cv: default_uncertainty_cv(),
            seed: default_uncertainty_seed(),
        }
    }
}

/// Causal simulation execution parameters (`§10.3.3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimParams {
    /// Propagation solver mode (`STEADY` or `STEPPED`).
    #[serde(default)]
    pub mode: SimMode,
    /// Jacobi damping coefficient `α ∈ (0, 1]` (default `0.85`).
    #[serde(default = "default_damping")]
    pub damping: f64,
    /// Convergence tolerance under infinity norm (default `1e-9`).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Maximum Jacobi iterations per cyclic SCC (default `500`).
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Minimum relative/absolute effect threshold to retain in output (default `1e-6`).
    #[serde(default = "default_min_effect")]
    pub min_effect: f64,
    /// Optional hop-distance cap from intervention sources (`None` = unlimited).
    #[serde(default)]
    pub max_hops: Option<u32>,
    /// Number of time steps in `STEPPED` mode (`1..=104`, default `12`).
    #[serde(default = "default_horizon_steps")]
    pub horizon_steps: u32,
    /// Optional Monte-Carlo uncertainty sampling configuration.
    #[serde(default)]
    pub uncertainty: Option<UncertaintyConfig>,
    /// Checkpoint persistence stride (`1` = every step in `STEPPED`; power-of-two iterations in `STEADY`).
    #[serde(default = "default_checkpoint_every")]
    pub checkpoint_every: u32,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            mode: SimMode::Steady,
            damping: DEFAULT_SIM_DAMPING,
            tolerance: DEFAULT_SIM_TOLERANCE,
            max_iterations: DEFAULT_SIM_MAX_ITERATIONS,
            min_effect: DEFAULT_SIM_MIN_EFFECT,
            max_hops: None,
            horizon_steps: DEFAULT_SIM_HORIZON_STEPS,
            uncertainty: None,
            checkpoint_every: DEFAULT_SIM_CHECKPOINT_EVERY,
        }
    }
}

/// Non-fatal simulation diagnostic warning (e.g., `METRIC_MISSING` on an intermediate node).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimWarning {
    /// Affected node UUID.
    pub node_id: Uuid,
    /// Machine-readable warning code (e.g., `METRIC_MISSING`).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

/// Single time-step value in a `STEPPED` simulation series (`§10.5`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepSeriesPoint {
    /// Zero-based simulation time step.
    pub step: u32,
    /// Metric value at `step`.
    pub value: f64,
}

/// Single node/metric causal impact entry (`§10.4`–`§10.7`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactRow {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node `type_name`.
    pub type_name: String,
    /// Node display title.
    pub title: String,
    /// Affected metric key.
    #[serde(alias = "metric_key")]
    pub metric: String,
    /// Baseline value before intervention (`None` when `ABSOLUTE` intervenes on a missing metric).
    pub before_val: Option<f64>,
    /// Simulated value after propagation and clamping.
    pub after_val: f64,
    /// Absolute difference (`after_val - before_val`), `None` if `before_val` is missing.
    pub delta: Option<f64>,
    /// Relative ratio `(after_val - before_val) / before_val`, `None` if `before_val` is missing or zero.
    pub delta_ratio: Option<f64>,
    /// 10th percentile under uncertainty sampling, if enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p10: Option<f64>,
    /// 90th percentile under uncertainty sampling, if enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p90: Option<f64>,
    /// Shortest hop distance from any intervention target (`0` = intervened node).
    pub hop_distance: u32,
    /// Dominant causal propagation path `[source_node_id, ..., node_id]` maximizing `Π |W|`.
    #[serde(default)]
    pub top_path: Vec<Uuid>,
    /// Time-step series populated for top 50 impacted nodes in `STEPPED` mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub series: Vec<StepSeriesPoint>,
}

/// Per-`(type_name, metric)` aggregated KPI summary across affected nodes (`§10.7.1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimKpiRow {
    /// Node type name.
    pub type_name: String,
    /// Metric key.
    pub metric: String,
    /// Sum of baseline values across impacted nodes with numeric baseline.
    pub before_sum: f64,
    /// Sum of simulated values across impacted nodes.
    pub after_sum: f64,
    /// Total absolute change (`after_sum - before_sum`).
    pub delta: f64,
    /// Relative change across the group (`delta / before_sum`), `None` if `before_sum == 0`.
    pub delta_ratio: Option<f64>,
    /// Number of impacted nodes of this type/metric.
    pub affected_nodes: usize,
}

/// Incremental replay checkpoint snapshot (`ax_sim_checkpoints`, `§10.5`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Sequential checkpoint index (`0..=horizon_steps` or iteration sample index).
    pub seq: i32,
    /// Human-readable checkpoint label (e.g., `"t=0"` or `"iter=4"`).
    pub label: String,
    /// Sparse map `{node_id: {metric: value}}` containing only values changed since previous checkpoint.
    pub changes: serde_json::Value,
}

/// Full output returned by the pure `AxSim` solver kernel (`steady::solve` / `stepped::solve` / `uncertainty::solve`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimResult {
    /// Impact rows ordered by `|delta|` descending.
    pub impacts: Vec<ImpactRow>,
    /// Total affected `(node, metric)` entries.
    pub affected_count: usize,
    /// Aggregated KPIs grouped by `(type_name, metric)`.
    #[serde(default)]
    pub kpis: Vec<SimKpiRow>,
    /// Diagnostic warnings emitted during propagation (e.g., `METRIC_MISSING`).
    #[serde(default)]
    pub warnings: Vec<SimWarning>,
    /// Whether cyclic SCC iteration converged within `max_iterations`.
    #[serde(default = "default_true")]
    pub converged: bool,
    /// Optional non-convergence reason (`"DIVERGENT"` or `"MAX_ITER"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Maximum Jacobi iteration count executed across SCCs (or step count in `STEPPED` mode).
    #[serde(default)]
    pub iterations: u32,
    /// Incremental replay checkpoints generated during simulation.
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
}

/// Compact summary persisted on `ax_sim_scenarios.result` (top 200 impacts + KPIs + convergence metadata).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioSummary {
    /// Total affected entries across the full run.
    pub affected_count: usize,
    /// Aggregated KPIs by `(type_name, metric)`.
    #[serde(default)]
    pub kpis: Vec<SimKpiRow>,
    /// Top 200 impact rows sorted by `|delta|` descending.
    #[serde(default)]
    pub top_impacts: Vec<ImpactRow>,
    /// Non-fatal warnings recorded during execution.
    #[serde(default)]
    pub warnings: Vec<SimWarning>,
    /// Whether solver converged.
    #[serde(default = "default_true")]
    pub converged: bool,
    /// Optional non-convergence reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Iteration count executed.
    #[serde(default)]
    pub iterations: u32,
}

/// Request payload for `POST /api/axsim/sandboxes/{id}/scenarios` (`§10.7.1`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateScenarioRequest {
    /// Scenario name (unique within sandbox).
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Interventions to apply.
    pub actions: Vec<AxSimAction>,
    /// Simulation solver parameters (defaults to steady-state).
    #[serde(default)]
    pub params: SimParams,
}

/// Persisted `ax_sim_scenarios` record (`§10.7.1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioRecord {
    /// Scenario UUID.
    pub id: Uuid,
    /// Parent sandbox UUID.
    pub sandbox_id: Uuid,
    /// Scenario name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Configured actions.
    pub actions: Vec<AxSimAction>,
    /// Solver parameters.
    pub params: SimParams,
    /// Overlay hash captured at last execution.
    pub overlay_hash: String,
    /// `true` when the sandbox overlay has mutated since `overlay_hash` was recorded.
    pub is_stale: bool,
    /// Compact execution summary (Top 200 impacts & KPIs).
    pub result: Option<ScenarioSummary>,
    /// Lifecycle status.
    pub status: ScenarioStatus,
    /// Error message when `status == FAILED`.
    pub error_message: Option<String>,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Completion timestamp.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Query parameters for `GET /api/axsim/scenarios/{id}/impacts` (`§10.9`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactListQuery {
    /// Optional `type_name` filter.
    #[serde(default)]
    pub type_name: Option<String>,
    /// Optional `metric` filter.
    #[serde(default)]
    pub metric: Option<String>,
    /// Page size (`1..=500`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Response payload for `GET /api/axsim/scenarios/{id}/checkpoints` (`§10.5`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointsResponse {
    /// Ordered checkpoint frames.
    pub items: Vec<Checkpoint>,
}

/// Query parameters for `GET /api/axsim/scenarios/compare` (`§10.7.2`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareScenariosQuery {
    /// Comma-separated 2..=5 scenario UUIDs within the same sandbox.
    pub ids: String,
    /// Optional comma-separated metric whitelist.
    #[serde(default)]
    pub metrics: Option<String>,
    /// Optional `type_name` filter.
    #[serde(default)]
    pub type_name: Option<String>,
    /// When `true`, invokes `SUMMARIZE` LLM to generate grounded comparison briefing.
    #[serde(default)]
    pub briefing: Option<bool>,
}

/// Scenario column header in `CompareScenariosResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareScenarioColumn {
    /// Scenario UUID.
    pub id: Uuid,
    /// Scenario name.
    pub name: String,
    /// Scenario status.
    pub status: ScenarioStatus,
    /// Whether overlay changed since run.
    pub is_stale: bool,
}

/// Aggregated KPI comparison row across scenarios (`§10.7.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareKpiRow {
    /// Node type name.
    pub type_name: String,
    /// Metric key.
    pub metric: String,
    /// Baseline sum.
    pub before_sum: f64,
    /// Per-scenario simulated sum & delta keyed by scenario UUID string.
    pub values: BTreeMap<String, CompareKpiCell>,
}

/// Per-scenario KPI cell in `CompareKpiRow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareKpiCell {
    /// Simulated sum.
    pub after_sum: f64,
    /// Absolute change vs baseline.
    pub delta: f64,
    /// Relative change vs baseline.
    pub delta_ratio: Option<f64>,
    /// Affected node count.
    pub affected_nodes: usize,
}

/// Per-scenario node value in `CompareNodeRow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareNodeCell {
    /// Simulated value.
    pub after_val: f64,
    /// Relative delta vs baseline.
    pub delta_ratio: Option<f64>,
    /// Absolute delta vs baseline.
    pub delta: Option<f64>,
}

/// Node-level comparison row sorted by maximum cross-scenario spread (`§10.7.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareNodeRow {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Node title.
    pub title: String,
    /// Metric key.
    pub metric: String,
    /// Baseline value.
    pub before_val: Option<f64>,
    /// Simulated values keyed by scenario UUID string.
    pub values: BTreeMap<String, CompareNodeCell>,
    /// Maximum `after_val` spread across compared scenarios.
    pub max_spread: f64,
}

/// Grounded AI executive briefing generated when `briefing=true` (`§10.7.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareBriefing {
    /// Executive summary grounded in input numbers.
    pub summary: String,
    /// Recommended scenario UUID (must be one of the compared scenarios).
    pub recommended_scenario_id: Option<Uuid>,
    /// Supporting reasons.
    #[serde(default)]
    pub reasons: Vec<String>,
    /// Highlighted risks or trade-offs.
    #[serde(default)]
    pub risks: Vec<String>,
    /// Present when LLM output failed grounding/schema verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response payload for `GET /api/axsim/scenarios/compare` (`§10.7.2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareScenariosResponse {
    /// Shared sandbox UUID.
    pub sandbox_id: Uuid,
    /// Compared scenario headers.
    pub scenarios: Vec<CompareScenarioColumn>,
    /// Aggregated KPI comparison table.
    pub kpis: Vec<CompareKpiRow>,
    /// Top 200 node-level comparison rows sorted by divergence across scenarios.
    pub nodes: Vec<CompareNodeRow>,
    /// Optional AI executive briefing (`briefing=true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<CompareBriefing>,
}

/// Node item returned in `GET /api/axsim/sandboxes/{id}/graph` (`§10.2.2` & `§10.9`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxGraphNode {
    /// Node UUID.
    pub id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Business node key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Effective merged metrics.
    pub metrics: BTreeMap<String, f64>,
    /// Effective merged properties.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// `true` if modified in overlay (excluding newly created nodes).
    pub is_modified: bool,
    /// `true` if created inside the sandbox overlay.
    pub is_new: bool,
}

/// Link item returned in `GET /api/axsim/sandboxes/{id}/graph` (`§10.2.2` & `§10.9`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxGraphLink {
    /// Link UUID.
    pub id: Uuid,
    /// Relationship type name.
    pub link_type: String,
    /// Source node UUID.
    pub src_node_id: Uuid,
    /// Destination node UUID.
    pub dst_node_id: Uuid,
    /// Effective propagation weight.
    pub weight: f64,
    /// `true` if modified in overlay.
    pub is_modified: bool,
    /// `true` if created inside the sandbox overlay.
    pub is_new: bool,
}

/// Merged Copy-on-Write sandbox graph view (`GET /api/axsim/sandboxes/{id}/graph`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxGraphResponse {
    /// Sandbox UUID.
    pub sandbox_id: Uuid,
    /// Effective merged nodes (deleted excluded).
    pub nodes: Vec<SandboxGraphNode>,
    /// Effective merged links (deleted excluded).
    pub links: Vec<SandboxGraphLink>,
    /// Current canonical overlay hash.
    pub overlay_hash: String,
}

/// Request payload for `PATCH /api/axsim/sandboxes/{id}/nodes/{node_id}` (`§10.2.3`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchOverlayNodeRequest {
    /// Optional replacement title.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional key-level metric patch merged onto baseline metrics.
    #[serde(default)]
    pub metrics: Option<BTreeMap<String, f64>>,
    /// Optional key-level property patch merged onto baseline properties.
    #[serde(default)]
    pub properties: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Request payload for `POST /api/axsim/sandboxes/{id}/nodes` (`§10.2.3`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOverlayNodeRequest {
    /// Node type name.
    pub type_name: String,
    /// Business node key.
    pub node_key: String,
    /// Display title.
    pub title: String,
    /// Initial numeric metrics.
    #[serde(default)]
    pub metrics: BTreeMap<String, f64>,
    /// Initial static properties.
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// Request payload for `POST /api/axsim/sandboxes/{id}/links` (`§10.2.3`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOverlayLinkRequest {
    /// Relationship type name.
    pub link_type: String,
    /// Source node UUID (must exist in effective sandbox graph).
    pub src_node_id: Uuid,
    /// Destination node UUID (must exist in effective sandbox graph).
    pub dst_node_id: Uuid,
    /// Propagation weight.
    #[serde(default = "default_link_weight")]
    pub weight: f64,
}

/// Request payload for `PATCH /api/axsim/sandboxes/{id}/links/{link_id}` (`§10.2.3`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchOverlayLinkRequest {
    /// Optional replacement relationship type name.
    #[serde(default)]
    pub link_type: Option<String>,
    /// Optional replacement propagation weight.
    #[serde(default)]
    pub weight: Option<f64>,
}

/// Response payload for `POST /api/axsim/sandboxes/{id}/refresh-base` (`§10.2.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshBaseResponse {
    /// Updated sandbox metadata after re-cloning production baseline.
    pub sandbox: AxSandboxRecord,
    /// Overlay node IDs whose baseline production node no longer exists in scope.
    pub orphaned_node_ids: Vec<Uuid>,
    /// Overlay link IDs whose baseline production link no longer exists in scope.
    pub orphaned_link_ids: Vec<Uuid>,
}

/// Request payload for `POST /api/axsim/ask` (`§10.7.3`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimAskRequest {
    /// Natural-language question or hypothetical what-if prompt.
    pub question: String,
    /// Target sandbox UUID.
    pub sandbox_id: Uuid,
    /// Optional existing scenario UUID (triggers `QUERY_RESULT` when referencing scenario results).
    #[serde(default)]
    pub scenario_id: Option<Uuid>,
}

/// Disambiguation candidate returned when `WHAT_IF` node resolution matches multiple nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimAskCandidate {
    /// Candidate node UUID.
    pub node_id: Uuid,
    /// Node type name.
    pub type_name: String,
    /// Node key.
    pub node_key: String,
    /// Node title.
    pub title: String,
}

/// Response payload for `POST /api/axsim/ask` (`§10.7.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimAskResponse {
    /// Classified intent (`WHAT_IF` or `QUERY_RESULT`).
    pub intent: SimAskIntent,
    /// Created or queried scenario UUID, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<Uuid>,
    /// Grounded natural-language answer.
    pub answer: String,
    /// Executed `DuckDB` SQL when `intent == QUERY_RESULT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Query result columns when `intent == QUERY_RESULT`.
    #[serde(default)]
    pub columns: Vec<crate::analytics::QueryColumn>,
    /// Query result rows when `intent == QUERY_RESULT`.
    #[serde(default)]
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    /// `true` when multiple sandbox nodes matched and user clarification is required.
    #[serde(default)]
    pub need_clarification: bool,
    /// Candidate nodes when `need_clarification == true`.
    #[serde(default)]
    pub candidates: Vec<SimAskCandidate>,
    /// Top impact rows when `intent == WHAT_IF`.
    #[serde(default)]
    pub impacts: Vec<ImpactRow>,
    /// Summary KPIs when `intent == WHAT_IF`.
    #[serde(default)]
    pub kpis: Vec<SimKpiRow>,
}

/// Single proposed metric writeback item in `ax_sim_decisions.changes` (`§10.8`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionChangeItem {
    /// Target production `xb_nodes.id`.
    pub node_id: Uuid,
    /// Target metric key.
    pub metric: String,
    /// Baseline value captured in the sandbox at proposal time (`None` if metric was absent).
    #[serde(default)]
    pub base_value: Option<f64>,
    /// Target value to write to production upon approval.
    pub target_value: f64,
}

/// Input change item accepted by `POST /api/axsim/scenarios/{id}/decisions`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionChangeInput {
    /// Target `xb_nodes.id`.
    pub node_id: Uuid,
    /// Target metric key.
    pub metric: String,
    /// Target value to apply.
    pub target_value: f64,
}

/// Request payload for `POST /api/axsim/scenarios/{id}/decisions` (`§10.8`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDecisionRequest {
    /// Human-readable decision proposal title.
    pub title: String,
    /// Optional explicit changes; defaults to converting the scenario's interventions to target values.
    #[serde(default)]
    pub changes: Option<Vec<DecisionChangeInput>>,
    /// Relative drift tolerance before triggering `CONFLICT` (`0.0` = exact match).
    #[serde(default)]
    pub tolerance_ratio: f64,
    /// Optional `ax_flows.id` (`PROD` deployment) launched once changes are applied.
    #[serde(default)]
    pub trigger_flow_id: Option<Uuid>,
}

/// Conflict entry recorded in `ax_sim_decisions.conflict_report` when production drifted (`§10.8`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionConflictItem {
    /// Conflicting node UUID.
    pub node_id: Uuid,
    /// Conflicting metric key.
    pub metric: String,
    /// Sandbox baseline value recorded in proposal.
    pub base_value: Option<f64>,
    /// Current production value locked by `SELECT ... FOR UPDATE`.
    pub current_value: Option<f64>,
    /// Requested target value.
    pub target_value: f64,
    /// Observed relative drift ratio.
    pub drift_ratio: f64,
    /// Conflict diagnostic reason.
    pub reason: String,
}

/// Persisted `ax_sim_decisions` record (`§10.8`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// Decision UUID.
    pub id: Uuid,
    /// Source scenario UUID.
    pub scenario_id: Uuid,
    /// Proposal title.
    pub title: String,
    /// Proposed changes with captured `base_value`.
    pub changes: Vec<DecisionChangeItem>,
    /// Optional downstream `AxFlow` UUID triggered after apply.
    pub trigger_flow_id: Option<Uuid>,
    /// Current lifecycle status.
    pub status: DecisionStatus,
    /// Linked `ax_approvals.id` (`source_kind = SIM_APPLY`).
    pub approval_id: Option<Uuid>,
    /// Conflict details when `status == CONFLICT`.
    pub conflict_report: Vec<DecisionConflictItem>,
    /// Drift tolerance ratio.
    pub tolerance_ratio: f64,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Production application timestamp.
    pub applied_at: Option<DateTime<Utc>>,
    /// Approving/applying user UUID.
    pub applied_by: Option<Uuid>,
}

const fn default_scope_depth() -> u32 {
    3
}

const fn default_scope_direction() -> TraversalDirection {
    TraversalDirection::Both
}

const fn default_max_nodes() -> usize {
    DEFAULT_SANDBOX_MAX_NODES
}

const fn default_uncertainty_samples() -> u32 {
    100
}

const fn default_uncertainty_cv() -> f64 {
    0.2
}

const fn default_uncertainty_seed() -> u64 {
    42
}

const fn default_damping() -> f64 {
    DEFAULT_SIM_DAMPING
}

const fn default_tolerance() -> f64 {
    DEFAULT_SIM_TOLERANCE
}

const fn default_max_iterations() -> u32 {
    DEFAULT_SIM_MAX_ITERATIONS
}

const fn default_min_effect() -> f64 {
    DEFAULT_SIM_MIN_EFFECT
}

const fn default_horizon_steps() -> u32 {
    DEFAULT_SIM_HORIZON_STEPS
}

const fn default_checkpoint_every() -> u32 {
    DEFAULT_SIM_CHECKPOINT_EVERY
}

const fn default_link_weight() -> f64 {
    1.0
}

const fn default_true() -> bool {
    true
}
