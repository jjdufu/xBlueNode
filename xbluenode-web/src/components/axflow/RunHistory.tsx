import { useState } from "react";
import {
  Ban,
  ChevronDown,
  ChevronUp,
  Play,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { FlowRunRecord, StartFlowRunRequest } from "@/lib/api/axflow";
import type { Environment, RunStatus } from "@/lib/api/enums";
import {
  Badge,
  Button,
  Input,
  Select,
  Textarea,
} from "@/components/ui/primitives";
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
  switch (status) {
    case "SUCCEEDED":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium border border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400">
          SUCCEEDED
        </span>
      );
    case "WAITING_APPROVAL":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium border border-amber-500/40 bg-amber-500/15 text-amber-700 dark:text-amber-300">
          WAITING_APPROVAL
        </span>
      );
    case "RUNNING":
    case "PENDING":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium border border-xblue/35 bg-xblue-soft text-xblue">
          {status}
        </span>
      );
    case "FAILED":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium border border-red-500/35 bg-red-500/10 text-red-700 dark:text-red-400">
          FAILED
        </span>
      );
    case "CANCELED":
      return (
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium border border-border bg-muted text-muted-foreground">
          CANCELED
        </span>
      );
  }
}

export interface RunHistoryProps {
  runs: FlowRunRecord[];
  selectedRunId: string | null;
  runEnvironment: Environment;
  startingRun: boolean;
  onChangeEnvironment: (env: Environment) => void;
  onStartRun: (req: StartFlowRunRequest) => Promise<void>;
  onSelectRun: (runId: string) => void;
  onCancelRun: (runId: string) => Promise<void>;
  onRetryRun: (runId: string) => Promise<void>;
  onRefresh: () => Promise<void>;
}

export function RunHistory({
  runs,
  selectedRunId,
  runEnvironment,
  startingRun,
  onChangeEnvironment,
  onStartRun,
  onSelectRun,
  onCancelRun,
  onRetryRun,
  onRefresh,
}: RunHistoryProps) {
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canRun = can("FLOW_RUN");

  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [idempotencyKey, setIdempotencyKey] = useState("");
  const [payloadText, setPayloadText] = useState("{}");
  const [payloadErr, setPayloadErr] = useState<string | null>(null);

  const selectedRun = runs.find((r) => r.id === selectedRunId) ?? null;
  const canCancel =
    canRun &&
    selectedRun !== null &&
    (selectedRun.status === "PENDING" ||
      selectedRun.status === "RUNNING" ||
      selectedRun.status === "WAITING_APPROVAL");
  const canRetry = canRun && selectedRun !== null && selectedRun.status === "FAILED";

  const triggerManualRun = async () => {
    if (!canRun) return;
    let parsedPayload: Record<string, unknown> = {};
    if (payloadText.trim().length > 0) {
      try {
        const parsed: unknown = JSON.parse(payloadText);
        if (
          typeof parsed !== "object" ||
          parsed === null ||
          Array.isArray(parsed)
        ) {
          setPayloadErr(tx("载荷需为 JSON 对象", "Payload must be JSON object"));
          return;
        }
        parsedPayload = parsed as Record<string, unknown>;
        setPayloadErr(null);
      } catch {
        setPayloadErr(tx("JSON 语法错误", "Invalid JSON"));
        return;
      }
    }

    await onStartRun({
      environment: runEnvironment,
      payload: parsedPayload,
      idempotency_key: idempotencyKey.trim() || undefined,
    });
  };

  return (
    <div
      className="flex flex-col h-[320px] rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-run-history"
    >
      {/* Run Control Header */}
      <div className="p-3 border-b border-border space-y-2 shrink-0">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-semibold">
              {tx("运行历史", "Run History")}
            </span>
            <Badge variant="neutral">{runs.length}</Badge>
          </div>
          <div className="flex items-center gap-1.5">
            <Select
              data-testid="run-history-env-select"
              value={runEnvironment}
              onChange={(e) =>
                onChangeEnvironment(e.target.value as Environment)
              }
              className="h-7 px-2 font-mono-tech text-[11px]"
            >
              <option value="TEST">TEST</option>
              <option value="PROD">PROD</option>
            </Select>
            {canRun && (
              <Button
                type="button"
                size="sm"
                className="h-7 px-2.5"
                data-testid="run-history-start-btn"
                disabled={startingRun}
                onClick={() => void triggerManualRun()}
              >
                <Play className="h-3 w-3" />
                {startingRun
                  ? tx("启动中…", "Starting…")
                  : tx("立即执行", "Run")}
              </Button>
            )}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2"
              title={tx("运行参数", "Run Options")}
              onClick={() => setAdvancedOpen((v) => !v)}
            >
              {advancedOpen ? (
                <ChevronUp className="h-3.5 w-3.5" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5" />
              )}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2"
              title={tx("刷新列表", "Refresh")}
              data-testid="refresh-runs-btn"
              onClick={() => void onRefresh()}
            >
              <RefreshCw className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>

        {(canCancel || canRetry) && selectedRun && (
          <div className="flex items-center justify-between rounded border border-border bg-muted/30 px-2.5 py-1 text-[11px]">
            <span className="font-mono-tech truncate">
              #{selectedRun.id.slice(0, 8)}
            </span>
            <div className="flex items-center gap-1.5">
              {canCancel && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-6 px-2 text-[11px]"
                  data-testid="cancel-run-btn"
                  onClick={() => void onCancelRun(selectedRun.id)}
                >
                  <Ban className="h-3 w-3" />
                  {tx("取消", "Cancel")}
                </Button>
              )}
              {canRetry && (
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
            </div>
          </div>
        )}
      </div>

      {/* Runs List & Optional Advanced Drawer */}
      <div className="flex-1 min-h-0 overflow-y-auto divide-y divide-border">
        {advancedOpen && (
          <div className="p-2.5 bg-muted/20 space-y-2 text-xs">
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("幂等键", "Idempotency Key")}
              </label>
              <Input
                data-testid="run-idempotency-input"
                value={idempotencyKey}
                onChange={(e) => setIdempotencyKey(e.target.value)}
                placeholder="batch-2026-01-31"
                className="h-7 font-mono-tech text-xs"
              />
            </div>
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("触发载荷 JSON", "Payload JSON")}
              </label>
              <Textarea
                data-testid="run-payload-textarea"
                rows={2}
                value={payloadText}
                onChange={(e) => setPayloadText(e.target.value)}
                className="font-mono-tech text-xs"
              />
              {payloadErr && (
                <div className="text-[11px] text-red-600 dark:text-red-400 mt-0.5">
                  {payloadErr}
                </div>
              )}
            </div>
          </div>
        )}

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
                  "w-full text-left p-2.5 transition-colors hover:bg-muted/40 space-y-1.5 cursor-pointer",
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
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
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
