import { useEffect, useMemo, useRef } from "react";
import * as echarts from "echarts";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { buildColumnIndexMap, formatDisplayValue, getCell, toNumericValue } from "../utils";

export interface SimCompareWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
}

export function SimCompareWidget({
  widget,
  data,
  isDark = false,
}: SimCompareWidgetProps) {
  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const parsed = useMemo(() => {
    if (!data || data.rows.length === 0) {
      return { metrics: [] as string[], scenarios: [] as string[], values: new Map<string, number>() };
    }
    const colMap = buildColumnIndexMap(data);
    const hasScenarioName = colMap.has("scenario_name") || colMap.has("scenario");
    const hasMetric = colMap.has("metric");
    const hasAfter = colMap.has("after_val") || colMap.has("after_sum") || colMap.has("value");

    if (hasScenarioName && hasMetric && hasAfter) {
      const metrics: string[] = [];
      const scenarios: string[] = [];
      const mSet = new Set<string>();
      const sSet = new Set<string>();
      const values = new Map<string, number>();

      for (const r of data.rows) {
        const sName = String(
          getCell(r, colMap, "scenario_name", 0) ??
            getCell(r, colMap, "scenario", 0) ??
            "方案"
        );
        const mName = String(getCell(r, colMap, "metric", 1) ?? "metric");
        const val =
          toNumericValue(
            getCell(r, colMap, "after_val", 2) ??
              getCell(r, colMap, "after_sum", 2) ??
              getCell(r, colMap, "value", 2)
          ) ?? 0;

        if (!mSet.has(mName)) {
          mSet.add(mName);
          metrics.push(mName);
        }
        if (!sSet.has(sName)) {
          sSet.add(sName);
          scenarios.push(sName);
        }
        values.set(`${sName}::${mName}`, val);
      }
      return { metrics, scenarios, values };
    }

    // Generic tabular comparison: col 0 = metric/label, cols 1..n = scenarios
    const labelCol = data.columns[0]?.name ?? "metric";
    const scenarioCols = data.columns.slice(1).map((c) => c.name);
    const metrics: string[] = [];
    const values = new Map<string, number>();
    for (const r of data.rows) {
      const mName = String(getCell(r, colMap, labelCol, 0) ?? "");
      if (!mName) continue;
      if (!metrics.includes(mName)) metrics.push(mName);
      scenarioCols.forEach((sc, idx) => {
        const v = toNumericValue(r[idx + 1]) ?? 0;
        values.set(`${sc}::${mName}`, v);
      });
    }
    return { metrics, scenarios: scenarioCols, values };
  }, [data]);

  useEffect(() => {
    const el = chartContainerRef.current;
    if (!el) return;
    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 320,
      height: el.clientHeight > 0 ? el.clientHeight : 180,
    });
    chartRef.current = chart;
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
    if (!chart || chart.isDisposed() || parsed.metrics.length === 0) return;

    const muted = isDark ? "#a1a1aa" : "#71717a";
    const gridLine = isDark ? "#27272a" : "#e4e4e7";
    const palette = isDark
      ? ["#3b82f6", "#fafafa", "#a1a1aa", "#60a5fa"]
      : ["#2563eb", "#09090b", "#71717a", "#60a5fa"];

    chart.setOption(
      {
        animation: false,
        color: palette,
        tooltip: { trigger: "axis" },
        legend: { top: 0, textStyle: { color: muted, fontSize: 10 } },
        grid: { top: 26, right: 12, bottom: 22, left: 40, containLabel: true },
        xAxis: {
          type: "category",
          data: parsed.metrics,
          axisLabel: { color: muted, fontSize: 10 },
          axisLine: { lineStyle: { color: gridLine } },
        },
        yAxis: {
          type: "value",
          axisLabel: { color: muted, fontSize: 10 },
          splitLine: { lineStyle: { color: gridLine, type: "dashed" } },
        },
        series: parsed.scenarios.map((sc) => ({
          name: sc,
          type: "bar",
          barMaxWidth: 26,
          data: parsed.metrics.map((m) => parsed.values.get(`${sc}::${m}`) ?? 0),
        })),
      },
      true
    );
  }, [isDark, parsed]);

  if (!data || data.rows.length === 0) {
    return (
      <div
        className="h-full flex items-center justify-center text-xs text-muted-foreground"
        data-testid={`widget-sim-${widget.id}`}
      >
        暂无沙盘对比数据
      </div>
    );
  }

  return (
    <div
      className="h-full flex flex-col gap-2 overflow-hidden"
      data-testid={`widget-sim-${widget.id}`}
    >
      <div ref={chartContainerRef} className="h-36 shrink-0 w-full" />
      <div className="flex-1 overflow-auto border border-border rounded">
        <table className="w-full border-collapse text-[11px]">
          <thead className="sticky top-0 bg-muted border-b border-border">
            <tr>
              {data.columns.map((c) => (
                <th
                  key={c.name}
                  className="px-2 py-1 text-left font-medium text-muted-foreground whitespace-nowrap"
                >
                  {c.name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {data.rows.map((r, rIdx) => (
              <tr key={rIdx} className="hover:bg-muted/30">
                {data.columns.map((c, cIdx) => (
                  <td
                    key={`${rIdx}-${c.name}`}
                    className="px-2 py-1 whitespace-nowrap font-mono-tech"
                  >
                    {formatDisplayValue(r[cIdx])}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
