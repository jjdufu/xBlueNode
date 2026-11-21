import { requestJson } from "./client";
import type { JobKind, JobStatus, RetentionTarget, SyncMode } from "./enums";

export type WorkItemKind = "TASK" | "ISSUE" | "CHANGE";
export type WorkItemStatus = "OPEN" | "IN_PROGRESS" | "BLOCKED" | "DONE" | "CANCELED";
export type WorkItemPriority = "P0" | "P1" | "P2" | "P3";
export type WorkItemSourceKind =
  | "MANUAL"
  | "FLOW_ACTION"
  | "ALERT"
  | "SIM_DECISION"
  | "TEMPLATE";
export type WorkItemEventKind = "CREATED" | "STATUS" | "ASSIGN" | "COMMENT" | "LINK";

export type TemplateInstallStatus =
  | "PLANNED"
  | "INSTALLING"
  | "INSTALLED"
  | "FAILED"
  | "UNINSTALLED";
export type TemplateConflictStrategy = "SKIP" | "RENAME";

export type BackupKind = "FULL" | "DATABASE" | "LAKE";
export type BackupStatus = "PENDING" | "RUNNING" | "SUCCEEDED" | "FAILED" | "VERIFIED";
export type DemoScenario =
  | "supply_chain"
  | "manufacturing_equipment"
  | "retail_store";

export interface OpsJobRecord {
  id: string;
  kind: JobKind;
  status: JobStatus;
  attempt: number;
  max_attempts: number;
  next_run_at: string;
  last_error: string | null;
  result: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
}

export interface OpsJobListResponse {
  items: OpsJobRecord[];
  next_cursor?: string | null;
}

export interface DatabaseHealth {
  ok: boolean;
  latency_ms: number;
  replication_lag_s: number | null;
  error?: string;
}

export interface ParserHealth {
  ok: boolean;
  version: string;
  error?: string;
}

export interface LakeHealth {
  ok: boolean;
  snapshot_id: number;
  error?: string;
}

export interface LlmHealth {
  ok: boolean;
  provider: string;
  error?: string;
}

export interface EmbedHealth {
  ok: boolean;
  error?: string;
}

export interface DiskHealth {
  ok: boolean;
  data_root_free_bytes: number;
}

export interface JobsHealthStats {
  pending: number;
  running: number;
  failed_24h: number;
  oldest_pending_s: number;
}

export interface OutboxStats {
  lag_s: number;
  pending: number;
}

export interface MigrationsHealth {
  ok: boolean;
  current: number;
  pending: number;
}

export interface DeepHealthReport {
  database: DatabaseHealth;
  parser: ParserHealth;
  lake: LakeHealth;
  llm: LlmHealth;
  embed: EmbedHealth;
  disk: DiskHealth;
  jobs: JobsHealthStats;
  outbox: OutboxStats;
  migrations: MigrationsHealth;
  version: string;
}

export interface SystemInfoResponse {
  version: string;
  env: string;
  tenant_id: string;
  uptime_s: number;
  data_root: string;
  template_packs_count: number;
}

export interface RetentionPolicyRecord {
  target: RetentionTarget;
  keep_days: number;
}

export interface RetentionListResponse {
  items: RetentionPolicyRecord[];
}

export interface BackupRecord {
  id: string;
  backup_kind: BackupKind;
  status: BackupStatus;
  location: string;
  size_bytes: number | null;
  manifest: Record<string, unknown>;
  error_message: string | null;
  started_at: string;
  finished_at: string | null;
  verified_at: string | null;
}

export interface BackupListResponse {
  items: BackupRecord[];
  next_cursor?: string | null;
}

export interface TemplateDatasetEntry {
  key: string;
  name?: string;
  file: string;
  sync_mode: SyncMode;
  primary_key?: string[];
}

export interface TemplateContents {
  schema: string;
  datasets: TemplateDatasetEntry[];
  mappings?: string | null;
  metrics?: string | null;
  rules?: string | null;
  flows: string[];
  dashboards: string[];
  agents: string[];
  export_templates?: string[];
}

export interface TemplateManifest {
  id: string;
  name: string;
  version: string;
  industry: string;
  description: string;
  contents: TemplateContents;
  sha256: Record<string, string>;
}

export interface TemplatePackDetail extends TemplateManifest {
  readme: string;
}

export interface TemplatePlannedObject {
  category: string;
  key: string;
  source_name: string;
  target_name: string;
  conflict: boolean;
  action: string;
}

export interface TemplateInstallPlan {
  pack_id: string;
  pack_version: string;
  on_conflict: TemplateConflictStrategy;
  objects: TemplatePlannedObject[];
  conflicts: string[];
}

export interface TemplateCreatedObject {
  id: string;
  category: string;
  target_name: string;
  content_hash: string;
}

export interface TemplateInstallRecord {
  id: string;
  tenant_id: string;
  pack_id: string;
  pack_version: string;
  status: TemplateInstallStatus;
  plan: TemplateInstallPlan;
  object_map: Record<string, TemplateCreatedObject>;
  error_message: string | null;
  installed_by: string | null;
  created_at: string;
  finished_at: string | null;
}

export interface TemplateUninstallResponse {
  id: string;
  status: TemplateInstallStatus;
  deleted: string[];
  kept: string[];
}

export interface DemoLoadResponse {
  scenario: DemoScenario;
  install_id: string;
  flow_run_id: string | null;
  metric_points_written: number;
  sandbox_id: string | null;
  scenario_id: string | null;
  dashboard_slug: string;
}

export interface WorkItem {
  id: string;
  tenant_id: string;
  item_no: number;
  code: string;
  title: string;
  description: string;
  item_kind: WorkItemKind;
  status: WorkItemStatus;
  priority: WorkItemPriority;
  assignee: string | null;
  source_kind: WorkItemSourceKind;
  source_ref: string;
  node_ids: string[];
  due_at: string | null;
  created_by: string | null;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
}

export interface WorkItemEvent {
  id: number;
  tenant_id: string;
  work_item_id: string;
  event_kind: WorkItemEventKind;
  detail: Record<string, unknown>;
  actor: string | null;
  created_at: string;
}

export interface WorkItemDetail extends WorkItem {
  events: WorkItemEvent[];
}

export interface WorkItemListResponse {
  items: WorkItem[];
  next_cursor?: string | null;
}

export interface CreateWorkItemPayload {
  title: string;
  description?: string;
  item_kind?: WorkItemKind;
  priority?: WorkItemPriority;
  assignee?: string | null;
  source_kind?: WorkItemSourceKind;
  source_ref?: string;
  node_ids?: string[];
  due_at?: string | null;
}

export interface PatchWorkItemPayload {
  title?: string;
  description?: string;
  item_kind?: WorkItemKind;
  status?: WorkItemStatus;
  priority?: WorkItemPriority;
  assignee?: string | null;
  clear_assignee?: boolean;
  node_ids?: string[];
  due_at?: string | null;
}

function buildQuery(params: Record<string, string | number | undefined | null>): string {
  const sp = new URLSearchParams();
  for (const [key, val] of Object.entries(params)) {
    if (val !== undefined && val !== null && val !== "") {
      sp.set(key, String(val));
    }
  }
  const qs = sp.toString();
  return qs ? `?${qs}` : "";
}

export const axopsApi = {
  listJobs(query?: {
    kind?: JobKind;
    status?: JobStatus;
    limit?: number;
    cursor?: string;
  }): Promise<OpsJobListResponse> {
    return requestJson<OpsJobListResponse>(
      `/api/axops/jobs${buildQuery(query ?? {})}`
    );
  },

  getJob(id: string): Promise<OpsJobRecord> {
    return requestJson<OpsJobRecord>(`/api/axops/jobs/${encodeURIComponent(id)}`);
  },

  retryJob(id: string): Promise<OpsJobRecord> {
    return requestJson<OpsJobRecord>(
      `/api/axops/jobs/${encodeURIComponent(id)}/retry`,
      { method: "POST", body: JSON.stringify({}) }
    );
  },

  cancelJob(id: string): Promise<OpsJobRecord> {
    return requestJson<OpsJobRecord>(
      `/api/axops/jobs/${encodeURIComponent(id)}/cancel`,
      { method: "POST", body: JSON.stringify({}) }
    );
  },

  triggerRollupNow(): Promise<{ job_id: string }> {
    return requestJson<{ job_id: string }>("/api/axops/jobs/rollup-now", {
      method: "POST",
      body: JSON.stringify({}),
    });
  },

  getDeepHealth(): Promise<DeepHealthReport> {
    return requestJson<DeepHealthReport>("/api/axops/health/deep");
  },

  getOutboxStats(): Promise<OutboxStats> {
    return requestJson<OutboxStats>("/api/axops/outbox/stats");
  },

  getSystemInfo(): Promise<SystemInfoResponse> {
    return requestJson<SystemInfoResponse>("/api/axops/system/info");
  },

  listRetentionPolicies(): Promise<RetentionListResponse> {
    return requestJson<RetentionListResponse>("/api/axops/retention");
  },

  updateRetentionPolicy(
    target: RetentionTarget,
    keepDays: number
  ): Promise<RetentionPolicyRecord> {
    return requestJson<RetentionPolicyRecord>(
      `/api/axops/retention/${encodeURIComponent(target)}`,
      {
        method: "PUT",
        body: JSON.stringify({ keep_days: keepDays }),
      }
    );
  },

  runRetentionNow(): Promise<{ job_id: string }> {
    return requestJson<{ job_id: string }>("/api/axops/retention/run-now", {
      method: "POST",
      body: JSON.stringify({}),
    });
  },

  listBackups(query?: { limit?: number; cursor?: string }): Promise<BackupListResponse> {
    return requestJson<BackupListResponse>(
      `/api/axops/backups${buildQuery(query ?? {})}`
    );
  },

  createBackup(backupKind: BackupKind = "FULL"): Promise<BackupRecord> {
    return requestJson<BackupRecord>("/api/axops/backups", {
      method: "POST",
      body: JSON.stringify({ backup_kind: backupKind }),
    });
  },

  getBackup(id: string): Promise<BackupRecord> {
    return requestJson<BackupRecord>(
      `/api/axops/backups/${encodeURIComponent(id)}`
    );
  },

  listTemplates(): Promise<{ items: TemplateManifest[] }> {
    return requestJson<{ items: TemplateManifest[] }>("/api/axops/templates");
  },

  getTemplate(packId: string): Promise<TemplatePackDetail> {
    return requestJson<TemplatePackDetail>(
      `/api/axops/templates/${encodeURIComponent(packId)}`
    );
  },

  planTemplateInstall(
    packId: string,
    onConflict: TemplateConflictStrategy = "RENAME"
  ): Promise<TemplateInstallRecord> {
    return requestJson<TemplateInstallRecord>(
      `/api/axops/templates/${encodeURIComponent(packId)}/plan`,
      {
        method: "POST",
        body: JSON.stringify({ on_conflict: onConflict }),
      }
    );
  },

  listTemplateInstalls(): Promise<{ items: TemplateInstallRecord[] }> {
    return requestJson<{ items: TemplateInstallRecord[] }>(
      "/api/axops/templates/installs"
    );
  },

  applyTemplateInstall(installId: string): Promise<TemplateInstallRecord> {
    return requestJson<TemplateInstallRecord>(
      `/api/axops/templates/installs/${encodeURIComponent(installId)}/apply`,
      { method: "POST", body: JSON.stringify({}) }
    );
  },

  uninstallTemplate(installId: string): Promise<TemplateUninstallResponse> {
    return requestJson<TemplateUninstallResponse>(
      `/api/axops/templates/installs/${encodeURIComponent(installId)}/uninstall`,
      { method: "POST", body: JSON.stringify({}) }
    );
  },

  loadDemoScenario(scenario: DemoScenario): Promise<DemoLoadResponse> {
    return requestJson<DemoLoadResponse>(
      `/api/axops/demo/${encodeURIComponent(scenario)}/load`,
      { method: "POST", body: JSON.stringify({}) }
    );
  },

  listWorkItems(query?: {
    status?: WorkItemStatus;
    priority?: WorkItemPriority;
    assignee?: string;
    source_kind?: WorkItemSourceKind;
    source_ref?: string;
    limit?: number;
    cursor?: string;
  }): Promise<WorkItemListResponse> {
    return requestJson<WorkItemListResponse>(
      `/api/axops/work-items${buildQuery(query ?? {})}`
    );
  },

  createWorkItem(payload: CreateWorkItemPayload): Promise<WorkItem> {
    return requestJson<WorkItem>("/api/axops/work-items", {
      method: "POST",
      body: JSON.stringify(payload),
    });
  },

  getWorkItem(id: string): Promise<WorkItemDetail> {
    return requestJson<WorkItemDetail>(
      `/api/axops/work-items/${encodeURIComponent(id)}`
    );
  },

  patchWorkItem(id: string, payload: PatchWorkItemPayload): Promise<WorkItem> {
    return requestJson<WorkItem>(
      `/api/axops/work-items/${encodeURIComponent(id)}`,
      {
        method: "PATCH",
        body: JSON.stringify(payload),
      }
    );
  },

  addWorkItemComment(id: string, comment: string): Promise<WorkItemEvent> {
    return requestJson<WorkItemEvent>(
      `/api/axops/work-items/${encodeURIComponent(id)}/comments`,
      {
        method: "POST",
        body: JSON.stringify({ comment }),
      }
    );
  },
};
