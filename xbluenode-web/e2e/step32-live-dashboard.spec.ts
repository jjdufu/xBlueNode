import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const IS_LIVE = process.env.AXPOINT_E2E_LIVE === "1";

interface PulseResponsePayload {
  generated_at: string;
  totals: {
    subgraph_count: number;
    node_type_count: number;
    node_count: number;
    link_count: number;
    dataset_count: number;
    lake_total_rows: number;
    metric_points_24h: number;
    active_stream_count: number;
    open_alert_count: number;
    pending_approval_count: number;
  };
  hourly_series: Array<{
    hour_utc: string;
    metric_points: number;
    synced_rows: number;
  }>;
  subgraphs: Array<{
    slug: string;
    display_name: string;
    description: string | null;
    is_dev_sandbox: boolean;
    schema_version: number;
    node_type_count: number;
    edge_type_count: number;
    node_count: number;
    link_count: number;
    alert_count?: number;
    cert_level: string | null;
    readiness_score: number | null;
    business_domains?: string[];
    primary_dashboard_slug: string | null;
    latest_sandbox_id: string | null;
  }>;
  active_alerts: Array<{
    id: string;
    severity: string;
    status: string;
    title: string;
    node_id: string | null;
    subgraph_slug: string | null;
    last_value: number | null;
    occurrences: number;
    last_seen_at: string;
  }>;
  pending_approvals: Array<{
    id: string;
    source_kind: string;
    source_ref: string;
    title: string;
    run_id: string | null;
    created_at: string;
    expires_at: string | null;
  }>;
  recent_simulations: Array<{
    scenario_id: string;
    sandbox_id: string;
    sandbox_name: string;
    subgraph_slug: string;
    name: string;
    status: string;
    affected_count: number;
    earliest_breach_step: number | null;
    earliest_deadline_step: number | null;
    deadline_status: string | null;
    created_at: string;
  }>;
}

function buildHourlyBuckets(nonZero: boolean): PulseResponsePayload["hourly_series"] {
  const baseMs = Date.UTC(2026, 2, 1, 0, 0, 0);
  return Array.from({ length: 24 }, (_, idx) => ({
    hour_utc: new Date(baseMs + idx * 3_600_000).toISOString(),
    metric_points: nonZero ? 120 + idx * 15 : 0,
    synced_rows: nonZero ? (idx % 4 === 0 ? 640 : 80) : 0,
  }));
}

function buildMockPopulatedPulse(): PulseResponsePayload {
  return {
    generated_at: "2026-03-01T23:45:00Z",
    totals: {
      subgraph_count: 2,
      node_type_count: 5,
      node_count: 324,
      link_count: 548,
      dataset_count: 4,
      lake_total_rows: 148200,
      metric_points_24h: 7200,
      active_stream_count: 2,
      open_alert_count: 2,
      pending_approval_count: 1,
    },
    hourly_series: buildHourlyBuckets(true),
    subgraphs: [
      {
        slug: "power_grid",
        display_name: "新型电力与配网调度",
        description: "220kV 变电站与馈线负荷协同模型",
        is_dev_sandbox: false,
        schema_version: 3,
        node_type_count: 3,
        edge_type_count: 2,
        node_count: 210,
        link_count: 360,
        alert_count: 1,
        cert_level: "L4_CERTIFIED",
        readiness_score: 96,
        business_domains: ["输配电域", "调度域"],
        primary_dashboard_slug: "power-grid-cockpit",
        latest_sandbox_id: "00000000-0000-0000-0000-000000003241",
      },
      {
        slug: "supply_chain",
        display_name: "多级仓储与履约协同",
        description: "华东仓网库存与订单履约模型",
        is_dev_sandbox: false,
        schema_version: 2,
        node_type_count: 2,
        edge_type_count: 1,
        node_count: 114,
        link_count: 188,
        alert_count: 1,
        cert_level: "L3_VERIFIED",
        readiness_score: 89,
        business_domains: ["履约域"],
        primary_dashboard_slug: "supply-overview",
        latest_sandbox_id: null,
      },
    ],
    active_alerts: [
      {
        id: "00000000-0000-0000-0000-000000003251",
        severity: "CRITICAL",
        status: "OPEN",
        title: "220kV 城北变 主变负载率越限",
        node_id: "00000000-0000-0000-0000-000000003211",
        subgraph_slug: "power_grid",
        last_value: 94.2,
        occurrences: 3,
        last_seen_at: "2026-03-01T23:40:00Z",
      },
    ],
    pending_approvals: [
      {
        id: "00000000-0000-0000-0000-000000003261",
        source_kind: "OBJECT_ACTION",
        source_ref: "Substation:transfer_load",
        title: "城北变负荷跨站转供",
        run_id: null,
        created_at: "2026-03-01T23:41:00Z",
        expires_at: null,
      },
    ],
    recent_simulations: [
      {
        scenario_id: "00000000-0000-0000-0000-000000003271",
        sandbox_id: "00000000-0000-0000-0000-000000003241",
        sandbox_name: "城北变晚高峰推演沙盘",
        subgraph_slug: "power_grid",
        name: "主变跳闸晚高峰推演",
        status: "SUCCEEDED",
        affected_count: 18,
        earliest_breach_step: 6,
        earliest_deadline_step: 3,
        deadline_status: "ACTIONABLE_WINDOW",
        created_at: "2026-03-01T23:35:00Z",
      },
    ],
  };
}

function buildMockEmptyPulse(): PulseResponsePayload {
  return {
    generated_at: "2026-03-01T23:00:00Z",
    totals: {
      subgraph_count: 1,
      node_type_count: 0,
      node_count: 0,
      link_count: 0,
      dataset_count: 0,
      lake_total_rows: 0,
      metric_points_24h: 0,
      active_stream_count: 0,
      open_alert_count: 0,
      pending_approval_count: 0,
    },
    hourly_series: buildHourlyBuckets(false),
    subgraphs: [],
    active_alerts: [],
    pending_approvals: [],
    recent_simulations: [],
  };
}

function formatZhInt(val: number): string {
  return new Intl.NumberFormat("zh-CN", {
    minimumFractionDigits: 0,
    maximumFractionDigits: 0,
  }).format(val);
}

async function installDashboardRouteMocks(
  page: Page,
  options: {
    getPulse: () => PulseResponsePayload;
    onLoadShowcase?: () => void;
    leakedTenantUrls: string[];
  }
): Promise<void> {
  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const url = new URL(req.url());
    const { pathname, searchParams } = url;

    if (searchParams.has("tenant_id")) {
      options.leakedTenantUrls.push(req.url());
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "INVALID_ARGUMENT",
          message: "tenant_id query parameter is forbidden",
          trace_id: "step32-400",
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
            "VIEW_EDIT",
            "GUARD_ADMIN",
            "OPS_ADMIN",
          ],
          token_kind: "DEV_STATIC",
        }),
      });
      return;
    }

    if (pathname === "/api/axops/pulse" && req.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(options.getPulse()),
      });
      return;
    }

    if (pathname === "/api/axops/onboarding-status" && req.method() === "GET") {
      const p = options.getPulse();
      const populated = p.totals.node_count > 0 && p.totals.dataset_count > 0;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          completed_steps: populated ? 4 : 0,
          total_steps: 5,
          progress_pct: populated ? 80 : 0,
          recommended_next_step: populated ? "COCKPIT_AND_AGENT" : "DATA_INGEST",
          has_trial_subgraph: false,
          steps: [
            {
              step_no: 1,
              step_key: "DATA_INGEST",
              label_zh: "1. 数据接入",
              label_en: "1. Data Ingest",
              completed: populated,
              count_primary: p.totals.dataset_count,
              count_secondary: 0,
              metric_label: `${p.totals.dataset_count} 数据集`,
              target_path: "/hub",
            },
            {
              step_no: 2,
              step_key: "NODE_MODELING",
              label_zh: "2. 节点建模",
              label_en: "2. Node Modeling",
              completed: populated,
              count_primary: p.totals.node_type_count,
              count_secondary: 2,
              metric_label: `${p.totals.node_type_count} 节点类型`,
              target_path: "/studio",
            },
            {
              step_no: 3,
              step_key: "MAPPING_AND_FLOW",
              label_zh: "3. 映射与编排",
              label_en: "3. Mapping & Flow",
              completed: populated,
              count_primary: p.totals.node_count,
              count_secondary: 1,
              metric_label: `${p.totals.node_count} 业务节点`,
              target_path: "/flow",
            },
            {
              step_no: 4,
              step_key: "SIM_VERIFICATION",
              label_zh: "4. 推演验证",
              label_en: "4. Simulation",
              completed: populated,
              count_primary: p.recent_simulations.length,
              count_secondary: 1,
              metric_label: `${p.recent_simulations.length} 推演方案`,
              target_path: "/sim",
            },
            {
              step_no: 5,
              step_key: "COCKPIT_AND_AGENT",
              label_zh: "5. 驾驶舱与智能体",
              label_en: "5. Apps & Agents",
              completed: false,
              count_primary: 0,
              count_secondary: 0,
              metric_label: "0 驾驶舱",
              target_path: "/view",
            },
          ],
        }),
      });
      return;
    }

    if (
      pathname === "/api/axops/demo/showcase/load" &&
      req.method() === "POST"
    ) {
      options.onLoadShowcase?.();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ loaded_subgraphs: ["power_grid", "supply_chain"] }),
      });
      return;
    }

    if (pathname === "/api/xblue/subgraphs") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000003201",
            slug: "power_grid",
            display_name: "新型电力与配网调度",
            description: "",
            owner_dept: "default",
            is_main: true,
            current_version: 3,
            created_at: "2026-03-01T00:00:00Z",
            updated_at: "2026-03-01T00:00:00Z",
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
          subgraph_slug: searchParams.get("subgraph") ?? "power_grid",
          version: 3,
          node_types: [],
          edge_types: [],
          mappings: [],
        }),
      });
      return;
    }

    if (
      pathname === "/api/xblue/drafts" ||
      pathname === "/api/xblue/mappings" ||
      pathname === "/api/xblue/schema/versions" ||
      pathname === "/api/axsim/sandboxes" ||
      pathname === "/api/axflow/flows"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname.startsWith("/api/xblueview/dashboards/")) {
      const slug = pathname.split("/").pop() ?? "power-grid-cockpit";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: "00000000-0000-0000-0000-000000003281",
          tenant_id: "default",
          name: "配网调度驾驶舱",
          slug,
          description: "",
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
          created_at: "2026-03-01T00:00:00Z",
          updated_at: "2026-03-01T00:00:00Z",
          widgets: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards") {
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
      body: JSON.stringify({ items: [], next_cursor: null }),
    });
  });
}

test("dashboard renders live pulse KPIs, ECharts canvases, smart onboarding toggle, and subgraph deep links with zero legacy cards", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  if (!IS_LIVE) {
    const mockPulse = buildMockPopulatedPulse();
    await installDashboardRouteMocks(page, {
      getPulse: () => mockPulse,
      leakedTenantUrls,
    });
  } else {
    page.on("request", (req) => {
      const url = new URL(req.url());
      if (url.pathname.startsWith("/api/") && url.searchParams.has("tenant_id")) {
        leakedTenantUrls.push(req.url());
      }
    });
  }

  const pulseRespPromise = page.waitForResponse(
    (resp) =>
      new URL(resp.url()).pathname === "/api/axops/pulse" &&
      resp.request().method() === "GET" &&
      resp.status() === 200
  );

  await page.goto("/");
  const pulseResp = await pulseRespPromise;
  const pulseData = (await pulseResp.json()) as PulseResponsePayload;

  await expect(
    page.getByRole("heading", { level: 1, name: "总控看板" })
  ).toBeVisible();

  // 1. Legacy static 5-way recall card must be completely absent
  await expect(page.getByText("五路融合召回架构")).toHaveCount(0);
  await expect(page.getByText("5-Way Recall Channels")).toHaveCount(0);
  await expect(page.getByText("倒数秩融合重排")).toHaveCount(0);

  // 2. All 6 KPI cards match /api/axops/pulse totals exactly and render inline SVG sparklines
  await expect(page.getByTestId("pulse-kpi-value-nodes")).toHaveText(
    formatZhInt(pulseData.totals.node_count)
  );
  await expect(page.getByTestId("pulse-kpi-value-links")).toHaveText(
    formatZhInt(pulseData.totals.link_count)
  );
  await expect(page.getByTestId("pulse-kpi-value-lake-rows")).toHaveText(
    formatZhInt(pulseData.totals.lake_total_rows)
  );
  await expect(page.getByTestId("pulse-kpi-value-throughput")).toHaveText(
    formatZhInt(pulseData.totals.metric_points_24h)
  );
  await expect(page.getByTestId("pulse-kpi-value-subgraphs")).toHaveText(
    formatZhInt(pulseData.totals.subgraph_count)
  );
  await expect(page.getByTestId("pulse-kpi-value-alerts")).toHaveText(
    `${formatZhInt(pulseData.totals.open_alert_count)} / ${formatZhInt(
      pulseData.totals.pending_approval_count
    )}`
  );

  for (const kpiId of [
    "pulse-kpi-nodes",
    "pulse-kpi-links",
    "pulse-kpi-lake-rows",
    "pulse-kpi-throughput",
    "pulse-kpi-subgraphs",
    "pulse-kpi-alerts",
  ]) {
    await expect(
      page.getByTestId(kpiId).getByTestId("pulse-sparkline-svg")
    ).toBeVisible();
  }

  // 3. Both ECharts containers mount real <canvas> elements with width > 0 and height > 0
  for (const chartTestId of [
    "pulse-throughput-chart",
    "pulse-subgraph-bar-chart",
  ]) {
    const canvas = page.getByTestId(chartTestId).locator("canvas").first();
    await expect(canvas).toBeVisible();
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();
    expect(box?.width ?? 0).toBeGreaterThan(0);
    expect(box?.height ?? 0).toBeGreaterThan(0);
  }

  // 4. Smart onboarding auto-collapse behavior when node_count > 0 && dataset_count > 0
  if (
    pulseData.totals.node_count > 0 &&
    pulseData.totals.dataset_count > 0
  ) {
    const compactToggle = page.getByTestId("dashboard-onboarding-compact-toggle");
    await expect(compactToggle).toBeVisible();
    await expect(page.getByTestId("onboarding-strip")).toHaveCount(0);

    await compactToggle.click();
    await expect(page.getByTestId("onboarding-strip")).toBeVisible();

    await compactToggle.click();
    await expect(page.getByTestId("onboarding-strip")).toHaveCount(0);
  }

  // 5. Subgraph matrix card deep links: 建模 -> /studio, 推演 -> /sim, 看板 -> /view
  expect(pulseData.subgraphs.length).toBeGreaterThan(0);
  const firstSg = pulseData.subgraphs[0];
  if (firstSg) {
    const card = page.getByTestId(`subgraph-card-${firstSg.slug}`);
    await expect(card).toBeVisible();

    await page.getByTestId(`subgraph-model-btn-${firstSg.slug}`).click();
    await expect(page).toHaveURL(
      new RegExp(`/studio\\?subgraph=${encodeURIComponent(firstSg.slug)}`)
    );

    await page.goto("/");
    await expect(page.getByTestId(`subgraph-card-${firstSg.slug}`)).toBeVisible();
    await page.getByTestId(`subgraph-sim-btn-${firstSg.slug}`).click();
    if (firstSg.latest_sandbox_id) {
      await expect(page).toHaveURL(
        new RegExp(
          `/sim\\?sandbox=${encodeURIComponent(firstSg.latest_sandbox_id)}`
        )
      );
    } else {
      await expect(page).toHaveURL(
        new RegExp(`/sim\\?subgraph=${encodeURIComponent(firstSg.slug)}`)
      );
    }

    await page.goto("/");
    await expect(page.getByTestId(`subgraph-card-${firstSg.slug}`)).toBeVisible();
    await page.getByTestId(`subgraph-view-btn-${firstSg.slug}`).click();
    if (firstSg.primary_dashboard_slug) {
      await expect(page).toHaveURL(
        new RegExp(
          `/view/${encodeURIComponent(firstSg.primary_dashboard_slug)}`
        )
      );
    } else {
      await expect(page).toHaveURL(/\/view$/);
    }
  }

  expect(leakedTenantUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("empty tenant keeps onboarding strip expanded by default and OPS_ADMIN showcase loader collapses it upon population", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  let loaded = false;
  const emptyPulse = buildMockEmptyPulse();
  const populatedPulse = buildMockPopulatedPulse();

  await installDashboardRouteMocks(page, {
    getPulse: () => (loaded ? populatedPulse : emptyPulse),
    onLoadShowcase: () => {
      loaded = true;
    },
    leakedTenantUrls,
  });

  await page.goto("/");
  await expect(page.getByTestId("onboarding-strip")).toBeVisible();
  await expect(
    page.getByTestId("dashboard-onboarding-compact-toggle")
  ).toHaveCount(0);

  const showcaseBtn = page.getByTestId("dashboard-load-showcase-btn");
  await expect(showcaseBtn).toBeVisible();
  await showcaseBtn.click();

  await expect(page.getByTestId("pulse-kpi-value-nodes")).toHaveText("324");
  await expect(
    page.getByTestId("dashboard-onboarding-compact-toggle")
  ).toBeVisible();
  await expect(page.getByTestId("onboarding-strip")).toHaveCount(0);

  expect(leakedTenantUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
