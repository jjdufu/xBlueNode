import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const CODENAME_PREFIX_RE =
  /\b(AxHub|AxIngest|AxAnalytics|AxQuery|xBlueStudio|xBlueGraph|AxFlow|AxAlerts|AxAgent|AxSim|xBlueView|AxGuard|AxOps)\s*·/;

const SCREAMING_SNAKE_RE = /\b[A-Z]{2,}_[A-Z0-9_]+\b/g;

function makeStep23Jwt(): string {
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
    jti: "step23-jti-1",
  })}.step23_eddsa_sig`;
}

async function installStep23Mocks(
  page: Page,
  leakedTenantUrls: string[],
  missingAuthUrls: string[]
): Promise<void> {
  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const method = request.method();
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
          trace_id: "step23-trace-400",
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

    if (pathname === "/api/auth/login" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: makeStep23Jwt(),
          token_type: "Bearer",
          expires_in: 900,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/refresh" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: makeStep23Jwt(),
          token_type: "Bearer",
          expires_in: 900,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/logout" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
      });
      return;
    }

    const authHeader = request.headers()["authorization"] ?? "";
    if (
      !authHeader.startsWith("Bearer ") ||
      authHeader.slice(7).trim().length === 0
    ) {
      missingAuthUrls.push(request.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "step23-trace-401",
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
          username: "ops_admin",
          dept_id: "default",
          roles: ["ADMIN"],
          permissions: [
            "DATA_READ",
            "DATA_SYNC",
            "STUDIO_EDIT",
            "STUDIO_PUBLISH",
            "FLOW_EDIT",
            "FLOW_RUN",
            "RULE_EDIT",
            "ALERT_ACK",
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
          doc_count: 4,
          para_count: 24,
          entity_count: 16,
          edge_count: 28,
          embedding_count: 24,
          embedding_dims: [1024],
          timestamp: "2026-01-31T08:00:00Z",
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
            description: "供应链权威主图",
            owner_dept: "default",
            is_main: true,
            current_version: 2,
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
          version: 2,
          node_types: [
            {
              id: "00000000-0000-0000-0000-000000000011",
              name: "Warehouse",
              display_name: "仓库",
              description: "区域履约仓储中心",
              icon: "warehouse",
              color: "#2563eb",
              title_template: "{node_key}",
              semantic_layer_tier: "STATIC_FACT",
              properties: [
                {
                  name: "warehouse_code",
                  display_name: "仓库编码",
                  description: "仓库主键编码",
                  kind: "ATTRIBUTE",
                  data_type: "STRING",
                  required: true,
                  pii_category: "NONE",
                },
                {
                  name: "inventory",
                  display_name: "在库库存",
                  description: "实时可用库存",
                  kind: "METRIC",
                  data_type: "FLOAT64",
                  unit: "件",
                  required: false,
                  pii_category: "NONE",
                },
                {
                  name: "total_demand",
                  display_name: "总需求量",
                  description: "下游履约订单聚合需求",
                  kind: "METRIC",
                  data_type: "FLOAT64",
                  unit: "件",
                  required: false,
                  pii_category: "NONE",
                },
              ],
              agg_rules: [
                {
                  target_metric: "total_demand",
                  op: "SUM",
                  via_link_type: "FULFILLS",
                  direction: "OUT",
                  source_type_name: "Order",
                  source_metric: "qty",
                  weight_by_link: false,
                },
              ],
              derived_rules: [],
              sync_to_knowledge: true,
              pos_x: 120,
              pos_y: 120,
            },
            {
              id: "00000000-0000-0000-0000-000000000014",
              name: "Order",
              display_name: "履约订单",
              description: "下游客户履约订单",
              icon: "shopping-cart",
              color: "#09090b",
              title_template: "订单 {node_key}",
              semantic_layer_tier: "EVENT_DYNAMIC",
              properties: [
                {
                  name: "qty",
                  display_name: "需求数量",
                  description: "订购件数",
                  kind: "METRIC",
                  data_type: "FLOAT64",
                  required: true,
                  pii_category: "NONE",
                },
              ],
              agg_rules: [],
              derived_rules: [],
              sync_to_knowledge: false,
              pos_x: 460,
              pos_y: 140,
            },
          ],
          edge_types: [
            {
              id: "00000000-0000-0000-0000-000000000012",
              name: "FULFILLS",
              src_type_name: "Warehouse",
              dst_type_name: "Order",
              cardinality: "ONE_TO_MANY",
              impact_weight: 0.85,
              description: "仓库向订单发货履约",
              properties: [],
              propagation: {},
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
            version: 2,
            change_summary: "供应链基线模型",
            published_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === "/api/xblue/drafts" ||
      pathname === "/api/xblue/mappings"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (
      pathname === "/api/xblue/nodes" ||
      pathname === "/api/xblue/graph/subgraph"
    ) {
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
              properties: { warehouse_code: "W-1" },
              metrics: { inventory: 500, total_demand: 320 },
              row_version: 1,
            },
          ],
          links: [],
          items: [
            {
              id: "00000000-0000-0000-0000-000000000021",
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              properties: { warehouse_code: "W-1" },
              metrics: { inventory: 500, total_demand: 320 },
              row_version: 1,
            },
          ],
          truncated: false,
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/connections") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000000101",
              name: "demo_pg",
              display_name: "华东生产库",
              kind: "POSTGRES",
              status: "OK",
              secret_fields_set: ["password"],
              config: { host: "127.0.0.1", port: 5432, database: "supply" },
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/datasets") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000000102",
              name: "demo_orders",
              display_name: "履约订单明细",
              source_kind: "DB_TABLE",
              status: "READY",
              row_count: 100,
              columns: [
                { name: "order_id", data_type: "VARCHAR", nullable: false },
                { name: "amount", data_type: "DOUBLE", nullable: false },
              ],
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/catalog") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          datasets: [
            {
              id: "00000000-0000-0000-0000-000000000102",
              name: "demo_orders",
              display_name: "履约订单明细",
              row_count: 100,
              columns: [
                { name: "order_id", data_type: "VARCHAR" },
                { name: "amount", data_type: "DOUBLE" },
              ],
            },
          ],
          platform_views: [],
          live_connections: [],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/metrics") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000201",
            name: "paid_order_amount",
            display_name: "已支付订单额",
            description: "状态为已支付的订单金额合计",
            dataset_id: "00000000-0000-0000-0000-000000000102",
            dataset_name: "demo_orders",
            expression: "SUM(amount)",
            filter_sql: "status = 'PAID'",
            time_column: "updated_at",
            default_grain: "MONTH",
            dimensions: [
              { name: "status", column: "status", display_name: "订单状态" },
            ],
            unit: "CNY",
            format: "0,0.00",
            synonyms: ["GMV", "成交额"],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: null,
            version: 1,
            created_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === "/api/analytics/saved-queries" ||
      pathname === "/api/analytics/history" ||
      pathname === "/api/axagent/agents"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/axflow/flows") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000000031",
              name: "daily_supply_sync",
              slug: "daily_supply_sync",
              description: "日度库存抽取与指标判定",
              nodes: [
                {
                  id: "n1",
                  kind: "DATA_EXTRACT",
                  title: "订单增量同步",
                  pos_x: 80,
                  pos_y: 120,
                  config: {},
                },
                {
                  id: "n2",
                  kind: "RULE_EVAL",
                  title: "安全库存阈值判定",
                  pos_x: 360,
                  pos_y: 120,
                  config: {},
                },
              ],
              edges: [{ id: "e1", source: "n1", target: "n2" }],
              trigger_cfg: { kind: "MANUAL" },
              status: "PUBLISHED",
              latest_version: 1,
              issues: [],
            },
          ],
          next_cursor: null,
        }),
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
            status: "READY",
            scope: {
              depth: 3,
              direction: "BOTH",
              type_names: ["Warehouse", "Order"],
            },
            node_count: 2,
            link_count: 1,
            base_captured_at: "2026-01-31T08:00:00Z",
            schema_version: 2,
            sim_result: null,
            created_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    const simGraphMatch = pathname.match(
      /^\/api\/axsim\/sandboxes\/([^/]+)\/graph$/
    );
    if (simGraphMatch && method === "GET") {
      const sbId = simGraphMatch[1] ?? "00000000-0000-0000-0000-000000000041";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sandbox_id: sbId,
          nodes: [
            {
              id: "00000000-0000-0000-0000-000000000021",
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              properties: {},
              metrics: { inventory: 500 },
              overlay_state: "BASE",
            },
          ],
          links: [],
          overlay_hash: "ov-hash-1",
        }),
      });
      return;
    }

    if (
      /^\/api\/axsim\/sandboxes\/[^/]+\/(scenarios|decisions)$/.test(pathname)
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
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
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards/by-slug/supply-overview") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: "00000000-0000-0000-0000-000000000051",
          tenant_id: "default",
          name: "供应链总览",
          slug: "supply-overview",
          description: "华东供应链孪生监控看板",
          filters: [],
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
          widgets: [],
        }),
      });
      return;
    }

    if (pathname === "/api/guard/users") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000000000",
              username: "ops_admin",
              display_name: "平台管理员",
              dept_id: "default",
              roles: ["ADMIN"],
              status: "ACTIVE",
              created_at: "2026-01-31T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/guard/roles") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "r-admin",
              name: "ADMIN",
              display_name: "系统管理员",
              permissions: ["DATA_READ", "STUDIO_EDIT", "GUARD_ADMIN"],
              is_builtin: true,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/guard/audit") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: 1,
              actor_id: "00000000-0000-0000-0000-000000000000",
              actor_name: "ops_admin",
              action: "SCHEMA_PUBLISH",
              resource_kind: "SCHEMA_DOC",
              resource_id: "main@v2",
              decision: "ALLOW",
              prev_hash: "0000",
              entry_hash: "a1b2c3d4",
              detail: {},
              created_at: "2026-01-31T08:00:00Z",
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

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [], next_cursor: null }),
    });
  });
}

async function ensureSignedIn(page: Page): Promise<void> {
  const isLive = process.env.PLAYWRIGHT_LIVE_API === "1";
  if (!isLive) {
    await page.goto("/login");
    await expect(
      page.getByRole("heading", { level: 1, name: "登录" })
    ).toBeVisible();
    await page.getByTestId("login-tenant-input").fill("default");
    await page.getByTestId("login-username-input").fill("ops_admin");
    await page.getByTestId("login-password-input").fill("dev-pass-step23");
    await page.getByTestId("login-submit-btn").click();
    await expect(page).toHaveURL(/\/$/);
    return;
  }
  await page.goto("/");
  await expect(page.getByTestId("app-sidebar")).toBeVisible();
}

async function collectUiEnumLeaks(page: Page): Promise<string[]> {
  return page.evaluate((patternSource: string) => {
    const re = new RegExp(patternSource, "g");
    const allow = new Set(["SHA_256", "AES_256"]);
    const selectors = [
      "h1",
      "h2",
      "h3",
      "nav a",
      "button",
      "[data-testid*='badge']",
      "[data-testid*='status']",
    ];
    const leaks = new Set<string>();
    for (const sel of selectors) {
      const nodes = document.querySelectorAll<HTMLElement>(sel);
      for (const el of nodes) {
        if (el.offsetParent === null) continue;
        if (el.closest("code, pre, textarea, .font-mono-tech")) continue;
        const text = (el.innerText || "").trim();
        const matches = text.match(re);
        if (!matches) continue;
        for (const token of matches) {
          if (!allow.has(token)) {
            leaks.add(`${sel}: ${token}`);
          }
        }
      }
    }
    return [...leaks];
  }, SCREAMING_SNAKE_RE.source);
}

test("Step 23 Gate: 8-domain IA sidebar, legacy route redirects, 4-quadrant bilingual purity, cmdk jump, and ResizableWorkspace persistence", async ({
  page,
}) => {
  test.setTimeout(90_000);
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
    await installStep23Mocks(page, leakedTenantUrls, missingAuthUrls);
  }

  await page.setViewportSize({ width: 1440, height: 900 });
  await ensureSignedIn(page);

  // =========================================================================
  // 1. ia-8-domains: 8 Primary Domains + Bottom Governance & Zero Codename
  // =========================================================================
  const primaryNav = page.getByTestId("primary-domain-nav");
  await expect(primaryNav).toBeVisible();
  const primaryLinks = primaryNav.locator("a");
  await expect(primaryLinks).toHaveCount(8);

  const primaryLabels = await primaryLinks.allInnerTexts();
  expect(primaryLabels.map((s) => s.trim())).toEqual([
    "总控看板",
    "数据与知识",
    "建模工作室",
    "查询与制图",
    "推演沙盘",
    "编排与告警",
    "数字看板",
    "智能体与接口",
  ]);

  const governanceSection = page.getByTestId("governance-domain-section");
  await expect(governanceSection).toBeVisible();
  const govLink = page.getByTestId("sidebar-nav-settings");
  await expect(govLink).toBeVisible();
  await expect(govLink).toHaveText("系统设置");

  const sidebarText = await page.getByTestId("app-sidebar").innerText();
  expect(sidebarText).not.toMatch(CODENAME_PREFIX_RE);
  expect(sidebarText).not.toMatch(
    /\b(AxHub|AxIngest|AxAnalytics|AxQuery|xBlueStudio|xBlueGraph|AxFlow|AxAlerts|AxAgent|AxSim|xBlueView|AxGuard|AxOps)\b/
  );

  // =========================================================================
  // 2. legacy-route-redirect & ?tab= Workbench Convergence
  // =========================================================================
  await page.goto("/ingest");
  await expect(page).toHaveURL(/\/hub\?tab=knowledge/);
  await expect(
    page.getByRole("heading", { level: 1, name: "数据与知识" })
  ).toBeVisible();

  await page.goto("/query?q=%E5%BA%93%E5%AD%98");
  await expect(page).toHaveURL(/\/analytics\?.*tab=ask/);
  await expect(page).toHaveURL(/q=%E5%BA%93%E5%AD%98/);
  await expect(
    page.getByRole("heading", { level: 1, name: "查询与制图" })
  ).toBeVisible();
  await expect(page.getByTestId("axquery-input")).toBeVisible();

  await page.goto("/graph");
  await expect(page).toHaveURL(/\/studio\?tab=explorer/);
  await expect(
    page.getByRole("heading", { level: 1, name: "建模工作室" })
  ).toBeVisible();
  await expect(page.getByTestId("xbluegraph-canvas")).toBeVisible();

  await page.goto("/alerts");
  await expect(page).toHaveURL(/\/flow\?tab=alerts/);
  await expect(
    page.getByRole("heading", { level: 1, name: "编排与告警" })
  ).toBeVisible();
  await expect(page.getByTestId("axalerts-page")).toBeVisible();

  await page.goto("/governance?tab=access");
  await expect(page).toHaveURL(/\/settings\?tab=access/);
  await expect(
    page.getByRole("heading", { level: 1, name: "系统设置" })
  ).toBeVisible();
  await expect(page.getByTestId("axguard-tab-users")).toBeVisible();

  await page.goto("/governance?tab=jobs");
  await expect(page).toHaveURL(/\/settings\?tab=jobs/);
  await expect(
    page.getByRole("heading", { level: 1, name: "系统设置" })
  ).toBeVisible();
  await expect(page.getByTestId("axops-tab-jobs")).toBeVisible();

  // =========================================================================
  // 3. 4-Quadrant Bilingual Purity (`zh-CN` & `en-US` x `light` & `dark`)
  // =========================================================================
  const zhDomainRoutes: ReadonlyArray<{ path: string; h1: string }> = [
    { path: "/", h1: "总控看板" },
    { path: "/hub?tab=datasets", h1: "数据与知识" },
    { path: "/studio", h1: "建模工作室" },
    { path: "/analytics?tab=semantic", h1: "查询与制图" },
    { path: "/sim", h1: "推演沙盘" },
    { path: "/flow", h1: "编排与告警" },
    { path: "/view", h1: "数字看板" },
    { path: "/agents", h1: "智能体与接口" },
    { path: "/settings", h1: "系统设置" },
  ];

  for (const route of zhDomainRoutes) {
    await page.goto(route.path);
    await expect(
      page.getByRole("heading", { level: 1, name: route.h1 })
    ).toBeVisible();
    const bodyText = await page.locator("body").innerText();
    expect(bodyText).not.toMatch(CODENAME_PREFIX_RE);
    expect(bodyText).not.toContain("Cedar ABAC");
    expect(bodyText).not.toContain("Hybrid 5-Way RRF");
    const enumLeaks = await collectUiEnumLeaks(page);
    expect(enumLeaks).toEqual([]);
  }

  // Specific Studio & Flow localized enum checks in zh-CN
  await page.goto("/studio");
  const warehouseCard = page.getByTestId("canvas-node-Warehouse");
  await expect(warehouseCard).toBeVisible();
  await expect(warehouseCard).toContainText("主数据");
  await expect(warehouseCard).toContainText("属性");
  await expect(warehouseCard).toContainText("指标");
  await expect(warehouseCard).not.toContainText("STATIC_FACT");
  await expect(warehouseCard).not.toContainText("FLOAT64");

  await page.goto("/flow");
  await expect(page.getByTestId("resizable-workspace-flow")).toBeVisible();
  const paletteText = await page
    .getByTestId("resizable-pane-flow-palette")
    .innerText();
  expect(paletteText).not.toContain("DATA_EXTRACT");
  expect(paletteText).not.toContain("RULE_EVAL");

  // Toggle dark mode in zh-CN
  await page.getByTestId("theme-toggle-btn").click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { level: 1, name: "总控看板" })
  ).toBeVisible();
  expect(await collectUiEnumLeaks(page)).toEqual([]);

  // Toggle locale to en-US (dark mode -> then light mode)
  await page.getByTestId("locale-toggle-btn").click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Dashboard" })
  ).toBeVisible();
  const enPrimaryLabels = await primaryLinks.allInnerTexts();
  expect(enPrimaryLabels.map((s) => s.trim())).toEqual([
    "Dashboard",
    "Data Hub",
    "Modeling Studio",
    "Analytics & Charts",
    "Simulation",
    "Automation & Alerts",
    "Cockpits",
    "Agents & APIs",
  ]);
  await expect(govLink).toHaveText("Settings");

  // Restore light mode + zh-CN
  await page.getByTestId("theme-toggle-btn").click();
  await expect(page.locator("html")).not.toHaveClass(/dark/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Dashboard" })
  ).toBeVisible();
  await page.getByTestId("locale-toggle-btn").click();
  await expect(
    page.getByRole("heading", { level: 1, name: "总控看板" })
  ).toBeVisible();

  // =========================================================================
  // 4. cmdk-jump-ok: Global ⌘K / Ctrl+K Command Palette Search & Jump
  // =========================================================================
  await page.keyboard.press("Control+k");
  const cmdkDialog = page.getByTestId("global-cmdk-dialog");
  await expect(cmdkDialog).toBeVisible();
  const cmdkInput = page.getByTestId("global-cmdk-input");
  await expect(cmdkInput).toBeFocused();

  await expect(cmdkDialog).toContainText("快捷动作");
  await expect(cmdkDialog).toContainText("工作台视图");

  // Chinese query jump -> /analytics?tab=pivot
  await cmdkInput.fill("透视");
  await expect(page.getByTestId("cmdk-item-nav-analytics-pivot")).toBeVisible();
  await cmdkInput.press("Enter");
  await expect(cmdkDialog).toHaveCount(0);
  await expect(page).toHaveURL(/\/analytics\?tab=pivot/);

  // Slug query jump -> live cockpit /view/supply-overview
  await page.keyboard.press("Meta+k");
  await expect(cmdkDialog).toBeVisible();
  await expect(page.getByTestId("cmdk-item-view-db-supply-overview")).toBeVisible();
  await cmdkInput.fill("supply-overview");
  await cmdkInput.press("Enter");
  await expect(cmdkDialog).toHaveCount(0);
  await expect(page).toHaveURL(/\/view\/supply-overview/);

  // Escape closes cleanly
  await page.keyboard.press("Control+k");
  await expect(cmdkDialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(cmdkDialog).toHaveCount(0);

  // =========================================================================
  // 5. resizable-pane-persist: Drag Resize, Reload Persistence & Double-Click Collapse
  // =========================================================================
  await page.goto("/studio");
  const studioWorkspace = page.getByTestId("resizable-workspace-studio");
  await expect(studioWorkspace).toBeVisible();

  const inspectorPane = page.getByTestId("resizable-pane-studio-inspector");
  const rightHandle = studioWorkspace.getByTestId("resizable-handle-right");
  await expect(inspectorPane).toBeVisible();
  await expect(rightHandle).toBeVisible();

  const initialInspectorBox = await inspectorPane.boundingBox();
  const handleBox = await rightHandle.boundingBox();
  expect(initialInspectorBox).not.toBeNull();
  expect(handleBox).not.toBeNull();

  const initialWidth = initialInspectorBox!.width;
  const startX = handleBox!.x + handleBox!.width / 2;
  const startY = handleBox!.y + handleBox!.height / 2;

  // Drag separator 140px left -> right inspector pane grows wider
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX - 140, startY, { steps: 10 });
  await page.mouse.up();

  await expect
    .poll(async () => (await inspectorPane.boundingBox())?.width ?? 0)
    .toBeGreaterThan(initialWidth + 60);

  const widenedWidth = (await inspectorPane.boundingBox())!.width;
  const storedLayoutJson = await page.evaluate(() =>
    localStorage.getItem("axpoint.layout.studio")
  );
  expect(storedLayoutJson).toBeTruthy();

  // Reload page and verify widened width persisted via localStorage
  await page.reload();
  await expect(studioWorkspace).toBeVisible();
  const reloadedWidth = (await inspectorPane.boundingBox())!.width;
  expect(Math.abs(reloadedWidth - widenedWidth)).toBeLessThan(24);
  expect(reloadedWidth).toBeGreaterThan(initialWidth + 50);

  // Double-click separator to collapse inspector, then double-click again to restore
  await rightHandle.dblclick();
  await expect
    .poll(async () => (await inspectorPane.boundingBox())?.width ?? 999)
    .toBeLessThanOrEqual(12);

  await rightHandle.dblclick();
  await expect
    .poll(async () => (await inspectorPane.boundingBox())?.width ?? 0)
    .toBeGreaterThan(180);

  // Verify ResizableWorkspace shells across the remaining multi-pane workbenches
  await page.goto("/studio?tab=explorer");
  await expect(
    page.getByTestId("resizable-workspace-studio-explorer")
  ).toBeVisible();

  await page.goto("/analytics");
  await expect(page.getByTestId("resizable-workspace-analytics")).toBeVisible();

  await page.goto("/sim?tab=simulate");
  await expect(page.getByTestId("resizable-workspace-sim")).toBeVisible();

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
