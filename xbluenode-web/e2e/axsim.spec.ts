import { expect, type Page, type Route, test } from "@playwright/test";
import { chooseSelectOption } from "./helpers";

const SANDBOX_INITIAL_ID = "00000000-0000-0000-0000-000000000801";
const SANDBOX_CREATED_ID = "00000000-0000-0000-0000-000000000802";

const NODE_F1_ID = "00000000-0000-0000-0000-000000000811";
const NODE_W1_ID = "00000000-0000-0000-0000-000000000812";
const NODE_ORD1_ID = "00000000-0000-0000-0000-000000000813";
const NODE_VIRTUAL_ID = "00000000-0000-0000-0000-000000000814";

const LINK_SUPPLIES_ID = "00000000-0000-0000-0000-000000000821";
const LINK_FULFILLS_ID = "00000000-0000-0000-0000-000000000822";

const SCENARIO_S1_ID = "00000000-0000-0000-0000-000000000831";
const SCENARIO_S2_ID = "00000000-0000-0000-0000-000000000832";
const SCENARIO_STEPPED_ID = "00000000-0000-0000-0000-000000000833";

const DECISION_ID = "00000000-0000-0000-0000-000000000841";
const FLOW_ID = "00000000-0000-0000-0000-000000000851";

interface MockNode {
  id: string;
  type_name: string;
  node_key: string;
  title: string;
  metrics: Record<string, number>;
  properties: Record<string, unknown>;
  is_modified: boolean;
  is_new: boolean;
}

interface MockLink {
  id: string;
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight: number;
  is_modified: boolean;
  is_new: boolean;
}

interface MockScenario {
  id: string;
  sandbox_id: string;
  name: string;
  description: string;
  actions: Array<Record<string, unknown>>;
  params: Record<string, unknown>;
  overlay_hash: string;
  is_stale: boolean;
  result: Record<string, unknown> | null;
  status: "PENDING" | "RUNNING" | "SUCCEEDED" | "FAILED";
  error_message: string | null;
  created_by: string | null;
  created_at: string;
  finished_at: string | null;
}

interface MockAxSimState {
  sandboxes: Array<Record<string, unknown>>;
  nodesBySandbox: Record<string, MockNode[]>;
  linksBySandbox: Record<string, MockLink[]>;
  scenariosBySandbox: Record<string, MockScenario[]>;
  overlayHash: string;
  decisionStatus: "PENDING_APPROVAL" | "CONFLICT" | "APPLIED";
  decisionPollCount: number;
  prodMutationUrls: string[];
}

function baseNodes(): MockNode[] {
  return [
    {
      id: NODE_F1_ID,
      type_name: "Factory",
      node_key: "F-1",
      title: "华东工厂",
      metrics: { output: 1000 },
      properties: { region: "east" },
      is_modified: false,
      is_new: false,
    },
    {
      id: NODE_W1_ID,
      type_name: "Warehouse",
      node_key: "W-1",
      title: "华东中心仓",
      metrics: { inventory: 500 },
      properties: { region: "east" },
      is_modified: false,
      is_new: false,
    },
    {
      id: NODE_ORD1_ID,
      type_name: "Order",
      node_key: "ORD-1",
      title: "华东核心订单",
      metrics: { fulfillment: 100 },
      properties: { priority: "P0" },
      is_modified: false,
      is_new: false,
    },
  ];
}

function baseLinks(): MockLink[] {
  return [
    {
      id: LINK_SUPPLIES_ID,
      link_type: "SUPPLIES",
      src_node_id: NODE_F1_ID,
      dst_node_id: NODE_W1_ID,
      weight: 0.8,
      is_modified: false,
      is_new: false,
    },
    {
      id: LINK_FULFILLS_ID,
      link_type: "FULFILLS",
      src_node_id: NODE_W1_ID,
      dst_node_id: NODE_ORD1_ID,
      weight: 0.9,
      is_modified: false,
      is_new: false,
    },
  ];
}

function buildScenarioImpacts(scale: number) {
  return [
    {
      node_id: NODE_F1_ID,
      type_name: "Factory",
      title: "华东工厂",
      metric: "output",
      before_val: 1000,
      after_val: 1000 * (1 + scale),
      delta: 1000 * scale,
      delta_ratio: scale,
      p10: 1000 * (1 + scale),
      p90: 1000 * (1 + scale),
      hop_distance: 0,
      top_path: [NODE_F1_ID],
    },
    {
      node_id: NODE_W1_ID,
      type_name: "Warehouse",
      title: "华东中心仓",
      metric: "inventory",
      before_val: 500,
      after_val: 500 * (1 + scale * 0.8),
      delta: 500 * scale * 0.8,
      delta_ratio: scale * 0.8,
      p10: 500 * (1 + scale * 0.95),
      p90: 500 * (1 + scale * 0.65),
      hop_distance: 1,
      top_path: [NODE_F1_ID, NODE_W1_ID],
    },
    {
      node_id: NODE_ORD1_ID,
      type_name: "Order",
      title: "华东核心订单",
      metric: "fulfillment",
      before_val: 100,
      after_val: 100 * (1 + scale * 0.72),
      delta: 100 * scale * 0.72,
      delta_ratio: scale * 0.72,
      p10: 100 * (1 + scale * 0.88),
      p90: 100 * (1 + scale * 0.56),
      hop_distance: 2,
      top_path: [NODE_F1_ID, NODE_W1_ID, NODE_ORD1_ID],
    },
  ];
}

function createInitialState(): MockAxSimState {
  const s2Preset: MockScenario = {
    id: SCENARIO_S2_ID,
    sandbox_id: SANDBOX_INITIAL_ID,
    name: "baseline_cut_50",
    description: "预置高强度减产对比方案",
    actions: [
      {
        target_node_id: NODE_F1_ID,
        metric_key: "output",
        mode: "RATIO",
        value: -0.5,
        at_step: 0,
      },
    ],
    params: { mode: "STEADY", damping: 0.85 },
    overlay_hash: "hash_v1",
    is_stale: false,
    status: "SUCCEEDED",
    error_message: null,
    created_by: null,
    created_at: "2026-03-01T08:00:00Z",
    finished_at: "2026-03-01T08:00:01Z",
    result: {
      affected_count: 3,
      converged: true,
      iterations: 2,
      warnings: [],
      kpis: [
        {
          type_name: "Order",
          metric: "fulfillment",
          before_sum: 100,
          after_sum: 64,
          delta: -36,
          delta_ratio: -0.36,
          affected_nodes: 1,
        },
      ],
      top_impacts: buildScenarioImpacts(-0.5),
    },
  };

  return {
    sandboxes: [
      {
        id: SANDBOX_INITIAL_ID,
        name: "华东现有沙盘",
        status: "READY",
        scope: { depth: 3, direction: "BOTH", type_names: ["Factory", "Warehouse", "Order"] },
        node_count: 3,
        link_count: 2,
        base_captured_at: "2026-03-01T08:00:00Z",
        schema_version: 1,
        sim_result: null,
        created_at: "2026-03-01T08:00:00Z",
      },
    ],
    nodesBySandbox: {
      [SANDBOX_INITIAL_ID]: baseNodes(),
    },
    linksBySandbox: {
      [SANDBOX_INITIAL_ID]: baseLinks(),
    },
    scenariosBySandbox: {
      [SANDBOX_INITIAL_ID]: [s2Preset],
    },
    overlayHash: "hash_v1",
    decisionStatus: "PENDING_APPROVAL",
    decisionPollCount: 0,
    prodMutationUrls: [],
  };
}

async function installAxSimMocks(
  page: Page,
  state: MockAxSimState,
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
          trace_id: "axsim-e2e-400",
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
              trace_id: "axsim-e2e-body-400",
            }),
          });
          return;
        }
      } catch {
        // non-JSON
      }
    }

    const authHeader = req.headers()["authorization"] ?? "";
    if (!authHeader.startsWith("Bearer ") || authHeader.slice(7).trim().length === 0) {
      missingAuthUrls.push(req.url());
      await route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "TOKEN_MISSING",
          message: "Authorization Bearer token required",
          trace_id: "axsim-e2e-401",
        }),
      });
      return;
    }

    // Track accidental production mutations on /api/xblue/nodes or /api/xblue/links
    if (
      method !== "GET" &&
      (pathname.startsWith("/api/xblue/nodes") || pathname.startsWith("/api/xblue/links")) &&
      !pathname.endsWith("/search")
    ) {
      state.prodMutationUrls.push(`${method} ${pathname}`);
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({
          error_code: "FORBIDDEN_PROD_MUTATION",
          message: "Sandbox operations must never mutate production endpoints",
          trace_id: "axsim-e2e-prod-guard",
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
          permissions: ["DATA_READ", "SIM_RUN", "SIM_APPLY"],
          token_kind: "DEV_STATIC",
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/subgraphs") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            id: "00000000-0000-0000-0000-000000000010",
            slug: "main",
            display_name: "企业主图",
            description: "企业主图",
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

    if (pathname === "/api/xblue/schema") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          subgraph_slug: "main",
          version: 1,
          node_types: [
            { name: "Factory", display_name: "工厂", properties: [], agg_rules: [], derived_rules: [], sync_to_knowledge: false, description: "", icon: "", color: "", title_template: "", pos_x: 0, pos_y: 0 },
            { name: "Warehouse", display_name: "仓库", properties: [], agg_rules: [], derived_rules: [], sync_to_knowledge: false, description: "", icon: "", color: "", title_template: "", pos_x: 0, pos_y: 0 },
            { name: "Order", display_name: "订单", properties: [], agg_rules: [], derived_rules: [], sync_to_knowledge: false, description: "", icon: "", color: "", title_template: "", pos_x: 0, pos_y: 0 },
          ],
          edge_types: [
            { name: "SUPPLIES", src_type_name: "Factory", dst_type_name: "Warehouse", impact_weight: 0.8, cardinality: "MANY_TO_MANY", description: "", properties: [], propagation: {} },
            { name: "FULFILLS", src_type_name: "Warehouse", dst_type_name: "Order", impact_weight: 0.9, cardinality: "MANY_TO_MANY", description: "", properties: [], propagation: {} },
          ],
          mappings: [],
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/graph/subgraph" || pathname === "/api/xblue/nodes") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          nodes: baseNodes().map((n) => ({
            id: n.id,
            type_name: n.type_name,
            node_key: n.node_key,
            title: n.title,
            metrics: n.metrics,
          })),
          links: [],
          truncated: false,
        }),
      });
      return;
    }

    if (pathname === "/api/xblue/nodes/search") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: NODE_F1_ID,
              type_name: "Factory",
              node_key: "F-1",
              title: "华东工厂",
              metrics: { output: 1000 },
              score: 0.99,
              channels: ["EXACT_KEY"],
            },
          ],
        }),
      });
      return;
    }

    if (pathname === "/api/axflow/flows" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: FLOW_ID,
              name: "供应链应急调度流程",
              description: "审批后自动触发调拨",
              nodes: [],
              edges: [],
              trigger_cfg: { type: "SIM_APPLY" },
              status: "PUBLISHED",
              last_run_at: null,
              created_at: "2026-03-01T08:00:00Z",
              updated_at: "2026-03-01T08:00:00Z",
              issues: [],
            },
          ],
          next_cursor: null,
        }),
      });
      return;
    }

    // AxSim Sandboxes
    if (pathname === "/api/axsim/sandboxes" && method === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.sandboxes),
      });
      return;
    }

    if (pathname === "/api/axsim/sandboxes" && method === "POST") {
      const body = JSON.parse(rawPost ?? "{}") as {
        name?: string;
        scope?: Record<string, unknown>;
      };
      const created = {
        id: SANDBOX_CREATED_ID,
        name: body.name ?? "新沙盘",
        status: "READY",
        scope: body.scope ?? { depth: 3, direction: "BOTH" },
        node_count: 3,
        link_count: 2,
        base_captured_at: "2026-03-01T09:00:00Z",
        schema_version: 1,
        sim_result: null,
        created_at: "2026-03-01T09:00:00Z",
      };
      state.sandboxes.unshift(created);
      state.nodesBySandbox[SANDBOX_CREATED_ID] = baseNodes();
      state.linksBySandbox[SANDBOX_CREATED_ID] = baseLinks();
      state.scenariosBySandbox[SANDBOX_CREATED_ID] = [
        {
          id: SCENARIO_S2_ID,
          sandbox_id: SANDBOX_CREATED_ID,
          name: "baseline_cut_50",
          description: "基线减产 50% 对比方案",
          actions: [
            {
              target_node_id: NODE_F1_ID,
              metric_key: "output",
              mode: "RATIO",
              value: -0.5,
              at_step: 0,
            },
          ],
          params: { mode: "STEADY", damping: 0.85 },
          overlay_hash: state.overlayHash,
          is_stale: false,
          status: "SUCCEEDED",
          error_message: null,
          created_by: null,
          created_at: "2026-03-01T09:01:00Z",
          finished_at: "2026-03-01T09:01:01Z",
          result: {
            affected_count: 3,
            converged: true,
            iterations: 2,
            warnings: [],
            kpis: [
              {
                type_name: "Order",
                metric: "fulfillment",
                before_sum: 100,
                after_sum: 64,
                delta: -36,
                delta_ratio: -0.36,
                affected_nodes: 1,
              },
            ],
            top_impacts: buildScenarioImpacts(-0.5),
          },
        },
      ];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    // Sandbox graph
    const graphMatch = pathname.match(/^\/api\/axsim\/sandboxes\/([^/]+)\/graph$/);
    if (graphMatch && method === "GET") {
      const sbId = graphMatch[1];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sandbox_id: sbId,
          nodes: state.nodesBySandbox[sbId] ?? baseNodes(),
          links: state.linksBySandbox[sbId] ?? baseLinks(),
          overlay_hash: state.overlayHash,
        }),
      });
      return;
    }

    // Create overlay node
    const createOverlayNodeMatch = pathname.match(
      /^\/api\/axsim\/sandboxes\/([^/]+)\/nodes$/
    );
    if (createOverlayNodeMatch && method === "POST") {
      const sbId = createOverlayNodeMatch[1];
      const body = JSON.parse(rawPost ?? "{}") as {
        type_name: string;
        node_key: string;
        title: string;
        metrics?: Record<string, number>;
        properties?: Record<string, unknown>;
      };
      const createdNode: MockNode = {
        id: NODE_VIRTUAL_ID,
        type_name: body.type_name,
        node_key: body.node_key,
        title: body.title,
        metrics: body.metrics ?? {},
        properties: body.properties ?? {},
        is_modified: true,
        is_new: true,
      };
      state.nodesBySandbox[sbId] = [
        ...(state.nodesBySandbox[sbId] ?? baseNodes()),
        createdNode,
      ];
      state.overlayHash = "hash_v2_overlay";
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(createdNode),
      });
      return;
    }

    // List / Create scenarios
    const scenariosMatch = pathname.match(
      /^\/api\/axsim\/sandboxes\/([^/]+)\/scenarios$/
    );
    if (scenariosMatch && method === "GET") {
      const sbId = scenariosMatch[1];
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(state.scenariosBySandbox[sbId] ?? []),
      });
      return;
    }

    if (scenariosMatch && method === "POST") {
      const sbId = scenariosMatch[1];
      const body = JSON.parse(rawPost ?? "{}") as {
        name: string;
        description?: string;
        actions: Array<{
          target_node_id: string;
          metric_key: string;
          mode?: string;
          value?: number;
        }>;
        params?: {
          mode?: string;
          horizon_steps?: number;
        };
      };
      const isStepped = body.params?.mode === "STEPPED";
      const scenarioId = isStepped ? SCENARIO_STEPPED_ID : SCENARIO_S1_ID;
      const ratioVal =
        typeof body.actions?.[0]?.value === "number"
          ? body.actions[0].value
          : -0.3;
      const created: MockScenario = {
        id: scenarioId,
        sandbox_id: sbId,
        name: body.name,
        description: body.description ?? "",
        actions: body.actions,
        params: body.params ?? { mode: "STEADY" },
        overlay_hash: state.overlayHash,
        is_stale: false,
        status: "SUCCEEDED",
        error_message: null,
        created_by: null,
        created_at: "2026-03-01T09:05:00Z",
        finished_at: "2026-03-01T09:05:01Z",
        result: {
          affected_count: 3,
          converged: true,
          iterations: isStepped ? 6 : 2,
          warnings: [],
          kpis: [
            {
              type_name: "Order",
              metric: "fulfillment",
              before_sum: 100,
              after_sum: 100 * (1 + ratioVal * 0.72),
              delta: 100 * ratioVal * 0.72,
              delta_ratio: ratioVal * 0.72,
              affected_nodes: 1,
            },
          ],
          top_impacts: buildScenarioImpacts(ratioVal),
        },
      };
      const existing = state.scenariosBySandbox[sbId] ?? [];
      state.scenariosBySandbox[sbId] = [
        created,
        ...existing.filter((s) => s.id !== scenarioId),
      ];
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    // Compare scenarios
    if (pathname === "/api/axsim/scenarios/compare" && method === "GET") {
      const briefing = searchParams.get("briefing") === "true";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sandbox_id: SANDBOX_CREATED_ID,
          scenarios: [
            { id: SCENARIO_S1_ID, name: "cut_30_scenario", status: "SUCCEEDED", is_stale: false },
            { id: SCENARIO_S2_ID, name: "baseline_cut_50", status: "SUCCEEDED", is_stale: false },
          ],
          kpis: [
            {
              type_name: "Order",
              metric: "fulfillment",
              before_sum: 100,
              values: {
                [SCENARIO_S1_ID]: {
                  after_sum: 78.4,
                  delta: -21.6,
                  delta_ratio: -0.216,
                  affected_nodes: 1,
                },
                [SCENARIO_S2_ID]: {
                  after_sum: 64.0,
                  delta: -36.0,
                  delta_ratio: -0.36,
                  affected_nodes: 1,
                },
              },
            },
          ],
          nodes: [
            {
              node_id: NODE_ORD1_ID,
              type_name: "Order",
              title: "华东核心订单",
              metric: "fulfillment",
              before_val: 100,
              max_spread: 14.4,
              values: {
                [SCENARIO_S1_ID]: { after_val: 78.4, delta: -21.6, delta_ratio: -0.216 },
                [SCENARIO_S2_ID]: { after_val: 64.0, delta: -36.0, delta_ratio: -0.36 },
              },
            },
          ],
          briefing: briefing
            ? {
                summary:
                  "建议优先采纳 cut_30_scenario：华东核心订单履约率保留至 78.4%，较 baseline_cut_50（64.0%）减少 14.4 个百分点的履约损失。",
                recommended_scenario_id: SCENARIO_S1_ID,
                reasons: [
                  "Order.fulfillment 汇总保留率更高（78.4 vs 64.0）",
                  "Warehouse 库存缓冲受冲击更可控（-24.0%）",
                ],
                risks: ["需关注 F-1 复产周期，若超过 3 天需启用备选仓"],
              }
            : null,
        }),
      });
      return;
    }

    // Scenario impacts
    const impactsMatch = pathname.match(/^\/api\/axsim\/scenarios\/([^/]+)\/impacts$/);
    if (impactsMatch && method === "GET") {
      const scId = impactsMatch[1];
      const scale = scId === SCENARIO_S2_ID ? -0.5 : -0.3;
      const items = buildScenarioImpacts(scale);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items,
          next_cursor: null,
        }),
      });
      return;
    }

    // Scenario checkpoints
    const ckptMatch = pathname.match(
      /^\/api\/axsim\/scenarios\/([^/]+)\/checkpoints$/
    );
    if (ckptMatch && method === "GET") {
      const scId = ckptMatch[1];
      if (scId === SCENARIO_STEPPED_ID) {
        const items = Array.from({ length: 7 }, (_, step) => ({
          seq: step,
          label: `Step ${step}`,
          changes: {
            [NODE_F1_ID]: { output: 700 },
            ...(step >= 1
              ? { [NODE_W1_ID]: { inventory: Math.round(500 - step * 20) } }
              : {}),
            ...(step >= 2
              ? {
                  [NODE_ORD1_ID]: {
                    fulfillment: Number((100 - step * 3.6).toFixed(1)),
                  },
                }
              : {}),
          },
        }));
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ items }),
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ items: [] }),
        });
      }
      return;
    }

    // SimAsk
    if (pathname === "/api/axsim/ask" && method === "POST") {
      const body = JSON.parse(rawPost ?? "{}") as { question?: string };
      const q = body.question ?? "";
      const isWhatIf = q.includes("如果") || q.includes("假设");
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(
          isWhatIf
            ? {
                intent: "WHAT_IF",
                scenario_id: SCENARIO_S1_ID,
                answer:
                  "若 F-1 华东工厂产能下降 30%，将波及华东中心仓（inventory -24.0%）并导致华东核心订单履约率下降 21.6% 至 78.4。",
                sql: null,
                columns: [],
                rows: [],
                need_clarification: false,
                candidates: [],
                impacts: buildScenarioImpacts(-0.3),
                kpis: [],
              }
            : {
                intent: "QUERY_RESULT",
                scenario_id: SCENARIO_S1_ID,
                answer: "已查询 DuckDB 影响明细表，受影响订单为华东核心订单（fulfillment = 78.4）。",
                sql: "SELECT title, metric, after_val, delta_ratio FROM impacts WHERE type_name = 'Order' ORDER BY ABS(delta) DESC LIMIT 5",
                columns: [
                  { name: "title", data_type: "VARCHAR" },
                  { name: "metric", data_type: "VARCHAR" },
                  { name: "after_val", data_type: "DOUBLE" },
                ],
                rows: [
                  {
                    title: "华东核心订单",
                    metric: "fulfillment",
                    after_val: 78.4,
                  },
                ],
                need_clarification: false,
                candidates: [],
                impacts: [],
                kpis: [],
              }
        ),
      });
      return;
    }

    // Decisions create / get / rebase
    const createDecisionMatch = pathname.match(
      /^\/api\/axsim\/scenarios\/([^/]+)\/decisions$/
    );
    if (createDecisionMatch && method === "POST") {
      const scId = createDecisionMatch[1];
      const body = JSON.parse(rawPost ?? "{}") as {
        title?: string;
        tolerance_ratio?: number;
        trigger_flow_id?: string | null;
      };
      state.decisionStatus = "PENDING_APPROVAL";
      state.decisionPollCount = 0;
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: DECISION_ID,
          scenario_id: scId,
          title: body.title ?? "下调 F-1 产能",
          changes: [
            {
              node_id: NODE_F1_ID,
              metric: "output",
              base_value: 1000,
              target_value: 700,
            },
          ],
          trigger_flow_id: body.trigger_flow_id ?? null,
          status: "PENDING_APPROVAL",
          approval_id: "00000000-0000-0000-0000-000000000899",
          conflict_report: [],
          tolerance_ratio: body.tolerance_ratio ?? 0.05,
          created_by: null,
          created_at: "2026-03-01T09:10:00Z",
          applied_at: null,
          applied_by: null,
        }),
      });
      return;
    }

    if (pathname === `/api/axsim/decisions/${DECISION_ID}` && method === "GET") {
      state.decisionPollCount += 1;
      state.decisionStatus = "CONFLICT";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: DECISION_ID,
          scenario_id: SCENARIO_S1_ID,
          title: "下调 F-1 产能",
          changes: [
            {
              node_id: NODE_F1_ID,
              metric: "output",
              base_value: 1000,
              target_value: 700,
            },
          ],
          trigger_flow_id: FLOW_ID,
          status: "CONFLICT",
          approval_id: "00000000-0000-0000-0000-000000000899",
          conflict_report: [
            {
              node_id: NODE_F1_ID,
              metric: "output",
              base_value: 1000,
              current_value: 1180,
              target_value: 700,
              drift_ratio: 0.18,
              reason: "生产基线已变更 (+18.0%)，超过容差阈值",
            },
          ],
          tolerance_ratio: 0.05,
          created_by: null,
          created_at: "2026-03-01T09:10:00Z",
          applied_at: null,
          applied_by: null,
        }),
      });
      return;
    }

    if (
      pathname === `/api/axsim/decisions/${DECISION_ID}/rebase` &&
      method === "POST"
    ) {
      state.decisionStatus = "PENDING_APPROVAL";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: DECISION_ID,
          scenario_id: SCENARIO_S1_ID,
          title: "下调 F-1 产能",
          changes: [
            {
              node_id: NODE_F1_ID,
              metric: "output",
              base_value: 1180,
              target_value: 700,
            },
          ],
          trigger_flow_id: FLOW_ID,
          status: "PENDING_APPROVAL",
          approval_id: "00000000-0000-0000-0000-000000000900",
          conflict_report: [],
          tolerance_ratio: 0.05,
          created_by: null,
          created_at: "2026-03-01T09:12:00Z",
          applied_at: null,
          applied_by: null,
        }),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({}),
    });
  });
}

test("AxSim Step 10 end-to-end workbench: sandbox, intervention, overlay, stepped replay, compare+briefing, ask, and decision conflict/rebase", async ({
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

  const state = createInitialState();
  await installAxSimMocks(page, state, leakedTenantUrls, missingAuthUrls);

  // 1) Open /sim and create a new sandbox via SandboxWizard
  await page.goto("/sim");
  await expect(
    page.getByRole("heading", { level: 1, name: "AxSim · 因果推演" })
  ).toBeVisible();

  await page.getByTestId("open-sandbox-wizard-btn").click();
  await expect(page.getByTestId("sandbox-wizard-panel")).toBeVisible();
  await page.getByTestId("wizard-sandbox-name").fill("华东断供推演沙盘");
  await page.getByTestId("wizard-create-submit").click();

  await expect(page.getByTestId("sandbox-selector")).toHaveAttribute(
    "data-value",
    SANDBOX_CREATED_ID
  );

  // 2) Add intervention (F-1 / output -30% RATIO) with uncertainty and run scenario
  await chooseSelectOption(page, "action-node-select-0", NODE_F1_ID);
  await page.getByTestId("action-metric-input-0").fill("output");
  await chooseSelectOption(page, "action-mode-select-0", "RATIO");
  await page.getByTestId("action-value-input-0").fill("-0.3");
  await page.getByTestId("sim-uncertainty-toggle").check();
  await page.getByTestId("scenario-name-input").fill("cut_30_scenario");
  await page.getByTestId("run-scenario-btn").click();

  // Assert Order row & P10-P90 render in ImpactTable + ImpactGraph highlights
  const orderRow = page.getByTestId("impact-row-Order-fulfillment");
  await expect(orderRow).toBeVisible();
  await expect(orderRow).toContainText("华东核心订单");
  await expect(orderRow).toContainText("-21.6%");
  await expect(orderRow).toContainText("73.60 ~ 83.20");
  await expect(page.getByTestId(`impact-graph-node-${NODE_ORD1_ID}`)).toBeVisible();

  // Highlight top_path from ImpactTable
  await page.getByTestId(`highlight-path-${NODE_ORD1_ID}`).click();
  await expect(page.getByTestId("impact-graph-card")).toContainText("已高亮传导路径 (3 节点)");

  // 3) Edit virtual overlay (add virtual node) and ensure zero production mutations
  await page.getByTestId("axsim-tab-topology").click();
  await expect(page.getByTestId("overlay-editor")).toBeVisible();
  await page.getByTestId("overlay-add-node-toggle").click();
  await page.getByTestId("overlay-new-node-type").fill("Warehouse");
  await page.getByTestId("overlay-new-node-key").fill("W-VIRTUAL");
  await page.getByTestId("overlay-new-node-title").fill("华东应急备选仓");
  await page.getByTestId("overlay-new-node-metric-key").fill("inventory");
  await page.getByTestId("overlay-new-node-metric-val").fill("300");
  await page.getByTestId("overlay-new-node-submit").click();

  const virtualRow = page.getByTestId("overlay-node-row-W-VIRTUAL");
  await expect(virtualRow).toBeVisible();
  await expect(virtualRow).toContainText("OVERLAY · NEW");
  expect(state.prodMutationUrls).toEqual([]);

  // 4) Switch to STEPPED replay timeline and scrub slider t = 0..6
  await page.getByTestId("axsim-tab-simulate").click();
  await page.getByTestId("sim-mode-stepped").click();
  await page.getByTestId("sim-horizon-steps").fill("6");
  await page.getByTestId("scenario-name-input").fill("stepped_cut_30");
  await page.getByTestId("run-scenario-btn").click();

  const slider = page.getByTestId("replay-step-slider");
  await expect(slider).toBeVisible();
  await expect(page.getByTestId("replay-current-step-badge")).toContainText("t = 6");
  await slider.fill("2");
  await expect(page.getByTestId("replay-current-step-badge")).toContainText("t = 2");
  await expect(page.getByTestId(`replay-diff-${NODE_ORD1_ID}-fulfillment`)).toContainText("92.8");

  // 5) Compare two scenarios & trigger decision briefing
  await page.getByTestId("axsim-tab-compare").click();
  await page.getByTestId("compare-select-cut_30_scenario").check();
  await page.getByTestId("compare-select-baseline_cut_50").check();
  await page.getByTestId("compare-briefing-btn").click();

  await expect(page.getByTestId("compare-kpi-table")).toContainText("Order");
  await expect(page.getByTestId("compare-nodes-table")).toContainText("华东核心订单");
  await expect(page.getByTestId("compare-briefing-card")).toContainText("建议优先采纳 cut_30_scenario");

  // 6) Submit SimAsk What-If query
  await page.getByTestId("sim-ask-input").fill("如果 F-1 产能下降 30%，会对哪些订单履约造成影响？");
  await page.getByTestId("sim-ask-submit").click();
  await expect(page.getByTestId("sim-ask-intent-badge")).toHaveText("WHAT_IF");
  await expect(page.getByTestId("sim-ask-response")).toContainText("78.4");

  // 7) Propose decision & test conflict/rebase flow
  await page.getByTestId("axsim-tab-decisions").click();
  await chooseSelectOption(page, "decision-flow-select", FLOW_ID);
  await page.getByTestId("create-decision-btn").click();

  await expect(page.getByTestId("decision-status-badge")).toContainText("PENDING_APPROVAL");
  await page.getByTestId("decision-refresh-btn").click();
  await expect(page.getByTestId("decision-status-badge")).toContainText("CONFLICT");
  await expect(page.getByTestId("decision-conflict-table")).toContainText("1180");

  await page.getByTestId("decision-rebase-btn").click();
  await expect(page.getByTestId("decision-status-badge")).toContainText("PENDING_APPROVAL");

  expect(state.prodMutationUrls).toEqual([]);
  expect(leakedTenantUrls).toEqual([]);
  expect(missingAuthUrls).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});
