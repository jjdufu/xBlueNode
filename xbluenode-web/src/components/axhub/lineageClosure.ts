export interface SimpleDirectedEdge {
  readonly id: string;
  readonly source: string;
  readonly target: string;
}

/**
 * Computes full upstream + downstream dependency closure (`nodeIds` and `edgeIds`)
 * around `focusNodeId` in a directed lineage DAG.
 */
export function computeLineageClosure(
  focusNodeId: string,
  edges: readonly SimpleDirectedEdge[]
): {
  nodeIds: Set<string>;
  edgeIds: Set<string>;
} {
  const fwd = new Map<string, SimpleDirectedEdge[]>();
  const rev = new Map<string, SimpleDirectedEdge[]>();

  for (const e of edges) {
    const f = fwd.get(e.source) ?? [];
    f.push(e);
    fwd.set(e.source, f);

    const r = rev.get(e.target) ?? [];
    r.push(e);
    rev.set(e.target, r);
  }

  const nodeIds = new Set<string>([focusNodeId]);
  const edgeIds = new Set<string>();

  // Upstream traversal
  const upQueue = [focusNodeId];
  const visitedUp = new Set<string>([focusNodeId]);
  while (upQueue.length > 0) {
    const curr = upQueue.shift();
    if (!curr) continue;
    for (const edge of rev.get(curr) ?? []) {
      edgeIds.add(edge.id);
      nodeIds.add(edge.source);
      if (!visitedUp.has(edge.source)) {
        visitedUp.add(edge.source);
        upQueue.push(edge.source);
      }
    }
  }

  // Downstream traversal
  const downQueue = [focusNodeId];
  const visitedDown = new Set<string>([focusNodeId]);
  while (downQueue.length > 0) {
    const curr = downQueue.shift();
    if (!curr) continue;
    for (const edge of fwd.get(curr) ?? []) {
      edgeIds.add(edge.id);
      nodeIds.add(edge.target);
      if (!visitedDown.has(edge.target)) {
        visitedDown.add(edge.target);
        downQueue.push(edge.target);
      }
    }
  }

  return { nodeIds, edgeIds };
}
