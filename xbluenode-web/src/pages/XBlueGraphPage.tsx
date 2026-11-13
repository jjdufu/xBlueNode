import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  Cpu,
  Info,
  RefreshCw,
  Route,
  Search,
  Share2,
} from "lucide-react";
import { api, type TopologyGraph } from "@/lib/api/rag";
import {
  xblueApi,
  type AlgoTopEntry,
  type BlueLink,
  type BlueNode,
  type LinkRecord,
  type NodeSearchHit,
  type PathResponse,
  type SubgraphResponse,
} from "@/lib/api/xblue";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import {
  AlgoPanel,
  type AlgoVisualMode,
} from "@/components/xbluegraph/AlgoPanel";
import { NeighborExpand } from "@/components/xbluegraph/NeighborExpand";
import { NodeInspector } from "@/components/xbluegraph/NodeInspector";
import { PathFinder } from "@/components/xbluegraph/PathFinder";

type SideTab = "inspector" | "neighbors" | "path" | "algo";
type GraphLayer = "business" | "hypergraph";

interface CanvasEdge {
  id: string;
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight: number;
}

function toCanvasEdge(lk: BlueLink | LinkRecord): CanvasEdge {
  return {
    id:
      lk.id ||
      `${lk.link_type}:${lk.src_node_id}->${lk.dst_node_id}`,
    link_type: lk.link_type,
    src_node_id: lk.src_node_id,
    dst_node_id: lk.dst_node_id,
    weight: typeof lk.weight === "number" ? lk.weight : 1,
  };
}

function mergeNodesDeduplicated(
  existing: BlueNode[],
  incoming: BlueNode[]
): BlueNode[] {
  const map = new Map<string, BlueNode>();
  for (const n of existing) {
    map.set(n.id, n);
  }
  for (const n of incoming) {
    const prev = map.get(n.id);
    map.set(n.id, prev ? { ...prev, ...n } : n);
  }
  return Array.from(map.values());
}

function mergeEdgesDeduplicated(
  existing: CanvasEdge[],
  incoming: (BlueLink | LinkRecord | CanvasEdge)[]
): CanvasEdge[] {
  const map = new Map<string, CanvasEdge>();
  for (const e of existing) {
    const key = `${e.link_type}:${e.src_node_id}->${e.dst_node_id}`;
    map.set(key, e);
  }
  for (const raw of incoming) {
    const e = toCanvasEdge(raw);
    const key = `${e.link_type}:${e.src_node_id}->${e.dst_node_id}`;
    map.set(key, e);
  }
  return Array.from(map.values());
}

function extractNodeIdFromEvent(data: unknown): string | null {
  if (typeof data === "object" && data !== null && "id" in data) {
    const idVal = (data as { id?: unknown }).id;
    if (typeof idVal === "string") {
      return idVal;
    }
  }
  return null;
}

const COMMUNITY_PALETTE_LIGHT = [
  "#2563eb",
  "#09090b",
  "#52525b",
  "#1d4ed8",
  "#3f3f46",
  "#60a5fa",
];

const COMMUNITY_PALETTE_DARK = [
  "#3b82f6",
  "#fafafa",
  "#a1a1aa",
  "#60a5fa",
  "#d4d4d8",
  "#93c5fd",
];

export function XBlueGraphPage() {
  const canvasContainerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  const [graphLayer, setGraphLayer] = useState<GraphLayer>("business");
  const [hypergraph, setHypergraph] = useState<TopologyGraph | null>(null);

  const [nodes, setNodes] = useState<BlueNode[]>([]);
  const [edges, setEdges] = useState<CanvasEdge[]>([]);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [sideTab, setSideTab] = useState<SideTab>("inspector");
  const [typeFilter, setTypeFilter] = useState<string>("");

  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<NodeSearchHit[]>([]);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searching, setSearching] = useState(false);

  const [highlightedPathIds, setHighlightedPathIds] = useState<string[]>([]);
  const [algoMode, setAlgoMode] = useState<AlgoVisualMode>("NONE");
  const [algoEntries, setAlgoEntries] = useState<AlgoTopEntry[]>([]);

  const [loading, setLoading] = useState(false);
  const [expanding, setExpanding] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [isDark, setIsDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  const loadInitialGraph = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const sub = await xblueApi
        .querySubgraph({ limit: 200 })
        .catch(() => ({ nodes: [], links: [], truncated: false }));
      const subNodes = Array.isArray(sub?.nodes) ? sub.nodes : [];
      const subLinks = Array.isArray(sub?.links) ? sub.links : [];

      if (subNodes.length > 0) {
        setNodes(subNodes);
        setEdges(mergeEdgesDeduplicated([], subLinks));
        setSelectedNodeId((prev) =>
          prev && subNodes.some((n) => n.id === prev)
            ? prev
            : (subNodes[0]?.id ?? null)
        );
      } else {
        const fallback = await xblueApi
          .listNodes({ limit: 200 })
          .catch(() => ({ items: [], nodes: [], links: [], next_cursor: null }));
        const fbNodes =
          Array.isArray(fallback?.items) && fallback.items.length > 0
            ? fallback.items
            : Array.isArray(fallback?.nodes)
              ? fallback.nodes
              : [];
        const fbLinks = Array.isArray(fallback?.links) ? fallback.links : [];
        setNodes(fbNodes);
        setEdges(mergeEdgesDeduplicated([], fbLinks));
        setSelectedNodeId((prev) =>
          prev && fbNodes.some((n) => n.id === prev)
            ? prev
            : (fbNodes[0]?.id ?? null)
        );
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadHypergraph = useCallback(async () => {
    try {
      const data = await api.topology();
      setHypergraph({
        nodes: Array.isArray(data?.nodes) ? data.nodes : [],
        edges: Array.isArray(data?.edges) ? data.edges : [],
      });
    } catch {
      setHypergraph({ nodes: [], edges: [] });
    }
  }, []);

  useEffect(() => {
    void loadInitialGraph();
  }, [loadInitialGraph]);

  useEffect(() => {
    if (graphLayer === "hypergraph" && !hypergraph) {
      void loadHypergraph();
    }
  }, [graphLayer, hypergraph, loadHypergraph]);

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

  const mergeSubgraphIntoCanvas = useCallback((res: SubgraphResponse) => {
    const incomingNodes = Array.isArray(res?.nodes) ? res.nodes : [];
    const incomingLinks = Array.isArray(res?.links) ? res.links : [];
    setNodes((prev) => mergeNodesDeduplicated(prev, incomingNodes));
    setEdges((prev) => mergeEdgesDeduplicated(prev, incomingLinks));
  }, []);

  const handleSelectNode = useCallback((nodeId: string) => {
    setSelectedNodeId(nodeId);
    setSideTab("inspector");
  }, []);

  const handleExpandOneHop = useCallback(
    async (nodeId: string) => {
      setExpanding(true);
      setErrorMsg(null);
      try {
        const res = await xblueApi.queryNeighbors({
          node_id: nodeId,
          depth: 1,
          direction: "BOTH",
        });
        mergeSubgraphIntoCanvas(res);
      } catch (err) {
        setErrorMsg(err instanceof Error ? err.message : String(err));
      } finally {
        setExpanding(false);
      }
    },
    [mergeSubgraphIntoCanvas]
  );

  // Keep refs updated for ECharts click / dblclick listeners
  const selectHandlerRef = useRef(handleSelectNode);
  const expandHandlerRef = useRef(handleExpandOneHop);
  useEffect(() => {
    selectHandlerRef.current = handleSelectNode;
    expandHandlerRef.current = handleExpandOneHop;
  }, [handleSelectNode, handleExpandOneHop]);

  const distinctTypes = useMemo(() => {
    const set = new Set<string>();
    for (const n of nodes) {
      if (n.type_name) set.add(n.type_name);
    }
    return Array.from(set).sort();
  }, [nodes]);

  const knownLinkTypes = useMemo(() => {
    const set = new Set<string>();
    for (const e of edges) {
      if (e.link_type) set.add(e.link_type);
    }
    return Array.from(set).sort();
  }, [edges]);

  const visibleNodes = useMemo(
    () =>
      typeFilter ? nodes.filter((n) => n.type_name === typeFilter) : nodes,
    [nodes, typeFilter]
  );

  const visibleNodeIdSet = useMemo(
    () => new Set(visibleNodes.map((n) => n.id)),
    [visibleNodes]
  );

  const visibleEdges = useMemo(
    () =>
      edges.filter(
        (e) =>
          visibleNodeIdSet.has(e.src_node_id) &&
          visibleNodeIdSet.has(e.dst_node_id)
      ),
    [edges, visibleNodeIdSet]
  );

  const selectedNode = useMemo(
    () => nodes.find((n) => n.id === selectedNodeId) ?? null,
    [nodes, selectedNodeId]
  );

  // Initialize ECharts Force Graph Canvas
  useEffect(() => {
    if (graphLayer !== "business") return;
    const el = canvasContainerRef.current;
    if (!el) return;

    const chart = echarts.init(el);
    chartRef.current = chart;

    chart.on("click", (params) => {
      if (params.dataType === "node") {
        const nodeId = extractNodeIdFromEvent(params.data);
        if (nodeId) {
          selectHandlerRef.current(nodeId);
        }
      }
    });

    chart.on("dblclick", (params) => {
      if (params.dataType === "node") {
        const nodeId = extractNodeIdFromEvent(params.data);
        if (nodeId) {
          selectHandlerRef.current(nodeId);
          void expandHandlerRef.current(nodeId);
        }
      }
    });

    const resizeObserver = new ResizeObserver(() => {
      chart.resize();
    });
    resizeObserver.observe(el);

    return () => {
      resizeObserver.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, [graphLayer]);

  // Update ECharts Force Graph Option
  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || graphLayer !== "business") return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const cardBg = isDark ? "#18181b" : "#f4f4f5";
    const borderCol = isDark ? "#3f3f46" : "#d4d4d8";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const palette = isDark ? COMMUNITY_PALETTE_DARK : COMMUNITY_PALETTE_LIGHT;

    const categories = distinctTypes.map((t) => ({ name: t }));
    const typeIdxMap = new Map<string, number>();
    distinctTypes.forEach((t, idx) => typeIdxMap.set(t, idx));

    const pathNodeSet = new Set(highlightedPathIds);
    const pathPairSet = new Set<string>();
    for (let i = 0; i < highlightedPathIds.length - 1; i += 1) {
      const a = highlightedPathIds[i];
      const b = highlightedPathIds[i + 1];
      if (a && b) {
        pathPairSet.add(`${a}->${b}`);
        pathPairSet.add(`${b}->${a}`);
      }
    }

    const algoMap = new Map<string, AlgoTopEntry>();
    let maxScore = 0;
    for (const entry of algoEntries) {
      algoMap.set(entry.node_id, entry);
      if (entry.score !== null && entry.score > maxScore) {
        maxScore = entry.score;
      }
    }

    const seriesData = visibleNodes.map((node) => {
      const isSelected = node.id === selectedNodeId;
      const isInPath = pathNodeSet.has(node.id);
      const algoEntry = algoMap.get(node.id);
      const catIdx = typeIdxMap.get(node.type_name) ?? 0;

      let symbolSize = isSelected || isInPath ? 32 : 24;
      if (
        algoMode === "SCORE_SIZE" &&
        algoEntry?.score !== null &&
        algoEntry?.score !== undefined &&
        maxScore > 0
      ) {
        symbolSize = Math.round(18 + (algoEntry.score / maxScore) * 30);
      }

      let fillColor = palette[catIdx % palette.length] ?? fgColor;
      if (
        algoMode === "COMMUNITY_COLOR" &&
        algoEntry?.community !== null &&
        algoEntry?.community !== undefined
      ) {
        const commIdx = Math.abs(algoEntry.community) % palette.length;
        fillColor = palette[commIdx] ?? accentBlue;
      }
      if (isSelected || isInPath) {
        fillColor = accentBlue;
      }

      return {
        id: node.id,
        name: `${node.title} (${node.node_key})`,
        category: catIdx,
        symbolSize,
        itemStyle: {
          color: fillColor,
          borderColor: isSelected || isInPath ? accentBlue : fgColor,
          borderWidth: isSelected || isInPath ? 3 : 1.5,
        },
      };
    });

    const seriesLinks = visibleEdges.map((edge) => {
      const inPath = pathPairSet.has(`${edge.src_node_id}->${edge.dst_node_id}`);
      return {
        source: edge.src_node_id,
        target: edge.dst_node_id,
        label: {
          show: visibleEdges.length <= 60 || inPath,
          formatter: edge.link_type,
          color: inPath ? accentBlue : mutedColor,
          fontSize: 10,
        },
        lineStyle: {
          color: inPath ? accentBlue : mutedColor,
          width: inPath ? 3 : 1.5,
          opacity: inPath ? 1 : 0.65,
          curveness: 0.08,
        },
      };
    });

    const option: echarts.EChartsOption = {
      animation: false,
      tooltip: {
        trigger: "item",
        backgroundColor: cardBg,
        borderColor: borderCol,
        textStyle: { color: fgColor, fontSize: 12 },
      },
      legend:
        categories.length > 0
          ? {
              top: 8,
              left: 10,
              data: categories.map((c) => c.name),
              textStyle: { color: mutedColor, fontSize: 11 },
            }
          : undefined,
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
            repulsion: 280,
            edgeLength: [80, 145],
            gravity: 0.12,
          },
          label: {
            show: visibleNodes.length <= 120,
            position: "bottom",
            color: fgColor,
            fontSize: 11,
          },
          data: seriesData,
          links: seriesLinks,
        },
      ],
    };

    chart.setOption(option, true);
  }, [
    algoEntries,
    algoMode,
    distinctTypes,
    graphLayer,
    highlightedPathIds,
    isDark,
    selectedNodeId,
    visibleEdges,
    visibleNodes,
  ]);

  const handleHybridSearch = async (e: React.FormEvent) => {
    e.preventDefault();
    const q = searchQuery.trim();
    if (!q) {
      setSearchOpen(false);
      setSearchHits([]);
      return;
    }
    setSearching(true);
    setErrorMsg(null);
    try {
      const res = await xblueApi.searchNodes({
        q,
        type_name: typeFilter || undefined,
        k: 20,
      });
      const hits = Array.isArray(res?.items) ? res.items : [];
      setSearchHits(hits);
      setSearchOpen(true);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSearching(false);
    }
  };

  const handlePickSearchHit = (hit: NodeSearchHit) => {
    setNodes((prev) => mergeNodesDeduplicated(prev, [hit]));
    setSelectedNodeId(hit.id);
    setSideTab("inspector");
    setSearchOpen(false);
  };

  const handlePathFound = (pathRes: PathResponse) => {
    setNodes((prev) => mergeNodesDeduplicated(prev, pathRes.nodes));
    setEdges((prev) => mergeEdgesDeduplicated(prev, pathRes.links));
    setHighlightedPathIds(pathRes.path);
  };

  return (
    <div className="space-y-4" data-testid="xbluegraph-page">
      {/* Header */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            xBlueGraph · 拓扑浏览器
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            力导向业务对象关系网络、时序指标钻取、多跳遍历、最短路径与图算法工作台
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <div className="flex items-center rounded-md border border-border bg-muted/40 p-0.5 text-xs">
            <button
              type="button"
              onClick={() => setGraphLayer("business")}
              className={`rounded px-2.5 py-1 font-medium cursor-pointer ${
                graphLayer === "business"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              业务对象层 (xb_nodes)
            </button>
            <button
              type="button"
              onClick={() => setGraphLayer("hypergraph")}
              className={`rounded px-2.5 py-1 font-medium cursor-pointer ${
                graphLayer === "hypergraph"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              知识超图层 (hg_docs)
            </button>
          </div>

          <Button
            variant="outline"
            onClick={() =>
              void (graphLayer === "business"
                ? loadInitialGraph()
                : loadHypergraph())
            }
            disabled={loading}
            data-testid="refresh-graph-btn"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
            刷新拓扑
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div
          className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs flex items-center justify-between"
          data-testid="graph-error-banner"
        >
          <span>{errorMsg}</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground cursor-pointer"
            onClick={() => setErrorMsg(null)}
          >
            关闭
          </button>
        </div>
      )}

      {graphLayer === "business" ? (
        <>
          {/* Search, Type Filter & Canvas Telemetry Bar */}
          <Card className="p-3 space-y-2.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <form
                onSubmit={(e) => void handleHybridSearch(e)}
                className="flex flex-1 min-w-[260px] items-center gap-2"
              >
                <Input
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="全文 + 向量混合检索业务节点（如 华东仓 / W-1）..."
                  className="h-8 text-xs"
                  data-testid="graph-search-input"
                />
                <select
                  value={typeFilter}
                  onChange={(e) => setTypeFilter(e.target.value)}
                  className="h-8 rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                  data-testid="graph-type-filter"
                >
                  <option value="">全部节点类型</option>
                  {distinctTypes.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
                <Button
                  type="submit"
                  variant="outline"
                  className="px-3 py-1.5 text-xs"
                  disabled={searching}
                  data-testid="graph-search-btn"
                >
                  <Search className="h-3.5 w-3.5" />
                  检索
                </Button>
              </form>

              <div className="flex items-center gap-2 text-xs">
                <span className="rounded border border-border bg-muted px-2.5 py-1 font-mono-tech">
                  节点{" "}
                  <strong
                    className="text-xblue"
                    data-testid="canvas-node-count"
                  >
                    {visibleNodes.length}
                  </strong>
                </span>
                <span className="rounded border border-border bg-muted px-2.5 py-1 font-mono-tech">
                  关系{" "}
                  <strong data-testid="canvas-edge-count">
                    {visibleEdges.length}
                  </strong>
                </span>

                <Button
                  type="button"
                  variant="outline"
                  className="px-2.5 py-1 text-xs"
                  disabled={!selectedNode || expanding}
                  onClick={() => {
                    if (selectedNode) {
                      void handleExpandOneHop(selectedNode.id);
                    }
                  }}
                  data-testid="expand-node-btn"
                >
                  <Share2 className="h-3.5 w-3.5 text-xblue" />
                  {expanding ? "展开中..." : "展开选中 1 跳"}
                </Button>
              </div>
            </div>

            {/* Hybrid Search Dropdown Results */}
            {searchOpen && (
              <div
                className="rounded border border-border bg-background p-2 space-y-1.5"
                data-testid="graph-search-results"
              >
                <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                  <span>混合检索命中 ({searchHits.length})</span>
                  <button
                    type="button"
                    onClick={() => setSearchOpen(false)}
                    className="hover:text-foreground cursor-pointer"
                  >
                    收起
                  </button>
                </div>
                {searchHits.length === 0 ? (
                  <div className="text-xs text-muted-foreground py-1">
                    未命中匹配的业务节点。
                  </div>
                ) : (
                  <div className="flex flex-wrap gap-1.5">
                    {searchHits.map((hit) => (
                      <button
                        key={hit.id}
                        type="button"
                        onClick={() => handlePickSearchHit(hit)}
                        data-testid={`search-hit-${hit.node_key}`}
                        className="rounded border border-border px-2.5 py-1 text-xs flex items-center gap-1.5 hover:border-xblue hover:bg-xblue-soft transition-colors cursor-pointer"
                      >
                        <span className="font-medium">{hit.title}</span>
                        <span className="font-mono-tech text-[11px] text-muted-foreground">
                          ({hit.node_key})
                        </span>
                        <Badge variant="xblue">
                          {(hit.channels ?? []).join("+") || hit.type_name}
                        </Badge>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* Quick Canvas Node Index Strip (Single-click selects, Double-click expands 1-hop) */}
            {visibleNodes.length > 0 && (
              <div
                className="flex items-center gap-1.5 overflow-x-auto pt-1 border-t border-border"
                data-testid="canvas-node-list"
              >
                <span className="text-[11px] text-muted-foreground shrink-0">
                  画布节点（单击查看详情 / 双击展开 1 跳）:
                </span>
                {visibleNodes.slice(0, 30).map((node) => {
                  const active = node.id === selectedNodeId;
                  return (
                    <button
                      key={node.id}
                      type="button"
                      onClick={() => handleSelectNode(node.id)}
                      onDoubleClick={() => {
                        handleSelectNode(node.id);
                        void handleExpandOneHop(node.id);
                      }}
                      data-testid={`graph-node-${node.node_key}`}
                      className={`shrink-0 rounded border px-2 py-0.5 text-[11px] font-mono-tech transition-colors cursor-pointer ${
                        active
                          ? "border-xblue bg-xblue-soft text-xblue font-medium"
                          : "border-border bg-background hover:bg-muted"
                      }`}
                    >
                      {node.title} ({node.node_key})
                    </button>
                  );
                })}
              </div>
            )}
          </Card>

          {/* Main Canvas + Right Tool Drawer */}
          <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
            <Card className="lg:col-span-8 p-2 overflow-hidden relative">
              <div
                ref={canvasContainerRef}
                className="w-full h-[540px] bg-background"
                data-testid="xbluegraph-canvas"
              />
              {visibleNodes.length === 0 && !loading && (
                <div className="absolute inset-0 flex items-center justify-center pointer-events-none text-xs text-muted-foreground">
                  当前画布暂无业务节点，请通过 xBlueStudio 或数据映射写入业务对象。
                </div>
              )}
            </Card>

            <Card className="lg:col-span-4 p-3 space-y-3 max-h-[558px] overflow-y-auto">
              {/* Side Drawer Tool Tabs */}
              <div className="grid grid-cols-4 gap-1 rounded-md border border-border bg-muted/40 p-0.5">
                {(
                  [
                    { key: "inspector", label: "详情", icon: Info },
                    { key: "neighbors", label: "扩展", icon: Share2 },
                    { key: "path", label: "路径", icon: Route },
                    { key: "algo", label: "算法", icon: Cpu },
                  ] as const
                ).map((item) => {
                  const Icon = item.icon;
                  const isActive = sideTab === item.key;
                  return (
                    <button
                      key={item.key}
                      type="button"
                      onClick={() => setSideTab(item.key)}
                      data-testid={`side-tab-${item.key}`}
                      className={`inline-flex items-center justify-center gap-1 rounded py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                        isActive
                          ? "bg-background text-foreground border border-border"
                          : "text-muted-foreground hover:text-foreground"
                      }`}
                    >
                      <Icon className={`h-3 w-3 ${isActive ? "text-xblue" : ""}`} />
                      {item.label}
                    </button>
                  );
                })}
              </div>

              {sideTab === "inspector" && (
                <NodeInspector
                  node={selectedNode}
                  onExpandOneHop={handleExpandOneHop}
                />
              )}

              {sideTab === "neighbors" && (
                <NeighborExpand
                  nodes={nodes}
                  selectedNodeId={selectedNodeId}
                  knownLinkTypes={knownLinkTypes}
                  onMergeSubgraph={mergeSubgraphIntoCanvas}
                />
              )}

              {sideTab === "path" && (
                <PathFinder
                  nodes={nodes}
                  onPathFound={handlePathFound}
                  onClearHighlight={() => setHighlightedPathIds([])}
                />
              )}

              {sideTab === "algo" && (
                <AlgoPanel
                  onApplyVisualMapping={(mode, entries) => {
                    setAlgoMode(mode);
                    setAlgoEntries(entries);
                  }}
                  onSelectNodeId={(id) => handleSelectNode(id)}
                />
              )}
            </Card>
          </div>
        </>
      ) : (
        /* Secondary Hypergraph Knowledge Layer Compatibility View */
        <Card className="p-4 space-y-3" data-testid="hypergraph-layer-panel">
          <div className="text-xs font-semibold">
            超图文档与高频实体网络 ({hypergraph?.nodes.length ?? 0} 节点 ·{" "}
            {hypergraph?.edges.length ?? 0} 边)
          </div>
          {!hypergraph || hypergraph.nodes.length === 0 ? (
            <div className="py-12 text-center text-xs text-muted-foreground">
              暂无超图节点，请先在「AxIngest · 知识入库」页面写入文档。
            </div>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-3 gap-2 text-xs">
              {hypergraph.nodes.map((hn) => (
                <div
                  key={hn.id}
                  className="rounded border border-border p-2.5 flex items-center justify-between"
                >
                  <span className="font-medium truncate">{hn.label}</span>
                  <div className="flex items-center gap-1.5 shrink-0">
                    <Badge variant="xblue">{hn.node_type}</Badge>
                    <span className="font-mono-tech text-muted-foreground">
                      w={hn.weight}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
