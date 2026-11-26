import { getLocale } from "@/i18n";
import type { WidgetKind } from "@/lib/api/enums";
import type {
  CanvasConfig,
  WidgetDataResponse,
  WidgetDataSource,
  WidgetPosition,
  WidgetRecord,
} from "@/lib/api/xblueview";

export interface ThresholdRule {
  value: number;
  color: string;
  label?: string;
}

export interface CrossFilterConfig {
  field: string;
  target_filter_key: string;
}

export interface SelectedNodeContext {
  nodeId: string;
  typeName?: string;
  nodeKey?: string;
  title?: string;
  metricName?: string;
  metricValue?: number | null;
}

export interface KnownNodeSummary {
  title: string;
  nodeKey?: string;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function readString(
  obj: Record<string, unknown> | undefined,
  key: string
): string | undefined {
  if (!obj) return undefined;
  const v = obj[key];
  return typeof v === "string" && v.trim().length > 0 ? v : undefined;
}

export function readNumber(
  obj: Record<string, unknown> | undefined,
  key: string
): number | undefined {
  if (!obj) return undefined;
  const v = obj[key];
  if (typeof v === "number" && Number.isFinite(v)) return v;
  if (typeof v === "string" && v.trim().length > 0) {
    const parsed = Number(v);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

export function readBoolean(
  obj: Record<string, unknown> | undefined,
  key: string
): boolean | undefined {
  if (!obj) return undefined;
  const v = obj[key];
  return typeof v === "boolean" ? v : undefined;
}

export function readStringArray(
  obj: Record<string, unknown> | undefined,
  key: string
): string[] {
  if (!obj) return [];
  const v = obj[key];
  if (Array.isArray(v)) {
    return v.filter(
      (item): item is string =>
        typeof item === "string" && item.trim().length > 0
    );
  }
  if (typeof v === "string" && v.trim().length > 0) {
    return [v.trim()];
  }
  return [];
}

export function parseThresholds(
  options: Record<string, unknown> | undefined
): ThresholdRule[] {
  if (!options) return [];
  const raw = options.thresholds;
  if (!Array.isArray(raw)) return [];
  const rules: ThresholdRule[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const value = readNumber(item, "value");
    const color = readString(item, "color");
    if (value !== undefined && color) {
      rules.push({
        value,
        color,
        label: readString(item, "label"),
      });
    }
  }
  return rules.sort((a, b) => a.value - b.value);
}

export function resolveThresholdColor(
  value: number | null | undefined,
  thresholds: ThresholdRule[],
  fallbackColor = "#2563eb"
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return fallbackColor;
  }
  if (thresholds.length === 0) {
    return fallbackColor;
  }
  let matched = fallbackColor;
  for (const rule of thresholds) {
    if (value >= rule.value) {
      matched = rule.color;
    }
  }
  return matched;
}

export function parseCrossFilter(
  options: Record<string, unknown> | undefined
): CrossFilterConfig | null {
  if (!options) return null;
  const raw = options.cross_filter;
  if (!isRecord(raw)) return null;
  const field = readString(raw, "field");
  const targetKey = readString(raw, "target_filter_key");
  if (!field || !targetKey) return null;
  return { field, target_filter_key: targetKey };
}

export function toNumericValue(val: unknown): number | null {
  if (typeof val === "number" && Number.isFinite(val)) return val;
  if (typeof val === "string" && val.trim().length > 0) {
    const num = Number(val);
    if (Number.isFinite(num)) return num;
  }
  return null;
}

export function formatDisplayValue(
  val: unknown,
  options?: {
    unit?: string;
    decimals?: number;
    prefix?: string;
    suffix?: string;
  }
): string {
  if (val === null || val === undefined) return "-";
  const num = toNumericValue(val);
  if (num !== null) {
    const decimals = options?.decimals ?? (Number.isInteger(num) ? 0 : 2);
    const formatted = num.toLocaleString(getLocale(), {
      minimumFractionDigits: 0,
      maximumFractionDigits: Math.max(0, Math.min(6, decimals)),
    });
    const unitPart = options?.unit
      ? options.unit.startsWith("%")
        ? options.unit
        : ` ${options.unit}`
      : "";
    return `${options?.prefix ?? ""}${formatted}${unitPart}${options?.suffix ?? ""}`;
  }
  if (typeof val === "object") {
    return JSON.stringify(val);
  }
  return String(val);
}

export function buildColumnIndexMap(
  data: WidgetDataResponse | null | undefined
): Map<string, number> {
  const map = new Map<string, number>();
  if (!data) return map;
  data.columns.forEach((col, idx) => {
    map.set(col.name, idx);
    map.set(col.name.toLowerCase(), idx);
  });
  return map;
}

export function getCell(
  row: unknown[],
  colMap: Map<string, number>,
  colName: string | undefined,
  fallbackIndex: number
): unknown {
  if (colName) {
    const idx = colMap.get(colName) ?? colMap.get(colName.toLowerCase());
    if (idx !== undefined && idx >= 0 && idx < row.length) {
      return row[idx];
    }
  }
  if (fallbackIndex >= 0 && fallbackIndex < row.length) {
    return row[fallbackIndex];
  }
  return undefined;
}

export function findFirstNumericColIndex(
  data: WidgetDataResponse,
  excludeIndices: Set<number>
): number {
  for (let i = 0; i < data.columns.length; i += 1) {
    if (excludeIndices.has(i)) continue;
    const col = data.columns[i];
    if (!col) continue;
    const dt = col.data_type.toUpperCase();
    if (
      dt === "INT64" ||
      dt === "FLOAT64" ||
      dt === "DECIMAL" ||
      dt === "NUMBER"
    ) {
      return i;
    }
  }
  for (let i = 0; i < data.columns.length; i += 1) {
    if (excludeIndices.has(i)) continue;
    const firstRow = data.rows[0];
    if (firstRow && toNumericValue(firstRow[i]) !== null) {
      return i;
    }
  }
  return data.columns.length > 1 ? 1 : 0;
}

export function extractKnownNodesFromWidgetDataMap(
  widgetDataMap: Readonly<Record<string, WidgetDataResponse | null>>
): Record<string, KnownNodeSummary> {
  const out: Record<string, KnownNodeSummary> = {};
  for (const data of Object.values(widgetDataMap)) {
    if (!data || !Array.isArray(data.columns) || !Array.isArray(data.rows)) {
      continue;
    }
    const colMap = buildColumnIndexMap(data);
    const nodeIdIdx = colMap.get("node_id") ?? colMap.get("id");
    const titleIdx = colMap.get("title");
    const nodeKeyIdx = colMap.get("node_key");
    const recordKindIdx = colMap.get("record_kind");

    if (nodeIdIdx === undefined || (titleIdx === undefined && nodeKeyIdx === undefined)) {
      continue;
    }

    for (const r of data.rows) {
      if (recordKindIdx !== undefined) {
        const rk = String(r[recordKindIdx] ?? "").toUpperCase();
        if (rk && rk !== "NODE") continue;
      }
      const rawId = r[nodeIdIdx];
      if (typeof rawId !== "string" || !rawId.trim()) continue;
      const nodeId = rawId.trim();
      const rawTitle =
        titleIdx !== undefined && typeof r[titleIdx] === "string"
          ? r[titleIdx].trim()
          : "";
      const rawKey =
        nodeKeyIdx !== undefined && typeof r[nodeKeyIdx] === "string"
          ? r[nodeKeyIdx].trim()
          : undefined;
      if (rawTitle || rawKey) {
        out[nodeId] = {
          title: rawTitle || rawKey || nodeId,
          nodeKey: rawKey,
        };
      }
    }
  }
  return out;
}

export const WIDGET_KIND_LABELS: Record<WidgetKind, string> = {
  KPI_CARD: "KPI 指标卡",
  TABLE: "明细数据表",
  PIVOT_TABLE: "多维透视表",
  LINE_CHART: "折线趋势图",
  BAR_CHART: "柱状对比图",
  PIE_CHART: "饼图 / 环形图",
  SCATTER: "散点分布图",
  SANKEY: "桑基流向图",
  HEATMAP: "矩阵热力图",
  GAUGE: "仪表盘",
  RADAR: "多维雷达图",
  GRAPH_TOPOLOGY: "关系拓扑图",
  MAP: "空间地理地图",
  FLOOR_PLAN: "二维平面孪生",
  SCENE_3D: "三维数字孪生",
  ALERT_LIST: "实时告警列表",
  SIM_COMPARE: "沙盘方案对比",
  ACTION_BUTTON: "动作触发按钮",
  ACTION_FORM: "交互回写表单",
  MARKDOWN_NOTE: "说明卡片",
};

export const WIDGET_KIND_LABELS_EN: Record<WidgetKind, string> = {
  KPI_CARD: "KPI Card",
  TABLE: "Data Table",
  PIVOT_TABLE: "Pivot Table",
  LINE_CHART: "Line Chart",
  BAR_CHART: "Bar Chart",
  PIE_CHART: "Pie Chart",
  SCATTER: "Scatter Plot",
  SANKEY: "Sankey Diagram",
  HEATMAP: "Heatmap",
  GAUGE: "Gauge",
  RADAR: "Radar Chart",
  GRAPH_TOPOLOGY: "Topology Graph",
  MAP: "Geo Map",
  FLOOR_PLAN: "2D Floor Plan",
  SCENE_3D: "3D Twin",
  ALERT_LIST: "Alert Feed",
  SIM_COMPARE: "Scenario Compare",
  ACTION_BUTTON: "Action Button",
  ACTION_FORM: "Writeback Form",
  MARKDOWN_NOTE: "Markdown Note",
};

export function getWidgetKindLabel(kind: WidgetKind, isZh = true): string {
  return isZh ? WIDGET_KIND_LABELS[kind] : WIDGET_KIND_LABELS_EN[kind];
}

export const ALL_WIDGET_KINDS: readonly WidgetKind[] = [
  "KPI_CARD",
  "TABLE",
  "PIVOT_TABLE",
  "LINE_CHART",
  "BAR_CHART",
  "PIE_CHART",
  "SCATTER",
  "SANKEY",
  "HEATMAP",
  "GAUGE",
  "RADAR",
  "GRAPH_TOPOLOGY",
  "MAP",
  "FLOOR_PLAN",
  "SCENE_3D",
  "ALERT_LIST",
  "SIM_COMPARE",
  "ACTION_BUTTON",
  "ACTION_FORM",
  "MARKDOWN_NOTE",
];

export function defaultDataSourceForKind(kind: WidgetKind): WidgetDataSource {
  switch (kind) {
    case "KPI_CARD":
    case "GAUGE":
      return {
        kind: "METRIC",
        metric: "paid_amount",
        dimensions: [],
        time_grain: "DAY",
        time_range: "LAST_30_DAYS",
      };
    case "LINE_CHART":
    case "BAR_CHART":
    case "PIE_CHART":
    case "SCATTER":
    case "SANKEY":
    case "HEATMAP":
    case "RADAR":
    case "TABLE":
      return {
        kind: "SQL",
        sql: "SELECT status, sum(amount) AS amt FROM demo_orders GROUP BY 1",
      };
    case "PIVOT_TABLE":
      return {
        kind: "PIVOT",
        spec: {
          dataset: "demo_orders",
          rows: [{ field: "status" }],
          columns: [],
          values: [{ field: "amount", agg: "SUM", alias: "amt" }],
          filters: [],
          sort: [],
          subtotals: true,
          grand_total: true,
          row_limit: 100,
          column_limit: 50,
        },
      };
    case "GRAPH_TOPOLOGY":
      return {
        kind: "GRAPH",
        type_names: ["Factory", "Warehouse"],
        link_types: ["SUPPLIES"],
        limit: 500,
      };
    case "MAP":
      return { kind: "NODES_GEO", type_names: ["Factory", "Warehouse"] };
    case "FLOOR_PLAN":
      return { kind: "FLOOR_PLAN", asset_id: "" };
    case "SCENE_3D":
      return { kind: "SCENE_3D", asset_id: "" };
    case "ALERT_LIST":
      return {
        kind: "ALERTS",
        status: ["OPEN", "ACK"],
        severity: ["WARN", "CRITICAL"],
        limit: 20,
      };
    case "SIM_COMPARE":
      return { kind: "SIM_SCENARIOS", scenario_ids: [], metrics: [] };
    case "ACTION_BUTTON":
      return {
        kind: "ACTION",
        action_type: "RUN_FLOW",
        flow_id: undefined,
        input_schema: { type: "object", properties: {} },
        confirm_text: "确认执行该业务动作？",
      };
    case "ACTION_FORM":
      return {
        kind: "ACTION",
        action_type: "WRITE_NODE",
        node_id: undefined,
        target_kind: "METRIC",
        key: "inventory",
        input_schema: {
          type: "object",
          required: ["value"],
          properties: {
            value: { type: "number", title: "盘点库存值" },
          },
        },
      };
    case "MARKDOWN_NOTE":
      return { kind: "STATIC", columns: [], rows: [] };
  }
}

export function defaultPositionForKind(
  kind: WidgetKind,
  existingWidgets: WidgetRecord[]
): WidgetPosition {
  let maxY = 0;
  for (const w of existingWidgets) {
    const bottom = (w.position?.y ?? 0) + (w.position?.h ?? 4);
    if (bottom > maxY) maxY = bottom;
  }
  if (kind === "KPI_CARD") return { x: 0, y: maxY, w: 6, h: 4 };
  if (kind === "GAUGE" || kind === "ACTION_BUTTON")
    return { x: 0, y: maxY, w: 6, h: 6 };
  if (kind === "SCENE_3D" || kind === "MAP" || kind === "FLOOR_PLAN") {
    return { x: 0, y: maxY, w: 12, h: 9 };
  }
  return { x: 0, y: maxY, w: 8, h: 7 };
}

export const DEFAULT_CANVAS: CanvasConfig = {
  cols: 24,
  row_height: 30,
  screen_width: 1920,
  screen_height: 1080,
};
