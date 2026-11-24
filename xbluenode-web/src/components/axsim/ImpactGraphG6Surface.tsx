import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
} from "react";
import type {
  AxSimAction,
  ImpactRow,
  SandboxGraphLink,
  SandboxGraphNode,
} from "@/lib/api/axsim";
import {
  ComboEvent,
  Graph,
  NodeEvent,
  ensureG6ReactExtension,
  extractG6EventTargetId,
  type ComboData,
  type EdgeData,
  type GraphOptions,
  type IEvent,
  type NodeData,
  type PluginOptions,
} from "@/components/graph/g6Engine";
import { SimNodeCardG6, type SimNodeCardG6Props } from "./SimNodeCardG6";
import {
  buildSimComboGroups,
  computeHighImpactHullClusters,
  resolveNodeComboKey,
} from "./causalHull";

export interface ImpactGraphSurfaceHandle {
  zoomBy: (ratio: number) => void;
  fitView: () => void;
}

export interface PreparedSimCardViewModel
  extends Omit<SimNodeCardG6Props, "onSelect"> {
  readonly x: number;
  readonly y: number;
  readonly comboId: string;
}

export interface ImpactGraphG6SurfaceProps {
  nodes: readonly SandboxGraphNode[];
  links: readonly SandboxGraphLink[];
  impacts: readonly ImpactRow[];
  actions: readonly AxSimAction[];
  cardModels: readonly PreparedSimCardViewModel[];
  collapsedComboIds: ReadonlySet<string>;
  highlightedEdgePairs: ReadonlySet<string>;
  activeStepNodes: ReadonlySet<string> | null;
  hasHighlightedPath: boolean;
  isDark: boolean;
  riskHullLabel: string;
  onSelectNode: (nodeId: string) => void;
  onComboCollapsedChange: (comboId: string, collapsed: boolean) => void;
}

/**
 * Lazy-loaded `@antv/g6` v5 causal propagation surface with `ReactNode` 3-track micro-cards,
 * `Combo` domain grouping, `Hull` high-impact risk contours, and wave-pulse edges.
 */
export const ImpactGraphG6Surface = forwardRef<
  ImpactGraphSurfaceHandle,
  ImpactGraphG6SurfaceProps
>(function ImpactGraphG6Surface(
  {
    nodes,
    links,
    impacts,
    actions,
    cardModels,
    collapsedComboIds,
    highlightedEdgePairs,
    activeStepNodes,
    hasHighlightedPath,
    isDark,
    riskHullLabel,
    onSelectNode,
    onComboCollapsedChange,
  },
  ref
) {
  ensureG6ReactExtension();

  const containerRef = useRef<HTMLDivElement | null>(null);
  const graphRef = useRef<Graph | null>(null);
  const renderReadyRef = useRef<Promise<void>>(Promise.resolve());
  const appliedCollapsedRef = useRef<Set<string>>(new Set());

  const onSelectRef = useRef(onSelectNode);
  onSelectRef.current = onSelectNode;
  const onComboChangeRef = useRef(onComboCollapsedChange);
  onComboChangeRef.current = onComboCollapsedChange;

  const comboGroups = useMemo(() => buildSimComboGroups(nodes), [nodes]);

  const hiddenByCollapsedCombo = useMemo(() => {
    const hidden = new Set<string>();
    for (const n of nodes) {
      const cid = `combo:${resolveNodeComboKey(n)}`;
      if (collapsedComboIds.has(cid)) {
        hidden.add(n.id);
      }
    }
    return hidden;
  }, [collapsedComboIds, nodes]);

  const hullClusters = useMemo(
    () =>
      computeHighImpactHullClusters(
        nodes,
        links,
        impacts,
        actions,
        hiddenByCollapsedCombo,
        0.15
      ),
    [actions, hiddenByCollapsedCombo, impacts, links, nodes]
  );

  const cardById = useMemo(
    () => new Map(cardModels.map((m) => [m.nodeId, m])),
    [cardModels]
  );

  useImperativeHandle(
    ref,
    () => ({
      zoomBy: (ratio: number) => {
        void graphRef.current?.zoomBy(ratio);
      },
      fitView: () => {
        void graphRef.current?.fitView();
      },
    }),
    []
  );

  const hullPlugins: PluginOptions = useMemo(
    () =>
      hullClusters.map((hc) => ({
        key: hc.key,
        type: "hull",
        members: [...hc.memberIds],
        padding: 22,
        corner: "smooth",
        fill: isDark ? "rgba(239, 68, 68, 0.13)" : "rgba(239, 68, 68, 0.08)",
        stroke: "#ef4444",
        lineWidth: 1.5,
        lineDash: [6, 4],
        labelText: `${riskHullLabel} (≥15%)`,
        labelFontSize: 10,
        labelFill: isDark ? "#fca5a5" : "#dc2626",
      })),
    [hullClusters, isDark, riskHullLabel]
  );
  const hullPluginsRef = useRef(hullPlugins);
  hullPluginsRef.current = hullPlugins;

  const baseG6Data = useMemo(() => {
    const g6Nodes: NodeData[] = cardModels.map((card) => ({
      id: card.nodeId,
      combo: card.comboId,
      style: {
        x: card.x,
        y: card.y,
      },
      data: {
        cardId: card.nodeId,
      },
    }));

    const g6Combos: ComboData[] = comboGroups.map((cg) => {
      const memberCards = cg.memberIds
        .map((id) => cardById.get(id))
        .filter((c): c is PreparedSimCardViewModel => Boolean(c));
      const cx =
        memberCards.length > 0
          ? memberCards.reduce((acc, c) => acc + c.x, 0) / memberCards.length
          : 240;
      const cy =
        memberCards.length > 0
          ? memberCards.reduce((acc, c) => acc + c.y, 0) / memberCards.length
          : 200;
      return {
        id: cg.comboId,
        data: {
          label: cg.label,
          count: cg.memberIds.length,
        },
        style: {
          x: cx,
          y: cy,
        },
      };
    });

    const baseStroke = isDark ? "#71717a" : "#94a3b8";
    const accentStroke = isDark ? "#3b82f6" : "#2563eb";

    const g6Edges: EdgeData[] = links.map((lk) => {
      const isHighlighted = highlightedEdgePairs.has(
        `${lk.src_node_id}->${lk.dst_node_id}`
      );
      const isStepWave =
        activeStepNodes !== null &&
        activeStepNodes.has(lk.src_node_id) &&
        activeStepNodes.has(lk.dst_node_id);
      const isOverlay = lk.is_new || lk.is_modified;
      const active = isHighlighted || isStepWave;

      return {
        id: lk.id,
        source: lk.src_node_id,
        target: lk.dst_node_id,
        data: {
          label: `${lk.link_type} (${lk.weight.toFixed(2)})`,
        },
        style: {
          stroke: active || isOverlay ? accentStroke : baseStroke,
          lineWidth: active ? 2.6 : isOverlay ? 2 : 1.5,
          lineDash: isOverlay ? [4, 4] : isStepWave ? [6, 4] : undefined,
          strokeOpacity: hasHighlightedPath && !isHighlighted ? 0.3 : 0.9,
          endArrow: true,
          labelText: lk.link_type,
          labelFontSize: 10,
          labelFill: isDark ? "#a1a1aa" : "#64748b",
          labelBackground: true,
          labelBackgroundFill: isDark ? "#18181b" : "#ffffff",
          labelBackgroundRadius: 3,
        },
      };
    });

    return { g6Nodes, g6Combos, g6Edges };
  }, [
    activeStepNodes,
    cardById,
    cardModels,
    comboGroups,
    hasHighlightedPath,
    highlightedEdgePairs,
    isDark,
    links,
  ]);

  // Mount & render G6 graph on structural / visual model changes (without tearing down on combo collapse)
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    let disposed = false;
    appliedCollapsedRef.current = new Set();

    const width = Math.max(360, el.clientWidth || 760);
    const height = Math.max(320, el.clientHeight || 400);

    const options: GraphOptions = {
      container: el,
      width,
      height,
      animation: false,
      autoFit: "view",
      padding: [28, 28, 28, 28],
      data: {
        nodes: baseG6Data.g6Nodes,
        edges: baseG6Data.g6Edges,
        combos: baseG6Data.g6Combos,
      },
      node: {
        type: "react-node",
        style: {
          size: [228, 96],
          dx: -114,
          dy: -48,
          component: (datum: NodeData) => {
            const vm = cardById.get(String(datum.id));
            if (!vm) {
              return <div />;
            }
            return (
              <SimNodeCardG6
                {...vm}
                onSelect={(id) => onSelectRef.current(id)}
              />
            );
          },
        },
      },
      edge: {
        type: "cubic-horizontal",
      },
      combo: {
        type: "rect",
        style: {
          radius: 8,
          padding: [26, 18, 18, 18],
          fill: isDark ? "rgba(39, 39, 42, 0.35)" : "rgba(241, 245, 249, 0.65)",
          stroke: isDark ? "#3f3f46" : "#cbd5e1",
          lineDash: [5, 3],
          lineWidth: 1.2,
          collapsedSize: [168, 52],
          labelText: (d: ComboData) => {
            const lbl = String(d.data?.label ?? d.id);
            const cnt = Number(d.data?.count ?? 0);
            return cnt > 0 ? `${lbl} (${cnt})` : lbl;
          },
          labelPlacement: "top",
          labelFontSize: 11,
          labelFill: isDark ? "#e4e4e7" : "#334155",
        },
      },
      behaviors: [
        "drag-canvas",
        "zoom-canvas",
        "drag-element",
        {
          type: "collapse-expand",
          trigger: "dblclick",
          animation: false,
          onCollapse: (id: string) => {
            const cid = String(id);
            appliedCollapsedRef.current.add(cid);
            onComboChangeRef.current(cid, true);
          },
          onExpand: (id: string) => {
            const cid = String(id);
            appliedCollapsedRef.current.delete(cid);
            onComboChangeRef.current(cid, false);
          },
        },
      ],
      plugins: hullPluginsRef.current,
    };

    const graph = new Graph(options);
    graphRef.current = graph;

    graph.on(NodeEvent.CLICK, (evt: IEvent) => {
      const tid = extractG6EventTargetId(evt);
      if (tid) {
        onSelectRef.current(tid);
      }
    });

    graph.on(ComboEvent.DBLCLICK, (evt: IEvent) => {
      const cid = extractG6EventTargetId(evt);
      if (!cid) return;
      const isCol = appliedCollapsedRef.current.has(cid);
      onComboChangeRef.current(cid, !isCol);
    });

    renderReadyRef.current = graph.render().catch(() => {
      // Ignore if unmounted mid-render
    });

    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry || disposed || !graphRef.current) return;
      const nextW = Math.max(320, Math.round(entry.contentRect.width));
      const nextH = Math.max(300, Math.round(entry.contentRect.height));
      graphRef.current.resize(nextW, nextH);
    });
    ro.observe(el);

    return () => {
      disposed = true;
      ro.disconnect();
      graph.destroy();
      if (graphRef.current === graph) {
        graphRef.current = null;
      }
    };
  }, [baseG6Data, cardById, isDark]);

  // Synchronize Combo collapse/expand & Hull plugins sequentially AFTER `graph.render()` completes
  useEffect(() => {
    let cancelled = false;
    const syncCombosAndHulls = async () => {
      await renderReadyRef.current;
      const g = graphRef.current;
      if (cancelled || !g) return;

      // Update hull plugins first so no hull references a node about to be collapsed
      g.setPlugins(hullPlugins);

      for (const cg of comboGroups) {
        if (cancelled || graphRef.current !== g) return;
        if (!g.hasCombo(cg.comboId)) continue;
        const shouldCollapse = collapsedComboIds.has(cg.comboId);
        const alreadyCollapsed = appliedCollapsedRef.current.has(cg.comboId);
        if (shouldCollapse && !alreadyCollapsed) {
          appliedCollapsedRef.current.add(cg.comboId);
          try {
            await g.collapseElement(cg.comboId, false);
          } catch {
            // Safely ignore if element is transitioning
          }
        } else if (!shouldCollapse && alreadyCollapsed) {
          appliedCollapsedRef.current.delete(cg.comboId);
          try {
            await g.expandElement(cg.comboId, false);
          } catch {
            // Safely ignore if element is transitioning
          }
        }
      }
      if (!cancelled && graphRef.current === g) {
        await g.draw().catch(() => {});
      }
    };
    void syncCombosAndHulls();
    return () => {
      cancelled = true;
    };
  }, [collapsedComboIds, comboGroups, hullPlugins]);

  return (
    <div
      ref={containerRef}
      data-testid="impact-graph-g6-canvas"
      data-combo-count={comboGroups.length}
      data-collapsed-combo-count={collapsedComboIds.size}
      data-hull-count={hullClusters.length}
      className="relative h-[420px] w-full overflow-hidden"
    />
  );
});

export default ImpactGraphG6Surface;
