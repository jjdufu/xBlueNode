import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { applyScopeToAskContext } from "../lib/api/analytics.ts";
import type { BlueNode } from "../lib/api/xblue.ts";
import {
  detectAtMention,
  formatAppliedScopeBadge,
  hasActiveChatScope,
  normalizeChatQueryScope,
  stripAtMention,
} from "./axquery/logic.ts";
import {
  aggregateNodesByType,
  computeNodeDegrees,
  formatGraphPatternExpression,
  parseGraphPatternExpression,
  prioritizeDstTypes,
  prioritizeEdgeTypes,
  selectTopMetricColumns,
} from "./xbluegraph/logic.ts";

describe("Step 20 AxQuery session scope & AskContext mapping", () => {
  it("normalizes ChatQueryScope and formats turn-applied-scope-badge strings", () => {
    assert.equal(normalizeChatQueryScope({}), undefined);
    assert.equal(
      normalizeChatQueryScope({
        dataset_ids: [],
        doc_ids: [],
        doc_title_prefix: "   ",
        subgraph_slug: null,
      }),
      undefined
    );
    assert.equal(hasActiveChatScope({}), false);

    const normalized = normalizeChatQueryScope({
      dataset_ids: ["ds-1", "ds-2", "ds-1"],
      subgraph_slug: " main ",
      doc_title_prefix: "",
    });
    assert.deepEqual(normalized, {
      dataset_ids: ["ds-1", "ds-2"],
      subgraph_slug: "main",
    });

    assert.equal(
      formatAppliedScopeBadge(normalized, true),
      "已限定 2 表 · 1 子图"
    );
    assert.equal(
      formatAppliedScopeBadge(normalized, false),
      "Scoped 2 tables · 1 subgraph"
    );
  });

  it("maps ChatQueryScope fields into AskContext for analyticsApi.ask", () => {
    const ctx = applyScopeToAskContext(
      { dashboard_id: "dash-1" },
      {
        dataset_ids: ["ds-a"],
        doc_ids: ["doc-1"],
        doc_title_prefix: "SOP-2026-",
        subgraph_slug: "main",
        endpoint_slugs: ["order_sla"],
      }
    );
    assert.deepEqual(ctx, {
      dashboard_id: "dash-1",
      dataset_ids: ["ds-a"],
      doc_ids: ["doc-1"],
      doc_title_prefix: "SOP-2026-",
      subgraph_slug: "main",
      endpoint_slugs: ["order_sla"],
    });
  });

  it("detects and strips @keyword mentions cleanly without matching emails", () => {
    assert.equal(detectAtMention("contact ops@company.com"), null);

    const m = detectAtMention("统计 @demo_ord 的总金额", 12);
    assert.deepEqual(m, { start: 3, end: 12, keyword: "demo_ord" });
    if (m) {
      assert.equal(stripAtMention("统计 @demo_ord 的总金额", m), "统计 的总金额");
    }
  });
});

describe("Step 20 xBlueGraph deterministic pattern parser & summary aggregations", () => {
  const knownNodes = new Set(["Supplier", "Factory", "Warehouse", "Order"]);
  const knownEdges = new Set(["SUPPLIES", "FULFILLS"]);

  it("parses (Supplier)-[SUPPLIES]->(Factory), Cypher alias syntax, and wildcards", () => {
    assert.deepEqual(
      parseGraphPatternExpression(
        "(Supplier)-[SUPPLIES]->(Factory)",
        knownNodes,
        knownEdges
      ),
      {
        ok: true,
        srcType: "Supplier",
        edgeType: "SUPPLIES",
        dstType: "Factory",
      }
    );

    assert.deepEqual(
      parseGraphPatternExpression(
        "(s:Factory)-[r:SUPPLIES]->(w:Warehouse)",
        knownNodes,
        knownEdges
      ),
      {
        ok: true,
        srcType: "Factory",
        edgeType: "SUPPLIES",
        dstType: "Warehouse",
      }
    );

    assert.deepEqual(
      parseGraphPatternExpression("()-[FULFILLS]->(Order)", knownNodes, knownEdges),
      {
        ok: true,
        srcType: "",
        edgeType: "FULFILLS",
        dstType: "Order",
      }
    );

    assert.equal(
      formatGraphPatternExpression("Supplier", "SUPPLIES", "Factory"),
      "(Supplier)-[SUPPLIES]->(Factory)"
    );
  });

  it("rejects malformed pattern expressions and unknown schema types", () => {
    const badSyntax = parseGraphPatternExpression(
      "Supplier->Factory",
      knownNodes,
      knownEdges
    );
    assert.equal(badSyntax.ok, false);
    if (!badSyntax.ok) {
      assert.equal(badSyntax.errorCode, "SYNTAX_ERROR");
    }

    const unknownNode = parseGraphPatternExpression(
      "(GhostType)-[SUPPLIES]->(Factory)",
      knownNodes,
      knownEdges
    );
    assert.equal(unknownNode.ok, false);
    if (!unknownNode.ok) {
      assert.equal(unknownNode.errorCode, "UNKNOWN_SRC_TYPE");
    }
  });

  it("prioritizes valid edge types and auto-locks unique target node type", () => {
    const edges = [
      {
        id: "e1",
        name: "SUPPLIES",
        src_type_name: "Supplier",
        dst_type_name: "Factory",
        impact_weight: 0.8,
      },
      {
        id: "e2",
        name: "FULFILLS",
        src_type_name: "Warehouse",
        dst_type_name: "Order",
        impact_weight: 0.9,
      },
    ];

    const edgePrio = prioritizeEdgeTypes(edges, [], "Supplier");
    assert.deepEqual(edgePrio, {
      valid: ["SUPPLIES"],
      others: ["FULFILLS"],
    });

    const dstPrio = prioritizeDstTypes(
      ["Supplier", "Factory", "Warehouse", "Order"],
      edges,
      "Supplier",
      "SUPPLIES"
    );
    assert.deepEqual(dstPrio.valid, ["Factory"]);
    assert.equal(dstPrio.autoLockDst, "Factory");
  });

  it("aggregates summary facets, degrees, and top 3 numeric metric columns", () => {
    const nodes: BlueNode[] = [
      {
        id: "n1",
        type_name: "Warehouse",
        node_key: "W-1",
        title: "华东仓",
        metrics: { inventory: 400, capacity: 800, demand: 200, extra: 1 },
      },
      {
        id: "n2",
        type_name: "Warehouse",
        node_key: "W-2",
        title: "华南仓",
        metrics: { inventory: 600, capacity: 1000, demand: 300 },
      },
      {
        id: "n3",
        type_name: "Factory",
        node_key: "F-1",
        title: "昆山厂",
        metrics: { output: 1200 },
      },
    ];
    const links = [{ src_node_id: "n3", dst_node_id: "n1" }];

    const facets = aggregateNodesByType(nodes);
    assert.equal(facets[0]?.typeName, "Warehouse");
    assert.equal(facets[0]?.count, 2);
    assert.equal(facets[0]?.topMetric, "capacity");
    assert.equal(facets[0]?.topMetricMean, 900);

    const degs = computeNodeDegrees(nodes, links);
    assert.equal(degs.get("n3"), 1);
    assert.equal(degs.get("n1"), 1);
    assert.equal(degs.get("n2"), 0);

    const cols = selectTopMetricColumns(nodes, 3);
    assert.deepEqual(cols, ["capacity", "demand", "inventory"]);
  });
});
