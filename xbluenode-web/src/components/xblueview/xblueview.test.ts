import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  buildAskFilterSpecs,
  buildEChartsOption,
  buildWidgetFilters,
  chartSpec,
  classifyPivotRow,
  computeMagneticSnapGuides,
  containsFunctionValue,
  detectImplicitNodeMetricGrouping,
  formatAxisTimestampLabel,
  mergeLockedShareFilters,
  parseGraphTopologyData,
  parseNodesGeoData,
  resolveActionTargetNodeId,
  resolveFreeFloatLayout,
  resolveLeafDrillNodeKey,
  sanitizeExportFilters,
  sanitizeOptionalUuid,
  tidyDashboardLayout,
  type GridLayoutBox,
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

  it("buildEChartsOption applies chartSpec light/dark palettes, Chinese labels, pie/gauge geometry, and encodingFallbacks", () => {
    const barData = {
      columns: [
        { name: "region", data_type: "STRING" },
        { name: "total_amount", data_type: "DECIMAL" },
      ],
      rows: [
        ["华东战区", "12800.50"],
        ["华南战区", "9400.00"],
      ],
    };

    const lightBar = buildEChartsOption({
      kind: "BAR_CHART",
      title: "战区订单营收",
      encoding: {},
      data: barData,
      isDark: false,
    });
    assert.equal(lightBar.clickField, "region");
    assert.deepEqual(lightBar.option.color, chartSpec.palette.light);
    assert.deepEqual(lightBar.option.grid, chartSpec.grid);
    assert.deepEqual(lightBar.option.textStyle, {
      fontFamily: chartSpec.fontFamily,
      color: chartSpec.tokens.light.text,
    });
    const xAxisLight = lightBar.option.xAxis as Record<string, unknown>;
    assert.deepEqual(xAxisLight.data, ["华东战区", "华南战区"]);

    const darkPie = buildEChartsOption({
      kind: "PIE_CHART",
      title: "订单结构占比",
      encoding: {},
      data: {
        columns: [
          { name: "status", data_type: "STRING" },
          { name: "amt", data_type: "DECIMAL" },
        ],
        rows: [
          ["已支付订单", 8200],
          ["待发货订单", 1800],
        ],
      },
      isDark: true,
    });
    assert.equal(darkPie.clickField, "status");
    assert.deepEqual(darkPie.option.color, chartSpec.palette.dark);
    const pieSeries = (darkPie.option.series as Record<string, unknown>[])[0];
    assert.deepEqual(pieSeries?.radius, [
      chartSpec.pie.innerRadius,
      chartSpec.pie.outerRadius,
    ]);

    const gaugeRes = buildEChartsOption({
      kind: "GAUGE",
      title: "履约达成率",
      options: { unit: "%" },
      data: {
        columns: [{ name: "metric_value", data_type: "FLOAT64" }],
        rows: [[96.5]],
      },
      isDark: false,
    });
    const gaugeSeries = (gaugeRes.option.series as Record<string, unknown>[])[0];
    assert.equal(gaugeSeries?.min, chartSpec.gauge.defaultMin);
    assert.equal(gaugeSeries?.max, chartSpec.gauge.defaultMax);
  });

  it("buildEChartsOption precomputes numeric SCATTER symbolSize and round-trips JSON.stringify with zero functions across all chart kinds", () => {
    const scatterRes = buildEChartsOption({
      kind: "SCATTER",
      title: "仓库吞吐散点",
      encoding: { x: "orders", y: "latency", size: "weight" },
      data: {
        columns: [
          { name: "orders", data_type: "INT64" },
          { name: "latency", data_type: "FLOAT64" },
          { name: "weight", data_type: "FLOAT64" },
        ],
        rows: [
          [100, 12.5, 2],    // clamped up to minSymbolSize (6)
          [240, 18.0, 20],   // within [6, 36] -> 20
          [500, 31.2, 999],  // clamped down to maxSymbolSize (36)
          [80, 9.1, null],   // fallback to defaultSymbolSize (12)
        ],
      },
      isDark: false,
    });

    assert.equal(containsFunctionValue(scatterRes.option), false);
    const scatterSeries = (scatterRes.option.series as Record<string, unknown>[])[0];
    assert.equal(scatterSeries?.symbolSize, chartSpec.scatter.defaultSymbolSize);
    const points = scatterSeries?.data as { value: number[]; symbolSize: number }[];
    assert.equal(points[0]?.symbolSize, chartSpec.scatter.minSymbolSize);
    assert.equal(points[1]?.symbolSize, 20);
    assert.equal(points[2]?.symbolSize, chartSpec.scatter.maxSymbolSize);
    assert.equal(points[3]?.symbolSize, chartSpec.scatter.defaultSymbolSize);
    assert.deepEqual(
      JSON.parse(JSON.stringify(scatterRes.option)),
      scatterRes.option
    );

    const allKinds = [
      "LINE_CHART",
      "BAR_CHART",
      "PIE_CHART",
      "SCATTER",
      "SANKEY",
      "HEATMAP",
      "GAUGE",
      "RADAR",
      "GRAPH_TOPOLOGY",
    ];
    const sampleData = {
      columns: [
        { name: "source", data_type: "STRING" },
        { name: "target", data_type: "STRING" },
        { name: "value", data_type: "FLOAT64" },
      ],
      rows: [
        ["苏州工厂", "上海一号仓", 68],
        ["杭州工厂", "南京分拨仓", 42],
      ],
    };

    for (const kind of allKinds) {
      for (const isDark of [false, true]) {
        const built = buildEChartsOption({
          kind,
          title: `测试图表 · ${kind}`,
          data: sampleData,
          isDark,
        });
        assert.equal(
          containsFunctionValue(built.option),
          false,
          `${kind} (dark=${String(isDark)}) must not contain any JS function`
        );
        const roundTripped = JSON.parse(JSON.stringify(built.option)) as Record<
          string,
          unknown
        >;
        assert.deepEqual(roundTripped, built.option);
      }
    }
  });

  it("sanitizeExportFilters and mergeLockedShareFilters strip blanks and enforce locked share filters", () => {
    assert.deepEqual(
      sanitizeExportFilters({
        status: "PAID",
        emptyStr: "   ",
        nullVal: null,
        undefVal: undefined,
        emptyArr: [],
        region: ["华东", "华南"],
      }),
      {
        status: "PAID",
        region: ["华东", "华南"],
      }
    );

    assert.deepEqual(
      mergeLockedShareFilters(
        { status: "PENDING", warehouse: "W-2" },
        { status: "PAID" }
      ),
      { status: "PAID", warehouse: "W-2" }
    );
  });

  it("buildEChartsOption formats ISO-8601 axis timestamps, applies Y-axis splitNumber/hideOverlap, assigns distinct palette colors, and resolves UUID series names", () => {
    assert.equal(
      formatAxisTimestampLabel("2026-11-14T20:06:17.670202205+00:00"),
      "11-14 20:06"
    );
    assert.equal(formatAxisTimestampLabel("PAID"), "PAID");

    const uuid1 = "00000000-0000-0000-0000-000000000301";
    const uuid2 = "00000000-0000-0000-0000-000000000302";
    const lineRes = buildEChartsOption({
      kind: "LINE_CHART",
      title: "仓库库存趋势",
      data: {
        columns: [
          { name: "bucket_start", data_type: "TIMESTAMP" },
          { name: "group_key", data_type: "STRING" },
          { name: "value", data_type: "FLOAT64" },
        ],
        rows: [
          ["2026-11-14T20:06:17.670202205+00:00", uuid1, 500],
          ["2026-11-14T20:06:17.670202205+00:00", uuid2, 320],
        ],
      },
      isDark: false,
      nodeLabelMap: {
        [uuid1]: "华东仓 (W-1)",
      },
    });

    const xAxis = lineRes.option.xAxis as Record<string, unknown>;
    assert.deepEqual(xAxis.data, ["11-14 20:06"]);

    const yAxis = lineRes.option.yAxis as Record<string, unknown>;
    assert.equal(yAxis.splitNumber, 4);
    assert.equal(
      (yAxis.axisLabel as Record<string, unknown>)?.hideOverlap,
      true
    );

    const series = lineRes.option.series as Record<string, unknown>[];
    assert.equal(series.length, 2);
    assert.equal(series[0]?.name, "华东仓 (W-1)");
    assert.equal(series[1]?.name, "#000302");
    assert.equal(
      (series[0]?.itemStyle as Record<string, unknown>)?.color,
      chartSpec.palette.light[0]
    );
    assert.equal(
      (series[1]?.itemStyle as Record<string, unknown>)?.color,
      chartSpec.palette.light[1]
    );
    assert.notEqual(
      (series[0]?.itemStyle as Record<string, unknown>)?.color,
      (series[1]?.itemStyle as Record<string, unknown>)?.color
    );
    assert.equal(containsFunctionValue(lineRes.option), false);
  });

  it("tidyDashboardLayout aligns near-row |Δy|<=1 cards, normalizes heights, and expands [cols-2, cols) rows to full cols", () => {
    const ragged = [
      { id: "w1", position: { x: 0, y: 0, w: 5, h: 4 } },
      { id: "w2", position: { x: 6, y: 1, w: 6, h: 5 } }, // |Δy| = 1, totalW = 11 in [10, 12)
      { id: "w3", position: { x: 0, y: 7, w: 12, h: 4 } },
    ];
    const tidied = tidyDashboardLayout(ragged, 12);
    assert.equal(tidied.length, 3);
    // w1 and w2 aligned to y=0, h=5, and widths expanded from 5+6=11 to 6+6=12
    assert.equal(tidied[0]?.position.y, 0);
    assert.equal(tidied[1]?.position.y, 0);
    assert.equal(tidied[0]?.position.h, 5);
    assert.equal(tidied[1]?.position.h, 5);
    assert.equal(
      (tidied[0]?.position.w ?? 0) + (tidied[1]?.position.w ?? 0),
      12
    );
    assert.equal(tidied[0]?.position.x, 0);
    assert.equal(tidied[1]?.position.x, tidied[0]?.position.w);
    // Lower card w3 preserved without overlap
    assert.equal(tidied[2]?.position.y, 7);
    assert.equal(tidied[2]?.position.w, 12);
  });

  it("resolveFreeFloatLayout never compacts lower cards upward or pushes locked neighbors on collision", () => {
    const baseline = new Map<string, GridLayoutBox>([
      ["top", { i: "top", x: 0, y: 0, w: 6, h: 4 }],
      ["lower", { i: "lower", x: 0, y: 6, w: 6, h: 4 }],
    ]);

    // Moving upper card horizontally to (6, 0) leaves lower card untouched at (0, 6)
    const movedCleanly = resolveFreeFloatLayout(
      [
        { i: "top", x: 6, y: 0, w: 6, h: 4 },
        { i: "lower", x: 0, y: 6, w: 6, h: 4 },
      ],
      12,
      "top",
      baseline
    );
    assert.deepEqual(movedCleanly[0], { i: "top", x: 6, y: 0, w: 6, h: 4 });
    assert.deepEqual(movedCleanly[1], { i: "lower", x: 0, y: 6, w: 6, h: 4 });

    // Even if RGL proposes pushing `lower` down to y=10 when `top` collides at (0, 6),
    // resolveFreeFloatLayout locks `lower` at (0, 6) and snaps `top` back to (0, 0).
    const collisionAttempt = resolveFreeFloatLayout(
      [
        { i: "top", x: 0, y: 6, w: 6, h: 4 },
        { i: "lower", x: 0, y: 10, w: 6, h: 4 },
      ],
      12,
      "top",
      baseline
    );
    assert.deepEqual(collisionAttempt[1], {
      i: "lower",
      x: 0,
      y: 6,
      w: 6,
      h: 4,
    });
    assert.deepEqual(collisionAttempt[0], {
      i: "top",
      x: 0,
      y: 0,
      w: 6,
      h: 4,
    });
  });

  it("computeMagneticSnapGuides and Step 25 chart variants produce pure JSON options", () => {
    const guides = computeMagneticSnapGuides(
      { i: "a", x: 6, y: 4, w: 6, h: 4 },
      [
        { i: "a", x: 6, y: 4, w: 6, h: 4 },
        { i: "b", x: 0, y: 4, w: 6, h: 4 },
      ]
    );
    assert.deepEqual(guides.verticalCols, [6]);
    assert.deepEqual(guides.horizontalRows, [4, 8]);

    for (const variant of [
      "STACKED_BAR",
      "HORIZONTAL_BAR",
      "AREA",
      "DUAL_AXIS",
      "ROSE",
      "FUNNEL",
      "TREEMAP",
    ]) {
      const kind =
        variant === "ROSE" || variant === "FUNNEL" || variant === "TREEMAP"
          ? "PIE_CHART"
          : variant === "AREA"
            ? "LINE_CHART"
            : "BAR_CHART";
      const res = buildEChartsOption({
        kind,
        encoding: { x: "status", y: ["amt"], secondary_y: "cnt" },
        options: { chart_variant: variant, mark_line_value: 100 },
        data: {
          columns: [
            { name: "status", data_type: "STRING" },
            { name: "amt", data_type: "DECIMAL" },
            { name: "cnt", data_type: "INT64" },
          ],
          rows: [
            ["PAID", 1200, 15],
            ["PENDING", 300, 4],
          ],
        },
        isDark: false,
      });
      assert.equal(containsFunctionValue(res.option), false);
    }
  });
});
