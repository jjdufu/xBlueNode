import type { AlgoTopEntry } from "@/lib/api/xblue";
import type { AlgoVisualMode } from "@/components/xbluegraph/AlgoPanel";
import type { GraphColorMode } from "./SigmaCanvas";

export interface AlgoLegendProps {
  colorMode: GraphColorMode;
  colorMetricKey: string;
  distinctTypes: string[];
  algoMode: AlgoVisualMode;
  algoEntries: AlgoTopEntry[];
  isDark: boolean;
}

const LIGHT_PALETTE = [
  "#2563eb",
  "#09090b",
  "#52525b",
  "#1d4ed8",
  "#3f3f46",
  "#60a5fa",
];

const DARK_PALETTE = [
  "#3b82f6",
  "#fafafa",
  "#a1a1aa",
  "#60a5fa",
  "#d4d4d8",
  "#93c5fd",
];

export function AlgoLegend({
  colorMode,
  colorMetricKey,
  distinctTypes,
  algoMode,
  algoEntries,
  isDark,
}: AlgoLegendProps) {
  const palette = isDark ? DARK_PALETTE : LIGHT_PALETTE;

  if (colorMode === "METRIC") {
    return (
      <div
        className="flex flex-wrap items-center gap-3 px-3 py-1.5 border-t border-border text-[11px] text-muted-foreground"
        data-testid="algo-legend"
      >
        <span className="font-medium text-foreground">
          指标热度 ({colorMetricKey || "—"}):
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-[#71717a]" /> 低值
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-[#60a5fa]" /> 中值
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-[#2563eb]" /> 高值
        </span>
      </div>
    );
  }

  if (colorMode === "ALGO" || algoMode !== "NONE") {
    const communities = Array.from(
      new Set(
        algoEntries
          .map((e) => e.community)
          .filter((c): c is number => c !== null && c !== undefined)
      )
    ).slice(0, 6);

    return (
      <div
        className="flex flex-wrap items-center gap-3 px-3 py-1.5 border-t border-border text-[11px] text-muted-foreground"
        data-testid="algo-legend"
      >
        <span className="font-medium text-foreground">
          图算法映射 ({algoMode}):
        </span>
        {communities.length > 0 ? (
          communities.map((c) => (
            <span key={c} className="inline-flex items-center gap-1 font-mono-tech">
              <span
                className="h-2.5 w-2.5 rounded-full"
                style={{ backgroundColor: palette[Math.abs(c) % palette.length] }}
              />
              C-{c}
            </span>
          ))
        ) : (
          <span>节点尺寸按算法中心性得分缩放</span>
        )}
      </div>
    );
  }

  return (
    <div
      className="flex flex-wrap items-center gap-3 px-3 py-1.5 border-t border-border text-[11px] text-muted-foreground"
      data-testid="algo-legend"
    >
      <span className="font-medium text-foreground">节点类型:</span>
      {distinctTypes.length === 0 ? (
        <span>暂无类型</span>
      ) : (
        distinctTypes.map((t, idx) => (
          <span key={t} className="inline-flex items-center gap-1 font-mono-tech">
            <span
              className="h-2.5 w-2.5 rounded-full"
              style={{ backgroundColor: palette[idx % palette.length] }}
            />
            {t}
          </span>
        ))
      )}
    </div>
  );
}
