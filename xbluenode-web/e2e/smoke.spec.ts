import { expect, type Page, type Route, test } from "@playwright/test";

interface SmokeRouteAssertion {
  readonly path: string;
  readonly sidebarLabel: string;
  readonly heading: string;
}

function makeSmokeJwt(): string {
  const encodeSeg = (obj: Record<string, unknown>): string =>
    btoa(JSON.stringify(obj))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  const nowSec = Math.floor(Date.now() / 1000);
  return `${encodeSeg({ alg: "EdDSA", typ: "JWT", kid: "k1" })}.${encodeSeg({
    iss: "axpoint",
    aud: "axpoint-api",
    sub: "00000000-0000-0000-0000-000000000000",
    tid: "default",
    dept: "default",
    roles: ["ADMIN"],
    typ: "access",
    iat: nowSec,
    exp: nowSec + 900,
    jti: "smoke-jti-1",
  })}.smoke_eddsa_sig`;
}

const SHELL_SMOKE_ROUTES: readonly SmokeRouteAssertion[] = [
  { path: "/", sidebarLabel: "总控看板", heading: "总控看板" },
  { path: "/hub", sidebarLabel: "数据与知识", heading: "数据与知识" },
  { path: "/studio", sidebarLabel: "建模工作室", heading: "建模工作室" },
  { path: "/analytics", sidebarLabel: "查询与制图", heading: "查询与制图" },
  { path: "/sim", sidebarLabel: "推演沙盘", heading: "推演沙盘" },
  { path: "/flow", sidebarLabel: "编排与告警", heading: "编排与告警" },
  { path: "/view", sidebarLabel: "数字看板", heading: "数字看板" },
  { path: "/agents", sidebarLabel: "智能体与接口", heading: "智能体与接口" },
  { path: "/settings", sidebarLabel: "系统设置", heading: "系统设置" },
  { path: "/governance", sidebarLabel: "系统设置", heading: "系统设置" },
  { path: "/ingest", sidebarLabel: "数据与知识", heading: "数据与知识" },
  { path: "/query", sidebarLabel: "查询与制图", heading: "查询与制图" },
  { path: "/graph", sidebarLabel: "建模工作室", heading: "建模工作室" },
  { path: "/alerts", sidebarLabel: "编排与告警", heading: "编排与告警" },
  { path: "/guard", sidebarLabel: "系统设置", heading: "权限与安全" },
  { path: "/ops", sidebarLabel: "系统设置", heading: "运维与备份" },
];

async function installDeterministicApiMocks(
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
          trace_id: "smoke-trace-400",
        }),
      });
      return;
    }

    if (pathname === "/api/health") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          status: "ok",
          service: "axpoint-core",
          embedding_model: "bge-m3",
          embedding_dim: 1024,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/config") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          env: "development",
          oidc_enabled: true,
          oidc_display_name: "企业单点登录 (OIDC)",
          default_tenant: "default",
        }),
      });
      return;
    }

    if (pathname === "/api/auth/login" && request.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: makeSmokeJwt(),
          token_type: "Bearer",
          expires_in: 900,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/refresh" && request.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: makeSmokeJwt(),
          token_type: "Bearer",
          expires_in: 900,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/logout" && request.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
      });
      return;
    }

    const authHeader = request.headers()["authorization"] ?? "";
    if (!authHeader.startsWith("Bearer ") || authHeader.slice(7).trim().length === 0) {
      missingAuthUrls.push(request.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "smoke-trace-401",
        }),
      });
      return;
    }

    if (pathname === "/api/auth/me") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          user_id: "00000000-0000-0000-0000-000000000000",
          tenant_id: "default",
          username: "dev",
          dept_id: "default",
          roles: ["ADMIN"],
          permissions: [
            "DATA_READ",
            "DATA_SYNC",
            "STUDIO_EDIT",
            "STUDIO_PUBLISH",
            "FLOW_EDIT",
            "FLOW_RUN",
            "SIM_RUN",
            "GUARD_ADMIN",
            "OPS_ADMIN",
          ],
          token_kind: "DEV_STATIC",
        }),
      });
      return;
    }

    if (pathname === "/api/stats") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          doc_count: 3,
          para_count: 18,
          entity_count: 12,
          edge_count: 24,
          embedding_count: 18,
          embedding_dims: [1024],
          timestamp: "2026-01-31T08:00:00Z",
        }),
      });
      return;
    }

    if (pathname === "/api/graph/topology") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: [
            {
              id: "00000000-0000-0000-0000-000000000001",
              node_type: "DOC",
              label: "供应链制度",
              weight: 3,
            },
            {
              id: "00000000-0000-0000-0000-000000000002",
              node_type: "ENTITY",
              label: "华东仓",
              weight: 2,
            },
          ],
          edges: [
            {
              id: "00000000-0000-0000-0000-000000000003",
              source: "00000000-0000-0000-0000-000000000001",
              target: "00000000-0000-0000-0000-000000000002",
              rel_type: "MENTIONS",
              fact: "华东仓负责履约订单",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/subgraphs") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000010",
            slug: "main",
            display_name: "企业主图",
            description: "企业权威生产主图",
            owner_dept: "default",
            is_main: true,
            current_version: 1,
            created_at: "2026-01-31T08:00:00Z",
            updated_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/xblue/schema") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: "main",
          version: 1,
          node_types: [
            {
              id: "00000000-0000-0000-0000-000000000011",
              name: "Warehouse",
              display_name: "仓库",
              properties: [{ name: "inventory", data_type: "number" }],
              agg_rules: [{ target_metric: "inventory", op: "SUM" }],
              pos_x: 120,
              pos_y: 120,
            },
          ],
          edge_types: [
            {
              id: "00000000-0000-0000-0000-000000000012",
              name: "FULFILLS",
              src_type_name: "Warehouse",
              dst_type_name: "Order",
              impact_weight: 0.8,
            },
          ],
          mappings: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/validate") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ valid: true, issues: [] }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/versions") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000013",
            subgraph_slug: "main",
            version: 1,
            change_summary: "基线模型快照",
            published_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/xblue/drafts" || pathname === "/api/xblue/mappings") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/xblue/nodes") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: [
            {
              id: "00000000-0000-0000-0000-000000000021",
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              metrics: { inventory: 500, output: 500 },
            },
          ],
          links: [],
        }),
      });
      return;
    }

    if (pathname === "/api/axflow/flows") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000031",
            name: "供应链日度产能同步",
            description: "多源数据摄取与业务节点映射",
            steps: [
              { id: "s1", kind: "INGEST", label: "多源数据摄取", config: {} },
            ],
            status: "PUBLISHED",
            last_run_at: null,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axsim/sandboxes") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000041",
            name: "华东应急推演",
            node_count: 1,
            link_count: 0,
            sim_result: null,
            created_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000000051",
              tenant_id: "default",
              name: "供应链总览",
              slug: "supply-overview",
              description: "华东供应链孪生监控看板",
              filters: [],
              theme: "light",
              canvas: { cols: 24, row_height: 30, screen_width: 1920, screen_height: 1080 },
              status: "PUBLISHED",
              version: 1,
              created_by: null,
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/axops/health/deep") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          database: { ok: true, latency_ms: 2, replication_lag_s: 0 },
          parser: { ok: true, version: "0.1.0" },
          lake: { ok: true, snapshot_id: 1 },
          llm: { ok: true, provider: "cassette" },
          embed: { ok: true },
          disk: { ok: true, data_root_free_bytes: 50_000_000_000 },
          jobs: { pending: 0, running: 0, failed_24h: 0, oldest_pending_s: 0 },
          outbox: { lag_s: 0, pending: 0 },
          migrations: { ok: true, current: 14, pending: 0 },
          version: "0.1.0",
        }),
      });
      return;
    }

    if (pathname === "/api/axops/outbox/stats") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ lag_s: 0, pending: 0 }),
      });
      return;
    }

    if (pathname === "/api/axops/system/info") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          version: "0.1.0",
          env: "development",
          tenant_id: "default",
          uptime_s: 3600,
          data_root: "/var/lib/axpoint",
          template_packs_count: 3,
        }),
      });
      return;
    }

    if (
      pathname === "/api/axops/jobs" ||
      pathname === "/api/axops/retention" ||
      pathname === "/api/axops/backups" ||
      pathname === "/api/axops/templates" ||
      pathname === "/api/axops/templates/installs" ||
      pathname === "/api/axops/work-items"
    ) {
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
}

test("all registered routes and /login (password + OIDC + PAT accordion) render cleanly with zero console errors", async ({
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

  await installDeterministicApiMocks(page, leakedTenantUrls, missingAuthUrls);

  for (const routeSpec of SHELL_SMOKE_ROUTES) {
    await page.goto(routeSpec.path);
    await expect(page.getByTestId("app-top-header")).toContainText("xBlueNode");
    await expect(page.locator("aside")).toBeVisible();
    await expect(
      page.getByRole("link", { name: routeSpec.sidebarLabel })
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { level: 1, name: routeSpec.heading }).first()
    ).toBeVisible();
  }

  // 1. Primary Username + Password login flow & dynamic OIDC button
  await page.goto("/login");
  await expect(
    page.getByRole("heading", { level: 1, name: "登录" })
  ).toBeVisible();
  await expect(page.getByTestId("oidc-login-btn")).toContainText(
    "企业单点登录 (OIDC)"
  );

  await page.getByTestId("login-username-input").fill("ops_admin");
  await page.getByTestId("login-password-input").fill("Ops#Admin2026!Safe");
  await page.getByTestId("login-submit-btn").click();
  await expect(page).toHaveURL(/\/$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "总控看板" })
  ).toBeVisible();

  // 2. Collapsible PAT login flow
  await page.goto("/login");
  await page.getByTestId("pat-accordion-toggle").click();
  await page.locator("#access-token-input").fill("smoke-candidate-token-0000000000000000");
  await page.getByTestId("pat-submit-btn").click();
  await expect(page).toHaveURL(/\/$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "总控看板" })
  ).toBeVisible();

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
