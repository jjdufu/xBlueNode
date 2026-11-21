import { expect, type Page, type Route, test } from "@playwright/test";

const OPS_ADMIN_ID = "00000000-0000-0000-0000-000000001301";
const ANALYST_EAST_ID = "00000000-0000-0000-0000-000000001302";
const ANALYST_SOUTH_ID = "00000000-0000-0000-0000-000000001303";
const DEMO_ORDERS_DS_ID = "00000000-0000-0000-0000-000000001310";

function buildMockEdDsaJwt(sub: string, label: string, ttlSeconds = 900): string {
  const encodeSeg = (obj: Record<string, unknown>): string =>
    btoa(JSON.stringify(obj))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  const nowSec = Math.floor(Date.now() / 1000);
  return `${encodeSeg({ alg: "EdDSA", typ: "JWT", kid: "k1" })}.${encodeSeg({
    iss: "axpoint",
    aud: "axpoint-api",
    sub,
    tid: "default",
    dept: "east",
    roles: ["ADMIN"],
    typ: "access",
    iat: nowSec,
    exp: nowSec + ttlSeconds,
    jti: label,
  })}.ed25519_sig_${label}`;
}

interface AxGuardMockState {
  currentJwt: string;
  refreshedJwt: string;
  refreshCallCount: number;
  forceNextUsers401: boolean;
  tamperedAudit: boolean;
  mePermissions: string[];
  meRoles: string[];
  policies: Array<{
    id: string;
    tenant_id: string;
    name: string;
    description: string;
    policy_text: string;
    enabled: boolean;
    version: number;
    updated_by: string | null;
    updated_at: string;
  }>;
  grants: Array<{
    id: string;
    tenant_id: string;
    resource_kind: string;
    resource_ref: string;
    grantee_kind: string;
    grantee_ref: string;
    actions: string[];
    row_predicate: string | null;
    reason: string;
    status: string;
    approval_id: string | null;
    requested_by: string;
    expires_at: string;
    created_at: string;
    decided_at: string | null;
  }>;
}

async function installAxGuardMocks(
  page: Page,
  state: AxGuardMockState,
  leakedTenantUrls: string[],
  missingAuthUrls: string[]
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
          trace_id: "axguard-e2e-400",
        }),
      });
      return;
    }

    if (pathname === "/api/health") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ status: "ok" }),
      });
      return;
    }

    if (pathname === "/api/auth/config" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          env: "development",
          oidc_enabled: true,
          oidc_display_name: "企业统一身份登录 (OIDC)",
          default_tenant: "default",
        }),
      });
      return;
    }

    if (pathname === "/api/auth/login" && method === "POST") {
      const body = JSON.parse(req.postData() ?? "{}") as {
        tenant?: string;
        username?: string;
        password?: string;
      };
      if (body.username === "locked_user") {
        await route.fulfill({
          status: 401,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "ACCOUNT_LOCKED",
            message: "Account locked for 15 minutes",
            trace_id: "axguard-lockout-401",
          }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: state.currentJwt,
          token_type: "Bearer",
          expires_in: 900,
        }),
      });
      return;
    }

    if (pathname === "/api/auth/refresh" && method === "POST") {
      state.refreshCallCount += 1;
      state.currentJwt = state.refreshedJwt;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          access_token: state.refreshedJwt,
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

    const authHeader = req.headers()["authorization"] ?? "";
    if (!authHeader.startsWith("Bearer ") || authHeader.slice(7).trim().length === 0) {
      missingAuthUrls.push(req.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Bearer token required",
          trace_id: "axguard-missing-401",
        }),
      });
      return;
    }

    // Exercise transparent single-flight 401 refresh retry!
    if (pathname === "/api/axguard/users" && state.forceNextUsers401) {
      state.forceNextUsers401 = false;
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_EXPIRED",
          message: "Access token expired",
          trace_id: "axguard-expired-401",
        }),
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
          roles: state.meRoles,
          permissions: state.mePermissions,
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
          doc_count: 5,
          para_count: 40,
          entity_count: 16,
          edge_count: 30,
          embedding_count: 40,
          embedding_dims: [1024],
          timestamp: "2026-06-01T10:00:00Z",
        }),
      });
      return;
    }

    if (pathname === "/api/axguard/users" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: OPS_ADMIN_ID,
            tenant_id: "default",
            username: "ops_admin",
            display_name: "安全管理员",
            email: "ops@axpoint.internal",
            dept_id: "east",
            clearance: "SECRET",
            auth_source: "LOCAL",
            status: "ACTIVE",
            roles: ["ADMIN"],
            attributes: {},
            failed_logins: 0,
            locked_until: null,
            last_login_at: "2026-06-01T09:50:00Z",
            created_at: "2026-01-01T00:00:00Z",
          },
          {
            id: ANALYST_EAST_ID,
            tenant_id: "default",
            username: "analyst_east",
            display_name: "华东数据分析师",
            email: "east@axpoint.internal",
            dept_id: "east",
            clearance: "CONFIDENTIAL",
            auth_source: "LOCAL",
            status: "ACTIVE",
            roles: ["ANALYST"],
            attributes: { warehouses: ["W-1"] },
            failed_logins: 5,
            locked_until: "2099-01-01T00:00:00Z",
            last_login_at: "2026-06-01T08:00:00Z",
            created_at: "2026-01-01T00:00:00Z",
          },
          {
            id: ANALYST_SOUTH_ID,
            tenant_id: "default",
            username: "analyst_south",
            display_name: "华南数据分析师",
            email: "south@axpoint.internal",
            dept_id: "south",
            clearance: "CONFIDENTIAL",
            auth_source: "OIDC",
            status: "ACTIVE",
            roles: ["ANALYST"],
            attributes: {},
            failed_logins: 0,
            locked_until: null,
            last_login_at: "2026-06-01T08:30:00Z",
            created_at: "2026-01-01T00:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/roles" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000001321",
            tenant_id: "default",
            name: "ADMIN",
            permissions: ["DATA_READ", "DATA_ADMIN", "GUARD_ADMIN", "OPS_ADMIN"],
            is_builtin: true,
            created_at: "2026-01-01T00:00:00Z",
          },
          {
            id: "00000000-0000-0000-0000-000000001322",
            tenant_id: "default",
            name: "ANALYST",
            permissions: ["DATA_READ", "EXPORT_DATA", "SIM_RUN"],
            is_builtin: true,
            created_at: "2026-01-01T00:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/departments" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          { tenant_id: "default", dept_id: "default", name: "集团总部", parent_id: null },
          { tenant_id: "default", dept_id: "east", name: "华东大区", parent_id: "default" },
          { tenant_id: "default", dept_id: "south", name: "华南大区", parent_id: "default" },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/policies" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.policies),
      });
      return;
    }

    if (pathname === "/api/axguard/policies/validate" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ valid: true, errors: [] }),
      });
      return;
    }

    if (pathname.startsWith("/api/axguard/policies/") && method === "PUT") {
      const name = decodeURIComponent(pathname.split("/").pop() ?? "custom");
      const body = JSON.parse(req.postData() ?? "{}") as {
        policy_text?: string;
        description?: string;
        enabled?: boolean;
      };
      const saved = {
        id: "00000000-0000-0000-0000-000000001331",
        tenant_id: "default",
        name,
        description: body.description ?? "",
        policy_text: body.policy_text ?? "",
        enabled: body.enabled ?? true,
        version: 1,
        updated_by: OPS_ADMIN_ID,
        updated_at: "2026-06-01T10:05:00Z",
      };
      state.policies = [
        ...state.policies.filter((p) => p.name !== name),
        saved,
      ];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(saved),
      });
      return;
    }

    if (pathname === "/api/axguard/policies/simulate" && method === "POST") {
      const body = JSON.parse(req.postData() ?? "{}") as {
        action?: string;
        hour?: number;
      };
      const deniedByOffHours =
        body.action === "EXPORT_DATA" &&
        typeof body.hour === "number" &&
        (body.hour < 8 || body.hour >= 20);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          decision: deniedByOffHours ? "DENY" : "ALLOW",
          allowed: !deniedByOffHours,
          determining_policies: deniedByOffHours
            ? ["tenant::forbid_offhours_export"]
            : ["base::permit_dept_clearance"],
          diagnostics: [],
        }),
      });
      return;
    }

    if (pathname === "/api/axguard/row-policies" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000001341",
            tenant_id: "default",
            name: "east_warehouses",
            target_kind: "DATASET",
            target_ref: DEMO_ORDERS_DS_ID,
            applies_to: { roles: ["ANALYST"], depts: [], users: [] },
            predicate: "warehouse_code IN {{user.attr.warehouses}}",
            enabled: true,
            created_at: "2026-06-01T09:00:00Z",
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/row-policy-defaults" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            tenant_id: "default",
            target_kind: "DATASET",
            target_ref: DEMO_ORDERS_DS_ID,
            default_deny: true,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/row-policies/preview" && method === "POST") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          columns: [
            { name: "order_no", data_type: "STRING" },
            { name: "warehouse_code", data_type: "STRING" },
            { name: "phone", data_type: "STRING" },
          ],
          rows: [
            {
              order_no: "ORD-2026-001",
              warehouse_code: "W-1",
              phone: "138****5678",
            },
            {
              order_no: "ORD-2026-002",
              warehouse_code: "W-1",
              phone: "139****9988",
            },
          ],
          row_count: 2,
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
              id: DEMO_ORDERS_DS_ID,
              connection_id: null,
              name: "demo_orders",
              display_name: "华东履约订单表",
              description: "机密级履约订单",
              source_kind: "SQL_TABLE",
              source_ref: {},
              sync_mode: "SNAPSHOT",
              primary_key: ["order_no"],
              cursor_column: null,
              cursor_value: null,
              delete_column: null,
              schema_policy: "STRICT",
              lake_schema: "tenant_default",
              lake_table: "demo_orders",
              schema_json: [
                {
                  name: "order_no",
                  ordinal: 1,
                  data_type: "STRING",
                  nullable: false,
                  null_count: 0,
                  null_ratio: 0,
                  distinct_estimate: 100,
                  sample_values: ["ORD-2026-001"],
                  pii_category: "NONE",
                  pii_hit_ratio: 0,
                  source_comment: "",
                },
                {
                  name: "phone",
                  ordinal: 2,
                  data_type: "STRING",
                  nullable: true,
                  null_count: 0,
                  null_ratio: 0,
                  distinct_estimate: 95,
                  sample_values: ["138****5678"],
                  pii_category: "PHONE",
                  pii_hit_ratio: 0.96,
                  source_comment: "收货手机号",
                },
              ],
              schema_version: 1,
              row_count: 100,
              last_snapshot_id: 1,
              last_synced_at: "2026-06-01T08:00:00Z",
              schedule_cron: null,
              tags: ["orders"],
              owner_user_id: null,
              status: "READY",
              created_at: "2026-06-01T08:00:00Z",
              updated_at: "2026-06-01T08:00:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axhub/datasets/${DEMO_ORDERS_DS_ID}/columns`) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            dataset_id: DEMO_ORDERS_DS_ID,
            column_name: "phone",
            data_type: "STRING",
            pii_category: "PHONE",
            pii_confirmed: true,
            description: "收货人手机号",
            business_term: "",
            unit: "",
            ordinal: 2,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axguard/column-policies" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/axguard/grants" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.grants),
      });
      return;
    }

    if (pathname === "/api/axguard/grants" && method === "POST") {
      const body = JSON.parse(req.postData() ?? "{}") as {
        resource_kind: string;
        resource_ref: string;
        grantee_kind: string;
        grantee_ref: string;
        actions?: string[];
        row_predicate?: string;
        reason: string;
        expires_in_days: number;
      };
      const created = {
        id: "00000000-0000-0000-0000-000000001361",
        tenant_id: "default",
        resource_kind: body.resource_kind,
        resource_ref: body.resource_ref,
        grantee_kind: body.grantee_kind,
        grantee_ref: body.grantee_ref,
        actions: body.actions ?? ["DATA_READ"],
        row_predicate: body.row_predicate ?? null,
        reason: body.reason,
        status: "PENDING",
        approval_id: "00000000-0000-0000-0000-000000001362",
        requested_by: OPS_ADMIN_ID,
        expires_at: "2026-06-08T10:00:00Z",
        created_at: "2026-06-01T10:10:00Z",
        decided_at: null,
      };
      state.grants = [created, ...state.grants];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    if (pathname === "/api/axguard/audit" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: 42,
              tenant_id: "default",
              user_id: OPS_ADMIN_ID,
              username: state.tamperedAudit ? "tampered_actor" : "ops_admin",
              action: "POLICY_CHANGE",
              resource_ref: "policy:forbid_offhours_export",
              status_code: 200,
              client_ip: "10.0.1.15",
              trace_id: "trace-audit-42",
              detail_json: { version: 1 },
              prev_hash: "00001111222233334444555566667777",
              row_hash: "88889999aaaabbbbccccddddeeeeffff",
              created_at: "2026-06-01T10:05:00Z",
            },
            {
              id: 41,
              tenant_id: "default",
              user_id: ANALYST_EAST_ID,
              username: "analyst_east",
              action: "QUERY_SQL",
              resource_ref: `dataset:${DEMO_ORDERS_DS_ID}`,
              status_code: 200,
              client_ip: "10.0.1.22",
              trace_id: "trace-audit-41",
              detail_json: { rows: 2 },
              prev_hash: "",
              row_hash: "00001111222233334444555566667777",
              created_at: "2026-06-01T09:58:00Z",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/axguard/audit/stats" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          by_action: { QUERY_SQL: 28, LOGIN: 12, POLICY_CHANGE: 4 },
          by_user: [
            ["analyst_east", 24],
            ["ops_admin", 16],
          ],
          by_day: [
            ["2026-05-30", 14],
            ["2026-05-31", 19],
            ["2026-06-01", 11],
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axguard/audit/verify" && method === "GET") {
      if (state.tamperedAudit) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ok: false,
            checked_count: 42,
            first_broken_id: 42,
          }),
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ok: true,
            checked_count: 42,
            first_broken_id: null,
          }),
        });
      }
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [], next_cursor: null }),
    });
  });
}

test("AxGuard login, single-flight silent refresh, all 8 governance tabs, Cedar validate/simulate, row 20-row preview, cross-dept grant, and audit hash-chain verify", async ({
  page,
}) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];

  page.on("console", (msg) => {
    if (msg.type() === "error") {
      const text = msg.text();
      if (
        text.includes(
          "Failed to load resource: the server responded with a status of 401"
        )
      ) {
        return;
      }
      consoleErrors.push(text);
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });

  const mockState: AxGuardMockState = {
    currentJwt: buildMockEdDsaJwt(OPS_ADMIN_ID, "initial"),
    refreshedJwt: buildMockEdDsaJwt(OPS_ADMIN_ID, "refreshed"),
    refreshCallCount: 0,
    forceNextUsers401: true,
    tamperedAudit: false,
    mePermissions: ["DATA_READ", "GUARD_ADMIN", "AUDIT_READ"],
    meRoles: ["ADMIN"],
    policies: [],
    grants: [],
  };

  await installAxGuardMocks(page, mockState, leakedTenantUrls, missingAuthUrls);

  // 1. LoginPage lockout error banner + successful username/password login
  await page.goto("/login");
  await expect(
    page.getByRole("heading", { level: 1, name: "AxGuard · 统一身份认证" })
  ).toBeVisible();

  await page.getByTestId("login-username-input").fill("locked_user");
  await page.getByTestId("login-password-input").fill("Wrong#Password123!");
  await page.getByTestId("login-submit-btn").click();
  await expect(page.getByTestId("login-error-banner")).toContainText("15 分钟");

  await page.getByTestId("login-username-input").fill("ops_admin");
  await page.getByTestId("login-password-input").fill("Ops#Admin2026!Safe");
  await page.getByTestId("login-submit-btn").click();
  await expect(page).toHaveURL(/\/$/);

  // 2. Navigate to /guard — forceNextUsers401 triggers single-flight /api/auth/refresh transparently!
  await page.getByRole("link", { name: "AxGuard · 安全治理" }).click();
  await expect(page).toHaveURL(/\/guard$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "AxGuard · 安全治理中心" })
  ).toBeVisible();

  // Tab 1: 用户管理 (loaded after transparent 401 -> refresh retry!)
  await expect(page.getByTestId("axguard-users-panel")).toContainText(
    "analyst_east"
  );
  expect(mockState.refreshCallCount).toBeGreaterThanOrEqual(1);

  // Tab 2: 角色与权限
  await page.getByRole("tab", { name: "角色与权限" }).click();
  await expect(page.getByTestId("axguard-roles-panel")).toContainText(
    "数据与分析"
  );

  // Tab 3: 部门树
  await page.getByRole("tab", { name: "部门树" }).click();
  await expect(page.getByTestId("axguard-depts-panel")).toContainText(
    "华东大区"
  );

  // Tab 4: ABAC 策略 — Validate & Simulate Cedar policy
  await page.getByRole("tab", { name: "ABAC 策略" }).click();
  await expect(page.getByTestId("axguard-policies-panel")).toBeVisible();
  await page.getByTestId("validate-cedar-btn").click();
  await expect(page.getByTestId("cedar-validation-result")).toContainText(
    "校验通过 · 符合 AxPoint Cedar Schema"
  );
  await page.getByTestId("save-cedar-btn").click();

  await page.getByTestId("sim-action-select").selectOption("EXPORT_DATA");
  await page.getByTestId("sim-hour-input").fill("22");
  await page.getByTestId("run-cedar-simulate-btn").click();
  await expect(page.getByTestId("cedar-simulate-result")).toContainText(
    "DENY · 拒绝访问"
  );
  await expect(page.getByTestId("cedar-simulate-result")).toContainText(
    "tenant::forbid_offhours_export"
  );

  // Tab 5: 行级策略 — Preview 20 rows with row filter & phone masking
  await page.getByRole("tab", { name: "行级策略" }).click();
  await expect(page.getByTestId("axguard-row-policies-panel")).toContainText(
    "east_warehouses"
  );
  await page.getByTestId("row-policy-preview-btn").click();
  await expect(page.getByTestId("row-policy-preview-table")).toContainText(
    "138****5678"
  );
  await expect(page.getByTestId("row-policy-preview-table")).toContainText(
    "W-1"
  );

  // Tab 6: 列脱敏
  await page.getByRole("tab", { name: "列脱敏" }).click();
  await expect(page.getByTestId("axguard-column-policies-panel")).toContainText(
    "手机号 (PHONE)"
  );
  await expect(page.getByTestId("axguard-column-policies-panel")).toContainText(
    "命中 96%"
  );

  // Tab 7: 跨部门授权 — Create cross-department grant
  await page.getByRole("tab", { name: "跨部门授权" }).click();
  await page.getByTestId("open-create-grant-btn").click();
  await page.getByTestId("grant-grantee-ref-input").fill("south");
  await page
    .getByTestId("grant-row-predicate-input")
    .fill("warehouse_code = 'W-2'");
  await page
    .getByTestId("grant-reason-input")
    .fill("华南协同仓调拨核对履约订单");
  await page.getByTestId("submit-create-grant-btn").click();
  await expect(page.getByTestId("axguard-grants-panel")).toContainText(
    "PENDING · 待属主审批"
  );
  await expect(page.getByTestId("axguard-grants-panel")).toContainText(
    "warehouse_code = 'W-2'"
  );

  // Tab 8: 审计日志 — Verify hash chain (ok: true, then tampered ok: false, first_broken_id: 42)
  await page.getByRole("tab", { name: "审计日志" }).click();
  await expect(page.getByTestId("axguard-audit-panel")).toBeVisible();

  await page.getByTestId("audit-verify-btn").click();
  await expect(page.getByTestId("audit-verify-ok-badge")).toContainText(
    "哈希链完整无篡改"
  );
  await expect(page.getByTestId("audit-verify-ok-badge")).toContainText("42");

  mockState.tamperedAudit = true;
  await page.getByTestId("audit-verify-btn").click();
  await expect(page.getByTestId("audit-tamper-alert")).toContainText(
    "首个异常记录 ID: #42"
  );

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("AUDIT_READ-only principal defaults directly to 审计日志 tab on /guard", async ({
  page,
}) => {
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];

  const mockState: AxGuardMockState = {
    currentJwt: buildMockEdDsaJwt(OPS_ADMIN_ID, "auditor"),
    refreshedJwt: buildMockEdDsaJwt(OPS_ADMIN_ID, "auditor-refreshed"),
    refreshCallCount: 0,
    forceNextUsers401: false,
    tamperedAudit: false,
    mePermissions: ["AUDIT_READ"],
    meRoles: ["VIEWER"],
    policies: [],
    grants: [],
  };

  await installAxGuardMocks(page, mockState, leakedTenantUrls, missingAuthUrls);

  await page.goto("/guard");
  await expect(page.getByTestId("axguard-audit-panel")).toBeVisible();
  await expect(page.getByRole("tab", { name: "用户管理" })).toBeDisabled();
  expect(leakedTenantUrls).toEqual([]);
});
