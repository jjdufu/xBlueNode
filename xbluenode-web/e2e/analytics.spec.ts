import { expect, type Page, type Route, test } from "@playwright/test";
import { chooseSelectOption } from "./helpers";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const DEMO_DATASET_ID = "00000000-0000-0000-0000-000000000202";
const DEMO_METRIC_ID = "00000000-0000-0000-0000-000000000401";
const DEMO_SESSION_ID = "00000000-0000-0000-0000-000000000410";
const DEMO_TURN_1_ID = "00000000-0000-0000-0000-000000000411";
const DEMO_TURN_2_ID = "00000000-0000-0000-0000-000000000412";
const DEMO_DRAFT_ID = "00000000-0000-0000-0000-000000000420";

interface FeedbackCapture {
  turnId: string;
  body: Record<string, unknown>;
}

interface ChatQueryCapture {
  body: Record<string, unknown>;
}

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

async function installAnalyticsMocks(
  page: Page,
  feedbackCaptures: FeedbackCapture[],
  chatCaptures: ChatQueryCapture[]
): Promise<void> {
  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const request = route.request();
    const url = new URL(request.url());
    const { pathname, searchParams } = url;

    if (searchParams.has("tenant_id")) {
      await route.fulfill({
        status: 400,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "INVALID_ARGUMENT",
          message: "tenant_id query parameter is forbidden",
          trace_id: "analytics-e2e-400",
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
          trace_id: "analytics-e2e-401",
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/catalog" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          datasets: [
            {
              id: DEMO_DATASET_ID,
              name: "demo_orders",
              display_name: "演示订单表",
              lake_table: "demo_orders",
              schema_version: 1,
              row_count: 100,
              columns: [
                {
                  dataset_id: DEMO_DATASET_ID,
                  column_name: "id",
                  data_type: "INT64",
                  pii_category: "NONE",
                  pii_confirmed: false,
                  description: "订单主键",
                  business_term: "",
                  unit: "",
                  ordinal: 0,
                },
                {
                  dataset_id: DEMO_DATASET_ID,
                  column_name: "status",
                  data_type: "STRING",
                  pii_category: "NONE",
                  pii_confirmed: false,
                  description: "履约状态",
                  business_term: "订单状态",
                  unit: "",
                  ordinal: 1,
                },
                {
                  dataset_id: DEMO_DATASET_ID,
                  column_name: "amount",
                  data_type: "DECIMAL",
                  pii_category: "NONE",
                  pii_confirmed: false,
                  description: "订单金额",
                  business_term: "交易金额",
                  unit: "CNY",
                  ordinal: 2,
                },
                {
                  dataset_id: DEMO_DATASET_ID,
                  column_name: "phone",
                  data_type: "STRING",
                  pii_category: "PHONE",
                  pii_confirmed: true,
                  description: "收货人手机号",
                  business_term: "",
                  unit: "",
                  ordinal: 3,
                },
                {
                  dataset_id: DEMO_DATASET_ID,
                  column_name: "updated_at",
                  data_type: "TIMESTAMP",
                  pii_category: "NONE",
                  pii_confirmed: false,
                  description: "更新时间",
                  business_term: "",
                  unit: "",
                  ordinal: 4,
                },
              ],
            },
          ],
          platform_views: [
            {
              name: "ax.nodes",
              description: "xBlue 业务节点联邦只读视图",
              columns: [
                {
                  name: "id",
                  data_type: "STRING",
                  pii_category: "NONE",
                  masked: false,
                },
                {
                  name: "type_name",
                  data_type: "STRING",
                  pii_category: "NONE",
                  masked: false,
                },
                {
                  name: "title",
                  data_type: "STRING",
                  pii_category: "NONE",
                  masked: false,
                },
              ],
            },
          ],
          live_connections: [
            {
              id: "00000000-0000-0000-0000-000000000201",
              name: "demo_pg",
              alias: "live_demo_pg",
              connector_type: "POSTGRES",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/sql/run" && request.method() === "POST") {
      const reqBody = JSON.parse(request.postData() ?? "{}") as {
        sql?: string;
      };
      const sql = reqBody.sql ?? "";
      if (/count\s*\(\s*\*\s*\)/i.test(sql)) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            columns: [
              {
                name: "count_star()",
                data_type: "INT64",
                pii_category: "NONE",
                masked: false,
              },
            ],
            rows: [{ "count_star()": 100 }],
            row_count: 1,
            truncated: false,
            sql: 'SELECT * FROM (SELECT count(*) FROM lake."tenant_default"."demo_orders") AS q LIMIT 1001',
            duration_ms: 12,
          }),
        });
        return;
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            {
              name: "id",
              data_type: "INT64",
              pii_category: "NONE",
              masked: false,
            },
            {
              name: "status",
              data_type: "STRING",
              pii_category: "NONE",
              masked: false,
            },
            {
              name: "amount",
              data_type: "DECIMAL",
              pii_category: "NONE",
              masked: false,
            },
            {
              name: "phone",
              data_type: "STRING",
              pii_category: "PHONE",
              masked: true,
            },
          ],
          rows: [
            {
              id: 1,
              status: "PAID",
              amount: "120.50",
              phone: "138****0001",
            },
          ],
          row_count: 1,
          truncated: false,
          sql,
          duration_ms: 9,
        }),
      });
      return;
    }

    if (
      pathname === "/api/analytics/sql/explain" &&
      request.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sql: 'SELECT * FROM (SELECT count(*) FROM lake."tenant_default"."demo_orders") AS q LIMIT 1001',
          plan: "UNGROUPED_AGGREGATE count_star()\n  SEQ_SCAN demo_orders",
          referenced_datasets: [DEMO_DATASET_ID],
          uses_platform_views: false,
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/pivot" && request.method() === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          row_headers: [["PAID"], ["PENDING"], ["CANCELED"], ["总计"]],
          column_headers: [["SUM(amount)"]],
          cells: [["3517.50"], ["980.00"], ["210.00"], ["4707.50"]],
          row_kinds: ["DATA", "DATA", "DATA", "GRAND_TOTAL"],
          columns_truncated: false,
          sql: 'SELECT "status", SUM("amount") FROM lake."tenant_default"."demo_orders" GROUP BY GROUPING SETS (("status"), ())',
          duration_ms: 18,
        }),
      });
      return;
    }

    if (pathname === "/api/analytics/metrics" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: DEMO_METRIC_ID,
            name: "paid_amount",
            display_name: "已支付订单总金额",
            description: "状态为 PAID 的订单金额合计",
            dataset_id: DEMO_DATASET_ID,
            expression: "SUM(amount)",
            filter_sql: "status = 'PAID'",
            time_column: "updated_at",
            default_grain: "MONTH",
            dimensions: [
              { name: "status", column: "status", display_name: "订单状态" },
            ],
            unit: "CNY",
            format: "0,0.00",
            synonyms: ["已付款金额", "实收金额"],
            certified: true,
            schema_version: 1,
            broken: false,
            owner_user_id: null,
            version: 1,
            created_at: "2026-01-31T08:00:00Z",
            updated_at: "2026-01-31T08:00:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === "/api/analytics/metrics/paid_amount/query" &&
      request.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          metrics: ["paid_amount"],
          columns: [
            {
              name: "period",
              data_type: "TIMESTAMP",
              pii_category: "NONE",
              masked: false,
            },
            {
              name: "value",
              data_type: "DECIMAL",
              pii_category: "NONE",
              masked: false,
            },
          ],
          rows: [{ period: "2026-01-01T00:00:00Z", value: "3517.50" }],
          sql: 'SELECT date_trunc(\'month\', "updated_at") AS period, SUM(amount) AS value FROM demo_orders WHERE status = \'PAID\' GROUP BY ALL',
          duration_ms: 15,
        }),
      });
      return;
    }

    if (
      pathname === "/api/analytics/transform/draft" &&
      request.method() === "POST"
    ) {
      const body = JSON.parse(request.postData() ?? "{}") as {
        instruction?: string;
        source_datasets?: string[];
        target_dataset_name?: string;
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: DEMO_DRAFT_ID,
          user_id: null,
          instruction: body.instruction ?? "",
          source_dataset_ids: body.source_datasets ?? [DEMO_DATASET_ID],
          sql_text:
            "SELECT status, COUNT(*) AS order_cnt, SUM(amount) AS total_amount FROM demo_orders WHERE status = 'PAID' GROUP BY status",
          preview: {
            columns: [
              {
                name: "status",
                data_type: "STRING",
                pii_category: "NONE",
                masked: false,
              },
              {
                name: "order_cnt",
                data_type: "INT64",
                pii_category: "NONE",
                masked: false,
              },
              {
                name: "total_amount",
                data_type: "DECIMAL",
                pii_category: "NONE",
                masked: false,
              },
            ],
            rows: [{ status: "PAID", order_cnt: 68, total_amount: "3517.50" }],
            row_count_estimate: 1,
            added_columns: ["order_cnt", "total_amount"],
            removed_columns: ["id", "phone", "updated_at"],
          },
          target_dataset_name:
            body.target_dataset_name ?? "derived_clean_orders",
          status: "DRAFT",
          dataset_id: null,
          created_at: "2026-01-31T08:10:00Z",
        }),
      });
      return;
    }

    if (
      pathname === `/api/analytics/transform/${DEMO_DRAFT_ID}/apply` &&
      request.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          draft: {
            id: DEMO_DRAFT_ID,
            user_id: null,
            instruction: "清洗聚合已支付订单",
            source_dataset_ids: [DEMO_DATASET_ID],
            sql_text:
              "SELECT status, COUNT(*) AS order_cnt, SUM(amount) AS total_amount FROM demo_orders WHERE status = 'PAID' GROUP BY status",
            preview: {
              columns: [],
              rows: [],
              row_count_estimate: 1,
              added_columns: ["order_cnt", "total_amount"],
              removed_columns: [],
            },
            target_dataset_name: "derived_clean_orders",
            status: "APPLIED",
            dataset_id: "00000000-0000-0000-0000-000000000499",
            created_at: "2026-01-31T08:10:00Z",
          },
          dataset: {
            id: "00000000-0000-0000-0000-000000000499",
            connection_id: null,
            name: "derived_clean_orders",
            display_name: "derived_clean_orders",
            description: "AI 派生数据集",
            source_kind: "DERIVED_VIEW",
            source_ref: {},
            sync_mode: "SNAPSHOT",
            primary_key: [],
            cursor_column: null,
            cursor_value: null,
            delete_column: null,
            schema_policy: "EVOLVE",
            lake_schema: "tenant_default",
            lake_table: "derived_clean_orders",
            schema_json: [],
            schema_version: 1,
            row_count: 1,
            last_snapshot_id: 1,
            last_synced_at: "2026-01-31T08:11:00Z",
            schedule_cron: null,
            tags: [],
            owner_user_id: null,
            status: "READY",
            created_at: "2026-01-31T08:11:00Z",
            updated_at: "2026-01-31T08:11:00Z",
          },
          job_id: "00000000-0000-0000-0000-000000000498",
        }),
      });
      return;
    }

    if (
      pathname === "/api/analytics/saved-queries" &&
      request.method() === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/analytics/history" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/chat/query" && request.method() === "POST") {
      const payload = JSON.parse(request.postData() ?? "{}") as Record<
        string,
        unknown
      >;
      chatCaptures.push({ body: payload });

      if (payload.mode === "DATA") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            answer: "各状态订单笔数统计完成，其中 PAID 共 68 笔 [表]。",
            hits: [],
            seed_entities: [],
            elapsed_ms: 42,
            intent: "DATA",
            session_id: DEMO_SESSION_ID,
            turn_id: DEMO_TURN_2_ID,
            citations: [],
            sql: "SELECT status, COUNT(*) AS cnt FROM demo_orders GROUP BY status",
            columns: [
              {
                name: "status",
                data_type: "STRING",
                pii_category: "NONE",
                masked: false,
              },
              {
                name: "cnt",
                data_type: "INT64",
                pii_category: "NONE",
                masked: false,
              },
            ],
            rows: [
              { status: "PAID", cnt: 68 },
              { status: "PENDING", cnt: 22 },
              { status: "CANCELED", cnt: 10 },
            ],
            chart: {
              kind: "BAR_CHART",
              x_field: "status",
              y_fields: ["cnt"],
              title: "订单状态笔数",
            },
            related_nodes: [],
          }),
        });
        return;
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          answer:
            "已支付订单总金额为 3517.50 [表]。根据履约规范 [文1]，华东仓需在 24 小时内完成拣配发运。",
          hits: [],
          seed_entities: ["华东仓"],
          elapsed_ms: 85,
          intent: "HYBRID",
          session_id: DEMO_SESSION_ID,
          turn_id: DEMO_TURN_1_ID,
          citations: [
            {
              para_id: "00000000-0000-0000-0000-000000000431",
              doc_id: "00000000-0000-0000-0000-000000000430",
              doc_title: "华东仓供应链履约制度",
              snippet: "所有状态为 PAID 的订单须在 24 小时内由华东仓优先履约出库。",
            },
          ],
          sql: "SELECT status, SUM(amount) AS paid_amount FROM demo_orders WHERE status = 'PAID' GROUP BY status",
          columns: [
            {
              name: "status",
              data_type: "STRING",
              pii_category: "NONE",
              masked: false,
            },
            {
              name: "paid_amount",
              data_type: "DECIMAL",
              pii_category: "NONE",
              masked: false,
            },
          ],
          rows: [{ status: "PAID", paid_amount: "3517.50" }],
          chart: {
            kind: "BAR_CHART",
            x_field: "status",
            y_fields: ["paid_amount"],
            title: "已支付订单金额",
          },
          related_nodes: [
            {
              id: "00000000-0000-0000-0000-000000000301",
              type_name: "Warehouse",
              title: "华东仓",
              metrics: { inventory: 500 },
            },
          ],
        }),
      });
      return;
    }

    const feedbackMatch = /^\/api\/analytics\/ask\/([^/]+)\/feedback$/.exec(
      pathname
    );
    if (feedbackMatch && request.method() === "POST") {
      const turnId = decodeURIComponent(feedbackMatch[1] ?? "");
      const body = JSON.parse(request.postData() ?? "{}") as Record<
        string,
        unknown
      >;
      feedbackCaptures.push({ turnId, body });
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: turnId,
          session_id: DEMO_SESSION_ID,
          user_id: null,
          question: "已支付订单总金额是多少？",
          intent: "HYBRID",
          linked_objects: {
            datasets: ["demo_orders"],
            columns: ["demo_orders.amount"],
            metrics: ["paid_amount"],
            nodes: [],
          },
          sql_text:
            "SELECT status, SUM(amount) AS paid_amount FROM demo_orders WHERE status = 'PAID' GROUP BY status",
          result_preview: null,
          answer: "已支付订单总金额为 3517.50",
          citations: [],
          feedback: body.feedback ?? "UP",
          corrected_sql: body.corrected_sql ?? null,
          model_name: "mock-llm",
          created_at: "2026-01-31T08:15:00Z",
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

test("AxAnalytics /analytics executes SQL count(*) -> 100 and drag-and-drop PivotBuilder renders Grand Total (总计) row", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  const feedbackCaptures: FeedbackCapture[] = [];
  const chatCaptures: ChatQueryCapture[] = [];

  attachSecurityMonitors(
    page,
    leakedTenantUrls,
    missingAuthUrls,
    consoleErrors,
    pageErrors
  );

  const isLive = process.env.PLAYWRIGHT_LIVE_API === "1";
  if (!isLive) {
    await installAnalyticsMocks(page, feedbackCaptures, chatCaptures);
  }

  await page.goto("/analytics");

  await expect(
    page.getByRole("heading", { level: 1, name: "AxAnalytics · 分析与问数" })
  ).toBeVisible();

  // Verify SchemaTree loads demo_orders
  await expect(page.getByTestId("schema-dataset-demo_orders")).toBeVisible();

  // 1. SQL Tab: enter SELECT count(*) FROM demo_orders and run -> table displays 100
  const sqlTextarea = page.getByTestId("sql-editor-textarea");
  await expect(sqlTextarea).toBeVisible();
  await sqlTextarea.fill("SELECT count(*) FROM demo_orders");
  await page.getByTestId("sql-run-btn").click();

  const resultTable = page.getByTestId("result-table");
  await expect(resultTable).toBeVisible();
  await expect(resultTable).toContainText("100");

  // 2. Pivot Tab: select demo_orders, drag status -> rows and amount -> values -> renders 总计 row
  await page.getByTestId("analytics-tab-pivot").click();
  const datasetSelect = page.getByTestId("pivot-dataset-select");
  await expect(datasetSelect).toBeVisible();
  await chooseSelectOption(page, "pivot-dataset-select", "demo_orders");

  const statusField = page.getByTestId("pivot-field-status");
  const amountField = page.getByTestId("pivot-field-amount");
  await expect(statusField).toBeVisible();
  await expect(amountField).toBeVisible();

  await statusField.dragTo(page.getByTestId("pivot-drop-rows"));
  await expect(page.getByTestId("pivot-row-chip-status")).toBeVisible();

  await amountField.dragTo(page.getByTestId("pivot-drop-values"));
  await expect(page.getByTestId("pivot-val-chip-amount")).toBeVisible();

  const pivotTable = page.getByTestId("pivot-result-table");
  await expect(pivotTable).toBeVisible();

  const grandTotalRow = page.getByTestId("pivot-row-grand-total");
  await expect(grandTotalRow).toBeVisible();
  await expect(grandTotalRow).toContainText("总计");
  await expect(grandTotalRow.locator("td").last()).toHaveText(/\d+/);

  if (!isLive) {
    await expect(grandTotalRow).toContainText("4707.50");

    // Verify Metrics tab certified badge & trial run in Mock mode
    await page.getByTestId("analytics-tab-metrics").click();
    await expect(page.getByTestId("metric-certified-paid_amount")).toContainText(
      "已认证"
    );
    await page.getByTestId("metric-run-query-btn").click();
    await expect(page.getByTestId("result-table")).toContainText("3517.50");

    // Verify AI Transform Wizard draft -> preview -> apply in Mock mode
    await page.getByTestId("analytics-tab-transform").click();
    await page
      .getByTestId("transform-instruction-input")
      .fill("清洗聚合已支付订单");
    await page.getByTestId("transform-generate-btn").click();
    await expect(page.getByTestId("transform-draft-panel")).toBeVisible();
    await expect(page.getByTestId("transform-schema-diff")).toContainText(
      "+total_amount"
    );
    await page.getByTestId("transform-apply-btn").click();
    await expect(page.getByTestId("transform-applied-banner")).toContainText(
      "derived_clean_orders"
    );
  }

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("AxQuery /query renders HYBRID & DATA intent badges, expands SQL/ResultGrid/ChartPanel, and submits UP & corrected_sql feedback", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  const feedbackCaptures: FeedbackCapture[] = [];
  const chatCaptures: ChatQueryCapture[] = [];

  attachSecurityMonitors(
    page,
    leakedTenantUrls,
    missingAuthUrls,
    consoleErrors,
    pageErrors
  );

  const isLive = process.env.PLAYWRIGHT_LIVE_API === "1";
  if (!isLive) {
    await installAnalyticsMocks(page, feedbackCaptures, chatCaptures);
  }

  await page.goto("/query");

  await expect(
    page.getByRole("heading", { level: 1, name: "AxQuery · 智能问答" })
  ).toBeVisible();
  await expect(page.getByTestId("axquery-mode-AUTO")).toBeVisible();
  await expect(page.getByTestId("axquery-mode-DOC")).toBeVisible();
  await expect(page.getByTestId("axquery-mode-DATA")).toBeVisible();

  if (!isLive) {
    // 1. Ask HYBRID question in AUTO mode
    await page
      .getByTestId("axquery-input")
      .fill("已支付订单总金额是多少？华东仓履约制度是什么？");
    await page.getByTestId("axquery-submit-btn").click();

    const turn0 = page.getByTestId("axquery-turn-0");
    await expect(turn0).toBeVisible();
    await expect(turn0.getByTestId("axquery-intent-badge")).toContainText(
      "HYBRID"
    );

    // Verify citation card & related node
    await expect(turn0.getByTestId("axquery-citation-0")).toContainText(
      "华东仓供应链履约制度"
    );
    await expect(turn0.getByTestId("axquery-related-nodes")).toContainText(
      "华东仓"
    );

    // Expand SQL, ResultGrid & ChartPanel
    await turn0.getByTestId("axquery-toggle-sql-btn").click();
    await expect(turn0.getByTestId("axquery-sql-code")).toContainText(
      "SUM(amount)"
    );
    await expect(turn0.getByTestId("axquery-result-grid")).toContainText(
      "3517.50"
    );
    const chartCanvas = turn0
      .getByTestId("axquery-chart-panel")
      .locator("canvas")
      .first();
    await expect(chartCanvas).toBeVisible();

    // Thumbs UP feedback
    await turn0.getByTestId("axquery-feedback-up").click();
    await expect(turn0.getByTestId("axquery-feedback-notice")).toContainText(
      "已记录点赞反馈"
    );
    expect(feedbackCaptures[0]).toEqual({
      turnId: DEMO_TURN_1_ID,
      body: { feedback: "UP" },
    });

    // Corrected SQL feedback submission
    await turn0.getByTestId("axquery-correct-sql-toggle").click();
    const correctedSql =
      "SELECT SUM(amount) AS paid_amount FROM demo_orders WHERE status = 'PAID'";
    await turn0.getByTestId("axquery-corrected-sql-input").fill(correctedSql);
    await turn0.getByTestId("axquery-submit-corrected-sql").click();
    await expect(turn0.getByTestId("axquery-feedback-notice")).toContainText(
      "修正 SQL 已提交"
    );
    expect(feedbackCaptures[1]).toEqual({
      turnId: DEMO_TURN_1_ID,
      body: {
        feedback: "DOWN",
        corrected_sql: correctedSql,
      },
    });

    // 2. Switch to DATA mode & ask multi-turn follow-up -> keeps session_id
    await page.getByTestId("axquery-mode-DATA").click();
    await page.getByTestId("axquery-input").fill("各状态订单笔数是多少？");
    await page.getByTestId("axquery-submit-btn").click();

    const turn1 = page.getByTestId("axquery-turn-1");
    await expect(turn1).toBeVisible();
    await expect(turn1.getByTestId("axquery-intent-badge")).toContainText(
      "DATA"
    );
    expect(chatCaptures[1]?.body).toMatchObject({
      mode: "DATA",
      session_id: DEMO_SESSION_ID,
    });
  }

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
