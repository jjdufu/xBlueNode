import {
  Activity,
  AlertTriangle,
  Bot,
  CheckCircle2,
  Cpu,
  Database,
  FileCode2,
  HardDrive,
  Layers,
  RefreshCw,
  Server,
} from "lucide-react";
import type {
  DeepHealthReport,
  OutboxStats,
  SystemInfoResponse,
} from "@/lib/api/axops";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import {
  evaluateOverallDeepHealth,
  formatByteSize,
  formatDurationSeconds,
} from "./logic";

export interface HealthPanelProps {
  report: DeepHealthReport | null;
  outboxStats: OutboxStats | null;
  systemInfo: SystemInfoResponse | null;
  embeddingModel?: string | null;
  loading: boolean;
  onRefresh: () => Promise<void>;
}

function shortenLocalPath(rawPath: string | null | undefined): string {
  if (!rawPath) return "—";
  const normalized = rawPath.replace(/\\/g, "/").replace(/\/+$/, "");
  if (!normalized) return "/";
  const parts = normalized.split("/").filter(Boolean);
  if (parts.length <= 2) return normalized;
  return `…/${parts.slice(-2).join("/")}`;
}

function HealthStatusPill({ ok }: { ok: boolean }) {
  const { tx } = useI18n();
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-semibold",
        ok
          ? "border border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
          : "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400"
      )}
    >
      {ok ? (
        <CheckCircle2 className="h-3 w-3" />
      ) : (
        <AlertTriangle className="h-3 w-3" />
      )}
      {ok ? tx("健康", "Healthy") : tx("降级", "Degraded")}
    </span>
  );
}

export function HealthPanel({
  report,
  outboxStats,
  systemInfo,
  embeddingModel,
  loading,
  onRefresh,
}: HealthPanelProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const isOpsAdmin = can("OPS_ADMIN");

  const overall = evaluateOverallDeepHealth(report);
  const effectiveOutbox = outboxStats ?? report?.outbox ?? null;
  const jobsStats = report?.jobs ?? null;
  const outboxLagHigh = (effectiveOutbox?.lag_s ?? 0) > 60;

  const embedStatusLabel = report?.embed.ok
    ? report.embed.provider?.trim() ||
      embeddingModel?.trim() ||
      tx("向量服务就绪", "Embedding Ready")
    : tx("不可用", "Unavailable");

  return (
    <div className="space-y-4" data-testid="axops-health-panel">
      {/* Top Summary Banner */}
      <Card className="p-4 flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center gap-2">
            <Activity className="h-4 w-4 text-xblue" />
            <span className="text-sm font-semibold">
              {tx("全链路深度健康巡检", "Deep Subsystem Health")}
            </span>
          </div>
          {report ? (
            <HealthStatusPill ok={overall.allOk} />
          ) : (
            <Badge variant="outline">{tx("待加载", "Pending")}</Badge>
          )}
          {!overall.allOk && overall.degradedComponents.length > 0 && (
            <span
              className="text-xs font-mono-tech text-red-600 dark:text-red-400"
              data-testid="axops-health-degraded-list"
            >
              {tx("异常组件", "Degraded")}:{" "}
              {overall.degradedComponents.join(", ")}
            </span>
          )}
        </div>

        <div className="flex items-center gap-2">
          {report?.version && (
            <Badge variant="outline" className="font-mono-tech">
              v{report.version}
            </Badge>
          )}
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="axops-health-refresh-btn"
            disabled={loading}
            onClick={() => void onRefresh()}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
            {loading ? tx("巡检中…", "Probing…") : tx("刷新", "Refresh")}
          </Button>
        </div>
      </Card>

      {/* System Info & Outbox / Jobs Summary Strip */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        {/* System Info */}
        <Card className="p-4 space-y-2" data-testid="axops-system-info-card">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground flex items-center gap-1.5">
              <Server className="h-3.5 w-3.5 text-xblue" />
              {tx("实例运行信息", "Instance Runtime")}
            </span>
            {systemInfo && (
              <Badge variant="xblue">
                {systemInfo.env === "prod"
                  ? tx("生产环境", "Production")
                  : systemInfo.env === "test"
                    ? tx("测试环境", "Test")
                    : systemInfo.env.toUpperCase()}
              </Badge>
            )}
          </div>
          {systemInfo ? (
            <div className="grid grid-cols-2 gap-2 pt-1 text-xs">
              <div>
                <div className="text-[11px] text-muted-foreground">
                  {tx("运行时长", "Uptime")}
                </div>
                <div className="font-mono-tech font-semibold">
                  {formatDurationSeconds(systemInfo.uptime_s)}
                </div>
              </div>
              <div>
                <div className="text-[11px] text-muted-foreground">
                  {tx("内置模板包", "Template Packs")}
                </div>
                <div className="font-mono-tech font-semibold">
                  {fmtNum(systemInfo.template_packs_count, 0)}
                </div>
              </div>
              <div className="col-span-2">
                <div className="text-[11px] text-muted-foreground">
                  {tx("数据卷目录", "Data Volume")}
                </div>
                <div
                  className="font-mono-tech text-[11px] truncate"
                  title={isOpsAdmin ? systemInfo.data_root : undefined}
                >
                  {shortenLocalPath(systemInfo.data_root)}
                </div>
              </div>
            </div>
          ) : (
            <div className="text-xs text-muted-foreground py-2">
              {tx("加载中…", "Loading…")}
            </div>
          )}
        </Card>

        {/* Outbox Lag & Pending */}
        <Card className="p-4 space-y-2" data-testid="axops-outbox-stats-card">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground flex items-center gap-1.5">
              <Layers className="h-3.5 w-3.5 text-xblue" />
              {tx("事务外发队列", "Transactional Outbox")}
            </span>
            <Badge variant={outboxLagHigh ? "warning" : "success"}>
              {outboxLagHigh
                ? tx("延迟偏高 (>60s)", "High Lag (>60s)")
                : tx("正常", "Normal")}
            </Badge>
          </div>
          <div className="grid grid-cols-2 gap-2 pt-1 text-xs">
            <div>
              <div className="text-[11px] text-muted-foreground">
                {tx("最大消费延迟", "Max Consumer Lag")}
              </div>
              <div
                className={cn(
                  "text-lg font-mono-tech font-semibold",
                  outboxLagHigh && "text-amber-600 dark:text-amber-400"
                )}
                data-testid="axops-outbox-lag"
              >
                {effectiveOutbox ? `${effectiveOutbox.lag_s.toFixed(1)}s` : "—"}
              </div>
            </div>
            <div>
              <div className="text-[11px] text-muted-foreground">
                {tx("待分发事件", "Pending Events")}
              </div>
              <div
                className="text-lg font-mono-tech font-semibold"
                data-testid="axops-outbox-pending"
              >
                {effectiveOutbox ? fmtNum(effectiveOutbox.pending, 0) : "—"}
              </div>
            </div>
          </div>
        </Card>

        {/* Jobs Queue Summary */}
        <Card className="p-4 space-y-2" data-testid="axops-jobs-health-card">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground flex items-center gap-1.5">
              <Cpu className="h-3.5 w-3.5 text-xblue" />
              {tx("异步任务队列", "Async Job Queue")}
            </span>
            <Badge variant="outline">
              {tx("24h 失败", "24h Failed")}: {jobsStats?.failed_24h ?? 0}
            </Badge>
          </div>
          <div className="grid grid-cols-3 gap-2 pt-1 text-xs">
            <div>
              <div className="text-[11px] text-muted-foreground">
                {tx("排队中", "Pending")}
              </div>
              <div className="text-base font-mono-tech font-semibold">
                {jobsStats ? fmtNum(jobsStats.pending, 0) : "—"}
              </div>
            </div>
            <div>
              <div className="text-[11px] text-muted-foreground">
                {tx("运行中", "Running")}
              </div>
              <div className="text-base font-mono-tech font-semibold text-xblue">
                {jobsStats ? fmtNum(jobsStats.running, 0) : "—"}
              </div>
            </div>
            <div>
              <div className="text-[11px] text-muted-foreground">
                {tx("最久等待", "Oldest Wait")}
              </div>
              <div className="text-base font-mono-tech font-semibold">
                {jobsStats
                  ? formatDurationSeconds(jobsStats.oldest_pending_s)
                  : "—"}
              </div>
            </div>
          </div>
        </Card>
      </div>

      {/* 7 Subsystem Probe Cards */}
      {report && (
        <div
          className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3"
          data-testid="axops-health-components-grid"
        >
          {/* 1. Database */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-database">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <Database className="h-3.5 w-3.5 text-xblue" />
                {tx("PostgreSQL 数据库", "PostgreSQL")}
              </span>
              <HealthStatusPill ok={report.database.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>
                {tx("探针延迟", "Latency")}: {report.database.latency_ms} ms
              </div>
              <div>
                {tx("复制延迟", "Replication Lag")}:{" "}
                {report.database.replication_lag_s !== null
                  ? `${report.database.replication_lag_s.toFixed(2)}s`
                  : tx("单主零延迟", "Primary (0s)")}
              </div>
              {report.database.error && (
                <div className="text-red-600 dark:text-red-400 break-all">
                  {report.database.error}
                </div>
              )}
            </div>
          </Card>

          {/* 2. Parser Sidecar */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-parser">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <FileCode2 className="h-3.5 w-3.5 text-xblue" />
                {tx("多格式解析与出品引擎", "Parser & Export Engine")}
              </span>
              <HealthStatusPill ok={report.parser.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>
                {tx("版本", "Version")}: {report.parser.version || "—"}
              </div>
              <div>
                {tx(
                  "双轨入湖 / 湖维护 / 多格式出品",
                  "Ingest / Lake Maintenance / Exports"
                )}
              </div>
              {report.parser.error && (
                <div className="text-red-600 dark:text-red-400 break-all">
                  {report.parser.error}
                </div>
              )}
            </div>
          </Card>

          {/* 3. DuckLake */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-lake">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <Layers className="h-3.5 w-3.5 text-xblue" />
                {tx("DuckLake 分析湖", "DuckLake Catalog")}
              </span>
              <HealthStatusPill ok={report.lake.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>
                {tx("最新快照", "Snapshot")}: #{report.lake.snapshot_id}
              </div>
              <div>lake_catalog + Parquet</div>
              {report.lake.error && (
                <div className="text-red-600 dark:text-red-400 break-all">
                  {report.lake.error}
                </div>
              )}
            </div>
          </Card>

          {/* 4. Migrations */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-migrations">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <Database className="h-3.5 w-3.5 text-xblue" />
                {tx("数据库模式迁移", "Schema Migrations")}
              </span>
              <HealthStatusPill
                ok={report.migrations.ok && report.migrations.pending === 0}
              />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>
                {tx("当前版本", "Current")}: #{report.migrations.current}
              </div>
              <div>
                {tx("待执行数", "Pending")}: {report.migrations.pending}
              </div>
            </div>
          </Card>

          {/* 5. LLM */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-llm">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <Bot className="h-3.5 w-3.5 text-xblue" />
                {tx("大模型推理通道", "LLM Gateway")}
              </span>
              <HealthStatusPill ok={report.llm.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>
                {tx("提供方", "Provider")}: {report.llm.provider || "—"}
              </div>
              {report.llm.error && (
                <div className="text-red-600 dark:text-red-400 break-all">
                  {report.llm.error}
                </div>
              )}
            </div>
          </Card>

          {/* 6. Embedding */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-embed">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <Cpu className="h-3.5 w-3.5 text-xblue" />
                {tx("向量表征服务", "Embedding Service")}
              </span>
              <HealthStatusPill ok={report.embed.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>
                {tx("提供方", "Provider")}: {embedStatusLabel}
              </div>
              {report.embed.error && (
                <div className="text-red-600 dark:text-red-400 break-all">
                  {report.embed.error}
                </div>
              )}
            </div>
          </Card>

          {/* 7. Disk */}
          <Card
            className="p-3.5 space-y-2 sm:col-span-2"
            data-testid="health-card-disk"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <HardDrive className="h-3.5 w-3.5 text-xblue" />
                {tx("共享数据卷可用空间", "Data Volume Free Space")}
              </span>
              <HealthStatusPill ok={report.disk.ok} />
            </div>
            <div className="flex flex-wrap items-center justify-between gap-3 text-xs pt-1">
              <div className="space-y-0.5">
                <div className="text-muted-foreground">
                  {tx("剩余容量", "Available Capacity")}
                </div>
                {systemInfo?.data_root && (
                  <div className="font-mono-tech text-[11px] text-muted-foreground/80">
                    {shortenLocalPath(systemInfo.data_root)}
                  </div>
                )}
              </div>
              <span className="text-base font-mono-tech font-semibold text-foreground tabular-nums">
                {formatByteSize(report.disk.data_root_free_bytes)}
              </span>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
