import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const DASHBOARD_ID = "00000000-0000-0000-0000-000000000500";
const KPI_WIDGET_ID = "00000000-0000-0000-0000-000000000501";
const BAR_WIDGET_ID = "00000000-0000-0000-0000-000000000502";
const LINE_WIDGET_ID = "00000000-0000-0000-0000-000000000503";
const MAP_WIDGET_ID = "00000000-0000-0000-0000-000000000504";
const SCENE3D_WIDGET_ID = "00000000-0000-0000-0000-000000000505";
const ALERT_WIDGET_ID = "00000000-0000-0000-0000-000000000506";
const SIM_WIDGET_ID = "00000000-0000-0000-0000-000000000507";
const ACTION_WIDGET_ID = "00000000-0000-0000-0000-000000000508";
const MODEL3D_ASSET_ID = "00000000-0000-0000-0000-000000000599";
const FACTORY_F1_ID = "00000000-0000-0000-0000-000000000300";
const WAREHOUSE_W1_ID = "00000000-0000-0000-0000-000000000301";
const RULE_R1_ID = "00000000-0000-0000-0000-000000000701";

async function installXBlueViewMocks(
  page: Page,
  leakedTenantUrls: string[],
  missingAuthUrls: string[]
): Promise<{
  invokedPayloads: unknown[];
  barFilterPayloads: Record<string, unknown>[];
}> {
  const invokedPayloads: unknown[] = [];
  const barFilterPayloads: Record<string, unknown>[] = [];
  let currentInventory = 500;

  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const { pathname, searchParams } = url;

    if (searchParams.has("tenant_id")) {
      leakedTenantUrls.push(request.url());
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "INVALID_ARGUMENT",
          message: "tenant_id query parameter is forbidden",
          trace_id: "xblueview-e2e-400",
        }),
      });
      return;
    }

    const authHeader = request.headers()["authorization"] ?? "";
    if (
      pathname !== "/api/health" &&
      (!authHeader.startsWith("Bearer ") ||
        authHeader.slice(7).trim().length === 0)
    ) {
      missingAuthUrls.push(request.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "xblueview-e2e-401",
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards/by-slug/supply-overview") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: DASHBOARD_ID,
          tenant_id: "default",
          name: "供应链全景驾驶舱",
          slug: "supply-overview",
          description: "华东供应链指标、下钻分析、三维数字孪生与回写闭环驾驶舱",
          filters: [
            {
              key: "status",
              label: "订单状态",
              kind: "SELECT",
              field_map: { [BAR_WIDGET_ID]: "status" },
              default: null,
            },
          ],
          theme: "light",
          canvas: {
            cols: 24,
            row_height: 30,
            screen_width: 1920,
            screen_height: 1080,
          },
          status: "PUBLISHED",
          version: 1,
          created_by: null,
          created_at: "2026-01-31T08:00:00Z",
          updated_at: "2026-01-31T08:00:00Z",
          widgets: [
            {
              id: KPI_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "KPI_CARD",
              title: "已支付金额",
              position: { x: 0, y: 0, w: 6, h: 4 },
              data_source: {
                kind: "METRIC",
                metric: "paid_amount",
                dimensions: [],
              },
              encoding: { value: "value" },
              options: { unit: "元" },
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: [],
                metrics: ["paid_amount"],
                alerts: false,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: BAR_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "BAR_CHART",
              title: "按状态订单金额",
              position: { x: 6, y: 0, w: 10, h: 6 },
              data_source: {
                kind: "SQL",
                sql: "SELECT status, warehouse_code, sum(amount) AS total_amount FROM demo_orders GROUP BY status, warehouse_code",
              },
              encoding: { x: "status", y: ["total_amount"] },
              options: {},
              drill: [
                { field: "status" },
                {
                  field: "warehouse_code",
                  node_type: "Warehouse",
                  node_key_field: "warehouse_code",
                },
              ],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: [],
                metrics: [],
                alerts: false,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: LINE_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "LINE_CHART",
              title: "仓库库存趋势",
              position: { x: 16, y: 0, w: 8, h: 6 },
              data_source: {
                kind: "NODE_METRIC",
                type_name: "Warehouse",
                metric: "inventory",
                agg: "AVG",
                grain: "HOUR",
                time_range: "LAST_7_DAYS",
                group_by: "time",
              },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: ["Warehouse"],
                metrics: ["inventory"],
                alerts: false,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: MAP_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "MAP",
              title: "供应链地理态势",
              position: { x: 0, y: 4, w: 6, h: 6 },
              data_source: {
                kind: "NODES_GEO",
                type_names: ["Factory", "Warehouse"],
                color_metric: "inventory",
                link_types: ["SUPPLIES"],
              },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: ["Factory", "Warehouse"],
                metrics: ["inventory"],
                alerts: false,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: SCENE3D_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "SCENE_3D",
              title: "仓库三维孪生",
              position: { x: 6, y: 6, w: 10, h: 8 },
              data_source: {
                kind: "SCENE_3D",
                asset_id: MODEL3D_ASSET_ID,
                color_metric: "inventory",
              },
              encoding: {},
              options: {
                thresholds: [
                  { value: 0, color: "#ef4444" },
                  { value: 400, color: "#2563eb" },
                ],
              },
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: ["Warehouse"],
                metrics: ["inventory"],
                alerts: true,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: ALERT_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "ALERT_LIST",
              title: "运营告警列表",
              position: { x: 16, y: 6, w: 8, h: 4 },
              data_source: { kind: "ALERTS", status: ["OPEN", "ACK"], limit: 20 },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: [],
                metrics: [],
                alerts: true,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: SIM_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "SIM_COMPARE",
              title: "推演方案对比",
              position: { x: 0, y: 10, w: 6, h: 4 },
              data_source: {
                kind: "SIM_SCENARIOS",
                scenario_ids: ["s1", "s2"],
                metrics: ["fulfillment"],
              },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "MANUAL",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: [],
                metrics: [],
                alerts: false,
                scenario_ids: ["s1", "s2"],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: ACTION_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "ACTION_FORM",
              title: "库存盘点回写",
              position: { x: 16, y: 10, w: 8, h: 4 },
              data_source: {
                kind: "ACTION",
                action_type: "WRITE_NODE",
                node_id: WAREHOUSE_W1_ID,
                target_kind: "METRIC",
                key: "inventory",
                input_schema: {
                  type: "object",
                  required: ["value"],
                  properties: {
                    value: { type: "number", title: "盘点库存数量" },
                  },
                },
              },
              encoding: {},
              options: { button_label: "提交盘点回写" },
              drill: [],
              refresh_mode: "MANUAL",
              refresh_seconds: 60,
              deps: {
                dataset_ids: [],
                type_names: [],
                metrics: [],
                alerts: false,
                scenario_ids: [],
              },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}/stream`) {
      await route.fulfill({
        status: 200,
        headers: {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
        },
        body: ": keepalive\n\n",
      });
      return;
    }

    if (pathname === "/api/xblueview/widgets/preview") {
      // Exact 10-column GRAPH contract: [record_kind, id, type_name, node_key, title, metric_value, src_node_id, dst_node_id, link_type, weight]
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
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
            [
              "NODE",
              FACTORY_F1_ID,
              "Factory",
              "F-1",
              "苏州工厂",
              900,
              null,
              null,
              null,
              null,
            ],
            [
              "NODE",
              WAREHOUSE_W1_ID,
              "Warehouse",
              "W-1",
              "华东仓",
              currentInventory,
              null,
              null,
              null,
              null,
            ],
            [
              "LINK",
              "00000000-0000-0000-0000-000000000399",
              null,
              null,
              null,
              null,
              FACTORY_F1_ID,
              WAREHOUSE_W1_ID,
              "SUPPLIES",
              1.0,
            ],
          ],
          row_count: 3,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${KPI_WIDGET_ID}/data`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [{ name: "value", data_type: "DECIMAL" }],
          rows: [[12500]],
          row_count: 1,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${BAR_WIDGET_ID}/data`) {
      const bodyJson = request.postDataJSON() as {
        filters?: Record<string, unknown>;
        drill_path?: { field: string; value: unknown }[];
      } | null;
      if (bodyJson?.filters && Object.keys(bodyJson.filters).length > 0) {
        barFilterPayloads.push(bodyJson.filters);
      }
      const drillPath = bodyJson?.drill_path ?? [];
      if (drillPath.length > 0) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            columns: [
              { name: "warehouse_code", data_type: "STRING" },
              { name: "total_amount", data_type: "DECIMAL" },
            ],
            rows: [
              ["W-1", 8200],
              ["W-2", 4300],
            ],
            row_count: 2,
            truncated: false,
            generated_at: "2026-01-31T08:00:01Z",
            data_version: "v2",
            drill_level: 1,
          }),
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            columns: [
              { name: "status", data_type: "STRING" },
              { name: "total_amount", data_type: "DECIMAL" },
            ],
            rows: [
              ["PAID", 12500],
              ["PENDING", 3400],
            ],
            row_count: 2,
            truncated: false,
            generated_at: "2026-01-31T08:00:00Z",
            data_version: "v1",
            drill_level: 0,
          }),
        });
      }
      return;
    }

    if (pathname === `/api/xblueview/widgets/${LINE_WIDGET_ID}/data`) {
      // Exact 3-column NODE_METRIC contract: [ts, group_key, value]
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "ts", data_type: "TIMESTAMP" },
            { name: "group_key", data_type: "STRING" },
            { name: "value", data_type: "FLOAT64" },
          ],
          rows: [
            ["2026-01-31T08:00:00Z", null, 480],
            ["2026-01-31T09:00:00Z", null, currentInventory],
          ],
          row_count: 2,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${MAP_WIDGET_ID}/data`) {
      // Exact 15-column NODES_GEO with link_types fly-lines contract
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
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
              FACTORY_F1_ID,
              "Factory",
              "F-1",
              "苏州工厂",
              120.62,
              31.3,
              900,
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
              WAREHOUSE_W1_ID,
              "Warehouse",
              "W-1",
              "华东仓",
              121.47,
              31.23,
              currentInventory,
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
              "00000000-0000-0000-0000-000000000399",
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
              1.0,
            ],
          ],
          row_count: 3,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${SCENE3D_WIDGET_ID}/data`) {
      // Exact 11-column SCENE_3D contract: [node_id, type_name, node_key, title, mesh_name, pos_x, pos_y, pos_z, scale, metric_value, open_alerts]
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "node_id", data_type: "STRING" },
            { name: "type_name", data_type: "STRING" },
            { name: "node_key", data_type: "STRING" },
            { name: "title", data_type: "STRING" },
            { name: "mesh_name", data_type: "STRING" },
            { name: "pos_x", data_type: "FLOAT64" },
            { name: "pos_y", data_type: "FLOAT64" },
            { name: "pos_z", data_type: "FLOAT64" },
            { name: "scale", data_type: "FLOAT64" },
            { name: "metric_value", data_type: "FLOAT64" },
            { name: "open_alerts", data_type: "INT64" },
          ],
          rows: [
            [
              WAREHOUSE_W1_ID,
              "Warehouse",
              "W-1",
              "华东仓",
              "Rack_A",
              0,
              0.5,
              0,
              1,
              currentInventory,
              0,
            ],
          ],
          row_count: 1,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${ALERT_WIDGET_ID}/data`) {
      // Exact 10-column ALERTS contract: [id, rule_id, severity, status, title, node_id, dataset_id, first_seen_at, last_seen_at, occurrences]
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "id", data_type: "STRING" },
            { name: "rule_id", data_type: "STRING" },
            { name: "severity", data_type: "STRING" },
            { name: "status", data_type: "STRING" },
            { name: "title", data_type: "STRING" },
            { name: "node_id", data_type: "STRING" },
            { name: "dataset_id", data_type: "STRING" },
            { name: "first_seen_at", data_type: "TIMESTAMP" },
            { name: "last_seen_at", data_type: "TIMESTAMP" },
            { name: "occurrences", data_type: "INT64" },
          ],
          rows: [
            [
              "00000000-0000-0000-0000-000000000801",
              RULE_R1_ID,
              "WARN",
              "OPEN",
              "华东仓安全库存预警",
              WAREHOUSE_W1_ID,
              null,
              "2026-01-31T07:30:00Z",
              "2026-01-31T08:00:00Z",
              2,
            ],
          ],
          row_count: 1,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${SIM_WIDGET_ID}/data`) {
      // Exact 10-column SIM_SCENARIOS contract
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "scenario_id", data_type: "STRING" },
            { name: "scenario_name", data_type: "STRING" },
            { name: "node_id", data_type: "STRING" },
            { name: "type_name", data_type: "STRING" },
            { name: "title", data_type: "STRING" },
            { name: "metric", data_type: "STRING" },
            { name: "before_val", data_type: "FLOAT64" },
            { name: "after_val", data_type: "FLOAT64" },
            { name: "delta", data_type: "FLOAT64" },
            { name: "delta_ratio", data_type: "FLOAT64" },
          ],
          rows: [
            [
              "s1",
              "方案A · 增补库存",
              WAREHOUSE_W1_ID,
              "Warehouse",
              "华东仓",
              "fulfillment",
              0.88,
              0.94,
              0.06,
              0.068,
            ],
            [
              "s2",
              "方案B · 应急调拨",
              WAREHOUSE_W1_ID,
              "Warehouse",
              "华东仓",
              "fulfillment",
              0.88,
              0.98,
              0.1,
              0.114,
            ],
          ],
          row_count: 2,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${ACTION_WIDGET_ID}/data`) {
      // Exact 7-column ACTION contract: [id, action_kind, target, status, payload, error_message, created_at]
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "id", data_type: "INT64" },
            { name: "action_kind", data_type: "STRING" },
            { name: "target", data_type: "STRING" },
            { name: "status", data_type: "STRING" },
            { name: "payload", data_type: "JSON" },
            { name: "error_message", data_type: "STRING" },
            { name: "created_at", data_type: "TIMESTAMP" },
          ],
          rows: [
            [
              101,
              "WRITE_NODE_METRIC",
              `node:${WAREHOUSE_W1_ID}:inventory`,
              "DONE",
              { value: currentInventory },
              null,
              "2026-01-31T08:00:00Z",
            ],
          ],
          row_count: 1,
          truncated: false,
          generated_at: "2026-01-31T08:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${ACTION_WIDGET_ID}/invoke`) {
      const payload = request.postDataJSON() as {
        inputs?: { value?: number };
      } | null;
      invokedPayloads.push(payload);
      if (typeof payload?.inputs?.value === "number") {
        currentInventory = payload.inputs.value;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mode: "WRITE_NODE",
          run_id: null,
          status: "DONE",
          node_id: WAREHOUSE_W1_ID,
          action_log_id: 102,
          idempotency_replay: false,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/assets/${MODEL3D_ASSET_ID}/content`) {
      const jsonStr = JSON.stringify({
        asset: { version: "2.0" },
        scene: 0,
        scenes: [{ nodes: [0] }],
        nodes: [{ name: "Rack_A" }],
      });
      const padLen = (4 - (jsonStr.length % 4)) % 4;
      const paddedJson = jsonStr + " ".repeat(padLen);
      const totalLen = 12 + 8 + paddedJson.length;
      const buf = new ArrayBuffer(totalLen);
      const view = new DataView(buf);
      view.setUint32(0, 0x46546c67, true);
      view.setUint32(4, 2, true);
      view.setUint32(8, totalLen, true);
      view.setUint32(12, paddedJson.length, true);
      view.setUint32(16, 0x4e4f534a, true);
      const bytes = new Uint8Array(buf);
      for (let i = 0; i < paddedJson.length; i += 1) {
        bytes[20 + i] = paddedJson.charCodeAt(i);
      }
      await route.fulfill({
        status: 200,
        headers: { "Content-Type": "model/gltf-binary" },
        body: Buffer.from(bytes),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [] }),
    });
  });

  return { invokedPayloads, barFilterPayloads };
}

test("xBlueView /view/supply-overview renders 8 isomorphic widgets incl. 3D scene & action form, drills PAID bar into breadcrumb, and scales ?screen=1", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  const { invokedPayloads, barFilterPayloads } = await installXBlueViewMocks(
    page,
    leakedTenantUrls,
    missingAuthUrls
  );

  await page.goto("/view/supply-overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "供应链全景驾驶舱" })
  ).toBeVisible();

  // Verify all 8 widgets rendered
  const frames = page.locator("[data-testid^='widget-frame-']");
  await expect(frames).toHaveCount(8);

  // Verify 3D twin widget, alert timestamp (not node UUID), and 7-column action log status
  await expect(page.getByTestId("scene3d-canvas")).toBeVisible();
  const actionBox = page.getByTestId(`widget-action-${ACTION_WIDGET_ID}`);
  await expect(actionBox).toBeVisible();
  await expect(actionBox).toContainText("DONE");
  await expect(page.getByTestId(`widget-alerts-${ALERT_WIDGET_ID}`)).not.toContainText(
    WAREHOUSE_W1_ID
  );

  // Verify global filter sends FilterDef.key ("status")
  await page.getByTestId("filter-status").fill("PAID");
  await expect
    .poll(() => barFilterPayloads.some((p) => p.status === "PAID"))
    .toBe(true);

  // Click PAID bar category -> drill-down breadcrumb displays PAID
  const paidTrigger = page.getByTestId("chart-category-PAID");
  await expect(paidTrigger).toBeVisible();
  await paidTrigger.click();
  await expect(page.getByTestId("drill-breadcrumb")).toContainText("PAID");

  // Submit inventory writeback action form -> updates 3D scene value
  await page.getByTestId("action-input-value").fill("620");
  await page.getByTestId(`action-submit-${ACTION_WIDGET_ID}`).click();
  await expect(
    page.getByTestId(`action-result-${ACTION_WIDGET_ID}`)
  ).toContainText("DONE");
  expect(invokedPayloads.length).toBe(1);

  // Verify 10-column GRAPH_TOPOLOGY preview in WidgetEditor
  await page.getByTestId("toggle-edit-mode-btn").click();
  await expect(page.getByTestId("open-dashboard-settings-btn")).toBeVisible();
  await page.getByTestId("add-widget-btn").click();
  await page.getByTestId("widget-editor-kind").selectOption("GRAPH_TOPOLOGY");
  await page.getByTestId("widget-editor-preview-btn").click();
  await expect(page.getByTestId("widget-chart-preview-widget")).toBeVisible();
  await page.getByRole("button", { name: "取消" }).click();
  await page.getByRole("button", { name: "完成" }).click();

  // Toggle ?screen=1 -> BigScreen scaled layout without sidebar
  await page.getByTestId("enter-big-screen-btn").click();
  await expect(page).toHaveURL(/screen=1/);
  await expect(page.getByTestId("big-screen-container")).toBeVisible();
  await expect(page.locator("aside")).toHaveCount(0);

  const stageTransform = await page
    .getByTestId("big-screen-stage")
    .evaluate((el) => getComputedStyle(el).transform);
  expect(stageTransform).not.toBe("none");

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("xBlueView ExportDialog configures PDF & DOCX template placeholders, polls ExportList to SUCCEEDED download, and creates axs_ share link", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  await installXBlueViewMocks(page, leakedTenantUrls, missingAuthUrls);

  const TEMPLATE_ID = "00000000-0000-0000-0000-000000000901";
  const EXPORT_JOB_ID = "00000000-0000-0000-0000-000000000902";
  const SHARE_LINK_ID = "00000000-0000-0000-0000-000000000903";
  const submittedExportBodies: Record<string, unknown>[] = [];
  let exportListPolls = 0;
  let downloadedCount = 0;

  await page.route("**/api/xblueview/export-templates**", async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        items: [
          {
            id: TEMPLATE_ID,
            tenant_id: "default",
            name: "经营周报标准母版",
            export_format: "DOCX",
            sha256: "a".repeat(64),
            placeholders: ["department", "author"],
            created_by: "00000000-0000-0000-0000-000000000000",
            created_at: "2026-01-31T08:00:00Z",
          },
        ],
        next_cursor: null,
      }),
    });
  });

  await page.route("**/api/xblueview/exports**", async (route: Route) => {
    const req = route.request();
    const url = new URL(req.url());

    if (url.pathname === `/api/xblueview/exports/${EXPORT_JOB_ID}/download`) {
      downloadedCount += 1;
      await route.fulfill({
        status: 200,
        headers: {
          "Content-Type":
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
          "Content-Disposition":
            "attachment; filename*=UTF-8''%E4%BE%9B%E5%BA%94%E9%93%BE%E5%91%A8%E6%8A%A5.docx",
        },
        body: "PK-mock-docx-binary",
      });
      return;
    }

    if (req.method() === "POST" && url.pathname === "/api/xblueview/exports") {
      const body = req.postDataJSON() as Record<string, unknown>;
      submittedExportBodies.push(body);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: EXPORT_JOB_ID,
          tenant_id: "default",
          export_format: body.export_format ?? "DOCX",
          source: body.source,
          options: body.options ?? {},
          template_id: body.template_id ?? null,
          job_id: "00000000-0000-0000-0000-000000000999",
          status: "PENDING",
          file_name: null,
          file_size: null,
          sha256: null,
          error_message: null,
          created_by: "00000000-0000-0000-0000-000000000000",
          created_at: "2026-01-31T08:05:00Z",
          finished_at: null,
          expires_at: null,
        }),
      });
      return;
    }

    exportListPolls += 1;
    const isDone = exportListPolls >= 2;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        items: [
          {
            id: EXPORT_JOB_ID,
            tenant_id: "default",
            export_format: "DOCX",
            source: {
              kind: "DASHBOARD",
              dashboard_id: DASHBOARD_ID,
              filters: {},
            },
            options: { title: "供应链全景周报", watermark: true },
            template_id: TEMPLATE_ID,
            status: isDone ? "SUCCEEDED" : "RUNNING",
            file_name: isDone ? "供应链周报.docx" : null,
            file_size: isDone ? 49152 : null,
            sha256: isDone ? "b".repeat(64) : null,
            error_message: null,
            created_by: "00000000-0000-0000-0000-000000000000",
            created_at: "2026-01-31T08:05:00Z",
            finished_at: isDone ? "2026-01-31T08:05:02Z" : null,
            expires_at: isDone ? "2026-02-07T08:05:02Z" : null,
          },
        ],
        next_cursor: null,
      }),
    });
  });

  await page.route("**/api/xblueview/shares**", async (route: Route) => {
    const req = route.request();
    if (req.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: SHARE_LINK_ID,
          token: "axs_demo1234_secret5678901234567890123456",
          url: "/share/axs_demo1234_secret5678901234567890123456",
          expires_at: "2026-02-03T08:05:00Z",
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [], next_cursor: null }),
    });
  });

  await page.goto("/view/supply-overview");
  await expect(
    page.getByRole("heading", { level: 1, name: "供应链全景驾驶舱" })
  ).toBeVisible();

  // Open ExportDialog from toolbar, inspect PDF options, then switch to DOCX template + placeholders
  await page.getByTestId("open-export-dialog-btn").click();
  await expect(page.getByTestId("export-dialog")).toBeVisible();
  await page.getByTestId("export-pdf-page-size").selectOption("A3");
  await page.getByTestId("export-pdf-orientation").selectOption("landscape");

  await page.getByTestId("export-format-DOCX").click();
  await page.getByTestId("export-title-input").fill("供应链全景周报");
  await page.getByTestId("export-template-select").selectOption(TEMPLATE_ID);
  await page.getByTestId("export-placeholder-department").fill("华东供应链中心");
  await page.getByTestId("export-placeholder-author").fill("张工");
  await page.getByTestId("submit-export-btn").click();

  expect(submittedExportBodies.length).toBe(1);
  expect(submittedExportBodies[0]?.export_format).toBe("DOCX");
  expect(submittedExportBodies[0]?.template_id).toBe(TEMPLATE_ID);
  expect(
    (submittedExportBodies[0]?.options as Record<string, unknown>)?.placeholders
  ).toEqual({
    department: "华东供应链中心",
    author: "张工",
  });

  // ExportList opens automatically, shows RUNNING then auto-polls to SUCCEEDED
  await expect(page.getByTestId("export-list-drawer")).toBeVisible();
  await expect(
    page.getByTestId(`export-job-status-${EXPORT_JOB_ID}`)
  ).toContainText("SUCCEEDED", { timeout: 8000 });

  await page.getByTestId(`download-export-${EXPORT_JOB_ID}`).click();
  await expect.poll(() => downloadedCount).toBe(1);

  // Open ShareDialog from ExportList shortcut and create one-time axs_ link
  await page.getByTestId(`share-export-file-${EXPORT_JOB_ID}`).click();
  await expect(page.getByTestId("share-dialog")).toBeVisible();
  await page.getByTestId("submit-create-share-btn").click();
  await expect(page.getByTestId("created-share-url")).toHaveValue(
    /\/share\/axs_demo1234_secret5678901234567890123456$/
  );

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("public /share/:token unlocks password into X-Share-Session, renders chromeless read-only dashboard without ACTION_* controls, and handles 404 expired/revoked state", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const shareAuthHeaderLeaks: string[] = [];
  const unexpectedInternalUrls: string[] = [];
  const seenShareSessions: string[] = [];

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      const text = msg.text();
      // Expected negative-path HTTP 401 (wrong password) and 404 (revoked share) resource status logs from Chromium
      if (
        text.startsWith(
          "Failed to load resource: the server responded with a status of 401"
        ) ||
        text.startsWith(
          "Failed to load resource: the server responded with a status of 404"
        )
      ) {
        return;
      }
      consoleErrors.push(text);
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  const SHARE_TOKEN = "axs_demo1234_secret5678901234567890123456";
  const REVOKED_TOKEN = "axs_revoked0_secret0000000000000000000000";
  const VALID_SESSION = "sess_hmac_step12_verified";

  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const url = new URL(req.url());
    const { pathname } = url;
    const headers = req.headers();

    if (pathname.startsWith("/api/xblueview/share/")) {
      if (headers["authorization"]) {
        shareAuthHeaderLeaks.push(req.url());
      }
      const shareSess = headers["x-share-session"] ?? "";
      if (shareSess) {
        seenShareSessions.push(`${pathname}:${shareSess}`);
      }

      if (pathname === `/api/xblueview/share/${REVOKED_TOKEN}`) {
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "SHARE_NOT_FOUND",
            message: "share link not found, expired, or revoked",
            trace_id: "share-404",
          }),
        });
        return;
      }

      if (pathname === `/api/xblueview/share/${SHARE_TOKEN}/unlock`) {
        const body = req.postDataJSON() as { password?: string } | null;
        if (body?.password !== "p@ss-123456") {
          await route.fulfill({
            status: 401,
            contentType: "application/json",
            body: JSON.stringify({
              error_code: "INVALID_PASSWORD",
              message: "invalid share password",
              trace_id: "share-unlock-401",
            }),
          });
          return;
        }
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            share_session: VALID_SESSION,
            expires_at: "2026-01-31T10:00:00Z",
          }),
        });
        return;
      }

      if (pathname === `/api/xblueview/share/${SHARE_TOKEN}`) {
        if (shareSess !== VALID_SESSION) {
          await route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ password_required: true }),
          });
          return;
        }
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            password_required: false,
            target_kind: "DASHBOARD",
            expires_at: "2026-02-01T12:00:00Z",
            locked_filters: { status: "PAID" },
            allow_filters: true,
            allow_export: true,
            dashboard: {
              id: DASHBOARD_ID,
              tenant_id: "default",
              name: "供应链外部共享看板",
              slug: "supply-overview",
              description: "只读外部共享驾驶舱视图",
              filters: [
                {
                  key: "status",
                  label: "订单状态",
                  kind: "SELECT",
                  field_map: { [BAR_WIDGET_ID]: "status" },
                  default: null,
                },
                {
                  key: "region",
                  label: "大区",
                  kind: "TEXT",
                  field_map: { [BAR_WIDGET_ID]: "region" },
                  default: null,
                },
              ],
              theme: "light",
              canvas: {
                cols: 24,
                row_height: 30,
                screen_width: 1920,
                screen_height: 1080,
              },
              status: "PUBLISHED",
              version: 1,
              created_by: null,
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
              widgets: [
                {
                  id: KPI_WIDGET_ID,
                  tenant_id: "default",
                  dashboard_id: DASHBOARD_ID,
                  widget_kind: "KPI_CARD",
                  title: "已支付订单总额",
                  position: { x: 0, y: 0, w: 8, h: 4 },
                  data_source: {
                    kind: "METRIC",
                    metric: "paid_amount",
                    dimensions: [],
                  },
                  encoding: { value: "value" },
                  options: { unit: "元" },
                  drill: [],
                  refresh_mode: "MANUAL",
                  refresh_seconds: 60,
                  deps: {
                    dataset_ids: [],
                    type_names: [],
                    metrics: ["paid_amount"],
                    alerts: false,
                    scenario_ids: [],
                  },
                  created_at: "2026-01-31T08:00:00Z",
                  updated_at: "2026-01-31T08:00:00Z",
                },
                {
                  id: BAR_WIDGET_ID,
                  tenant_id: "default",
                  dashboard_id: DASHBOARD_ID,
                  widget_kind: "BAR_CHART",
                  title: "按状态订单分布",
                  position: { x: 8, y: 0, w: 16, h: 6 },
                  data_source: {
                    kind: "SQL",
                    sql: "SELECT status, sum(amount) AS total_amount FROM demo_orders GROUP BY 1",
                  },
                  encoding: { x: "status", y: ["total_amount"] },
                  options: {},
                  drill: [],
                  refresh_mode: "MANUAL",
                  refresh_seconds: 60,
                  deps: {
                    dataset_ids: [],
                    type_names: [],
                    metrics: [],
                    alerts: false,
                    scenario_ids: [],
                  },
                  created_at: "2026-01-31T08:00:00Z",
                  updated_at: "2026-01-31T08:00:00Z",
                },
                {
                  id: ACTION_WIDGET_ID,
                  tenant_id: "default",
                  dashboard_id: DASHBOARD_ID,
                  widget_kind: "ACTION_FORM",
                  title: "库存盘点回写",
                  position: { x: 0, y: 6, w: 8, h: 4 },
                  data_source: {
                    kind: "ACTION",
                    action_type: "WRITE_NODE",
                    node_id: WAREHOUSE_W1_ID,
                    target_kind: "METRIC",
                    key: "inventory",
                    input_schema: {
                      type: "object",
                      required: ["value"],
                      properties: {
                        value: { type: "number", title: "盘点库存数量" },
                      },
                    },
                  },
                  encoding: {},
                  options: { button_label: "提交盘点回写" },
                  drill: [],
                  refresh_mode: "MANUAL",
                  refresh_seconds: 60,
                  deps: {
                    dataset_ids: [],
                    type_names: [],
                    metrics: [],
                    alerts: false,
                    scenario_ids: [],
                  },
                  created_at: "2026-01-31T08:00:00Z",
                  updated_at: "2026-01-31T08:00:00Z",
                },
              ],
            },
          }),
        });
        return;
      }

      if (
        pathname ===
        `/api/xblueview/share/${SHARE_TOKEN}/widgets/${KPI_WIDGET_ID}/data`
      ) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            columns: [{ name: "value", data_type: "DECIMAL" }],
            rows: [["12500.00"]],
            row_count: 1,
            truncated: false,
            generated_at: "2026-01-31T08:00:00Z",
            data_version: "v1",
            drill_level: 0,
          }),
        });
        return;
      }

      if (
        pathname ===
        `/api/xblueview/share/${SHARE_TOKEN}/widgets/${BAR_WIDGET_ID}/data`
      ) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            columns: [
              { name: "status", data_type: "STRING" },
              { name: "total_amount", data_type: "DECIMAL" },
            ],
            rows: [["PAID", "12500.00"]],
            row_count: 1,
            truncated: false,
            generated_at: "2026-01-31T08:00:00Z",
            data_version: "v1",
            drill_level: 0,
          }),
        });
        return;
      }
    }

    unexpectedInternalUrls.push(req.url());
    await route.fulfill({
      status: 401,
      contentType: "application/json",
      body: JSON.stringify({
        error_code: "UNAUTHORIZED",
        message: "unexpected internal API call on public share page",
        trace_id: "share-unexpected-401",
      }),
    });
  });

  await page.goto(`/share/${SHARE_TOKEN}`);

  // Chromeless isolation: zero sidebar, zero internal navigation links
  await expect(page.locator("aside")).toHaveCount(0);
  await expect(page.locator("a[href='/'], a[href='/login'], a[href='/view']")).toHaveCount(0);
  await expect(page.getByTestId("share-password-card")).toBeVisible();

  // Wrong password -> 401 error message inside password card
  await page.getByTestId("share-unlock-password-input").fill("wrong-pass");
  await page.getByTestId("share-unlock-submit-btn").click();
  await expect(page.getByTestId("share-unlock-error")).toContainText("401");

  // Correct password -> unlocks via X-Share-Session in memory and renders shared dashboard
  await page.getByTestId("share-unlock-password-input").fill("p@ss-123456");
  await page.getByTestId("share-unlock-submit-btn").click();

  await expect(page.getByTestId("share-dashboard-title")).toContainText(
    "供应链外部共享看板"
  );
  await expect(page.getByTestId("share-locked-filter-status")).toContainText(
    "订单状态: PAID"
  );
  await expect(page.getByTestId("share-expires-badge")).toBeVisible();
  await expect(page.getByTestId("share-download-btn")).toBeVisible();

  // Locked filter input is disabled, unlocked filter input is interactive
  await expect(page.getByTestId("filter-status")).toBeDisabled();
  await expect(page.getByTestId("filter-region")).toBeEnabled();

  // KPI & BAR rendered, ACTION_FORM completely filtered out (zero writeback triggers)
  await expect(page.getByTestId(`widget-frame-${KPI_WIDGET_ID}`)).toBeVisible();
  await expect(page.getByTestId(`widget-frame-${BAR_WIDGET_ID}`)).toBeVisible();
  await expect(page.locator("[data-widget-kind='ACTION_FORM']")).toHaveCount(0);
  await expect(page.getByTestId(`widget-frame-${ACTION_WIDGET_ID}`)).toHaveCount(0);

  // Verify X-Share-Session was sent in headers and NEVER stored in localStorage
  expect(
    seenShareSessions.some((entry) => entry.endsWith(`:${VALID_SESSION}`))
  ).toBe(true);
  const storageDump = await page.evaluate(() => JSON.stringify(localStorage));
  expect(storageDump).not.toContain(VALID_SESSION);
  expect(shareAuthHeaderLeaks).toEqual([]);
  expect(unexpectedInternalUrls).toEqual([]);

  // Navigate to revoked/expired token -> 404 card displayed
  await page.goto(`/share/${REVOKED_TOKEN}`);
  await expect(page.getByTestId("share-error-card")).toContainText("404");

  expect(unexpectedInternalUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

if (process.env.AXPOINT_E2E_LIVE === "1") {
  test("live backend /view/supply-overview renders 8 real widgets without mocks, drills PAID into breadcrumb, and scales ?screen=1", async ({
    page,
  }) => {
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err.message);
    });

    const coreBaseUrl = (
      process.env.CORE_BASE_URL ?? "http://127.0.0.1:9995"
    ).replace(/\/$/, "");

    // vite preview natively proxies /api -> http://127.0.0.1:9995 (including SSE).
    // When CORE_BASE_URL points to a non-default origin, forward /api/** directly to CORE_BASE_URL.
    if (coreBaseUrl !== "http://127.0.0.1:9995") {
      await page.route("**/api/**", async (route: Route) => {
        const reqUrl = new URL(route.request().url());
        const targetUrl = `${coreBaseUrl}${reqUrl.pathname}${reqUrl.search}`;
        if (reqUrl.pathname.endsWith("/stream")) {
          await route.fulfill({
            status: 200,
            headers: {
              "Content-Type": "text/event-stream",
              "Cache-Control": "no-cache",
            },
            body: ": keepalive\n\n",
          });
          return;
        }
        const upstream = await route.fetch({ url: targetUrl });
        await route.fulfill({ response: upstream });
      });
    }

    await page.goto("/view/supply-overview");

    await expect(
      page.getByRole("heading", { level: 1, name: "供应链全景驾驶舱" })
    ).toBeVisible({ timeout: 20_000 });

    const frames = page.locator("[data-testid^='widget-frame-']");
    await expect(frames).toHaveCount(8, { timeout: 20_000 });

    // Verify 3D twin scene and inventory writeback ACTION_FORM rendered against real backend data
    await expect(page.getByTestId("scene3d-canvas")).toBeVisible({
      timeout: 20_000,
    });
    const actionFormFrame = page.locator("[data-widget-kind='ACTION_FORM']");
    await expect(actionFormFrame).toBeVisible();
    await expect(actionFormFrame).toContainText("库存盘点回写");
    await expect(page.getByTestId("action-input-value")).toBeVisible();

    // Click PAID bar category -> real backend drill-down query updates breadcrumb
    const paidTrigger = page.getByTestId("chart-category-PAID");
    await expect(paidTrigger).toBeVisible({ timeout: 20_000 });
    await paidTrigger.click();
    await expect(page.getByTestId("drill-breadcrumb")).toContainText("PAID", {
      timeout: 15_000,
    });

    // Switch to ?screen=1 BigScreen mode and assert proportional CSS scale transform
    await page.getByTestId("enter-big-screen-btn").click();
    await expect(page).toHaveURL(/screen=1/);
    await expect(page.getByTestId("big-screen-container")).toBeVisible();
    await expect(page.locator("aside")).toHaveCount(0);

    const stageTransform = await page
      .getByTestId("big-screen-stage")
      .evaluate((el) => getComputedStyle(el).transform);
    expect(stageTransform).not.toBe("none");
    expect(pageErrors).toEqual([]);
  });
}
