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

    const fgColor = isDark ? "#f8fafc" : "#0f172a";
    const mutedColor = isDark ? "#94a3b8" : "#475569";
    const gridBorder = isDark ? "rgba(148, 163, 184, 0.16)" : "#e2e8f0";
    const cardBg = isDark ? "#11141d" : "#ffffff";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const barSlate = isDark ? "#475569" : "#cbd5e1";

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

    const fgColor = isDark ? "#f8fafc" : "#0f172a";
    const mutedColor = isDark ? "#94a3b8" : "#475569";
    const gridBorder = isDark ? "rgba(148, 163, 184, 0.16)" : "#e2e8f0";
    const cardBg = isDark ? "#11141d" : "#ffffff";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const linkColor = isDark ? "#64748b" : "#94a3b8";
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
    const alertCountByName = new Map<string, number>();
    for (const sg of topSubgraphs) {
      alertCountByName.set(sg.display_name, sg.alert_count);
    }

    const nodesLabel = tx("节点", "Nodes");
    const linksLabel = tx("关系边", "Links");
    const alertsSuffix = tx("告警", "alerts");

    const option: echarts.EChartsOption = {
      animation: false,
      grid: { top: 26, right: 24, bottom: 20, left: 8, containLabel: true },
      tooltip: {
        trigger: "axis",
        axisPointer: { type: "shadow" },
        backgroundColor: cardBg,
        borderColor: gridBorder,
        textStyle: { color: fgColor, fontSize: 11 },
        formatter: (params) => {
          const list = Array.isArray(params) ? params : [params];
          const first = list[0];
          const catName = String(first?.name ?? "");
          const ac = alertCountByName.get(catName) ?? 0;
          const lines = list.map(
            (p) => `${p.marker ?? ""} ${p.seriesName}: <b>${p.value}</b>`
          );
          if (ac > 0) {
            lines.push(
              `<span style="color:${alertColor}">▲ ${alertsSuffix}: <b>${ac}</b></span>`
            );
          }
          return `<div style="font-size:11px"><b>${catName}</b><br/>${lines.join("<br/>")}</div>`;
        },
      },
      legend: {
        top: 0,
        right: 0,
        itemWidth: 10,
        itemHeight: 6,
        textStyle: { color: mutedColor, fontSize: 10 },
        data: [nodesLabel, linksLabel],
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
          width: 172,
          overflow: "truncate",
          formatter: (val: string) => {
            const ac = alertCountByName.get(val) ?? 0;
            const shortName = val.length > 11 ? `${val.slice(0, 10)}…` : val;
            return ac > 0 ? `{n|${shortName}} {a|!${ac}}` : shortName;
          },
          rich: {
            n: { color: mutedColor, fontSize: 10 },
            a: {
              color: alertColor,
              fontSize: 10,
              fontWeight: 600,
              backgroundColor: isDark
                ? "rgba(248,113,113,0.14)"
                : "rgba(220,38,38,0.1)",
              borderRadius: 3,
              padding: [1, 4],
            },
          },
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
