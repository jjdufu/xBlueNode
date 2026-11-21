import { useCallback, useEffect, useRef, useState } from "react";
import * as echarts from "echarts";
import { Activity, RefreshCw } from "lucide-react";
import {
  xblueApi,
  type MetricQueryGrain,
  type MetricSeriesPoint,
  type WindowKind,
} from "@/lib/api/xblue";
import type { AggFn } from "@/lib/api/enums";
import { Badge, Button } from "@/components/ui/primitives";

type TimeRangePreset = "6h" | "24h" | "7d" | "30d" | "90d";
type GrainOption = "AUTO" | MetricQueryGrain;
type QueryMode = "SERIES" | WindowKind;

const RANGE_HOURS: Record<TimeRangePreset, number> = {
  "6h": 6,
  "24h": 24,
  "7d": 24 * 7,
  "30d": 24 * 30,
  "90d": 24 * 90,
};

const AGG_OPTIONS: AggFn[] = ["AVG", "SUM", "MIN", "MAX", "COUNT_DISTINCT"];

export interface MetricChartProps {
  nodeId: string;
  nodeTitle: string;
  metric: string;
}

export function MetricChart({ nodeId, nodeTitle, metric }: MetricChartProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const [rangePreset, setRangePreset] = useState<TimeRangePreset>("30d");
  const [grain, setGrain] = useState<GrainOption>("AUTO");
  const [agg, setAgg] = useState<AggFn>("AVG");
  const [queryMode, setQueryMode] = useState<QueryMode>("SERIES");
  const [points, setPoints] = useState<MetricSeriesPoint[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [isDark, setIsDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  const loadMetrics = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const now = new Date();
      const fromDate = new Date(
        now.getTime() - RANGE_HOURS[rangePreset] * 3600 * 1000
      );
      const fromIso = fromDate.toISOString();
      const toIso = now.toISOString();

      if (queryMode === "SERIES") {
        const res = await xblueApi.queryNodeMetrics(nodeId, metric, {
          from: fromIso,
          to: toIso,
          grain: grain === "AUTO" ? undefined : grain,
          agg,
        });
        setPoints(Array.isArray(res) ? res : []);
      } else {
        const winSize =
          rangePreset === "6h" || rangePreset === "24h" ? "1h" : "1d";
        const winStep =
          queryMode === "SLIDING"
            ? rangePreset === "6h" || rangePreset === "24h"
              ? "15m"
              : "6h"
            : undefined;
        const winRes = await xblueApi.queryWindow({
          node_ids: [nodeId],
          metric,
          from: fromIso,
          to: toIso,
          window: {
            kind: queryMode,
            size: winSize,
            step: winStep,
          },
          agg,
          group_by: "none",
        });
        const items = Array.isArray(winRes?.items) ? winRes.items : [];
        setPoints(items.map((b) => ({ ts: b.ts, value: b.value })));
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
      setPoints([]);
    } finally {
      setLoading(false);
    }
  }, [agg, grain, metric, nodeId, queryMode, rangePreset]);

  useEffect(() => {
    void loadMetrics();
  }, [loadMetrics]);

  useEffect(() => {
    if (typeof document === "undefined") return;
    const observer = new MutationObserver(() => {
      setIsDark(document.documentElement.classList.contains("dark"));
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 320,
      height: el.clientHeight > 0 ? el.clientHeight : 180,
    });
    chartRef.current = chart;

    const resizeObserver = new ResizeObserver(() => {
      if (!chart.isDisposed() && el.clientWidth > 0 && el.clientHeight > 0) {
        chart.resize({ width: el.clientWidth, height: el.clientHeight });
      }
    });
    resizeObserver.observe(el);

    return () => {
      resizeObserver.disconnect();
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
    if (!chart || chart.isDisposed()) return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const gridBorder = isDark ? "#27272a" : "#e4e4e7";
    const cardBg = isDark ? "#18181b" : "#f4f4f5";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    const xLabels = points.map((p) => p.ts.replace("T", " ").slice(0, 16));
    const yValues = points.map((p) => p.value);

    const option: echarts.EChartsOption = {
      animation: false,
      grid: {
        top: 24,
        right: 14,
        bottom: 28,
        left: 44,
      },
      tooltip: {
        trigger: "axis",
        backgroundColor: cardBg,
        borderColor: gridBorder,
        textStyle: { color: fgColor, fontSize: 11 },
      },
      xAxis: {
        type: "category",
        data: xLabels,
        axisLine: { lineStyle: { color: gridBorder } },
        axisLabel: { color: mutedColor, fontSize: 10 },
      },
      yAxis: {
        type: "value",
        splitLine: { lineStyle: { color: gridBorder, type: "dashed" } },
        axisLabel: { color: mutedColor, fontSize: 10 },
      },
      series: [
        {
          name: metric,
          type: "line",
          smooth: false,
          showSymbol: points.length <= 48,
          symbolSize: 5,
          data: yValues,
          lineStyle: {
            color: accentBlue,
            width: 2,
          },
          itemStyle: {
            color: accentBlue,
          },
          areaStyle: {
            color: accentBlue,
            opacity: 0.08,
          },
        },
      ],
    };

    chart.setOption(option, true);
    chart.resize();
  }, [isDark, metric, points]);

  return (
    <div
      className="rounded-md border border-border p-3 space-y-2.5 bg-muted/20"
      data-testid="metric-chart-panel"
    >
      <div className="flex flex-wrap items-center justify-between gap-1.5">
        <div className="flex items-center gap-1.5 text-xs font-semibold">
          <Activity className="h-3.5 w-3.5 text-xblue" />
          <span>{nodeTitle}</span>
          <Badge variant="xblue">{metric}</Badge>
          <span className="text-[11px] font-normal text-muted-foreground font-mono-tech">
            ({points.length} pts)
          </span>
        </div>

        <Button
          type="button"
          variant="ghost"
          className="px-2 py-1 text-[11px]"
          onClick={() => void loadMetrics()}
          disabled={loading}
        >
          <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
        </Button>
      </div>

      {/* Controls: Range, Grain, Agg, Window */}
      <div className="grid grid-cols-2 gap-1.5 text-[11px]">
        <select
          value={rangePreset}
          onChange={(e) => setRangePreset(e.target.value as TimeRangePreset)}
          className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-[11px]"
          data-testid="metric-range-select"
        >
          <option value="6h">最近 6 小时</option>
          <option value="24h">最近 24 小时</option>
          <option value="7d">最近 7 天</option>
          <option value="30d">最近 30 天</option>
          <option value="90d">最近 90 天</option>
        </select>

        <select
          value={grain}
          onChange={(e) => setGrain(e.target.value as GrainOption)}
          disabled={queryMode !== "SERIES"}
          className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-[11px]"
          data-testid="metric-grain-select"
        >
          <option value="AUTO">粒度: 自动</option>
          <option value="RAW">粒度: RAW</option>
          <option value="MINUTE">粒度: MINUTE</option>
          <option value="HOUR">粒度: HOUR</option>
          <option value="DAY">粒度: DAY</option>
        </select>

        <select
          value={agg}
          onChange={(e) => setAgg(e.target.value as AggFn)}
          className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-[11px]"
          data-testid="metric-agg-select"
        >
          {AGG_OPTIONS.map((fn) => (
            <option key={fn} value={fn}>
              聚合: {fn}
            </option>
          ))}
        </select>

        <select
          value={queryMode}
          onChange={(e) => setQueryMode(e.target.value as QueryMode)}
          className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-[11px]"
          data-testid="metric-window-select"
        >
          <option value="SERIES">模式: 时序汇总</option>
          <option value="TUMBLING">窗口: TUMBLING</option>
          <option value="SLIDING">窗口: SLIDING</option>
        </select>
      </div>

      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-2 py-1 text-[11px]">
          {errorMsg}
        </div>
      )}

      <div
        ref={containerRef}
        className="h-52 w-full rounded border border-border bg-background"
        data-testid="metric-line-chart"
      />
    </div>
  );
}
