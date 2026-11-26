import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
} from "react";
import { cn } from "@/lib/utils";
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
  isFullscreen?: boolean;
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
    isFullscreen = false,
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
  const cardByIdRef = useRef(cardById);
  cardByIdRef.current = cardById;

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

  const edgeStyles = useMemo(() => {
    const baseStroke = isDark ? "#71717a" : "#94a3b8";
    const accentStroke = isDark ? "#3b82f6" : "#2563eb";
    return links.map((lk) => {
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
        style: {
          stroke: active || isOverlay ? accentStroke : baseStroke,
          lineWidth: active ? 2.6 : isOverlay ? 2 : 1.5,
          lineDash: isOverlay ? [4, 4] : isStepWave ? [6, 4] : undefined,
          strokeOpacity: hasHighlightedPath && !isHighlighted ? 0.3 : 0.9,
        },
      };
    });
  }, [activeStepNodes, hasHighlightedPath, highlightedEdgePairs, isDark, links]);
  const edgeStylesRef = useRef(edgeStyles);
  edgeStylesRef.current = edgeStyles;

  const structuralSig = useMemo(
    () =>
      cardModels
        .map((c) => `${c.nodeId}:${Math.round(c.x)}:${Math.round(c.y)}:${c.comboId}`)
        .join("|") +
      "::" +
      links.map((l) => `${l.id}:${l.src_node_id}->${l.dst_node_id}`).join("|"),
    [cardModels, links]
  );

  const structuralG6Data = useMemo(() => {
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

    const edgeStyleMap = new Map(edgeStylesRef.current.map((e) => [e.id, e.style]));
    const baseStroke = isDark ? "#71717a" : "#94a3b8";

    const g6Edges: EdgeData[] = links.map((lk) => {
      const st = edgeStyleMap.get(lk.id);
      return {
        id: lk.id,
        source: lk.src_node_id,
        target: lk.dst_node_id,
        data: {
          label: `${lk.link_type} (${lk.weight.toFixed(2)})`,
        },
        style: {
          stroke: st?.stroke ?? baseStroke,
          lineWidth: st?.lineWidth ?? 1.5,
          lineDash: st?.lineDash,
          strokeOpacity: st?.strokeOpacity ?? 0.9,
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
    // Rebuild graph only on structural / theme change so step playback & highlight preserve user zoom
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [structuralSig, isDark]);

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
        nodes: structuralG6Data.g6Nodes,
        edges: structuralG6Data.g6Edges,
        combos: structuralG6Data.g6Combos,
      },
      node: {
        type: "react-node",
        style: {
          size: [228, 96],
          dx: -114,
          dy: -48,
          component: (datum: NodeData) => {
            const vm = cardByIdRef.current.get(String(datum.id));
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

    const renderPromise = graph.render().catch(() => {
      // Ignore if unmounted mid-render
    });
    renderReadyRef.current = renderPromise;

    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry || disposed || !graphRef.current) return;
      const nextW = Math.max(320, Math.round(entry.contentRect.width));
      const nextH = Math.max(300, Math.round(entry.contentRect.height));
      try {
        graphRef.current.resize(nextW, nextH);
      } catch {
        // Ignore resize during teardown
      }
    });
    ro.observe(el);

    return () => {
      disposed = true;
      ro.disconnect();
      if (graphRef.current === graph) {
        graphRef.current = null;
      }
      void renderPromise.finally(() => {
        try {
          graph.destroy();
        } catch {
          // Guard against @antv/g6 v5 Hull/Plugin partial-shape teardown
        }
      });
    };
  }, [structuralG6Data, isDark]);

  // Incrementally update highlight & step styles without destroying G6 instance or resetting user zoom
  useEffect(() => {
    let cancelled = false;
    const syncHighlights = async () => {
      await renderReadyRef.current;
      const g = graphRef.current;
      if (cancelled || !g) return;
      try {
        g.updateEdgeData(edgeStyles);
        await g.draw();
      } catch {
        // Ignore transient updates during teardown
      }
    };
    void syncHighlights();
    return () => {
      cancelled = true;
    };
  }, [cardModels, edgeStyles]);

  // Synchronize Combo collapse/expand & Hull plugins sequentially AFTER `graph.render()` completes
  useEffect(() => {
    let cancelled = false;
    const syncCombosAndHulls = async () => {
      await renderReadyRef.current;
      const g = graphRef.current;
      if (cancelled || !g) return;

      // Update hull plugins safely when member visibility changes
      try {
        g.setPlugins(hullPlugins);
      } catch {
        // Ignore transient G6 extension update errors during rapid toggles
      }

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
      className={cn(
        "relative w-full overflow-hidden",
        isFullscreen ? "h-full flex-1 min-h-[480px]" : "h-[480px]"
      )}
    />
  );
});

export default ImpactGraphG6Surface;
