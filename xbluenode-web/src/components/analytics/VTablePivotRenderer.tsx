import { useMemo } from "react";
import { PivotTable } from "@visactor/react-vtable";
import type { IIndicator, TYPES } from "@visactor/vtable";

export interface NormalizedPivotDimension {
  key: string;
  title: string;
}

export interface NormalizedPivotMeasure {
  key: string;
  title: string;
  min: number;
  max: number;
}

export interface VTablePivotRendererProps {
  rowDimensions: NormalizedPivotDimension[];
  measures: NormalizedPivotMeasure[];
  records: Record<string, unknown>[];
  isDark: boolean;
  showDataBars: boolean;
  showHeatmap: boolean;
  freezeHeaders: boolean;
  height?: number;
  onCellClick?: (
    field: string,
    value: unknown,
    record: Record<string, unknown>
  ) => void;
}

function readCssToken(name: string, fallback: string): string {
  if (typeof window === "undefined" || typeof document === "undefined") {
    return fallback;
  }
  const raw = window
    .getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return raw || fallback;
}

function parseNumericCell(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const cleaned = value.replace(/,/g, "").trim();
    if (!cleaned || cleaned === "-") return null;
    const parsed = Number(cleaned);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

export function computeBipolarHeatmapBg(
  value: number,
  min: number,
  max: number,
  isDark: boolean
): string {
  if (value < 0) {
    const denom = Math.max(Math.abs(min), 1e-9);
    const ratio = Math.min(1, Math.max(0.08, Math.abs(value) / denom));
    const alpha = (0.08 + ratio * (isDark ? 0.34 : 0.24)).toFixed(3);
    return isDark
      ? `rgba(251, 113, 133, ${alpha})`
      : `rgba(225, 29, 72, ${alpha})`;
  }
  const span = max - Math.min(0, min);
  const ratio =
    span > 1e-9
      ? Math.min(1, Math.max(0.06, (value - Math.min(0, min)) / span))
      : 0.25;
  const alpha = (0.06 + ratio * (isDark ? 0.34 : 0.24)).toFixed(3);
  return isDark
    ? `rgba(59, 130, 246, ${alpha})`
    : `rgba(37, 99, 235, ${alpha})`;
}

export default function VTablePivotRenderer({
  rowDimensions,
  measures,
  records,
  isDark,
  showDataBars,
  showHeatmap,
  freezeHeaders,
  height = 240,
  onCellClick,
}: VTablePivotRendererProps) {
  const palette = useMemo(() => {
    const cardBg = readCssToken("--card", isDark ? "#0f172a" : "#ffffff");
    const headerBg = readCssToken("--muted", isDark ? "#1e293b" : "#f8fafc");
    const border = readCssToken("--border", isDark ? "#1e293b" : "#e2e8f0");
    const fg = readCssToken("--foreground", isDark ? "#f8fafc" : "#0f172a");
    const mutedFg = readCssToken(
      "--muted-foreground",
      isDark ? "#94a3b8" : "#475569"
    );
    const accent = readCssToken("--xblue", isDark ? "#3b82f6" : "#2563eb");
    return { cardBg, headerBg, border, fg, mutedFg, accent };
  }, [isDark]);

  const vtableTheme = useMemo<TYPES.ITableThemeDefine>(
    () => ({
      underlayBackgroundColor: palette.cardBg,
      defaultStyle: {
        bgColor: palette.cardBg,
        color: palette.fg,
        borderColor: palette.border,
        fontSize: 12,
        fontFamily:
          "Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
      },
      headerStyle: {
        bgColor: palette.headerBg,
        color: palette.fg,
        borderColor: palette.border,
        fontSize: 12,
        fontWeight: 600,
      },
      rowHeaderStyle: {
        bgColor: palette.headerBg,
        color: palette.fg,
        borderColor: palette.border,
        fontSize: 12,
        fontWeight: 500,
      },
      cornerHeaderStyle: {
        bgColor: palette.headerBg,
        color: palette.mutedFg,
        borderColor: palette.border,
        fontSize: 12,
        fontWeight: 600,
      },
      bodyStyle: {
        bgColor: palette.cardBg,
        color: palette.fg,
        borderColor: palette.border,
        fontSize: 12,
        hover: {
          cellBgColor: isDark
            ? "rgba(59, 130, 246, 0.14)"
            : "rgba(37, 99, 235, 0.08)",
        },
      },
      frameStyle: {
        borderColor: palette.border,
        borderLineWidth: 1,
      },
    }),
    [isDark, palette]
  );

  const vtableRows = useMemo(
    () =>
      rowDimensions.map((dim) => ({
        dimensionKey: dim.key,
        title: dim.title,
        width: "auto" as const,
        minWidth: 100,
      })),
    [rowDimensions]
  );

  const vtableIndicators = useMemo<IIndicator[]>(() => {
    const monoFont =
      "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace";

    return measures.map((m): IIndicator => {
      const resolveBg = (rawVal: unknown): string => {
        if (!showHeatmap) return palette.cardBg;
        const num = parseNumericCell(rawVal);
        if (num === null) return palette.cardBg;
        return computeBipolarHeatmapBg(num, m.min, m.max, isDark);
      };

      if (showDataBars) {
        return {
          indicatorKey: m.key,
          title: m.title,
          cellType: "progressbar",
          min: Math.min(0, m.min),
          max: Math.max(1, m.max),
          barType: m.min < 0 ? "negative" : "default",
          width: "auto",
          minWidth: 118,
          style: (styleArg) => ({
            bgColor: resolveBg(styleArg.dataValue ?? styleArg.value),
            color: palette.fg,
            fontSize: 12,
            fontFamily: monoFont,
            textAlign: "right",
            barColor: isDark
              ? "rgba(59, 130, 246, 0.36)"
              : "rgba(37, 99, 235, 0.24)",
            barPositiveColor: isDark
              ? "rgba(59, 130, 246, 0.38)"
              : "rgba(37, 99, 235, 0.26)",
            barNegativeColor: isDark
              ? "rgba(251, 113, 133, 0.38)"
              : "rgba(225, 29, 72, 0.26)",
            barHeight: 16,
            barBottom: 6,
          }),
        };
      }

      return {
        indicatorKey: m.key,
        title: m.title,
        cellType: "text",
        width: "auto",
        minWidth: 104,
        style: (styleArg) => ({
          bgColor: resolveBg(styleArg.dataValue ?? styleArg.value),
          color: palette.fg,
          fontSize: 12,
          fontFamily: monoFont,
          textAlign: "right",
        }),
      };
    });
  }, [isDark, measures, palette.cardBg, palette.fg, showDataBars, showHeatmap]);

  return (
    <div
      className="w-full border-b border-border bg-card overflow-hidden"
      style={{ height: `${height}px` }}
      data-testid="vtable-canvas-stage"
      data-databars={showDataBars ? "on" : "off"}
      data-heatmap={showHeatmap ? "on" : "off"}
      data-freeze={freezeHeaders ? "on" : "off"}
    >
      <PivotTable
        records={records}
        rows={vtableRows}
        columns={[]}
        indicators={vtableIndicators}
        indicatorsAsCol={true}
        rowHierarchyType={rowDimensions.length > 1 ? "grid-tree" : "grid"}
        rowExpandLevel={3}
        columnResizeType="all"
        frozenColCount={freezeHeaders ? rowDimensions.length : 0}
        frozenRowCount={freezeHeaders ? 1 : 0}
        width="100%"
        height={height}
        theme={vtableTheme}
        onClickCell={(args) => {
          if (!onCellClick) return;
          const originRecord = args.originData;
          if (originRecord && typeof originRecord === "object") {
            const rec = originRecord as Record<string, unknown>;
            const firstDim = rowDimensions[0]?.key;
            if (firstDim && firstDim in rec) {
              onCellClick(firstDim, rec[firstDim], rec);
            }
          }
        }}
      />
    </div>
  );
}
