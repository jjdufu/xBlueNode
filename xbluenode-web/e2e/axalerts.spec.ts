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

const RULE_LOW_INV_ID = "00000000-0000-0000-0000-000000000801";
const CHANNEL_SINK_ID = "00000000-0000-0000-0000-000000000811";
const ALERT_LOW_INV_ID = "00000000-0000-0000-0000-000000000821";
const DELIVERY_OPENED_ID = "00000000-0000-0000-0000-000000000831";
const DELIVERY_TEST_ID = "00000000-0000-0000-0000-000000000832";
const WAREHOUSE_NODE_ID = "00000000-0000-0000-0000-000000000841";
const FLOW_DAILY_ID = "00000000-0000-0000-0000-000000000701";
const FLOW_RUN_ID = "00000000-0000-0000-0000-000000000711";
const DATASET_ORDERS_ID = "00000000-0000-0000-0000-000000000501";

// Never returned in any GET/POST channel response — used to assert zero secret leakage
const PLAINTEXT_SIGNING_SECRET = "whsec_super_secret_should_never_echo_8899";

interface MockAlertItem {
  id: string;
  rule_id: string;
  dedup_key: string;
  severity: "INFO" | "WARN" | "CRITICAL";
  status: "OPEN" | "ACK" | "RESOLVED";
  title: string;
  body: string;
  node_id: string | null;
  dataset_id: string | null;
  last_value: number | null;
  detail: Record<string, unknown>;
  occurrences: number;
  first_seen_at: string;
  last_seen_at: string;
  last_false_at: string | null;
  acked_by: string | null;
  acked_at: string | null;
  escalated_at: string | null;
  resolved_at: string | null;
  resolve_reason: "AUTO" | "MANUAL" | "RULE_DISABLED" | null;
  deliveries: {
    id: string;
    alert_id: string | null;
    channel_id: string;
    event: "OPENED" | "ESCALATED" | "RESOLVED" | "TEST" | "ACTION";
    status: "PENDING" | "SENT" | "FAILED";
    attempt: number;
    response_code: number | null;
    error_message: string | null;
    next_retry_at: string;
    created_at: string;
    sent_at: string | null;
  }[];
}

interface MockAlertEventItem {
  id: number;
  alert_id: string;
  event:
    | "OPENED"
    | "RETRIGGERED"
    | "ACKED"
    | "ESCALATED"
    | "RESOLVED"
    | "NOTIFIED"
    | "NOTIFY_FAILED"
    | "SILENCED"
    | "FLOW_STARTED";
  detail: Record<string, unknown>;
  actor: string | null;
  created_at: string;
}

interface MockSilenceItem {
  id: string;
  matcher: {
    rule_ids: string[];
    severities: string[];
    node_ids: string[];
    type_names: string[];
  };
  starts_at: string;
  ends_at: string;
  reason: string;
  created_by: string | null;
  created_at: string;
}

interface MockAxAlertsState {
  alerts: MockAlertItem[];
  eventsByAlertId: Record<string, MockAlertEventItem[]>;
  rules: Record<string, unknown>[];
  channels: Record<string, unknown>[];
  silences: MockSilenceItem[];
  nextEventId: number;
  nextSilenceSeq: number;
}

interface AlertsTargetHandles {
  alertId: string;
  ruleId: string;
  channelId: string;
}

function createInitialMockState(): MockAxAlertsState {
  const nowIso = new Date().toISOString();
  const fiveMinAgoIso = new Date(Date.now() - 5 * 60_000).toISOString();

  return {
    alerts: [
      {
        id: ALERT_LOW_INV_ID,
        rule_id: RULE_LOW_INV_ID,
        dedup_key: `${RULE_LOW_INV_ID}:${WAREHOUSE_NODE_ID}`,
        severity: "CRITICAL",
        status: "OPEN",
        title: "[CRITICAL] 华东一号仓 指标 inventory=10 (阈值 50)",
        body: "规则 low_inventory 命中：对象 华东一号仓 当前在库库存 10 低于安全水位 50。",
        node_id: WAREHOUSE_NODE_ID,
        dataset_id: null,
        last_value: 10,
        detail: {
          type_name: "Warehouse",
          metric: "inventory",
          threshold: 50,
          flow_run_id: FLOW_RUN_ID,
        },
        occurrences: 2,
        first_seen_at: fiveMinAgoIso,
        last_seen_at: nowIso,
        last_false_at: null,
        acked_by: null,
        acked_at: null,
        escalated_at: null,
        resolved_at: null,
        resolve_reason: null,
        deliveries: [
          {
            id: DELIVERY_OPENED_ID,
            alert_id: ALERT_LOW_INV_ID,
            channel_id: CHANNEL_SINK_ID,
            event: "OPENED",
            status: "SENT",
            attempt: 1,
            response_code: 200,
            error_message: null,
            next_retry_at: fiveMinAgoIso,
            created_at: fiveMinAgoIso,
            sent_at: fiveMinAgoIso,
          },
        ],
      },
    ],
    eventsByAlertId: {
      [ALERT_LOW_INV_ID]: [
        {
          id: 101,
          alert_id: ALERT_LOW_INV_ID,
          event: "OPENED",
          detail: { value: 10, threshold: 50 },
          actor: null,
          created_at: fiveMinAgoIso,
        },
        {
          id: 102,
          alert_id: ALERT_LOW_INV_ID,
          event: "NOTIFIED",
          detail: { channel_id: CHANNEL_SINK_ID, response_code: 200 },
          actor: null,
          created_at: fiveMinAgoIso,
        },
        {
          id: 103,
          alert_id: ALERT_LOW_INV_ID,
          event: "FLOW_STARTED",
          detail: { flow_id: FLOW_DAILY_ID, flow_run_id: FLOW_RUN_ID },
          actor: null,
          created_at: fiveMinAgoIso,
        },
      ],
    },
    rules: [
      {
        id: RULE_LOW_INV_ID,
        name: "low_inventory",
        description: "核心仓储在库库存跌破安全阈值自动预警并启动补货编排",
        scope_kind: "METRIC",
        target: {
          type_name: "Warehouse",
          metric: "inventory",
        },
        condition: {
          kind: "THRESHOLD",
          op: "LT",
          value: 50,
          for_minutes: 0,
        },
        severity: "CRITICAL",
        title_template:
          "[{{severity}}] {{node.title}} 指标 {{metric}}={{value}} (阈值 {{threshold}})",
        body_template:
          "规则 {{rule.name}} 命中：对象 {{node.title}} 当前值 {{value}} 低于阈值 {{threshold}}。",
        cooldown_minutes: 0,
        resolve_after_minutes: 0,
        escalate_after_minutes: 15,
        channel_ids: [CHANNEL_SINK_ID],
        escalation_channel_ids: [],
        run_flow_id: FLOW_DAILY_ID,
        enabled: true,
        version: 1,
        created_by: "u-admin",
        created_at: fiveMinAgoIso,
        updated_at: fiveMinAgoIso,
      },
    ],
    channels: [
      {
        id: CHANNEL_SINK_ID,
        name: "gate_sink",
        kind: "WEBHOOK",
        config: {
          url: "http://127.0.0.1:18900/hook",
          headers: { "X-Env": "step08" },
        },
        secret_fields_set: ["signing_secret"],
        allow_private_network: true,
        enabled: true,
        created_at: fiveMinAgoIso,
      },
    ],
    silences: [],
    nextEventId: 200,
    nextSilenceSeq: 1,
  };
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

    const postText = request.postData();
    if (postText && postText.includes('"tenant_id"')) {
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

async function installAxAlertsMocks(
  page: Page,
  state: MockAxAlertsState
): Promise<AlertsTargetHandles> {
  await page.route("**/favicon.ico", async (route: Route) => {
    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const method = req.method();
    const url = new URL(req.url());
    const { pathname, searchParams } = url;

    if (pathname === "/api/auth/me" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          user_id: "u-admin",
          tenant_id: "default",
          roles: ["ADMIN"],
          permissions: ["DATA_READ", "RULE_EDIT", "ALERT_ACK", "GUARD_ADMIN"],
        }),
      });
      return;
    }

    // Alerts list: GET /api/axflow/alerts
    if (pathname === "/api/axflow/alerts" && method === "GET") {
      const statusFilter = searchParams.get("status");
      const severityFilter = searchParams.get("severity");
      const ruleFilter = searchParams.get("rule_id");
      const typeFilter = searchParams.get("type_name");

      const items = state.alerts.filter((a) => {
        if (statusFilter && a.status !== statusFilter) return false;
        if (severityFilter && a.severity !== severityFilter) return false;
        if (ruleFilter && a.rule_id !== ruleFilter) return false;
        if (typeFilter && a.detail.type_name !== typeFilter) return false;
        return true;
      });

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items, next_cursor: null }),
      });
      return;
    }

    // Batch ACK: POST /api/axflow/alerts/ack-batch
    if (pathname === "/api/axflow/alerts/ack-batch" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        alert_ids?: string[];
        comment?: string;
      };
      const ids = Array.isArray(body.alert_ids) ? body.alert_ids : [];
      const nowIso = new Date().toISOString();
      const updatedItems: MockAlertItem[] = [];

      for (const item of state.alerts) {
        if (ids.includes(item.id) && item.status === "OPEN") {
          item.status = "ACK";
          item.acked_by = "u-admin";
          item.acked_at = nowIso;
          state.nextEventId += 1;
          const evList = state.eventsByAlertId[item.id] ?? [];
          evList.push({
            id: state.nextEventId,
            alert_id: item.id,
            event: "ACKED",
            detail: { comment: body.comment ?? "" },
            actor: "u-admin",
            created_at: nowIso,
          });
          state.eventsByAlertId[item.id] = evList;
          updatedItems.push(item);
        }
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          acked_count: updatedItems.length,
          items: updatedItems,
          alert_ids: updatedItems.map((i) => i.id),
        }),
      });
      return;
    }

    // Alert Events: GET /api/axflow/alerts/:id/events
    const alertEventsMatch = pathname.match(
      /^\/api\/axflow\/alerts\/([^/]+)\/events$/
    );
    if (alertEventsMatch && method === "GET") {
      const alertId = decodeURIComponent(alertEventsMatch[1] ?? "");
      const events = state.eventsByAlertId[alertId] ?? [];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: events, next_cursor: null }),
      });
      return;
    }

    // Alert ACK: POST /api/axflow/alerts/:id/ack
    const alertAckMatch = pathname.match(
      /^\/api\/axflow\/alerts\/([^/]+)\/ack$/
    );
    if (alertAckMatch && method === "POST") {
      const alertId = decodeURIComponent(alertAckMatch[1] ?? "");
      const body = (req.postDataJSON() ?? {}) as { comment?: string };
      const target = state.alerts.find((a) => a.id === alertId);
      if (!target) {
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "NOT_FOUND",
            message: "alert not found",
          }),
        });
        return;
      }

      const nowIso = new Date().toISOString();
      target.status = "ACK";
      target.acked_by = "u-admin";
      target.acked_at = nowIso;

      state.nextEventId += 1;
      const evList = state.eventsByAlertId[alertId] ?? [];
      evList.push({
        id: state.nextEventId,
        alert_id: alertId,
        event: "ACKED",
        detail: { comment: body.comment ?? "" },
        actor: "u-admin",
        created_at: nowIso,
      });
      state.eventsByAlertId[alertId] = evList;

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(target),
      });
      return;
    }

    // Alert Resolve: POST /api/axflow/alerts/:id/resolve
    const alertResolveMatch = pathname.match(
      /^\/api\/axflow\/alerts\/([^/]+)\/resolve$/
    );
    if (alertResolveMatch && method === "POST") {
      const alertId = decodeURIComponent(alertResolveMatch[1] ?? "");
      const body = (req.postDataJSON() ?? {}) as { comment?: string };
      const target = state.alerts.find((a) => a.id === alertId);
      if (!target) {
        await route.fulfill({ status: 404, body: "{}" });
        return;
      }
      const nowIso = new Date().toISOString();
      target.status = "RESOLVED";
      target.resolved_at = nowIso;
      target.resolve_reason = "MANUAL";

      state.nextEventId += 1;
      const evList = state.eventsByAlertId[alertId] ?? [];
      evList.push({
        id: state.nextEventId,
        alert_id: alertId,
        event: "RESOLVED",
        detail: { reason: "MANUAL", comment: body.comment ?? "" },
        actor: "u-admin",
        created_at: nowIso,
      });
      state.eventsByAlertId[alertId] = evList;

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(target),
      });
      return;
    }

    // Alert Detail: GET /api/axflow/alerts/:id
    const alertDetailMatch = pathname.match(/^\/api\/axflow\/alerts\/([^/]+)$/);
    if (alertDetailMatch && method === "GET") {
      const alertId = decodeURIComponent(alertDetailMatch[1] ?? "");
      const target = state.alerts.find((a) => a.id === alertId);
      if (!target) {
        await route.fulfill({ status: 404, body: "{}" });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(target),
      });
      return;
    }

    // Rules list & create: /api/axflow/rules
    if (pathname === "/api/axflow/rules" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.rules, next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/axflow/rules" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as Record<string, unknown>;
      const nowIso = new Date().toISOString();
      const created = {
        ...body,
        id: `00000000-0000-0000-0000-00000000089${state.rules.length + 1}`,
        version: 1,
        created_by: "u-admin",
        created_at: nowIso,
        updated_at: nowIso,
      };
      state.rules.push(created);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    // Saved or Adhoc Rule Dry-Run: POST /api/axflow/rules/:id/dry-run or /api/axflow/rules/dry-run
    const savedDryRunMatch = pathname.match(
      /^\/api\/axflow\/rules\/([^/]+)\/dry-run$/
    );
    if (
      (savedDryRunMatch || pathname === "/api/axflow/rules/dry-run") &&
      method === "POST"
    ) {
      const ruleId = savedDryRunMatch
        ? decodeURIComponent(savedDryRunMatch[1] ?? "")
        : RULE_LOW_INV_ID;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          rule_id: ruleId,
          evaluated_objects: 3,
          matched_count: 1,
          would_trigger: [
            {
              dedup_key: `${ruleId}:${WAREHOUSE_NODE_ID}`,
              severity: "CRITICAL",
              title: "[CRITICAL] 华东一号仓 指标 inventory=10 (阈值 50)",
              body: "规则 low_inventory 命中：对象 华东一号仓 当前值 10 低于阈值 50。",
              node_id: WAREHOUSE_NODE_ID,
              dataset_id: null,
              value: 10,
              detail: {
                type_name: "Warehouse",
                metric: "inventory",
                threshold: 50,
              },
            },
          ],
        }),
      });
      return;
    }

    // Channels list & create
    if (pathname === "/api/axflow/channels" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.channels, next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/axflow/channels" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        name?: string;
        kind?: string;
        config?: Record<string, unknown>;
        secrets?: Record<string, string>;
        allow_private_network?: boolean;
        enabled?: boolean;
      };
      const secretKeys = Object.keys(body.secrets ?? {});
      const created = {
        id: `00000000-0000-0000-0000-00000000086${state.channels.length + 1}`,
        name: body.name ?? "new_channel",
        kind: body.kind ?? "WEBHOOK",
        config: body.config ?? {},
        secret_fields_set: secretKeys,
        allow_private_network: Boolean(body.allow_private_network),
        enabled: body.enabled ?? true,
        created_at: new Date().toISOString(),
      };
      state.channels.push(created);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    // Channel Test Delivery: POST /api/axflow/channels/:id/test
    const channelTestMatch = pathname.match(
      /^\/api\/axflow\/channels\/([^/]+)\/test$/
    );
    if (channelTestMatch && method === "POST") {
      const channelId = decodeURIComponent(channelTestMatch[1] ?? "");
      const nowIso = new Date().toISOString();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ok: true,
          delivery: {
            id: DELIVERY_TEST_ID,
            alert_id: null,
            channel_id: channelId,
            event: "TEST",
            status: "SENT",
            attempt: 1,
            response_code: 200,
            error_message: null,
            next_retry_at: nowIso,
            created_at: nowIso,
            sent_at: nowIso,
          },
        }),
      });
      return;
    }

    // Silences list, create & delete
    if (pathname === "/api/axflow/silences" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: state.silences, next_cursor: null }),
      });
      return;
    }

    if (pathname === "/api/axflow/silences" && method === "POST") {
      const body = (req.postDataJSON() ?? {}) as {
        matcher?: MockSilenceItem["matcher"];
        starts_at?: string;
        ends_at?: string;
        reason?: string;
      };
      const seq = state.nextSilenceSeq;
      state.nextSilenceSeq += 1;
      const nowIso = new Date().toISOString();
      const created: MockSilenceItem = {
        id: `00000000-0000-0000-0000-00000000087${seq}`,
        matcher: {
          rule_ids: body.matcher?.rule_ids ?? [],
          severities: body.matcher?.severities ?? ["WARN"],
          node_ids: body.matcher?.node_ids ?? [],
          type_names: body.matcher?.type_names ?? ["Warehouse"],
        },
        starts_at: body.starts_at ?? nowIso,
        ends_at:
          body.ends_at ?? new Date(Date.now() + 2 * 3600_000).toISOString(),
        reason: body.reason ?? "例行静默",
        created_by: "u-admin",
        created_at: nowIso,
      };
      state.silences.push(created);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    const silenceDeleteMatch = pathname.match(
      /^\/api\/axflow\/silences\/([^/]+)$/
    );
    if (silenceDeleteMatch && method === "DELETE") {
      const silenceId = decodeURIComponent(silenceDeleteMatch[1] ?? "");
      state.silences = state.silences.filter((s) => s.id !== silenceId);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ deleted: true }),
      });
      return;
    }

    // Auxiliary metadata routes
    if (pathname === "/api/axflow/flows" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: FLOW_DAILY_ID,
              name: "daily_supply_refresh",
              slug: "daily_supply_refresh",
              description: "供应链补货编排流水线",
              nodes: [],
              edges: [],
              trigger_cfg: { kind: "MANUAL" },
              issues: [],
              latest_version: 1,
            },
          ],
          next_cursor: null,
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
              name: "demo_orders",
              display_name: "履约订单数据集",
              status: "READY",
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/schema" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          node_types: [
            {
              id: "00000000-0000-0000-0000-000000000511",
              name: "Warehouse",
              display_name: "仓储枢纽",
              properties: [
                {
                  name: "inventory",
                  display_name: "在库库存",
                  data_type: "FLOAT64",
                  kind: "METRIC",
                },
              ],
              agg_rules: [],
              pos_x: 100,
              pos_y: 120,
            },
          ],
          edge_types: [],
        }),
      });
      return;
    }

    if (
      pathname.startsWith(`/api/xblue/nodes/${WAREHOUSE_NODE_ID}/metrics/`) &&
      method === "GET"
    ) {
      const now = Date.now();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          { ts: new Date(now - 30 * 60_000).toISOString(), value: 82 },
          { ts: new Date(now - 15 * 60_000).toISOString(), value: 44 },
          { ts: new Date(now - 2 * 60_000).toISOString(), value: 10 },
        ]),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ items: [], next_cursor: null }),
    });
  });

  return {
    alertId: ALERT_LOW_INV_ID,
    ruleId: RULE_LOW_INV_ID,
    channelId: CHANNEL_SINK_ID,
  };
}

async function ensureLiveAlertsFixtures(
  page: Page,
  request: APIRequestContext
): Promise<AlertsTargetHandles> {
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

  // 1. Ensure Warehouse schema & node exist
  let warehouseNodeId = WAREHOUSE_NODE_ID;
  const nodesResp = await request.get(
    `${apiBase}/api/xblue/nodes?type_name=Warehouse`,
    { headers: authHeaders }
  );
  if (nodesResp.ok()) {
    const nodesData = (await nodesResp.json()) as {
      nodes?: { id?: string }[];
      items?: { id?: string }[];
    };
    const existingList = nodesData.nodes ?? nodesData.items ?? [];
    const firstId = existingList[0]?.id;
    if (typeof firstId === "string" && firstId.length > 0) {
      warehouseNodeId = firstId;
    }
  }

  // 2. Ensure a WEBHOOK channel with sealed `signing_secret` exists
  let channelId = "";
  const chResp = await request.get(`${apiBase}/api/axflow/channels`, {
    headers: authHeaders,
  });
  if (chResp.ok()) {
    const chJson = (await chResp.json()) as {
      items?: { id?: string; secret_fields_set?: string[] }[];
    };
    const found = (chJson.items ?? []).find(
      (c) =>
        typeof c.id === "string" &&
        Array.isArray(c.secret_fields_set) &&
        c.secret_fields_set.length > 0
    );
    if (found?.id) {
      channelId = found.id;
    }
  }
  if (!channelId) {
    const createdCh = await request.post(`${apiBase}/api/axflow/channels`, {
      headers: authHeaders,
      data: {
        name: "e2e_gate_sink",
        kind: "WEBHOOK",
        config: { url: "http://127.0.0.1:18900/hook" },
        secrets: { signing_secret: PLAINTEXT_SIGNING_SECRET },
        allow_private_network: true,
        enabled: true,
      },
    });
    if (createdCh.ok()) {
      const createdJson = (await createdCh.json()) as { id?: string };
      channelId = createdJson.id ?? "";
    }
  }

  // 3. Ensure a METRIC rule exists
  let ruleId = "";
  const rulesResp = await request.get(`${apiBase}/api/axflow/rules`, {
    headers: authHeaders,
  });
  if (rulesResp.ok()) {
    const rulesJson = (await rulesResp.json()) as {
      items?: { id?: string; enabled?: boolean }[];
    };
    const foundRule = (rulesJson.items ?? []).find(
      (r) => typeof r.id === "string" && r.enabled !== false
    );
    if (foundRule?.id) {
      ruleId = foundRule.id;
    }
  }
  if (!ruleId) {
    const createdRule = await request.post(`${apiBase}/api/axflow/rules`, {
      headers: authHeaders,
      data: {
        name: "e2e_low_inventory",
        description: "E2E live inventory rule",
        scope_kind: "METRIC",
        target: { type_name: "Warehouse", metric: "inventory" },
        condition: { kind: "THRESHOLD", op: "LT", value: 50, for_minutes: 0 },
        severity: "CRITICAL",
        title_template:
          "[{{severity}}] {{node.title}} inventory={{value}} (< {{threshold}})",
        body_template: "Rule {{rule.name}} triggered at {{value}}",
        cooldown_minutes: 0,
        resolve_after_minutes: 10,
        channel_ids: channelId ? [channelId] : [],
        enabled: true,
      },
    });
    if (createdRule.ok()) {
      const createdRuleJson = (await createdRule.json()) as { id?: string };
      ruleId = createdRuleJson.id ?? "";
    }
  }

  // 4. Ensure an OPEN alert exists (write low inventory metric if needed)
  let alertId = "";
  const checkOpenAlerts = async (): Promise<string> => {
    const aResp = await request.get(
      `${apiBase}/api/axflow/alerts?status=OPEN`,
      { headers: authHeaders }
    );
    if (!aResp.ok()) return "";
    const aJson = (await aResp.json()) as { items?: { id?: string }[] };
    return aJson.items?.[0]?.id ?? "";
  };

  alertId = await checkOpenAlerts();
  if (!alertId && warehouseNodeId) {
    await request.post(`${apiBase}/api/xblue/metrics/write`, {
      headers: authHeaders,
      data: {
        points: [
          {
            node_id: warehouseNodeId,
            metric: "inventory",
            ts: new Date().toISOString(),
            value: 10,
          },
        ],
        source_tag: "MANUAL",
      },
    });
    for (let attempt = 0; attempt < 15; attempt += 1) {
      alertId = await checkOpenAlerts();
      if (alertId) break;
      await page.waitForTimeout(1000);
    }
  }

  return { alertId, ruleId, channelId };
}

test("AxAlerts /alerts renders active OPEN alert, inspects timeline & sparkline in detail drawer, and ACKs alert (§08.7)", async ({
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
  const handles = isLive
    ? await ensureLiveAlertsFixtures(page, request)
    : await installAxAlertsMocks(page, createInitialMockState());

  await page.goto("/alerts");

  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "编排与告警",
    })
  ).toBeVisible();
  await expect(page.getByTestId("alerts-polling-badge")).toContainText(
    "15s 自动轮询"
  );

  // 1. Verify active OPEN alert row in AlertTable
  const alertRow = page.getByTestId(`alert-row-${handles.alertId}`);
  await expect(alertRow).toBeVisible();
  await expect(
    page.getByTestId(`alert-status-${handles.alertId}`)
  ).toHaveText("待处理");

  // 2. Open AlertDetail drawer and verify timeline events & sparkline
  await page.getByTestId(`open-alert-detail-btn-${handles.alertId}`).click();
  const drawer = page.getByTestId("alert-detail-drawer");
  await expect(drawer).toBeVisible();
  await expect(page.getByTestId("alert-detail-status")).toHaveText("待处理");
  await expect(page.getByTestId("alert-detail-timeline")).toBeVisible();
  await expect(
    page.getByTestId("alert-event-OPENED").first()
  ).toBeVisible();
  await expect(page.getByTestId("alert-metric-sparkline")).toBeVisible();

  // 3. Click ACK inside detail drawer and verify status transitions OPEN -> ACK
  await page
    .getByTestId("alert-detail-comment-input")
    .fill("值班工程师已接手核查华东一号仓补货单");
  await page.getByTestId("alert-detail-ack-btn").click();

  await expect(page.getByTestId("alert-detail-status")).toHaveText("已确认");
  await expect(
    page.getByTestId("alert-event-ACKED").first()
  ).toBeVisible();

  // 4. Close drawer and verify AlertTable row status also reflects ACK
  await page.getByTestId("close-alert-detail-btn").click();
  await expect(drawer).toBeHidden();
  await expect(
    page.getByTestId(`alert-status-${handles.alertId}`)
  ).toHaveText("已确认");

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("AxAlerts /alerts runs rule dry-run preview, verifies sealed channel secret badges + test delivery SENT, and creates silence window", async ({
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
  const handles = isLive
    ? await ensureLiveAlertsFixtures(page, request)
    : await installAxAlertsMocks(page, createInitialMockState());

  await page.goto("/alerts");
  await expect(page.getByTestId("axalerts-page")).toBeVisible();

  // 1. Switch to Rules tab and execute read-only Dry Run preview
  await page.getByTestId("tab-rules").click();
  await expect(page.getByTestId("rule-editor-workspace")).toBeVisible();
  await expect(
    page.getByTestId(`rule-item-${handles.ruleId}`)
  ).toBeVisible();

  await page.getByTestId(`dry-run-rule-btn-${handles.ruleId}`).click();
  const dryRunCard = page.getByTestId("rule-dry-run-results");
  await expect(dryRunCard).toBeVisible();
  await expect(page.getByTestId("dry-run-matched-count")).toContainText(
    "命中触发:"
  );
  if (!isLive) {
    await expect(page.getByTestId("dry-run-matched-count")).toContainText("1");
  }

  // 2. Switch to Channels tab, verify sealed secret_fields_set badge & zero plaintext leak, then trigger Test Delivery
  await page.getByTestId("tab-channels").click();
  const channelCard = page.getByTestId(`channel-card-${handles.channelId}`);
  await expect(channelCard).toBeVisible();

  const secretsBadgeBox = page.getByTestId(
    `channel-secrets-badges-${handles.channelId}`
  );
  await expect(secretsBadgeBox).toContainText("已封存");
  await expect(page.locator("body")).not.toContainText(
    PLAINTEXT_SIGNING_SECRET
  );

  await page.getByTestId(`test-channel-btn-${handles.channelId}`).click();
  const testResultBanner = page.getByTestId(
    `test-channel-result-${handles.channelId}`
  );
  await expect(testResultBanner).toBeVisible();
  await expect(testResultBanner).toContainText("SENT");

  // 3. Switch to Silences tab and create an active maintenance silence window
  await page.getByTestId("tab-silences").click();
  await expect(page.getByTestId("silence-form")).toBeVisible();

  const silenceReason = "华东一号仓例行盘点维护静默窗口";
  await page.getByTestId("silence-reason-input").fill(silenceReason);
  await page.getByTestId("create-silence-btn").click();

  const silencesList = page.getByTestId("silences-list");
  await expect(silencesList).toContainText(silenceReason);
  await expect(silencesList).toContainText("ACTIVE · 生效中");

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
