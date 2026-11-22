import { useCallback, useEffect, useRef, useState } from "react";
import * as echarts from "echarts";
import { Activity, RefreshCw } from "lucide-react";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import {
  xblueApi,
  type MetricQueryGrain,
  type MetricSeriesPoint,
  type WindowKind,
} from "@/lib/api/xblue";
import type { AggFn } from "@/lib/api/enums";
import { Badge, Button, Select } from "@/components/ui/primitives";

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
  const { tx } = useI18n();
  const { isDark } = useTheme();

  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const [rangePreset, setRangePreset] = useState<TimeRangePreset>("30d");
  const [grain, setGrain] = useState<GrainOption>("AUTO");
  const [agg, setAgg] = useState<AggFn>("AVG");
  const [queryMode, setQueryMode] = useState<QueryMode>("SERIES");
  const [points, setPoints] = useState<MetricSeriesPoint[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

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
    const el = containerRef.current;
    if (!el) return;
    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 320,
      height: el.clientHeight > 0 ? el.clientHeight : 192,
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
        top: 20,
        right: 14,
        bottom: 26,
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
            opacity: 0.1,
          },
        },
      ],
    };

    chart.setOption(option, true);
    chart.resize();
  }, [isDark, metric, points]);

  return (
    <div
      className="flex h-full flex-col space-y-2"
      data-testid="metric-chart-panel"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 text-xs font-semibold min-w-0">
          <Activity className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span className="truncate">{nodeTitle}</span>
          <Badge variant="xblue">{metric}</Badge>
          <span className="text-[11px] font-normal text-muted-foreground font-mono-tech shrink-0">
            ({points.length} pts)
          </span>
        </div>

        {/* Single-Row Controls: Range, Grain, Agg, Window + Refresh */}
        <div className="flex flex-wrap items-center gap-1.5">
          <Select
            value={rangePreset}
            onChange={(e) => setRangePreset(e.target.value as TimeRangePreset)}
            className="h-7 px-2 font-mono-tech text-[11px]"
            data-testid="metric-range-select"
          >
            <option value="6h">{tx("近 6 小时", "Last 6h")}</option>
            <option value="24h">{tx("近 24 小时", "Last 24h")}</option>
            <option value="7d">{tx("近 7 天", "Last 7d")}</option>
            <option value="30d">{tx("近 30 天", "Last 30d")}</option>
            <option value="90d">{tx("近 90 天", "Last 90d")}</option>
          </Select>

          <Select
            value={grain}
            onChange={(e) => setGrain(e.target.value as GrainOption)}
            disabled={queryMode !== "SERIES"}
            className="h-7 px-2 font-mono-tech text-[11px]"
            data-testid="metric-grain-select"
          >
            <option value="AUTO">{tx("粒度: 自动", "Grain: Auto")}</option>
            <option value="RAW">{tx("粒度: 原始", "Grain: Raw")}</option>
            <option value="MINUTE">{tx("粒度: 分钟", "Grain: Minute")}</option>
            <option value="HOUR">{tx("粒度: 小时", "Grain: Hour")}</option>
            <option value="DAY">{tx("粒度: 天", "Grain: Day")}</option>
          </Select>

          <Select
            value={agg}
            onChange={(e) => setAgg(e.target.value as AggFn)}
            className="h-7 px-2 font-mono-tech text-[11px]"
            data-testid="metric-agg-select"
          >
            {AGG_OPTIONS.map((fn) => (
              <option key={fn} value={fn}>
                {tx("聚合", "Agg")}: {fn}
              </option>
            ))}
          </Select>

          <Select
            value={queryMode}
            onChange={(e) => setQueryMode(e.target.value as QueryMode)}
            className="h-7 px-2 font-mono-tech text-[11px]"
            data-testid="metric-window-select"
          >
            <option value="SERIES">{tx("时序序列", "Series")}</option>
            <option value="TUMBLING">{tx("滚动窗口", "Tumbling")}</option>
            <option value="SLIDING">{tx("滑动窗口", "Sliding")}</option>
          </Select>

          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-2"
            onClick={() => void loadMetrics()}
            disabled={loading}
            title={tx("刷新趋势", "Refresh Trend")}
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-2 py-1 text-[11px]">
          {errorMsg}
        </div>
      )}

      <div
        ref={containerRef}
        className="flex-1 min-h-[168px] w-full rounded border border-border/70 bg-background"
        data-testid="metric-line-chart"
      />
    </div>
  );
}
