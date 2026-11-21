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
  loading: boolean;
  onRefresh: () => Promise<void>;
}

function HealthStatusPill({ ok }: { ok: boolean }) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-mono-tech font-semibold",
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
      {ok ? "HEALTHY" : "DEGRADED"}
    </span>
  );
}

export function HealthPanel({
  report,
  outboxStats,
  systemInfo,
  loading,
  onRefresh,
}: HealthPanelProps) {
  const overall = evaluateOverallDeepHealth(report);
  const effectiveOutbox = outboxStats ?? report?.outbox ?? null;
  const jobsStats = report?.jobs ?? null;
  const outboxLagHigh = (effectiveOutbox?.lag_s ?? 0) > 60;

  return (
    <div className="space-y-4" data-testid="axops-health-panel">
      {/* Top Summary Banner */}
      <Card className="p-4 flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center gap-2">
            <Activity className="h-4 w-4 text-xblue" />
            <span className="text-sm font-semibold">全链路深度健康巡检</span>
          </div>
          {report ? (
            <HealthStatusPill ok={overall.allOk} />
          ) : (
            <Badge variant="outline">待加载</Badge>
          )}
          {!overall.allOk && overall.degradedComponents.length > 0 && (
            <span
              className="text-xs font-mono-tech text-red-600 dark:text-red-400"
              data-testid="axops-health-degraded-list"
            >
              异常组件: {overall.degradedComponents.join(", ")}
            </span>
          )}
        </div>

        <div className="flex items-center gap-2">
          {report?.version && (
            <Badge variant="outline" className="font-mono-tech">
              Core v{report.version}
            </Badge>
          )}
          <Button
            type="button"
            variant="outline"
            className="h-8 px-3 text-xs"
            data-testid="axops-health-refresh-btn"
            disabled={loading}
            onClick={() => void onRefresh()}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
            {loading ? "巡检中..." : "刷新健康巡检"}
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
              实例运行信息 (System Info)
            </span>
            {systemInfo && (
              <Badge variant="xblue" className="uppercase">
                {systemInfo.env}
              </Badge>
            )}
          </div>
          {systemInfo ? (
            <div className="grid grid-cols-2 gap-2 pt-1 text-xs">
              <div>
                <div className="text-[11px] text-muted-foreground">持续运行时长</div>
                <div className="font-mono-tech font-semibold">
                  {formatDurationSeconds(systemInfo.uptime_s)}
                </div>
              </div>
              <div>
                <div className="text-[11px] text-muted-foreground">预置行业模板包</div>
                <div className="font-mono-tech font-semibold">
                  {systemInfo.template_packs_count} 套
                </div>
              </div>
              <div className="col-span-2">
                <div className="text-[11px] text-muted-foreground">数据根目录</div>
                <div className="font-mono-tech text-[11px] truncate">
                  {systemInfo.data_root}
                </div>
              </div>
            </div>
          ) : (
            <div className="text-xs text-muted-foreground py-2">
              正在读取实例环境元数据...
            </div>
          )}
        </Card>

        {/* Outbox Lag & Pending */}
        <Card className="p-4 space-y-2" data-testid="axops-outbox-stats-card">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground flex items-center gap-1.5">
              <Layers className="h-3.5 w-3.5 text-xblue" />
              事务外发积压 (ax_outbox)
            </span>
            <Badge variant={outboxLagHigh ? "outline" : "neutral"}>
              {outboxLagHigh ? "延迟告警 (>60s)" : "投递正常"}
            </Badge>
          </div>
          <div className="grid grid-cols-2 gap-2 pt-1 text-xs">
            <div>
              <div className="text-[11px] text-muted-foreground">最大消费延迟</div>
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
              <div className="text-[11px] text-muted-foreground">待分发事件数</div>
              <div
                className="text-lg font-mono-tech font-semibold"
                data-testid="axops-outbox-pending"
              >
                {effectiveOutbox ? effectiveOutbox.pending : "—"}
              </div>
            </div>
          </div>
        </Card>

        {/* Jobs Queue Summary */}
        <Card className="p-4 space-y-2" data-testid="axops-jobs-health-card">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground flex items-center gap-1.5">
              <Cpu className="h-3.5 w-3.5 text-xblue" />
              异步任务队列健康 (ax_jobs)
            </span>
            <Badge variant="outline">
              24h 失败: {jobsStats?.failed_24h ?? 0}
            </Badge>
          </div>
          <div className="grid grid-cols-3 gap-2 pt-1 text-xs">
            <div>
              <div className="text-[11px] text-muted-foreground">排队中</div>
              <div className="text-base font-mono-tech font-semibold">
                {jobsStats?.pending ?? "—"}
              </div>
            </div>
            <div>
              <div className="text-[11px] text-muted-foreground">运行中</div>
              <div className="text-base font-mono-tech font-semibold text-xblue">
                {jobsStats?.running ?? "—"}
              </div>
            </div>
            <div>
              <div className="text-[11px] text-muted-foreground">最老等待</div>
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
                PostgreSQL 主备库
              </span>
              <HealthStatusPill ok={report.database.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>探针延迟: {report.database.latency_ms} ms</div>
              <div>
                流复制延迟:{" "}
                {report.database.replication_lag_s !== null
                  ? `${report.database.replication_lag_s.toFixed(2)}s`
                  : "单主 / 同步零延迟"}
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
                多格式解析与出品引擎
              </span>
              <HealthStatusPill ok={report.parser.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>版本: {report.parser.version || "—"}</div>
              <div>职责: 双轨入湖 / 湖维护 / 多格式出品</div>
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
                DuckLake 分析湖目录
              </span>
              <HealthStatusPill ok={report.lake.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>最新快照号: #{report.lake.snapshot_id}</div>
              <div>存储模式: lake_catalog + Parquet</div>
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
                数据库模式迁移
              </span>
              <HealthStatusPill
                ok={report.migrations.ok && report.migrations.pending === 0}
              />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>当前生效版本: #{report.migrations.current}</div>
              <div>待执行迁移数: {report.migrations.pending}</div>
            </div>
          </Card>

          {/* 5. LLM */}
          <Card className="p-3.5 space-y-2" data-testid="health-card-llm">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-semibold flex items-center gap-1.5">
                <Bot className="h-3.5 w-3.5 text-xblue" />
                大模型推理通道 (LLM)
              </span>
              <HealthStatusPill ok={report.llm.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>Provider: {report.llm.provider || "—"}</div>
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
                向量化表征引擎 (Embed)
              </span>
              <HealthStatusPill ok={report.embed.ok} />
            </div>
            <div className="text-[11px] font-mono-tech text-muted-foreground space-y-0.5">
              <div>状态: {report.embed.ok ? "BGE-M3 / Embedding 就绪" : "不可用"}</div>
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
                共享数据卷可用空间 (AXPOINT_DATA_ROOT)
              </span>
              <HealthStatusPill ok={report.disk.ok} />
            </div>
            <div className="flex flex-wrap items-baseline justify-between gap-2 text-xs">
              <span className="text-muted-foreground">剩余可用容量:</span>
              <span className="text-base font-mono-tech font-semibold text-foreground">
                {formatByteSize(report.disk.data_root_free_bytes)}
              </span>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
