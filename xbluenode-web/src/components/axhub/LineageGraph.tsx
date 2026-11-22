import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import { GitFork, RefreshCw } from "lucide-react";
import { useI18n } from "@/i18n";
import { axhubApi, type Dataset, type LineageEdge } from "@/lib/api/axhub";
import { useTheme } from "@/lib/theme";
import { Badge, Button, Card, Select } from "@/components/ui/primitives";

export interface LineageGraphProps {
  dataset: Dataset;
  connectionNames?: Record<string, string>;
}

interface LineageVisualSpec {
  symbol: "circle" | "rect" | "roundRect" | "diamond" | "triangle";
  categoryIdx: number;
}

const KIND_SPECS: Record<string, LineageVisualSpec> = {
  CONNECTION: { symbol: "roundRect", categoryIdx: 0 },
  UPLOAD: { symbol: "diamond", categoryIdx: 1 },
  DATASET: { symbol: "circle", categoryIdx: 2 },
  DOC: { symbol: "rect", categoryIdx: 3 },
  METRIC: { symbol: "triangle", categoryIdx: 4 },
  NODE_TYPE: { symbol: "triangle", categoryIdx: 4 },
  FLOW: { symbol: "triangle", categoryIdx: 4 },
  AGENT: { symbol: "triangle", categoryIdx: 4 },
  DASHBOARD: { symbol: "triangle", categoryIdx: 4 },
  EXPORT: { symbol: "triangle", categoryIdx: 4 },
};

export function LineageGraph({ dataset, connectionNames }: LineageGraphProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { isDark } = useTheme();
  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const [depth, setDepth] = useState(3);
  const [edges, setEdges] = useState<LineageEdge[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const formatLineageKind = useCallback(
    (kind: string): string => {
      switch (kind) {
        case "CONNECTION":
          return tx("连接源", "Connection");
        case "UPLOAD":
          return tx("上传文件", "Upload");
        case "DATASET":
          return tx("数据集", "Dataset");
        case "DOC":
          return tx("知识文档", "Document");
        case "METRIC":
          return tx("语义指标", "Metric");
        case "NODE_TYPE":
          return tx("节点类型", "Node Type");
        case "FLOW":
          return tx("编排流程", "Flow");
        case "AGENT":
          return tx("智能体", "Agent");
        case "DASHBOARD":
          return tx("驾驶舱", "Dashboard");
        case "EXPORT":
          return tx("导出任务", "Export");
        case "DOWNSTREAM":
          return tx("下游资产", "Downstream");
        default:
          return tEnum(kind);
      }
    },
    [tEnum, tx]
  );

  const categories = useMemo(
    () => [
      { name: formatLineageKind("CONNECTION") },
      { name: formatLineageKind("UPLOAD") },
      { name: formatLineageKind("DATASET") },
      { name: formatLineageKind("DOC") },
      { name: formatLineageKind("DOWNSTREAM") },
    ],
    [formatLineageKind]
  );

  const loadLineage = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axhubApi.getLineage(dataset.id, depth);
      setEdges(Array.isArray(res?.edges) ? res.edges : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [dataset.id, depth]);

  useEffect(() => {
    void loadLineage();
  }, [loadLineage]);

  const formatNodeTitle = useCallback(
    (kind: string, rawId: string): string => {
      if (kind === "DATASET" && rawId === dataset.id) {
        return dataset.name;
      }
      if (kind === "CONNECTION" && connectionNames?.[rawId]) {
        return connectionNames[rawId];
      }
      return rawId.length > 14 ? `${rawId.slice(0, 8)}…` : rawId;
    },
    [dataset.id, dataset.name, connectionNames]
  );

  const graphElements = useMemo(() => {
    const nodeMap = new Map<
      string,
      {
        id: string;
        name: string;
        kind: string;
        rawId: string;
        symbol: LineageVisualSpec["symbol"];
        category: number;
        isCenter: boolean;
      }
    >();

    const centerKey = `DATASET:${dataset.id}`;
    nodeMap.set(centerKey, {
      id: centerKey,
      name: `[${formatLineageKind("DATASET")}] ${dataset.name}`,
      kind: "DATASET",
      rawId: dataset.id,
      symbol: "circle",
      category: 2,
      isCenter: true,
    });

    for (const edge of edges) {
      const srcKey = `${edge.src_kind}:${edge.src_id}`;
      if (!nodeMap.has(srcKey)) {
        const spec = KIND_SPECS[edge.src_kind] ?? {
          symbol: "rect",
          categoryIdx: 4,
        };
        nodeMap.set(srcKey, {
          id: srcKey,
          name: `[${formatLineageKind(edge.src_kind)}] ${formatNodeTitle(edge.src_kind, edge.src_id)}`,
          kind: edge.src_kind,
          rawId: edge.src_id,
          symbol: spec.symbol,
          category: spec.categoryIdx,
          isCenter: srcKey === centerKey,
        });
      }

      const dstKey = `${edge.dst_kind}:${edge.dst_id}`;
      if (!nodeMap.has(dstKey)) {
        const spec = KIND_SPECS[edge.dst_kind] ?? {
          symbol: "rect",
          categoryIdx: 4,
        };
        nodeMap.set(dstKey, {
          id: dstKey,
          name: `[${formatLineageKind(edge.dst_kind)}] ${formatNodeTitle(edge.dst_kind, edge.dst_id)}`,
          kind: edge.dst_kind,
          rawId: edge.dst_id,
          symbol: spec.symbol,
          category: spec.categoryIdx,
          isCenter: dstKey === centerKey,
        });
      }
    }

    const links = edges.map((edge) => ({
      source: `${edge.src_kind}:${edge.src_id}`,
      target: `${edge.dst_kind}:${edge.dst_id}`,
      via: edge.via ? tEnum(edge.via) : "",
    }));

    return {
      nodes: Array.from(nodeMap.values()),
      links,
    };
  }, [
    dataset.id,
    dataset.name,
    edges,
    formatLineageKind,
    formatNodeTitle,
    tEnum,
  ]);

  useEffect(() => {
    const el = chartContainerRef.current;
    if (!el) return;

    const chart = echarts.init(el);
    chartRef.current = chart;

    const resizeObserver = new ResizeObserver(() => {
      chart.resize();
    });
    resizeObserver.observe(el);

    return () => {
      resizeObserver.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart) return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const cardBg = isDark ? "#18181b" : "#f4f4f5";
    const borderCol = isDark ? "#3f3f46" : "#d4d4d8";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    const option: echarts.EChartsOption = {
      tooltip: {
        trigger: "item",
        backgroundColor: cardBg,
        borderColor: borderCol,
        textStyle: { color: fgColor, fontSize: 12 },
      },
      legend: {
        top: 6,
        left: 8,
        data: categories.map((c) => c.name),
        textStyle: { color: mutedColor, fontSize: 11 },
      },
      series: [
        {
          type: "graph",
          layout: "force",
          roam: true,
          draggable: true,
          categories,
          edgeSymbol: ["none", "arrow"],
          edgeSymbolSize: [4, 8],
          force: {
            repulsion: 260,
            edgeLength: [90, 150],
            gravity: 0.12,
          },
          label: {
            show: true,
            position: "bottom",
            color: fgColor,
            fontSize: 11,
          },
          data: graphElements.nodes.map((node) => ({
            id: node.id,
            name: node.name,
            symbol: node.symbol,
            symbolSize: node.isCenter ? 32 : 22,
            category: node.category,
            itemStyle: {
              color:
                node.kind === "DATASET"
                  ? accentBlue
                  : node.kind === "CONNECTION" || node.kind === "UPLOAD"
                    ? fgColor
                    : cardBg,
              borderColor: node.isCenter ? accentBlue : fgColor,
              borderWidth: node.isCenter ? 2.5 : 1.5,
            },
          })),
          links: graphElements.links.map((lk) => ({
            source: lk.source,
            target: lk.target,
            label: lk.via
              ? {
                  show: true,
                  formatter: lk.via,
                  color: mutedColor,
                  fontSize: 10,
                }
              : undefined,
            lineStyle: {
              color: mutedColor,
              width: 1.5,
              curveness: 0.08,
            },
          })),
        },
      ],
    };

    chart.setOption(option, true);
  }, [categories, graphElements, isDark]);

  return (
    <Card className="p-4 space-y-3" data-testid="lineage-graph-panel">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2">
          <GitFork className="h-4 w-4 text-xblue" />
          <h3 className="text-xs font-semibold">
            {tx("血缘拓扑", "Lineage Graph")} · {dataset.name}
          </h3>
        </div>

        <div className="flex items-center gap-2">
          <label className="text-xs text-muted-foreground">
            {tx("跳数", "Hops")}
          </label>
          <Select
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            className="h-8"
            data-testid="lineage-depth-select"
          >
            {[1, 2, 3, 4, 5].map((d) => (
              <option key={d} value={d}>
                {tx(`${d} 跳`, `${d} hops`)}
              </option>
            ))}
          </Select>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void loadLineage()}
            disabled={loading}
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
            {tx("刷新", "Refresh")}
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div className="rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-600 dark:text-red-400">
          {errorMsg}
        </div>
      )}

      <div
        ref={chartContainerRef}
        className="h-72 w-full rounded border border-border bg-background"
        data-testid="lineage-graph-canvas"
      />

      <div className="space-y-1.5">
        <div className="text-[11px] font-medium text-muted-foreground">
          {tx(
            `${fmtNum(edges.length, 0)} 条关联边 · ${fmtNum(graphElements.nodes.length, 0)} 个节点`,
            `${fmtNum(edges.length, 0)} edges · ${fmtNum(graphElements.nodes.length, 0)} nodes`
          )}
        </div>
        {edges.length === 0 ? (
          <div className="text-xs text-muted-foreground">
            {tx("暂无上下游关联边", "No upstream or downstream edges")}
          </div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-1.5 text-xs">
            {edges.map((edge) => (
              <div
                key={edge.id}
                className="rounded border border-border px-2.5 py-1.5 flex items-center justify-between font-mono-tech text-[11px]"
                data-testid={`lineage-edge-${edge.id}`}
              >
                <span className="truncate">
                  <Badge variant="outline" className="mr-1">
                    {formatLineageKind(edge.src_kind)}
                  </Badge>
                  {formatNodeTitle(edge.src_kind, edge.src_id)} →{" "}
                  <Badge variant="xblue" className="mx-1">
                    {formatLineageKind(edge.dst_kind)}
                  </Badge>
                  {formatNodeTitle(edge.dst_kind, edge.dst_id)}
                </span>
                {edge.via && (
                  <span className="text-muted-foreground ml-2 shrink-0">
                    {tEnum(edge.via)}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </Card>
  );
}
