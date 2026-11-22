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

const SUPERVISOR_AGENT_ID = "00000000-0000-0000-0000-000000000901";
const ANALYST_AGENT_ID = "00000000-0000-0000-0000-000000000902";
const ENDPOINT_METRIC_ID = "00000000-0000-0000-0000-000000000911";
const ENDPOINT_STAGE_ID = "00000000-0000-0000-0000-000000000912";
const ENDPOINT_PROMOTED_ID = "00000000-0000-0000-0000-000000000913";
const PROVIDER_PRIMARY_ID = "00000000-0000-0000-0000-000000000921";
const MCP_CLIENT_ID = "00000000-0000-0000-0000-000000000931";
const WORKBENCH_CONV_ID = "00000000-0000-0000-0000-000000000951";
const EMBED_CONV_ID = "00000000-0000-0000-0000-000000000981";
const EMBED_RECORD_ID = "00000000-0000-0000-0000-000000000991";

interface MockAxAgentState {
  promotedToProd: boolean;
  mcpSynced: boolean;
  embedTurnCount: number;
  embedReceivedConvIds: Array<string | null>;
  embedAuthHeaders: string[];
  persistedWorkbenchMessages: Array<Record<string, unknown>>;
}

function createMockState(): MockAxAgentState {
  return {
    promotedToProd: false,
    mcpSynced: false,
    embedTurnCount: 0,
    embedReceivedConvIds: [],
    embedAuthHeaders: [],
    persistedWorkbenchMessages: [],
  };
}

function attachRuntimeMonitors(
  page: Page,
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
}

async function installAxAgentMocks(
  page: Page,
  state: MockAxAgentState,
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
          trace_id: "axagent-e2e-400",
        }),
      });
      return;
    }

    const rawPost = req.postData();
    if (rawPost) {
      try {
        const parsed: unknown = JSON.parse(rawPost);
        if (
          typeof parsed === "object" &&
          parsed !== null &&
          !Array.isArray(parsed) &&
          "tenant_id" in parsed
        ) {
          leakedTenantUrls.push(req.url());
          await route.fulfill({
            status: 400,
            contentType: "application/json",
            body: JSON.stringify({
              error_code: "INVALID_ARGUMENT",
              message: "tenant_id body field is forbidden",
              trace_id: "axagent-e2e-body-400",
            }),
          });
          return;
        }
      } catch {
        // non-JSON body
      }
    }

    // Public unauthenticated embed chat endpoint
    if (pathname.startsWith("/api/axagent/embed/") && method === "POST") {
      const authHeader = req.headers()["authorization"];
      if (authHeader) {
        state.embedAuthHeaders.push(authHeader);
      }

      if (pathname.includes("axe_revoked_token")) {
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "NOT_FOUND",
            message: "Embed link revoked or expired",
            trace_id: "embed-404",
          }),
        });
        return;
      }

      if (pathname.includes("axe_origin_blocked")) {
        await route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({
            error_code: "FORBIDDEN",
            message: "Origin not allowed for embed link",
            trace_id: "embed-403",
          }),
        });
        return;
      }

      const body = JSON.parse(rawPost ?? "{}") as {
        message?: string;
        conversation_id?: string;
      };
      state.embedTurnCount += 1;
      state.embedReceivedConvIds.push(body.conversation_id ?? null);

      const turn = state.embedTurnCount;
      const sseFrames =
        turn === 1
          ? [
              `event: conversation_started\ndata: ${JSON.stringify({
                type: "conversation_started",
                conversation_id: EMBED_CONV_ID,
              })}\n\n`,
              `event: tool_call\ndata: ${JSON.stringify({
                type: "tool_call",
                id: "embed_call_1",
                name: "query_objects",
                arguments: { type_name: "Warehouse", keys: ["W-1"] },
              })}\n\n`,
              `event: tool_result\ndata: ${JSON.stringify({
                type: "tool_result",
                tool_call_id: "embed_call_1",
                name: "query_objects",
                content: JSON.stringify({
                  items: [{ node_key: "W-1", title: "华东一号仓", inventory: 500 }],
                }),
                truncated: false,
              })}\n\n`,
              `event: token\ndata: ${JSON.stringify({
                type: "token",
                delta: "华东一号仓当前在库库存为 500 件，运行正常。",
              })}\n\n`,
              `event: done\ndata: ${JSON.stringify({
                type: "done",
                conversation_id: EMBED_CONV_ID,
              })}\n\n`,
            ].join("")
          : [
              `event: conversation_started\ndata: ${JSON.stringify({
                type: "conversation_started",
                conversation_id: EMBED_CONV_ID,
              })}\n\n`,
              `event: token\ndata: ${JSON.stringify({
                type: "token",
                delta: "根据仓储管理规范，安全库存阈值为 50 件。",
              })}\n\n`,
              `event: done\ndata: ${JSON.stringify({
                type: "done",
                conversation_id: EMBED_CONV_ID,
              })}\n\n`,
            ].join("");

      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: sseFrames,
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

    const authHeader = req.headers()["authorization"] ?? "";
    if (
      !authHeader.startsWith("Bearer ") ||
      authHeader.slice(7).trim().length === 0
    ) {
      missingAuthUrls.push(req.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "axagent-e2e-401",
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
            "STUDIO_PUBLISH",
            "FLOW_RUN",
            "FLOW_PUBLISH",
            "AGENT_EDIT",
            "AGENT_RUN",
            "VIEW_SHARE",
            "APPROVAL_DECIDE",
            "GUARD_ADMIN",
          ],
          token_kind: "DEV_STATIC",
        }),
      });
      return;
    }

    if (pathname === "/api/auth/tokens" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      });
      return;
    }

    if (pathname === "/api/auth/tokens" && method === "POST") {
      const body = JSON.parse(rawPost ?? "{}") as {
        name?: string;
        scopes?: string[];
      };
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: "00000000-0000-0000-0000-000000000941",
          name: body.name ?? "test-pat",
          token_prefix: "axp_e2e12345",
          token: "axp_e2e12345_secret0123456789abcdef0123456789",
          scopes: body.scopes ?? ["DATA_READ", "AGENT_RUN"],
          expires_at: null,
          created_at: "2026-02-01T08:00:00Z",
        }),
      });
      return;
    }

    const agentsList = [
      {
        id: SUPERVISOR_AGENT_ID,
        tenant_id: "default",
        name: "ops_supervisor",
        description: "供应链运营主管智能体，协同数据问数与制度专家",
        spec: {
          system_prompt: "你是供应链运营主管智能体。",
          provider_purpose: "AGENT",
          provider_id: null,
          temperature: 0.2,
          max_steps: 8,
          memory_turns: 10,
          tools: [
            { kind: "BUILTIN", name: "ask_data" },
            { kind: "BUILTIN", name: "search_knowledge" },
            {
              kind: "AGENT",
              agent_id: ANALYST_AGENT_ID,
              as_tool_name: "data_analyst",
            },
          ],
          approval_required_tools: ["start_flow"],
          knowledge_scope: {
            doc_title_prefix: null,
            dataset_names: ["demo_orders"],
          },
          output_schema: null,
        },
        version: 1,
        created_by: "00000000-0000-0000-0000-000000000000",
        created_at: "2026-02-01T08:00:00Z",
        updated_at: "2026-02-01T08:00:00Z",
      },
      {
        id: ANALYST_AGENT_ID,
        tenant_id: "default",
        name: "data_analyst",
        description: "智能问数与 SQL 核验专家",
        spec: {
          system_prompt: "你是数据分析专家。",
          provider_purpose: "AGENT",
          provider_id: null,
          temperature: 0.1,
          max_steps: 6,
          memory_turns: 8,
          tools: [{ kind: "BUILTIN", name: "ask_data" }],
          approval_required_tools: [],
          knowledge_scope: {
            doc_title_prefix: null,
            dataset_names: ["demo_orders"],
          },
          output_schema: null,
        },
        version: 1,
        created_by: "00000000-0000-0000-0000-000000000000",
        created_at: "2026-02-01T08:00:00Z",
        updated_at: "2026-02-01T08:00:00Z",
      },
    ];

    if (pathname === "/api/axagent/agents" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(agentsList),
      });
      return;
    }

    if (
      pathname === `/api/axagent/agents/${SUPERVISOR_AGENT_ID}/chat` &&
      method === "POST"
    ) {
      const body = JSON.parse(rawPost ?? "{}") as { message?: string };
      const question = body.message ?? "";
      const answerText = "已支付订单总金额为 128,500 元，覆盖 3 笔已完成履约订单。";

      state.persistedWorkbenchMessages = [
        {
          id: 1,
          tenant_id: "default",
          conversation_id: WORKBENCH_CONV_ID,
          role: "USER",
          content: question,
          created_at: "2026-02-01T08:10:00Z",
        },
        {
          id: 2,
          tenant_id: "default",
          conversation_id: WORKBENCH_CONV_ID,
          role: "ASSISTANT",
          content: "",
          tool_calls: [
            {
              id: "call_ask_data_01",
              name: "ask_data",
              arguments: { question },
            },
          ],
          agent_name: "data_analyst",
          prompt_tokens: 96,
          completion_tokens: 28,
          latency_ms: 140,
          created_at: "2026-02-01T08:10:01Z",
        },
        {
          id: 3,
          tenant_id: "default",
          conversation_id: WORKBENCH_CONV_ID,
          role: "TOOL",
          content: JSON.stringify({
            rows: [{ status: "PAID", paid_amount: 128500 }],
          }),
          tool_call_id: "call_ask_data_01",
          agent_name: "data_analyst",
          created_at: "2026-02-01T08:10:02Z",
        },
        {
          id: 4,
          tenant_id: "default",
          conversation_id: WORKBENCH_CONV_ID,
          role: "ASSISTANT",
          content: answerText,
          agent_name: "ops_supervisor",
          prompt_tokens: 142,
          completion_tokens: 45,
          latency_ms: 185,
          created_at: "2026-02-01T08:10:03Z",
        },
      ];

      const sseBody = [
        `event: conversation_started\ndata: ${JSON.stringify({
          type: "conversation_started",
          conversation_id: WORKBENCH_CONV_ID,
        })}\n\n`,
        `event: agent_switched\ndata: ${JSON.stringify({
          type: "agent_switched",
          agent_name: "data_analyst",
        })}\n\n`,
        `event: tool_call\ndata: ${JSON.stringify({
          type: "tool_call",
          id: "call_ask_data_01",
          name: "ask_data",
          arguments: { question },
          agent_name: "data_analyst",
        })}\n\n`,
        `event: tool_result\ndata: ${JSON.stringify({
          type: "tool_result",
          tool_call_id: "call_ask_data_01",
          name: "ask_data",
          content: JSON.stringify({
            rows: [{ status: "PAID", paid_amount: 128500 }],
          }),
          truncated: false,
          agent_name: "data_analyst",
        })}\n\n`,
        `event: token\ndata: ${JSON.stringify({
          type: "token",
          delta: "已支付订单总金额为 128,500 元，",
          agent_name: "ops_supervisor",
        })}\n\n`,
        `event: token\ndata: ${JSON.stringify({
          type: "token",
          delta: "覆盖 3 笔已完成履约订单。",
          agent_name: "ops_supervisor",
        })}\n\n`,
        `event: done\ndata: ${JSON.stringify({
          type: "done",
          conversation_id: WORKBENCH_CONV_ID,
        })}\n\n`,
      ].join("");

      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: sseBody,
      });
      return;
    }

    if (pathname === "/api/axagent/conversations" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          state.persistedWorkbenchMessages.length > 0
            ? [
                {
                  id: WORKBENCH_CONV_ID,
                  tenant_id: "default",
                  agent_id: SUPERVISOR_AGENT_ID,
                  agent_version: 1,
                  user_id: "00000000-0000-0000-0000-000000000000",
                  channel: "WORKBENCH",
                  embed_id: null,
                  title: "已支付订单总金额是多少？",
                  status: "ACTIVE",
                  created_at: "2026-02-01T08:10:00Z",
                  updated_at: "2026-02-01T08:10:03Z",
                },
              ]
            : []
        ),
      });
      return;
    }

    if (
      pathname === `/api/axagent/conversations/${WORKBENCH_CONV_ID}/messages` &&
      method === "GET"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: state.persistedWorkbenchMessages,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axagent/agents/${SUPERVISOR_AGENT_ID}/embeds` &&
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
      pathname === `/api/axagent/agents/${SUPERVISOR_AGENT_ID}/embeds` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: EMBED_RECORD_ID,
          agent_id: SUPERVISOR_AGENT_ID,
          token: "axe_test_token_123456",
          url: "/share/agent/axe_test_token_123456",
          allowed_origins: [],
          permissions: ["DATA_READ", "AGENT_RUN"],
          rate_limit_rpm: 20,
          expires_at: null,
          created_at: "2026-02-01T08:15:00Z",
        }),
      });
      return;
    }

    const baseEndpoints = [
      {
        id: ENDPOINT_METRIC_ID,
        tenant_id: "default",
        slug: "paid-amount-by-status",
        environment: "PROD",
        kind: "METRIC",
        target_id: "paid_amount",
        target_version: 1,
        target_snapshot: { name: "paid_amount" },
        title: "已支付金额（按状态）",
        description: "按订单状态与月度粒度聚合已支付金额",
        input_schema: {
          type: "object",
          properties: {
            grain: {
              type: "string",
              enum: ["DAY", "MONTH"],
              default: "MONTH",
            },
          },
        },
        output_shape: "ROWS",
        required_scope: "AGENT_RUN",
        rate_limit_rpm: 60,
        cache_ttl_s: 30,
        expose_mcp: true,
        show_as_app: true,
        enabled: true,
        published_by: "00000000-0000-0000-0000-000000000000",
        published_at: "2026-02-01T08:00:00Z",
      },
      {
        id: ENDPOINT_STAGE_ID,
        tenant_id: "default",
        slug: "warehouse-objects",
        environment: "TEST",
        kind: "OBJECT_TYPE",
        target_id: "Warehouse",
        target_version: null,
        target_snapshot: null,
        title: "仓储领域对象查询",
        description: "查询主图已发布 Warehouse 节点属性与指标",
        input_schema: {
          type: "object",
          properties: {
            q: { type: "string" },
          },
        },
        output_shape: "OBJECT",
        required_scope: "DATA_READ",
        rate_limit_rpm: 60,
        cache_ttl_s: 0,
        expose_mcp: true,
        show_as_app: true,
        enabled: true,
        published_by: "00000000-0000-0000-0000-000000000000",
        published_at: "2026-02-01T08:02:00Z",
      },
    ];

    if (state.promotedToProd) {
      baseEndpoints.push({
        id: ENDPOINT_PROMOTED_ID,
        tenant_id: "default",
        slug: "warehouse-objects",
        environment: "PROD",
        kind: "OBJECT_TYPE",
        target_id: "Warehouse",
        target_version: 1,
        target_snapshot: { name: "Warehouse" },
        title: "仓储领域对象查询",
        description: "查询主图已发布 Warehouse 节点属性与指标",
        input_schema: {
          type: "object",
          properties: {
            q: { type: "string" },
          },
        },
        output_shape: "OBJECT",
        required_scope: "DATA_READ",
        rate_limit_rpm: 60,
        cache_ttl_s: 0,
        expose_mcp: true,
        show_as_app: true,
        enabled: true,
        published_by: "00000000-0000-0000-0000-000000000000",
        published_at: "2026-02-01T08:05:00Z",
      });
    }

    if (pathname === "/api/axagent/endpoints" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(baseEndpoints),
      });
      return;
    }

    if (
      pathname === `/api/axagent/endpoints/${ENDPOINT_STAGE_ID}/promote` &&
      method === "POST"
    ) {
      state.promotedToProd = true;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: ENDPOINT_PROMOTED_ID,
          tenant_id: "default",
          slug: "warehouse-objects",
          environment: "PROD",
          kind: "OBJECT_TYPE",
          target_id: "Warehouse",
          target_version: 1,
          target_snapshot: { name: "Warehouse" },
          title: "仓储领域对象查询",
          description: "查询主图已发布 Warehouse 节点属性与指标",
          input_schema: { type: "object", properties: { q: { type: "string" } } },
          output_shape: "OBJECT",
          required_scope: "DATA_READ",
          rate_limit_rpm: 60,
          cache_ttl_s: 0,
          expose_mcp: true,
          show_as_app: true,
          enabled: true,
          published_by: "00000000-0000-0000-0000-000000000000",
          published_at: "2026-02-01T08:05:00Z",
        }),
      });
      return;
    }

    if (pathname === "/api/axagent/endpoints/openapi.json" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          openapi: "3.1.0",
          info: { title: "AxAgent Published Capabilities", version: "1.0.0" },
          paths: {
            "/api/published/prod/default/paid-amount-by-status": { post: {} },
            "/api/published/prod/default/warehouse-objects": { post: {} },
          },
        }),
      });
      return;
    }

    if (
      pathname === `/api/axagent/apps/${ENDPOINT_METRIC_ID}/run` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          rows: [
            { status: "PAID", month: "2026-01", paid_amount: 128500 },
            { status: "PAID", month: "2026-02", paid_amount: 94200 },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axagent/providers" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: PROVIDER_PRIMARY_ID,
            tenant_id: "default",
            name: "primary-gateway",
            base_url: "https://llm.example.com/v1",
            model: "qwen-max-instruct",
            purposes: ["AGENT", "CHAT", "NL2SQL"],
            has_api_key: true,
            priority: 10,
            temperature: 0.2,
            max_tokens: 4096,
            timeout_s: 60,
            rpm_limit: 120,
            supports_tools: true,
            embedding_dim: null,
            allow_private_network: false,
            enabled: true,
            created_at: "2026-02-01T07:00:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === `/api/axagent/providers/${PROVIDER_PRIMARY_ID}/test` &&
      method === "POST"
    ) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          ok: true,
          latency_ms: 42,
          model: "qwen-max-instruct",
          reply: "PONG",
        }),
      });
      return;
    }

    if (pathname === "/api/axagent/usage" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            provider_id: PROVIDER_PRIMARY_ID,
            provider_name: "primary-gateway",
            usage_date: "2026-02-01",
            purpose: "AGENT",
            requests: 18,
            errors: 0,
            prompt_tokens: 6420,
            completion_tokens: 1890,
          },
        ]),
      });
      return;
    }

    if (pathname === "/api/axagent/mcp-clients" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: MCP_CLIENT_ID,
            tenant_id: "default",
            name: "supply_mcp",
            url: "https://mcp.supply.example.com/mcp",
            has_headers: true,
            allowed_tools: [],
            tools_cache: state.mcpSynced
              ? [
                  {
                    name: "track_shipment",
                    description: "实时查询干线物流运单轨迹",
                    input_schema: { type: "object" },
                  },
                ]
              : [],
            allow_private_network: false,
            enabled: true,
            last_synced_at: state.mcpSynced ? "2026-02-01T08:20:00Z" : null,
            created_at: "2026-02-01T07:30:00Z",
          },
        ]),
      });
      return;
    }

    if (
      pathname === `/api/axagent/mcp-clients/${MCP_CLIENT_ID}/sync` &&
      method === "POST"
    ) {
      state.mcpSynced = true;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: MCP_CLIENT_ID,
          tenant_id: "default",
          name: "supply_mcp",
          url: "https://mcp.supply.example.com/mcp",
          has_headers: true,
          allowed_tools: [],
          tools_cache: [
            {
              name: "track_shipment",
              description: "实时查询干线物流运单轨迹",
              input_schema: { type: "object" },
            },
          ],
          allow_private_network: false,
          enabled: true,
          last_synced_at: "2026-02-01T08:20:00Z",
          created_at: "2026-02-01T07:30:00Z",
        }),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([]),
    });
  });
}

async function installLiveApiProxyAndSeed(
  page: Page,
  request: APIRequestContext
): Promise<{ token: string; targetAgentId: string }> {
  const apiBase =
    process.env.PLAYWRIGHT_API_BASE_URL ?? "http://127.0.0.1:9995";
  const token =
    process.env.AXPOINT_DEV_TOKEN ??
    process.env.VITE_DEV_TOKEN ??
    "dev-admin-token";

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

  const authHeaders = {
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
  };

  let targetAgentId = "";
  const agentsResp = await request.get(`${apiBase}/api/axagent/agents`, {
    headers: authHeaders,
  });
  if (agentsResp.ok()) {
    const raw = (await agentsResp.json()) as
      | Array<{ id: string; name: string }>
      | { items?: Array<{ id: string; name: string }> };
    const items = Array.isArray(raw) ? raw : (raw.items ?? []);
    const supervisor = items.find((a) => a.name === "ops_supervisor");
    const analyst = items.find((a) => a.name === "data_analyst");
    targetAgentId = supervisor?.id ?? analyst?.id ?? items[0]?.id ?? "";
  }

  if (!targetAgentId) {
    const created = await request.post(`${apiBase}/api/axagent/agents`, {
      headers: authHeaders,
      data: {
        name: "data_analyst",
        description: "E2E live data analyst agent",
        spec: {
          system_prompt: "你是企业数据分析助手，使用 ask_data 与 search_knowledge 回答。",
          provider_purpose: "AGENT",
          temperature: 0.2,
          max_steps: 6,
          memory_turns: 8,
          tools: [
            { kind: "BUILTIN", name: "ask_data" },
            { kind: "BUILTIN", name: "search_knowledge" },
          ],
          approval_required_tools: [],
          knowledge_scope: { doc_title_prefix: null, dataset_names: [] },
          output_schema: null,
        },
      },
    });
    if (created.ok()) {
      const createdJson = (await created.json()) as { id: string };
      targetAgentId = createdJson.id;
      await request.post(
        `${apiBase}/api/axagent/agents/${encodeURIComponent(targetAgentId)}/publish`,
        { headers: authHeaders }
      );
    }
  }

  return { token, targetAgentId };
}

test("AxAgent 6-tab workbench, SSE tool-calling stream, Query-as-App, Endpoint promote, Provider test, and MCP sync (§09.10)", async ({
  page,
  request,
}) => {
  const isLive = process.env.PLAYWRIGHT_LIVE_API === "1";
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];

  attachRuntimeMonitors(page, consoleErrors, pageErrors);

  if (isLive) {
    const { token, targetAgentId } = await installLiveApiProxyAndSeed(
      page,
      request
    );

    await page.goto("/login");
    await page.getByTestId("pat-accordion-toggle").click();
    await page.locator("#access-token-input").fill(token);
    await page.getByTestId("pat-submit-btn").click();
    await expect(page).toHaveURL(/\/$/);

    await page.goto("/agents");
    await expect(
      page.getByRole("heading", { level: 1, name: "AxAgent 智能体与能力发布" })
    ).toBeVisible();

    if (targetAgentId) {
      await page
        .getByTestId("workbench-agent-select")
        .selectOption(targetAgentId);
    }

    await page
      .getByTestId("workbench-chat-input")
      .fill("已支付订单总金额是多少？依据哪份制度补货？");
    await page.getByTestId("workbench-send-btn").click();

    await expect(
      page.getByTestId("chat-message-assistant").last()
    ).toBeVisible({ timeout: 30_000 });

    const toolCount = await page.getByTestId("tool-call-card").count();
    const badgeCount = await page.getByTestId("specialist-badge").count();
    expect(toolCount + badgeCount).toBeGreaterThan(0);

    // Create real embed link via UI and verify public /share/agent/:token reply
    await page.getByTestId("axagent-tab-agents").click();
    await page.getByTestId("open-embed-dialog-btn").click();
    await page.getByTestId("create-embed-btn").click();
    const shareUrlText = await page
      .getByTestId("embed-share-url")
      .textContent();
    expect(shareUrlText).toBeTruthy();
    const embedPath = new URL(shareUrlText!.trim()).pathname;

    await page.goto(embedPath);
    await expect(page.getByTestId("agent-embed-page")).toBeVisible();
    await page.getByTestId("embed-chat-input").fill("你好");
    const chatRespPromise = page.waitForResponse(
      (res) =>
        res.url().includes("/api/axagent/embed/") && res.request().method() === "POST"
    );
    await page.getByTestId("embed-send-btn").click();
    const chatResp = await chatRespPromise;
    expect(chatResp.status()).toBe(200);
    await expect(
      page.getByTestId("chat-message-assistant").first()
    ).toBeVisible({ timeout: 30_000 });

    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
    return;
  }

  // Deterministic Mock Mode
  const mockState = createMockState();
  await installAxAgentMocks(
    page,
    mockState,
    leakedTenantUrls,
    missingAuthUrls
  );

  await page.goto("/agents");
  await expect(
    page.getByRole("heading", { level: 1, name: "AxAgent 智能体与能力发布" })
  ).toBeVisible();

  // Verify all 6 tabs render cleanly
  for (const key of [
    "workbench",
    "agents",
    "apps",
    "endpoints",
    "providers",
    "mcp",
  ]) {
    await expect(page.getByTestId(`axagent-tab-${key}`)).toBeVisible();
  }

  // 1. Workbench tab: SSE streaming with ask_data tool call card + final answer
  await page
    .getByTestId("workbench-chat-input")
    .fill("已支付订单总金额是多少？");
  await page.getByTestId("workbench-send-btn").click();

  const toolCard = page.getByTestId("tool-call-card").first();
  await expect(toolCard).toBeVisible();
  await expect(toolCard).toContainText("ask_data");
  await expect(
    page.getByTestId("chat-message-assistant").last()
  ).toContainText("128,500");

  // 2. Agents tab: inspect editor & ToolPicker, create embed link
  await page.getByTestId("axagent-tab-agents").click();
  await expect(page.getByTestId("tool-picker")).toBeVisible();
  await page.getByTestId("open-embed-dialog-btn").click();
  await page.getByTestId("create-embed-btn").click();
  await expect(page.getByTestId("embed-share-url")).toContainText(
    "/share/agent/axe_test_token_123456"
  );
  await page.getByTestId("embed-dialog").locator("button").first().click();

  // 3. Apps tab: open AppCard drawer & execute dynamic form
  await page.getByTestId("axagent-tab-apps").click();
  await page.getByTestId("open-app-btn-paid-amount-by-status").click();
  await expect(page.getByTestId("app-runner-drawer")).toBeVisible();
  await page.getByTestId("app-run-submit-btn").click();
  await expect(page.getByTestId("app-run-result")).toContainText("128500");
  await expect(page.getByTestId("app-export-csv-btn")).toBeVisible();
  await page
    .getByTestId("app-runner-drawer")
    .locator("button")
    .first()
    .click();

  // 4. Endpoints tab: promote TEST -> PROD & view OpenAPI 3.1
  await page.getByTestId("axagent-tab-endpoints").click();
  await page.getByTestId("promote-endpoint-btn-warehouse-objects").click();
  await expect(page.getByTestId("endpoint-promote-feedback")).toContainText(
    "PROD"
  );
  await expect(
    page.getByTestId("endpoint-row-warehouse-objects-PROD")
  ).toBeVisible();

  await page.getByTestId("view-openapi-btn").click();
  await expect(page.getByTestId("openapi-modal")).toContainText("3.1.0");
  await page.getByTestId("close-openapi-modal-btn").click();

  // 5. Providers tab: test provider connectivity
  await page.getByTestId("axagent-tab-providers").click();
  await page.getByTestId("test-provider-btn-primary-gateway").click();
  await expect(
    page.getByTestId("provider-test-result-primary-gateway")
  ).toContainText("42ms");
  await expect(
    page.getByTestId("provider-test-result-primary-gateway")
  ).toContainText("qwen-max-instruct");

  // 6. MCP tab: sync external MCP client tools
  await page.getByTestId("axagent-tab-mcp").click();
  await expect(page.getByTestId("builtin-mcp-server-card")).toBeVisible();
  await page.getByTestId("sync-mcp-btn-supply_mcp").click();
  await expect(page.getByTestId("mcp-tools-list-supply_mcp")).toContainText(
    "track_shipment"
  );

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("Standalone /share/agent/:token renders multi-turn unauthenticated embed chat without sidebar and zero console errors", async ({
  page,
}) => {
  test.skip(
    process.env.PLAYWRIGHT_LIVE_API === "1",
    "Covered by live embed flow in primary test"
  );

  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const leakedTenantUrls: string[] = [];
  const missingAuthUrls: string[] = [];
  const mockState = createMockState();

  attachRuntimeMonitors(page, consoleErrors, pageErrors);
  await installAxAgentMocks(
    page,
    mockState,
    leakedTenantUrls,
    missingAuthUrls
  );

  await page.goto("/share/agent/axe_test_token_123456");
  await expect(page.getByTestId("agent-embed-page")).toBeVisible();
  await expect(page.locator("aside")).toHaveCount(0);

  // Turn 1: initial question without conversation_id
  await page
    .getByTestId("embed-chat-input")
    .fill("你好，请查询华东一号仓状态");
  await page.getByTestId("embed-send-btn").click();

  await expect(page.getByTestId("tool-call-card").first()).toContainText(
    "query_objects"
  );
  await expect(
    page.getByTestId("chat-message-assistant").first()
  ).toContainText("500 件");
  await expect(page.getByTestId("embed-conv-badge")).toBeVisible();

  // Turn 2: follow-up question carrying conversation_id in memory
  await page.getByTestId("embed-chat-input").fill("继续说明安全库存阈值");
  await page.getByTestId("embed-send-btn").click();
  await expect(
    page.getByTestId("chat-message-assistant").nth(1)
  ).toContainText("50 件");

  expect(mockState.embedReceivedConvIds).toEqual([null, EMBED_CONV_ID]);
  expect(mockState.embedAuthHeaders).toEqual([]);

  // Reset conversation via "新对话"
  await page.getByTestId("embed-reset-btn").click();
  await expect(page.getByTestId("chat-message-assistant")).toHaveCount(0);

  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test("AgentEmbedPage displays clear error banners on revoked (404) and origin-blocked (403) embed tokens", async ({
  page,
}) => {
  test.skip(
    process.env.PLAYWRIGHT_LIVE_API === "1",
    "Mock-specific 403/404 embed error banner assertions"
  );

  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(err.message));

  await installAxAgentMocks(page, createMockState(), [], []);

  await page.goto("/share/agent/axe_revoked_token");
  await page.getByTestId("embed-chat-input").fill("你好");
  await page.getByTestId("embed-send-btn").click();
  await expect(page.getByTestId("embed-error-banner")).toContainText("404");

  await page.goto("/share/agent/axe_origin_blocked");
  await page.getByTestId("embed-chat-input").fill("你好");
  await page.getByTestId("embed-send-btn").click();
  await expect(page.getByTestId("embed-error-banner")).toContainText("403");

  expect(pageErrors).toEqual([]);
});

