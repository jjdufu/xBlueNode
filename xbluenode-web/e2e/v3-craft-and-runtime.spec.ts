import {
  expect,
  type APIRequestContext,
  type Locator,
  type Page,
  type Route,
  test,
} from "@playwright/test";
import { chooseSelectOption } from "./helpers";

async function selectFieldByLabel(
  page: Page,
  container: Locator,
  labelPrefix: string,
  optionValue: string
): Promise<void> {
  const field = container
    .locator("div.space-y-1")
    .filter({ hasText: labelPrefix })
    .first();
  await field.locator("button[role='combobox']").click();
  await page
    .locator(`[role="listbox"] [data-option-value="${optionValue}"]`)
    .click();
}

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const CODENAME_PREFIX_RE =
  /\b(AxHub|AxIngest|AxParser|AxGraph|AxAnalytics|AxQuery|xBlueStudio|xBlueGraph|AxFlow|AxAlerts|AxAgent|AxBuilder|AxSim|xBlueView|AxGuard|AxOps)\s*·/;

const SCREAMING_SNAKE_RE = /\b[A-Z]{2,}_[A-Z0-9_]+\b/g;

const OPS_ADMIN_ID = "00000000-0000-0000-0000-000000003001";
const MOCK_CERT_SLUG = "v3_cert_sandbox";
const MOCK_DEMO_DS_ID = "00000000-0000-0000-0000-000000003002";
const MOCK_CHART_ID = "00000000-0000-0000-0000-000000003010";
const MOCK_PIN_WIDGET_ID = "00000000-0000-0000-0000-000000003011";
const MOCK_DASHBOARD_ID = "00000000-0000-0000-0000-000000003020";
const MOCK_WIDGET_KPI_ID = "00000000-0000-0000-0000-000000003021";
const MOCK_WIDGET_BAR_ID = "00000000-0000-0000-0000-000000003022";
const MOCK_WIDGET_ACT_ID = "00000000-0000-0000-0000-000000003023";
const MOCK_FLOW_ID = "00000000-0000-0000-0000-000000003030";
const MOCK_FLOW_RUN_ID = "00000000-0000-0000-0000-000000003031";
const MOCK_SANDBOX_ID = "00000000-0000-0000-0000-000000003040";
const MOCK_SCENARIO_ID = "00000000-0000-0000-0000-000000003041";
const MOCK_PLANT_ID = "00000000-0000-0000-0000-000000003051";
const MOCK_HUB_SH_ID = "00000000-0000-0000-0000-000000003052";
const MOCK_HUB_SUZ_ID = "00000000-0000-0000-0000-000000003053";

function buildMockJwt(label: string): string {
  const encodeSeg = (obj: Record<string, unknown>): string =>
    btoa(JSON.stringify(obj))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  const nowSec = Math.floor(Date.now() / 1000);
  return `${encodeSeg({ alg: "EdDSA", typ: "JWT", kid: "k1" })}.${encodeSeg({
    iss: "axpoint",
    aud: "axpoint-api",
    sub: OPS_ADMIN_ID,
    tid: "default",
    dept: "east",
    roles: ["ADMIN"],
    typ: "access",
    iat: nowSec,
    exp: nowSec + 900,
    jti: label,
  })}.sig_${label}`;
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

function buildV3CompoundSchemaDoc() {
  return {
    node_types: [
      {
        name: "V3Plant",
        display_name: "制造基地",
        description: "上游产能供给工厂",
        icon: "factory",
        color: "#0f766e",
        title_template: "{node_key}",
        semantic_layer_tier: "STATIC_FACT",
        domain_group: "制造域",
        properties: [
          {
            name: "output",
            display_name: "日产能",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "件",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        sync_to_knowledge: false,
        pos_x: 96,
        pos_y: 144,
      },
      {
        name: "V3Hub",
        display_name: "区域枢纽仓",
        description: "核心仓储与履约调拨枢纽",
        icon: "warehouse",
        color: "#2563eb",
        title_template: "{node_key}",
        semantic_layer_tier: "STATIC_FACT",
        domain_group: "履约域",
        properties: [
          {
            name: "region",
            display_name: "所属大区",
            data_type: "STRING",
            kind: "ATTRIBUTE",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "inventory",
            display_name: "在库库存",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "件",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "outbound_total",
            display_name: "累计出库",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "件",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [
          {
            target_metric: "outbound_total",
            op: "SUM",
            via_link_type: "V3_RECORDS_OUTBOUND",
            direction: "OUT",
            source_type_name: "V3Outbound",
            source_metric: "qty",
            weight_by_link: false,
          },
        ],
        derived_rules: [],
        actions: [
          {
            name: "dispatch_stock",
            display_name: "紧急发运调拨",
            parameters: [
              {
                name: "qty",
                display_name: "调拨件数",
                data_type: "FLOAT64",
                required: true,
              },
            ],
            precondition_expr: "inventory >= param.qty and param.qty > 0",
            side_effect_tier: "SELF_ONLY",
            mutations: [
              {
                target: "self.inventory",
                expression: "inventory - param.qty",
              },
            ],
          },
        ],
        sync_to_knowledge: true,
        pos_x: 512,
        pos_y: 144,
      },
      {
        name: "V3Outbound",
        display_name: "出库流水单",
        description: "枢纽仓单笔出库事件",
        icon: "truck",
        color: "#d97706",
        title_template: "{node_key}",
        semantic_layer_tier: "EVENT_DYNAMIC",
        domain_group: "履约域",
        properties: [
          {
            name: "qty",
            display_name: "出库数量",
            data_type: "FLOAT64",
            kind: "METRIC",
            unit: "件",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        actions: [],
        sync_to_knowledge: false,
        pos_x: 928,
        pos_y: 96,
      },
      {
        name: "V3StockCube",
        display_name: "库存日度汇总",
        description: "枢纽仓日度周转汇总切片",
        icon: "database",
        color: "#7c3aed",
        title_template: "{node_key}",
        semantic_layer_tier: "CUBE_ROLLUP",
        domain_group: "履约域",
        properties: [
          {
            name: "turnover_rate",
            display_name: "日周转率",
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
        sync_to_knowledge: false,
        pos_x: 928,
        pos_y: 352,
      },
    ],
    edge_types: [
      {
        name: "V3_SUPPLIES",
        src_type_name: "V3Plant",
        dst_type_name: "V3Hub",
        impact_weight: 1.0,
        cardinality: "ONE_TO_MANY",
        description: "工厂向枢纽仓补货",
        properties: [],
        propagation: {
          rules: [
            {
              from_metric: "output",
              to_metric: "inventory",
              coefficient: 1.0,
              lag_steps: 3,
            },
          ],
        },
      },
      {
        name: "V3_BACKUP_SUPPLIES",
        src_type_name: "V3Plant",
        dst_type_name: "V3Hub",
        impact_weight: 0.45,
        cardinality: "MANY_TO_MANY",
        description: "应急备选补货通道",
        properties: [],
        propagation: {},
      },
      {
        name: "V3_RECORDS_OUTBOUND",
        src_type_name: "V3Hub",
        dst_type_name: "V3Outbound",
        impact_weight: 0.85,
        cardinality: "ONE_TO_MANY",
        description: "枢纽仓产生出库流水",
        properties: [],
        propagation: {},
      },
      {
        name: "V3_SUMMARIZES_STOCK",
        src_type_name: "V3Hub",
        dst_type_name: "V3StockCube",
        impact_weight: 0.75,
        cardinality: "ONE_TO_MANY",
        description: "枢纽仓归集日度立方",
        properties: [],
        propagation: {},
      },
    ],
    mappings: [
      {
        name: "v3_orders_to_outbound",
        dataset: "demo_orders",
        node_type: "V3Outbound",
        key_column: "order_id",
        title: "{order_id}",
        attributes: {},
        metrics: { qty: "amount" },
        geo: { lng_column: null, lat_column: null },
        links: [],
        on_source_delete: "KEEP",
        mode: "FULL",
      },
    ],
  };
}

interface MockV3State {
  hubInventory: number;
  kpiSeries: Array<[string, number]>;
  savedCharts: Array<Record<string, unknown>>;
  pinnedWidgets: Array<Record<string, unknown>>;
  compactMode: "VERTICAL_COMPACT" | "FREE_FLOAT";
  widgetPositions: Record<
    string,
    { x: number; y: number; w: number; h: number }
  >;
  flowDoc: Record<string, unknown>;
  flowRuns: Array<Record<string, unknown>>;
  scenarios: Array<Record<string, unknown>>;
  competencyQuestions: Array<Record<string, unknown>>;
  harnessRuns: Array<Record<string, unknown>>;
}

async function installV3Mocks(
  page: Page,
  leakedTenantUrls: string[],
  cockpitSlug: string
): Promise<MockV3State> {
  const compoundDoc = buildV3CompoundSchemaDoc();
  const nowIso = "2026-11-24T10:00:00Z";

  const state: MockV3State = {
    hubInventory: 500,
    kpiSeries: [
      ["2026-11-21T00:00:00Z", 460],
      ["2026-11-22T00:00:00Z", 480],
      ["2026-11-23T00:00:00Z", 500],
    ],
    savedCharts: [],
    pinnedWidgets: [],
    compactMode: "VERTICAL_COMPACT",
    widgetPositions: {
      [MOCK_WIDGET_KPI_ID]: { x: 0, y: 0, w: 5, h: 3 },
      [MOCK_WIDGET_BAR_ID]: { x: 5, y: 1, w: 6, h: 4 },
      [MOCK_WIDGET_ACT_ID]: { x: 0, y: 6, w: 6, h: 4 },
    },
    flowDoc: {
      id: MOCK_FLOW_ID,
      name: "v3_supply_guard_flow",
      description: "库存巡检与审批编排",
      slug: "v3-supply-guard",
      nodes: [
        {
          id: "sync_step",
          kind: "SQL_TRANSFORM",
          label: "库存快照提取",
          config: { sql: "SELECT 1 AS n", max_rows: 50 },
          retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" },
          timeout_s: 120,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 96, y: 160 },
        },
        {
          id: "eval_step",
          kind: "SQL_TRANSFORM",
          label: "安全红线核验",
          config: { sql: "SELECT 2 AS m", max_rows: 50 },
          retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" },
          timeout_s: 120,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 460, y: 160 },
        },
      ],
      edges: [{ from: "sync_step", to: "eval_step", when: "n > 0" }],
      trigger_cfg: { type: "MANUAL" },
      status: "PUBLISHED",
      last_run_at: nowIso,
      created_at: nowIso,
      updated_at: nowIso,
      issues: [],
    },
    flowRuns: [
      {
        id: MOCK_FLOW_RUN_ID,
        flow_id: MOCK_FLOW_ID,
        version: 1,
        environment: "TEST",
        trigger_source: "MANUAL",
        trigger_payload: {},
        idempotency_key: null,
        status: "WAITING_APPROVAL",
        outputs: {},
        error_message: null,
        started_by: null,
        created_at: "2026-11-24T10:00:00.000Z",
        started_at: "2026-11-24T10:00:00.000Z",
        finished_at: null,
      },
    ],
    scenarios: [],
    competencyQuestions: [],
    harnessRuns: [],
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
      await route.fulfill({ status: 400, body: "forbidden tenant_id" });
      return;
    }

    if (pathname === "/api/health") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ status: "ok", service: "axpoint-core" }),
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

    if (
      (pathname === "/api/auth/login" || pathname === "/api/auth/refresh") &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: buildMockJwt("step30"),
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
          display_name: "平台管理员",
          dept_id: "east",
          roles: ["ADMIN"],
          permissions: [
            "DATA_READ",
            "DATA_WRITE",
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
            "VIEW_READ",
            "VIEW_EDIT",
            "VIEW_PUBLISH",
            "GUARD_ADMIN",
            "OPS_ADMIN",
          ],
          token_kind: "JWT",
        }),
      });
      return;
    }

    if (pathname === "/api/auth/tokens" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        name?: string;
        token_profile?: string;
      };
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: "00000000-0000-0000-0000-000000003099",
          name: body.name ?? "axbuilder-step30",
          token: "axp_builder_step30_mock_pat",
          token_profile: body.token_profile ?? "BUILDER",
          permissions: ["DATA_READ", "STUDIO_EDIT", "FLOW_EDIT", "SIM_RUN", "VIEW_EDIT"],
          created_at: nowIso,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/tokens" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/mcp" && method === "POST") {
      const rpc = (req.postDataJSON() ?? {}) as { id?: number; method?: string };
      if (rpc.method === "initialize") {
        await route.fulfill({
          status: 200,
          headers: {
            "Content-Type": "application/json",
            "Mcp-Session-Id": "mcp-step30-mock-sid",
          },
          body: JSON.stringify({
            jsonrpc: "2.0",
            id: rpc.id ?? 1,
            result: { protocolVersion: "2025-03-26", capabilities: { tools: {} } },
          }),
        });
        return;
      }
      if (rpc.method === "tools/list") {
        const toolNames = [
          "axbuilder_preflight",
          "axbuilder_pull_workspace",
          "axbuilder_list_datasets",
          "axbuilder_inspect_dataset_profile",
          "axbuilder_sample_dataset_rows",
          "axbuilder_clone_template",
          "axbuilder_validate_schema",
          "axbuilder_push_schema_draft",
          "axbuilder_diff_schema_draft",
          "axbuilder_eval_expression",
          "axbuilder_upsert_flow_draft",
          "axbuilder_dryrun_action_spec",
          "axbuilder_run_dev_subgraph",
          "axbuilder_reset_dev_subgraph",
          "axbuilder_certify_subgraph",
          "axbuilder_compile_axql",
          "axbuilder_sim_trial_run",
          "axbuilder_upsert_chart_draft",
          "axbuilder_upsert_dashboard_draft",
          "axbuilder_preflight_dashboard",
          "axbuilder_upsert_competency_suite",
          "axbuilder_verify_competency_suite",
        ];
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            jsonrpc: "2.0",
            id: rpc.id ?? 2,
            result: {
              tools: toolNames.map((name) => ({
                name,
                description: name,
                inputSchema: { type: "object" },
              })),
            },
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ jsonrpc: "2.0", id: rpc.id ?? 0, result: {} }),
      });
      return;
    }

    // xBlueStudio & Ontology Runtime
    if (pathname === "/api/xblue/subgraphs" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000003000",
            slug: "main",
            display_name: "企业主图",
            description: "生产权威主图",
            owner_dept: "default",
            is_main: true,
            is_dev_sandbox: false,
            current_version: 2,
            created_at: nowIso,
            updated_at: nowIso,
          },
          {
            id: "00000000-0000-0000-0000-000000003005",
            slug: MOCK_CERT_SLUG,
            display_name: "履约演进沙盒",
            description: "Step30 复合建模与认证沙盒",
            owner_dept: "default",
            is_main: false,
            is_dev_sandbox: true,
            current_version: 1,
            created_at: nowIso,
            updated_at: nowIso,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/xblue/schema" && method === "GET") {
      const sg = searchParams.get("subgraph") ?? "main";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: sg,
          version: sg === "main" ? 2 : 1,
          ...compoundDoc,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/validate" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        node_types?: Array<{
          derived_rules?: Array<{ target_metric?: string }>;
        }>;
      };
      const hasOrphan = (body.node_types ?? []).some((nt) =>
        (nt.derived_rules ?? []).some((dr) => dr.target_metric === "orphan_gap")
      );
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          valid: true,
          issues: hasOrphan
            ? [
                {
                  severity: "WARN",
                  code: "DEAD_KNOWLEDGE",
                  path: "node_types[0].derived_rules[0]",
                  message: "派生指标 orphan_gap 未被下游消费",
                },
              ]
            : [],
          pass_count: 14,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblue/subgraphs/${MOCK_CERT_SLUG}/certify` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: MOCK_CERT_SLUG,
          schema_version: 1,
          cert_level: "L4_CERTIFIED",
          trial_tick_passed: true,
          dead_knowledge_cnt: 0,
          issues: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/graphql/sdl" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          version: 2,
          sdl: "type V3Hub { id: ID! nodeKey: String! inventory: Float }\ntype Mutation { v3Hub_dispatchStock(key: String!, idempotencyKey: String!, input: V3HubDispatchStockInput!): ActionMutationResult! }",
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/objects" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ version: 2, items: [] }),
      });
      return;
    }

    if (pathname === "/api/xblue/graphql" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as { query?: string };
      if ((body.query ?? "").includes("v3Hub_dispatchStock")) {
        state.hubInventory = Math.max(0, state.hubInventory - 20);
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            data: {
              v3Hub_dispatchStock: {
                status: "APPLIED",
                mutatedNodes: 1,
                idempotencyReplay: false,
              },
            },
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: {} }),
      });
      return;
    }

    if (
      (pathname === "/api/xblue/nodes" ||
        pathname === "/api/xblue/graph/subgraph") &&
      method === "GET"
    ) {
      const authHeader = req.headers()["authorization"] ?? "";
      const isRiskObserver = authHeader.includes("obs_risk_token");
      const visibleHub = !isRiskObserver || state.hubInventory < 100;
      const hubNode = {
        id: MOCK_HUB_SH_ID,
        type_name: "V3Hub",
        node_key: "W-30-SH",
        title: "上海枢纽仓",
        properties: { region: "east", domain_group: "履约域" },
        metrics: { inventory: state.hubInventory, outbound_total: 300 },
        row_version: 1,
      };
      const items = visibleHub ? [hubNode] : [];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: items,
          links: [],
          items,
          truncated: false,
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblue/nodes/${MOCK_HUB_SH_ID}` &&
      method === "PATCH"
    ) {
      const body = (req.postDataJSON() ?? {}) as {
        metrics?: { inventory?: number };
      };
      if (typeof body.metrics?.inventory === "number") {
        state.hubInventory = body.metrics.inventory;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: MOCK_HUB_SH_ID,
          node_key: "W-30-SH",
          metrics: { inventory: state.hubInventory },
        }),
      });
      return;
    }

    // Competency QA Suite endpoints (Stage 8)
    const compSuiteMatch =
      /^\/api\/xblue\/subgraphs\/([^/]+)\/competency-suite$/.exec(pathname);
    if (compSuiteMatch && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: decodeURIComponent(compSuiteMatch[1] ?? "main"),
          questions: state.competencyQuestions,
          updated_at: nowIso,
        }),
      });
      return;
    }
    if (compSuiteMatch && method === "PUT") {
      const body = (req.postDataJSON() ?? {}) as {
        questions?: Array<Record<string, unknown>>;
      };
      state.competencyQuestions = Array.isArray(body.questions)
        ? body.questions
        : [];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: decodeURIComponent(compSuiteMatch[1] ?? "main"),
          questions: state.competencyQuestions,
          updated_at: nowIso,
        }),
      });
      return;
    }

    const compVerifyMatch =
      /^\/api\/xblue\/subgraphs\/([^/]+)\/competency-verify$/.exec(pathname);
    if (compVerifyMatch && method === "POST") {
      const slug = decodeURIComponent(compVerifyMatch[1] ?? "main");
      const knownMetrics = new Set(["inventory", "outbound_total", "output", "qty", "turnover_rate"]);
      const gaps: Array<Record<string, unknown>> = [];
      let passedCount = 0;

      for (const q of state.competencyQuestions) {
        const qId = String(q.id ?? "cq_1");
        const expMetrics = Array.isArray(q.expected_metrics)
          ? (q.expected_metrics as string[])
          : [];
        const missing = expMetrics.find((m) => !knownMetrics.has(m));
        if (missing) {
          gaps.push({
            question_id: qId,
            gap_kind: "MISSING_METRIC",
            target_ref: missing,
            message: `缺失指标符号 '${missing}'`,
            suggested_tool: "axbuilder_push_schema_draft",
          });
        } else {
          passedCount += 1;
        }
      }

      const total = state.competencyQuestions.length;
      const runRecord = {
        id: `00000000-0000-0000-0000-00000000308${state.harnessRuns.length}`,
        subgraph_slug: slug,
        total_questions: total,
        passed_count: passedCount,
        status: gaps.length === 0 && passedCount === total ? "PASS" : "GAP_DETECTED",
        gap_diagnostics: gaps,
        results: state.competencyQuestions.map((q) => ({
          question_id: String(q.id ?? "cq_1"),
          question: String(q.question ?? ""),
          passed: gaps.length === 0,
          grounding_mode: "GUARDED_SQL",
          abstained: gaps.length > 0,
          answer: gaps.length === 0 ? "有效记录总数为 100 条。" : "",
          observed_numbers: gaps.length === 0 ? [100] : [],
          sql: "SELECT COUNT(*) FROM demo_orders",
          failure_reason: null,
        })),
        created_at: nowIso,
      };
      state.harnessRuns = [runRecord, ...state.harnessRuns];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(runRecord),
      });
      return;
    }

    if (/^\/api\/xblue\/subgraphs\/[^/]+\/harness-runs$/.test(pathname)) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.harnessRuns }),
      });
      return;
    }

    if (
      pathname === "/api/xblue/drafts" ||
      pathname === "/api/xblue/schema/versions" ||
      pathname === "/api/xblue/mappings"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    // AxHub & Analytics & AxQL (Stages 3 & 4)
    if (pathname === "/api/axhub/datasets" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: MOCK_DEMO_DS_ID,
              name: "demo_orders",
              display_name: "履约订单明细",
              source_kind: "SQL_TABLE",
              status: "READY",
              row_count: 12,
              columns: [
                { name: "order_id", data_type: "STRING", nullable: false },
                { name: "status", data_type: "STRING", nullable: false },
                { name: "amount", data_type: "DECIMAL", nullable: false },
              ],
              created_at: nowIso,
              updated_at: nowIso,
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/catalog" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          datasets: [
            {
              id: MOCK_DEMO_DS_ID,
              name: "demo_orders",
              display_name: "履约订单明细",
              row_count: 12,
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

    if (pathname === "/api/analytics/axql/run" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "exact_inv", data_type: "FLOAT64", pii_category: "NONE", masked: false },
            { name: "order_cnt", data_type: "INT64", pii_category: "NONE", masked: false },
          ],
          rows: [{ exact_inv: 500, order_cnt: 2 }],
          row_count: 1,
          truncated: false,
          duration_ms: 8,
          sql: "WITH __dedup_w AS (SELECT DISTINCT node_key, inventory FROM v3_hub) SELECT SUM(inventory) AS exact_inv, 2 AS order_cnt FROM __dedup_w",
          plan: {
            compiled_sql: "SELECT SUM(inventory) AS exact_inv FROM __dedup_w",
            referenced_node_types: ["V3Hub", "V3Outbound"],
            referenced_edge_types: ["V3_RECORDS_OUTBOUND"],
            referenced_datasets: [],
            dedup_entities: ["w"],
            knowledge_recall_nodes: 0,
            fanout_protected: true,
          },
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
            { name: "status", data_type: "STRING", pii_category: "NONE", masked: false },
            { name: "amount", data_type: "DECIMAL", pii_category: "NONE", masked: false },
          ],
          rows: [
            { status: "PAID", amount: 18500 },
            { status: "SHIPPED", amount: 9400 },
            { status: "PENDING", amount: 3200 },
          ],
          row_count: 3,
          truncated: false,
          duration_ms: 9,
          sql: "SELECT status, SUM(amount) AS amount FROM demo_orders GROUP BY status",
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/pivot" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          row_headers: [["PAID"], ["SHIPPED"], ["PENDING"], ["总计"]],
          column_headers: [["SUM(amount)"]],
          cells: [["18500.00"], ["9400.00"], ["3200.00"], ["31100.00"]],
          row_kinds: ["DATA", "DATA", "DATA", "GRAND_TOTAL"],
          columns_truncated: false,
          duration_ms: 7,
          sql: "SELECT status, SUM(amount) FROM demo_orders GROUP BY ROLLUP(status)",
        }),
      });
      return;
    }

    if (
      pathname === "/api/analytics/metrics" ||
      pathname === "/api/analytics/saved-queries"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/analytics/history") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    // AxFlow (Stage 5)
    if (pathname === "/api/axflow/flows" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [state.flowDoc], next_cursor: null }),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${MOCK_FLOW_ID}` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.flowDoc),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${MOCK_FLOW_ID}` && method === "PUT") {
      const body = (req.postDataJSON() ?? {}) as Record<string, unknown>;
      state.flowDoc = { ...state.flowDoc, ...body };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.flowDoc),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${MOCK_FLOW_ID}/runs` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.flowRuns, next_cursor: null }),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${MOCK_FLOW_ID}/runs` && method === "POST") {
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(state.flowRuns[0]),
      });
      return;
    }

    if (pathname === `/api/axflow/runs/${MOCK_FLOW_RUN_ID}` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.flowRuns[0]),
      });
      return;
    }

    if (pathname === `/api/axflow/runs/${MOCK_FLOW_RUN_ID}/steps` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: 1,
              run_id: MOCK_FLOW_RUN_ID,
              node_id: "sync_step",
              attempt: 1,
              status: "SUCCEEDED",
              job_id: null,
              input: {},
              output: { rows_written: 12 },
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-11-24T10:00:00.000Z",
              finished_at: "2026-11-24T10:00:00.135Z",
            },
            {
              id: 2,
              run_id: MOCK_FLOW_RUN_ID,
              node_id: "human_approval_1",
              attempt: 1,
              status: "RUNNING",
              job_id: null,
              input: {},
              output: {},
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-11-24T10:00:00.140Z",
              finished_at: null,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${MOCK_FLOW_ID}/versions` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], deployments: [], next_cursor: null }),
      });
      return;
    }

    // AxSim 2.0 (Stage 6)
    if (pathname === "/api/axsim/sandboxes" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: MOCK_SANDBOX_ID,
            name: "华东断供与红线沙盘",
            status: "READY",
            scope: { depth: 3, direction: "BOTH", type_names: ["V3Plant", "V3Hub"] },
            node_count: 3,
            link_count: 2,
            base_captured_at: nowIso,
            schema_version: 2,
            sim_result: null,
            created_at: nowIso,
          },
        ]),
      });
      return;
    }

    if (pathname === `/api/axsim/sandboxes/${MOCK_SANDBOX_ID}/graph` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sandbox_id: MOCK_SANDBOX_ID,
          overlay_hash: "v3-ov-hash",
          nodes: [
            {
              id: MOCK_PLANT_ID,
              type_name: "V3Plant",
              node_key: "P-30-1",
              title: "昆山总厂",
              metrics: { output: 300 },
              properties: { domain_group: "制造域" },
              is_modified: false,
              is_new: false,
            },
            {
              id: MOCK_HUB_SH_ID,
              type_name: "V3Hub",
              node_key: "W-30-SH",
              title: "上海枢纽仓",
              metrics: { inventory: 300 },
              properties: { region: "east", domain_group: "履约域" },
              is_modified: false,
              is_new: false,
            },
            {
              id: MOCK_HUB_SUZ_ID,
              type_name: "V3Hub",
              node_key: "W-30-SUZ",
              title: "苏州前置仓",
              metrics: { inventory: 260 },
              properties: { region: "east", domain_group: "履约域" },
              is_modified: false,
              is_new: false,
            },
          ],
          links: [
            {
              id: "v3-sim-lk-1",
              link_type: "V3_SUPPLIES",
              src_node_id: MOCK_PLANT_ID,
              dst_node_id: MOCK_HUB_SH_ID,
              weight: 1.0,
              is_modified: false,
              is_new: false,
            },
            {
              id: "v3-sim-lk-2",
              link_type: "V3_SUPPLIES",
              src_node_id: MOCK_PLANT_ID,
              dst_node_id: MOCK_HUB_SUZ_ID,
              weight: 1.0,
              is_modified: false,
              is_new: false,
            },
          ],
        }),
      });
      return;
    }

    const simImpacts = [
      {
        node_id: MOCK_PLANT_ID,
        type_name: "V3Plant",
        title: "昆山总厂",
        metric: "output",
        before_val: 300,
        after_val: 150,
        delta: -150,
        delta_ratio: -0.5,
        p10: 140,
        p90: 160,
        hop_distance: 0,
        top_path: [MOCK_PLANT_ID],
      },
      {
        node_id: MOCK_HUB_SH_ID,
        type_name: "V3Hub",
        title: "上海枢纽仓",
        metric: "inventory",
        before_val: 300,
        after_val: 230,
        delta: -70,
        delta_ratio: -0.2333,
        p10: 215,
        p90: 245,
        hop_distance: 1,
        top_path: [MOCK_PLANT_ID, MOCK_HUB_SH_ID],
      },
      {
        node_id: MOCK_HUB_SUZ_ID,
        type_name: "V3Hub",
        title: "苏州前置仓",
        metric: "inventory",
        before_val: 260,
        after_val: 130,
        delta: -130,
        delta_ratio: -0.5,
        p10: 120,
        p90: 140,
        hop_distance: 1,
        top_path: [MOCK_PLANT_ID, MOCK_HUB_SUZ_ID],
      },
    ];

    if (
      pathname === `/api/axsim/sandboxes/${MOCK_SANDBOX_ID}/scenarios` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.scenarios),
      });
      return;
    }

    if (
      pathname === `/api/axsim/sandboxes/${MOCK_SANDBOX_ID}/scenarios` &&
      method === "POST"
    ) {
      const body = (req.postDataJSON() ?? {}) as { name?: string };
      const created = {
        id: MOCK_SCENARIO_ID,
        sandbox_id: MOCK_SANDBOX_ID,
        name: body.name ?? "step30_event_deadline",
        actions: [],
        params: { mode: "STEPPED", horizon_steps: 12, damping: 1.0 },
        overlay_hash: "v3-ov-hash",
        is_stale: false,
        status: "SUCCEEDED",
        created_at: nowIso,
        finished_at: nowIso,
        fired_events: [
          {
            event_id: "ev_1",
            event_name: "第4步突发减产",
            step: 4,
            trigger_kind: "AT_STEP",
            trigger_value: null,
          },
          {
            event_id: "ev_2",
            event_name: "低库存应急补货",
            step: 8,
            trigger_kind: "METRIC_THRESHOLD",
            trigger_value: 150,
          },
        ],
        deadline_report: {
          status: "ACTIONABLE_WINDOW",
          earliest_breach_step: 7,
          earliest_deadline_step: 3,
          items: [
            {
              constraint_id: "goal_1",
              constraint_name: "长三角安全库存红线",
              breached: true,
              breach_node_id: MOCK_HUB_SH_ID,
              breach_step: 7,
              breach_value: 150,
              min_lag_steps: 3,
              deadline_step: 3,
            },
          ],
        },
        result: {
          affected_count: 3,
          converged: true,
          iterations: 12,
          warnings: [],
          kpis: [],
          top_impacts: simImpacts,
        },
      };
      state.scenarios = [created];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    if (
      pathname === `/api/axsim/scenarios/${MOCK_SCENARIO_ID}/impacts` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: simImpacts, next_cursor: null }),
      });
      return;
    }

    if (
      pathname === `/api/axsim/scenarios/${MOCK_SCENARIO_ID}/checkpoints` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            { seq: 0, label: "Step 0", changes: { [MOCK_PLANT_ID]: { output: 300 } } },
            { seq: 4, label: "Step 4", changes: { [MOCK_PLANT_ID]: { output: 150 } } },
            {
              seq: 7,
              label: "Step 7",
              changes: {
                [MOCK_PLANT_ID]: { output: 150 },
                [MOCK_HUB_SH_ID]: { inventory: 150 },
                [MOCK_HUB_SUZ_ID]: { inventory: 130 },
              },
            },
          ],
        }),
      });
      return;
    }

    // xBlueView Charts & Cockpit (Stages 3 & 7)
    if (pathname === "/api/xblueview/widgets/preview" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "status", data_type: "STRING" },
            { name: "amount", data_type: "DECIMAL" },
          ],
          rows: [
            ["PAID", 18500],
            ["SHIPPED", 9400],
            ["PENDING", 3200],
          ],
          row_count: 3,
          truncated: false,
          generated_at: nowIso,
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/charts" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.savedCharts, next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/xblueview/charts" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as Record<string, unknown>;
      const created = {
        id: MOCK_CHART_ID,
        tenant_id: "default",
        name: body.name ?? "履约订单组合图",
        description: "",
        widget_kind: body.widget_kind ?? "BAR_CHART",
        data_source: body.data_source ?? { kind: "SQL", sql: "SELECT 1" },
        encoding: body.encoding ?? {},
        options: body.options ?? {},
        created_by: OPS_ADMIN_ID,
        created_at: nowIso,
        updated_at: nowIso,
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
    if (pinMatch && method === "POST") {
      const chartId = pinMatch[1] ?? MOCK_CHART_ID;
      const body = (req.postDataJSON() ?? {}) as { target_zone?: string };
      const zone = body.target_zone ?? "INSIGHT";
      const pinnedWidget = {
        id: MOCK_PIN_WIDGET_ID,
        tenant_id: "default",
        dashboard_id: MOCK_DASHBOARD_ID,
        widget_kind: "BAR_CHART",
        title: String(state.savedCharts[0]?.name ?? "履约订单组合图"),
        position: { x: 0, y: 11, w: 6, h: 4 },
        data_source: {
          kind: "SQL",
          sql: "SELECT status, amount FROM demo_orders",
        },
        encoding: { x: "status", y: ["amount"] },
        options: { source_chart_id: chartId, semantic_zone: zone },
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
        created_at: nowIso,
        updated_at: nowIso,
      };
      state.pinnedWidgets = [pinnedWidget];
      state.widgetPositions[MOCK_PIN_WIDGET_ID] = pinnedWidget.position;
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          widget_id: MOCK_PIN_WIDGET_ID,
          semantic_zone: zone,
          dashboard_id: MOCK_DASHBOARD_ID,
          dashboard_slug: cockpitSlug,
          deep_link: `/view/${cockpitSlug}`,
          widget: pinnedWidget,
        }),
      });
      return;
    }

    const buildCockpitRecord = () => ({
      id: MOCK_DASHBOARD_ID,
      tenant_id: "default",
      slug: cockpitSlug,
      name: "供应链五区驾驶舱",
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
      created_by: OPS_ADMIN_ID,
      created_at: nowIso,
      updated_at: nowIso,
    });

    if (pathname === "/api/xblueview/dashboards" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [buildCockpitRecord()],
          next_cursor: null,
        }),
      });
      return;
    }

    const buildCockpitWidgets = () => [
      {
        id: MOCK_WIDGET_KPI_ID,
        tenant_id: "default",
        dashboard_id: MOCK_DASHBOARD_ID,
        widget_kind: "KPI_CARD",
        title: "枢纽实时库存",
        position: state.widgetPositions[MOCK_WIDGET_KPI_ID],
        data_source: {
          kind: "NODE_METRIC",
          type_name: "V3Hub",
          metric: "inventory",
          agg: "LAST",
          grain: "DAY",
          group_by: "time",
        },
        encoding: { value: "value", x: "bucket" },
        options: { unit: "件", decimals: 0, target_value: 1000 },
        drill: [],
        refresh_mode: "MANUAL",
        refresh_seconds: 60,
        deps: {
          dataset_ids: [],
          type_names: ["V3Hub"],
          metrics: ["inventory"],
          alerts: false,
          scenario_ids: [],
        },
        created_at: nowIso,
        updated_at: nowIso,
      },
      {
        id: MOCK_WIDGET_BAR_ID,
        tenant_id: "default",
        dashboard_id: MOCK_DASHBOARD_ID,
        widget_kind: "BAR_CHART",
        title: "订单履约对比",
        position: state.widgetPositions[MOCK_WIDGET_BAR_ID],
        data_source: {
          kind: "SQL",
          sql: "SELECT status, amount FROM demo_orders",
        },
        encoding: { x: "status", y: ["amount"] },
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
        created_at: nowIso,
        updated_at: nowIso,
      },
      {
        id: MOCK_WIDGET_ACT_ID,
        tenant_id: "default",
        dashboard_id: MOCK_DASHBOARD_ID,
        widget_kind: "ACTION_FORM",
        title: "库存紧急回写",
        position: state.widgetPositions[MOCK_WIDGET_ACT_ID],
        data_source: {
          kind: "ACTION",
          action_type: "WRITE_NODE",
          node_id: MOCK_HUB_SH_ID,
          target_kind: "METRIC",
          key: "inventory",
          input_schema: {
            type: "object",
            required: ["value"],
            properties: {
              value: { type: "number", title: "目标库存" },
            },
          },
        },
        encoding: {},
        options: { button_label: "提交回写", skip_confirm: true },
        drill: [],
        refresh_mode: "MANUAL",
        refresh_seconds: 60,
        deps: {
          dataset_ids: [],
          type_names: ["V3Hub"],
          metrics: ["inventory"],
          alerts: false,
          scenario_ids: [],
        },
        created_at: nowIso,
        updated_at: nowIso,
      },
      ...state.pinnedWidgets.map((pw) => ({
        ...pw,
        position:
          state.widgetPositions[String(pw.id)] ??
          (pw.position as { x: number; y: number; w: number; h: number }),
      })),
    ];

    if (
      (pathname === `/api/xblueview/dashboards/by-slug/${cockpitSlug}` ||
        pathname === `/api/xblueview/dashboards/${MOCK_DASHBOARD_ID}`) &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ...buildCockpitRecord(),
          widgets: buildCockpitWidgets(),
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${MOCK_DASHBOARD_ID}` &&
      method === "PUT"
    ) {
      const body = (req.postDataJSON() ?? {}) as {
        canvas?: { compact_mode?: string };
      };
      if (body.canvas?.compact_mode === "FREE_FLOAT") {
        state.compactMode = "FREE_FLOAT";
      } else if (body.canvas?.compact_mode === "VERTICAL_COMPACT") {
        state.compactMode = "VERTICAL_COMPACT";
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(buildCockpitRecord()),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${MOCK_DASHBOARD_ID}/layout` &&
      method === "PUT"
    ) {
      const body = (req.postDataJSON() ?? {}) as {
        widgets?: Array<{
          id: string;
          position: { x: number; y: number; w: number; h: number };
        }>;
        items?: Array<{
          id: string;
          position: { x: number; y: number; w: number; h: number };
        }>;
      };
      const list = body.widgets ?? body.items ?? [];
      for (const item of list) {
        state.widgetPositions[item.id] = item.position;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(buildCockpitWidgets()),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${MOCK_WIDGET_KPI_ID}/data` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "bucket", data_type: "TIMESTAMP" },
            { name: "value", data_type: "FLOAT64" },
          ],
          rows: state.kpiSeries,
          row_count: state.kpiSeries.length,
          truncated: false,
          generated_at: nowIso,
          data_version: `v-${state.kpiSeries.length}`,
          drill_level: 0,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${MOCK_WIDGET_ACT_ID}/invoke` &&
      method === "POST"
    ) {
      const body = (req.postDataJSON() ?? {}) as {
        inputs?: { value?: number };
      };
      const written = Number(body.inputs?.value ?? 680);
      state.hubInventory = written;
      state.kpiSeries = [...state.kpiSeries, ["2026-11-24T10:05:00Z", written]];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mode: "WRITE_NODE",
          run_id: null,
          status: "DONE",
          node_id: MOCK_HUB_SH_ID,
          action_log_id: 1,
          idempotency_replay: false,
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
            ["PAID", 18500],
            ["SHIPPED", 9400],
          ],
          row_count: 2,
          truncated: false,
          generated_at: nowIso,
          data_version: "v1",
          drill_level: 0,
        }),
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

    if (pathname === "/api/stats") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          doc_count: 3,
          para_count: 18,
          entity_count: 12,
          edge_count: 20,
          embedding_count: 18,
          embedding_dims: [1024],
          timestamp: nowIso,
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
          parser: { ok: true, version: "0.4.0" },
          lake: { ok: true, snapshot_id: 1 },
          llm: { ok: true, provider: "cassette" },
          embed: { ok: true },
          disk: { ok: true, data_root_free_bytes: 50_000_000_000 },
          jobs: { pending: 0, running: 0, failed_24h: 0, oldest_pending_s: 0 },
          outbox: { lag_s: 0, pending: 0 },
          migrations: { ok: true, current: 24, pending: 0 },
          version: "0.4.0",
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

  return state;
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

test.describe("Step 30 Phase 3 Craft UX & World Runtime Full Journey", () => {
  test("completes all 8 stages: IA shell & cmdk, compound studio & L4 cert, AxQL & 1-step chart pin, VTable & state visibility, flow splice, AxSim 2.0 deadline & G6 hull, cockpit magnetic grid & writeback, and 22-tool Competency QA", async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const isLive = process.env.AXPOINT_E2E_LIVE === "1";
    const coreBaseUrl = (
      process.env.CORE_BASE_URL ?? "http://127.0.0.1:9995"
    ).replace(/\/$/, "");
    const runSuffix = String(Date.now()).slice(-6);
    const cockpitSlug = isLive ? `v3-cockpit-${runSuffix}` : "v3-cockpit-mock";

    const leakedTenantUrls: string[] = [];
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err.message);
    });

    if (!isLive) {
      await installV3Mocks(page, leakedTenantUrls, cockpitSlug);
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
    }

    const cleanup = {
      adminToken: "",
      subgraphs: [] as string[],
      nodeIds: [] as string[],
      flowIds: [] as string[],
      sandboxIds: [] as string[],
      dashboardIds: [] as string[],
      chartIds: [] as string[],
      rowPolicyIds: [] as string[],
      userIds: [] as string[],
      tokenIds: [] as string[],
      resetDefaultDenyRef: false,
    };

    let opsPassword = "AxPointDemo#2026";
    let certSubgraphSlug = MOCK_CERT_SLUG;
    let hubKey = "W-30-SH";
    let hubNodeId = MOCK_HUB_SH_ID;
    let plantNodeId = MOCK_PLANT_ID;
    let suzNodeId = MOCK_HUB_SUZ_ID;
    let flowId = MOCK_FLOW_ID;
    let simSandboxId = MOCK_SANDBOX_ID;
    let dashboardId = MOCK_DASHBOARD_ID;
    let widgetKpiId = MOCK_WIDGET_KPI_ID;
    let widgetBarId = MOCK_WIDGET_BAR_ID;
    let widgetActId = MOCK_WIDGET_ACT_ID;
    let observerToken = "obs_risk_token";
    let demoOrdersRowCount = 100;

    if (isLive) {
      const auth = await resolveLiveAdminSession(page.request, coreBaseUrl);
      opsPassword = auth.password;
      cleanup.adminToken = auth.accessToken;
      const headers = {
        Authorization: `Bearer ${cleanup.adminToken}`,
        "Content-Type": "application/json",
      };

      // Inspect demo_orders exact SQL row count for deterministic Competency QA verification
      const sqlCntResp = await page.request.post(
        `${coreBaseUrl}/api/analytics/sql/run`,
        {
          headers,
          data: { sql: "SELECT COUNT(*) AS cnt FROM demo_orders", max_rows: 5 },
        }
      );
      if (sqlCntResp.ok()) {
        const sqlBody = (await sqlCntResp.json()) as {
          rows?: Array<unknown[] | Record<string, unknown>>;
        };
        const firstRow = sqlBody.rows?.[0];
        const rawCnt = Array.isArray(firstRow)
          ? firstRow[0]
          : (firstRow as { cnt?: unknown } | undefined)?.cnt;
        const parsedCnt = Number(rawCnt);
        if (Number.isFinite(parsedCnt) && parsedCnt > 0) {
          demoOrdersRowCount = parsedCnt;
        }
      }

      // 1. Create isolated dev sandbox subgraph for Stage 2 & Stage 8
      certSubgraphSlug = `v3_cert_${runSuffix}`;
      const createSgResp = await page.request.post(
        `${coreBaseUrl}/api/xblue/subgraphs`,
        {
          headers,
          data: {
            slug: certSubgraphSlug,
            display_name: "履约演进沙盒",
            is_dev_sandbox: true,
          },
        }
      );
      expect(createSgResp.ok()).toBe(true);
      cleanup.subgraphs.push(certSubgraphSlug);

      const compoundDoc = buildV3CompoundSchemaDoc();
      const pubCertSchemaResp = await page.request.post(
        `${coreBaseUrl}/api/xblue/schema?subgraph=${certSubgraphSlug}`,
        { headers, data: compoundDoc }
      );
      expect(
        pubCertSchemaResp.ok(),
        await pubCertSchemaResp.text()
      ).toBe(true);

      // 2. Ensure V3Plant / V3Hub / V3Outbound & causal edges exist on `main`
      const curMainResp = await page.request.get(
        `${coreBaseUrl}/api/xblue/schema?subgraph=main`,
        { headers }
      );
      expect(curMainResp.ok()).toBe(true);
      const curMain = (await curMainResp.json()) as {
        node_types?: Array<{ name: string }>;
        edge_types?: Array<{ name: string }>;
        mappings?: unknown[];
      };
      const v3TypeNames = new Set(compoundDoc.node_types.map((t) => t.name));
      const v3EdgeNames = new Set(compoundDoc.edge_types.map((e) => e.name));
      const mergedMainDoc = {
        node_types: [
          ...(curMain.node_types ?? []).filter((t) => !v3TypeNames.has(t.name)),
          ...compoundDoc.node_types,
        ],
        edge_types: [
          ...(curMain.edge_types ?? []).filter((e) => !v3EdgeNames.has(e.name)),
          ...compoundDoc.edge_types,
        ],
        mappings: curMain.mappings ?? [],
        force: true,
      };
      const pubMainResp = await page.request.post(
        `${coreBaseUrl}/api/xblue/schema?subgraph=main`,
        { headers, data: mergedMainDoc }
      );
      expect(pubMainResp.ok()).toBe(true);

      // Clean any leftover V3* test nodes from prior interrupted runs so type-wide rollups stay isolated
      for (const staleType of ["V3Outbound", "V3Hub", "V3Plant"]) {
        const listStale = await page.request.get(
          `${coreBaseUrl}/api/xblue/nodes?type_name=${staleType}&limit=100`,
          { headers }
        );
        if (listStale.ok()) {
          const staleBody = (await listStale.json()) as {
            items?: Array<{ id: string; node_key: string }>;
          };
          for (const item of staleBody.items ?? []) {
            if (item.node_key.includes("-30-")) {
              await page.request
                .delete(`${coreBaseUrl}/api/xblue/nodes/${item.id}`, { headers })
                .catch(() => null);
            }
          }
        }
      }

      // 3. Seed runtime nodes & links on `main` for AxQL, State Visibility, AxSim 2.0 & Action Writeback
      hubKey = `W-30-${runSuffix}`;
      const plantResp = await page.request.post(
        `${coreBaseUrl}/api/xblue/nodes`,
        {
          headers,
          data: {
            type_name: "V3Plant",
            node_key: `P-30-${runSuffix}`,
            title: "昆山总厂",
            properties: { domain_group: "制造域" },
            metrics: { output: 300 },
            geo: { lng: 121.2, lat: 31.15 },
          },
        }
      );
      expect(plantResp.ok()).toBe(true);
      plantNodeId = ((await plantResp.json()) as { id: string }).id;
      cleanup.nodeIds.push(plantNodeId);

      const hubResp = await page.request.post(
        `${coreBaseUrl}/api/xblue/nodes`,
        {
          headers,
          data: {
            type_name: "V3Hub",
            node_key: hubKey,
            title: "上海枢纽仓",
            properties: { region: "east", domain_group: "履约域" },
            metrics: { inventory: 500, outbound_total: 300 },
            geo: { lng: 121.4737, lat: 31.2304 },
          },
        }
      );
      expect(hubResp.ok()).toBe(true);
      hubNodeId = ((await hubResp.json()) as { id: string }).id;
      cleanup.nodeIds.push(hubNodeId);

      const suzResp = await page.request.post(
        `${coreBaseUrl}/api/xblue/nodes`,
        {
          headers,
          data: {
            type_name: "V3Hub",
            node_key: `SUZ-30-${runSuffix}`,
            title: "苏州前置仓",
            properties: { region: "east", domain_group: "履约域" },
            metrics: { inventory: 260, outbound_total: 100 },
            geo: { lng: 120.5853, lat: 31.2989 },
          },
        }
      );
      expect(suzResp.ok()).toBe(true);
      suzNodeId = ((await suzResp.json()) as { id: string }).id;
      cleanup.nodeIds.push(suzNodeId);

      // Seed 2 outbound child events linked to hubKey for SUM_BY_ENTITY anti-fanout test
      for (const idx of [1, 2]) {
        const outResp = await page.request.post(
          `${coreBaseUrl}/api/xblue/nodes`,
          {
            headers,
            data: {
              type_name: "V3Outbound",
              node_key: `OUT-30-${idx}-${runSuffix}`,
              title: `出库单-${idx}`,
              properties: { domain_group: "履约域" },
              metrics: { qty: idx * 100 },
            },
          }
        );
        expect(outResp.ok()).toBe(true);
        const outId = ((await outResp.json()) as { id: string }).id;
        cleanup.nodeIds.push(outId);
        await page.request.post(`${coreBaseUrl}/api/xblue/links`, {
          headers,
          data: {
            link_type: "V3_RECORDS_OUTBOUND",
            src_node_id: hubNodeId,
            dst_node_id: outId,
            weight: 0.85,
          },
        });
      }

      await page.request.post(`${coreBaseUrl}/api/xblue/links`, {
        headers,
        data: {
          link_type: "V3_SUPPLIES",
          src_node_id: plantNodeId,
          dst_node_id: hubNodeId,
          weight: 1.0,
        },
      });
      await page.request.post(`${coreBaseUrl}/api/xblue/links`, {
        headers,
        data: {
          link_type: "V3_SUPPLIES",
          src_node_id: plantNodeId,
          dst_node_id: suzNodeId,
          weight: 1.0,
        },
      });

      // Seed 3 historical series points (-18m, -12m, -6m) on hubNodeId and wait for rollup job
      const nowMs = Date.now();
      const seedMwRes = await page.request.post(
        `${coreBaseUrl}/api/xblue/metrics/write`,
        {
          headers,
          data: {
            points: [
              {
                node_id: hubNodeId,
                metric: "inventory",
                ts: new Date(nowMs - 18 * 60_000).toISOString(),
                value: 460,
              },
              {
                node_id: hubNodeId,
                metric: "inventory",
                ts: new Date(nowMs - 12 * 60_000).toISOString(),
                value: 480,
              },
              {
                node_id: hubNodeId,
                metric: "inventory",
                ts: new Date(nowMs - 6 * 60_000).toISOString(),
                value: 500,
              },
            ],
            source_tag: "MANUAL",
          },
        }
      );
      if (seedMwRes.ok()) {
        const seedMwBody = (await seedMwRes.json()) as { rollup_job_id?: string };
        if (seedMwBody.rollup_job_id) {
          for (let attempt = 0; attempt < 25; attempt += 1) {
            const jr = await page.request.get(
              `${coreBaseUrl}/api/axops/jobs/${seedMwBody.rollup_job_id}`,
              { headers }
            );
            if (jr.ok()) {
              const jb = (await jr.json()) as { status?: string };
              if (jb.status === "SUCCEEDED") break;
            }
            await page.waitForTimeout(200);
          }
        }
      }

      // 4. Create dedicated AxFlow workflow for Stage 5
      const flowCreateResp = await page.request.post(
        `${coreBaseUrl}/api/axflow/flows`,
        {
          headers,
          data: {
            name: `v3_guard_flow_${runSuffix}`,
            slug: `v3-guard-${runSuffix}`,
            description: "Step30 中点插算子与流光验证",
            nodes: [
              {
                id: "sync_step",
                kind: "SQL_TRANSFORM",
                label: "库存快照提取",
                config: { sql: "SELECT 1 AS n", max_rows: 10 },
                retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" },
                timeout_s: 60,
                on_failure: "FAIL_RUN",
                join: "ALL_SUCCESS",
                pos: { x: 96, y: 160 },
              },
              {
                id: "eval_step",
                kind: "SQL_TRANSFORM",
                label: "安全红线核验",
                config: { sql: "SELECT 2 AS m", max_rows: 10 },
                retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" },
                timeout_s: 60,
                on_failure: "FAIL_RUN",
                join: "ALL_SUCCESS",
                pos: { x: 460, y: 160 },
              },
            ],
            edges: [{ from: "sync_step", to: "eval_step", when: "n > 0" }],
            trigger_cfg: { type: "MANUAL" },
          },
        }
      );
      expect(flowCreateResp.ok()).toBe(true);
      flowId = ((await flowCreateResp.json()) as { id: string }).id;
      cleanup.flowIds.push(flowId);

      // 5. Create dedicated AxSim sandbox for Stage 6 (with initial inventory = 300 on hub clone)
      await page.request.patch(`${coreBaseUrl}/api/xblue/nodes/${hubNodeId}`, {
        headers,
        data: { metrics: { inventory: 300 }, metrics_merge: "MERGE" },
      });
      const sbxResp = await page.request.post(
        `${coreBaseUrl}/api/axsim/sandboxes`,
        {
          headers,
          data: {
            name: `v3_sim_sbx_${runSuffix}`,
            scope: { type_names: ["V3Plant", "V3Hub"] },
          },
        }
      );
      expect(sbxResp.ok()).toBe(true);
      simSandboxId = ((await sbxResp.json()) as { id: string }).id;
      cleanup.sandboxIds.push(simSandboxId);
      // Restore live hub inventory to 500 for Stages 2/3/4
      await page.request.patch(`${coreBaseUrl}/api/xblue/nodes/${hubNodeId}`, {
        headers,
        data: { metrics: { inventory: 500 }, metrics_merge: "MERGE" },
      });

      // 6. Create dedicated Five-Zone Cockpit + KPI, Bar & Action Writeback widgets for Stages 3 & 7
      const dashResp = await page.request.post(
        `${coreBaseUrl}/api/xblueview/dashboards`,
        {
          headers,
          data: {
            name: "供应链五区驾驶舱",
            slug: cockpitSlug,
            theme: "light",
            zone_layout: "STANDARD_FIVE_ZONE",
          },
        }
      );
      expect(dashResp.ok()).toBe(true);
      const dashJson = (await dashResp.json()) as {
        id?: string;
        dashboard?: { id?: string };
      };
      dashboardId = dashJson.dashboard?.id ?? dashJson.id ?? "";
      expect(dashboardId.length).toBeGreaterThan(0);
      cleanup.dashboardIds.push(dashboardId);

      const kpiWResp = await page.request.post(
        `${coreBaseUrl}/api/xblueview/dashboards/${dashboardId}/widgets`,
        {
          headers,
          data: {
            widget_kind: "KPI_CARD",
            title: "枢纽实时库存",
            position: { x: 0, y: 0, w: 5, h: 3 },
            data_source: {
              kind: "NODE_METRIC",
              type_name: "V3Hub",
              metric: "inventory",
              agg: "LAST",
              grain: "MINUTE",
              time_range: "LAST_24_HOURS",
              group_by: "time",
            },
            encoding: { value: "value", x: "bucket" },
            options: { unit: "件", decimals: 0, target_value: 1000 },
            refresh_mode: "LIVE",
          },
        }
      );
      expect(kpiWResp.ok()).toBe(true);
      widgetKpiId = ((await kpiWResp.json()) as { id: string }).id;

      const barWResp = await page.request.post(
        `${coreBaseUrl}/api/xblueview/dashboards/${dashboardId}/widgets`,
        {
          headers,
          data: {
            widget_kind: "BAR_CHART",
            title: "订单履约对比",
            position: { x: 5, y: 1, w: 6, h: 4 },
            data_source: {
              kind: "SQL",
              sql: "SELECT status, SUM(amount) AS amount FROM demo_orders GROUP BY status",
            },
            encoding: { x: "status", y: ["amount"] },
            options: {},
          },
        }
      );
      expect(barWResp.ok()).toBe(true);
      widgetBarId = ((await barWResp.json()) as { id: string }).id;

      const actWResp = await page.request.post(
        `${coreBaseUrl}/api/xblueview/dashboards/${dashboardId}/widgets`,
        {
          headers,
          data: {
            widget_kind: "ACTION_FORM",
            title: "库存紧急回写",
            position: { x: 0, y: 6, w: 6, h: 4 },
            data_source: {
              kind: "ACTION",
              action_type: "WRITE_NODE",
              node_id: hubNodeId,
              target_kind: "METRIC",
              key: "inventory",
              input_schema: {
                type: "object",
                required: ["value"],
                properties: {
                  value: { type: "number", title: "目标库存" },
                },
              },
            },
            encoding: {},
            options: { button_label: "提交回写", skip_confirm: true },
          },
        }
      );
      expect(actWResp.ok()).toBe(true);
      widgetActId = ((await actWResp.json()) as { id: string }).id;

      // 7. Create cross-department risk observer user + STATE_DRIVEN row policy for Stage 4
      await page.request.put(`${coreBaseUrl}/api/axguard/departments/risk_obs`, {
        headers,
        data: { name: "风控观察组" },
      });
      const obsUsername = `obs30_${runSuffix}`;
      const obsPassword = "Step30Obs!9876";
      const obsCreateResp = await page.request.post(
        `${coreBaseUrl}/api/axguard/users`,
        {
          headers,
          data: {
            username: obsUsername,
            display_name: "跨部风控员",
            password: obsPassword,
            dept_id: "risk_obs",
            clearance: "INTERNAL",
            roles: ["ANALYST"],
          },
        }
      );
      expect(obsCreateResp.ok()).toBe(true);
      cleanup.userIds.push(((await obsCreateResp.json()) as { id: string }).id);

      const obsLoginResp = await page.request.post(
        `${coreBaseUrl}/api/auth/login`,
        {
          data: { tenant: "default", username: obsUsername, password: obsPassword },
        }
      );
      expect(obsLoginResp.ok()).toBe(true);
      observerToken = (
        (await obsLoginResp.json()) as { access_token: string }
      ).access_token;

      await page.request.put(`${coreBaseUrl}/api/axguard/row-policy-defaults`, {
        headers,
        data: {
          target_kind: "NODE_TYPE",
          target_ref: "V3Hub",
          default_deny: true,
        },
      });
      cleanup.resetDefaultDenyRef = true;

      const rpResp = await page.request.post(
        `${coreBaseUrl}/api/axguard/row-policies`,
        {
          headers,
          data: {
            name: `v3_state_vis_${runSuffix}`,
            target_kind: "NODE_TYPE",
            target_ref: "V3Hub",
            applies_to: { depts: ["risk_obs"] },
            predicate:
              "properties.region == {{user.dept}} OR metrics.inventory < 100",
            policy_mode: "STATE_DRIVEN",
            enabled: true,
          },
        }
      );
      expect(rpResp.ok()).toBe(true);
      cleanup.rowPolicyIds.push(((await rpResp.json()) as { id: string }).id);
    }

    page.on("response", async (resp) => {
      if (
        isLive &&
        resp.url().endsWith("/api/xblueview/charts") &&
        resp.request().method() === "POST" &&
        resp.status() === 201
      ) {
        try {
          const j = (await resp.json()) as { id?: string };
          if (j.id) cleanup.chartIds.push(j.id);
        } catch {
          // ignore
        }
      }
      if (
        isLive &&
        resp.url().endsWith("/api/auth/tokens") &&
        resp.request().method() === "POST" &&
        resp.ok()
      ) {
        try {
          const j = (await resp.json()) as { id?: string };
          if (j.id) cleanup.tokenIds.push(j.id);
        } catch {
          // ignore
        }
      }
    });

    try {
      await page.setViewportSize({ width: 1440, height: 900 });

      // =======================================================================
      // Stage 1: 8-Domain Shell, ⌘K Jump, Monolingual Purity & Resizable Pane
      // =======================================================================
      console.log("[v3-e2e] Stage 1: 8-domain shell, ⌘K & resizable split pane");
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
      await expect(page).toHaveURL(/\/$/);

      const primaryNav = page.getByTestId("primary-domain-nav");
      await expect(primaryNav).toBeVisible();
      const primaryLinks = primaryNav.locator("a");
      await expect(primaryLinks).toHaveCount(8);
      const navLabels = (await primaryLinks.allInnerTexts()).map((s) =>
        s.trim()
      );
      expect(navLabels).toEqual([
        "总控看板",
        "数据与知识",
        "建模工作室",
        "查询与制图",
        "推演沙盘",
        "编排与告警",
        "数字看板",
        "智能体与接口",
      ]);
      await expect(page.getByTestId("sidebar-nav-settings")).toHaveText(
        "系统设置"
      );

      const sidebarText = await page.getByTestId("app-sidebar").innerText();
      expect(sidebarText).not.toMatch(CODENAME_PREFIX_RE);
      expect(sidebarText).not.toMatch(
        /\b(AxHub|AxIngest|AxAnalytics|AxQuery|xBlueStudio|xBlueGraph|AxFlow|AxAlerts|AxAgent|AxSim|xBlueView|AxGuard|AxOps)\b/
      );

      // Legacy redirects
      await page.goto("/ingest");
      await expect(page).toHaveURL(/\/hub\?tab=knowledge/);
      await expect(
        page.getByRole("heading", { level: 1, name: "数据与知识" })
      ).toBeVisible();

      await page.goto("/graph");
      await expect(page).toHaveURL(/\/studio\?tab=explorer/);
      await expect(
        page.getByRole("heading", { level: 1, name: "建模工作室" })
      ).toBeVisible();

      // Traverse primary routes & assert zh-CN monolingual purity (0 codename prefixes, 0 raw SCREAMING_SNAKE_CASE)
      for (const route of [
        { path: "/", h1: "总控看板" },
        { path: "/hub?tab=datasets", h1: "数据与知识" },
        { path: "/studio", h1: "建模工作室" },
        { path: "/analytics", h1: "查询与制图" },
        { path: "/sim", h1: "推演沙盘" },
        { path: "/flow", h1: "编排与告警" },
        { path: "/view", h1: "数字看板" },
        { path: "/agents", h1: "智能体与接口" },
        { path: "/settings", h1: "系统设置" },
      ]) {
        await page.goto(route.path);
        await expect(
          page.getByRole("heading", { level: 1, name: route.h1 })
        ).toBeVisible();
        const bodyText = await page.locator("body").innerText();
        expect(bodyText).not.toMatch(CODENAME_PREFIX_RE);
        expect(await collectUiEnumLeaks(page)).toEqual([]);
      }

      // Dark mode + English toggle roundtrip
      await page.getByTestId("theme-toggle-btn").click();
      await expect(page.locator("html")).toHaveClass(/dark/);
      await page.getByTestId("locale-toggle-btn").click();
      await expect(
        page.getByRole("heading", { level: 1, name: "Settings" })
      ).toBeVisible();
      await page.getByTestId("locale-toggle-btn").click();
      await page.getByTestId("theme-toggle-btn").click();
      await expect(page.locator("html")).not.toHaveClass(/dark/);

      // Global ⌘K / Ctrl+K command palette jump
      await page.keyboard.press("Control+k");
      const cmdkDialog = page.getByTestId("global-cmdk-dialog");
      await expect(cmdkDialog).toBeVisible();
      const cmdkInput = page.getByTestId("global-cmdk-input");
      await cmdkInput.fill("透视");
      await expect(
        page.getByTestId("cmdk-item-nav-analytics-pivot")
      ).toBeVisible();
      await cmdkInput.press("Enter");
      await expect(cmdkDialog).toHaveCount(0);
      await expect(page).toHaveURL(/\/analytics\?tab=pivot/);

      // Resizable split pane drag + reload persistence
      await page.goto(`/studio?subgraph=${certSubgraphSlug}`);
      const studioWorkspace = page.getByTestId("resizable-workspace-studio");
      await expect(studioWorkspace).toBeVisible();
      const inspectorPane = page.getByTestId("resizable-pane-studio-inspector");
      const rightHandle = studioWorkspace.getByTestId("resizable-handle-right");
      const initialWidth = (await inspectorPane.boundingBox())!.width;
      const handleBox = (await rightHandle.boundingBox())!;
      await page.mouse.move(
        handleBox.x + handleBox.width / 2,
        handleBox.y + handleBox.height / 2
      );
      await page.mouse.down();
      await page.mouse.move(
        handleBox.x + handleBox.width / 2 - 120,
        handleBox.y + handleBox.height / 2,
        { steps: 8 }
      );
      await page.mouse.up();

      await expect
        .poll(async () => (await inspectorPane.boundingBox())?.width ?? 0)
        .toBeGreaterThan(initialWidth + 50);
      const widenedWidth = (await inspectorPane.boundingBox())!.width;

      await page.reload();
      await expect(studioWorkspace).toBeVisible();
      const reloadedWidth = (await inspectorPane.boundingBox())!.width;
      expect(Math.abs(reloadedWidth - widenedWidth)).toBeLessThan(24);

      // Restore default studio split width before Stage 2 canvas interactions
      await page.evaluate(() => {
        for (const key of Object.keys(localStorage)) {
          if (/resizable|split|studio/i.test(key)) {
            localStorage.removeItem(key);
          }
        }
      });
      await page.reload();
      await expect(studioWorkspace).toBeVisible();

      // =======================================================================
      // Stage 2: xBlueStudio Dual-Mode Canvas, 3-Tier Folding, ActionSpec & L4
      // =======================================================================
      console.log("[v3-e2e] Stage 2: xBlueStudio ELK 0-overlap, folding, ActionSpec & L4");
      await expect(page.getByTestId("xbluestudio-canvas")).toBeVisible();
      await expect(page.getByTestId("canvas-node-V3Plant")).toBeVisible();
      await expect(page.getByTestId("canvas-node-V3Hub")).toBeVisible();
      await expect(page.getByTestId("canvas-node-V3Outbound")).toBeVisible();
      await expect(page.getByTestId("canvas-node-V3StockCube")).toBeVisible();

      // Trigger ELK orthogonal layout & assert STRICT 0-overlap between edge pills and node cards
      await page.getByTestId("canvas-elk-layout-btn").click();
      await page.waitForTimeout(320);

      const canvasRects = await page.evaluate(() => {
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
      expect(canvasRects.nodes.length).toBeGreaterThanOrEqual(4);
      expect(canvasRects.edges.length).toBeGreaterThanOrEqual(4);
      for (const eRect of canvasRects.edges) {
        for (const nRect of canvasRects.nodes) {
          expect(computeRectOverlapArea(eRect, nRect)).toBe(0);
        }
      }

      // Domain swimlane collapse/expand (use DOM click on toggle button in case EdgeLabelRenderer summary badge overlaps header)
      await page.getByTestId("canvas-domain-group-toggle").click();
      await expect(page.getByTestId("domain-group-履约域")).toBeVisible();
      await page
        .getByTestId("domain-group-toggle-履约域")
        .evaluate((el: HTMLElement) => el.click());
      await expect(page.getByTestId("canvas-node-V3Hub")).toHaveCount(0);
      await page
        .getByTestId("domain-group-toggle-履约域")
        .evaluate((el: HTMLElement) => el.click());
      await expect(page.getByTestId("canvas-node-V3Hub")).toBeVisible();
      await page.getByTestId("canvas-domain-group-toggle").click();

      // 3-tier subordinate folding (EVENT_DYNAMIC + CUBE_ROLLUP collapse into V3Hub drawer)
      await page
        .getByTestId("fold-tier-btn-V3Hub")
        .evaluate((el: HTMLElement) => el.click());
      await expect(page.getByTestId("canvas-node-V3Outbound")).toHaveCount(0);
      await expect(page.getByTestId("canvas-node-V3StockCube")).toHaveCount(0);
      await expect(page.getByTestId("folded-tier-drawer-V3Hub")).toBeVisible();
      await page
        .getByTestId("fold-tier-btn-V3Hub")
        .evaluate((el: HTMLElement) => el.click());
      await expect(page.getByTestId("canvas-node-V3Outbound")).toBeVisible();

      // Dual-mode Pipeline view + 50-step Undo/Redo (⌘Z / ⌘⇧Z)
      await page.getByTestId("studio-view-MAPPING_PIPELINE").click();
      await expect(page.getByTestId("studio-pipeline-canvas")).toBeVisible();
      await page.getByTestId("pipeline-add-mapping-btn").click();
      await page
        .getByTestId("pipeline-mapping-name-input")
        .fill("v3_plant_sync_map");
      await page.getByTestId("pipeline-mapping-submit-btn").click();
      await expect(
        page.getByTestId("pipeline-mapping-v3_plant_sync_map")
      ).toBeVisible();
      await page.keyboard.press("ControlOrMeta+z");
      await expect(
        page.getByTestId("pipeline-mapping-v3_plant_sync_map")
      ).toHaveCount(0);
      await page.keyboard.press("ControlOrMeta+Shift+z");
      await expect(
        page.getByTestId("pipeline-mapping-v3_plant_sync_map")
      ).toBeVisible();
      await page.getByTestId("studio-view-ONTOLOGY_TOPOLOGY").click();

      // ActionSpec GraphQL Mutation execution via DomainApiDrawer
      await page.getByTestId("open-domain-drawer-btn").click();
      await expect(page.getByTestId("domain-api-drawer")).toBeVisible();
      await page.getByTestId("domain-tab-graphql").click();
      await page
        .getByTestId("graphql-query-input")
        .fill(
          `mutation { v3Hub_dispatchStock(key: "${hubKey}", idempotencyKey: "v3-gql-${runSuffix}", input: { qty: 20 }) { status mutatedNodes } }`
        );
      await page.getByTestId("graphql-run-btn").click();
      await expect(page.getByTestId("graphql-response-output")).toContainText(
        "APPLIED"
      );
      await page.getByTestId("close-domain-drawer-btn").click();

      // L4 Trial-Tick Certification (first detect DEAD_KNOWLEDGE on orphan metric, then certify clean subgraph -> L4_CERTIFIED)
      const certOutcome = await page.evaluate(
        async ({ slug, authTok }) => {
          const hdrs: Record<string, string> = {
            "Content-Type": "application/json",
          };
          if (authTok) hdrs.Authorization = `Bearer ${authTok}`;
          const deadRes = await fetch(
            `/api/xblue/schema/validate?subgraph=${encodeURIComponent(slug)}`,
            {
              method: "POST",
              headers: hdrs,
              body: JSON.stringify({
                node_types: [
                  {
                    name: "V3OrphanProbe",
                    display_name: "探针",
                    properties: [
                      {
                        name: "stock",
                        display_name: "库存",
                        data_type: "FLOAT64",
                        kind: "METRIC",
                      },
                      {
                        name: "orphan_gap",
                        display_name: "孤立指标",
                        data_type: "FLOAT64",
                        kind: "METRIC",
                      },
                    ],
                    derived_rules: [
                      {
                        target_metric: "orphan_gap",
                        expression: "stock * 2",
                        enabled: true,
                      },
                    ],
                  },
                ],
                edge_types: [],
                mappings: [],
              }),
            }
          );
          const deadJson = (await deadRes.json()) as {
            issues?: Array<{ code: string }>;
          };

          const certRes = await fetch(
            `/api/xblue/subgraphs/${encodeURIComponent(slug)}/certify`,
            {
              method: "POST",
              headers: hdrs,
              body: JSON.stringify({}),
            }
          );
          const certJson = (await certRes.json()) as {
            cert_level: string;
            trial_tick_passed: boolean;
            dead_knowledge_cnt: number;
          };
          return {
            hasDeadKnowledge: (deadJson.issues ?? []).some(
              (i) => i.code === "DEAD_KNOWLEDGE"
            ),
            cert: certJson,
          };
        },
        { slug: certSubgraphSlug, authTok: cleanup.adminToken }
      );
      expect(certOutcome.hasDeadKnowledge).toBe(true);
      expect(certOutcome.cert.trial_tick_passed).toBe(true);
      expect(certOutcome.cert.dead_knowledge_cnt).toBe(0);
      expect(["L4_CERTIFIED", "L3_VERIFIED"]).toContain(
        certOutcome.cert.cert_level
      );

      // =======================================================================
      // Stage 3: Explore & Charts (AxQL Anti-Fanout + 1-Step Pin to Cockpit)
      // =======================================================================
      console.log("[v3-e2e] Stage 3: Explore & Charts (AxQL + Chart Studio Pin)");
      await page.goto("/analytics");
      await expect(page.getByTestId("analytics-sql-editor")).toBeVisible();

      // Restore hub inventory to 500 before running SUM_BY_ENTITY anti-fanout check
      if (isLive) {
        await page.request.patch(`${coreBaseUrl}/api/xblue/nodes/${hubNodeId}`, {
          headers: {
            Authorization: `Bearer ${cleanup.adminToken}`,
            "Content-Type": "application/json",
          },
          data: { metrics: { inventory: 500 }, metrics_merge: "MERGE" },
        });
      }

      const axqlResult = await page.evaluate(
        async ({ targetKey, authTok }) => {
          const hdrs: Record<string, string> = {
            "Content-Type": "application/json",
          };
          if (authTok) hdrs.Authorization = `Bearer ${authTok}`;
          const res = await fetch("/api/analytics/axql/run", {
            method: "POST",
            headers: hdrs,
            body: JSON.stringify({
              axql: `SELECT SUM_BY_ENTITY(w.inventory BY w) AS exact_inv, COUNT(o.node_key) AS order_cnt FROM MATCH (w:V3Hub)-[:V3_RECORDS_OUTBOUND]->(o:V3Outbound) WHERE w.node_key = '${targetKey}'`,
            }),
          });
          return (await res.json()) as {
            rows: Array<{ exact_inv: number | string; order_cnt: number | string }>;
            plan: { fanout_protected: boolean };
          };
        },
        { targetKey: hubKey, authTok: cleanup.adminToken }
      );
      expect(axqlResult.plan.fanout_protected).toBe(true);
      expect(Number(axqlResult.rows[0]?.exact_inv)).toBe(500);
      expect(Number(axqlResult.rows[0]?.order_cnt)).toBe(2);

      // Run SQL -> Open Chart Studio -> Save xv_charts -> 1-step Pin to Five-Zone INSIGHT
      await page.getByTestId("sql-run-btn").click();
      const openChartStudioBtn = page.getByTestId(
        "result-open-chart-studio-btn"
      );
      await expect(openChartStudioBtn).toBeVisible({ timeout: 20_000 });
      await openChartStudioBtn.click();

      await expect(page.getByTestId("chart-studio-panel")).toBeVisible();
      await expect(
        page
          .getByTestId("chart-studio-type-grid")
          .locator("button[data-testid^='chart-studio-kind-']")
      ).toHaveCount(16);

      const pinnedChartName = `履约洞察资产_${runSuffix}`;
      await page.getByTestId("chart-studio-kind-bar").click();
      await page.getByTestId("chart-studio-name-input").fill(pinnedChartName);
      await page.getByTestId("chart-studio-markline-toggle").check();
      await page.getByTestId("chart-studio-markline-input").fill("5000");

      await page.getByTestId("chart-studio-save-btn").click();
      await expect(page.getByTestId("chart-studio-status-badge")).toContainText(
        "已保存",
        { timeout: 15_000 }
      );

      await page.getByTestId("chart-studio-pin-btn").click();
      await expect(page.getByTestId("pin-to-dashboard-modal")).toBeVisible();
      await chooseSelectOption(page, "pin-modal-dashboard-select", dashboardId);
      await chooseSelectOption(page, "pin-modal-zone-select", "INSIGHT");
      await page.getByTestId("pin-modal-confirm-btn").click();

      await expect(page.getByTestId("pin-modal-success")).toContainText(
        "已钉入",
        { timeout: 15_000 }
      );
      await expect(page.getByTestId("pin-modal-result-zone")).toContainText(
        "归因洞察"
      );
      await page.getByTestId("pin-modal-close-btn").click();

      // =======================================================================
      // Stage 4: VTable Multi-Dimensional Pivot & State-Driven Dynamic Visibility
      // =======================================================================
      console.log("[v3-e2e] Stage 4: VTable Pivot & State-Driven Dynamic Visibility");
      await page.getByTestId("analytics-tab-pivot").click();
      await expect(page.getByTestId("analytics-pivot-builder")).toBeVisible();
      await chooseSelectOption(page, "pivot-dataset-select", "demo_orders");

      const firstQuickRow = page
        .locator("[data-testid^='pivot-quick-row-']")
        .first();
      const amountQuickVal = page.getByTestId("pivot-quick-val-amount");
      const firstQuickVal = page
        .locator("[data-testid^='pivot-quick-val-']")
        .first();
      await expect(firstQuickVal).toBeVisible({ timeout: 15_000 });
      if (await firstQuickRow.isVisible()) {
        await firstQuickRow.click();
      }
      if (await amountQuickVal.isVisible()) {
        await amountQuickVal.click();
      } else {
        await firstQuickVal.click();
      }
      const pivotRunBtn = page.getByTestId("pivot-run-btn");
      if (await pivotRunBtn.isEnabled()) {
        await pivotRunBtn.click();
      }

      const vtableStage = page.getByTestId("vtable-canvas-stage");
      await expect(vtableStage).toBeVisible({ timeout: 20_000 });
      await expect(vtableStage.locator("canvas").first()).toBeVisible({
        timeout: 15_000,
      });
      await expect(vtableStage).toHaveAttribute("data-databars", "on");

      await page.getByTestId("vtable-toggle-heatmap").click();
      await expect(vtableStage).toHaveAttribute("data-heatmap", "on");
      await expect(
        page
          .locator(
            "[data-testid='pivot-result-table'] [data-heatmap-active='true']"
          )
          .first()
      ).toBeVisible();

      // State-driven dynamic row visibility: 0 rows when inventory=500 -> 1 row when inventory=60 (< 100)
      const countObserverHubRows = async (): Promise<number> =>
        page.evaluate(
          async ({ obsTok, targetKey }) => {
            const r = await fetch("/api/xblue/nodes?type_name=V3Hub", {
              headers: { Authorization: `Bearer ${obsTok}` },
            });
            const body = (await r.json()) as {
              items?: Array<{ node_key: string }>;
            };
            return (body.items ?? []).filter((n) => n.node_key === targetKey)
              .length;
          },
          { obsTok: observerToken, targetKey: hubKey }
        );

      expect(await countObserverHubRows()).toBe(0);

      await page.evaluate(
        async ({ nid, authTok }) => {
          const hdrs: Record<string, string> = {
            "Content-Type": "application/json",
          };
          if (authTok) hdrs.Authorization = `Bearer ${authTok}`;
          await fetch(`/api/xblue/nodes/${nid}`, {
            method: "PATCH",
            headers: hdrs,
            body: JSON.stringify({
              metrics: { inventory: 60 },
              metrics_merge: "MERGE",
            }),
          });
        },
        { nid: hubNodeId, authTok: cleanup.adminToken }
      );
      expect(await countObserverHubRows()).toBe(1);

      // Restore inventory to 500
      await page.evaluate(
        async ({ nid, authTok }) => {
          const hdrs: Record<string, string> = {
            "Content-Type": "application/json",
          };
          if (authTok) hdrs.Authorization = `Bearer ${authTok}`;
          await fetch(`/api/xblue/nodes/${nid}`, {
            method: "PATCH",
            headers: hdrs,
            body: JSON.stringify({
              metrics: { inventory: 500 },
              metrics_merge: "MERGE",
            }),
          });
        },
        { nid: hubNodeId, authTok: cleanup.adminToken }
      );

      // =======================================================================
      // Stage 5: AxFlow Midpoint + Operator Splice & Pulse Execution
      // =======================================================================
      console.log("[v3-e2e] Stage 5: AxFlow Midpoint + Operator Splice & Pulse");
      await page.goto("/flow");
      await expect(page.getByTestId("axflow-canvas")).toBeVisible();
      if (isLive) {
        await chooseSelectOption(page, "axflow-flow-select", flowId);
      }

      await expect(
        page.getByTestId("flow-edge-sync_step-eval_step")
      ).toContainText("when: n > 0");

      const insertBtn = page.getByTestId(
        "flow-edge-insert-btn-sync_step-eval_step"
      );
      await insertBtn.hover({ force: true });
      await insertBtn.click({ force: true });

      await expect(page.getByTestId("quick-operator-popover")).toBeVisible();
      await page.getByTestId("quick-op-item-HUMAN_APPROVAL").click();

      await expect(page.getByTestId("flow-node-human_approval_1")).toBeVisible();
      await expect(
        page.getByTestId("flow-edge-sync_step-eval_step")
      ).toHaveCount(0);
      await expect(
        page.getByTestId("flow-edge-sync_step-human_approval_1")
      ).toContainText("when: n > 0");
      await expect(
        page.getByTestId("flow-edge-human_approval_1-eval_step")
      ).toBeVisible();

      const approvalTitleInput = page.getByTestId("config-field-title");
      if (await approvalTitleInput.isVisible()) {
        await approvalTitleInput.fill("库存调拨门禁审批");
      }

      if (isLive) {
        const saveDone = page.waitForResponse(
          (r) =>
            r.url().includes(`/api/axflow/flows/${flowId}`) &&
            r.request().method() === "PUT" &&
            r.ok()
        );
        await page.getByTestId("header-save-flow-btn").click();
        await saveDone;

        const authHdrs = {
          Authorization: `Bearer ${cleanup.adminToken}`,
          "Content-Type": "application/json",
        };
        const pubRes = await page.request.post(
          `${coreBaseUrl}/api/axflow/flows/${flowId}/versions`,
          {
            headers: authHdrs,
            data: { note: "v3 splice human_approval_1" },
          }
        );
        expect(pubRes.ok()).toBe(true);
        const pubVer = ((await pubRes.json()) as { version?: number }).version ?? 1;

        const depRes = await page.request.post(
          `${coreBaseUrl}/api/axflow/flows/${flowId}/deploy`,
          {
            headers: authHdrs,
            data: { version: pubVer, environment: "TEST" },
          }
        );
        expect(depRes.ok()).toBe(true);

        await chooseSelectOption(page, "axflow-run-env-select", "TEST");
        await page.getByTestId("axflow-run-btn").click();
      }

      await expect(
        page.getByTestId("canvas-node-duration-sync_step")
      ).toContainText("ms", { timeout: 20_000 });
      await expect(
        page.getByTestId("canvas-node-status-human_approval_1")
      ).toBeVisible({ timeout: 20_000 });

      // =======================================================================
      // Stage 6: AxSim 2.0 Event Queue, Deadline Banner, Geofence & G6 Hull
      // =======================================================================
      console.log("[v3-e2e] Stage 6: AxSim 2.0 Event Queue, Deadline & G6 Hull");
      if (isLive) {
        const authHdrs = {
          Authorization: `Bearer ${cleanup.adminToken}`,
          "Content-Type": "application/json",
        };
        await page.request.patch(`${coreBaseUrl}/api/xblue/nodes/${hubNodeId}`, {
          headers: authHdrs,
          data: { metrics: { inventory: 300 }, metrics_merge: "MERGE" },
        });
        const freshSbxResp = await page.request.post(
          `${coreBaseUrl}/api/axsim/sandboxes`,
          {
            headers: authHdrs,
            data: {
              name: `v3_sim_live_${runSuffix}`,
              scope: { type_names: ["V3Plant", "V3Hub"] },
            },
          }
        );
        expect(freshSbxResp.ok()).toBe(true);
        simSandboxId = ((await freshSbxResp.json()) as { id: string }).id;
        cleanup.sandboxIds.push(simSandboxId);
      }
      await page.goto(`/sim?sandbox=${simSandboxId}`);
      await expect(page.getByTestId("event-queue-editor")).toBeVisible();

      // Event 0: AT_STEP = 4, ADD -150 on plantNodeId.output
      await page.getByTestId("add-sim-event-btn").click();
      const evRow0 = page.getByTestId("sim-event-row-0");
      await expect(evRow0).toBeVisible();
      await page.getByTestId("sim-event-name-0").fill("第4步突发减产");
      await page.getByTestId("sim-event-step-0").fill("4");
      await selectFieldByLabel(page, evRow0, "作用节点", plantNodeId);
      await selectFieldByLabel(page, evRow0, "作用指标", "output");
      await evRow0.locator("input[placeholder='-150']").fill("-150");

      // Event 1: METRIC_THRESHOLD, watch hubNodeId.inventory LT 200 -> ADD +80
      await page.getByTestId("add-sim-event-btn").click();
      const evRow1 = page.getByTestId("sim-event-row-1");
      await page.getByTestId("sim-event-name-1").fill("低库存应急补货");
      await chooseSelectOption(page, "sim-event-kind-1", "METRIC_THRESHOLD");
      await selectFieldByLabel(page, evRow1, "监控节点", hubNodeId);
      await selectFieldByLabel(page, evRow1, "监控指标", "inventory");
      await evRow1.locator("input[placeholder='200']").fill("200");
      await selectFieldByLabel(page, evRow1, "作用节点", hubNodeId);
      await selectFieldByLabel(page, evRow1, "作用指标", "inventory");
      await evRow1.locator("input[placeholder='-150']").fill("80");

      // Goal Constraint 0: Shanghai Hub inventory >= 200 with Polygon Geofence & Controllable Plant
      await page.getByTestId("add-sim-goal-btn").click();
      const goalRow0 = page.getByTestId("sim-goal-row-0");
      await page.getByTestId("sim-goal-name-0").fill("长三角安全库存红线");
      await selectFieldByLabel(page, goalRow0, "目标节点", hubNodeId);
      await selectFieldByLabel(page, goalRow0, "考核指标", "inventory");
      await selectFieldByLabel(page, goalRow0, "可控源头", plantNodeId);
      await selectFieldByLabel(page, goalRow0, "空间过滤", "GEO_WITHIN_POLYGON");
      await goalRow0
        .locator("input[placeholder*='119.5']")
        .fill(
          "[[[119.5,29.8],[122.2,29.8],[122.2,32.0],[119.5,32.0],[119.5,29.8]]]"
        );

      // Remove default initial action row (actions: []) so scheduled events drive exact step=4 -> breach=7 -> deadline=3 without pinning any node
      await page
        .getByTestId("action-row-0")
        .locator("> div.grid button:not([role='combobox'])")
        .click();
      await expect(page.getByTestId("action-row-0")).toHaveCount(0);
      const dampingInput = page
        .getByTestId("action-list-panel")
        .locator("input[min='0.001'][max='1']");
      await dampingInput.fill("1");
      await page.getByTestId("sim-horizon-steps").fill("12");

      await page.getByTestId("run-scenario-btn").click();

      const deadlineBanner = page.getByTestId("goal-deadline-banner");
      await expect(deadlineBanner).toBeVisible({ timeout: 25_000 });
      await expect(page.getByTestId("earliest-breach-step")).toContainText("7");
      await expect(page.getByTestId("earliest-deadline-step")).toContainText(
        "3"
      );
      await expect(page.getByTestId("fired-events-strip")).toBeVisible();

      // Verify @antv/g6 ImpactGraph 3-track card, Hull contour & Combo collapse/expand
      const g6Canvas = page.getByTestId("impact-graph-g6-canvas");
      await expect(g6Canvas).toBeVisible({ timeout: 20_000 });
      await expect(g6Canvas.locator("canvas").first()).toBeVisible();
      await expect(page.getByTestId("impact-hull-badge")).toBeVisible();
      await expect(
        page.getByTestId(`sim-card-tracks-${hubNodeId}`)
      ).toBeVisible();

      const comboToggle = page.getByTestId("impact-combo-toggle-履约域");
      await expect(comboToggle).toBeVisible();
      await comboToggle.click();
      await expect(comboToggle).toHaveAttribute("data-collapsed", "true");
      await comboToggle.click();
      await expect(comboToggle).toHaveAttribute("data-collapsed", "false");

      if (isLive) {
        await page.request.patch(`${coreBaseUrl}/api/xblue/nodes/${hubNodeId}`, {
          headers: {
            Authorization: `Bearer ${cleanup.adminToken}`,
            "Content-Type": "application/json",
          },
          data: { metrics: { inventory: 500 }, metrics_merge: "MERGE" },
        });
      }

      // =======================================================================
      // Stage 7: xBlueView Cockpit Magnetic Grid (FREE_FLOAT + Tidy) & Writeback
      // =======================================================================
      console.log("[v3-e2e] Stage 7: xBlueView Cockpit FREE_FLOAT, Tidy & Writeback");
      await page.goto(`/view/${cockpitSlug}`);

      const kpiWidget = page.getByTestId(`widget-kpi-${widgetKpiId}`);
      await expect(kpiWidget).toBeVisible({ timeout: 20_000 });
      await expect(kpiWidget.getByTestId("kpi-sparkline")).toBeVisible();
      await expect(page.locator("body")).toContainText(pinnedChartName);

      // Enter edit mode -> visible dashed tracks -> switch to FREE_FLOAT ("锁定位置")
      await page.getByTestId("toggle-edit-mode-btn").click();
      const gridTracks = page.getByTestId("dashboard-grid-tracks");
      await expect(gridTracks).toBeVisible();

      await page.getByTestId("grid-compact-free-float-btn").click();
      await expect(page.getByTestId("dashboard-grid")).toHaveAttribute(
        "data-compact-mode",
        "FREE_FLOAT"
      );
      await page.waitForTimeout(250);

      const barFrame = page.getByTestId(`widget-frame-${widgetBarId}`);
      const lowerActionFrame = page.getByTestId(`widget-frame-${widgetActId}`);
      const gridBefore = (await gridTracks.boundingBox())!;
      const lowerBefore = (await lowerActionFrame.boundingBox())!;
      const lowerRelYBefore = lowerBefore.y - gridBefore.y;

      // Drag upper BAR card upward by 1 row -> lower ACTION card at y=6 must NOT shift in FREE_FLOAT mode
      const barHandle = barFrame.locator(".widget-drag-handle").first();
      const barHandleBox = (await barHandle.boundingBox())!;
      await page.mouse.move(
        barHandleBox.x + barHandleBox.width / 2,
        barHandleBox.y + barHandleBox.height / 2
      );
      await page.mouse.down();
      await page.mouse.move(
        barHandleBox.x + barHandleBox.width / 2,
        barHandleBox.y + barHandleBox.height / 2 - 84,
        { steps: 8 }
      );
      await page.mouse.up();
      await page.waitForTimeout(250);

      const gridAfter = (await gridTracks.boundingBox())!;
      const lowerAfter = (await lowerActionFrame.boundingBox())!;
      expect(
        Math.abs(lowerAfter.y - gridAfter.y - lowerRelYBefore)
      ).toBeLessThan(2);

      // Click Tidy ("对齐整理") + Save Layout ("保存布局") -> handleSaveLayout automatically exits edit mode
      await page.getByTestId("grid-tidy-layout-btn").click();
      await page.getByTestId("save-layout-btn").click();
      await expect(page.getByTestId("dashboard-grid-tracks")).toHaveCount(0);

      // Trigger Action Writeback in runtime mode -> writes node metric & refreshes KPI sparkline
      const sparkPathBefore = await kpiWidget
        .getByTestId("kpi-sparkline")
        .locator("path")
        .nth(1)
        .getAttribute("d");
      expect(sparkPathBefore).toBeTruthy();

      const actionBox = page.getByTestId(`widget-action-${widgetActId}`);
      await expect(actionBox).toBeVisible();
      await actionBox.getByTestId("action-input-value").fill("680");
      await actionBox.getByTestId(`action-submit-${widgetActId}`).click();

      await expect(
        page.getByTestId(`action-result-${widgetActId}`)
      ).toContainText("DONE", { timeout: 15_000 });

      const writtenHubInv = await page.evaluate(
        async ({ targetKey, authTok }) => {
          const hdrs: Record<string, string> = {};
          if (authTok) hdrs.Authorization = `Bearer ${authTok}`;
          const r = await fetch("/api/xblue/nodes?type_name=V3Hub", {
            headers: hdrs,
          });
          const body = (await r.json()) as {
            items?: Array<{
              node_key: string;
              metrics?: { inventory?: number };
            }>;
          };
          const found = (body.items ?? []).find(
            (n) => n.node_key === targetKey
          );
          return Number(found?.metrics?.inventory ?? 0);
        },
        { targetKey: hubKey, authTok: cleanup.adminToken }
      );
      expect(writtenHubInv).toBe(680);

      if (isLive) {
        const authHdrs = {
          Authorization: `Bearer ${cleanup.adminToken}`,
          "Content-Type": "application/json",
        };
        const mwRes = await page.request.post(
          `${coreBaseUrl}/api/xblue/metrics/write`,
          {
            headers: authHdrs,
            data: {
              points: [
                {
                  node_id: hubNodeId,
                  metric: "inventory",
                  ts: new Date(Date.now() - 60_000).toISOString(),
                  value: 680,
                },
              ],
              source_tag: "MANUAL",
            },
          }
        );
        if (mwRes.ok()) {
          const mwBody = (await mwRes.json()) as { rollup_job_id?: string };
          if (mwBody.rollup_job_id) {
            for (let attempt = 0; attempt < 25; attempt += 1) {
              const jobRes = await page.request.get(
                `${coreBaseUrl}/api/axops/jobs/${mwBody.rollup_job_id}`,
                { headers: authHdrs }
              );
              if (jobRes.ok()) {
                const jobBody = (await jobRes.json()) as { status?: string };
                if (jobBody.status === "SUCCEEDED") break;
              }
              await page.waitForTimeout(200);
            }
          }
        }
        await page.request.put(
          `${coreBaseUrl}/api/xblueview/widgets/${widgetKpiId}`,
          {
            headers: authHdrs,
            data: { refresh_mode: "LIVE" },
          }
        );
      }

      await expect
        .poll(
          async () => {
            if (isLive) {
              const refreshBtn = page.getByTestId("refresh-all-widgets-btn");
              if (await refreshBtn.isVisible()) {
                await refreshBtn.click();
              }
            }
            return await kpiWidget
              .getByTestId("kpi-sparkline")
              .locator("path")
              .nth(1)
              .getAttribute("d");
          },
          { timeout: 15_000, intervals: [500, 1000, 1500, 2000] }
        )
        .not.toBe(sparkPathBefore);

      // =======================================================================
      // Stage 8: AxBuilder 22 Tools & Golden Competency QA Suite Loop
      // =======================================================================
      console.log("[v3-e2e] Stage 8: AxBuilder 22 Tools & Competency QA Suite");
      await page.goto("/agents?tab=mcp");
      const mcpCard = page.getByTestId("mcp-builder-config-card");
      await expect(mcpCard).toBeVisible();
      await expect(
        page.getByTestId("mcp-builder-tool-count-badge")
      ).toContainText("22");

      const toolCatalog = page.getByTestId("mcp-builder-22-tools");
      await expect(toolCatalog).toBeVisible();
      await expect(
        toolCatalog.locator("span").filter({ hasText: /^axbuilder_/ })
      ).toHaveCount(22);

      // Issue BUILDER token and verify /api/mcp tools/list exposes all 22 tools
      await page.getByTestId("mcp-builder-token-name").fill(`v3-pat-${runSuffix}`);
      await page.getByTestId("mcp-builder-issue-btn").click();
      await expect(
        page.getByTestId("mcp-builder-token-ready-badge")
      ).toBeVisible({ timeout: 15_000 });
      const snippetText = await page
        .getByTestId("mcp-builder-snippet")
        .innerText();
      const patMatch = /Bearer\s+([^\s"]+)/.exec(snippetText);
      expect(patMatch?.[1]).toBeTruthy();

      const mcpToolCount = await page.evaluate(async (pat: string) => {
        const initRes = await fetch("/api/mcp", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${pat}`,
            Accept: "application/json, text/event-stream",
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            method: "initialize",
            params: {
              protocolVersion: "2025-03-26",
              capabilities: {},
              clientInfo: { name: "v3-e2e", version: "1.0" },
            },
          }),
        });
        const sid = initRes.headers.get("mcp-session-id") ?? "";
        await fetch("/api/mcp", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${pat}`,
            Accept: "application/json, text/event-stream",
            "Content-Type": "application/json",
            ...(sid ? { "Mcp-Session-Id": sid } : {}),
          },
          body: JSON.stringify({
            jsonrpc: "2.0",
            method: "notifications/initialized",
          }),
        });
        const listRes = await fetch("/api/mcp", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${pat}`,
            Accept: "application/json, text/event-stream",
            "Content-Type": "application/json",
            ...(sid ? { "Mcp-Session-Id": sid } : {}),
          },
          body: JSON.stringify({
            jsonrpc: "2.0",
            id: 2,
            method: "tools/list",
            params: {},
          }),
        });
        const rawText = await listRes.text();
        const jsonLine =
          rawText
            .split("\n")
            .map((l) => l.replace(/^data:\s*/, "").trim())
            .filter((l) => l.startsWith("{"))
            .pop() ?? "{}";
        const parsed = JSON.parse(jsonLine) as {
          result?: { tools?: unknown[] };
        };
        return parsed.result?.tools?.length ?? 0;
      }, patMatch![1]!);
      expect(mcpToolCount).toBe(22);

      // Open CompetencySuiteModal in /studio -> gap diagnostic -> fix metric -> PASS
      await page.goto(`/studio?subgraph=${certSubgraphSlug}`);
      await page.getByTestId("open-competency-suite-btn").click();
      const compModal = page.getByTestId("competency-suite-modal");
      await expect(compModal).toBeVisible();

      await page.getByTestId("competency-add-question-btn").click();
      const qRow0 = page.getByTestId("competency-question-row-0");
      await expect(qRow0).toBeVisible();
      await qRow0
        .getByLabel("问数语句")
        .fill("统计第30步履约订单有效记录总数");
      await qRow0
        .locator("input[placeholder='stock_qty, order_amount']")
        .fill("unmodeled_safety_index");
      await qRow0
        .locator("input[placeholder='1200']")
        .fill(String(demoOrdersRowCount));

      // Round 1: Missing metric -> GAP_DETECTED
      await page.getByTestId("competency-verify-btn").click();
      await expect(page.getByTestId("competency-gap-list")).toBeVisible({
        timeout: 20_000,
      });
      await expect(page.getByTestId("competency-gap-item-0")).toContainText(
        "unmodeled_safety_index"
      );

      // Round 2: Align to modeled metric `inventory` on V3Hub -> status === "PASS"
      await qRow0
        .locator("input[placeholder='stock_qty, order_amount']")
        .fill("inventory");
      await page.getByTestId("competency-verify-btn").click();

      await expect(page.getByTestId("competency-run-report")).toContainText(
        "1 / 1",
        { timeout: 25_000 }
      );
      await expect(page.getByTestId("competency-gap-list")).toHaveCount(0);
      await expect(page.getByTestId("competency-latest-status")).toContainText(
        "通过"
      );
      await page.getByTestId("close-competency-suite-btn").click();

      expect(leakedTenantUrls).toEqual([]);
      expect(pageErrors).toEqual([]);
    } finally {
      await page.unrouteAll({ behavior: "ignoreErrors" }).catch(() => null);
      if (isLive && cleanup.adminToken) {
        const headers = {
          Authorization: `Bearer ${cleanup.adminToken}`,
          "Content-Type": "application/json",
        };
        for (const id of cleanup.rowPolicyIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axguard/row-policies/${id}`, {
              headers,
            })
            .catch(() => null);
        }
        if (cleanup.resetDefaultDenyRef) {
          await page.request
            .put(`${coreBaseUrl}/api/axguard/row-policy-defaults`, {
              headers,
              data: {
                target_kind: "NODE_TYPE",
                target_ref: "V3Hub",
                default_deny: false,
              },
            })
            .catch(() => null);
        }
        for (const id of cleanup.dashboardIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/xblueview/dashboards/${id}`, {
              headers,
            })
            .catch(() => null);
        }
        for (const id of cleanup.chartIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/xblueview/charts/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.sandboxIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axsim/sandboxes/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.flowIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axflow/flows/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.nodeIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/xblue/nodes/${id}`, { headers })
            .catch(() => null);
        }
        for (const slug of cleanup.subgraphs) {
          await page.request
            .delete(`${coreBaseUrl}/api/xblue/subgraphs/${slug}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.tokenIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/auth/tokens/${id}`, { headers })
            .catch(() => null);
        }
      }
    }
  });
});
