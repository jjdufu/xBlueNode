import { z } from "zod";
import { getAccessToken } from "./auth";
import { ApiError, ApiErrorBodySchema, requestJson } from "./client";
import type {
  AggFn,
  Cardinality,
  DataType,
  GraphAlgo,
  MetricSourceTag,
  PiiCategory,
  RollupGrain,
  SubgraphMergeStrategy,
} from "./enums";

export type PropertyKind = "ATTRIBUTE" | "METRIC";
export type LinkDirection = "OUT" | "IN";
export type OnSourceDelete = "KEEP" | "DELETE";
export type MappingMode = "FULL" | "INCREMENTAL";
export type IssueSeverity = "ERROR" | "WARN";
export type DraftSource = "MANUAL" | "AI" | "RESTORE" | "TEMPLATE" | "MERGE";
export type DraftStatus = "DRAFT" | "PUBLISHED" | "DISCARDED";
export type MergeConflictKind =
  | "PROPERTY_TYPE_CONFLICT"
  | "RULE_CONFLICT"
  | "EDGE_CONFLICT"
  | "MAPPING_NAME_CONFLICT";
export type MappingRunStatus = "RUNNING" | "SUCCEEDED" | "FAILED";

export interface PropertyDef {
  name: string;
  data_type: string;
  display_name?: string;
  description?: string;
  unit?: string | null;
  kind?: PropertyKind;
  required?: boolean;
  pii_category?: PiiCategory;
  allowed_values?: string[] | null;
}

export interface AggRuleDef {
  target_metric: string;
  op: string;
  via_link_type?: string;
  direction?: LinkDirection;
  source_type_name?: string;
  source_metric?: string;
  filter?: string | null;
  weight_by_link?: boolean;
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

export interface PropertySpec {
  name: string;
  display_name: string;
  description: string;
  unit?: string | null;
  data_type: DataType;
  kind: PropertyKind;
  required: boolean;
  pii_category: PiiCategory;
  allowed_values?: string[] | null;
}

export interface AggRuleSpec {
  target_metric: string;
  op: AggFn;
  via_link_type: string;
  direction: LinkDirection;
  source_type_name: string;
  source_metric: string;
  filter?: string | null;
  weight_by_link: boolean;
}

export interface DerivedRuleSpec {
  target_metric: string;
  expression: string;
  enabled: boolean;
}

export interface NodeTypeSpec {
  id?: string;
  name: string;
  display_name: string;
  description: string;
  icon: string;
  color: string;
  title_template: string;
  properties: PropertySpec[];
  agg_rules: AggRuleSpec[];
  derived_rules: DerivedRuleSpec[];
  sync_to_knowledge: boolean;
  pos_x: number;
  pos_y: number;
}

export interface EdgeTypeSpec {
  id?: string;
  name: string;
  src_type_name: string;
  dst_type_name: string;
  impact_weight: number;
  cardinality: Cardinality;
  description: string;
  properties: PropertySpec[];
  propagation: Record<string, unknown>;
}

export interface MappingGeoSpec {
  lng_column: string | null;
  lat_column: string | null;
}

export interface MappingLinkSpec {
  link_type: string;
  direction: LinkDirection;
  target_type: string;
  target_key_column: string;
  weight_column?: string | null;
  create_missing_target: boolean;
}

export interface MappingSpec {
  name: string;
  dataset: string;
  node_type: string;
  key_column: string;
  title: string;
  filter?: string | null;
  attributes: Record<string, string>;
  metrics: Record<string, string>;
  metric_time_column?: string | null;
  geo: MappingGeoSpec;
  links: MappingLinkSpec[];
  on_source_delete: OnSourceDelete;
  mode: MappingMode;
}

export interface SchemaDoc {
  node_types: NodeTypeSpec[];
  edge_types: EdgeTypeSpec[];
  mappings: MappingSpec[];
}

export interface ValidationIssue {
  severity: IssueSeverity;
  path: string;
  code: string;
  message: string;
}

export interface ValidateSchemaResponse {
  valid: boolean;
  issues: ValidationIssue[];
}

export interface FieldDiff {
  path: string;
  before: unknown;
  after: unknown;
}

export interface ChangedItemDiff {
  name: string;
  fields: FieldDiff[];
}

export interface SectionDiff {
  added: string[];
  removed: string[];
  changed: ChangedItemDiff[];
}

export interface SchemaDiff {
  node_types: SectionDiff;
  edge_types: SectionDiff;
  mappings: SectionDiff;
}

export interface SubgraphInfo {
  id: string;
  slug: string;
  display_name: string;
  description: string;
  owner_dept: string;
  is_main: boolean;
  current_version: number;
  created_by?: string | null;
  created_at: string;
  updated_at: string;
}

export interface CreateSubgraphRequest {
  slug: string;
  display_name: string;
  description?: string;
  owner_dept?: string;
  seed_from_main?: boolean;
  seed_node_types?: string[];
}

export interface UpdateSubgraphRequest {
  display_name?: string;
  description?: string;
  owner_dept?: string;
}

export interface PublishedSchemaResponse extends SchemaDoc {
  subgraph_slug: string;
  version: number;
}

export interface SaveSchemaDirectRequest extends SchemaDoc {
  base_version?: number;
  change_summary?: string;
  force?: boolean;
  cascade?: boolean;
}

export interface SchemaVersionSummary {
  id: string;
  subgraph_slug: string;
  version: number;
  change_summary: string;
  published_by?: string | null;
  published_at: string;
}

export interface SchemaVersionDetail extends SchemaVersionSummary {
  schema_doc: SchemaDoc;
  diff: SchemaDiff;
}

export interface SchemaDraft {
  id: string;
  subgraph_slug: string;
  title: string;
  source: DraftSource;
  base_version: number;
  schema_doc: SchemaDoc;
  ai_context: Record<string, unknown>;
  issues: ValidationIssue[];
  status: DraftStatus;
  created_by?: string | null;
  created_at: string;
  updated_at: string;
}

export interface CreateDraftRequest {
  title: string;
  schema_doc: SchemaDoc;
  base_version?: number;
  source?: DraftSource;
  ai_context?: Record<string, unknown>;
}

export interface UpdateDraftRequest {
  title?: string;
  schema_doc: SchemaDoc;
}

export interface PublishDraftRequest {
  change_summary?: string;
  force?: boolean;
  cascade?: boolean;
}

export interface PublishDraftResponse {
  subgraph_slug: string;
  version: number;
  draft_id: string;
  schema_doc: SchemaDoc;
  diff: SchemaDiff;
  mapping_jobs: string[];
  aggregate_jobs: string[];
}

export interface AiDraftRequest {
  title: string;
  dataset_ids?: string[];
  doc_ids?: string[];
  description?: string;
}

export interface TypeAlignmentCandidate {
  sub_type: string;
  main_type: string;
  score: number;
  suggested_strategy: SubgraphMergeStrategy;
}

export interface MergeConflict {
  kind: MergeConflictKind;
  sub_item: string;
  main_item: string;
  detail: string;
}

export interface TypeMergeResolution {
  sub_type: string;
  main_type: string;
  strategy: SubgraphMergeStrategy;
  bridge_edge_name?: string;
}

export interface MergePreviewRequest {
  resolutions?: TypeMergeResolution[];
}

export interface MergePreview {
  subgraph_slug: string;
  subgraph_version: number;
  main_version: number;
  alignments: TypeAlignmentCandidate[];
  conflicts: MergeConflict[];
  added_node_types: string[];
  added_edge_types: string[];
  added_mappings: string[];
}

export interface MergeSubgraphRequest {
  base_main_version?: number;
  default_strategy: SubgraphMergeStrategy;
  resolutions?: TypeMergeResolution[];
  change_summary?: string;
  force?: boolean;
}

export interface MappingRecord {
  id: string;
  subgraph_slug: string;
  name: string;
  dataset_id: string;
  dataset_name: string;
  node_type: string;
  spec: MappingSpec;
  enabled: boolean;
  schema_version: number;
  last_snapshot_id: number | null;
  created_at: string;
  updated_at: string;
}

export interface MappingRejectSample {
  row: unknown;
  reason: string;
}

export interface MappingRunRecord {
  id: string;
  mapping_id: string;
  job_id: string;
  mode: MappingMode;
  from_snapshot: number | null;
  to_snapshot: number | null;
  nodes_upserted: number;
  nodes_deleted: number;
  links_upserted: number;
  points_written: number;
  rows_rejected: number;
  reject_samples: MappingRejectSample[];
  status: MappingRunStatus;
  error_message: string | null;
  started_at: string;
  finished_at: string | null;
}

export interface TriggerMappingRunResponse {
  job_id: string;
  mapping_id: string;
  mode: MappingMode;
}

export interface MappingPreviewNodeRow {
  node_key: string;
  title: string;
  attributes: Record<string, unknown>;
  metrics: Record<string, number>;
  geo?: GeoPoint | null;
  links: unknown[];
}

export interface MappingPreviewResponse {
  mapping_id: string;
  sampled_rows: number;
  items: MappingPreviewNodeRow[];
  rows_rejected: number;
  reject_samples: MappingRejectSample[];
}

export interface ObjectFieldSig {
  name: string;
  display_name: string;
  kind: PropertyKind;
  data_type: DataType;
  required: boolean;
  unit?: string | null;
  pii_category: PiiCategory;
}

export interface ObjectNeighborSig {
  field_name: string;
  link_type: string;
  direction: LinkDirection;
  target_type: string;
  cardinality: Cardinality;
}

export interface ObjectTypeCatalogItem {
  name: string;
  display_name: string;
  description: string;
  collection_path: string;
  item_path: string;
  graphql_field: string;
  graphql_list_field: string;
  fields: ObjectFieldSig[];
  neighbors: ObjectNeighborSig[];
  sample_graphql: string;
}

export interface ObjectCatalogResponse {
  version: number;
  items: ObjectTypeCatalogItem[];
}

export interface DomainObjectListQuery {
  q?: string;
  filter?: string;
  limit?: number;
  cursor?: string;
}

export interface DomainObjectRecord {
  id: string;
  type_name: string;
  node_key: string;
  title: string;
  attributes: Record<string, unknown>;
  metrics: Record<string, number>;
  geo?: GeoPoint | null;
  updated_at: string;
  neighbors?: Record<string, DomainObjectRecord[]> | null;
}

export interface DomainObjectListResponse {
  items: DomainObjectRecord[];
  next_cursor: string | null;
}

export interface GraphqlSdlResponse {
  version: number;
  sdl: string;
}

export interface GraphqlErrorItem {
  message: string;
  locations?: { line: number; column: number }[];
  path?: (string | number)[];
}

export interface GraphqlExecutionResponse {
  data?: Record<string, unknown> | null;
  errors?: GraphqlErrorItem[];
}

const CANONICAL_DATA_TYPES: readonly DataType[] = [
  "STRING",
  "INT64",
  "FLOAT64",
  "DECIMAL",
  "BOOLEAN",
  "DATE",
  "TIMESTAMP",
  "BINARY",
  "JSON",
  "LIST",
  "STRUCT",
];

const CANONICAL_PII_CATEGORIES: readonly PiiCategory[] = [
  "NONE",
  "PHONE",
  "ID_CARD",
  "EMAIL",
  "BANK_CARD",
  "NAME",
  "ADDRESS",
  "SECRET",
];

const CANONICAL_AGG_FNS: readonly AggFn[] = [
  "SUM",
  "AVG",
  "COUNT",
  "COUNT_DISTINCT",
  "MIN",
  "MAX",
  "MEDIAN",
  "LAST",
];

const CANONICAL_CARDINALITIES: readonly Cardinality[] = [
  "ONE_TO_ONE",
  "ONE_TO_MANY",
  "MANY_TO_ONE",
  "MANY_TO_MANY",
];

const CANONICAL_MERGE_STRATEGIES: readonly SubgraphMergeStrategy[] = [
  "REUSE_MAIN",
  "EXTEND_PROPS",
  "BRIDGE_LINK",
];

const UUID_REGEX =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Normalizes legacy lowercase data_type aliases ("number", "string", "boolean") to canonical DataType. */
export function normalizeStudioDataType(raw: unknown): DataType {
  if (typeof raw !== "string") return "STRING";
  const trimmed = raw.trim();
  const lower = trimmed.toLowerCase();
  if (lower === "number" || lower === "float" || lower === "double") {
    return "FLOAT64";
  }
  if (lower === "string" || lower === "text") {
    return "STRING";
  }
  if (lower === "boolean" || lower === "bool") {
    return "BOOLEAN";
  }
  if (lower === "integer" || lower === "int") {
    return "INT64";
  }
  const upper = trimmed.toUpperCase() as DataType;
  if ((CANONICAL_DATA_TYPES as readonly string[]).includes(upper)) {
    return upper;
  }
  return "STRING";
}

function normalizePiiCategory(raw: unknown): PiiCategory {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase() as PiiCategory;
    if ((CANONICAL_PII_CATEGORIES as readonly string[]).includes(upper)) {
      return upper;
    }
  }
  return "NONE";
}

function normalizeAggFn(raw: unknown): AggFn {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase() as AggFn;
    if ((CANONICAL_AGG_FNS as readonly string[]).includes(upper)) {
      return upper;
    }
  }
  return "SUM";
}

function normalizeCardinality(raw: unknown): Cardinality {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase() as Cardinality;
    if ((CANONICAL_CARDINALITIES as readonly string[]).includes(upper)) {
      return upper;
    }
  }
  return "MANY_TO_MANY";
}

function normalizeMergeStrategy(raw: unknown): SubgraphMergeStrategy {
  if (typeof raw === "string") {
    const upper = raw.trim().toUpperCase() as SubgraphMergeStrategy;
    if ((CANONICAL_MERGE_STRATEGIES as readonly string[]).includes(upper)) {
      return upper;
    }
  }
  return "EXTEND_PROPS";
}

export const PropertyKindSchema = z
  .unknown()
  .transform((val): PropertyKind => (val === "METRIC" ? "METRIC" : "ATTRIBUTE"));

export const LinkDirectionSchema = z
  .unknown()
  .transform((val): LinkDirection => (val === "IN" ? "IN" : "OUT"));

export const DataTypeSchema = z.unknown().transform(normalizeStudioDataType);
export const PiiCategorySchema = z.unknown().transform(normalizePiiCategory);
export const AggFnSchema = z.unknown().transform(normalizeAggFn);
export const CardinalitySchema = z.unknown().transform(normalizeCardinality);
export const SubgraphMergeStrategySchema = z
  .unknown()
  .transform(normalizeMergeStrategy);

export const PropertySpecSchema: z.ZodType<PropertySpec> = z.object({
  name: z.string().default(""),
  display_name: z.string().default(""),
  description: z.string().default(""),
  unit: z.string().nullable().optional(),
  data_type: DataTypeSchema,
  kind: PropertyKindSchema,
  required: z.boolean().default(false),
  pii_category: PiiCategorySchema,
  allowed_values: z.array(z.string()).nullable().optional(),
});

export const AggRuleSpecSchema: z.ZodType<AggRuleSpec> = z.object({
  target_metric: z.string().default(""),
  op: AggFnSchema,
  via_link_type: z.string().default(""),
  direction: LinkDirectionSchema,
  source_type_name: z.string().default(""),
  source_metric: z.string().default(""),
  filter: z.string().nullable().optional(),
  weight_by_link: z.boolean().default(false),
});

export const DerivedRuleSpecSchema: z.ZodType<DerivedRuleSpec> = z.object({
  target_metric: z.string().default(""),
  expression: z.string().default(""),
  enabled: z.boolean().default(true),
});

export const NodeTypeSpecSchema: z.ZodType<NodeTypeSpec> = z.object({
  id: z.string().optional(),
  name: z.string().default(""),
  display_name: z.string().default(""),
  description: z.string().default(""),
  icon: z.string().default("box"),
  color: z.string().default("#2563eb"),
  title_template: z.string().default("{node_key}"),
  properties: z.array(PropertySpecSchema).default([]),
  agg_rules: z.array(AggRuleSpecSchema).default([]),
  derived_rules: z.array(DerivedRuleSpecSchema).default([]),
  sync_to_knowledge: z.boolean().default(false),
  pos_x: z.number().default(120),
  pos_y: z.number().default(120),
});

export const EdgeTypeSpecSchema: z.ZodType<EdgeTypeSpec> = z.object({
  id: z.string().optional(),
  name: z.string().default(""),
  src_type_name: z.string().default(""),
  dst_type_name: z.string().default(""),
  impact_weight: z.number().default(0.6),
  cardinality: CardinalitySchema,
  description: z.string().default(""),
  properties: z.array(PropertySpecSchema).default([]),
  propagation: z.record(z.string(), z.unknown()).default({}),
});

export const MappingGeoSpecSchema: z.ZodType<MappingGeoSpec> = z.object({
  lng_column: z.string().nullable().default(null),
  lat_column: z.string().nullable().default(null),
});

export const MappingLinkSpecSchema: z.ZodType<MappingLinkSpec> = z.object({
  link_type: z.string().default(""),
  direction: LinkDirectionSchema,
  target_type: z.string().default(""),
  target_key_column: z.string().default(""),
  weight_column: z.string().nullable().optional(),
  create_missing_target: z.boolean().default(false),
});

export const MappingSpecSchema: z.ZodType<MappingSpec> = z.object({
  name: z.string().default(""),
  dataset: z.string().default(""),
  node_type: z.string().default(""),
  key_column: z.string().default("id"),
  title: z.string().default("{id}"),
  filter: z.string().nullable().optional(),
  attributes: z.record(z.string(), z.string()).default({}),
  metrics: z.record(z.string(), z.string()).default({}),
  metric_time_column: z.string().nullable().optional(),
  geo: MappingGeoSpecSchema.default({ lng_column: null, lat_column: null }),
  links: z.array(MappingLinkSpecSchema).default([]),
  on_source_delete: z
    .unknown()
    .transform((v): OnSourceDelete => (v === "DELETE" ? "DELETE" : "KEEP")),
  mode: z
    .unknown()
    .transform((v): MappingMode => (v === "FULL" ? "FULL" : "INCREMENTAL")),
});

export const SchemaDocSchema: z.ZodType<SchemaDoc> = z.object({
  node_types: z.array(NodeTypeSpecSchema).default([]),
  edge_types: z.array(EdgeTypeSpecSchema).default([]),
  mappings: z.array(MappingSpecSchema).default([]),
});

export const ValidationIssueSchema: z.ZodType<ValidationIssue> = z.object({
  severity: z
    .unknown()
    .transform((v): IssueSeverity => (v === "ERROR" ? "ERROR" : "WARN")),
  path: z.string().default(""),
  code: z.string().default("UNKNOWN"),
  message: z.string().default(""),
});

export const ValidateSchemaResponseSchema: z.ZodType<ValidateSchemaResponse> =
  z.object({
    valid: z.boolean().default(true),
    issues: z.array(ValidationIssueSchema).default([]),
  });

export const FieldDiffSchema: z.ZodType<FieldDiff> = z.object({
  path: z.string().default(""),
  before: z.unknown(),
  after: z.unknown(),
});

export const ChangedItemDiffSchema: z.ZodType<ChangedItemDiff> = z.object({
  name: z.string().default(""),
  fields: z.array(FieldDiffSchema).default([]),
});

export const SectionDiffSchema: z.ZodType<SectionDiff> = z.object({
  added: z.array(z.string()).default([]),
  removed: z.array(z.string()).default([]),
  changed: z.array(ChangedItemDiffSchema).default([]),
});

const EMPTY_SECTION_DIFF: SectionDiff = {
  added: [],
  removed: [],
  changed: [],
};

export const SchemaDiffSchema: z.ZodType<SchemaDiff> = z.object({
  node_types: SectionDiffSchema.default(EMPTY_SECTION_DIFF),
  edge_types: SectionDiffSchema.default(EMPTY_SECTION_DIFF),
  mappings: SectionDiffSchema.default(EMPTY_SECTION_DIFF),
});

export const SubgraphInfoSchema: z.ZodType<SubgraphInfo> = z.object({
  id: z.string().default("00000000-0000-0000-0000-000000000000"),
  slug: z.string().default("main"),
  display_name: z.string().default("企业主图"),
  description: z.string().default(""),
  owner_dept: z.string().default("default"),
  is_main: z.boolean().default(false),
  current_version: z.number().default(1),
  created_by: z.string().nullable().optional(),
  created_at: z.string().default(""),
  updated_at: z.string().default(""),
});

export const PublishedSchemaResponseSchema: z.ZodType<PublishedSchemaResponse> =
  z.object({
    subgraph_slug: z.string().default("main"),
    version: z.number().default(1),
    node_types: z.array(NodeTypeSpecSchema).default([]),
    edge_types: z.array(EdgeTypeSpecSchema).default([]),
    mappings: z.array(MappingSpecSchema).default([]),
  });

export const SchemaVersionSummarySchema: z.ZodType<SchemaVersionSummary> =
  z.object({
    id: z.string().default(""),
    subgraph_slug: z.string().default("main"),
    version: z.number().default(1),
    change_summary: z.string().default(""),
    published_by: z.string().nullable().optional(),
    published_at: z.string().default(""),
  });

export const SchemaVersionDetailSchema: z.ZodType<SchemaVersionDetail> =
  z.object({
    id: z.string().default(""),
    subgraph_slug: z.string().default("main"),
    version: z.number().default(1),
    schema_doc: SchemaDocSchema.default({
      node_types: [],
      edge_types: [],
      mappings: [],
    }),
    diff: SchemaDiffSchema.default({
      node_types: EMPTY_SECTION_DIFF,
      edge_types: EMPTY_SECTION_DIFF,
      mappings: EMPTY_SECTION_DIFF,
    }),
    change_summary: z.string().default(""),
    published_by: z.string().nullable().optional(),
    published_at: z.string().default(""),
  });

export const SchemaDraftSchema: z.ZodType<SchemaDraft> = z.object({
  id: z.string().default(""),
  subgraph_slug: z.string().default("main"),
  title: z.string().default("未命名草案"),
  source: z
    .unknown()
    .transform((v): DraftSource => {
      if (
        v === "MANUAL" ||
        v === "AI" ||
        v === "RESTORE" ||
        v === "TEMPLATE" ||
        v === "MERGE"
      ) {
        return v;
      }
      return "MANUAL";
    }),
  base_version: z.number().default(1),
  schema_doc: SchemaDocSchema.default({
    node_types: [],
    edge_types: [],
    mappings: [],
  }),
  ai_context: z.record(z.string(), z.unknown()).default({}),
  issues: z.array(ValidationIssueSchema).default([]),
  status: z
    .unknown()
    .transform((v): DraftStatus => {
      if (v === "DRAFT" || v === "PUBLISHED" || v === "DISCARDED") {
        return v;
      }
      return "DRAFT";
    }),
  created_by: z.string().nullable().optional(),
  created_at: z.string().default(""),
  updated_at: z.string().default(""),
});

export const PublishDraftResponseSchema: z.ZodType<PublishDraftResponse> =
  z.object({
    subgraph_slug: z.string().default("main"),
    version: z.number().default(1),
    draft_id: z.string().default(""),
    schema_doc: SchemaDocSchema.default({
      node_types: [],
      edge_types: [],
      mappings: [],
    }),
    diff: SchemaDiffSchema.default({
      node_types: EMPTY_SECTION_DIFF,
      edge_types: EMPTY_SECTION_DIFF,
      mappings: EMPTY_SECTION_DIFF,
    }),
    mapping_jobs: z.array(z.string()).default([]),
    aggregate_jobs: z.array(z.string()).default([]),
  });

export const TypeAlignmentCandidateSchema: z.ZodType<TypeAlignmentCandidate> =
  z.object({
    sub_type: z.string().default(""),
    main_type: z.string().default(""),
    score: z.number().default(0),
    suggested_strategy: SubgraphMergeStrategySchema,
  });

export const MergeConflictSchema: z.ZodType<MergeConflict> = z.object({
  kind: z
    .unknown()
    .transform((v): MergeConflictKind => {
      if (
        v === "PROPERTY_TYPE_CONFLICT" ||
        v === "RULE_CONFLICT" ||
        v === "EDGE_CONFLICT" ||
        v === "MAPPING_NAME_CONFLICT"
      ) {
        return v;
      }
      return "PROPERTY_TYPE_CONFLICT";
    }),
  sub_item: z.string().default(""),
  main_item: z.string().default(""),
  detail: z.string().default(""),
});

export const MergePreviewSchema: z.ZodType<MergePreview> = z.object({
  subgraph_slug: z.string().default(""),
  subgraph_version: z.number().default(1),
  main_version: z.number().default(1),
  alignments: z.array(TypeAlignmentCandidateSchema).default([]),
  conflicts: z.array(MergeConflictSchema).default([]),
  added_node_types: z.array(z.string()).default([]),
  added_edge_types: z.array(z.string()).default([]),
  added_mappings: z.array(z.string()).default([]),
});

export const MappingRecordSchema: z.ZodType<MappingRecord> = z.object({
  id: z.string().default(""),
  subgraph_slug: z.string().default("main"),
  name: z.string().default(""),
  dataset_id: z.string().default(""),
  dataset_name: z.string().default(""),
  node_type: z.string().default(""),
  spec: MappingSpecSchema.default({
    name: "",
    dataset: "",
    node_type: "",
    key_column: "id",
    title: "{id}",
    attributes: {},
    metrics: {},
    geo: { lng_column: null, lat_column: null },
    links: [],
    on_source_delete: "KEEP",
    mode: "INCREMENTAL",
  }),
  enabled: z.boolean().default(true),
  schema_version: z.number().default(1),
  last_snapshot_id: z.number().nullable().default(null),
  created_at: z.string().default(""),
  updated_at: z.string().default(""),
});

export const MappingRejectSampleSchema: z.ZodType<MappingRejectSample> =
  z.object({
    row: z.unknown(),
    reason: z.string().default(""),
  });

export const MappingRunRecordSchema: z.ZodType<MappingRunRecord> = z.object({
  id: z.string().default(""),
  mapping_id: z.string().default(""),
  job_id: z.string().default(""),
  mode: z
    .unknown()
    .transform((v): MappingMode => (v === "FULL" ? "FULL" : "INCREMENTAL")),
  from_snapshot: z.number().nullable().default(null),
  to_snapshot: z.number().nullable().default(null),
  nodes_upserted: z.number().default(0),
  nodes_deleted: z.number().default(0),
  links_upserted: z.number().default(0),
  points_written: z.number().default(0),
  rows_rejected: z.number().default(0),
  reject_samples: z.array(MappingRejectSampleSchema).default([]),
  status: z
    .unknown()
    .transform((v): MappingRunStatus => {
      if (v === "RUNNING" || v === "SUCCEEDED" || v === "FAILED") {
        return v;
      }
      return "SUCCEEDED";
    }),
  error_message: z.string().nullable().default(null),
  started_at: z.string().default(""),
  finished_at: z.string().nullable().default(null),
});

export const TriggerMappingRunResponseSchema: z.ZodType<TriggerMappingRunResponse> =
  z.object({
    job_id: z.string().default(""),
    mapping_id: z.string().default(""),
    mode: z
      .unknown()
      .transform((v): MappingMode => (v === "FULL" ? "FULL" : "INCREMENTAL")),
  });

export const MappingPreviewNodeRowSchema: z.ZodType<MappingPreviewNodeRow> =
  z.object({
    node_key: z.string().default(""),
    title: z.string().default(""),
    attributes: z.record(z.string(), z.unknown()).default({}),
    metrics: z.record(z.string(), z.number()).default({}),
    geo: z
      .object({
        lng: z.number(),
        lat: z.number(),
      })
      .nullable()
      .optional(),
    links: z.array(z.unknown()).default([]),
  });

export const MappingPreviewResponseSchema: z.ZodType<MappingPreviewResponse> =
  z.object({
    mapping_id: z.string().default(""),
    sampled_rows: z.number().default(0),
    items: z.array(MappingPreviewNodeRowSchema).default([]),
    rows_rejected: z.number().default(0),
    reject_samples: z.array(MappingRejectSampleSchema).default([]),
  });

export const ObjectFieldSigSchema: z.ZodType<ObjectFieldSig> = z.object({
  name: z.string().default(""),
  display_name: z.string().default(""),
  kind: PropertyKindSchema,
  data_type: DataTypeSchema,
  required: z.boolean().default(false),
  unit: z.string().nullable().optional(),
  pii_category: PiiCategorySchema,
});

export const ObjectNeighborSigSchema: z.ZodType<ObjectNeighborSig> = z.object({
  field_name: z.string().default(""),
  link_type: z.string().default(""),
  direction: LinkDirectionSchema,
  target_type: z.string().default(""),
  cardinality: CardinalitySchema,
});

export const ObjectTypeCatalogItemSchema: z.ZodType<ObjectTypeCatalogItem> =
  z.object({
    name: z.string().default(""),
    display_name: z.string().default(""),
    description: z.string().default(""),
    collection_path: z.string().default(""),
    item_path: z.string().default(""),
    graphql_field: z.string().default(""),
    graphql_list_field: z.string().default(""),
    fields: z.array(ObjectFieldSigSchema).default([]),
    neighbors: z.array(ObjectNeighborSigSchema).default([]),
    sample_graphql: z.string().default(""),
  });

export const ObjectCatalogResponseSchema: z.ZodType<ObjectCatalogResponse> =
  z.object({
    version: z.number().default(0),
    items: z.array(ObjectTypeCatalogItemSchema).default([]),
  });

export const DomainObjectRecordSchema: z.ZodType<DomainObjectRecord> = z.lazy(
  () =>
    z.object({
      id: z.string().default(""),
      type_name: z.string().default(""),
      node_key: z.string().default(""),
      title: z.string().default(""),
      attributes: z.record(z.string(), z.unknown()).default({}),
      metrics: z.record(z.string(), z.number()).default({}),
      geo: z
        .object({
          lng: z.number(),
          lat: z.number(),
        })
        .nullable()
        .optional(),
      updated_at: z.string().default(""),
      neighbors: z
        .record(z.string(), z.array(DomainObjectRecordSchema))
        .nullable()
        .optional(),
    })
);

export const DomainObjectListResponseSchema: z.ZodType<DomainObjectListResponse> =
  z.object({
    items: z.array(DomainObjectRecordSchema).default([]),
    next_cursor: z.string().nullable().default(null),
  });

export const GraphqlExecutionResponseSchema: z.ZodType<GraphqlExecutionResponse> =
  z.object({
    data: z.record(z.string(), z.unknown()).nullable().optional(),
    errors: z
      .array(
        z.object({
          message: z.string().default("GraphQL error"),
          locations: z
            .array(z.object({ line: z.number(), column: z.number() }))
            .optional(),
          path: z.array(z.union([z.string(), z.number()])).optional(),
        })
      )
      .optional(),
  });

function parseResilient<T>(schema: z.ZodType<T>, raw: unknown, fallback: T): T {
  const res = schema.safeParse(raw ?? {});
  return res.success ? res.data : fallback;
}

function parseListEnvelope<T>(schema: z.ZodType<T>, raw: unknown): T[] {
  const candidates: unknown[] = Array.isArray(raw)
    ? raw
    : typeof raw === "object" &&
        raw !== null &&
        "items" in raw &&
        Array.isArray((raw as { items?: unknown }).items)
      ? ((raw as { items: unknown[] }).items ?? [])
      : [];
  const out: T[] = [];
  for (const entry of candidates) {
    const parsed = schema.safeParse(entry);
    if (parsed.success) {
      out.push(parsed.data);
    }
  }
  return out;
}

function sanitizePropertyForWire(p: PropertySpec): PropertySpec {
  const out: PropertySpec = {
    name: p.name.trim(),
    display_name: p.display_name.trim(),
    description: p.description,
    data_type: normalizeStudioDataType(p.data_type),
    kind: p.kind,
    required: p.required,
    pii_category: normalizePiiCategory(p.pii_category),
  };
  if (p.unit && p.unit.trim().length > 0) {
    out.unit = p.unit.trim();
  }
  if (p.allowed_values && p.allowed_values.length > 0) {
    out.allowed_values = p.allowed_values;
  }
  return out;
}

/** Sanitizes `SchemaDoc` so optional UUIDs and empty optional strings strictly satisfy Rust serde contracts. */
export function sanitizeSchemaDocForWire(doc: SchemaDoc): SchemaDoc {
  return {
    node_types: doc.node_types.map((nt) => {
      const cleaned: NodeTypeSpec = {
        name: nt.name.trim(),
        display_name: nt.display_name.trim(),
        description: nt.description,
        icon: nt.icon || "box",
        color: nt.color || "#2563eb",
        title_template: nt.title_template || "{node_key}",
        properties: nt.properties.map(sanitizePropertyForWire),
        agg_rules: nt.agg_rules.map((r) => {
          const rule: AggRuleSpec = {
            target_metric: r.target_metric.trim(),
            op: normalizeAggFn(r.op),
            via_link_type: r.via_link_type.trim(),
            direction: r.direction === "IN" ? "IN" : "OUT",
            source_type_name: r.source_type_name.trim(),
            source_metric: r.source_metric.trim(),
            weight_by_link: Boolean(r.weight_by_link),
          };
          if (r.filter && r.filter.trim().length > 0) {
            rule.filter = r.filter.trim();
          }
          return rule;
        }),
        derived_rules: nt.derived_rules.map((d) => ({
          target_metric: d.target_metric.trim(),
          expression: d.expression,
          enabled: d.enabled !== false,
        })),
        sync_to_knowledge: Boolean(nt.sync_to_knowledge),
        pos_x: Number.isFinite(nt.pos_x) ? nt.pos_x : 120,
        pos_y: Number.isFinite(nt.pos_y) ? nt.pos_y : 120,
      };
      if (nt.id && UUID_REGEX.test(nt.id)) {
        cleaned.id = nt.id;
      }
      return cleaned;
    }),
    edge_types: doc.edge_types.map((et) => {
      const cleaned: EdgeTypeSpec = {
        name: et.name.trim(),
        src_type_name: et.src_type_name.trim(),
        dst_type_name: et.dst_type_name.trim(),
        impact_weight: Number.isFinite(et.impact_weight) ? et.impact_weight : 0.6,
        cardinality: normalizeCardinality(et.cardinality),
        description: et.description ?? "",
        properties: (et.properties ?? []).map(sanitizePropertyForWire),
        propagation: et.propagation ?? {},
      };
      if (et.id && UUID_REGEX.test(et.id)) {
        cleaned.id = et.id;
      }
      return cleaned;
    }),
    mappings: (doc.mappings ?? []).map((m) => {
      const cleaned: MappingSpec = {
        name: m.name.trim(),
        dataset: m.dataset.trim(),
        node_type: m.node_type.trim(),
        key_column: m.key_column.trim(),
        title: m.title || "{id}",
        attributes: m.attributes ?? {},
        metrics: m.metrics ?? {},
        geo: {
          lng_column:
            m.geo?.lng_column && m.geo.lng_column.trim().length > 0
              ? m.geo.lng_column.trim()
              : null,
          lat_column:
            m.geo?.lat_column && m.geo.lat_column.trim().length > 0
              ? m.geo.lat_column.trim()
              : null,
        },
        links: (m.links ?? []).map((lk) => {
          const link: MappingLinkSpec = {
            link_type: lk.link_type.trim(),
            direction: lk.direction === "IN" ? "IN" : "OUT",
            target_type: lk.target_type.trim(),
            target_key_column: lk.target_key_column.trim(),
            create_missing_target: Boolean(lk.create_missing_target),
          };
          if (lk.weight_column && lk.weight_column.trim().length > 0) {
            link.weight_column = lk.weight_column.trim();
          }
          return link;
        }),
        on_source_delete: m.on_source_delete === "DELETE" ? "DELETE" : "KEEP",
        mode: m.mode === "FULL" ? "FULL" : "INCREMENTAL",
      };
      if (m.filter && m.filter.trim().length > 0) {
        cleaned.filter = m.filter.trim();
      }
      if (m.metric_time_column && m.metric_time_column.trim().length > 0) {
        cleaned.metric_time_column = m.metric_time_column.trim();
      }
      return cleaned;
    }),
  };
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

function subgraphQs(subgraph?: string): string {
  if (!subgraph || subgraph.trim() === "" || subgraph.trim() === "main") {
    return subgraph === "main" ? "?subgraph=main" : "";
  }
  return buildQuery({ subgraph: subgraph.trim() });
}

async function fetchGraphqlSdlResilient(): Promise<GraphqlSdlResponse> {
  const headers = new Headers();
  const token = getAccessToken();
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  const resp = await fetch("/api/xblue/graphql/sdl", { headers });
  const rawText = await resp.text();
  if (!resp.ok) {
    const trimmed = rawText.trim();
    if (trimmed.length > 0) {
      try {
        const parsed: unknown = JSON.parse(trimmed);
        const validated = ApiErrorBodySchema.safeParse(parsed);
        if (validated.success) {
          throw new ApiError({
            status: resp.status,
            errorCode: validated.data.error_code,
            message: validated.data.message,
            traceId: validated.data.trace_id,
          });
        }
      } catch (err) {
        if (err instanceof ApiError) throw err;
      }
    }
    throw new ApiError({
      status: resp.status,
      errorCode: `HTTP_${resp.status}`,
      message: trimmed || resp.statusText || `HTTP ${resp.status}`,
      traceId: resp.headers.get("x-request-id") ?? "",
    });
  }

  const trimmed = rawText.trim();
  if (!trimmed) {
    return { version: 0, sdl: "" };
  }
  if (trimmed.startsWith("{") || trimmed.startsWith('"')) {
    try {
      const parsed: unknown = JSON.parse(trimmed);
      if (typeof parsed === "string") {
        return { version: 1, sdl: parsed };
      }
      if (typeof parsed === "object" && parsed !== null) {
        const rec = parsed as { version?: unknown; sdl?: unknown };
        return {
          version: typeof rec.version === "number" ? rec.version : 0,
          sdl: typeof rec.sdl === "string" ? rec.sdl : "",
        };
      }
    } catch {
      // Plain text SDL starting with `{` is handled below
    }
  }
  return { version: 1, sdl: rawText };
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

  // =========================================================================
  // Step 05: xBlueStudio Subgraphs, Schema Versions, Drafts, Mappings & Domain Objects
  // =========================================================================

  listSubgraphs: async (): Promise<SubgraphInfo[]> => {
    const raw = await requestJson<unknown>("/api/xblue/subgraphs");
    return parseListEnvelope(SubgraphInfoSchema, raw);
  },
  createSubgraph: async (
    payload: CreateSubgraphRequest
  ): Promise<SubgraphInfo> => {
    const body: CreateSubgraphRequest = {
      slug: payload.slug.trim(),
      display_name: payload.display_name.trim(),
    };
    if (payload.description !== undefined) {
      body.description = payload.description;
    }
    if (payload.owner_dept && payload.owner_dept.trim().length > 0) {
      body.owner_dept = payload.owner_dept.trim();
    }
    if (payload.seed_from_main !== undefined) {
      body.seed_from_main = payload.seed_from_main;
    }
    if (payload.seed_node_types && payload.seed_node_types.length > 0) {
      body.seed_node_types = payload.seed_node_types;
    }
    const raw = await requestJson<unknown>("/api/xblue/subgraphs", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    return parseResilient(SubgraphInfoSchema, raw, {
      id: "00000000-0000-0000-0000-000000000000",
      slug: body.slug,
      display_name: body.display_name,
      description: body.description ?? "",
      owner_dept: body.owner_dept ?? "default",
      is_main: false,
      current_version: 0,
      created_at: "",
      updated_at: "",
    });
  },
  updateSubgraph: async (
    slug: string,
    payload: UpdateSubgraphRequest
  ): Promise<SubgraphInfo> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/subgraphs/${encodeURIComponent(slug)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      }
    );
    return parseResilient(SubgraphInfoSchema, raw, {
      id: "00000000-0000-0000-0000-000000000000",
      slug,
      display_name: payload.display_name ?? slug,
      description: payload.description ?? "",
      owner_dept: payload.owner_dept ?? "default",
      is_main: slug === "main",
      current_version: 0,
      created_at: "",
      updated_at: "",
    });
  },
  deleteSubgraph: async (slug: string): Promise<{ deleted: boolean }> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/subgraphs/${encodeURIComponent(slug)}`,
      { method: "DELETE" }
    );
    const parsed = z
      .object({ deleted: z.boolean().default(true) })
      .safeParse(raw ?? {});
    return parsed.success ? parsed.data : { deleted: true };
  },
  previewSubgraphMerge: async (
    slug: string,
    payload?: MergePreviewRequest
  ): Promise<MergePreview> => {
    const body: MergePreviewRequest = {};
    if (payload?.resolutions && payload.resolutions.length > 0) {
      body.resolutions = payload.resolutions;
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/subgraphs/${encodeURIComponent(slug)}/merge-preview`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(MergePreviewSchema, raw, {
      subgraph_slug: slug,
      subgraph_version: 1,
      main_version: 1,
      alignments: [],
      conflicts: [],
      added_node_types: [],
      added_edge_types: [],
      added_mappings: [],
    });
  },
  mergeSubgraph: async (
    slug: string,
    payload: MergeSubgraphRequest
  ): Promise<PublishDraftResponse> => {
    const body: MergeSubgraphRequest = {
      default_strategy: payload.default_strategy,
      change_summary: payload.change_summary ?? "",
    };
    if (payload.base_main_version !== undefined) {
      body.base_main_version = payload.base_main_version;
    }
    if (payload.resolutions && payload.resolutions.length > 0) {
      body.resolutions = payload.resolutions;
    }
    if (payload.force !== undefined) {
      body.force = payload.force;
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/subgraphs/${encodeURIComponent(slug)}/merge`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(PublishDraftResponseSchema, raw, {
      subgraph_slug: "main",
      version: 1,
      draft_id: "",
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      diff: {
        node_types: EMPTY_SECTION_DIFF,
        edge_types: EMPTY_SECTION_DIFF,
        mappings: EMPTY_SECTION_DIFF,
      },
      mapping_jobs: [],
      aggregate_jobs: [],
    });
  },

  getPublishedSchema: async (
    subgraph?: string
  ): Promise<PublishedSchemaResponse> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/schema${subgraphQs(subgraph)}`
    );
    return parseResilient(PublishedSchemaResponseSchema, raw, {
      subgraph_slug: subgraph ?? "main",
      version: 1,
      node_types: [],
      edge_types: [],
      mappings: [],
    });
  },
  saveSchemaDirect: async (
    payload: SaveSchemaDirectRequest,
    subgraph?: string
  ): Promise<PublishedSchemaResponse> => {
    const sanitized = sanitizeSchemaDocForWire(payload);
    const body: SaveSchemaDirectRequest = {
      ...sanitized,
    };
    if (payload.base_version !== undefined) {
      body.base_version = payload.base_version;
    }
    if (payload.change_summary !== undefined) {
      body.change_summary = payload.change_summary;
    }
    if (payload.force !== undefined) {
      body.force = payload.force;
    }
    if (payload.cascade !== undefined) {
      body.cascade = payload.cascade;
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/schema${subgraphQs(subgraph)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(PublishedSchemaResponseSchema, raw, {
      subgraph_slug: subgraph ?? "main",
      version: 1,
      ...sanitized,
    });
  },
  validateSchema: async (
    doc: SchemaDoc,
    subgraph?: string
  ): Promise<ValidateSchemaResponse> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/schema/validate${subgraphQs(subgraph)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(sanitizeSchemaDocForWire(doc)),
      }
    );
    return parseResilient(ValidateSchemaResponseSchema, raw, {
      valid: true,
      issues: [],
    });
  },

  listSchemaVersions: async (
    subgraph?: string
  ): Promise<SchemaVersionSummary[]> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/schema/versions${subgraphQs(subgraph)}`
    );
    return parseListEnvelope(SchemaVersionSummarySchema, raw);
  },
  getSchemaVersion: async (
    v: number,
    subgraph?: string
  ): Promise<SchemaVersionDetail> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/schema/versions/${encodeURIComponent(String(v))}${subgraphQs(subgraph)}`
    );
    return parseResilient(SchemaVersionDetailSchema, raw, {
      id: "",
      subgraph_slug: subgraph ?? "main",
      version: v,
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      diff: {
        node_types: EMPTY_SECTION_DIFF,
        edge_types: EMPTY_SECTION_DIFF,
        mappings: EMPTY_SECTION_DIFF,
      },
      change_summary: "",
      published_at: "",
    });
  },
  diffSchemaVersions: async (
    a: number,
    b: number,
    subgraph?: string
  ): Promise<SchemaDiff> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/schema/versions/${encodeURIComponent(String(a))}/diff/${encodeURIComponent(String(b))}${subgraphQs(subgraph)}`
    );
    return parseResilient(SchemaDiffSchema, raw, {
      node_types: EMPTY_SECTION_DIFF,
      edge_types: EMPTY_SECTION_DIFF,
      mappings: EMPTY_SECTION_DIFF,
    });
  },
  restoreSchemaVersion: async (
    v: number,
    subgraph?: string
  ): Promise<SchemaDraft> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/schema/versions/${encodeURIComponent(String(v))}/restore${subgraphQs(subgraph)}`,
      { method: "POST" }
    );
    return parseResilient(SchemaDraftSchema, raw, {
      id: "",
      subgraph_slug: subgraph ?? "main",
      title: `恢复自 v${v}`,
      source: "RESTORE",
      base_version: v,
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      ai_context: {},
      issues: [],
      status: "DRAFT",
      created_at: "",
      updated_at: "",
    });
  },

  listDrafts: async (subgraph?: string): Promise<SchemaDraft[]> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts${subgraphQs(subgraph)}`
    );
    return parseListEnvelope(SchemaDraftSchema, raw);
  },
  createDraft: async (
    payload: CreateDraftRequest,
    subgraph?: string
  ): Promise<SchemaDraft> => {
    const sanitized = sanitizeSchemaDocForWire(payload.schema_doc);
    const body: CreateDraftRequest = {
      title: payload.title.trim() || "未命名草案",
      schema_doc: sanitized,
    };
    if (payload.base_version !== undefined) {
      body.base_version = payload.base_version;
    }
    if (payload.source !== undefined) {
      body.source = payload.source;
    }
    if (payload.ai_context !== undefined) {
      body.ai_context = payload.ai_context;
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts${subgraphQs(subgraph)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(SchemaDraftSchema, raw, {
      id: "",
      subgraph_slug: subgraph ?? "main",
      title: body.title,
      source: body.source ?? "MANUAL",
      base_version: body.base_version ?? 1,
      schema_doc: sanitized,
      ai_context: body.ai_context ?? {},
      issues: [],
      status: "DRAFT",
      created_at: "",
      updated_at: "",
    });
  },
  getDraft: async (id: string): Promise<SchemaDraft> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts/${encodeURIComponent(id)}`
    );
    return parseResilient(SchemaDraftSchema, raw, {
      id,
      subgraph_slug: "main",
      title: "未命名草案",
      source: "MANUAL",
      base_version: 1,
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      ai_context: {},
      issues: [],
      status: "DRAFT",
      created_at: "",
      updated_at: "",
    });
  },
  updateDraft: async (
    id: string,
    payload: UpdateDraftRequest
  ): Promise<SchemaDraft> => {
    const sanitized = sanitizeSchemaDocForWire(payload.schema_doc);
    const body: UpdateDraftRequest = {
      schema_doc: sanitized,
    };
    if (payload.title !== undefined) {
      body.title = payload.title;
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts/${encodeURIComponent(id)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(SchemaDraftSchema, raw, {
      id,
      subgraph_slug: "main",
      title: payload.title ?? "未命名草案",
      source: "MANUAL",
      base_version: 1,
      schema_doc: sanitized,
      ai_context: {},
      issues: [],
      status: "DRAFT",
      created_at: "",
      updated_at: "",
    });
  },
  discardDraft: async (id: string): Promise<SchemaDraft> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts/${encodeURIComponent(id)}/discard`,
      { method: "POST" }
    );
    return parseResilient(SchemaDraftSchema, raw, {
      id,
      subgraph_slug: "main",
      title: "已丢弃草案",
      source: "MANUAL",
      base_version: 1,
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      ai_context: {},
      issues: [],
      status: "DISCARDED",
      created_at: "",
      updated_at: "",
    });
  },
  createAiDraft: async (
    payload: AiDraftRequest,
    subgraph?: string
  ): Promise<SchemaDraft> => {
    const body: AiDraftRequest = {
      title: payload.title.trim() || "AI 智能建模草案",
    };
    if (payload.dataset_ids && payload.dataset_ids.length > 0) {
      body.dataset_ids = payload.dataset_ids;
    }
    if (payload.doc_ids && payload.doc_ids.length > 0) {
      body.doc_ids = payload.doc_ids;
    }
    if (payload.description && payload.description.trim().length > 0) {
      body.description = payload.description.trim();
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts/ai${subgraphQs(subgraph)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(SchemaDraftSchema, raw, {
      id: "",
      subgraph_slug: subgraph ?? "main",
      title: body.title,
      source: "AI",
      base_version: 1,
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      ai_context: {},
      issues: [],
      status: "DRAFT",
      created_at: "",
      updated_at: "",
    });
  },
  publishDraft: async (
    id: string,
    payload?: PublishDraftRequest
  ): Promise<PublishDraftResponse> => {
    const body: PublishDraftRequest = {
      change_summary: payload?.change_summary ?? "",
    };
    if (payload?.force !== undefined) {
      body.force = payload.force;
    }
    if (payload?.cascade !== undefined) {
      body.cascade = payload.cascade;
    }
    const raw = await requestJson<unknown>(
      `/api/xblue/drafts/${encodeURIComponent(id)}/publish`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    );
    return parseResilient(PublishDraftResponseSchema, raw, {
      subgraph_slug: "main",
      version: 1,
      draft_id: id,
      schema_doc: { node_types: [], edge_types: [], mappings: [] },
      diff: {
        node_types: EMPTY_SECTION_DIFF,
        edge_types: EMPTY_SECTION_DIFF,
        mappings: EMPTY_SECTION_DIFF,
      },
      mapping_jobs: [],
      aggregate_jobs: [],
    });
  },

  listMappings: async (subgraph?: string): Promise<MappingRecord[]> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/mappings${subgraphQs(subgraph)}`
    );
    return parseListEnvelope(MappingRecordSchema, raw);
  },
  listMappingRuns: async (id: string): Promise<MappingRunRecord[]> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/mappings/${encodeURIComponent(id)}/runs`
    );
    return parseListEnvelope(MappingRunRecordSchema, raw);
  },
  runMapping: async (
    id: string,
    mode: MappingMode = "INCREMENTAL"
  ): Promise<TriggerMappingRunResponse> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/mappings/${encodeURIComponent(id)}/run`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ mode }),
      }
    );
    return parseResilient(TriggerMappingRunResponseSchema, raw, {
      job_id: "",
      mapping_id: id,
      mode,
    });
  },
  previewMapping: async (id: string): Promise<MappingPreviewResponse> => {
    const raw = await requestJson<unknown>(
      `/api/xblue/mappings/${encodeURIComponent(id)}/preview`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      }
    );
    return parseResilient(MappingPreviewResponseSchema, raw, {
      mapping_id: id,
      sampled_rows: 0,
      items: [],
      rows_rejected: 0,
      reject_samples: [],
    });
  },

  listDomainObjects: async (): Promise<ObjectCatalogResponse> => {
    const raw = await requestJson<unknown>("/api/xblue/objects");
    return parseResilient(ObjectCatalogResponseSchema, raw, {
      version: 0,
      items: [],
    });
  },
  queryDomainObjects: async (
    typeName: string,
    params?: DomainObjectListQuery
  ): Promise<DomainObjectListResponse> => {
    const qs = buildQuery({
      q: params?.q,
      filter: params?.filter,
      limit: params?.limit,
      cursor: params?.cursor,
    });
    const raw = await requestJson<unknown>(
      `/api/xblue/objects/${encodeURIComponent(typeName)}${qs}`
    );
    return parseResilient(DomainObjectListResponseSchema, raw, {
      items: [],
      next_cursor: null,
    });
  },
  getDomainObject: async (
    typeName: string,
    nodeKey: string,
    includeNeighbors?: boolean
  ): Promise<DomainObjectRecord> => {
    const qs =
      includeNeighbors !== undefined
        ? buildQuery({ include_neighbors: includeNeighbors })
        : "";
    const raw = await requestJson<unknown>(
      `/api/xblue/objects/${encodeURIComponent(typeName)}/${encodeURIComponent(nodeKey)}${qs}`
    );
    return parseResilient(DomainObjectRecordSchema, raw, {
      id: "",
      type_name: typeName,
      node_key: nodeKey,
      title: nodeKey,
      attributes: {},
      metrics: {},
      updated_at: "",
    });
  },
  getGraphqlSdl: (): Promise<GraphqlSdlResponse> => fetchGraphqlSdlResilient(),
  executeGraphql: async (
    query: string,
    variables?: Record<string, unknown>
  ): Promise<GraphqlExecutionResponse> => {
    const body: { query: string; variables?: Record<string, unknown> } = {
      query,
    };
    if (variables && Object.keys(variables).length > 0) {
      body.variables = variables;
    }
    const raw = await requestJson<unknown>("/api/xblue/graphql", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    return parseResilient(GraphqlExecutionResponseSchema, raw, {});
  },
};
