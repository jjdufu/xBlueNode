import { useEffect, useMemo, useRef } from "react";
import {
  Blocks,
  BookOpen,
  Bot,
  Cable,
  Database,
  FileUp,
  Gauge,
  LayoutDashboard,
  Workflow,
} from "lucide-react";
import {
  Graph,
  NodeEvent,
  ensureG6ReactExtension,
  extractG6EventTargetId,
  type EdgeData,
  type GraphOptions,
  type IEvent,
  type NodeData,
} from "@/components/graph/g6Engine";
import { cn } from "@/lib/utils";
import { computeLineageClosure } from "./lineageClosure";

export interface LineageAssetNodeModel {
  readonly id: string;
  readonly kind: string;
  readonly kindLabel: string;
  readonly title: string;
  readonly rawId: string;
  readonly isCenter: boolean;
}

export interface LineageAssetEdgeModel {
  readonly id: string;
  readonly source: string;
  readonly target: string;
  readonly viaLabel: string;
}

export interface LineageGraphG6SurfaceProps {
  nodes: readonly LineageAssetNodeModel[];
  edges: readonly LineageAssetEdgeModel[];
  focusedNodeId: string | null;
  isDark: boolean;
  onSelectNode: (nodeId: string | null) => void;
  onNavigateAsset: (kind: string, rawId: string) => void;
}

function renderAssetIcon(kind: string) {
  const cls = "h-3.5 w-3.5 shrink-0";
  switch (kind) {
    case "CONNECTION":
      return <Cable className={cls} />;
    case "UPLOAD":
      return <FileUp className={cls} />;
    case "DATASET":
      return <Database className={cls} />;
    case "DOC":
      return <BookOpen className={cls} />;
    case "NODE_TYPE":
      return <Blocks className={cls} />;
    case "METRIC":
      return <Gauge className={cls} />;
    case "FLOW":
      return <Workflow className={cls} />;
    case "AGENT":
      return <Bot className={cls} />;
    case "DASHBOARD":
      return <LayoutDashboard className={cls} />;
    default:
      return <Database className={cls} />;
  }
}

/**
 * Deterministic `antv-dagre` (`LR`) lineage graph surface powered by lazy `@antv/g6` v5.
 */
export function LineageGraphG6Surface({
  nodes,
  edges,
  focusedNodeId,
  isDark,
  onSelectNode,
  onNavigateAsset,
}: LineageGraphG6SurfaceProps) {
  ensureG6ReactExtension();

  const containerRef = useRef<HTMLDivElement | null>(null);
  const graphRef = useRef<Graph | null>(null);
  const onSelectRef = useRef(onSelectNode);
  onSelectRef.current = onSelectNode;
  const onNavigateRef = useRef(onNavigateAsset);
  onNavigateRef.current = onNavigateAsset;

  const closure = useMemo(
    () =>
      focusedNodeId ? computeLineageClosure(focusedNodeId, edges) : null,
    [edges, focusedNodeId]
  );

  const nodeMap = useMemo(
    () => new Map(nodes.map((n) => [n.id, n])),
    [nodes]
  );

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const width = Math.max(320, el.clientWidth || 680);
    const height = Math.max(240, el.clientHeight || 288);

    const g6Nodes: NodeData[] = nodes.map((n) => ({
      id: n.id,
      data: {
        kind: n.kind,
        rawId: n.rawId,
      },
    }));

    const accentStroke = isDark ? "#3b82f6" : "#2563eb";
    const idleStroke = isDark ? "#71717a" : "#94a3b8";

    const g6Edges: EdgeData[] = edges.map((e) => {
      const inClosure = closure ? closure.edgeIds.has(e.id) : false;
      const dimmed = closure !== null && !inClosure;
      return {
        id: e.id,
        source: e.source,
        target: e.target,
        style: {
          stroke: inClosure ? accentStroke : idleStroke,
          lineWidth: inClosure ? 2.2 : 1.4,
          strokeOpacity: dimmed ? 0.25 : 0.9,
          endArrow: true,
          labelText: e.viaLabel || undefined,
          labelFontSize: 9.5,
          labelFill: isDark ? "#a1a1aa" : "#64748b",
        },
      };
    });

    const options: GraphOptions = {
      container: el,
      width,
      height,
      animation: false,
      autoFit: "view",
      padding: [20, 24, 20, 24],
      layout: {
        type: "antv-dagre",
        rankdir: "LR",
        nodesep: 28,
        ranksep: 96,
      },
      data: {
        nodes: g6Nodes,
        edges: g6Edges,
      },
      node: {
        type: "react-node",
        style: {
          size: [176, 52],
          dx: -88,
          dy: -26,
          component: (datum: NodeData) => {
            const item = nodeMap.get(String(datum.id));
            if (!item) return <div />;
            const inClosure = closure ? closure.nodeIds.has(item.id) : true;
            const isFocused = focusedNodeId === item.id;
            return (
              <div
                data-testid={`lineage-node-${item.id}`}
                data-closure-active={inClosure ? "true" : "false"}
                onClick={(ev) => {
                  ev.stopPropagation();
                  onSelectRef.current(
                    focusedNodeId === item.id ? null : item.id
                  );
                }}
                onDoubleClick={(ev) => {
                  ev.stopPropagation();
                  onNavigateRef.current(item.kind, item.rawId);
                }}
                style={{
                  width: "176px",
                  opacity: inClosure ? 1 : 0.32,
                }}
                className={cn(
                  "rounded-md border bg-card px-2.5 py-1.5 text-xs shadow-2xs transition-opacity select-none cursor-pointer",
                  item.isCenter || isFocused
                    ? "border-xblue ring-2 ring-xblue/30"
                    : closure && inClosure
                      ? "border-xblue/65"
                      : "border-border hover:border-foreground/40"
                )}
              >
                <div className="flex items-center gap-1.5 text-[10px] text-xblue font-medium">
                  {renderAssetIcon(item.kind)}
                  <span className="truncate">{item.kindLabel}</span>
                </div>
                <div className="truncate font-mono-tech text-[11px] font-semibold text-foreground mt-0.5">
                  {item.title}
                </div>
              </div>
            );
          },
        },
      },
      edge: {
        type: "cubic-horizontal",
      },
      behaviors: ["drag-canvas", "zoom-canvas", "drag-element"],
    };

    const graph = new Graph(options);
    graphRef.current = graph;

    graph.on(NodeEvent.CLICK, (evt: IEvent) => {
      const nid = extractG6EventTargetId(evt);
      if (nid) {
        onSelectRef.current(focusedNodeId === nid ? null : nid);
      }
    });

    void graph.render();

    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry || !graphRef.current) return;
      const nextW = Math.max(240, Math.round(entry.contentRect.width));
      const nextH = Math.max(200, Math.round(entry.contentRect.height));
      graphRef.current.resize(nextW, nextH);
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      graph.destroy();
      graphRef.current = null;
    };
  }, [closure, edges, focusedNodeId, isDark, nodeMap, nodes]);

  return (
    <div
      ref={containerRef}
      data-layout="antv-dagre-lr"
      className="h-full w-full"
    />
  );
}

export default LineageGraphG6Surface;
