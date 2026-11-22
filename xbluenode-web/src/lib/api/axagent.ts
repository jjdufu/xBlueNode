import { z } from "zod";
import { ApiError, ApiErrorBodySchema, requestJson } from "./client";
import { getAccessToken } from "./auth";
import type {
  EndpointKind,
  Environment,
  Permission,
  StructuredOutputMode,
} from "./enums";

export type { StructuredOutputMode };

export type ProviderScope = "SYSTEM" | "PERSONAL";

export type LlmPurpose =
  | "CHAT"
  | "EMBED"
  | "NL2SQL"
  | "EXTRACT"
  | "AGENT"
  | "SUMMARIZE";

export type OutputShape = "ROWS" | "OBJECT" | "TEXT" | "RUN";

export type ConversationChannel =
  | "WORKBENCH"
  | "EMBED"
  | "API"
  | "FLOW"
  | "MCP";

export type ConversationStatus = "ACTIVE" | "WAITING_APPROVAL" | "CLOSED";

export type MessageRole = "USER" | "ASSISTANT" | "TOOL" | "SYSTEM";

export interface PersonalAccessTokenSummary {
  id: string;
  name: string;
  token_prefix: string;
  scopes: Permission[];
  expires_at: string | null;
  last_used_at: string | null;
  revoked_at: string | null;
  created_at: string;
}

export interface CreateApiTokenRequest {
  name: string;
  scopes: Permission[];
  expires_in_days?: number;
}

export interface CreateApiTokenResponse {
  id: string;
  name: string;
  token_prefix: string;
  token: string;
  scopes: Permission[];
  expires_at: string | null;
  created_at: string;
}

export interface LlmProviderRecord {
  id: string;
  tenant_id: string;
  name: string;
  base_url: string;
  model: string;
  purposes: LlmPurpose[];
  has_api_key: boolean;
  priority: number;
  temperature: number;
  max_tokens: number;
  timeout_s: number;
  rpm_limit: number | null;
  supports_tools: boolean;
  embedding_dim: number | null;
  allow_private_network: boolean;
  allow_pii?: boolean;
  structured_mode?: StructuredOutputMode;
  deterministic_seed?: number | null;
  is_local_model?: boolean;
  max_context_tokens?: number;
  owner_user_id?: string | null;
  is_readonly?: boolean;
  enabled: boolean;
  created_at: string;
}

export interface SaveLlmProviderRequest {
  name: string;
  scope?: ProviderScope;
  base_url: string;
  model: string;
  purposes: LlmPurpose[];
  api_key?: string;
  priority?: number;
  temperature?: number;
  max_tokens?: number;
  max_context_tokens?: number;
  timeout_s?: number;
  rpm_limit?: number | null;
  supports_tools?: boolean;
  embedding_dim?: number | null;
  allow_private_network?: boolean;
  allow_pii?: boolean;
  structured_mode?: StructuredOutputMode;
  deterministic_seed?: number | null;
  is_local_model?: boolean;
  enabled?: boolean;
}

export interface ProviderTestResponse {
  ok: boolean;
  latency_ms: number;
  model: string;
  reply: string;
  structured_mode?: StructuredOutputMode;
  structured_verified?: boolean;
  effective_temperature?: number;
}

export interface UserLlmPreferenceRecord {
  purpose: LlmPurpose;
  provider_id: string;
  provider_name: string;
  updated_at: string;
}

export interface UserLlmPreferenceItem {
  purpose: LlmPurpose;
  provider_id: string;
}

export interface PutUserLlmPreferencesRequest {
  preferences: UserLlmPreferenceItem[];
}

export interface LlmUsageRecord {
  provider_id: string;
  provider_name: string;
  usage_date: string;
  purpose: LlmPurpose;
  requests: number;
  errors: number;
  prompt_tokens: number;
  completion_tokens: number;
}

export interface EndpointSpec {
  id: string;
  tenant_id: string;
  slug: string;
  environment: Environment;
  kind: EndpointKind;
  target_id: string;
  target_version: number | null;
  target_snapshot: Record<string, unknown> | null;
  title: string;
  description: string;
  input_schema: Record<string, unknown>;
  output_shape: OutputShape;
  required_scope: Permission;
  rate_limit_rpm: number;
  cache_ttl_s: number;
  expose_mcp: boolean;
  show_as_app: boolean;
  enabled: boolean;
  published_by: string | null;
  published_at: string;
}

export interface CreateEndpointRequest {
  slug: string;
  environment?: Environment;
  kind: EndpointKind;
  target_id: string;
  title: string;
  description?: string;
  input_schema?: Record<string, unknown> | null;
  output_shape?: OutputShape;
  required_scope?: Permission;
  rate_limit_rpm?: number;
  cache_ttl_s?: number;
  expose_mcp?: boolean;
  show_as_app?: boolean;
  enabled?: boolean;
}

export interface UpdateEndpointRequest {
  title?: string;
  description?: string;
  input_schema?: Record<string, unknown>;
  required_scope?: Permission;
  rate_limit_rpm?: number;
  cache_ttl_s?: number;
  expose_mcp?: boolean;
  show_as_app?: boolean;
  enabled?: boolean;
}

export interface SkillManifest {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  invoke: { method: string; url: string };
  auth: string;
}

export interface McpToolInfo {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

export interface McpClientRecord {
  id: string;
  tenant_id: string;
  name: string;
  url: string;
  has_headers: boolean;
  allowed_tools: string[];
  tools_cache: McpToolInfo[];
  allow_private_network: boolean;
  enabled: boolean;
  last_synced_at: string | null;
  created_at: string;
}

export interface CreateMcpClientRequest {
  name: string;
  url: string;
  headers?: Record<string, string>;
  allowed_tools?: string[];
  allow_private_network?: boolean;
  enabled?: boolean;
}

export type ToolRef =
  | { kind: "BUILTIN"; name: string }
  | { kind: "ENDPOINT"; endpoint_id: string }
  | { kind: "MCP"; client: string; tool: string }
  | { kind: "AGENT"; agent_id: string; as_tool_name: string };

export interface KnowledgeScope {
  doc_title_prefix: string | null;
  dataset_names: string[];
}

export interface AgentSpec {
  system_prompt: string;
  provider_purpose: LlmPurpose;
  provider_id: string | null;
  temperature: number;
  max_steps: number;
  memory_turns: number;
  tools: ToolRef[];
  approval_required_tools: string[];
  knowledge_scope: KnowledgeScope;
  output_schema: Record<string, unknown> | null;
}

export interface AgentRecord {
  id: string;
  tenant_id: string;
  name: string;
  description: string;
  spec: AgentSpec;
  version: number;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface AgentVersionRecord {
  tenant_id: string;
  agent_id: string;
  version: number;
  spec: AgentSpec;
  published_by: string | null;
  published_at: string;
}

export interface CreateAgentRequest {
  name: string;
  description?: string;
  spec: AgentSpec;
}

export interface UpdateAgentRequest {
  name?: string;
  description?: string;
  spec: AgentSpec;
}

export interface ConversationRecord {
  id: string;
  tenant_id: string;
  agent_id: string;
  agent_version: number;
  user_id: string | null;
  channel: ConversationChannel;
  embed_id: string | null;
  title: string;
  status: ConversationStatus;
  created_at: string;
  updated_at: string;
}

export interface ToolCallSpec {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export interface MessageRecord {
  id: number;
  tenant_id: string;
  conversation_id: string;
  role: MessageRole;
  content: string;
  tool_calls?: ToolCallSpec[];
  tool_call_id?: string;
  agent_name?: string;
  provider_id?: string;
  prompt_tokens?: number;
  completion_tokens?: number;
  latency_ms?: number;
  created_at: string;
}

export interface AgentChatRequest {
  message: string;
  conversation_id?: string;
  draft?: boolean;
}

export interface AgentChatResponse {
  conversation_id: string;
  status: ConversationStatus;
  answer: string;
  structured?: Record<string, unknown>;
  approval_id?: string;
  messages: MessageRecord[];
}

export type AgentEvent =
  | { type: "conversation_started"; conversation_id: string }
  | { type: "agent_switched"; agent_name: string; from_agent?: string }
  | { type: "token"; delta: string; agent_name?: string }
  | {
      type: "tool_call";
      id: string;
      name: string;
      arguments: Record<string, unknown>;
      agent_name?: string;
    }
  | {
      type: "tool_result";
      tool_call_id: string;
      name: string;
      content: string;
      truncated: boolean;
      agent_name?: string;
    }
  | {
      type: "approval_required";
      approval_id: string;
      conversation_id: string;
      tool_call_id: string;
      tool_name: string;
      arguments: Record<string, unknown>;
    }
  | {
      type: "final";
      conversation_id: string;
      answer: string;
      structured?: Record<string, unknown>;
    }
  | {
      type: "done";
      conversation_id?: string;
      answer?: string;
      structured?: Record<string, unknown>;
    }
  | { type: "error"; error_code: string; message: string };

const JsonRecordSchema = z.record(z.string(), z.unknown());

export const AgentEventSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("conversation_started"),
    conversation_id: z.string(),
  }),
  z.object({
    type: z.literal("agent_switched"),
    agent_name: z.string(),
    from_agent: z.string().optional(),
  }),
  z.object({
    type: z.literal("token"),
    delta: z.string(),
    agent_name: z.string().optional(),
  }),
  z.object({
    type: z.literal("tool_call"),
    id: z.string(),
    name: z.string(),
    arguments: JsonRecordSchema.default({}),
    agent_name: z.string().optional(),
  }),
  z.object({
    type: z.literal("tool_result"),
    tool_call_id: z.string(),
    name: z.string().default("tool"),
    content: z.string(),
    truncated: z.boolean().default(false),
    agent_name: z.string().optional(),
  }),
  z.object({
    type: z.literal("approval_required"),
    approval_id: z.string(),
    conversation_id: z.string(),
    tool_call_id: z.string(),
    tool_name: z.string(),
    arguments: JsonRecordSchema.default({}),
  }),
  z.object({
    type: z.literal("final"),
    conversation_id: z.string(),
    answer: z.string(),
    structured: JsonRecordSchema.optional(),
  }),
  z.object({
    type: z.literal("done"),
    conversation_id: z.string().optional(),
    answer: z.string().optional(),
    structured: JsonRecordSchema.optional(),
  }),
  z.object({
    type: z.literal("error"),
    error_code: z.string().default("AGENT_ERROR"),
    message: z.string(),
  }),
]);

export interface AgentEmbedRecord {
  id: string;
  tenant_id: string;
  agent_id: string;
  allowed_origins: string[];
  permissions: Permission[];
  rate_limit_rpm: number;
  expires_at: string | null;
  revoked_at: string | null;
  created_by: string;
  created_at: string;
}

export interface CreateAgentEmbedRequest {
  allowed_origins: string[];
  permissions: Permission[];
  expires_at?: string | null;
  rate_limit_rpm?: number;
}

export interface CreateAgentEmbedResponse {
  id: string;
  agent_id: string;
  token: string;
  url: string;
  allowed_origins: string[];
  permissions: Permission[];
  rate_limit_rpm: number;
  expires_at: string | null;
  created_at: string;
}

export interface PaginatedItems<T> {
  items: T[];
  next_cursor?: string | null;
}

function unwrapItems<T>(
  res: T[] | PaginatedItems<T> | Record<string, unknown> | null | undefined
): T[] {
  if (Array.isArray(res)) {
    return res;
  }
  if (
    res !== null &&
    res !== undefined &&
    typeof res === "object" &&
    "items" in res &&
    Array.isArray(res.items)
  ) {
    return res.items as T[];
  }
  return [];
}

export async function listApiTokens(): Promise<PersonalAccessTokenSummary[]> {
  const res = await requestJson<
    PersonalAccessTokenSummary[] | PaginatedItems<PersonalAccessTokenSummary>
  >("/api/auth/tokens");
  return unwrapItems(res);
}

export async function createApiToken(
  req: CreateApiTokenRequest
): Promise<CreateApiTokenResponse> {
  return requestJson<CreateApiTokenResponse>("/api/auth/tokens", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}

export async function revokeApiToken(id: string): Promise<void> {
  await requestJson<unknown>(`/api/auth/tokens/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

export async function listLlmProviders(): Promise<LlmProviderRecord[]> {
  const res = await requestJson<
    LlmProviderRecord[] | PaginatedItems<LlmProviderRecord>
  >("/api/axagent/providers");
  return unwrapItems(res);
}

export async function createLlmProvider(
  req: SaveLlmProviderRequest
): Promise<LlmProviderRecord> {
  return requestJson<LlmProviderRecord>("/api/axagent/providers", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}

export async function updateLlmProvider(
  id: string,
  req: SaveLlmProviderRequest
): Promise<LlmProviderRecord> {
  return requestJson<LlmProviderRecord>(
    `/api/axagent/providers/${encodeURIComponent(id)}`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }
  );
}

export async function deleteLlmProvider(id: string): Promise<void> {
  await requestJson<unknown>(
    `/api/axagent/providers/${encodeURIComponent(id)}`,
    { method: "DELETE" }
  );
}

export async function testLlmProvider(
  id: string
): Promise<ProviderTestResponse> {
  return requestJson<ProviderTestResponse>(
    `/api/axagent/providers/${encodeURIComponent(id)}/test`,
    { method: "POST" }
  );
}

export async function getUserLlmPreferences(): Promise<
  UserLlmPreferenceRecord[]
> {
  const res = await requestJson<
    UserLlmPreferenceRecord[] | PaginatedItems<UserLlmPreferenceRecord>
  >("/api/axagent/llm-preferences");
  return unwrapItems(res);
}

export async function putUserLlmPreferences(
  req: PutUserLlmPreferencesRequest
): Promise<UserLlmPreferenceRecord[]> {
  const res = await requestJson<
    UserLlmPreferenceRecord[] | PaginatedItems<UserLlmPreferenceRecord>
  >("/api/axagent/llm-preferences", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
  return unwrapItems(res);
}

export async function fetchLlmUsage(params?: {
  from?: string;
  to?: string;
}): Promise<LlmUsageRecord[]> {
  const qs = new URLSearchParams();
  if (params?.from) qs.set("from", params.from);
  if (params?.to) qs.set("to", params.to);
  const query = qs.toString();
  const res = await requestJson<
    LlmUsageRecord[] | PaginatedItems<LlmUsageRecord>
  >(`/api/axagent/usage${query ? `?${query}` : ""}`);
  return unwrapItems(res);
}

export async function listEndpoints(params?: {
  environment?: Environment;
  kind?: EndpointKind;
  show_as_app?: boolean;
}): Promise<EndpointSpec[]> {
  const qs = new URLSearchParams();
  if (params?.environment) qs.set("environment", params.environment);
  if (params?.kind) qs.set("kind", params.kind);
  if (params?.show_as_app !== undefined) {
    qs.set("show_as_app", String(params.show_as_app));
  }
  const query = qs.toString();
  const res = await requestJson<EndpointSpec[] | PaginatedItems<EndpointSpec>>(
    `/api/axagent/endpoints${query ? `?${query}` : ""}`
  );
  return unwrapItems(res);
}

export async function createEndpoint(
  req: CreateEndpointRequest
): Promise<EndpointSpec> {
  return requestJson<EndpointSpec>("/api/axagent/endpoints", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}

export async function updateEndpoint(
  id: string,
  req: UpdateEndpointRequest
): Promise<EndpointSpec> {
  return requestJson<EndpointSpec>(
    `/api/axagent/endpoints/${encodeURIComponent(id)}`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }
  );
}

export async function deleteEndpoint(id: string): Promise<void> {
  await requestJson<unknown>(
    `/api/axagent/endpoints/${encodeURIComponent(id)}`,
    { method: "DELETE" }
  );
}

export async function promoteEndpoint(id: string): Promise<EndpointSpec> {
  return requestJson<EndpointSpec>(
    `/api/axagent/endpoints/${encodeURIComponent(id)}/promote`,
    { method: "POST" }
  );
}

export async function fetchEndpointsOpenApi(
  environment: Environment = "PROD"
): Promise<Record<string, unknown>> {
  return requestJson<Record<string, unknown>>(
    `/api/axagent/endpoints/openapi.json?environment=${encodeURIComponent(environment)}`
  );
}

export const fetchPublishedOpenApi = fetchEndpointsOpenApi;

export async function fetchEndpointSkill(id: string): Promise<SkillManifest> {
  return requestJson<SkillManifest>(
    `/api/axagent/endpoints/${encodeURIComponent(id)}/skill.json`
  );
}

export const fetchEndpointSkillManifest = fetchEndpointSkill;

export async function runPublishedApp(
  endpointId: string,
  payload: Record<string, unknown>
): Promise<Record<string, unknown>> {
  return requestJson<Record<string, unknown>>(
    `/api/axagent/apps/${encodeURIComponent(endpointId)}/run`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }
  );
}

async function parseIsolatedJsonResponse<T>(resp: Response): Promise<T> {
  if (!resp.ok) {
    const raw = await resp.text();
    try {
      const parsed = ApiErrorBodySchema.safeParse(JSON.parse(raw));
      if (parsed.success) {
        throw new ApiError({
          status: resp.status,
          errorCode: parsed.data.error_code,
          message: parsed.data.message,
          traceId: parsed.data.trace_id,
        });
      }
    } catch (err) {
      if (err instanceof ApiError) throw err;
    }
    throw new ApiError({
      status: resp.status,
      errorCode: `HTTP_${resp.status}`,
      message: raw || resp.statusText,
      traceId: "",
    });
  }
  return (await resp.json()) as T;
}

/**
 * Invokes a published capability either via external `/api/published/{env}/{tenant}/{slug}`
 * when a `Bearer axp_...` PAT is supplied, or via internal `/api/axagent/apps/{id}/run`
 * using the active authenticated workbench session.
 */
export async function invokePublishedEndpoint(
  endpoint: Pick<EndpointSpec, "id" | "environment" | "tenant_id" | "slug">,
  payload: Record<string, unknown>,
  options?: { patToken?: string | null }
): Promise<Record<string, unknown>> {
  const pat = options?.patToken?.trim() ?? "";
  if (pat.startsWith("axp_")) {
    const envPath = endpoint.environment.toLowerCase();
    const resp = await fetch(
      `/api/published/${encodeURIComponent(envPath)}/${encodeURIComponent(endpoint.tenant_id)}/${encodeURIComponent(endpoint.slug)}`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${pat}`,
        },
        body: JSON.stringify(payload),
      }
    );
    return parseIsolatedJsonResponse<Record<string, unknown>>(resp);
  }
  return runPublishedApp(endpoint.id, payload);
}

export async function getPublishedRunStatus(
  endpoint: Pick<EndpointSpec, "environment" | "tenant_id" | "slug">,
  runId: string,
  patToken: string
): Promise<Record<string, unknown>> {
  const envPath = endpoint.environment.toLowerCase();
  const resp = await fetch(
    `/api/published/${encodeURIComponent(envPath)}/${encodeURIComponent(endpoint.tenant_id)}/${encodeURIComponent(endpoint.slug)}/runs/${encodeURIComponent(runId)}`,
    {
      method: "GET",
      headers: {
        Authorization: `Bearer ${patToken.trim()}`,
      },
    }
  );
  return parseIsolatedJsonResponse<Record<string, unknown>>(resp);
}

export async function listMcpClients(): Promise<McpClientRecord[]> {
  const res = await requestJson<
    McpClientRecord[] | PaginatedItems<McpClientRecord>
  >("/api/axagent/mcp-clients");
  return unwrapItems(res);
}

export async function createMcpClient(
  req: CreateMcpClientRequest
): Promise<McpClientRecord> {
  return requestJson<McpClientRecord>("/api/axagent/mcp-clients", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}

export async function deleteMcpClient(id: string): Promise<void> {
  await requestJson<unknown>(
    `/api/axagent/mcp-clients/${encodeURIComponent(id)}`,
    { method: "DELETE" }
  );
}

export async function syncMcpClient(id: string): Promise<McpClientRecord> {
  return requestJson<McpClientRecord>(
    `/api/axagent/mcp-clients/${encodeURIComponent(id)}/sync`,
    { method: "POST" }
  );
}

export async function listAgents(): Promise<AgentRecord[]> {
  const res = await requestJson<AgentRecord[] | PaginatedItems<AgentRecord>>(
    "/api/axagent/agents"
  );
  return unwrapItems(res);
}

export async function getAgent(id: string): Promise<AgentRecord> {
  return requestJson<AgentRecord>(
    `/api/axagent/agents/${encodeURIComponent(id)}`
  );
}

export async function createAgent(
  req: CreateAgentRequest
): Promise<AgentRecord> {
  return requestJson<AgentRecord>("/api/axagent/agents", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}

export async function updateAgent(
  id: string,
  req: UpdateAgentRequest
): Promise<AgentRecord> {
  return requestJson<AgentRecord>(
    `/api/axagent/agents/${encodeURIComponent(id)}`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }
  );
}

export async function deleteAgent(id: string): Promise<void> {
  await requestJson<unknown>(
    `/api/axagent/agents/${encodeURIComponent(id)}`,
    { method: "DELETE" }
  );
}

export async function publishAgent(id: string): Promise<AgentVersionRecord> {
  return requestJson<AgentVersionRecord>(
    `/api/axagent/agents/${encodeURIComponent(id)}/publish`,
    { method: "POST" }
  );
}

export async function listAgentVersions(
  agentId: string
): Promise<AgentVersionRecord[]> {
  try {
    const res = await requestJson<
      AgentVersionRecord[] | PaginatedItems<AgentVersionRecord>
    >(`/api/axagent/agents/${encodeURIComponent(agentId)}/versions`);
    return unwrapItems(res);
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) {
      return [];
    }
    throw err;
  }
}

export async function chatAgentJson(
  agentId: string,
  req: AgentChatRequest
): Promise<AgentChatResponse> {
  return requestJson<AgentChatResponse>(
    `/api/axagent/agents/${encodeURIComponent(agentId)}/chat`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      body: JSON.stringify(req),
    }
  );
}

export const chatWithAgent = chatAgentJson;

function dispatchParsedSseFrame(
  frame: string,
  onEvent: (event: AgentEvent) => void
): void {
  let eventName = "";
  const dataLines: string[] = [];
  for (const rawLine of frame.split("\n")) {
    const line = rawLine.replace(/\r$/, "");
    if (line.startsWith("event:")) {
      eventName = line.slice(6).trim();
    } else if (line.startsWith("data:")) {
      dataLines.push(line.slice(5).trimStart());
    }
  }
  if (dataLines.length === 0) return;
  const payloadText = dataLines.join("\n").trim();
  if (payloadText.length === 0 || payloadText === "[DONE]") {
    if (eventName === "done") {
      onEvent({ type: "done" });
    }
    return;
  }
  try {
    const parsedRaw: unknown = JSON.parse(payloadText);
    if (
      typeof parsedRaw === "object" &&
      parsedRaw !== null &&
      !Array.isArray(parsedRaw)
    ) {
      const record = parsedRaw as Record<string, unknown>;
      const effectiveType =
        typeof record.type === "string" && record.type.length > 0
          ? record.type
          : eventName;
      const candidate: Record<string, unknown> = {
        ...record,
        type: effectiveType,
      };
      if (
        effectiveType === "tool_call" &&
        (typeof candidate.arguments !== "object" ||
          candidate.arguments === null ||
          Array.isArray(candidate.arguments))
      ) {
        if (typeof candidate.arguments === "string") {
          try {
            const parsedArgs: unknown = JSON.parse(candidate.arguments);
            if (
              typeof parsedArgs === "object" &&
              parsedArgs !== null &&
              !Array.isArray(parsedArgs)
            ) {
              candidate.arguments = parsedArgs;
            } else {
              candidate.arguments = { value: parsedArgs };
            }
          } catch {
            candidate.arguments = { raw: candidate.arguments };
          }
        } else {
          candidate.arguments = {};
        }
      }
      if (
        effectiveType === "tool_result" &&
        typeof candidate.content !== "string" &&
        candidate.content !== undefined
      ) {
        candidate.content = JSON.stringify(candidate.content);
      }
      if (
        effectiveType === "tool_result" &&
        typeof candidate.tool_call_id !== "string" &&
        typeof candidate.id === "string"
      ) {
        candidate.tool_call_id = candidate.id;
      }
      const validated = AgentEventSchema.safeParse(candidate);
      if (validated.success) {
        onEvent(validated.data);
      }
    }
  } catch {
    // Ignore malformed comment/heartbeat frames.
  }
}

async function consumeAgentStreamResponse(
  resp: Response,
  onEvent: (event: AgentEvent) => void
): Promise<void> {
  if (!resp.ok) {
    const raw = await resp.text();
    let parsedErr: ApiError | null = null;
    try {
      const check = ApiErrorBodySchema.safeParse(JSON.parse(raw));
      if (check.success) {
        parsedErr = new ApiError({
          status: resp.status,
          errorCode: check.data.error_code,
          message: check.data.message,
          traceId: check.data.trace_id,
        });
      }
    } catch {
      // Fallback below.
    }
    throw (
      parsedErr ??
      new ApiError({
        status: resp.status,
        errorCode: `HTTP_${resp.status}`,
        message: raw || resp.statusText,
        traceId: "",
      })
    );
  }
  const contentType = resp.headers.get("content-type") ?? "";
  if (contentType.includes("application/json")) {
    const jsonResp = (await resp.json()) as AgentChatResponse;
    if (jsonResp.conversation_id) {
      onEvent({
        type: "conversation_started",
        conversation_id: jsonResp.conversation_id,
      });
    }
    const messages = Array.isArray(jsonResp.messages) ? jsonResp.messages : [];
    const callNameById = new Map<string, string>();
    for (const msg of messages) {
      if (msg.tool_calls) {
        for (const tc of msg.tool_calls) {
          callNameById.set(tc.id, tc.name);
          onEvent({
            type: "tool_call",
            id: tc.id,
            name: tc.name,
            arguments: tc.arguments,
            agent_name: msg.agent_name,
          });
        }
      } else if (msg.role === "TOOL" && msg.tool_call_id) {
        onEvent({
          type: "tool_result",
          tool_call_id: msg.tool_call_id,
          name: callNameById.get(msg.tool_call_id) ?? msg.agent_name ?? "tool",
          content: msg.content,
          truncated: false,
          agent_name: msg.agent_name,
        });
      }
    }
    if (
      jsonResp.status === "WAITING_APPROVAL" &&
      typeof jsonResp.approval_id === "string"
    ) {
      onEvent({
        type: "approval_required",
        approval_id: jsonResp.approval_id,
        conversation_id: jsonResp.conversation_id,
        tool_call_id: "",
        tool_name: "approval_gate",
        arguments: {},
      });
      return;
    }
    onEvent({
      type: "final",
      conversation_id: jsonResp.conversation_id,
      answer: jsonResp.answer,
      structured: jsonResp.structured,
    });
    return;
  }
  if (!resp.body) {
    return;
  }
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) {
      buffer += decoder.decode();
      const normalizedRest = buffer.replace(/\r\n/g, "\n").trim();
      if (normalizedRest.length > 0) {
        for (const trailingFrame of normalizedRest.split("\n\n")) {
          dispatchParsedSseFrame(trailingFrame, onEvent);
        }
      }
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    buffer = buffer.replace(/\r\n/g, "\n");
    const frames = buffer.split("\n\n");
    buffer = frames.pop() ?? "";
    for (const frame of frames) {
      dispatchParsedSseFrame(frame, onEvent);
    }
  }
}

export async function streamAgentChat(
  agentId: string,
  req: AgentChatRequest,
  onEvent: (event: AgentEvent) => void,
  signal?: AbortSignal
): Promise<void> {
  const headers = new Headers({
    "Content-Type": "application/json",
    Accept: "text/event-stream, application/json",
  });
  const token = getAccessToken();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  const resp = await fetch(
    `/api/axagent/agents/${encodeURIComponent(agentId)}/chat`,
    {
      method: "POST",
      headers,
      body: JSON.stringify(req),
      signal,
    }
  );
  await consumeAgentStreamResponse(resp, onEvent);
}

export async function listConversations(params?: {
  agent_id?: string;
  limit?: number;
}): Promise<ConversationRecord[]> {
  const qs = new URLSearchParams();
  if (params?.agent_id) qs.set("agent_id", params.agent_id);
  if (params?.limit) qs.set("limit", String(params.limit));
  const query = qs.toString();
  const res = await requestJson<
    ConversationRecord[] | PaginatedItems<ConversationRecord>
  >(`/api/axagent/conversations${query ? `?${query}` : ""}`);
  return unwrapItems(res);
}

export async function listConversationMessages(
  conversationId: string
): Promise<MessageRecord[]> {
  const res = await requestJson<MessageRecord[] | PaginatedItems<MessageRecord>>(
    `/api/axagent/conversations/${encodeURIComponent(conversationId)}/messages`
  );
  return unwrapItems(res);
}

export const getConversationMessages = listConversationMessages;

export async function deleteConversation(
  conversationId: string
): Promise<void> {
  await requestJson<unknown>(
    `/api/axagent/conversations/${encodeURIComponent(conversationId)}`,
    { method: "DELETE" }
  );
}

export async function listAgentEmbeds(
  agentId: string
): Promise<AgentEmbedRecord[]> {
  const res = await requestJson<
    AgentEmbedRecord[] | PaginatedItems<AgentEmbedRecord>
  >(`/api/axagent/agents/${encodeURIComponent(agentId)}/embeds`);
  return unwrapItems(res);
}

export async function createAgentEmbed(
  agentId: string,
  req: CreateAgentEmbedRequest
): Promise<CreateAgentEmbedResponse> {
  return requestJson<CreateAgentEmbedResponse>(
    `/api/axagent/agents/${encodeURIComponent(agentId)}/embeds`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }
  );
}

export async function revokeAgentEmbed(embedId: string): Promise<void> {
  await requestJson<unknown>(
    `/api/axagent/embeds/${encodeURIComponent(embedId)}`,
    { method: "DELETE" }
  );
}

/** Unauthenticated public embed chat request (`POST /api/axagent/embed/{token}/chat`, never redirects to `/login`). */
export async function postEmbedChat(
  token: string,
  req: { message: string; conversation_id?: string }
): Promise<AgentChatResponse> {
  const resp = await fetch(
    `/api/axagent/embed/${encodeURIComponent(token)}/chat`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      body: JSON.stringify(req),
    }
  );
  return parseIsolatedJsonResponse<AgentChatResponse>(resp);
}

/**
 * Unauthenticated streaming embed chat (`POST /api/axagent/embed/{token}/chat`),
 * supporting both SSE (`text/event-stream`) and synchronous JSON (`application/json`) responses.
 */
export async function streamEmbedChat(
  token: string,
  req: { message: string; conversation_id?: string },
  onEvent: (event: AgentEvent) => void,
  signal?: AbortSignal
): Promise<void> {
  const resp = await fetch(
    `/api/axagent/embed/${encodeURIComponent(token)}/chat`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "text/event-stream, application/json",
      },
      body: JSON.stringify(req),
      signal,
    }
  );
  await consumeAgentStreamResponse(resp, onEvent);
}
