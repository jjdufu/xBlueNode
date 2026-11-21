import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  buildAskFilterSpecs,
  buildWidgetFilters,
  classifyPivotRow,
  detectImplicitNodeMetricGrouping,
  parseGraphTopologyData,
  parseNodesGeoData,
  resolveActionTargetNodeId,
  resolveLeafDrillNodeKey,
  sanitizeOptionalUuid,
  type LogicFilterDef,
} from "./logic.ts";

describe("xBlueView contract logic", () => {
  it("buildWidgetFilters preserves FilterDef.key instead of mapped column and drops undefined keys", () => {
    const filters: LogicFilterDef[] = [
      {
        key: "order_status_flt",
        label: "订单状态",
        kind: "SELECT",
        field_map: {
          "widget-bar": "status",
        },
      },
      {
        key: "date_window",
        label: "下单日期",
        kind: "DATE_RANGE",
        field_map: {
          "widget-bar": "created_date",
        },
      },
    ];

    const values: Record<string, unknown> = {
      order_status_flt: "PAID",
      date_window: ["2026-01-01", null],
      unmapped_ad_hoc_key: "SHOULD_NOT_PASS",
    };

    const barOut = buildWidgetFilters("widget-bar", filters, values);
    assert.deepEqual(barOut, {
      order_status_flt: "PAID",
      date_window: ["2026-01-01", null],
    });
    assert.equal("status" in barOut, false);
    assert.equal("unmapped_ad_hoc_key" in barOut, false);

    const kpiOut = buildWidgetFilters("widget-kpi-unmapped", filters, values);
    assert.deepEqual(kpiOut, {});
  });

  it("buildAskFilterSpecs maps FilterKind to Between/In/Like/Eq and resolves field_map columns", () => {
    const filters: LogicFilterDef[] = [
      {
        key: "dt",
        label: "日期区间",
        kind: "DATE_RANGE",
        field_map: { w1: "order_date" },
      },
      {
        key: "amt_range",
        label: "金额范围",
        kind: "NUMBER_RANGE",
        field_map: { w1: "amount" },
      },
      {
        key: "statuses",
        label: "多状态",
        kind: "MULTI_SELECT",
        field_map: { w1: "status" },
      },
      {
        key: "keyword",
        label: "客户搜索",
        kind: "TEXT",
        field_map: { w1: "customer_name" },
      },
      {
        key: "wh",
        label: "仓库",
        kind: "SELECT",
        field_map: {},
      },
    ];

    const specs = buildAskFilterSpecs(filters, {
      dt: ["2026-01-01", null],
      amt_range: [100, 5000],
      statuses: ["PAID", "SHIPPED"],
      keyword: "华东",
      wh: "W-1",
    });

    assert.deepEqual(specs, [
      { field: "order_date", op: "Between", value: ["2026-01-01", null] },
      { field: "amount", op: "Between", value: [100, 5000] },
      { field: "status", op: "In", value: ["PAID", "SHIPPED"] },
      { field: "customer_name", op: "Like", value: "%华东%" },
      { field: "wh", op: "Eq", value: "W-1" },
    ]);
  });

  it("parseGraphTopologyData parses 10-column GRAPH record_kind NODE/LINK and 3-column preview tables", () => {
    const tenColData = {
      columns: [
        { name: "record_kind", data_type: "STRING" },
        { name: "id", data_type: "STRING" },
        { name: "type_name", data_type: "STRING" },
        { name: "node_key", data_type: "STRING" },
        { name: "title", data_type: "STRING" },
        { name: "metric_value", data_type: "FLOAT64" },
        { name: "src_node_id", data_type: "STRING" },
        { name: "dst_node_id", data_type: "STRING" },
        { name: "link_type", data_type: "STRING" },
        { name: "weight", data_type: "FLOAT64" },
      ],
      rows: [
        ["NODE", "n-factory", "Factory", "F-1", "苏州工厂", 1200, null, null, null, null],
        ["NODE", "n-wh", "Warehouse", "W-1", "", 500, null, null, null, null],
        ["LINK", "l-1", null, null, null, null, "n-factory", "n-wh", "SUPPLIES", 0.85],
      ],
    };

    const parsed = parseGraphTopologyData(tenColData);
    assert.equal(parsed.nodes.length, 2);
    assert.deepEqual(parsed.nodes[0], {
      id: "n-factory",
      name: "苏州工厂",
      category: "Factory",
      value: 1200,
    });
    assert.deepEqual(parsed.nodes[1], {
      id: "n-wh",
      name: "W-1",
      category: "Warehouse",
      value: 500,
    });
    assert.deepEqual(parsed.links, [
      {
        source: "n-factory",
        target: "n-wh",
        value: 0.85,
        label: "SUPPLIES",
      },
    ]);

    const threeColPreview = parseGraphTopologyData({
      columns: [
        { name: "source", data_type: "STRING" },
        { name: "target", data_type: "STRING" },
        { name: "value", data_type: "FLOAT64" },
      ],
      rows: [["F-1", "W-1", 42]],
    });
    assert.equal(threeColPreview.nodes.length, 2);
    assert.equal(threeColPreview.links.length, 1);
    assert.equal(threeColPreview.links[0]?.value, 42);
  });

  it("parseNodesGeoData separates NODE scatter points and LINK fly-lines", () => {
    const geoData = {
      columns: [
        { name: "node_id", data_type: "STRING" },
        { name: "type_name", data_type: "STRING" },
        { name: "node_key", data_type: "STRING" },
        { name: "title", data_type: "STRING" },
        { name: "lng", data_type: "FLOAT64" },
        { name: "lat", data_type: "FLOAT64" },
        { name: "metric_value", data_type: "FLOAT64" },
        { name: "open_alerts", data_type: "INT64" },
        { name: "record_kind", data_type: "STRING" },
        { name: "link_type", data_type: "STRING" },
        { name: "src_lng", data_type: "FLOAT64" },
        { name: "src_lat", data_type: "FLOAT64" },
        { name: "dst_lng", data_type: "FLOAT64" },
        { name: "dst_lat", data_type: "FLOAT64" },
        { name: "weight", data_type: "FLOAT64" },
      ],
      rows: [
        [
          "n1",
          "Factory",
          "F-1",
          "苏州厂",
          120.62,
          31.3,
          800,
          0,
          "NODE",
          null,
          null,
          null,
          null,
          null,
          null,
        ],
        [
          "n2",
          "Warehouse",
          "W-1",
          "上海仓",
          121.47,
          31.23,
          500,
          1,
          "NODE",
          null,
          null,
          null,
          null,
          null,
          null,
        ],
        [
          "l1",
          "SUPPLIES",
          null,
          null,
          null,
          null,
          null,
          null,
          "LINK",
          "SUPPLIES",
          120.62,
          31.3,
          121.47,
          31.23,
          1.5,
        ],
      ],
    };

    const { points, flyLines } = parseNodesGeoData(geoData);
    assert.equal(points.length, 2);
    assert.equal(points[0]?.nodeKey, "F-1");
    assert.equal(points[1]?.openAlerts, 1);
    assert.equal(flyLines.length, 1);
    assert.deepEqual(flyLines[0], {
      linkType: "SUPPLIES",
      srcLng: 120.62,
      srcLat: 31.3,
      dstLng: 121.47,
      dstLat: 31.23,
      weight: 1.5,
    });
  });

  it("classifyPivotRow identifies _row_kind SUBTOTAL/TOTAL and fallback Chinese/English labels", () => {
    const colsWithRowKind = [
      { name: "region", data_type: "STRING" },
      { name: "amt", data_type: "DECIMAL" },
      { name: "_row_kind", data_type: "STRING" },
    ];
    assert.equal(
      classifyPivotRow(colsWithRowKind, ["华东", 100, "DETAIL"]),
      "DETAIL"
    );
    assert.equal(
      classifyPivotRow(colsWithRowKind, ["华东", 300, "SUBTOTAL"]),
      "SUBTOTAL"
    );
    assert.equal(
      classifyPivotRow(colsWithRowKind, ["", 900, "TOTAL"]),
      "TOTAL"
    );

    const plainCols = [
      { name: "region", data_type: "STRING" },
      { name: "warehouse", data_type: "STRING" },
      { name: "amt", data_type: "DECIMAL" },
    ];
    assert.equal(classifyPivotRow(plainCols, ["华东", "小计", 300]), "SUBTOTAL");
    assert.equal(classifyPivotRow(plainCols, ["总计", "", 900]), "TOTAL");
    assert.equal(classifyPivotRow(plainCols, ["合计", "", 900]), "TOTAL");
    assert.equal(classifyPivotRow(plainCols, ["华东", "SUBTOTAL", 300]), "SUBTOTAL");
  });

  it("sanitizeOptionalUuid & resolveActionTargetNodeId strip empty string UUIDs", () => {
    assert.equal(sanitizeOptionalUuid(""), undefined);
    assert.equal(sanitizeOptionalUuid("   "), undefined);
    assert.equal(sanitizeOptionalUuid(null), undefined);
    assert.equal(
      sanitizeOptionalUuid("00000000-0000-0000-0000-000000000301"),
      "00000000-0000-0000-0000-000000000301"
    );

    assert.equal(resolveActionTargetNodeId("", ""), undefined);
    assert.equal(
      resolveActionTargetNodeId("  ", "00000000-0000-0000-0000-000000000301"),
      "00000000-0000-0000-0000-000000000301"
    );
  });

  it("resolveLeafDrillNodeKey & detectImplicitNodeMetricGrouping handle leaf drill and group_by=node", () => {
    const key = resolveLeafDrillNodeKey({
      clickedField: "warehouse_name",
      clickedValue: "上海一号仓",
      levelField: "warehouse_name",
      nodeKeyField: "warehouse_code",
      widgetData: {
        columns: [
          { name: "warehouse_name", data_type: "STRING" },
          { name: "warehouse_code", data_type: "STRING" },
          { name: "amt", data_type: "DECIMAL" },
        ],
        rows: [["上海一号仓", "W-1", 8200]],
      },
    });
    assert.equal(key, "W-1");

    const grouped = detectImplicitNodeMetricGrouping(
      {
        columns: [
          { name: "bucket_start", data_type: "TIMESTAMP" },
          { name: "group_key", data_type: "STRING" },
          { name: "value", data_type: "FLOAT64" },
        ],
        rows: [
          ["2026-01-31T08:00:00Z", "W-1", 500],
          ["2026-01-31T08:00:00Z", "W-2", 320],
        ],
      },
      undefined
    );
    assert.deepEqual(grouped, {
      xField: "bucket_start",
      seriesField: "group_key",
      yField: "value",
    });
  });
});
