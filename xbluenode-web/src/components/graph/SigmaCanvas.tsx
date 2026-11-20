import { useEffect, useMemo, useRef, useState } from "react";
import Graph from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import Sigma from "sigma";
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
  isDark: boolean;
  onSelectNode: (nodeId: string) => void;
  onExpandNode: (nodeId: string) => void;
  onSetPathEndpoint: (role: "from" | "to", nodeId: string) => void;
}

const LIGHT_PALETTE = [
  "#2563eb",
  "#09090b",
  "#52525b",
  "#1d4ed8",
  "#3f3f46",
  "#60a5fa",
];

const DARK_PALETTE = [
  "#3b82f6",
  "#fafafa",
  "#a1a1aa",
  "#60a5fa",
  "#d4d4d8",
  "#93c5fd",
];

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
  isDark,
  onSelectNode,
  onExpandNode,
  onSetPathEndpoint,
}: SigmaCanvasProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const fallbackCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const graphRef = useRef<Graph | null>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const fa2Ref = useRef<FA2Layout | null>(null);

  const [webglReady, setWebglReady] = useState<boolean>(() =>
    isWebGl2Supported()
  );
  const [contextMenu, setContextMenu] = useState<{
    nodeId: string;
    title: string;
    x: number;
    y: number;
  } | null>(null);

  const selectRef = useRef(onSelectNode);
  const expandRef = useRef(onExpandNode);
  useEffect(() => {
    selectRef.current = onSelectNode;
    expandRef.current = onExpandNode;
  }, [onExpandNode, onSelectNode]);

  const visualByNodeId = useMemo(() => {
    const palette = isDark ? DARK_PALETTE : LIGHT_PALETTE;
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";
    const fg = isDark ? "#fafafa" : "#09090b";

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

      let size = isSelected || isInPath ? 15 : 10;
      let color = palette[typeIdx % palette.length] ?? fg;

      if (colorMode === "METRIC" && colorMetricKey) {
        const val = n.metrics?.[colorMetricKey];
        if (typeof val === "number" && Number.isFinite(val)) {
          const span = maxMetric - minMetric;
          const ratio = span > 0 ? (val - minMetric) / span : 0.5;
          size = Math.round(8 + ratio * 12);
          color = ratio >= 0.7 ? accentBlue : ratio >= 0.35 ? "#60a5fa" : "#71717a";
        }
      }

      if (colorMode === "ALGO" || algoMode !== "NONE") {
        if (
          algoMode === "SCORE_SIZE" &&
          algoEntry?.score !== null &&
          algoEntry?.score !== undefined &&
          maxScore > 0
        ) {
          size = Math.round(8 + (algoEntry.score / maxScore) * 14);
        }
        if (
          algoMode === "COMMUNITY_COLOR" &&
          algoEntry?.community !== null &&
          algoEntry?.community !== undefined
        ) {
          color = palette[Math.abs(algoEntry.community) % palette.length] ?? accentBlue;
        }
      }

      if (isSelected || isInPath) {
        color = accentBlue;
      }

      map.set(n.id, { color, size });
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

  // Build / sync Graphology graph & positions
  const graph = useMemo(() => {
    const prevGraph = graphRef.current;
    const g = new Graph({ multi: true, type: "directed" });
    const count = Math.max(nodes.length, 1);

    nodes.forEach((n, idx) => {
      const prevAttrs =
        prevGraph && prevGraph.hasNode(n.id)
          ? prevGraph.getNodeAttributes(n.id)
          : null;
      const angle = (2 * Math.PI * idx) / count;
      const radius = count <= 1 ? 0 : 120;
      const x =
        typeof prevAttrs?.x === "number" ? prevAttrs.x : Math.cos(angle) * radius;
      const y =
        typeof prevAttrs?.y === "number" ? prevAttrs.y : Math.sin(angle) * radius;
      const vis = visualByNodeId.get(n.id) ?? { color: "#2563eb", size: 10 };

      g.addNode(n.id, {
        x,
        y,
        size: vis.size,
        color: vis.color,
        label: `${n.title} (${n.node_key})`,
        nodeType: n.type_name,
      });
    });

    const pathPairSet = new Set<string>();
    for (let i = 0; i < highlightedPathIds.length - 1; i += 1) {
      const a = highlightedPathIds[i];
      const b = highlightedPathIds[i + 1];
      if (a && b) {
        pathPairSet.add(`${a}->${b}`);
        pathPairSet.add(`${b}->${a}`);
      }
    }

    const edgeColorMuted = isDark ? "#3f3f46" : "#d4d4d8";
    const accentBlue = isDark ? "#3b82f6" : "#2563eb";

    edges.forEach((e, idx) => {
      if (!g.hasNode(e.src_node_id) || !g.hasNode(e.dst_node_id)) return;
      const inPath = pathPairSet.has(`${e.src_node_id}->${e.dst_node_id}`);
      const edgeKey = `${e.id}-${idx}`;
      if (!g.hasEdge(edgeKey)) {
        g.addDirectedEdgeWithKey(edgeKey, e.src_node_id, e.dst_node_id, {
          size: inPath ? 3 : 1.2,
          color: inPath ? accentBlue : edgeColorMuted,
          label: e.link_type,
        });
      }
    });

    if (nodes.length > 1 && nodes.length <= 2500 && !prevGraph) {
      forceAtlas2.assign(g, {
        iterations: 40,
        settings: forceAtlas2.inferSettings(g),
      });
    }

    graphRef.current = g;
    return g;
  }, [edges, highlightedPathIds, isDark, nodes, visualByNodeId]);

  // Mount Sigma (WebGL2) or fallback cleanly if WebGL2 is absent
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
        settings: {
          renderLabels: true,
          renderEdgeLabels: edges.length <= 80,
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

    sigma.on("clickNode", ({ node }) => {
      setContextMenu(null);
      selectRef.current(node);
    });

    sigma.on("doubleClickNode", ({ node, preventSigmaDefault }) => {
      preventSigmaDefault();
      setContextMenu(null);
      selectRef.current(node);
      expandRef.current(node);
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

    let fa2: FA2Layout | null = null;
    if (fa2Running && graph.order > 1) {
      try {
        fa2 = new FA2Layout(graph, {
          settings: forceAtlas2.inferSettings(graph),
        });
        fa2.start();
        fa2Ref.current = fa2;
      } catch {
        fa2Ref.current = null;
      }
    }

    return () => {
      if (fa2) {
        fa2.kill();
      }
      fa2Ref.current = null;
      sigma.kill();
      sigmaRef.current = null;
    };
  }, [edges.length, fa2Running, graph]);

  // 2D Canvas fallback renderer when WebGL2 is unavailable (e.g. headless browser)
  useEffect(() => {
    if (webglReady) return;
    const canvas = fallbackCanvasRef.current;
    if (!canvas) return;

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

      if (nodes.length === 0) return;

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
      const spanX = Math.max(maxX - minX, 1);
      const spanY = Math.max(maxY - minY, 1);
      const project = (gx: number, gy: number): [number, number] => [
        pad + ((gx - minX) / spanX) * (w - pad * 2),
        pad + ((gy - minY) / spanY) * (h - pad * 2),
      ];

      // Draw edges
      graph.forEachEdge((_eid, attrs, _src, _dst, srcAttrs, dstAttrs) => {
        const [x1, y1] = project(Number(srcAttrs.x ?? 0), Number(srcAttrs.y ?? 0));
        const [x2, y2] = project(Number(dstAttrs.x ?? 0), Number(dstAttrs.y ?? 0));
        ctx.strokeStyle = String(attrs.color ?? "#d4d4d8");
        ctx.lineWidth = Number(attrs.size ?? 1.2);
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.lineTo(x2, y2);
        ctx.stroke();
      });

      // Draw nodes
      graph.forEachNode((id, attrs) => {
        const [px, py] = project(Number(attrs.x ?? 0), Number(attrs.y ?? 0));
        const r = Number(attrs.size ?? 10);
        ctx.fillStyle = String(attrs.color ?? "#2563eb");
        ctx.beginPath();
        ctx.arc(px, py, r, 0, Math.PI * 2);
        ctx.fill();
        if (id === selectedNodeId) {
          ctx.strokeStyle = isDark ? "#fafafa" : "#09090b";
          ctx.lineWidth = 2.5;
          ctx.stroke();
        }
        ctx.fillStyle = isDark ? "#fafafa" : "#09090b";
        ctx.font = "11px sans-serif";
        ctx.textAlign = "center";
        ctx.fillText(String(attrs.label ?? id), px, py + r + 13);
      });
    };

    draw();
    const ro = new ResizeObserver(draw);
    if (canvas.parentElement) ro.observe(canvas.parentElement);
    return () => ro.disconnect();
  }, [graph, isDark, nodes.length, selectedNodeId, webglReady]);

  return (
    <div
      className="relative w-full h-[540px] bg-background overflow-hidden"
      data-testid="xbluegraph-canvas"
      onContextMenu={(e) => e.preventDefault()}
    >
      {webglReady ? (
        <div ref={containerRef} className="w-full h-full" />
      ) : (
        <canvas
          ref={fallbackCanvasRef}
          className="w-full h-full block"
          onClick={() => setContextMenu(null)}
        />
      )}

      {contextMenu && (
        <div
          style={{
            left: `${Math.max(8, Math.min(contextMenu.x, 420))}px`,
            top: `${Math.max(8, Math.min(contextMenu.y, 380))}px`,
          }}
          className="absolute z-20 w-48 rounded-md border border-border bg-background shadow-lg p-1.5 text-xs space-y-1"
          data-testid="graph-context-menu"
        >
          <div className="px-2 py-1 font-medium truncate border-b border-border">
            {contextMenu.title}
          </div>
          <button
            type="button"
            onClick={() => {
              onExpandNode(contextMenu.nodeId);
              setContextMenu(null);
            }}
            className="w-full text-left rounded px-2 py-1 hover:bg-muted cursor-pointer"
          >
            展开 1 跳邻居
          </button>
          <button
            type="button"
            onClick={() => {
              onSetPathEndpoint("from", contextMenu.nodeId);
              setContextMenu(null);
            }}
            className="w-full text-left rounded px-2 py-1 hover:bg-muted cursor-pointer"
          >
            查找路径：设为起点
          </button>
          <button
            type="button"
            onClick={() => {
              onSetPathEndpoint("to", contextMenu.nodeId);
              setContextMenu(null);
            }}
            className="w-full text-left rounded px-2 py-1 hover:bg-muted text-xblue cursor-pointer"
          >
            查找路径：设为终点
          </button>
        </div>
      )}
    </div>
  );
}
