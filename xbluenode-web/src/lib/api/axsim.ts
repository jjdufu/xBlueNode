import { z } from "zod";
import { requestJson } from "./client";
import type { InterventionMode } from "./enums";

export type SandboxStatus = "CLONING" | "READY" | "ARCHIVED";
export type GraphDirection = "OUT" | "IN" | "BOTH";
export type SimMode = "STEADY" | "STEPPED";
export type ScenarioStatus = "PENDING" | "RUNNING" | "SUCCEEDED" | "FAILED";
export type DecisionStatus =
  | "PENDING_APPROVAL"
  | "APPROVED"
  | "APPLIED"
  | "REJECTED"
  | "CONFLICT"
  | "FAILED";
export type SimAskIntent = "WHAT_IF" | "QUERY_RESULT";

export interface SandboxScope {
  root_node_ids?: string[];
  depth?: number;
  direction?: GraphDirection;
  type_names?: string[];
  link_types?: string[];
  max_nodes?: number;
}

export interface UncertaintyConfig {
  samples: number;
  coefficient_cv: number;
  seed: number;
}

export interface SimParams {
  mode?: SimMode;
  damping?: number;
  tolerance?: number;
  max_iterations?: number;
  min_effect?: number;
  max_hops?: number | null;
  horizon_steps?: number;
  uncertainty?: UncertaintyConfig | null;
  checkpoint_every?: number;
}

export interface AxSimAction {
  target_node_id: string;
  metric_key: string;
  mode?: InterventionMode | null;
  value?: number | null;
  delta_ratio?: number | null;
  at_step?: number;
}

export interface SimWarning {
  node_id: string;
  code: string;
  message: string;
}

export interface StepSeriesPoint {
  step: number;
  value: number;
}

export interface ImpactRow {
  node_id: string;
  type_name: string;
  title: string;
  metric: string;
  before_val: number | null;
  after_val: number;
  delta: number | null;
  delta_ratio: number | null;
  p10?: number | null;
  p90?: number | null;
  hop_distance: number;
  top_path: string[];
  series?: StepSeriesPoint[];
}

export interface SimKpiRow {
  type_name: string;
  metric: string;
  before_sum: number;
  after_sum: number;
  delta: number;
  delta_ratio: number | null;
  affected_nodes: number;
}

export interface AxSimImpactRowCompat {
  node_id: string;
  type_name: string;
  title: string;
  metric_key: string;
  before_val: number;
  after_val: number;
  delta: number;
  delta_ratio?: number | null;
  hop_distance: number;
  top_path?: string[];
}

export interface AxSimQuickResult {
  impacts: AxSimImpactRowCompat[];
  affected_count: number;
  kpis?: SimKpiRow[];
  warnings?: SimWarning[];
  converged?: boolean;
}

export type AxSimImpactRow = AxSimImpactRowCompat;
export type AxSimResult = AxSimQuickResult;

export interface AxSandboxRecord {
  id: string;
  name: string;
  status?: SandboxStatus;
  scope?: SandboxScope;
  node_count: number;
  link_count: number;
  base_captured_at?: string | null;
  schema_version?: number | null;
  created_by?: string | null;
  actions?: AxSimAction[];
  sim_result: AxSimQuickResult | null;
  created_at: string;
  updated_at?: string;
}

export interface CreateSandboxRequest {
  name: string;
  scope?: SandboxScope;
}

export interface SandboxGraphNode {
  id: string;
  type_name: string;
  node_key: string;
  title: string;
  metrics: Record<string, number>;
  properties: Record<string, unknown>;
  is_modified: boolean;
  is_new: boolean;
}

export interface SandboxGraphLink {
  id: string;
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight: number;
  is_modified: boolean;
  is_new: boolean;
}

export interface SandboxGraphResponse {
  sandbox_id: string;
  nodes: SandboxGraphNode[];
  links: SandboxGraphLink[];
  overlay_hash: string;
}

export interface PatchOverlayNodeRequest {
  title?: string;
  metrics?: Record<string, number>;
  properties?: Record<string, unknown>;
}

export interface CreateOverlayNodeRequest {
  type_name: string;
  node_key: string;
  title: string;
  metrics?: Record<string, number>;
  properties?: Record<string, unknown>;
}

export interface CreateOverlayLinkRequest {
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight?: number;
}

export interface PatchOverlayLinkRequest {
  link_type?: string;
  weight?: number;
}

export interface RefreshBaseResponse {
  sandbox: AxSandboxRecord;
  orphaned_node_ids: string[];
  orphaned_link_ids: string[];
}

export interface ScenarioSummary {
  affected_count: number;
  kpis: SimKpiRow[];
  top_impacts: ImpactRow[];
  warnings: SimWarning[];
  converged: boolean;
  reason?: string | null;
  iterations: number;
}

export interface ScenarioRecord {
  id: string;
  sandbox_id: string;
  name: string;
  description: string;
  actions: AxSimAction[];
  params: SimParams;
  overlay_hash: string;
  is_stale: boolean;
  result: ScenarioSummary | null;
  status: ScenarioStatus;
  error_message: string | null;
  created_by: string | null;
  created_at: string;
  finished_at: string | null;
}

export interface CreateScenarioRequest {
  name: string;
  description?: string;
  actions: AxSimAction[];
  params?: SimParams;
}

export interface ImpactsPage {
  items: ImpactRow[];
  next_cursor: string | null;
}

export interface Checkpoint {
  seq: number;
  label: string;
  changes: Record<string, Record<string, number>>;
}

export interface CheckpointsResponse {
  items: Checkpoint[];
}

export interface CompareScenarioColumn {
  id: string;
  name: string;
  status: ScenarioStatus;
  is_stale: boolean;
}

export interface CompareKpiCell {
  after_sum: number;
  delta: number;
  delta_ratio: number | null;
  affected_nodes: number;
}

export interface CompareKpiRow {
  type_name: string;
  metric: string;
  before_sum: number;
  values: Record<string, CompareKpiCell>;
}

export interface CompareNodeCell {
  after_val: number;
  delta_ratio: number | null;
  delta: number | null;
}

export interface CompareNodeRow {
  node_id: string;
  type_name: string;
  title: string;
  metric: string;
  before_val: number | null;
  values: Record<string, CompareNodeCell>;
  max_spread: number;
}

export interface CompareBriefing {
  summary: string;
  recommended_scenario_id: string | null;
  reasons: string[];
  risks: string[];
  error?: string | null;
}

export interface CompareScenariosResponse {
  sandbox_id: string;
  scenarios: CompareScenarioColumn[];
  kpis: CompareKpiRow[];
  nodes: CompareNodeRow[];
  briefing?: CompareBriefing | null;
}

export interface SimAskRequest {
  question: string;
  sandbox_id: string;
  scenario_id?: string | null;
}

export interface SimAskCandidate {
  node_id: string;
  type_name: string;
  node_key: string;
  title: string;
}

export interface SimAskResponse {
  intent: SimAskIntent;
  scenario_id?: string | null;
  answer: string;
  sql?: string | null;
  columns: Array<{ name: string; data_type: string }>;
  rows: Array<Record<string, unknown>>;
  need_clarification: boolean;
  candidates: SimAskCandidate[];
  impacts: ImpactRow[];
  kpis: SimKpiRow[];
}

export interface DecisionChangeInput {
  node_id: string;
  metric: string;
  target_value: number;
}

export interface DecisionChangeItem {
  node_id: string;
  metric: string;
  base_value: number | null;
  target_value: number;
}

export interface DecisionConflictItem {
  node_id: string;
  metric: string;
  base_value: number | null;
  current_value: number | null;
  target_value: number;
  drift_ratio: number;
  reason: string;
}

export interface CreateDecisionRequest {
  title: string;
  changes?: DecisionChangeInput[];
  tolerance_ratio?: number;
  trigger_flow_id?: string | null;
}

export interface DecisionRecord {
  id: string;
  scenario_id: string;
  title: string;
  changes: DecisionChangeItem[];
  trigger_flow_id: string | null;
  status: DecisionStatus;
  approval_id: string | null;
  conflict_report: DecisionConflictItem[];
  tolerance_ratio: number;
  created_by: string | null;
  created_at: string;
  applied_at: string | null;
  applied_by: string | null;
}

export const ImpactRowSchema = z.object({
  node_id: z.string(),
  type_name: z.string(),
  title: z.string(),
  metric: z.string(),
  before_val: z.number().nullable(),
  after_val: z.number(),
  delta: z.number().nullable(),
  delta_ratio: z.number().nullable(),
  p10: z.number().nullable().optional(),
  p90: z.number().nullable().optional(),
  hop_distance: z.number(),
  top_path: z.array(z.string()).default([]),
  series: z
    .array(z.object({ step: z.number(), value: z.number() }))
    .optional(),
});

export const SandboxRecordSchema = z.object({
  id: z.string(),
  name: z.string(),
  status: z.enum(["CLONING", "READY", "ARCHIVED"]).optional(),
  node_count: z.number(),
  link_count: z.number(),
  base_captured_at: z.string().nullable().optional(),
  schema_version: z.number().nullable().optional(),
  created_by: z.string().nullable().optional(),
  sim_result: z.unknown().nullable(),
  created_at: z.string(),
  updated_at: z.string().optional(),
});

export const axsimApi = {
  listSandboxes: () => requestJson<AxSandboxRecord[]>("/api/axsim/sandboxes"),

  createSandbox: (req: CreateSandboxRequest) =>
    requestJson<AxSandboxRecord>("/api/axsim/sandboxes", {
      method: "POST",
      body: JSON.stringify(req),
    }),

  getSandbox: (sandboxId: string) =>
    requestJson<AxSandboxRecord>(`/api/axsim/sandboxes/${sandboxId}`),

  deleteSandbox: (sandboxId: string) =>
    requestJson<{ deleted: boolean }>(`/api/axsim/sandboxes/${sandboxId}`, {
      method: "DELETE",
    }),

  getSandboxGraph: (sandboxId: string) =>
    requestJson<SandboxGraphResponse>(`/api/axsim/sandboxes/${sandboxId}/graph`),

  patchOverlayNode: (
    sandboxId: string,
    nodeId: string,
    req: PatchOverlayNodeRequest
  ) =>
    requestJson<SandboxGraphNode>(
      `/api/axsim/sandboxes/${sandboxId}/nodes/${nodeId}`,
      {
        method: "PATCH",
        body: JSON.stringify(req),
      }
    ),

  createOverlayNode: (sandboxId: string, req: CreateOverlayNodeRequest) =>
    requestJson<SandboxGraphNode>(`/api/axsim/sandboxes/${sandboxId}/nodes`, {
      method: "POST",
      body: JSON.stringify(req),
    }),

  deleteOverlayNode: (sandboxId: string, nodeId: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axsim/sandboxes/${sandboxId}/nodes/${nodeId}`,
      { method: "DELETE" }
    ),

  createOverlayLink: (sandboxId: string, req: CreateOverlayLinkRequest) =>
    requestJson<SandboxGraphLink>(`/api/axsim/sandboxes/${sandboxId}/links`, {
      method: "POST",
      body: JSON.stringify(req),
    }),

  patchOverlayLink: (
    sandboxId: string,
    linkId: string,
    req: PatchOverlayLinkRequest
  ) =>
    requestJson<SandboxGraphLink>(
      `/api/axsim/sandboxes/${sandboxId}/links/${linkId}`,
      {
        method: "PATCH",
        body: JSON.stringify(req),
      }
    ),

  deleteOverlayLink: (sandboxId: string, linkId: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axsim/sandboxes/${sandboxId}/links/${linkId}`,
      { method: "DELETE" }
    ),

  resetOverlay: (sandboxId: string) =>
    requestJson<{ reset: boolean }>(`/api/axsim/sandboxes/${sandboxId}/reset`, {
      method: "POST",
    }),

  refreshBase: (sandboxId: string) =>
    requestJson<RefreshBaseResponse>(
      `/api/axsim/sandboxes/${sandboxId}/refresh-base`,
      { method: "POST" }
    ),

  quickSimulate: (
    sandboxId: string,
    actions: AxSimAction[],
    params?: SimParams
  ) =>
    requestJson<AxSimQuickResult>(
      `/api/axsim/sandboxes/${sandboxId}/simulate`,
      {
        method: "POST",
        body: JSON.stringify({ actions, params }),
      }
    ),

  listScenarios: (sandboxId: string) =>
    requestJson<ScenarioRecord[]>(
      `/api/axsim/sandboxes/${sandboxId}/scenarios`
    ),

  createScenario: (sandboxId: string, req: CreateScenarioRequest) =>
    requestJson<ScenarioRecord>(
      `/api/axsim/sandboxes/${sandboxId}/scenarios`,
      {
        method: "POST",
        body: JSON.stringify(req),
      }
    ),

  getScenario: (scenarioId: string) =>
    requestJson<ScenarioRecord>(`/api/axsim/scenarios/${scenarioId}`),

  rerunScenario: (scenarioId: string) =>
    requestJson<ScenarioRecord>(`/api/axsim/scenarios/${scenarioId}/rerun`, {
      method: "POST",
    }),

  deleteScenario: (scenarioId: string) =>
    requestJson<{ deleted: boolean }>(`/api/axsim/scenarios/${scenarioId}`, {
      method: "DELETE",
    }),

  listImpacts: (
    scenarioId: string,
    params?: {
      type_name?: string;
      metric?: string;
      limit?: number;
      cursor?: string;
    }
  ) => {
    const qs = new URLSearchParams();
    if (params?.type_name) qs.set("type_name", params.type_name);
    if (params?.metric) qs.set("metric", params.metric);
    if (params?.limit !== undefined) qs.set("limit", String(params.limit));
    if (params?.cursor) qs.set("cursor", params.cursor);
    const suffix = qs.toString() ? `?${qs.toString()}` : "";
    return requestJson<ImpactsPage>(
      `/api/axsim/scenarios/${scenarioId}/impacts${suffix}`
    );
  },

  listCheckpoints: (scenarioId: string) =>
    requestJson<CheckpointsResponse>(
      `/api/axsim/scenarios/${scenarioId}/checkpoints`
    ),

  compareScenarios: (params: {
    ids: string[];
    metrics?: string[];
    type_name?: string;
    briefing?: boolean;
  }) => {
    const qs = new URLSearchParams();
    qs.set("ids", params.ids.join(","));
    if (params.metrics && params.metrics.length > 0) {
      qs.set("metrics", params.metrics.join(","));
    }
    if (params.type_name) qs.set("type_name", params.type_name);
    if (params.briefing !== undefined) {
      qs.set("briefing", String(params.briefing));
    }
    return requestJson<CompareScenariosResponse>(
      `/api/axsim/scenarios/compare?${qs.toString()}`
    );
  },

  ask: (req: SimAskRequest) =>
    requestJson<SimAskResponse>("/api/axsim/ask", {
      method: "POST",
      body: JSON.stringify(req),
    }),

  createDecision: (scenarioId: string, req: CreateDecisionRequest) =>
    requestJson<DecisionRecord>(
      `/api/axsim/scenarios/${scenarioId}/decisions`,
      {
        method: "POST",
        body: JSON.stringify(req),
      }
    ),

  getDecision: (decisionId: string) =>
    requestJson<DecisionRecord>(`/api/axsim/decisions/${decisionId}`),

  rebaseDecision: (decisionId: string) =>
    requestJson<DecisionRecord>(`/api/axsim/decisions/${decisionId}/rebase`, {
      method: "POST",
    }),
};
