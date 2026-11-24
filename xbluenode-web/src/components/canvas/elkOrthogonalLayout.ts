import ELK, { type ElkExtendedEdge, type ElkNode } from "elkjs/lib/elk.bundled.js";
import type { MeasuredBox } from "./alignmentGuides.ts";

export interface OrthogonalNodeInput {
  readonly id: string;
  readonly width: number;
  readonly height: number;
  readonly domainGroup?: string | null;
}

export interface OrthogonalEdgeInput {
  readonly id: string;
  readonly source: string;
  readonly target: string;
  readonly labelWidth?: number;
  readonly labelHeight?: number;
}

export interface PortAllocation {
  readonly inPorts: readonly string[];
  readonly outPorts: readonly string[];
}

export interface EdgePortBinding {
  readonly edgeId: string;
  readonly sourcePortId: string;
  readonly targetPortId: string;
  readonly sourcePortIndex: number;
  readonly sourcePortCount: number;
  readonly targetPortIndex: number;
  readonly targetPortCount: number;
}

export interface DomainContainerBox {
  readonly id: string;
  readonly domainGroup: string;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

export interface OrthogonalLayoutResult {
  readonly nodePositions: Readonly<Record<string, { x: number; y: number }>>;
  readonly domainBoxes: Readonly<Record<string, DomainContainerBox>>;
  readonly portsByNode: Readonly<Record<string, PortAllocation>>;
  readonly edgeBindings: Readonly<Record<string, EdgePortBinding>>;
}

export const STUDIO_ELK_LAYOUT_OPTIONS: Readonly<Record<string, string>> = {
  "elk.algorithm": "layered",
  "elk.direction": "RIGHT",
  "elk.edgeRouting": "ORTHOGONAL",
  "elk.layered.spacing.nodeNodeBetweenLayers": "220",
  "elk.spacing.nodeNode": "96",
  "elk.spacing.edgeEdge": "28",
  "elk.hierarchyHandling": "INCLUDE_CHILDREN",
  "elk.padding": "[top=64,left=56,bottom=56,right=56]",
};

/**
 * Allocates deterministic multi-port handles (`in-0..k`, `out-0..m`) on each node's
 * left (`WEST`) and right (`EAST`) sides so parallel/converging edges never stack on a single point.
 */
export function allocateMultiPorts(
  nodes: readonly Pick<OrthogonalNodeInput, "id">[],
  edges: readonly OrthogonalEdgeInput[]
): {
  portsByNode: Record<string, PortAllocation>;
  edgeBindings: Record<string, EdgePortBinding>;
} {
  const inEdgesByNode = new Map<string, OrthogonalEdgeInput[]>();
  const outEdgesByNode = new Map<string, OrthogonalEdgeInput[]>();

  for (const n of nodes) {
    inEdgesByNode.set(n.id, []);
    outEdgesByNode.set(n.id, []);
  }

  for (const edge of edges) {
    const outs = outEdgesByNode.get(edge.source);
    const ins = inEdgesByNode.get(edge.target);
    if (!outs || !ins) continue;
    outs.push(edge);
    ins.push(edge);
  }

  const portsByNode: Record<string, PortAllocation> = {};
  const edgeBindings: Record<string, EdgePortBinding> = {};
  const partialSource = new Map<
    string,
    { portId: string; index: number; count: number }
  >();
  const partialTarget = new Map<
    string,
    { portId: string; index: number; count: number }
  >();

  for (const n of nodes) {
    const outs = outEdgesByNode.get(n.id) ?? [];
    const ins = inEdgesByNode.get(n.id) ?? [];

    const outPorts =
      outs.length > 0
        ? outs.map((e, idx) => {
            const portId = `out-${idx}`;
            partialSource.set(e.id, {
              portId,
              index: idx,
              count: outs.length,
            });
            return portId;
          })
        : ["out-0"];

    const inPorts =
      ins.length > 0
        ? ins.map((e, idx) => {
            const portId = `in-${idx}`;
            partialTarget.set(e.id, {
              portId,
              index: idx,
              count: ins.length,
            });
            return portId;
          })
        : ["in-0"];

    portsByNode[n.id] = { inPorts, outPorts };
  }

  for (const edge of edges) {
    const src = partialSource.get(edge.id);
    const dst = partialTarget.get(edge.id);
    if (!src || !dst) continue;
    edgeBindings[edge.id] = {
      edgeId: edge.id,
      sourcePortId: src.portId,
      targetPortId: dst.portId,
      sourcePortIndex: src.index,
      sourcePortCount: src.count,
      targetPortIndex: dst.index,
      targetPortCount: dst.count,
    };
  }

  return { portsByNode, edgeBindings };
}

/**
 * Computes exact intersection area between two axis-aligned rectangles.
 * Returns `0` when separated or merely touching edges.
 */
export function rectIntersectionArea(a: MeasuredBox, b: MeasuredBox): number {
  const overlapW = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
  const overlapH =
    Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
  if (overlapW <= 0 || overlapH <= 0) {
    return 0;
  }
  return overlapW * overlapH;
}

/**
 * Computes collision-free center coordinates `(x, y)` for every edge label pill
 * so NO edge label (`[data-testid^="canvas-edge-"]`) ever intersects ANY node card
 * (`[data-testid^="canvas-node-"]`) or any sibling edge label.
 */
export function computeCollisionFreeEdgeLabels(
  edges: ReadonlyArray<{
    readonly id: string;
    readonly sourceX: number;
    readonly sourceY: number;
    readonly targetX: number;
    readonly targetY: number;
    readonly pillWidth: number;
    readonly pillHeight: number;
  }>,
  nodeObstacles: readonly MeasuredBox[],
  safetyPadding = 10
): Record<string, { x: number; y: number }> {
  const expandedObstacles: MeasuredBox[] = nodeObstacles.map((obs) => ({
    id: obs.id,
    x: obs.x - safetyPadding,
    y: obs.y - safetyPadding,
    width: obs.width + safetyPadding * 2,
    height: obs.height + safetyPadding * 2,
  }));

  const placedLabels: MeasuredBox[] = [];
  const placements: Record<string, { x: number; y: number }> = {};

  const collides = (cx: number, cy: number, w: number, h: number): boolean => {
    const candidate: MeasuredBox = {
      id: "candidate",
      x: cx - w / 2,
      y: cy - h / 2,
      width: w,
      height: h,
    };
    for (const obs of expandedObstacles) {
      if (rectIntersectionArea(candidate, obs) > 0) {
        return true;
      }
    }
    for (const prev of placedLabels) {
      if (rectIntersectionArea(candidate, prev) > 0) {
        return true;
      }
    }
    return false;
  };

  for (const edge of edges) {
    const w = Math.max(64, edge.pillWidth);
    const h = Math.max(22, edge.pillHeight);
    const midX = (edge.sourceX + edge.targetX) / 2;
    const midY = (edge.sourceY + edge.targetY) / 2;

    const candidates: Array<{ x: number; y: number }> = [{ x: midX, y: midY }];

    // Interpolated points along the channel + perpendicular vertical/horizontal offsets
    const tFractions = [0.5, 0.38, 0.62, 0.28, 0.72];
    const dySteps = [0, -32, 32, -64, 64, -96, 96, -132, 132, -172, 172, -220, 220];
    const dxSteps = [0, -44, 44, -88, 88];

    for (const dy of dySteps) {
      for (const t of tFractions) {
        const bx = edge.sourceX + (edge.targetX - edge.sourceX) * t;
        const by = edge.sourceY + (edge.targetY - edge.sourceY) * t;
        for (const dx of dxSteps) {
          candidates.push({ x: bx + dx, y: by + dy });
        }
      }
    }

    // Guaranteed fallback above/below global obstacle envelope
    if (expandedObstacles.length > 0) {
      const globalTop = Math.min(...expandedObstacles.map((o) => o.y));
      const globalBottom = Math.max(
        ...expandedObstacles.map((o) => o.y + o.height)
      );
      candidates.push({ x: midX, y: globalTop - h - 16 });
      candidates.push({ x: midX, y: globalBottom + h + 16 });
    }

    let chosen = { x: midX, y: midY };
    for (const cand of candidates) {
      if (!collides(cand.x, cand.y, w, h)) {
        chosen = {
          x: Math.round(cand.x),
          y: Math.round(cand.y),
        };
        break;
      }
    }

    placements[edge.id] = chosen;
    placedLabels.push({
      id: edge.id,
      x: chosen.x - w / 2 - 4,
      y: chosen.y - h / 2 - 4,
      width: w + 8,
      height: h + 8,
    });
  }

  return placements;
}

/**
 * Runs ELK `layered` + `ORTHOGONAL` + `FIXED_ORDER` multi-port layout with optional
 * compound `domainGroup` hierarchy (`INCLUDE_CHILDREN`).
 */
export async function computeElkOrthogonalLayout(
  nodes: readonly OrthogonalNodeInput[],
  edges: readonly OrthogonalEdgeInput[],
  options?: {
    readonly groupByDomain?: boolean;
    readonly collapsedDomains?: ReadonlySet<string>;
  }
): Promise<OrthogonalLayoutResult> {
  const groupByDomain = Boolean(options?.groupByDomain);
  const collapsedDomains = options?.collapsedDomains ?? new Set<string>();
  const { portsByNode, edgeBindings } = allocateMultiPorts(nodes, edges);

  if (nodes.length === 0) {
    return {
      nodePositions: {},
      domainBoxes: {},
      portsByNode,
      edgeBindings,
    };
  }

  const elk = new ELK();
  const buildElkNode = (n: OrthogonalNodeInput): ElkNode => {
    const alloc = portsByNode[n.id] ?? { inPorts: ["in-0"], outPorts: ["out-0"] };
    const inCount = alloc.inPorts.length;
    const outCount = alloc.outPorts.length;
    return {
      id: n.id,
      width: Math.max(200, Math.round(n.width)),
      height: Math.max(120, Math.round(n.height)),
      layoutOptions: {
        "org.eclipse.elk.portConstraints": "FIXED_ORDER",
      },
      ports: [
        ...alloc.inPorts.map((pid, idx) => ({
          id: `${n.id}::${pid}`,
          x: 0,
          y: Math.round(((idx + 1) / (inCount + 1)) * n.height),
          layoutOptions: {
            "org.eclipse.elk.port.side": "WEST",
            "org.eclipse.elk.port.index": String(idx),
          },
        })),
        ...alloc.outPorts.map((pid, idx) => ({
          id: `${n.id}::${pid}`,
          x: Math.round(n.width),
          y: Math.round(((idx + 1) / (outCount + 1)) * n.height),
          layoutOptions: {
            "org.eclipse.elk.port.side": "EAST",
            "org.eclipse.elk.port.index": String(idx),
          },
        })),
      ],
    };
  };

  let topChildren: ElkNode[] = [];
  const nodeToEffectiveEndpoint = new Map<string, string>();
  const domainGroupsMap = new Map<string, OrthogonalNodeInput[]>();

  if (groupByDomain) {
    for (const n of nodes) {
      const dg = n.domainGroup?.trim();
      if (dg) {
        const bucket = domainGroupsMap.get(dg) ?? [];
        bucket.push(n);
        domainGroupsMap.set(dg, bucket);
      } else {
        topChildren.push(buildElkNode(n));
        nodeToEffectiveEndpoint.set(n.id, n.id);
      }
    }

    for (const [dg, members] of domainGroupsMap.entries()) {
      const domainId = `domain:${dg}`;
      if (collapsedDomains.has(dg)) {
        topChildren.push({
          id: domainId,
          width: 264,
          height: 92,
        });
        for (const m of members) {
          nodeToEffectiveEndpoint.set(m.id, domainId);
        }
      } else {
        topChildren.push({
          id: domainId,
          layoutOptions: {
            "elk.algorithm": "layered",
            "elk.direction": "RIGHT",
            "elk.edgeRouting": "ORTHOGONAL",
            "elk.layered.spacing.nodeNodeBetweenLayers": "220",
            "elk.spacing.nodeNode": "96",
            "elk.padding": "[top=56,left=28,bottom=28,right=28]",
          },
          children: members.map(buildElkNode),
        });
        for (const m of members) {
          nodeToEffectiveEndpoint.set(m.id, m.id);
        }
      }
    }
  } else {
    topChildren = nodes.map(buildElkNode);
    for (const n of nodes) {
      nodeToEffectiveEndpoint.set(n.id, n.id);
    }
  }

  const elkEdges: ElkExtendedEdge[] = [];
  for (const edge of edges) {
    const effSrc = nodeToEffectiveEndpoint.get(edge.source);
    const effDst = nodeToEffectiveEndpoint.get(edge.target);
    if (!effSrc || !effDst || effSrc === effDst) continue;

    const binding = edgeBindings[edge.id];
    const useSrcPort =
      effSrc === edge.source && binding
        ? `${edge.source}::${binding.sourcePortId}`
        : effSrc;
    const useDstPort =
      effDst === edge.target && binding
        ? `${edge.target}::${binding.targetPortId}`
        : effDst;

    elkEdges.push({
      id: edge.id,
      sources: [useSrcPort],
      targets: [useDstPort],
      labels: [
        {
          text: edge.id,
          width: edge.labelWidth ?? 120,
          height: edge.labelHeight ?? 28,
        },
      ],
    });
  }

  const laidOut = await elk.layout({
    id: "studio-orthogonal-root",
    layoutOptions: { ...STUDIO_ELK_LAYOUT_OPTIONS },
    children: topChildren,
    edges: elkEdges,
  });

  const nodePositions: Record<string, { x: number; y: number }> = {};
  const domainBoxes: Record<string, DomainContainerBox> = {};

  for (const child of laidOut.children ?? []) {
    const cx = Math.round((child.x ?? 0) / 16) * 16;
    const cy = Math.round((child.y ?? 0) / 16) * 16;
    if (child.id.startsWith("domain:")) {
      const dg = child.id.slice("domain:".length);
      domainBoxes[dg] = {
        id: child.id,
        domainGroup: dg,
        x: cx,
        y: cy,
        width: Math.max(264, Math.round(child.width ?? 320)),
        height: Math.max(92, Math.round(child.height ?? 220)),
      };
      for (const sub of child.children ?? []) {
        // Store absolute flow position in schema, while relative position inside parent is derived cleanly
        const relX = Math.round((sub.x ?? 28) / 8) * 8;
        const relY = Math.round((sub.y ?? 56) / 8) * 8;
        nodePositions[sub.id] = {
          x: cx + relX,
          y: cy + relY,
        };
      }
    } else {
      nodePositions[child.id] = { x: cx, y: cy };
    }
  }

  return {
    nodePositions,
    domainBoxes,
    portsByNode,
    edgeBindings,
  };
}
