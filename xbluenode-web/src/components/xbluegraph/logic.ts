import type {
  BlueEdgeTypeDef,
  BlueNode,
} from "../../lib/api/xblue.ts";

export interface ParsedGraphPattern {
  readonly ok: true;
  readonly srcType: string;
  readonly edgeType: string;
  readonly dstType: string;
}

export interface GraphPatternParseFailure {
  readonly ok: false;
  readonly errorCode:
    | "SYNTAX_ERROR"
    | "UNKNOWN_SRC_TYPE"
    | "UNKNOWN_EDGE_TYPE"
    | "UNKNOWN_DST_TYPE";
  readonly message: string;
}

export type GraphPatternParseResult =
  | ParsedGraphPattern
  | GraphPatternParseFailure;

const IDENT_RE = /^[A-Za-z_\u4e00-\u9fff][A-Za-z0-9_\u4e00-\u9fff]*$/;

function parseSlotToken(raw: string): string | null {
  const trimmed = raw.trim();
  if (trimmed === "" || trimmed === "*") {
    return "";
  }
  const colonIdx = trimmed.indexOf(":");
  if (colonIdx >= 0) {
    const alias = trimmed.slice(0, colonIdx).trim();
    const typeName = trimmed.slice(colonIdx + 1).trim();
    if (alias !== "" && !IDENT_RE.test(alias)) {
      return null;
    }
    if (typeName === "" || typeName === "*") {
      return "";
    }
    return IDENT_RE.test(typeName) ? typeName : null;
  }
  return IDENT_RE.test(trimmed) ? trimmed : null;
}

type ParserState =
  | "EXPECT_SRC_OPEN"
  | "READ_SRC_BODY"
  | "EXPECT_DASH_1"
  | "AFTER_DASH_1"
  | "READ_EDGE_BODY"
  | "EXPECT_DASH_2"
  | "EXPECT_GT"
  | "EXPECT_DST_OPEN"
  | "READ_DST_BODY"
  | "EXPECT_END";

function isSpace(ch: string): boolean {
  return ch === " " || ch === "\t";
}

/**
 * Deterministic finite-state parser for `(src:NodeType)-[r:EdgeType]->(dst:NodeType)`
 * and `(Supplier)-[SUPPLIES]->(Factory)` pattern expressions.
 */
export function parseGraphPatternExpression(
  input: string,
  knownNodeTypes?: ReadonlySet<string>,
  knownEdgeTypes?: ReadonlySet<string>
): GraphPatternParseResult {
  const s = input.trim();
  if (s.length === 0) {
    return { ok: true, srcType: "", edgeType: "", dstType: "" };
  }

  let state: ParserState = "EXPECT_SRC_OPEN";
  let srcBuf = "";
  let edgeBuf = "";
  let dstBuf = "";

  for (let i = 0; i < s.length; i += 1) {
    const ch = s[i] ?? "";
    switch (state) {
      case "EXPECT_SRC_OPEN":
        if (isSpace(ch)) break;
        if (ch !== "(") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        state = "READ_SRC_BODY";
        break;

      case "READ_SRC_BODY":
        if (ch === ")") {
          state = "EXPECT_DASH_1";
        } else if (ch === "(" || ch === "[" || ch === "]" || ch === "\n" || ch === "\r") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        } else {
          srcBuf += ch;
        }
        break;

      case "EXPECT_DASH_1":
        if (isSpace(ch)) break;
        if (ch !== "-") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        state = "AFTER_DASH_1";
        break;

      case "AFTER_DASH_1":
        if (isSpace(ch)) break;
        if (ch === "[") {
          state = "READ_EDGE_BODY";
        } else if (ch === "-") {
          state = "EXPECT_GT";
        } else if (ch === ">") {
          state = "EXPECT_DST_OPEN";
        } else {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        break;

      case "READ_EDGE_BODY":
        if (ch === "]") {
          state = "EXPECT_DASH_2";
        } else if (ch === "[" || ch === "(" || ch === ")" || ch === "\n" || ch === "\r") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        } else {
          edgeBuf += ch;
        }
        break;

      case "EXPECT_DASH_2":
        if (isSpace(ch)) break;
        if (ch !== "-") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        state = "EXPECT_GT";
        break;

      case "EXPECT_GT":
        if (ch !== ">") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        state = "EXPECT_DST_OPEN";
        break;

      case "EXPECT_DST_OPEN":
        if (isSpace(ch)) break;
        if (ch !== "(") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        state = "READ_DST_BODY";
        break;

      case "READ_DST_BODY":
        if (ch === ")") {
          state = "EXPECT_END";
        } else if (ch === "(" || ch === "[" || ch === "]" || ch === "\n" || ch === "\r") {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        } else {
          dstBuf += ch;
        }
        break;

      case "EXPECT_END":
        if (!isSpace(ch)) {
          return {
            ok: false,
            errorCode: "SYNTAX_ERROR",
            message: "(Supplier)-[SUPPLIES]->(Factory)",
          };
        }
        break;
    }
  }

  if (state !== "EXPECT_END") {
    return {
      ok: false,
      errorCode: "SYNTAX_ERROR",
      message: "(Supplier)-[SUPPLIES]->(Factory)",
    };
  }

  const srcType = parseSlotToken(srcBuf);
  const edgeType = parseSlotToken(edgeBuf);
  const dstType = parseSlotToken(dstBuf);

  if (srcType === null || edgeType === null || dstType === null) {
    return {
      ok: false,
      errorCode: "SYNTAX_ERROR",
      message: "(Supplier)-[SUPPLIES]->(Factory)",
    };
  }

  if (knownNodeTypes && knownNodeTypes.size > 0) {
    if (srcType !== "" && !knownNodeTypes.has(srcType)) {
      return {
        ok: false,
        errorCode: "UNKNOWN_SRC_TYPE",
        message: srcType,
      };
    }
    if (dstType !== "" && !knownNodeTypes.has(dstType)) {
      return {
        ok: false,
        errorCode: "UNKNOWN_DST_TYPE",
        message: dstType,
      };
    }
  }

  if (knownEdgeTypes && knownEdgeTypes.size > 0) {
    if (edgeType !== "" && !knownEdgeTypes.has(edgeType)) {
      return {
        ok: false,
        errorCode: "UNKNOWN_EDGE_TYPE",
        message: edgeType,
      };
    }
  }

  return {
    ok: true,
    srcType,
    edgeType,
    dstType,
  };
}

export function formatGraphPatternExpression(
  srcType: string,
  edgeType: string,
  dstType: string
): string {
  if (!srcType && !edgeType && !dstType) {
    return "";
  }
  return `(${srcType})-[${edgeType}]->(${dstType})`;
}

export function prioritizeEdgeTypes(
  edgeTypes: readonly BlueEdgeTypeDef[],
  fallbackEdgeNames: readonly string[],
  srcType: string
): { valid: string[]; others: string[] } {
  const validSet = new Set<string>();
  const allSet = new Set<string>();

  for (const et of edgeTypes) {
    if (!et.name) continue;
    allSet.add(et.name);
    if (srcType && et.src_type_name === srcType) {
      validSet.add(et.name);
    }
  }
  for (const name of fallbackEdgeNames) {
    if (name) allSet.add(name);
  }

  const valid = Array.from(validSet).sort();
  const others = Array.from(allSet)
    .filter((name) => !validSet.has(name))
    .sort();
  return { valid, others };
}

export function prioritizeDstTypes(
  allNodeNames: readonly string[],
  edgeTypes: readonly BlueEdgeTypeDef[],
  srcType: string,
  edgeType: string
): { valid: string[]; others: string[]; autoLockDst: string | null } {
  const validSet = new Set<string>();
  for (const et of edgeTypes) {
    if (!et.dst_type_name) continue;
    const matchesEdge = !edgeType || et.name === edgeType;
    const matchesSrc = !srcType || et.src_type_name === srcType;
    if ((edgeType || srcType) && matchesEdge && matchesSrc) {
      validSet.add(et.dst_type_name);
    }
  }

  const allSet = new Set<string>(allNodeNames);
  for (const d of validSet) {
    allSet.add(d);
  }

  const valid = Array.from(validSet).sort();
  const others = Array.from(allSet)
    .filter((n) => !validSet.has(n))
    .sort();
  const autoLockDst = edgeType && valid.length === 1 ? (valid[0] ?? null) : null;
  return { valid, others, autoLockDst };
}

export interface TypeFacetSummary {
  readonly typeName: string;
  readonly count: number;
  readonly topMetric: string | null;
  readonly topMetricMean: number | null;
}

export function aggregateNodesByType(
  nodes: readonly BlueNode[]
): TypeFacetSummary[] {
  const byType = new Map<string, BlueNode[]>();
  for (const n of nodes) {
    const t = n.type_name || "Node";
    const bucket = byType.get(t);
    if (bucket) {
      bucket.push(n);
    } else {
      byType.set(t, [n]);
    }
  }

  const out: TypeFacetSummary[] = [];
  for (const [typeName, group] of byType.entries()) {
    const metricTotals = new Map<string, { sum: number; cnt: number }>();
    for (const node of group) {
      for (const [k, v] of Object.entries(node.metrics ?? {})) {
        if (typeof v === "number" && Number.isFinite(v)) {
          const prev = metricTotals.get(k);
          if (prev) {
            prev.sum += v;
            prev.cnt += 1;
          } else {
            metricTotals.set(k, { sum: v, cnt: 1 });
          }
        }
      }
    }

    let bestMetric: string | null = null;
    let bestMean: number | null = null;
    let bestCnt = 0;
    for (const [mKey, stat] of Array.from(metricTotals.entries()).sort((a, b) =>
      a[0].localeCompare(b[0])
    )) {
      if (stat.cnt > bestCnt) {
        bestCnt = stat.cnt;
        bestMetric = mKey;
        bestMean = stat.sum / stat.cnt;
      }
    }

    out.push({
      typeName,
      count: group.length,
      topMetric: bestMetric,
      topMetricMean: bestMean,
    });
  }

  return out.sort((a, b) => b.count - a.count || a.typeName.localeCompare(b.typeName));
}

export function computeNodeDegrees(
  nodes: readonly BlueNode[],
  edges: readonly { src_node_id: string; dst_node_id: string }[]
): Map<string, number> {
  const degrees = new Map<string, number>();
  for (const n of nodes) {
    degrees.set(n.id, 0);
  }
  for (const e of edges) {
    if (degrees.has(e.src_node_id)) {
      degrees.set(e.src_node_id, (degrees.get(e.src_node_id) ?? 0) + 1);
    }
    if (degrees.has(e.dst_node_id)) {
      degrees.set(e.dst_node_id, (degrees.get(e.dst_node_id) ?? 0) + 1);
    }
  }
  return degrees;
}

export function selectTopMetricColumns(
  nodes: readonly BlueNode[],
  maxCols = 3
): string[] {
  const freq = new Map<string, number>();
  for (const n of nodes) {
    for (const [k, v] of Object.entries(n.metrics ?? {})) {
      if (typeof v === "number" && Number.isFinite(v)) {
        freq.set(k, (freq.get(k) ?? 0) + 1);
      }
    }
  }
  return Array.from(freq.entries())
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, Math.max(0, maxCols))
    .map(([k]) => k);
}
