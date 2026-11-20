import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import { Cpu, Info, RefreshCw, Route, Share2 } from "lucide-react";
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
import { Badge, Button, Card } from "@/components/ui/primitives";
import { AlgoLegend } from "@/components/graph/AlgoLegend";
import { GraphToolbar } from "@/components/graph/GraphToolbar";
import { PathPanel } from "@/components/graph/PathPanel";
import {
  SigmaCanvas,
  type CanvasEdge,
  type GraphColorMode,
} from "@/components/graph/SigmaCanvas";
import {
  AlgoPanel,
  type AlgoVisualMode,
} from "@/components/xbluegraph/AlgoPanel";
import { NeighborExpand } from "@/components/xbluegraph/NeighborExpand";
import { NodeInspector } from "@/components/xbluegraph/NodeInspector";

type SideTab = "inspector" | "neighbors" | "path" | "algo";
type GraphLayer = "business" | "hypergraph";

function toCanvasEdge(lk: BlueLink | LinkRecord): CanvasEdge {
  return {
    id: lk.id || `${lk.link_type}:${lk.src_node_id}->${lk.dst_node_id}`,
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

export function XBlueGraphPage() {
  const [searchParams] = useSearchParams();
  const initialNodeIdParam = searchParams.get("node_id");

  const [graphLayer, setGraphLayer] = useState<GraphLayer>("business");
  const [hypergraph, setHypergraph] = useState<TopologyGraph | null>(null);

  const [nodes, setNodes] = useState<BlueNode[]>([]);
  const [edges, setEdges] = useState<CanvasEdge[]>([]);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(
    initialNodeIdParam
  );
  const [sideTab, setSideTab] = useState<SideTab>("inspector");
  const [typeFilter, setTypeFilter] = useState<string>("");

  const [colorMode, setColorMode] = useState<GraphColorMode>("TYPE");
  const [colorMetricKey, setColorMetricKey] = useState<string>("inventory");
  const [fa2Running, setFa2Running] = useState<boolean>(false);

  const [presetPathFrom, setPresetPathFrom] = useState<string | null>(null);
  const [presetPathTo, setPresetPathTo] = useState<string | null>(null);

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
      let sub = await xblueApi
        .querySubgraph({ type_names: "Factory,Warehouse", limit: 1000 })
        .catch(() => ({ nodes: [], links: [], truncated: false }));
      if (!Array.isArray(sub?.nodes) || sub.nodes.length === 0) {
        sub = await xblueApi
          .querySubgraph({ limit: 1000 })
          .catch(() => ({ nodes: [], links: [], truncated: false }));
      }
      let subNodes = Array.isArray(sub?.nodes) ? sub.nodes : [];
      const subLinks = Array.isArray(sub?.links) ? sub.links : [];

      if (initialNodeIdParam && !subNodes.some((n) => n.id === initialNodeIdParam)) {
        const focused = await xblueApi
          .getNode(initialNodeIdParam)
          .catch(() => null);
        if (focused) {
          subNodes = [focused, ...subNodes];
        }
      }

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
          .listNodes({ limit: 500 })
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
  }, [initialNodeIdParam]);

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

  const distinctTypes = useMemo(() => {
    const set = new Set<string>();
    for (const n of nodes) {
      if (n.type_name) set.add(n.type_name);
    }
    return Array.from(set).sort();
  }, [nodes]);

  const availableMetrics = useMemo(() => {
    const set = new Set<string>();
    for (const n of nodes) {
      for (const k of Object.keys(n.metrics ?? {})) {
        set.add(k);
      }
    }
    return Array.from(set).sort();
  }, [nodes]);

  useEffect(() => {
    if (
      availableMetrics.length > 0 &&
      !availableMetrics.includes(colorMetricKey)
    ) {
      setColorMetricKey(availableMetrics[0] ?? "");
    }
  }, [availableMetrics, colorMetricKey]);

  const knownLinkTypes = useMemo(() => {
    const set = new Set<string>();
    for (const e of edges) {
      if (e.link_type) set.add(e.link_type);
    }
    return Array.from(set).sort();
  }, [edges]);

  const visibleNodes = useMemo(
    () => (typeFilter ? nodes.filter((n) => n.type_name === typeFilter) : nodes),
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

  const handleColorModeChange = (mode: GraphColorMode) => {
    setColorMode(mode);
    if (mode === "ALGO" && algoEntries.length === 0) {
      xblueApi
        .getAlgorithmResults("PAGERANK", { limit: 200 })
        .then((res) => {
          const items = Array.isArray(res?.items) ? res.items : [];
          if (items.length > 0) {
            setAlgoEntries(
              items.map((r) => ({
                node_id: r.node_id,
                title: r.title,
                score: r.score,
                community: r.community,
              }))
            );
            setAlgoMode("SCORE_SIZE");
          }
        })
        .catch(() => {
          // Ignore if no precomputed algorithm results exist
        });
    }
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
            Sigma.js WebGL 万级节点力导向关系网络、时序指标钻取、多跳遍历、最短路径与图算法工作台
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
          <GraphToolbar
            searchQuery={searchQuery}
            onSearchQueryChange={setSearchQuery}
            onSearchSubmit={(e) => void handleHybridSearch(e)}
            searching={searching}
            searchOpen={searchOpen}
            onCloseSearch={() => setSearchOpen(false)}
            searchHits={searchHits}
            onPickSearchHit={handlePickSearchHit}
            typeFilter={typeFilter}
            onTypeFilterChange={setTypeFilter}
            distinctTypes={distinctTypes}
            colorMode={colorMode}
            onColorModeChange={handleColorModeChange}
            colorMetricKey={colorMetricKey}
            onColorMetricKeyChange={setColorMetricKey}
            availableMetrics={availableMetrics}
            fa2Running={fa2Running}
            onToggleFa2={() => setFa2Running((prev) => !prev)}
            visibleNodes={visibleNodes}
            visibleEdgeCount={visibleEdges.length}
            selectedNodeId={selectedNodeId}
            expanding={expanding}
            onSelectNode={handleSelectNode}
            onExpandNode={(id) => void handleExpandOneHop(id)}
          />

          {/* Main Sigma Canvas + Right Tool Drawer */}
          <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
            <Card className="lg:col-span-8 p-0 overflow-hidden relative flex flex-col">
              <SigmaCanvas
                nodes={visibleNodes}
                edges={visibleEdges}
                selectedNodeId={selectedNodeId}
                highlightedPathIds={highlightedPathIds}
                colorMode={colorMode}
                colorMetricKey={colorMetricKey}
                algoMode={algoMode}
                algoEntries={algoEntries}
                fa2Running={fa2Running}
                isDark={isDark}
                onSelectNode={handleSelectNode}
                onExpandNode={(id) => void handleExpandOneHop(id)}
                onSetPathEndpoint={(role, nodeId) => {
                  if (role === "from") setPresetPathFrom(nodeId);
                  else setPresetPathTo(nodeId);
                  setSideTab("path");
                }}
              />
              {visibleNodes.length === 0 && !loading && (
                <div className="absolute inset-0 flex items-center justify-center pointer-events-none text-xs text-muted-foreground">
                  当前画布暂无业务节点，请通过 xBlueStudio 或数据映射写入业务对象。
                </div>
              )}
              <AlgoLegend
                colorMode={colorMode}
                colorMetricKey={colorMetricKey}
                distinctTypes={distinctTypes}
                algoMode={algoMode}
                algoEntries={algoEntries}
                isDark={isDark}
              />
            </Card>

            <Card className="lg:col-span-4 p-3 space-y-3 max-h-[580px] overflow-y-auto">
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
                <PathPanel
                  nodes={nodes}
                  presetFromId={presetPathFrom}
                  presetToId={presetPathTo}
                  onPathFound={handlePathFound}
                  onClearHighlight={() => setHighlightedPathIds([])}
                />
              )}

              {sideTab === "algo" && (
                <AlgoPanel
                  onApplyVisualMapping={(mode, entries) => {
                    setAlgoMode(mode);
                    setAlgoEntries(entries);
                    if (mode !== "NONE") setColorMode("ALGO");
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
