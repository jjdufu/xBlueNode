//! One-click industry demo scenario & multi-world showcase loaders
//! (`POST /api/axops/demo/{scenario}/load`, `POST /api/axops/demo/showcase/load`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration as StdDuration, Instant};

use axpoint_domain::AppError;
use axpoint_domain::enums::{
    AuditAction, Environment, InterventionMode, MetricSourceTag, TriggerType,
};
use axpoint_domain::flow::StartRunParams;
use axpoint_domain::graph::{MetricPointInput, NodeListQuery, WriteMetricsRequest};
use axpoint_domain::ops::{
    DemoLoadResponse, DemoScenario, ShowcaseLoadResponse, TemplateConflictStrategy,
    TemplateInstallRecord, TemplateInstallStatus,
};
use axpoint_domain::platform::{AuditRecord, BoxFuture};
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository, GraphStorePort,
    MetricStorePort, OpsRepository, RulesRepository, SidecarPort, SimRepository,
    StudioExecutionPort, StudioRepository, ViewRepository,
};
use axpoint_domain::sim::{CreateScenarioRequest, SandboxScope, SimGraph, SimNode, SimParams};
use axpoint_domain::topology::{AxSimAction, BlueNode, CreateAxSandboxRequest};
use axpoint_security::AuthPrincipal;
use chrono::{DateTime, Duration, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::OpsServices;
use super::templates::TemplateAuditContext;

const SYNTHETIC_HISTORY_DAYS: u32 = 30;
const METRIC_WRITE_CHUNK_SIZE: usize = 25_000;
const SHOWCASE_TIMEOUT_SECS: u64 = 60;

/// Canonical four Phase 4 industry world subgraph slugs loaded by `scripts/seed-showcase.sh` (`05-STEP35`).
pub const SHOWCASE_SUBGRAPHS: &[&str] = &["power_grid", "fin_risk", "semi_supply", "med_dispatch"];

static SHOWCASE_LOAD_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Execution and audit context passed to [`OpsServices::load_showcase_demo`] (`05-STEP35` §35.0).
#[derive(Debug, Clone, Copy)]
pub struct ShowcaseLoadContext<'a> {
    /// Originating client IP address.
    pub client_ip: &'a str,
    /// Active `W3C` trace identifier.
    pub trace_id: &'a str,
    /// Local loopback or host base URL targeting this `AxPoint` Core instance.
    pub core_base_url: &'a str,
    /// Optional `Bearer` credential extracted from the caller's `Authorization` header.
    pub caller_bearer_token: Option<&'a str>,
}

/// Resolves the canonical filesystem path to `scripts/seed-showcase.sh` (`05-STEP35` §35.0).
#[must_use]
pub fn resolve_showcase_seed_script() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var("AXPOINT_SHOWCASE_SEED_SCRIPT")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        let candidate = PathBuf::from(explicit);
        if candidate.is_file() {
            return Some(std::fs::canonicalize(&candidate).unwrap_or(candidate));
        }
    }

    let candidates = [
        PathBuf::from("./scripts/seed-showcase.sh"),
        PathBuf::from("../scripts/seed-showcase.sh"),
        PathBuf::from("/app/scripts/seed-showcase.sh"),
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../scripts/seed-showcase.sh"
        )),
    ];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
}

/// Preset configuration describing the `AxSim` sandbox and scenario created for a [`DemoScenario`].
#[derive(Debug, Clone)]
pub struct DemoPreset {
    /// Human-readable `AxSim` sandbox name.
    pub sandbox_name: &'static str,
    /// Pre-populated `AxSim` scenario title (`§14.6`).
    pub scenario_name: &'static str,
    /// Scenario summary description.
    pub scenario_description: &'static str,
    /// Preferred target `NodeTypeSpec.name`.
    pub preferred_type: &'static str,
    /// Preferred target `node_key`s in priority order.
    pub preferred_keys: &'static [&'static str],
    /// Preferred target metric key.
    pub preferred_metric: &'static str,
    /// Relative perturbation ratio (`mode = RATIO`).
    pub ratio_delta: f64,
    /// Fallback default dashboard slug if `object_map` has none.
    pub fallback_dashboard_slug: &'static str,
}

/// Returns the deterministic [`DemoPreset`] for `scenario` (`§14.6`).
#[must_use]
pub const fn demo_scenario_preset(scenario: DemoScenario) -> DemoPreset {
    match scenario {
        DemoScenario::SupplyChain => DemoPreset {
            sandbox_name: "供应链协同推演沙箱",
            scenario_name: "F-1 产能下降 30%",
            scenario_description: "模拟华东一号工厂（F-1）设备检修导致产能下降 30% 对下游仓库库存与订单履约率的连锁影响",
            preferred_type: "Factory",
            preferred_keys: &["F-1", "F-2"],
            preferred_metric: "capacity_units",
            ratio_delta: -0.30,
            fallback_dashboard_slug: "supply-overview",
        },
        DemoScenario::ManufacturingEquipment => DemoPreset {
            sandbox_name: "产线设备健康推演沙箱",
            scenario_name: "CNC-A01 停机检修",
            scenario_description: "模拟主力数控加工机台停机降载检修对产线最高主轴温度与综合效率的传导影响",
            preferred_type: "Machine",
            preferred_keys: &["CNC-A01", "MCH-102", "MCH-101"],
            preferred_metric: "spindle_temp",
            ratio_delta: -0.25,
            fallback_dashboard_slug: "equipment-overview",
        },
        DemoScenario::RetailStore => DemoPreset {
            sandbox_name: "门店经营与促销推演沙箱",
            scenario_name: "满减促销销量提升 25%",
            scenario_description: "模拟核心旗舰门店开展限时满减促销拉动日销售额提升 25% 对大区总营收的贡献",
            preferred_type: "Store",
            preferred_keys: &["STR-01", "STR-02"],
            preferred_metric: "daily_sales",
            ratio_delta: 0.25,
            fallback_dashboard_slug: "retail-overview",
        },
    }
}

/// Generates 30 daily synthetic time-series points ending at `anchor` for every numeric metric on `nodes`.
#[must_use]
pub fn generate_synthetic_series_points(
    nodes: &[BlueNode],
    anchor: DateTime<Utc>,
) -> Vec<MetricPointInput> {
    let mut points = Vec::new();
    for (node_idx, node) in nodes.iter().enumerate() {
        let phase_u32 = u32::try_from(node_idx % 17).unwrap_or(0);
        let phase = f64::from(phase_u32) * 0.45;
        for (metric_name, &base_val) in &node.metrics {
            if !base_val.is_finite() {
                continue;
            }
            for day_offset in (0..SYNTHETIC_HISTORY_DAYS).rev() {
                let ts = anchor - Duration::days(i64::from(day_offset));
                let val = if day_offset == 0 {
                    base_val
                } else {
                    let t = f64::from(day_offset);
                    let wave = (t * 0.35 + phase).sin() * 0.08 + (t * 0.13).cos() * 0.03;
                    let raw = base_val * (1.0 + wave);
                    if base_val >= 0.0 { raw.max(0.0) } else { raw }
                };
                points.push(MetricPointInput {
                    node_id: Some(node.id),
                    node: None,
                    metric: metric_name.clone(),
                    ts,
                    value: Some((val * 10_000.0).round() / 10_000.0),
                    value_text: None,
                });
            }
        }
    }
    points
}

impl<S, P> OpsServices<S, P>
where
    S: OpsRepository
        + StudioRepository
        + StudioExecutionPort
        + DatasetRepository
        + AnalyticsRepository
        + FlowRepository
        + RulesRepository
        + ViewRepository
        + AgentRepository
        + GraphStorePort
        + MetricStorePort
        + SimRepository
        + Sync,
    P: SidecarPort + Sync,
{
    /// Installs (or reuses) the matching industry template pack, replays 30 days of time-series
    /// telemetry, triggers a primary `AxFlow` run, and creates a pre-solved `AxSim` scenario (`§14.6`).
    pub fn load_demo_scenario<'a>(
        &'a self,
        principal: &'a AuthPrincipal,
        ctx: TemplateAuditContext<'a>,
        scenario: DemoScenario,
    ) -> BoxFuture<'a, DemoLoadResponse> {
        Box::pin(async move {
            let actor = (!principal.user_id.is_nil()).then_some(principal.user_id);
            let install = self
                .ensure_demo_pack_installed(principal, ctx, scenario)
                .await?;
            let preset = demo_scenario_preset(scenario);

            let dashboard_slug = install
                .object_map
                .values()
                .find(|o| o.category == "dashboard")
                .map_or_else(
                    || String::from(preset.fallback_dashboard_slug),
                    |o| o.target_name.clone(),
                );

            let flow_run_id = self
                .trigger_demo_flow_run(&principal.tenant_id, actor, &install)
                .await?;

            let metric_points_written = self
                .replay_demo_metric_history(&principal.tenant_id)
                .await?;

            let (sandbox_id, scenario_id) = self
                .provision_demo_simulation(&principal.tenant_id, actor, &preset)
                .await?;

            Ok(DemoLoadResponse {
                scenario,
                install_id: install.id,
                flow_run_id,
                metric_points_written,
                sandbox_id,
                scenario_id,
                dashboard_slug,
            })
        })
    }

    async fn ensure_demo_pack_installed(
        &self,
        principal: &AuthPrincipal,
        ctx: TemplateAuditContext<'_>,
        scenario: DemoScenario,
    ) -> Result<TemplateInstallRecord, AppError> {
        let pack_id = scenario.as_str();
        let actor = (!principal.user_id.is_nil()).then_some(principal.user_id);
        let existing_list = self
            .store
            .list_template_installs(&principal.tenant_id)
            .await?;

        if let Some(installed) = existing_list
            .iter()
            .find(|r| r.pack_id == pack_id && r.status == TemplateInstallStatus::Installed)
        {
            return Ok(installed.clone());
        }

        if let Some(resumable) = existing_list.iter().find(|r| {
            r.pack_id == pack_id
                && matches!(
                    r.status,
                    TemplateInstallStatus::Planned
                        | TemplateInstallStatus::Failed
                        | TemplateInstallStatus::Installing
                )
        }) {
            return self
                .apply_template_install(principal, ctx, resumable.id)
                .await;
        }

        let planned = self
            .plan_template_install(
                &principal.tenant_id,
                actor,
                pack_id,
                Some(TemplateConflictStrategy::Rename),
            )
            .await?;
        self.apply_template_install(principal, ctx, planned.id)
            .await
    }

    async fn trigger_demo_flow_run(
        &self,
        tenant_id: &str,
        actor: Option<Uuid>,
        install: &TemplateInstallRecord,
    ) -> Result<Option<Uuid>, AppError> {
        let Some(flow_obj) = install.object_map.values().find(|o| o.category == "flow") else {
            return Ok(None);
        };
        let Ok(flow_id) = Uuid::parse_str(&flow_obj.id) else {
            return Ok(None);
        };
        let (run, _) = self
            .store
            .start_flow_run(
                tenant_id,
                StartRunParams {
                    flow_id,
                    environment: Environment::Prod,
                    explicit_version: None,
                    trigger_source: TriggerType::Manual,
                    trigger_payload: serde_json::json!({ "source": "demo_load" }),
                    idempotency_key: Some(format!(
                        "demo:{}:{}",
                        install.id,
                        Utc::now().date_naive()
                    )),
                    started_by: actor,
                },
            )
            .await?;
        Ok(Some(run.id))
    }

    async fn replay_demo_metric_history(&self, tenant_id: &str) -> Result<u64, AppError> {
        let mut all_nodes: Vec<BlueNode> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = self
                .store
                .list_nodes(
                    tenant_id,
                    NodeListQuery {
                        limit: Some(500),
                        cursor,
                        ..NodeListQuery::default()
                    },
                )
                .await?;
            all_nodes.extend(page.items);
            if page.next_cursor.is_none() || all_nodes.len() >= 2_000 {
                break;
            }
            cursor = page.next_cursor;
        }

        let points = generate_synthetic_series_points(&all_nodes, Utc::now());
        if points.is_empty() {
            return Ok(0);
        }

        let mut total_written: u64 = 0;
        for chunk in points.chunks(METRIC_WRITE_CHUNK_SIZE) {
            let res = self
                .store
                .write_metrics(
                    tenant_id,
                    WriteMetricsRequest {
                        points: chunk.to_vec(),
                        source_tag: MetricSourceTag::Sync,
                        batch_id: Some(Uuid::new_v4()),
                    },
                )
                .await?;
            total_written = total_written.saturating_add(u64::try_from(res.written).unwrap_or(0));
        }

        let _rolled = self.store.run_rollup_for_tenant(tenant_id).await?;
        Ok(total_written)
    }

    async fn provision_demo_simulation(
        &self,
        tenant_id: &str,
        actor: Option<Uuid>,
        preset: &DemoPreset,
    ) -> Result<(Option<Uuid>, Option<Uuid>), AppError> {
        let existing_sandboxes = self.store.list_sandboxes(tenant_id).await?;
        if let Some(sb) = existing_sandboxes
            .iter()
            .find(|s| s.name == preset.sandbox_name)
        {
            let scenarios = self.store.list_scenarios(tenant_id, sb.id).await?;
            if let Some(sc) = scenarios.first() {
                return Ok((Some(sb.id), Some(sc.id)));
            }
        }

        let sandbox = self
            .store
            .create_sandbox(
                tenant_id,
                actor,
                CreateAxSandboxRequest {
                    name: String::from(preset.sandbox_name),
                    scope: Some(SandboxScope::default()),
                },
            )
            .await?;

        let graph = self.store.load_graph(tenant_id, sandbox.id).await?;
        let Some(action) = select_demo_sim_action(&graph, preset) else {
            return Ok((Some(sandbox.id), None));
        };

        let overlay_hash = self
            .store
            .compute_overlay_hash(tenant_id, sandbox.id)
            .await?;
        let params = SimParams::default();
        let created = self
            .store
            .create_scenario_record(
                tenant_id,
                sandbox.id,
                actor,
                &overlay_hash,
                CreateScenarioRequest {
                    name: String::from(preset.scenario_name),
                    description: String::from(preset.scenario_description),
                    actions: vec![action.clone()],
                    scheduled_events: Vec::new(),
                    goal_constraints: Vec::new(),
                    params: params.clone(),
                },
            )
            .await?;

        self.store
            .mark_scenario_running(tenant_id, created.id, &overlay_hash)
            .await?;
        match axpoint_domain::sim::uncertainty::solve(&graph, &[action], &params) {
            Ok(sim_res) => {
                let saved = self
                    .store
                    .save_scenario_success(tenant_id, created.id, &overlay_hash, &sim_res)
                    .await?;
                Ok((Some(sandbox.id), Some(saved.id)))
            }
            Err(err) => {
                let msg = err.to_string();
                let _failed = self
                    .store
                    .save_scenario_failure(tenant_id, created.id, &msg)
                    .await;
                Ok((Some(sandbox.id), Some(created.id)))
            }
        }
    }
}

impl<S, P> OpsServices<S, P>
where
    S: OpsRepository + Sync,
    P: SidecarPort + Sync,
{
    /// Executes `scripts/seed-showcase.sh` idempotently for `principal.tenant_id`, records a
    /// hash-chained `TEMPLATE_INSTALL` (`v4_industry_showcase`) audit entry, and returns the
    /// tenant's post-load [`ShowcaseLoadResponse`] scale summary (`05-STEP35` §35.0).
    pub fn load_showcase_demo<'a>(
        &'a self,
        principal: &'a AuthPrincipal,
        ctx: ShowcaseLoadContext<'a>,
    ) -> BoxFuture<'a, ShowcaseLoadResponse> {
        Box::pin(async move {
            let lock_guard = SHOWCASE_LOAD_MUTEX.lock().await;
            let script_path = resolve_showcase_seed_script().ok_or_else(|| {
                AppError::Config(String::from("scripts/seed-showcase.sh not found"))
            })?;
            let repo_root = script_path
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new("."));

            let started = Instant::now();
            let mut cmd = tokio::process::Command::new("bash");
            cmd.arg(&script_path)
                .current_dir(repo_root)
                .env("SHOWCASE_TENANT_ID", &principal.tenant_id)
                .env("CORE_BASE_URL", ctx.core_base_url)
                .env("PARSER_BASE_URL", &self.parser_base_url)
                .env("AXPOINT_DATA_ROOT", &self.data_root)
                .kill_on_drop(true);
            if let Some(tok) = ctx
                .caller_bearer_token
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                cmd.env("AXPOINT_DEV_TOKEN", tok);
            }

            let output =
                tokio::time::timeout(StdDuration::from_secs(SHOWCASE_TIMEOUT_SECS), cmd.output())
                    .await
                    .map_err(|_| AppError::Timeout("SHOWCASE_LOAD_TIMEOUT"))?
                    .map_err(|err| {
                        AppError::Config(format!("failed to execute seed-showcase.sh: {err}"))
                    })?;

            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            drop(lock_guard);

            if !output.status.success() {
                let stderr_text = String::from_utf8_lossy(&output.stderr);
                let stdout_text = String::from_utf8_lossy(&output.stdout);
                let detail = if stderr_text.trim().is_empty() {
                    stdout_text.trim()
                } else {
                    stderr_text.trim()
                };
                tracing::error!(
                    tenant_id = %principal.tenant_id,
                    status = %output.status,
                    detail = %detail,
                    "seed-showcase.sh failed"
                );
                return Err(AppError::Config(format!(
                    "seed-showcase.sh exited with {}: {detail}",
                    output.status
                )));
            }

            self.emit_showcase_install_audit(principal, ctx, elapsed_ms)
                .await;

            let pulse = self
                .store
                .fetch_platform_pulse(&principal.tenant_id)
                .await?;
            Ok(ShowcaseLoadResponse {
                loaded_subgraphs: SHOWCASE_SUBGRAPHS
                    .iter()
                    .map(|slug| (*slug).to_owned())
                    .collect(),
                node_count: pulse.totals.node_count,
                link_count: pulse.totals.link_count,
                dataset_count: pulse.totals.dataset_count,
                lake_total_rows: pulse.totals.lake_total_rows,
                metric_points_24h: pulse.totals.metric_points_24h,
                elapsed_ms,
            })
        })
    }

    async fn emit_showcase_install_audit(
        &self,
        principal: &AuthPrincipal,
        ctx: ShowcaseLoadContext<'_>,
        elapsed_ms: u64,
    ) {
        let actor_id = (!principal.user_id.is_nil()).then_some(principal.user_id);
        self.audit.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: actor_id,
            username: principal.username.clone(),
            action: AuditAction::TemplateInstall,
            resource_ref: String::from("TEMPLATE:v4_industry_showcase"),
            status_code: 200,
            client_ip: String::from(ctx.client_ip),
            trace_id: String::from(ctx.trace_id),
            detail_json: serde_json::json!({
                "resource_kind": "TEMPLATE",
                "resource_id": "v4_industry_showcase",
                "loaded_subgraphs": SHOWCASE_SUBGRAPHS,
                "elapsed_ms": elapsed_ms,
            }),
            created_at: Utc::now(),
        });
        if let Err(err) = self.audit.flush().await {
            tracing::warn!(error = %err, "showcase install audit flush failed");
        }
    }
}

fn select_demo_sim_action(graph: &SimGraph, preset: &DemoPreset) -> Option<AxSimAction> {
    let target_node: &SimNode = preset
        .preferred_keys
        .iter()
        .find_map(|key| graph.nodes.iter().find(|n| n.node_key == *key))
        .or_else(|| {
            graph
                .nodes
                .iter()
                .find(|n| n.type_name.starts_with(preset.preferred_type) && !n.metrics.is_empty())
        })
        .or_else(|| graph.nodes.iter().find(|n| !n.metrics.is_empty()))
        .or_else(|| graph.nodes.first())?;

    let (metric_key, baseline_opt) =
        if let Some(&v) = target_node.metrics.get(preset.preferred_metric) {
            (String::from(preset.preferred_metric), Some(v))
        } else if let Some((k, &v)) = target_node.metrics.iter().next() {
            (k.clone(), Some(v))
        } else {
            (String::from(preset.preferred_metric), None)
        };

    let (mode, value) = match baseline_opt {
        Some(base) if base.abs() > 1e-9 => (InterventionMode::Ratio, preset.ratio_delta),
        _ => (InterventionMode::Absolute, 100.0),
    };

    Some(AxSimAction {
        target_node_id: target_node.id,
        metric_key,
        mode: Some(mode),
        value: Some(value),
        delta_ratio: None,
        at_step: 0,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn demo_presets_cover_all_three_scenarios() {
        for scenario in DemoScenario::ALL {
            let preset = demo_scenario_preset(*scenario);
            assert_ne!(preset.sandbox_name, "");
            assert_ne!(preset.scenario_name, "");
            assert_ne!(preset.preferred_metric, "");
        }
        assert_eq!(
            demo_scenario_preset(DemoScenario::SupplyChain).scenario_name,
            "F-1 产能下降 30%"
        );
    }

    #[test]
    fn synthetic_series_generates_30_days_preserving_latest_baseline() {
        let now = Utc::now();
        let node = BlueNode {
            id: Uuid::from_u128(7),
            type_name: String::from("Factory"),
            node_key: String::from("F-1"),
            title: String::from("华东一号工厂"),
            properties: serde_json::Map::new(),
            metrics: BTreeMap::from([(String::from("capacity_units"), 5200.0)]),
            geo: None,
            geo_shape: None,
            source_dataset_id: None,
            source_row_key: None,
            row_version: 1,
            created_at: now,
            updated_at: now,
        };
        let pts = generate_synthetic_series_points(&[node], now);
        assert_eq!(pts.len(), 30);
        assert_eq!(pts.last().and_then(|p| p.value), Some(5200.0));
    }
}
