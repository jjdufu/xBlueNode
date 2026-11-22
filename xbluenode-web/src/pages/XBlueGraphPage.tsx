import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import {
  ChevronDown,
  ChevronUp,
  Cpu,
  FileText,
  Info,
  RefreshCw,
  Route,
  Share2,
  Table2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import { MODULE_META } from "@/modules/meta";
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
import {
  Badge,
  Button,
  Card,
  EmptyState,
  IconButton,
  PageHeader,
  SegmentedControl,
} from "@/components/ui/primitives";
import { AlgoLegend } from "@/components/graph/AlgoLegend";
import { GraphToolbar } from "@/components/graph/GraphToolbar";
import { PathPanel } from "@/components/graph/PathPanel";
import {
  SigmaCanvas,
  translateEdgeTypeLabel,
  translateNodeTypeLabel,
  type CanvasEdge,
  type GraphColorMode,
} from "@/components/graph/SigmaCanvas";
import {
  AlgoPanel,
  type AlgoVisualMode,
} from "@/components/xbluegraph/AlgoPanel";
import {
  GraphPatternBar,
  type GraphPatternFilterValues,
} from "@/components/xbluegraph/GraphPatternBar";
import {
  aggregateNodesByType,
  computeNodeDegrees,
  selectTopMetricColumns,
} from "@/components/xbluegraph/logic";
import { MetricChart } from "@/components/xbluegraph/MetricChart";
import { MetricExplainChain } from "@/components/xbluegraph/MetricExplainChain";
import { NeighborExpand } from "@/components/xbluegraph/NeighborExpand";
import { NodeInspector } from "@/components/xbluegraph/NodeInspector";
import { cn } from "@/lib/utils";

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
  const { isZh, tx, fmtNum } = useI18n();
  const { isDark } = useTheme();
  const [searchParams] = useSearchParams();
  const initialNodeIdParam = searchParams.get("node_id");

  const [graphLayer, setGraphLayer] = useState<GraphLayer>("business");
  const [hypergraph, setHypergraph] = useState<TopologyGraph | null>(null);
  const [hyperSelectedId, setHyperSelectedId] = useState<string | null>(null);
  const [hyperTypeFilter, setHyperTypeFilter] = useState<string>("");
  const [hyperSearchQuery, setHyperSearchQuery] = useState<string>("");
  const [hyperFa2Running, setHyperFa2Running] = useState<boolean>(false);

  const [nodes, setNodes] = useState<BlueNode[]>([]);
  const [edges, setEdges] = useState<CanvasEdge[]>([]);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(
    initialNodeIdParam
  );
  const [activeMetric, setActiveMetric] = useState<string | null>(null);
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
  const [activeSubgraph, setActiveSubgraph] = useState("main");
  const [summaryOpen, setSummaryOpen] = useState(false);

  const loadInitialGraph = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const sub = await xblueApi
        .querySubgraph({ limit: 300 })
        .catch(() => ({ nodes: [], links: [], truncated: false }));
      let subNodes = Array.isArray(sub?.nodes) ? sub.nodes : [];
      const subLinks = Array.isArray(sub?.links) ? sub.links : [];

      if (
        initialNodeIdParam &&
        !subNodes.some((n) => n.id === initialNodeIdParam)
      ) {
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
          .listNodes({ limit: 300 })
          .catch(() => ({
            items: [],
            nodes: [],
            links: [],
            next_cursor: null,
          }));
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
    setLoading(true);
    try {
      const data = await api.topology();
      const safeNodes = Array.isArray(data?.nodes) ? data.nodes : [];
      const safeEdges = Array.isArray(data?.edges) ? data.edges : [];
      setHypergraph({
        nodes: safeNodes,
        edges: safeEdges,
      });
      setHyperSelectedId((prev) =>
        prev && safeNodes.some((n) => n.id === prev)
          ? prev
          : (safeNodes[0]?.id ?? null)
      );
    } catch {
      setHypergraph({ nodes: [], edges: [] });
    } finally {
      setLoading(false);
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

  const mergeSubgraphIntoCanvas = useCallback((res: SubgraphResponse) => {
    const incomingNodes = Array.isArray(res?.nodes) ? res.nodes : [];
    const incomingLinks = Array.isArray(res?.links) ? res.links : [];
    setNodes((prev) => mergeNodesDeduplicated(prev, incomingNodes));
    setEdges((prev) => mergeEdgesDeduplicated(prev, incomingLinks));
  }, []);

  const handleSelectNode = useCallback((nodeId: string) => {
    setSelectedNodeId(nodeId);
    setActiveMetric(null);
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

  const summaryTypeFacets = useMemo(
    () => aggregateNodesByType(visibleNodes),
    [visibleNodes]
  );

  const nodeDegrees = useMemo(
    () => computeNodeDegrees(visibleNodes, visibleEdges),
    [visibleEdges, visibleNodes]
  );

  const summaryMetricCols = useMemo(
    () => selectTopMetricColumns(visibleNodes, 3),
    [visibleNodes]
  );

  const handleApplyPattern = useCallback(
    async (pat: GraphPatternFilterValues) => {
      setLoading(true);
      setErrorMsg(null);
      setTypeFilter("");
      try {
        const sub = await xblueApi.querySubgraph({
          subgraph: pat.subgraph || undefined,
          src_type: pat.srcType || undefined,
          edge_type: pat.edgeType || undefined,
          dst_type: pat.dstType || undefined,
          limit: 500,
        });
        const matchedNodes = Array.isArray(sub?.nodes) ? sub.nodes : [];
        const matchedLinks = Array.isArray(sub?.links) ? sub.links : [];
        setNodes(matchedNodes);
        setEdges(mergeEdgesDeduplicated([], matchedLinks));
        setHighlightedPathIds([]);
        setSelectedNodeId((prev) =>
          prev && matchedNodes.some((n) => n.id === prev)
            ? prev
            : (matchedNodes[0]?.id ?? null)
        );
      } catch (err) {
        setErrorMsg(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    },
    []
  );

  const handleResetPattern = useCallback(() => {
    setTypeFilter("");
    setHighlightedPathIds([]);
    void loadInitialGraph();
  }, [loadInitialGraph]);

  // Convert hypergraph TopologyGraph into interactive SigmaCanvas nodes & edges (no synthetic ENTITY-xx suffix!)
  const hyperCanvasNodes: BlueNode[] = useMemo(() => {
    const list = hypergraph?.nodes ?? [];
    const q = hyperSearchQuery.trim().toLowerCase();
    return list
      .filter((hn) => {
        if (hyperTypeFilter && hn.node_type !== hyperTypeFilter) return false;
        if (q && !hn.label.toLowerCase().includes(q)) return false;
        return true;
      })
      .map((hn) => ({
        id: hn.id,
        type_name: hn.node_type || "ENTITY",
        node_key: "",
        title: hn.label,
        properties: {},
        metrics: { weight: hn.weight },
      }));
  }, [hypergraph?.nodes, hyperTypeFilter, hyperSearchQuery]);

  const hyperCanvasNodeIds = useMemo(
    () => new Set(hyperCanvasNodes.map((n) => n.id)),
    [hyperCanvasNodes]
  );

  const hyperCanvasEdges: CanvasEdge[] = useMemo(() => {
    const list = hypergraph?.edges ?? [];
    return list
      .filter(
        (he) =>
          hyperCanvasNodeIds.has(he.source) && hyperCanvasNodeIds.has(he.target)
      )
      .map((he, idx) => ({
        id: he.id || `hg-edge-${idx}`,
        link_type: he.rel_type || "RELATES",
        src_node_id: he.source,
        dst_node_id: he.target,
        weight: 1,
      }));
  }, [hypergraph?.edges, hyperCanvasNodeIds]);

  const hyperDistinctTypes = useMemo(() => {
    const s = new Set<string>();
    for (const hn of hypergraph?.nodes ?? []) {
      if (hn.node_type) s.add(hn.node_type);
    }
    return Array.from(s).sort();
  }, [hypergraph?.nodes]);

  const selectedHyperNode = useMemo(
    () =>
      (hypergraph?.nodes ?? []).find((n) => n.id === hyperSelectedId) ?? null,
    [hypergraph?.nodes, hyperSelectedId]
  );

  const selectedHyperEdges = useMemo(() => {
    if (!selectedHyperNode) return [];
    return (hypergraph?.edges ?? []).filter(
      (e) =>
        e.source === selectedHyperNode.id || e.target === selectedHyperNode.id
    );
  }, [hypergraph?.edges, selectedHyperNode]);

  const hyperNodeLabelById = useMemo(() => {
    const m = new Map<string, string>();
    for (const hn of hypergraph?.nodes ?? []) {
      m.set(hn.id, hn.label);
    }
    return m;
  }, [hypergraph?.nodes]);

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
    <div
      className="flex flex-1 min-h-0 flex-col gap-2.5 overflow-hidden"
      data-testid="xbluegraph-page"
    >
      {/* Unified Header */}
      <PageHeader
        className="shrink-0 pb-2.5"
        title={tx(MODULE_META.xbluegraph.label, MODULE_META.xbluegraph.enLabel)}
        badges={
          <div className="flex items-center gap-3 pl-1 text-xs text-muted-foreground">
            <span className="inline-flex items-baseline gap-1">
              <span>{tx("节点", "Nodes")}</span>
              <strong
                className="font-mono-tech font-semibold text-foreground tabular-nums"
                data-testid="canvas-node-count"
              >
                {graphLayer === "business"
                  ? visibleNodes.length
                  : hyperCanvasNodes.length}
              </strong>
            </span>
            <span className="text-border">·</span>
            <span className="inline-flex items-baseline gap-1">
              <span>{tx("关系", "Edges")}</span>
              <strong
                className="font-mono-tech font-semibold text-foreground tabular-nums"
                data-testid="canvas-edge-count"
              >
                {graphLayer === "business"
                  ? visibleEdges.length
                  : hyperCanvasEdges.length}
              </strong>
            </span>
          </div>
        }
        actions={
          <>
            <SegmentedControl<GraphLayer>
              value={graphLayer}
              onChange={setGraphLayer}
              items={[
                {
                  key: "business",
                  label: tx("业务对象图谱", "Business Graph"),
                },
                {
                  key: "hypergraph",
                  label: tx("知识超图网络", "Knowledge Hypergraph"),
                },
              ]}
            />

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
              <RefreshCw
                className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
              />
              {tx("刷新", "Refresh")}
            </Button>
          </>
        }
      />

      {errorMsg && (
        <div
          className="shrink-0 rounded border border-red-500/30 bg-red-500/5 px-3 py-1.5 text-xs flex items-center justify-between"
          data-testid="graph-error-banner"
        >
          <span className="text-red-600 dark:text-red-400">{errorMsg}</span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground cursor-pointer"
            onClick={() => setErrorMsg(null)}
          >
            {tx("关闭", "Close")}
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

          {/* Main 100vh Split: Left Canvas (+ Wide Bottom Telemetry Panel on Metric Click) & Right Compact Inspector */}
          <div className="grid flex-1 min-h-0 grid-cols-1 lg:grid-cols-12 gap-3 overflow-hidden">
            <div className="lg:col-span-8 flex flex-col min-h-0 gap-2.5 overflow-hidden">
              <GraphPatternBar
                subgraph={activeSubgraph}
                onSubgraphChange={setActiveSubgraph}
                fallbackNodeTypes={distinctTypes}
                fallbackEdgeTypes={knownLinkTypes}
                loading={loading}
                onApplyPattern={(pat) => void handleApplyPattern(pat)}
                onResetPattern={handleResetPattern}
              />

              <Card className="flex-1 min-h-0 p-0 overflow-hidden relative flex flex-col">
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
                    {tx("暂无业务节点", "No business nodes")}
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

              {/* Collapsible Bottom Summary Drawer */}
              <Card
                className="shrink-0 overflow-hidden"
                data-testid="graph-summary-drawer"
              >
                <div
                  className="flex items-center justify-between gap-2 px-3 py-1.5 bg-muted/20 text-xs cursor-pointer select-none"
                  onClick={() => setSummaryOpen((v) => !v)}
                >
                  <div className="flex items-center gap-2">
                    <Table2 className="h-3.5 w-3.5 text-xblue" />
                    <span className="font-semibold">
                      {tx("汇总表", "Summary")}
                    </span>
                    <Badge variant="outline" data-testid="graph-summary-badge">
                      {tx(
                        `节点 ${visibleNodes.length} · 关系 ${visibleEdges.length}`,
                        `Nodes ${visibleNodes.length} · Edges ${visibleEdges.length}`
                      )}
                    </Badge>
                  </div>
                  <IconButton
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setSummaryOpen((v) => !v);
                    }}
                    aria-label={tx("汇总表", "Summary")}
                    data-testid="graph-summary-toggle-btn"
                  >
                    {summaryOpen ? (
                      <ChevronDown className="h-3.5 w-3.5" />
                    ) : (
                      <ChevronUp className="h-3.5 w-3.5" />
                    )}
                  </IconButton>
                </div>

                {summaryOpen && (
                  <div className="border-t border-border p-2.5 grid grid-cols-1 lg:grid-cols-12 gap-3 max-h-56 overflow-hidden text-xs">
                    {/* Left: Per-type count & top metric mean pills */}
                    <div className="lg:col-span-4 overflow-y-auto space-y-1.5 pr-1">
                      {summaryTypeFacets.length === 0 ? (
                        <div className="py-4 text-center text-muted-foreground">
                          {tx("暂无节点", "No nodes")}
                        </div>
                      ) : (
                        summaryTypeFacets.map((facet) => {
                          const active = typeFilter === facet.typeName;
                          return (
                            <button
                              key={facet.typeName}
                              type="button"
                              onClick={() =>
                                setTypeFilter((prev) =>
                                  prev === facet.typeName ? "" : facet.typeName
                                )
                              }
                              data-testid={`summary-type-pill-${facet.typeName}`}
                              className={cn(
                                "w-full flex items-center justify-between gap-2 rounded border px-2.5 py-1.5 text-left transition-colors cursor-pointer",
                                active
                                  ? "border-xblue bg-xblue-soft text-xblue font-medium"
                                  : "border-border bg-background hover:bg-muted/50"
                              )}
                            >
                              <span className="inline-flex items-center gap-1.5 truncate">
                                <span className="font-medium truncate">
                                  {translateNodeTypeLabel(facet.typeName, isZh)}
                                </span>
                                <Badge variant="neutral">{facet.count}</Badge>
                              </span>
                              {facet.topMetric &&
                                facet.topMetricMean !== null && (
                                  <span className="font-mono-tech text-[11px] text-muted-foreground shrink-0 tabular-nums">
                                    {facet.topMetric}={fmtNum(facet.topMetricMean, 1)}
                                  </span>
                                )}
                            </button>
                          );
                        })
                      )}
                    </div>

                    {/* Right: Node detail table */}
                    <div className="lg:col-span-8 overflow-auto rounded border border-border">
                      <table
                        className="w-full border-collapse text-xs"
                        data-testid="graph-summary-table"
                      >
                        <thead className="sticky top-0 bg-muted border-b border-border text-[11px] text-muted-foreground">
                          <tr>
                            <th className="px-2.5 py-1.5 text-left font-medium">
                              {tx("标识", "Key")}
                            </th>
                            <th className="px-2.5 py-1.5 text-left font-medium">
                              {tx("名称", "Name")}
                            </th>
                            <th className="px-2.5 py-1.5 text-left font-medium">
                              {tx("类型", "Type")}
                            </th>
                            <th className="px-2.5 py-1.5 text-right font-medium">
                              {tx("度数", "Degree")}
                            </th>
                            {summaryMetricCols.map((mKey) => (
                              <th
                                key={mKey}
                                className="px-2.5 py-1.5 text-right font-mono-tech font-medium"
                              >
                                {mKey}
                              </th>
                            ))}
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-border/70">
                          {visibleNodes.slice(0, 300).map((n) => {
                            const rowActive = n.id === selectedNodeId;
                            const deg = nodeDegrees.get(n.id) ?? 0;
                            return (
                              <tr
                                key={n.id}
                                onClick={() => handleSelectNode(n.id)}
                                data-testid={`summary-node-row-${n.node_key || n.id}`}
                                className={cn(
                                  "cursor-pointer transition-colors",
                                  rowActive
                                    ? "bg-xblue-soft/60 text-xblue font-medium"
                                    : "hover:bg-muted/40"
                                )}
                              >
                                <td className="px-2.5 py-1.5 font-mono-tech whitespace-nowrap">
                                  {n.node_key || n.id.slice(0, 8)}
                                </td>
                                <td className="px-2.5 py-1.5 truncate max-w-44">
                                  {n.title}
                                </td>
                                <td className="px-2.5 py-1.5 whitespace-nowrap">
                                  {translateNodeTypeLabel(n.type_name, isZh)}
                                </td>
                                <td className="px-2.5 py-1.5 text-right font-mono-tech tabular-nums">
                                  {deg}
                                </td>
                                {summaryMetricCols.map((mKey) => {
                                  const mv = n.metrics?.[mKey];
                                  return (
                                    <td
                                      key={`${n.id}-${mKey}`}
                                      className="px-2.5 py-1.5 text-right font-mono-tech tabular-nums whitespace-nowrap"
                                    >
                                      {typeof mv === "number"
                                        ? fmtNum(mv, 2)
                                        : "-"}
                                    </td>
                                  );
                                })}
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  </div>
                )}
              </Card>

              {/* Wide Docked Bottom Metric Telemetry & Explain DAG Panel (Replaces narrow sidebar cramming) */}
              {selectedNode && activeMetric && (
                <Card className="shrink-0 h-68 p-3 flex flex-col gap-2 overflow-hidden">
                  <div className="grid flex-1 min-h-0 grid-cols-1 xl:grid-cols-12 gap-3 overflow-hidden">
                    <div className="xl:col-span-7 min-w-0 flex flex-col min-h-0">
                      <MetricChart
                        nodeId={selectedNode.id}
                        nodeTitle={selectedNode.title}
                        metric={activeMetric}
                      />
                    </div>
                    <div className="xl:col-span-5 min-w-0 flex flex-col min-h-0 overflow-y-auto">
                      <MetricExplainChain
                        nodeId={selectedNode.id}
                        metric={activeMetric}
                        onClose={() => setActiveMetric(null)}
                      />
                    </div>
                  </div>
                </Card>
              )}
            </div>

            <Card className="lg:col-span-4 h-full min-h-0 p-3 flex flex-col gap-3 overflow-hidden">
              <SegmentedControl<SideTab>
                className="w-full grid grid-cols-4 shrink-0"
                value={sideTab}
                onChange={setSideTab}
                items={[
                  {
                    key: "inspector",
                    label: tx("详情", "Details"),
                    icon: <Info className="h-3 w-3" />,
                    testId: "side-tab-inspector",
                  },
                  {
                    key: "neighbors",
                    label: tx("扩展", "Expand"),
                    icon: <Share2 className="h-3 w-3" />,
                    testId: "side-tab-neighbors",
                  },
                  {
                    key: "path",
                    label: tx("路径", "Path"),
                    icon: <Route className="h-3 w-3" />,
                    testId: "side-tab-path",
                  },
                  {
                    key: "algo",
                    label: tx("算法", "Algo"),
                    icon: <Cpu className="h-3 w-3" />,
                    testId: "side-tab-algo",
                  },
                ]}
              />

              <div className="flex-1 min-h-0 overflow-y-auto pr-0.5">
                {sideTab === "inspector" && (
                  <NodeInspector
                    node={selectedNode}
                    selectedMetric={activeMetric}
                    onSelectMetric={(m) =>
                      setActiveMetric((prev) => (prev === m ? null : m))
                    }
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
              </div>
            </Card>
          </div>
        </>
      ) : (
        /* Unified Interactive Knowledge Hypergraph View */
        <div
          className="flex flex-1 min-h-0 flex-col gap-2.5 overflow-hidden"
          data-testid="hypergraph-layer-panel"
        >
          <GraphToolbar
            searchQuery={hyperSearchQuery}
            onSearchQueryChange={setHyperSearchQuery}
            onSearchSubmit={(e) => {
              e.preventDefault();
              if (hyperCanvasNodes[0]) {
                setHyperSelectedId(hyperCanvasNodes[0].id);
              }
            }}
            searching={false}
            searchOpen={false}
            onCloseSearch={() => {}}
            searchHits={[]}
            onPickSearchHit={() => {}}
            typeFilter={hyperTypeFilter}
            onTypeFilterChange={setHyperTypeFilter}
            distinctTypes={hyperDistinctTypes}
            colorMode="TYPE"
            onColorModeChange={() => {}}
            colorMetricKey="weight"
            onColorMetricKeyChange={() => {}}
            availableMetrics={[]}
            fa2Running={hyperFa2Running}
            onToggleFa2={() => setHyperFa2Running((v) => !v)}
            visibleNodes={hyperCanvasNodes}
            visibleEdgeCount={hyperCanvasEdges.length}
            selectedNodeId={hyperSelectedId}
            expanding={false}
            onSelectNode={(id) => setHyperSelectedId(id)}
            onExpandNode={() => {}}
            hideColorModeControls
          />

          <div className="grid flex-1 min-h-0 grid-cols-1 lg:grid-cols-12 gap-3 overflow-hidden">
            <Card className="lg:col-span-8 flex-1 min-h-0 p-0 overflow-hidden relative flex flex-col">
              <SigmaCanvas
                nodes={hyperCanvasNodes}
                edges={hyperCanvasEdges}
                selectedNodeId={hyperSelectedId}
                highlightedPathIds={[]}
                colorMode="TYPE"
                colorMetricKey="weight"
                algoMode="NONE"
                algoEntries={[]}
                fa2Running={hyperFa2Running}
                isDark={isDark}
                testId="hypergraph-sigma-canvas"
                onSelectNode={(id) => setHyperSelectedId(id)}
              />
              {hyperCanvasNodes.length === 0 && !loading && (
                <div className="absolute inset-0 flex items-center justify-center pointer-events-none text-xs text-muted-foreground">
                  {tx("暂无匹配超图节点", "No matching hypergraph nodes")}
                </div>
              )}
              <AlgoLegend
                colorMode="TYPE"
                colorMetricKey="weight"
                distinctTypes={hyperDistinctTypes}
                algoMode="NONE"
                algoEntries={[]}
                isDark={isDark}
              />
            </Card>

            <Card className="lg:col-span-4 h-full min-h-0 p-3.5 flex flex-col gap-3 overflow-y-auto text-xs">
              <div className="flex items-center justify-between border-b border-border pb-2 shrink-0">
                <span className="font-semibold flex items-center gap-1.5">
                  <FileText className="h-3.5 w-3.5 text-xblue" />
                  {tx("超图节点详情", "Hypergraph Node")}
                </span>
                {selectedHyperNode && (
                  <Badge variant="xblue">
                    {translateNodeTypeLabel(selectedHyperNode.node_type, isZh)}
                  </Badge>
                )}
              </div>

              {!selectedHyperNode ? (
                <EmptyState
                  title={tx("暂无选中超图节点", "No hypergraph node selected")}
                />
              ) : (
                <div className="space-y-3">
                  <div className="rounded-md bg-muted/35 p-2.5 space-y-1">
                    <div className="text-sm font-semibold leading-snug">
                      {selectedHyperNode.label}
                    </div>
                    <div className="text-[11px] text-muted-foreground">
                      {tx("连接度权重", "Connectivity weight")}:{" "}
                      <span className="font-mono-tech tabular-nums text-foreground font-medium">
                        {fmtNum(selectedHyperNode.weight, 2)}
                      </span>
                    </div>
                  </div>

                  <div className="space-y-1.5">
                    <div className="text-[11px] font-semibold text-muted-foreground">
                      {tx("关联超边与事实", "Connected Hyperedges")} (
                      {selectedHyperEdges.length})
                    </div>
                    {selectedHyperEdges.length === 0 ? (
                      <div className="rounded-md bg-muted/25 py-6 text-center text-muted-foreground">
                        {tx("当前节点暂无直接关联超边", "No connected hyperedges")}
                      </div>
                    ) : (
                      <div className="space-y-1.5">
                        {selectedHyperEdges.slice(0, 30).map((he, idx) => {
                          const peerId =
                            he.source === selectedHyperNode.id
                              ? he.target
                              : he.source;
                          const peerLabel =
                            hyperNodeLabelById.get(peerId) ??
                            peerId.slice(0, 8);
                          return (
                            <button
                              key={`${he.id}-${idx}`}
                              type="button"
                              onClick={() => setHyperSelectedId(peerId)}
                              className="w-full text-left rounded-md border border-border/80 bg-background p-2.5 space-y-1 hover:border-xblue hover:bg-xblue-soft/20 transition-colors cursor-pointer"
                            >
                              <div className="flex items-center justify-between gap-2">
                                <span className="font-medium truncate">
                                  {peerLabel}
                                </span>
                                <Badge variant="outline">
                                  {translateEdgeTypeLabel(he.rel_type, isZh)}
                                </Badge>
                              </div>
                              {he.fact && (
                                <div className="text-[11px] text-muted-foreground leading-relaxed line-clamp-3">
                                  {he.fact}
                                </div>
                              )}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                </div>
              )}
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
