import { expect, type Page, type Route, test } from "@playwright/test";

const FLOW_DAILY_ID = "00000000-0000-0000-0000-000000000701";
const FLOW_CYCLIC_ID = "00000000-0000-0000-0000-000000000702";
const RUN_1_ID = "00000000-0000-0000-0000-000000000711";
const APPROVAL_1_ID = "00000000-0000-0000-0000-000000000721";

interface MockFlowState {
  runStarted: boolean;
  approvalDecided: boolean;
  lastRunRequestBody: Record<string, unknown> | null;
  lastDecisionBody: Record<string, unknown> | null;
  publishedVersion: number;
  prodVersion: number;
  testVersion: number;
}

const DAILY_SUPPLY_NODES = [
  {
    id: "sync_orders",
    kind: "SOURCE_INGEST",
    label: "同步订单数据入湖",
    config: {
      dataset_id: "00000000-0000-0000-0000-000000000202",
      wait: true,
      timeout_s: 300,
    },
    retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
    timeout_s: 600,
    on_failure: "FAIL_RUN",
    join: "ALL_SUCCESS",
    pos: { x: 40, y: 140 },
  },
  {
    id: "map_orders",
    kind: "XBLUE_MAP",
    label: "映射订单业务节点",
    config: {
      mapping_id: "00000000-0000-0000-0000-000000000530",
      mode: "INCREMENTAL",
    },
    retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
    timeout_s: 600,
    on_failure: "FAIL_RUN",
    join: "ALL_SUCCESS",
    pos: { x: 280, y: 140 },
  },
  {
    id: "agg",
    kind: "AGG_TRIGGER",
    label: "重算仓储动态聚合指标",
    config: { type_name: "Warehouse" },
    retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
    timeout_s: 600,
    on_failure: "FAIL_RUN",
    join: "ALL_SUCCESS",
    pos: { x: 520, y: 140 },
  },
  {
    id: "check",
    kind: "SQL_TRANSFORM",
    label: "核验已支付待履约订单数",
    config: {
      sql: "SELECT count(*) AS n FROM demo_orders WHERE status = 'PAID'",
      max_rows: 10,
    },
    retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
    timeout_s: 600,
    on_failure: "FAIL_RUN",
    join: "ALL_SUCCESS",
    pos: { x: 40, y: 320 },
  },
  {
    id: "approve",
    kind: "HUMAN_APPROVAL",
    label: "补货人工审批门禁",
    config: {
      title: "补货审批",
      detail_template: { warehouse: "W-1", paid_orders: "{{ nodes.check.output.row_count }}" },
      assignee_roles: ["ADMIN", "OPERATOR"],
      assignee_users: [],
      expires_in_minutes: 1440,
    },
    retry: { max_attempts: 1, backoff_s: 10, backoff: "FIXED" },
    timeout_s: 3600,
    on_failure: "FAIL_RUN",
    join: "ALL_SUCCESS",
    pos: { x: 300, y: 320 },
  },
  {
    id: "emit",
    kind: "ACTION_EMIT",
    label: "下发补货指标回写",
    config: {
      actions: [
        {
          kind: "WRITE_NODE_METRIC",
          node_ref: "Warehouse:W-1",
          metric: "replenish_flag",
          value: 1,
        },
      ],
    },
    retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
    timeout_s: 600,
    on_failure: "FAIL_RUN",
    join: "ALL_SUCCESS",
    pos: { x: 560, y: 320 },
  },
];

const DAILY_SUPPLY_EDGES = [
  { from: "sync_orders", to: "map_orders" },
  { from: "map_orders", to: "agg" },
  { from: "agg", to: "check" },
  { from: "check", to: "approve", when: "n > 0" },
  { from: "approve", to: "emit" },
];

async function installAxFlowMocks(
  page: Page,
  state: MockFlowState,
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
          trace_id: "axflow-e2e-400",
        }),
      });
      return;
    }

    const postDataText = request.postData();
    if (postDataText) {
      try {
        const parsedBody: unknown = JSON.parse(postDataText);
        if (
          typeof parsedBody === "object" &&
          parsedBody !== null &&
          !Array.isArray(parsedBody) &&
          "tenant_id" in parsedBody
        ) {
          leakedTenantUrls.push(request.url());
          await route.fulfill({
            status: 400,
            contentType: "application/json",
            body: JSON.stringify({
              error_code: "INVALID_ARGUMENT",
              message: "tenant_id body field is forbidden",
              trace_id: "axflow-e2e-body-400",
            }),
          });
          return;
        }
      } catch {
        // non-json payload
      }
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
          trace_id: "axflow-e2e-401",
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
            "FLOW_EDIT",
            "FLOW_RUN",
            "FLOW_PUBLISH",
            "APPROVAL_DECIDE",
          ],
          token_kind: "DEV_STATIC",
        }),
      });
      return;
    }

    const dailyFlowRecord = {
      id: FLOW_DAILY_ID,
      name: "daily_supply_refresh",
      description: "供应链日度订单同步、聚合核验、人工审批与补货动作回写",
      slug: "daily-supply-refresh",
      nodes: DAILY_SUPPLY_NODES,
      edges: DAILY_SUPPLY_EDGES,
      trigger_cfg: { type: "MANUAL" },
      status: "PUBLISHED",
      last_run_at: state.runStarted ? "2026-02-01T08:00:05Z" : null,
      created_at: "2026-02-01T07:00:00Z",
      updated_at: "2026-02-01T07:30:00Z",
      issues: [],
    };

    const cyclicFlowRecord = {
      id: FLOW_CYCLIC_ID,
      name: "cyclic_demo",
      description: "含环检测演示草稿",
      slug: null,
      nodes: [
        {
          id: "step_a",
          kind: "SQL_TRANSFORM",
          label: "节点 A",
          config: { sql: "SELECT 1" },
          retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
          timeout_s: 600,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 100, y: 140 },
        },
        {
          id: "step_b",
          kind: "AGG_TRIGGER",
          label: "节点 B",
          config: {},
          retry: { max_attempts: 3, backoff_s: 10, backoff: "FIXED" },
          timeout_s: 600,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 360, y: 140 },
        },
      ],
      edges: [
        { from: "step_a", to: "step_b" },
        { from: "step_b", to: "step_a" },
      ],
      trigger_cfg: { type: "MANUAL" },
      status: "DRAFT",
      last_run_at: null,
      created_at: "2026-02-01T07:10:00Z",
      updated_at: "2026-02-01T07:10:00Z",
      issues: [],
    };

    if (pathname === "/api/axflow/operators" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/axflow/flows" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [dailyFlowRecord, cyclicFlowRecord],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${FLOW_DAILY_ID}`) {
      if (request.method() === "PUT") {
        const body = JSON.parse(request.postData() ?? "{}") as Record<
          string,
          unknown
        >;
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ...dailyFlowRecord,
            ...body,
            issues: [],
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(dailyFlowRecord),
      });
      return;
    }

    if (pathname === `/api/axflow/flows/${FLOW_CYCLIC_ID}`) {
      if (request.method() === "PUT") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(cyclicFlowRecord),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(cyclicFlowRecord),
      });
      return;
    }

    if (
      pathname === `/api/axflow/flows/${FLOW_CYCLIC_ID}/validate` &&
      request.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          valid: false,
          issues: [
            {
              code: "CYCLE_DETECTED",
              message: "检测到有向环路: step_a -> step_b -> step_a",
              severity: "ERROR",
              node_id: "step_a",
            },
          ],
        }),
      });
      return;
    }

    if (
      pathname === `/api/axflow/flows/${FLOW_DAILY_ID}/versions` &&
      request.method() === "GET"
    ) {
      const items = [];
      for (let v = state.publishedVersion; v >= 1; v -= 1) {
        items.push({
          id: `00000000-0000-0000-0000-00000000073${v}`,
          flow_id: FLOW_DAILY_ID,
          version: v,
          nodes: DAILY_SUPPLY_NODES,
          edges: DAILY_SUPPLY_EDGES,
          trigger_cfg: { type: "MANUAL" },
          note: v === 1 ? "初始供应链补货编排基线" : `迭代版本 v${v}`,
          published_by: "00000000-0000-0000-0000-000000000000",
          published_at: "2026-02-01T07:30:00Z",
        });
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items,
          deployments: [
            {
              flow_id: FLOW_DAILY_ID,
              environment: "TEST",
              version: state.testVersion,
              deployed_by: "00000000-0000-0000-0000-000000000000",
              deployed_at: "2026-02-01T07:31:00Z",
            },
            {
              flow_id: FLOW_DAILY_ID,
              environment: "PROD",
              version: state.prodVersion,
              deployed_by: "00000000-0000-0000-0000-000000000000",
              deployed_at: "2026-02-01T07:32:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axflow/flows/${FLOW_DAILY_ID}/versions` &&
      request.method() === "POST"
    ) {
      state.publishedVersion += 1;
      const body = JSON.parse(request.postData() ?? "{}") as { note?: string };
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: `00000000-0000-0000-0000-00000000073${state.publishedVersion}`,
          flow_id: FLOW_DAILY_ID,
          version: state.publishedVersion,
          nodes: DAILY_SUPPLY_NODES,
          edges: DAILY_SUPPLY_EDGES,
          trigger_cfg: { type: "MANUAL" },
          note: body.note ?? "",
          published_by: "00000000-0000-0000-0000-000000000000",
          published_at: "2026-02-01T08:10:00Z",
        }),
      });
      return;
    }

    if (
      pathname === `/api/axflow/flows/${FLOW_DAILY_ID}/deploy` &&
      request.method() === "POST"
    ) {
      const body = JSON.parse(request.postData() ?? "{}") as {
        environment: "TEST" | "PROD";
        version: number;
      };
      if (body.environment === "PROD") {
        state.prodVersion = body.version;
      } else {
        state.testVersion = body.version;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          flow_id: FLOW_DAILY_ID,
          environment: body.environment,
          version: body.version,
          deployed_by: "00000000-0000-0000-0000-000000000000",
          deployed_at: "2026-02-01T08:12:00Z",
        }),
      });
      return;
    }

    if (
      pathname === `/api/axflow/flows/${FLOW_DAILY_ID}/hook` &&
      request.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: "00000000-0000-0000-0000-000000000799",
          flow_id: FLOW_DAILY_ID,
          slug: "daily-supply-refresh",
          path: "/api/axflow/hooks/default/daily-supply-refresh",
          secret: "axh_onetime_secret_9f8e7d6c5b4a",
          created_at: "2026-02-01T08:15:00Z",
        }),
      });
      return;
    }

    const buildCurrentRunRecord = () => ({
      id: RUN_1_ID,
      flow_id: FLOW_DAILY_ID,
      version: 1,
      environment: "TEST",
      trigger_source: "MANUAL",
      trigger_payload: {},
      idempotency_key: null,
      status: state.approvalDecided ? "SUCCEEDED" : "WAITING_APPROVAL",
      outputs: state.approvalDecided
        ? { emit: { done: 0, dry_run: 1, failed: 0 } }
        : {},
      error_message: null,
      started_by: "00000000-0000-0000-0000-000000000000",
      created_at: "2026-02-01T08:00:00Z",
      started_at: "2026-02-01T08:00:00Z",
      finished_at: state.approvalDecided ? "2026-02-01T08:00:08Z" : null,
    });

    if (
      pathname === `/api/axflow/flows/${FLOW_DAILY_ID}/runs` &&
      request.method() === "POST"
    ) {
      state.runStarted = true;
      state.lastRunRequestBody = JSON.parse(
        request.postData() ?? "{}"
      ) as Record<string, unknown>;
      await route.fulfill({
        status: 202,
        contentType: "application/json",
        body: JSON.stringify({
          run_id: RUN_1_ID,
          status: "WAITING_APPROVAL",
          existing: false,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axflow/flows/${FLOW_DAILY_ID}/runs` &&
      request.method() === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.runStarted ? [buildCurrentRunRecord()] : [],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axflow/runs/${RUN_1_ID}` && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(buildCurrentRunRecord()),
      });
      return;
    }

    if (
      pathname === `/api/axflow/runs/${RUN_1_ID}/steps` &&
      request.method() === "GET"
    ) {
      const upstreamSteps = [
        {
          id: 1,
          run_id: RUN_1_ID,
          node_id: "sync_orders",
          attempt: 1,
          status: "SUCCEEDED",
          job_id: null,
          input: { wait: true },
          output: { rows_written: 42, snapshot_id: 7 },
          output_ref: null,
          error_code: null,
          error_message: null,
          started_at: "2026-02-01T08:00:00Z",
          finished_at: "2026-02-01T08:00:01Z",
        },
        {
          id: 2,
          run_id: RUN_1_ID,
          node_id: "map_orders",
          attempt: 1,
          status: "SUCCEEDED",
          job_id: null,
          input: { mode: "INCREMENTAL" },
          output: { nodes_upserted: 42, links_upserted: 42 },
          output_ref: null,
          error_code: null,
          error_message: null,
          started_at: "2026-02-01T08:00:01Z",
          finished_at: "2026-02-01T08:00:02Z",
        },
        {
          id: 3,
          run_id: RUN_1_ID,
          node_id: "agg",
          attempt: 1,
          status: "SUCCEEDED",
          job_id: null,
          input: { type_name: "Warehouse" },
          output: { nodes_changed: 1 },
          output_ref: null,
          error_code: null,
          error_message: null,
          started_at: "2026-02-01T08:00:02Z",
          finished_at: "2026-02-01T08:00:03Z",
        },
        {
          id: 4,
          run_id: RUN_1_ID,
          node_id: "check",
          attempt: 1,
          status: "SUCCEEDED",
          job_id: null,
          input: { sql: "SELECT count(*) AS n FROM demo_orders WHERE status = 'PAID'" },
          output: { n: 12, row_count: 1, rows: [{ n: 12 }] },
          output_ref: null,
          error_code: null,
          error_message: null,
          started_at: "2026-02-01T08:00:03Z",
          finished_at: "2026-02-01T08:00:04Z",
        },
      ];

      const steps = state.approvalDecided
        ? [
            ...upstreamSteps,
            {
              id: 5,
              run_id: RUN_1_ID,
              node_id: "approve",
              attempt: 1,
              status: "SUCCEEDED",
              job_id: null,
              input: { title: "补货审批" },
              output: { approved: true, comment: "同意补货" },
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-02-01T08:00:04Z",
              finished_at: "2026-02-01T08:00:07Z",
            },
            {
              id: 6,
              run_id: RUN_1_ID,
              node_id: "emit",
              attempt: 1,
              status: "SUCCEEDED",
              job_id: null,
              input: { actions: [{ kind: "WRITE_NODE_METRIC", metric: "replenish_flag", value: 1 }] },
              output: { done: 0, dry_run: 1, failed: 0 },
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-02-01T08:00:07Z",
              finished_at: "2026-02-01T08:00:08Z",
            },
          ]
        : [
            ...upstreamSteps,
            {
              id: 5,
              run_id: RUN_1_ID,
              node_id: "approve",
              attempt: 1,
              status: "WAITING_APPROVAL",
              job_id: null,
              input: { title: "补货审批" },
              output: null,
              output_ref: null,
              error_code: null,
              error_message: null,
              started_at: "2026-02-01T08:00:04Z",
              finished_at: null,
            },
          ];

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: steps, next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/axflow/approvals" && request.method() === "GET") {
      const statusParam = searchParams.get("status");
      const showPending =
        state.runStarted &&
        !state.approvalDecided &&
        (!statusParam || statusParam === "PENDING");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: showPending
            ? [
                {
                  id: APPROVAL_1_ID,
                  run_id: RUN_1_ID,
                  node_id: "approve",
                  source_kind: "FLOW",
                  source_ref: `${RUN_1_ID}:approve`,
                  title: "补货审批",
                  detail: { warehouse: "W-1", paid_orders: 12 },
                  assignee_roles: ["ADMIN", "OPERATOR"],
                  assignee_users: [],
                  status: "PENDING",
                  decided_by: null,
                  decided_at: null,
                  comment: "",
                  expires_at: "2026-02-02T08:00:00Z",
                  created_at: "2026-02-01T08:00:04Z",
                },
              ]
            : [],
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axflow/approvals/${APPROVAL_1_ID}/decide` &&
      request.method() === "POST"
    ) {
      state.approvalDecided = true;
      state.lastDecisionBody = JSON.parse(
        request.postData() ?? "{}"
      ) as Record<string, unknown>;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: APPROVAL_1_ID,
          run_id: RUN_1_ID,
          node_id: "approve",
          source_kind: "FLOW",
          source_ref: `${RUN_1_ID}:approve`,
          title: "补货审批",
          detail: { warehouse: "W-1", paid_orders: 12 },
          assignee_roles: ["ADMIN", "OPERATOR"],
          assignee_users: [],
          status: "APPROVED",
          decided_by: "00000000-0000-0000-0000-000000000000",
          decided_at: "2026-02-01T08:00:07Z",
          comment: String(state.lastDecisionBody.comment ?? ""),
          expires_at: "2026-02-02T08:00:00Z",
          created_at: "2026-02-01T08:00:04Z",
        }),
      });
      return;
    }

    if (pathname === "/api/axflow/actions" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.approvalDecided
            ? [
                {
                  id: 101,
                  run_id: RUN_1_ID,
                  node_id: "emit",
                  source_kind: "FLOW",
                  action_kind: "WRITE_NODE_METRIC",
                  target: "Warehouse:W-1",
                  payload: { metric: "replenish_flag", value: 1 },
                  status: "DRY_RUN",
                  idempotency_key: `${RUN_1_ID}:emit:0`,
                  error_message: null,
                  created_at: "2026-02-01T08:00:08Z",
                },
              ]
            : [],
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

test("daily_supply_refresh DAG loads on canvas, pauses at HUMAN_APPROVAL, and completes after approval decision", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  const mockState: MockFlowState = {
    runStarted: false,
    approvalDecided: false,
    lastRunRequestBody: null,
    lastDecisionBody: null,
    publishedVersion: 1,
    prodVersion: 1,
    testVersion: 1,
  };

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  await installAxFlowMocks(page, mockState, leakedTenantUrls, missingAuthUrls);

  // 1) Navigate to /flow and verify heading, deployment badges, and 6 DAG nodes + conditional edge
  await page.goto("/flow");
  await expect(
    page.getByRole("heading", { level: 1, name: "AxFlow · 智能编排流水线" })
  ).toBeVisible();

  await expect(page.getByTestId("axflow-badge-test")).toHaveText("TEST v1");
  await expect(page.getByTestId("axflow-badge-prod")).toHaveText("PROD v1");

  for (const nodeId of [
    "sync_orders",
    "map_orders",
    "agg",
    "check",
    "approve",
    "emit",
  ]) {
    await expect(page.getByTestId(`flow-node-${nodeId}`)).toBeVisible();
  }
  await expect(page.getByTestId("flow-edge-check-approve")).toContainText(
    "when: n > 0"
  );

  // 2) Trigger run -> timeline renders upstream SUCCEEDED steps and approve in WAITING_APPROVAL
  await page.getByTestId("axflow-run-btn").click();
  expect(mockState.lastRunRequestBody).toMatchObject({ environment: "TEST" });

  await expect(page.getByTestId("axflow-run-timeline")).toBeVisible();
  await expect(page.getByTestId("timeline-run-status")).toContainText(
    "WAITING_APPROVAL"
  );
  await expect(page.getByTestId("timeline-step-status-sync_orders")).toHaveText(
    "SUCCEEDED"
  );
  await expect(page.getByTestId("timeline-step-status-check")).toHaveText(
    "SUCCEEDED"
  );
  await expect(page.getByTestId("timeline-step-status-approve")).toHaveText(
    "WAITING_APPROVAL"
  );
  await expect(page.getByTestId("canvas-node-status-approve")).toContainText(
    "WAITING_APPROVAL"
  );

  // 3) Pending approval appears in ApprovalInbox -> fill comment "同意补货" -> click "通过"
  const approvalCard = page.getByTestId(`approval-item-${APPROVAL_1_ID}`);
  await expect(approvalCard).toBeVisible();
  await expect(approvalCard).toContainText("补货审批");
  await expect(approvalCard).toContainText("HUMAN_APPROVAL");

  await page.getByTestId(`approval-comment-${APPROVAL_1_ID}`).fill("同意补货");
  await page.getByTestId(`approval-approve-${APPROVAL_1_ID}`).click();

  expect(mockState.lastDecisionBody).toEqual({
    decision: "APPROVE",
    comment: "同意补货",
  });

  // Verify approval clears from pending inbox and all 6 steps transition to SUCCEEDED
  await expect(page.getByTestId("approval-inbox-empty")).toBeVisible();
  await expect(page.getByTestId("timeline-run-status")).toContainText(
    "SUCCEEDED"
  );
  for (const nodeId of [
    "sync_orders",
    "map_orders",
    "agg",
    "check",
    "approve",
    "emit",
  ]) {
    await expect(page.getByTestId(`timeline-step-status-${nodeId}`)).toHaveText(
      "SUCCEEDED"
    );
  }
  await expect(page.getByTestId("timeline-action-logs")).toContainText(
    "DRY_RUN"
  );

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("supports operator palette authoring, webhook secret rotation, version deploy, and CYCLE_DETECTED validation", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  const mockState: MockFlowState = {
    runStarted: false,
    approvalDecided: false,
    lastRunRequestBody: null,
    lastDecisionBody: null,
    publishedVersion: 1,
    prodVersion: 1,
    testVersion: 1,
  };

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      consoleErrors.push(msg.text());
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  await installAxFlowMocks(page, mockState, leakedTenantUrls, missingAuthUrls);
  await page.goto("/flow");

  // Select `approve` node and verify strict DAG ancestor placeholder chips appear
  await page.getByTestId("flow-node-approve").click();
  await expect(page.getByTestId("ancestor-template-helper")).toContainText(
    "{{ nodes.check.output.row_count }}"
  );

  // Add an operator via accessible palette button
  await page.getByTestId("palette-add-HTTP_CALL").click();
  await expect(page.getByTestId("flow-node-http_call_1")).toBeVisible();

  // Switch to Trigger tab, select WEBHOOK, and rotate one-time secret
  await page.getByTestId("inspector-tab-trigger").click();
  await page.getByTestId("trigger-type-select").selectOption("WEBHOOK");
  await expect(page.getByTestId("trigger-webhook-url")).toContainText(
    "/api/axflow/hooks/default/daily-supply-refresh"
  );
  await page.getByTestId("rotate-webhook-secret-btn").click();
  await expect(page.getByTestId("webhook-secret-box")).toContainText(
    "axh_onetime_secret_9f8e7d6c5b4a"
  );

  // Switch to Version & Deploy tab, publish v2, and deploy to PROD
  await page.getByTestId("inspector-tab-version").click();
  await page.getByTestId("publish-note-input").fill("新增外部回调节点");
  await page.getByTestId("publish-version-btn").click();
  await expect(page.getByTestId("version-item-v2")).toBeVisible();
  await page.getByTestId("deploy-prod-v2").click();
  await expect(page.getByTestId("axflow-badge-prod")).toHaveText("PROD v2");

  // Switch to cyclic_demo flow and verify static validation reports CYCLE_DETECTED
  await page.getByTestId("axflow-flow-select").selectOption(FLOW_CYCLIC_ID);
  await page.getByTestId("validate-flow-btn").click();
  await expect(page.getByTestId("flow-validation-issues")).toContainText(
    "CYCLE_DETECTED"
  );

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
