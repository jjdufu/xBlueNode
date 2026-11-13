import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const WAREHOUSE_W1_ID = "00000000-0000-0000-0000-000000000301";
const FACTORY_F1_ID = "00000000-0000-0000-0000-000000000302";
const ORDER_O3_ID = "00000000-0000-0000-0000-000000000303";
const ORDER_O4_ID = "00000000-0000-0000-0000-000000000304";

async function installXBlueGraphMocks(
  page: Page,
  leakedTenantUrls: string[],
  missingAuthUrls: string[]
): Promise<void> {
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
          trace_id: "xbluegraph-e2e-400",
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
          trace_id: "xbluegraph-e2e-401",
        }),
      });
      return;
    }

    // Initial subgraph returns 2 nodes (F-1 -> W-1); 1-hop expand on W-1 adds O-3 and O-4!
    if (pathname === "/api/xblue/graph/subgraph") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: [
            {
              id: WAREHOUSE_W1_ID,
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              properties: { region: "华东", tier: "核心枢纽" },
              metrics: { inventory: 500, capacity: 900 },
              geo: { lng: 121.4737, lat: 31.2304 },
              row_version: 2,
            },
            {
              id: FACTORY_F1_ID,
              type_name: "Factory",
              node_key: "F-1",
              title: "昆山总装厂",
              properties: { region: "华东" },
              metrics: { output: 1200 },
              geo: { lng: 120.9807, lat: 31.3856 },
              row_version: 1,
            },
          ],
          links: [
            {
              id: "00000000-0000-0000-0000-000000000310",
              link_type: "SUPPLIES",
              src_node_id: FACTORY_F1_ID,
              dst_node_id: WAREHOUSE_W1_ID,
              weight: 0.85,
              properties: {},
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          truncated: false,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/nodes/search") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: WAREHOUSE_W1_ID,
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              properties: { region: "华东" },
              metrics: { inventory: 500, capacity: 900 },
              geo: { lng: 121.4737, lat: 31.2304 },
              score: 0.98,
              channels: ["FTS", "VECTOR"],
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/graph/neighbors") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: [
            {
              id: WAREHOUSE_W1_ID,
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              metrics: { inventory: 500, capacity: 900 },
            },
            {
              id: FACTORY_F1_ID,
              type_name: "Factory",
              node_key: "F-1",
              title: "昆山总装厂",
              metrics: { output: 1200 },
            },
            {
              id: ORDER_O3_ID,
              type_name: "Order",
              node_key: "O-3",
              title: "华东履约单 O-3",
              metrics: { amount: 315 },
            },
            {
              id: ORDER_O4_ID,
              type_name: "Order",
              node_key: "O-4",
              title: "华东履约单 O-4",
              metrics: { amount: 420 },
            },
          ],
          links: [
            {
              id: "00000000-0000-0000-0000-000000000311",
              link_type: "FULFILLS",
              src_node_id: WAREHOUSE_W1_ID,
              dst_node_id: ORDER_O3_ID,
              weight: 0.8,
              properties: {},
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
            {
              id: "00000000-0000-0000-0000-000000000312",
              link_type: "FULFILLS",
              src_node_id: WAREHOUSE_W1_ID,
              dst_node_id: ORDER_O4_ID,
              weight: 0.8,
              properties: {},
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          truncated: false,
        }),
      });
      return;
    }

    if (
      pathname ===
      `/api/xblue/nodes/${WAREHOUSE_W1_ID}/metrics/inventory`
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          { ts: "2026-01-25T00:00:00Z", value: 460 },
          { ts: "2026-01-26T00:00:00Z", value: 485 },
          { ts: "2026-01-27T00:00:00Z", value: 510 },
          { ts: "2026-01-28T00:00:00Z", value: 495 },
          { ts: "2026-01-29T00:00:00Z", value: 530 },
          { ts: "2026-01-30T00:00:00Z", value: 500 },
        ]),
      });
      return;
    }

    if (pathname === `/api/xblue/nodes/${WAREHOUSE_W1_ID}/documents`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              doc_id: "00000000-0000-0000-0000-000000000320",
              title: "华东仓储履约作业规范",
              source_name: "supply_sop.md",
              entity_id: "00000000-0000-0000-0000-000000000321",
              entity_name: "华东仓",
              snippets: ["华东仓负责长三角核心区域订单 24 小时出库履约。"],
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/entities/candidates") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/xblue/graph/path") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          path: [FACTORY_F1_ID, WAREHOUSE_W1_ID, ORDER_O3_ID],
          nodes: [],
          links: [],
          total_cost: 2.426,
        }),
      });
      return;
    }

    if (pathname.startsWith("/api/xblue/graph/algorithms/")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          algo: "PAGERANK",
          params_hash: "pr-default",
          node_count: 4,
          edge_count: 3,
          duration_ms: 3,
          top: [
            {
              node_id: WAREHOUSE_W1_ID,
              title: "华东仓",
              score: 0.4125,
              community: null,
            },
            {
              node_id: FACTORY_F1_ID,
              title: "昆山总装厂",
              score: 0.251,
              community: null,
            },
          ],
        }),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [] }),
    });
  });
}

test("xBlueGraph /graph locates 华东仓 (W-1), expands 1-hop neighbors on double-click increasing node count, and renders inventory trend line chart", async ({
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

  if (process.env.PLAYWRIGHT_LIVE_API !== "1") {
    await installXBlueGraphMocks(page, leakedTenantUrls, missingAuthUrls);
  }

  await page.goto("/graph");

  await expect(
    page.getByRole("heading", { level: 1, name: "xBlueGraph · 拓扑浏览器" })
  ).toBeVisible();

  // Verify main force graph canvas is mounted and non-empty
  const graphCanvas = page
    .getByTestId("xbluegraph-canvas")
    .locator("canvas")
    .first();
  await expect(graphCanvas).toBeVisible();

  // Search and locate 华东仓 (W-1)
  await page.getByTestId("graph-search-input").fill("华东");
  await page.getByTestId("graph-search-btn").click();
  await expect(page.getByTestId("search-hit-W-1")).toBeVisible();
  await page.getByTestId("search-hit-W-1").click();

  const nodeCountEl = page.getByTestId("canvas-node-count");
  await expect(nodeCountEl).toBeVisible();
  const initialCount = Number((await nodeCountEl.textContent()) ?? "0");
  expect(initialCount).toBeGreaterThanOrEqual(1);

  // Double-click 华东仓 (W-1) to expand 1-hop neighbors & assert node count increases
  const w1NodeChip = page.getByTestId("graph-node-W-1");
  await expect(w1NodeChip).toContainText("华东仓");
  await w1NodeChip.dblclick();

  await expect
    .poll(async () => Number((await nodeCountEl.textContent()) ?? "0"))
    .toBeGreaterThan(initialCount);

  // Inspect 华东仓 details & verify inventory metric line chart canvas is visible and non-empty
  const inspector = page.getByTestId("node-inspector");
  await expect(inspector).toBeVisible();
  await expect(page.getByTestId("inspector-node-title")).toHaveText("华东仓");
  await expect(page.getByTestId("inspector-node-key")).toHaveText("W-1");

  await page.getByTestId("metric-item-inventory").click();

  const metricChartCanvas = page
    .getByTestId("metric-line-chart")
    .locator("canvas")
    .first();
  await expect(metricChartCanvas).toBeVisible();

  const chartBox = await metricChartCanvas.boundingBox();
  expect(chartBox?.width ?? 0).toBeGreaterThan(80);
  expect(chartBox?.height ?? 0).toBeGreaterThan(80);

  const hasNonZeroPixels = await metricChartCanvas.evaluate(
    (canvas: HTMLCanvasElement) => {
      const ctx = canvas.getContext("2d");
      if (!ctx || canvas.width === 0 || canvas.height === 0) return false;
      const img = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
      for (let i = 3; i < img.length; i += 4) {
        if ((img[i] ?? 0) > 0) return true;
      }
      return false;
    }
  );
  expect(hasNonZeroPixels).toBe(true);

  // Also verify PathFinder & AlgoPanel tabs execute without errors
  await page.getByTestId("side-tab-path").click();
  await page.getByTestId("path-find-submit").click();
  await expect(page.getByTestId("path-result-box")).toBeVisible();

  await page.getByTestId("side-tab-algo").click();
  await page.getByTestId("algo-run-btn").click();
  await expect(page.getByTestId("algo-results")).toContainText("华东仓");

  await expect(page.locator("[data-testid$='error-banner']")).toHaveCount(0);
  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
