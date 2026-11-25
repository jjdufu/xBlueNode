import { useEffect, useRef } from "react";
import * as echarts from "echarts";
import { Activity, BarChart3 } from "lucide-react";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import type { PulseHourlyBucket, SubgraphPulseCard } from "@/lib/api/axops";
import { Badge, Card } from "@/components/ui/primitives";

export interface PulseThroughputChartProps {
  hourlySeries: PulseHourlyBucket[];
  subgraphs: SubgraphPulseCard[];
}

function formatHourLabel(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return iso.slice(11, 16);
  }
  const hh = String(date.getUTCHours()).padStart(2, "0");
  return `${hh}:00`;
}

export function PulseThroughputChart({
  hourlySeries,
  subgraphs,
}: PulseThroughputChartProps) {
  const { tx, fmtNum } = useI18n();
  const { isDark } = useTheme();

  const throughputContainerRef = useRef<HTMLDivElement | null>(null);
  const throughputChartRef = useRef<echarts.ECharts | null>(null);

  const subgraphContainerRef = useRef<HTMLDivElement | null>(null);
  const subgraphChartRef = useRef<echarts.ECharts | null>(null);

  useEffect(() => {
    const el = throughputContainerRef.current;
    if (!el) return;
    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 420,
      height: el.clientHeight > 0 ? el.clientHeight : 196,
    });
    throughputChartRef.current = chart;

    const observer = new ResizeObserver(() => {
      if (!chart.isDisposed() && el.clientWidth > 0 && el.clientHeight > 0) {
        chart.resize({ width: el.clientWidth, height: el.clientHeight });
      }
    });
    observer.observe(el);

    return () => {
      observer.disconnect();
      throughputChartRef.current = null;
      try {
        if (!chart.isDisposed()) {
          chart.dispose();
        }
      } catch {
        // Ignore ZRender 0x0 dispose race during teardown
      }
    };
  }, []);

  useEffect(() => {
    const el = subgraphContainerRef.current;
    if (!el) return;
    const chart = echarts.init(el, undefined, {
      width: el.clientWidth > 0 ? el.clientWidth : 420,
      height: el.clientHeight > 0 ? el.clientHeight : 164,
    });
    subgraphChartRef.current = chart;

    const observer = new ResizeObserver(() => {
      if (!chart.isDisposed() && el.clientWidth > 0 && el.clientHeight > 0) {
        chart.resize({ width: el.clientWidth, height: el.clientHeight });
      }
    });
    observer.observe(el);

    return () => {
      observer.disconnect();
      subgraphChartRef.current = null;
      try {
        if (!chart.isDisposed()) {
          chart.dispose();
        }
      } catch {
        // Ignore ZRender 0x0 dispose race during teardown
      }
    };
  }, []);

  useEffect(() => {
    const chart = throughputChartRef.current;
    if (!chart || chart.isDisposed()) return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const gridBorder = isDark ? "#27272a" : "#e4e4e7";
    const cardBg = isDark ? "#18181b" : "#ffffff";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const barSlate = isDark ? "#52525b" : "#94a3b8";

    const xLabels = hourlySeries.map((b) => formatHourLabel(b.hour_utc));
    const metricPts = hourlySeries.map((b) => b.metric_points);
    const syncedRows = hourlySeries.map((b) => b.synced_rows);

    const metricLabel = tx("时序指标点", "Metric Points");
    const syncLabel = tx("入湖行数", "Synced Rows");

    const option: echarts.EChartsOption = {
      animation: false,
      grid: { top: 28, right: 44, bottom: 24, left: 44 },
      tooltip: {
        trigger: "axis",
        backgroundColor: cardBg,
        borderColor: gridBorder,
        textStyle: { color: fgColor, fontSize: 11 },
      },
      legend: {
        top: 0,
        right: 0,
        itemWidth: 10,
        itemHeight: 6,
        textStyle: { color: mutedColor, fontSize: 10 },
        data: [metricLabel, syncLabel],
      },
      xAxis: {
        type: "category",
        data: xLabels,
        axisLine: { lineStyle: { color: gridBorder } },
        axisLabel: { color: mutedColor, fontSize: 10, interval: 3 },
      },
      yAxis: [
        {
          type: "value",
          minInterval: 1,
          splitLine: { lineStyle: { color: gridBorder, type: "dashed" } },
          axisLabel: { color: mutedColor, fontSize: 10 },
        },
        {
          type: "value",
          minInterval: 1,
          splitLine: { show: false },
          axisLabel: { color: mutedColor, fontSize: 10 },
        },
      ],
      series: [
        {
          name: syncLabel,
          type: "bar",
          yAxisIndex: 1,
          barMaxWidth: 12,
          data: syncedRows,
          itemStyle: { color: barSlate, borderRadius: [2, 2, 0, 0] },
        },
        {
          name: metricLabel,
          type: "line",
          yAxisIndex: 0,
          smooth: true,
          showSymbol: false,
          data: metricPts,
          lineStyle: { color: accentBlue, width: 2 },
          itemStyle: { color: accentBlue },
          areaStyle: { color: accentBlue, opacity: 0.14 },
        },
      ],
    };

    chart.setOption(option, true);
    chart.resize();
  }, [hourlySeries, isDark, tx]);

  useEffect(() => {
    const chart = subgraphChartRef.current;
    if (!chart || chart.isDisposed()) return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const gridBorder = isDark ? "#27272a" : "#e4e4e7";
    const cardBg = isDark ? "#18181b" : "#ffffff";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const linkColor = isDark ? "#71717a" : "#64748b";
    const alertColor = isDark ? "#f87171" : "#dc2626";

    const topSubgraphs = subgraphs.slice(0, 6);
    const categories =
      topSubgraphs.length > 0
        ? topSubgraphs.map((s) => s.display_name)
        : [tx("暂无子图", "Empty")];
    const nodeVals =
      topSubgraphs.length > 0 ? topSubgraphs.map((s) => s.node_count) : [0];
    const linkVals =
      topSubgraphs.length > 0 ? topSubgraphs.map((s) => s.link_count) : [0];
    const alertVals =
      topSubgraphs.length > 0 ? topSubgraphs.map((s) => s.alert_count) : [0];

    const nodesLabel = tx("节点", "Nodes");
    const linksLabel = tx("关系边", "Links");
    const alertsLabel = tx("活跃告警", "Alerts");

    const option: echarts.EChartsOption = {
      animation: false,
      grid: { top: 24, right: 18, bottom: 20, left: 96 },
      tooltip: {
        trigger: "axis",
        axisPointer: { type: "shadow" },
        backgroundColor: cardBg,
        borderColor: gridBorder,
        textStyle: { color: fgColor, fontSize: 11 },
      },
      legend: {
        top: 0,
        right: 0,
        itemWidth: 10,
        itemHeight: 6,
        textStyle: { color: mutedColor, fontSize: 10 },
        data: [nodesLabel, linksLabel, alertsLabel],
      },
      xAxis: {
        type: "value",
        minInterval: 1,
        splitLine: { lineStyle: { color: gridBorder, type: "dashed" } },
        axisLabel: { color: mutedColor, fontSize: 10 },
      },
      yAxis: {
        type: "category",
        inverse: true,
        data: categories,
        axisLine: { lineStyle: { color: gridBorder } },
        axisLabel: {
          color: mutedColor,
          fontSize: 10,
          width: 84,
          overflow: "truncate",
        },
      },
      series: [
        {
          name: nodesLabel,
          type: "bar",
          barMaxWidth: 10,
          data: nodeVals,
          itemStyle: { color: accentBlue, borderRadius: [0, 2, 2, 0] },
        },
        {
          name: linksLabel,
          type: "bar",
          barMaxWidth: 10,
          data: linkVals,
          itemStyle: { color: linkColor, borderRadius: [0, 2, 2, 0] },
        },
        {
          name: alertsLabel,
          type: "bar",
          barMaxWidth: 10,
          data: alertVals,
          itemStyle: { color: alertColor, borderRadius: [0, 2, 2, 0] },
        },
      ],
    };

    chart.setOption(option, true);
    chart.resize();
  }, [isDark, subgraphs, tx]);

  const totalSynced24h = hourlySeries.reduce((acc, b) => acc + b.synced_rows, 0);

  return (
    <Card
      className="p-4 flex flex-col gap-3.5 h-full"
      data-testid="pulse-charts-panel"
    >
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2 border-b border-border pb-2">
          <div className="flex items-center gap-2 min-w-0">
            <Activity className="h-4 w-4 text-xblue shrink-0" />
            <h2 className="text-sm font-semibold truncate">
              {tx("24h 时序与入湖趋势", "24h Telemetry & Lake Trend")}
            </h2>
          </div>
          <Badge variant="outline" className="font-mono-tech tabular-nums">
            {tx(
              `入湖 ${fmtNum(totalSynced24h, 0)} 行`,
              `${fmtNum(totalSynced24h, 0)} rows`
            )}
          </Badge>
        </div>

        <div
          ref={throughputContainerRef}
          className="h-[196px] w-full"
          data-testid="pulse-throughput-chart"
        />
      </div>

      <div className="space-y-2 pt-1 border-t border-border/70">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <BarChart3 className="h-3.5 w-3.5 text-xblue shrink-0" />
            <h3 className="text-xs font-semibold truncate">
              {tx("子图规模与告警分布", "Subgraph Scale & Alerts")}
            </h3>
          </div>
          <span className="text-[10px] font-mono-tech text-muted-foreground">
            UTC 24h
          </span>
        </div>

        <div
          ref={subgraphContainerRef}
          className="h-[164px] w-full"
          data-testid="pulse-subgraph-bar-chart"
        />
      </div>
    </Card>
  );
}
