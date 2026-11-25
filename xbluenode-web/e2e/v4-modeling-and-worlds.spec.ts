import {
  expect,
  type APIRequestContext,
  type Page,
  type Route,
  test,
} from "@playwright/test";
import { chooseSelectOption } from "./helpers";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const OPS_ADMIN_ID = "00000000-0000-0000-0000-000000003501";
const MOCK_PG_SANDBOX_ID = "00000000-0000-0000-0000-000000003511";
const MOCK_PG_SCENARIO_ID = "00000000-0000-0000-0000-000000003512";
const MOCK_PG_PP01_ID = "00000000-0000-0000-0000-000000003513";
const MOCK_PG_SUB01_ID = "00000000-0000-0000-0000-000000003514";
const MOCK_PG_CL01_ID = "00000000-0000-0000-0000-000000003515";
const MOCK_PG_DASH_ID = "00000000-0000-0000-0000-000000003520";
const MOCK_PG_MAP_WIDGET_ID = "00000000-0000-0000-0000-000000003521";
const MOCK_PG_KPI_WIDGET_ID = "00000000-0000-0000-0000-000000003522";
const MOCK_MED_DASH_ID = "00000000-0000-0000-0000-000000003530";
const MOCK_MED_KPI_WIDGET_ID = "00000000-0000-0000-0000-000000003531";
const MOCK_MED_MAP_WIDGET_ID = "00000000-0000-0000-0000-000000003532";
const MOCK_MED_ACT_WIDGET_ID = "00000000-0000-0000-0000-000000003533";
const MOCK_HOSP01_ID = "00000000-0000-0000-0000-000000003540";

interface PulsePayload {
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
    alert_count: number;
    cert_level: string | null;
    readiness_score: number | null;
    business_domains: string[];
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

interface MockV4State {
  surgeInvocations: number;
  hospFreeBeds: number;
}

function buildMockJwt(username: string): string {
  const encodeSeg = (obj: Record<string, unknown>): string =>
    btoa(JSON.stringify(obj))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  const nowSec = Math.floor(Date.now() / 1000);
  const roles = username === "fin_analyst_sh" ? ["ANALYST"] : ["ADMIN"];
  const dept = username === "fin_analyst_sh" ? "sh_branch" : "east";
  return `${encodeSeg({ alg: "EdDSA", typ: "JWT", kid: "k1" })}.${encodeSeg({
    iss: "axpoint",
    aud: "axpoint-api",
    sub: OPS_ADMIN_ID,
    tid: "default",
    dept,
    roles,
    typ: "access",
    iat: nowSec,
    exp: nowSec + 900,
    jti: `v4-${username}`,
  })}.sig_${username}`;
}

function formatZhInt(val: number): string {
  return new Intl.NumberFormat("zh-CN", {
    minimumFractionDigits: 0,
    maximumFractionDigits: 0,
  }).format(val);
}

function computeRectOverlapArea(
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number }
): number {
  const w = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
  const h = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
  if (w <= 0 || h <= 0) return 0;
  return w * h;
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

function buildSemiSupplySchemaMock() {
  return {
    subgraph_slug: "semi_supply",
    version: 2,
    node_types: [
      {
        name: "SpecialtyVendor",
        display_name: "特种气体与硅片商",
        description: "上游电子级特种材料供应商",
        icon: "flask-conical",
        color: "#0891b2",
        title_template: "{node_key}",
        domain_group: "采购域",
        semantic_layer_tier: "STATIC_FACT",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "supply_index",
            display_name: "供应保障指数",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "分",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 80,
        pos_y: 220,
      },
      {
        name: "WaferFab",
        display_name: "晶圆制造厂",
        description: "车规晶圆前道制造基地",
        icon: "factory",
        color: "#2563eb",
        title_template: "{node_key}",
        domain_group: "制造域",
        semantic_layer_tier: "STATIC_FACT",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "wafer_starts_wpm",
            display_name: "月投片产能",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "片/月",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "fab_yield_pct",
            display_name: "综合良率",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "%",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 540,
        pos_y: 120,
      },
      {
        name: "LithoTool",
        display_name: "关键光刻机台",
        description: "核心图形化曝光机台",
        icon: "cog",
        color: "#4f46e5",
        title_template: "{node_key}",
        domain_group: "制造域",
        semantic_layer_tier: "STATIC_FACT",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "oee_pct",
            display_name: "设备综合效率",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "%",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 540,
        pos_y: 420,
      },
      {
        name: "WaferLotEvent",
        display_name: "晶圆批次流片记录",
        description: "单批次流片工序计量事件",
        icon: "layers",
        color: "#0284c7",
        title_template: "{node_key}",
        domain_group: "制造域",
        semantic_layer_tier: "EVENT_DYNAMIC",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "die_good_cnt",
            display_name: "良品裸片数",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "颗",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 540,
        pos_y: 720,
      },
      {
        name: "YieldRollupCube",
        display_name: "制程良率汇总块",
        description: "月度平均良率汇总块",
        icon: "bar-chart-3",
        color: "#7c3aed",
        title_template: "{node_key}",
        domain_group: "制造域",
        semantic_layer_tier: "CUBE_ROLLUP",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "monthly_avg_yield",
            display_name: "月均良率",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "%",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 540,
        pos_y: 1020,
      },
      {
        name: "CustomsPort",
        display_name: "清关口岸与航站",
        description: "跨国晶圆恒温通关航空港",
        icon: "plane",
        color: "#0d9488",
        title_template: "{node_key}",
        domain_group: "物流域",
        semantic_layer_tier: "STATIC_FACT",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "clearance_hours",
            display_name: "清关耗时",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "小时",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 1020,
        pos_y: 220,
      },
      {
        name: "RegionalHub",
        display_name: "区域恒温分拨仓",
        description: "车规芯片恒温分拨枢纽",
        icon: "warehouse",
        color: "#16a34a",
        title_template: "{node_key}",
        domain_group: "履约域",
        semantic_layer_tier: "STATIC_FACT",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "chip_inventory_k",
            display_name: "芯片库存",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "千颗",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 1500,
        pos_y: 180,
      },
      {
        name: "AutoOemOrder",
        display_name: "主机厂车规芯片单",
        description: "车企芯片保供交付订单",
        icon: "truck",
        color: "#ea580c",
        title_template: "{node_key}",
        domain_group: "履约域",
        semantic_layer_tier: "EVENT_DYNAMIC",
        maturity: "CONFIRMED",
        properties: [
          {
            name: "ordered_qty_k",
            display_name: "订单需求量",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "千颗",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        pos_x: 1500,
        pos_y: 520,
      },
    ],
    edge_types: [
      {
        name: "SUPPLIES_MATERIAL",
        src_type_name: "SpecialtyVendor",
        dst_type_name: "WaferFab",
        cardinality: "MANY_TO_MANY",
        impact_weight: 0.8,
      },
      {
        name: "EQUIPPED_WITH",
        src_type_name: "WaferFab",
        dst_type_name: "LithoTool",
        cardinality: "ONE_TO_MANY",
        impact_weight: 0.9,
      },
      {
        name: "RUNS_LOT",
        src_type_name: "WaferFab",
        dst_type_name: "WaferLotEvent",
        cardinality: "ONE_TO_MANY",
        impact_weight: 0.85,
      },
      {
        name: "SUMMARIZES_YIELD",
        src_type_name: "WaferFab",
        dst_type_name: "YieldRollupCube",
        cardinality: "ONE_TO_MANY",
        impact_weight: 0.75,
      },
      {
        name: "SHIPS_VIA_PORT",
        src_type_name: "WaferFab",
        dst_type_name: "CustomsPort",
        cardinality: "MANY_TO_MANY",
        impact_weight: 0.85,
      },
      {
        name: "DISPATCHES_TO_HUB",
        src_type_name: "CustomsPort",
        dst_type_name: "RegionalHub",
        cardinality: "MANY_TO_MANY",
        impact_weight: 0.85,
      },
      {
        name: "DELIVERS_ORDER",
        src_type_name: "RegionalHub",
        dst_type_name: "AutoOemOrder",
        cardinality: "ONE_TO_MANY",
        impact_weight: 0.9,
      },
    ],
    mappings: [],
  };
}

function buildMockPulse(state: MockV4State): PulsePayload {
  const baseMs = Date.UTC(2026, 3, 1, 0, 0, 0);
  const hourly = Array.from({ length: 24 }, (_, idx) => ({
    hour_utc: new Date(baseMs + idx * 3_600_000).toISOString(),
    metric_points:
      4_200 + idx * 75 + (idx === 23 ? state.surgeInvocations : 0),
    synced_rows: idx % 6 === 0 ? 25_200 : 0,
  }));

  return {
    generated_at: new Date(
      baseMs + 23 * 3_600_000 + state.surgeInvocations * 1_000
    ).toISOString(),
    totals: {
      subgraph_count: 5,
      node_type_count: 25,
      node_count: 2_337,
      link_count: 6_699,
      dataset_count: 5,
      lake_total_rows: 151_200,
      metric_points_24h: 104_960 + state.surgeInvocations,
      active_stream_count: 2,
      open_alert_count: 4,
      pending_approval_count: 1,
    },
    hourly_series: hourly,
    subgraphs: [
      {
        slug: "fin_risk",
        display_name: "集团授信与互保风控",
        description: "集团母体、关联法人、实控人与授信借据穿透风控模型",
        is_dev_sandbox: false,
        schema_version: 2,
        node_type_count: 5,
        edge_type_count: 6,
        node_count: 1_210,
        link_count: 4_280,
        alert_count: 1,
        cert_level: "L4_CERTIFIED",
        readiness_score: 97,
        business_domains: ["客户域", "敞口域", "担保域", "授信域"],
        primary_dashboard_slug: "fin-risk-cockpit",
        latest_sandbox_id: null,
      },
      {
        slug: "semi_supply",
        display_name: "半导体全球供应链",
        description: "采购、制造、物流、履约四域泳道与晶圆良率三层模型",
        is_dev_sandbox: false,
        schema_version: 2,
        node_type_count: 8,
        edge_type_count: 7,
        node_count: 526,
        link_count: 1_350,
        alert_count: 1,
        cert_level: "L4_CERTIFIED",
        readiness_score: 96,
        business_domains: ["制造域", "履约域", "物流域", "采购域"],
        primary_dashboard_slug: "semi-supply-cockpit",
        latest_sandbox_id: "00000000-0000-0000-0000-000000003551",
      },
      {
        slug: "power_grid",
        display_name: "新型电力与配网调度",
        description: "电源、输变电、配电与重点负荷多边形围栏调度模型",
        is_dev_sandbox: false,
        schema_version: 2,
        node_type_count: 6,
        edge_type_count: 5,
        node_count: 324,
        link_count: 552,
        alert_count: 1,
        cert_level: "L4_CERTIFIED",
        readiness_score: 98,
        business_domains: ["电源域", "负荷域", "输变电域", "配电域"],
        primary_dashboard_slug: "power-grid-cockpit",
        latest_sandbox_id: MOCK_PG_SANDBOX_ID,
      },
      {
        slug: "med_dispatch",
        display_name: "城市急诊床位调度",
        description: "院区统筹、临床救治与120院前急救空间凸包调度模型",
        is_dev_sandbox: false,
        schema_version: 2,
        node_type_count: 6,
        edge_type_count: 5,
        node_count: 264,
        link_count: 508,
        alert_count: 1,
        cert_level: "L4_CERTIFIED",
        readiness_score: 96,
        business_domains: ["临床救治域", "院前急救域", "院区统筹域"],
        primary_dashboard_slug: "med-dispatch-cockpit",
        latest_sandbox_id: "00000000-0000-0000-0000-000000003561",
      },
      {
        slug: "main",
        display_name: "企业主图",
        description: "跨行业核心锚点主图",
        is_dev_sandbox: false,
        schema_version: 3,
        node_type_count: 14,
        edge_type_count: 12,
        node_count: 13,
        link_count: 9,
        alert_count: 0,
        cert_level: "L3_VERIFIED",
        readiness_score: 90,
        business_domains: ["制造域", "院区统筹域"],
        primary_dashboard_slug: "main-ops-cockpit",
        latest_sandbox_id: null,
      },
    ],
    active_alerts: [
      {
        id: "00000000-0000-0000-0000-000000003571",
        severity: "CRITICAL",
        status: "OPEN",
        title: "院区急诊饱和红线告警：徐汇重症医学临床总院",
        node_id: MOCK_HOSP01_ID,
        subgraph_slug: "med_dispatch",
        last_value: 84.6,
        occurrences: 2,
        last_seen_at: "2026-04-01T23:40:00Z",
      },
    ],
    pending_approvals: [],
    recent_simulations: [
      {
        scenario_id: MOCK_PG_SCENARIO_ID,
        sandbox_id: MOCK_PG_SANDBOX_ID,
        sandbox_name: "迎峰度夏配网调度推演沙盘",
        subgraph_slug: "power_grid",
        name: "迎峰度夏城北 220kV 主变跳闸推演",
        status: "SUCCEEDED",
        affected_count: 24,
        earliest_breach_step: 4,
        earliest_deadline_step: 2,
        deadline_status: "ACTIONABLE_WINDOW",
        created_at: "2026-04-01T23:30:00Z",
      },
    ],
  };
}

async function installV4Mocks(
  page: Page,
  state: MockV4State,
  leakedTenantUrls: string[]
): Promise<void> {
  const northPoly = {
    type: "Polygon",
    coordinates: [
      [
        [120.12, 30.28],
        [120.22, 30.28],
        [120.22, 30.36],
        [120.12, 30.36],
        [120.12, 30.28],
      ],
    ],
  };

  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const method = req.method();
    const url = new URL(req.url());
    const { pathname, searchParams } = url;

    if (searchParams.has("tenant_id")) {
      leakedTenantUrls.push(req.url());
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "INVALID_ARGUMENT",
          message: "tenant_id query parameter is forbidden",
          trace_id: "v4-trace-400",
        }),
      });
      return;
    }

    if (pathname.endsWith("/stream")) {
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
          oidc_enabled: false,
          default_tenant: "default",
        }),
      });
      return;
    }

    if (pathname === "/api/auth/login" && method === "POST") {
      const bodyRaw = req.postDataJSON() as { username?: string } | null;
      const uname = bodyRaw?.username ?? "ops_admin";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: buildMockJwt(uname),
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
          access_token: buildMockJwt("ops_admin"),
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

    if (pathname === "/api/auth/me") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          user_id: OPS_ADMIN_ID,
          tenant_id: "default",
          username: "ops_admin",
          dept_id: "east",
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
            "EXPORT_DATA",
            "AUDIT_READ",
            "GUARD_ADMIN",
            "OPS_ADMIN",
          ],
          token_kind: "ACCESS",
        }),
      });
      return;
    }

    if (pathname === "/api/axops/pulse" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(buildMockPulse(state)),
      });
      return;
    }

    if (pathname === "/api/axops/onboarding-status") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          completed_steps: 5,
          total_steps: 5,
          progress_pct: 100,
          recommended_next_step: null,
          has_trial_subgraph: false,
          steps: [],
        }),
      });
      return;
    }

    if (pathname === "/api/axops/demo/showcase/load" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          loaded_subgraphs: [
            "power_grid",
            "fin_risk",
            "semi_supply",
            "med_dispatch",
          ],
          node_count: 2_337,
          link_count: 6_699,
          dataset_count: 5,
          lake_total_rows: 151_200,
          metric_points_24h: 104_960,
          elapsed_ms: 4_120,
        }),
      });
      return;
    }

    if (pathname === "/api/axsim/sandboxes" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: MOCK_PG_SANDBOX_ID,
            name: "迎峰度夏配网调度推演沙盘",
            status: "READY",
            node_count: 324,
            link_count: 552,
            scope: {
              type_names: [
                "PowerPlant",
                "Substation",
                "DistTransformer",
                "CriticalLoad",
              ],
            },
            actions: [],
            sim_result: null,
            created_at: "2026-04-01T20:00:00Z",
            updated_at: "2026-04-01T23:30:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === `/api/axsim/sandboxes/${MOCK_PG_SANDBOX_ID}/graph`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          overlay_hash: "pg_overlay_v4",
          nodes: [
            {
              id: MOCK_PG_PP01_ID,
              type_name: "PowerPlant",
              node_key: "PP-001",
              title: "城北燃气热电厂",
              metrics: { generation_mw: 240 },
              properties: { domain_group: "电源域" },
              geo_lng: 120.14,
              geo_lat: 30.31,
              geo_shape: null,
            },
            {
              id: MOCK_PG_SUB01_ID,
              type_name: "Substation",
              node_key: "SUB-001",
              title: "城北220kV枢纽变电站",
              metrics: { load_ratio: 0.94, bus_voltage_pu: 0.96 },
              properties: { domain_group: "输变电域" },
              geo_lng: 120.17,
              geo_lat: 30.32,
              geo_shape: northPoly,
            },
            {
              id: MOCK_PG_CL01_ID,
              type_name: "CriticalLoad",
              node_key: "CL-001",
              title: "城北分区01号三甲医院",
              metrics: { bus_voltage_pu: 0.91, power_draw_mw: 0.85 },
              properties: { domain_group: "负荷域" },
              geo_lng: 120.18,
              geo_lat: 30.33,
              geo_shape: null,
            },
          ],
          links: [
            {
              id: "00000000-0000-0000-0000-000000003518",
              link_type: "SUPPLIES_SUBSTATION",
              src_node_id: MOCK_PG_PP01_ID,
              dst_node_id: MOCK_PG_SUB01_ID,
              weight: 1.0,
            },
            {
              id: "00000000-0000-0000-0000-000000003519",
              link_type: "SERVES_LOAD",
              src_node_id: MOCK_PG_SUB01_ID,
              dst_node_id: MOCK_PG_CL01_ID,
              weight: 1.0,
            },
          ],
        }),
      });
      return;
    }

    const pgScenarioObj = {
      id: MOCK_PG_SCENARIO_ID,
      sandbox_id: MOCK_PG_SANDBOX_ID,
      name: "迎峰度夏城北 220kV 主变跳闸推演",
      description: "城北主变跳闸引发供电围栏电压越线与最晚干预窗口反推",
      actions: [],
      params: { mode: "STEPPED", horizon_steps: 12, damping: 1.0 },
      overlay_hash: "pg_overlay_v4",
      is_stale: false,
      status: "SUCCEEDED",
      error_message: null,
      created_by: OPS_ADMIN_ID,
      created_at: "2026-04-01T23:30:00Z",
      finished_at: "2026-04-01T23:30:01Z",
      deadline_report: {
        all_satisfied: false,
        status: "ACTIONABLE_WINDOW",
        earliest_breach_step: 4,
        earliest_deadline_step: 2,
        items: [
          {
            constraint_id: "goal_north_geofence_voltage",
            constraint_name: "城北220kV供电围栏重点负荷母线电压红线",
            breached: true,
            breach_step: 4,
            breach_node_id: MOCK_PG_CL01_ID,
            breach_value: 0.912,
            min_lag_steps: 2,
            deadline_step: 2,
            status: "ACTIONABLE_WINDOW",
          },
        ],
      },
      fired_events: [
        {
          event_id: "ev_north_trip_step3",
          event_name: "第3步城北220kV主变跳闸减供",
          step: 3,
          trigger_value: null,
        },
        {
          event_id: "ev_ess_auto_boost",
          event_name: "主变负载越限自动投入站内储能",
          step: 4,
          trigger_value: 0.95,
        },
      ],
      result: {
        affected_count: 24,
        converged: true,
        iterations: 12,
        kpis: [],
        warnings: [],
        top_impacts: [
          {
            node_id: MOCK_PG_CL01_ID,
            type_name: "CriticalLoad",
            title: "城北分区01号三甲医院",
            metric: "bus_voltage_pu",
            before_val: 0.96,
            after_val: 0.912,
            delta: -0.048,
            delta_ratio: -0.05,
            hop_distance: 2,
            top_path: [MOCK_PG_PP01_ID, MOCK_PG_SUB01_ID, MOCK_PG_CL01_ID],
          },
        ],
      },
    };

    if (pathname === `/api/axsim/sandboxes/${MOCK_PG_SANDBOX_ID}/scenarios`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([pgScenarioObj]),
      });
      return;
    }

    if (pathname === `/api/axsim/scenarios/${MOCK_PG_SCENARIO_ID}`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(pgScenarioObj),
      });
      return;
    }

    if (pathname === `/api/axsim/scenarios/${MOCK_PG_SCENARIO_ID}/impacts`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: pgScenarioObj.result.top_impacts,
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axsim/scenarios/${MOCK_PG_SCENARIO_ID}/checkpoints`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    const pgDashDetail = {
      id: MOCK_PG_DASH_ID,
      tenant_id: "default",
      name: "新型电力与配网调度看板",
      slug: "power-grid-cockpit",
      description: "220kV供电多边形围栏与馈线负载监控",
      filters: [],
      theme: "light",
      canvas: {
        cols: 12,
        row_height: 32,
        screen_width: 1920,
        screen_height: 1080,
        zone_layout: "STANDARD_FIVE_ZONE",
        compact_mode: "VERTICAL_COMPACT",
      },
      status: "PUBLISHED",
      version: 1,
      created_by: OPS_ADMIN_ID,
      created_at: "2026-04-01T20:00:00Z",
      updated_at: "2026-04-01T20:00:00Z",
      widgets: [
        {
          id: MOCK_PG_KPI_WIDGET_ID,
          dashboard_id: MOCK_PG_DASH_ID,
          widget_kind: "KPI_CARD",
          title: "馈线平均负载率",
          position: { x: 0, y: 0, w: 4, h: 3 },
          data_source: {
            kind: "SQL",
            sql: "SELECT 0.782 AS value",
          },
          encoding: { y: "value" },
          options: { semantic_zone: "KPI", unit: "ratio", decimals: 3 },
          drill: [],
          refresh_mode: "LIVE",
          refresh_seconds: 60,
        },
        {
          id: MOCK_PG_MAP_WIDGET_ID,
          dashboard_id: MOCK_PG_DASH_ID,
          widget_kind: "MAP",
          title: "变电站供电围栏与重点负荷地图",
          position: { x: 0, y: 3, w: 12, h: 9 },
          data_source: {
            kind: "NODES_GEO",
            type_names: ["PowerPlant", "Substation", "CriticalLoad"],
            color_metric: "load_ratio",
            link_types: ["SUPPLIES_SUBSTATION", "SERVES_LOAD"],
          },
          encoding: {},
          options: { semantic_zone: "SCENE", show_geo_shapes: true },
          drill: [],
          refresh_mode: "LIVE",
          refresh_seconds: 60,
        },
      ],
    };

    const medDashDetail = {
      id: MOCK_MED_DASH_ID,
      tenant_id: "default",
      name: "城市急诊与医疗床位调度驾驶舱",
      slug: "med-dispatch-cockpit",
      description: "全市三甲院区急诊负荷、120急救车空间凸包与应急加床驾驶舱",
      filters: [],
      theme: "light",
      canvas: {
        cols: 12,
        row_height: 32,
        screen_width: 1920,
        screen_height: 1080,
        zone_layout: "STANDARD_FIVE_ZONE",
        compact_mode: "VERTICAL_COMPACT",
      },
      status: "PUBLISHED",
      version: 1,
      created_by: OPS_ADMIN_ID,
      created_at: "2026-04-01T20:00:00Z",
      updated_at: "2026-04-01T20:00:00Z",
      widgets: [
        {
          id: MOCK_MED_KPI_WIDGET_ID,
          dashboard_id: MOCK_MED_DASH_ID,
          widget_kind: "KPI_CARD",
          title: "全市可用空床总数",
          position: { x: 0, y: 0, w: 4, h: 3 },
          data_source: {
            kind: "NODE_METRIC",
            type_name: "HospitalCampus",
            metric: "total_free_beds",
            agg: "SUM",
            grain: "DAY",
            time_range: "LAST_7_DAYS",
          },
          encoding: {},
          options: { semantic_zone: "KPI", unit: "张", decimals: 0 },
          drill: [],
          refresh_mode: "LIVE",
          refresh_seconds: 60,
        },
        {
          id: MOCK_MED_MAP_WIDGET_ID,
          dashboard_id: MOCK_MED_DASH_ID,
          widget_kind: "MAP",
          title: "全市三甲院区与120急救车空间态势",
          position: { x: 0, y: 3, w: 8, h: 9 },
          data_source: {
            kind: "NODES_GEO",
            type_names: ["HospitalCampus", "AmbulanceUnit"],
            color_metric: "eta_minutes",
            link_types: ["TRANSPORTS_TO_CAMPUS"],
          },
          encoding: {},
          options: { semantic_zone: "SCENE", show_convex_hull: true },
          drill: [],
          refresh_mode: "LIVE",
          refresh_seconds: 30,
        },
        {
          id: MOCK_MED_ACT_WIDGET_ID,
          dashboard_id: MOCK_MED_DASH_ID,
          widget_kind: "ACTION_BUTTON",
          title: "启用应急加床",
          position: { x: 8, y: 3, w: 4, h: 5 },
          data_source: {
            kind: "ACTION",
            action_type: "WRITE_NODE",
            node_id: MOCK_HOSP01_ID,
            target_kind: "METRIC",
            key: "total_free_beds",
            input_schema: {
              type: "object",
              required: ["value"],
              properties: {
                value: {
                  type: "number",
                  title: "扩容后总空床数",
                  default: 62,
                },
              },
            },
          },
          encoding: {},
          options: {
            semantic_zone: "DECISION",
            button_label: "启用应急加床",
            skip_confirm: true,
          },
          drill: [],
          refresh_mode: "LIVE",
          refresh_seconds: 30,
        },
      ],
    };

    if (
      pathname === "/api/xblueview/dashboards/by-slug/power-grid-cockpit" ||
      pathname === `/api/xblueview/dashboards/${MOCK_PG_DASH_ID}`
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(pgDashDetail),
      });
      return;
    }

    if (
      pathname === "/api/xblueview/dashboards/by-slug/med-dispatch-cockpit" ||
      pathname === `/api/xblueview/dashboards/${MOCK_MED_DASH_ID}`
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(medDashDetail),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [pgDashDetail, medDashDetail],
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${MOCK_PG_MAP_WIDGET_ID}/data` &&
      method === "POST"
    ) {
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
            { name: "geo_shape", data_type: "JSON" },
          ],
          rows: [
            [
              MOCK_PG_SUB01_ID,
              "Substation",
              "SUB-001",
              "城北220kV枢纽变电站",
              120.17,
              30.32,
              0.94,
              northPoly,
            ],
            [
              MOCK_PG_CL01_ID,
              "CriticalLoad",
              "CL-001",
              "城北分区01号三甲医院",
              120.18,
              30.33,
              0.96,
              null,
            ],
          ],
          row_count: 2,
          truncated: false,
          generated_at: "2026-04-01T23:45:00Z",
          data_version: "v4-pg-map",
          drill_level: 0,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${MOCK_MED_MAP_WIDGET_ID}/data` &&
      method === "POST"
    ) {
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
          ],
          rows: [
            [
              MOCK_HOSP01_ID,
              "HospitalCampus",
              "HOSP-01",
              "市中心第一三甲总院",
              121.46,
              31.22,
              12.0,
            ],
            [
              "00000000-0000-0000-0000-000000003541",
              "AmbulanceUnit",
              "AMB-01",
              "沪120-急救01车",
              121.38,
              31.18,
              14.5,
            ],
            [
              "00000000-0000-0000-0000-000000003542",
              "AmbulanceUnit",
              "AMB-02",
              "沪120-急救02车",
              121.54,
              31.19,
              16.2,
            ],
            [
              "00000000-0000-0000-0000-000000003543",
              "AmbulanceUnit",
              "AMB-03",
              "沪120-急救03车",
              121.48,
              31.31,
              11.8,
            ],
            [
              "00000000-0000-0000-0000-000000003544",
              "AmbulanceUnit",
              "AMB-04",
              "沪120-急救04车",
              121.36,
              31.29,
              18.4,
            ],
          ],
          row_count: 5,
          truncated: false,
          generated_at: "2026-04-01T23:45:00Z",
          data_version: "v4-med-map",
          drill_level: 0,
        }),
      });
      return;
    }

    if (
      pathname.startsWith("/api/xblueview/widgets/") &&
      pathname.endsWith("/data") &&
      method === "POST"
    ) {
      const val = pathname.includes(MOCK_MED_KPI_WIDGET_ID)
        ? state.hospFreeBeds
        : 0.782;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [{ name: "value", data_type: "FLOAT64" }],
          rows: [[val]],
          row_count: 1,
          truncated: false,
          generated_at: "2026-04-01T23:45:00Z",
          data_version: `v4-kpi-${state.surgeInvocations}`,
          drill_level: 0,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${MOCK_MED_ACT_WIDGET_ID}/invoke` &&
      method === "POST"
    ) {
      state.surgeInvocations += 1;
      state.hospFreeBeds = 62;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mode: "WRITE_NODE",
          run_id: null,
          status: "DONE",
          node_id: MOCK_HOSP01_ID,
          action_log_id: state.surgeInvocations,
          idempotency_replay: false,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/geo/convex-hull" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          point_count: 64,
          area_sq_deg: 0.084,
          hull: {
            type: "Polygon",
            coordinates: [
              [
                [121.36, 31.18],
                [121.54, 31.19],
                [121.48, 31.31],
                [121.36, 31.29],
                [121.36, 31.18],
              ],
            ],
          },
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
              id: "00000000-0000-0000-0000-000000003580",
              name: "fin_corp_wire_transfers",
              display_name: "对公资金划转流水表（6万行）",
              row_count: 60_000,
              columns: [
                { column_name: "tx_id", data_type: "STRING" },
                { column_name: "branch_dept", data_type: "STRING" },
                { column_name: "risk_tier", data_type: "STRING" },
                { column_name: "operator_phone", data_type: "STRING" },
                { column_name: "payee_card_no", data_type: "STRING" },
                { column_name: "transfer_amt", data_type: "FLOAT64" },
                { column_name: "tx_count", data_type: "INT64" },
              ],
            },
            {
              id: "00000000-0000-0000-0000-000000003581",
              name: "grid_feeder_telemetry",
              display_name: "馈线遥测明细表（4.32万行）",
              row_count: 43_200,
              columns: [
                { column_name: "grid_zone", data_type: "STRING" },
                { column_name: "active_mw", data_type: "FLOAT64" },
              ],
            },
          ],
          platform_views: [],
          live_connections: [],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/pivot" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          row_headers: [
            ["sh_branch"],
            ["hz_branch"],
            ["sz_branch"],
            ["bj_branch"],
            ["nj_branch"],
            ["cd_branch"],
            ["总计"],
          ],
          column_headers: [["transfer_amt (SUM)"]],
          row_kinds: [
            "DATA",
            "DATA",
            "DATA",
            "DATA",
            "DATA",
            "DATA",
            "GRAND_TOTAL",
          ],
          cells: [
            ["184520.5"],
            ["152310.0"],
            ["168940.2"],
            ["141200.8"],
            ["129880.4"],
            ["119450.1"],
            ["896302.0"],
          ],
          columns_truncated: false,
          sql: "SELECT branch_dept, SUM(transfer_amt) FROM fin_corp_wire_transfers GROUP BY 1",
          duration_ms: 18,
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/sql/run" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "operator_phone", data_type: "STRING" },
            { name: "payee_card_no", data_type: "STRING" },
          ],
          rows: [
            {
              operator_phone: "138****5621",
              payee_card_no: "622848*********3019",
            },
            {
              operator_phone: "139****8842",
              payee_card_no: "622202*********7741",
            },
          ],
          row_count: 2,
          truncated: false,
          duration_ms: 9,
          sql: "SELECT operator_phone, payee_card_no FROM fin_corp_wire_transfers LIMIT 5",
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
            id: "00000000-0000-0000-0000-000000003590",
            slug: "main",
            display_name: "企业主图",
            description: "跨行业核心主图",
            owner_dept: "default",
            is_main: true,
            current_version: 3,
          },
          {
            id: "00000000-0000-0000-0000-000000003591",
            slug: "semi_supply",
            display_name: "半导体全球供应链",
            description: "采购、制造、物流、履约四域泳道模型",
            owner_dept: "default",
            is_main: false,
            current_version: 2,
          },
          {
            id: "00000000-0000-0000-0000-000000003592",
            slug: "power_grid",
            display_name: "新型电力与配网调度",
            description: "配网多边形围栏与主变调度模型",
            owner_dept: "default",
            is_main: false,
            current_version: 2,
          },
          {
            id: "00000000-0000-0000-0000-000000003593",
            slug: "fin_risk",
            display_name: "集团授信与互保风控",
            description: "集团互保环与敏感脱敏模型",
            owner_dept: "default",
            is_main: false,
            current_version: 2,
          },
          {
            id: "00000000-0000-0000-0000-000000003594",
            slug: "med_dispatch",
            display_name: "城市急诊床位调度",
            description: "120院前急救空间凸包模型",
            owner_dept: "default",
            is_main: false,
            current_version: 2,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/xblue/schema") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(buildSemiSupplySchemaMock()),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/validate") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ valid: true, issues: [], pass_count: 14 }),
      });
      return;
    }

    if (pathname === "/api/xblue/nodes" && method === "GET") {
      const typeName = searchParams.get("type_name") ?? "";
      if (typeName === "BeneficialOwner") {
        const items = Array.from({ length: 10 }, (_, idx) => {
          const code = `BO-${String(idx + 1).padStart(3, "0")}`;
          return {
            id: `00000000-0000-0000-0000-0000000036${String(idx).padStart(2, "0")}`,
            type_name: "BeneficialOwner",
            node_key: code,
            title: `实控人·${code}`,
            properties: {
              full_name: "张*",
              mobile_phone: "138****6612",
              national_id: "310115********401X",
              settlement_card: "622848*********3019",
            },
            metrics: { guaranteed_amt_wan: 1200 },
          };
        });
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ items, nodes: items, links: [], next_cursor: null }),
        });
        return;
      }
      if (typeName === "Substation") {
        const items = [
          {
            id: MOCK_PG_SUB01_ID,
            type_name: "Substation",
            node_key: "SUB-001",
            title: "城北220kV枢纽变电站",
            properties: {},
            metrics: { load_ratio: 0.94 },
            geo: { lng: 120.17, lat: 30.32 },
            geo_shape: northPoly,
          },
        ];
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ items, nodes: items, links: [], next_cursor: null }),
        });
        return;
      }
      if (typeName === "HospitalCampus") {
        const items = [
          {
            id: MOCK_HOSP01_ID,
            type_name: "HospitalCampus",
            node_key: "HOSP-01",
            title: "市中心第一三甲总院",
            properties: {},
            metrics: { total_free_beds: state.hospFreeBeds },
          },
        ];
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ items, nodes: items, links: [], next_cursor: null }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], nodes: [], links: [], next_cursor: null }),
      });
      return;
    }

    if (
      pathname === "/api/xblue/drafts" ||
      pathname === "/api/xblue/mappings" ||
      pathname === "/api/xblue/schema/versions" ||
      pathname === "/api/analytics/metrics" ||
      pathname === "/api/analytics/saved" ||
      pathname === "/api/axguard/users" ||
      pathname === "/api/axguard/roles" ||
      pathname === "/api/axguard/departments" ||
      pathname === "/api/axguard/policies" ||
      pathname === "/api/axguard/row-policies" ||
      pathname === "/api/axguard/column-policies" ||
      pathname === "/api/axguard/grants" ||
      pathname === "/api/axagent/providers" ||
      pathname === "/api/axagent/llm-preferences"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/axops/health/deep") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          database: { ok: true, latency_ms: 2, replication_lag_s: 0 },
          parser: { ok: true, version: "0.4.0" },
          lake: { ok: true, snapshot_id: 4 },
          llm: { ok: true, provider: "cassette" },
          embed: { ok: true },
          disk: { ok: true, data_root_free_bytes: 50_000_000_000 },
          jobs: { pending: 0, running: 0, failed_24h: 0, oldest_pending_s: 0 },
          outbox: { lag_s: 0, pending: 0 },
          migrations: { ok: true, current: 25, pending: 0 },
          version: "0.4.0",
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
          version: "0.4.0",
          env: "development",
          tenant_id: "default",
          uptime_s: 3600,
          data_root: "/var/lib/axpoint",
          template_packs_count: 7,
        }),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [], nodes: [], links: [], edges: [], next_cursor: null }),
    });
  });
}

async function resolveLiveAdminSession(
  request: APIRequestContext,
  coreBaseUrl: string
): Promise<{ password: string; accessToken: string }> {
  const candidates = [
    process.env.OPS_ADMIN_PASSWORD,
    "Axp#Ops_2026!99",
    "AxPointDemo#2026",
  ].filter((v): v is string => Boolean(v && v.trim()));

  for (const pw of candidates) {
    const resp = await request.post(`${coreBaseUrl}/api/auth/login`, {
      data: { tenant: "default", username: "ops_admin", password: pw },
    });
    if (resp.ok()) {
      const body = (await resp.json()) as { access_token: string };
      return { password: pw, accessToken: body.access_token };
    }
  }
  throw new Error("Unable to authenticate ops_admin against live Core API");
}

test.describe("Step 35 Phase 4 Modeling Console & Four Industry Worlds Full Journey", () => {
  test("completes all 6 stages: plain modeling nav & flat settings, live 145k+ pulse dashboard, power_grid geofence & deadline_step, fin_risk 60k VTable & PII masking, semi_supply 4-domain 0-overlap swimlanes & med_dispatch ambulance convex hull, and surge-bed writeback to pulse", async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const isLive = process.env.AXPOINT_E2E_LIVE === "1";
    const coreBaseUrl = (
      process.env.CORE_BASE_URL ?? "http://127.0.0.1:9995"
    ).replace(/\/$/, "");

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

    const mockState: MockV4State = {
      surgeInvocations: 0,
      hospFreeBeds: 36,
    };

    let opsPassword = "Axp#Ops_2026!99";
    let adminToken = "";

    if (!isLive) {
      await installV4Mocks(page, mockState, leakedTenantUrls);
    } else {
      page.on("request", (req) => {
        if (req.url().includes("/api/")) {
          const parsed = new URL(req.url());
          if (parsed.searchParams.has("tenant_id")) {
            leakedTenantUrls.push(req.url());
          }
        }
      });

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

      const auth = await resolveLiveAdminSession(page.request, coreBaseUrl);
      opsPassword = auth.password;
      adminToken = auth.accessToken;
      const authHeaders = {
        Authorization: `Bearer ${adminToken}`,
        "Content-Type": "application/json",
      };

      const checkPulse = await page.request.get(
        `${coreBaseUrl}/api/axops/pulse`,
        { headers: authHeaders }
      );
      let showcaseReady = false;
      if (checkPulse.ok()) {
        const p = (await checkPulse.json()) as PulsePayload;
        const l4Slugs = new Set(
          p.subgraphs
            .filter((s) => s.cert_level === "L4_CERTIFIED")
            .map((s) => s.slug)
        );
        showcaseReady =
          p.totals.node_count >= 2_300 &&
          p.totals.lake_total_rows >= 145_000 &&
          ["power_grid", "fin_risk", "semi_supply", "med_dispatch"].every((s) =>
            l4Slugs.has(s)
          );
      }
      if (!showcaseReady) {
        const loadResp = await page.request.post(
          `${coreBaseUrl}/api/axops/demo/showcase/load`,
          {
            headers: authHeaders,
            data: {},
            timeout: 60_000,
          }
        );
        expect(loadResp.ok(), await loadResp.text()).toBe(true);
      }
    }

    await page.setViewportSize({ width: 1440, height: 900 });

    // =========================================================================
    // Stage 1: Plain Modeling Navigation, Flat Single-Row Settings & Zero Jargon
    // =========================================================================
    await page.context().clearCookies();
    await page.goto("/login");
    await page.evaluate(() => {
      localStorage.clear();
      sessionStorage.clear();
      sessionStorage.setItem("axpoint_logged_out", "1");
    });
    await page.goto("/login");

    await expect(
      page.getByRole("heading", { level: 1, name: "登录" })
    ).toBeVisible();
    await page.getByTestId("login-tenant-input").fill("default");
    await page.getByTestId("login-username-input").fill("ops_admin");
    await page.getByTestId("login-password-input").fill(opsPassword);
    await page.getByTestId("login-submit-btn").click();
    await expect(page).toHaveURL(/\/$/, { timeout: 20_000 });

    const primaryNav = page.getByTestId("primary-domain-nav");
    await expect(primaryNav).toBeVisible();
    const primaryLinks = primaryNav.locator("a");
    await expect(primaryLinks).toHaveCount(8);

    const zhNavLabels = (await primaryLinks.allInnerTexts()).map((s) =>
      s.trim()
    );
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

    await expect(page.getByTestId("governance-section-toggle")).toHaveCount(0);
    await expect(page.getByTestId("sidebar-nav-governance")).toHaveCount(0);
    const settingsLink = page.getByTestId("sidebar-nav-settings");
    await expect(settingsLink).toBeVisible();
    await expect(settingsLink).toHaveText("系统设置");
    await expect(settingsLink).toHaveAttribute("href", "/settings");

    await page.goto("/governance?section=security");
    await expect(page).toHaveURL(/\/settings\?section=security/);
    await expect(
      page.getByRole("heading", { level: 1, name: "系统设置" })
    ).toBeVisible();
    expect(await collectVisibleJargonLeaks(page)).toEqual([]);

    // Bilingual (`en-US` & `zh-CN`) zero-jargon verification
    await page.getByTestId("locale-toggle-btn").click();
    await expect(
      page.getByRole("heading", { level: 1, name: "Settings" })
    ).toBeVisible();
    const enNavLabels = (await primaryLinks.allInnerTexts()).map((s) =>
      s.trim()
    );
    expect(enNavLabels[2]).toBe("Modeling Studio");
    await expect(settingsLink).toHaveText("Settings");
    expect(await collectVisibleJargonLeaks(page)).toEqual([]);

    await page.getByTestId("locale-toggle-btn").click();
    await expect(
      page.getByRole("heading", { level: 1, name: "系统设置" })
    ).toBeVisible();

    // =========================================================================
    // Stage 2: Live Operational Dashboard (`/`) & 4 L4 Industry Worlds Matrix
    // =========================================================================
    const pulsePromise = page.waitForResponse(
      (resp) =>
        new URL(resp.url()).pathname === "/api/axops/pulse" &&
        resp.request().method() === "GET" &&
        resp.status() === 200
    );
    await page.goto("/");
    const pulseBefore = (await (await pulsePromise).json()) as PulsePayload;

    await expect(
      page.getByRole("heading", { level: 1, name: "总控看板" })
    ).toBeVisible();
    expect(await collectVisibleJargonLeaks(page)).toEqual([]);

    // Zero static legacy recall card
    await expect(page.getByText("五路融合召回架构")).toHaveCount(0);
    await expect(page.getByText("5-Way Recall Channels")).toHaveCount(0);
    await expect(page.getByText("倒数秩融合重排")).toHaveCount(0);

    // Exact match with /api/axops/pulse & scale redlines (>= 2,300 nodes, >= 145,000 lake rows)
    expect(pulseBefore.totals.node_count).toBeGreaterThanOrEqual(2_300);
    expect(pulseBefore.totals.lake_total_rows).toBeGreaterThanOrEqual(145_000);

    await expect(page.getByTestId("pulse-kpi-value-nodes")).toHaveText(
      formatZhInt(pulseBefore.totals.node_count)
    );
    await expect(page.getByTestId("pulse-kpi-value-links")).toHaveText(
      formatZhInt(pulseBefore.totals.link_count)
    );
    await expect(page.getByTestId("pulse-kpi-value-lake-rows")).toHaveText(
      formatZhInt(pulseBefore.totals.lake_total_rows)
    );
    await expect(page.getByTestId("pulse-kpi-value-throughput")).toHaveText(
      formatZhInt(pulseBefore.totals.metric_points_24h)
    );

    for (const worldSlug of [
      "power_grid",
      "fin_risk",
      "semi_supply",
      "med_dispatch",
    ]) {
      await expect(page.getByTestId(`subgraph-card-${worldSlug}`)).toBeVisible();
      await expect(
        page.getByTestId(`subgraph-cert-${worldSlug}`)
      ).toContainText("L4");
    }

    for (const chartId of [
      "pulse-throughput-chart",
      "pulse-subgraph-bar-chart",
    ]) {
      const canvas = page.getByTestId(chartId).locator("canvas").first();
      await expect(canvas).toBeVisible();
      const box = await canvas.boundingBox();
      expect(box?.width ?? 0).toBeGreaterThan(0);
      expect(box?.height ?? 0).toBeGreaterThan(0);
    }

    // =========================================================================
    // Stage 3: Power Grid World (`power_grid` Sim Deadline & Polygon Geofence)
    // =========================================================================
    await page.getByTestId("subgraph-sim-btn-power_grid").click();
    await expect(page).toHaveURL(/\/sim\?/);
    await expect(page.getByTestId("axsim-workbench")).toBeVisible();

    const deadlineBanner = page.getByTestId("goal-deadline-banner");
    await expect(deadlineBanner).toBeVisible({ timeout: 20_000 });
    await expect(page.getByTestId("earliest-breach-step")).toContainText(/\d+/);
    await expect(page.getByTestId("earliest-deadline-step")).toContainText(
      /\d+/
    );

    await page.goto("/");
    await expect(page.getByTestId("subgraph-card-power_grid")).toBeVisible();
    await page.getByTestId("subgraph-view-btn-power_grid").click();
    await expect(page).toHaveURL(/\/view\/power-grid-cockpit/);

    const pgMapWidget = page.locator("[data-testid^='widget-map-']").first();
    await expect(pgMapWidget).toBeVisible({ timeout: 25_000 });
    const pgMapCanvas = pgMapWidget.locator("canvas").first();
    await expect(pgMapCanvas).toBeVisible({ timeout: 20_000 });
    const pgMapBox = await pgMapCanvas.boundingBox();
    expect(pgMapBox?.width ?? 0).toBeGreaterThan(0);
    expect(pgMapBox?.height ?? 0).toBeGreaterThan(0);

    const polygonSubstationCount = await page.evaluate(
      async ({ tok }) => {
        const hdrs: Record<string, string> = {};
        if (tok) hdrs.Authorization = `Bearer ${tok}`;
        const r = await fetch(
          "/api/xblue/nodes?subgraph=power_grid&type_name=Substation&limit=50",
          { headers: hdrs }
        );
        const body = (await r.json()) as {
          items?: Array<{ geo_shape?: { type?: string } | null }>;
        };
        return (body.items ?? []).filter(
          (n) => n.geo_shape?.type === "Polygon"
        ).length;
      },
      { tok: adminToken }
    );
    expect(polygonSubstationCount).toBeGreaterThan(0);

    // =========================================================================
    // Stage 4: Financial Risk World (`fin_risk` 60k VTable & Analyst PII Mask)
    // =========================================================================
    await page.goto("/analytics?tab=pivot");
    await expect(page.getByTestId("analytics-pivot-builder")).toBeVisible();
    await chooseSelectOption(
      page,
      "pivot-dataset-select",
      "fin_corp_wire_transfers"
    );

    const branchQuickRow = page.getByTestId("pivot-quick-row-branch_dept");
    const fallbackQuickRow = page
      .locator("[data-testid^='pivot-quick-row-']")
      .first();
    const amtQuickVal = page.getByTestId("pivot-quick-val-transfer_amt");
    const fallbackQuickVal = page
      .locator("[data-testid^='pivot-quick-val-']")
      .first();

    await expect(fallbackQuickVal).toBeVisible({ timeout: 15_000 });
    if (await branchQuickRow.isVisible()) {
      await branchQuickRow.click();
    } else if (await fallbackQuickRow.isVisible()) {
      await fallbackQuickRow.click();
    }
    if (await amtQuickVal.isVisible()) {
      await amtQuickVal.click();
    } else {
      await fallbackQuickVal.click();
    }

    const pivotRunBtn = page.getByTestId("pivot-run-btn");
    if (await pivotRunBtn.isEnabled()) {
      await pivotRunBtn.click();
    }

    const vtableStage = page.getByTestId("vtable-canvas-stage");
    await expect(vtableStage).toBeVisible({ timeout: 25_000 });
    const vtableCanvas = vtableStage.locator("canvas").first();
    await expect(vtableCanvas).toBeVisible({ timeout: 15_000 });
    const vtableBox = await vtableCanvas.boundingBox();
    expect(vtableBox?.width ?? 0).toBeGreaterThan(0);
    expect(vtableBox?.height ?? 0).toBeGreaterThan(0);
    await expect(vtableStage).toHaveAttribute("data-databars", "on");

    await page.getByTestId("vtable-toggle-heatmap").click();
    await expect(vtableStage).toHaveAttribute("data-heatmap", "on");

    // Verify BeneficialOwner & wire-transfer PII masking under non-privileged ANALYST role
    const piiAudit = await page.evaluate(async () => {
      const loginResp = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          tenant: "default",
          username: "fin_analyst_sh",
          password: "Axp#FinSh_2026!55",
        }),
      });
      const loginJson = (await loginResp.json()) as { access_token?: string };
      const analystTok = loginJson.access_token ?? "";
      const hdrs = {
        Authorization: `Bearer ${analystTok}`,
        "Content-Type": "application/json",
      };

      const boResp = await fetch(
        "/api/xblue/nodes?subgraph=fin_risk&type_name=BeneficialOwner&limit=10",
        { headers: hdrs }
      );
      const boJson = (await boResp.json()) as {
        items?: Array<{
          node_key: string;
          title: string;
          properties: Record<string, unknown>;
        }>;
      };
      const boItems = boJson.items ?? [];

      const sqlResp = await fetch("/api/analytics/sql/run", {
        method: "POST",
        headers: hdrs,
        body: JSON.stringify({
          sql: "SELECT operator_phone, payee_card_no FROM fin_corp_wire_transfers LIMIT 5",
        }),
      });
      const sqlJson = (await sqlResp.json()) as {
        rows?: Array<Record<string, unknown>>;
      };
      const sqlRows = sqlJson.rows ?? [];

      return {
        boCount: boItems.length,
        boMasked:
          boItems.length > 0 &&
          boItems.every(
            (b) =>
              b.title === `实控人·${b.node_key}` &&
              String(b.properties.full_name ?? "").includes("*") &&
              String(b.properties.mobile_phone ?? "").includes("*") &&
              String(b.properties.national_id ?? "").includes("*")
          ),
        wireMasked:
          sqlRows.length > 0 &&
          sqlRows.every(
            (r) =>
              String(r.operator_phone ?? "").includes("*") &&
              String(r.payee_card_no ?? "").includes("*")
          ),
      };
    });
    expect(piiAudit.boCount).toBeGreaterThan(0);
    expect(piiAudit.boMasked).toBe(true);
    expect(piiAudit.wireMasked).toBe(true);

    // =========================================================================
    // Stage 5: Semiconductor 4-Domain Swimlanes (0 Overlap) & Med Convex Hull
    // =========================================================================
    await page.goto("/studio?subgraph=semi_supply");
    await expect(page.getByTestId("xbluestudio-canvas")).toBeVisible();
    await expect(page.getByTestId("canvas-node-WaferFab")).toBeVisible({
      timeout: 20_000,
    });
    await expect(page.getByTestId("canvas-node-SpecialtyVendor")).toBeVisible();
    await expect(page.getByTestId("canvas-node-CustomsPort")).toBeVisible();
    await expect(page.getByTestId("canvas-node-RegionalHub")).toBeVisible();

    await page.getByTestId("canvas-domain-group-toggle").click();
    for (const domainName of ["采购域", "制造域", "物流域", "履约域"]) {
      await expect(
        page.getByTestId(`domain-group-${domainName}`)
      ).toBeVisible();
    }

    await page.waitForTimeout(280);
    const swimlaneRects = await page.evaluate(() => {
      const nodeEls = Array.from(
        document.querySelectorAll<HTMLElement>('[data-testid^="canvas-node-"]')
      ).filter((el) => {
        const tid = el.getAttribute("data-testid") ?? "";
        return (
          !tid.startsWith("canvas-node-badge-") &&
          !tid.startsWith("canvas-node-tier-") &&
          !tid.startsWith("canvas-node-proposed-")
        );
      });
      const edgeEls = Array.from(
        document.querySelectorAll<HTMLElement>('[data-testid^="canvas-edge-"]')
      );
      return {
        nodes: nodeEls.map((el) => {
          const r = el.getBoundingClientRect();
          return {
            id: el.getAttribute("data-testid") ?? "",
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
          };
        }),
        edges: edgeEls.map((el) => {
          const r = el.getBoundingClientRect();
          return {
            id: el.getAttribute("data-testid") ?? "",
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
          };
        }),
      };
    });
    expect(swimlaneRects.nodes.length).toBe(8);
    for (let i = 0; i < swimlaneRects.nodes.length; i += 1) {
      for (let j = i + 1; j < swimlaneRects.nodes.length; j += 1) {
        expect(
          computeRectOverlapArea(swimlaneRects.nodes[i]!, swimlaneRects.nodes[j]!)
        ).toBe(0);
      }
    }
    for (const eRect of swimlaneRects.edges) {
      for (const nRect of swimlaneRects.nodes) {
        expect(computeRectOverlapArea(eRect, nRect)).toBe(0);
      }
    }

    // Verify 3-tier folding on WaferFab collapses WaferLotEvent & YieldRollupCube into drawer
    await page
      .getByTestId("fold-tier-btn-WaferFab")
      .evaluate((el: HTMLElement) => el.click());
    await expect(page.getByTestId("canvas-node-WaferLotEvent")).toHaveCount(0);
    await expect(page.getByTestId("canvas-node-YieldRollupCube")).toHaveCount(0);
    await expect(page.getByTestId("folded-tier-drawer-WaferFab")).toBeVisible();
    await page
      .getByTestId("fold-tier-btn-WaferFab")
      .evaluate((el: HTMLElement) => el.click());
    await expect(page.getByTestId("canvas-node-WaferLotEvent")).toBeVisible();

    // Navigate to med-dispatch-cockpit & verify 120 ambulance map + convex hull polygon
    const hullRespPromise = page
      .waitForResponse(
        (resp) =>
          new URL(resp.url()).pathname === "/api/xblue/geo/convex-hull" &&
          resp.status() === 200,
        { timeout: 20_000 }
      )
      .catch(() => null);

    await page.goto("/view/med-dispatch-cockpit");
    const medMapWidget = page.locator("[data-testid^='widget-map-']").first();
    await expect(medMapWidget).toBeVisible({ timeout: 25_000 });
    const medMapCanvas = medMapWidget.locator("canvas").first();
    await expect(medMapCanvas).toBeVisible({ timeout: 20_000 });
    const medMapBox = await medMapCanvas.boundingBox();
    expect(medMapBox?.width ?? 0).toBeGreaterThan(0);
    expect(medMapBox?.height ?? 0).toBeGreaterThan(0);

    const hullToggle = page.getByTestId("map-convex-hull-toggle");
    await expect(hullToggle).toBeVisible({ timeout: 15_000 });
    let hullResp = await hullRespPromise;
    const toggleClass = (await hullToggle.getAttribute("class")) ?? "";
    if (!toggleClass.includes("border-xblue")) {
      const toggleHullPromise = page.waitForResponse(
        (resp) =>
          new URL(resp.url()).pathname === "/api/xblue/geo/convex-hull" &&
          resp.status() === 200
      );
      await hullToggle.click();
      hullResp = await toggleHullPromise;
    }
    expect(hullResp).not.toBeNull();
    if (hullResp) {
      const hullBody = (await hullResp.json()) as {
        point_count?: number;
        hull?: { type?: string };
      };
      expect(hullBody.hull?.type).toBe("Polygon");
      expect(hullBody.point_count ?? 0).toBeGreaterThanOrEqual(3);
    }
    await expect(hullToggle).toHaveClass(/border-xblue/);

    // =========================================================================
    // Stage 6: Action Writeback (`启用应急加床`) & Live Pulse Refresh Loop
    // =========================================================================
    const surgeTriggerBtn = page.getByRole("button", { name: "启用应急加床" });
    await expect(surgeTriggerBtn).toBeVisible({ timeout: 15_000 });
    await surgeTriggerBtn.click();

    await expect(
      page.locator("[data-testid^='action-result-']").first()
    ).toContainText("DONE", { timeout: 15_000 });

    const pulseAfterPromise = page.waitForResponse(
      (resp) =>
        new URL(resp.url()).pathname === "/api/axops/pulse" &&
        resp.request().method() === "GET" &&
        resp.status() === 200
    );
    await page.goto("/");
    const pulseAfter = (await (await pulseAfterPromise).json()) as PulsePayload;

    await expect(
      page.getByRole("heading", { level: 1, name: "总控看板" })
    ).toBeVisible();
    await expect(page.getByTestId("pulse-kpi-value-throughput")).toHaveText(
      formatZhInt(pulseAfter.totals.metric_points_24h)
    );
    expect(pulseAfter.totals.metric_points_24h).toBeGreaterThanOrEqual(
      pulseBefore.totals.metric_points_24h
    );

    expect(leakedTenantUrls).toEqual([]);
    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
  });
});
