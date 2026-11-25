import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

function makeStep31Jwt(): string {
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
    jti: "step31-jti-1",
  })}.step31_eddsa_sig`;
}

async function installStep31Mocks(
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
          trace_id: "step31-trace-400",
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
          access_token: makeStep31Jwt(),
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
          access_token: makeStep31Jwt(),
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
          trace_id: "step31-trace-401",
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
            "DATA_ADMIN",
            "STUDIO_EDIT",
            "STUDIO_PUBLISH",
            "FLOW_EDIT",
            "FLOW_RUN",
            "FLOW_PUBLISH",
            "RULE_EDIT",
            "ALERT_ACK",
            "AGENT_EDIT",
            "AGENT_RUN",
            "SIM_RUN",
            "SIM_APPLY",
            "VIEW_EDIT",
            "VIEW_SHARE",
            "AUDIT_READ",
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
            description: "供应链权威生产主图",
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
                  kind: "ATTRIBUTE",
                  data_type: "STRING",
                  required: true,
                  pii_category: "NONE",
                },
                {
                  name: "inventory",
                  display_name: "在库库存",
                  kind: "METRIC",
                  data_type: "FLOAT64",
                  unit: "件",
                  required: false,
                  pii_category: "NONE",
                },
              ],
              agg_rules: [],
              derived_rules: [],
              action_specs: [],
              pos_x: 120,
              pos_y: 120,
            },
          ],
          edge_types: [
            {
              id: "00000000-0000-0000-0000-000000000012",
              name: "FULFILLS",
              display_name: "履约供给",
              src_type_name: "Warehouse",
              dst_type_name: "Warehouse",
              cardinality: "ONE_TO_MANY",
              impact_weight: 0.8,
            },
          ],
          mappings: [],
        }),
      });
      return;
    }

    if (
      pathname === "/api/xblue/drafts" ||
      pathname === "/api/xblue/mappings" ||
      pathname === "/api/xblue/schema/versions" ||
      pathname === "/api/analytics/metrics" ||
      pathname === "/api/axsim/sandboxes" ||
      pathname === "/api/axagent/agents" ||
      pathname === "/api/axagent/endpoints" ||
      pathname === "/api/axagent/mcp-clients" ||
      pathname === "/api/axagent/usage" ||
      pathname === "/api/axagent/llm-preferences" ||
      pathname === "/api/axguard/policies" ||
      pathname === "/api/axguard/row-policies" ||
      pathname === "/api/axguard/row-policies/defaults" ||
      pathname === "/api/axguard/column-policies" ||
      pathname === "/api/axguard/grants"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/axagent/providers") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000071",
            tenant_id: "default",
            name: "primary-llm",
            base_url: "https://api.example.internal/v1",
            model: "qwen2.5-72b",
            purposes: ["AGENT", "CHAT", "NL2SQL", "SUMMARIZE", "EXTRACT"],
            has_api_key: true,
            priority: 10,
            temperature: 0,
            max_tokens: 4096,
            timeout_s: 30,
            rpm_limit: 120,
            supports_tools: true,
            embedding_dim: null,
            allow_private_network: false,
            owner_user_id: null,
            enabled: true,
            created_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/users") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000000",
            tenant_id: "default",
            username: "ops_admin",
            display_name: "平台管理员",
            email: "ops@example.com",
            dept_id: "default",
            roles: ["ADMIN"],
            clearance: "SECRET",
            attributes: {},
            status: "ACTIVE",
            auth_source: "LOCAL",
            locked_until: null,
            last_login_at: "2026-01-31T08:00:00Z",
            created_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/roles") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000081",
            tenant_id: "default",
            name: "ADMIN",
            is_builtin: true,
            permissions: ["DATA_READ", "STUDIO_EDIT", "GUARD_ADMIN", "OPS_ADMIN"],
            created_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/departments") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            tenant_id: "default",
            dept_id: "default",
            name: "总部中心",
            parent_id: null,
          },
        ]),
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
              metrics: { inventory: 500 },
            },
          ],
          links: [],
        }),
      });
      return;
    }

    if (pathname === "/api/graph/topology") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ nodes: [], edges: [] }),
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
          migrations: { ok: true, current: 22, pending: 0 },
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

    if (pathname === "/api/axops/templates") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "supply_chain",
              name: "供应链履约模板",
              version: "1.0.0",
              industry: "制造业",
              description: "多级仓储库存与订单履约协同业务模型",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axops/templates/supply_chain/preview") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          manifest: {
            id: "supply_chain",
            name: "供应链履约模板",
            version: "1.0.0",
            industry: "制造业",
            description: "多级仓储库存与订单履约协同业务模型",
          },
          topology_summary: {
            node_types: [
              {
                name: "Warehouse",
                display_name: "仓库",
                property_count: 2,
                metric_count: 1,
              },
            ],
            edge_types: [
              {
                name: "FULFILLS",
                src_type_name: "Warehouse",
                dst_type_name: "Order",
                cardinality: "ONE_TO_MANY",
              },
            ],
            agg_rule_count: 1,
            derived_rule_count: 0,
          },
          sample_metrics: [],
          dashboard_previews: [],
          agents: [],
          sample_rows: [],
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
  if (process.env.PLAYWRIGHT_LIVE_API !== "1") {
    await page.goto("/login");
    await expect(
      page.getByRole("heading", { level: 1, name: "登录" })
    ).toBeVisible();
    await page.getByTestId("login-tenant-input").fill("default");
    await page.getByTestId("login-username-input").fill("ops_admin");
    await page.getByTestId("login-password-input").fill("dev-pass-step31");
    await page.getByTestId("login-submit-btn").click();
    await expect(page).toHaveURL(/\/$/);
    return;
  }
  await page.goto("/");
  await expect(page.getByTestId("app-sidebar")).toBeVisible();
}

async function collectVisibleJargonLeaks(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const jargonRe = /本体|\bontology\b/i;
    const findings: string[] = [];

    const bodyText = document.body.innerText || "";
    for (const rawLine of bodyText.split("\n")) {
      const line = rawLine.trim();
      if (line && jargonRe.test(line)) {
        findings.push(`text: ${line.slice(0, 120)}`);
      }
    }

    const attrTargets = document.querySelectorAll<HTMLElement>(
      "[placeholder], [aria-label], [title]"
    );
    for (const el of attrTargets) {
      if (el.offsetParent === null) continue;
      for (const attrName of ["placeholder", "aria-label", "title"]) {
        const val = el.getAttribute(attrName);
        if (val && jargonRe.test(val)) {
          findings.push(`${attrName}: ${val.slice(0, 120)}`);
        }
      }
    }

    return findings;
  });
}

test("Step 31 Gate: modeling-first 8-domain nav order, flat bottom /settings with 5 groups & legacy /governance query compat, and bilingual zero-jargon DOM scan", async ({
  page,
}) => {
  test.setTimeout(120_000);
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
    await installStep31Mocks(page, leakedTenantUrls, missingAuthUrls);
  }

  await page.setViewportSize({ width: 1440, height: 900 });
  await ensureSignedIn(page);

  // =========================================================================
  // 1. nav-modeling-promoted: 8-Domain Modeling-First Order (Studio #3)
  // =========================================================================
  const primaryNav = page.getByTestId("primary-domain-nav");
  await expect(primaryNav).toBeVisible();
  const primaryLinks = primaryNav.locator("a");
  await expect(primaryLinks).toHaveCount(8);

  const zhNavLabels = (await primaryLinks.allInnerTexts()).map((s) => s.trim());
  expect(zhNavLabels).toEqual([
    "总控看板",
    "数据与知识",
    "建模工作室",
    "查询与制图",
    "推演沙盘",
    "编排与告警",
    "数字看板",
    "智能体与接口",
  ]);
  expect(zhNavLabels[2]).toBe("建模工作室");
  await expect(primaryLinks.nth(2)).toHaveAttribute("href", "/studio");

  // =========================================================================
  // 2. settings-flat-entry: Flat Single-Row Bottom Settings & /governance Compat
  // =========================================================================
  await expect(page.getByTestId("governance-section-toggle")).toHaveCount(0);
  await expect(page.getByTestId("sidebar-nav-governance")).toHaveCount(0);

  const settingsLink = page.getByTestId("sidebar-nav-settings");
  await expect(settingsLink).toBeVisible();
  await expect(settingsLink).toHaveText("系统设置");
  await expect(settingsLink).toHaveAttribute("href", "/settings");

  // Seamless /governance redirect with query parameters
  await page.goto("/governance?tab=access");
  await expect(page).toHaveURL(/\/settings\?tab=access/);
  await expect(
    page.getByRole("heading", { level: 1, name: "系统设置" })
  ).toBeVisible();
  await expect(page.getByTestId("governance-tab-members")).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect(page.getByTestId("axguard-tab-users")).toBeVisible();
  await expect(page.getByTestId("axguard-users-panel")).toBeVisible();

  await page.goto("/governance?tab=jobs");
  await expect(page).toHaveURL(/\/settings\?tab=jobs/);
  await expect(page.getByTestId("governance-tab-ops")).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect(page.getByTestId("axops-tab-jobs")).toBeVisible();

  await page.goto("/governance?section=security");
  await expect(page).toHaveURL(/\/settings\?section=security/);
  await expect(page.getByTestId("governance-tab-security")).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect(page.getByTestId("axguard-tab-policies")).toBeVisible();

  await page.goto("/governance?section=ops");
  await expect(page).toHaveURL(/\/settings\?section=ops/);
  await expect(page.getByTestId("governance-tab-ops")).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect(page.getByTestId("axops-page")).toBeVisible();

  // Verify all 5 flat Settings group tabs switch cleanly
  await page.goto("/settings");
  await expect(page.getByTestId("governance-tab-members")).toHaveText(
    "成员与权限"
  );
  await expect(page.getByTestId("governance-tab-security")).toHaveText(
    "安全与脱敏"
  );
  await expect(page.getByTestId("governance-tab-models")).toHaveText(
    "模型通道"
  );
  await expect(page.getByTestId("governance-tab-ops")).toHaveText("任务与备份");
  await expect(page.getByTestId("governance-tab-audit")).toHaveText("审计日志");

  await page.getByTestId("governance-tab-security").click();
  await expect(page.getByTestId("axguard-policies-panel")).toBeVisible();
  await page.getByTestId("axguard-tab-roles").isVisible().then((v) => {
    expect(v).toBe(false);
  });

  await page.getByTestId("governance-tab-models").click();
  await expect(page.getByTestId("settings-models-panel")).toBeVisible();

  await page.getByTestId("governance-tab-ops").click();
  await expect(page.getByTestId("axops-page")).toBeVisible();

  await page.getByTestId("governance-tab-audit").click();
  await expect(page.getByTestId("axguard-audit-panel")).toBeVisible();

  await page.getByTestId("governance-tab-members").click();
  await expect(page.getByTestId("axguard-users-panel")).toBeVisible();

  // =========================================================================
  // 3. Command Palette Hidden Keyword Alias & Plain Display Copy
  // =========================================================================
  await page.keyboard.press("Control+k");
  const cmdkDialog = page.getByTestId("global-cmdk-dialog");
  await expect(cmdkDialog).toBeVisible();
  expect(await cmdkDialog.innerText()).not.toMatch(/本体|\bontology\b/i);

  const cmdkInput = page.getByTestId("global-cmdk-input");
  await cmdkInput.fill("ontology");
  const explorerItem = page.getByTestId("cmdk-item-nav-studio-explorer");
  await expect(explorerItem).toBeVisible();
  await expect(explorerItem).toContainText("建模工作室 · 图谱浏览");

  await cmdkInput.fill("系统治理");
  const govItem = page.getByTestId("cmdk-item-nav-governance");
  await expect(govItem).toBeVisible();
  await expect(govItem).toContainText("系统设置");
  await cmdkInput.press("Enter");
  await expect(cmdkDialog).toHaveCount(0);
  await expect(page).toHaveURL(/\/settings$/);

  // =========================================================================
  // 4. ui-zero-ontology-jargon: Full-Route Visible DOM Scan (`zh-CN` & `en-US`)
  // =========================================================================
  const zhRoutes: ReadonlyArray<{ path: string; h1: string }> = [
    { path: "/", h1: "总控看板" },
    { path: "/hub", h1: "数据与知识" },
    { path: "/studio", h1: "建模工作室" },
    { path: "/studio?tab=explorer", h1: "建模工作室" },
    { path: "/analytics", h1: "查询与制图" },
    { path: "/analytics?tab=ask", h1: "查询与制图" },
    { path: "/sim", h1: "推演沙盘" },
    { path: "/flow", h1: "编排与告警" },
    { path: "/flow?tab=alerts", h1: "编排与告警" },
    { path: "/view", h1: "数字看板" },
    { path: "/agents", h1: "智能体与接口" },
    { path: "/agents?tab=mcp", h1: "智能体与接口" },
    { path: "/settings?section=members&tab=roles", h1: "系统设置" },
    { path: "/settings?section=security", h1: "系统设置" },
    { path: "/settings?section=models", h1: "系统设置" },
    { path: "/settings?section=ops", h1: "系统设置" },
    { path: "/settings?section=audit", h1: "系统设置" },
  ];

  for (const route of zhRoutes) {
    await page.goto(route.path);
    await expect(
      page.getByRole("heading", { level: 1, name: route.h1 }).first()
    ).toBeVisible();
    const leaks = await collectVisibleJargonLeaks(page);
    expect(leaks, `zh-CN jargon leak on ${route.path}`).toEqual([]);
  }

  // Verify AI Draft Wizard default title in Modeling Studio
  await page.goto("/studio");
  const aiWizardBtn = page.getByTestId("open-ai-wizard-btn");
  if ((await aiWizardBtn.count()) > 0 && (await aiWizardBtn.isEnabled())) {
    await aiWizardBtn.click();
    await expect(page.getByTestId("ai-draft-wizard")).toBeVisible();
    await expect(page.getByTestId("ai-draft-title-input")).toHaveValue(
      "AI 供应链模型草案"
    );
    expect(
      await collectVisibleJargonLeaks(page),
      "zh-CN jargon leak in AiDraftWizard"
    ).toEqual([]);
    await page.keyboard.press("Escape");
  }

  // Switch to English (`en-US`) and scan all primary routes + 5 Settings groups
  await page.goto("/");
  await page.getByTestId("locale-toggle-btn").click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Dashboard" })
  ).toBeVisible();

  const enNavLabels = (await primaryLinks.allInnerTexts()).map((s) => s.trim());
  expect(enNavLabels).toEqual([
    "Dashboard",
    "Data Hub",
    "Modeling Studio",
    "Analytics & Charts",
    "Simulation",
    "Automation & Alerts",
    "Cockpits",
    "Agents & APIs",
  ]);
  await expect(settingsLink).toHaveText("Settings");

  const enRoutes: ReadonlyArray<{ path: string; h1: string }> = [
    { path: "/", h1: "Dashboard" },
    { path: "/hub", h1: "Data Hub" },
    { path: "/studio", h1: "Modeling Studio" },
    { path: "/studio?tab=explorer", h1: "Modeling Studio" },
    { path: "/analytics", h1: "Analytics & Charts" },
    { path: "/analytics?tab=ask", h1: "Analytics & Charts" },
    { path: "/sim", h1: "Simulation" },
    { path: "/flow", h1: "Automation & Alerts" },
    { path: "/view", h1: "Cockpits" },
    { path: "/agents", h1: "Agents & APIs" },
    { path: "/agents?tab=mcp", h1: "Agents & APIs" },
    { path: "/settings?section=members&tab=roles", h1: "Settings" },
    { path: "/settings?section=security", h1: "Settings" },
    { path: "/settings?section=models", h1: "Settings" },
    { path: "/settings?section=ops", h1: "Settings" },
    { path: "/settings?section=audit", h1: "Settings" },
  ];

  for (const route of enRoutes) {
    await page.goto(route.path);
    await expect(
      page.getByRole("heading", { level: 1, name: route.h1 }).first()
    ).toBeVisible();
    const leaks = await collectVisibleJargonLeaks(page);
    expect(leaks, `en-US jargon leak on ${route.path}`).toEqual([]);
  }

  await expect(page.getByTestId("governance-tab-members")).toHaveText(
    "Members & Roles"
  );
  await expect(page.getByTestId("governance-tab-security")).toHaveText(
    "Security & Masking"
  );
  await expect(page.getByTestId("governance-tab-models")).toHaveText(
    "Model Channels"
  );
  await expect(page.getByTestId("governance-tab-ops")).toHaveText(
    "Jobs & Backups"
  );
  await expect(page.getByTestId("governance-tab-audit")).toHaveText(
    "Audit Logs"
  );

  // Restore zh-CN
  await page.getByTestId("locale-toggle-btn").click();
  await expect(
    page.getByRole("heading", { level: 1, name: "系统设置" })
  ).toBeVisible();

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
