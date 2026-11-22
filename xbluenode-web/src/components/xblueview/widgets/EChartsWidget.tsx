import { useEffect, useMemo, useRef } from "react";
import * as echarts from "echarts";
import { useI18n } from "@/i18n";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { useTheme } from "@/lib/theme";
import { buildEChartsOption } from "../logic";
import { buildColumnIndexMap, getCell, readString } from "../utils";

export interface EChartsWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  nodeLabelMap?: Readonly<Record<string, string>>;
  onDataPointClick?: (field: string, value: unknown) => void;
}

const ISO_TIMESTAMP_PREFIX_RE = /^\d{4}-\d{2}-\d{2}[T\s]/;

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
  const { tx } = useI18n();
  const { isDark: themeDark } = useTheme();
  const effectiveDark = isDark ?? themeDark;

  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const clickHandlerRef = useRef(onDataPointClick);

  useEffect(() => {
    clickHandlerRef.current = onDataPointClick;
  }, [onDataPointClick]);

  const computed = useMemo(() => {
    if (!data) return null;
    return buildEChartsOption({
      kind: widget.widget_kind,
      title: widget.title,
      encoding: widget.encoding,
      options: widget.options,
      data,
      isDark: effectiveDark,
      nodeLabelMap,
    });
  }, [data, effectiveDark, nodeLabelMap, widget]);

  const clickFieldRef = useRef("value");
  useEffect(() => {
    if (computed) {
      clickFieldRef.current = computed.clickField;
    }
  }, [computed]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 320,
      height: el.clientHeight > 0 ? el.clientHeight : 180,
    });
    chartRef.current = chart;

    chart.on("click", (params) => {
      const val = extractClickValue(params);
      if (val !== undefined && val !== null) {
        clickHandlerRef.current?.(clickFieldRef.current, val);
      }
    });

    const ro = new ResizeObserver(() => {
      if (!chart.isDisposed() && el.clientWidth > 0 && el.clientHeight > 0) {
        chart.resize({ width: el.clientWidth, height: el.clientHeight });
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
  }, []);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || chart.isDisposed() || !computed) return;
    chart.setOption(computed.option as echarts.EChartsOption, true);
  }, [computed]);

  // Only render discrete category drilldown pills for BAR_CHART and PIE_CHART (never LINE_CHART / AREA_CHART)
  const quickCategories = useMemo(() => {
    if (
      !data ||
      !onDataPointClick ||
      (widget.widget_kind !== "BAR_CHART" &&
        widget.widget_kind !== "PIE_CHART")
    ) {
      return [];
    }
    const xField =
      computed?.clickField ??
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
      const label = raw === undefined || raw === null ? "" : String(raw).trim();
      if (!label) continue;
      if (ISO_TIMESTAMP_PREFIX_RE.test(label)) {
        return [];
      }
      if (!seen.has(label)) {
        seen.add(label);
        items.push(label);
        if (items.length > 8) {
          return [];
        }
      }
    }
    return items.map((label) => ({ field: xField, label }));
  }, [
    computed?.clickField,
    data,
    onDataPointClick,
    widget.encoding,
    widget.widget_kind,
  ]);

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
    >
      <div ref={containerRef} className="flex-1 min-h-[140px] w-full" />
      {quickCategories.length > 0 && (
        <div className="flex flex-wrap items-center gap-1 pt-1 border-t border-border/60 shrink-0">
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
