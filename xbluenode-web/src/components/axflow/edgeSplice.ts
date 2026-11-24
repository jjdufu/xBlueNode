import type { FlowEdgeDef, FlowNodeDef } from "../../lib/api/axflow.ts";

export interface EdgeSegmentGeometry {
  readonly edgeIndex: number;
  readonly sourceX: number;
  readonly sourceY: number;
  readonly targetX: number;
  readonly targetY: number;
}

function pointToSegmentDist(
  px: number,
  py: number,
  x1: number,
  y1: number,
  x2: number,
  y2: number
): number {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const lenSq = dx * dx + dy * dy;
  if (lenSq === 0) {
    return Math.hypot(px - x1, py - y1);
  }
  const t = Math.max(0, Math.min(1, ((px - x1) * dx + (py - y1) * dy) / lenSq));
  const projX = x1 + t * dx;
  const projY = y1 + t * dy;
  return Math.hypot(px - projX, py - projY);
}

/**
 * Finds the closest existing DAG edge within `thresholdPx` (default `18px`) of a cursor flow point.
 */
export function findClosestEdgeWithinThreshold(
  point: { readonly x: number; readonly y: number },
  edges: readonly EdgeSegmentGeometry[],
  thresholdPx = 18
): number | null {
  let bestIdx: number | null = null;
  let bestDist = thresholdPx;

  for (const e of edges) {
    const midX = (e.sourceX + e.targetX) / 2;
    const d1 = pointToSegmentDist(
      point.x,
      point.y,
      e.sourceX,
      e.sourceY,
      midX,
      e.sourceY
    );
    const d2 = pointToSegmentDist(
      point.x,
      point.y,
      midX,
      e.sourceY,
      midX,
      e.targetY
    );
    const d3 = pointToSegmentDist(
      point.x,
      point.y,
      midX,
      e.targetY,
      e.targetX,
      e.targetY
    );
    const dDirect = pointToSegmentDist(
      point.x,
      point.y,
      e.sourceX,
      e.sourceY,
      e.targetX,
      e.targetY
    );
    const minDist = Math.min(d1, d2, d3, dDirect);
    if (minDist < bestDist) {
      bestDist = minDist;
      bestIdx = e.edgeIndex;
    }
  }

  return bestIdx;
}

/**
 * Atomically splits edge `(u, v)` at `edgeIndex` into `(u, newNode)` (inheriting `when`)
 * and `(newNode, v)`, nudging overlapping downstream nodes to preserve clean spacing.
 */
export function spliceNodeIntoFlowEdge(
  nodes: readonly FlowNodeDef[],
  edges: readonly FlowEdgeDef[],
  edgeIndex: number,
  newNode: FlowNodeDef
): { nodes: FlowNodeDef[]; edges: FlowEdgeDef[] } {
  const targetEdge = edges[edgeIndex];
  if (!targetEdge) {
    return {
      nodes: [...nodes, newNode],
      edges: [...edges],
    };
  }

  const srcNode = nodes.find((n) => n.id === targetEdge.from);
  const dstNode = nodes.find((n) => n.id === targetEdge.to);

  const midPos =
    srcNode && dstNode
      ? {
          x: Math.round(((srcNode.pos.x + dstNode.pos.x) / 2) / 16) * 16,
          y: Math.round(((srcNode.pos.y + dstNode.pos.y) / 2) / 16) * 16,
        }
      : newNode.pos;

  const needPushRight =
    dstNode !== undefined && dstNode.pos.x - midPos.x < 240;
  const pushDeltaX = needPushRight ? 256 : 0;

  // Collect downstream closure from `targetEdge.to` if nudging right
  const downstreamSet = new Set<string>();
  if (pushDeltaX > 0 && dstNode) {
    const queue: string[] = [dstNode.id];
    downstreamSet.add(dstNode.id);
    while (queue.length > 0) {
      const curr = queue.shift();
      if (!curr) continue;
      for (const ed of edges) {
        if (ed.from === curr && !downstreamSet.has(ed.to)) {
          downstreamSet.add(ed.to);
          queue.push(ed.to);
        }
      }
    }
  }

  const placedNewNode: FlowNodeDef = {
    ...newNode,
    pos: midPos,
  };

  const nextNodes = [
    ...nodes.map((n) =>
      downstreamSet.has(n.id)
        ? { ...n, pos: { x: n.pos.x + pushDeltaX, y: n.pos.y } }
        : n
    ),
    placedNewNode,
  ];

  const nextEdges: FlowEdgeDef[] = [];
  edges.forEach((ed, idx) => {
    if (idx === edgeIndex) {
      nextEdges.push({
        from: targetEdge.from,
        to: placedNewNode.id,
        when: targetEdge.when ?? null,
      });
      nextEdges.push({
        from: placedNewNode.id,
        to: targetEdge.to,
        when: null,
      });
    } else {
      nextEdges.push(ed);
    }
  });

  return { nodes: nextNodes, edges: nextEdges };
}
