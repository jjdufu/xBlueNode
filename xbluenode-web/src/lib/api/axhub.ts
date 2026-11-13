import { requestJson } from "./client";
import type {
  ConnectorType,
  DataType,
  JobStatus,
  PiiCategory,
  QualityRuleKind,
  SourceKind,
  SyncMode,
} from "./enums";

export type SchemaPolicy = "STRICT" | "ADDITIVE" | "EVOLVE";
export type ConnectionStatus = "UNTESTED" | "OK" | "FAILED";
export type DatasetStatus = "DRAFT" | "READY" | "SYNCING" | "FAILED" | "DISABLED";
export type UploadStatus = "PENDING" | "PARSED" | "COMMITTED" | "FAILED";
export type WebhookAuthScheme = "HMAC_SHA256" | "BEARER";
export type QualityOnFail = "WARN" | "BLOCK";

export interface ColumnProfile {
  name: string;
  ordinal: number;
  data_type: DataType;
  nullable: boolean;
  null_count: number;
  null_ratio: number;
  distinct_estimate: number;
  sample_values: string[];
  pii_category: PiiCategory;
  pii_hit_ratio: number;
  source_comment: string;
}

export interface ConnectorSpec {
  connector_type: ConnectorType;
  display_name: string;
  category: string;
  config_schema: Record<string, unknown>;
  secret_fields: string[];
  supports_modes: SyncMode[];
  supports_discover: boolean;
  image_tier: "lite" | "full";
  available: boolean;
}

export interface Connection {
  id: string;
  name: string;
  connector_type: ConnectorType;
  config: Record<string, unknown>;
  secret_fields_set: string[];
  allow_private_network: boolean;
  status: ConnectionStatus;
  last_test_message: string;
  last_tested_at: string | null;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface ConnectionTestResult {
  ok: boolean;
  message: string;
  latency_ms: number;
}

export interface DiscoveredColumn {
  name: string;
  native_type: string;
  data_type: DataType;
  nullable: boolean;
}

export interface DiscoveredObject {
  ref: Record<string, unknown>;
  display_name: string;
  source_kind: SourceKind;
  columns: DiscoveredColumn[];
  primary_key: string[];
  suggested_cursor: string | null;
  estimated_rows: number | null;
}

export interface PreviewResponse {
  columns: ColumnProfile[];
  rows: Record<string, unknown>[];
}

export interface Dataset {
  id: string;
  connection_id: string | null;
  name: string;
  display_name: string;
  description: string;
  source_kind: SourceKind;
  source_ref: Record<string, unknown>;
  sync_mode: SyncMode;
  primary_key: string[];
  cursor_column: string | null;
  cursor_value: string | null;
  delete_column: string | null;
  schema_policy: SchemaPolicy;
  lake_schema: string;
  lake_table: string;
  schema_json: ColumnProfile[];
  schema_version: number;
  row_count: number;
  last_snapshot_id: number | null;
  last_synced_at: string | null;
  schedule_cron: string | null;
  tags: string[];
  owner_user_id: string | null;
  status: DatasetStatus;
  created_at: string;
  updated_at: string;
}

export interface PaginatedList<T> {
  items: T[];
  next_cursor: string | null;
}

export interface ColumnMeta {
  dataset_id: string;
  column_name: string;
  data_type: DataType;
  pii_category: PiiCategory;
  pii_confirmed: boolean;
  description: string;
  business_term: string;
  unit: string;
  ordinal: number;
}

export interface QualityRule {
  id: string;
  dataset_id: string;
  rule_kind: QualityRuleKind;
  column_name: string | null;
  params: Record<string, unknown>;
  on_fail: QualityOnFail;
  created_at: string;
}

export interface QualityRuleOutcome {
  rule_id: string;
  rule_kind: string;
  column_name: string | null;
  passed: boolean;
  on_fail: QualityOnFail;
  detail: string;
}

export interface QualityReport {
  passed: boolean;
  blocked: boolean;
  rules: QualityRuleOutcome[];
}

export interface SchemaChange {
  added: ColumnProfile[];
  removed: string[];
  widened: [string, string, string][];
  narrowed: [string, string, string][];
  breaking: boolean;
}

export interface SyncJob {
  id: string;
  dataset_id: string;
  job_id: string;
  batch_id: string;
  sync_mode: SyncMode;
  status: JobStatus;
  cursor_from: string | null;
  cursor_to: string | null;
  rows_read: number;
  rows_written: number;
  rows_deleted: number;
  snapshot_id: number | null;
  schema_change: SchemaChange;
  quality_result: QualityReport;
  error_code: string | null;
  error_message: string | null;
  started_at: string | null;
  finished_at: string | null;
  created_at: string;
}

export interface LakeSnapshotInfo {
  snapshot_id: number;
  committed_at: string;
  rows_added: number;
  rows_deleted: number;
}

export interface UploadRecord {
  id: string;
  filename: string;
  sha256: string;
  size_bytes: number;
  parse_id: string | null;
  doc_id: string | null;
  dataset_ids: string[];
  status: UploadStatus;
  error_code: string | null;
  created_by: string | null;
  created_at: string;
}

export interface EnqueueUploadResponse {
  upload_id: string;
  job_id: string | null;
  deduplicated: boolean;
}

export interface WebhookConfig {
  id: string;
  dataset_id: string;
  slug: string;
  auth_scheme: WebhookAuthScheme;
  record_path: string;
  max_body_bytes: number;
  enabled: boolean;
  created_at: string;
}

export interface CreateWebhookResponse {
  webhook: WebhookConfig;
  dataset_id: string;
  url: string;
  secret: string;
}

export interface GlossaryTerm {
  id: string;
  term: string;
  definition: string;
  synonyms: string[];
  created_at: string;
}

export interface CatalogSearchHit {
  id: string;
  name: string;
  display_name: string;
  description: string;
  source_kind: SourceKind;
  row_count: number;
  tags: string[];
  matched_columns: string[];
}

export interface LineageEdge {
  id: number;
  src_kind: string;
  src_id: string;
  dst_kind: string;
  dst_id: string;
  via: string;
  created_at: string;
}

export const axhubApi = {
  listConnectorTypes: () =>
    requestJson<ConnectorSpec[]>("/api/axhub/connector-types"),

  listConnections: () =>
    requestJson<PaginatedList<Connection>>("/api/axhub/connections"),

  getConnection: (id: string) =>
    requestJson<Connection>(
      `/api/axhub/connections/${encodeURIComponent(id)}`
    ),

  createConnection: (payload: {
    name: string;
    connector_type: ConnectorType;
    config?: Record<string, unknown>;
    secrets?: Record<string, string>;
    allow_private_network?: boolean;
  }) =>
    requestJson<Connection>("/api/axhub/connections", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),

  updateConnection: (
    id: string,
    payload: {
      name?: string;
      config?: Record<string, unknown>;
      secrets?: Record<string, string | null>;
      allow_private_network?: boolean;
    }
  ) =>
    requestJson<Connection>(
      `/api/axhub/connections/${encodeURIComponent(id)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      }
    ),

  deleteConnection: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axhub/connections/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),

  testConnection: (id: string) =>
    requestJson<ConnectionTestResult>(
      `/api/axhub/connections/${encodeURIComponent(id)}/test`,
      { method: "POST" }
    ),

  discoverConnection: (id: string) =>
    requestJson<DiscoveredObject[]>(
      `/api/axhub/connections/${encodeURIComponent(id)}/discover`
    ),

  previewConnection: (
    id: string,
    payload: { ref: Record<string, unknown>; limit?: number }
  ) =>
    requestJson<PreviewResponse>(
      `/api/axhub/connections/${encodeURIComponent(id)}/preview`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      }
    ),

  rotateCredentials: () =>
    requestJson<{ job_id: string }>("/api/axhub/credentials/rotate", {
      method: "POST",
    }),

  listDatasets: (params?: {
    q?: string;
    tag?: string;
    source_kind?: SourceKind;
    limit?: number;
    cursor?: string;
  }) => {
    const search = new URLSearchParams();
    if (params?.q) search.set("q", params.q);
    if (params?.tag) search.set("tag", params.tag);
    if (params?.source_kind) search.set("source_kind", params.source_kind);
    if (params?.limit) search.set("limit", String(params.limit));
    if (params?.cursor) search.set("cursor", params.cursor);
    const qs = search.toString();
    return requestJson<PaginatedList<Dataset>>(
      `/api/axhub/datasets${qs ? `?${qs}` : ""}`
    );
  },

  getDataset: (id: string) =>
    requestJson<Dataset>(`/api/axhub/datasets/${encodeURIComponent(id)}`),

  createDataset: (payload: {
    connection_id?: string | null;
    name: string;
    display_name?: string;
    description?: string;
    source_kind: SourceKind;
    source_ref?: Record<string, unknown>;
    sync_mode?: SyncMode;
    primary_key?: string[];
    cursor_column?: string | null;
    delete_column?: string | null;
    schema_policy?: SchemaPolicy;
    schedule_cron?: string | null;
    tags?: string[];
  }) =>
    requestJson<Dataset>("/api/axhub/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),

  updateDataset: (
    id: string,
    payload: {
      display_name?: string;
      description?: string;
      sync_mode?: SyncMode;
      primary_key?: string[];
      cursor_column?: string | null;
      delete_column?: string | null;
      schema_policy?: SchemaPolicy;
      schedule_cron?: string | null;
      tags?: string[];
      status?: DatasetStatus;
    }
  ) =>
    requestJson<Dataset>(`/api/axhub/datasets/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),

  deleteDataset: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axhub/datasets/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),

  syncDataset: (id: string) =>
    requestJson<{ job_id: string; existing?: boolean }>(
      `/api/axhub/datasets/${encodeURIComponent(id)}/sync`,
      { method: "POST" }
    ),

  listSyncJobs: (id: string, limit = 20) =>
    requestJson<PaginatedList<SyncJob>>(
      `/api/axhub/datasets/${encodeURIComponent(id)}/sync-jobs?limit=${limit}`
    ),

  previewDataset: (id: string, limit = 100, snapshotId?: number) => {
    const search = new URLSearchParams({ limit: String(limit) });
    if (snapshotId !== undefined) {
      search.set("snapshot_id", String(snapshotId));
    }
    return requestJson<PreviewResponse>(
      `/api/axhub/datasets/${encodeURIComponent(id)}/preview?${search.toString()}`
    );
  },

  listSnapshots: (id: string) =>
    requestJson<LakeSnapshotInfo[]>(
      `/api/axhub/datasets/${encodeURIComponent(id)}/snapshots`
    ),

  listColumns: (id: string) =>
    requestJson<ColumnMeta[]>(
      `/api/axhub/datasets/${encodeURIComponent(id)}/columns`
    ),

  updateColumnMeta: (
    datasetId: string,
    columnName: string,
    payload: {
      pii_category?: PiiCategory;
      pii_confirmed?: boolean;
      description?: string;
      business_term?: string;
      unit?: string;
    }
  ) =>
    requestJson<ColumnMeta>(
      `/api/axhub/datasets/${encodeURIComponent(datasetId)}/columns/${encodeURIComponent(columnName)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      }
    ),

  listQualityRules: (datasetId: string) =>
    requestJson<QualityRule[]>(
      `/api/axhub/datasets/${encodeURIComponent(datasetId)}/quality-rules`
    ),

  createQualityRule: (
    datasetId: string,
    payload: {
      rule_kind: QualityRuleKind;
      column_name?: string | null;
      params?: Record<string, unknown>;
      on_fail?: QualityOnFail;
    }
  ) =>
    requestJson<QualityRule>(
      `/api/axhub/datasets/${encodeURIComponent(datasetId)}/quality-rules`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      }
    ),

  deleteQualityRule: (ruleId: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axhub/quality-rules/${encodeURIComponent(ruleId)}`,
      { method: "DELETE" }
    ),

  getLineage: (datasetId: string, depth = 3) =>
    requestJson<{ edges: LineageEdge[] }>(
      `/api/axhub/datasets/${encodeURIComponent(datasetId)}/lineage?depth=${depth}`
    ),

  uploadFile: (formData: FormData) =>
    requestJson<EnqueueUploadResponse>("/api/axhub/uploads", {
      method: "POST",
      body: formData,
    }),

  getUpload: (id: string) =>
    requestJson<UploadRecord>(`/api/axhub/uploads/${encodeURIComponent(id)}`),

  listWebhooks: () =>
    requestJson<PaginatedList<WebhookConfig>>("/api/axhub/webhooks"),

  createWebhook: (payload: {
    dataset_name: string;
    slug: string;
    auth_scheme?: WebhookAuthScheme;
    record_path?: string;
    max_body_bytes?: number;
  }) =>
    requestJson<CreateWebhookResponse>("/api/axhub/webhooks", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),

  deleteWebhook: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axhub/webhooks/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),

  searchCatalog: (q: string, limit = 50) =>
    requestJson<PaginatedList<CatalogSearchHit>>(
      `/api/axhub/catalog/search?q=${encodeURIComponent(q)}&limit=${limit}`
    ),

  listGlossary: () =>
    requestJson<PaginatedList<GlossaryTerm>>("/api/axhub/glossary"),

  createGlossaryTerm: (payload: {
    term: string;
    definition?: string;
    synonyms?: string[];
  }) =>
    requestJson<GlossaryTerm>("/api/axhub/glossary", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }),

  deleteGlossaryTerm: (id: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/axhub/glossary/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    ),
};
