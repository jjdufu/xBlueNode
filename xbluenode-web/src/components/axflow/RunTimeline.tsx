import { useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  FileCode2,
  Loader2,
  Send,
} from "lucide-react";
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
  const [expandedStepIds, setExpandedStepIds] = useState<
    Record<string, boolean>
  >({});

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
        className="flex flex-col items-center justify-center rounded-md border border-border bg-card p-10 text-xs text-muted-foreground"
        data-testid="axflow-run-timeline-empty"
      >
        选择左侧一条运行记录或点击「运行流水线」查看逐节点甘特时间线与输入/输出快照
      </div>
    );
  }

  const toggleExpand = (key: string) => {
    setExpandedStepIds((prev) => ({ ...prev, [key]: !prev[key] }));
  };

  return (
    <div
      className="flex flex-col rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-run-timeline"
    >
      {/* Run Summary Header */}
      <div className="p-3 border-b border-border flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-semibold">步骤执行甘特时间线</span>
          <span data-testid="timeline-run-status">
            {renderRunStatusBadge(run.status)}
          </span>
          <Badge variant={run.environment === "PROD" ? "xblue" : "neutral"}>
            {run.environment} v{run.version}
          </Badge>
          <span className="font-mono-tech text-[11px] text-muted-foreground">
            Run #{run.id.slice(0, 8)}
          </span>
        </div>

        <div className="flex items-center gap-2 text-[11px] text-muted-foreground font-mono-tech">
          {polling && (
            <span className="inline-flex items-center gap-1 text-xblue">
              <Loader2 className="h-3 w-3 animate-spin" />
              实时同步中
            </span>
          )}
          <span>
            总耗时:{" "}
            {formatDurationMs(
              run.started_at || run.created_at,
              run.finished_at
            )}
          </span>
        </div>
      </div>

      {/* Gantt Step Rows */}
      <div className="divide-y divide-border">
        {steps.length === 0 ? (
          <div className="py-8 text-center text-xs text-muted-foreground">
            正在等待调度器派发根步骤...
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
                className="p-2.5 space-y-2"
              >
                <div
                  onClick={() => toggleExpand(rowKey)}
                  className="grid grid-cols-12 items-center gap-2 cursor-pointer text-xs"
                >
                  {/* Node Name & Attempt */}
                  <div className="col-span-5 sm:col-span-4 flex items-center gap-1.5 min-w-0">
                    {expanded ? (
                      <ChevronDown className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                    ) : (
                      <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                    )}
                    <div className="min-w-0">
                      <div className="truncate font-medium">
                        {nodeLabelMap[st.node_id] ?? st.node_id}
                      </div>
                      <div className="font-mono-tech text-[10px] text-muted-foreground">
                        {st.node_id} · 尝试 #{st.attempt}
                      </div>
                    </div>
                  </div>

                  {/* Gantt Proportional Bar */}
                  <div className="col-span-4 sm:col-span-5">
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

                  {/* Status & Duration */}
                  <div className="col-span-3 flex items-center justify-end gap-1.5">
                    <span
                      data-testid={`timeline-step-status-${st.node_id}`}
                      className={cn(
                        "rounded border px-1.5 py-0.5 font-mono-tech text-[10px] font-medium",
                        stepBadgeClass(st.status)
                      )}
                    >
                      {st.status}
                    </span>
                    <span className="hidden md:inline font-mono-tech text-[10px] text-muted-foreground w-12 text-right">
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
                          渲染后输入 (input)
                        </div>
                        <pre className="max-h-36 overflow-auto rounded border border-border bg-card p-2 font-mono-tech text-[10px]">
                          {JSON.stringify(st.input, null, 2)}
                        </pre>
                      </div>

                      <div>
                        <div className="font-medium text-muted-foreground mb-1">
                          节点输出 (output)
                        </div>
                        {st.output_ref ? (
                          <div className="inline-flex items-center gap-1.5 rounded border border-border bg-card px-2.5 py-1.5 font-mono-tech text-[10px]">
                            <FileCode2 className="h-3.5 w-3.5 text-xblue" />
                            大对象落盘引用: {st.output_ref}
                          </div>
                        ) : (
                          <pre className="max-h-36 overflow-auto rounded border border-border bg-card p-2 font-mono-tech text-[10px]">
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

      {/* Writeback Action Logs (`ax_action_log`) */}
      {actions.length > 0 && (
        <div
          className="border-t border-border bg-muted/20 p-3 space-y-2"
          data-testid="timeline-action-logs"
        >
          <div className="flex items-center gap-1.5 text-xs font-semibold">
            <Send className="h-3.5 w-3.5 text-xblue" />
            动作回写审计流水 ({actions.length})
          </div>
          <div className="space-y-1.5">
            {actions.map((act) => (
              <div
                key={act.id || act.idempotency_key}
                data-testid={`action-log-item-${act.id}`}
                className="flex flex-wrap items-center justify-between gap-2 rounded border border-border bg-card px-2.5 py-1.5 text-[11px]"
              >
                <div className="flex items-center gap-1.5 font-mono-tech">
                  <Badge
                    variant={act.status === "DONE" ? "xblue" : "neutral"}
                  >
                    {act.status}
                  </Badge>
                  <span className="font-medium">{act.action_kind}</span>
                  <span className="text-muted-foreground">→ {act.target}</span>
                </div>
                <span className="font-mono-tech text-[10px] text-muted-foreground">
                  {JSON.stringify(act.payload)}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
