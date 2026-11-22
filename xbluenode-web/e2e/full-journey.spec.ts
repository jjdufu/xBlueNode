import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const OPS_ADMIN_ID = "00000000-0000-0000-0000-000000001401";
const UPLOAD_ID = "00000000-0000-0000-0000-000000001402";
const SUPPLY_DS_ID = "00000000-0000-0000-0000-000000001403";
const DEMO_ORDERS_DS_ID = "00000000-0000-0000-0000-000000001404";
const WAREHOUSE_W1_ID = "00000000-0000-0000-0000-000000001410";
const ORDER_O1_ID = "00000000-0000-0000-0000-000000001411";
const SANDBOX_ID = "00000000-0000-0000-0000-000000001420";
const SCENARIO_ID = "00000000-0000-0000-0000-000000001421";
const DASHBOARD_ID = "00000000-0000-0000-0000-000000001430";
const KPI_WIDGET_ID = "00000000-0000-0000-0000-000000001431";
const EXPORT_JOB_ID = "00000000-0000-0000-0000-000000001440";

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

interface JourneyMockState {
  hasSession: boolean;
  schemaVersion: number;
  uploadedDatasets: Array<{
    id: string;
    name: string;
    display_name: string;
    status: string;
    row_count: number;
  }>;
  scenarios: Array<{
    id: string;
    sandbox_id: string;
    name: string;
    description: string;
    actions: Array<{
      target_node_id: string;
      metric_key: string;
      mode: string;
      value: number;
    }>;
    params: { mode: string };
    overlay_hash: string;
    is_stale: boolean;
    status: string;
    error_message: null;
    created_by: null;
    created_at: string;
    finished_at: string;
    result: {
      affected_count: number;
      kpis: unknown[];
      top_impacts: Array<{
        node_id: string;
        type_name: string;
        title: string;
        metric: string;
        before_val: number;
        after_val: number;
        delta: number;
        delta_ratio: number;
        hop_distance: number;
        top_path: string[];
      }>;
      warnings: unknown[];
      converged: boolean;
      iterations: number;
    };
  }>;
  exports: Array<{
    id: string;
    export_format: string;
    source: { kind: string; dashboard_id: string; filters: Record<string, unknown> };
    options: { title?: string };
    status: string;
    file_name: string;
    file_size: number;
    created_at: string;
    expires_at: string;
  }>;
}

async function installFullJourneyMocks(
  page: Page,
  state: JourneyMockState,
  leakedTenantUrls: string[]
): Promise<void> {
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
          trace_id: "journey-400",
        }),
      });
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
            error_code: "TOKEN_EXPIRED",
            message: "No active session",
            trace_id: "journey-refresh-401",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: buildMockJwt("refreshed"),
          token_type: "Bearer",
          expires_in: 900,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/logout" && method === "POST") {
      state.hasSession = false;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
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
            "SIM_RUN",
            "SIM_APPLY",
            "VIEW_EDIT",
            "VIEW_SHARE",
            "EXPORT_DATA",
            "GUARD_ADMIN",
            "OPS_ADMIN",
          ],
          token_kind: "ACCESS",
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
          entity_count: 12,
          edge_count: 20,
          embedding_count: 24,
          embedding_dims: [1024],
          timestamp: "2026-06-01T10:00:00Z",
        }),
      });
      return;
    }

    // --- Stage 2: AxHub Uploads & Datasets ---
    if (pathname === "/api/axhub/connector-types") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/axhub/connections") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/axhub/uploads" && method === "POST") {
      if (!state.uploadedDatasets.some((d) => d.id === SUPPLY_DS_ID)) {
        state.uploadedDatasets.unshift({
          id: SUPPLY_DS_ID,
          name: "supply_chain_ku_cun_2",
          display_name: "供应链库存表 (supply_chain.xlsx)",
          status: "READY",
          row_count: 24,
        });
      }
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          upload_id: UPLOAD_ID,
          deduplicated: false,
        }),
      });
      return;
    }

    if (pathname === `/api/axhub/uploads/${UPLOAD_ID}` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: UPLOAD_ID,
          filename: "supply_chain.xlsx",
          size_bytes: 8330,
          sha256: "a1b2c3d4e5f60718293a4b5c6d7e8f90",
          status: "COMMITTED",
          dataset_ids: [SUPPLY_DS_ID],
          doc_id: "00000000-0000-0000-0000-000000001409",
          error_code: null,
          created_at: "2026-06-01T10:01:00Z",
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/datasets" && method === "GET") {
      const allDs = [
        ...state.uploadedDatasets,
        {
          id: DEMO_ORDERS_DS_ID,
          name: "demo_orders",
          display_name: "示范履约订单表",
          status: "READY",
          row_count: 100,
        },
      ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: allDs.map((d) => ({
            id: d.id,
            connection_id: null,
            name: d.name,
            display_name: d.display_name,
            description: d.display_name,
            source_kind: "FILE_TABLE",
            source_ref: {},
            sync_mode: "SNAPSHOT",
            primary_key: ["id"],
            cursor_column: null,
            cursor_value: null,
            delete_column: null,
            schema_policy: "EVOLVE",
            lake_schema: "t_default",
            lake_table: d.name,
            schema_json: [],
            schema_version: 1,
            row_count: d.row_count,
            last_snapshot_id: 1,
            last_synced_at: "2026-06-01T10:01:00Z",
            schedule_cron: null,
            tags: ["supply_chain"],
            owner_user_id: null,
            status: d.status,
            created_at: "2026-06-01T10:00:00Z",
            updated_at: "2026-06-01T10:01:00Z",
          })),
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname.startsWith("/api/axhub/datasets/") && pathname.endsWith("/preview")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            {
              name: "warehouse_code",
              ordinal: 1,
              data_type: "STRING",
              nullable: false,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 2,
              sample_values: ["W-1"],
              pii_category: "NONE",
              pii_hit_ratio: 0,
              source_comment: "",
            },
          ],
          rows: [{ warehouse_code: "W-1" }],
        }),
      });
      return;
    }

    // --- Stage 3 & 4: xBlueStudio & xBlueGraph ---
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
            current_version: state.schemaVersion,
            created_at: "2026-06-01T08:00:00Z",
            updated_at: "2026-06-01T10:00:00Z",
          },
        ]),
      });
      return;
    }

    const journeySchemaDoc = {
      node_types: [
        {
          id: "00000000-0000-0000-0000-000000000011",
          name: "Warehouse",
          display_name: "仓储枢纽",
          description: "区域履约仓储中心",
          icon: "warehouse",
          color: "#2563eb",
          title_template: "{node_key}",
          properties: [
            {
              name: "region",
              display_name: "大区",
              description: "所属大区",
              data_type: "STRING",
              kind: "ATTRIBUTE",
              required: true,
              pii_category: "NONE",
            },
            {
              name: "inventory",
              display_name: "在库库存",
              description: "实时库存",
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
          pos_x: 120,
          pos_y: 120,
        },
        {
          id: "00000000-0000-0000-0000-000000000012",
          name: "Order",
          display_name: "履约订单",
          description: "下游客户订单",
          icon: "shopping-cart",
          color: "#09090b",
          title_template: "订单 {node_key}",
          properties: [
            {
              name: "demand",
              display_name: "需求量",
              description: "订单需求数量",
              unit: "件",
              data_type: "FLOAT64",
              kind: "METRIC",
              required: false,
              pii_category: "NONE",
            },
          ],
          agg_rules: [],
          derived_rules: [],
          sync_to_knowledge: false,
          pos_x: 420,
          pos_y: 120,
        },
      ],
      edge_types: [
        {
          id: "00000000-0000-0000-0000-000000000013",
          name: "FULFILLS",
          src_type_name: "Warehouse",
          dst_type_name: "Order",
          impact_weight: 0.8,
          cardinality: "ONE_TO_MANY",
          description: "仓库履约订单",
          properties: [],
          propagation: {},
        },
      ],
      mappings: [],
    };

    if (pathname === "/api/xblue/schema" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: "main",
          version: state.schemaVersion,
          ...journeySchemaDoc,
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
            id: "00000000-0000-0000-0000-000000000015",
            subgraph_slug: "main",
            version: state.schemaVersion,
            change_summary: "基线模型快照",
            published_at: "2026-06-01T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (
      (pathname === "/api/xblue/drafts" || pathname === "/api/xblue/mappings") &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/xblue/drafts" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        title?: string;
        schema_doc?: unknown;
        base_version?: number;
      };
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: "00000000-0000-0000-0000-000000001450",
          subgraph_slug: "main",
          title: body.title ?? "main 发布草案",
          base_version: body.base_version ?? state.schemaVersion,
          source: "MANUAL",
          status: "DRAFT",
          ai_context: {},
          schema_doc: body.schema_doc ?? journeySchemaDoc,
          issues: [],
          created_at: "2026-06-01T10:02:00Z",
          updated_at: "2026-06-01T10:02:00Z",
        }),
      });
      return;
    }

    if (pathname.startsWith("/api/xblue/drafts/") && pathname.endsWith("/publish")) {
      state.schemaVersion += 1;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: "main",
          version: state.schemaVersion,
          draft_id: "00000000-0000-0000-0000-000000001450",
          schema_doc: journeySchemaDoc,
          diff: {
            node_types: { added: [], removed: [], changed: [] },
            edge_types: { added: [], removed: [], changed: [] },
            mappings: { added: [], removed: [], changed: [] },
          },
          mapping_jobs: [],
          aggregate_jobs: [],
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

    if (
      pathname === "/api/xblue/nodes" ||
      pathname === "/api/xblue/nodes/search"
    ) {
      const w1 = {
        id: WAREHOUSE_W1_ID,
        type_name: "Warehouse",
        node_key: "W-1",
        title: "华东仓",
        properties: { region: "华东" },
        metrics: { inventory: 500 },
      };
      const o1 = {
        id: ORDER_O1_ID,
        type_name: "Order",
        node_key: "O-1",
        title: "订单 O-1",
        properties: {},
        metrics: { demand: 120 },
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [w1, o1],
          nodes: [w1, o1],
          links: [
            {
              id: "00000000-0000-0000-0000-000000001415",
              link_type: "FULFILLS",
              src_node_id: WAREHOUSE_W1_ID,
              dst_node_id: ORDER_O1_ID,
              weight: 0.8,
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname.startsWith("/api/xblue/nodes/") && pathname.endsWith("/neighbors")) {
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
              metrics: { inventory: 500 },
            },
            {
              id: ORDER_O1_ID,
              type_name: "Order",
              node_key: "O-1",
              title: "订单 O-1",
              metrics: { demand: 120 },
            },
          ],
          links: [
            {
              id: "00000000-0000-0000-0000-000000001415",
              link_type: "FULFILLS",
              src_node_id: WAREHOUSE_W1_ID,
              dst_node_id: ORDER_O1_ID,
              weight: 0.8,
            },
          ],
        }),
      });
      return;
    }

    // --- Stage 5: AxAnalytics ---
    if (pathname === "/api/analytics/schema") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          datasets: [
            {
              id: DEMO_ORDERS_DS_ID,
              name: "demo_orders",
              display_name: "示范履约订单表",
              row_count: 100,
              columns: [
                {
                  name: "order_no",
                  data_type: "STRING",
                  pii_category: "NONE",
                  description: "",
                },
                {
                  name: "amount",
                  data_type: "DECIMAL",
                  pii_category: "NONE",
                  description: "",
                },
              ],
            },
          ],
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
            {
              name: "total_orders",
              data_type: "INT64",
              pii_category: "NONE",
              masked: false,
            },
          ],
          rows: [{ total_orders: 100 }],
          row_count: 1,
          truncated: false,
          elapsed_ms: 12,
        }),
      });
      return;
    }

    // --- Stage 6: AxSim ---
    if (pathname === "/api/axsim/sandboxes" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: SANDBOX_ID,
            name: "华东供应链全旅程推演沙盘",
            status: "READY",
            node_count: 2,
            link_count: 1,
            sim_result: null,
            created_at: "2026-06-01T09:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === `/api/axsim/sandboxes/${SANDBOX_ID}/graph`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sandbox_id: SANDBOX_ID,
          overlay_hash: "h1",
          nodes: [
            {
              id: WAREHOUSE_W1_ID,
              type_name: "Warehouse",
              node_key: "W-1",
              title: "华东仓",
              metrics: { inventory: 500 },
              properties: {},
              is_modified: false,
              is_new: false,
            },
            {
              id: ORDER_O1_ID,
              type_name: "Order",
              node_key: "O-1",
              title: "订单 O-1",
              metrics: { demand: 120 },
              properties: {},
              is_modified: false,
              is_new: false,
            },
          ],
          links: [
            {
              id: "00000000-0000-0000-0000-000000001415",
              link_type: "FULFILLS",
              src_node_id: WAREHOUSE_W1_ID,
              dst_node_id: ORDER_O1_ID,
              weight: 0.8,
              is_modified: false,
              is_new: false,
            },
          ],
        }),
      });
      return;
    }

    if (
      pathname === `/api/axsim/sandboxes/${SANDBOX_ID}/scenarios` &&
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
      pathname === `/api/axsim/sandboxes/${SANDBOX_ID}/scenarios` &&
      method === "POST"
    ) {
      const body = (req.postDataJSON() ?? {}) as { name?: string };
      const createdScenario = {
        id: SCENARIO_ID,
        sandbox_id: SANDBOX_ID,
        name: body.name ?? "journey_cut_30",
        description: "",
        actions: [
          {
            target_node_id: WAREHOUSE_W1_ID,
            metric_key: "inventory",
            mode: "RATIO",
            value: -0.3,
          },
        ],
        params: { mode: "STEADY" },
        overlay_hash: "h1",
        is_stale: false,
        status: "SUCCEEDED",
        error_message: null,
        created_by: null,
        created_at: "2026-06-01T10:05:00Z",
        finished_at: "2026-06-01T10:05:01Z",
        result: {
          affected_count: 2,
          kpis: [],
          top_impacts: [
            {
              node_id: ORDER_O1_ID,
              type_name: "Order",
              title: "订单 O-1",
              metric: "demand",
              before_val: 120,
              after_val: 91.2,
              delta: -28.8,
              delta_ratio: -0.24,
              hop_distance: 1,
              top_path: [WAREHOUSE_W1_ID, ORDER_O1_ID],
            },
          ],
          warnings: [],
          converged: true,
          iterations: 3,
        },
      };
      state.scenarios = [createdScenario];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(createdScenario),
      });
      return;
    }

    if (pathname === `/api/axsim/scenarios/${SCENARIO_ID}/impacts`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.scenarios[0]?.result.top_impacts ?? [],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axsim/scenarios/${SCENARIO_ID}/checkpoints`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [] }),
      });
      return;
    }

    // --- Stage 7: xBlueView & XLSX Export ---
    if (pathname === "/api/xblueview/dashboards" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DASHBOARD_ID,
              tenant_id: "default",
              name: "供应链全景驾驶舱",
              slug: "supply-overview",
              description: "华东供应链指标与履约监控看板",
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
              created_at: "2026-06-01T08:00:00Z",
              updated_at: "2026-06-01T08:00:00Z",
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
          id: DASHBOARD_ID,
          tenant_id: "default",
          name: "供应链全景驾驶舱",
          slug: "supply-overview",
          description: "华东供应链指标与履约监控看板",
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
          created_at: "2026-06-01T08:00:00Z",
          updated_at: "2026-06-01T08:00:00Z",
          widgets: [
            {
              id: KPI_WIDGET_ID,
              tenant_id: "default",
              dashboard_id: DASHBOARD_ID,
              widget_kind: "KPI_CARD",
              title: "已支付金额",
              position: { x: 0, y: 0, w: 8, h: 4 },
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
              deps: {
                dataset_ids: [],
                type_names: [],
                metrics: ["paid_amount"],
                alerts: false,
                scenario_ids: [],
              },
              created_at: "2026-06-01T08:00:00Z",
              updated_at: "2026-06-01T08:00:00Z",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/widgets/${KPI_WIDGET_ID}/data`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [{ name: "value", data_type: "DECIMAL" }],
          rows: [["3517.50"]],
          row_count: 1,
          truncated: false,
          generated_at: "2026-06-01T10:06:00Z",
          data_version: "v1",
          drill_level: 0,
        }),
      });
      return;
    }

    if (pathname === "/api/xblueview/exports" && method === "POST") {
      const createdJob = {
        id: EXPORT_JOB_ID,
        export_format: "XLSX",
        source: {
          kind: "DASHBOARD",
          dashboard_id: DASHBOARD_ID,
          filters: {},
        },
        options: { title: "供应链全景驾驶舱" },
        status: "SUCCEEDED",
        file_name: "supply-overview.xlsx",
        file_size: 4096,
        created_at: "2026-06-01T10:06:30Z",
        expires_at: "2026-06-08T10:06:30Z",
      };
      state.exports = [createdJob];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(createdJob),
      });
      return;
    }

    if (pathname === "/api/xblueview/exports" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.exports,
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/xblueview/exports/${EXPORT_JOB_ID}/download`) {
      await route.fulfill({
        status: 200,
        headers: {
          "Content-Type":
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
          "Content-Disposition": 'attachment; filename="supply-overview.xlsx"',
        },
        body: "PK\x03\x04mock_xlsx_payload",
      });
      return;
    }

    // --- Stage 8: AxOps ---
    if (pathname === "/api/axops/health/deep") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          database: { ok: true, latency_ms: 2, replication_lag_s: 0 },
          parser: { ok: true, version: "0.1.0" },
          lake: { ok: true, snapshot_id: 4 },
          llm: { ok: true, provider: "cassette" },
          embed: { ok: true },
          disk: { ok: true, data_root_free_bytes: 64_000_000_000 },
          jobs: { pending: 0, running: 0, failed_24h: 0, oldest_pending_s: 0 },
          outbox: { lag_s: 0.4, pending: 0 },
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
        body: JSON.stringify({ lag_s: 0.4, pending: 0 }),
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
          uptime_s: 7200,
          data_root: "/var/lib/axpoint",
          template_packs_count: 3,
        }),
      });
      return;
    }

    if (pathname === "/api/axops/work-items" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000001460",
              tenant_id: "default",
              item_no: 1,
              code: "WI-1",
              title: "核验华东一号仓补货履约时效",
              description: "跟进低库存补货计划",
              item_kind: "ISSUE",
              status: "OPEN",
              priority: "P1",
              assignee: null,
              source_kind: "ALERT",
              source_ref: "alert-1",
              node_ids: [WAREHOUSE_W1_ID],
              due_at: null,
              created_by: OPS_ADMIN_ID,
              created_at: "2026-06-01T09:30:00Z",
              updated_at: "2026-06-01T09:30:00Z",
              closed_at: null,
            },
          ],
          next_cursor: null,
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

test("Step 14 full-journey acceptance: login ops_admin -> upload supply_chain.xlsx -> publish Studio schema -> browse Graph -> run Analytics SQL -> run Sim scenario -> render View & download XLSX -> inspect AxOps -> logout & redirect /view to /login", async ({
  page,
}) => {
  test.setTimeout(120_000);

  const isLive = process.env.AXPOINT_E2E_LIVE === "1";
  const opsPassword = process.env.OPS_ADMIN_PASSWORD ?? "Axp#Ops_2026!99";
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];

  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  if (isLive) {
    const coreBaseUrl = (
      process.env.CORE_BASE_URL ?? "http://127.0.0.1:9995"
    ).replace(/\/$/, "");

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
  } else {
    const mockState: JourneyMockState = {
      hasSession: false,
      schemaVersion: 1,
      uploadedDatasets: [],
      scenarios: [],
      exports: [],
    };
    await installFullJourneyMocks(page, mockState, leakedTenantUrls);
  }

  // -------------------------------------------------------------------------
  // 1. Clear storage / cookies, open /login, and log in with local ops_admin
  // -------------------------------------------------------------------------
  await page.context().clearCookies();
  await page.goto("/login");
  await page.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
    sessionStorage.setItem("axpoint_logged_out", "1");
  });
  await page.goto("/login");

  await expect(
    page.getByRole("heading", { level: 1, name: "AxGuard · 统一身份认证" })
  ).toBeVisible();

  await page.getByTestId("login-tenant-input").fill("default");
  await page.getByTestId("login-username-input").fill("ops_admin");
  await page.getByTestId("login-password-input").fill(opsPassword);
  await page.getByTestId("login-submit-btn").click();

  await expect(page).toHaveURL(/\/$/, { timeout: 20_000 });
  await expect(
    page.getByRole("heading", { level: 1, name: "系统概览" })
  ).toBeVisible();

  // -------------------------------------------------------------------------
  // 2. Navigate to /hub, upload supply_chain.xlsx, verify dataset READY
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "AxHub · 数据接入与目录" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "AxHub · 数据接入与目录" })
  ).toBeVisible();

  await page.getByTestId("axhub-nav-uploads").click();
  await page
    .getByTestId("upload-file-input")
    .setInputFiles("../axpoint-parser/tests/fixtures/supply_chain.xlsx");

  await expect(
    page.getByTestId("upload-record-supply_chain.xlsx")
  ).toContainText(/COMMITTED|已提交/, { timeout: 45_000 });

  await page.getByTestId("axhub-nav-workspace").click();
  const readyDatasetRows = page.locator("[data-testid^='dataset-row-']");
  await expect(readyDatasetRows.first()).toBeVisible({ timeout: 20_000 });
  await expect(readyDatasetRows.first()).toContainText(/READY|就绪/);

  // -------------------------------------------------------------------------
  // 3. Navigate to /studio, publish model draft (SchemaDoc)
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "xBlueStudio · 节点建模" }).click();
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "xBlueStudio · 节点建模",
    })
  ).toBeVisible();
  await expect(page.getByTestId("xbluestudio-canvas")).toBeVisible();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible({
    timeout: 15_000,
  });

  // Discard all stale open drafts left by prior gate steps so base_version matches latest main
  const discardDraftBtn = page.getByRole("button", { name: "丢弃草案" });
  for (let i = 0; i < 10 && (await discardDraftBtn.count()) > 0; i += 1) {
    const discardResp = page.waitForResponse(
      (res) =>
        res.url().includes("/api/xblue/drafts/") &&
        res.url().endsWith("/discard") &&
        res.request().method() === "POST"
    );
    const reloadDraftsResp = page.waitForResponse(
      (res) =>
        new URL(res.url()).pathname === "/api/xblue/drafts" &&
        res.request().method() === "GET"
    );
    await discardDraftBtn.first().click();
    await Promise.all([discardResp, reloadDraftsResp]);
    await expect
      .poll(
        async () =>
          (await discardDraftBtn.count()) === 0 ||
          (await discardDraftBtn.first().isEnabled()),
        { timeout: 10_000 }
      )
      .toBe(true);
  }
  await expect(discardDraftBtn).toHaveCount(0, { timeout: 10_000 });

  await page.getByTestId("open-publish-modal-btn").click();
  await page.getByTestId("publish-summary-input").fill("全旅程终验模型发布");
  await page.getByTestId("publish-schema-btn").click();
  await expect(page.getByTestId("studio-status-toast")).toContainText("发布", {
    timeout: 20_000,
  });

  // -------------------------------------------------------------------------
  // 4. Navigate to /graph, verify business nodes render
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "xBlueGraph · 图浏览" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "xBlueGraph · 图浏览" })
  ).toBeVisible();
  await expect(
    page.getByTestId("xbluegraph-canvas").locator("canvas").first()
  ).toBeVisible({ timeout: 15_000 });

  await page.getByTestId("graph-search-input").fill("华东");
  await page.getByTestId("graph-search-btn").click();
  await expect(page.getByTestId("search-hit-W-1")).toBeVisible({
    timeout: 15_000,
  });
  await page.getByTestId("search-hit-W-1").click();
  await expect(page.getByTestId("graph-node-W-1")).toContainText("华东仓");

  // -------------------------------------------------------------------------
  // 5. Navigate to /analytics, run guarded SQL query and verify result table
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "AxAnalytics · 分析与问数" }).click();
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "AxAnalytics · 分析与问数",
    })
  ).toBeVisible();

  await page
    .getByTestId("sql-editor-textarea")
    .fill("SELECT count(*) AS total_orders FROM demo_orders");
  await page.getByTestId("sql-run-btn").click();
  const sqlResultTable = page.getByTestId("result-table");
  await expect(sqlResultTable).toBeVisible({ timeout: 20_000 });
  await expect(sqlResultTable).toContainText(/\d+/);

  // -------------------------------------------------------------------------
  // 6. Navigate to /sim, run scenario and verify impact rows render
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "AxSim · 因果推演" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "AxSim · 因果推演" })
  ).toBeVisible();

  const sandboxSelect = page.getByTestId("sandbox-selector");
  await expect(sandboxSelect).toBeVisible();
  if (((await sandboxSelect.getAttribute("data-value")) ?? "") === "") {
    await page.getByTestId("open-sandbox-wizard-btn").click();
    await page.getByTestId("wizard-sandbox-name").fill("终验推演沙盘");
    await page.getByTestId("wizard-create-submit").click();
    await expect(sandboxSelect).toHaveAttribute("data-value", /.+/, {
      timeout: 15_000,
    });
  }

  await expect(page.getByTestId("action-node-select-0")).toHaveAttribute(
    "data-value",
    /.+/,
    {
      timeout: 15_000,
    }
  );
  await page.getByTestId("scenario-name-input").fill("journey_cut_30");
  await page.getByTestId("run-scenario-btn").click();

  const impactRows = page.locator("[data-testid^='impact-row-']");
  await expect(impactRows.first()).toBeVisible({ timeout: 25_000 });

  // -------------------------------------------------------------------------
  // 7. Navigate to /view, verify dashboard widgets & XLSX export download
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "xBlueView · 孪生驾驶舱" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "xBlueView · 孪生驾驶舱" })
  ).toBeVisible();

  const overviewCard = page.getByTestId("dashboard-card-supply-overview");
  await expect(overviewCard).toBeVisible({ timeout: 15_000 });
  await overviewCard.getByRole("button", { name: "打开" }).click();

  await expect(page).toHaveURL(/\/view\/supply-overview/);
  const widgetFrames = page.locator("[data-testid^='widget-frame-']");
  await expect(widgetFrames.first()).toBeVisible({ timeout: 25_000 });

  await page.getByTestId("open-export-dialog-btn").click();
  await expect(page.getByTestId("export-dialog")).toBeVisible();
  await page.getByTestId("export-format-XLSX").click();
  await page.getByTestId("submit-export-btn").click();

  await expect(page.getByTestId("export-list-drawer")).toBeVisible({
    timeout: 15_000,
  });
  const downloadBtn = page
    .getByTestId("export-list-drawer")
    .locator("[data-testid^='download-export-']")
    .first();
  await expect(downloadBtn).toBeVisible({ timeout: 30_000 });

  const [download] = await Promise.all([
    page.waitForEvent("download"),
    downloadBtn.click(),
  ]);
  expect(download.suggestedFilename().toLowerCase()).toContain(".xlsx");

  await page.getByLabel("关闭出品中心").click();
  await expect(page.getByTestId("export-list-drawer")).toHaveCount(0);

  // -------------------------------------------------------------------------
  // 8. Navigate to /ops, verify AxOps health & work-item board render
  // -------------------------------------------------------------------------
  await page.getByRole("link", { name: "AxOps · 运维与交付" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "AxOps · 运维与交付" })
  ).toBeVisible();
  await expect(page.getByTestId("axops-health-panel")).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByTestId("axops-health-panel")).toContainText(/HEALTHY|正常|健康/);

  await page.getByTestId("axops-tab-work_items").click();
  await expect(page.getByTestId("axops-workitems-panel")).toBeVisible();
  await expect(page.getByTestId("workitem-kanban-board")).toBeVisible();

  // -------------------------------------------------------------------------
  // 9. Logout, then navigate to /view and assert URL redirects to /login
  // -------------------------------------------------------------------------
  await page.getByTestId("shell-user-menu-btn").click();
  await page.getByTestId("shell-logout-btn").click();
  await expect(page).toHaveURL(/\/login$/, { timeout: 15_000 });

  await page.goto("/view");
  await expect(page).toHaveURL(/\/login$/, { timeout: 15_000 });
  await expect(
    page.getByRole("heading", { level: 1, name: "AxGuard · 统一身份认证" })
  ).toBeVisible();

  expect(leakedTenantUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
});
