import type { EdgeTypeSpec, NodeTypeSpec } from "../../lib/api/xblue.ts";

/**
 * Resolves `EVENT_DYNAMIC` and `CUBE_ROLLUP` subordinate object types that belong
 * to a unique `STATIC_FACT` master entity via `ONE_TO_MANY` / `MANY_TO_ONE` links
 * (including transitive `STATIC_FACT -> EVENT_DYNAMIC -> CUBE_ROLLUP` chains).
 */
export function computeSubordinateTierMap(
  nodeTypes: readonly NodeTypeSpec[],
  edgeTypes: readonly EdgeTypeSpec[]
): {
  dependentsByMaster: Record<string, NodeTypeSpec[]>;
  masterByDependent: Record<string, string>;
} {
  const byName = new Map<string, NodeTypeSpec>(
    nodeTypes.map((nt) => [nt.name, nt])
  );
  const candidateMasters = new Map<string, Set<string>>();

  const recordParentLink = (parentName: string, childName: string) => {
    const parent = byName.get(parentName);
    const child = byName.get(childName);
    if (!parent || !child) return;
    if (
      child.semantic_layer_tier !== "EVENT_DYNAMIC" &&
      child.semantic_layer_tier !== "CUBE_ROLLUP"
    ) {
      return;
    }
    if (parent.semantic_layer_tier === "STATIC_FACT") {
      const set = candidateMasters.get(child.name) ?? new Set<string>();
      set.add(parent.name);
      candidateMasters.set(child.name, set);
    }
  };

  // Pass 1: Direct edges connecting `STATIC_FACT` <-> (`EVENT_DYNAMIC` | `CUBE_ROLLUP`)
  for (const edge of edgeTypes) {
    if (
      edge.cardinality === "ONE_TO_MANY" ||
      edge.cardinality === "ONE_TO_ONE"
    ) {
      recordParentLink(edge.src_type_name, edge.dst_type_name);
    }
    if (edge.cardinality === "MANY_TO_ONE") {
      recordParentLink(edge.dst_type_name, edge.src_type_name);
    }
  }

  // Pass 2: Transitive `EVENT_DYNAMIC -> CUBE_ROLLUP` inheriting unique `STATIC_FACT` master
  for (const edge of edgeTypes) {
    const pairs: Array<[string, string]> = [];
    if (
      edge.cardinality === "ONE_TO_MANY" ||
      edge.cardinality === "ONE_TO_ONE"
    ) {
      pairs.push([edge.src_type_name, edge.dst_type_name]);
    }
    if (edge.cardinality === "MANY_TO_ONE") {
      pairs.push([edge.dst_type_name, edge.src_type_name]);
    }
    for (const [midName, leafName] of pairs) {
      const mid = byName.get(midName);
      const leaf = byName.get(leafName);
      if (
        mid?.semantic_layer_tier === "EVENT_DYNAMIC" &&
        leaf?.semantic_layer_tier === "CUBE_ROLLUP"
      ) {
        const midMasters = candidateMasters.get(mid.name);
        if (midMasters && midMasters.size === 1) {
          const masterName = Array.from(midMasters)[0];
          if (masterName) {
            const leafSet = candidateMasters.get(leaf.name) ?? new Set<string>();
            leafSet.add(masterName);
            candidateMasters.set(leaf.name, leafSet);
          }
        }
      }
    }
  }

  const dependentsByMaster: Record<string, NodeTypeSpec[]> = {};
  const masterByDependent: Record<string, string> = {};

  for (const nt of nodeTypes) {
    const masters = candidateMasters.get(nt.name);
    if (!masters || masters.size !== 1) continue;
    const masterName = Array.from(masters)[0];
    if (!masterName) continue;
    masterByDependent[nt.name] = masterName;
    const list = dependentsByMaster[masterName] ?? [];
    list.push(nt);
    dependentsByMaster[masterName] = list;
  }

  return { dependentsByMaster, masterByDependent };
}
