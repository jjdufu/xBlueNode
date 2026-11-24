import { useMemo, useRef, useState } from "react";
import {
  Eye,
  Info,
  Maximize2,
  Route,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import type {
  AxSimAction,
  ImpactRow,
  SandboxGraphLink,
  SandboxGraphNode,
} from "@/lib/api/axsim";
import { Badge, Button, Card, IconButton } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";

export interface ImpactGraphProps {
  nodes: SandboxGraphNode[];
  links: SandboxGraphLink[];
  impacts: ImpactRow[];
  actions: AxSimAction[];
  highlightedPath: string[];
  checkpointChanges?: Record<string, Record<string, number>> | null;
  activeStepLabel?: string | null;
  selectedNodeId?: string | null;
  onSelectNode?: (nodeId: string | null) => void;
  onClearPath?: () => void;
}

interface PositionedNode {
  node: SandboxGraphNode;
  x: number;
  y: number;
  layer: number;
  acted: boolean;
  primaryImpact: ImpactRow | null;
  stepDeltaEntries: Array<[string, number]>;
}

const MAX_ROWS_PER_COL = 8;
const DEFAULT_IDLE_NODE_CAP = 24;

export function ImpactGraph({
  nodes,
  links,
  impacts,
  actions,
  highlightedPath,
  checkpointChanges = null,
  activeStepLabel = null,
  selectedNodeId = null,
  onSelectNode,
  onClearPath,
}: ImpactGraphProps) {
  const { tx, fmtNum } = useI18n();
  const { isDark } = useTheme();

  const [internalSelectedId, setInternalSelectedId] = useState<string | null>(
    null
  );
  const [showAllNodes, setShowAllNodes] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState<{ x: number; y: number }>({ x: 0, y: 0 });

  const dragStateRef = useRef<{
    active: boolean;
    startX: number;
    startY: number;
    originPanX: number;
    originPanY: number;
    moved: boolean;
  }>({
    active: false,
    startX: 0,
    startY: 0,
    originPanX: 0,
    originPanY: 0,
    moved: false,
  });

  const activeSelectedId =
    selectedNodeId !== undefined ? selectedNodeId : internalSelectedId;

  const setSelected = (id: string | null) => {
    setInternalSelectedId(id);
    onSelectNode?.(id);
  };

  const resetViewport = () => {
    setZoom(1);
    setPan({ x: 0, y: 0 });
  };

  const actedNodeIds = useMemo(
    () => new Set(actions.map((a) => a.target_node_id).filter(Boolean)),
    [actions]
  );

  // Dominant impact rows per node
  const impactByNode = useMemo(() => {
    const map = new Map<string, ImpactRow[]>();
    for (const row of impacts) {
      const list = map.get(row.node_id) ?? [];
      list.push(row);
      map.set(row.node_id, list);
    }
    return map;
  }, [impacts]);

  // Filter nodes to avoid scaling down 100+ unaffected baseline nodes into a 1px speck
  const { visibleNodes, visibleLinks, hiddenCount } = useMemo(() => {
    if (nodes.length === 0) {
      return {
        visibleNodes: [] as SandboxGraphNode[],
        visibleLinks: [] as SandboxGraphLink[],
        hiddenCount: 0,
      };
    }

    if (showAllNodes) {
      const allIds = new Set(nodes.map((n) => n.id));
      return {
        visibleNodes: nodes,
        visibleLinks: links.filter(
          (l) => allIds.has(l.src_node_id) && allIds.has(l.dst_node_id)
        ),
        hiddenCount: 0,
      };
    }

    const fwdAdj = new Map<string, string[]>();
    const revAdj = new Map<string, string[]>();
    for (const lk of links) {
      const f = fwdAdj.get(lk.src_node_id) ?? [];
      f.push(lk.dst_node_id);
      fwdAdj.set(lk.src_node_id, f);

      const r = revAdj.get(lk.dst_node_id) ?? [];
      r.push(lk.src_node_id);
      revAdj.set(lk.dst_node_id, r);
    }

    const keepIds = new Set<string>();

    // (a) Intervention origins
    for (const id of actedNodeIds) {
      keepIds.add(id);
    }
    // (b) Nodes present in impactByNode or active checkpoint changes
    for (const id of impactByNode.keys()) {
      keepIds.add(id);
    }
    for (const imp of impacts) {
      for (const pid of imp.top_path ?? []) {
        keepIds.add(pid);
      }
    }
    if (checkpointChanges) {
      for (const id of Object.keys(checkpointChanges)) {
        keepIds.add(id);
      }
    }
    // (c) Highlighted path & currently selected node
    for (const id of highlightedPath) {
      keepIds.add(id);
    }
    if (activeSelectedId) {
      keepIds.add(activeSelectedId);
    }

    if (impacts.length > 0 || (checkpointChanges && Object.keys(checkpointChanges).length > 0)) {
      // (d) Intermediate bridge nodes along causal paths from actedNodeIds -> impacted/highlighted nodes
      const reachableFromOrigins = new Map<string, number>();
      const fwdQueue: string[] = [];
      for (const id of actedNodeIds) {
        reachableFromOrigins.set(id, 0);
        fwdQueue.push(id);
      }
      while (fwdQueue.length > 0) {
        const curr = fwdQueue.shift();
        if (!curr) continue;
        const d = reachableFromOrigins.get(curr) ?? 0;
        if (d >= 6) continue;
        for (const nxt of fwdAdj.get(curr) ?? []) {
          if (!reachableFromOrigins.has(nxt)) {
            reachableFromOrigins.set(nxt, d + 1);
            fwdQueue.push(nxt);
          }
        }
      }

      const canReachTargets = new Map<string, number>();
      const revQueue: string[] = [];
      for (const id of keepIds) {
        canReachTargets.set(id, 0);
        revQueue.push(id);
      }
      while (revQueue.length > 0) {
        const curr = revQueue.shift();
        if (!curr) continue;
        const d = canReachTargets.get(curr) ?? 0;
        if (d >= 6) continue;
        for (const prev of revAdj.get(curr) ?? []) {
          if (!canReachTargets.has(prev)) {
            canReachTargets.set(prev, d + 1);
            revQueue.push(prev);
          }
        }
      }

      for (const id of reachableFromOrigins.keys()) {
        if (canReachTargets.has(id)) {
          keepIds.add(id);
        }
      }
    } else {
      // Idle state (impacts.length === 0): show actedNodeIds + 1-hop neighbors + overlay edits, capped
      const seeds =
        actedNodeIds.size > 0
          ? Array.from(actedNodeIds)
          : nodes[0]
            ? [nodes[0].id]
            : [];
      for (const sid of seeds) {
        keepIds.add(sid);
        for (const nxt of fwdAdj.get(sid) ?? []) {
          if (keepIds.size < DEFAULT_IDLE_NODE_CAP) keepIds.add(nxt);
        }
        for (const prev of revAdj.get(sid) ?? []) {
          if (keepIds.size < DEFAULT_IDLE_NODE_CAP) keepIds.add(prev);
        }
      }
      for (const n of nodes) {
        if ((n.is_new || n.is_modified) && keepIds.size < DEFAULT_IDLE_NODE_CAP) {
          keepIds.add(n.id);
        }
      }
      if (keepIds.size < Math.min(8, nodes.length)) {
        for (const n of nodes) {
          if (keepIds.size >= Math.min(12, DEFAULT_IDLE_NODE_CAP)) break;
          keepIds.add(n.id);
        }
      }
    }

    const filteredNodes = nodes.filter((n) => keepIds.has(n.id));
    const finalNodes =
      filteredNodes.length > 0
        ? filteredNodes
        : nodes.slice(0, Math.min(DEFAULT_IDLE_NODE_CAP, nodes.length));
    const visibleIdSet = new Set(finalNodes.map((n) => n.id));
    const filteredLinks = links.filter(
      (l) => visibleIdSet.has(l.src_node_id) && visibleIdSet.has(l.dst_node_id)
    );

    return {
      visibleNodes: finalNodes,
      visibleLinks: filteredLinks,
      hiddenCount: Math.max(0, nodes.length - finalNodes.length),
    };
  }, [
    actedNodeIds,
    activeSelectedId,
    checkpointChanges,
    highlightedPath,
    impactByNode,
    impacts,
    links,
    nodes,
    showAllNodes,
  ]);

  // Layered layout with per-column vertical cap (MAX_ROWS_PER_COL = 8)
  const { positionedNodes, svgWidth, svgHeight } = useMemo(() => {
    if (visibleNodes.length === 0) {
      return {
        positionedNodes: [] as PositionedNode[],
        svgWidth: 720,
        svgHeight: 320,
      };
    }

    const visibleSet = new Set(visibleNodes.map((n) => n.id));
    const hopMap = new Map<string, number>();
    for (const a of actions) {
      if (visibleSet.has(a.target_node_id)) {
        hopMap.set(a.target_node_id, 0);
      }
    }
    for (const row of impacts) {
      if (!visibleSet.has(row.node_id)) continue;
      const prev = hopMap.get(row.node_id);
      if (prev === undefined || row.hop_distance < prev) {
        hopMap.set(row.node_id, row.hop_distance);
      }
    }

    const adj = new Map<string, string[]>();
    for (const lk of visibleLinks) {
      const list = adj.get(lk.src_node_id) ?? [];
      list.push(lk.dst_node_id);
      adj.set(lk.src_node_id, list);
    }

    const queue: string[] = Array.from(hopMap.keys());
    if (queue.length === 0 && visibleNodes[0]) {
      hopMap.set(visibleNodes[0].id, 0);
      queue.push(visibleNodes[0].id);
    }
    while (queue.length > 0) {
      const curr = queue.shift();
      if (!curr) continue;
      const currHop = hopMap.get(curr) ?? 0;
      for (const nxt of adj.get(curr) ?? []) {
        if (!hopMap.has(nxt)) {
          hopMap.set(nxt, currHop + 1);
          queue.push(nxt);
        }
      }
    }

    const maxAssigned = Array.from(hopMap.values()).reduce(
      (acc, v) => Math.max(acc, v),
      0
    );

    const layers = new Map<number, SandboxGraphNode[]>();
    for (const n of visibleNodes) {
      const layer = hopMap.get(n.id) ?? maxAssigned + 1;
      const bucket = layers.get(layer) ?? [];
      bucket.push(n);
      layers.set(layer, bucket);
    }

    const sortedLayers = Array.from(layers.keys()).sort((a, b) => a - b);

    // Split oversized layer buckets into wrapped sub-columns (max 8 rows/col)
    const visualColumns: Array<{ layer: number; items: SandboxGraphNode[] }> = [];
    for (const layerIdx of sortedLayers) {
      const bucket = layers.get(layerIdx) ?? [];
      for (let i = 0; i < bucket.length; i += MAX_ROWS_PER_COL) {
        visualColumns.push({
          layer: layerIdx,
          items: bucket.slice(i, i + MAX_ROWS_PER_COL),
        });
      }
    }

    const maxColRows = visualColumns.reduce(
      (acc, col) => Math.max(acc, col.items.length),
      1
    );

    const colWidth = 205;
    const rowHeight = 84;
    const padX = 110;
    const padY = 58;
    const width = Math.max(
      700,
      padX * 2 + Math.max(0, visualColumns.length - 1) * colWidth
    );
    const height = Math.max(
      320,
      padY * 2 + Math.max(0, maxColRows - 1) * rowHeight
    );

    const positioned: PositionedNode[] = [];
    visualColumns.forEach((col, colIdx) => {
      const totalColHeight = (col.items.length - 1) * rowHeight;
      const startY = height / 2 - totalColHeight / 2;

      col.items.forEach((n, idxInCol) => {
        const nodeImpacts = impactByNode.get(n.id) ?? [];
        const primary =
          nodeImpacts.length > 0
            ? [...nodeImpacts].sort(
                (a, b) =>
                  Math.abs(b.delta_ratio ?? b.delta ?? 0) -
                  Math.abs(a.delta_ratio ?? a.delta ?? 0)
              )[0]
            : null;
        const ckpt = checkpointChanges?.[n.id];
        const stepEntries = ckpt ? Object.entries(ckpt) : [];

        positioned.push({
          node: n,
          x: padX + colIdx * colWidth,
          y: startY + idxInCol * rowHeight,
          layer: col.layer,
          acted: actedNodeIds.has(n.id),
          primaryImpact: primary,
          stepDeltaEntries: stepEntries,
        });
      });
    });

    return { positionedNodes: positioned, svgWidth: width, svgHeight: height };
  }, [
    actedNodeIds,
    actions,
    checkpointChanges,
    impactByNode,
    impacts,
    visibleLinks,
    visibleNodes,
  ]);

  const posMap = useMemo(() => {
    const map = new Map<string, PositionedNode>();
    for (const p of positionedNodes) {
      map.set(p.node.id, p);
    }
    return map;
  }, [positionedNodes]);

  const highlightedEdgePairs = useMemo(() => {
    const pairs = new Set<string>();
    for (let i = 0; i < highlightedPath.length - 1; i += 1) {
      pairs.add(`${highlightedPath[i]}->${highlightedPath[i + 1]}`);
    }
    return pairs;
  }, [highlightedPath]);

  const highlightedNodeSet = useMemo(
    () => new Set(highlightedPath),
    [highlightedPath]
  );

  const selectedNode = useMemo(
    () => nodes.find((n) => n.id === activeSelectedId) ?? null,
    [activeSelectedId, nodes]
  );

  const selectedImpacts = useMemo(
    () => (selectedNode ? impactByNode.get(selectedNode.id) ?? [] : []),
    [impactByNode, selectedNode]
  );

  const vbWidth = svgWidth / zoom;
  const vbHeight = svgHeight / zoom;
  const vbX = (svgWidth - vbWidth) / 2 - pan.x;
  const vbY = (svgHeight - vbHeight) / 2 - pan.y;

  const baseEdgeStroke = isDark ? "#71717a" : "#94a3b8";
  const accentStroke = isDark ? "#3b82f6" : "#2563eb";
  const neutralStroke = isDark ? "#a1a1aa" : "#71717a";

  return (
    <Card className="p-4 space-y-3" data-testid="impact-graph-card">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-sm font-medium">
            {tx("因果传导拓扑图", "Causal Propagation Graph")}
          </h3>
          <Badge variant="neutral">
            {tx(
              `${visibleNodes.length} / ${nodes.length} 节点`,
              `${visibleNodes.length} / ${nodes.length} nodes`
            )}
          </Badge>
          {activeStepLabel && (
            <Badge variant="xblue">
              {tx(`检查点: ${activeStepLabel}`, `Step: ${activeStepLabel}`)}
            </Badge>
          )}
          {highlightedPath.length > 0 && (
            <div className="inline-flex items-center gap-1.5 rounded border border-xblue/40 bg-xblue-soft px-2 py-0.5 text-[11px] text-xblue font-mono-tech">
              <Route className="h-3 w-3" />
              <span>
                {tx(
                  `已高亮传导路径 (${highlightedPath.length} 节点)`,
                  `Highlighted path (${highlightedPath.length} nodes)`
                )}
              </span>
              <button
                type="button"
                onClick={onClearPath}
                className="hover:opacity-75 cursor-pointer"
                aria-label={tx("清除路径", "Clear path")}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {(hiddenCount > 0 || showAllNodes) && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => {
                setShowAllNodes((v) => !v);
                resetViewport();
              }}
              data-testid="impact-graph-toggle-all-btn"
            >
              <Eye className="h-3.5 w-3.5 text-xblue" />
              {showAllNodes
                ? tx("仅看关联节点", "Show affected only")
                : tx("显示全部基线节点", "Show unaffected nodes")}
            </Button>
          )}

          <div className="inline-flex items-center gap-0.5 rounded-md border border-border bg-background p-0.5">
            <IconButton
              size="sm"
              onClick={() => setZoom((z) => Math.min(3, Number((z * 1.25).toFixed(2))))}
              title={tx("放大", "Zoom in")}
              aria-label={tx("放大", "Zoom in")}
              data-testid="impact-graph-zoom-in"
            >
              <ZoomIn className="h-3.5 w-3.5" />
            </IconButton>
            <IconButton
              size="sm"
              onClick={() => setZoom((z) => Math.max(0.5, Number((z / 1.25).toFixed(2))))}
              title={tx("缩小", "Zoom out")}
              aria-label={tx("缩小", "Zoom out")}
              data-testid="impact-graph-zoom-out"
            >
              <ZoomOut className="h-3.5 w-3.5" />
            </IconButton>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-[11px]"
              onClick={resetViewport}
              data-testid="impact-graph-fit-btn"
            >
              <Maximize2 className="h-3 w-3" />
              {tx("适应画布", "Fit")}
            </Button>
          </div>
        </div>
      </div>

      {/* Compact Legend */}
      <div className="flex flex-wrap items-center gap-3 text-[11px] text-muted-foreground">
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full border-2 border-xblue bg-xblue-soft inline-block" />
          {tx("干预起点", "Intervention")}
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-red-500/80 inline-block" />
          {tx("负向冲击", "Negative Delta")}
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="h-2.5 w-2.5 rounded-full bg-emerald-500/80 inline-block" />
          {tx("正向增益", "Positive Delta")}
        </span>
        <span className="inline-flex items-center gap-1">
          <span className="w-4 border-t-2 border-dashed border-xblue inline-block" />
          {tx("覆盖层变更", "Overlay Edit")}
        </span>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-12 gap-3">
        <div
          className={`${
            selectedNode ? "lg:col-span-8" : "lg:col-span-12"
          } overflow-hidden rounded-md border border-border bg-muted/15 relative`}
        >
          {positionedNodes.length === 0 ? (
            <div className="h-64 flex items-center justify-center text-xs text-muted-foreground">
              {tx("暂无沙盘拓扑节点", "No topology nodes")}
            </div>
          ) : (
            <svg
              viewBox={`${vbX} ${vbY} ${vbWidth} ${vbHeight}`}
              className="w-full h-[360px] cursor-grab active:cursor-grabbing touch-none select-none"
              data-testid="impact-graph-svg"
              onPointerDown={(e) => {
                dragStateRef.current = {
                  active: true,
                  startX: e.clientX,
                  startY: e.clientY,
                  originPanX: pan.x,
                  originPanY: pan.y,
                  moved: false,
                };
              }}
              onPointerMove={(e) => {
                const st = dragStateRef.current;
                if (!st.active) return;
                const dx = e.clientX - st.startX;
                const dy = e.clientY - st.startY;
                if (Math.hypot(dx, dy) > 4) {
                  st.moved = true;
                }
                if (st.moved) {
                  const rect = e.currentTarget.getBoundingClientRect();
                  const scaleX = rect.width > 0 ? vbWidth / rect.width : 1;
                  const scaleY = rect.height > 0 ? vbHeight / rect.height : 1;
                  setPan({
                    x: st.originPanX + dx * scaleX,
                    y: st.originPanY + dy * scaleY,
                  });
                }
              }}
              onPointerUp={() => {
                dragStateRef.current.active = false;
              }}
              onPointerLeave={() => {
                dragStateRef.current.active = false;
              }}
            >
              <defs>
                <marker
                  id="axsim-arrow"
                  viewBox="0 0 10 10"
                  refX="9"
                  refY="5"
                  markerWidth="6"
                  markerHeight="6"
                  orient="auto-start-reverse"
                >
                  <path d="M 0 1 L 10 5 L 0 9 z" fill={baseEdgeStroke} />
                </marker>
                <marker
                  id="axsim-arrow-active"
                  viewBox="0 0 10 10"
                  refX="9"
                  refY="5"
                  markerWidth="6"
                  markerHeight="6"
                  orient="auto-start-reverse"
                >
                  <path d="M 0 1 L 10 5 L 0 9 z" fill={accentStroke} />
                </marker>
              </defs>

              {/* Edges */}
              {visibleLinks.map((lk) => {
                const src = posMap.get(lk.src_node_id);
                const dst = posMap.get(lk.dst_node_id);
                if (!src || !dst) return null;

                const isHighlighted = highlightedEdgePairs.has(
                  `${lk.src_node_id}->${lk.dst_node_id}`
                );
                const isOverlay = lk.is_new || lk.is_modified;

                const dx = dst.x - src.x;
                const dy = dst.y - src.y;
                const dist = Math.hypot(dx, dy) || 1;
                const startOffset = 58;
                const endOffset = 60;
                const x1 =
                  dist > startOffset + endOffset
                    ? src.x + (dx / dist) * startOffset
                    : src.x;
                const y1 =
                  dist > startOffset + endOffset
                    ? src.y + (dy / dist) * 24
                    : src.y;
                const x2 =
                  dist > startOffset + endOffset
                    ? dst.x - (dx / dist) * endOffset
                    : dst.x;
                const y2 =
                  dist > startOffset + endOffset
                    ? dst.y - (dy / dist) * 24
                    : dst.y;

                return (
                  <g key={lk.id}>
                    <line
                      x1={x1}
                      y1={y1}
                      x2={x2}
                      y2={y2}
                      stroke={
                        isHighlighted || isOverlay ? accentStroke : baseEdgeStroke
                      }
                      strokeWidth={isHighlighted ? 2.8 : isOverlay ? 2 : 1.5}
                      strokeDasharray={isOverlay ? "5 4" : undefined}
                      strokeOpacity={
                        highlightedPath.length > 0 && !isHighlighted ? 0.28 : 0.88
                      }
                      markerEnd={
                        isHighlighted || isOverlay
                          ? "url(#axsim-arrow-active)"
                          : "url(#axsim-arrow)"
                      }
                    />
                    <text
                      x={(x1 + x2) / 2}
                      y={(y1 + y2) / 2 - 5}
                      textAnchor="middle"
                      fontSize="9.5"
                      className="fill-muted-foreground font-mono-tech select-none"
                    >
                      {lk.link_type} ({fmtNum(lk.weight, 2)})
                    </text>
                  </g>
                );
              })}

              {/* Node Cards */}
              {positionedNodes.map((pn) => {
                const n = pn.node;
                const isSelected = activeSelectedId === n.id;
                const isOnPath = highlightedNodeSet.has(n.id);
                const isOverlay = n.is_new || n.is_modified;

                let deltaVal: number | null = pn.primaryImpact?.delta ?? null;
                let badgeText = "";
                if (pn.stepDeltaEntries.length > 0) {
                  const [mKey, val] = pn.stepDeltaEntries[0];
                  const baseVal = n.metrics[mKey];
                  deltaVal =
                    typeof baseVal === "number" ? val - baseVal : val;
                  badgeText = `${mKey}: ${fmtNum(val, 2)}`;
                } else if (pn.primaryImpact) {
                  const ratio = pn.primaryImpact.delta_ratio;
                  if (ratio !== null && Number.isFinite(ratio)) {
                    badgeText = `${ratio > 0 ? "+" : ""}${(ratio * 100).toFixed(1)}%`;
                  } else if (pn.primaryImpact.delta !== null) {
                    badgeText = `${pn.primaryImpact.delta > 0 ? "+" : ""}${fmtNum(pn.primaryImpact.delta, 2)}`;
                  }
                }

                let strokeColor = neutralStroke;
                let cardFill = isDark ? "#18181b" : "#ffffff";
                if (deltaVal !== null && deltaVal < 0) {
                  strokeColor = "#ef4444";
                  cardFill = isDark ? "#2a1215" : "#fef2f2";
                } else if (deltaVal !== null && deltaVal > 0) {
                  strokeColor = "#10b981";
                  cardFill = isDark ? "#0f2922" : "#ecfdf5";
                }
                if (pn.acted || isOnPath || isSelected) {
                  strokeColor = accentStroke;
                }

                const cardW = 112;
                const cardH = 48;
                const displayTitle =
                  n.title.length > 10 ? `${n.title.slice(0, 9)}…` : n.title;

                return (
                  <g
                    key={n.id}
                    transform={`translate(${pn.x}, ${pn.y})`}
                    onClick={() => {
                      if (dragStateRef.current.moved) {
                        dragStateRef.current.moved = false;
                        return;
                      }
                      setSelected(n.id);
                    }}
                    className="cursor-pointer"
                    data-testid={`impact-graph-node-${n.id}`}
                  >
                    {(pn.acted || isOnPath || isSelected) && (
                      <rect
                        x={-cardW / 2 - 4}
                        y={-cardH / 2 - 4}
                        width={cardW + 8}
                        height={cardH + 8}
                        rx={9}
                        fill="none"
                        stroke={accentStroke}
                        strokeWidth={isSelected ? 2 : 1.4}
                        strokeOpacity={0.5}
                      />
                    )}
                    <rect
                      x={-cardW / 2}
                      y={-cardH / 2}
                      width={cardW}
                      height={cardH}
                      rx={7}
                      fill={cardFill}
                      stroke={strokeColor}
                      strokeWidth={isOnPath || pn.acted ? 2.2 : 1.5}
                      strokeDasharray={isOverlay ? "4 3" : undefined}
                    />
                    <text
                      y={-5}
                      textAnchor="middle"
                      fontSize="10.5"
                      fontWeight="600"
                      className="fill-foreground select-none"
                    >
                      {displayTitle}
                    </text>
                    <text
                      y={11}
                      textAnchor="middle"
                      fontSize="9"
                      className="fill-muted-foreground font-mono-tech select-none"
                    >
                      {n.node_key} · {n.type_name}
                    </text>
                    {badgeText && (
                      <text
                        y={-cardH / 2 - 6}
                        textAnchor="middle"
                        fontSize="10"
                        fontWeight="600"
                        fill={
                          deltaVal !== null && deltaVal < 0
                            ? "#ef4444"
                            : deltaVal !== null && deltaVal > 0
                              ? "#10b981"
                              : accentStroke
                        }
                        className="font-mono-tech select-none"
                      >
                        {pn.acted ? `DO ${badgeText}` : badgeText}
                      </text>
                    )}
                  </g>
                );
              })}
            </svg>
          )}
        </div>

        {/* Node Inspect Panel */}
        {selectedNode && (
          <div
            className="lg:col-span-4 rounded-md border border-border bg-muted/20 p-3 space-y-3"
            data-testid="impact-node-inspector"
          >
            <div className="flex items-start justify-between gap-2 border-b border-border pb-2">
              <div className="space-y-0.5">
                <div className="flex items-center gap-1.5 text-xs font-semibold">
                  <Info className="h-3.5 w-3.5 text-xblue" />
                  <span>{selectedNode.title}</span>
                </div>
                <div className="text-[11px] font-mono-tech text-muted-foreground">
                  [{selectedNode.type_name}] {selectedNode.node_key}
                </div>
              </div>
              <IconButton
                size="sm"
                onClick={() => setSelected(null)}
                aria-label={tx("关闭", "Close")}
              >
                <X className="h-3.5 w-3.5" />
              </IconButton>
            </div>

            <div className="flex flex-wrap gap-1.5">
              {actedNodeIds.has(selectedNode.id) && (
                <Badge variant="xblue">{tx("干预起点", "Intervention")}</Badge>
              )}
              {(selectedNode.is_new || selectedNode.is_modified) && (
                <Badge
                  variant="outline"
                  className="border-dashed border-xblue text-xblue"
                >
                  {tx("覆盖层", "Overlay")}
                </Badge>
              )}
            </div>

            <div className="space-y-1.5 text-xs">
              <div className="text-[11px] font-medium text-muted-foreground">
                {tx("基线指标", "Baseline Metrics")}
              </div>
              <div className="grid grid-cols-2 gap-1.5">
                {Object.entries(selectedNode.metrics ?? {}).map(([k, v]) => (
                  <div
                    key={k}
                    className="rounded border border-border bg-background px-2 py-1 font-mono-tech text-[11px]"
                  >
                    <span className="text-muted-foreground">{k}: </span>
                    <span className="font-semibold">{fmtNum(v, 4)}</span>
                  </div>
                ))}
              </div>
            </div>

            {selectedImpacts.length > 0 && (
              <div className="space-y-1.5 text-xs">
                <div className="text-[11px] font-medium text-muted-foreground">
                  {tx("推演影响指标", "Simulated Impacts")}
                </div>
                <div className="space-y-1.5">
                  {selectedImpacts.map((imp) => (
                    <div
                      key={`${imp.node_id}-${imp.metric}`}
                      className="rounded border border-border bg-background p-2 text-[11px] space-y-1"
                    >
                      <div className="flex items-center justify-between font-mono-tech">
                        <span className="font-semibold">{imp.metric}</span>
                        <span>
                          {tx(`${imp.hop_distance} 跳`, `${imp.hop_distance} hop`)}
                        </span>
                      </div>
                      <div className="flex items-center justify-between font-mono-tech text-muted-foreground">
                        <span>
                          {fmtNum(imp.before_val, 4)} →{" "}
                          <strong className="text-foreground">
                            {fmtNum(imp.after_val, 4)}
                          </strong>
                        </span>
                        <span
                          className={
                            (imp.delta ?? 0) < 0
                              ? "text-red-600 dark:text-red-400"
                              : "text-emerald-600 dark:text-emerald-400"
                          }
                        >
                          {imp.delta !== null
                            ? `${imp.delta > 0 ? "+" : ""}${fmtNum(imp.delta, 4)}`
                            : "-"}
                        </span>
                      </div>
                      {imp.p10 != null && imp.p90 != null && (
                        <div className="font-mono-tech text-[10px] text-muted-foreground">
                          P10–P90: {imp.p10.toFixed(2)} ~ {imp.p90.toFixed(2)}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </Card>
  );
}
