import { requestJson } from "./client";

export interface PropertyDef {
  name: string;
  data_type: string;
}

export interface AggRuleDef {
  target_metric: string;
  op: string;
}

export interface BlueNodeTypeDef {
  id: string;
  name: string;
  display_name: string;
  properties: PropertyDef[];
  agg_rules: AggRuleDef[];
  pos_x: number;
  pos_y: number;
}

export interface BlueEdgeTypeDef {
  id: string;
  name: string;
  src_type_name: string;
  dst_type_name: string;
  impact_weight: number;
}

export interface XBlueSchema {
  node_types: BlueNodeTypeDef[];
  edge_types: BlueEdgeTypeDef[];
}

export interface BlueNode {
  id: string;
  type_name: string;
  node_key: string;
  title: string;
  metrics: Record<string, number>;
}

export interface BlueLink {
  id: string;
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight: number;
}

export interface AxSimSnapshot {
  nodes: BlueNode[];
  links: BlueLink[];
}

export interface AxFlowStep {
  id: string;
  kind: string;
  label: string;
  config: Record<string, unknown>;
}

export interface AxFlowDef {
  id: string;
  name: string;
  description: string;
  steps: AxFlowStep[];
  status: string;
  last_run_at: string | null;
}

export interface AxFlowRunResult {
  flow_id: string;
  status: string;
  steps_executed: number;
  logs: string[];
}

export interface AxSimImpactRow {
  node_id: string;
  type_name: string;
  title: string;
  metric_key: string;
  before_val: number;
  after_val: number;
  delta: number;
  hop_distance: number;
}

export interface AxSimResult {
  impacts: AxSimImpactRow[];
  affected_count: number;
}

export interface AxSandboxRecord {
  id: string;
  name: string;
  node_count: number;
  link_count: number;
  sim_result: AxSimResult | null;
  created_at: string;
}

export const xblueApi = {
  getSchema: (tenantId = "default") =>
    requestJson<XBlueSchema>(`/api/xblue/schema?tenant_id=${encodeURIComponent(tenantId)}`),
  saveSchema: (payload: {
    tenant_id?: string;
    node_types: Omit<BlueNodeTypeDef, "id">[];
    edge_types: Omit<BlueEdgeTypeDef, "id">[];
  }) =>
    requestJson<XBlueSchema>("/api/xblue/schema", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  getNodes: (tenantId = "default") =>
    requestJson<AxSimSnapshot>(`/api/xblue/nodes?tenant_id=${encodeURIComponent(tenantId)}`),
  upsertNode: (payload: {
    tenant_id?: string;
    type_name: string;
    node_key: string;
    title: string;
    metrics: Record<string, number>;
    link_to_node_id?: string;
    link_type?: string;
    link_weight?: number;
  }) =>
    requestJson<BlueNode>("/api/xblue/nodes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  listFlows: (tenantId = "default") =>
    requestJson<AxFlowDef[]>(`/api/axflow/flows?tenant_id=${encodeURIComponent(tenantId)}`),
  createFlow: (payload: {
    tenant_id?: string;
    name: string;
    description: string;
    steps: AxFlowStep[];
  }) =>
    requestJson<AxFlowDef>("/api/axflow/flows", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  runFlow: (id: string, tenantId = "default") =>
    requestJson<AxFlowRunResult>(
      `/api/axflow/flows/${id}/run?tenant_id=${encodeURIComponent(tenantId)}`,
      { method: "POST" }
    ),
  listSandboxes: (tenantId = "default") =>
    requestJson<AxSandboxRecord[]>(`/api/axsim/sandboxes?tenant_id=${encodeURIComponent(tenantId)}`),
  createSandbox: (payload: { tenant_id?: string; name: string }) =>
    requestJson<AxSandboxRecord>("/api/axsim/sandboxes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  simulateSandbox: (
    id: string,
    actions: { target_node_id: string; metric_key: string; delta_ratio: number }[],
    tenantId = "default"
  ) =>
    requestJson<AxSimResult>(
      `/api/axsim/sandboxes/${id}/simulate?tenant_id=${encodeURIComponent(tenantId)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ actions }),
      }
    ),
};
