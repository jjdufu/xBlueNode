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
const WAREHOUSE_W1_ID = "00000000-0000-0000-0000-000000000301";

async function installXBlueViewMocks(
  page: Page,
  leakedTenantUrls: string[],
  missingAuthUrls: string[]
): Promise<{ invokedPayloads: unknown[] }> {
  const invokedPayloads: unknown[] = [];
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
      (!authHeader.startsWith("Bearer ") || authHeader.slice(7).trim().length === 0)
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
          name: "供应链总览",
          slug: "supply-overview",
          description: "Step 11 供应孪生驾驶舱",
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
              deps: { dataset_ids: [], type_names: [], metrics: ["paid_amount"], alerts: false, scenario_ids: [] },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: BAR_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "BAR_CHART",
              title: "按状态金额",
              position: { x: 6, y: 0, w: 10, h: 6 },
              data_source: {
                kind: "SQL",
                sql: "SELECT status, sum(amount) AS amt FROM lake.demo_orders GROUP BY 1",
              },
              encoding: { x: "status", y: ["amt"] },
              options: {},
              drill: [
                { field: "status" },
                { field: "warehouse_code", node_type: "Warehouse", node_key_field: "warehouse_code" },
              ],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: { dataset_ids: [], type_names: [], metrics: [], alerts: false, scenario_ids: [] },
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
                agg: "SUM",
                grain: "HOUR",
              },
              encoding: { x: "ts", y: ["value"] },
              options: {},
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: { dataset_ids: [], type_names: ["Warehouse"], metrics: ["inventory"], alerts: false, scenario_ids: [] },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: MAP_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "MAP",
              title: "节点地理分布",
              position: { x: 0, y: 4, w: 6, h: 7 },
              data_source: { kind: "NODES_GEO", type_names: ["Factory", "Warehouse"] },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: { dataset_ids: [], type_names: ["Factory", "Warehouse"], metrics: [], alerts: false, scenario_ids: [] },
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
              deps: { dataset_ids: [], type_names: ["Warehouse"], metrics: ["inventory"], alerts: true, scenario_ids: [] },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: ALERT_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "ALERT_LIST",
              title: "活动告警",
              position: { x: 16, y: 6, w: 8, h: 5 },
              data_source: { kind: "ALERTS", status: ["OPEN", "ACK"], limit: 20 },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "LIVE",
              refresh_seconds: 60,
              deps: { dataset_ids: [], type_names: [], metrics: [], alerts: true, scenario_ids: [] },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: SIM_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "SIM_COMPARE",
              title: "沙盘推演对比",
              position: { x: 0, y: 11, w: 12, h: 6 },
              data_source: { kind: "SIM_SCENARIOS", scenario_ids: ["s1", "s2"], metrics: ["fulfillment"] },
              encoding: {},
              options: {},
              drill: [],
              refresh_mode: "MANUAL",
              refresh_seconds: 60,
              deps: { dataset_ids: [], type_names: [], metrics: [], alerts: false, scenario_ids: ["s1", "s2"] },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: ACTION_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "ACTION_FORM",
              title: "库存盘点回写",
              position: { x: 12, y: 11, w: 12, h: 6 },
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
              deps: { dataset_ids: [], type_names: [], metrics: [], alerts: false, scenario_ids: [] },
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
        drill_path?: { field: string; value: unknown }[];
      } | null;
      const drillPath = bodyJson?.drill_path ?? [];
      if (drillPath.length > 0) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            columns: [
              { name: "warehouse_code", data_type: "STRING" },
              { name: "amt", data_type: "DECIMAL" },
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
              { name: "amt", data_type: "DECIMAL" },
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
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "ts", data_type: "STRING" },
            { name: "value", data_type: "FLOAT64" },
          ],
          rows: [
            ["08:00", 480],
            ["09:00", currentInventory],
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
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "node_id", data_type: "STRING" },
            { name: "type_name", data_type: "STRING" },
            { name: "node_key", data_type: "STRING" },
            { name: "title", data_type: "STRING" },
            { name: "geo_lng", data_type: "FLOAT64" },
            { name: "geo_lat", data_type: "FLOAT64" },
            { name: "metric_value", data_type: "FLOAT64" },
          ],
          rows: [
            [WAREHOUSE_W1_ID, "Warehouse", "W-1", "华东仓", 121.47, 31.23, currentInventory],
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

    if (pathname === `/api/xblueview/widgets/${SCENE3D_WIDGET_ID}/data`) {
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
            [WAREHOUSE_W1_ID, "Warehouse", "W-1", "华东仓", "Rack_A", 0, 0.5, 0, 1, currentInventory, 0],
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
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "id", data_type: "STRING" },
            { name: "title", data_type: "STRING" },
            { name: "severity", data_type: "STRING" },
            { name: "status", data_type: "STRING" },
            { name: "node_id", data_type: "STRING" },
          ],
          rows: [
            ["a1", "华东仓安全库存预警", "WARN", "OPEN", WAREHOUSE_W1_ID],
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
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "scenario_name", data_type: "STRING" },
            { name: "metric", data_type: "STRING" },
            { name: "after_val", data_type: "FLOAT64" },
          ],
          rows: [
            ["方案A · 增补库存", "fulfillment", 0.94],
            ["方案B · 应急调拨", "fulfillment", 0.98],
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
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "id", data_type: "STRING" },
            { name: "action_kind", data_type: "STRING" },
            { name: "status", data_type: "STRING" },
          ],
          rows: [["1", "WRITE_NODE_METRIC", "DONE"]],
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
      const payload = request.postDataJSON() as { inputs?: { value?: number } } | null;
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
          action_log_id: 101,
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
      // glTF header
      view.setUint32(0, 0x46546c67, true);
      view.setUint32(4, 2, true);
      view.setUint32(8, totalLen, true);
      // JSON chunk header
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

  return { invokedPayloads };
}

test("xBlueView /view/supply-overview renders 8 widgets incl. 3D scene & action form, drills PAID bar into breadcrumb, and scales ?screen=1", async ({
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

  const { invokedPayloads } = await installXBlueViewMocks(
    page,
    leakedTenantUrls,
    missingAuthUrls
  );

  await page.goto("/view/supply-overview");

  await expect(
    page.getByRole("heading", { level: 1, name: "供应链总览" })
  ).toBeVisible();

  // Verify all 8 widgets rendered
  const frames = page.locator("[data-testid^='widget-frame-']");
  await expect(frames).toHaveCount(8);

  // Verify 3D twin widget and action writeback form are visible
  await expect(page.getByTestId("scene3d-canvas")).toBeVisible();
  await expect(
    page.getByTestId(`widget-action-${ACTION_WIDGET_ID}`)
  ).toBeVisible();

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
