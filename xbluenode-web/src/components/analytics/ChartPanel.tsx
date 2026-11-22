import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  BarChart3,
  Hash,
  LineChart,
  PieChart,
  Sparkles,
  Table2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { ChartSuggestion, QueryColumn } from "@/lib/api/analytics";
import type { DataType } from "@/lib/api/enums";
import { useTheme } from "@/lib/theme";
import { Badge } from "@/components/ui/primitives";

export type SupportedChartKind =
  | "BAR_CHART"
  | "LINE_CHART"
  | "PIE_CHART"
  | "KPI_CARD"
  | "TABLE";

export interface ChartPanelProps {
  columns: QueryColumn[];
  rows: Record<string, unknown>[];
  suggestion?: ChartSuggestion | null;
  testId?: string;
}

const NUMERIC_DATA_TYPES: ReadonlySet<DataType> = new Set([
  "INT64",
  "FLOAT64",
  "DECIMAL",
]);

const TEMPORAL_DATA_TYPES: ReadonlySet<DataType> = new Set([
  "DATE",
  "TIMESTAMP",
]);

const PREFERRED_MEASURE_RE =
  /amount|total|revenue|value|qty|quantity|price|cost|profit|sales|paid|balance|inventory|demand|output|count|cnt|sum|avg|score|metric|ratio|rate/i;

function isIdentifierLikeColumn(colName: string): boolean {
  const lower = colName.toLowerCase().trim();
  return (
    lower === "id" ||
    lower === "pk" ||
    lower.endsWith("_id") ||
    lower.startsWith("id_") ||
    lower.endsWith("_key") ||
    lower.endsWith("_code") ||
    lower.endsWith("_no") ||
    lower === "ordinal" ||
    lower === "version" ||
    lower.endsWith("_version")
  );
}

function parseNumericCell(val: unknown): number | null {
  if (typeof val === "number") {
    return Number.isFinite(val) ? val : null;
  }
  if (typeof val === "string") {
    const cleaned = val.replace(/,/g, "").trim();
    if (!cleaned) return null;
    const parsed = Number(cleaned);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function isSupportedChartKind(
  kind: string | undefined
): kind is SupportedChartKind {
  return (
    kind === "BAR_CHART" ||
    kind === "LINE_CHART" ||
    kind === "PIE_CHART" ||
    kind === "KPI_CARD" ||
    kind === "TABLE"
  );
}

export function ChartPanel({
  columns,
  rows,
  suggestion = null,
  testId = "analytics-chart-panel",
}: ChartPanelProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { isDark } = useTheme();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const [manualKind, setManualKind] = useState<SupportedChartKind | null>(null);
  const [manualXField, setManualXField] = useState<string>("");
  const [manualYField, setManualYField] = useState<string>("");

  const chartKindOptions: ReadonlyArray<{
    kind: SupportedChartKind;
    label: string;
    icon: typeof BarChart3;
  }> = [
    { kind: "BAR_CHART", label: tx("柱状", "Bar"), icon: BarChart3 },
    { kind: "LINE_CHART", label: tx("折线", "Line"), icon: LineChart },
    { kind: "PIE_CHART", label: tx("饼图", "Pie"), icon: PieChart },
    { kind: "KPI_CARD", label: tx("指标", "KPI"), icon: Hash },
    { kind: "TABLE", label: tx("表格", "Table"), icon: Table2 },
  ];

  const schemaSignature = useMemo(
    () =>
      `${columns.map((c) => `${c.name}:${c.data_type}`).join("|")}::${
        suggestion?.kind ?? ""
      }::${suggestion?.x_field ?? ""}::${(suggestion?.y_fields ?? []).join(",")}`,
    [columns, suggestion]
  );

  useEffect(() => {
    setManualKind(null);
    setManualXField("");
    setManualYField("");
  }, [schemaSignature]);

  const numericColumns = useMemo(() => {
    return columns.filter((col) => {
      if (NUMERIC_DATA_TYPES.has(col.data_type)) return true;
      for (let i = 0; i < Math.min(rows.length, 10); i += 1) {
        const sample = rows[i]?.[col.name];
        if (sample !== null && sample !== undefined) {
          return parseNumericCell(sample) !== null;
        }
      }
      return false;
    });
  }, [columns, rows]);

  const measureColumns = useMemo(
    () => numericColumns.filter((col) => !isIdentifierLikeColumn(col.name)),
    [numericColumns]
  );

  const temporalColumns = useMemo(() => {
    return columns.filter(
      (col) =>
        TEMPORAL_DATA_TYPES.has(col.data_type) ||
        /date|time|month|day|period|ts|created_at|updated_at/i.test(col.name)
    );
  }, [columns]);

  const recommendedKind = useMemo<SupportedChartKind>(() => {
    if (suggestion && isSupportedChartKind(suggestion.kind)) {
      return suggestion.kind;
    }
    if (columns.length === 0 || rows.length === 0) {
      return "BAR_CHART";
    }
    const effectiveMeasureCount =
      measureColumns.length > 0 ? measureColumns.length : numericColumns.length;
    if (
      rows.length === 1 &&
      effectiveMeasureCount >= 1 &&
      columns.length <= 2
    ) {
      return "KPI_CARD";
    }
    if (temporalColumns.length >= 1 && measureColumns.length >= 1) {
      return "LINE_CHART";
    }
    if (measureColumns.length >= 1) {
      return "BAR_CHART";
    }
    return "TABLE";
  }, [
    columns.length,
    measureColumns.length,
    numericColumns.length,
    rows.length,
    suggestion,
    temporalColumns.length,
  ]);

  const activeKind: SupportedChartKind = manualKind ?? recommendedKind;

  const defaultXField = useMemo(() => {
    if (
      suggestion?.x_field &&
      columns.some((c) => c.name === suggestion.x_field)
    ) {
      return suggestion.x_field;
    }
    if (temporalColumns[0]) return temporalColumns[0].name;
    const categoricalNonId = columns.find(
      (c) =>
        !numericColumns.some((n) => n.name === c.name) &&
        !isIdentifierLikeColumn(c.name)
    );
    if (categoricalNonId) return categoricalNonId.name;
    const nonNumeric = columns.find(
      (c) => !numericColumns.some((n) => n.name === c.name)
    );
    return nonNumeric?.name ?? columns[0]?.name ?? "";
  }, [columns, numericColumns, suggestion, temporalColumns]);

  const defaultYField = useMemo(() => {
    const suggestedY = suggestion?.y_fields?.[0];
    if (suggestedY && columns.some((c) => c.name === suggestedY)) {
      return suggestedY;
    }
    const nonXMeasures = measureColumns.filter((c) => c.name !== defaultXField);
    const preferredMeasure = nonXMeasures.find((c) =>
      PREFERRED_MEASURE_RE.test(c.name)
    );
    if (preferredMeasure) return preferredMeasure.name;
    if (nonXMeasures[0]) return nonXMeasures[0].name;
    if (measureColumns[0]) return measureColumns[0].name;

    const fallbackNumeric = numericColumns.find(
      (c) => c.name !== defaultXField
    );
    return (
      fallbackNumeric?.name ??
      numericColumns[0]?.name ??
      columns[columns.length - 1]?.name ??
      ""
    );
  }, [columns, defaultXField, measureColumns, numericColumns, suggestion]);

  const xField =
    manualXField && columns.some((c) => c.name === manualXField)
      ? manualXField
      : defaultXField;
  const yField =
    manualYField && columns.some((c) => c.name === manualYField)
      ? manualYField
      : defaultYField;

  const hasData = columns.length > 0 && rows.length > 0;

  const isCanvasChart =
    (activeKind === "BAR_CHART" ||
      activeKind === "LINE_CHART" ||
      activeKind === "PIE_CHART") &&
    hasData;

  useEffect(() => {
    if (!isCanvasChart) {
      if (chartRef.current) {
        chartRef.current.dispose();
        chartRef.current = null;
      }
      return;
    }

    const el = containerRef.current;
    if (!el) return;

    const chart = echarts.init(el);
    chartRef.current = chart;

    const handleResize = () => {
      if (chartRef.current && !chartRef.current.isDisposed()) {
        chartRef.current.resize();
      }
    };

    const resizeObserver = new ResizeObserver(handleResize);
    resizeObserver.observe(el);
    window.addEventListener("resize", handleResize);

    return () => {
      window.removeEventListener("resize", handleResize);
      resizeObserver.disconnect();
      if (!chart.isDisposed()) {
        chart.dispose();
      }
      chartRef.current = null;
    };
  }, [isCanvasChart]);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || chart.isDisposed() || !isCanvasChart) return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const gridBorder = isDark ? "#27272a" : "#e4e4e7";
    const cardBg = isDark ? "#18181b" : "#f4f4f5";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const piePalette = isDark
      ? ["#3b82f6", "#60a5fa", "#93c5fd", "#a1a1aa", "#71717a", "#52525b"]
      : ["#2563eb", "#3b82f6", "#60a5fa", "#52525b", "#71717a", "#a1a1aa"];

    const slicedRows = rows.slice(0, 50);
    const categories = slicedRows.map((r, i) => {
      const raw = xField ? r[xField] : i + 1;
      return raw === null || raw === undefined ? "NULL" : String(raw);
    });
    const values = slicedRows.map((r) => {
      const raw = yField ? r[yField] : 0;
      return parseNumericCell(raw) ?? 0;
    });

    let option: echarts.EChartsOption;

    if (activeKind === "PIE_CHART") {
      option = {
        animation: false,
        color: piePalette,
        tooltip: {
          trigger: "item",
          backgroundColor: cardBg,
          borderColor: gridBorder,
          textStyle: { color: fgColor, fontSize: 11 },
        },
        legend: {
          bottom: 0,
          textStyle: { color: mutedColor, fontSize: 10 },
        },
        series: [
          {
            name: yField || tx("指标", "Value"),
            type: "pie",
            radius: ["36%", "66%"],
            center: ["50%", "44%"],
            label: { color: fgColor, fontSize: 10 },
            data: categories.slice(0, 16).map((name, idx) => ({
              name,
              value: values[idx] ?? 0,
            })),
          },
        ],
      };
    } else {
      option = {
        animation: false,
        grid: {
          top: 24,
          right: 16,
          bottom: 36,
          left: 52,
        },
        tooltip: {
          trigger: "axis",
          backgroundColor: cardBg,
          borderColor: gridBorder,
          textStyle: { color: fgColor, fontSize: 11 },
        },
        xAxis: {
          type: "category",
          data: categories,
          axisLine: { lineStyle: { color: gridBorder } },
          axisLabel: {
            color: mutedColor,
            fontSize: 10,
            interval: "auto",
            rotate: categories.length > 10 ? 20 : 0,
          },
        },
        yAxis: {
          type: "value",
          splitLine: { lineStyle: { color: gridBorder, type: "dashed" } },
          axisLabel: { color: mutedColor, fontSize: 10 },
        },
        series: [
          activeKind === "LINE_CHART"
            ? {
                name: yField || tx("指标", "Value"),
                type: "line",
                smooth: false,
                symbolSize: 5,
                data: values,
                lineStyle: { color: accentBlue, width: 2 },
                itemStyle: { color: accentBlue },
                areaStyle: { color: accentBlue, opacity: 0.08 },
              }
            : {
                name: yField || tx("指标", "Value"),
                type: "bar",
                barMaxWidth: 32,
                data: values,
                itemStyle: {
                  color: accentBlue,
                  borderRadius: [2, 2, 0, 0],
                },
              },
        ],
      };
    }

    chart.setOption(option, true);
    chart.resize();
  }, [activeKind, isCanvasChart, isDark, rows, tx, xField, yField]);

  const kpiSummary = useMemo(() => {
    if (!hasData) return null;
    const targetCol = yField || columns[0]?.name || "";
    const firstVal = rows[0]?.[targetCol];
    const parsedNum = parseNumericCell(firstVal);
    const displayValue =
      firstVal === null || firstVal === undefined
        ? "NULL"
        : parsedNum !== null && !isIdentifierLikeColumn(targetCol)
          ? fmtNum(parsedNum, 2)
          : String(firstVal);
    return {
      label: suggestion?.title || targetCol || tx("指标值", "Value"),
      value: displayValue,
      subLabel:
        rows.length > 1
          ? tx(
              `首行数值 · 共 ${fmtNum(rows.length, 0)} 行`,
              `First row · ${fmtNum(rows.length, 0)} rows`
            )
          : tx("单值聚合结果", "Single aggregated value"),
    };
  }, [columns, fmtNum, hasData, rows, suggestion, tx, yField]);

  return (
    <div
      className="rounded-md border border-border bg-card overflow-hidden"
      data-testid={testId}
    >
      {/* Chart Header & Switcher */}
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border bg-muted/30 px-3 py-2 text-xs">
        <div className="flex items-center gap-2 min-w-0">
          <Sparkles className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span className="font-medium truncate">
            {suggestion?.title || tx("图表分析", "Chart View")}
          </span>
          {hasData && (
            <Badge variant="xblue" data-testid="chart-recommended-badge">
              {tx("推荐", "Suggested")} {tEnum(recommendedKind)}
            </Badge>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          {hasData &&
            columns.length > 1 &&
            (activeKind === "BAR_CHART" ||
              activeKind === "LINE_CHART" ||
              activeKind === "PIE_CHART") && (
              <div className="flex items-center gap-1 text-[11px] shrink-0">
                <span className="text-muted-foreground">X:</span>
                <select
                  value={xField}
                  onChange={(e) => setManualXField(e.target.value)}
                  className="h-6 max-w-[110px] rounded border border-input bg-background px-1.5 font-mono-tech text-[11px]"
                  data-testid="chart-x-select"
                >
                  {columns.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name}
                    </option>
                  ))}
                </select>
                <span className="text-muted-foreground ml-1">Y:</span>
                <select
                  value={yField}
                  onChange={(e) => setManualYField(e.target.value)}
                  className="h-6 max-w-[110px] rounded border border-input bg-background px-1.5 font-mono-tech text-[11px]"
                  data-testid="chart-y-select"
                >
                  {columns.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name}
                    </option>
                  ))}
                </select>
              </div>
            )}

          <div className="inline-flex items-center rounded border border-border bg-background p-0.5 overflow-x-auto max-w-full">
            {chartKindOptions.map((opt) => {
              const Icon = opt.icon;
              const selected = activeKind === opt.kind;
              return (
                <button
                  key={opt.kind}
                  type="button"
                  onClick={() => setManualKind(opt.kind)}
                  data-testid={`chart-kind-${opt.kind}`}
                  className={`inline-flex items-center gap-1 rounded px-2 py-1 text-[11px] font-medium whitespace-nowrap shrink-0 transition-colors cursor-pointer ${
                    selected
                      ? "bg-xblue-soft text-xblue"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                >
                  <Icon className="h-3 w-3 shrink-0" />
                  <span>{opt.label}</span>
                </button>
              );
            })}
          </div>
        </div>
      </div>

      {/* Chart Content */}
      {!hasData ? (
        <div
          key="chart-empty"
          className="flex h-52 items-center justify-center px-4 text-xs text-muted-foreground"
        >
          {tx("暂无可绘制数据", "No data to visualize")}
        </div>
      ) : activeKind === "KPI_CARD" && kpiSummary ? (
        <div
          key="chart-kpi"
          className="flex h-52 flex-col items-center justify-center gap-2 p-6 text-center"
          data-testid="chart-kpi-card"
        >
          <span className="font-mono-tech text-xs uppercase tracking-wider text-muted-foreground">
            {kpiSummary.label}
          </span>
          <span className="font-mono-tech text-3xl font-semibold tracking-tight text-xblue">
            {kpiSummary.value}
          </span>
          <span className="text-[11px] text-muted-foreground">
            {kpiSummary.subLabel}
          </span>
        </div>
      ) : activeKind === "TABLE" ? (
        <div
          key="chart-table"
          className="max-h-52 overflow-auto"
          data-testid="chart-compact-table"
        >
          <table className="w-full border-collapse text-left text-xs">
            <thead className="sticky top-0 bg-muted border-b border-border">
              <tr>
                {columns.map((c) => (
                  <th
                    key={c.name}
                    className="px-2.5 py-1.5 font-mono-tech text-[11px] font-medium whitespace-nowrap"
                  >
                    {c.name}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.slice(0, 15).map((r, rIdx) => (
                <tr
                  key={rIdx}
                  className="border-b border-border last:border-b-0"
                >
                  {columns.map((c) => (
                    <td
                      key={c.name}
                      className="px-2.5 py-1 font-mono-tech text-[11px] truncate max-w-[200px]"
                    >
                      {r[c.name] === null || r[c.name] === undefined
                        ? "NULL"
                        : String(r[c.name])}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div
          key="chart-echarts"
          className="h-56 w-full p-2"
          data-testid="chart-echarts-canvas"
        >
          <div ref={containerRef} className="h-full w-full" />
        </div>
      )}
    </div>
  );
}
