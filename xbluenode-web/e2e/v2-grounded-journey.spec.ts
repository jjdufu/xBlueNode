import {
  expect,
  type APIRequestContext,
  type Locator,
  type Page,
  type Route,
  test,
} from "@playwright/test";
import { chooseSelectOption } from "./helpers";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const OPS_ADMIN_ID = "00000000-0000-0000-0000-000000002201";
const DEMO_ORDERS_DS_ID = "00000000-0000-0000-0000-000000002202";
const ISOLATED_VAULT_DS_ID = "00000000-0000-0000-0000-000000002203";
const POLICY_DOC_ID = "00000000-0000-0000-0000-000000002204";
const POLICY_PARA_ID = "00000000-0000-0000-0000-000000002205";
const FACTORY_F1_ID = "00000000-0000-0000-0000-000000002210";
const WAREHOUSE_W1_ID = "00000000-0000-0000-0000-000000002211";
const ORDER_O1_ID = "00000000-0000-0000-0000-000000002212";
const TRIAL_DRAFT_ID = "00000000-0000-0000-0000-000000002220";
const SNAPSHOT_1_ID = "00000000-0000-0000-0000-000000002221";
const SNAPSHOT_2_ID = "00000000-0000-0000-0000-000000002222";
const SANDBOX_ID = "00000000-0000-0000-0000-000000002230";
const DASHBOARD_ID = "00000000-0000-0000-0000-000000002240";
const BROKEN_WIDGET_ID = "00000000-0000-0000-0000-000000002241";
const AGENT_ID = "00000000-0000-0000-0000-000000002250";
const IM_CONNECTOR_ID = "00000000-0000-0000-0000-000000002260";
const IM_BINDING_ID = "00000000-0000-0000-0000-000000002261";
const MOCK_IM_BIND_TOKEN = "axim_step22_mock_challenge_token";

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

interface MockPropertySpec {
  name: string;
  display_name: string;
  description: string;
  unit?: string | null;
  data_type: string;
  kind: string;
  required: boolean;
  pii_category: string;
  maturity?: string;
}

interface MockNodeTypeSpec {
  name: string;
  display_name: string;
  description: string;
  icon: string;
  color: string;
  title_template: string;
  properties: MockPropertySpec[];
  agg_rules: Array<Record<string, unknown>>;
  derived_rules: Array<Record<string, unknown>>;
  sync_to_knowledge: boolean;
  pos_x: number;
  pos_y: number;
}

interface MockSchemaDoc {
  node_types: MockNodeTypeSpec[];
  edge_types: Array<Record<string, unknown>>;
  mappings: Array<Record<string, unknown>>;
}

interface MockWidgetRecord {
  id: string;
  tenant_id: string;
  dashboard_id: string;
  widget_kind: string;
  title: string;
  position: {
    x: number;
    y: number;
    w: number;
    h: number;
    semantic_zone?: string | null;
  };
  data_source: Record<string, unknown>;
  encoding: Record<string, unknown>;
  options: Record<string, unknown>;
  drill: unknown[];
  refresh_mode: string;
  refresh_seconds: number;
  deps: { datasets: string[]; metrics: string[]; sandboxes: string[] };
  created_at: string;
  updated_at: string;
}

interface V2JourneyMockState {
  hasSession: boolean;
  trialSubgraphSlug: string | null;
  trialBaseVersion: number;
  trialDraftDoc: MockSchemaDoc;
  prePatchDoc: MockSchemaDoc | null;
  snapshots: Array<{
    id: string;
    draft_id: string;
    snapshot_no: number;
    reason: string;
    created_at: string;
  }>;
  issuedTokens: Array<{
    id: string;
    name: string;
    token: string;
    token_profile: string;
    permissions: string[];
    created_at: string;
  }>;
  sandboxes: Array<{
    id: string;
    name: string;
    status: string;
    node_count: number;
    link_count: number;
    scope: Record<string, unknown>;
    base_captured_at: string;
    schema_version: number;
  }>;
  dashboards: Array<{
    id: string;
    tenant_id: string;
    name: string;
    slug: string;
    description: string;
    filters: unknown[];
    theme: string;
    canvas: {
      columns: number;
      row_height: number;
      gap: number;
      zone_layout?: string;
    };
    status: string;
    version: number;
    created_by: string;
    created_at: string;
    updated_at: string;
    widgets: MockWidgetRecord[];
  }>;
  imConnectors: Array<{
    id: string;
    tenant_id: string;
    name: string;
    platform: string;
    agent_id: string;
    has_secret: boolean;
    config: Record<string, unknown>;
    identity_policy: string;
    delegated_user_id: string | null;
    enabled: boolean;
    leader_holder: string | null;
    lease_until: string | null;
    status: string;
    last_error: string;
    last_heartbeat_at: string | null;
    created_by: string | null;
    created_at: string;
    updated_at: string;
  }>;
  imBindings: Array<{
    id: string;
    tenant_id: string;
    platform: string;
    external_user_id: string;
    external_display_name: string;
    user_id: string;
    username: string;
    bound_at: string;
  }>;
  bindTokenConsumed: boolean;
}

function buildInitialSupplyChainDoc(): MockSchemaDoc {
  return {
    node_types: [
      {
        name: "Supplier",
        display_name: "供应商",
        description: "上游原材料与零部件供应商",
        icon: "truck",
        color: "#0891b2",
        title_template: "{prop.supplier_name}",
        properties: [
          {
            name: "supplier_name",
            display_name: "供应商名称",
            description: "企业简称",
            data_type: "STRING",
            kind: "ATTRIBUTE",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "lead_days",
            display_name: "平均交期",
            description: "采购交付天数",
            unit: "天",
            data_type: "FLOAT64",
            kind: "METRIC",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        sync_to_knowledge: true,
        pos_x: 80,
        pos_y: 180,
      },
      {
        name: "Factory",
        display_name: "工厂",
        description: "生产制造与补货工厂",
        icon: "factory",
        color: "#2563eb",
        title_template: "{prop.factory_name}",
        properties: [
          {
            name: "factory_name",
            display_name: "工厂名称",
            description: "工厂显示名称",
            data_type: "STRING",
            kind: "ATTRIBUTE",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "capacity_units",
            display_name: "日产能",
            description: "标准日产出上限",
            unit: "件",
            data_type: "FLOAT64",
            kind: "METRIC",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        sync_to_knowledge: true,
        pos_x: 340,
        pos_y: 180,
      },
      {
        name: "Warehouse",
        display_name: "仓库",
        description: "区域分拨与订单履约中心仓",
        icon: "warehouse",
        color: "#16a34a",
        title_template: "{prop.warehouse_name}",
        properties: [
          {
            name: "warehouse_name",
            display_name: "仓库名称",
            description: "区域仓名称",
            data_type: "STRING",
            kind: "ATTRIBUTE",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "inventory_qty",
            display_name: "可用库存量",
            description: "现货件数",
            unit: "件",
            data_type: "FLOAT64",
            kind: "METRIC",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "demand_qty",
            display_name: "待履约需求量",
            description: "未出库订单需求件数",
            unit: "件",
            data_type: "FLOAT64",
            kind: "METRIC",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "fill_rate",
            display_name: "订单履约率",
            description: "库存对需求满足率",
            unit: "ratio",
            data_type: "FLOAT64",
            kind: "METRIC",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [
          {
            target_metric: "fill_rate",
            expression: "if(demand_qty > 0, inventory_qty / demand_qty, 1.0)",
            enabled: true,
          },
        ],
        sync_to_knowledge: true,
        pos_x: 600,
        pos_y: 180,
      },
      {
        name: "Order",
        display_name: "客户订单",
        description: "终端客户履约订单",
        icon: "shopping-cart",
        color: "#d97706",
        title_template: "{prop.order_label}",
        properties: [
          {
            name: "order_label",
            display_name: "订单标题",
            description: "订单标识",
            data_type: "STRING",
            kind: "ATTRIBUTE",
            required: false,
            pii_category: "NONE",
          },
          {
            name: "amount",
            display_name: "订单金额",
            description: "含税金额",
            unit: "CNY",
            data_type: "FLOAT64",
            kind: "METRIC",
            required: false,
            pii_category: "NONE",
          },
        ],
        agg_rules: [],
        derived_rules: [],
        sync_to_knowledge: true,
        pos_x: 860,
        pos_y: 180,
      },
    ],
    edge_types: [
      {
        name: "SUPPLIES",
        src_type_name: "Factory",
        dst_type_name: "Warehouse",
        impact_weight: 0.85,
        cardinality: "ONE_TO_MANY",
        description: "工厂向仓库补货供给",
        properties: [],
        propagation: {
          rules: [
            {
              source_metric: "capacity_units",
              target_metric: "inventory_qty",
              elasticity: 0.8,
            },
          ],
        },
      },
      {
        name: "FULFILLS",
        src_type_name: "Warehouse",
        dst_type_name: "Order",
        impact_weight: 0.9,
        cardinality: "ONE_TO_MANY",
        description: "仓库履约订单",
        properties: [],
        propagation: {
          rules: [
            {
              source_metric: "inventory_qty",
              target_metric: "amount",
              elasticity: 0.6,
            },
          ],
        },
      },
    ],
    mappings: [
      {
        name: "warehouses_to_warehouse",
        dataset: "demo_warehouses",
        node_type: "Warehouse",
        key_column: "warehouse_code",
        title: "{warehouse_name}",
        attributes: { warehouse_name: "warehouse_name" },
        metrics: { inventory_qty: "inventory_qty" },
        geo: { lng_column: null, lat_column: null },
        links: [],
        on_source_delete: "KEEP",
        mode: "FULL",
      },
    ],
  };
}

function cloneSchemaDoc(doc: MockSchemaDoc): MockSchemaDoc {
  return JSON.parse(JSON.stringify(doc)) as MockSchemaDoc;
}

/**
 * Deterministic Phase 2 grounding assertion helper (`08-STEP22` §22.1):
 * 1. Verifies every standalone decimal number in the turn's answer prose matches a rendered cell
 *    value (or explicit row-count header) in the turn's `ResultGrid` when tabular rows exist.
 * 2. Verifies every `[文k]` citation marker satisfies `1 <= k <= citations.length`, and triggering
 *    each marker highlights the corresponding citation card (`axquery-citation-(k-1)`).
 */
async function assertTurnGrounded(turnLocator: Locator): Promise<void> {
  const toggleSqlBtn = turnLocator.getByTestId("axquery-toggle-sql-btn");
  const resultGrid = turnLocator.getByTestId("axquery-result-grid");
  if ((await toggleSqlBtn.count()) > 0 && !(await resultGrid.isVisible())) {
    await toggleSqlBtn.click();
    await expect(resultGrid).toBeVisible();
  }

  const proseBox = turnLocator.locator(".text-sm.leading-relaxed").first();
  await expect(proseBox).toBeVisible();
  const rawProse = await proseBox.innerText();

  // 1. Tabular numeric cell grounding verification (when non-empty data rows are rendered)
  const resultTable = turnLocator.getByTestId("result-table");
  if ((await resultTable.count()) > 0 && (await resultTable.isVisible())) {
    const bodyRows = resultTable.locator("tbody tr");
    const rowCount = await bodyRows.count();
    const hasEmptyPlaceholder =
      rowCount === 1 &&
      (await bodyRows.first().locator("td[colspan]").count()) > 0;

    if (rowCount > 0 && !hasEmptyPlaceholder) {
      const cellNumbers: number[] = [];
      const cellStrings: string[] = [];

      for (let r = 0; r < rowCount; r += 1) {
        const dataCells = bodyRows.nth(r).locator("td:not(:first-child)");
        const cellCount = await dataCells.count();
        for (let c = 0; c < cellCount; c += 1) {
          const text = (await dataCells.nth(c).innerText()).trim();
          cellStrings.push(text);
          const clean = text.replace(/,/g, "");
          if (clean.endsWith("%")) {
            const pctNum = Number(clean.slice(0, -1).trim());
            if (Number.isFinite(pctNum)) {
              cellNumbers.push(pctNum, pctNum / 100);
            }
          } else {
            const parsed = Number(clean);
            if (clean.length > 0 && Number.isFinite(parsed)) {
              cellNumbers.push(parsed);
            }
          }
        }
      }

      // Verify explicit row-count mentions (e.g. fallback summary) match rendered rowCount
      const rowCountMentions = Array.from(
        rawProse.matchAll(/(?:查询返回|共)\s*(\d+)\s*行/g)
      );
      for (const match of rowCountMentions) {
        expect(Number(match[1])).toBe(rowCount);
      }

      const strippedProse = rawProse
        .replace(/\[\s*(?:文|Doc\s*)\d+\s*\]/g, " ")
        .replace(/\[\s*(?:表|Table)\s*\]/g, " ")
        .replace(/(?:查询返回|共)\s*\d+\s*行/g, " ")
        .replace(/\b\d{4}-\d{2}(?:-\d{2})?\b/g, " ")
        .replace(/\b(?:19|20)\d{2}\b/g, " ")
        .replace(/\b[A-Za-z_][A-Za-z0-9_]*-\d+\b/g, " ");

      const proseNumberMatches = Array.from(
        strippedProse.matchAll(
          /(?<![A-Za-z0-9_])-?\d{1,3}(?:,\d{3})+(?:\.\d+)?%?|-?\d+(?:\.\d+)?%?(?![A-Za-z0-9_])/g
        )
      );

      for (const m of proseNumberMatches) {
        const rawToken = m[0];
        const isPercent = rawToken.endsWith("%");
        const numericStr = (isPercent ? rawToken.slice(0, -1) : rawToken).replace(
          /,/g,
          ""
        );
        const val = Number(numericStr);
        if (!Number.isFinite(val)) {
          continue;
        }
        const approxMatch = cellNumbers.some((cellVal) => {
          if (Math.abs(val - cellVal) <= 1e-6) return true;
          const rounded2 = Math.round(cellVal * 100) / 100;
          if (Math.abs(val - rounded2) <= 1e-6) return true;
          if (isPercent && Math.abs(val - cellVal * 100) <= 1e-6) return true;
          return false;
        });
        const verbatimCellMatch =
          !isPercent &&
          !numericStr.includes(".") &&
          cellStrings.some((s) => s.split(/\D+/).includes(numericStr));

        expect(
          approxMatch || verbatimCellMatch,
          `Ungrounded prose number "${rawToken}" in answer "${rawProse}" vs cells [${cellStrings.join(", ")}]`
        ).toBe(true);
      }
    }
  }

  // 2. Citation marker bounds & interactive highlight verification
  const citationCards = turnLocator.locator('[data-testid^="axquery-citation-"]');
  const citationCount = await citationCards.count();
  const markerMatches = Array.from(
    rawProse.matchAll(/\[\s*(?:文|Doc\s*)(\d+)\s*\]/g)
  );
  for (const m of markerMatches) {
    const k = Number(m[1]);
    expect(
      k >= 1 && k <= citationCount,
      `Citation marker [文${k}] out of bounds (citations.length = ${citationCount})`
    ).toBe(true);
  }

  const citationButtons = proseBox.locator("button");
  const btnCount = await citationButtons.count();
  for (let i = 0; i < btnCount; i += 1) {
    const btn = citationButtons.nth(i);
    const label = (await btn.innerText()).trim();
    const citeBtnMatch = /^\[\s*(?:文|Doc\s*)(\d+)\s*\]$/.exec(label);
    if (!citeBtnMatch) {
      continue;
    }
    const k = Number(citeBtnMatch[1]);
    expect(k).toBeGreaterThanOrEqual(1);
    expect(k).toBeLessThanOrEqual(citationCount);

    await btn.click();
    const targetCard = turnLocator.getByTestId(`axquery-citation-${k - 1}`);
    await expect(targetCard).toBeVisible();
    await expect(targetCard).toHaveClass(/border-xblue/);
  }
}

async function installV2JourneyMocks(
  page: Page,
  state: V2JourneyMockState,
  leakedTenantUrls: string[]
): Promise<void> {
  const nowIso = "2026-08-22T08:00:00Z";
  const adminUser = {
    id: OPS_ADMIN_ID,
    tenant_id: "default",
    username: "ops_admin",
    display_name: "Ops Admin",
    email: "ops@axpoint.local",
    department: "east",
    roles: ["ADMIN"],
    permissions: [
      "DATA_READ",
      "DATA_WRITE",
      "STUDIO_EDIT",
      "STUDIO_PUBLISH",
      "FLOW_EDIT",
      "FLOW_RUN",
      "SIM_RUN",
      "SIM_APPLY",
      "VIEW_EDIT",
      "VIEW_PUBLISH",
      "AGENT_EDIT",
      "AGENT_RUN",
      "ADMIN_MANAGE",
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
          trace_id: "v2-journey-400",
        }),
      });
      return;
    }

    // Enforce BUILDER PAT isolation on production publish / SIM_APPLY endpoints
    const authHeader = req.headers()["authorization"] ?? "";
    const bearerTok = authHeader.startsWith("Bearer ")
      ? authHeader.slice("Bearer ".length).trim()
      : "";
    const matchedPat = state.issuedTokens.find((t) => t.token === bearerTok);
    if (matchedPat && matchedPat.token_profile === "BUILDER") {
      if (
        pathname.startsWith("/api/axops/templates/") ||
        pathname.endsWith("/publish") ||
        (pathname === "/api/xblue/schema" && method === "POST")
      ) {
        await route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "FORBIDDEN",
            message: "BUILDER token cannot mutate production endpoints",
            trace_id: "v2-builder-403",
          }),
        });
        return;
      }
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
          embedding_dim: 1024,
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
          para_count: 14,
          entity_count: 18,
          edge_count: 24,
          embedding_count: 14,
          embedding_dims: [1024],
        }),
      });
      return;
    }

    if (pathname === "/api/topology") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: [
            {
              id: POLICY_DOC_ID,
              node_type: "DOC",
              label: "补货审批制度",
              weight: 1,
            },
          ],
          edges: [],
        }),
      });
      return;
    }

    if (pathname === "/api/auth/config" && method === "GET") {
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
      state.hasSession = true;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: buildMockJwt("login"),
          token_type: "Bearer",
          expires_in: 900,
          user: adminUser,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/refresh" && method === "POST") {
      if (!state.hasSession) {
        await route.fulfill({
          status: 401,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "UNAUTHENTICATED",
            message: "No refresh cookie",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: buildMockJwt("refresh"),
          token_type: "Bearer",
          expires_in: 900,
          user: adminUser,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/me" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(adminUser),
      });
      return;
    }

    if (pathname === "/api/auth/tokens" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.issuedTokens.map((t) => ({
            id: t.id,
            name: t.name,
            token_prefix: t.token.slice(0, 12),
            token_profile: t.token_profile,
            permissions: t.permissions,
            expires_at: null,
            last_used_at: null,
            created_at: t.created_at,
          })),
        }),
      });
      return;
    }

    if (pathname === "/api/auth/tokens" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        name?: string;
        token_profile?: string;
        permissions?: string[];
      };
      const idx = state.issuedTokens.length + 1;
      const id = `00000000-0000-0000-0000-00000000230${idx}`;
      const profile = body.token_profile ?? "CUSTOM";
      const permissions =
        profile === "BUILDER"
          ? (body.permissions ?? ["STUDIO_EDIT", "VIEW_EDIT"]).filter(
              (p) => p !== "STUDIO_PUBLISH" && p !== "SIM_APPLY"
            )
          : (body.permissions ?? []);
      const plain = `axp_pat_builder_step22_${idx}`;
      state.issuedTokens.push({
        id,
        name: body.name ?? `token-${idx}`,
        token: plain,
        token_profile: profile,
        permissions,
        created_at: nowIso,
      });
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id,
          name: body.name ?? `token-${idx}`,
          token: plain,
          token_prefix: plain.slice(0, 12),
          token_profile: profile,
          permissions,
          expires_at: null,
          created_at: nowIso,
          mcp_config_snippet: {
            mcpServers: {
              axbuilder: {
                url: `${url.origin}/api/mcp`,
                headers: { Authorization: `Bearer ${plain}` },
              },
            },
          },
        }),
      });
      return;
    }

    // Stage 1: Onboarding & Template Hub
    if (pathname === "/api/axops/onboarding-status" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          completed_steps: 3,
          total_steps: 5,
          progress_pct: 60,
          recommended_next_step: "SIM_VERIFICATION",
          has_trial_subgraph: Boolean(state.trialSubgraphSlug),
          steps: [
            {
              step_no: 1,
              step_key: "DATA_INGEST",
              label_zh: "接入数据",
              label_en: "Connect Data",
              completed: true,
              count_primary: 2,
              count_secondary: 1,
              metric_label: "2 表 · 1 规程",
              target_path: "/hub",
            },
            {
              step_no: 2,
              step_key: "NODE_MODELING",
              label_zh: "节点建模",
              label_en: "Node Modeling",
              completed: true,
              count_primary: 4,
              count_secondary: 2,
              metric_label: "4 实体 · 2 关系",
              target_path: "/studio",
            },
            {
              step_no: 3,
              step_key: "MAPPING_AND_FLOW",
              label_zh: "映射入图",
              label_en: "Map & Sync",
              completed: true,
              count_primary: 3,
              count_secondary: 2,
              metric_label: "3 节点",
              target_path: "/flow",
            },
            {
              step_no: 4,
              step_key: "SIM_VERIFICATION",
              label_zh: "沙盘推演",
              label_en: "Simulate",
              completed: state.sandboxes.length > 0,
              count_primary: state.sandboxes.length,
              count_secondary: 0,
              metric_label: `${state.sandboxes.length} 沙盘`,
              target_path: "/sim",
            },
            {
              step_no: 5,
              step_key: "COCKPIT_AND_AGENT",
              label_zh: "驾驶舱与智能体",
              label_en: "Cockpit & Agent",
              completed: state.dashboards.some((d) => d.status === "PUBLISHED"),
              count_primary: state.dashboards.length,
              count_secondary: 1,
              metric_label: `${state.dashboards.length} 看板`,
              target_path: "/view",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axops/templates" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "supply_chain",
              name: "供应链产销协同包",
              version: "2.0.0",
              industry: "制造业供应链",
              description: "覆盖供应商、工厂、区域仓与客户订单四级拓扑。",
              contents: {
                node_types: 4,
                edge_types: 2,
                metrics: 5,
                dashboards: 1,
                agents: 1,
              },
              sha256: {},
            },
          ],
        }),
      });
      return;
    }

    if (
      pathname === "/api/axops/templates/supply_chain/preview" &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          manifest: {
            id: "supply_chain",
            name: "供应链产销协同包",
            version: "2.0.0",
            industry: "制造业供应链",
            description: "覆盖供应商、工厂、区域仓与客户订单四级拓扑。",
            contents: {
              node_types: 4,
              edge_types: 2,
              metrics: 5,
              dashboards: 1,
              agents: 1,
            },
            sha256: {},
          },
          readme_markdown: "# 供应链产销协同模板",
          topology_summary: {
            node_types: [
              {
                name: "Factory",
                display_name: "工厂",
                property_count: 2,
                metric_count: 1,
              },
              {
                name: "Warehouse",
                display_name: "仓库",
                property_count: 4,
                metric_count: 3,
              },
            ],
            edge_types: [
              {
                name: "SUPPLIES",
                src_type_name: "Factory",
                dst_type_name: "Warehouse",
                cardinality: "ONE_TO_MANY",
              },
            ],
            agg_rule_count: 0,
            derived_rule_count: 1,
          },
          sample_metrics: [
            {
              name: "paid_amount",
              display_name: "已支付订单金额",
              unit: "CNY",
            },
          ],
          dashboard_previews: [
            {
              slug: "supply-chain-cockpit",
              name: "供应链全景驾驶舱",
              widget_count: 5,
            },
          ],
          sample_rows: [],
          agents: [
            {
              name: "supply_chain_copilot",
              description: "供应链履约分析智能体",
              tool_count: 2,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/subgraphs/from-template" && method === "POST") {
      const slug = "trial_supply_chain_2201";
      state.trialSubgraphSlug = slug;
      state.trialDraftDoc = buildInitialSupplyChainDoc();
      state.prePatchDoc = null;
      state.snapshots = [];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: slug,
          display_name: "演练 · 供应链产销协同包",
          is_dev_sandbox: true,
          source_template_id: "supply_chain",
          draft_id: TRIAL_DRAFT_ID,
          nodes_materialized: 6,
          links_materialized: 5,
          studio_url: `/studio?subgraph=${slug}&draft=${TRIAL_DRAFT_ID}`,
        }),
      });
      return;
    }

    // Stage 2: xBlueStudio Subgraphs, Drafts, Copilot, Snapshots, Lineage & Spec Doc
    if (pathname === "/api/xblue/subgraphs" && method === "GET") {
      const items: Array<Record<string, unknown>> = [
        {
          id: "00000000-0000-0000-0000-000000000000",
          slug: "main",
          display_name: "企业主图",
          description: "生产权威主图",
          owner_dept: "default",
          is_main: true,
          is_dev_sandbox: false,
          source_template_id: null,
          current_version: 1,
          created_at: nowIso,
          updated_at: nowIso,
        },
      ];
      if (state.trialSubgraphSlug) {
        items.push({
          id: "00000000-0000-0000-0000-000000002219",
          slug: state.trialSubgraphSlug,
          display_name: "演练 · 供应链产销协同包",
          description: "模板克隆隔离子图",
          owner_dept: "default",
          is_main: false,
          is_dev_sandbox: true,
          source_template_id: "supply_chain",
          current_version: state.trialBaseVersion,
          created_at: nowIso,
          updated_at: nowIso,
        });
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items }),
      });
      return;
    }

    if (pathname === "/api/xblue/nodes" && method === "GET") {
      const sg = searchParams.get("subgraph") ?? "main";
      const q = (searchParams.get("q") ?? "").toLowerCase();
      const mainNodes = [
        {
          id: FACTORY_F1_ID,
          node_key: "F-1",
          type_name: "Factory",
          title: "苏州一厂",
          attributes: { city: "苏州" },
          metrics: { capacity_units: 1200 },
          subgraph_slug: "main",
          updated_at: nowIso,
        },
        {
          id: WAREHOUSE_W1_ID,
          node_key: "W-1",
          type_name: "Warehouse",
          title: "华东仓",
          attributes: { city: "上海" },
          metrics: { inventory_qty: 800 },
          subgraph_slug: "main",
          updated_at: nowIso,
        },
        {
          id: ORDER_O1_ID,
          node_key: "O-1",
          type_name: "Order",
          title: "华东急单 O-1",
          attributes: {},
          metrics: { amount: 33239.25 },
          subgraph_slug: "main",
          updated_at: nowIso,
        },
      ];
      const pool =
        sg === "main"
          ? mainNodes
          : mainNodes.map((n, idx) => ({
              ...n,
              id: `00000000-0000-0000-0000-00000000229${idx}`,
              subgraph_slug: sg,
            }));
      const filtered = q
        ? pool.filter(
            (n) =>
              n.node_key.toLowerCase().includes(q) ||
              n.title.toLowerCase().includes(q)
          )
        : pool;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: filtered,
          total: filtered.length,
          next_cursor: null,
        }),
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
          version: sg === state.trialSubgraphSlug ? state.trialBaseVersion : 1,
          ...buildInitialSupplyChainDoc(),
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/validate" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ valid: true, issues: [], pass_count: 14 }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/versions" && method === "GET") {
      const sg = searchParams.get("subgraph") ?? "main";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000002218",
              subgraph_slug: sg,
              version: 1,
              change_summary: "初始模板基线",
              published_at: nowIso,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/drafts" && method === "GET") {
      const sg = searchParams.get("subgraph") ?? "main";
      const items =
        sg === state.trialSubgraphSlug
          ? [
              {
                id: TRIAL_DRAFT_ID,
                subgraph_slug: sg,
                title: "供应链演练草案",
                source: "TEMPLATE",
                base_version: state.trialBaseVersion,
                schema_doc: state.trialDraftDoc,
                ai_context: {},
                issues: [],
                pass_count: 14,
                status: "OPEN",
                created_at: nowIso,
                updated_at: nowIso,
              },
            ]
          : [];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items }),
      });
      return;
    }

    if (
      pathname === `/api/xblue/drafts/${TRIAL_DRAFT_ID}` &&
      (method === "GET" || method === "PUT")
    ) {
      if (method === "PUT") {
        const body = (req.postDataJSON() ?? {}) as {
          schema_doc?: MockSchemaDoc;
        };
        if (body.schema_doc) {
          state.trialDraftDoc = body.schema_doc;
        }
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: TRIAL_DRAFT_ID,
          subgraph_slug: state.trialSubgraphSlug ?? "trial_supply_chain_2201",
          title: "供应链演练草案",
          source: "TEMPLATE",
          base_version: state.trialBaseVersion,
          schema_doc: state.trialDraftDoc,
          ai_context: {},
          issues: [],
          pass_count: 15,
          status: "OPEN",
          created_at: nowIso,
          updated_at: nowIso,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblue/drafts/${TRIAL_DRAFT_ID}/snapshots` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.snapshots }),
      });
      return;
    }

    if (
      pathname === `/api/xblue/drafts/${TRIAL_DRAFT_ID}/copilot` &&
      method === "POST"
    ) {
      const body = (req.postDataJSON() ?? {}) as {
        mode?: string;
        prompt?: string;
      };
      if (body.mode === "DIAGNOSE_ONLY") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            mode: "DIAGNOSE_ONLY",
            mutated: false,
            snapshot_id: null,
            snapshot_no: null,
            diagnosis: {
              summary:
                "Warehouse 节点缺少安全库存基准指标 safety_stock，建议补充度量定义。",
              findings: [
                {
                  severity: "WARN",
                  target_path: "node_types[Warehouse].properties",
                  rationale: "缺少安全库存基准指标 safety_stock。",
                  suggested_fix: "新增 safety_stock（单位：件）指标。",
                },
              ],
            },
            diff: null,
            pass_count: 14,
            issues: [],
          }),
        });
        return;
      }

      state.prePatchDoc = cloneSchemaDoc(state.trialDraftDoc);
      const nextNo = state.snapshots.length + 1;
      const snapId = nextNo === 1 ? SNAPSHOT_1_ID : SNAPSHOT_2_ID;
      state.snapshots.unshift({
        id: snapId,
        draft_id: TRIAL_DRAFT_ID,
        snapshot_no: nextNo,
        reason: "COPILOT_PATCH",
        created_at: nowIso,
      });

      const wh = state.trialDraftDoc.node_types.find(
        (nt) => nt.name === "Warehouse"
      );
      if (wh && !wh.properties.some((p) => p.name === "safety_stock")) {
        wh.properties.push({
          name: "safety_stock",
          display_name: "安全库存",
          description: "区域仓安全库存基准水位",
          unit: "件",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
          maturity: "PROPOSED",
        });
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mode: "PATCH_DRAFT",
          mutated: true,
          snapshot_id: snapId,
          snapshot_no: nextNo,
          diagnosis: {
            summary: "已为 Warehouse 增量生成安全库存指标 safety_stock 提案。",
            findings: [
              {
                severity: "INFO",
                target_path: "node_types[Warehouse].properties[safety_stock]",
                rationale: "新增安全库存度量字段并标记为待确认提案。",
                suggested_fix: "审阅定义后确认提案。",
              },
            ],
          },
          diff: {
            node_types: {
              added: [],
              removed: [],
              changed: [
                {
                  name: "Warehouse",
                  fields: [
                    {
                      path: "properties.safety_stock",
                      before: null,
                      after: "FLOAT64",
                    },
                  ],
                },
              ],
            },
            edge_types: { added: [], removed: [], changed: [] },
            mappings: { added: [], removed: [], changed: [] },
          },
          pass_count: 15,
          issues: [],
        }),
      });
      return;
    }

    const snapRestoreMatch =
      /^\/api\/xblue\/drafts\/([^/]+)\/snapshots\/([^/]+)\/restore$/.exec(
        pathname
      );
    if (snapRestoreMatch && method === "POST") {
      if (state.prePatchDoc) {
        state.trialDraftDoc = cloneSchemaDoc(state.prePatchDoc);
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: TRIAL_DRAFT_ID,
          subgraph_slug: state.trialSubgraphSlug ?? "trial_supply_chain_2201",
          title: "供应链演练草案",
          source: "TEMPLATE",
          base_version: state.trialBaseVersion,
          schema_doc: state.trialDraftDoc,
          ai_context: {},
          issues: [],
          pass_count: 14,
          status: "OPEN",
          created_at: nowIso,
          updated_at: nowIso,
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblue/drafts/${TRIAL_DRAFT_ID}/confirm-proposals` &&
      method === "POST"
    ) {
      for (const nt of state.trialDraftDoc.node_types) {
        for (const p of nt.properties) {
          p.maturity = "CONFIRMED";
        }
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: TRIAL_DRAFT_ID,
          subgraph_slug: state.trialSubgraphSlug ?? "trial_supply_chain_2201",
          title: "供应链演练草案",
          source: "TEMPLATE",
          base_version: state.trialBaseVersion,
          schema_doc: state.trialDraftDoc,
          ai_context: {},
          issues: [],
          pass_count: 15,
          status: "OPEN",
          created_at: nowIso,
          updated_at: nowIso,
        }),
      });
      return;
    }

    const lineageMatch = /^\/api\/xblue\/subgraphs\/([^/]+)\/lineage\/([^/]+)$/.exec(
      pathname
    );
    if (lineageMatch && method === "GET") {
      const sg = decodeURIComponent(lineageMatch[1]);
      const nodeType = decodeURIComponent(lineageMatch[2]);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: sg,
          node_type: nodeType,
          display_name: "仓库",
          badge_counts: {
            property_count: 1,
            metric_count: 4,
            rule_count: 1,
          },
          stages: [
            {
              stage: "DATASET_COLUMNS",
              nodes: [
                {
                  id: "col_inventory",
                  label: "demo_warehouses.inventory_qty",
                  meta: {},
                },
              ],
            },
            {
              stage: "MAPPING_SPEC",
              nodes: [
                {
                  id: "map_wh",
                  label: "warehouses_to_warehouse",
                  meta: {},
                },
              ],
            },
            {
              stage: "NODE_SCHEMA",
              nodes: [
                {
                  id: "prop_safety_stock",
                  label: "Warehouse.safety_stock",
                  meta: {},
                },
              ],
            },
            {
              stage: "COMPUTE_RULES",
              nodes: [
                {
                  id: "rule_fill_rate",
                  label: "fill_rate = inventory_qty / demand_qty",
                  meta: {},
                },
              ],
            },
            {
              stage: "DOWNSTREAM_CONSUMERS",
              nodes: [
                {
                  id: "consumer_cockpit",
                  label: "supply-chain-cockpit",
                  meta: {},
                },
              ],
            },
          ],
          edges: [
            { from: "col_inventory", to: "map_wh", label: "映射" },
            { from: "map_wh", to: "prop_safety_stock", label: "写入" },
            { from: "prop_safety_stock", to: "rule_fill_rate", label: "派生" },
            {
              from: "rule_fill_rate",
              to: "consumer_cockpit",
              label: "驱动看板",
            },
          ],
        }),
      });
      return;
    }

    const specMatch = /^\/api\/xblue\/subgraphs\/([^/]+)\/spec-doc$/.exec(
      pathname
    );
    if (specMatch && method === "GET") {
      const sg = decodeURIComponent(specMatch[1]);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: sg,
          display_name: "演练 · 供应链产销协同包",
          version: state.trialBaseVersion,
          draft_id: TRIAL_DRAFT_ID,
          pass_count: 15,
          unconfirmed_count: 0,
          sections: [
            {
              key: "sec_01_overview",
              title: "模型总览与健康度",
              summary: "4 类实体、2 类关系。",
              rows: [{ item: "节点类型", count: 4 }],
            },
            {
              key: "sec_02_semantic_layers",
              title: "业务域与语义分层",
              summary: "履约与制造双域。",
              rows: [{ domain: "履约域", entities: "Warehouse, Order" }],
            },
            {
              key: "sec_03_entity_dictionary",
              title: "实体属性与指标字典",
              summary: "包含 Warehouse 与新增指标 safety_stock。",
              rows: [
                {
                  entity: "Warehouse",
                  field: "safety_stock",
                  display_name: "安全库存",
                  unit: "件",
                },
              ],
            },
            {
              key: "sec_04_topology_matrix",
              title: "拓扑关系与因果传导",
              summary: "Factory -> Warehouse -> Order。",
              rows: [{ edge: "SUPPLIES", route: "Factory -> Warehouse" }],
            },
            {
              key: "sec_05_compute_formulas",
              title: "聚合与派生公式",
              summary: "订单履约率公式。",
              rows: [{ metric: "fill_rate", expr: "inventory_qty / demand_qty" }],
            },
            {
              key: "sec_06_source_mappings",
              title: "数据湖映射规则",
              summary: "仓库主数据入图映射。",
              rows: [{ mapping: "warehouses_to_warehouse", dataset: "demo_warehouses" }],
            },
            {
              key: "sec_07_acceptance_prompts",
              title: "推荐验收问数集",
              summary: "6 条标准零幻觉验收问题。",
              rows: [{ id: "q1", question: "各仓库当前可用库存量与安全库存对比" }],
            },
          ],
          acceptance_prompts: [
            {
              id: "q1",
              question: "各仓库当前可用库存量与安全库存对比",
              expected_grounding_mode: "DOMAIN_OBJECT",
              target_node_types: ["Warehouse"],
              target_metrics: ["inventory_qty", "safety_stock"],
            },
          ],
          markdown: "# 供应链演练规格书",
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/entity-candidates" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    // Stage 4: AxSim Sandboxes & Scope Preflight
    if (pathname === "/api/axsim/sandboxes" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.sandboxes }),
      });
      return;
    }

    if (pathname === "/api/axsim/sandboxes/preflight" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        scope?: { type_names?: string[] };
      };
      const types = body.scope?.type_names ?? [];
      const isFactoryOnly =
        types.length > 0 &&
        types.includes("Factory") &&
        (!types.includes("Warehouse") || !types.includes("Order"));

      if (isFactoryOnly) {
        const missing = ["Warehouse", "Order"].filter(
          (t) => !types.includes(t)
        );
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            readiness_grade: "DEGRADED",
            estimated_nodes: 1,
            estimated_links: 0,
            causal_edge_coverage_ratio: 0,
            agg_rule_closure_ratio: 0.4,
            isolated_node_ratio: 1,
            missing_upstream_types: missing,
            truncated_edge_types: [
              {
                link_type: "SUPPLIES",
                in_scope_count: 0,
                boundary_dropped_count: 1,
                excluded_neighbor_type: missing[0] ?? "Warehouse",
                has_causal_rule: true,
                has_agg_rule: false,
              },
            ],
            reason_codes: ["CAUSAL_BOUNDARY_TRUNCATED"],
          }),
        });
        return;
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          readiness_grade: "READY",
          estimated_nodes: 3,
          estimated_links: 2,
          causal_edge_coverage_ratio: 1.0,
          agg_rule_closure_ratio: 1.0,
          isolated_node_ratio: 0,
          missing_upstream_types: [],
          truncated_edge_types: [],
          reason_codes: [],
        }),
      });
      return;
    }

    if (pathname === "/api/axsim/sandboxes" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        name?: string;
        scope?: Record<string, unknown>;
      };
      const record = {
        id: SANDBOX_ID,
        name: body.name ?? "e2e-step22-preflight-sbx",
        status: "READY",
        node_count: 3,
        link_count: 2,
        scope: body.scope ?? {},
        base_captured_at: nowIso,
        schema_version: 1,
      };
      state.sandboxes = [record];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(record),
      });
      return;
    }

    if (pathname.startsWith(`/api/axsim/sandboxes/${SANDBOX_ID}/`)) {
      if (pathname.endsWith("/topology")) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            nodes: [
              {
                id: FACTORY_F1_ID,
                node_key: "F-1",
                type_name: "Factory",
                title: "苏州一厂",
                metrics: { capacity_units: 1200 },
              },
              {
                id: WAREHOUSE_W1_ID,
                node_key: "W-1",
                type_name: "Warehouse",
                title: "华东仓",
                metrics: { inventory_qty: 800 },
              },
            ],
            links: [
              {
                id: "00000000-0000-0000-0000-000000002239",
                link_type: "SUPPLIES",
                from_id: FACTORY_F1_ID,
                to_id: WAREHOUSE_W1_ID,
                weight: 1,
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
      return;
    }

    // Stage 5: xBlueView Five-Zone Cockpit & Publish Preflight
    if (pathname === "/api/xblueview/dashboards" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.dashboards.map(({ widgets: _w, ...rest }) => rest),
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/dashboards" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        name?: string;
        slug?: string;
        description?: string;
        zone_layout?: string;
      };
      const dash = {
        id: DASHBOARD_ID,
        tenant_id: "default",
        name: body.name ?? "Step22 五区驾驶舱",
        slug: body.slug ?? "e2e-step22-cockpit",
        description: body.description ?? "",
        filters: [],
        theme: "light",
        canvas: {
          columns: 12,
          row_height: 72,
          gap: 12,
          zone_layout: body.zone_layout ?? "STANDARD_FIVE_ZONE",
        },
        status: "DRAFT",
        version: 1,
        created_by: OPS_ADMIN_ID,
        created_at: nowIso,
        updated_at: nowIso,
        widgets: [] as MockWidgetRecord[],
      };
      state.dashboards.push(dash);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(dash),
      });
      return;
    }

    if (pathname === "/api/xblueview/assets" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/xblueview/exports" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    const dashBySlugMatch = /^\/api\/xblueview\/dashboards\/by-slug\/([^/]+)$/.exec(
      pathname
    );
    if (dashBySlugMatch && method === "GET") {
      const slug = decodeURIComponent(dashBySlugMatch[1]);
      const found = state.dashboards.find((d) => d.slug === slug);
      await route.fulfill({
        status: found ? 200 : 404,
        contentType: "application/json",
        body: JSON.stringify(found ?? { error_code: "NOT_FOUND" }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}` &&
      method === "GET"
    ) {
      const found = state.dashboards.find((d) => d.id === DASHBOARD_ID);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(found),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}/widgets` &&
      method === "POST"
    ) {
      const body = (req.postDataJSON() ?? {}) as {
        widget_kind?: string;
        title?: string;
        position?: MockWidgetRecord["position"];
        data_source?: Record<string, unknown>;
      };
      const found = state.dashboards.find((d) => d.id === DASHBOARD_ID);
      const widget: MockWidgetRecord = {
        id: BROKEN_WIDGET_ID,
        tenant_id: "default",
        dashboard_id: DASHBOARD_ID,
        widget_kind: body.widget_kind ?? "ACTION_BUTTON",
        title: body.title ?? "未配置调拨动作",
        position: body.position ?? {
          x: 0,
          y: 13,
          w: 4,
          h: 3,
          semantic_zone: "DECISION",
        },
        data_source: body.data_source ?? { type: "ACTION", flow_id: null },
        encoding: {},
        options: {},
        drill: [],
        refresh_mode: "MANUAL",
        refresh_seconds: 0,
        deps: { datasets: [], metrics: [], sandboxes: [] },
        created_at: nowIso,
        updated_at: nowIso,
      };
      found?.widgets.push(widget);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(widget),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${BROKEN_WIDGET_ID}` &&
      method === "DELETE"
    ) {
      const found = state.dashboards.find((d) => d.id === DASHBOARD_ID);
      if (found) {
        found.widgets = found.widgets.filter((w) => w.id !== BROKEN_WIDGET_ID);
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ deleted: true }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/widgets/${BROKEN_WIDGET_ID}/data` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ columns: [], rows: [], truncated: false }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}/preflight` &&
      method === "POST"
    ) {
      const found = state.dashboards.find((d) => d.id === DASHBOARD_ID);
      const broken = found?.widgets.find(
        (w) =>
          w.widget_kind === "ACTION_BUTTON" &&
          !w.data_source.flow_id &&
          !w.data_source.endpoint_id &&
          !w.data_source.scenario_id
      );
      if (broken) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            dashboard_id: DASHBOARD_ID,
            status: "BLOCKED",
            checked_widgets: found?.widgets.length ?? 1,
            blocking_count: 1,
            warning_count: 0,
            issues: [
              {
                widget_id: broken.id,
                widget_title: broken.title,
                semantic_zone: broken.position.semantic_zone ?? "DECISION",
                severity: "BLOCKING",
                code: "UNCONFIGURED_ACTION",
                message: "动作按钮缺少绑定的作业流、端点或推演预案目标。",
              },
            ],
          }),
        });
        return;
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          dashboard_id: DASHBOARD_ID,
          status: "PASS",
          checked_widgets: found?.widgets.length ?? 0,
          blocking_count: 0,
          warning_count: 0,
          issues: [],
        }),
      });
      return;
    }

    if (
      pathname === `/api/xblueview/dashboards/${DASHBOARD_ID}/publish` &&
      method === "POST"
    ) {
      const found = state.dashboards.find((d) => d.id === DASHBOARD_ID);
      if (found) {
        found.status = "PUBLISHED";
        found.version += 1;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(found),
      });
      return;
    }

    // Stage 6: AxHub Datasets, Documents & Scoped Grounded AxQuery
    if (pathname === "/api/axhub/datasets" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DEMO_ORDERS_DS_ID,
              name: "demo_orders",
              display_name: "演示订单明细表",
              source_kind: "SQL_TABLE",
              status: "READY",
              row_count: 12,
              created_at: nowIso,
              updated_at: nowIso,
            },
            {
              id: ISOLATED_VAULT_DS_ID,
              name: "finance_secret_vault",
              display_name: "财务隔离核算表",
              source_kind: "FILE_TABLE",
              status: "READY",
              row_count: 4,
              created_at: nowIso,
              updated_at: nowIso,
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/documents" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: POLICY_DOC_ID,
              doc_id: POLICY_DOC_ID,
              title: "补货审批制度",
              source_name: "replenishment_policy_step22.md",
              para_count: 2,
              created_at: nowIso,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/metrics" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000002270",
              name: "paid_amount",
              display_name: "已支付订单金额",
              dataset_id: DEMO_ORDERS_DS_ID,
              expression: "SUM(amount)",
              certified: true,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/saved-queries" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/axflow/flows" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/chat/query" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        query?: string;
        scope?: {
          dataset_ids?: string[];
          doc_ids?: string[];
          subgraph_slug?: string | null;
        };
      };
      const q = body.query ?? "";
      const appliedScope = {
        dataset_ids: body.scope?.dataset_ids ?? [],
        doc_ids: body.scope?.doc_ids ?? [],
        subgraph_slug: body.scope?.subgraph_slug ?? null,
        endpoint_slugs: [],
      };

      if (q.includes("finance_secret_vault")) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            turn_id: "00000000-0000-0000-0000-000000002283",
            session_id: "00000000-0000-0000-0000-000000002280",
            intent: "DATA",
            answer:
              "当前会话作用域未包含目标数据表，无法在已授权范围内检索到匹配依据。",
            sql: "",
            columns: [],
            rows: [],
            citations: [],
            hits: [],
            related_nodes: [],
            seed_entities: [],
            grounding_mode: "ABSTAINED",
            verified_numbers: 0,
            verified_citations: 0,
            abstained: true,
            applied_scope: appliedScope,
            elapsed_ms: 9,
          }),
        });
        return;
      }

      if (q.includes("补货审批制度")) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            turn_id: "00000000-0000-0000-0000-000000002282",
            session_id: "00000000-0000-0000-0000-000000002280",
            intent: "DOC",
            answer:
              "根据《补货审批制度》规定，当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程 [文1]。",
            sql: "",
            columns: [],
            rows: [],
            citations: [
              {
                para_id: POLICY_PARA_ID,
                doc_id: POLICY_DOC_ID,
                doc_title: "补货审批制度",
                snippet:
                  "第一条 补货审批制度触发条件：当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程。",
              },
            ],
            hits: [],
            related_nodes: [],
            seed_entities: ["区域仓", "补货审批制度"],
            grounding_mode: "GROUNDED_DOC",
            verified_numbers: 2,
            verified_citations: 1,
            abstained: false,
            applied_scope: appliedScope,
            elapsed_ms: 14,
          }),
        });
        return;
      }

      if (q.includes("按状态统计已支付订单金额")) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            turn_id: "00000000-0000-0000-0000-000000002284",
            session_id: "00000000-0000-0000-0000-000000002280",
            intent: "DATA",
            answer: "查询返回 2 行，主要结果见表格",
            sql: "SELECT DATE_TRUNC('month', order_date) AS month, status, SUM(amount) AS paid_amount FROM demo_orders WHERE status = 'PAID' GROUP BY 1, 2",
            columns: [
              {
                name: "status",
                data_type: "STRING",
                masked: false,
                pii_category: "NONE",
              },
              {
                name: "paid_amount",
                data_type: "FLOAT64",
                masked: false,
                pii_category: "NONE",
              },
            ],
            rows: [
              { status: "PAID", paid_amount: 21239.25 },
              { status: "PAID", paid_amount: 12000.0 },
            ],
            citations: [],
            hits: [],
            related_nodes: [],
            seed_entities: [],
            grounding_mode: "SEMANTIC_METRIC",
            slot_payload: {
              route: "SINGLE_METRIC",
              metric_names: ["paid_amount"],
              dimensions: ["status"],
              grain: "MONTH",
            },
            verified_numbers: 0,
            verified_citations: 0,
            abstained: false,
            applied_scope: appliedScope,
            elapsed_ms: 12,
          }),
        });
        return;
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          turn_id: "00000000-0000-0000-0000-000000002281",
          session_id: "00000000-0000-0000-0000-000000002280",
          intent: "DATA",
          answer: "已支付订单的总金额为 33,239.25 元。",
          sql: "SELECT SUM(amount) AS paid_amount FROM demo_orders WHERE status = 'PAID'",
          columns: [
            {
              name: "paid_amount",
              data_type: "FLOAT64",
              masked: false,
              pii_category: "NONE",
            },
          ],
          rows: [{ paid_amount: 33239.25 }],
          citations: [],
          hits: [],
          related_nodes: [],
          seed_entities: [],
          grounding_mode: "GUARDED_SQL",
          verified_numbers: 1,
          verified_citations: 0,
          abstained: false,
          applied_scope: appliedScope,
          elapsed_ms: 11,
        }),
      });
      return;
    }

    // Stage 3 & 7: AxAgent Providers, Agents, Endpoints, MCP & IM Connectors
    if (pathname === "/api/axagent/providers" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/axagent/providers/preferences" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ preferences: [] }),
      });
      return;
    }

    if (pathname === "/api/axagent/usage" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/axagent/endpoints" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/axagent/mcp-clients" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/axagent/conversations" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    if (pathname === "/api/axagent/agents" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: AGENT_ID,
              tenant_id: "default",
              name: "supply_chain_im_assistant",
              description: "供应链 IM 问答助手",
              version: 1,
              spec: {
                system_prompt: "供应链经营数据助手",
                provider_purpose: "AGENT",
                provider_id: null,
                temperature: 0,
                max_steps: 4,
                memory_turns: 6,
                tools: [{ kind: "BUILTIN", name: "ask_analytics" }],
                approval_required_tools: [],
                knowledge_scope: { doc_title_prefix: null, dataset_names: [] },
                output_schema: null,
              },
              created_by: OPS_ADMIN_ID,
              created_at: nowIso,
              updated_at: nowIso,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axagent/im-connectors" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.imConnectors }),
      });
      return;
    }

    if (pathname === "/api/axagent/im-connectors" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        name?: string;
        platform?: string;
        agent_id?: string;
        identity_policy?: string;
        config?: Record<string, unknown>;
      };
      const created = {
        id: IM_CONNECTOR_ID,
        tenant_id: "default",
        name: body.name ?? "feishu_step22_e2e",
        platform: body.platform ?? "FEISHU",
        agent_id: body.agent_id ?? AGENT_ID,
        has_secret: true,
        config: body.config ?? {},
        identity_policy: body.identity_policy ?? "REQUIRE_USER_BIND",
        delegated_user_id: null,
        enabled: true,
        leader_holder: "core-node-01",
        lease_until: "2026-08-22T08:05:00Z",
        status: "CONNECTED",
        last_error: "",
        last_heartbeat_at: nowIso,
        created_by: OPS_ADMIN_ID,
        created_at: nowIso,
        updated_at: nowIso,
      };
      state.imConnectors = [created];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    if (pathname === "/api/axagent/im-bindings" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.imBindings }),
      });
      return;
    }

    if (pathname === "/api/axagent/im-bind/preview" && method === "GET") {
      const tok = searchParams.get("token") ?? "";
      if (tok !== MOCK_IM_BIND_TOKEN || state.bindTokenConsumed) {
        await route.fulfill({
          status: 400,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "INVALID_BIND_TOKEN",
            message: "绑定令牌无效或已过期",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          connector_id: IM_CONNECTOR_ID,
          connector_name:
            state.imConnectors[0]?.name ?? "feishu_step22_e2e",
          platform: "FEISHU",
          external_user_id: "ou_step22_e2e_user",
          external_display_name: "李东区",
          expires_at: "2026-08-22T08:15:00Z",
        }),
      });
      return;
    }

    if (pathname === "/api/axagent/im-bind/confirm" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as { token?: string };
      if (body.token !== MOCK_IM_BIND_TOKEN || state.bindTokenConsumed) {
        await route.fulfill({
          status: 400,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "INVALID_BIND_TOKEN",
            message: "绑定令牌已失效",
          }),
        });
        return;
      }
      state.bindTokenConsumed = true;
      state.imBindings.push({
        id: IM_BINDING_ID,
        tenant_id: "default",
        platform: "FEISHU",
        external_user_id: "ou_step22_e2e_user",
        external_display_name: "李东区",
        user_id: OPS_ADMIN_ID,
        username: "ops_admin",
        bound_at: nowIso,
      });
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          connector_id: IM_CONNECTOR_ID,
          platform: "FEISHU",
          external_user_id: "ou_step22_e2e_user",
          external_display_name: "李东区",
          user_id: OPS_ADMIN_ID,
          bound_at: nowIso,
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

test.describe("Step 22 Phase 2 Grounded AI & Builder UX Full Journey", () => {
  test("completes 7-stage onboarding, copilot rollback, builder MCP guard, sim preflight, five-zone view preflight, scoped grounded ask, and IM bind", async ({
    page,
  }) => {
    test.setTimeout(180_000);
    const isLive = process.env.AXPOINT_E2E_LIVE === "1";
    const coreBaseUrl = (
      process.env.CORE_BASE_URL ?? "http://127.0.0.1:9995"
    ).replace(/\/$/, "");
    const mockImHubUrl = (
      process.env.MOCK_IM_HUB_URL ?? "http://127.0.0.1:19822"
    ).replace(/\/$/, "");
    const runSuffix = String(Date.now()).slice(-6);

    const leakedTenantUrls: string[] = [];
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err.message);
    });

    const mockState: V2JourneyMockState = {
      hasSession: false,
      trialSubgraphSlug: null,
      trialBaseVersion: 1,
      trialDraftDoc: buildInitialSupplyChainDoc(),
      prePatchDoc: null,
      snapshots: [],
      issuedTokens: [],
      sandboxes: [
        {
          id: SANDBOX_ID,
          name: "initial_seed_sandbox",
          status: "READY",
          node_count: 2,
          link_count: 1,
          scope: {},
          base_captured_at: "2026-08-22T07:55:00Z",
          schema_version: 1,
        },
      ],
      dashboards: [],
      imConnectors: [],
      imBindings: [],
      bindTokenConsumed: false,
    };

    if (!isLive) {
      await installV2JourneyMocks(page, mockState, leakedTenantUrls);
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
      trialSubgraphs: [] as string[],
      tokenIds: [] as string[],
      sandboxIds: [] as string[],
      dashboardIds: [] as string[],
      datasetIds: [] as string[],
      docIds: [] as string[],
      imConnectorIds: [] as string[],
      imBindingIds: [] as string[],
      agentIds: [] as string[],
    };

    let opsPassword = "AxPointDemo#2026";
    if (isLive) {
      const auth = await resolveLiveAdminSession(page.request, coreBaseUrl);
      opsPassword = auth.password;
      cleanup.adminToken = auth.accessToken;
    }

    const liveHeaders = (): Record<string, string> => ({
      Authorization: `Bearer ${cleanup.adminToken}`,
      "Content-Type": "application/json",
    });

    try {
      // =========================================================================
      // Stage 1: Login + 5-Step Onboarding Strip + Trial Template Clone (`/`)
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

      await expect(page).toHaveURL(/\/$/);
      await expect(page.getByTestId("dashboard-page")).toBeVisible();

      const onboardingStrip = page.getByTestId("onboarding-strip");
      await expect(onboardingStrip).toBeVisible();
      await expect(
        page.getByTestId("onboarding-completion-badge")
      ).toContainText("/ 5");

      // Wait for async onboarding-status payload (`<5/5` renders steps; `5/5` renders expand toggle)
      const expandToggle = page.getByTestId("onboarding-expand-toggle");
      await expect(
        page.getByTestId("onboarding-step-DATA_INGEST").or(expandToggle)
      ).toBeVisible();
      if (await expandToggle.isVisible()) {
        await expandToggle.click();
      }
      for (const stepKey of [
        "DATA_INGEST",
        "NODE_MODELING",
        "MAPPING_AND_FLOW",
        "SIM_VERIFICATION",
        "COCKPIT_AND_AGENT",
      ]) {
        await expect(
          page.getByTestId(`onboarding-step-${stepKey}`)
        ).toBeVisible();
      }

      const readMainNodeCount = async (): Promise<number> => {
        if (isLive) {
          const resp = await page.request.get(
            `${coreBaseUrl}/api/xblue/nodes?subgraph=main&limit=200`,
            { headers: liveHeaders() }
          );
          expect(resp.ok()).toBe(true);
          const body = (await resp.json()) as {
            items?: unknown[];
            total?: number;
          };
          return typeof body.total === "number"
            ? body.total
            : (body.items?.length ?? 0);
        }
        return page.evaluate(async () => {
          const r = await fetch("/api/xblue/nodes?subgraph=main&limit=200");
          const j = (await r.json()) as { total?: number; items?: unknown[] };
          return typeof j.total === "number"
            ? j.total
            : (j.items?.length ?? 0);
        });
      };

      const mainCountBeforeClone = await readMainNodeCount();

      await page.getByTestId("onboarding-try-template-btn").click();
      const templateModal = page.getByTestId("template-hub-modal");
      await expect(templateModal).toBeVisible();
      await page.getByTestId("template-hub-pack-supply_chain").click();
      await expect(page.getByTestId("template-preview-pane")).toContainText(
        "Warehouse"
      );

      const cloneRespPromise = page.waitForResponse(
        (res) =>
          res.url().includes("/api/xblue/subgraphs/from-template") &&
          res.request().method() === "POST"
      );
      await page.getByTestId("template-trial-clone-btn").click();
      const cloneResp = await cloneRespPromise;
      expect(cloneResp.ok()).toBe(true);
      const cloneJson = (await cloneResp.json()) as {
        subgraph_slug: string;
        draft_id: string;
      };
      expect(cloneJson.subgraph_slug).toMatch(/^trial_supply_chain/);
      if (isLive) {
        cleanup.trialSubgraphs.push(cloneJson.subgraph_slug);
      }

      await expect(page).toHaveURL(
        new RegExp(`/studio\\?subgraph=${cloneJson.subgraph_slug}`)
      );
      const mainCountAfterClone = await readMainNodeCount();
      expect(mainCountAfterClone).toBe(mainCountBeforeClone);

      // =========================================================================
      // Stage 2: Studio Copilot Diagnose, Patch + Rollback + Confirm, Lineage & Spec
      // =========================================================================
      await expect(
        page.getByTestId("studio-dev-sandbox-badge")
      ).toBeVisible();
      await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();
      await page.getByTestId("canvas-node-Warehouse").click();

      const expandCopilotBtn = page.getByTestId("studio-copilot-expand-btn");
      if (await expandCopilotBtn.isVisible()) {
        await expandCopilotBtn.click();
      }
      await expect(page.getByTestId("studio-copilot-panel")).toBeVisible();

      const baseVersionBeforeDiag = (
        await page.getByTestId("studio-base-version").innerText()
      ).trim();

      // 2.1 Read-only diagnosis: zero version bump and zero snapshots created
      await page.getByTestId("copilot-mode-diagnose").click();
      await page
        .getByTestId("copilot-prompt-input")
        .fill("完备度体检：检查 Warehouse 的指标与映射完备度");
      await page.getByTestId("copilot-send-btn").click();

      await expect(page.getByTestId("copilot-findings-list")).toBeVisible();
      await expect(page.getByTestId("copilot-finding-0")).toBeVisible();
      await expect(page.getByTestId("studio-base-version")).toHaveText(
        baseVersionBeforeDiag
      );
      await expect(page.getByTestId("copilot-snapshot-row-1")).toHaveCount(0);

      // 2.2 Patch draft -> PROPOSED -> Snapshot rollback -> Re-apply -> CONFIRMED
      await page.getByTestId("copilot-mode-patch").click();
      await page
        .getByTestId("copilot-prompt-input")
        .fill("为 Warehouse 增加安全库存指标 safety_stock（单位：件）");
      await page.getByTestId("copilot-send-btn").click();

      await expect(
        page.getByTestId("copilot-patch-summary-card")
      ).toBeVisible();
      await expect(page.getByTestId("copilot-snapshot-row-1")).toBeVisible();
      await expect(
        page.getByTestId("canvas-node-proposed-Warehouse")
      ).toBeVisible();

      await page.getByTestId("inspector-tab-properties").click();
      await expect(
        page.getByTestId("prop-proposed-badge-safety_stock")
      ).toBeVisible();

      // Rollback snapshot #1 -> verify safety_stock proposal disappears cleanly
      await page.getByTestId("copilot-rollback-latest-btn").click();
      await expect(
        page.getByTestId("prop-proposed-badge-safety_stock")
      ).toHaveCount(0);
      await expect(
        page.getByTestId("canvas-node-proposed-Warehouse")
      ).toHaveCount(0);

      // Re-apply patch and confirm into CONFIRMED state
      await page
        .getByTestId("copilot-prompt-input")
        .fill("为 Warehouse 增加安全库存指标 safety_stock（单位：件）");
      await page.getByTestId("copilot-send-btn").click();
      await expect(
        page.getByTestId("prop-proposed-badge-safety_stock")
      ).toBeVisible();

      const dockConfirmAllBtn = page.getByTestId("dock-confirm-all-btn");
      if (await dockConfirmAllBtn.isVisible()) {
        await dockConfirmAllBtn.click();
      } else {
        await page.getByTestId("prop-confirm-btn-safety_stock").click();
      }
      await expect(
        page.getByTestId("prop-proposed-badge-safety_stock")
      ).toHaveCount(0);
      await expect(page.getByTestId("property-table-editor")).toContainText(
        "安全库存"
      );

      // 2.3 Single-node horizontal 5-stage lineage (`Warehouse`)
      await page.getByTestId("canvas-node-Warehouse").dblclick();
      const lineageCanvas = page.getByTestId("node-lineage-canvas");
      await expect(lineageCanvas).toBeVisible();
      for (const stage of [
        "DATASET_COLUMNS",
        "MAPPING_SPEC",
        "NODE_SCHEMA",
        "COMPUTE_RULES",
        "DOWNSTREAM_CONSUMERS",
      ]) {
        await expect(page.getByTestId(`lineage-stage-${stage}`)).toBeVisible();
      }
      await page.getByTestId("lineage-back-canvas-btn").click();
      await expect(lineageCanvas).toHaveCount(0);

      // 2.4 Subgraph Spec Overview (`ModelSpecOverview`)
      await page.getByTestId("open-spec-doc-btn").click();
      const specDrawer = page.getByTestId("model-spec-overview");
      await expect(specDrawer).toBeVisible();
      await expect(
        specDrawer.getByTestId("spec-section-sec_03_entity_dictionary")
      ).toContainText("Warehouse");
      await expect(
        specDrawer.getByTestId("spec-section-sec_07_acceptance_prompts")
      ).toBeVisible();
      await expect(specDrawer.getByTestId("spec-prompt-card-q1")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(specDrawer).toHaveCount(0);

      // =========================================================================
      // Stage 3: AxBuilder Build Token, MCP JSON Snippet & HTTP 403 Prod Guard
      // =========================================================================
      await page.goto("/agents");
      await expect(page.getByTestId("axagent-tab-workbench")).toBeVisible();

      // Verify BUILDER profile locks out STUDIO_PUBLISH & SIM_APPLY in PAT Modal
      await page.getByTestId("open-pat-modal-btn").click();
      const patModal = page.getByTestId("pat-modal");
      await expect(patModal).toBeVisible();
      await chooseSelectOption(page, "pat-profile-select", "BUILDER");
      await expect(
        page.getByTestId("pat-scope-checkbox-STUDIO_PUBLISH")
      ).toBeDisabled();
      await expect(
        page.getByTestId("pat-scope-checkbox-SIM_APPLY")
      ).toBeDisabled();
      await page.keyboard.press("Escape");
      await expect(patModal).toHaveCount(0);

      // Issue BUILDER token from MCP Builder Drawer/Tab & inspect MCP JSON snippet
      await page.getByTestId("axagent-tab-mcp").click();
      const mcpCard = page.getByTestId("mcp-builder-config-card");
      await expect(mcpCard).toBeVisible();
      await page
        .getByTestId("mcp-builder-token-name")
        .fill(`e2e-step22-builder-${runSuffix}`);

      const issueTokenRespPromise = page.waitForResponse(
        (res) =>
          res.url().includes("/api/auth/tokens") &&
          res.request().method() === "POST"
      );
      await page.getByTestId("mcp-builder-issue-btn").click();
      const issueTokenResp = await issueTokenRespPromise;
      expect(issueTokenResp.ok()).toBe(true);
      const issuedJson = (await issueTokenResp.json()) as {
        id: string;
        token: string;
        permissions: string[];
      };
      if (isLive) {
        cleanup.tokenIds.push(issuedJson.id);
      }

      await expect(
        page.getByTestId("mcp-builder-token-ready-badge")
      ).toBeVisible();
      const snippetPre = page.getByTestId("mcp-builder-snippet");
      await expect(snippetPre).toContainText("/api/mcp");
      await expect(snippetPre).toContainText(issuedJson.token);

      await page.getByTestId("mcp-builder-tab-cursor").click();
      await expect(snippetPre).toContainText("/api/mcp");
      await page.getByTestId("mcp-builder-tab-antigravity").click();

      // Verify BUILDER token receives HTTP 403 on production SIM_APPLY & template install
      const probeProdStatus = async (
        targetPath: string,
        payload: Record<string, unknown>
      ): Promise<number> => {
        if (isLive) {
          const r = await page.request.post(`${coreBaseUrl}${targetPath}`, {
            headers: {
              Authorization: `Bearer ${issuedJson.token}`,
              "Content-Type": "application/json",
            },
            data: payload,
          });
          return r.status();
        }
        return page.evaluate(
          async ({ p, tok, body }) => {
            const r = await fetch(p, {
              method: "POST",
              headers: {
                Authorization: `Bearer ${tok}`,
                "Content-Type": "application/json",
              },
              body: JSON.stringify(body),
            });
            return r.status;
          },
          { p: targetPath, tok: issuedJson.token, body: payload }
        );
      };

      const prodPublishStatus = await probeProdStatus(
        "/api/xblue/drafts/00000000-0000-0000-0000-000000000001/publish",
        { change_summary: "probe" }
      );
      expect(prodPublishStatus).toBe(403);

      const prodInstallStatus = await probeProdStatus(
        "/api/axops/templates/supply_chain/install",
        { dry_run: true }
      );
      expect(prodInstallStatus).toBe(403);

      // =========================================================================
      // Stage 4: AxSim Scope Preflight (`DEGRADED` -> `补齐类型` -> `READY`)
      // =========================================================================
      await page.goto("/sim");
      await expect(page.getByTestId("axsim-workbench")).toBeVisible();

      const wizardPanel = page.getByTestId("sandbox-wizard-panel");
      if (!(await wizardPanel.isVisible())) {
        await page.getByTestId("open-sandbox-wizard-btn").click();
      }
      await expect(wizardPanel).toBeVisible();

      await page
        .getByTestId("wizard-sandbox-name")
        .fill(`e2e_step22_sbx_${runSuffix}`);

      // Pick F-1 Factory seed node
      await page.getByTestId("wizard-seed-search").fill("F-1");
      await wizardPanel.getByRole("button", { name: "检索" }).click();
      const f1Candidate = wizardPanel
        .locator("button")
        .filter({ hasText: "(F-1)" })
        .first();
      await expect(f1Candidate).toBeVisible();
      await f1Candidate.click();

      // Restrict scope to `Factory` only -> expect DEGRADED preflight with missing upstream/neighbor types
      const factoryTypeBtn = wizardPanel
        .locator("button")
        .filter({ hasText: /^Factory$/ })
        .first();
      await expect(factoryTypeBtn).toBeVisible();
      await factoryTypeBtn.click();

      const preflightBadge = page.getByTestId("sim-preflight-grade-badge");
      await expect(preflightBadge).toContainText(/有损|阻断/);
      await expect(page.getByTestId("sim-preflight-card")).toContainText(
        "Warehouse"
      );

      // Trigger `补齐类型` (up to 2 hops: Factory -> Warehouse -> Order) until READY
      const fillTypesBtn = page.getByTestId("sim-preflight-fill-types-btn");
      await expect(fillTypesBtn).toBeVisible();
      for (let hop = 0; hop < 3; hop += 1) {
        if (await fillTypesBtn.isVisible()) {
          const preflightRespPromise = page.waitForResponse(
            (res) =>
              res.url().endsWith("/api/axsim/sandboxes/preflight") &&
              res.request().method() === "POST"
          );
          await fillTypesBtn.click();
          await preflightRespPromise;
        }
        const badgeText = await preflightBadge.innerText();
        if (badgeText.includes("就绪")) {
          break;
        }
      }
      await expect(preflightBadge).toContainText("就绪");

      const createSbxRespPromise = page.waitForResponse(
        (res) =>
          res.url().endsWith("/api/axsim/sandboxes") &&
          res.request().method() === "POST"
      );
      await page.getByTestId("wizard-create-submit").click();
      const createSbxResp = await createSbxRespPromise;
      expect(createSbxResp.ok()).toBe(true);
      const createdSbx = (await createSbxResp.json()) as { id: string };
      if (isLive) {
        cleanup.sandboxIds.push(createdSbx.id);
      }
      await expect(page.getByTestId("sandbox-status-badge")).toContainText(
        "就绪",
        { timeout: 15_000 }
      );

      // =========================================================================
      // Stage 5: xBlueView Five-Zone Cockpit & Publish Preflight Blocking/Fix
      // =========================================================================
      await page.goto("/view");
      await expect(page.getByTestId("xblueview-gallery-page")).toBeVisible();
      await page.getByTestId("open-create-dashboard-btn").click();

      const cockpitSlug = `e2e-step22-cockpit-${runSuffix}`;
      const createForm = page
        .getByTestId("xblueview-gallery-page")
        .locator("form");
      await expect(createForm).toBeVisible();
      await createForm
        .locator("input")
        .nth(0)
        .fill(`Step22 五区驾驶舱 ${runSuffix}`);
      await createForm.locator("input").nth(1).fill(cockpitSlug);
      await page.getByTestId("zone-layout-five-zone-btn").click();

      const createDashRespPromise = page.waitForResponse(
        (res) =>
          res.url().endsWith("/api/xblueview/dashboards") &&
          res.request().method() === "POST"
      );
      await createForm.getByRole("button", { name: "创建" }).click();
      const createDashResp = await createDashRespPromise;
      expect(createDashResp.ok()).toBe(true);
      const createdDash = (await createDashResp.json()) as { id: string };
      if (isLive) {
        cleanup.dashboardIds.push(createdDash.id);
      }

      await expect(page.getByTestId("xblueview-detail-page")).toBeVisible();
      await page.getByTestId("toggle-edit-mode-btn").click();

      // Verify all 5 semantic zone headers render inside the 12-column blueprint
      await expect(
        page.getByTestId("five-zone-blueprint-strip")
      ).toBeVisible();
      for (const zone of ["CONTROL", "KPI", "SCENE", "INSIGHT", "DECISION"]) {
        await expect(
          page.getByTestId(`semantic-zone-header-${zone}`)
        ).toBeVisible();
      }

      // Add an unconfigured ACTION_BUTTON widget (`flow_id: null`)
      await page.getByTestId("add-widget-btn").click();
      const widgetEditor = page.getByTestId("widget-editor");
      await expect(widgetEditor).toBeVisible();
      await page.getByTestId("widget-editor-title").fill("未配置调拨动作");
      await chooseSelectOption(page, "widget-editor-kind", "ACTION_BUTTON");

      const createWidgetRespPromise = page.waitForResponse(
        (res) =>
          res.url().includes(`/api/xblueview/dashboards/${createdDash.id}/widgets`) &&
          res.request().method() === "POST"
      );
      await page.getByTestId("widget-editor-save-btn").click();
      const createWidgetResp = await createWidgetRespPromise;
      expect(createWidgetResp.ok()).toBe(true);
      const createdWidget = (await createWidgetResp.json()) as { id: string };

      // Trigger Publish -> PublishPreflightModal must BLOCK on UNCONFIGURED_ACTION
      await page.getByTestId("publish-dashboard-btn").click();
      const preflightModal = page.getByTestId("publish-preflight-modal");
      await expect(preflightModal).toBeVisible();
      await expect(page.getByTestId("preflight-status-badge")).toContainText(
        "阻断"
      );
      await expect(preflightModal).toContainText("UNCONFIGURED_ACTION");
      await expect(
        page.getByTestId("preflight-confirm-publish-btn")
      ).toBeDisabled();

      // Locate broken widget, remove it, and re-run preflight -> PUBLISHED
      await page.getByTestId("preflight-locate-btn").first().click();
      await expect(preflightModal).toHaveCount(0);
      if (await widgetEditor.isVisible()) {
        await page.keyboard.press("Escape");
        await expect(widgetEditor).toHaveCount(0);
      }

      await page.getByTestId(`delete-widget-${createdWidget.id}`).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "删除" })
        .click();
      await expect(
        page.getByTestId(`widget-frame-${createdWidget.id}`)
      ).toHaveCount(0);

      await page.getByTestId("publish-dashboard-btn").click();
      await expect(preflightModal).toBeVisible();
      const confirmPublishBtn = page.getByTestId(
        "preflight-confirm-publish-btn"
      );
      await expect(confirmPublishBtn).toBeEnabled();
      await confirmPublishBtn.click();
      await expect(preflightModal).toHaveCount(0);
      await expect(page.getByTestId("xblueview-detail-page")).toContainText(
        "已发布"
      );

      // =========================================================================
      // Stage 6: AxQuery Explicit `@` Scope Mounting, `assertTurnGrounded` & Abstention
      // =========================================================================
      let demoOrdersId = DEMO_ORDERS_DS_ID;
      let policyDocId = POLICY_DOC_ID;
      let isolatedDatasetName = "finance_secret_vault";

      if (isLive) {
        const dsListResp = await page.request.get(
          `${coreBaseUrl}/api/axhub/datasets?limit=50`,
          { headers: liveHeaders() }
        );
        expect(dsListResp.ok()).toBe(true);
        const dsListJson = (await dsListResp.json()) as {
          items: Array<{ id: string; name: string }>;
        };
        const foundOrders = dsListJson.items.find(
          (d) => d.name === "demo_orders"
        );
        expect(foundOrders).toBeDefined();
        demoOrdersId = foundOrders!.id;

        const otherDs = dsListJson.items.find((d) => d.id !== demoOrdersId);
        if (otherDs) {
          isolatedDatasetName = otherDs.name;
        } else {
          const uploadResp = await page.request.post(
            `${coreBaseUrl}/api/axhub/uploads`,
            {
              headers: { Authorization: `Bearer ${cleanup.adminToken}` },
              multipart: {
                file: {
                  name: "step22_vault.csv",
                  mimeType: "text/csv",
                  buffer: new TextEncoder().encode(
                    "secret_sku_code,vault_qty\nSKU-Z22,8421\n"
                  ),
                },
              },
            }
          );
          expect(uploadResp.ok()).toBe(true);
          const upJson = (await uploadResp.json()) as { id: string };
          isolatedDatasetName = `step22_vault_${runSuffix}`;
          const createDsResp = await page.request.post(
            `${coreBaseUrl}/api/axhub/datasets`,
            {
              headers: liveHeaders(),
              data: {
                name: isolatedDatasetName,
                display_name: "Step22 隔离对照表",
                source_kind: "FILE_TABLE",
                source_ref: { upload_id: upJson.id, sheet: 0 },
              },
            }
          );
          expect(createDsResp.ok()).toBe(true);
          const createdDs = (await createDsResp.json()) as { id: string };
          cleanup.datasetIds.push(createdDs.id);
        }

        const ingestResp = await page.request.post(
          `${coreBaseUrl}/api/documents/ingest`,
          {
            headers: liveHeaders(),
            data: {
              title: "补货审批制度",
              source_name: `replenishment_policy_step22_${runSuffix}.md`,
              content:
                "第一条 补货审批制度触发条件：当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程。\n\n第二条 紧急补货单须经区域仓储主管与供应链总监双级签批后方可下发执行。",
            },
          }
        );
        expect(ingestResp.ok()).toBe(true);
        const ingestJson = (await ingestResp.json()) as { doc_id: string };
        policyDocId = ingestJson.doc_id;
        cleanup.docIds.push(policyDocId);
      }

      await page.goto("/query");
      const queryInput = page.getByTestId("axquery-input");
      await expect(queryInput).toBeVisible();

      // Mount dataset pill via `@` mention popover
      await queryInput.click();
      await queryInput.fill("@");
      const scopePopover = page.getByTestId("query-scope-popover");
      await expect(scopePopover).toBeVisible();
      await page
        .getByTestId(`query-scope-option-datasets-${demoOrdersId}`)
        .click();
      await expect(
        page.getByTestId(`scope-pill-dataset-${demoOrdersId}`)
      ).toBeVisible();

      // Mount regulation doc pill via `@` mention popover
      await queryInput.click();
      await queryInput.fill("@");
      await expect(scopePopover).toBeVisible();
      await page.getByTestId("query-scope-tab-docs").click();
      await page.getByTestId(`query-scope-option-docs-${policyDocId}`).click();
      await expect(
        page.getByTestId(`scope-pill-doc-${policyDocId}`)
      ).toBeVisible();

      // Turn 0: Guarded SQL with exact numeric cell verification (`33,239.25` / `33239.25`)
      await queryInput.fill("已支付订单的总金额是多少");
      await queryInput.press("Enter");
      const turn0 = page.getByTestId("axquery-turn-0");
      await expect(turn0).toBeVisible({ timeout: 20_000 });
      await expect(turn0.getByTestId("turn-applied-scope-badge")).toBeVisible();
      await expect(
        turn0.locator('[data-testid^="grounding-badge-"]')
      ).toBeVisible();
      await assertTurnGrounded(turn0);

      // Turn 1: Stage 0 Certified Ontology Metric (`SEMANTIC_METRIC`)
      await queryInput.fill("按状态统计已支付订单金额（按月汇总）");
      await queryInput.press("Enter");
      const turn1 = page.getByTestId("axquery-turn-1");
      await expect(turn1).toBeVisible({ timeout: 20_000 });
      await expect(
        turn1.getByTestId("grounding-badge-SEMANTIC_METRIC")
      ).toBeVisible();
      await assertTurnGrounded(turn1);

      // Turn 2: Grounded Doc citation bounds & click-to-highlight (`[文1]`)
      await queryInput.fill("补货审批制度的具体触发条件是什么？");
      await queryInput.press("Enter");
      const turn2 = page.getByTestId("axquery-turn-2");
      await expect(turn2).toBeVisible({ timeout: 20_000 });
      await expect(
        turn2.getByTestId("grounding-badge-GROUNDED_DOC")
      ).toBeVisible();
      await assertTurnGrounded(turn2);

      // Turn 3: Out-of-scope dataset query -> deterministic abstention (0 leaked rows)
      await page.getByTestId("axquery-mode-DATA").click();
      await queryInput.fill(`统计表 ${isolatedDatasetName} 中的记录总数`);
      await queryInput.press("Enter");
      const turn3 = page.getByTestId("axquery-turn-3");
      await expect(turn3).toBeVisible({ timeout: 20_000 });
      await expect(
        turn3.locator(
          '[data-testid="grounding-badge-ABSTAINED"], [data-testid="grounding-badge-CLARIFICATION"]'
        )
      ).toBeVisible();
      await expect(turn3.getByTestId("result-table")).toHaveCount(0);

      // =========================================================================
      // Stage 7: Enterprise IM Connector Lease & `?im_bind=axim_...` Confirmation
      // =========================================================================
      let liveImHubReady = false;
      let targetAgentId = AGENT_ID;
      if (isLive) {
        const agentCreateResp = await page.request.post(
          `${coreBaseUrl}/api/axagent/agents`,
          {
            headers: liveHeaders(),
            data: {
              name: `im_step22_agent_${runSuffix}`,
              description: "Step22 IM 绑定验证智能体",
              spec: {
                system_prompt: "你是企业经营数据助手，严格依据工具结果回答。",
                tools: [{ kind: "BUILTIN", name: "ask_analytics" }],
                max_steps: 4,
              },
            },
          }
        );
        expect(agentCreateResp.ok()).toBe(true);
        const agentJson = (await agentCreateResp.json()) as { id: string };
        targetAgentId = agentJson.id;
        cleanup.agentIds.push(targetAgentId);

        const pubAgentResp = await page.request.post(
          `${coreBaseUrl}/api/axagent/agents/${targetAgentId}/publish`,
          { headers: liveHeaders(), data: {} }
        );
        expect(pubAgentResp.ok()).toBe(true);

        try {
          const hubHealth = await page.request.get(
            `${mockImHubUrl}/test/health`,
            { timeout: 2_000 }
          );
          liveImHubReady = hubHealth.ok();
        } catch {
          liveImHubReady = false;
        }
      }

      await page.goto("/agents");
      await page.getByTestId("axagent-tab-im").click();
      await expect(page.getByTestId("im-connector-panel")).toBeVisible();

      const connectorName = `feishu_step22_${runSuffix}`;
      await page.getByTestId("new-im-connector-btn").click();
      const imModal = page.getByTestId("im-connector-modal");
      await expect(imModal).toBeVisible();
      await page.getByTestId("im-connector-name-input").fill(connectorName);
      await chooseSelectOption(page, "im-connector-platform-select", "FEISHU");
      await chooseSelectOption(
        page,
        "im-connector-agent-select",
        targetAgentId
      );
      await chooseSelectOption(
        page,
        "im-connector-policy-select",
        "REQUIRE_USER_BIND"
      );
      await page
        .getByTestId("im-connector-credential-id-input")
        .fill("cli_step22_app");
      await page
        .getByTestId("im-connector-secret-input")
        .fill("sec_step22_secret");

      if (!isLive || liveImHubReady) {
        await page
          .getByTestId("im-connector-endpoint-input")
          .fill(mockImHubUrl);
        const privSwitch = page.getByTestId("im-connector-private-net-switch");
        if ((await privSwitch.getAttribute("aria-checked")) !== "true") {
          await privSwitch.click();
        }
      }

      const saveConnRespPromise = page.waitForResponse(
        (res) =>
          res.url().endsWith("/api/axagent/im-connectors") &&
          res.request().method() === "POST"
      );
      await page.getByTestId("save-im-connector-btn").click();
      const saveConnResp = await saveConnRespPromise;
      expect(saveConnResp.ok()).toBe(true);
      const savedConn = (await saveConnResp.json()) as { id: string };
      if (isLive) {
        cleanup.imConnectorIds.push(savedConn.id);
      }

      await expect(
        page.getByTestId(`im-connector-card-${connectorName}`)
      ).toBeVisible();

      let activeBindToken = process.env.AXPOINT_IM_BIND_TOKEN ?? "";
      if (!isLive) {
        activeBindToken = MOCK_IM_BIND_TOKEN;
      } else if (!activeBindToken && liveImHubReady) {
        // Wait for ImGatewaySupervisor to claim lease and connect to mock_im_hub
        for (let attempt = 0; attempt < 25; attempt += 1) {
          const listResp = await page.request.get(
            `${coreBaseUrl}/api/axagent/im-connectors`,
            { headers: liveHeaders() }
          );
          const listJson = (await listResp.json()) as
            | { items?: Array<{ id: string; status: string; leader_holder: string | null }> }
            | Array<{ id: string; status: string; leader_holder: string | null }>;
          const items = Array.isArray(listJson)
            ? listJson
            : (listJson.items ?? []);
          const current = items.find((c) => c.id === savedConn.id);
          if (current?.status === "CONNECTED" && current.leader_holder) {
            break;
          }
          await page.waitForTimeout(1_000);
        }

        const beforeRepliesResp = await page.request.get(
          `${mockImHubUrl}/test/replies`
        );
        const beforeReplies = (
          (await beforeRepliesResp.json()) as { items?: unknown[] }
        ).items?.length ?? 0;

        const injectResp = await page.request.post(
          `${mockImHubUrl}/test/inject`,
          {
            data: {
              event_id: `evt_step22_${runSuffix}`,
              external_user_id: "ou_step22_e2e_user",
              external_display_name: "李东区",
              chat_id: "oc_step22_dm",
              text: "查询已支付订单总金额",
            },
          }
        );
        expect(injectResp.ok()).toBe(true);

        for (let attempt = 0; attempt < 15; attempt += 1) {
          const repResp = await page.request.get(`${mockImHubUrl}/test/replies`);
          const repJson = (await repResp.json()) as {
            items?: Array<{ text: string }>;
          };
          const items = repJson.items ?? [];
          if (items.length > beforeReplies) {
            const lastText = items[items.length - 1]?.text ?? "";
            const match = /axim_[A-Za-z0-9_-]+/.exec(lastText);
            if (match) {
              activeBindToken = match[0];
              break;
            }
          }
          await page.waitForTimeout(1_000);
        }
        expect(activeBindToken).toMatch(/^axim_/);
      }

      if (activeBindToken) {
        await page.goto(`/agents?im_bind=${encodeURIComponent(activeBindToken)}`);
        const bindModal = page.getByTestId("im-bind-confirm-modal");
        await expect(bindModal).toBeVisible();
        await expect(
          page.getByTestId("im-bind-connector-name")
        ).toContainText(connectorName);
        await expect(page.getByTestId("im-bind-external-user")).toContainText(
          "ou_step22_e2e_user"
        );

        await expect(
          page.getByTestId(`im-connector-status-${connectorName}`)
        ).toContainText("已连接");
        await expect(
          page.getByTestId(`im-connector-leader-${connectorName}`)
        ).toBeVisible();

        await page.getByTestId("confirm-im-bind-btn").click();
        await expect(bindModal).toHaveCount(0);

        const bindingsCard = page.getByTestId("im-bindings-card");
        await expect(bindingsCard).toContainText("ou_step22_e2e_user");
        await expect(bindingsCard).toContainText("ops_admin");

        if (isLive) {
          const bindListResp = await page.request.get(
            `${coreBaseUrl}/api/axagent/im-bindings`,
            { headers: liveHeaders() }
          );
          if (bindListResp.ok()) {
            const bindListJson = (await bindListResp.json()) as
              | { items?: Array<{ id: string; external_user_id: string }> }
              | Array<{ id: string; external_user_id: string }>;
            const rows = Array.isArray(bindListJson)
              ? bindListJson
              : (bindListJson.items ?? []);
            for (const row of rows) {
              if (row.external_user_id === "ou_step22_e2e_user") {
                cleanup.imBindingIds.push(row.id);
              }
            }
          }
        }
      } else {
        // Standalone live run without external IM WS hub: verify deep-link modal & expired token guard
        await page.goto("/agents?im_bind=axim_unissued_probe_token");
        const bindModal = page.getByTestId("im-bind-confirm-modal");
        await expect(bindModal).toBeVisible();
        await expect(page.getByTestId("im-bind-error")).toBeVisible();
        await expect(
          page.getByTestId("confirm-im-bind-btn")
        ).toBeDisabled();
        await page.getByTestId("cancel-im-bind-btn").click();
        await expect(bindModal).toHaveCount(0);
      }

      expect(leakedTenantUrls).toEqual([]);
      expect(pageErrors).toEqual([]);
    } finally {
      if (isLive && cleanup.adminToken) {
        const headers = { Authorization: `Bearer ${cleanup.adminToken}` };
        for (const id of cleanup.imBindingIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axagent/im-bindings/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.imConnectorIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axagent/im-connectors/${id}`, {
              headers,
            })
            .catch(() => null);
        }
        for (const id of cleanup.agentIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axagent/agents/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.docIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/documents/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.datasetIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axhub/datasets/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.dashboardIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/xblueview/dashboards/${id}`, {
              headers,
            })
            .catch(() => null);
        }
        for (const id of cleanup.sandboxIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/axsim/sandboxes/${id}`, { headers })
            .catch(() => null);
        }
        for (const id of cleanup.tokenIds) {
          await page.request
            .delete(`${coreBaseUrl}/api/auth/tokens/${id}`, { headers })
            .catch(() => null);
        }
        for (const slug of cleanup.trialSubgraphs) {
          await page.request
            .delete(`${coreBaseUrl}/api/xblue/subgraphs/${slug}`, { headers })
            .catch(() => null);
        }
      }
    }
  });
});
