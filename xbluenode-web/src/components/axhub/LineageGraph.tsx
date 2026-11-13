import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import { GitFork, RefreshCw } from "lucide-react";
import { axhubApi, type Dataset, type LineageEdge } from "@/lib/api/axhub";
import { Badge, Button, Card } from "@/components/ui/primitives";

export interface LineageGraphProps {
  dataset: Dataset;
  connectionNames?: Record<string, string>;
}

interface LineageVisualSpec {
  symbol: "circle" | "rect" | "roundRect" | "diamond" | "triangle";
  categoryIdx: number;
  label: string;
}

const KIND_SPECS: Record<string, LineageVisualSpec> = {
  CONNECTION: { symbol: "roundRect", categoryIdx: 0, label: "连接源 (CONNECTION)" },
  UPLOAD: { symbol: "diamond", categoryIdx: 1, label: "上传文件 (UPLOAD)" },
  DATASET: { symbol: "circle", categoryIdx: 2, label: "数据集 (DATASET)" },
  DOC: { symbol: "rect", categoryIdx: 3, label: "知识文档 (DOC)" },
  METRIC: { symbol: "triangle", categoryIdx: 4, label: "指标 / 下游资产" },
  NODE_TYPE: { symbol: "triangle", categoryIdx: 4, label: "指标 / 下游资产" },
  FLOW: { symbol: "triangle", categoryIdx: 4, label: "指标 / 下游资产" },
  AGENT: { symbol: "triangle", categoryIdx: 4, label: "指标 / 下游资产" },
  DASHBOARD: { symbol: "triangle", categoryIdx: 4, label: "指标 / 下游资产" },
  EXPORT: { symbol: "triangle", categoryIdx: 4, label: "指标 / 下游资产" },
};

const CATEGORIES = [
  { name: "CONNECTION" },
  { name: "UPLOAD" },
  { name: "DATASET" },
  { name: "DOC" },
  { name: "DOWNSTREAM" },
];

export function LineageGraph({ dataset, connectionNames }: LineageGraphProps) {
  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const [depth, setDepth] = useState(3);
  const [edges, setEdges] = useState<LineageEdge[]>([]);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [isDark, setIsDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
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
      name: `[DATASET] ${dataset.name}`,
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
          label: edge.src_kind,
        };
        nodeMap.set(srcKey, {
          id: srcKey,
          name: `[${edge.src_kind}] ${formatNodeTitle(edge.src_kind, edge.src_id)}`,
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
          label: edge.dst_kind,
        };
        nodeMap.set(dstKey, {
          id: dstKey,
          name: `[${edge.dst_kind}] ${formatNodeTitle(edge.dst_kind, edge.dst_id)}`,
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
      via: edge.via,
    }));

    return {
      nodes: Array.from(nodeMap.values()),
      links,
    };
  }, [dataset.id, dataset.name, edges, formatNodeTitle]);

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
        data: CATEGORIES.map((c) => c.name),
        textStyle: { color: mutedColor, fontSize: 11 },
      },
      series: [
        {
          type: "graph",
          layout: "force",
          roam: true,
          draggable: true,
          categories: CATEGORIES,
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
  }, [graphElements, isDark]);

  return (
    <Card className="p-4 space-y-3" data-testid="lineage-graph-panel">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2">
          <GitFork className="h-4 w-4 text-xblue" />
          <div>
            <h3 className="text-xs font-semibold">
              上下游血缘拓扑 · {dataset.name}
            </h3>
            <p className="text-[11px] text-muted-foreground">
              追踪 CONNECTION / UPLOAD → DATASET → DOC / METRIC / NODE_TYPE / FLOW 全链路血缘
            </p>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <label className="text-xs text-muted-foreground">深度</label>
          <select
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            className="h-8 rounded-md border border-input bg-background px-2 text-xs"
            data-testid="lineage-depth-select"
          >
            {[1, 2, 3, 4, 5].map((d) => (
              <option key={d} value={d}>
                {d} 跳
              </option>
            ))}
          </select>
          <Button
            type="button"
            variant="outline"
            className="px-2.5 py-1 text-xs"
            onClick={() => void loadLineage()}
            disabled={loading}
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
            刷新血缘
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs">
          {errorMsg}
        </div>
      )}

      <div
        ref={chartContainerRef}
        className="h-72 w-full rounded border border-border bg-background"
        data-testid="lineage-graph-canvas"
      />

      {/* Structured Lineage Edge List */}
      <div className="space-y-1.5">
        <div className="text-[11px] font-medium text-muted-foreground">
          血缘关系明细 ({edges.length} 条边，{graphElements.nodes.length} 个资产节点)
        </div>
        {edges.length === 0 ? (
          <div className="text-xs text-muted-foreground">
            当前深度范围内暂无上下游血缘边，中心节点为 {dataset.name}。
          </div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-1.5 text-xs">
            {edges.map((edge) => (
              <div
                key={edge.id}
                className="rounded border border-border px-2.5 py-1.5 flex items-center justify-between font-mono-tech text-[11px]"
                data-testid={`lineage-edge-${edge.id}`}
              >
                <span>
                  <Badge variant="outline" className="mr-1">
                    {edge.src_kind}
                  </Badge>
                  {formatNodeTitle(edge.src_kind, edge.src_id)} →{" "}
                  <Badge variant="xblue" className="mx-1">
                    {edge.dst_kind}
                  </Badge>
                  {formatNodeTitle(edge.dst_kind, edge.dst_id)}
                </span>
                {edge.via && (
                  <span className="text-muted-foreground ml-2">{edge.via}</span>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </Card>
  );
}
