import { Ban, RefreshCw, RotateCcw } from "lucide-react";
import { translateEnum, useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { FlowRunRecord } from "@/lib/api/axflow";
import type { RunStatus } from "@/lib/api/enums";
import { Badge, Button, StatText } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export function formatDurationMs(
  startedAt: string | null,
  finishedAt: string | null
): string {
  if (!startedAt) return "—";
  const startMs = Date.parse(startedAt);
  if (Number.isNaN(startMs)) return "—";
  const endMs = finishedAt ? Date.parse(finishedAt) : Date.now();
  if (Number.isNaN(endMs) || endMs < startMs) return "—";
  const diff = endMs - startMs;
  if (diff < 1000) return `${diff}ms`;
  if (diff < 60_000) return `${(diff / 1000).toFixed(1)}s`;
  return `${Math.floor(diff / 60_000)}m ${Math.round((diff % 60_000) / 1000)}s`;
}

export function renderRunStatusBadge(status: RunStatus) {
  const label = translateEnum(status);
  switch (status) {
    case "SUCCEEDED":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-sans font-medium border border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400">
          {label}
        </span>
      );
    case "WAITING_APPROVAL":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-sans font-medium border border-amber-500/40 bg-amber-500/15 text-amber-700 dark:text-amber-300">
          {label}
        </span>
      );
    case "RUNNING":
    case "PENDING":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-sans font-medium border border-xblue/35 bg-xblue-soft text-xblue">
          {label}
        </span>
      );
    case "FAILED":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-sans font-medium border border-red-500/35 bg-red-500/10 text-red-700 dark:text-red-400">
          {label}
        </span>
      );
    case "CANCELED":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-sans font-medium border border-border bg-muted text-muted-foreground">
          {label}
        </span>
      );
  }
}

export interface RunHistoryProps {
  runs: FlowRunRecord[];
  selectedRunId: string | null;
  onSelectRun: (runId: string) => void;
  onCancelRun: (runId: string) => Promise<void>;
  onRetryRun: (runId: string) => Promise<void>;
  onRefresh: () => Promise<void>;
}

export function RunHistory({
  runs,
  selectedRunId,
  onSelectRun,
  onCancelRun,
  onRetryRun,
  onRefresh,
}: RunHistoryProps) {
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canRun = can("FLOW_RUN");

  const selectedRun = runs.find((r) => r.id === selectedRunId) ?? null;
  const canCancel =
    canRun &&
    selectedRun !== null &&
    (selectedRun.status === "PENDING" ||
      selectedRun.status === "RUNNING" ||
      selectedRun.status === "WAITING_APPROVAL");
  const canRetry =
    canRun && selectedRun !== null && selectedRun.status === "FAILED";

  return (
    <div
      className="flex flex-col h-full min-h-0 rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-run-history"
    >
      {/* Clean Single-Purpose Header (Single Run Action lives in Top Toolbar) */}
      <div className="px-3 py-2 border-b border-border flex items-center justify-between gap-2 shrink-0">
        <div className="flex items-center gap-2">
          <span className="text-xs font-semibold">
            {tx("运行历史", "Run History")}
          </span>
          <StatText label="" value={runs.length} />
        </div>

        <div className="flex items-center gap-1">
          {canCancel && selectedRun && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-6 px-2 text-[11px]"
              data-testid="cancel-run-btn"
              onClick={() => void onCancelRun(selectedRun.id)}
            >
              <Ban className="h-3 w-3" />
              {tx("中止", "Cancel")}
            </Button>
          )}
          {canRetry && selectedRun && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-6 px-2 text-[11px]"
              data-testid="retry-run-btn"
              onClick={() => void onRetryRun(selectedRun.id)}
            >
              <RotateCcw className="h-3 w-3 text-xblue" />
              {tx("重试", "Retry")}
            </Button>
          )}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-2"
            title={tx("刷新列表", "Refresh")}
            data-testid="refresh-runs-btn"
            onClick={() => void onRefresh()}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {/* Runs List */}
      <div className="flex-1 min-h-0 overflow-y-auto divide-y divide-border">
        {runs.length === 0 ? (
          <div className="h-full flex items-center justify-center p-6 text-center text-xs text-muted-foreground">
            {tx("暂无运行记录", "No run records")}
          </div>
        ) : (
          runs.map((run) => {
            const active = run.id === selectedRunId;
            return (
              <button
                key={run.id}
                type="button"
                data-testid={`run-history-item-${run.id}`}
                onClick={() => onSelectRun(run.id)}
                className={cn(
                  "w-full text-left px-3 py-2 transition-colors hover:bg-muted/40 space-y-1 cursor-pointer",
                  active && "bg-xblue-soft/50 border-l-2 border-l-xblue"
                )}
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-1.5">
                    {renderRunStatusBadge(run.status)}
                    <Badge
                      variant={run.environment === "PROD" ? "xblue" : "neutral"}
                    >
                      {run.environment} v{run.version}
                    </Badge>
                  </div>
                  <span className="font-mono-tech text-[11px] text-muted-foreground tabular-nums">
                    {formatDurationMs(
                      run.started_at || run.created_at,
                      run.finished_at
                    )}
                  </span>
                </div>

                <div className="flex items-center justify-between text-[11px] text-muted-foreground font-mono-tech">
                  <span>
                    {run.trigger_source} · #{run.id.slice(0, 8)}
                  </span>
                  {run.idempotency_key && (
                    <span
                      className="truncate max-w-28"
                      title={run.idempotency_key}
                    >
                      key:{run.idempotency_key}
                    </span>
                  )}
                </div>

                {run.error_message && (
                  <div className="truncate text-[11px] text-red-600 dark:text-red-400">
                    {run.error_message}
                  </div>
                )}
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}
