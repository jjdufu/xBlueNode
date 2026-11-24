import { cn } from "@/lib/utils";

export interface SimNodeCardG6Props {
  nodeId: string;
  title: string;
  nodeKey: string;
  typeName: string;
  metricKey: string | null;
  baselineLabel: string;
  interventionLabel: string;
  simulatedLabel: string;
  baselineValueText: string;
  interventionValueText: string;
  simulatedValueText: string;
  deltaBadgeText: string | null;
  deltaPct: number | null;
  p10?: number | null;
  meanVal?: number | null;
  p90?: number | null;
  acted: boolean;
  selected: boolean;
  onPath: boolean;
  isOverlay: boolean;
  dimmed: boolean;
  onSelect: (nodeId: string) => void;
}

/**
 * `228px` 3-track high-density causal micro-card (`基线 | 干预 | 推演值` + delta pill + P10–P90 4px bar)
 * mounted inside `@antv/g6` v5 via `@antv/g6-extension-react`.
 */
export function SimNodeCardG6({
  nodeId,
  title,
  nodeKey,
  typeName,
  metricKey,
  baselineLabel,
  interventionLabel,
  simulatedLabel,
  baselineValueText,
  interventionValueText,
  simulatedValueText,
  deltaBadgeText,
  deltaPct,
  p10 = null,
  meanVal = null,
  p90 = null,
  acted,
  selected,
  onPath,
  isOverlay,
  dimmed,
  onSelect,
}: SimNodeCardG6Props) {
  const absPct = deltaPct !== null ? Math.abs(deltaPct) : 0;
  let accentColor = "#71717a";
  if (acted) {
    accentColor = "#2563eb";
  } else if (deltaPct !== null && deltaPct < 0) {
    accentColor = absPct >= 15 ? "#dc2626" : "#d97706";
  } else if (deltaPct !== null && deltaPct > 0) {
    accentColor = "#059669";
  }

  const hasCi =
    typeof p10 === "number" &&
    typeof p90 === "number" &&
    Number.isFinite(p10) &&
    Number.isFinite(p90);

  let meanRatio = 0.5;
  if (hasCi && typeof meanVal === "number" && p90 > p10) {
    meanRatio = Math.max(0.08, Math.min(0.92, (meanVal - p10) / (p90 - p10)));
  }

  return (
    <div
      data-testid={`impact-graph-node-${nodeId}`}
      data-dimmed={dimmed ? "true" : "false"}
      onClick={(e) => {
        e.stopPropagation();
        onSelect(nodeId);
      }}
      style={{
        width: "228px",
        opacity: dimmed ? 0.35 : 1,
      }}
      className={cn(
        "rounded-md border bg-card text-card-foreground shadow-xs transition-opacity select-none cursor-pointer overflow-hidden",
        selected || onPath
          ? "border-xblue ring-2 ring-xblue/35 shadow-md"
          : "border-border hover:border-foreground/45",
        isOverlay && "border-dashed border-xblue"
      )}
    >
      <div className="h-1.5 w-full" style={{ backgroundColor: accentColor }} />

      <div className="p-2 space-y-1.5">
        <div className="flex items-start justify-between gap-1.5">
          <div className="min-w-0">
            <div className="truncate text-[11px] font-semibold leading-tight text-foreground">
              {title}
            </div>
            <div className="truncate font-mono-tech text-[9.5px] text-muted-foreground">
              {nodeKey} · {typeName}
              {metricKey ? ` · ${metricKey}` : ""}
            </div>
          </div>

          {deltaBadgeText && (
            <span
              data-testid={`sim-card-delta-${nodeId}`}
              className={cn(
                "shrink-0 rounded px-1.5 py-0.5 font-mono-tech text-[10px] font-semibold tabular-nums",
                acted
                  ? "bg-xblue-soft text-xblue"
                  : (deltaPct ?? 0) < 0
                    ? "bg-red-500/15 text-red-600 dark:text-red-400"
                    : "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400"
              )}
            >
              {deltaBadgeText}
            </span>
          )}
        </div>

        {/* 3-Track Aligned Numeric Grid: Baseline | Intervention | Simulated */}
        <div
          data-testid={`sim-card-tracks-${nodeId}`}
          className="grid grid-cols-3 gap-1 rounded bg-muted/45 px-1.5 py-1 text-center"
        >
          <div className="min-w-0">
            <div className="text-[9px] text-muted-foreground leading-tight">
              {baselineLabel}
            </div>
            <div className="truncate font-mono-tech text-[10.5px] font-medium tabular-nums">
              {baselineValueText}
            </div>
          </div>
          <div className="min-w-0 border-x border-border/60 px-0.5">
            <div className="text-[9px] text-muted-foreground leading-tight">
              {interventionLabel}
            </div>
            <div
              className={cn(
                "truncate font-mono-tech text-[10.5px] font-medium tabular-nums",
                acted ? "text-xblue font-semibold" : "text-muted-foreground"
              )}
            >
              {interventionValueText}
            </div>
          </div>
          <div className="min-w-0">
            <div className="text-[9px] text-muted-foreground leading-tight">
              {simulatedLabel}
            </div>
            <div className="truncate font-mono-tech text-[10.5px] font-semibold tabular-nums text-foreground">
              {simulatedValueText}
            </div>
          </div>
        </div>

        {/* Optional Monte Carlo [P10 — Mean — P90] 4px Confidence Band */}
        {hasCi && p10 !== null && p90 !== null && (
          <div data-testid={`sim-card-ci-${nodeId}`} className="space-y-0.5 pt-0.5">
            <div className="relative h-1 w-full rounded-full bg-muted overflow-hidden">
              <div
                className="absolute inset-y-0 left-[12%] right-[12%] rounded-full"
                style={{ backgroundColor: accentColor, opacity: 0.45 }}
              />
              <span
                style={{ left: `${Math.round(meanRatio * 100)}%` }}
                className="absolute top-0 bottom-0 w-1 -translate-x-1/2 rounded-full bg-foreground"
              />
            </div>
            <div className="flex items-center justify-between font-mono-tech text-[9px] text-muted-foreground tabular-nums">
              <span>P10 {p10.toFixed(1)}</span>
              <span>P90 {p90.toFixed(1)}</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
