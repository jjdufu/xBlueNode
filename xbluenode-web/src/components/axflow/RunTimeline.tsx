import { useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  FileCode2,
  Loader2,
  Send,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  ActionLogRecord,
  FlowNodeDef,
  FlowRunRecord,
  FlowStepRecord,
} from "@/lib/api/axflow";
import type { StepStatus } from "@/lib/api/enums";
import { Badge } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { formatDurationMs, renderRunStatusBadge } from "./RunHistory";

const UUID_REGEX =
  /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;

function formatCompactTarget(raw: string): string {
  return raw.replace(UUID_REGEX, (m) => `#${m.slice(0, 8)}`);
}

function ganttBarColor(status: StepStatus): string {
  switch (status) {
    case "SUCCEEDED":
      return "bg-emerald-600 dark:bg-emerald-500";
    case "WAITING_APPROVAL":
      return "bg-amber-500";
    case "RUNNING":
      return "bg-xblue animate-pulse";
    case "FAILED":
      return "bg-red-600";
    case "SKIPPED":
      return "bg-muted-foreground/35";
    case "PENDING":
      return "bg-muted-foreground/25";
  }
}

function stepBadgeClass(status: StepStatus): string {
  switch (status) {
    case "SUCCEEDED":
      return "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400";
    case "WAITING_APPROVAL":
      return "border-amber-500/40 bg-amber-500/15 text-amber-700 dark:text-amber-300";
    case "RUNNING":
      return "border-xblue/40 bg-xblue-soft text-xblue";
    case "FAILED":
      return "border-red-500/40 bg-red-500/10 text-red-700 dark:text-red-400";
    case "SKIPPED":
    case "PENDING":
      return "border-border bg-muted text-muted-foreground";
  }
}

export interface RunTimelineProps {
  run: FlowRunRecord | null;
  steps: FlowStepRecord[];
  actions: ActionLogRecord[];
  nodes: FlowNodeDef[];
  polling: boolean;
}

export function RunTimeline({
  run,
  steps,
  actions,
  nodes,
  polling,
}: RunTimelineProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const [expandedStepIds, setExpandedStepIds] = useState<
    Record<string, boolean>
  >({});
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const nodeLabelMap = useMemo(() => {
    const m: Record<string, string> = {};
    for (const n of nodes) {
      m[n.id] = n.label || n.id;
    }
    return m;
  }, [nodes]);

  const timeBounds = useMemo(() => {
    let minMs = Infinity;
    let maxMs = -Infinity;
    for (const st of steps) {
      if (st.started_at) {
        const s = Date.parse(st.started_at);
        if (!Number.isNaN(s)) minMs = Math.min(minMs, s);
      }
      const endStr = st.finished_at ?? st.started_at;
      if (endStr) {
        const e = Date.parse(endStr);
        if (!Number.isNaN(e)) maxMs = Math.max(maxMs, e);
      }
    }
    if (!Number.isFinite(minMs)) {
      const fallback = run?.started_at ? Date.parse(run.started_at) : 0;
      minMs = Number.isNaN(fallback) ? 0 : fallback;
    }
    if (!Number.isFinite(maxMs) || maxMs <= minMs) {
      maxMs = minMs + 1000;
    }
    return { minMs, spanMs: Math.max(1, maxMs - minMs) };
  }, [steps, run?.started_at]);

  if (!run) {
    return (
      <div
        className="flex flex-col h-full min-h-0 rounded-md border border-border bg-card text-card-foreground overflow-hidden"
        data-testid="axflow-run-timeline-empty"
      >
        <div className="px-3 py-2 border-b border-border flex items-center justify-between shrink-0">
          <span className="text-xs font-semibold">
            {tx("步骤甘特时间线", "Step Gantt Timeline")}
          </span>
        </div>
        <div className="flex-1 flex items-center justify-center p-6 text-center text-xs text-muted-foreground">
          {tx("未选中运行实例", "No run instance selected")}
        </div>
      </div>
    );
  }

  const toggleExpand = (key: string) => {
    setExpandedStepIds((prev) => ({ ...prev, [key]: !prev[key] }));
  };

  return (
    <div
      className="flex flex-col h-full min-h-0 rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-run-timeline"
    >
      {/* Run Summary Header */}
      <div className="px-3 py-2 border-b border-border flex flex-wrap items-center justify-between gap-2 shrink-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-semibold">
            {tx("步骤甘特时间线", "Step Gantt Timeline")}
          </span>
          <span data-testid="timeline-run-status">
            {renderRunStatusBadge(run.status)}
          </span>
          <Badge variant={run.environment === "PROD" ? "xblue" : "neutral"}>
            {run.environment} v{run.version}
          </Badge>
          <span className="font-mono-tech text-[11px] text-muted-foreground">
            #{run.id.slice(0, 8)}
          </span>
        </div>

        <div className="flex items-center gap-2 text-[11px] text-muted-foreground font-mono-tech tabular-nums">
          {polling && (
            <span className="inline-flex items-center gap-1 text-xblue">
              <Loader2 className="h-3 w-3 animate-spin" />
              {tx("同步中…", "Syncing…")}
            </span>
          )}
          <span>
            {tx("耗时:", "Elapsed:")}{" "}
            {formatDurationMs(
              run.started_at || run.created_at,
              run.finished_at
            )}
          </span>
        </div>
      </div>

      {/* Wide 8-Col Gantt Step Rows + Compact Audit Logs (Zero step-name truncation) */}
      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className="divide-y divide-border">
          {steps.length === 0 ? (
            <div className="py-8 text-center text-xs text-muted-foreground">
              {tx("等待调度器派发步骤…", "Awaiting step dispatch…")}
            </div>
          ) : (
            steps.map((st, idx) => {
              const rowKey = `${st.node_id}-${st.attempt}-${st.id || idx}`;
              const expanded = Boolean(expandedStepIds[rowKey]);
              const startMs = st.started_at ? Date.parse(st.started_at) : NaN;
              const endMs = st.finished_at
                ? Date.parse(st.finished_at)
                : !Number.isNaN(startMs)
                  ? startMs + Math.max(80, timeBounds.spanMs * 0.12)
                  : NaN;

              const leftPct = !Number.isNaN(startMs)
                ? Math.min(
                    92,
                    Math.max(
                      0,
                      ((startMs - timeBounds.minMs) / timeBounds.spanMs) * 100
                    )
                  )
                : idx * 10;
              const widthPct =
                !Number.isNaN(startMs) && !Number.isNaN(endMs)
                  ? Math.min(
                      100 - leftPct,
                      Math.max(6, ((endMs - startMs) / timeBounds.spanMs) * 100)
                    )
                  : 12;

              return (
                <div
                  key={rowKey}
                  data-testid={`timeline-step-${st.node_id}`}
                  data-step-status={st.status}
                  className="px-3 py-2 space-y-2"
                >
                  <div
                    onClick={() => toggleExpand(rowKey)}
                    className="grid grid-cols-12 items-center gap-3 cursor-pointer text-xs"
                  >
                    <div className="col-span-5 flex items-center gap-1.5 min-w-0">
                      {expanded ? (
                        <ChevronDown className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                      ) : (
                        <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                      )}
                      <span className="truncate font-medium">
                        {nodeLabelMap[st.node_id] ?? st.node_id}
                      </span>
                      <span className="font-mono-tech text-[10px] text-muted-foreground shrink-0">
                        ({st.node_id} #{st.attempt})
                      </span>
                    </div>

                    <div className="col-span-4">
                      <div className="relative h-2.5 w-full rounded-full bg-muted overflow-hidden">
                        <div
                          className={cn(
                            "absolute top-0 bottom-0 rounded-full transition-all",
                            ganttBarColor(st.status)
                          )}
                          style={{
                            left: `${leftPct}%`,
                            width: `${widthPct}%`,
                          }}
                        />
                      </div>
                    </div>

                    <div className="col-span-3 flex items-center justify-end gap-2">
                      <span
                        data-testid={`timeline-step-status-${st.node_id}`}
                        className={cn(
                          "rounded border px-1.5 py-0.5 font-sans text-[10px] font-medium shrink-0",
                          stepBadgeClass(st.status)
                        )}
                      >
                        {tEnum(st.status)}
                      </span>
                      <span className="hidden md:inline font-mono-tech text-[10px] text-muted-foreground tabular-nums w-12 text-right">
                        {formatDurationMs(st.started_at, st.finished_at)}
                      </span>
                    </div>
                  </div>

                  {expanded && (
                    <div
                      data-testid={`timeline-step-detail-${st.node_id}`}
                      className="rounded-md border border-border bg-muted/30 p-2.5 space-y-2 text-[11px]"
                    >
                      {st.error_message && (
                        <div className="rounded border border-red-500/30 bg-red-500/10 p-2 text-red-700 dark:text-red-300 font-mono-tech">
                          [{st.error_code ?? "STEP_FAILED"}] {st.error_message}
                        </div>
                      )}

                      <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
                        <div>
                          <div className="font-medium text-muted-foreground mb-1">
                            {tx("渲染输入", "Resolved Input")}
                          </div>
                          <pre className="max-h-32 overflow-auto rounded border border-border bg-card p-2 font-mono-tech text-[10px]">
                            {JSON.stringify(st.input, null, 2)}
                          </pre>
                        </div>

                        <div>
                          <div className="font-medium text-muted-foreground mb-1">
                            {tx("节点输出", "Step Output")}
                          </div>
                          {st.output_ref ? (
                            <div className="inline-flex items-center gap-1.5 rounded border border-border bg-card px-2.5 py-1.5 font-mono-tech text-[10px]">
                              <FileCode2 className="h-3.5 w-3.5 text-xblue" />
                              {st.output_ref}
                            </div>
                          ) : (
                            <pre className="max-h-32 overflow-auto rounded border border-border bg-card p-2 font-mono-tech text-[10px]">
                              {JSON.stringify(st.output, null, 2)}
                            </pre>
                          )}
                        </div>
                      </div>
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>

        {/* Writeback Action Logs (`ax_action_log` with formatted short UUIDs) */}
        {actions.length > 0 && (
          <div
            className="border-t border-border bg-muted/20 p-2.5 space-y-1.5"
            data-testid="timeline-action-logs"
          >
            <div className="flex items-center gap-1.5 text-xs font-semibold">
              <Send className="h-3.5 w-3.5 text-xblue" />
              {tx("动作回写审计", "Action Audit Log")} ({actions.length})
            </div>
            <div className="space-y-1">
              {actions.map((act) => {
                const payloadEntries = Object.entries(act.payload ?? {});
                const shortTarget = formatCompactTarget(act.target);
                const itemKey = String(act.id || act.idempotency_key);
                return (
                  <div
                    key={itemKey}
                    data-testid={`action-log-item-${act.id}`}
                    className="flex flex-wrap items-center justify-between gap-2 rounded border border-border bg-card px-2.5 py-1.5 text-[11px]"
                  >
                    <div className="flex flex-wrap items-center gap-1.5 font-mono-tech">
                      <Badge
                        variant={act.status === "DONE" ? "success" : "neutral"}
                      >
                        {tEnum(act.status)}
                      </Badge>
                      <span className="font-sans font-medium">
                        {tEnum(act.action_kind)}
                      </span>
                      <button
                        type="button"
                        title={act.target}
                        onClick={() => {
                          void navigator.clipboard
                            ?.writeText(act.target)
                            .catch(() => {});
                          setCopiedId(itemKey);
                          window.setTimeout(
                            () =>
                              setCopiedId((prev) =>
                                prev === itemKey ? null : prev
                              ),
                            1500
                          );
                        }}
                        className="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground cursor-pointer"
                      >
                        <span>→ {shortTarget}</span>
                        {copiedId === itemKey ? (
                          <Check className="h-3 w-3 text-emerald-600" />
                        ) : (
                          <Copy className="h-3 w-3 opacity-60" />
                        )}
                      </button>
                    </div>
                    <div className="flex flex-wrap items-center gap-1">
                      {payloadEntries.map(([pk, pv]) => (
                        <Badge key={pk} variant="outline">
                          {pk}:{" "}
                          {typeof pv === "number"
                            ? fmtNum(pv, 2)
                            : typeof pv === "string"
                              ? formatCompactTarget(pv)
                              : JSON.stringify(pv)}
                        </Badge>
                      ))}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
