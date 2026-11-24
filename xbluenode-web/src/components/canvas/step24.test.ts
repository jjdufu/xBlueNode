import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  findClosestEdgeWithinThreshold,
  spliceNodeIntoFlowEdge,
} from "../axflow/edgeSplice.ts";
import { computeLineageClosure } from "../axhub/lineageClosure.ts";
import {
  buildSimComboGroups,
  computeHighImpactHullClusters,
} from "../axsim/causalHull.ts";
import type { SandboxGraphNode } from "../../lib/api/axsim.ts";
import { computeSubordinateTierMap } from "../xbluestudio/tierFolding.ts";
import {
  computeAlignmentGuides,
  computeSelectionAlignment,
  type MeasuredBox,
} from "./alignmentGuides.ts";
import {
  allocateMultiPorts,
  computeCollisionFreeEdgeLabels,
  computeElkOrthogonalLayout,
  rectIntersectionArea,
} from "./elkOrthogonalLayout.ts";
import { easeOutCubic } from "./useAnimatedNodes.ts";
import { createCanvasHistoryStore } from "./useCanvasHistory.ts";

describe("Step 24 6-axis magnetic alignment & multi-select distribution", () => {
  const anchorA: MeasuredBox = {
    id: "A",
    x: 100,
    y: 120,
    width: 240,
    height: 180,
  };
  const anchorB: MeasuredBox = {
    id: "B",
    x: 520,
    y: 360,
    width: 240,
    height: 180,
  };

  it("snaps within 6px across left/centerX/right and top/centerY/bottom axes", () => {
    // Dragged left at 104 (4px from A.left=100) and bottom at 537 (3px from B.bottom=540)
    const dragged: MeasuredBox = {
      id: "D",
      x: 104,
      y: 357,
      width: 240,
      height: 180,
    };
    const snap = computeAlignmentGuides(dragged, [anchorA, anchorB], 6);
    assert.equal(snap.snappedX, 100);
    assert.equal(snap.snappedY, 360);
    assert.ok(snap.verticalLines.length >= 1);
    assert.ok(snap.horizontalLines.length >= 1);
    assert.equal(snap.verticalLines[0]?.y1, 120);
    assert.equal(snap.verticalLines[0]?.y2, 540);
  });

  it("does not snap when distance exceeds thresholdPx", () => {
    const far: MeasuredBox = {
      id: "D",
      x: 109,
      y: 129,
      width: 240,
      height: 180,
    };
    const snap = computeAlignmentGuides(far, [anchorA], 6);
    assert.equal(snap.snappedX, null);
    assert.equal(snap.snappedY, null);
    assert.equal(snap.verticalLines.length, 0);
    assert.equal(snap.horizontalLines.length, 0);
  });

  it("aligns and evenly distributes multi-selected boxes", () => {
    const b1: MeasuredBox = { id: "n1", x: 100, y: 80, width: 100, height: 60 };
    const b2: MeasuredBox = { id: "n2", x: 260, y: 190, width: 100, height: 60 };
    const b3: MeasuredBox = { id: "n3", x: 600, y: 140, width: 100, height: 60 };

    const topAligned = computeSelectionAlignment([b1, b2, b3], "top");
    assert.equal(topAligned.n1?.y, 80);
    assert.equal(topAligned.n2?.y, 80);
    assert.equal(topAligned.n3?.y, 80);

    const distX = computeSelectionAlignment([b1, b2, b3], "distributeX");
    // Span = [100, 700] = 600, total width = 300 -> gap = 150 -> x = 100, 350, 600
    assert.equal(distX.n1?.x, 100);
    assert.equal(distX.n2?.x, 350);
    assert.equal(distX.n3?.x, 600);
  });
});

describe("Step 24 multi-port ELK orthogonal layout & zero-overlap label solver", () => {
  it("allocates independent WEST/EAST ports and guarantees 0 label-card overlap", async () => {
    const nodes = [
      { id: "Supplier", width: 240, height: 188, domainGroup: "采购域" },
      { id: "Warehouse", width: 240, height: 196, domainGroup: "履约域" },
      { id: "Order", width: 240, height: 188, domainGroup: "履约域" },
    ];
    const edges = [
      { id: "e0", source: "Supplier", target: "Warehouse" },
      { id: "e1", source: "Warehouse", target: "Order" },
      { id: "e2", source: "Supplier", target: "Order" },
    ];

    const { portsByNode, edgeBindings } = allocateMultiPorts(nodes, edges);
    assert.equal(portsByNode.Supplier?.outPorts.length, 2);
    assert.equal(portsByNode.Order?.inPorts.length, 2);
    assert.notEqual(
      edgeBindings.e0?.sourcePortId,
      edgeBindings.e2?.sourcePortId
    );

    const layoutRes = await computeElkOrthogonalLayout(nodes, edges, {
      groupByDomain: true,
    });
    assert.ok(layoutRes.nodePositions.Supplier);
    assert.ok(layoutRes.nodePositions.Warehouse);
    assert.ok(layoutRes.nodePositions.Order);
    assert.ok(layoutRes.domainBoxes["履约域"]);

    // Place an obstacle directly at the midpoint between Supplier and Order and verify 0 overlap
    const obstacles: MeasuredBox[] = [
      { id: "Supplier", x: 100, y: 120, width: 240, height: 188 },
      { id: "Warehouse", x: 460, y: 120, width: 240, height: 196 },
      { id: "Order", x: 820, y: 120, width: 240, height: 188 },
    ];
    const labels = computeCollisionFreeEdgeLabels(
      [
        {
          id: "e0",
          sourceX: 340,
          sourceY: 200,
          targetX: 460,
          targetY: 200,
          pillWidth: 112,
          pillHeight: 26,
        },
        {
          id: "e2",
          sourceX: 340,
          sourceY: 210,
          targetX: 820,
          targetY: 210,
          pillWidth: 120,
          pillHeight: 26,
        },
      ],
      obstacles,
      10
    );

    for (const [eid, center] of Object.entries(labels)) {
      const pillBox: MeasuredBox = {
        id: eid,
        x: center.x - 60,
        y: center.y - 13,
        width: 120,
        height: 26,
      };
      for (const obs of obstacles) {
        assert.equal(rectIntersectionArea(pillBox, obs), 0);
      }
    }
  });

  it("evaluates easeOutCubic endpoints accurately", () => {
    assert.equal(easeOutCubic(0), 0);
    assert.equal(easeOutCubic(1), 1);
    assert.ok(easeOutCubic(0.5) > 0.8);
  });
});

describe("Step 24 zundo 50-step canvas history store", () => {
  it("records, undoes, redoes, and caps history at limit=50", () => {
    const history = createCanvasHistoryStore<number>(0, 50);
    for (let i = 1; i <= 55; i += 1) {
      history.pushSnapshot(i);
    }
    assert.equal(history.getDoc(), 55);
    assert.equal(history.pastCount(), 50);

    const prev = history.undo();
    assert.equal(prev, 54);
    assert.equal(history.canRedo(), true);

    const restored = history.redo();
    assert.equal(restored, 55);

    history.resetHistory(10);
    assert.equal(history.getDoc(), 10);
    assert.equal(history.canUndo(), false);
    assert.equal(history.canRedo(), false);
  });
});

describe("Step 24 Studio 3-tier folding, AxFlow edge splice, and G6 causal/lineage helpers", () => {
  it("groups EVENT_DYNAMIC and transitive CUBE_ROLLUP under unique STATIC_FACT master", () => {
    const makeSpec = (
      name: string,
      tier: "STATIC_FACT" | "EVENT_DYNAMIC" | "CUBE_ROLLUP"
    ) => ({
      name,
      display_name: name,
      description: "",
      icon: "box",
      color: "#2563eb",
      title_template: "{node_key}",
      properties: [],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 0,
      pos_y: 0,
      semantic_layer_tier: tier,
    });

    const { dependentsByMaster } = computeSubordinateTierMap(
      [
        makeSpec("Warehouse", "STATIC_FACT"),
        makeSpec("OutboundEvent", "EVENT_DYNAMIC"),
        makeSpec("DailyOutboundCube", "CUBE_ROLLUP"),
      ],
      [
        {
          name: "EMITS_EVENT",
          src_type_name: "Warehouse",
          dst_type_name: "OutboundEvent",
          impact_weight: 0.8,
          cardinality: "ONE_TO_MANY",
          description: "",
          properties: [],
          propagation: {},
        },
        {
          name: "ROLLS_UP_TO",
          src_type_name: "OutboundEvent",
          dst_type_name: "DailyOutboundCube",
          impact_weight: 0.9,
          cardinality: "ONE_TO_MANY",
          description: "",
          properties: [],
          propagation: {},
        },
      ]
    );

    assert.deepEqual(
      dependentsByMaster.Warehouse?.map((d) => d.name),
      ["OutboundEvent", "DailyOutboundCube"]
    );
  });

  it("splices operator onto edge preserving guard `when` and detects <18px proximity", () => {
    const baseNode = (id: string, x: number) => ({
      id,
      kind: "SQL_TRANSFORM" as const,
      label: id,
      config: {},
      retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" as const },
      timeout_s: 60,
      on_failure: "FAIL_RUN" as const,
      join: "ALL_SUCCESS" as const,
      pos: { x, y: 144 },
    });

    const nearIdx = findClosestEdgeWithinThreshold(
      { x: 350, y: 196 },
      [{ edgeIndex: 0, sourceX: 304, sourceY: 202, targetX: 448, targetY: 202 }],
      18
    );
    assert.equal(nearIdx, 0);

    const spliced = spliceNodeIntoFlowEdge(
      [baseNode("check", 80), baseNode("approve", 400)],
      [{ from: "check", to: "approve", when: "n > 0" }],
      0,
      baseNode("http_call_1", 240)
    );
    assert.equal(spliced.nodes.length, 3);
    assert.deepEqual(spliced.edges, [
      { from: "check", to: "http_call_1", when: "n > 0" },
      { from: "http_call_1", to: "approve", when: null },
    ]);
  });

  it("detects >=15% high-impact hull clusters and upstream/downstream lineage closure", () => {
    const simNodes: SandboxGraphNode[] = [
      {
        id: "f1",
        type_name: "Factory",
        node_key: "F-1",
        title: "工厂",
        metrics: { output: 1000 },
        properties: { domain_group: "制造域" },
        is_modified: false,
        is_new: false,
      },
      {
        id: "w1",
        type_name: "Warehouse",
        node_key: "W-1",
        title: "中心仓",
        metrics: { inventory: 500 },
        properties: { domain_group: "履约域" },
        is_modified: false,
        is_new: false,
      },
      {
        id: "o1",
        type_name: "Order",
        node_key: "O-1",
        title: "订单",
        metrics: { fulfillment: 100 },
        properties: { domain_group: "履约域" },
        is_modified: false,
        is_new: false,
      },
    ];
    const combos = buildSimComboGroups(simNodes);
    assert.equal(combos.length, 2);

    const hulls = computeHighImpactHullClusters(
      simNodes,
      [
        {
          id: "l1",
          link_type: "SUPPLIES",
          src_node_id: "f1",
          dst_node_id: "w1",
          weight: 0.8,
          is_modified: false,
          is_new: false,
        },
        {
          id: "l2",
          link_type: "FULFILLS",
          src_node_id: "w1",
          dst_node_id: "o1",
          weight: 0.9,
          is_modified: false,
          is_new: false,
        },
      ],
      [
        {
          node_id: "w1",
          type_name: "Warehouse",
          title: "中心仓",
          metric: "inventory",
          before_val: 500,
          after_val: 380,
          delta: -120,
          delta_ratio: -0.24,
          hop_distance: 1,
          top_path: ["f1", "w1"],
        },
        {
          node_id: "o1",
          type_name: "Order",
          title: "订单",
          metric: "fulfillment",
          before_val: 100,
          after_val: 78.4,
          delta: -21.6,
          delta_ratio: -0.216,
          hop_distance: 2,
          top_path: ["f1", "w1", "o1"],
        },
      ],
      [{ target_node_id: "f1", metric_key: "output", mode: "RATIO", value: -0.3 }]
    );
    assert.equal(hulls.length, 1);
    assert.equal(hulls[0]?.memberIds.length, 3);

    const closure = computeLineageClosure("DATASET:ds1", [
      { id: "e1", source: "CONNECTION:c1", target: "DATASET:ds1" },
      { id: "e2", source: "DATASET:ds1", target: "NODE_TYPE:Order" },
      { id: "e3", source: "NODE_TYPE:Order", target: "DASHBOARD:d1" },
      { id: "e4", source: "CONNECTION:c2", target: "DATASET:ds2" },
    ]);
    assert.equal(closure.nodeIds.has("CONNECTION:c1"), true);
    assert.equal(closure.nodeIds.has("DASHBOARD:d1"), true);
    assert.equal(closure.nodeIds.has("DATASET:ds2"), false);
  });
});
