import { useEffect, useMemo, useRef, useState } from "react";
import * as echarts from "echarts";
import {
  Check,
  Edit3,
  Link2,
  Network,
  Plus,
  RotateCcw,
  ShieldAlert,
  Table2,
  Trash2,
  X,
} from "lucide-react";
import {
  axsimApi,
  type SandboxGraphLink,
  type SandboxGraphNode,
} from "@/lib/api/axsim";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export interface OverlayEditorProps {
  sandboxId: string;
  nodes: SandboxGraphNode[];
  links: SandboxGraphLink[];
  overlayHash: string;
  loading?: boolean;
  onRefreshGraph: () => Promise<void>;
}

type ViewMode = "table" | "graph";

interface MetricDraftRow {
  key: string;
  value: string;
}

export function OverlayEditor({
  sandboxId,
  nodes,
  links,
  overlayHash,
  loading = false,
  onRefreshGraph,
}: OverlayEditorProps) {
  const [viewMode, setViewMode] = useState<ViewMode>("table");
  const [searchText, setSearchText] = useState("");
  const [showAddNode, setShowAddNode] = useState(false);
  const [showAddLink, setShowAddLink] = useState(false);

  // Add Virtual Node state
  const [newNodeType, setNewNodeType] = useState("Warehouse");
  const [newNodeKey, setNewNodeKey] = useState("");
  const [newNodeTitle, setNewNodeTitle] = useState("");
  const [newNodeMetricKey, setNewNodeMetricKey] = useState("inventory");
  const [newNodeMetricVal, setNewNodeMetricVal] = useState("100");
  const [newNodePropsJson, setNewNodePropsJson] = useState("{}");

  // Add Virtual Link state
  const [newLinkSrc, setNewLinkSrc] = useState("");
  const [newLinkDst, setNewLinkDst] = useState("");
  const [newLinkType, setNewLinkType] = useState("SUPPLIES");
  const [newLinkWeight, setNewLinkWeight] = useState("0.8");

  // Patch Node state
  const [editingNode, setEditingNode] = useState<SandboxGraphNode | null>(null);
  const [editTitle, setEditTitle] = useState("");
  const [editMetrics, setEditMetrics] = useState<MetricDraftRow[]>([]);
  const [newMetricKey, setNewMetricKey] = useState("");
  const [newMetricValue, setNewMetricValue] = useState("");
  const [editPropsText, setEditPropsText] = useState("{}");

  // Patch Link weight inline
  const [editingLinkId, setEditingLinkId] = useState<string | null>(null);
  const [editLinkWeight, setEditLinkWeight] = useState("");

  const [busy, setBusy] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // ECharts graph refs
  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const [isDark, setIsDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

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
    if (nodes.length > 0 && !newLinkSrc) {
      setNewLinkSrc(nodes[0].id);
    }
    if (nodes.length > 1 && !newLinkDst) {
      setNewLinkDst(nodes[1].id);
    } else if (nodes.length === 1 && !newLinkDst) {
      setNewLinkDst(nodes[0].id);
    }
  }, [nodes, newLinkDst, newLinkSrc]);

  const nodeTitleMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const n of nodes) {
      map.set(n.id, `${n.title} (${n.node_key})`);
    }
    return map;
  }, [nodes]);

  const filteredNodes = useMemo(() => {
    const q = searchText.trim().toLowerCase();
    if (!q) return nodes;
    return nodes.filter(
      (n) =>
        n.title.toLowerCase().includes(q) ||
        n.node_key.toLowerCase().includes(q) ||
        n.type_name.toLowerCase().includes(q)
    );
  }, [nodes, searchText]);

  const overlayStats = useMemo(() => {
    const newNodes = nodes.filter((n) => n.is_new).length;
    const modNodes = nodes.filter((n) => n.is_modified && !n.is_new).length;
    const overlayLinks = links.filter((l) => l.is_new || l.is_modified).length;
    return { newNodes, modNodes, overlayLinks };
  }, [links, nodes]);

  const startEditNode = (node: SandboxGraphNode) => {
    setEditingNode(node);
    setEditTitle(node.title);
    setEditMetrics(
      Object.entries(node.metrics ?? {}).map(([key, val]) => ({
        key,
        value: String(val),
      }))
    );
    setNewMetricKey("");
    setNewMetricValue("");
    setEditPropsText(JSON.stringify(node.properties ?? {}, null, 2));
    setErrorMsg(null);
  };

  // Initialize ECharts when in graph mode
  useEffect(() => {
    if (viewMode !== "graph") {
      if (chartRef.current) {
        chartRef.current.dispose();
        chartRef.current = null;
      }
      return;
    }
    const el = chartContainerRef.current;
    if (!el) return;

    const chart = echarts.init(el);
    chartRef.current = chart;

    const resizeObserver = new ResizeObserver(() => {
      chart.resize();
    });
    resizeObserver.observe(el);

    chart.on("click", (params) => {
      if (
        params.dataType === "node" &&
        typeof params.data === "object" &&
        params.data !== null &&
        "id" in params.data
      ) {
        const nodeId = String((params.data as { id: unknown }).id);
        const found = nodes.find((n) => n.id === nodeId);
        if (found) {
          startEditNode(found);
        }
      }
    });

    return () => {
      resizeObserver.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, [viewMode, nodes]);

  // Update ECharts options
  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || viewMode !== "graph") return;

    const fgColor = isDark ? "#fafafa" : "#09090b";
    const mutedColor = isDark ? "#a1a1aa" : "#71717a";
    const cardBg = isDark ? "#18181b" : "#ffffff";
    const borderColor = isDark ? "#27272a" : "#e4e4e7";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    const chartNodes = filteredNodes.map((n, idx) => {
      const isOverlay = n.is_new || n.is_modified;
      const angle = (2 * Math.PI * idx) / Math.max(1, filteredNodes.length);
      return {
        id: n.id,
        name: n.title,
        value: n.type_name,
        x: 240 + Math.cos(angle) * 160,
        y: 160 + Math.sin(angle) * 110,
        symbolSize: isOverlay ? 36 : 28,
        itemStyle: {
          color: n.is_new
            ? accentBlue
            : n.is_modified
              ? isDark
                ? "#1e3a8a"
                : "#dbeafe"
              : cardBg,
          borderColor: isOverlay ? accentBlue : mutedColor,
          borderWidth: isOverlay ? 2.5 : 1.5,
          borderType: isOverlay ? ("dashed" as const) : ("solid" as const),
        },
        label: {
          show: true,
          position: "bottom" as const,
          color: fgColor,
          fontSize: 11,
          formatter: () => `${n.title}${isOverlay ? " [OVERLAY]" : ""}`,
        },
      };
    });

    const visibleIds = new Set(filteredNodes.map((n) => n.id));
    const chartLinks = links
      .filter((l) => visibleIds.has(l.src_node_id) && visibleIds.has(l.dst_node_id))
      .map((l) => {
        const isOverlay = l.is_new || l.is_modified;
        return {
          source: l.src_node_id,
          target: l.dst_node_id,
          value: l.weight,
          lineStyle: {
            color: isOverlay ? accentBlue : mutedColor,
            width: isOverlay ? 2.2 : 1.3,
            type: isOverlay ? ("dashed" as const) : ("solid" as const),
            curveness: 0.12,
          },
          label: {
            show: true,
            formatter: `${l.link_type} · ${l.weight}`,
            fontSize: 10,
            color: mutedColor,
          },
        };
      });

    const option: echarts.EChartsOption = {
      animation: false,
      tooltip: {
        trigger: "item",
        backgroundColor: cardBg,
        borderColor,
        textStyle: { color: fgColor, fontSize: 11 },
      },
      series: [
        {
          type: "graph",
          layout: "force",
          roam: true,
          draggable: true,
          force: {
            repulsion: 280,
            edgeLength: [90, 160],
          },
          edgeSymbol: ["none", "arrow"],
          edgeSymbolSize: [4, 9],
          data: chartNodes,
          links: chartLinks,
        },
      ],
    };

    chart.setOption(option, true);
    chart.resize();
  }, [filteredNodes, isDark, links, viewMode]);

  const handleCreateNode = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newNodeType.trim() || !newNodeKey.trim() || !newNodeTitle.trim()) {
      setErrorMsg("请填写虚拟节点类型、标识键和标题");
      return;
    }
    setBusy(true);
    setErrorMsg(null);
    try {
      const metrics: Record<string, number> = {};
      if (newNodeMetricKey.trim()) {
        const parsed = Number.parseFloat(newNodeMetricVal);
        if (!Number.isNaN(parsed)) {
          metrics[newNodeMetricKey.trim()] = parsed;
        }
      }
      let properties: Record<string, unknown> = {};
      if (newNodePropsJson.trim()) {
        const parsed: unknown = JSON.parse(newNodePropsJson);
        if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
          properties = parsed as Record<string, unknown>;
        }
      }

      await axsimApi.createOverlayNode(sandboxId, {
        type_name: newNodeType.trim(),
        node_key: newNodeKey.trim(),
        title: newNodeTitle.trim(),
        metrics,
        properties,
      });
      setNewNodeKey("");
      setNewNodeTitle("");
      setShowAddNode(false);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSaveNodePatch = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!editingNode) return;
    setBusy(true);
    setErrorMsg(null);
    try {
      const metrics: Record<string, number> = {};
      for (const row of editMetrics) {
        const k = row.key.trim();
        const v = Number.parseFloat(row.value);
        if (k && Number.isFinite(v)) {
          metrics[k] = v;
        }
      }
      if (newMetricKey.trim()) {
        const v = Number.parseFloat(newMetricValue);
        if (Number.isFinite(v)) {
          metrics[newMetricKey.trim()] = v;
        }
      }

      let properties: Record<string, unknown> | undefined;
      if (editPropsText.trim()) {
        const parsed: unknown = JSON.parse(editPropsText);
        if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
          properties = parsed as Record<string, unknown>;
        }
      }

      await axsimApi.patchOverlayNode(sandboxId, editingNode.id, {
        title: editTitle.trim() || editingNode.title,
        metrics,
        ...(properties ? { properties } : {}),
      });
      setEditingNode(null);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleDeleteNode = async (nodeId: string) => {
    setBusy(true);
    setErrorMsg(null);
    try {
      await axsimApi.deleteOverlayNode(sandboxId, nodeId);
      if (editingNode?.id === nodeId) setEditingNode(null);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleCreateLink = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newLinkSrc || !newLinkDst || !newLinkType.trim()) {
      setErrorMsg("请选择起止节点与关系类型");
      return;
    }
    const weight = Number.parseFloat(newLinkWeight);
    setBusy(true);
    setErrorMsg(null);
    try {
      await axsimApi.createOverlayLink(sandboxId, {
        src_node_id: newLinkSrc,
        dst_node_id: newLinkDst,
        link_type: newLinkType.trim(),
        weight: Number.isFinite(weight) ? weight : 1,
      });
      setShowAddLink(false);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSaveLinkWeight = async (linkId: string) => {
    const weight = Number.parseFloat(editLinkWeight);
    if (!Number.isFinite(weight)) return;
    setBusy(true);
    setErrorMsg(null);
    try {
      await axsimApi.patchOverlayLink(sandboxId, linkId, { weight });
      setEditingLinkId(null);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleDeleteLink = async (linkId: string) => {
    setBusy(true);
    setErrorMsg(null);
    try {
      await axsimApi.deleteOverlayLink(sandboxId, linkId);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleResetOverlay = async () => {
    setBusy(true);
    setErrorMsg(null);
    try {
      await axsimApi.resetOverlay(sandboxId);
      setEditingNode(null);
      await onRefreshGraph();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4" data-testid="overlay-editor">
      {/* CoW isolation notice & toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-muted/20 px-3.5 py-2.5">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <Badge variant="xblue">COW OVERLAY · 隔离写入</Badge>
          <span className="text-muted-foreground flex items-center gap-1">
            <ShieldAlert className="h-3.5 w-3.5 text-xblue" />
            所有修改仅写入沙盘覆盖层，绝不修改生产主图
          </span>
          {overlayHash && (
            <Badge variant="outline">hash: {overlayHash.slice(0, 8)}</Badge>
          )}
          <Badge variant="neutral">
            +{overlayStats.newNodes} 虚拟节点 · {overlayStats.modNodes} 已修改 ·{" "}
            {overlayStats.overlayLinks} 覆盖关系
          </Badge>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <div className="inline-flex rounded-md border border-border bg-background p-0.5">
            <button
              type="button"
              onClick={() => setViewMode("table")}
              className={`inline-flex items-center gap-1 rounded px-2.5 py-1 text-xs cursor-pointer ${
                viewMode === "table"
                  ? "bg-muted font-medium text-foreground"
                  : "text-muted-foreground"
              }`}
              data-testid="overlay-view-table-btn"
            >
              <Table2 className="h-3.5 w-3.5" />
              表格视图
            </button>
            <button
              type="button"
              onClick={() => setViewMode("graph")}
              className={`inline-flex items-center gap-1 rounded px-2.5 py-1 text-xs cursor-pointer ${
                viewMode === "graph"
                  ? "bg-muted font-medium text-foreground"
                  : "text-muted-foreground"
              }`}
              data-testid="overlay-view-graph-btn"
            >
              <Network className="h-3.5 w-3.5" />
              拓扑图视图
            </button>
          </div>

          <Button
            type="button"
            variant="outline"
            className="h-8 px-2.5 text-xs"
            onClick={() => setShowAddNode((v) => !v)}
            data-testid="overlay-add-node-toggle"
          >
            <Plus className="h-3.5 w-3.5" />
            虚拟节点
          </Button>
          <Button
            type="button"
            variant="outline"
            className="h-8 px-2.5 text-xs"
            onClick={() => setShowAddLink((v) => !v)}
            data-testid="overlay-add-link-toggle"
          >
            <Link2 className="h-3.5 w-3.5" />
            虚拟关系
          </Button>
          <Button
            type="button"
            variant="ghost"
            className="h-8 px-2.5 text-xs"
            onClick={() => void handleResetOverlay()}
            disabled={busy}
            data-testid="overlay-reset-btn"
          >
            <RotateCcw className="h-3.5 w-3.5" />
            清空覆盖层
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      {/* Add virtual node form */}
      {showAddNode && (
        <Card className="p-4 border-dashed border-xblue/50 space-y-3" data-testid="overlay-add-node-form">
          <div className="flex items-center justify-between text-xs font-medium">
            <span>新增沙盘虚拟节点 (OVERLAY · NEW)</span>
            <button
              type="button"
              onClick={() => setShowAddNode(false)}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <form onSubmit={(e) => void handleCreateNode(e)} className="grid grid-cols-1 md:grid-cols-6 gap-2.5">
            <Input
              value={newNodeType}
              onChange={(e) => setNewNodeType(e.target.value)}
              placeholder="类型 (如 Warehouse)"
              className="h-8 text-xs"
              data-testid="overlay-new-node-type"
            />
            <Input
              value={newNodeKey}
              onChange={(e) => setNewNodeKey(e.target.value)}
              placeholder="标识键 (如 W-VIRTUAL-1)"
              className="h-8 text-xs font-mono-tech"
              data-testid="overlay-new-node-key"
            />
            <Input
              value={newNodeTitle}
              onChange={(e) => setNewNodeTitle(e.target.value)}
              placeholder="显示名称 (如 应急备选仓)"
              className="h-8 text-xs"
              data-testid="overlay-new-node-title"
            />
            <Input
              value={newNodeMetricKey}
              onChange={(e) => setNewNodeMetricKey(e.target.value)}
              placeholder="初始指标名"
              className="h-8 text-xs font-mono-tech"
              data-testid="overlay-new-node-metric-key"
            />
            <Input
              type="number"
              step="any"
              value={newNodeMetricVal}
              onChange={(e) => setNewNodeMetricVal(e.target.value)}
              placeholder="指标值"
              className="h-8 text-xs font-mono-tech"
              data-testid="overlay-new-node-metric-val"
            />
            <Button type="submit" disabled={busy} className="h-8 text-xs" data-testid="overlay-new-node-submit">
              创建虚拟节点
            </Button>
            <div className="md:col-span-6 flex items-center gap-2">
              <span className="text-[11px] text-muted-foreground shrink-0">属性 JSON:</span>
              <Input
                value={newNodePropsJson}
                onChange={(e) => setNewNodePropsJson(e.target.value)}
                className="h-7 text-[11px] font-mono-tech"
              />
            </div>
          </form>
        </Card>
      )}

      {/* Add virtual link form */}
      {showAddLink && (
        <Card className="p-4 border-dashed border-xblue/50 space-y-3" data-testid="overlay-add-link-form">
          <div className="flex items-center justify-between text-xs font-medium">
            <span>新增沙盘虚拟关系 (OVERLAY · NEW)</span>
            <button
              type="button"
              onClick={() => setShowAddLink(false)}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <form onSubmit={(e) => void handleCreateLink(e)} className="grid grid-cols-1 md:grid-cols-5 gap-2.5">
            <select
              value={newLinkSrc}
              onChange={(e) => setNewLinkSrc(e.target.value)}
              className="h-8 rounded-md border border-input bg-background px-2 text-xs"
              data-testid="overlay-new-link-src"
            >
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  [{n.type_name}] {n.title}
                </option>
              ))}
            </select>
            <select
              value={newLinkDst}
              onChange={(e) => setNewLinkDst(e.target.value)}
              className="h-8 rounded-md border border-input bg-background px-2 text-xs"
              data-testid="overlay-new-link-dst"
            >
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  [{n.type_name}] {n.title}
                </option>
              ))}
            </select>
            <Input
              value={newLinkType}
              onChange={(e) => setNewLinkType(e.target.value)}
              placeholder="关系类型 (如 SUPPLIES)"
              className="h-8 text-xs font-mono-tech"
              data-testid="overlay-new-link-type"
            />
            <Input
              type="number"
              step="any"
              value={newLinkWeight}
              onChange={(e) => setNewLinkWeight(e.target.value)}
              placeholder="权重 (0.8)"
              className="h-8 text-xs font-mono-tech"
              data-testid="overlay-new-link-weight"
            />
            <Button type="submit" disabled={busy} className="h-8 text-xs" data-testid="overlay-new-link-submit">
              创建虚拟关系
            </Button>
          </form>
        </Card>
      )}

      {/* Search bar */}
      <div className="flex items-center justify-between gap-2">
        <Input
          value={searchText}
          onChange={(e) => setSearchText(e.target.value)}
          placeholder="搜索沙盘节点标题、node_key 或类型..."
          className="h-8 max-w-sm text-xs"
          data-testid="overlay-search-input"
        />
        <span className="text-[11px] font-mono-tech text-muted-foreground">
          {loading ? "加载沙盘视图中..." : `共 ${filteredNodes.length} 节点 / ${links.length} 关系`}
        </span>
      </div>

      {/* Inline node patch drawer/panel */}
      {editingNode && (
        <Card className="p-4 border-xblue space-y-3" data-testid="overlay-node-patch-panel">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-xs font-semibold">
              <Edit3 className="h-3.5 w-3.5 text-xblue" />
              <span>
                编辑沙盘节点：[{editingNode.type_name}] {editingNode.title}
              </span>
              <Badge variant="xblue">OVERLAY PATCH</Badge>
            </div>
            <button
              type="button"
              onClick={() => setEditingNode(null)}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          <form onSubmit={(e) => void handleSaveNodePatch(e)} className="space-y-3">
            <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">节点标题</label>
                <Input
                  value={editTitle}
                  onChange={(e) => setEditTitle(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-[11px] text-muted-foreground">属性 JSON (properties)</label>
                <Input
                  value={editPropsText}
                  onChange={(e) => setEditPropsText(e.target.value)}
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
            </div>

            <div className="space-y-2">
              <div className="text-[11px] text-muted-foreground">指标键值对 (metrics)</div>
              <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 gap-2">
                {editMetrics.map((m, idx) => (
                  <div key={m.key} className="flex items-center gap-1.5">
                    <span className="text-[11px] font-mono-tech w-24 truncate" title={m.key}>
                      {m.key}
                    </span>
                    <Input
                      type="number"
                      step="any"
                      value={m.value}
                      onChange={(e) =>
                        setEditMetrics((prev) =>
                          prev.map((row, i) =>
                            i === idx ? { ...row, value: e.target.value } : row
                          )
                        )
                      }
                      className="h-7 text-xs font-mono-tech"
                      data-testid={`overlay-patch-metric-${m.key}`}
                    />
                  </div>
                ))}
                <div className="flex items-center gap-1.5">
                  <Input
                    value={newMetricKey}
                    onChange={(e) => setNewMetricKey(e.target.value)}
                    placeholder="新增指标名"
                    className="h-7 text-xs font-mono-tech"
                  />
                  <Input
                    type="number"
                    step="any"
                    value={newMetricValue}
                    onChange={(e) => setNewMetricValue(e.target.value)}
                    placeholder="数值"
                    className="h-7 w-20 text-xs font-mono-tech"
                  />
                </div>
              </div>
            </div>

            <div className="flex justify-end gap-2">
              <Button type="button" variant="outline" className="h-8 text-xs" onClick={() => setEditingNode(null)}>
                取消
              </Button>
              <Button type="submit" disabled={busy} className="h-8 text-xs" data-testid="overlay-patch-save-btn">
                保存覆盖层修改
              </Button>
            </div>
          </form>
        </Card>
      )}

      {viewMode === "graph" ? (
        <Card className="p-3 space-y-2">
          <div className="flex items-center justify-between text-[11px] text-muted-foreground">
            <span>虚线边框/虚线连线表示覆盖层修改或新建元素；点击节点可直接编辑指标</span>
            <Badge variant="outline">ECharts Force Graph</Badge>
          </div>
          <div
            ref={chartContainerRef}
            className="h-[380px] w-full rounded border border-border bg-background"
            data-testid="overlay-echarts-graph"
          />
        </Card>
      ) : (
        <div className="space-y-4">
          {/* Nodes table */}
          <Card className="p-4 space-y-2">
            <div className="text-xs font-semibold">沙盘节点清单（Base + Overlay 合并视图）</div>
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-border text-left text-muted-foreground">
                    <th className="py-2 pr-2">状态</th>
                    <th className="py-2 pr-2">节点名称</th>
                    <th className="py-2 pr-2">类型 / Key</th>
                    <th className="py-2 pr-2">指标 (metrics)</th>
                    <th className="py-2 text-right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredNodes.map((n) => {
                    const isOverlay = n.is_new || n.is_modified;
                    return (
                      <tr
                        key={n.id}
                        data-testid={`overlay-node-row-${n.node_key}`}
                        className={`border-b border-border/60 ${
                          isOverlay ? "bg-xblue-soft/30" : ""
                        }`}
                      >
                        <td className="py-2 pr-2">
                          {n.is_new ? (
                            <Badge variant="xblue" className="border-dashed">
                              OVERLAY · NEW
                            </Badge>
                          ) : n.is_modified ? (
                            <Badge variant="xblue" className="border-dashed">
                              OVERLAY · PATCHED
                            </Badge>
                          ) : (
                            <Badge variant="neutral">BASE</Badge>
                          )}
                        </td>
                        <td className="py-2 pr-2 font-medium">{n.title}</td>
                        <td className="py-2 pr-2 font-mono-tech text-muted-foreground">
                          [{n.type_name}] {n.node_key}
                        </td>
                        <td className="py-2 pr-2 font-mono-tech">
                          {Object.entries(n.metrics ?? {}).length > 0
                            ? Object.entries(n.metrics)
                                .map(([k, v]) => `${k}=${v}`)
                                .join(" · ")
                            : "—"}
                        </td>
                        <td className="py-2 text-right space-x-1">
                          <Button
                            type="button"
                            variant="outline"
                            className="h-7 px-2 text-[11px]"
                            onClick={() => startEditNode(n)}
                            data-testid={`overlay-edit-node-${n.node_key}`}
                          >
                            <Edit3 className="h-3 w-3" />
                            编辑
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            className="h-7 px-2 text-[11px] text-red-600 dark:text-red-400"
                            onClick={() => void handleDeleteNode(n.id)}
                            disabled={busy}
                            data-testid={`overlay-delete-node-${n.node_key}`}
                          >
                            <Trash2 className="h-3 w-3" />
                          </Button>
                        </td>
                      </tr>
                    );
                  })}
                  {filteredNodes.length === 0 && (
                    <tr>
                      <td colSpan={5} className="py-6 text-center text-muted-foreground">
                        暂无沙盘节点
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          </Card>

          {/* Links table */}
          <Card className="p-4 space-y-2">
            <div className="text-xs font-semibold">沙盘关系清单</div>
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-border text-left text-muted-foreground">
                    <th className="py-2 pr-2">状态</th>
                    <th className="py-2 pr-2">源节点</th>
                    <th className="py-2 pr-2">关系类型</th>
                    <th className="py-2 pr-2">目标节点</th>
                    <th className="py-2 pr-2">传导权重</th>
                    <th className="py-2 text-right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {links.map((lk) => {
                    const isOverlay = lk.is_new || lk.is_modified;
                    const isEditing = editingLinkId === lk.id;
                    return (
                      <tr
                        key={lk.id}
                        className={`border-b border-border/60 ${
                          isOverlay ? "bg-xblue-soft/30" : ""
                        }`}
                      >
                        <td className="py-2 pr-2">
                          {lk.is_new ? (
                            <Badge variant="xblue" className="border-dashed">
                              OVERLAY · NEW
                            </Badge>
                          ) : lk.is_modified ? (
                            <Badge variant="xblue" className="border-dashed">
                              OVERLAY · PATCHED
                            </Badge>
                          ) : (
                            <Badge variant="neutral">BASE</Badge>
                          )}
                        </td>
                        <td className="py-2 pr-2">
                          {nodeTitleMap.get(lk.src_node_id) ?? lk.src_node_id}
                        </td>
                        <td className="py-2 pr-2 font-mono-tech">{lk.link_type}</td>
                        <td className="py-2 pr-2">
                          {nodeTitleMap.get(lk.dst_node_id) ?? lk.dst_node_id}
                        </td>
                        <td className="py-2 pr-2 font-mono-tech">
                          {isEditing ? (
                            <div className="flex items-center gap-1">
                              <Input
                                type="number"
                                step="any"
                                value={editLinkWeight}
                                onChange={(e) => setEditLinkWeight(e.target.value)}
                                className="h-7 w-20 text-xs font-mono-tech"
                              />
                              <Button
                                type="button"
                                variant="outline"
                                className="h-7 px-1.5"
                                onClick={() => void handleSaveLinkWeight(lk.id)}
                              >
                                <Check className="h-3 w-3" />
                              </Button>
                              <Button
                                type="button"
                                variant="ghost"
                                className="h-7 px-1.5"
                                onClick={() => setEditingLinkId(null)}
                              >
                                <X className="h-3 w-3" />
                              </Button>
                            </div>
                          ) : (
                            lk.weight
                          )}
                        </td>
                        <td className="py-2 text-right space-x-1">
                          <Button
                            type="button"
                            variant="outline"
                            className="h-7 px-2 text-[11px]"
                            onClick={() => {
                              setEditingLinkId(lk.id);
                              setEditLinkWeight(String(lk.weight));
                            }}
                          >
                            调权
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            className="h-7 px-2 text-[11px] text-red-600 dark:text-red-400"
                            onClick={() => void handleDeleteLink(lk.id)}
                            disabled={busy}
                          >
                            <Trash2 className="h-3 w-3" />
                          </Button>
                        </td>
                      </tr>
                    );
                  })}
                  {links.length === 0 && (
                    <tr>
                      <td colSpan={6} className="py-5 text-center text-muted-foreground">
                        暂无关系连线
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
