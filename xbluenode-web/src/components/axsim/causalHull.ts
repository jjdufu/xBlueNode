import type {
  AxSimAction,
  ImpactRow,
  SandboxGraphLink,
  SandboxGraphNode,
} from "../../lib/api/axsim.ts";

export interface SimComboGroupSpec {
  readonly comboId: string;
  readonly key: string;
  readonly label: string;
  readonly memberIds: readonly string[];
}

export interface HullClusterSpec {
  readonly key: string;
  readonly memberIds: readonly string[];
  readonly maxAbsRatio: number;
}

/**
 * Resolves the combo group key (`domain_group` property if present, else `type_name`) for a sandbox node.
 */
export function resolveNodeComboKey(node: SandboxGraphNode): string {
  const rawDomain = node.properties?.domain_group;
  if (typeof rawDomain === "string" && rawDomain.trim().length > 0) {
    return rawDomain.trim();
  }
  return node.type_name || "Default";
}

/**
 * Computes `Combo` groupings for visible sandbox nodes.
 */
export function buildSimComboGroups(
  nodes: readonly SandboxGraphNode[]
): SimComboGroupSpec[] {
  const byKey = new Map<string, string[]>();
  for (const n of nodes) {
    const key = resolveNodeComboKey(n);
    const list = byKey.get(key) ?? [];
    list.push(n.id);
    byKey.set(key, list);
  }
  return Array.from(byKey.entries()).map(([key, memberIds]) => ({
    comboId: `combo:${key}`,
    key,
    label: key,
    memberIds,
  }));
}

/**
 * Extracts effective relative impact magnitude `|delta_ratio|` for a node.
 */
export function computeNodeAbsDeltaRatio(
  node: SandboxGraphNode,
  impacts: readonly ImpactRow[],
  actions: readonly AxSimAction[]
): number {
  let maxAbs = 0;
  for (const imp of impacts) {
    if (imp.node_id !== node.id) continue;
    if (typeof imp.delta_ratio === "number" && Number.isFinite(imp.delta_ratio)) {
      maxAbs = Math.max(maxAbs, Math.abs(imp.delta_ratio));
    } else if (
      typeof imp.delta === "number" &&
      typeof imp.before_val === "number" &&
      imp.before_val !== 0
    ) {
      maxAbs = Math.max(maxAbs, Math.abs(imp.delta / imp.before_val));
    }
  }
  for (const act of actions) {
    if (act.target_node_id !== node.id) continue;
    if (act.mode === "RATIO" && typeof act.value === "number") {
      maxAbs = Math.max(maxAbs, Math.abs(act.value));
    } else if (typeof act.delta_ratio === "number") {
      maxAbs = Math.max(maxAbs, Math.abs(act.delta_ratio));
    }
  }
  return maxAbs;
}

/**
 * Identifies connected subgraphs containing `>= 2` high-impact nodes (`|delta_pct| >= 15%`)
 * so G6 `hull` plugin draws soft risk contours around shock clusters.
 */
export function computeHighImpactHullClusters(
  nodes: readonly SandboxGraphNode[],
  links: readonly SandboxGraphLink[],
  impacts: readonly ImpactRow[],
  actions: readonly AxSimAction[],
  hiddenNodeIds: ReadonlySet<string> = new Set(),
  thresholdRatio = 0.15
): HullClusterSpec[] {
  const ratioByNode = new Map<string, number>();
  for (const n of nodes) {
    if (hiddenNodeIds.has(n.id)) continue;
    const absRatio = computeNodeAbsDeltaRatio(n, impacts, actions);
    if (absRatio >= thresholdRatio) {
      ratioByNode.set(n.id, absRatio);
    }
  }

  if (ratioByNode.size < 2) {
    return [];
  }

  const adj = new Map<string, Set<string>>();
  for (const id of ratioByNode.keys()) {
    adj.set(id, new Set());
  }
  for (const lk of links) {
    if (ratioByNode.has(lk.src_node_id) && ratioByNode.has(lk.dst_node_id)) {
      adj.get(lk.src_node_id)?.add(lk.dst_node_id);
      adj.get(lk.dst_node_id)?.add(lk.src_node_id);
    }
  }
  // Also connect consecutive nodes in `top_path`
  for (const imp of impacts) {
    const path = imp.top_path ?? [];
    for (let i = 0; i < path.length - 1; i += 1) {
      const u = path[i];
      const v = path[i + 1];
      if (u && v && ratioByNode.has(u) && ratioByNode.has(v)) {
        adj.get(u)?.add(v);
        adj.get(v)?.add(u);
      }
    }
  }

  const visited = new Set<string>();
  const clusters: HullClusterSpec[] = [];
  let clusterIdx = 0;

  for (const startId of ratioByNode.keys()) {
    if (visited.has(startId)) continue;
    const queue = [startId];
    visited.add(startId);
    const component: string[] = [];
    let maxRatio = 0;

    while (queue.length > 0) {
      const curr = queue.shift();
      if (!curr) continue;
      component.push(curr);
      maxRatio = Math.max(maxRatio, ratioByNode.get(curr) ?? 0);
      for (const nxt of adj.get(curr) ?? []) {
        if (!visited.has(nxt)) {
          visited.add(nxt);
          queue.push(nxt);
        }
      }
    }

    if (component.length >= 2) {
      clusters.push({
        key: `risk-hull-${clusterIdx}`,
        memberIds: component,
        maxAbsRatio: maxRatio,
      });
      clusterIdx += 1;
    }
  }

  return clusters;
}
