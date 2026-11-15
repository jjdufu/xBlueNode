import {
  expect,
  type APIRequestContext,
  type Page,
  type Route,
  test,
} from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const DATASET_ORDERS_ID = "00000000-0000-0000-0000-000000000501";
const AI_DRAFT_ID = "00000000-0000-0000-0000-000000000520";
const MAPPING_ID = "00000000-0000-0000-0000-000000000530";
const CANDIDATE_ENTITY_ID = "00000000-0000-0000-0000-000000000611";

function attachSecurityMonitors(
  page: Page,
  leakedTenantUrls: string[],
  missingAuthUrls: string[],
  consoleErrors: string[],
  pageErrors: string[]
): void {
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });

  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  page.on("request", (request) => {
    const url = new URL(request.url());
    if (!url.pathname.startsWith("/api/")) return;

    if (url.searchParams.has("tenant_id")) {
      leakedTenantUrls.push(request.url());
    }

    const authHeader = request.headers()["authorization"] ?? "";
    if (
      url.pathname !== "/api/health" &&
      (!authHeader.startsWith("Bearer ") ||
        authHeader.slice(7).trim().length === 0)
    ) {
      missingAuthUrls.push(request.url());
    }
  });
}

const MAIN_SCHEMA_DOC = {
  node_types: [
    {
      id: "00000000-0000-0000-0000-000000000511",
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
          description: "所属销售大区",
          data_type: "STRING",
          kind: "ATTRIBUTE",
          required: true,
          pii_category: "NONE",
        },
        {
          name: "inventory",
          display_name: "在库库存",
          description: "实时可用库存",
          unit: "件",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
        },
        {
          name: "total_demand",
          display_name: "总需求量",
          description: "下游履约订单聚合需求",
          unit: "件",
          data_type: "FLOAT64",
          kind: "METRIC",
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
          source_metric: "demand",
          weight_by_link: false,
        },
      ],
      derived_rules: [],
      sync_to_knowledge: true,
      pos_x: 100,
      pos_y: 140,
    },
    {
      id: "00000000-0000-0000-0000-000000000512",
      name: "Order",
      display_name: "履约订单",
      description: "下游客户履约订单",
      icon: "shopping-cart",
      color: "#09090b",
      title_template: "订单 {node_key}",
      properties: [
        {
          name: "status",
          display_name: "状态",
          description: "订单状态",
          data_type: "STRING",
          kind: "ATTRIBUTE",
          required: true,
          pii_category: "NONE",
        },
        {
          name: "demand",
          display_name: "需求数量",
          description: "订购件数",
          unit: "件",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: true,
          pii_category: "NONE",
        },
      ],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 440,
      pos_y: 140,
    },
  ],
  edge_types: [
    {
      id: "00000000-0000-0000-0000-000000000513",
      name: "FULFILLS",
      src_type_name: "Warehouse",
      dst_type_name: "Order",
      impact_weight: 0.85,
      cardinality: "ONE_TO_MANY",
      description: "仓库向订单发货履约",
      properties: [],
      propagation: {},
    },
  ],
  mappings: [
    {
      name: "orders_to_order_nodes",
      dataset: "demo_orders",
      node_type: "Order",
      key_column: "id",
      title: "订单 {id}",
      attributes: { status: "status" },
      metrics: { demand: "amount" },
      geo: { lng_column: null, lat_column: null },
      links: [
        {
          link_type: "FULFILLS",
          direction: "IN",
          target_type: "Warehouse",
          target_key_column: "warehouse_code",
          create_missing_target: false,
        },
      ],
      on_source_delete: "KEEP",
      mode: "INCREMENTAL",
    },
  ],
};

const LOGISTICS_SCHEMA_DOC = {
  node_types: [
    {
      id: "00000000-0000-0000-0000-000000000515",
      name: "StorageHub",
      display_name: "物流仓配中心",
      description: "物流部门视角仓储节点",
      icon: "warehouse",
      color: "#2563eb",
      title_template: "{node_key}",
      properties: [
        {
          name: "region",
          display_name: "区域",
          description: "",
          data_type: "STRING",
          kind: "ATTRIBUTE",
          required: false,
          pii_category: "NONE",
        },
        {
          name: "dock_count",
          display_name: "月台数",
          description: "",
          data_type: "INT64",
          kind: "ATTRIBUTE",
          required: false,
          pii_category: "NONE",
        },
      ],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 140,
      pos_y: 160,
    },
    {
      id: "00000000-0000-0000-0000-000000000516",
      name: "Shipment",
      display_name: "干线运单",
      description: "物流干线运输任务",
      icon: "truck",
      color: "#334155",
      title_template: "运单 {node_key}",
      properties: [
        {
          name: "carrier",
          display_name: "承运商",
          description: "",
          data_type: "STRING",
          kind: "ATTRIBUTE",
          required: false,
          pii_category: "NONE",
        },
      ],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 450,
      pos_y: 160,
    },
  ],
  edge_types: [
    {
      id: "00000000-0000-0000-0000-000000000517",
      name: "DISPATCHES",
      src_type_name: "StorageHub",
      dst_type_name: "Shipment",
      impact_weight: 0.75,
      cardinality: "ONE_TO_MANY",
      description: "仓配中心发运干线运单",
      properties: [],
      propagation: {},
    },
  ],
  mappings: [],
};

async function installStudioMocks(page: Page): Promise<void> {
  const draftsBySubgraph: Record<string, unknown[]> = {
    main: [],
    logistics: [],
  };

  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const method = request.method();
    const url = new URL(request.url());
    const { pathname, searchParams } = url;
    const subgraph = searchParams.get("subgraph") || "main";

    if (searchParams.has("tenant_id")) {
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "INVALID_ARGUMENT",
          message: "tenant_id query parameter is forbidden",
          trace_id: "xbluestudio-e2e-400",
        }),
      });
      return;
    }

    const authHeader = request.headers()["authorization"] ?? "";
    if (
      pathname !== "/api/health" &&
      (!authHeader.startsWith("Bearer ") ||
        authHeader.slice(7).trim().length === 0)
    ) {
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "xbluestudio-e2e-401",
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/subgraphs" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000500",
            slug: "main",
            display_name: "企业主图",
            description: "企业权威生产主图",
            owner_dept: "default",
            is_main: true,
            current_version: 2,
            created_at: "2026-01-31T08:00:00Z",
            updated_at: "2026-01-31T08:30:00Z",
          },
          {
            id: "00000000-0000-0000-0000-000000000502",
            slug: "logistics",
            display_name: "物流履约子图",
            description: "物流部门独立建模子图",
            owner_dept: "logistics",
            is_main: false,
            current_version: 1,
            created_at: "2026-01-31T08:10:00Z",
            updated_at: "2026-01-31T08:20:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === "/api/xblue/subgraphs/logistics/merge-preview" &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: "logistics",
          subgraph_version: 1,
          main_version: 2,
          alignments: [
            {
              sub_type: "StorageHub",
              main_type: "Warehouse",
              score: 0.86,
              suggested_strategy: "EXTEND_PROPS",
            },
          ],
          conflicts: [],
          added_node_types: ["Shipment"],
          added_edge_types: ["DISPATCHES"],
          added_mappings: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema" && method === "GET") {
      const doc =
        subgraph === "logistics" ? LOGISTICS_SCHEMA_DOC : MAIN_SCHEMA_DOC;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: subgraph,
          version: subgraph === "logistics" ? 1 : 2,
          ...doc,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/validate" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ valid: true, issues: [] }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/versions" && method === "GET") {
      const list =
        subgraph === "logistics"
          ? [
              {
                id: "00000000-0000-0000-0000-000000000540",
                subgraph_slug: "logistics",
                version: 1,
                change_summary: "初始化物流仓配与运单模型",
                published_at: "2026-01-31T08:20:00Z",
              },
            ]
          : [
              {
                id: "00000000-0000-0000-0000-000000000542",
                subgraph_slug: "main",
                version: 2,
                change_summary: "新增 Order 履约订单与 total_demand 聚合规则",
                published_at: "2026-01-31T08:30:00Z",
              },
              {
                id: "00000000-0000-0000-0000-000000000541",
                subgraph_slug: "main",
                version: 1,
                change_summary: "初始化 Warehouse 仓储基线",
                published_at: "2026-01-31T08:00:00Z",
              },
            ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(list),
      });
      return;
    }

    if (pathname.includes("/api/xblue/schema/versions/") && pathname.includes("/diff/")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          node_types: {
            added: ["Order"],
            removed: [],
            changed: [
              {
                name: "Warehouse",
                fields: [
                  {
                    path: "agg_rules.total_demand",
                    before: null,
                    after: "SUM(Order.demand)",
                  },
                ],
              },
            ],
          },
          edge_types: {
            added: ["FULFILLS"],
            removed: [],
            changed: [],
          },
          mappings: {
            added: ["orders_to_order_nodes"],
            removed: [],
            changed: [],
          },
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/drafts" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(draftsBySubgraph[subgraph] ?? []),
      });
      return;
    }

    if (pathname === "/api/xblue/drafts/ai" && method === "POST") {
      const body = (request.postDataJSON() ?? {}) as { title?: string };
      const generatedDraft = {
        id: AI_DRAFT_ID,
        subgraph_slug: subgraph,
        title: body.title || "AI 供应链本体草案",
        source: "AI",
        base_version: 2,
        schema_doc: {
          node_types: [
            ...MAIN_SCHEMA_DOC.node_types,
            {
              name: "Supplier",
              display_name: "上游供应商",
              description: "AI 基于数据集与描述自动推导的上游供应商实体",
              icon: "factory",
              color: "#0f766e",
              title_template: "{node_key}",
              properties: [
                {
                  name: "lead_time_days",
                  display_name: "交付周期",
                  description: "",
                  unit: "天",
                  data_type: "FLOAT64",
                  kind: "METRIC",
                  required: false,
                  pii_category: "NONE",
                },
              ],
              agg_rules: [],
              derived_rules: [],
              sync_to_knowledge: false,
              pos_x: 100,
              pos_y: 360,
            },
          ],
          edge_types: [
            ...MAIN_SCHEMA_DOC.edge_types,
            {
              name: "SUPPLIES",
              src_type_name: "Supplier",
              dst_type_name: "Warehouse",
              impact_weight: 0.8,
              cardinality: "ONE_TO_MANY",
              description: "供应商向仓库补货",
              properties: [],
              propagation: {},
            },
          ],
          mappings: MAIN_SCHEMA_DOC.mappings,
        },
        ai_context: { model_name: "deterministic+llm" },
        issues: [],
        status: "DRAFT",
        created_at: "2026-01-31T08:45:00Z",
        updated_at: "2026-01-31T08:45:00Z",
      };
      draftsBySubgraph[subgraph] = [generatedDraft];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(generatedDraft),
      });
      return;
    }

    if (pathname === "/api/xblue/mappings" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: MAPPING_ID,
            subgraph_slug: "main",
            name: "orders_to_order_nodes",
            dataset_id: DATASET_ORDERS_ID,
            dataset_name: "demo_orders",
            node_type: "Order",
            spec: MAIN_SCHEMA_DOC.mappings[0],
            enabled: true,
            schema_version: 1,
            last_snapshot_id: 1,
            created_at: "2026-01-31T08:30:00Z",
            updated_at: "2026-01-31T08:30:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === `/api/xblue/mappings/${MAPPING_ID}/preview` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mapping_id: MAPPING_ID,
          sampled_rows: 3,
          items: [
            {
              node_key: "O-101",
              title: "订单 O-101",
              attributes: { status: "PAID" },
              metrics: { demand: 120 },
              links: [],
            },
          ],
          rows_rejected: 1,
          reject_samples: [
            {
              row: { id: null, amount: "bad" },
              reason: "key_column id is null",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/graphql/sdl" && method === "GET") {
      const sdl = `type Warehouse {
  id: ID!
  nodeKey: String!
  title: String!
  region: String
  inventory: Float
  total_demand: Float
  fulfillsOrders(limit: Int = 50): [Order!]!
}

type Order {
  id: ID!
  nodeKey: String!
  title: String!
  status: String
  demand: Float
}

type Query {
  warehouse(nodeKey: String!): Warehouse
  warehouseList(q: String, limit: Int = 50, cursor: String): [Warehouse!]!
  order(nodeKey: String!): Order
  orderList(q: String, limit: Int = 50, cursor: String): [Order!]!
}`;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ version: 2, sdl }),
      });
      return;
    }

    if (pathname === "/api/xblue/objects" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          version: 2,
          items: [
            {
              name: "Warehouse",
              display_name: "仓储枢纽",
              description: "区域履约仓储中心",
              collection_path: "/api/xblue/objects/Warehouse",
              item_path: "/api/xblue/objects/Warehouse/{node_key}",
              graphql_field: "warehouse",
              graphql_list_field: "warehouseList",
              fields: [
                {
                  name: "inventory",
                  display_name: "在库库存",
                  kind: "METRIC",
                  data_type: "FLOAT64",
                  required: false,
                  unit: "件",
                  pii_category: "NONE",
                },
              ],
              neighbors: [
                {
                  field_name: "fulfillsOrders",
                  link_type: "FULFILLS",
                  direction: "OUT",
                  target_type: "Order",
                  cardinality: "ONE_TO_MANY",
                },
              ],
              sample_graphql: `{ warehouse(nodeKey: "W-1") { title inventory fulfillsOrders(limit: 5) { nodeKey } } }`,
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/graphql" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          data: {
            warehouse: {
              title: "华东仓",
              inventory: 500,
              fulfillsOrders: [{ nodeKey: "O-101" }, { nodeKey: "O-102" }],
            },
          },
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/datasets" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DATASET_ORDERS_ID,
              connection_id: null,
              name: "demo_orders",
              display_name: "示范履约订单表",
              description: "订单明细数据集",
              source_kind: "FILE_TABLE",
              source_ref: {},
              sync_mode: "SNAPSHOT",
              primary_key: ["id"],
              cursor_column: null,
              cursor_value: null,
              delete_column: null,
              schema_policy: "EVOLVE",
              lake_schema: "t_default",
              lake_table: "demo_orders",
              schema_json: [],
              schema_version: 1,
              row_count: 100,
              last_snapshot_id: 1,
              last_synced_at: "2026-01-31T08:00:00Z",
              schedule_cron: null,
              tags: [],
              owner_user_id: null,
              status: "READY",
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/nodes" && method === "GET") {
      const sampleNode = {
        id: "00000000-0000-0000-0000-000000000551",
        type_name: "Warehouse",
        node_key: "W-1",
        title: "华东仓",
        metrics: { inventory: 500, total_demand: 320 },
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [sampleNode],
          nodes: [sampleNode],
          links: [],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/expr/validate" && method === "POST") {
      const body = (request.postDataJSON() ?? {}) as {
        type_name?: string;
        expression?: string;
      };
      const expr = (body.expression ?? "").trim();
      if (expr.includes("1 < 2 < 3")) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            valid: false,
            issues: [
              {
                code: "EXPR_INVALID",
                message: "chained comparison is not associative",
                column: 7,
              },
            ],
          }),
        });
        return;
      }
      const isBoolFilter =
        expr.includes("!=") || expr.includes("==") || expr.startsWith("not ");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          valid: true,
          inferred_type: isBoolFilter ? "BOOL" : "NUMBER",
          issues: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/expr/evaluate" && method === "POST") {
      const body = (request.postDataJSON() ?? {}) as { expression?: string };
      const expr = (body.expression ?? "").trim();
      if (expr.includes("/ 0")) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            value: null,
            value_type: "NULL",
            warnings: ["DIV_BY_ZERO"],
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          value: 1.5625,
          value_type: "NUMBER",
          warnings: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/aggregate/recompute" && method === "POST") {
      const body = (request.postDataJSON() ?? {}) as { type_name?: string };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          job_id: "00000000-0000-0000-0000-000000000601",
          type_name: body.type_name ?? "Warehouse",
        }),
      });
      return;
    }

    if (
      pathname === "/api/xblue/knowledge/node-candidates" &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              entity_id: CANDIDATE_ENTITY_ID,
              name: "杭州分拨仓",
              entity_type: "仓储枢纽",
              suggested_type_name: "Warehouse",
              occur_count: 5,
              confidence: 0.92,
              sample_docs: ["长三角仓配作业规范.md", "华东季度履约复盘.md"],
            },
          ],
        }),
      });
      return;
    }

    if (
      pathname ===
        `/api/xblue/knowledge/node-candidates/${CANDIDATE_ENTITY_ID}/accept` &&
      method === "POST"
    ) {
      const body = (request.postDataJSON() ?? {}) as {
        type_name?: string;
        node_key?: string;
        title?: string;
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          node: {
            id: "00000000-0000-0000-0000-000000000612",
            type_name: body.type_name ?? "Warehouse",
            node_key: body.node_key ?? "W-HZ-01",
            title: body.title ?? "杭州分拨仓",
            metrics: {},
          },
          entity_id: CANDIDATE_ENTITY_ID,
          confirmed: true,
        }),
      });
      return;
    }

    if (pathname === "/api/graph/topology" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: [
            {
              id: "00000000-0000-0000-0000-000000000560",
              node_type: "DOC",
              label: "华东仓储履约作业规范.md",
              weight: 1,
            },
          ],
          edges: [],
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

async function ensureLiveStudioFixtures(
  page: Page,
  request: APIRequestContext
): Promise<void> {
  const apiBase =
    process.env.PLAYWRIGHT_API_BASE_URL ?? "http://127.0.0.1:9995";
  const token =
    process.env.VITE_DEV_TOKEN ??
    process.env.AXPOINT_DEV_TOKEN ??
    "dev-smoke-token-00000000000000000000";
  const authHeaders = {
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
  };

  // Forward /api/** on vite preview (:4173) to live Core (:9995)
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const incomingUrl = new URL(req.url());
    const targetUrl = `${apiBase}${incomingUrl.pathname}${incomingUrl.search}`;
    const resp = await request.fetch(targetUrl, {
      method: req.method(),
      headers: req.headers(),
      data: req.postDataBuffer() ?? undefined,
    });
    await route.fulfill({ response: resp });
  });

  // Ensure main has Warehouse + Order + FULFILLS published
  const mainSchemaResp = await request.get(`${apiBase}/api/xblue/schema`, {
    headers: authHeaders,
  });
  if (mainSchemaResp.ok()) {
    const cur = (await mainSchemaResp.json()) as {
      node_types?: { name?: string }[];
    };
    const names = new Set((cur.node_types ?? []).map((n) => n.name));
    if (!names.has("Warehouse") || !names.has("Order")) {
      await request.post(`${apiBase}/api/xblue/schema`, {
        headers: authHeaders,
        data: {
          node_types: MAIN_SCHEMA_DOC.node_types.map(({ id: _id, ...rest }) => rest),
          edge_types: MAIN_SCHEMA_DOC.edge_types.map(({ id: _id, ...rest }) => rest),
          mappings: [],
          change_summary: "E2E live seed Warehouse and Order",
          force: true,
        },
      });
    }
  }

  // Ensure logistics subgraph exists
  const sgResp = await request.get(`${apiBase}/api/xblue/subgraphs`, {
    headers: authHeaders,
  });
  if (sgResp.ok()) {
    const raw = (await sgResp.json()) as
      | { slug?: string }[]
      | { items?: { slug?: string }[] };
    const list = Array.isArray(raw) ? raw : raw.items ?? [];
    if (!list.some((s) => s.slug === "logistics")) {
      await request.post(`${apiBase}/api/xblue/subgraphs`, {
        headers: authHeaders,
        data: {
          slug: "logistics",
          display_name: "物流履约子图",
          owner_dept: "logistics",
          seed_from_main: true,
        },
      });
    }
  }
}

test("xBlueStudio /studio renders canvas cards & edges, switches subgraph for merge preview, inspects version diff, runs AI draft wizard, and executes live Domain GraphQL", async ({
  page,
  request,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];

  attachSecurityMonitors(
    page,
    leakedTenantUrls,
    missingAuthUrls,
    consoleErrors,
    pageErrors
  );

  const isLive = process.env.PLAYWRIGHT_LIVE_API === "1";
  if (isLive) {
    await ensureLiveStudioFixtures(page, request);
  } else {
    await installStudioMocks(page);
  }

  await page.goto("/studio");

  await expect(
    page.getByRole("heading", { level: 1, name: "xBlueStudio · 节点与关系建模" })
  ).toBeVisible();

  // 1. Assert top status bar & @xyflow/react canvas renders Warehouse, Order, and FULFILLS
  await expect(page.getByTestId("studio-status-bar")).toContainText("main");
  await expect(page.getByTestId("xbluestudio-canvas")).toBeVisible();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();
  await expect(page.getByTestId("canvas-node-Order")).toBeVisible();
  await expect(page.getByTestId("canvas-edge-FULFILLS")).toContainText(
    "FULFILLS"
  );

  // Trigger ELK layered layout cleanly
  await page.getByTestId("canvas-elk-layout-btn").click();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();

  // 2. Switch to department subgraph `logistics` and open Merge Preview dialog
  await page.getByTestId("subgraph-tab-logistics").click();
  await expect(page.getByTestId("studio-status-bar")).toContainText(
    "logistics"
  );

  const mergeBtn = page.getByTestId("open-merge-dialog-btn");
  await expect(mergeBtn).toBeEnabled();
  await mergeBtn.click();

  const mergeDialog = page.getByTestId("subgraph-merge-dialog");
  await expect(mergeDialog).toBeVisible();
  if (!isLive) {
    await expect(mergeDialog).toContainText("StorageHub");
    await expect(mergeDialog).toContainText("0.86");
  }
  await mergeDialog.locator("button").first().click();

  // Switch back to main subgraph
  await page.getByTestId("subgraph-tab-main").click();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();

  // 3. Open Version History panel & assert version list and structured DiffView
  await page.getByTestId("open-version-panel-btn").click();
  const versionPanel = page.getByTestId("version-panel");
  await expect(versionPanel).toBeVisible();
  await expect(page.getByTestId("version-list")).toBeVisible();
  await expect(page.getByTestId("studio-diff-view")).toBeVisible();
  if (!isLive) {
    await expect(page.getByTestId("studio-diff-view")).toContainText("+ Order");
  }
  await page.getByTestId("close-version-panel-btn").click();

  // 4. Open AI Draft Wizard, generate AI draft, and assert draft loads onto canvas
  await page.getByTestId("open-ai-wizard-btn").click();
  const aiWizard = page.getByTestId("ai-draft-wizard");
  await expect(aiWizard).toBeVisible();
  await page
    .getByTestId("ai-draft-description-input")
    .fill("补充上游供应商 Supplier 与仓库补货 SUPPLIES 关系");
  await page.getByTestId("ai-draft-submit-btn").click();
  await expect(aiWizard).toHaveCount(0);

  if (!isLive) {
    await expect(page.getByTestId("canvas-node-Supplier")).toBeVisible();
    await expect(page.getByTestId("studio-active-draft-label")).toContainText(
      "AI 供应链本体草案"
    );
  }

  // 5. Open Domain API Drawer, verify GraphQL SDL (`type Warehouse`, `fulfillsOrders`), and execute sample query
  await page.getByTestId("open-domain-drawer-btn").click();
  const domainDrawer = page.getByTestId("domain-api-drawer");
  await expect(domainDrawer).toBeVisible();

  const sdlPre = page.getByTestId("domain-graphql-sdl");
  await expect(sdlPre).toContainText("type Warehouse");
  await expect(sdlPre).toContainText("fulfillsOrders");

  await page.getByTestId("graphql-run-btn").click();
  const gqlOut = page.getByTestId("graphql-response-output");
  await expect(gqlOut).toBeVisible();
  if (!isLive) {
    await expect(gqlOut).toContainText("华东仓");
  }
  await page.getByTestId("close-domain-drawer-btn").click();

  // 6. Verify right-side PropertyTable, RuleEditor & MappingEditor 50-row dry-run preview
  await page.getByTestId("canvas-node-Warehouse").click();
  await page.getByTestId("inspector-tab-properties").click();
  await expect(page.getByTestId("property-table-editor")).toBeVisible();

  await page.getByTestId("inspector-tab-rules").click();
  await expect(page.getByTestId("rule-editor")).toBeVisible();
  await expect(page.getByTestId("expr-dryrun-panel")).toBeVisible();
  await expect(page.getByTestId("recompute-agg-btn")).toBeVisible();

  if (!isLive) {
    // Validate agg filter inferred BOOL badge
    await page
      .getByTestId("agg-filter-input-0")
      .fill("prop.status != 'CANCELED'");
    await expect(page.getByTestId("agg-filter-inferred-type-0")).toHaveText(
      "BOOL"
    );

    // Add derived rule, verify NUMBER inferred badge & column-numbered error diagnostic
    await page.getByTestId("add-derived-rule-btn").click();
    await expect(page.getByTestId("derived-inferred-type-0")).toHaveText(
      "NUMBER"
    );
    await page.getByTestId("derived-expr-input-0").fill("1 < 2 < 3");
    await expect(page.getByTestId("derived-expr-issue-0-0")).toContainText(
      "列 7: chained comparison is not associative"
    );
    await page
      .getByTestId("derived-expr-input-0")
      .fill("if(total_demand > 0, inventory / total_demand, null)");
    await expect(page.getByTestId("derived-inferred-type-0")).toHaveText(
      "NUMBER"
    );

    // Trigger full aggregate recompute
    await page.getByTestId("recompute-agg-btn").click();
    await expect(page.getByTestId("recompute-agg-status")).toContainText(
      "Warehouse"
    );

    // Dry-run sample node evaluator: DIV_BY_ZERO warning then normal NUMBER evaluation
    await page.getByTestId("expr-dryrun-input").fill("inventory / 0");
    await page.getByTestId("expr-dryrun-submit-btn").click();
    await expect(page.getByTestId("expr-dryrun-value-type")).toHaveText("NULL");
    await expect(page.getByTestId("expr-dryrun-warnings")).toContainText(
      "DIV_BY_ZERO"
    );

    await page
      .getByTestId("expr-dryrun-input")
      .fill("inventory / total_demand");
    await page.getByTestId("expr-dryrun-submit-btn").click();
    await expect(page.getByTestId("expr-dryrun-value")).toHaveText("1.5625");
    await expect(page.getByTestId("expr-dryrun-value-type")).toHaveText(
      "NUMBER"
    );
  }

  await page.getByTestId("inspector-tab-mappings").click();
  await expect(page.getByTestId("mapping-editor")).toBeVisible();
  if (!isLive) {
    await page.getByTestId("mapping-preview-btn").click();
    await expect(page.getByTestId("mapping-preview-result")).toContainText(
      "O-101"
    );
  }

  // 6b. Open Knowledge Candidates panel and accept candidate entity into Warehouse node
  await page.getByTestId("knowledge-candidates-btn").click();
  const candModal = page.getByTestId("knowledge-candidates-modal");
  await expect(candModal).toBeVisible();
  if (!isLive) {
    await expect(
      page.getByTestId(`knowledge-candidate-item-${CANDIDATE_ENTITY_ID}`)
    ).toContainText("杭州分拨仓");
    await expect(
      page.getByTestId(`knowledge-candidate-item-${CANDIDATE_ENTITY_ID}`)
    ).toContainText("长三角仓配作业规范.md");
    await page
      .getByTestId(`candidate-node-key-input-${CANDIDATE_ENTITY_ID}`)
      .fill("W-HZ-01");
    await page
      .getByTestId(`accept-candidate-btn-${CANDIDATE_ENTITY_ID}`)
      .click();
    await expect(
      page.getByTestId(`candidate-accepted-badge-${CANDIDATE_ENTITY_ID}`)
    ).toContainText("W-HZ-01");
  }
  await page.getByTestId("close-knowledge-candidates-btn").click();
  await expect(candModal).toHaveCount(0);

  // 7. Security & zero-error assertions
  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
