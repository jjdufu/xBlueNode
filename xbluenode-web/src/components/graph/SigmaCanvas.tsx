import { useEffect, useMemo, useRef, useState } from "react";
import Graph from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import Sigma, { DEFAULT_STYLES } from "sigma";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import type { AlgoTopEntry, BlueNode } from "@/lib/api/xblue";
import type { AlgoVisualMode } from "@/components/xbluegraph/AlgoPanel";

export type GraphColorMode = "TYPE" | "METRIC" | "ALGO";

export interface CanvasEdge {
  id: string;
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight: number;
}

export interface SigmaCanvasProps {
  nodes: BlueNode[];
  edges: CanvasEdge[];
  selectedNodeId: string | null;
  highlightedPathIds: string[];
  colorMode: GraphColorMode;
  colorMetricKey: string;
  algoMode: AlgoVisualMode;
  algoEntries: AlgoTopEntry[];
  fa2Running: boolean;
  isDark?: boolean;
  testId?: string;
  onSelectNode: (nodeId: string) => void;
  onExpandNode?: (nodeId: string) => void;
  onSetPathEndpoint?: (role: "from" | "to", nodeId: string) => void;
}

export const LIGHT_TYPE_PALETTE = [
  "#2563eb", // Blue (Factory)
  "#059669", // Emerald (Warehouse)
  "#d97706", // Amber (Order)
  "#7c3aed", // Violet (Supplier)
  "#db2777", // Pink (Store)
  "#0891b2", // Cyan (Shipment / DOC)
  "#ea580c", // Orange (ENTITY)
  "#4f46e5", // Indigo
] as const;

export const DARK_TYPE_PALETTE = [
  "#3b82f6", // Blue (Factory)
  "#10b981", // Emerald (Warehouse)
  "#f59e0b", // Amber (Order)
  "#8b5cf6", // Violet (Supplier)
  "#ec4899", // Pink (Store)
  "#06b6d4", // Cyan (Shipment / DOC)
  "#f97316", // Orange (ENTITY)
  "#6366f1", // Indigo
] as const;

const SEMANTIC_TYPE_SLOT: Readonly<Record<string, number>> = {
  factory: 0,
  plant: 0,
  warehouse: 1,
  storagehub: 1,
  hub: 1,
  order: 2,
  salesorder: 2,
  supplier: 3,
  vendor: 3,
  store: 4,
  customer: 4,
  shipment: 5,
  doc: 5,
  entity: 6,
  para: 7,
};

export function resolveNodeTypeColor(
  typeName: string,
  fallbackIndex: number,
  isDark: boolean
): string {
  const palette = isDark ? DARK_TYPE_PALETTE : LIGHT_TYPE_PALETTE;
  const key = typeName.trim().toLowerCase().replace(/[^a-z0-9]/g, "");
  const semanticIdx = SEMANTIC_TYPE_SLOT[key];
  if (semanticIdx !== undefined) {
    return palette[semanticIdx % palette.length] ?? palette[0];
  }
  let hash = 0;
  for (let i = 0; i < key.length; i += 1) {
    hash = (hash * 31 + key.charCodeAt(i)) >>> 0;
  }
  const slot = key.length > 0 ? hash % palette.length : fallbackIndex % palette.length;
  return palette[slot] ?? palette[0];
}

function isWebGl2Supported(): boolean {
  if (typeof document === "undefined") return false;
  try {
    const probe = document.createElement("canvas");
    return Boolean(probe.getContext("webgl2"));
  } catch {
    return false;
  }
}

interface NodeVisual {
  color: string;
  size: number;
  highlighted: boolean;
}

interface FallbackProjection {
  minX: number;
  spanX: number;
  minY: number;
  spanY: number;
  w: number;
  h: number;
  pad: number;
}

export function SigmaCanvas({
  nodes,
  edges,
  selectedNodeId,
  highlightedPathIds,
  colorMode,
  colorMetricKey,
  algoMode,
  algoEntries,
  fa2Running,
  isDark: isDarkProp,
  testId = "xbluegraph-canvas",
  onSelectNode,
  onExpandNode,
  onSetPathEndpoint,
}: SigmaCanvasProps) {
  const { tx } = useI18n();
  const { isDark: themeDark } = useTheme();
  const isDark = isDarkProp ?? themeDark;

  const containerRef = useRef<HTMLDivElement | null>(null);
  const fallbackCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const graph = useMemo(() => new Graph({ multi: true, type: "directed" }), []);
  const sigmaRef = useRef<Sigma | null>(null);
  const fa2Ref = useRef<FA2Layout | null>(null);
  const positionsRef = useRef<Map<string, { x: number; y: number }>>(new Map());
  const initialLayoutDoneRef = useRef<boolean>(false);
  const drawFallbackRef = useRef<(() => void) | null>(null);
  const projectionRef = useRef<FallbackProjection | null>(null);
  const fallbackDragRef = useRef<{
    nodeId: string;
    lockedProj: FallbackProjection;
    moved: boolean;
  } | null>(null);

  const [webglReady, setWebglReady] = useState<boolean>(() =>
    isWebGl2Supported()
  );
  const [cursorMode, setCursorMode] = useState<
    "default" | "grab" | "grabbing"
  >("default");
  const [contextMenu, setContextMenu] = useState<{
    nodeId: string;
    title: string;
    x: number;
    y: number;
  } | null>(null);

  const selectRef = useRef(onSelectNode);
  const expandRef = useRef(onExpandNode);
  const isDarkRef = useRef(isDark);
  useEffect(() => {
    selectRef.current = onSelectNode;
    expandRef.current = onExpandNode;
    isDarkRef.current = isDark;
  }, [isDark, onExpandNode, onSelectNode]);

  const visualByNodeId = useMemo(() => {
    const palette = isDark ? DARK_TYPE_PALETTE : LIGHT_TYPE_PALETTE;
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    const distinctTypes = Array.from(
      new Set(nodes.map((n) => n.type_name).filter(Boolean))
    ).sort();
    const typeIdxMap = new Map(distinctTypes.map((t, i) => [t, i]));

    const algoMap = new Map(algoEntries.map((e) => [e.node_id, e]));
    let maxScore = 0;
    for (const e of algoEntries) {
      if (e.score !== null && e.score > maxScore) maxScore = e.score;
    }

    let minMetric = Infinity;
    let maxMetric = -Infinity;
    if (colorMode === "METRIC" && colorMetricKey) {
      for (const n of nodes) {
        const val = n.metrics?.[colorMetricKey];
        if (typeof val === "number" && Number.isFinite(val)) {
          minMetric = Math.min(minMetric, val);
          maxMetric = Math.max(maxMetric, val);
        }
      }
    }

    const pathSet = new Set(highlightedPathIds);
    const map = new Map<string, NodeVisual>();

    for (const n of nodes) {
      const isSelected = n.id === selectedNodeId;
      const isInPath = pathSet.has(n.id);
      const algoEntry = algoMap.get(n.id);
      const typeIdx = typeIdxMap.get(n.type_name) ?? 0;

      let baseSize = 10;
      let color = resolveNodeTypeColor(n.type_name, typeIdx, isDark);

      if (colorMode === "METRIC" && colorMetricKey) {
        const val = n.metrics?.[colorMetricKey];
        if (typeof val === "number" && Number.isFinite(val)) {
          const span = maxMetric - minMetric;
          const ratio = span > 0 ? (val - minMetric) / span : 0.5;
          baseSize = Math.round(8 + ratio * 12);
          color =
            ratio >= 0.7 ? accentBlue : ratio >= 0.35 ? "#06b6d4" : "#71717a";
        }
      }

      if (colorMode === "ALGO" || algoMode !== "NONE") {
        if (
          algoMode === "SCORE_SIZE" &&
          algoEntry?.score !== null &&
          algoEntry?.score !== undefined &&
          maxScore > 0
        ) {
          baseSize = Math.round(8 + (algoEntry.score / maxScore) * 14);
        }
        if (
          algoMode === "COMMUNITY_COLOR" &&
          algoEntry?.community !== null &&
          algoEntry?.community !== undefined
        ) {
          color =
            palette[Math.abs(algoEntry.community) % palette.length] ??
            accentBlue;
        }
      }

      // Preserve distinct node type color on selection; highlight via size + outer halo
      const size = isSelected
        ? Math.max(baseSize + 5, 16)
        : isInPath
          ? Math.max(baseSize + 3, 14)
          : baseSize;

      map.set(n.id, {
        color,
        size,
        highlighted: isSelected || isInPath,
      });
    }
    return map;
  }, [
    algoEntries,
    algoMode,
    colorMetricKey,
    colorMode,
    highlightedPathIds,
    isDark,
    nodes,
    selectedNodeId,
  ]);

  // Sync nodes, edges, and visual attributes into the persistent Graphology instance
  useEffect(() => {
    const incomingIds = new Set(nodes.map((n) => n.id));
    const existingIds = graph.nodes();
    for (const oldId of existingIds) {
      if (!incomingIds.has(oldId)) {
        graph.dropNode(oldId);
      }
    }

    const count = Math.max(nodes.length, 1);
    let addedNewNodes = false;

    nodes.forEach((n, idx) => {
      const savedPos = positionsRef.current.get(n.id);
      const hasInGraph = graph.hasNode(n.id);
      const angle = (2 * Math.PI * idx) / count;
      const radius = count <= 1 ? 0 : 140;
      const defaultX = Math.cos(angle) * radius;
      const defaultY = Math.sin(angle) * radius;

      const x = hasInGraph
        ? Number(graph.getNodeAttribute(n.id, "x") ?? savedPos?.x ?? defaultX)
        : (savedPos?.x ?? defaultX);
      const y = hasInGraph
        ? Number(graph.getNodeAttribute(n.id, "y") ?? savedPos?.y ?? defaultY)
        : (savedPos?.y ?? defaultY);

      const vis = visualByNodeId.get(n.id) ?? {
        color: resolveNodeTypeColor(n.type_name, idx, isDark),
        size: 10,
        highlighted: false,
      };
      const label = n.node_key ? `${n.title} (${n.node_key})` : n.title;

      const nodeLabelColor = isDark ? "#fafafa" : "#09090b";

      if (hasInGraph) {
        graph.mergeNodeAttributes(n.id, {
          x,
          y,
          size: vis.size,
          color: vis.color,
          highlighted: vis.highlighted,
          label,
          labelColor: nodeLabelColor,
          nodeType: n.type_name,
        });
      } else {
        addedNewNodes = true;
        graph.addNode(n.id, {
          x,
          y,
          size: vis.size,
          color: vis.color,
          highlighted: vis.highlighted,
          label,
          labelColor: nodeLabelColor,
          nodeType: n.type_name,
        });
      }
      positionsRef.current.set(n.id, { x, y });
    });

    graph.clearEdges();

    const pathPairSet = new Set<string>();
    for (let i = 0; i < highlightedPathIds.length - 1; i += 1) {
      const a = highlightedPathIds[i];
      const b = highlightedPathIds[i + 1];
      if (a && b) {
        pathPairSet.add(`${a}->${b}`);
        pathPairSet.add(`${b}->${a}`);
      }
    }

    const edgeColorMuted = isDark ? "#52525b" : "#cbd5e1";
    const edgeLabelColor = isDark ? "#a1a1aa" : "#71717a";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    edges.forEach((e, idx) => {
      if (!graph.hasNode(e.src_node_id) || !graph.hasNode(e.dst_node_id)) return;
      const inPath = pathPairSet.has(`${e.src_node_id}->${e.dst_node_id}`);
      const edgeKey = `${e.id}-${idx}`;
      if (!graph.hasEdge(edgeKey)) {
        graph.addDirectedEdgeWithKey(edgeKey, e.src_node_id, e.dst_node_id, {
          size: inPath ? 3 : 1.4,
          color: inPath ? accentBlue : edgeColorMuted,
          label: e.link_type,
          labelColor: edgeLabelColor,
        });
      }
    });

    if (
      nodes.length > 1 &&
      nodes.length <= 2500 &&
      !initialLayoutDoneRef.current &&
      addedNewNodes
    ) {
      initialLayoutDoneRef.current = true;
      forceAtlas2.assign(graph, {
        iterations: 45,
        settings: forceAtlas2.inferSettings(graph),
      });
      graph.forEachNode((id, attrs) => {
        if (typeof attrs.x === "number" && typeof attrs.y === "number") {
          positionsRef.current.set(id, { x: attrs.x, y: attrs.y });
        }
      });
    }

    if (sigmaRef.current) {
      sigmaRef.current.refresh();
    }
    drawFallbackRef.current?.();
  }, [edges, graph, highlightedPathIds, isDark, nodes, visualByNodeId]);

  // Mount Sigma (WebGL2) once with interactive node dragging
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    if (!isWebGl2Supported()) {
      setWebglReady(false);
      return;
    }

    let sigma: Sigma;
    try {
      sigma = new Sigma(graph, container, {
        styles: {
          ...DEFAULT_STYLES,
          nodes: {
            ...DEFAULT_STYLES.nodes,
            labelColor: {
              attribute: "labelColor",
              defaultValue: isDarkRef.current ? "#fafafa" : "#09090b",
            },
          },
          edges: {
            ...DEFAULT_STYLES.edges,
            labelColor: {
              attribute: "labelColor",
              defaultValue: isDarkRef.current ? "#a1a1aa" : "#71717a",
            },
          },
        },
        settings: {
          renderLabels: true,
          renderEdgeLabels: true,
          labelDensity: 0.7,
          labelRenderedSizeThreshold: 6,
          allowInvalidContainer: true,
        },
      });
    } catch {
      setWebglReady(false);
      return;
    }

    setWebglReady(true);
    sigmaRef.current = sigma;

    let draggedNode: string | null = null;
    let isDragging = false;

    sigma.on("enterNode", () => {
      if (!isDragging) {
        setCursorMode("grab");
      }
    });

    sigma.on("leaveNode", () => {
      if (!isDragging) {
        setCursorMode("default");
      }
    });

    sigma.on("downNode", ({ node, event, preventSigmaDefault }) => {
      if (
        event.original instanceof MouseEvent &&
        event.original.button !== 0
      ) {
        return;
      }
      preventSigmaDefault();
      setContextMenu(null);
      draggedNode = node;
      isDragging = true;
      setCursorMode("grabbing");
      if (!sigma.getCustomBBox()) {
        sigma.setCustomBBox(sigma.getBBox());
      }
    });

    const captor = sigma.getMouseCaptor();

    captor.on("mousemovebody", (e) => {
      if (!isDragging || !draggedNode) return;
      const pos = sigma.viewportToGraph({ x: e.x, y: e.y });
      graph.setNodeAttribute(draggedNode, "x", pos.x);
      graph.setNodeAttribute(draggedNode, "y", pos.y);
      positionsRef.current.set(draggedNode, { x: pos.x, y: pos.y });
      e.preventSigmaDefault();
      if (e.original instanceof Event) {
        e.original.preventDefault();
        e.original.stopPropagation();
      }
    });

    const finishDrag = () => {
      if (draggedNode) {
        sigma.setCustomBBox(null);
        sigma.refresh();
      }
      draggedNode = null;
      isDragging = false;
      setCursorMode("default");
    };

    captor.on("mouseup", finishDrag);

    sigma.on("clickNode", ({ node }) => {
      setContextMenu(null);
      selectRef.current(node);
    });

    sigma.on("doubleClickNode", ({ node, preventSigmaDefault }) => {
      preventSigmaDefault();
      setContextMenu(null);
      selectRef.current(node);
      expandRef.current?.(node);
    });

    sigma.on("rightClickNode", ({ node, event, preventSigmaDefault }) => {
      preventSigmaDefault();
      const orig = event.original;
      if (orig instanceof MouseEvent) {
        orig.preventDefault();
      }
      const nTitle = String(graph.getNodeAttribute(node, "label") ?? node);
      setContextMenu({
        nodeId: node,
        title: nTitle,
        x: event.x,
        y: event.y,
      });
    });

    sigma.on("clickStage", () => {
      setContextMenu(null);
    });

    return () => {
      if (fa2Ref.current) {
        fa2Ref.current.kill();
        fa2Ref.current = null;
      }
      sigma.kill();
      sigmaRef.current = null;
    };
  }, [graph]);

  // Manage ForceAtlas2 background layout worker
  useEffect(() => {
    if (!webglReady || !sigmaRef.current) return;
    if (fa2Ref.current) {
      fa2Ref.current.kill();
      fa2Ref.current = null;
    }
    if (fa2Running && graph.order > 1) {
      try {
        const fa2 = new FA2Layout(graph, {
          settings: forceAtlas2.inferSettings(graph),
        });
        fa2.start();
        fa2Ref.current = fa2;
      } catch {
        fa2Ref.current = null;
      }
    }
    return () => {
      if (fa2Ref.current) {
        fa2Ref.current.kill();
        fa2Ref.current = null;
      }
    };
  }, [fa2Running, graph, nodes.length, webglReady]);

  const projectedNodesRef = useRef<
    { id: string; title: string; x: number; y: number; r: number }[]
  >([]);

  // 2D Canvas fallback renderer when WebGL2 is unavailable (with interactive node dragging)
  useEffect(() => {
    if (webglReady) {
      drawFallbackRef.current = null;
      return;
    }
    const canvas = fallbackCanvasRef.current;
    if (!canvas) return;

    const pathSet = new Set(highlightedPathIds);

    const draw = () => {
      const parent = canvas.parentElement;
      const w = parent ? Math.max(parent.clientWidth, 320) : 640;
      const h = parent ? Math.max(parent.clientHeight, 320) : 540;
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      ctx.fillStyle = isDark ? "#09090b" : "#ffffff";
      ctx.fillRect(0, 0, w, h);

      if (nodes.length === 0) {
        projectedNodesRef.current = [];
        projectionRef.current = null;
        return;
      }

      let proj: FallbackProjection;
      if (fallbackDragRef.current) {
        proj = {
          ...fallbackDragRef.current.lockedProj,
          w,
          h,
        };
      } else {
        let minX = Infinity;
        let maxX = -Infinity;
        let minY = Infinity;
        let maxY = -Infinity;
        graph.forEachNode((_id, attrs) => {
          const x = typeof attrs.x === "number" ? attrs.x : 0;
          const y = typeof attrs.y === "number" ? attrs.y : 0;
          minX = Math.min(minX, x);
          maxX = Math.max(maxX, x);
          minY = Math.min(minY, y);
          maxY = Math.max(maxY, y);
        });
        const pad = 64;
        proj = {
          minX,
          spanX: Math.max(maxX - minX, 1),
          minY,
          spanY: Math.max(maxY - minY, 1),
          w,
          h,
          pad,
        };
      }
      projectionRef.current = proj;

      const project = (gx: number, gy: number): [number, number] => {
        if (nodes.length === 1 && !fallbackDragRef.current) {
          return [proj.w / 2, proj.h / 2];
        }
        return [
          proj.pad +
            ((gx - proj.minX) / proj.spanX) * (proj.w - proj.pad * 2),
          proj.pad +
            ((gy - proj.minY) / proj.spanY) * (proj.h - proj.pad * 2),
        ];
      };

      // Draw edges
      graph.forEachEdge((_eid, attrs, _src, _dst, srcAttrs, dstAttrs) => {
        const [x1, y1] = project(
          Number(srcAttrs.x ?? 0),
          Number(srcAttrs.y ?? 0)
        );
        const [x2, y2] = project(
          Number(dstAttrs.x ?? 0),
          Number(dstAttrs.y ?? 0)
        );
        ctx.strokeStyle = String(attrs.color ?? "#cbd5e1");
        ctx.lineWidth = Number(attrs.size ?? 1.4);
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.lineTo(x2, y2);
        ctx.stroke();
      });

      // Draw nodes with outer selection/path halo while preserving palette color
      const nextProjected: {
        id: string;
        title: string;
        x: number;
        y: number;
        r: number;
      }[] = [];

      graph.forEachNode((id, attrs) => {
        const [px, py] = project(Number(attrs.x ?? 0), Number(attrs.y ?? 0));
        const r = Number(attrs.size ?? 10);
        const title = String(attrs.label ?? id);
        const fillColor = String(attrs.color ?? "#2563eb");
        const isSel = id === selectedNodeId;
        const inPath = pathSet.has(id);
        nextProjected.push({ id, title, x: px, y: py, r });

        if (isSel || inPath) {
          ctx.save();
          ctx.globalAlpha = 0.22;
          ctx.fillStyle = fillColor;
          ctx.beginPath();
          ctx.arc(px, py, r + 6, 0, Math.PI * 2);
          ctx.fill();
          ctx.restore();
        }

        ctx.fillStyle = fillColor;
        ctx.beginPath();
        ctx.arc(px, py, r, 0, Math.PI * 2);
        ctx.fill();

        if (isSel || inPath) {
          ctx.strokeStyle = isDark ? "#09090b" : "#ffffff";
          ctx.lineWidth = 2;
          ctx.stroke();

          ctx.strokeStyle = isSel
            ? isDark
              ? "#fafafa"
              : "#09090b"
            : isDark
              ? "#3b82f6"
              : "#2563eb";
          ctx.lineWidth = 2.5;
          ctx.beginPath();
          ctx.arc(px, py, r + 2, 0, Math.PI * 2);
          ctx.stroke();
        }

        ctx.fillStyle = isDark ? "#fafafa" : "#09090b";
        ctx.font = isSel ? "600 11px sans-serif" : "11px sans-serif";
        ctx.textAlign = "center";
        ctx.fillText(title, px, py + r + 15);
      });
      projectedNodesRef.current = nextProjected;
    };

    drawFallbackRef.current = draw;
    draw();
    const ro = new ResizeObserver(draw);
    if (canvas.parentElement) ro.observe(canvas.parentElement);
    return () => {
      drawFallbackRef.current = null;
      ro.disconnect();
    };
  }, [
    edges,
    graph,
    highlightedPathIds,
    isDark,
    nodes,
    selectedNodeId,
    visualByNodeId,
    webglReady,
  ]);

  const hitTestFallbackCanvas = (
    clientX: number,
    clientY: number
  ): {
    hit: { id: string; title: string; x: number; y: number; r: number } | null;
    localX: number;
    localY: number;
    canvasX: number;
    canvasY: number;
  } => {
    const canvas = fallbackCanvasRef.current;
    if (!canvas) {
      return { hit: null, localX: 0, localY: 0, canvasX: 0, canvasY: 0 };
    }
    const rect = canvas.getBoundingClientRect();
    const localX = clientX - rect.left;
    const localY = clientY - rect.top;
    const scaleX = rect.width > 0 ? canvas.width / rect.width : 1;
    const scaleY = rect.height > 0 ? canvas.height / rect.height : 1;
    const cx = localX * scaleX;
    const cy = localY * scaleY;

    const list = projectedNodesRef.current;
    for (let i = list.length - 1; i >= 0; i -= 1) {
      const item = list[i];
      if (!item) continue;
      const dist = Math.hypot(cx - item.x, cy - item.y);
      if (dist <= Math.max(item.r + 4, 12)) {
        return { hit: item, localX, localY, canvasX: cx, canvasY: cy };
      }
    }
    return { hit: null, localX, localY, canvasX: cx, canvasY: cy };
  };

  const cursorClass =
    cursorMode === "grabbing"
      ? "cursor-grabbing"
      : cursorMode === "grab"
        ? "cursor-grab"
        : "cursor-default";

  return (
    <div
      className={cn(
        "relative w-full h-[540px] bg-background overflow-hidden",
        cursorClass
      )}
      data-testid={testId}
      onContextMenu={(e) => e.preventDefault()}
    >
      {webglReady ? (
        <div ref={containerRef} className={cn("w-full h-full", cursorClass)} />
      ) : (
        <canvas
          ref={fallbackCanvasRef}
          className={cn("w-full h-full block", cursorClass)}
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            const { hit } = hitTestFallbackCanvas(e.clientX, e.clientY);
            if (hit && projectionRef.current) {
              setContextMenu(null);
              const lockedProj =
                nodes.length === 1
                  ? {
                      ...projectionRef.current,
                      minX: -100,
                      spanX: 200,
                      minY: -100,
                      spanY: 200,
                    }
                  : projectionRef.current;
              fallbackDragRef.current = {
                nodeId: hit.id,
                lockedProj,
                moved: false,
              };
              setCursorMode("grabbing");
            }
          }}
          onMouseMove={(e) => {
            const drag = fallbackDragRef.current;
            if (drag) {
              const { canvasX, canvasY } = hitTestFallbackCanvas(
                e.clientX,
                e.clientY
              );
              const lp = drag.lockedProj;
              const usableW = Math.max(1, lp.w - lp.pad * 2);
              const usableH = Math.max(1, lp.h - lp.pad * 2);
              const gx = lp.minX + ((canvasX - lp.pad) / usableW) * lp.spanX;
              const gy = lp.minY + ((canvasY - lp.pad) / usableH) * lp.spanY;
              drag.moved = true;
              if (graph.hasNode(drag.nodeId)) {
                graph.setNodeAttribute(drag.nodeId, "x", gx);
                graph.setNodeAttribute(drag.nodeId, "y", gy);
                positionsRef.current.set(drag.nodeId, { x: gx, y: gy });
                drawFallbackRef.current?.();
              }
              return;
            }
            const { hit } = hitTestFallbackCanvas(e.clientX, e.clientY);
            setCursorMode(hit ? "grab" : "default");
          }}
          onMouseUp={() => {
            if (fallbackDragRef.current) {
              fallbackDragRef.current = null;
              setCursorMode("grab");
            }
          }}
          onMouseLeave={() => {
            if (fallbackDragRef.current) {
              fallbackDragRef.current = null;
            }
            setCursorMode("default");
          }}
          onClick={(e) => {
            const { hit } = hitTestFallbackCanvas(e.clientX, e.clientY);
            setContextMenu(null);
            if (hit) {
              selectRef.current(hit.id);
            }
          }}
          onDoubleClick={(e) => {
            const { hit } = hitTestFallbackCanvas(e.clientX, e.clientY);
            setContextMenu(null);
            if (hit) {
              selectRef.current(hit.id);
              expandRef.current?.(hit.id);
            }
          }}
          onContextMenu={(e) => {
            e.preventDefault();
            const { hit, localX, localY } = hitTestFallbackCanvas(
              e.clientX,
              e.clientY
            );
            if (hit && (onExpandNode || onSetPathEndpoint)) {
              setContextMenu({
                nodeId: hit.id,
                title: hit.title,
                x: localX,
                y: localY,
              });
            } else {
              setContextMenu(null);
            }
          }}
        />
      )}

      {contextMenu && (onExpandNode || onSetPathEndpoint) && (
        <div
          style={{
            left: `${Math.max(8, Math.min(contextMenu.x, 420))}px`,
            top: `${Math.max(8, Math.min(contextMenu.y, 380))}px`,
          }}
          className="absolute z-20 w-48 rounded-md border border-border bg-card shadow-lg p-1.5 text-xs space-y-1"
          data-testid="graph-context-menu"
        >
          <div className="px-2 py-1 font-medium truncate border-b border-border">
            {contextMenu.title}
          </div>
          {onExpandNode && (
            <button
              type="button"
              onClick={() => {
                onExpandNode(contextMenu.nodeId);
                setContextMenu(null);
              }}
              className="w-full text-left rounded px-2 py-1 hover:bg-muted cursor-pointer"
            >
              {tx("展开 1 跳邻居", "Expand 1-Hop")}
            </button>
          )}
          {onSetPathEndpoint && (
            <>
              <button
                type="button"
                onClick={() => {
                  onSetPathEndpoint("from", contextMenu.nodeId);
                  setContextMenu(null);
                }}
                className="w-full text-left rounded px-2 py-1 hover:bg-muted cursor-pointer"
              >
                {tx("设为路径起点", "Set Path Start")}
              </button>
              <button
                type="button"
                onClick={() => {
                  onSetPathEndpoint("to", contextMenu.nodeId);
                  setContextMenu(null);
                }}
                className="w-full text-left rounded px-2 py-1 hover:bg-muted text-xblue cursor-pointer"
              >
                {tx("设为路径终点", "Set Path End")}
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
