import { expect, type Page, type Route, test } from "@playwright/test";
import { chooseSelectOption } from "./helpers";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const DASHBOARD_ID = "00000000-0000-0000-0000-000000002500";
const SAVED_CHART_ID = "00000000-0000-0000-0000-000000002510";
const PINNED_WIDGET_ID = "00000000-0000-0000-0000-000000002511";
const WIDGET_KPI_ID = "00000000-0000-0000-0000-000000002501";
const WIDGET_BAR_ID = "00000000-0000-0000-0000-000000002502";
const WIDGET_LOWER_ID = "00000000-0000-0000-0000-000000002503";

interface MockStep25State {
  savedCharts: Array<Record<string, unknown>>;
  pinRequests: Array<{ chartId: string; body: Record<string, unknown> }>;
  dashboardUpdates: Array<Record<string, unknown>>;
  widgetPositions: Record<
    string,
    { x: number; y: number; w: number; h: number }
  >;
  compactMode: "VERTICAL_COMPACT" | "FREE_FLOAT";
}

async function installStep25Mocks(
  page: Page,
  leakedTenantUrls: string[],
  pageErrors: string[]
): Promise<MockStep25State> {
  const state: MockStep25State = {
    savedCharts: [],
    pinRequests: [],
    dashboardUpdates: [],
    widgetPositions: {
      [WIDGET_KPI_ID]: { x: 0, y: 0, w: 5, h: 3 },
      [WIDGET_BAR_ID]: { x: 5, y: 1, w: 6, h: 4 },
      [WIDGET_LOWER_ID]: { x: 0, y: 5, w: 6, h: 4 },
    },
    compactMode: "VERTICAL_COMPACT",
  };

  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const url = new URL(req.url());
    const { pathname, searchParams } = url;

    if (searchParams.has("tenant_id")) {
      leakedTenantUrls.push(req.url());
      await route.fulfill({ status: 400, body: "forbidden tenant_id" });
      return;
    }

    if (pathname === "/api/auth/me" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          user_id: "00000000-0000-0000-0000-000000000001",
          tenant_id: "default",
          email: "admin@xbluenode.local",
          display_name: "Admin",
          roles: ["TENANT_ADMIN"],
          permissions: [
            "DATA_READ",
            "DATA_WRITE",
            "STUDIO_EDIT",
            "VIEW_READ",
            "VIEW_EDIT",
            "VIEW_PUBLISH",
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/catalog" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          datasets: [
            {
              id: "00000000-0000-0000-0000-000000000202",
              name: "demo_orders",
              display_name: "业务订单明细",
              row_count: 120,
              sensitivity: "INTERNAL",
              columns: [
                {
                  column_name: "status",
                  data_type: "STRING",
                  nullable: false,
                  pii_category: "NONE",
                },
                {
                  column_name: "amount",
                  data_type: "DECIMAL",
                  nullable: false,
                  pii_category: "NONE",
                },
                {
                  column_name: "order_date",
                  data_type: "DATE",
                  nullable: false,
                  pii_category: "NONE",
                },
              ],
            },
          ],
          platform_views: [],
          live_connections: [],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/metrics" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/analytics/saved-queries" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/analytics/history" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/analytics/sql/run" && req.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "status", data_type: "STRING", pii_category: "NONE", masked: false },
            { name: "amount", data_type: "DECIMAL", pii_category: "NONE", masked: false },
          ],
          rows: [
            { status: "PAID", amount: 12500 },
            { status: "PENDING", amount: 4200 },
            { status: "SHIPPED", amount: 8900 },
          ],
          row_count: 3,
          truncated: false,
          duration_ms: 14,
          sql: "SELECT status, SUM(amount) AS amount FROM demo_orders GROUP BY status",
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/pivot" && req.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          row_headers: [["PAID"], ["PENDING"], ["SHIPPED"], ["总计"]],
          column_headers: [["SUM(amount)"]],
          cells: [["12500.00"], ["4200.00"], ["8900.00"], ["25600.00"]],
          row_kinds: ["DATA", "DATA", "DATA", "GRAND_TOTAL"],
          columns_truncated: false,
          duration_ms: 11,
          sql: "SELECT status, SUM(amount) FROM demo_orders GROUP BY ROLLUP(status)",
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/widgets/preview" && req.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "status", data_type: "STRING" },
            { name: "amount", data_type: "DECIMAL" },
          ],
          rows: [
            ["PAID", 12500],
            ["PENDING", 4200],
            ["SHIPPED", 8900],
          ],
          row_count: 3,
          truncated: false,
          generated_at: "2026-11-24T10:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/charts" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.savedCharts, next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/xblueview/charts" && req.method() === "POST") {
      const body = JSON.parse(req.postData() ?? "{}") as Record<string, unknown>;
      const created = {
        id: SAVED_CHART_ID,
        tenant_id: "default",
        name: body.name ?? "Chart",
        description: body.description ?? "",
        widget_kind: body.widget_kind ?? "BAR_CHART",
        data_source: body.data_source ?? { kind: "SQL", sql: "SELECT 1" },
        encoding: body.encoding ?? {},
        options: body.options ?? {},
        created_by: "00000000-0000-0000-0000-000000000001",
        created_at: "2026-11-24T10:00:00Z",
        updated_at: "2026-11-24T10:00:00Z",
      };
      state.savedCharts = [created];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    const pinMatch = /^\/api\/xblueview\/charts\/([^/]+)\/pin$/.exec(pathname);
    if (pinMatch && req.method() === "POST") {
      const chartId = pinMatch[1]!;
      const body = JSON.parse(req.postData() ?? "{}") as Record<string, unknown>;
      state.pinRequests.push({ chartId, body });
      const zone =
        typeof body.target_zone === "string" ? body.target_zone : "INSIGHT";
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          widget_id: PINNED_WIDGET_ID,
          semantic_zone: zone,
          dashboard_id: DASHBOARD_ID,
          dashboard_slug: "cockpit-step25",
          deep_link: "/view/cockpit-step25",
          widget: {
            id: PINNED_WIDGET_ID,
            tenant_id: "default",
            dashboard_id: DASHBOARD_ID,
            widget_kind: "BAR_CHART",
            title: "已付订单趋势资产",
            position: { x: 6, y: 2, w: 6, h: 4 },
            data_source: { kind: "SQL", sql: "SELECT 1" },
            encoding: { x: "status", y: ["amount"] },
            options: {
              source_chart_id: chartId,
              semantic_zone: zone,
            },
            drill: [],
            refresh_mode: "INTERVAL",
            refresh_seconds: 60,
            deps: {
              dataset_ids: [],
              type_names: [],
              metrics: [],
              alerts: false,
              scenario_ids: [],
            },
            created_at: "2026-11-24T10:00:00Z",
            updated_at: "2026-11-24T10:00:00Z",
          },
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DASHBOARD_ID,
              tenant_id: "default",
              slug: "cockpit-step25",
              name: "经营全景五区驾驶舱",
              description: "",
              theme: "light",
              canvas: {
                cols: 12,
                row_height: 80,
                screen_width: 1920,
                screen_height: 1080,
                zone_layout: "STANDARD_FIVE_ZONE",
                compact_mode: state.compactMode,
              },
              filters: [],
              status: "PUBLISHED",
              published_version: 1,
              sensitivity: "INTERNAL",
              created_by: null,
              created_at: "2026-11-24T10:00:00Z",
              updated_at: "2026-11-24T10:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      (pathname === "/api/xblueview/dashboards/by-slug/cockpit-step25" ||
        pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}`) &&
      req.method() === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: DASHBOARD_ID,
          tenant_id: "default",
          slug: "cockpit-step25",
          name: "经营全景五区驾驶舱",
          description: "",
          theme: "light",
          canvas: {
            cols: 12,
            row_height: 80,
            screen_width: 1920,
            screen_height: 1080,
            zone_layout: "STANDARD_FIVE_ZONE",
            compact_mode: state.compactMode,
          },
          filters: [],
          status: "PUBLISHED",
          published_version: 1,
          sensitivity: "INTERNAL",
          created_by: null,
          created_at: "2026-11-24T10:00:00Z",
          updated_at: "2026-11-24T10:00:00Z",
          widgets: [
            {
              id: WIDGET_KPI_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "KPI_CARD",
              title: "累计实收金额",
              position: state.widgetPositions[WIDGET_KPI_ID],
              data_source: { kind: "SQL", sql: "SELECT order_date, amount FROM demo_orders" },
              encoding: { value: "amount", x: "order_date" },
              options: {
                unit: "元",
                decimals: 0,
                target_value: 20000,
              },
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
              created_at: "2026-11-24T10:00:00Z",
              updated_at: "2026-11-24T10:00:00Z",
            },
            {
              id: WIDGET_BAR_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "BAR_CHART",
              title: "订单状态对比",
              position: state.widgetPositions[WIDGET_BAR_ID],
              data_source: { kind: "SQL", sql: "SELECT status, amount FROM demo_orders" },
              encoding: { x: "status", y: ["amount"] },
              options: { show_mark_line: true, mark_line_value: 8000 },
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
              created_at: "2026-11-24T10:00:00Z",
              updated_at: "2026-11-24T10:00:00Z",
            },
            {
              id: WIDGET_LOWER_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "LINE_CHART",
              title: "履约趋势曲线",
              position: state.widgetPositions[WIDGET_LOWER_ID],
              data_source: { kind: "SQL", sql: "SELECT order_date, amount FROM demo_orders" },
              encoding: { x: "order_date", y: ["amount"] },
              options: { chart_variant: "AREA" },
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
              created_at: "2026-11-24T10:00:00Z",
              updated_at: "2026-11-24T10:00:00Z",
            },
          ],
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}` &&
      req.method() === "PUT"
    ) {
      const body = JSON.parse(req.postData() ?? "{}") as Record<string, unknown>;
      state.dashboardUpdates.push(body);
      const canvas = body.canvas as Record<string, unknown> | undefined;
      if (canvas?.compact_mode === "FREE_FLOAT") {
        state.compactMode = "FREE_FLOAT";
      } else if (canvas?.compact_mode === "VERTICAL_COMPACT") {
        state.compactMode = "VERTICAL_COMPACT";
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: DASHBOARD_ID,
          tenant_id: "default",
          slug: "cockpit-step25",
          name: "经营全景五区驾驶舱",
          description: "",
          theme: "light",
          canvas: {
            cols: 12,
            row_height: 80,
            screen_width: 1920,
            screen_height: 1080,
            zone_layout: "STANDARD_FIVE_ZONE",
            compact_mode: state.compactMode,
          },
          filters: [],
          status: "PUBLISHED",
          published_version: 1,
          sensitivity: "INTERNAL",
          created_by: null,
          created_at: "2026-11-24T10:00:00Z",
          updated_at: "2026-11-24T10:00:00Z",
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}/layout` &&
      req.method() === "PUT"
    ) {
      const body = JSON.parse(req.postData() ?? "{}") as {
        widgets?: Array<{ id: string; position: { x: number; y: number; w: number; h: number } }>;
        items?: Array<{ id: string; position: { x: number; y: number; w: number; h: number } }>;
      };
      const incoming = body.widgets ?? body.items ?? [];
      for (const item of incoming) {
        state.widgetPositions[item.id] = item.position;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          incoming.map((item) => ({
            id: item.id,
            tenant_id: "default",
            dashboard_id: DASHBOARD_ID,
            widget_kind:
              item.id === WIDGET_KPI_ID
                ? "KPI_CARD"
                : item.id === WIDGET_BAR_ID
                  ? "BAR_CHART"
                  : "LINE_CHART",
            title: item.id,
            position: item.position,
            data_source: { kind: "SQL", sql: "SELECT 1" },
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
              scenario_ids: [],
            },
            created_at: "2026-11-24T10:00:00Z",
            updated_at: "2026-11-24T10:00:00Z",
          }))
        ),
      });
      return;
    }

    if (pathname.endsWith("/stream")) {
      await route.fulfill({
        status: 200,
        headers: { "Content-Type": "text/event-stream" },
        body: ": keepalive\n\n",
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${WIDGET_KPI_ID}/data` &&
      req.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "order_date", data_type: "DATE" },
            { name: "amount", data_type: "DECIMAL" },
          ],
          rows: [
            ["2026-11-20", 12500],
            ["2026-11-21", 8400],
            ["2026-11-22", 14900],
            ["2026-11-23", 11200],
          ],
          row_count: 4,
          truncated: false,
          generated_at: "2026-11-24T10:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname.startsWith("/api/xblueview/widgets/") && pathname.endsWith("/data")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "status", data_type: "STRING" },
            { name: "amount", data_type: "DECIMAL" },
          ],
          rows: [
            ["PAID", 12500],
            ["PENDING", 4200],
            ["SHIPPED", 8900],
          ],
          row_count: 3,
          truncated: false,
          generated_at: "2026-11-24T10:00:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/assets" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/graph/nodes" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({}),
    });
  });

  return state;
}

test("Step 25: SQL result -> Chart Studio (16 types) -> Save xv_charts -> 1-step Pin to Five-Zone INSIGHT, plus VTable data bars & heatmap", async ({
  page,
}) => {
  const leakedTenantUrls: string[] = [];
  const pageErrors: string[] = [];
  const state = await installStep25Mocks(page, leakedTenantUrls, pageErrors);

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/analytics");

  // Run SQL query and verify persistent Chart & Pin actions on ResultGrid
  await page.getByTestId("sql-run-btn").click();
  const openStudioBtn = page.getByTestId("result-open-chart-studio-btn");
  const quickPinBtn = page.getByTestId("result-pin-dashboard-btn");
  await expect(openStudioBtn).toBeVisible();
  await expect(quickPinBtn).toBeVisible();

  // 1-click jump from SQL ResultGrid into first-class ChartStudioPanel
  await openStudioBtn.click();
  await expect(page.getByTestId("chart-studio-panel")).toBeVisible();

  // Verify 16 chart types are registered in the studio picker
  const typeButtons = page
    .getByTestId("chart-studio-type-grid")
    .locator("button[data-testid^='chart-studio-kind-']");
  await expect(typeButtons).toHaveCount(16);

  // Configure threshold markLine, name, save chart asset, and pin to STANDARD_FIVE_ZONE INSIGHT
  await page.getByTestId("chart-studio-name-input").fill("已付订单趋势资产");
  await page.getByTestId("chart-studio-markline-toggle").check();
  await page.getByTestId("chart-studio-markline-input").fill("8000");

  await page.getByTestId("chart-studio-save-btn").click();
  await expect(page.getByTestId("chart-studio-status-badge")).toContainText("已保存");
  expect(state.savedCharts.length).toBe(1);

  await page.getByTestId("chart-studio-pin-btn").click();
  await expect(page.getByTestId("pin-to-dashboard-modal")).toBeVisible();
  await expect(page.getByTestId("pin-modal-zone-select")).toBeVisible();
  await page.getByTestId("pin-modal-confirm-btn").click();

  await expect(page.getByTestId("pin-modal-success")).toContainText("已钉入");
  await expect(page.getByTestId("pin-modal-result-zone")).toContainText("归因洞察");
  expect(state.pinRequests.length).toBe(1);
  expect(state.pinRequests[0]?.chartId).toBe(SAVED_CHART_ID);
  expect(state.pinRequests[0]?.body.target_zone).toBe("INSIGHT");
  await page.getByTestId("pin-modal-close-btn").click();

  // Switch to Pivot tab -> verify lazy-loaded VTable Canvas + Data Bars + Heatmap + Freeze toggles
  await page.getByTestId("analytics-tab-pivot").click();
  await expect(page.getByTestId("analytics-pivot-builder")).toBeVisible();
  await page.getByTestId("pivot-quick-row-status").click();
  await page.getByTestId("pivot-quick-val-amount").click();

  const vtableStage = page.getByTestId("vtable-canvas-stage");
  await expect(vtableStage).toBeVisible({ timeout: 15_000 });
  await expect(vtableStage.locator("canvas").first()).toBeVisible({
    timeout: 15_000,
  });
  await expect(vtableStage).toHaveAttribute("data-databars", "on");
  await expect(page.getByTestId("vtable-databar-fill").first()).toBeVisible();

  // Toggle bipolar heatmap color scale
  await page.getByTestId("vtable-toggle-heatmap").click();
  await expect(vtableStage).toHaveAttribute("data-heatmap", "on");
  await expect(
    page.locator("[data-testid='pivot-result-table'] [data-heatmap-active='true']").first()
  ).toBeVisible();

  // Toggle header freeze
  await page.getByTestId("vtable-toggle-freeze").click();
  await expect(vtableStage).toHaveAttribute("data-freeze", "off");

  expect(leakedTenantUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("Step 25: KpiWidget inline SVG sparkline & target progress, plus DashboardGrid tracks, FREE_FLOAT non-pushing drag, and Tidy layout", async ({
  page,
}) => {
  const leakedTenantUrls: string[] = [];
  const pageErrors: string[] = [];
  const state = await installStep25Mocks(page, leakedTenantUrls, pageErrors);

  await page.setViewportSize({ width: 1440, height: 960 });
  await page.goto("/view/cockpit-step25");

  // 1. Verify KPI sparkline SVG + 4px target progress bar (12,500 / 20,000 = 62.5%)
  const kpiBox = page.getByTestId(`widget-kpi-${WIDGET_KPI_ID}`);
  await expect(kpiBox).toBeVisible();
  const sparkline = kpiBox.getByTestId("kpi-sparkline");
  await expect(sparkline).toBeVisible();
  await expect(sparkline.locator("circle")).toHaveCount(2);
  await expect(kpiBox.getByTestId("kpi-target-progress")).toBeVisible();
  await expect(kpiBox.getByTestId("kpi-target-pct")).toContainText("62.5%");

  // 2. Enter edit mode -> verify visible column tracks & 8-way resize handles
  await page.getByTestId("toggle-edit-mode-btn").click();
  await expect(page.getByTestId("dashboard-grid-tracks")).toBeVisible();

  const kpiFrame = page.getByTestId(`widget-frame-${WIDGET_KPI_ID}`);
  const barFrame = page.getByTestId(`widget-frame-${WIDGET_BAR_ID}`);
  const lowerFrame = page.getByTestId(`widget-frame-${WIDGET_LOWER_ID}`);

  // 3. Switch to FREE_FLOAT ("锁定位置") and verify lower card does NOT compact/move when upper card moves
  await page.getByTestId("grid-compact-free-float-btn").click();
  await expect(page.getByTestId("dashboard-grid")).toHaveAttribute(
    "data-compact-mode",
    "FREE_FLOAT"
  );
  await expect
    .poll(() => state.compactMode)
    .toBe("FREE_FLOAT");
  await page.waitForTimeout(300);

  const gridTrackContainer = page.getByTestId("dashboard-grid-tracks");
  const gridBefore = await gridTrackContainer.boundingBox();
  const lowerBefore = await lowerFrame.boundingBox();
  expect(gridBefore).not.toBeNull();
  expect(lowerBefore).not.toBeNull();
  const lowerRelYBefore = lowerBefore!.y - gridBefore!.y;

  // Drag BAR card upward from y=1 to y=0 (opening empty row y=4 above lower card at y=5)
  const barHandle = barFrame.locator(".widget-drag-handle").first();
  const barHandleBox = await barHandle.boundingBox();
  expect(barHandleBox).not.toBeNull();
  await page.mouse.move(
    barHandleBox!.x + barHandleBox!.width / 2,
    barHandleBox!.y + barHandleBox!.height / 2
  );
  await page.mouse.down();
  await page.mouse.move(
    barHandleBox!.x + barHandleBox!.width / 2,
    barHandleBox!.y + barHandleBox!.height / 2 - 88,
    { steps: 8 }
  );
  await page.mouse.up();

  // Drag KPI card downward by 1 row to y=1 so upper cards have |Δy|=1 and w=5+6=11
  const kpiHandle = kpiFrame.locator(".widget-drag-handle").first();
  const kpiHandleBox = await kpiHandle.boundingBox();
  expect(kpiHandleBox).not.toBeNull();
  await page.mouse.move(
    kpiHandleBox!.x + kpiHandleBox!.width / 2,
    kpiHandleBox!.y + kpiHandleBox!.height / 2
  );
  await page.mouse.down();
  await page.mouse.move(
    kpiHandleBox!.x + kpiHandleBox!.width / 2,
    kpiHandleBox!.y + kpiHandleBox!.height / 2 + 88,
    { steps: 8 }
  );
  await page.mouse.up();
  await page.waitForTimeout(260);

  // Lower card grid-relative Y position at y=5 must remain completely untouched in FREE_FLOAT mode
  const gridAfterDrag = await gridTrackContainer.boundingBox();
  const lowerAfterDrag = await lowerFrame.boundingBox();
  expect(gridAfterDrag).not.toBeNull();
  expect(lowerAfterDrag).not.toBeNull();
  const lowerRelYAfter = lowerAfterDrag!.y - gridAfterDrag!.y;
  expect(Math.abs(lowerRelYAfter - lowerRelYBefore)).toBeLessThan(2);
  expect(state.widgetPositions[WIDGET_LOWER_ID]?.y).toBe(5);

  // 4. Click Tidy ("对齐整理") + Save Layout ("保存布局") -> near-same-row upper cards snap to identical top Y and full 12-col width
  await page.getByTestId("grid-tidy-layout-btn").click();
  await page.getByTestId("save-layout-btn").click();
  await expect
    .poll(() => {
      const kpiPos = state.widgetPositions[WIDGET_KPI_ID];
      const barPos = state.widgetPositions[WIDGET_BAR_ID];
      return (
        kpiPos !== undefined &&
        barPos !== undefined &&
        kpiPos.y === barPos.y &&
        kpiPos.h === barPos.h &&
        kpiPos.w + barPos.w === 12
      );
    })
    .toBe(true);
  await page.waitForTimeout(260);

  const kpiTidyBox = await kpiFrame.boundingBox();
  const barTidyBox = await barFrame.boundingBox();
  expect(kpiTidyBox).not.toBeNull();
  expect(barTidyBox).not.toBeNull();
  expect(Math.abs(kpiTidyBox!.y - barTidyBox!.y)).toBeLessThan(2);

  expect(leakedTenantUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
});

if (process.env.AXPOINT_E2E_LIVE === "1") {
  test("live backend Step 25: SQL -> Chart Studio -> Save xv_charts -> Pin to real cockpit & VTable Pivot render", async ({
    page,
    request,
  }) => {
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err.message);
    });

    const coreBaseUrl = (
      process.env.CORE_BASE_URL ?? "http://127.0.0.1:9995"
    ).replace(/\/$/, "");
    const devToken =
      process.env.VITE_DEV_TOKEN ??
      process.env.AXPOINT_DEV_TOKEN ??
      "dev-smoke-token-00000000000000000000";

    const createDashResp = await request.post(
      `${coreBaseUrl}/api/xblueview/dashboards`,
      {
        headers: {
          Authorization: `Bearer ${devToken}`,
          "Content-Type": "application/json",
        },
        data: {
          name: "Step25 Live Pin Target",
          slug: `step25-live-${Date.now().toString(36)}`,
          theme: "light",
          zone_layout: "STANDARD_FIVE_ZONE",
        },
      }
    );
    expect(createDashResp.ok()).toBe(true);
    const createdDashJson = (await createDashResp.json()) as {
      id?: string;
      dashboard?: { id?: string };
    };
    const tempDashId =
      createdDashJson.dashboard?.id ?? createdDashJson.id ?? "";
    expect(tempDashId.length).toBeGreaterThan(0);

    let createdChartId = "";
    page.on("response", async (resp) => {
      if (
        resp.url().endsWith("/api/xblueview/charts") &&
        resp.request().method() === "POST" &&
        resp.status() === 201
      ) {
        try {
          const body = (await resp.json()) as { id?: string };
          if (body.id) createdChartId = body.id;
        } catch {
          // Ignore parse error
        }
      }
    });

    try {
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

      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto("/analytics");

      await page.getByTestId("sql-run-btn").click();
      await expect(
        page.getByTestId("result-open-chart-studio-btn")
      ).toBeVisible({ timeout: 20_000 });
      await page.getByTestId("result-open-chart-studio-btn").click();

      await expect(page.getByTestId("chart-studio-panel")).toBeVisible();
      const uniqueName = `实收分布图-${Date.now().toString(36)}`;
      await page.getByTestId("chart-studio-name-input").fill(uniqueName);
      await page.getByTestId("chart-studio-save-btn").click();
      await expect(page.getByTestId("chart-studio-status-badge")).toContainText(
        "已保存",
        { timeout: 15_000 }
      );

      await page.getByTestId("chart-studio-pin-btn").click();
      await expect(page.getByTestId("pin-to-dashboard-modal")).toBeVisible();
      await chooseSelectOption(page, "pin-modal-dashboard-select", tempDashId);
      await page.getByTestId("pin-modal-confirm-btn").click();
      await expect(page.getByTestId("pin-modal-success")).toContainText(
        "已钉入",
        { timeout: 15_000 }
      );
      await page.getByTestId("pin-modal-close-btn").click();

      // Verify live PivotBuilder + VTable Canvas
      await page.getByTestId("analytics-tab-pivot").click();
      await expect(page.getByTestId("analytics-pivot-builder")).toBeVisible();
      const firstQuickVal = page
        .locator("[data-testid^='pivot-quick-val-']")
        .first();
      await expect(firstQuickVal).toBeVisible({ timeout: 15_000 });
      await firstQuickVal.click();
      await expect(page.getByTestId("vtable-canvas-stage")).toBeVisible({
        timeout: 20_000,
      });

      expect(pageErrors).toEqual([]);
    } finally {
      await request
        .delete(`${coreBaseUrl}/api/xblueview/dashboards/${tempDashId}`, {
          headers: { Authorization: `Bearer ${devToken}` },
        })
        .catch(() => {});
      if (createdChartId) {
        await request
          .delete(`${coreBaseUrl}/api/xblueview/charts/${createdChartId}`, {
            headers: { Authorization: `Bearer ${devToken}` },
          })
          .catch(() => {});
      }
    }
  });
}
