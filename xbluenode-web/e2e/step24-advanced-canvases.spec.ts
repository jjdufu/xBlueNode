import { expect, type Page, type Route, test } from "@playwright/test";

const DATASET_ORDERS_ID = "00000000-0000-0000-0000-000000002401";
const FLOW_ID = "00000000-0000-0000-0000-000000002410";
const RUN_ID = "00000000-0000-0000-0000-000000002411";
const SANDBOX_ID = "00000000-0000-0000-0000-000000002420";
const SCENARIO_ID = "00000000-0000-0000-0000-000000002421";
const NODE_F1_ID = "00000000-0000-0000-0000-000000002431";
const NODE_W1_ID = "00000000-0000-0000-0000-000000002432";
const NODE_ORD1_ID = "00000000-0000-0000-0000-000000002433";
const NODE_IDLE_ID = "00000000-0000-0000-0000-000000002434";

function attachMonitors(
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
  page.on("request", (req) => {
    const url = new URL(req.url());
    if (!url.pathname.startsWith("/api/")) return;
    if (url.searchParams.has("tenant_id")) {
      leakedTenantUrls.push(req.url());
    }
    const auth = req.headers()["authorization"] ?? "";
    if (
      url.pathname !== "/api/health" &&
      (!auth.startsWith("Bearer ") || auth.slice(7).trim().length === 0)
    ) {
      missingAuthUrls.push(req.url());
    }
  });
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

const STEP24_STUDIO_DOC = {
  node_types: [
    {
      name: "Supplier",
      display_name: "上游供应商",
      description: "核心原材料供应方",
      icon: "factory",
      color: "#0f766e",
      title_template: "{node_key}",
      semantic_layer_tier: "STATIC_FACT",
      domain_group: "采购域",
      properties: [
        {
          name: "lead_days",
          display_name: "交付天数",
          data_type: "INT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
        },
      ],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 96,
      pos_y: 128,
    },
    {
      name: "Warehouse",
      display_name: "区域中心仓",
      description: "区域仓储履约枢纽",
      icon: "warehouse",
      color: "#2563eb",
      title_template: "{node_key}",
      semantic_layer_tier: "STATIC_FACT",
      domain_group: "履约域",
      properties: [
        {
          name: "region",
          display_name: "大区",
          data_type: "STRING",
          kind: "ATTRIBUTE",
          required: true,
          pii_category: "NONE",
        },
        {
          name: "inventory",
          display_name: "在库量",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
        },
        {
          name: "safety_stock",
          display_name: "安全库存",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
        },
        {
          name: "outbound_sum",
          display_name: "累计出库",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
        },
      ],
      agg_rules: [
        {
          target_metric: "outbound_sum",
          op: "SUM",
          via_link_type: "RECORDS_OUTBOUND",
          direction: "OUT",
          source_type_name: "OutboundEvent",
          source_metric: "qty",
          weight_by_link: false,
        },
      ],
      derived_rules: [],
      sync_to_knowledge: true,
      pos_x: 512,
      pos_y: 128,
    },
    {
      name: "OutboundEvent",
      display_name: "出库流水事件",
      description: "仓库单笔出库明细",
      icon: "truck",
      color: "#d97706",
      title_template: "{node_key}",
      semantic_layer_tier: "EVENT_DYNAMIC",
      domain_group: "履约域",
      properties: [
        {
          name: "qty",
          display_name: "出库件数",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: true,
          pii_category: "NONE",
        },
      ],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 928,
      pos_y: 96,
    },
    {
      name: "DailyStockCube",
      display_name: "日度库存汇总立方",
      description: "仓库日度库存与出库汇总",
      icon: "database",
      color: "#7c3aed",
      title_template: "{node_key}",
      semantic_layer_tier: "CUBE_ROLLUP",
      domain_group: "履约域",
      properties: [
        {
          name: "daily_turnover",
          display_name: "日周转率",
          data_type: "FLOAT64",
          kind: "METRIC",
          required: false,
          pii_category: "NONE",
        },
      ],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 928,
      pos_y: 352,
    },
  ],
  edge_types: [
    {
      name: "SUPPLIES",
      src_type_name: "Supplier",
      dst_type_name: "Warehouse",
      impact_weight: 0.85,
      cardinality: "ONE_TO_MANY",
      description: "向中心仓供货",
      properties: [],
      propagation: {},
    },
    {
      name: "BACKUP_SUPPLIES",
      src_type_name: "Supplier",
      dst_type_name: "Warehouse",
      impact_weight: 0.45,
      cardinality: "MANY_TO_MANY",
      description: "应急备选供货通道",
      properties: [],
      propagation: {},
    },
    {
      name: "RECORDS_OUTBOUND",
      src_type_name: "Warehouse",
      dst_type_name: "OutboundEvent",
      impact_weight: 0.9,
      cardinality: "ONE_TO_MANY",
      description: "仓库产生出库流水",
      properties: [],
      propagation: {},
    },
    {
      name: "SUMMARIZES_STOCK",
      src_type_name: "Warehouse",
      dst_type_name: "DailyStockCube",
      impact_weight: 0.75,
      cardinality: "ONE_TO_MANY",
      description: "仓库归集日度切片",
      properties: [],
      propagation: {},
    },
  ],
  mappings: [
    {
      name: "wh_master_map",
      dataset: "ods_warehouses",
      node_type: "Warehouse",
      key_column: "wh_code",
      title: "{wh_code}",
      attributes: { region: "region" },
      metrics: { inventory: "stock_qty" },
      geo: { lng_column: null, lat_column: null },
      links: [],
      on_source_delete: "KEEP",
      mode: "INCREMENTAL",
    },
  ],
};

async function installStep24Mocks(page: Page): Promise<void> {
  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const method = req.method();
    const url = new URL(req.url());
    const { pathname } = url;

    if (pathname === "/api/health") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ status: "ok", service: "axpoint-core" }),
      });
      return;
    }

    if (pathname === "/api/auth/me") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          user_id: "00000000-0000-0000-0000-000000000001",
          tenant_id: "default",
          username: "admin",
          display_name: "系统管理员",
          department: "default",
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
            "SIM_RUN",
            "SIM_APPLY",
            "VIEW_EDIT",
          ],
        }),
      });
      return;
    }

    // xBlueStudio endpoints
    if (pathname === "/api/xblue/subgraphs" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000002400",
            slug: "main",
            display_name: "企业主图",
            description: "",
            owner_dept: "default",
            is_main: true,
            current_version: 1,
            created_at: "2026-03-01T08:00:00Z",
            updated_at: "2026-03-01T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/xblue/schema" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: "main",
          version: 1,
          ...STEP24_STUDIO_DOC,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema/validate" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ valid: true, issues: [], pass_count: 12 }),
      });
      return;
    }

    if (
      (pathname === "/api/xblue/drafts" ||
        pathname === "/api/xblue/schema/versions" ||
        pathname === "/api/xblue/mappings") &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    // AxHub datasets & lineage endpoints
    const CONN_ERP_ID = "00000000-0000-0000-0000-000000002499";
    if (pathname === "/api/axhub/connections" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: CONN_ERP_ID,
              name: "erp_pg",
              connector_type: "POSTGRES",
              config: { host: "127.0.0.1", port: 5432, database: "axpoint" },
              secret_fields_set: ["password"],
              allow_private_network: true,
              status: "OK",
              last_test_message: "PostgreSQL connected",
              last_tested_at: "2026-03-01T08:00:00Z",
              created_by: null,
              created_at: "2026-03-01T08:00:00Z",
              updated_at: "2026-03-01T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/connectors" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
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
              connection_id: CONN_ERP_ID,
              name: "ods_warehouses",
              display_name: "仓储主数据表",
              description: "",
              source_kind: "SQL_TABLE",
              source_ref: { schema: "public", table: "ods_warehouses" },
              sync_mode: "SNAPSHOT",
              primary_key: ["wh_code"],
              cursor_column: null,
              cursor_value: null,
              delete_column: null,
              schema_policy: "ADDITIVE",
              lake_schema: "t_default",
              lake_table: "ods_warehouses",
              schema_json: [],
              schema_version: 1,
              row_count: 64,
              last_snapshot_id: 1,
              last_synced_at: "2026-03-01T08:00:00Z",
              schedule_cron: null,
              tags: [],
              owner_user_id: null,
              status: "READY",
              created_at: "2026-03-01T08:00:00Z",
              updated_at: "2026-03-01T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axhub/datasets/${DATASET_ORDERS_ID}/snapshots` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (
      pathname === `/api/axhub/datasets/${DATASET_ORDERS_ID}/preview` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            {
              name: "wh_code",
              ordinal: 0,
              data_type: "STRING",
              nullable: false,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 1,
              sample_values: ["W-01"],
              pii_category: "NONE",
              pii_hit_ratio: 0,
              source_comment: "",
            },
          ],
          rows: [{ wh_code: "W-01" }],
        }),
      });
      return;
    }

    if (
      pathname === `/api/axhub/datasets/${DATASET_ORDERS_ID}/lineage` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          edges: [
            {
              id: 1,
              src_kind: "CONNECTION",
              src_id: CONN_ERP_ID,
              dst_kind: "DATASET",
              dst_id: DATASET_ORDERS_ID,
              via: "SNAPSHOT",
              created_at: "2026-03-01T08:05:00Z",
            },
            {
              id: 2,
              src_kind: "DATASET",
              src_id: DATASET_ORDERS_ID,
              dst_kind: "NODE_TYPE",
              dst_id: "Warehouse",
              via: "XBLUE_MAP",
              created_at: "2026-03-01T08:06:00Z",
            },
            {
              id: 3,
              src_kind: "NODE_TYPE",
              src_id: "Warehouse",
              dst_kind: "DASHBOARD",
              dst_id: "supply-cockpit",
              via: "DERIVED",
              created_at: "2026-03-01T08:07:00Z",
            },
          ],
        }),
      });
      return;
    }

    // AxFlow endpoints
    if (
      (pathname === "/api/axflow/flows" ||
        pathname === `/api/axflow/flows/${FLOW_ID}`) &&
      method === "GET"
    ) {
      const flowObj = {
        id: FLOW_ID,
        name: "supply_guard_flow",
        description: "库存巡检与审批编排",
        slug: "supply-guard",
        nodes: [
          {
            id: "sync_orders",
            kind: "SOURCE_INGEST",
            label: "数据源同步",
            config: { dataset_id: DATASET_ORDERS_ID, wait: true, timeout_s: 300 },
            retry: { max_attempts: 2, backoff_s: 2, backoff: "FIXED" },
            timeout_s: 300,
            on_failure: "FAIL_RUN",
            join: "ALL_SUCCESS",
            pos: { x: 80, y: 160 },
          },
          {
            id: "check",
            kind: "SQL_TRANSFORM",
            label: "库存核验",
            config: { sql: "SELECT 1 AS n", max_rows: 50 },
            retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" },
            timeout_s: 120,
            on_failure: "FAIL_RUN",
            join: "ALL_SUCCESS",
            pos: { x: 400, y: 160 },
          },
          {
            id: "approve",
            kind: "HUMAN_APPROVAL",
            label: "人工审批门禁",
            config: { title: "补货审批" },
            retry: { max_attempts: 1, backoff_s: 1, backoff: "FIXED" },
            timeout_s: 3600,
            on_failure: "FAIL_RUN",
            join: "ALL_SUCCESS",
            pos: { x: 752, y: 160 },
          },
        ],
        edges: [
          { from: "sync_orders", to: "check", when: null },
          { from: "check", to: "approve", when: "n > 0" },
        ],
        trigger_cfg: { type: "MANUAL" },
        status: "PUBLISHED",
        last_run_at: "2026-03-01T08:10:00Z",
        created_at: "2026-03-01T08:00:00Z",
        updated_at: "2026-03-01T08:10:00Z",
        issues: [],
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          pathname === "/api/axflow/flows"
            ? { items: [flowObj], next_cursor: null }
            : flowObj
        ),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${FLOW_ID}/versions` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], deployments: [], next_cursor: null }),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${FLOW_ID}/runs` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: RUN_ID,
              flow_id: FLOW_ID,
              version: 1,
              environment: "TEST",
              trigger_source: "MANUAL",
              trigger_payload: {},
              idempotency_key: null,
              status: "RUNNING",
              outputs: {},
              error_message: null,
              started_by: null,
              created_at: "2026-03-01T08:10:00.000Z",
              started_at: "2026-03-01T08:10:00.000Z",
              finished_at: null,
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axflow/runs/${RUN_ID}` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: RUN_ID,
          flow_id: FLOW_ID,
          version: 1,
          environment: "TEST",
          trigger_source: "MANUAL",
          trigger_payload: {},
          idempotency_key: null,
          status: "RUNNING",
          outputs: {},
          error_message: null,
          started_by: null,
          created_at: "2026-03-01T08:10:00.000Z",
          started_at: "2026-03-01T08:10:00.000Z",
          finished_at: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axflow/runs/${RUN_ID}/steps` && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: 1,
              run_id: RUN_ID,
              node_id: "sync_orders",
              attempt: 1,
              status: "SUCCEEDED",
              job_id: null,
              input: {},
              output: { rows_written: 64 },
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-03-01T08:10:00.000Z",
              finished_at: "2026-03-01T08:10:00.142Z",
            },
            {
              id: 2,
              run_id: RUN_ID,
              node_id: "check",
              attempt: 1,
              status: "RUNNING",
              job_id: null,
              input: {},
              output: {},
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-03-01T08:10:00.150Z",
              finished_at: null,
            },
          ],
        }),
      });
      return;
    }

    // AxSim endpoints
    if (pathname === "/api/axsim/sandboxes" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: SANDBOX_ID,
            name: "长三角断供因果沙盘",
            status: "READY",
            scope: { depth: 3, direction: "BOTH" },
            node_count: 4,
            link_count: 3,
            base_captured_at: "2026-03-01T08:00:00Z",
            schema_version: 1,
            sim_result: null,
            created_at: "2026-03-01T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === `/api/axsim/sandboxes/${SANDBOX_ID}/graph` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sandbox_id: SANDBOX_ID,
          overlay_hash: "h1",
          nodes: [
            {
              id: NODE_F1_ID,
              type_name: "Factory",
              node_key: "F-1",
              title: "昆山制造厂",
              metrics: { output: 1000 },
              properties: { domain_group: "制造域" },
              is_modified: false,
              is_new: false,
            },
            {
              id: NODE_W1_ID,
              type_name: "Warehouse",
              node_key: "W-1",
              title: "上海中心仓",
              metrics: { inventory: 500 },
              properties: { domain_group: "履约域" },
              is_modified: false,
              is_new: false,
            },
            {
              id: NODE_ORD1_ID,
              type_name: "Order",
              node_key: "ORD-1",
              title: "华东重点订单",
              metrics: { fulfillment: 100 },
              properties: { domain_group: "履约域" },
              is_modified: false,
              is_new: false,
            },
            {
              id: NODE_IDLE_ID,
              type_name: "Warehouse",
              node_key: "W-2",
              title: "广州备用仓",
              metrics: { inventory: 400 },
              properties: { domain_group: "履约域" },
              is_modified: false,
              is_new: false,
            },
          ],
          links: [
            {
              id: "sim-lk-1",
              link_type: "SUPPLIES",
              src_node_id: NODE_F1_ID,
              dst_node_id: NODE_W1_ID,
              weight: 0.85,
              is_modified: false,
              is_new: false,
            },
            {
              id: "sim-lk-2",
              link_type: "FULFILLS",
              src_node_id: NODE_W1_ID,
              dst_node_id: NODE_ORD1_ID,
              weight: 0.9,
              is_modified: false,
              is_new: false,
            },
            {
              id: "sim-lk-3",
              link_type: "TRANSFERS",
              src_node_id: NODE_W1_ID,
              dst_node_id: NODE_IDLE_ID,
              weight: 0.2,
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
        node_id: NODE_W1_ID,
        type_name: "Warehouse",
        title: "上海中心仓",
        metric: "inventory",
        before_val: 500,
        after_val: 380,
        delta: -120,
        delta_ratio: -0.24,
        p10: 355,
        p90: 405,
        hop_distance: 1,
        top_path: [NODE_F1_ID, NODE_W1_ID],
      },
      {
        node_id: NODE_ORD1_ID,
        type_name: "Order",
        title: "华东重点订单",
        metric: "fulfillment",
        before_val: 100,
        after_val: 78.4,
        delta: -21.6,
        delta_ratio: -0.216,
        p10: 73.6,
        p90: 83.2,
        hop_distance: 2,
        top_path: [NODE_F1_ID, NODE_W1_ID, NODE_ORD1_ID],
      },
    ];

    if (
      pathname === `/api/axsim/sandboxes/${SANDBOX_ID}/scenarios` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: SCENARIO_ID,
            sandbox_id: SANDBOX_ID,
            name: "stepped_cut_30",
            description: "减产 30% 分步推演",
            actions: [
              {
                target_node_id: NODE_F1_ID,
                metric_key: "output",
                mode: "RATIO",
                value: -0.3,
                at_step: 0,
              },
            ],
            params: {
              mode: "STEPPED",
              horizon_steps: 3,
              uncertainty: { samples: 64, coefficient_cv: 0.1, seed: 42 },
            },
            overlay_hash: "h1",
            is_stale: false,
            status: "SUCCEEDED",
            error_message: null,
            created_by: null,
            created_at: "2026-03-01T08:05:00Z",
            finished_at: "2026-03-01T08:05:01Z",
            result: {
              affected_count: 2,
              converged: true,
              iterations: 3,
              warnings: [],
              kpis: [],
              top_impacts: simImpacts,
            },
          },
        ]),
      });
      return;
    }

    if (
      pathname === `/api/axsim/scenarios/${SCENARIO_ID}/impacts` &&
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
      pathname === `/api/axsim/scenarios/${SCENARIO_ID}/checkpoints` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              seq: 0,
              label: "Step 0",
              changes: { [NODE_F1_ID]: { output: 700 } },
            },
            {
              seq: 1,
              label: "Step 1",
              changes: {
                [NODE_F1_ID]: { output: 700 },
                [NODE_W1_ID]: { inventory: 420 },
              },
            },
            {
              seq: 2,
              label: "Step 2",
              changes: {
                [NODE_F1_ID]: { output: 700 },
                [NODE_W1_ID]: { inventory: 380 },
                [NODE_ORD1_ID]: { fulfillment: 78.4 },
              },
            },
          ],
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

test("Step 24 C78: xBlueStudio orthogonal 0-overlap routing, 6-axis magnetic snap, domain swimlane + 3-tier folding, pipeline sync, and 50-step undo/redo", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  attachMonitors(
    page,
    leakedTenantUrls,
    missingAuthUrls,
    consoleErrors,
    pageErrors
  );

  await installStep24Mocks(page);
  await page.goto("/studio");

  await expect(page.getByTestId("xbluestudio-canvas")).toBeVisible();
  await expect(page.getByTestId("canvas-node-Supplier")).toBeVisible();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();
  await expect(page.getByTestId("canvas-node-OutboundEvent")).toBeVisible();
  await expect(page.getByTestId("canvas-node-DailyStockCube")).toBeVisible();

  // 1) Trigger ELK multi-port orthogonal layout and assert STRICT 0 overlap between all edge labels and node cards
  await page.getByTestId("canvas-elk-layout-btn").click();
  await page.waitForTimeout(320);

  const checkZeroOverlap = async () => {
    const rects = await page.evaluate(() => {
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

    expect(rects.nodes.length).toBeGreaterThanOrEqual(4);
    expect(rects.edges.length).toBeGreaterThanOrEqual(4);

    for (const edgeRect of rects.edges) {
      for (const nodeRect of rects.nodes) {
        const area = computeRectOverlapArea(edgeRect, nodeRect);
        expect(
          area,
          `Edge pill ${edgeRect.id} overlapped node card ${nodeRect.id} by ${area}px^2`
        ).toBe(0);
      }
    }
  };

  await checkZeroOverlap();

  // 2) Drag Supplier near Warehouse's Y coordinate to trigger 6-axis magnetic helper line, and Shift+Click multi-select toolbar
  const supplierCard = page.getByTestId("canvas-node-Supplier");
  const warehouseCard = page.getByTestId("canvas-node-Warehouse");
  const supBox = await supplierCard.boundingBox();
  const whBox = await warehouseCard.boundingBox();
  expect(supBox).not.toBeNull();
  expect(whBox).not.toBeNull();

  if (supBox && whBox) {
    const startX = supBox.x + supBox.width / 2;
    const startY = supBox.y + 24;
    const targetY = whBox.y + 24 + 3; // within 6px threshold of Warehouse top!
    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(startX + 12, targetY, { steps: 6 });
    await expect(page.getByTestId("canvas-helper-lines")).toBeVisible();
    await page.mouse.up();
    await expect(page.getByTestId("canvas-helper-lines")).toHaveCount(0);
  }

  await supplierCard.click();
  await warehouseCard.click({ modifiers: ["Shift"] });
  await expect(page.getByTestId("selection-align-toolbar")).toBeVisible();
  await page.getByTestId("align-btn-top").click();

  // 3) Enable compound Domain Group swimlanes & collapse/expand
  await page.getByTestId("canvas-domain-group-toggle").click();
  await expect(page.getByTestId("domain-group-履约域")).toBeVisible();
  await page.getByTestId("domain-group-toggle-履约域").click();
  await expect(page.getByTestId("canvas-node-Warehouse")).toHaveCount(0);
  await page.getByTestId("domain-group-toggle-履约域").click();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();
  await page.getByTestId("canvas-domain-group-toggle").click();

  // 4) 3-tier folding on STATIC_FACT Warehouse -> collapses OutboundEvent & DailyStockCube into bottom drawer
  await page.getByTestId("fold-tier-btn-Warehouse").click();
  await expect(page.getByTestId("canvas-node-OutboundEvent")).toHaveCount(0);
  await expect(page.getByTestId("canvas-node-DailyStockCube")).toHaveCount(0);
  await expect(page.getByTestId("folded-tier-drawer-Warehouse")).toBeVisible();

  await page.getByTestId("folded-tier-slot-OutboundEvent").click();
  await expect(page.getByTestId("node-type-name-input")).toHaveValue(
    "OutboundEvent"
  );

  await page.getByTestId("fold-tier-btn-Warehouse").click();
  await expect(page.getByTestId("canvas-node-OutboundEvent")).toBeVisible();

  // 5) Switch ONTOLOGY_TOPOLOGY <-> MAPPING_PIPELINE & test 50-step Undo/Redo (`ControlOrMeta+z` / `ControlOrMeta+Shift+z`)
  await page.getByTestId("studio-view-MAPPING_PIPELINE").click();
  await expect(page.getByTestId("studio-pipeline-canvas")).toBeVisible();
  await expect(page.getByTestId("pipeline-mapping-wh_master_map")).toBeVisible();

  await page.getByTestId("pipeline-add-mapping-btn").click();
  await page
    .getByTestId("pipeline-mapping-name-input")
    .fill("supplier_sync_map");
  await page.getByTestId("pipeline-mapping-submit-btn").click();
  await expect(
    page.getByTestId("pipeline-mapping-supplier_sync_map")
  ).toBeVisible();

  await page.keyboard.press("ControlOrMeta+z");
  await expect(
    page.getByTestId("pipeline-mapping-supplier_sync_map")
  ).toHaveCount(0);

  await page.keyboard.press("ControlOrMeta+Shift+z");
  await expect(
    page.getByTestId("pipeline-mapping-supplier_sync_map")
  ).toBeVisible();

  await page.getByTestId("studio-view-ONTOLOGY_TOPOLOGY").click();
  await expect(page.getByTestId("canvas-node-Warehouse")).toBeVisible();

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("Step 24 C79: AxFlow edge midpoint + operator splice with undo/redo & lazy @antv/g6 ImpactGraph (3-track card, Combo, Hull) + LineageGraph LR DAG", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  attachMonitors(
    page,
    leakedTenantUrls,
    missingAuthUrls,
    consoleErrors,
    pageErrors
  );

  await installStep24Mocks(page);

  // Part A: AxFlow edge midpoint `+` splice + runtime duration pill + undo/redo
  await page.goto("/flow");
  await expect(page.getByTestId("axflow-canvas")).toBeVisible();
  await expect(page.getByTestId("flow-edge-check-approve")).toContainText(
    "when: n > 0"
  );
  await expect(
    page.getByTestId("canvas-node-duration-sync_orders")
  ).toContainText("142ms");

  const insertBtn = page.getByTestId("flow-edge-insert-btn-check-approve");
  await insertBtn.hover();
  await insertBtn.click();

  const popover = page.getByTestId("quick-operator-popover");
  await expect(popover).toBeVisible();
  await page.getByTestId("quick-operator-search-input").fill("HTTP");
  await page.getByTestId("quick-op-item-HTTP_CALL").click();

  await expect(page.getByTestId("flow-node-http_call_1")).toBeVisible();
  await expect(page.getByTestId("flow-edge-check-approve")).toHaveCount(0);
  // Original `when: n > 0` guard is preserved on `(check, http_call_1)`
  await expect(page.getByTestId("flow-edge-check-http_call_1")).toContainText(
    "when: n > 0"
  );
  await expect(page.getByTestId("flow-edge-http_call_1-approve")).toBeVisible();

  // Undo atomic splice in a single step
  await page.getByTestId("axflow-undo-btn").click();
  await expect(page.getByTestId("flow-node-http_call_1")).toHaveCount(0);
  await expect(page.getByTestId("flow-edge-check-approve")).toContainText(
    "when: n > 0"
  );

  await page.getByTestId("axflow-redo-btn").click();
  await expect(page.getByTestId("flow-node-http_call_1")).toBeVisible();

  // Part B: Lazy @antv/g6 v5 ImpactGraph 3-track card, P10-P90 CI bar, Hull contour, Combo collapse/expand & step dimming
  await page.goto("/sim");
  const g6Canvas = page.getByTestId("impact-graph-g6-canvas");
  await expect(g6Canvas).toBeVisible();
  await expect(g6Canvas.locator("canvas").first()).toBeVisible();
  await expect(g6Canvas).toHaveAttribute("data-hull-count", "1");
  await expect(page.getByTestId("impact-hull-badge")).toBeVisible();

  // 3-track micro-card + P10-P90 4px bar
  const ordCard = page.getByTestId(`impact-graph-node-${NODE_ORD1_ID}`);
  await expect(ordCard).toBeVisible();
  await expect(
    page.getByTestId(`sim-card-tracks-${NODE_ORD1_ID}`)
  ).toContainText("78.4");
  await expect(page.getByTestId(`sim-card-ci-${NODE_ORD1_ID}`)).toContainText(
    "P10 73.6"
  );

  // Scrub ReplayTimeline to Step 0 -> ORD-1 is not yet reached and dims to 35% opacity
  const slider = page.getByTestId("replay-step-slider");
  await expect(slider).toBeVisible();
  await slider.fill("0");
  await expect(ordCard).toHaveAttribute("data-dimmed", "true");
  await slider.fill("2");
  await expect(ordCard).toHaveAttribute("data-dimmed", "false");

  // Collapse & expand domain Combo
  const comboToggle = page.getByTestId("impact-combo-toggle-履约域");
  await comboToggle.click();
  await expect(comboToggle).toHaveAttribute("data-collapsed", "true");
  await comboToggle.click();
  await expect(comboToggle).toHaveAttribute("data-collapsed", "false");

  // Part C: Lazy @antv/g6 v5 LineageGraph deterministic `antv-dagre` LR DAG & closure highlight
  await page.goto("/hub");
  await page.getByTestId("preview-dataset-btn-ods_warehouses").click();
  await page.getByTestId("dataset-tab-lineage").click();

  const lineageContainer = page.getByTestId("lineage-graph-canvas");
  await expect(lineageContainer.locator("canvas").first()).toBeVisible();
  await expect(
    page.getByTestId("lineage-node-NODE_TYPE:Warehouse")
  ).toBeVisible();
  await page.getByTestId("lineage-node-NODE_TYPE:Warehouse").click();
  await expect(page.getByTestId("lineage-clear-focus-btn")).toBeVisible();

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
