import { useEffect, useMemo, useRef, useState } from "react";
import Graph from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import { Maximize2, Minimize2, RotateCcw, ZoomIn, ZoomOut } from "lucide-react";
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
  className?: string;
  onSelectNode: (nodeId: string) => void;
  onExpandNode?: (nodeId: string) => void;
  onSetPathEndpoint?: (role: "from" | "to", nodeId: string) => void;
}

export const LIGHT_TYPE_PALETTE = [
  "#2563eb", // 0: Blue (Factory)
  "#059669", // 1: Emerald (Warehouse)
  "#d97706", // 2: Amber (Order)
  "#7c3aed", // 3: Violet (Supplier / Line)
  "#e11d48", // 4: Rose (WorkOrder / Store)
  "#0891b2", // 5: Cyan (Machine / DOC)
  "#ea580c", // 6: Orange (Sensor / ENTITY)
  "#0d9488", // 7: Teal (Shipment)
  "#c026d3", // 8: Fuchsia (Product)
  "#4f46e5", // 9: Indigo (PARA)
] as const;

export const DARK_TYPE_PALETTE = [
  "#3b82f6", // 0: Blue
  "#10b981", // 1: Emerald
  "#f59e0b", // 2: Amber
  "#8b5cf6", // 3: Violet
  "#fb7185", // 4: Rose
  "#06b6d4", // 5: Cyan
  "#f97316", // 6: Orange
  "#2dd4bf", // 7: Teal
  "#e879f9", // 8: Fuchsia
  "#6366f1", // 9: Indigo
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
  line: 3,
  productionline: 3,
  store: 4,
  customer: 4,
  workorder: 4,
  machine: 5,
  equipment: 5,
  doc: 5,
  sensor: 6,
  entity: 6,
  shipment: 7,
  product: 8,
  para: 9,
};

const NODE_TYPE_ZH_MAP: Readonly<Record<string, string>> = {
  factory: "工厂",
  warehouse: "仓库",
  order: "订单",
  workorder: "生产工单",
  line: "生产产线",
  productionline: "生产产线",
  machine: "机台设备",
  sensor: "物联传感",
  supplier: "供应商",
  store: "门店客户",
  shipment: "物流运单",
  product: "物料产品",
  doc: "知识文档",
  entity: "知识实体",
  para: "正文段落",
};

const EDGE_TYPE_ZH_MAP: Readonly<Record<string, string>> = {
  SUPPLIES: "供应",
  FULFILLS: "履约",
  ASSIGNED_TO: "派工",
  HAS_LINE: "下辖产线",
  HAS_MACHINE: "配备机台",
  HAS_SENSOR: "挂载传感",
  LOCATED_IN: "位于",
  MENTIONS: "提及",
  HAS_PARA: "包含段落",
  CO_OCCURS: "同段共现",
  RELATES: "关联",
  RELATED_TO: "关联",
};

export function translateNodeTypeLabel(
  typeName: string,
  isZh: boolean
): string {
  if (!typeName) return "-";
  if (!isZh) return typeName;
  const key = typeName.trim().toLowerCase().replace(/[^a-z0-9]/g, "");
  return NODE_TYPE_ZH_MAP[key] ?? typeName;
}

export function translateEdgeTypeLabel(relType: string, isZh: boolean): string {
  if (!relType) return "";
  if (!isZh) return relType;
  return EDGE_TYPE_ZH_MAP[relType.toUpperCase()] ?? relType;
}

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
  const slot =
    key.length > 0 ? hash % palette.length : fallbackIndex % palette.length;
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
  className,
  onSelectNode,
  onExpandNode,
  onSetPathEndpoint,
}: SigmaCanvasProps) {
  const { isZh, tx } = useI18n();
  const { isDark: themeDark } = useTheme();
  const isDark = isDarkProp ?? themeDark;

  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const fallbackCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const graph = useMemo(() => new Graph({ multi: true, type: "directed" }), []);
  const sigmaRef = useRef<Sigma | null>(null);
  const fa2Ref = useRef<FA2Layout | null>(null);
  const positionsRef = useRef<Map<string, { x: number; y: number }>>(new Map());
  const prevFingerprintRef = useRef<string>("");
  const drawFallbackRef = useRef<(() => void) | null>(null);
  const projectionRef = useRef<FallbackProjection | null>(null);
  const fallbackZoomRef = useRef<number>(1);
  const fallbackDragRef = useRef<{
    nodeId: string;
    lockedProj: FallbackProjection;
    moved: boolean;
  } | null>(null);

  const [webglReady, setWebglReady] = useState<boolean>(() =>
    isWebGl2Supported()
  );
  const [isFullscreen, setIsFullscreen] = useState(false);
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

  useEffect(() => {
    const onFsChange = () => {
      setIsFullscreen(Boolean(document.fullscreenElement));
    };
    document.addEventListener("fullscreenchange", onFsChange);
    return () => document.removeEventListener("fullscreenchange", onFsChange);
  }, []);

  const neighborIdSet = useMemo(() => {
    const set = new Set<string>();
    if (!selectedNodeId) return set;
    for (const e of edges) {
      if (e.src_node_id === selectedNodeId) set.add(e.dst_node_id);
      if (e.dst_node_id === selectedNodeId) set.add(e.src_node_id);
    }
    return set;
  }, [edges, selectedNodeId]);

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
      const isNeighbor = neighborIdSet.has(n.id);
      const algoEntry = algoMap.get(n.id);
      const typeIdx = typeIdxMap.get(n.type_name) ?? 0;

      let baseSize = 6.5;
      let color = resolveNodeTypeColor(n.type_name, typeIdx, isDark);

      if (colorMode === "METRIC" && colorMetricKey) {
        const val = n.metrics?.[colorMetricKey];
        if (typeof val === "number" && Number.isFinite(val)) {
          const span = maxMetric - minMetric;
          const ratio = span > 0 ? (val - minMetric) / span : 0.5;
          baseSize = Math.round(6 + ratio * 9);
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
          baseSize = Math.round(6 + (algoEntry.score / maxScore) * 11);
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

      const size = isSelected
        ? Math.max(baseSize + 4.5, 11.5)
        : isInPath
          ? Math.max(baseSize + 3, 10)
          : isNeighbor
            ? Math.max(baseSize + 1.5, 8)
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
    neighborIdSet,
    nodes,
    selectedNodeId,
  ]);

  // Sync nodes, edges, and ForceAtlas2 well-spaced layout into Graphology
  useEffect(() => {
    const incomingIds = new Set(nodes.map((n) => n.id));
    for (const oldId of graph.nodes()) {
      if (!incomingIds.has(oldId)) {
        graph.dropNode(oldId);
      }
    }

    const goldenAngle = 2.399963229728653;
    nodes.forEach((n, idx) => {
      const savedPos = positionsRef.current.get(n.id);
      const hasInGraph = graph.hasNode(n.id);
      const radius = nodes.length <= 1 ? 0 : Math.sqrt(idx + 0.6) * 34;
      const theta = idx * goldenAngle;
      const defaultX = Math.cos(theta) * radius;
      const defaultY = Math.sin(theta) * radius;

      const x = hasInGraph
        ? Number(graph.getNodeAttribute(n.id, "x") ?? savedPos?.x ?? defaultX)
        : (savedPos?.x ?? defaultX);
      const y = hasInGraph
        ? Number(graph.getNodeAttribute(n.id, "y") ?? savedPos?.y ?? defaultY)
        : (savedPos?.y ?? defaultY);

      const vis = visualByNodeId.get(n.id) ?? {
        color: resolveNodeTypeColor(n.type_name, idx, isDark),
        size: 6.5,
        highlighted: false,
      };

      // Clean label: omit redundant suffix if title already contains node_key or if empty
      const rawTitle = n.title.trim();
      const keySuffix =
        n.node_key && !rawTitle.includes(n.node_key) ? ` (${n.node_key})` : "";
      const fullLabel = `${rawTitle}${keySuffix}`;
      const label =
        fullLabel.length > 26 && n.id !== selectedNodeId
          ? `${fullLabel.slice(0, 24)}…`
          : fullLabel;

      const nodeLabelColor = isDark ? "#f4f4f5" : "#18181b";

      const attrs = {
        x,
        y,
        size: vis.size,
        color: vis.color,
        highlighted: vis.highlighted,
        forceLabel: n.id === selectedNodeId || neighborIdSet.has(n.id),
        label,
        labelColor: nodeLabelColor,
        nodeType: n.type_name,
      };

      if (hasInGraph) {
        graph.mergeNodeAttributes(n.id, attrs);
      } else {
        graph.addNode(n.id, attrs);
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

    const edgeColorNormal = isDark ? "#71717a" : "#94a3b8";
    const edgeColorDimmed = isDark ? "#3f3f46" : "#cbd5e1";
    const edgeLabelColor = isDark ? "#a1a1aa" : "#64748b";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    edges.forEach((e, idx) => {
      if (!graph.hasNode(e.src_node_id) || !graph.hasNode(e.dst_node_id)) return;
      const inPath = pathPairSet.has(`${e.src_node_id}->${e.dst_node_id}`);
      const touchesSelected =
        Boolean(selectedNodeId) &&
        (e.src_node_id === selectedNodeId || e.dst_node_id === selectedNodeId);
      const edgeKey = `${e.id}-${idx}`;
      if (!graph.hasEdge(edgeKey)) {
        graph.addDirectedEdgeWithKey(edgeKey, e.src_node_id, e.dst_node_id, {
          size: inPath ? 3 : touchesSelected ? 2.4 : 1.5,
          color:
            inPath || touchesSelected
              ? accentBlue
              : selectedNodeId
                ? edgeColorDimmed
                : edgeColorNormal,
          label: translateEdgeTypeLabel(e.link_type, isZh),
          labelColor: edgeLabelColor,
        });
      }
    });

    const fingerprint = `${nodes.length}:${edges.length}:${nodes[0]?.id ?? ""}`;
    if (
      nodes.length > 1 &&
      nodes.length <= 2500 &&
      prevFingerprintRef.current !== fingerprint
    ) {
      prevFingerprintRef.current = fingerprint;
      const inferred = forceAtlas2.inferSettings(graph);
      forceAtlas2.assign(graph, {
        iterations: 100,
        settings: {
          ...inferred,
          gravity: 0.55,
          scalingRatio: 18,
          strongGravityMode: false,
          slowDown: 2,
        },
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
  }, [
    edges,
    graph,
    highlightedPathIds,
    isDark,
    isZh,
    neighborIdSet,
    nodes,
    selectedNodeId,
    visualByNodeId,
  ]);

  // Mount Sigma (WebGL2) once with stagePadding & clean label thresholds
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
              defaultValue: isDarkRef.current ? "#f4f4f5" : "#18181b",
            },
          },
          edges: {
            ...DEFAULT_STYLES.edges,
            labelColor: {
              attribute: "labelColor",
              defaultValue: isDarkRef.current ? "#a1a1aa" : "#64748b",
            },
          },
        },
        settings: {
          itemSizesReference: "screen",
          autoRescaleContent: "labels",
          renderLabels: true,
          renderEdgeLabels: false,
          labelDensity: 0.35,
          labelGridCellSize: 96,
          labelRenderedSizeThreshold: 6,
          stagePadding: 48,
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
        const inferred = forceAtlas2.inferSettings(graph);
        const fa2 = new FA2Layout(graph, {
          settings: {
            ...inferred,
            gravity: 0.55,
            scalingRatio: 18,
            strongGravityMode: false,
          },
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

  // 2D Canvas fallback renderer with collision avoidance and safe edge padding
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
      const h = parent ? Math.max(parent.clientHeight, 320) : 500;
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
        const pad = 88;
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

      const zoom = fallbackZoomRef.current;
      const cxMid = proj.w / 2;
      const cyMid = proj.h / 2;

      const project = (gx: number, gy: number): [number, number] => {
        if (nodes.length === 1 && !fallbackDragRef.current) {
          return [cxMid, cyMid];
        }
        const rawX =
          proj.pad +
          ((gx - proj.minX) / proj.spanX) * (proj.w - proj.pad * 2);
        const rawY =
          proj.pad +
          ((gy - proj.minY) / proj.spanY) * (proj.h - proj.pad * 2);
        return [
          cxMid + (rawX - cxMid) * zoom,
          cyMid + (rawY - cyMid) * zoom,
        ];
      };

      // Draw edges clearly
      graph.forEachEdge((_eid, attrs, _src, _dst, srcAttrs, dstAttrs) => {
        const [x1, y1] = project(
          Number(srcAttrs.x ?? 0),
          Number(srcAttrs.y ?? 0)
        );
        const [x2, y2] = project(
          Number(dstAttrs.x ?? 0),
          Number(dstAttrs.y ?? 0)
        );
        ctx.strokeStyle = String(attrs.color ?? "#94a3b8");
        ctx.lineWidth = Number(attrs.size ?? 1.5);
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.lineTo(x2, y2);
        ctx.stroke();
      });

      const nextProjected: {
        id: string;
        title: string;
        x: number;
        y: number;
        r: number;
      }[] = [];

      const occupiedBoxes: { x1: number; y1: number; x2: number; y2: number }[] =
        [];

      graph.forEachNode((id, attrs) => {
        const [px, py] = project(Number(attrs.x ?? 0), Number(attrs.y ?? 0));
        const r = Number(attrs.size ?? 7);
        const title = String(attrs.label ?? id);
        const fillColor = String(attrs.color ?? "#2563eb");
        const isSel = id === selectedNodeId;
        const inPath = pathSet.has(id);
        const isNeigh = neighborIdSet.has(id);
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
          ctx.strokeStyle = isDark ? "#fafafa" : "#09090b";
          ctx.lineWidth = 2;
          ctx.beginPath();
          ctx.arc(px, py, r + 2, 0, Math.PI * 2);
          ctx.stroke();
        }

        // Collision-aware label rendering with safe horizontal clamping
        ctx.font = isSel ? "600 11px sans-serif" : "11px sans-serif";
        const textW = ctx.measureText(title).width;
        const clampedX = Math.max(
          textW / 2 + 8,
          Math.min(w - textW / 2 - 8, px)
        );
        const labelY = py + r + 13;
        const box = {
          x1: clampedX - textW / 2 - 3,
          y1: labelY - 11,
          x2: clampedX + textW / 2 + 3,
          y2: labelY + 3,
        };
        const collides = occupiedBoxes.some(
          (b) =>
            !(
              box.x2 < b.x1 ||
              box.x1 > b.x2 ||
              box.y2 < b.y1 ||
              box.y1 > b.y2
            )
        );

        if (isSel || inPath || isNeigh || !collides) {
          occupiedBoxes.push(box);
          ctx.fillStyle = isDark ? "#f4f4f5" : "#18181b";
          ctx.textAlign = "center";
          ctx.fillText(title, clampedX, labelY);
        }
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
    neighborIdSet,
    nodes,
    selectedNodeId,
    visualByNodeId,
    webglReady,
  ]);

  const handleZoomIn = () => {
    if (sigmaRef.current) {
      void sigmaRef.current.getCamera().zoomIn({ duration: 180 });
    } else {
      fallbackZoomRef.current = Math.min(3, fallbackZoomRef.current * 1.25);
      drawFallbackRef.current?.();
    }
  };

  const handleZoomOut = () => {
    if (sigmaRef.current) {
      void sigmaRef.current.getCamera().zoomOut({ duration: 180 });
    } else {
      fallbackZoomRef.current = Math.max(0.5, fallbackZoomRef.current / 1.25);
      drawFallbackRef.current?.();
    }
  };

  const handleFitView = () => {
    if (sigmaRef.current) {
      void sigmaRef.current.getCamera().reset({ duration: 220 });
    } else {
      fallbackZoomRef.current = 1;
      drawFallbackRef.current?.();
    }
  };

  const handleToggleFullscreen = () => {
    const el = wrapperRef.current;
    if (!el || typeof document === "undefined") return;
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => {});
    } else {
      void el.requestFullscreen?.().catch(() => {});
    }
  };

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
      ref={wrapperRef}
      className={cn(
        "relative w-full flex-1 min-h-[360px] bg-background overflow-hidden",
        cursorClass,
        className
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

      {/* Floating Viewport Controls (Zoom In / Zoom Out / Fit / Fullscreen) */}
      <div
        className="absolute right-3 bottom-3 z-10 inline-flex items-center rounded-md border border-border bg-card/90 backdrop-blur-xs shadow-xs p-0.5"
        data-testid="graph-viewport-controls"
      >
        <button
          type="button"
          onClick={handleZoomIn}
          title={tx("放大", "Zoom in")}
          aria-label={tx("放大", "Zoom in")}
          className="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
        >
          <ZoomIn className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          onClick={handleZoomOut}
          title={tx("缩小", "Zoom out")}
          aria-label={tx("缩小", "Zoom out")}
          className="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
        >
          <ZoomOut className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          onClick={handleFitView}
          title={tx("适应画布", "Fit view")}
          aria-label={tx("适应画布", "Fit view")}
          className="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
        >
          <RotateCcw className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          onClick={handleToggleFullscreen}
          title={
            isFullscreen ? tx("退出全屏", "Exit fullscreen") : tx("全屏", "Fullscreen")
          }
          aria-label={
            isFullscreen ? tx("退出全屏", "Exit fullscreen") : tx("全屏", "Fullscreen")
          }
          className="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
        >
          {isFullscreen ? (
            <Minimize2 className="h-3.5 w-3.5" />
          ) : (
            <Maximize2 className="h-3.5 w-3.5" />
          )}
        </button>
      </div>

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
