import { z } from "zod";
import { getAccessToken } from "./auth";
import { ApiError, ApiErrorBodySchema, requestJson, requestMultipart } from "./client";
import type {
  AssetKind,
  CockpitZoneLayout,
  DataType,
  ExportFormat,
  GridCompactMode,
  JobStatus,
  SemanticZoneKind,
  Sensitivity,
  WidgetKind,
} from "./enums";

export type { CockpitZoneLayout, GridCompactMode, SemanticZoneKind } from "./enums";

export type DashboardTheme = "light" | "dark" | "screen";
export type DashboardStatus = "DRAFT" | "PUBLISHED";
export type RefreshMode = "LIVE" | "INTERVAL" | "MANUAL";
export type FilterKind =
  | "SELECT"
  | "MULTI_SELECT"
  | "DATE_RANGE"
  | "NUMBER_RANGE"
  | "TEXT";
export type WidgetActionType = "RUN_FLOW" | "WRITE_NODE";
export type WriteNodeTargetKind = "PROPERTY" | "METRIC";
export type InvokeWidgetActionMode = "FLOW" | "WRITE_NODE";

export interface CanvasConfig {
  cols: number;
  row_height: number;
  screen_width: number;
  screen_height: number;
  zone_layout?: CockpitZoneLayout;
  compact_mode?: GridCompactMode;
}

export interface WidgetPosition {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface DrillLevel {
  field: string;
  label?: string | null;
  node_type?: string | null;
  node_key_field?: string | null;
}

export interface WidgetDeps {
  dataset_ids: string[];
  type_names: string[];
  metrics: string[];
  alerts: boolean;
  scenario_ids: string[];
}

export interface WidgetColumn {
  name: string;
  data_type: DataType | string;
}

export type WidgetDataSource =
  | { kind: "SAVED_QUERY"; saved_query_id: string }
  | {
      kind: "METRIC";
      metric: string;
      dimensions?: string[];
      time_grain?: string | null;
      time_range?: string | null;
    }
  | { kind: "SQL"; sql: string }
  | {
      kind: "NODE_METRIC";
      type_name: string;
      metric: string;
      agg?: string | null;
      grain?: string | null;
      time_range?: string | null;
      group_by?: string | null;
    }
  | { kind: "PIVOT"; spec: Record<string, unknown> }
  | {
      kind: "GRAPH";
      type_names?: string[];
      link_types?: string[];
      color_metric?: string | null;
      limit?: number | null;
    }
  | {
      kind: "ALERTS";
      status?: string[];
      severity?: string[];
      limit?: number | null;
    }
  | {
      kind: "SIM_SCENARIOS";
      scenario_ids?: string[];
      metrics?: string[];
    }
  | {
      kind: "NODES_GEO";
      type_names?: string[];
      color_metric?: string | null;
      link_types?: string[];
    }
  | {
      kind: "FLOOR_PLAN";
      asset_id: string;
      color_metric?: string | null;
    }
  | {
      kind: "SCENE_3D";
      asset_id: string;
      color_metric?: string | null;
    }
  | {
      kind: "ACTION";
      action_type: WidgetActionType;
      flow_id?: string | null;
      node_id?: string | null;
      target_kind?: WriteNodeTargetKind | null;
      key?: string | null;
      input_schema: Record<string, unknown>;
      confirm_text?: string | null;
    }
  | {
      kind: "STATIC";
      columns?: WidgetColumn[];
      rows?: unknown[][];
    };

export interface FilterDef {
  key: string;
  label: string;
  kind: FilterKind;
  field_map: Record<string, string>;
  options_source?: WidgetDataSource | null;
  default?: unknown;
}

export interface DashboardRecord {
  id: string;
  tenant_id: string;
  name: string;
  slug: string;
  description: string;
  filters: FilterDef[];
  theme: DashboardTheme;
  canvas: CanvasConfig;
  status: DashboardStatus;
  version: number;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface WidgetRecord {
  id: string;
  tenant_id: string;
  dashboard_id: string;
  widget_kind: WidgetKind;
  title: string;
  position: WidgetPosition;
  data_source: WidgetDataSource;
  encoding: Record<string, unknown>;
  options: Record<string, unknown>;
  drill: DrillLevel[];
  refresh_mode: RefreshMode;
  refresh_seconds: number;
  deps: WidgetDeps;
  created_at: string;
  updated_at: string;
}

export interface DashboardDetail extends DashboardRecord {
  widgets: WidgetRecord[];
}

export interface ViewPaginatedList<T> {
  items: T[];
  next_cursor: string | null;
}

export interface CreateDashboardRequest {
  name: string;
  slug: string;
  description?: string;
  filters?: FilterDef[];
  theme?: DashboardTheme;
  canvas?: CanvasConfig;
  zone_layout?: CockpitZoneLayout;
}

export interface ApplyZonePresetRequest {
  zone_layout: CockpitZoneLayout;
}

export type DashboardPreflightStatus = "PASS" | "WARN" | "BLOCKED";

export type WidgetIssueCode =
  | "MISSING_DATASET"
  | "BROKEN_METRIC"
  | "MISSING_SAVED_QUERY"
  | "INVALID_SQL_AST"
  | "MISSING_ASSET"
  | "INSUFFICIENT_SCENARIOS"
  | "UNCONFIGURED_ACTION"
  | "UNAUTHORIZED_TARGET"
  | "STATIC_PLACEHOLDER"
  | "LAYOUT_OUT_OF_BOUNDS";

export type WidgetPreflightSeverity = "BLOCKING" | "WARNING";

export interface WidgetPreflightIssue {
  widget_id: string | null;
  widget_title: string;
  semantic_zone: SemanticZoneKind | null;
  severity: WidgetPreflightSeverity;
  code: WidgetIssueCode;
  message: string;
}

export interface DashboardPreflightReport {
  dashboard_id: string;
  status: DashboardPreflightStatus;
  checked_widgets: number;
  blocking_count: number;
  warning_count: number;
  issues: WidgetPreflightIssue[];
}

export interface UpdateDashboardRequest {
  name?: string;
  slug?: string;
  description?: string;
  filters?: FilterDef[];
  theme?: DashboardTheme;
  canvas?: CanvasConfig;
}

export interface CreateWidgetRequest {
  widget_kind: WidgetKind;
  title?: string;
  position: WidgetPosition;
  data_source?: WidgetDataSource;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
  drill?: DrillLevel[];
  refresh_mode?: RefreshMode;
  refresh_seconds?: number;
}

export interface UpdateWidgetRequest {
  widget_kind?: WidgetKind;
  title?: string;
  position?: WidgetPosition;
  data_source?: WidgetDataSource;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
  drill?: DrillLevel[];
  refresh_mode?: RefreshMode;
  refresh_seconds?: number;
}

export interface LayoutUpdateItem {
  id: string;
  position: WidgetPosition;
}

export interface AiGenerateDashboardRequest {
  prompt: string;
  title?: string;
  slug?: string;
  theme?: DashboardTheme;
}

export interface AiDraftDashboardRequest {
  name: string;
  slug?: string | null;
  prompt: string;
  subgraph_slug?: string | null;
  seed_node_types?: string[];
  theme?: DashboardTheme | null;
}

export interface DrillPathItem {
  field: string;
  value: unknown;
}

export interface WidgetDataRequest {
  filters?: Record<string, unknown>;
  drill_path?: DrillPathItem[];
}

export interface WidgetPreviewRequest {
  widget_kind: WidgetKind;
  data_source: WidgetDataSource;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
  drill?: DrillLevel[];
  filters?: Record<string, unknown>;
  drill_path?: DrillPathItem[];
}

export interface WidgetDataResponse {
  columns: WidgetColumn[];
  rows: unknown[][];
  row_count: number;
  truncated: boolean;
  generated_at: string;
  data_version: string;
  drill_level: number;
}

export interface InvokeWidgetActionRequest {
  inputs?: Record<string, unknown>;
  target_node_id?: string | null;
  idempotency_key?: string | null;
}

export interface InvokeWidgetActionResponse {
  mode: InvokeWidgetActionMode;
  run_id: string | null;
  status: string;
  node_id: string | null;
  action_log_id: number | null;
  idempotency_replay: boolean;
}

export interface AssetRecord {
  id: string;
  tenant_id: string;
  asset_kind: AssetKind;
  name: string;
  file_path: string;
  mime_type: string;
  sha256: string;
  width_px: number | null;
  height_px: number | null;
  created_at: string;
}

export interface FloorPlacementRecord {
  asset_id: string;
  node_id: string;
  x_ratio: number;
  y_ratio: number;
}

export interface Scene3dBindingRecord {
  asset_id: string;
  node_id: string;
  mesh_name: string;
  pos_x: number;
  pos_y: number;
  pos_z: number;
  scale: number;
}

export const WidgetDataResponseSchema = z.object({
  columns: z.array(
    z.object({
      name: z.string(),
      data_type: z.string(),
    })
  ),
  rows: z.array(z.array(z.unknown())),
  row_count: z.number(),
  truncated: z.boolean(),
  generated_at: z.string(),
  data_version: z.string(),
  drill_level: z.number(),
});

export const WidgetIssueCodeSchema = z.enum([
  "MISSING_DATASET",
  "BROKEN_METRIC",
  "MISSING_SAVED_QUERY",
  "INVALID_SQL_AST",
  "MISSING_ASSET",
  "INSUFFICIENT_SCENARIOS",
  "UNCONFIGURED_ACTION",
  "UNAUTHORIZED_TARGET",
  "STATIC_PLACEHOLDER",
  "LAYOUT_OUT_OF_BOUNDS",
]);

export const WidgetPreflightIssueSchema = z.object({
  widget_id: z.string().nullable(),
  widget_title: z.string(),
  semantic_zone: z
    .enum(["CONTROL", "KPI", "SCENE", "INSIGHT", "DECISION"])
    .nullable(),
  severity: z.enum(["BLOCKING", "WARNING"]),
  code: WidgetIssueCodeSchema,
  message: z.string(),
});

export const DashboardPreflightReportSchema = z.object({
  dashboard_id: z.string(),
  status: z.enum(["PASS", "WARN", "BLOCKED"]),
  checked_widgets: z.number(),
  blocking_count: z.number(),
  warning_count: z.number(),
  issues: z.array(WidgetPreflightIssueSchema).default([]),
});

function qs(params: Record<string, string | number | boolean | undefined>): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== "") {
      sp.set(k, String(v));
    }
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

export const xblueviewApi = {
  listDashboards: (params?: { limit?: number; cursor?: string }) =>
    requestJson<ViewPaginatedList<DashboardRecord>>(
      `/api/xblueview/dashboards${qs(params ?? {})}`
    ),

  createDashboard: (body: CreateDashboardRequest) =>
    requestJson<DashboardDetail>("/api/xblueview/dashboards", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  getDashboard: (id: string) =>
    requestJson<DashboardDetail>(`/api/xblueview/dashboards/${id}`),

  getDashboardBySlug: (slug: string) =>
    requestJson<DashboardDetail>(
      `/api/xblueview/dashboards/by-slug/${encodeURIComponent(slug)}`
    ),

  updateDashboard: (id: string, body: UpdateDashboardRequest) =>
    requestJson<DashboardRecord>(`/api/xblueview/dashboards/${id}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  deleteDashboard: (id: string) =>
    requestJson<{ deleted: boolean }>(`/api/xblueview/dashboards/${id}`, {
      method: "DELETE",
    }),

  aiGenerateDashboard: (body: AiGenerateDashboardRequest) =>
    requestJson<DashboardDetail>("/api/xblueview/dashboards/ai-generate", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  aiDraftDashboard: (body: AiDraftDashboardRequest) =>
    requestJson<DashboardDetail>("/api/xblueview/dashboards/ai-draft", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  publishDashboard: (id: string) =>
    requestJson<DashboardRecord>(`/api/xblueview/dashboards/${id}/publish`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{}",
    }),

  applyZonePreset: (id: string, body: ApplyZonePresetRequest) =>
    requestJson<DashboardDetail>(
      `/api/xblueview/dashboards/${encodeURIComponent(id)}/apply-zone-preset`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    ),

  preflightDashboard: async (id: string): Promise<DashboardPreflightReport> => {
    const raw = await requestJson<unknown>(
      `/api/xblueview/dashboards/${encodeURIComponent(id)}/preflight`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      }
    );
    return DashboardPreflightReportSchema.parse(raw);
  },

  duplicateDashboard: (id: string, body?: { name?: string; slug?: string }) =>
    requestJson<DashboardDetail>(`/api/xblueview/dashboards/${id}/duplicate`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body ?? {}),
    }),

  updateLayout: (dashboardId: string, widgets: LayoutUpdateItem[]) =>
    requestJson<WidgetRecord[]>(
      `/api/xblueview/dashboards/${dashboardId}/layout`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ widgets }),
      }
    ),

  createWidget: (dashboardId: string, body: CreateWidgetRequest) =>
    requestJson<WidgetRecord>(
      `/api/xblueview/dashboards/${dashboardId}/widgets`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    ),

  updateWidget: (widgetId: string, body: UpdateWidgetRequest) =>
    requestJson<WidgetRecord>(`/api/xblueview/widgets/${widgetId}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  deleteWidget: (widgetId: string) =>
    requestJson<{ deleted: boolean }>(`/api/xblueview/widgets/${widgetId}`, {
      method: "DELETE",
    }),

  executeWidgetData: async (
    widgetId: string,
    body?: WidgetDataRequest
  ): Promise<WidgetDataResponse> => {
    const raw = await requestJson<unknown>(
      `/api/xblueview/widgets/${widgetId}/data`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body ?? {}),
      }
    );
    return WidgetDataResponseSchema.parse(raw) as WidgetDataResponse;
  },

  previewWidget: async (
    body: WidgetPreviewRequest
  ): Promise<WidgetDataResponse> => {
    const raw = await requestJson<unknown>("/api/xblueview/widgets/preview", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    return WidgetDataResponseSchema.parse(raw) as WidgetDataResponse;
  },

  invokeWidgetAction: (widgetId: string, body: InvokeWidgetActionRequest) =>
    requestJson<InvokeWidgetActionResponse>(
      `/api/xblueview/widgets/${widgetId}/invoke`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    ),

  listAssets: (params?: {
    asset_kind?: AssetKind;
    limit?: number;
    cursor?: string;
  }) =>
    requestJson<ViewPaginatedList<AssetRecord>>(
      `/api/xblueview/assets${qs(params ?? {})}`
    ),

  uploadAsset: (file: File, assetKind: AssetKind, name: string) => {
    const form = new FormData();
    form.append("asset_kind", assetKind);
    form.append("name", name);
    form.append("file", file);
    return requestMultipart<AssetRecord>("/api/xblueview/assets", form);
  },

  getAssetContentUrl: (assetId: string) =>
    activeShareAssetScope
      ? `/api/xblueview/share/${encodeURIComponent(activeShareAssetScope.token)}/assets/${encodeURIComponent(assetId)}/content`
      : `/api/xblueview/assets/${assetId}/content`,

  fetchAssetBlobUrl: async (assetId: string): Promise<string> => {
    if (activeShareAssetScope) {
      return xblueviewShareApi.fetchSharedAssetBlobUrl(
        activeShareAssetScope.token,
        assetId,
        activeShareAssetScope.shareSession
      );
    }
    const token = getAccessToken();
    const headers = new Headers();
    if (token) {
      headers.set("Authorization", `Bearer ${token}`);
    }
    const resp = await fetch(`/api/xblueview/assets/${assetId}/content`, {
      headers,
    });
    if (!resp.ok) {
      throw new Error(`加载资产失败 (${resp.status})`);
    }
    const blob = await resp.blob();
    return URL.createObjectURL(blob);
  },

  deleteAsset: (assetId: string) =>
    requestJson<{ deleted: boolean }>(`/api/xblueview/assets/${assetId}`, {
      method: "DELETE",
    }),

  getFloorPlacements: (assetId: string) =>
    requestJson<FloorPlacementRecord[]>(
      `/api/xblueview/assets/${assetId}/placements`
    ),

  putFloorPlacements: (
    assetId: string,
    placements: Omit<FloorPlacementRecord, "asset_id">[]
  ) =>
    requestJson<FloorPlacementRecord[]>(
      `/api/xblueview/assets/${assetId}/placements`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ placements }),
      }
    ),

  putPlacements: (
    assetId: string,
    placements: Omit<FloorPlacementRecord, "asset_id">[]
  ) =>
    requestJson<FloorPlacementRecord[]>(
      `/api/xblueview/assets/${assetId}/placements`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ placements }),
      }
    ),

  getScene3dBindings: (assetId: string) =>
    requestJson<Scene3dBindingRecord[]>(
      `/api/xblueview/assets/${assetId}/bindings3d`
    ),

  putScene3dBindings: (
    assetId: string,
    bindings: Omit<Scene3dBindingRecord, "asset_id">[]
  ) =>
    requestJson<Scene3dBindingRecord[]>(
      `/api/xblueview/assets/${assetId}/bindings3d`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ bindings }),
      }
    ),

  createExport: (body: CreateExportRequest) =>
    requestJson<ExportJobRecord>("/api/xblueview/exports", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  listExports: (params?: {
    mine?: boolean;
    limit?: number;
    cursor?: string;
  }) =>
    requestJson<ViewPaginatedList<ExportJobRecord>>(
      `/api/xblueview/exports${qs(params ?? {})}`
    ),

  getExport: (exportId: string) =>
    requestJson<ExportJobRecord>(`/api/xblueview/exports/${exportId}`),

  deleteExport: (exportId: string) =>
    requestJson<{ deleted: boolean }>(`/api/xblueview/exports/${exportId}`, {
      method: "DELETE",
    }),

  downloadExportBlob: async (
    exportId: string
  ): Promise<{ blob: Blob; filename: string }> => {
    const token = getAccessToken();
    const headers = new Headers();
    if (token) {
      headers.set("Authorization", `Bearer ${token}`);
    }
    const resp = await fetch(`/api/xblueview/exports/${exportId}/download`, {
      headers,
    });
    if (!resp.ok) {
      throw await parsePublicFetchError(resp);
    }
    const cd = resp.headers.get("Content-Disposition") ?? "";
    const filename = parseContentDispositionFilename(cd) ?? `export-${exportId}`;
    const blob = await resp.blob();
    return { blob, filename };
  },

  downloadExport: async (
    exportId: string,
    fallbackFileName?: string
  ): Promise<void> => {
    const { blob, filename } = await xblueviewApi.downloadExportBlob(exportId);
    const resolvedName =
      filename === `export-${exportId}` && fallbackFileName
        ? fallbackFileName
        : filename;
    triggerBlobDownload(blob, resolvedName);
  },

  listExportTemplates: (params?: { limit?: number; cursor?: string }) =>
    requestJson<ViewPaginatedList<ExportTemplateRecord>>(
      `/api/xblueview/export-templates${qs(params ?? {})}`
    ),

  uploadExportTemplate: (
    file: File,
    exportFormat: "DOCX" | "PPTX",
    name: string
  ) => {
    const form = new FormData();
    form.append("export_format", exportFormat);
    form.append("name", name);
    form.append("file", file);
    return requestMultipart<ExportTemplateRecord>(
      "/api/xblueview/export-templates",
      form
    );
  },

  deleteExportTemplate: (templateId: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/xblueview/export-templates/${templateId}`,
      { method: "DELETE" }
    ),

  createShare: (body: CreateShareRequest) =>
    requestJson<CreateShareResponse>("/api/xblueview/shares", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  listShares: (params?: {
    target_id?: string;
    limit?: number;
    cursor?: string;
  }) =>
    requestJson<ViewPaginatedList<ShareLinkRecord>>(
      `/api/xblueview/shares${qs(params ?? {})}`
    ),

  revokeShare: (shareId: string) =>
    requestJson<{ deleted: boolean }>(`/api/xblueview/shares/${shareId}`, {
      method: "DELETE",
    }),

  listSavedCharts: (params?: { limit?: number; cursor?: string }) =>
    requestJson<ViewPaginatedList<SavedChartRecord>>(
      `/api/xblueview/charts${qs(params ?? {})}`
    ),

  createSavedChart: (body: SaveChartRequest) =>
    requestJson<SavedChartRecord>("/api/xblueview/charts", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  getSavedChart: (chartId: string) =>
    requestJson<SavedChartRecord>(
      `/api/xblueview/charts/${encodeURIComponent(chartId)}`
    ),

  updateSavedChart: (chartId: string, body: UpdateSavedChartRequest) =>
    requestJson<SavedChartRecord>(
      `/api/xblueview/charts/${encodeURIComponent(chartId)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    ),

  deleteSavedChart: (chartId: string) =>
    requestJson<{ deleted: boolean }>(
      `/api/xblueview/charts/${encodeURIComponent(chartId)}`,
      { method: "DELETE" }
    ),

  pinSavedChart: (chartId: string, body: PinChartToDashboardRequest) =>
    requestJson<PinChartToDashboardResponse>(
      `/api/xblueview/charts/${encodeURIComponent(chartId)}/pin`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    ),
};

export interface SavedChartRecord {
  id: string;
  tenant_id: string;
  name: string;
  description: string;
  widget_kind: WidgetKind;
  data_source: WidgetDataSource;
  encoding: Record<string, unknown>;
  options: Record<string, unknown>;
  owner_dept: string;
  sensitivity: Sensitivity;
  shared: boolean;
  created_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface SaveChartRequest {
  name: string;
  description?: string;
  widget_kind: WidgetKind;
  data_source: WidgetDataSource;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
  sensitivity?: Sensitivity;
  shared?: boolean;
}

export interface UpdateSavedChartRequest {
  name?: string;
  description?: string;
  widget_kind?: WidgetKind;
  data_source?: WidgetDataSource;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
  sensitivity?: Sensitivity;
  shared?: boolean;
}

export interface PinChartToDashboardRequest {
  dashboard_id: string;
  target_zone?: SemanticZoneKind;
  width?: number;
  height?: number;
}

export interface PinChartToDashboardResponse {
  widget_id: string;
  semantic_zone?: SemanticZoneKind | null;
  widget: WidgetRecord;
  dashboard_id: string;
  dashboard_slug: string;
  deep_link: string;
}

export type ExportPageSize = "A4" | "A3";
export type ExportOrientation = "portrait" | "landscape";
export type ShareTargetKind = "DASHBOARD" | "EXPORT_FILE" | "SCENARIO";

export type ExportSource =
  | {
      kind: "DASHBOARD";
      dashboard_id: string;
      filters?: Record<string, unknown>;
    }
  | {
      kind: "WIDGET";
      widget_id: string;
      filters?: Record<string, unknown>;
      drill_path?: DrillPathItem[];
    }
  | {
      kind: "SAVED_QUERY";
      saved_query_id: string;
    }
  | {
      kind: "PIVOT";
      spec: Record<string, unknown>;
    }
  | {
      kind: "SCENARIO";
      scenario_ids: string[];
    }
  | {
      kind: "CONVERSATION";
      conversation_id: string;
    }
  | {
      kind: "MARKDOWN";
      title?: string;
      markdown: string;
    };

export interface ExportOptions {
  title?: string;
  subtitle?: string;
  watermark?: boolean | string;
  csv_bom?: boolean;
  page_size?: ExportPageSize;
  orientation?: ExportOrientation;
  placeholders?: Record<string, string>;
  timezone?: string;
}

export interface CreateExportRequest {
  export_format: ExportFormat;
  source: ExportSource;
  options?: ExportOptions;
  template_id?: string | null;
}

export interface ExportJobRecord {
  id: string;
  tenant_id: string;
  export_format: ExportFormat;
  source: ExportSource;
  options: ExportOptions;
  template_id?: string | null;
  job_id?: string | null;
  status: JobStatus;
  file_name?: string | null;
  file_size?: number | null;
  sha256?: string | null;
  error_message?: string | null;
  created_by: string;
  created_at: string;
  finished_at?: string | null;
  expires_at?: string | null;
}

export interface ExportTemplateRecord {
  id: string;
  tenant_id: string;
  name: string;
  export_format: "DOCX" | "PPTX";
  sha256: string;
  placeholders: string[];
  created_by?: string | null;
  created_at: string;
}

export interface CreateShareRequest {
  target_kind: ShareTargetKind;
  target_id: string;
  expires_in_hours: number;
  max_views?: number | null;
  password?: string | null;
  locked_filters?: Record<string, unknown>;
  allow_filters?: boolean;
  allow_export?: boolean;
}

export interface CreateShareResponse {
  id: string;
  token: string;
  url: string;
  expires_at: string;
}

export interface ShareLinkRecord {
  id: string;
  tenant_id: string;
  token_prefix: string;
  target_kind: ShareTargetKind;
  target_id: string;
  locked_filters: Record<string, unknown>;
  allow_filters: boolean;
  allow_export: boolean;
  has_password: boolean;
  expires_at: string;
  max_views?: number | null;
  view_count: number;
  created_by: string;
  created_at: string;
  revoked_at?: string | null;
}

export interface SharedExportFileMeta {
  id: string;
  export_format: ExportFormat;
  file_name: string;
  file_size: number;
  finished_at?: string | null;
}

export interface SharePublicResponse {
  password_required: boolean;
  target_kind?: ShareTargetKind;
  expires_at?: string;
  locked_filters?: Record<string, unknown>;
  allow_filters?: boolean;
  allow_export?: boolean;
  dashboard?: DashboardDetail;
  export_file?: SharedExportFileMeta;
  scenario?: Record<string, unknown>;
}

export interface ShareUnlockResponse {
  share_session: string;
  expires_at: string;
}

async function parsePublicFetchError(resp: Response): Promise<ApiError> {
  try {
    const raw: unknown = await resp.json();
    const parsed = ApiErrorBodySchema.safeParse(raw);
    if (parsed.success) {
      return new ApiError({
        status: resp.status,
        errorCode: parsed.data.error_code,
        message: parsed.data.message,
        traceId: parsed.data.trace_id,
      });
    }
  } catch {
    // ignore non-json error body
  }
  return new ApiError({
    status: resp.status,
    errorCode: `HTTP_${resp.status}`,
    message: `请求失败 (${resp.status})`,
    traceId: "",
  });
}

function parseContentDispositionFilename(header: string): string | null {
  const utf8Match = /filename\*=UTF-8''([^;\s]+)/i.exec(header);
  if (utf8Match?.[1]) {
    try {
      return decodeURIComponent(utf8Match[1]);
    } catch {
      return utf8Match[1];
    }
  }
  const plainMatch = /filename="?([^";]+)"?/i.exec(header);
  return plainMatch?.[1] ?? null;
}

async function requestSharePublic<T>(
  path: string,
  shareSession?: string | null,
  init?: RequestInit
): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("Accept", "application/json");
  if (shareSession) {
    headers.set("X-Share-Session", shareSession);
  }
  const resp = await fetch(path, { ...init, headers });
  if (!resp.ok) {
    throw await parsePublicFetchError(resp);
  }
  return (await resp.json()) as T;
}

interface ShareAssetScope {
  token: string;
  shareSession?: string | null;
}

let activeShareAssetScope: ShareAssetScope | null = null;

/** Scopes raw spatial asset fetches (`FLOOR_PLAN` / `SCENE_3D`) to `/api/xblueview/share/:token/assets/:id/content` while `SharePage` is mounted. */
export function setShareAssetScope(scope: ShareAssetScope | null): void {
  activeShareAssetScope = scope;
}

function triggerBlobDownload(blob: Blob, filename: string): void {
  if (typeof document === "undefined") return;
  const objectUrl = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = objectUrl;
  anchor.download = filename;
  anchor.rel = "noopener";
  document.body.appendChild(anchor);
  anchor.click();
  document.body.removeChild(anchor);
  setTimeout(() => {
    URL.revokeObjectURL(objectUrl);
  }, 1000);
}

/** Public no-Bearer API client for `/share/:token` (`SharePage`) that never redirects to `/login`. */
export const xblueviewShareApi = {
  getShare: (token: string, shareSession?: string | null) =>
    requestSharePublic<SharePublicResponse>(
      `/api/xblueview/share/${encodeURIComponent(token)}`,
      shareSession
    ),

  unlockShare: (token: string, password: string) =>
    requestSharePublic<ShareUnlockResponse>(
      `/api/xblueview/share/${encodeURIComponent(token)}/unlock`,
      null,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ password }),
      }
    ),

  fetchSharedWidgetData: (
    token: string,
    widgetId: string,
    body: WidgetDataRequest,
    shareSession?: string | null
  ) =>
    requestSharePublic<WidgetDataResponse>(
      `/api/xblueview/share/${encodeURIComponent(token)}/widgets/${encodeURIComponent(widgetId)}/data`,
      shareSession,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    ),

  querySharedWidgetData: (
    token: string,
    widgetId: string,
    body: WidgetDataRequest,
    shareSession?: string | null
  ) =>
    xblueviewShareApi.fetchSharedWidgetData(
      token,
      widgetId,
      body,
      shareSession
    ),

  getSharedAssetRawUrl: (token: string, assetId: string) =>
    `/api/xblueview/share/${encodeURIComponent(token)}/assets/${encodeURIComponent(assetId)}/content`,

  fetchSharedAssetBlobUrl: async (
    token: string,
    assetId: string,
    shareSession?: string | null
  ): Promise<string> => {
    const headers = new Headers();
    if (shareSession) {
      headers.set("X-Share-Session", shareSession);
    }
    const resp = await fetch(
      xblueviewShareApi.getSharedAssetRawUrl(token, assetId),
      { headers }
    );
    if (!resp.ok) {
      throw await parsePublicFetchError(resp);
    }
    const blob = await resp.blob();
    return URL.createObjectURL(blob);
  },

  downloadSharedArtifact: async (
    token: string,
    shareSession?: string | null,
    format?: ExportFormat
  ): Promise<{ blob: Blob; filename: string }> => {
    const headers = new Headers();
    if (shareSession) {
      headers.set("X-Share-Session", shareSession);
    }
    const suffix = format ? `?format=${encodeURIComponent(format)}` : "";
    const resp = await fetch(
      `/api/xblueview/share/${encodeURIComponent(token)}/download${suffix}`,
      { headers }
    );
    if (!resp.ok) {
      throw await parsePublicFetchError(resp);
    }
    const cd = resp.headers.get("Content-Disposition") ?? "";
    const filename = parseContentDispositionFilename(cd) ?? `shared-export`;
    const blob = await resp.blob();
    return { blob, filename };
  },

  downloadShare: async (
    token: string,
    format?: ExportFormat,
    shareSession?: string | null,
    fallbackFileName?: string
  ): Promise<void> => {
    const { blob, filename } = await xblueviewShareApi.downloadSharedArtifact(
      token,
      shareSession,
      format
    );
    const resolvedName =
      filename === "shared-export" && fallbackFileName
        ? fallbackFileName
        : filename === "shared-export" && format
          ? `shared-report.${format.toLowerCase()}`
          : filename;
    triggerBlobDownload(blob, resolvedName);
  },
};
