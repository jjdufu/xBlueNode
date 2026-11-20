import { useEffect, useMemo, useRef } from "react";
import * as echarts from "echarts";
import type { WidgetKind } from "@/lib/api/enums";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import {
  buildColumnIndexMap,
  findFirstNumericColIndex,
  getCell,
  parseThresholds,
  readNumber,
  readString,
  readStringArray,
  resolveThresholdColor,
  toNumericValue,
} from "../utils";

export interface EChartsWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  onDataPointClick?: (field: string, value: unknown) => void;
}

const CHART_PALETTE_LIGHT = [
  "#2563eb",
  "#09090b",
  "#52525b",
  "#1d4ed8",
  "#71717a",
  "#60a5fa",
];

const CHART_PALETTE_DARK = [
  "#3b82f6",
  "#fafafa",
  "#a1a1aa",
  "#60a5fa",
  "#d4d4d8",
  "#93c5fd",
];

function extractClickValue(params: echarts.ECElementEvent): unknown {
  if (params.name && params.name.trim().length > 0) {
    return params.name;
  }
  if (typeof params.data === "object" && params.data !== null) {
    const rec = params.data as Record<string, unknown>;
    if ("nodeId" in rec) return rec.nodeId;
    if ("name" in rec) return rec.name;
  }
  return params.value;
}

function buildChartOption(
  kind: WidgetKind,
  widget: WidgetRecord,
  data: WidgetDataResponse,
  isDark: boolean
): { option: echarts.EChartsOption; clickField: string } {
  const fg = isDark ? "#fafafa" : "#09090b";
  const muted = isDark ? "#a1a1aa" : "#71717a";
  const gridLine = isDark ? "#27272a" : "#e4e4e7";
  const cardBg = isDark ? "#18181b" : "#ffffff";
  const palette = isDark ? CHART_PALETTE_DARK : CHART_PALETTE_LIGHT;

  const colMap = buildColumnIndexMap(data);
  const rows = data.rows;

  const baseTooltip: echarts.TooltipComponentOption = {
    trigger: kind === "LINE_CHART" || kind === "BAR_CHART" ? "axis" : "item",
    backgroundColor: cardBg,
    borderColor: gridLine,
    textStyle: { color: fg, fontSize: 12 },
  };

  if (kind === "LINE_CHART" || kind === "BAR_CHART") {
    const xField =
      readString(widget.encoding, "x") ?? data.columns[0]?.name ?? "x";
    const xIdx = colMap.get(xField) ?? 0;
    const seriesField = readString(widget.encoding, "series");
    const yFieldsEnc = readStringArray(widget.encoding, "y");
    const yFields =
      yFieldsEnc.length > 0
        ? yFieldsEnc
        : [
            data.columns[findFirstNumericColIndex(data, new Set([xIdx]))]?.name ??
              data.columns[1]?.name ??
              "y",
          ];

    if (seriesField && colMap.has(seriesField)) {
      const sIdx = colMap.get(seriesField) ?? -1;
      const yName = yFields[0] ?? "value";
      const yIdx = colMap.get(yName) ?? findFirstNumericColIndex(data, new Set([xIdx, sIdx]));

      const categories: string[] = [];
      const catSet = new Set<string>();
      const seriesNames: string[] = [];
      const serSet = new Set<string>();
      const matrix = new Map<string, number>();

      for (const r of rows) {
        const xv = String(r[xIdx] ?? "");
        const sv = String(r[sIdx] ?? "");
        const yv = toNumericValue(r[yIdx]) ?? 0;
        if (!catSet.has(xv)) {
          catSet.add(xv);
          categories.push(xv);
        }
        if (!serSet.has(sv)) {
          serSet.add(sv);
          seriesNames.push(sv);
        }
        matrix.set(`${sv}::${xv}`, (matrix.get(`${sv}::${xv}`) ?? 0) + yv);
      }

      return {
        clickField: xField,
        option: {
          animation: false,
          color: palette,
          tooltip: baseTooltip,
          legend: { top: 0, textStyle: { color: muted, fontSize: 11 } },
          grid: { top: 30, right: 16, bottom: 26, left: 44, containLabel: true },
          xAxis: {
            type: "category",
            data: categories,
            axisLabel: { color: muted, fontSize: 11 },
            axisLine: { lineStyle: { color: gridLine } },
          },
          yAxis: {
            type: "value",
            axisLabel: { color: muted, fontSize: 11 },
            splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
          },
          series: seriesNames.map((sn) => ({
            name: sn,
            type: kind === "LINE_CHART" ? "line" : "bar",
            smooth: kind === "LINE_CHART",
            barMaxWidth: 36,
            data: categories.map((c) => matrix.get(`${sn}::${c}`) ?? 0),
          })),
        },
      };
    }

    const categories = rows.map((r) => String(getCell(r, colMap, xField, 0) ?? ""));
    return {
      clickField: xField,
      option: {
        animation: false,
        color: palette,
        tooltip: baseTooltip,
        legend:
          yFields.length > 1
            ? { top: 0, textStyle: { color: muted, fontSize: 11 } }
            : undefined,
        grid: {
          top: yFields.length > 1 ? 30 : 14,
          right: 16,
          bottom: 24,
          left: 44,
          containLabel: true,
        },
        xAxis: {
          type: "category",
          data: categories,
          axisLabel: { color: muted, fontSize: 11 },
          axisLine: { lineStyle: { color: gridLine } },
        },
        yAxis: {
          type: "value",
          axisLabel: { color: muted, fontSize: 11 },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        series: yFields.map((yf) => ({
          name: yf,
          type: kind === "LINE_CHART" ? "line" : "bar",
          smooth: kind === "LINE_CHART",
          barMaxWidth: 40,
          data: rows.map(
            (r) =>
              toNumericValue(
                getCell(r, colMap, yf, findFirstNumericColIndex(data, new Set([xIdx])))
              ) ?? 0
          ),
        })),
      },
    };
  }

  if (kind === "PIE_CHART") {
    const catField =
      readString(widget.encoding, "category") ??
      readString(widget.encoding, "x") ??
      data.columns[0]?.name ??
      "category";
    const valField =
      readString(widget.encoding, "value") ??
      readString(widget.encoding, "y") ??
      data.columns[1]?.name ??
      "value";

    const pieData = rows.map((r) => ({
      name: String(getCell(r, colMap, catField, 0) ?? ""),
      value: toNumericValue(getCell(r, colMap, valField, 1)) ?? 0,
    }));

    return {
      clickField: catField,
      option: {
        animation: false,
        color: palette,
        tooltip: baseTooltip,
        legend: { bottom: 0, textStyle: { color: muted, fontSize: 11 } },
        series: [
          {
            type: "pie",
            radius: ["42%", "70%"],
            center: ["50%", "45%"],
            label: { color: fg, fontSize: 11 },
            data: pieData,
          },
        ],
      },
    };
  }

  if (kind === "SCATTER") {
    const xField = readString(widget.encoding, "x") ?? data.columns[0]?.name ?? "x";
    const yField = readString(widget.encoding, "y") ?? data.columns[1]?.name ?? "y";
    const sizeField = readString(widget.encoding, "size");

    return {
      clickField: xField,
      option: {
        animation: false,
        color: palette,
        tooltip: baseTooltip,
        grid: { top: 18, right: 18, bottom: 26, left: 44, containLabel: true },
        xAxis: {
          type: "value",
          name: xField,
          axisLabel: { color: muted, fontSize: 11 },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        yAxis: {
          type: "value",
          name: yField,
          axisLabel: { color: muted, fontSize: 11 },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        series: [
          {
            type: "scatter",
            symbolSize: (val: unknown) => {
              if (!sizeField || !Array.isArray(val)) return 10;
              const s = toNumericValue(val[2]);
              return s !== null ? Math.max(6, Math.min(28, s)) : 10;
            },
            data: rows.map((r) => [
              toNumericValue(getCell(r, colMap, xField, 0)) ?? 0,
              toNumericValue(getCell(r, colMap, yField, 1)) ?? 0,
              sizeField ? toNumericValue(getCell(r, colMap, sizeField, 2)) ?? 10 : 10,
            ]),
          },
        ],
      },
    };
  }

  if (kind === "SANKEY") {
    const srcField = readString(widget.encoding, "source") ?? "source";
    const dstField = readString(widget.encoding, "target") ?? "target";
    const valField = readString(widget.encoding, "value") ?? "value";
    const nodeNames = new Set<string>();
    const links: { source: string; target: string; value: number }[] = [];

    for (const r of rows) {
      const s = String(getCell(r, colMap, srcField, 0) ?? "");
      const t = String(getCell(r, colMap, dstField, 1) ?? "");
      const v = toNumericValue(getCell(r, colMap, valField, 2)) ?? 1;
      if (s && t && s !== t) {
        nodeNames.add(s);
        nodeNames.add(t);
        links.push({ source: s, target: t, value: Math.max(0.01, v) });
      }
    }

    return {
      clickField: srcField,
      option: {
        animation: false,
        color: palette,
        tooltip: baseTooltip,
        series: [
          {
            type: "sankey",
            emphasis: { focus: "adjacency" },
            label: { color: fg, fontSize: 11 },
            data: Array.from(nodeNames).map((name) => ({ name })),
            links,
          },
        ],
      },
    };
  }

  if (kind === "HEATMAP") {
    const xField = readString(widget.encoding, "x") ?? "x";
    const yField = readString(widget.encoding, "y") ?? "y";
    const valField = readString(widget.encoding, "value") ?? "value";

    const xCats: string[] = [];
    const yCats: string[] = [];
    const xSet = new Set<string>();
    const ySet = new Set<string>();
    let minV = Infinity;
    let maxV = -Infinity;

    for (const r of rows) {
      const xv = String(getCell(r, colMap, xField, 0) ?? "");
      const yv = String(getCell(r, colMap, yField, 1) ?? "");
      if (!xSet.has(xv)) {
        xSet.add(xv);
        xCats.push(xv);
      }
      if (!ySet.has(yv)) {
        ySet.add(yv);
        yCats.push(yv);
      }
    }

    const heatData: [number, number, number][] = [];
    for (const r of rows) {
      const xv = String(getCell(r, colMap, xField, 0) ?? "");
      const yv = String(getCell(r, colMap, yField, 1) ?? "");
      const val = toNumericValue(getCell(r, colMap, valField, 2)) ?? 0;
      minV = Math.min(minV, val);
      maxV = Math.max(maxV, val);
      heatData.push([xCats.indexOf(xv), yCats.indexOf(yv), val]);
    }

    return {
      clickField: xField,
      option: {
        animation: false,
        tooltip: { position: "top", ...baseTooltip },
        grid: { top: 16, right: 18, bottom: 32, left: 60 },
        xAxis: { type: "category", data: xCats, axisLabel: { color: muted, fontSize: 11 } },
        yAxis: { type: "category", data: yCats, axisLabel: { color: muted, fontSize: 11 } },
        visualMap: {
          min: Number.isFinite(minV) ? minV : 0,
          max: Number.isFinite(maxV) && maxV > minV ? maxV : 100,
          orient: "horizontal",
          left: "center",
          bottom: 0,
          inRange: { color: isDark ? ["#18181b", "#3b82f6"] : ["#eff6ff", "#2563eb"] },
          textStyle: { color: muted, fontSize: 10 },
        },
        series: [
          {
            type: "heatmap",
            data: heatData,
            label: { show: heatData.length <= 48, color: fg, fontSize: 10 },
          },
        ],
      },
    };
  }

  if (kind === "GAUGE") {
    const valField = readString(widget.encoding, "value") ?? "value";
    const val = toNumericValue(rows[0] ? getCell(rows[0], colMap, valField, 0) : 0) ?? 0;
    const min = readNumber(widget.options, "min") ?? 0;
    const max = readNumber(widget.options, "max") ?? 100;
    const unit = readString(widget.options, "unit") ?? "";
    const thresholds = parseThresholds(widget.options);
    const gaugeColor = resolveThresholdColor(val, thresholds, "#2563eb");

    return {
      clickField: valField,
      option: {
        animation: false,
        series: [
          {
            type: "gauge",
            min,
            max,
            progress: { show: true, width: 10, itemStyle: { color: gaugeColor } },
            axisLine: { lineStyle: { width: 10, color: [[1, gridLine]] } },
            axisTick: { show: false },
            splitLine: { length: 8, lineStyle: { width: 1.5, color: muted } },
            axisLabel: { distance: 14, color: muted, fontSize: 10 },
            pointer: { itemStyle: { color: gaugeColor } },
            detail: {
              valueAnimation: false,
              formatter: `{value}${unit ? ` ${unit}` : ""}`,
              color: fg,
              fontSize: 16,
              offsetCenter: [0, "65%"],
            },
            data: [{ value: val, name: widget.title }],
            title: { color: muted, fontSize: 11, offsetCenter: [0, "88%"] },
          },
        ],
      },
    };
  }

  if (kind === "RADAR") {
    const indField = readString(widget.encoding, "indicator") ?? "indicator";
    const valField = readString(widget.encoding, "value") ?? "value";
    const serField = readString(widget.encoding, "series");

    const indicators: string[] = [];
    const indSet = new Set<string>();
    const seriesMap = new Map<string, Map<string, number>>();

    let maxVal = 10;
    for (const r of rows) {
      const ind = String(getCell(r, colMap, indField, 0) ?? "");
      const val = toNumericValue(getCell(r, colMap, valField, 1)) ?? 0;
      const ser = serField ? String(getCell(r, colMap, serField, 2) ?? "当前") : widget.title || "当前";
      maxVal = Math.max(maxVal, val * 1.15);
      if (ind && !indSet.has(ind)) {
        indSet.add(ind);
        indicators.push(ind);
      }
      if (!seriesMap.has(ser)) {
        seriesMap.set(ser, new Map());
      }
      seriesMap.get(ser)?.set(ind, val);
    }

    return {
      clickField: indField,
      option: {
        animation: false,
        color: palette,
        tooltip: baseTooltip,
        legend:
          seriesMap.size > 1
            ? { bottom: 0, textStyle: { color: muted, fontSize: 11 } }
            : undefined,
        radar: {
          indicator:
            indicators.length > 0
              ? indicators.map((name) => ({ name, max: Math.ceil(maxVal) }))
              : [{ name: "默认", max: 100 }],
          axisName: { color: muted, fontSize: 11 },
          splitLine: { lineStyle: { color: gridLine } },
          axisLine: { lineStyle: { color: gridLine } },
        },
        series: [
          {
            type: "radar",
            data: Array.from(seriesMap.entries()).map(([name, valMap]) => ({
              name,
              value: indicators.map((ind) => valMap.get(ind) ?? 0),
            })),
          },
        ],
      },
    };
  }

  // GRAPH_TOPOLOGY fallback ECharts graph renderer
  const nodesMap = new Map<
    string,
    { id: string; name: string; value: number; category: string }
  >();
  const links: { source: string; target: string; value?: number }[] = [];

  for (const r of rows) {
    const first = r[0];
    if (typeof first === "string" && data.columns.length >= 2) {
      const col0 = data.columns[0]?.name.toLowerCase() ?? "";
      if (col0 === "source" || col0 === "src" || col0 === "src_node_id") {
        const s = String(r[0] ?? "");
        const t = String(r[1] ?? "");
        if (s && t) {
          if (!nodesMap.has(s)) nodesMap.set(s, { id: s, name: s, value: 1, category: "Node" });
          if (!nodesMap.has(t)) nodesMap.set(t, { id: t, name: t, value: 1, category: "Node" });
          const numEdge = toNumericValue(r[2]);
          links.push({
            source: s,
            target: t,
            value: numEdge ?? undefined,
          });
          continue;
        }
      }
      const id = String(getCell(r, colMap, "id", 0) ?? getCell(r, colMap, "node_id", 0) ?? "");
      const title = String(
        getCell(r, colMap, "title", 1) ?? getCell(r, colMap, "label", 1) ?? id
      );
      const typeName = String(getCell(r, colMap, "type_name", 2) ?? "Node");
      const val = toNumericValue(getCell(r, colMap, "metric_value", 3)) ?? 1;
      if (id) {
        nodesMap.set(id, { id, name: title, value: val, category: typeName });
      }
    }
  }

  const catList = Array.from(new Set(Array.from(nodesMap.values()).map((n) => n.category)));
  return {
    clickField: "node_id",
    option: {
      animation: false,
      color: palette,
      tooltip: baseTooltip,
      series: [
        {
          type: "graph",
          layout: "force",
          roam: true,
          force: { repulsion: 160, edgeLength: 70 },
          categories: catList.map((name) => ({ name })),
          label: { show: nodesMap.size <= 60, position: "bottom", color: fg, fontSize: 10 },
          data: Array.from(nodesMap.values()).map((n) => ({
            id: n.id,
            name: n.name,
            value: n.value,
            category: catList.indexOf(n.category),
            symbolSize: 20,
          })),
          links,
        },
      ],
    },
  };
}

export function EChartsWidget({
  widget,
  data,
  isDark = false,
  onDataPointClick,
}: EChartsWidgetProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const clickHandlerRef = useRef(onDataPointClick);

  useEffect(() => {
    clickHandlerRef.current = onDataPointClick;
  }, [onDataPointClick]);

  const computed = useMemo(() => {
    if (!data) return null;
    return buildChartOption(widget.widget_kind, widget, data, isDark);
  }, [data, isDark, widget]);

  const clickFieldRef = useRef("value");
  useEffect(() => {
    if (computed) {
      clickFieldRef.current = computed.clickField;
    }
  }, [computed]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const chart = echarts.init(el);
    chartRef.current = chart;

    chart.on("click", (params) => {
      const val = extractClickValue(params);
      if (val !== undefined && val !== null) {
        clickHandlerRef.current?.(clickFieldRef.current, val);
      }
    });

    const ro = new ResizeObserver(() => {
      chart.resize();
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || !computed) return;
    chart.setOption(computed.option, true);
  }, [computed]);

  const quickCategories = useMemo(() => {
    if (
      !data ||
      !onDataPointClick ||
      (widget.widget_kind !== "BAR_CHART" &&
        widget.widget_kind !== "LINE_CHART" &&
        widget.widget_kind !== "PIE_CHART")
    ) {
      return [];
    }
    const xField =
      readString(widget.encoding, "x") ??
      readString(widget.encoding, "category") ??
      data.columns[0]?.name ??
      "";
    if (!xField) return [];
    const seen = new Set<string>();
    const items: string[] = [];
    const colMap = buildColumnIndexMap(data);
    for (const row of data.rows) {
      const raw = getCell(row, colMap, xField, 0);
      const label = raw === undefined || raw === null ? "" : String(raw);
      if (label && !seen.has(label)) {
        seen.add(label);
        items.push(label);
      }
      if (items.length >= 8) break;
    }
    return items.map((label) => ({ field: xField, label }));
  }, [data, onDataPointClick, widget.encoding, widget.widget_kind]);

  if (!data || (data.rows.length === 0 && widget.widget_kind !== "GAUGE")) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        暂无图表数据
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col" data-testid={`widget-chart-${widget.id}`}>
      <div ref={containerRef} className="flex-1 min-h-[140px] w-full" />
      {quickCategories.length > 0 && (
        <div className="flex flex-wrap items-center gap-1 pt-1 border-t border-border/60">
          {quickCategories.map((item) => (
            <button
              key={item.label}
              type="button"
              onClick={() => onDataPointClick?.(item.field, item.label)}
              data-testid={`chart-category-${item.label}`}
              className="rounded border border-border px-1.5 py-0.5 text-[10px] font-mono-tech text-muted-foreground hover:border-xblue hover:text-xblue cursor-pointer"
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
