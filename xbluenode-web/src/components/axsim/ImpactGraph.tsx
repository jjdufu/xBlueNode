import { useMemo, useState } from "react";
import { Info, Route, X } from "lucide-react";
import type {
  AxSimAction,
  ImpactRow,
  SandboxGraphLink,
  SandboxGraphNode,
} from "@/lib/api/axsim";
import { Badge, Button, Card } from "@/components/ui/primitives";

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
  const [internalSelectedId, setInternalSelectedId] = useState<string | null>(
    null
  );
  const activeSelectedId =
    selectedNodeId !== undefined ? selectedNodeId : internalSelectedId;

  const setSelected = (id: string | null) => {
    setInternalSelectedId(id);
    onSelectNode?.(id);
  };

  const actedNodeIds = useMemo(
    () => new Set(actions.map((a) => a.target_node_id)),
    [actions]
  );

  // Pick dominant impact per node (largest |delta_ratio| or |delta|)
  const impactByNode = useMemo(() => {
    const map = new Map<string, ImpactRow[]>();
    for (const row of impacts) {
      const list = map.get(row.node_id) ?? [];
      list.push(row);
      map.set(row.node_id, list);
    }
    return map;
  }, [impacts]);

  // Compute layered coordinates (x by hop_distance or BFS from acted nodes)
  const { positionedNodes, svgWidth, svgHeight } = useMemo(() => {
    if (nodes.length === 0) {
      return { positionedNodes: [] as PositionedNode[], svgWidth: 720, svgHeight: 320 };
    }

    const hopMap = new Map<string, number>();
    for (const a of actions) {
      hopMap.set(a.target_node_id, 0);
    }
    for (const row of impacts) {
      const prev = hopMap.get(row.node_id);
      if (prev === undefined || row.hop_distance < prev) {
        hopMap.set(row.node_id, row.hop_distance);
      }
    }

    // BFS fallback for nodes without impact rows
    const adj = new Map<string, string[]>();
    for (const lk of links) {
      const list = adj.get(lk.src_node_id) ?? [];
      list.push(lk.dst_node_id);
      adj.set(lk.src_node_id, list);
    }
    const queue: string[] = Array.from(hopMap.keys());
    if (queue.length === 0 && nodes[0]) {
      hopMap.set(nodes[0].id, 0);
      queue.push(nodes[0].id);
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
    for (const n of nodes) {
      const layer = hopMap.get(n.id) ?? maxAssigned + 1;
      const bucket = layers.get(layer) ?? [];
      bucket.push(n);
      layers.set(layer, bucket);
    }

    const sortedLayers = Array.from(layers.keys()).sort((a, b) => a - b);
    const maxBucketSize = sortedLayers.reduce(
      (acc, l) => Math.max(acc, (layers.get(l) ?? []).length),
      1
    );

    const colWidth = 190;
    const rowHeight = 86;
    const padX = 100;
    const padY = 64;
    const width = Math.max(700, padX * 2 + Math.max(0, sortedLayers.length - 1) * colWidth);
    const height = Math.max(300, padY * 2 + Math.max(0, maxBucketSize - 1) * rowHeight);

    const positioned: PositionedNode[] = [];
    sortedLayers.forEach((layerIdx, colIdx) => {
      const bucket = layers.get(layerIdx) ?? [];
      const totalBucketHeight = (bucket.length - 1) * rowHeight;
      const startY = height / 2 - totalBucketHeight / 2;

      bucket.forEach((n, idxInLayer) => {
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
          y: startY + idxInLayer * rowHeight,
          layer: layerIdx,
          acted: actedNodeIds.has(n.id),
          primaryImpact: primary,
          stepDeltaEntries: stepEntries,
        });
      });
    });

    return { positionedNodes: positioned, svgWidth: width, svgHeight: height };
  }, [actedNodeIds, actions, checkpointChanges, impactByNode, impacts, links, nodes]);

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

  return (
    <Card className="p-4 space-y-3" data-testid="impact-graph-card">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-sm font-medium">因果传导拓扑图</h3>
          {activeStepLabel && (
            <Badge variant="xblue">回放检查点: {activeStepLabel}</Badge>
          )}
          {highlightedPath.length > 0 && (
            <div className="inline-flex items-center gap-1.5 rounded border border-xblue/40 bg-xblue-soft px-2 py-0.5 text-[11px] text-xblue font-mono-tech">
              <Route className="h-3 w-3" />
              <span>已高亮传导路径 ({highlightedPath.length} 节点)</span>
              <button
                type="button"
                onClick={onClearPath}
                className="hover:opacity-75 cursor-pointer"
                aria-label="清除高亮路径"
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-3 text-[11px] text-muted-foreground">
          <span className="inline-flex items-center gap-1">
            <span className="h-2.5 w-2.5 rounded-full border-2 border-xblue bg-xblue-soft inline-block" />
            干预起点 (DO)
          </span>
          <span className="inline-flex items-center gap-1">
            <span className="h-2.5 w-2.5 rounded-full bg-red-500/80 inline-block" />
            负向冲击
          </span>
          <span className="inline-flex items-center gap-1">
            <span className="h-2.5 w-2.5 rounded-full bg-emerald-500/80 inline-block" />
            正向增益
          </span>
          <span className="inline-flex items-center gap-1">
            <span className="w-4 border-t-2 border-dashed border-xblue inline-block" />
            覆盖层虚拟/修改边
          </span>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-12 gap-3">
        <div
          className={`${
            selectedNode ? "lg:col-span-8" : "lg:col-span-12"
          } overflow-x-auto rounded-md border border-border bg-muted/15`}
        >
          {positionedNodes.length === 0 ? (
            <div className="h-64 flex items-center justify-center text-xs text-muted-foreground">
              暂无沙盘拓扑节点
            </div>
          ) : (
            <svg
              viewBox={`0 0 ${svgWidth} ${svgHeight}`}
              className="w-full min-w-[640px] h-[340px]"
              data-testid="impact-graph-svg"
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
                  <path d="M 0 1 L 10 5 L 0 9 z" fill="currentColor" />
                </marker>
              </defs>

              {/* Edges */}
              {links.map((lk) => {
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
                const offset = 28;
                const x1 = src.x + (dx / dist) * offset;
                const y1 = src.y + (dy / dist) * offset;
                const x2 = dst.x - (dx / dist) * offset;
                const y2 = dst.y - (dy / dist) * offset;

                return (
                  <g key={lk.id}>
                    <line
                      x1={x1}
                      y1={y1}
                      x2={x2}
                      y2={y2}
                      stroke={
                        isHighlighted
                          ? "#2563eb"
                          : isOverlay
                            ? "#3b82f6"
                            : "#94a3b8"
                      }
                      strokeWidth={isHighlighted ? 3 : isOverlay ? 2 : 1.5}
                      strokeDasharray={isOverlay ? "5 4" : undefined}
                      strokeOpacity={
                        highlightedPath.length > 0 && !isHighlighted ? 0.3 : 0.85
                      }
                      markerEnd="url(#axsim-arrow)"
                      className={
                        isHighlighted
                          ? "text-xblue"
                          : isOverlay
                            ? "text-xblue"
                            : "text-muted-foreground"
                      }
                    />
                    <text
                      x={(x1 + x2) / 2}
                      y={(y1 + y2) / 2 - 5}
                      textAnchor="middle"
                      fontSize="9"
                      className="fill-muted-foreground font-mono-tech select-none"
                    >
                      {lk.link_type} ({lk.weight})
                    </text>
                  </g>
                );
              })}

              {/* Nodes */}
              {positionedNodes.map((pn) => {
                const n = pn.node;
                const isSelected = activeSelectedId === n.id;
                const isOnPath = highlightedNodeSet.has(n.id);
                const isOverlay = n.is_new || n.is_modified;

                // Determine delta value (prefer active checkpoint if provided)
                let deltaVal: number | null = pn.primaryImpact?.delta ?? null;
                let badgeText = "";
                if (pn.stepDeltaEntries.length > 0) {
                  const [mKey, val] = pn.stepDeltaEntries[0];
                  const baseVal = n.metrics[mKey];
                  deltaVal =
                    typeof baseVal === "number" ? val - baseVal : val;
                  badgeText = `${mKey}: ${Number.isInteger(val) ? val : val.toFixed(1)}`;
                } else if (pn.primaryImpact) {
                  const ratio = pn.primaryImpact.delta_ratio;
                  if (ratio !== null && Number.isFinite(ratio)) {
                    badgeText = `${ratio > 0 ? "+" : ""}${(ratio * 100).toFixed(1)}%`;
                  } else if (pn.primaryImpact.delta !== null) {
                    badgeText = `${pn.primaryImpact.delta > 0 ? "+" : ""}${pn.primaryImpact.delta.toFixed(1)}`;
                  }
                }

                const mag = Math.min(
                  1,
                  Math.abs(pn.primaryImpact?.delta_ratio ?? 0)
                );
                const radius = 20 + mag * 8;

                let fillClass = "fill-card";
                let strokeColor = "#71717a";
                if (deltaVal !== null && deltaVal < 0) {
                  fillClass = "fill-red-500/15";
                  strokeColor = "#ef4444";
                } else if (deltaVal !== null && deltaVal > 0) {
                  fillClass = "fill-emerald-500/15";
                  strokeColor = "#10b981";
                }
                if (pn.acted) {
                  strokeColor = "#2563eb";
                }
                if (isOnPath || isSelected) {
                  strokeColor = "#2563eb";
                }

                return (
                  <g
                    key={n.id}
                    transform={`translate(${pn.x}, ${pn.y})`}
                    onClick={() => setSelected(n.id)}
                    className="cursor-pointer"
                    data-testid={`impact-graph-node-${n.id}`}
                  >
                    {(pn.acted || isOnPath || isSelected) && (
                      <circle
                        r={radius + 5}
                        fill="none"
                        stroke="#2563eb"
                        strokeWidth={isSelected ? 2.5 : 1.5}
                        strokeOpacity={0.55}
                      />
                    )}
                    <circle
                      r={radius}
                      className={fillClass}
                      stroke={strokeColor}
                      strokeWidth={isOnPath || pn.acted ? 2.5 : 1.8}
                      strokeDasharray={isOverlay ? "4 3" : undefined}
                    />
                    <text
                      y={-2}
                      textAnchor="middle"
                      fontSize="10"
                      fontWeight="600"
                      className="fill-foreground select-none"
                    >
                      {n.node_key}
                    </text>
                    <text
                      y={10}
                      textAnchor="middle"
                      fontSize="8.5"
                      className="fill-muted-foreground font-mono-tech select-none"
                    >
                      {n.type_name}
                    </text>
                    <text
                      y={radius + 14}
                      textAnchor="middle"
                      fontSize="10"
                      className="fill-foreground font-medium select-none"
                    >
                      {n.title}
                    </text>
                    {badgeText && (
                      <text
                        y={-radius - 7}
                        textAnchor="middle"
                        fontSize="10"
                        fontWeight="600"
                        fill={
                          deltaVal !== null && deltaVal < 0
                            ? "#ef4444"
                            : deltaVal !== null && deltaVal > 0
                              ? "#10b981"
                              : "#2563eb"
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

        {/* Node Inspect Drawer */}
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
              <Button
                type="button"
                variant="ghost"
                className="h-6 px-1.5"
                onClick={() => setSelected(null)}
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            </div>

            <div className="flex flex-wrap gap-1.5">
              {actedNodeIds.has(selectedNode.id) && (
                <Badge variant="xblue">干预起点 (DO)</Badge>
              )}
              {(selectedNode.is_new || selectedNode.is_modified) && (
                <Badge variant="outline" className="border-dashed border-xblue text-xblue">
                  OVERLAY
                </Badge>
              )}
            </div>

            <div className="space-y-1.5 text-xs">
              <div className="text-[11px] font-medium text-muted-foreground">
                基线指标 (Base + Overlay)
              </div>
              <div className="grid grid-cols-2 gap-1.5">
                {Object.entries(selectedNode.metrics ?? {}).map(([k, v]) => (
                  <div
                    key={k}
                    className="rounded border border-border bg-background px-2 py-1 font-mono-tech text-[11px]"
                  >
                    <span className="text-muted-foreground">{k}: </span>
                    <span className="font-semibold">{v}</span>
                  </div>
                ))}
              </div>
            </div>

            {selectedImpacts.length > 0 && (
              <div className="space-y-1.5 text-xs">
                <div className="text-[11px] font-medium text-muted-foreground">
                  推演影响指标
                </div>
                <div className="space-y-1.5">
                  {selectedImpacts.map((imp) => (
                    <div
                      key={`${imp.node_id}-${imp.metric}`}
                      className="rounded border border-border bg-background p-2 text-[11px] space-y-1"
                    >
                      <div className="flex items-center justify-between font-mono-tech">
                        <span className="font-semibold">{imp.metric}</span>
                        <span>{imp.hop_distance} hop</span>
                      </div>
                      <div className="flex items-center justify-between font-mono-tech text-muted-foreground">
                        <span>
                          {imp.before_val ?? "—"} →{" "}
                          <strong className="text-foreground">
                            {imp.after_val.toFixed(2)}
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
                            ? `${imp.delta > 0 ? "+" : ""}${imp.delta.toFixed(2)}`
                            : "—"}
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
