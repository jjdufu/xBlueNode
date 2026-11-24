import { expect, type Page, type Route, test } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const DEMO_CONN_ID = "00000000-0000-0000-0000-000000000201";
const DEMO_DATASET_ID = "00000000-0000-0000-0000-000000000202";
const DEMO_SYNC_JOB_ID = "00000000-0000-0000-0000-000000000203";

async function installAxHubMocks(
  page: Page,
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
          trace_id: "axhub-e2e-400",
        }),
      });
      return;
    }

    const authHeader = request.headers()["authorization"] ?? "";
    if (
      pathname !== "/api/health" &&
      (!authHeader.startsWith("Bearer ") || authHeader.slice(7).trim().length === 0)
    ) {
      missingAuthUrls.push(request.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "axhub-e2e-401",
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/connector-types") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            connector_type: "POSTGRES",
            display_name: "PostgreSQL",
            category: "RELATIONAL",
            config_schema: {
              properties: {
                host: { type: "string", title: "主机地址" },
                port: { type: "integer", title: "端口", default: 5432 },
                database: { type: "string", title: "数据库名" },
                schema: { type: "string", title: "模式名", default: "public" },
                username: { type: "string", title: "用户名" },
              },
              required: ["host", "port", "database", "username"],
            },
            secret_fields: ["password"],
            supports_modes: ["SNAPSHOT", "APPEND", "UPSERT"],
            supports_discover: true,
            image_tier: "lite",
            available: true,
          },
          {
            connector_type: "KAFKA",
            display_name: "Apache Kafka",
            category: "STREAM",
            config_schema: {
              properties: {
                bootstrap_servers: { type: "string", title: "Bootstrap Servers" },
                topics: { type: "string", title: "Topics" },
              },
              required: ["bootstrap_servers", "topics"],
            },
            secret_fields: ["sasl_password"],
            supports_modes: ["APPEND"],
            supports_discover: true,
            image_tier: "lite",
            available: true,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axhub/connections" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DEMO_CONN_ID,
              name: "demo_pg",
              connector_type: "POSTGRES",
              config: {
                host: "127.0.0.1",
                port: 5432,
                database: "axpoint",
                schema: "demo_src",
                username: "axpoint_app",
              },
              secret_fields_set: ["password"],
              allow_private_network: true,
              status: "OK",
              last_test_message: "PostgreSQL 16 connected",
              last_tested_at: "2026-01-31T08:00:00Z",
              created_by: null,
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axhub/connections/${DEMO_CONN_ID}/test` &&
      request.method() === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ok: true,
          message: "PostgreSQL connection healthy",
          latency_ms: 8,
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/datasets" && request.method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DEMO_DATASET_ID,
              connection_id: DEMO_CONN_ID,
              name: "demo_orders",
              display_name: "演示订单表",
              description: "供应链演示订单主表",
              source_kind: "SQL_TABLE",
              source_ref: { schema: "demo_src", table: "orders" },
              sync_mode: "SNAPSHOT",
              primary_key: ["id"],
              cursor_column: "updated_at",
              cursor_value: "2026-01-31T09:40:00Z",
              delete_column: "is_deleted",
              schema_policy: "ADDITIVE",
              lake_schema: "tenant_default",
              lake_table: "demo_orders",
              schema_json: [],
              schema_version: 1,
              row_count: 100,
              last_snapshot_id: 1,
              last_synced_at: "2026-01-31T08:05:00Z",
              schedule_cron: null,
              tags: ["supply_chain", "orders"],
              owner_user_id: null,
              status: "READY",
              created_at: "2026-01-31T08:00:00Z",
              updated_at: "2026-01-31T08:05:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axhub/datasets/${DEMO_DATASET_ID}/snapshots`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            snapshot_id: 1,
            committed_at: "2026-01-31T08:05:00Z",
            rows_added: 100,
            rows_deleted: 0,
          },
        ]),
      });
      return;
    }

    if (pathname === `/api/axhub/datasets/${DEMO_DATASET_ID}/preview`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            {
              name: "id",
              ordinal: 0,
              data_type: "INT64",
              nullable: false,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 100,
              sample_values: ["1", "2"],
              pii_category: "NONE",
              pii_hit_ratio: 0,
              source_comment: "",
            },
            {
              name: "amount",
              ordinal: 1,
              data_type: "DECIMAL",
              nullable: false,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 100,
              sample_values: ["10.50", "21.00"],
              pii_category: "NONE",
              pii_hit_ratio: 0,
              source_comment: "",
            },
            {
              name: "status",
              ordinal: 2,
              data_type: "STRING",
              nullable: false,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 3,
              sample_values: ["PAID", "PENDING"],
              pii_category: "NONE",
              pii_hit_ratio: 0,
              source_comment: "",
            },
            {
              name: "phone",
              ordinal: 3,
              data_type: "STRING",
              nullable: true,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 100,
              sample_values: ["138****0001"],
              pii_category: "PHONE",
              pii_hit_ratio: 1,
              source_comment: "",
            },
            {
              name: "warehouse_code",
              ordinal: 4,
              data_type: "STRING",
              nullable: false,
              null_count: 0,
              null_ratio: 0,
              distinct_estimate: 2,
              sample_values: ["W-1", "W-2"],
              pii_category: "NONE",
              pii_hit_ratio: 0,
              source_comment: "",
            },
          ],
          rows: [
            {
              id: 1,
              amount: "10.50",
              status: "PENDING",
              phone: "138****0001",
              warehouse_code: "W-2",
            },
            {
              id: 2,
              amount: "21.00",
              status: "CANCELED",
              phone: "138****0002",
              warehouse_code: "W-1",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === `/api/axhub/datasets/${DEMO_DATASET_ID}/sync-jobs`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: DEMO_SYNC_JOB_ID,
              dataset_id: DEMO_DATASET_ID,
              job_id: "00000000-0000-0000-0000-000000000204",
              batch_id: "00000000-0000-0000-0000-000000000205",
              sync_mode: "SNAPSHOT",
              status: "SUCCEEDED",
              cursor_from: null,
              cursor_to: "2026-01-31T09:40:00Z",
              rows_read: 100,
              rows_written: 100,
              rows_deleted: 0,
              snapshot_id: 1,
              schema_change: {
                added: [],
                removed: [],
                widened: [],
                narrowed: [],
                breaking: false,
              },
              quality_result: {
                passed: true,
                blocked: false,
                rules: [
                  {
                    rule_id: "00000000-0000-0000-0000-000000000206",
                    rule_kind: "NOT_NULL",
                    column_name: "id",
                    passed: true,
                    on_fail: "BLOCK",
                    detail: "0 nulls",
                  },
                ],
              },
              error_code: null,
              error_message: null,
              started_at: "2026-01-31T08:04:58Z",
              finished_at: "2026-01-31T08:05:00Z",
              created_at: "2026-01-31T08:04:58Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axhub/datasets/${DEMO_DATASET_ID}/lineage`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          edges: [
            {
              id: 1,
              src_kind: "CONNECTION",
              src_id: DEMO_CONN_ID,
              dst_kind: "DATASET",
              dst_id: DEMO_DATASET_ID,
              via: "SNAPSHOT",
              created_at: "2026-01-31T08:05:00Z",
            },
            {
              id: 2,
              src_kind: "DATASET",
              src_id: DEMO_DATASET_ID,
              dst_kind: "NODE_TYPE",
              dst_id: "Order",
              via: "XBLUE_MAP",
              created_at: "2026-01-31T08:10:00Z",
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/catalog/search") {
      const q = (searchParams.get("q") ?? "").toLowerCase();
      const matched = q.includes("order") || q === "";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: matched
            ? [
                {
                  id: DEMO_DATASET_ID,
                  name: "demo_orders",
                  display_name: "演示订单表",
                  description: "供应链演示订单主表",
                  source_kind: "SQL_TABLE",
                  row_count: 100,
                  tags: ["supply_chain", "orders"],
                  matched_columns: ["id", "amount", "warehouse_code"],
                },
              ]
            : [],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/axhub/glossary") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-000000000209",
              term: "履约订单",
              definition: "已完成支付并分配仓库的订单记录",
              synonyms: ["demo_orders", "orders"],
              created_at: "2026-01-31T08:00:00Z",
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

test("AxHub /hub renders demo_pg & demo_orders, tests connectivity, previews table, searches catalog, and renders lineage & sync history cleanly", async ({
  page,
}) => {
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
    await installAxHubMocks(page, leakedTenantUrls, missingAuthUrls);
  }

  await page.goto("/hub");

  await expect(
    page.getByRole("heading", { level: 1, name: "数据与知识" })
  ).toBeVisible();

  // 1. Verify seed connection demo_pg with status OK & non-echoing secret on edit
  const demoConnCard = page.getByTestId("connection-card-demo_pg");
  await expect(demoConnCard).toBeVisible();
  await expect(page.getByTestId("connection-status-demo_pg")).toContainText(/OK|正常/);

  await page.getByTestId("test-connection-btn-demo_pg").click();
  await expect(page.getByTestId("conn-test-result-demo_pg")).toContainText("ms");

  await page.getByTestId("edit-connection-btn-demo_pg").click();
  const passwordInput = page.getByTestId("secret-field-password");
  await expect(passwordInput).toBeVisible();
  await expect(passwordInput).toHaveValue("");
  await expect(passwordInput).toHaveAttribute(
    "placeholder",
    "已设置（留空保持不变）"
  );
  await page.keyboard.press("Escape");

  // 2. Verify seed dataset demo_orders & wide detail drawer preview data table
  const demoDatasetRow = page.getByTestId("dataset-row-demo_orders");
  await expect(demoDatasetRow).toBeVisible();
  await page.getByTestId("preview-dataset-btn-demo_orders").click();

  const previewTable = page.getByTestId("dataset-preview-table");
  await expect(previewTable).toBeVisible();
  await expect(previewTable).toContainText("warehouse_code");
  await expect(previewTable).toContainText("W-1");

  // 3. Switch to Sync History & verify sync job summary
  await page.getByTestId("dataset-tab-history").click();
  const syncHistory = page.getByTestId("sync-history-panel");
  await expect(syncHistory).toBeVisible();
  await expect(syncHistory).toContainText(/SUCCEEDED|成功/);
  await expect(syncHistory).toContainText("100");

  // 4. Switch to Lineage Graph & verify ECharts canvas renders
  await page.getByTestId("dataset-tab-lineage").click();
  const lineageCanvas = page
    .getByTestId("lineage-graph-canvas")
    .locator("canvas")
    .first();
  await expect(lineageCanvas).toBeVisible();
  const lineageBox = await lineageCanvas.boundingBox();
  expect(lineageBox?.width ?? 0).toBeGreaterThan(50);
  expect(lineageBox?.height ?? 0).toBeGreaterThan(50);
  await page.getByTestId("close-dataset-inspector-btn").click();

  // 5. Switch to Catalog Search & search "orders" -> hits demo_orders
  await page.getByTestId("axhub-nav-catalog").click();
  await page.getByTestId("catalog-search-input").fill("orders");
  await page.getByTestId("catalog-search-btn").click();
  const catalogHit = page.getByTestId("catalog-hit-demo_orders");
  await expect(catalogHit).toBeVisible();
  await expect(catalogHit).toContainText("demo_orders");

  // 6. Verify zero error banners or uncaught exceptions
  await expect(page.locator("[data-testid$='error-banner']")).toHaveCount(0);
  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
