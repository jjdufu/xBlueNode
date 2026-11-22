import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import {
  Cpu,
  FileText,
  Info,
  Pause,
  Play,
  RefreshCw,
  Route,
  Share2,
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
  Select,
} from "@/components/ui/primitives";
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
  const { tx, tEnum, fmtNum } = useI18n();
  const { isDark } = useTheme();
  const [searchParams] = useSearchParams();
  const initialNodeIdParam = searchParams.get("node_id");

  const [graphLayer, setGraphLayer] = useState<GraphLayer>("business");
  const [hypergraph, setHypergraph] = useState<TopologyGraph | null>(null);
  const [hyperSelectedId, setHyperSelectedId] = useState<string | null>(null);
  const [hyperTypeFilter, setHyperTypeFilter] = useState<string>("");
  const [hyperFa2Running, setHyperFa2Running] = useState<boolean>(false);

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

  // Convert hypergraph TopologyGraph into interactive SigmaCanvas nodes & edges
  const hyperCanvasNodes: BlueNode[] = useMemo(() => {
    const list = hypergraph?.nodes ?? [];
    return list
      .filter((hn) => !hyperTypeFilter || hn.node_type === hyperTypeFilter)
      .map((hn, idx) => ({
        id: hn.id,
        type_name: hn.node_type || "ENTITY",
        node_key: `${hn.node_type || "H"}-${idx + 1}`,
        title: hn.label,
        properties: { node_type: hn.node_type },
        metrics: { weight: hn.weight },
      }));
  }, [hypergraph?.nodes, hyperTypeFilter]);

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
    <div className="space-y-4" data-testid="xbluegraph-page">
      {/* Header */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3.5">
        <div className="flex flex-wrap items-center gap-2.5">
          <h1 className="text-base font-semibold tracking-tight">
            {tx(MODULE_META.xbluegraph.label, MODULE_META.xbluegraph.enLabel)}
          </h1>
          <Badge variant="outline">
            {graphLayer === "business"
              ? `${fmtNum(visibleNodes.length, 0)} ${tx("节点", "Nodes")}`
              : `${fmtNum(hyperCanvasNodes.length, 0)} ${tx("实体", "Entities")}`}
          </Badge>
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
              {tx("业务对象图谱", "Business Graph")}
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
              {tx("知识超图网络", "Knowledge Hypergraph")}
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
            <RefreshCw
              className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
            />
            {tx("刷新拓扑", "Refresh")}
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

          {/* Main Sigma Canvas + Right Tool Inspector */}
          <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
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

            <Card className="lg:col-span-4 p-3 space-y-3 max-h-[620px] overflow-y-auto">
              {/* Side Drawer Tool Tabs */}
              <div className="grid grid-cols-4 gap-1 rounded-md border border-border bg-muted/40 p-0.5">
                {(
                  [
                    {
                      key: "inspector",
                      label: tx("详情", "Details"),
                      icon: Info,
                    },
                    {
                      key: "neighbors",
                      label: tx("扩展", "Expand"),
                      icon: Share2,
                    },
                    { key: "path", label: tx("路径", "Path"), icon: Route },
                    {
                      key: "algo",
                      label: tx("算法", "Algorithms"),
                      icon: Cpu,
                    },
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
                      <Icon
                        className={`h-3 w-3 ${isActive ? "text-xblue" : ""}`}
                      />
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
        /* Interactive Knowledge Hypergraph View */
        <div className="space-y-3" data-testid="hypergraph-layer-panel">
          <Card className="p-3 flex flex-wrap items-center justify-between gap-2 text-xs">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-semibold">
                {tx("知识超图网络", "Knowledge Hypergraph")}
              </span>
              <Badge variant="xblue">
                {fmtNum(hyperCanvasNodes.length, 0)} {tx("节点", "Nodes")}
              </Badge>
              <Badge variant="neutral">
                {fmtNum(hyperCanvasEdges.length, 0)} {tx("关系", "Edges")}
              </Badge>
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <Select
                value={hyperTypeFilter}
                onChange={(e) => setHyperTypeFilter(e.target.value)}
                className="h-8 font-mono-tech"
              >
                <option value="">{tx("全部超图类型", "All Types")}</option>
                {hyperDistinctTypes.map((ht) => (
                  <option key={ht} value={ht}>
                    {ht} ({tEnum(ht)})
                  </option>
                ))}
              </Select>

              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-8 px-2.5"
                onClick={() => setHyperFa2Running((v) => !v)}
              >
                {hyperFa2Running ? (
                  <Pause className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Play className="h-3.5 w-3.5" />
                )}
                {hyperFa2Running
                  ? tx("暂停布局", "Pause Layout")
                  : tx("力导布局", "Force Layout")}
              </Button>
            </div>
          </Card>

          <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
            <Card className="lg:col-span-8 p-0 overflow-hidden relative flex flex-col">
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
                  {tx("暂无超图节点", "No hypergraph nodes")}
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

            <Card className="lg:col-span-4 p-3.5 space-y-3 max-h-[620px] overflow-y-auto text-xs">
              <div className="flex items-center justify-between border-b border-border pb-2">
                <span className="font-semibold flex items-center gap-1.5">
                  <FileText className="h-3.5 w-3.5 text-xblue" />
                  {tx("超图节点详情", "Hypergraph Node")}
                </span>
                {selectedHyperNode && (
                  <Badge variant="xblue">
                    {tEnum(selectedHyperNode.node_type)}
                  </Badge>
                )}
              </div>

              {!selectedHyperNode ? (
                <EmptyState
                  title={tx("暂无选中超图节点", "No hypergraph node selected")}
                />
              ) : (
                <div className="space-y-3">
                  <div className="rounded border border-border bg-muted/20 p-2.5 space-y-1">
                    <div className="text-sm font-semibold">
                      {selectedHyperNode.label}
                    </div>
                    <div className="flex items-center gap-2 font-mono-tech text-[11px] text-muted-foreground">
                      <span>{selectedHyperNode.node_type}</span>
                      <span>·</span>
                      <span>
                        {tx("权重", "Weight")} ={" "}
                        {fmtNum(selectedHyperNode.weight, 2)}
                      </span>
                    </div>
                  </div>

                  <div className="space-y-1.5">
                    <div className="font-semibold text-muted-foreground">
                      {tx("关联超边与事实", "Connected Hyperedges")} (
                      {selectedHyperEdges.length})
                    </div>
                    {selectedHyperEdges.length === 0 ? (
                      <div className="text-muted-foreground">
                        {tx("暂无关联超边", "No connected edges")}
                      </div>
                    ) : (
                      <div className="space-y-1.5">
                        {selectedHyperEdges.slice(0, 20).map((he, idx) => {
                          const peerId =
                            he.source === selectedHyperNode.id
                              ? he.target
                              : he.source;
                          const peerLabel =
                            hyperNodeLabelById.get(peerId) ?? peerId.slice(0, 8);
                          return (
                            <button
                              key={`${he.id}-${idx}`}
                              type="button"
                              onClick={() => setHyperSelectedId(peerId)}
                              className="w-full text-left rounded border border-border bg-background p-2 space-y-1 hover:border-xblue transition-colors cursor-pointer"
                            >
                              <div className="flex items-center justify-between gap-2">
                                <span className="font-medium truncate">
                                  {peerLabel}
                                </span>
                                <Badge variant="outline">{he.rel_type}</Badge>
                              </div>
                              {he.fact && (
                                <div className="text-[11px] text-muted-foreground line-clamp-2">
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

              {(hypergraph?.nodes.length ?? 0) > 0 && (
                <div className="space-y-1.5 border-t border-border pt-3">
                  <div className="font-semibold text-muted-foreground">
                    {tx("节点索引", "Node Index")} (
                    {hypergraph?.nodes.length ?? 0})
                  </div>
                  <div className="flex flex-wrap gap-1.5 max-h-40 overflow-y-auto">
                    {(hypergraph?.nodes ?? []).slice(0, 40).map((hn) => {
                      const active = hn.id === hyperSelectedId;
                      return (
                        <button
                          key={hn.id}
                          type="button"
                          onClick={() => setHyperSelectedId(hn.id)}
                          className={`rounded border px-2 py-0.5 text-[11px] transition-colors cursor-pointer ${
                            active
                              ? "border-xblue bg-xblue-soft text-xblue font-medium"
                              : "border-border bg-background hover:bg-muted"
                          }`}
                        >
                          {hn.label}
                        </button>
                      );
                    })}
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
