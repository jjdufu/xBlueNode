import { z } from "zod";
import { getAccessToken } from "./auth";
import { requestJson, requestMultipart } from "./client";
import type { AssetKind, DataType, WidgetKind } from "./enums";

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

  publishDashboard: (id: string) =>
    requestJson<DashboardRecord>(`/api/xblueview/dashboards/${id}/publish`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{}",
    }),

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
    `/api/xblueview/assets/${assetId}/content`,

  fetchAssetBlobUrl: async (assetId: string): Promise<string> => {
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
};
