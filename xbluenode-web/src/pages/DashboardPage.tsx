import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Activity,
  Boxes,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Database,
  GitBranch,
  Layers,
  Package,
  RefreshCw,
  ShieldAlert,
  Sparkles,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { api, type HealthInfo } from "@/lib/api";
import { ApiError } from "@/lib/api/client";
import {
  axopsApi,
  type OnboardingStatusResponse,
  type PlatformPulseResponse,
  type PulseTotals,
} from "@/lib/api/axops";
import { usePrincipal } from "@/lib/permissions";
import { ActionAndAlertFeed } from "@/components/dashboard/ActionAndAlertFeed";
import { OnboardingStrip } from "@/components/dashboard/OnboardingStrip";
import { PulseThroughputChart } from "@/components/dashboard/PulseThroughputChart";
import { SubgraphMatrixGrid } from "@/components/dashboard/SubgraphMatrixGrid";
import { TemplateHubModal } from "@/components/dashboard/TemplateHubModal";
import {
  Badge,
  Button,
  Card,
  PageHeader,
  StatusBadge,
} from "@/components/ui/primitives";

const EMPTY_TOTALS: PulseTotals = {
  subgraph_count: 0,
  node_type_count: 0,
  node_count: 0,
  link_count: 0,
  dataset_count: 0,
  lake_total_rows: 0,
  metric_points_24h: 0,
  active_stream_count: 0,
  open_alert_count: 0,
  pending_approval_count: 0,
};

function SparklineSvg({
  values,
  strokeClassName = "text-xblue",
}: {
  values: readonly number[];
  strokeClassName?: string;
}) {
  const width = 96;
  const height = 26;
  const pad = 2;

  const { linePath, areaPath } = useMemo(() => {
    const pts = values.length >= 2 ? values : [0, 0];
    const maxVal = Math.max(...pts, 1);
    const minVal = Math.min(...pts, 0);
    const span = Math.max(maxVal - minVal, 1);
    const stepX = (width - pad * 2) / (pts.length - 1);

    const coords = pts.map((val, idx) => {
      const x = Number((pad + idx * stepX).toFixed(2));
      const ratio = (val - minVal) / span;
      const y = Number((height - pad - ratio * (height - pad * 2)).toFixed(2));
      return { x, y };
    });

    const first = coords[0] ?? { x: pad, y: height - pad };
    const last = coords[coords.length - 1] ?? {
      x: width - pad,
      y: height - pad,
    };
    const poly = coords.map((c) => `${c.x},${c.y}`).join(" L ");
    return {
      linePath: `M ${poly}`,
      areaPath: `M ${first.x},${height - pad} L ${poly} L ${last.x},${
        height - pad
      } Z`,
    };
  }, [values]);

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      aria-hidden="true"
      data-testid="pulse-sparkline-svg"
      className={`shrink-0 overflow-visible ${strokeClassName}`}
    >
      <path d={areaPath} fill="currentColor" fillOpacity={0.12} />
      <path
        d={linePath}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.6}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function DashboardPage() {
  const { tx, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const isOpsAdmin = can("OPS_ADMIN");

  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [pulse, setPulse] = useState<PlatformPulseResponse | null>(null);
  const [onboarding, setOnboarding] =
    useState<OnboardingStatusResponse | null>(null);
  const [onboardingExpanded, setOnboardingExpanded] = useState(false);
  const [templateHubOpen, setTemplateHubOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingShowcase, setLoadingShowcase] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [h, p, ob] = await Promise.all([
        api.health(),
        axopsApi.getPulse(),
        axopsApi.getOnboardingStatus().catch(() => null),
      ]);
      setHealth(h);
      setPulse(p);
      setOnboarding(ob);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleLoadShowcase = useCallback(async () => {
    setLoadingShowcase(true);
    setError(null);
    try {
      try {
        await axopsApi.loadShowcase();
      } catch (err) {
        if (err instanceof ApiError && err.status === 404) {
          await axopsApi.loadDemoScenario("supply_chain");
        } else {
          throw err;
        }
      }
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingShowcase(false);
    }
  }, [load]);

  const totals = pulse?.totals ?? EMPTY_TOTALS;
  const hourlySeries = pulse?.hourly_series ?? [];
  const subgraphs = pulse?.subgraphs ?? [];

  const serviceStatus = health
    ? health.status.toLowerCase() === "ok"
      ? "ONLINE"
      : "OFFLINE"
    : undefined;

  const shouldAutoCollapseOnboarding =
    totals.node_count > 0 && totals.dataset_count > 0;
  const showOnboardingStrip =
    !shouldAutoCollapseOnboarding || onboardingExpanded;

  const completedSteps = onboarding?.completed_steps ?? 0;
  const totalSteps = onboarding?.total_steps ?? 5;

  const metricSparkValues = useMemo(
    () => hourlySeries.map((b) => b.metric_points),
    [hourlySeries]
  );
  const syncSparkValues = useMemo(
    () => hourlySeries.map((b) => b.synced_rows),
    [hourlySeries]
  );
  const combinedSparkValues = useMemo(
    () => hourlySeries.map((b) => b.metric_points + b.synced_rows),
    [hourlySeries]
  );

  const certifiedCount = useMemo(
    () =>
      subgraphs.filter(
        (s) =>
          !s.is_dev_sandbox &&
          (s.cert_level === "L4_CERTIFIED" || s.cert_level === "L3_VERIFIED")
      ).length,
    [subgraphs]
  );

  const kpiCards = [
    {
      key: "nodes",
      testId: "pulse-kpi-nodes",
      valueTestId: "pulse-kpi-value-nodes",
      label: tx("业务节点总数", "Business Nodes"),
      value: fmtNum(totals.node_count, 0),
      meta: tx(
        `${fmtNum(totals.node_type_count, 0)} 节点类型`,
        `${fmtNum(totals.node_type_count, 0)} types`
      ),
      icon: <Layers className="h-3.5 w-3.5 text-xblue" />,
      spark: metricSparkValues,
      strokeClass: "text-xblue",
    },
    {
      key: "links",
      testId: "pulse-kpi-links",
      valueTestId: "pulse-kpi-value-links",
      label: tx("拓扑关系边数", "Topology Links"),
      value: fmtNum(totals.link_count, 0),
      meta: tx(
        `${fmtNum(totals.subgraph_count, 0)} 生产子图`,
        `${fmtNum(totals.subgraph_count, 0)} subgraphs`
      ),
      icon: <GitBranch className="h-3.5 w-3.5 text-xblue" />,
      spark: combinedSparkValues,
      strokeClass: "text-xblue",
    },
    {
      key: "lake-rows",
      testId: "pulse-kpi-lake-rows",
      valueTestId: "pulse-kpi-value-lake-rows",
      label: tx("湖表总行数", "Lake Rows"),
      value: fmtNum(totals.lake_total_rows, 0),
      meta: tx(
        `${fmtNum(totals.dataset_count, 0)} 表 · ${fmtNum(
          totals.active_stream_count,
          0
        )} 常驻流`,
        `${fmtNum(totals.dataset_count, 0)} tables · ${fmtNum(
          totals.active_stream_count,
          0
        )} streams`
      ),
      icon: <Database className="h-3.5 w-3.5 text-xblue" />,
      spark: syncSparkValues,
      strokeClass: "text-xblue",
    },
    {
      key: "throughput",
      testId: "pulse-kpi-throughput",
      valueTestId: "pulse-kpi-value-throughput",
      label: tx("24h 时序吞吐", "24h Metric Points"),
      value: fmtNum(totals.metric_points_24h, 0),
      meta: tx("近 24 小时写入点数", "Points written (24h)"),
      icon: <Activity className="h-3.5 w-3.5 text-xblue" />,
      spark: metricSparkValues,
      strokeClass: "text-xblue",
    },
    {
      key: "subgraphs",
      testId: "pulse-kpi-subgraphs",
      valueTestId: "pulse-kpi-value-subgraphs",
      label: tx("认证业务子图", "Active Subgraphs"),
      value: fmtNum(totals.subgraph_count, 0),
      meta: tx(
        `${fmtNum(certifiedCount, 0)} 已过试算认证`,
        `${fmtNum(certifiedCount, 0)} certified`
      ),
      icon: <Boxes className="h-3.5 w-3.5 text-xblue" />,
      spark: combinedSparkValues,
      strokeClass: "text-emerald-600 dark:text-emerald-400",
    },
    {
      key: "alerts",
      testId: "pulse-kpi-alerts",
      valueTestId: "pulse-kpi-value-alerts",
      label: tx("待处置告警 / 审批", "Alerts / Approvals"),
      value: `${fmtNum(totals.open_alert_count, 0)} / ${fmtNum(
        totals.pending_approval_count,
        0
      )}`,
      meta: tx(
        `${fmtNum(totals.open_alert_count, 0)} 告警 · ${fmtNum(
          totals.pending_approval_count,
          0
        )} 待批`,
        `${fmtNum(totals.open_alert_count, 0)} alerts · ${fmtNum(
          totals.pending_approval_count,
          0
        )} pending`
      ),
      icon: <ShieldAlert className="h-3.5 w-3.5 text-amber-500" />,
      spark: metricSparkValues,
      strokeClass:
        totals.open_alert_count > 0
          ? "text-red-600 dark:text-red-400"
          : "text-xblue",
    },
  ];

  return (
    <div className="space-y-4" data-testid="dashboard-page">
      <PageHeader
        title={tx(MODULE_META.dashboard.label, MODULE_META.dashboard.enLabel)}
        badges={
          <>
            {serviceStatus && <StatusBadge status={serviceStatus} />}
            {pulse?.generated_at && (
              <Badge variant="outline" className="font-mono-tech">
                {fmtTime(pulse.generated_at, true)}
              </Badge>
            )}
            {shouldAutoCollapseOnboarding && (
              <button
                type="button"
                data-testid="dashboard-onboarding-compact-toggle"
                onClick={() => setOnboardingExpanded((prev) => !prev)}
                className="inline-flex items-center gap-1 rounded border border-xblue/30 bg-xblue-soft/60 px-2 py-0.5 text-[11px] font-medium text-xblue hover:bg-xblue-soft transition-colors cursor-pointer tabular-nums"
              >
                <CheckCircle2 className="h-3 w-3" />
                <span>
                  {tx(
                    `引导 ${completedSteps}/${totalSteps}`,
                    `Guide ${completedSteps}/${totalSteps}`
                  )}
                </span>
                {onboardingExpanded ? (
                  <ChevronUp className="h-3 w-3" />
                ) : (
                  <ChevronDown className="h-3 w-3" />
                )}
              </button>
            )}
          </>
        }
        actions={
          <div className="flex items-center gap-2">
            {isOpsAdmin && (
              <Button
                type="button"
                variant="outline"
                onClick={() => void handleLoadShowcase()}
                disabled={loadingShowcase || loading}
                data-testid="dashboard-load-showcase-btn"
              >
                <Sparkles
                  className={`h-3.5 w-3.5 text-xblue ${
                    loadingShowcase ? "animate-pulse" : ""
                  }`}
                />
                {tx("装载样例", "Load Showcase")}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              onClick={() => setTemplateHubOpen(true)}
              data-testid="dashboard-template-hub-btn"
            >
              <Package className="h-3.5 w-3.5 text-xblue" />
              {tx("模板库", "Templates")}
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => void load()}
              disabled={loading}
              data-testid="dashboard-refresh-btn"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
              />
              {tx("刷新", "Refresh")}
            </Button>
          </div>
        }
      />

      {showOnboardingStrip && (
        <OnboardingStrip
          status={onboarding}
          onOpenTemplateHub={() => setTemplateHubOpen(true)}
        />
      )}

      <TemplateHubModal
        open={templateHubOpen}
        onClose={() => setTemplateHubOpen(false)}
      />

      {error && (
        <Card
          className="p-3.5 border-red-500/30 bg-red-500/5 text-xs"
          data-testid="dashboard-error-banner"
        >
          <span className="font-semibold text-red-600 dark:text-red-400">
            {tx("态势拉取失败：", "Pulse fetch failed: ")}
          </span>
          <span>{error}</span>
        </Card>
      )}

      {/* Block 1: 6 Core Operational KPI Cards with Live SVG Sparklines */}
      <div
        className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-6 gap-3"
        data-testid="dashboard-kpi-grid"
      >
        {kpiCards.map((card) => (
          <Card
            key={card.key}
            data-testid={card.testId}
            className="p-3.5 flex flex-col justify-between gap-2"
          >
            <div className="flex items-center justify-between gap-1.5 text-xs text-muted-foreground">
              <span className="font-medium truncate">{card.label}</span>
              {card.icon}
            </div>

            <div className="flex items-end justify-between gap-2">
              <div className="min-w-0">
                <div
                  data-testid={card.valueTestId}
                  className="text-lg font-mono-tech font-semibold text-foreground tabular-nums truncate leading-tight"
                >
                  {card.value}
                </div>
                <div className="text-[11px] text-muted-foreground truncate mt-0.5">
                  {card.meta}
                </div>
              </div>
              <SparklineSvg
                values={card.spark}
                strokeClassName={card.strokeClass}
              />
            </div>
          </Card>
        ))}
      </div>

      {/* Block 2: Subgraph Matrix Grid + 24h ECharts Throughput & Scale */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-stretch">
        <div className="lg:col-span-7">
          <SubgraphMatrixGrid subgraphs={subgraphs} />
        </div>
        <div className="lg:col-span-5">
          <PulseThroughputChart
            hourlySeries={hourlySeries}
            subgraphs={subgraphs}
          />
        </div>
      </div>

      {/* Block 3: Live Alerts & Approvals + Simulation Deadline Warnings */}
      <ActionAndAlertFeed
        activeAlerts={pulse?.active_alerts ?? []}
        pendingApprovals={pulse?.pending_approvals ?? []}
        recentSimulations={pulse?.recent_simulations ?? []}
      />
    </div>
  );
}
