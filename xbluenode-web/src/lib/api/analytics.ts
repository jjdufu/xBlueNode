import type { ColumnMeta, Dataset, PaginatedList } from "./axhub";
import { requestJson } from "./client.ts";
import type {
  AggFn,
  ConnectorType,
  DataType,
  GroundingMode,
  PiiCategory,
  RollupGrain,
  WidgetKind,
} from "./enums";

export interface QueryColumn {
  name: string;
  data_type: DataType;
  pii_category: PiiCategory;
  masked: boolean;
}

export interface CatalogDatasetNode {
  id: string;
  name: string;
  display_name: string;
  lake_table: string;
  schema_version: number;
  row_count: number;
  columns: ColumnMeta[];
}

export interface PlatformViewNode {
  name: string;
  description: string;
  columns: QueryColumn[];
}

export interface LiveConnectionNode {
  id: string;
  name: string;
  alias: string;
  connector_type: ConnectorType;
}

export interface AnalyticsCatalogResponse {
  datasets: CatalogDatasetNode[];
  platform_views: PlatformViewNode[];
  live_connections: LiveConnectionNode[];
}

export interface QueryRequest {
  sql: string;
  params?: Record<string, unknown>;
  max_rows?: number;
  timeout_ms?: number;
  allow_live?: boolean;
  reveal_pii?: boolean;
  strict_ontology_join?: boolean;
}

export interface QueryResult {
  columns: QueryColumn[];
  rows: Record<string, unknown>[];
  row_count: number;
  truncated: boolean;
  sql: string;
  duration_ms: number;
}

export interface ExplainResult {
  sql: string;
  plan: string;
  referenced_datasets: string[];
  uses_platform_views: boolean;
}

export type FilterOp =
  | "Eq"
  | "Ne"
  | "Gt"
  | "Gte"
  | "Lt"
  | "Lte"
  | "In"
  | "NotIn"
  | "Between"
  | "Like"
  | "IsNull"
  | "IsNotNull";

export interface FieldRef {
  field: string;
  label?: string;
}

export interface ValueSpec {
  field: string;
  agg: AggFn;
  alias?: string;
  format?: string;
}

export interface FilterSpec {
  field: string;
  op: FilterOp;
  value?: unknown;
}

export interface TimeBucket {
  field: string;
  grain: RollupGrain;
}

export interface SortSpec {
  field: string;
  dir?: "ASC" | "DESC";
}

export interface PivotSpec {
  dataset: string;
  rows: FieldRef[];
  columns: FieldRef[];
  values: ValueSpec[];
  filters: FilterSpec[];
  time_bucket?: TimeBucket;
  sort: SortSpec[];
  subtotals: boolean;
  grand_total: boolean;
  row_limit: number;
  column_limit: number;
}

export type PivotRowKind = "DATA" | "SUBTOTAL" | "GRAND_TOTAL";

export interface PivotResult {
  row_headers: string[][];
  column_headers: string[][];
  cells: string[][];
  row_kinds: PivotRowKind[];
  columns_truncated: boolean;
  sql: string;
  duration_ms: number;
}

export interface MetricDimension {
  name: string;
  column: string;
  display_name: string;
}

export interface MetricDef {
  id: string;
  name: string;
  display_name: string;
  description: string;
  dataset_id: string;
  expression: string;
  filter_sql: string;
  time_column: string | null;
  default_grain: RollupGrain;
  dimensions: MetricDimension[];
  unit: string;
  format: string;
  synonyms: string[];
  certified: boolean;
  schema_version: number;
  broken: boolean;
  owner_user_id: string | null;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface CreateMetricRequest {
  name: string;
  display_name: string;
  description?: string;
  dataset_id: string;
  expression: string;
  filter_sql?: string;
  time_column?: string | null;
  default_grain?: RollupGrain;
  dimensions?: MetricDimension[];
  unit?: string;
  format?: string;
  synonyms?: string[];
  certified?: boolean;
}

export interface UpdateMetricRequest {
  display_name?: string;
  description?: string;
  expression?: string;
  filter_sql?: string;
  time_column?: string | null;
  default_grain?: RollupGrain;
  dimensions?: MetricDimension[];
  unit?: string;
  format?: string;
  synonyms?: string[];
  certified?: boolean;
}

export type MetricCompareMode = "PREVIOUS_PERIOD" | "SAME_PERIOD_LAST_YEAR";

export interface MetricTimeRange {
  from: string;
  to: string;
}

export interface MetricQuery {
  dimensions?: string[];
  filters?: FilterSpec[];
  time_range?: MetricTimeRange;
  grain?: RollupGrain;
  compare?: MetricCompareMode;
}

export interface MultiMetricQuery extends MetricQuery {
  metric_names: string[];
}

export interface MetricQueryResult {
  metrics: string[];
  columns: QueryColumn[];
  rows: Record<string, unknown>[];
  sql: string;
  duration_ms: number;
}

export type SavedQueryKind = "SQL" | "PIVOT" | "METRIC" | "ASK";

export interface QueryParamSpec {
  name: string;
  data_type: DataType;
  default?: unknown;
}

export interface SavedQuery {
  id: string;
  name: string;
  kind: SavedQueryKind;
  spec: Record<string, unknown>;
  parameters: QueryParamSpec[];
  owner_user_id: string | null;
  shared: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateSavedQueryRequest {
  name: string;
  kind: SavedQueryKind;
  spec: Record<string, unknown>;
  parameters?: QueryParamSpec[];
  shared?: boolean;
}

export interface UpdateSavedQueryRequest {
  name?: string;
  spec?: Record<string, unknown>;
  parameters?: QueryParamSpec[];
  shared?: boolean;
}

export type QueryHistoryKind =
  | "SQL"
  | "PIVOT"
  | "METRIC"
  | "ASK"
  | "TRANSFORM";

export type QueryHistoryStatus =
  | "SUCCEEDED"
  | "FAILED"
  | "REJECTED"
  | "TIMEOUT";

export interface QueryHistoryEntry {
  id: number;
  user_id: string | null;
  kind: QueryHistoryKind;
  sql_text: string;
  status: QueryHistoryStatus;
  row_count: number | null;
  duration_ms: number | null;
  error_code: string | null;
  created_at: string;
}

export type AskIntent = "DOC" | "DATA" | "HYBRID" | "CHITCHAT";
export type ChatQueryMode = "AUTO" | "DOC" | "DATA";
export type AskFeedbackKind = "UP" | "DOWN";

export interface ChatQueryScope {
  dataset_ids?: string[];
  doc_ids?: string[];
  doc_title_prefix?: string | null;
  subgraph_slug?: string | null;
  endpoint_slugs?: string[];
}

export interface AskContext {
  dataset_ids?: string[];
  doc_ids?: string[];
  doc_title_prefix?: string | null;
  subgraph_slug?: string | null;
  endpoint_slugs?: string[];
  filters?: FilterSpec[];
  drill_path?: string[];
  dashboard_id?: string;
}

export interface AskRequest {
  question: string;
  session_id?: string;
  context?: AskContext;
  scope?: ChatQueryScope | null;
}

export function applyScopeToAskContext(
  context?: AskContext,
  scope?: ChatQueryScope | null
): AskContext | undefined {
  if (!scope) {
    return context;
  }
  const merged: AskContext = { ...(context ?? {}) };
  if (Array.isArray(scope.dataset_ids) && scope.dataset_ids.length > 0) {
    merged.dataset_ids = scope.dataset_ids;
  }
  if (Array.isArray(scope.doc_ids) && scope.doc_ids.length > 0) {
    merged.doc_ids = scope.doc_ids;
  }
  if (
    typeof scope.doc_title_prefix === "string" &&
    scope.doc_title_prefix.trim().length > 0
  ) {
    merged.doc_title_prefix = scope.doc_title_prefix.trim();
  }
  if (
    typeof scope.subgraph_slug === "string" &&
    scope.subgraph_slug.trim().length > 0
  ) {
    merged.subgraph_slug = scope.subgraph_slug.trim();
  }
  if (Array.isArray(scope.endpoint_slugs) && scope.endpoint_slugs.length > 0) {
    merged.endpoint_slugs = scope.endpoint_slugs;
  }
  return Object.keys(merged).length > 0 ? merged : undefined;
}

export interface ChartSuggestion {
  kind: WidgetKind;
  x_field?: string | null;
  y_fields: string[];
  title?: string | null;
}

export interface LinkedObjects {
  datasets: string[];
  columns: string[];
  metrics: string[];
  nodes: string[];
}

export interface AskCitation {
  para_id: string;
  doc_id: string;
  doc_title: string;
  snippet: string;
}

export interface RelatedNodeSummary {
  id: string;
  type_name: string;
  title: string;
  metrics: Record<string, unknown>;
}

export interface AskResponse {
  turn_id: string;
  session_id: string;
  intent: AskIntent;
  status: QueryHistoryStatus;
  sql: string;
  columns: QueryColumn[];
  rows: Record<string, unknown>[];
  truncated: boolean;
  answer: string;
  chart: ChartSuggestion | null;
  need_clarification: boolean;
  explanation: string;
  linked_objects: LinkedObjects;
  citations: AskCitation[];
  grounding_mode?: GroundingMode;
  verified_numbers?: number;
  verified_citations?: number;
  abstained?: boolean;
  slot_payload?: Record<string, unknown> | null;
}

export interface AskTurnRecord {
  id: string;
  session_id: string;
  user_id: string | null;
  question: string;
  intent: AskIntent;
  linked_objects: LinkedObjects;
  sql_text: string | null;
  result_preview: Record<string, unknown> | null;
  answer: string;
  citations: AskCitation[];
  feedback: AskFeedbackKind | null;
  corrected_sql: string | null;
  model_name: string;
  created_at: string;
  grounding_mode?: GroundingMode;
  verified_numbers?: number;
  verified_citations?: number;
  abstained?: boolean;
  slot_payload?: Record<string, unknown> | null;
}

export type GroundingEvalTrack = "CLOUD" | "LOCAL_7B";

export interface GroundingEvalRunRecord {
  id: string;
  suite_name: string;
  track: GroundingEvalTrack;
  model_name: string;
  total_cases: number;
  intercepted_cases: number;
  pass_rate: number;
  category_breakdown: Record<string, unknown>;
  failures: unknown[];
  duration_ms: number;
  created_at: string;
}

export interface GroundingEvalRunResponse {
  runs: GroundingEvalRunRecord[];
}

export interface AskFeedbackRequest {
  feedback: AskFeedbackKind;
  corrected_sql?: string;
}

export type TransformDraftStatus = "DRAFT" | "APPLIED" | "DISCARDED";

export interface TransformPreview {
  columns: QueryColumn[];
  rows: Record<string, unknown>[];
  row_count_estimate: number;
  added_columns: string[];
  removed_columns: string[];
}

export interface TransformDraft {
  id: string;
  user_id: string | null;
  instruction: string;
  source_dataset_ids: string[];
  sql_text: string;
  preview: TransformPreview;
  target_dataset_name: string;
  status: TransformDraftStatus;
  dataset_id: string | null;
  created_at: string;
}

export interface CreateTransformDraftRequest {
  instruction: string;
  source_datasets: string[];
  target_dataset_name: string;
}

export interface ApplyTransformResponse {
  draft: TransformDraft;
  dataset: Dataset;
  job_id: string;
}

function jsonPost<T>(url: string, body: unknown): Promise<T> {
  return requestJson<T>(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

function jsonPatch<T>(url: string, body: unknown): Promise<T> {
  return requestJson<T>(url, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export const analyticsApi = {
  getCatalog: () => requestJson<AnalyticsCatalogResponse>("/api/analytics/catalog"),
  runSql: (payload: QueryRequest) =>
    jsonPost<QueryResult>("/api/analytics/sql/run", payload),
  explainSql: (payload: QueryRequest) =>
    jsonPost<ExplainResult>("/api/analytics/sql/explain", payload),
  runPivot: (payload: PivotSpec) =>
    jsonPost<PivotResult>("/api/analytics/pivot", payload),

  listMetrics: () => requestJson<MetricDef[]>("/api/analytics/metrics"),
  getMetric: (name: string) =>
    requestJson<MetricDef>(`/api/analytics/metrics/${encodeURIComponent(name)}`),
  createMetric: (payload: CreateMetricRequest) =>
    jsonPost<MetricDef>("/api/analytics/metrics", payload),
  updateMetric: (name: string, payload: UpdateMetricRequest) =>
    jsonPatch<MetricDef>(
      `/api/analytics/metrics/${encodeURIComponent(name)}`,
      payload
    ),
  deleteMetric: (name: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/analytics/metrics/${encodeURIComponent(name)}`,
      { method: "DELETE" }
    ),
  queryMetric: (name: string, payload: MetricQuery) =>
    jsonPost<MetricQueryResult>(
      `/api/analytics/metrics/${encodeURIComponent(name)}/query`,
      payload
    ),
  queryMultiMetrics: (payload: MultiMetricQuery) =>
    jsonPost<MetricQueryResult>("/api/analytics/metrics/query-multi", payload),

  listSavedQueries: () =>
    requestJson<SavedQuery[]>("/api/analytics/saved-queries"),
  getSavedQuery: (id: string) =>
    requestJson<SavedQuery>(
      `/api/analytics/saved-queries/${encodeURIComponent(id)}`
    ),
  createSavedQuery: (payload: CreateSavedQueryRequest) =>
    jsonPost<SavedQuery>("/api/analytics/saved-queries", payload),
  updateSavedQuery: (id: string, payload: UpdateSavedQueryRequest) =>
    jsonPatch<SavedQuery>(
      `/api/analytics/saved-queries/${encodeURIComponent(id)}`,
      payload
    ),
  deleteSavedQuery: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/analytics/saved-queries/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),
  runSavedQuery: (id: string, params: Record<string, unknown> = {}) =>
    jsonPost<unknown>(
      `/api/analytics/saved-queries/${encodeURIComponent(id)}/run`,
      { params }
    ),

  ask: (payload: AskRequest) => {
    const { scope, context, ...rest } = payload;
    const nextContext = applyScopeToAskContext(context, scope);
    return jsonPost<AskResponse>("/api/analytics/ask", {
      ...rest,
      ...(nextContext ? { context: nextContext } : {}),
    });
  },
  askData: (payload: AskRequest) => {
    const { scope, context, ...rest } = payload;
    const nextContext = applyScopeToAskContext(context, scope);
    return jsonPost<AskResponse>("/api/analytics/ask", {
      ...rest,
      ...(nextContext ? { context: nextContext } : {}),
    });
  },
  submitAskFeedback: (turnId: string, payload: AskFeedbackRequest) =>
    jsonPost<AskTurnRecord>(
      `/api/analytics/ask/${encodeURIComponent(turnId)}/feedback`,
      payload
    ),
  getAskSession: (sessionId: string) =>
    requestJson<AskTurnRecord[]>(
      `/api/analytics/ask/sessions/${encodeURIComponent(sessionId)}`
    ),
  runGroundingEval: () =>
    jsonPost<GroundingEvalRunResponse>("/api/analytics/grounding-eval/run", {}),
  listGroundingEvalRuns: (limit = 20) =>
    requestJson<{ items: GroundingEvalRunRecord[] }>(
      `/api/analytics/grounding-eval/runs?limit=${encodeURIComponent(String(limit))}`
    ),

  createTransformDraft: (payload: CreateTransformDraftRequest) =>
    jsonPost<TransformDraft>("/api/analytics/transform/draft", payload),
  updateTransformDraft: (id: string, sqlText: string) =>
    jsonPatch<TransformDraft>(
      `/api/analytics/transform/${encodeURIComponent(id)}`,
      { sql_text: sqlText }
    ),
  applyTransformDraft: (id: string) =>
    jsonPost<ApplyTransformResponse>(
      `/api/analytics/transform/${encodeURIComponent(id)}/apply`,
      {}
    ),
  discardTransformDraft: (id: string) =>
    jsonPost<TransformDraft>(
      `/api/analytics/transform/${encodeURIComponent(id)}/discard`,
      {}
    ),

  listHistory: (params?: {
    kind?: QueryHistoryKind;
    status?: QueryHistoryStatus;
    limit?: number;
    cursor?: string;
  }) => {
    const qs = new URLSearchParams();
    if (params?.kind) qs.set("kind", params.kind);
    if (params?.status) qs.set("status", params.status);
    if (params?.limit !== undefined) qs.set("limit", String(params.limit));
    if (params?.cursor) qs.set("cursor", params.cursor);
    const suffix = qs.toString() ? `?${qs.toString()}` : "";
    return requestJson<PaginatedList<QueryHistoryEntry>>(
      `/api/analytics/history${suffix}`
    );
  },
};
