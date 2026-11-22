import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import type { AlgoTopEntry } from "@/lib/api/xblue";
import type { AlgoVisualMode } from "@/components/xbluegraph/AlgoPanel";
import {
  DARK_TYPE_PALETTE,
  LIGHT_TYPE_PALETTE,
  resolveNodeTypeColor,
  type GraphColorMode,
} from "./SigmaCanvas";

export interface AlgoLegendProps {
  colorMode: GraphColorMode;
  colorMetricKey: string;
  distinctTypes: string[];
  algoMode: AlgoVisualMode;
  algoEntries: AlgoTopEntry[];
  isDark?: boolean;
}

export function AlgoLegend({
  colorMode,
  colorMetricKey,
  distinctTypes,
  algoMode,
  algoEntries,
  isDark: isDarkProp,
}: AlgoLegendProps) {
  const { tx } = useI18n();
  const { isDark: themeDark } = useTheme();
  const isDark = isDarkProp ?? themeDark;
  const palette = isDark ? DARK_TYPE_PALETTE : LIGHT_TYPE_PALETTE;

  if (colorMode === "METRIC") {
    return (
      <div
        className="flex flex-wrap items-center gap-3 px-3 py-1.5 border-t border-border text-[11px] text-muted-foreground"
        data-testid="algo-legend"
      >
        <span className="font-medium text-foreground">
          {tx("指标热度", "Metric Heat")} ({colorMetricKey || "—"}):
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-[#71717a]" />
          {tx("低值", "Low")}
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-[#06b6d4]" />
          {tx("中值", "Medium")}
        </span>
        <span className="inline-flex items-center gap-1">
          <span
            className="h-2.5 w-2.5 rounded-full"
            style={{ backgroundColor: isDark ? "#3b82f6" : "#2563eb" }}
          />
          {tx("高值", "High")}
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
    ).slice(0, 8);

    return (
      <div
        className="flex flex-wrap items-center gap-3 px-3 py-1.5 border-t border-border text-[11px] text-muted-foreground"
        data-testid="algo-legend"
      >
        <span className="font-medium text-foreground">
          {tx("算法映射", "Algorithm Mapping")} ({algoMode}):
        </span>
        {communities.length > 0 ? (
          communities.map((c) => (
            <span
              key={c}
              className="inline-flex items-center gap-1 font-mono-tech"
            >
              <span
                className="h-2.5 w-2.5 rounded-full"
                style={{
                  backgroundColor: palette[Math.abs(c) % palette.length],
                }}
              />
              C-{c}
            </span>
          ))
        ) : (
          <span>
            {tx(
              "节点尺寸按中心性得分缩放",
              "Node size scaled by centrality score"
            )}
          </span>
        )}
      </div>
    );
  }

  return (
    <div
      className="flex flex-wrap items-center gap-3 px-3 py-1.5 border-t border-border text-[11px] text-muted-foreground"
      data-testid="algo-legend"
    >
      <span className="font-medium text-foreground">
        {tx("节点类型", "Node Types")}:
      </span>
      {distinctTypes.length === 0 ? (
        <span>{tx("暂无类型", "No types")}</span>
      ) : (
        distinctTypes.map((t, idx) => (
          <span
            key={t}
            className="inline-flex items-center gap-1 font-mono-tech"
          >
            <span
              className="h-2.5 w-2.5 rounded-full"
              style={{
                backgroundColor: resolveNodeTypeColor(t, idx, isDark),
              }}
            />
            {t}
          </span>
        ))
      )}
    </div>
  );
}
