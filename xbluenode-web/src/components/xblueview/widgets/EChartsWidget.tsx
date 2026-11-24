import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import { useI18n } from "@/i18n";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { enhanceClientEChartsOption } from "@/lib/chartTheme";
import { useTheme } from "@/lib/theme";
import { buildEChartsOption } from "../logic";
import {
  buildColumnIndexMap,
  getCell,
  readNumber,
  readString,
  toNumericValue,
} from "../utils";

export interface EChartsWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  nodeLabelMap?: Readonly<Record<string, string>>;
  onDataPointClick?: (field: string, value: unknown) => void;
}

function extractClickValue(params: echarts.ECElementEvent): unknown {
  if (typeof params.data === "object" && params.data !== null) {
    const rec = params.data as Record<string, unknown>;
    if (typeof rec.nodeId === "string" && rec.nodeId.trim().length > 0) {
      return rec.nodeId;
    }
  }
  if (params.name && params.name.trim().length > 0) {
    return params.name;
  }
  if (typeof params.data === "object" && params.data !== null) {
    const rec = params.data as Record<string, unknown>;
    if ("name" in rec) return rec.name;
  }
  return params.value;
}

export function EChartsWidget({
  widget,
  data,
  isDark,
  nodeLabelMap,
  onDataPointClick,
}: EChartsWidgetProps) {
  const { isZh, tx } = useI18n();
  const { isDark: themeDark } = useTheme();
  const effectiveDark = isDark ?? themeDark;

  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const clickHandlerRef = useRef(onDataPointClick);
  const [hitPointsJson, setHitPointsJson] = useState<string>("{}");

  useEffect(() => {
    clickHandlerRef.current = onDataPointClick;
  }, [onDataPointClick]);

  const computed = useMemo(() => {
    if (!data) return null;
    const base = buildEChartsOption({
      kind: widget.widget_kind,
      title: widget.title,
      encoding: widget.encoding,
      options: widget.options,
      data,
      isDark: effectiveDark,
      nodeLabelMap,
    });
    const enhanced = enhanceClientEChartsOption(base.option, {
      isDark: effectiveDark,
      locale: isZh ? "zh" : "en",
      unit: readString(widget.options, "unit"),
      decimals: readNumber(widget.options, "decimals"),
      markLineValue: readNumber(widget.options, "mark_line_value") ?? null,
      markLineLabel: readString(widget.options, "mark_line_label"),
    });
    return {
      clickField: base.clickField,
      option: enhanced,
    };
  }, [data, effectiveDark, isZh, nodeLabelMap, widget]);

  const clickFieldRef = useRef("value");
  const rawCategoriesRef = useRef<string[]>([]);
  const rawPrimaryValuesRef = useRef<number[]>([]);
  useEffect(() => {
    if (computed) {
      clickFieldRef.current = computed.clickField;
    }
    if (data && computed) {
      const colMap = buildColumnIndexMap(data);
      rawCategoriesRef.current = data.rows.map((r) => {
        const raw = getCell(r, colMap, computed.clickField, 0);
        return raw === undefined || raw === null ? "" : String(raw).trim();
      });
      rawPrimaryValuesRef.current = data.rows.map((r) => {
        const rawVal = r.length > 1 ? r[1] : r[0];
        return toNumericValue(rawVal) ?? 0;
      });
    } else {
      rawCategoriesRef.current = [];
      rawPrimaryValuesRef.current = [];
    }
  }, [computed, data]);

  const refreshHitPoints = useCallback(() => {
    const chart = chartRef.current;
    if (!chart || chart.isDisposed()) return;
    const rawCats = rawCategoriesRef.current;
    const rawVals = rawPrimaryValuesRef.current;
    if (rawCats.length === 0 || rawCats.length > 32) {
      setHitPointsJson("{}");
      return;
    }
    const pts: Record<string, [number, number]> = {};
    rawCats.forEach((cat, idx) => {
      if (!cat || cat in pts) return;
      try {
        const yVal = rawVals[idx] ?? 0;
        const topPx = chart.convertToPixel(
          { xAxisIndex: 0, yAxisIndex: 0 },
          [idx, yVal]
        );
        const basePx = chart.convertToPixel(
          { xAxisIndex: 0, yAxisIndex: 0 },
          [idx, 0]
        );
        if (
          Array.isArray(topPx) &&
          Array.isArray(basePx) &&
          typeof topPx[0] === "number" &&
          typeof topPx[1] === "number" &&
          typeof basePx[1] === "number" &&
          Number.isFinite(topPx[0]) &&
          Number.isFinite(topPx[1]) &&
          Number.isFinite(basePx[1])
        ) {
          pts[cat] = [
            Math.round(topPx[0]),
            Math.round((topPx[1] + basePx[1]) / 2),
          ];
        }
      } catch {
        // Non-cartesian chart types (e.g. pie/gauge) skip axis pixel conversion
      }
    });
    setHitPointsJson(JSON.stringify(pts));
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 320,
      height: el.clientHeight > 0 ? el.clientHeight : 180,
    });
    chartRef.current = chart;

    chart.on("click", (params) => {
      const idx =
        typeof params.dataIndex === "number" ? params.dataIndex : -1;
      const rawCat = idx >= 0 ? rawCategoriesRef.current[idx] : undefined;
      const val =
        rawCat && rawCat.length > 0 ? rawCat : extractClickValue(params);
      if (val !== undefined && val !== null) {
        clickHandlerRef.current?.(clickFieldRef.current, val);
      }
    });

    chart.on("finished", () => {
      refreshHitPoints();
    });

    const ro = new ResizeObserver(() => {
      if (!chart.isDisposed() && el.clientWidth > 0 && el.clientHeight > 0) {
        chart.resize({ width: el.clientWidth, height: el.clientHeight });
        refreshHitPoints();
      }
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      chartRef.current = null;
      try {
        if (!chart.isDisposed()) {
          chart.dispose();
        }
      } catch {
        // Ignore ZRender 0x0 canvas dispose error during unmount
      }
    };
  }, [refreshHitPoints]);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || chart.isDisposed() || !computed) return;
    chart.setOption(computed.option as echarts.EChartsOption, true);
    refreshHitPoints();
  }, [computed, refreshHitPoints]);

  if (!data || (data.rows.length === 0 && widget.widget_kind !== "GAUGE")) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        {tx("暂无图表数据", "No chart data")}
      </div>
    );
  }

  return (
    <div
      className="h-full flex flex-col"
      data-testid={`widget-chart-${widget.id}`}
      data-hit-points={hitPointsJson}
    >
      <div ref={containerRef} className="flex-1 min-h-[140px] w-full" />
    </div>
  );
}
