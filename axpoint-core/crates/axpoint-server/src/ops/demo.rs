//! One-click industry demo scenario loader (`POST /api/axops/demo/{scenario}/load`, `§14.6`).

use axpoint_domain::AppError;
use axpoint_domain::enums::{Environment, InterventionMode, MetricSourceTag, TriggerType};
use axpoint_domain::flow::StartRunParams;
use axpoint_domain::graph::{MetricPointInput, NodeListQuery, WriteMetricsRequest};
use axpoint_domain::ops::{
    DemoLoadResponse, DemoScenario, TemplateConflictStrategy, TemplateInstallRecord,
    TemplateInstallStatus,
};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository, GraphStorePort,
    MetricStorePort, OpsRepository, RulesRepository, SidecarPort, SimRepository,
    StudioExecutionPort, StudioRepository, ViewRepository,
};
use axpoint_domain::sim::{CreateScenarioRequest, SandboxScope, SimGraph, SimNode, SimParams};
use axpoint_domain::topology::{AxSimAction, BlueNode, CreateAxSandboxRequest};
use axpoint_security::AuthPrincipal;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::OpsServices;
use super::templates::TemplateAuditContext;

const SYNTHETIC_HISTORY_DAYS: u32 = 30;
const METRIC_WRITE_CHUNK_SIZE: usize = 25_000;

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

fn select_demo_sim_action(graph: &SimGraph, preset: &DemoPreset) -> Option<AxSimAction> {
    let target_node: &SimNode = preset
        .preferred_keys
        .iter()
        .find_map(|key| graph.nodes.values().find(|n| n.node_key == *key))
        .or_else(|| {
            graph
                .nodes
                .values()
                .find(|n| n.type_name.starts_with(preset.preferred_type) && !n.metrics.is_empty())
        })
        .or_else(|| graph.nodes.values().find(|n| !n.metrics.is_empty()))
        .or_else(|| graph.nodes.values().next())?;

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
            assert!(!preset.sandbox_name.is_empty());
            assert!(!preset.scenario_name.is_empty());
            assert!(!preset.preferred_metric.is_empty());
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
