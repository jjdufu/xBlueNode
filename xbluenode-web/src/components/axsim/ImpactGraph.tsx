import { Suspense, lazy, useMemo, useRef, useState } from "react";
import {
  Eye,
  Info,
  Layers,
  Loader2,
  Maximize2,
  Route,
  ShieldAlert,
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
import { cn } from "@/lib/utils";
import {
  buildSimComboGroups,
  computeHighImpactHullClusters,
  resolveNodeComboKey,
} from "./causalHull";
import type {
  ImpactGraphSurfaceHandle,
  PreparedSimCardViewModel,
} from "./ImpactGraphG6Surface";

const LazyImpactGraphG6Surface = lazy(
  () => import("./ImpactGraphG6Surface")
);

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

const MAX_ROWS_PER_COL = 6;
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
  const surfaceRef = useRef<ImpactGraphSurfaceHandle | null>(null);

  const [internalSelectedId, setInternalSelectedId] = useState<string | null>(
    null
  );
  const [showAllNodes, setShowAllNodes] = useState(false);
  const [collapsedComboIds, setCollapsedComboIds] = useState<Set<string>>(
    () => new Set()
  );

  const activeSelectedId =
    selectedNodeId !== undefined ? selectedNodeId : internalSelectedId;

  const setSelected = (id: string | null) => {
    setInternalSelectedId(id);
    onSelectNode?.(id);
  };

  const actedByNode = useMemo(() => {
    const map = new Map<string, AxSimAction>();
    for (const act of actions) {
      if (act.target_node_id && !map.has(act.target_node_id)) {
        map.set(act.target_node_id, act);
      }
    }
    return map;
  }, [actions]);

  const actedNodeIds = useMemo(
    () => new Set(actedByNode.keys()),
    [actedByNode]
  );

  const impactByNode = useMemo(() => {
    const map = new Map<string, ImpactRow[]>();
    for (const row of impacts) {
      const list = map.get(row.node_id) ?? [];
      list.push(row);
      map.set(row.node_id, list);
    }
    return map;
  }, [impacts]);

  // Filter unaffected baseline nodes by default so causal propagation stays crisp
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
    for (const id of actedNodeIds) keepIds.add(id);
    for (const id of impactByNode.keys()) keepIds.add(id);
    for (const imp of impacts) {
      for (const pid of imp.top_path ?? []) keepIds.add(pid);
    }
    if (checkpointChanges) {
      for (const id of Object.keys(checkpointChanges)) keepIds.add(id);
    }
    for (const id of highlightedPath) keepIds.add(id);
    if (activeSelectedId) keepIds.add(activeSelectedId);

    if (
      impacts.length > 0 ||
      (checkpointChanges && Object.keys(checkpointChanges).length > 0)
    ) {
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
        if (canReachTargets.has(id)) keepIds.add(id);
      }
    } else {
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
        if (
          (n.is_new || n.is_modified) &&
          keepIds.size < DEFAULT_IDLE_NODE_CAP
        ) {
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

  const activeStepNodes = useMemo(() => {
    if (!checkpointChanges || !activeStepLabel) return null;
    const set = new Set<string>(Object.keys(checkpointChanges));
    for (const id of actedNodeIds) set.add(id);
    return set;
  }, [actedNodeIds, activeStepLabel, checkpointChanges]);

  const baselineLabel = tx("基线", "Base");
  const interventionLabel = tx("干预", "Act");
  const simulatedLabel = tx("推演值", "Sim");

  // Compute deterministic hop-layered positions + 3-track card view models
  const cardModels: PreparedSimCardViewModel[] = useMemo(() => {
    if (visibleNodes.length === 0) return [];

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
    const visualColumns: Array<{ layer: number; items: SandboxGraphNode[] }> =
      [];
    for (const layerIdx of sortedLayers) {
      const bucket = layers.get(layerIdx) ?? [];
      for (let i = 0; i < bucket.length; i += MAX_ROWS_PER_COL) {
        visualColumns.push({
          layer: layerIdx,
          items: bucket.slice(i, i + MAX_ROWS_PER_COL),
        });
      }
    }

    const colWidth = 312;
    const rowHeight = 148;
    const out: PreparedSimCardViewModel[] = [];

    visualColumns.forEach((col, colIdx) => {
      const startY = -((col.items.length - 1) * rowHeight) / 2;
      col.items.forEach((n, idxInCol) => {
        const nodeImpacts = impactByNode.get(n.id) ?? [];
        const primary =
          nodeImpacts.length > 0
            ? [...nodeImpacts].sort(
                (a, b) =>
                  Math.abs(b.delta_ratio ?? b.delta ?? 0) -
                  Math.abs(a.delta_ratio ?? a.delta ?? 0)
              )[0] ?? null
            : null;
        const action = actedByNode.get(n.id) ?? null;
        const ckpt = checkpointChanges?.[n.id];
        const stepEntries = ckpt ? Object.entries(ckpt) : [];

        const firstBaseEntry = Object.entries(n.metrics ?? {})[0];
        const metricKey =
          stepEntries[0]?.[0] ??
          primary?.metric ??
          action?.metric_key ??
          firstBaseEntry?.[0] ??
          null;

        const baseNum =
          metricKey && typeof n.metrics?.[metricKey] === "number"
            ? n.metrics[metricKey]
            : (primary?.before_val ?? firstBaseEntry?.[1] ?? null);

        let simNum: number | null = baseNum;
        if (stepEntries[0]) {
          simNum = stepEntries[0][1];
        } else if (primary) {
          simNum = primary.after_val;
        }

        let interventionText = "—";
        if (action) {
          if (action.mode === "RATIO" && typeof action.value === "number") {
            interventionText = `${action.value > 0 ? "+" : ""}${(action.value * 100).toFixed(0)}%`;
          } else if (action.mode === "ADD" && typeof action.value === "number") {
            interventionText = `${action.value > 0 ? "+" : ""}${fmtNum(action.value, 1)}`;
          } else if (typeof action.value === "number") {
            interventionText = fmtNum(action.value, 1);
          }
        }

        let deltaPct: number | null = null;
        let deltaBadgeText: string | null = null;
        if (stepEntries[0] && typeof baseNum === "number" && baseNum !== 0) {
          deltaPct = ((stepEntries[0][1] - baseNum) / Math.abs(baseNum)) * 100;
          deltaBadgeText = `${deltaPct > 0 ? "+" : ""}${deltaPct.toFixed(1)}%`;
        } else if (
          primary?.delta_ratio !== null &&
          primary?.delta_ratio !== undefined &&
          Number.isFinite(primary.delta_ratio)
        ) {
          deltaPct = primary.delta_ratio * 100;
          deltaBadgeText = `${deltaPct > 0 ? "+" : ""}${deltaPct.toFixed(1)}%`;
        } else if (action?.mode === "RATIO" && typeof action.value === "number") {
          deltaPct = action.value * 100;
          deltaBadgeText = `${deltaPct > 0 ? "+" : ""}${deltaPct.toFixed(0)}%`;
        }

        const dimmed =
          activeStepNodes !== null && !activeStepNodes.has(n.id);

        out.push({
          nodeId: n.id,
          title: n.title,
          nodeKey: n.node_key,
          typeName: n.type_name,
          metricKey,
          baselineLabel,
          interventionLabel,
          simulatedLabel,
          baselineValueText:
            typeof baseNum === "number" ? fmtNum(baseNum, 1) : "—",
          interventionValueText: interventionText,
          simulatedValueText:
            typeof simNum === "number" ? fmtNum(simNum, 1) : "—",
          deltaBadgeText,
          deltaPct,
          p10: primary?.p10 ?? null,
          meanVal: simNum,
          p90: primary?.p90 ?? null,
          acted: actedNodeIds.has(n.id),
          selected: activeSelectedId === n.id,
          onPath: highlightedNodeSet.has(n.id),
          isOverlay: Boolean(n.is_new || n.is_modified),
          dimmed,
          x: 160 + colIdx * colWidth,
          y: 220 + startY + idxInCol * rowHeight,
          comboId: `combo:${resolveNodeComboKey(n)}`,
        });
      });
    });

    return out;
  }, [
    actedByNode,
    actedNodeIds,
    actions,
    activeSelectedId,
    activeStepNodes,
    baselineLabel,
    checkpointChanges,
    fmtNum,
    highlightedNodeSet,
    impactByNode,
    impacts,
    interventionLabel,
    simulatedLabel,
    visibleLinks,
    visibleNodes,
  ]);

  const comboGroups = useMemo(
    () => buildSimComboGroups(visibleNodes),
    [visibleNodes]
  );

  const hullPreviewCount = useMemo(
    () =>
      computeHighImpactHullClusters(
        visibleNodes,
        visibleLinks,
        impacts,
        actions,
        new Set(),
        0.15
      ).length,
    [actions, impacts, visibleLinks, visibleNodes]
  );

  const selectedNode = useMemo(
    () => nodes.find((n) => n.id === activeSelectedId) ?? null,
    [activeSelectedId, nodes]
  );

  const selectedImpacts = useMemo(
    () => (selectedNode ? impactByNode.get(selectedNode.id) ?? [] : []),
    [impactByNode, selectedNode]
  );

  const handleToggleComboChip = (comboId: string) => {
    setCollapsedComboIds((prev) => {
      const next = new Set(prev);
      if (next.has(comboId)) {
        next.delete(comboId);
      } else {
        next.add(comboId);
      }
      return next;
    });
  };

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
          {hullPreviewCount > 0 && (
            <Badge
              variant="danger"
              data-testid="impact-hull-badge"
              className="gap-1"
            >
              <ShieldAlert className="h-3 w-3" />
              {tx(
                `${hullPreviewCount} 风险簇`,
                `${hullPreviewCount} Risk Hull`
              )}
            </Badge>
          )}
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
              onClick={() => setShowAllNodes((v) => !v)}
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
              onClick={() => surfaceRef.current?.zoomBy(1.2)}
              title={tx("放大", "Zoom in")}
              aria-label={tx("放大", "Zoom in")}
              data-testid="impact-graph-zoom-in"
            >
              <ZoomIn className="h-3.5 w-3.5" />
            </IconButton>
            <IconButton
              size="sm"
              onClick={() => surfaceRef.current?.zoomBy(0.8)}
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
              onClick={() => surfaceRef.current?.fitView()}
              data-testid="impact-graph-fit-btn"
            >
              <Maximize2 className="h-3 w-3" />
              {tx("适应画布", "Fit")}
            </Button>
          </div>
        </div>
      </div>

      {/* Legend + Domain/Type Combo Quick Collapse Chips */}
      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <div className="flex flex-wrap items-center gap-3">
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

        {comboGroups.length > 0 && (
          <div className="flex flex-wrap items-center gap-1">
            <Layers className="h-3 w-3 text-xblue" />
            {comboGroups.map((cg) => {
              const collapsed = collapsedComboIds.has(cg.comboId);
              return (
                <button
                  key={cg.comboId}
                  type="button"
                  data-testid={`impact-combo-toggle-${cg.key}`}
                  data-collapsed={collapsed ? "true" : "false"}
                  onClick={() => handleToggleComboChip(cg.comboId)}
                  className={cn(
                    "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech transition-colors cursor-pointer",
                    collapsed
                      ? "border-xblue bg-xblue-soft text-xblue"
                      : "border-border bg-background text-foreground hover:border-xblue"
                  )}
                >
                  {cg.label} ({cg.memberIds.length})
                </button>
              );
            })}
          </div>
        )}
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-12 gap-3">
        <div
          className={`${
            selectedNode ? "lg:col-span-8" : "lg:col-span-12"
          } overflow-hidden rounded-md border border-border bg-muted/15 relative`}
        >
          {cardModels.length === 0 ? (
            <div className="h-64 flex items-center justify-center text-xs text-muted-foreground">
              {tx("暂无沙盘拓扑节点", "No topology nodes")}
            </div>
          ) : (
            <Suspense
              fallback={
                <div className="h-[420px] flex items-center justify-center gap-2 text-xs text-muted-foreground">
                  <Loader2 className="h-4 w-4 animate-spin text-xblue" />
                  <span>{tx("加载因果画布…", "Loading causal canvas…")}</span>
                </div>
              }
            >
              <LazyImpactGraphG6Surface
                ref={surfaceRef}
                nodes={visibleNodes}
                links={visibleLinks}
                impacts={impacts}
                actions={actions}
                cardModels={cardModels}
                collapsedComboIds={collapsedComboIds}
                highlightedEdgePairs={highlightedEdgePairs}
                activeStepNodes={activeStepNodes}
                hasHighlightedPath={highlightedPath.length > 0}
                isDark={isDark}
                riskHullLabel={tx("高冲击簇", "High Shock")}
                onSelectNode={(id) => setSelected(id)}
                onComboCollapsedChange={(cid, isCol) => {
                  setCollapsedComboIds((prev) => {
                    const next = new Set(prev);
                    if (isCol) {
                      next.add(cid);
                    } else {
                      next.delete(cid);
                    }
                    return next;
                  });
                }}
              />
            </Suspense>
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
