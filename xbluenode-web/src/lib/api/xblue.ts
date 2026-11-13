import { requestJson } from "./client";
import type { AggFn, GraphAlgo, MetricSourceTag, RollupGrain } from "./enums";

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

export interface GeoPoint {
  lng: number;
  lat: number;
}

export type MetricsMerge = "MERGE" | "REPLACE";

export interface BlueNode {
  id: string;
  type_name: string;
  node_key: string;
  title: string;
  properties?: Record<string, unknown>;
  metrics: Record<string, number>;
  geo?: GeoPoint | null;
  source_dataset_id?: string | null;
  source_row_key?: string | null;
  row_version?: number;
  created_at?: string;
  updated_at?: string;
}

export interface BlueLink {
  id: string;
  link_type: string;
  src_node_id: string;
  dst_node_id: string;
  weight: number;
}

export interface LinkRecord extends BlueLink {
  properties: Record<string, unknown>;
  source_dataset_id?: string | null;
  valid_from?: string | null;
  valid_to?: string | null;
  created_at: string;
  updated_at: string;
}

export interface AxSimSnapshot {
  nodes: BlueNode[];
  links: BlueLink[];
}

export interface NodeListResponse extends AxSimSnapshot {
  items: BlueNode[];
  next_cursor: string | null;
}

export interface NodeRef {
  type_name: string;
  node_key: string;
}

export interface BulkNodeInput {
  type_name: string;
  node_key: string;
  title: string;
  properties?: Record<string, unknown>;
  metrics?: Record<string, number>;
  metrics_merge?: MetricsMerge;
  geo?: GeoPoint | null;
  source_dataset_id?: string | null;
  source_row_key?: string | null;
}

export interface BulkLinkInput {
  link_type: string;
  src: string | NodeRef;
  dst: string | NodeRef;
  weight?: number;
  properties?: Record<string, unknown>;
  valid_from?: string | null;
  valid_to?: string | null;
}

export interface BulkNodesResponse {
  upserted_nodes: number;
  upserted_links: number;
  unresolved: { index: number; link_type: string; reason: string }[];
}

export interface MetricPointInput {
  node_id?: string;
  node?: NodeRef;
  metric: string;
  ts: string;
  value?: number;
  value_text?: string;
}

export interface WriteMetricsResponse {
  written: number;
  dirty_minutes: number;
  rollup_job_id: string;
}

export type MetricQueryGrain = "RAW" | RollupGrain;

export interface MetricSeriesPoint {
  ts: string;
  value: number | null;
  value_text?: string;
}

export type WindowKind = "TUMBLING" | "SLIDING";

export interface WindowSpec {
  kind: WindowKind;
  size: string;
  step?: string;
}

export interface WindowQuery {
  node_ids?: string[];
  type_name?: string;
  metric: string;
  from: string;
  to: string;
  window: WindowSpec;
  agg: AggFn;
  group_by?: "node" | "type" | "none";
}

export interface WindowBucketItem {
  ts: string;
  group_key?: string;
  value: number | null;
}

export interface MetricCatalogEntry {
  type_name: string;
  metric: string;
  point_count: number;
  min_ts: string | null;
  max_ts: string | null;
}

export interface NodeSearchHit extends BlueNode {
  score: number;
  channels: string[];
}

export interface NodeDocumentHit {
  doc_id: string;
  title: string;
  source_name: string;
  entity_id: string;
  entity_name: string;
  snippets: string[];
}

export type EntityMatchMethod =
  | "EXACT_KEY"
  | "EXACT_TITLE"
  | "SYNONYM"
  | "EMBEDDING"
  | "MANUAL";

export interface EntityLink {
  entity_id: string;
  entity_name: string;
  node_id: string;
  node_key: string;
  node_title: string;
  match_method: EntityMatchMethod;
  confidence: number;
  confirmed: boolean;
  rejected: boolean;
  created_at: string;
}

export interface ResolveEntitiesResponse {
  resolved: number;
  auto_confirmed: number;
  pending_candidates: number;
}

export type TraversalDirection = "OUT" | "IN" | "BOTH";

export interface SubgraphResponse {
  nodes: BlueNode[];
  links: LinkRecord[];
  truncated: boolean;
}

export interface PathResponse {
  path: string[];
  nodes: BlueNode[];
  links: LinkRecord[];
  total_cost: number | null;
}

export interface AlgoTopEntry {
  node_id: string;
  title: string;
  score: number | null;
  community: number | null;
}

export interface AlgoRunResponse {
  algo: GraphAlgo;
  params_hash: string;
  node_count: number;
  edge_count: number;
  duration_ms: number;
  top: AlgoTopEntry[];
}

export interface AlgoResultRow {
  algo: GraphAlgo;
  params_hash: string;
  node_id: string;
  title: string;
  score: number | null;
  community: number | null;
  computed_at: string;
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

function buildQuery(params: Record<string, string | number | boolean | undefined>): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== "") {
      sp.set(k, String(v));
    }
  }
  const qs = sp.toString();
  return qs ? `?${qs}` : "";
}

export const xblueApi = {
  getSchema: () => requestJson<XBlueSchema>("/api/xblue/schema"),
  saveSchema: (payload: {
    node_types: Omit<BlueNodeTypeDef, "id">[];
    edge_types: Omit<BlueEdgeTypeDef, "id">[];
  }) =>
    requestJson<XBlueSchema>("/api/xblue/schema", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        node_types: payload.node_types.map((nt) => ({
          name: nt.name,
          display_name: nt.display_name,
          properties: nt.properties,
          agg_rules: nt.agg_rules,
          pos_x: nt.pos_x,
          pos_y: nt.pos_y,
        })),
        edge_types: payload.edge_types.map((et) => ({
          name: et.name,
          src_type_name: et.src_type_name,
          dst_type_name: et.dst_type_name,
          impact_weight: et.impact_weight,
        })),
      }),
    }),
  getNodes: () => requestJson<NodeListResponse>("/api/xblue/nodes"),
  listNodes: (params?: {
    type_name?: string;
    q?: string;
    bbox?: string;
    limit?: number;
    cursor?: string;
  }) =>
    requestJson<NodeListResponse>(
      `/api/xblue/nodes${buildQuery(params ?? {})}`
    ),
  getNode: (id: string) =>
    requestJson<BlueNode>(`/api/xblue/nodes/${encodeURIComponent(id)}`),
  upsertNode: (payload: {
    type_name: string;
    node_key: string;
    title: string;
    properties?: Record<string, unknown>;
    metrics?: Record<string, number>;
    metrics_merge?: MetricsMerge;
    geo?: GeoPoint | null;
    if_row_version?: number;
    link_to_node_id?: string;
    link_type?: string;
    link_weight?: number;
  }) =>
    requestJson<BlueNode>("/api/xblue/nodes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  patchNode: (
    id: string,
    payload: {
      title?: string;
      properties?: Record<string, unknown>;
      metrics?: Record<string, number>;
      metrics_merge?: MetricsMerge;
      geo?: GeoPoint | null;
      if_row_version?: number;
    }
  ) =>
    requestJson<BlueNode>(`/api/xblue/nodes/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  deleteNode: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/xblue/nodes/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),
  bulkUpsertNodes: (payload: {
    nodes: BulkNodeInput[];
    links?: BulkLinkInput[];
  }) =>
    requestJson<BulkNodesResponse>("/api/xblue/nodes/bulk", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  searchNodes: (params: { q: string; type_name?: string; k?: number }) =>
    requestJson<{ items: NodeSearchHit[] }>(
      `/api/xblue/nodes/search${buildQuery(params)}`
    ),
  getNodeDocuments: (id: string) =>
    requestJson<{ items: NodeDocumentHit[] }>(
      `/api/xblue/nodes/${encodeURIComponent(id)}/documents`
    ),
  queryNodeMetrics: (
    id: string,
    metric: string,
    params: {
      from: string;
      to: string;
      grain?: MetricQueryGrain;
      agg?: AggFn;
    }
  ) =>
    requestJson<MetricSeriesPoint[]>(
      `/api/xblue/nodes/${encodeURIComponent(id)}/metrics/${encodeURIComponent(metric)}${buildQuery(params)}`
    ),
  listLinks: (params?: {
    node_id?: string;
    link_type?: string;
    limit?: number;
  }) =>
    requestJson<LinkRecord[]>(`/api/xblue/links${buildQuery(params ?? {})}`),
  upsertLink: (payload: {
    link_type: string;
    src_node_id: string;
    dst_node_id: string;
    weight?: number;
    properties?: Record<string, unknown>;
    valid_from?: string | null;
    valid_to?: string | null;
  }) =>
    requestJson<LinkRecord>("/api/xblue/links", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  deleteLink: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/xblue/links/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),
  writeMetrics: (payload: {
    points: MetricPointInput[];
    source_tag?: MetricSourceTag;
    batch_id?: string;
  }) =>
    requestJson<WriteMetricsResponse>("/api/xblue/metrics/write", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  queryWindow: (query: WindowQuery) =>
    requestJson<{ items: WindowBucketItem[] }>("/api/xblue/metrics/window", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(query),
    }),
  getMetricCatalog: () =>
    requestJson<MetricCatalogEntry[]>("/api/xblue/metrics/catalog"),
  queryNeighbors: (params: {
    node_id: string;
    depth?: number;
    direction?: TraversalDirection;
    link_types?: string;
    limit?: number;
  }) =>
    requestJson<SubgraphResponse>(
      `/api/xblue/graph/neighbors${buildQuery(params)}`
    ),
  queryPath: (params: {
    from: string;
    to: string;
    max_depth?: number;
    weighted?: boolean;
  }) =>
    requestJson<PathResponse>(`/api/xblue/graph/path${buildQuery(params)}`),
  querySubgraph: (params?: {
    type_names?: string;
    link_types?: string;
    limit?: number;
  }) =>
    requestJson<SubgraphResponse>(
      `/api/xblue/graph/subgraph${buildQuery(params ?? {})}`
    ),
  runAlgorithm: (
    algo: GraphAlgo,
    payload: {
      type_names?: string[];
      link_types?: string[];
      params?: Record<string, unknown>;
    }
  ) =>
    requestJson<AlgoRunResponse>(
      `/api/xblue/graph/algorithms/${encodeURIComponent(algo)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      }
    ),
  getAlgorithmResults: (
    algo: GraphAlgo,
    params?: { params_hash?: string; limit?: number; cursor?: string }
  ) =>
    requestJson<{ items: AlgoResultRow[]; next_cursor: string | null }>(
      `/api/xblue/graph/algorithms/${encodeURIComponent(algo)}/results${buildQuery(params ?? {})}`
    ),
  resolveEntities: (payload?: { node_id?: string }) =>
    requestJson<ResolveEntitiesResponse>("/api/xblue/entities/resolve", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload ?? {}),
    }),
  listEntityCandidates: () =>
    requestJson<{ items: EntityLink[] }>("/api/xblue/entities/candidates"),
  confirmEntityLink: (entityId: string, nodeId: string) =>
    requestJson<EntityLink>(
      `/api/xblue/entities/links/${encodeURIComponent(entityId)}/${encodeURIComponent(nodeId)}/confirm`,
      { method: "POST" }
    ),
  rejectEntityLink: (entityId: string, nodeId: string) =>
    requestJson<EntityLink>(
      `/api/xblue/entities/links/${encodeURIComponent(entityId)}/${encodeURIComponent(nodeId)}/reject`,
      { method: "POST" }
    ),
  listFlows: () => requestJson<AxFlowDef[]>("/api/axflow/flows"),
  createFlow: (payload: {
    name: string;
    description: string;
    steps: AxFlowStep[];
  }) =>
    requestJson<AxFlowDef>("/api/axflow/flows", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  runFlow: (id: string) =>
    requestJson<AxFlowRunResult>(
      `/api/axflow/flows/${encodeURIComponent(id)}/run`,
      { method: "POST" }
    ),
  listSandboxes: () => requestJson<AxSandboxRecord[]>("/api/axsim/sandboxes"),
  createSandbox: (payload: { name: string }) =>
    requestJson<AxSandboxRecord>("/api/axsim/sandboxes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),
  simulateSandbox: (
    id: string,
    actions: { target_node_id: string; metric_key: string; delta_ratio: number }[]
  ) =>
    requestJson<AxSimResult>(
      `/api/axsim/sandboxes/${encodeURIComponent(id)}/simulate`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ actions }),
      }
    ),
};
