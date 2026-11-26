/**
 * Pure data-contract and transformation logic for xBlueView widgets and dashboard state.
 * Kept free of JSX and bundler path aliases so Node 24 `node:test` can execute it natively.
 */
import chartSpec from "../../lib/chartSpec.json" with { type: "json" };

export { chartSpec };

export type LogicFilterKind =
  | "SELECT"
  | "MULTI_SELECT"
  | "DATE_RANGE"
  | "NUMBER_RANGE"
  | "TEXT";

export type LogicFilterOp =
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

export interface LogicFilterDef {
  key: string;
  label: string;
  kind: LogicFilterKind;
  field_map: Record<string, string>;
  default?: unknown;
}

export interface LogicFilterSpec {
  field: string;
  op: LogicFilterOp;
  value?: unknown;
}

export interface LogicColumn {
  name: string;
  data_type: string;
}

export interface LogicTabularData {
  columns: LogicColumn[];
  rows: unknown[][];
}

export interface ParsedGraphNode {
  id: string;
  name: string;
  category: string;
  value: number;
}

export interface ParsedGraphLink {
  source: string;
  target: string;
  value?: number;
  label?: string;
}

export interface ParsedGraphTopology {
  nodes: ParsedGraphNode[];
  links: ParsedGraphLink[];
  categories: string[];
}

export interface GeoNodePoint {
  nodeId: string;
  typeName: string;
  nodeKey: string;
  title: string;
  lng: number;
  lat: number;
  metricValue: number | null;
  openAlerts: number;
}

export interface GeoFlyLine {
  linkType: string;
  srcLng: number;
  srcLat: number;
  dstLng: number;
  dstLat: number;
  weight: number;
}

export type PivotRowClassification = "DETAIL" | "SUBTOTAL" | "TOTAL";

export function isNonEmptyFilterValue(val: unknown): boolean {
  if (val === undefined || val === null || val === "") {
    return false;
  }
  if (Array.isArray(val)) {
    if (val.length === 0) return false;
    return val.some(
      (item) => item !== undefined && item !== null && item !== ""
    );
  }
  if (typeof val === "string") {
    return val.trim().length > 0;
  }
  return true;
}

/**
 * Builds the `filters` payload for `POST /api/xblueview/widgets/{id}/data`.
 * Preserves `FilterDef.key` as the dictionary key (required by backend `resolve_dashboard_filters`)
 * and omits filters that have no `field_map[widgetId]` entry or are not defined in `dashFilters`.
 */
export function buildWidgetFilters(
  widgetId: string,
  dashFilters: readonly LogicFilterDef[],
  currentFilterValues: Record<string, unknown>
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of dashFilters) {
    const mappedField = f.field_map?.[widgetId];
    if (typeof mappedField !== "string" || mappedField.trim().length === 0) {
      continue;
    }
    const val = currentFilterValues[f.key];
    if (!isNonEmptyFilterValue(val)) {
      continue;
    }
    out[f.key] = val;
  }
  return out;
}

/**
 * Converts active dashboard filter values into `AskContext.filters` (`FilterSpec[]`)
 * using `FilterDef.kind` (`Between` / `In` / `Like` / `Eq`) and mapped dataset column names.
 */
export function buildAskFilterSpecs(
  dashFilters: readonly LogicFilterDef[],
  values: Record<string, unknown>
): LogicFilterSpec[] {
  const specs: LogicFilterSpec[] = [];
  for (const f of dashFilters) {
    const rawVal = values[f.key];
    if (!isNonEmptyFilterValue(rawVal)) {
      continue;
    }

    const mappedColumns = Array.from(
      new Set(
        Object.values(f.field_map ?? {})
          .map((col) => (typeof col === "string" ? col.trim() : ""))
          .filter((col) => col.length > 0)
      )
    );
    const targetFields = mappedColumns.length > 0 ? mappedColumns : [f.key];

    for (const field of targetFields) {
      switch (f.kind) {
        case "DATE_RANGE":
        case "NUMBER_RANGE": {
          const rangeVal = Array.isArray(rawVal) ? rawVal : [rawVal, rawVal];
          specs.push({
            field,
            op: "Between",
            value: [rangeVal[0] ?? null, rangeVal[1] ?? null],
          });
          break;
        }
        case "MULTI_SELECT": {
          const arrVal = Array.isArray(rawVal) ? rawVal : [rawVal];
          if (arrVal.length > 0) {
            specs.push({
              field,
              op: "In",
              value: arrVal,
            });
          }
          break;
        }
        case "TEXT": {
          const text = String(rawVal).trim();
          if (text.length > 0) {
            specs.push({
              field,
              op: "Like",
              value: text.includes("%") ? text : `%${text}%`,
            });
          }
          break;
        }
        case "SELECT": {
          if (Array.isArray(rawVal)) {
            specs.push({
              field,
              op: "In",
              value: rawVal,
            });
          } else {
            specs.push({
              field,
              op: "Eq",
              value: rawVal,
            });
          }
          break;
        }
      }
    }
  }
  return specs;
}

export function buildColMap(data: LogicTabularData | null | undefined): Map<string, number> {
  const map = new Map<string, number>();
  if (!data) return map;
  data.columns.forEach((col, idx) => {
    map.set(col.name, idx);
    map.set(col.name.toLowerCase(), idx);
  });
  return map;
}

export function readCellByName(
  row: readonly unknown[],
  colMap: Map<string, number>,
  colName: string,
  fallbackIndex = -1
): unknown {
  const idx = colMap.get(colName) ?? colMap.get(colName.toLowerCase());
  if (idx !== undefined && idx >= 0 && idx < row.length) {
    return row[idx];
  }
  if (fallbackIndex >= 0 && fallbackIndex < row.length) {
    return row[fallbackIndex];
  }
  return undefined;
}

export function toFiniteNumber(val: unknown): number | null {
  if (typeof val === "number" && Number.isFinite(val)) return val;
  if (typeof val === "string" && val.trim().length > 0) {
    const parsed = Number(val);
    if (Number.isFinite(parsed)) return parsed;
  }
  return null;
}

function nonEmptyStr(val: unknown): string | undefined {
  if (typeof val === "string") {
    const t = val.trim();
    return t.length > 0 ? t : undefined;
  }
  if (typeof val === "number" && Number.isFinite(val)) {
    return String(val);
  }
  return undefined;
}

/**
 * Parses backend 10-column `GRAPH` (`record_kind = "NODE" | "LINK"`) responses as well as
 * 3-column `[source, target, value]` preview tables into ECharts graph nodes & links.
 */
export function parseGraphTopologyData(data: LogicTabularData): ParsedGraphTopology {
  const colMap = buildColMap(data);
  const hasRecordKind = colMap.has("record_kind");
  const nodesMap = new Map<string, ParsedGraphNode>();
  const links: ParsedGraphLink[] = [];

  for (const row of data.rows) {
    if (hasRecordKind) {
      const kind = (nonEmptyStr(readCellByName(row, colMap, "record_kind")) ?? "").toUpperCase();
      if (kind === "NODE") {
        const id = nonEmptyStr(readCellByName(row, colMap, "id"));
        if (!id) continue;
        const nodeKey = nonEmptyStr(readCellByName(row, colMap, "node_key"));
        const title = nonEmptyStr(readCellByName(row, colMap, "title"));
        const typeName = nonEmptyStr(readCellByName(row, colMap, "type_name")) ?? "Node";
        const metricVal = toFiniteNumber(readCellByName(row, colMap, "metric_value")) ?? 1;
        nodesMap.set(id, {
          id,
          name: title ?? nodeKey ?? id,
          category: typeName,
          value: metricVal,
        });
      } else if (kind === "LINK") {
        const src = nonEmptyStr(readCellByName(row, colMap, "src_node_id"));
        const dst = nonEmptyStr(readCellByName(row, colMap, "dst_node_id"));
        if (!src || !dst) continue;
        if (!nodesMap.has(src)) {
          nodesMap.set(src, { id: src, name: src, category: "Node", value: 1 });
        }
        if (!nodesMap.has(dst)) {
          nodesMap.set(dst, { id: dst, name: dst, category: "Node", value: 1 });
        }
        const weight = toFiniteNumber(readCellByName(row, colMap, "weight"));
        const linkType = nonEmptyStr(readCellByName(row, colMap, "link_type"));
        links.push({
          source: src,
          target: dst,
          value: weight ?? undefined,
          label: linkType,
        });
      }
      continue;
    }

    const col0 = data.columns[0]?.name.toLowerCase() ?? "";
    if (
      data.columns.length >= 2 &&
      (col0 === "source" || col0 === "src" || col0 === "src_node_id")
    ) {
      const src = nonEmptyStr(row[0]);
      const dst = nonEmptyStr(row[1]);
      if (src && dst) {
        if (!nodesMap.has(src)) {
          nodesMap.set(src, { id: src, name: src, category: "Node", value: 1 });
        }
        if (!nodesMap.has(dst)) {
          nodesMap.set(dst, { id: dst, name: dst, category: "Node", value: 1 });
        }
        const edgeVal = toFiniteNumber(row[2]);
        links.push({
          source: src,
          target: dst,
          value: edgeVal ?? undefined,
        });
      }
      continue;
    }

    const id =
      nonEmptyStr(readCellByName(row, colMap, "id")) ??
      nonEmptyStr(readCellByName(row, colMap, "node_id")) ??
      nonEmptyStr(row[0]);
    if (!id) continue;
    const nodeKey = nonEmptyStr(readCellByName(row, colMap, "node_key"));
    const title =
      nonEmptyStr(readCellByName(row, colMap, "title")) ??
      nonEmptyStr(readCellByName(row, colMap, "label")) ??
      nodeKey ??
      id;
    const typeName = nonEmptyStr(readCellByName(row, colMap, "type_name", 2)) ?? "Node";
    const val = toFiniteNumber(readCellByName(row, colMap, "metric_value", 3)) ?? 1;
    nodesMap.set(id, { id, name: title, category: typeName, value: val });
  }

  const nodes = Array.from(nodesMap.values());
  const categories = Array.from(new Set(nodes.map((n) => n.category)));
  return { nodes, links, categories };
}

/**
 * Separates `NODES_GEO` tabular data into geo node points and directed fly-line edges
 * when `record_kind` (`"NODE"` vs `"LINK"`) and `src_lng/src_lat/dst_lng/dst_lat` are present.
 */
export function parseNodesGeoData(data: LogicTabularData | null): {
  points: GeoNodePoint[];
  flyLines: GeoFlyLine[];
} {
  if (!data) return { points: [], flyLines: [] };
  const colMap = buildColMap(data);
  const hasRecordKind = colMap.has("record_kind");
  const points: GeoNodePoint[] = [];
  const flyLines: GeoFlyLine[] = [];

  for (const row of data.rows) {
    const rawKind = hasRecordKind
      ? (nonEmptyStr(readCellByName(row, colMap, "record_kind")) ?? "NODE").toUpperCase()
      : "NODE";

    if (hasRecordKind && rawKind === "LINK") {
      const srcLng = toFiniteNumber(readCellByName(row, colMap, "src_lng"));
      const srcLat = toFiniteNumber(readCellByName(row, colMap, "src_lat"));
      const dstLng = toFiniteNumber(readCellByName(row, colMap, "dst_lng"));
      const dstLat = toFiniteNumber(readCellByName(row, colMap, "dst_lat"));
      if (srcLng !== null && srcLat !== null && dstLng !== null && dstLat !== null) {
        flyLines.push({
          linkType: nonEmptyStr(readCellByName(row, colMap, "link_type")) ?? "",
          srcLng,
          srcLat,
          dstLng,
          dstLat,
          weight: toFiniteNumber(readCellByName(row, colMap, "weight")) ?? 1,
        });
      }
      continue;
    }

    const lng = toFiniteNumber(
      readCellByName(row, colMap, "lng") ??
        readCellByName(row, colMap, "geo_lng", hasRecordKind ? -1 : 4)
    );
    const lat = toFiniteNumber(
      readCellByName(row, colMap, "lat") ??
        readCellByName(row, colMap, "geo_lat", hasRecordKind ? -1 : 5)
    );
    if (lng === null || lat === null) continue;

    const nodeId =
      nonEmptyStr(readCellByName(row, colMap, "node_id")) ??
      nonEmptyStr(readCellByName(row, colMap, "id", hasRecordKind ? -1 : 0)) ??
      "";
    const typeName =
      nonEmptyStr(readCellByName(row, colMap, "type_name", hasRecordKind ? -1 : 1)) ??
      "Node";
    const nodeKey =
      nonEmptyStr(readCellByName(row, colMap, "node_key", hasRecordKind ? -1 : 2)) ??
      nodeId;
    const title =
      nonEmptyStr(readCellByName(row, colMap, "title", hasRecordKind ? -1 : 3)) ??
      nodeKey;
    const metricValue = toFiniteNumber(
      readCellByName(row, colMap, "metric_value") ??
        readCellByName(row, colMap, "value", hasRecordKind ? -1 : 6)
    );
    const openAlerts =
      toFiniteNumber(readCellByName(row, colMap, "open_alerts", hasRecordKind ? -1 : 7)) ??
      0;

    points.push({
      nodeId,
      typeName,
      nodeKey,
      title,
      lng,
      lat,
      metricValue,
      openAlerts,
    });
  }

  return { points, flyLines };
}

const TOTAL_LABELS = new Set(["总计", "合计", "TOTAL", "GRAND_TOTAL"]);
const SUBTOTAL_LABELS = new Set(["小计", "SUBTOTAL"]);

/**
 * Classifies a pivot table row using `_row_kind` (if present) or fallback dimension label matching.
 */
export function classifyPivotRow(
  columns: readonly LogicColumn[],
  row: readonly unknown[]
): PivotRowClassification {
  const rowKindIdx = columns.findIndex(
    (c) => c.name.toLowerCase() === "_row_kind"
  );
  if (rowKindIdx >= 0) {
    const rawKind = String(row[rowKindIdx] ?? "")
      .trim()
      .toUpperCase();
    if (rawKind === "TOTAL" || rawKind === "GRAND_TOTAL") {
      return "TOTAL";
    }
    if (rawKind === "SUBTOTAL") {
      return "SUBTOTAL";
    }
  }

  for (let i = 0; i < columns.length; i += 1) {
    if (i === rowKindIdx) continue;
    const cell = row[i];
    if (typeof cell !== "string") continue;
    const trimmed = cell.trim();
    if (!trimmed) continue;
    const upper = trimmed.toUpperCase();
    if (
      TOTAL_LABELS.has(trimmed) ||
      TOTAL_LABELS.has(upper) ||
      trimmed.includes("总计") ||
      trimmed.includes("合计")
    ) {
      return "TOTAL";
    }
    if (
      SUBTOTAL_LABELS.has(trimmed) ||
      SUBTOTAL_LABELS.has(upper) ||
      trimmed.includes("小计")
    ) {
      return "SUBTOTAL";
    }
  }

  return "DETAIL";
}

/**
 * Cleans optional UUID fields (e.g. `flow_id`, `node_id`, `target_node_id`), converting
 * empty or whitespace-only strings to `undefined` so invalid `""` UUIDs are never sent.
 */
export function sanitizeOptionalUuid(
  raw: string | null | undefined
): string | undefined {
  if (typeof raw !== "string") return undefined;
  const trimmed = raw.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

export function resolveActionTargetNodeId(
  configuredNodeId: string | null | undefined,
  selectedNodeId: string | null | undefined
): string | undefined {
  return sanitizeOptionalUuid(configuredNodeId) ?? sanitizeOptionalUuid(selectedNodeId);
}

/**
 * Resolves the `nodeKey` for leaf-level drill-down (`DrillLevel.node_type`).
 * Prefers `node_key_field` from `rowObj` or matching row in `widgetData` when available.
 */
export function resolveLeafDrillNodeKey(params: {
  clickedField: string;
  clickedValue: unknown;
  levelField: string;
  nodeKeyField?: string | null;
  rowObj?: Record<string, unknown>;
  widgetData?: LogicTabularData | null;
}): string {
  const fallback = String(params.clickedValue ?? "");
  const keyField = params.nodeKeyField?.trim();
  if (!keyField) {
    return fallback;
  }

  if (params.rowObj) {
    const direct = nonEmptyStr(params.rowObj[keyField]);
    if (direct !== undefined) {
      return direct;
    }
  }

  if (params.widgetData && params.widgetData.columns.length > 0) {
    const colMap = buildColMap(params.widgetData);
    const keyIdx = colMap.get(keyField) ?? colMap.get(keyField.toLowerCase());
    if (keyIdx !== undefined) {
      const matchField = params.levelField || params.clickedField;
      const matchIdx =
        colMap.get(matchField) ?? colMap.get(matchField.toLowerCase()) ?? 0;
      const matchedRow = params.widgetData.rows.find(
        (r) => String(r[matchIdx] ?? "") === fallback
      );
      if (matchedRow) {
        const fromRow = nonEmptyStr(matchedRow[keyIdx]);
        if (fromRow !== undefined) {
          return fromRow;
        }
      }
    }
  }

  return fallback;
}

/**
 * Detects whether a `LINE_CHART` / `BAR_CHART` dataset matches `NODE_METRIC` multi-node
 * grouping (`[bucket_start | ts, group_key, value]`) when `encoding.series` is not set.
 */
export function detectImplicitNodeMetricGrouping(
  data: LogicTabularData,
  explicitSeries: string | undefined
): { xField: string; seriesField: string; yField: string } | null {
  if (explicitSeries && explicitSeries.trim().length > 0) {
    return null;
  }
  const colMap = buildColMap(data);
  const gkIdx = colMap.get("group_key");
  const valIdx = colMap.get("value");
  const bucketIdx = colMap.get("bucket_start");
  const tsIdx = colMap.get("ts");
  const timeCol =
    bucketIdx !== undefined
      ? data.columns[bucketIdx]?.name ?? "bucket_start"
      : tsIdx !== undefined
        ? data.columns[tsIdx]?.name ?? "ts"
        : null;

  if (!timeCol || gkIdx === undefined || valIdx === undefined) {
    return null;
  }

  const hasGroupKeyValues = data.rows.some((r) => {
    const cell = r[gkIdx];
    return cell !== null && cell !== undefined && String(cell).trim().length > 0;
  });

  if (!hasGroupKeyValues) {
    return null;
  }

  return {
    xField: timeCol,
    seriesField: data.columns[gkIdx]?.name ?? "group_key",
    yField: data.columns[valIdx]?.name ?? "value",
  };
}

export interface BuildEChartsOptionParams {
  kind: string;
  title?: string;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
  data: LogicTabularData;
  isDark?: boolean;
  nodeLabelMap?: Readonly<Record<string, string>>;
}

const ISO_AXIS_TS_RE =
  /^\d{4}-(\d{2})-(\d{2})[T\s](\d{2}):(\d{2})(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?$/;

const UUID_SERIES_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function formatAxisTimestampLabel(raw: string): string {
  const m = ISO_AXIS_TS_RE.exec(raw.trim());
  if (m) {
    return `${m[1]}-${m[2]} ${m[3]}:${m[4]}`;
  }
  return raw;
}

function resolveReadableSeriesName(
  rawSeries: string,
  row: readonly unknown[],
  colMap: Map<string, number>,
  nodeLabelMap?: Readonly<Record<string, string>>
): string {
  const trimmed = rawSeries.trim();
  if (!UUID_SERIES_RE.test(trimmed)) {
    return rawSeries;
  }
  const fromTitle =
    nonEmptyStr(readCellByName(row, colMap, "title")) ??
    nonEmptyStr(readCellByName(row, colMap, "node_title")) ??
    nonEmptyStr(readCellByName(row, colMap, "name"));
  const fromKey = nonEmptyStr(readCellByName(row, colMap, "node_key"));
  if (fromTitle && !UUID_SERIES_RE.test(fromTitle)) {
    return fromKey && fromKey !== fromTitle && !UUID_SERIES_RE.test(fromKey)
      ? `${fromTitle} (${fromKey})`
      : fromTitle;
  }
  if (fromKey && !UUID_SERIES_RE.test(fromKey)) {
    return fromKey;
  }
  const mapped = nodeLabelMap?.[trimmed];
  if (mapped && mapped.trim().length > 0) {
    return mapped.trim();
  }
  return `#${trimmed.slice(-6)}`;
}

export interface BuiltEChartsOptionResult {
  option: Record<string, unknown>;
  clickField: string;
}

interface LogicThresholdRule {
  value: number;
  color: string;
}

function readLogicStr(
  obj: Record<string, unknown> | undefined,
  key: string
): string | undefined {
  if (!obj) return undefined;
  const val = obj[key];
  return typeof val === "string" && val.trim().length > 0 ? val.trim() : undefined;
}

function readLogicNum(
  obj: Record<string, unknown> | undefined,
  key: string
): number | undefined {
  if (!obj) return undefined;
  return toFiniteNumber(obj[key]) ?? undefined;
}

function readLogicStrArray(
  obj: Record<string, unknown> | undefined,
  key: string
): string[] {
  if (!obj) return [];
  const val = obj[key];
  if (Array.isArray(val)) {
    return val
      .filter((item): item is string => typeof item === "string" && item.trim().length > 0)
      .map((item) => item.trim());
  }
  if (typeof val === "string" && val.trim().length > 0) {
    return [val.trim()];
  }
  return [];
}

function parseLogicThresholds(
  options: Record<string, unknown> | undefined
): LogicThresholdRule[] {
  if (!options || !Array.isArray(options.thresholds)) return [];
  const out: LogicThresholdRule[] = [];
  for (const item of options.thresholds) {
    if (typeof item !== "object" && item !== null) continue;
    if (item === null || Array.isArray(item)) continue;
    const rec = item as Record<string, unknown>;
    const v = toFiniteNumber(rec.value);
    const c = readLogicStr(rec, "color");
    if (v !== null && c) {
      out.push({ value: v, color: c });
    }
  }
  return out.sort((a, b) => a.value - b.value);
}

function resolveLogicThresholdColor(
  value: number,
  thresholds: readonly LogicThresholdRule[],
  fallbackColor: string
): string {
  let matched = fallbackColor;
  for (const rule of thresholds) {
    if (value >= rule.value) {
      matched = rule.color;
    }
  }
  return matched;
}

function findFirstNumericColIdx(
  data: LogicTabularData,
  excludeIndices: ReadonlySet<number>
): number {
  for (let i = 0; i < data.columns.length; i += 1) {
    if (excludeIndices.has(i)) continue;
    const dt = (data.columns[i]?.data_type ?? "").toUpperCase();
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
    if (firstRow && toFiniteNumber(firstRow[i]) !== null) {
      return i;
    }
  }
  return data.columns.length > 1 ? 1 : 0;
}

function matchCandidateColumn(
  data: LogicTabularData,
  colMap: Map<string, number>,
  candidates: readonly string[],
  excludeIndices?: ReadonlySet<number>,
  requireNonEmptyRowValue = false
): string | undefined {
  for (const candidate of candidates) {
    const idx = colMap.get(candidate) ?? colMap.get(candidate.toLowerCase());
    if (idx === undefined) continue;
    if (excludeIndices?.has(idx)) continue;
    if (requireNonEmptyRowValue) {
      const hasVal = data.rows.some((r) => {
        const cell = r[idx];
        return cell !== null && cell !== undefined && String(cell).trim().length > 0;
      });
      if (!hasVal) continue;
    }
    return data.columns[idx]?.name ?? candidate;
  }
  return undefined;
}

/**
 * Generates an ECharts option strictly driven by `chartSpec.json` with ZERO JS functions,
 * ensuring 100% isomorphism between browser rendering and server-side ECharts SSR (`render.mjs`).
 */
export function buildEChartsOption(
  params: BuildEChartsOptionParams
): BuiltEChartsOptionResult {
  const {
    kind,
    title = "",
    encoding,
    options,
    data,
    isDark = false,
    nodeLabelMap,
  } = params;
  const palette = [...(isDark ? chartSpec.palette.dark : chartSpec.palette.light)];
  const tokens = isDark ? chartSpec.tokens.dark : chartSpec.tokens.light;
  const fontFamily = chartSpec.fontFamily;
  const baseGrid = {
    top: chartSpec.grid.top,
    right: chartSpec.grid.right,
    bottom: chartSpec.grid.bottom,
    left: chartSpec.grid.left,
    containLabel: chartSpec.grid.containLabel,
  };

  const colMap = buildColMap(data);
  const rows = data.rows;
  const rootTextStyle = {
    fontFamily,
    color: tokens.text,
  };
  const isCartesianAxisChart =
    kind === "LINE_CHART" || kind === "BAR_CHART" || kind === "AREA_CHART";
  const chartVariant = (readLogicStr(options, "chart_variant") ?? "").toUpperCase();
  const isAreaVariant = kind === "AREA_CHART" || chartVariant === "AREA";
  const isLineLike =
    kind === "LINE_CHART" || kind === "AREA_CHART" || chartVariant === "AREA";
  const isStackedBar = chartVariant === "STACKED_BAR";
  const isHorizontalBar = chartVariant === "HORIZONTAL_BAR";
  const isDualAxis = chartVariant === "DUAL_AXIS";
  const secondaryYField = readLogicStr(encoding, "secondary_y");
  const markLineVal = readLogicNum(options, "mark_line_value");
  const showMarkLine = options?.show_mark_line === true;

  const buildPureMarkLine = (): Record<string, unknown> | undefined => {
    if (markLineVal !== undefined) {
      return {
        silent: true,
        symbol: ["none", "none"],
        lineStyle: {
          color: isDark ? "#fb7185" : "#e11d48",
          type: "dashed",
          width: 1.5,
        },
        data: [isHorizontalBar ? { xAxis: markLineVal } : { yAxis: markLineVal }],
      };
    }
    if (showMarkLine) {
      return {
        silent: true,
        symbol: ["none", "none"],
        lineStyle: {
          color: isDark ? "#fb7185" : "#e11d48",
          type: "dashed",
          width: 1.5,
        },
        data: [{ type: "average" }],
      };
    }
    return undefined;
  };

  const baseTooltip: Record<string, unknown> = {
    trigger:
      isCartesianAxisChart &&
      chartVariant !== "FUNNEL" &&
      chartVariant !== "TREEMAP"
        ? "axis"
        : "item",
    backgroundColor: tokens.tooltipBg,
    borderColor: tokens.tooltipBorder,
    textStyle: {
      color: tokens.text,
      fontFamily,
      fontSize: 12,
    },
  };

  if (isCartesianAxisChart) {
    const explicitSeries = readLogicStr(encoding, "series");
    const implicitGroup = detectImplicitNodeMetricGrouping(data, explicitSeries);
    const xCandidates = isLineLike
      ? [
          ...chartSpec.encodingFallbacks.timeCandidates,
          ...chartSpec.encodingFallbacks.categoryCandidates,
        ]
      : [
          ...chartSpec.encodingFallbacks.categoryCandidates,
          ...chartSpec.encodingFallbacks.timeCandidates,
        ];
    const xField =
      readLogicStr(encoding, "x") ??
      implicitGroup?.xField ??
      matchCandidateColumn(data, colMap, xCandidates) ??
      data.columns[0]?.name ??
      "x";
    const xIdx = colMap.get(xField) ?? colMap.get(xField.toLowerCase()) ?? 0;

    const yFieldsEnc = readLogicStrArray(encoding, "y");
    const fallbackSeriesCandidates = chartSpec.encodingFallbacks.seriesCandidates.filter(
      (c) => c !== "group_key"
    );
    const seriesField =
      explicitSeries ??
      implicitGroup?.seriesField ??
      (yFieldsEnc.length === 0
        ? matchCandidateColumn(
            data,
            colMap,
            fallbackSeriesCandidates,
            new Set([xIdx]),
            true
          )
        : undefined);

    const sIdxResolved =
      seriesField !== undefined
        ? colMap.get(seriesField) ?? colMap.get(seriesField.toLowerCase())
        : undefined;

    const excludeForValue = new Set<number>([xIdx]);
    if (sIdxResolved !== undefined) {
      excludeForValue.add(sIdxResolved);
    }
    const fallbackValCol =
      matchCandidateColumn(
        data,
        colMap,
        chartSpec.encodingFallbacks.valueCandidates,
        excludeForValue
      ) ??
      data.columns[findFirstNumericColIdx(data, excludeForValue)]?.name ??
      data.columns[1]?.name ??
      "y";

    const yFields =
      yFieldsEnc.length > 0
        ? yFieldsEnc
        : implicitGroup
          ? [implicitGroup.yField]
          : [fallbackValCol];

    if (seriesField && sIdxResolved !== undefined) {
      const yName = yFields[0] ?? "value";
      const yIdx =
        colMap.get(yName) ??
        colMap.get(yName.toLowerCase()) ??
        findFirstNumericColIdx(data, excludeForValue);

      const categories: string[] = [];
      const catSet = new Set<string>();
      const seriesKeys: string[] = [];
      const serSet = new Set<string>();
      const seriesLabelMap = new Map<string, string>();
      const matrix = new Map<string, number>();

      for (const r of rows) {
        const xv = String(r[xIdx] ?? "");
        const sv = String(r[sIdxResolved] ?? "");
        const yv = toFiniteNumber(r[yIdx]) ?? 0;
        if (!catSet.has(xv)) {
          catSet.add(xv);
          categories.push(xv);
        }
        if (!serSet.has(sv)) {
          serSet.add(sv);
          seriesKeys.push(sv);
          seriesLabelMap.set(
            sv,
            resolveReadableSeriesName(sv, r, colMap, nodeLabelMap)
          );
        }
        matrix.set(`${sv}::${xv}`, (matrix.get(`${sv}::${xv}`) ?? 0) + yv);
      }

      const displayCategories = categories.map(formatAxisTimestampLabel);
      const collapseManyBarSeries = !isLineLike && seriesKeys.length > 6;

      if (collapseManyBarSeries) {
        const singleBarData = categories.map((c, idx) => {
          let sum = 0;
          for (const sk of seriesKeys) {
            sum += matrix.get(`${sk}::${c}`) ?? 0;
          }
          return {
            value: sum,
            itemStyle: {
              color: palette[idx % palette.length] ?? "#2563eb",
              borderRadius: [4, 4, 0, 0],
            },
          };
        });

        return {
          clickField: xField,
          option: {
            animation: false,
            color: palette,
            textStyle: rootTextStyle,
            tooltip: baseTooltip,
            grid: { ...baseGrid, top: 20 },
            xAxis: {
              type: "category",
              data: displayCategories,
              axisLabel: {
                color: tokens.subtext,
                fontFamily,
                fontSize: 11,
                width: 96,
                overflow: "truncate",
                hideOverlap: true,
              },
              axisLine: { lineStyle: { color: tokens.axisLine } },
            },
            yAxis: {
              type: "value",
              splitNumber: 4,
              axisLabel: {
                color: tokens.subtext,
                fontFamily,
                fontSize: 11,
                hideOverlap: true,
              },
              splitLine: { lineStyle: { color: tokens.splitLine, type: "dashed" } },
            },
            series: [
              {
                name: seriesLabelMap.get(seriesKeys[0] ?? "") ?? yName,
                type: "bar",
                barMaxWidth: 28,
                data: singleBarData,
              },
            ],
          },
        };
      }

      return {
        clickField: xField,
        option: {
          animation: false,
          color: palette,
          textStyle: rootTextStyle,
          tooltip: baseTooltip,
          legend: {
            top: 0,
            type: "scroll",
            textStyle: { color: tokens.subtext, fontFamily, fontSize: 11 },
          },
          grid: baseGrid,
          xAxis: {
            type: "category",
            data: displayCategories,
            axisLabel: {
              color: tokens.subtext,
              fontFamily,
              fontSize: 11,
              width: 96,
              overflow: "truncate",
              hideOverlap: true,
            },
            axisLine: { lineStyle: { color: tokens.axisLine } },
          },
          yAxis: {
            type: "value",
            splitNumber: 4,
            axisLabel: {
              color: tokens.subtext,
              fontFamily,
              fontSize: 11,
              hideOverlap: true,
            },
            splitLine: { lineStyle: { color: tokens.splitLine, type: "dashed" } },
          },
          series: seriesKeys.map((sk, seriesIdx) => {
            const seriesColor =
              palette[seriesIdx % palette.length] ?? "#2563eb";
            const sLabel = seriesLabelMap.get(sk) ?? sk;
            return {
              name: sLabel,
              type: isLineLike ? "line" : "bar",
              smooth: isLineLike,
              barMaxWidth: 28,
              itemStyle: { color: seriesColor },
              ...(isLineLike
                ? { lineStyle: { color: seriesColor, width: 2 } }
                : {}),
              ...(kind === "AREA_CHART"
                ? { areaStyle: { opacity: 0.18 } }
                : {}),
              data: categories.map((c) => matrix.get(`${sk}::${c}`) ?? 0),
            };
          }),
        },
      };
    }

    const rawCategories = rows.map((r) =>
      String(readCellByName(r, colMap, xField, 0) ?? "")
    );
    const categories = rawCategories.map(formatAxisTimestampLabel);
    const defaultNumericIdx = findFirstNumericColIdx(data, new Set([xIdx]));
    const pureMarkLine = buildPureMarkLine();

    const categoryAxisDef: Record<string, unknown> = {
      type: "category",
      data: categories,
      ...(isHorizontalBar ? { inverse: true } : {}),
      axisLabel: {
        color: tokens.subtext,
        fontFamily,
        fontSize: 11,
        ...(!isHorizontalBar ? { width: 96, overflow: "truncate" } : {}),
        hideOverlap: true,
      },
      axisLine: { lineStyle: { color: tokens.axisLine } },
    };

    const valueAxisDef: Record<string, unknown> = {
      type: "value",
      splitNumber: 4,
      axisLabel: {
        color: tokens.subtext,
        fontFamily,
        fontSize: 11,
        hideOverlap: true,
      },
      splitLine: { lineStyle: { color: tokens.splitLine, type: "dashed" } },
    };

    const builtSeries: Record<string, unknown>[] = yFields.map(
      (yf, seriesIdx) => {
        const seriesColor = palette[seriesIdx % palette.length] ?? "#2563eb";
        const displaySeriesName = rows[0]
          ? resolveReadableSeriesName(yf, rows[0], colMap, nodeLabelMap)
          : yf;
        return {
          name: displaySeriesName,
          type: isLineLike ? "line" : "bar",
          smooth: isLineLike ? 0.25 : false,
          barMaxWidth: 28,
          ...(isStackedBar && !isLineLike ? { stack: "total" } : {}),
          itemStyle: {
            color: seriesColor,
            ...(!isLineLike && !isStackedBar
              ? {
                  borderRadius: isHorizontalBar ? [0, 4, 4, 0] : [4, 4, 0, 0],
                }
              : {}),
          },
          ...(isLineLike ? { lineStyle: { color: seriesColor, width: 2 } } : {}),
          ...(isAreaVariant ? { areaStyle: { opacity: 0.18 } } : {}),
          ...(seriesIdx === 0 && pureMarkLine ? { markLine: pureMarkLine } : {}),
          data: rows.map(
            (r) =>
              toFiniteNumber(
                readCellByName(r, colMap, yf, defaultNumericIdx)
              ) ?? 0
          ),
        };
      }
    );

    if (isDualAxis && secondaryYField) {
      const secColor = palette[builtSeries.length % palette.length] ?? "#0d9488";
      builtSeries.push({
        name: secondaryYField,
        type: "line",
        yAxisIndex: 1,
        smooth: 0.25,
        itemStyle: { color: secColor },
        lineStyle: { color: secColor, width: 2 },
        data: rows.map(
          (r) =>
            toFiniteNumber(
              readCellByName(r, colMap, secondaryYField, defaultNumericIdx)
            ) ?? 0
        ),
      });
    }

    const baseOption: Record<string, unknown> = {
      animation: false,
      color: palette,
      textStyle: rootTextStyle,
      tooltip: baseTooltip,
      grid: baseGrid,
      xAxis: isHorizontalBar ? valueAxisDef : categoryAxisDef,
      yAxis: isHorizontalBar
        ? categoryAxisDef
        : isDualAxis && secondaryYField
          ? [
              valueAxisDef,
              {
                ...valueAxisDef,
                splitLine: { show: false },
              },
            ]
          : valueAxisDef,
      series: builtSeries,
    };
    if (builtSeries.length > 1) {
      baseOption.legend = {
        top: 0,
        type: "scroll",
        textStyle: { color: tokens.subtext, fontFamily, fontSize: 11 },
      };
    }
    return {
      clickField: xField,
      option: baseOption,
    };
  }

  if (kind === "PIE_CHART") {
    const catField =
      readLogicStr(encoding, "category") ??
      readLogicStr(encoding, "x") ??
      matchCandidateColumn(data, colMap, chartSpec.encodingFallbacks.categoryCandidates) ??
      data.columns[0]?.name ??
      "category";
    const catIdx = colMap.get(catField) ?? colMap.get(catField.toLowerCase()) ?? 0;
    const yArr = readLogicStrArray(encoding, "y");
    const valField =
      readLogicStr(encoding, "value") ??
      yArr[0] ??
      matchCandidateColumn(
        data,
        colMap,
        chartSpec.encodingFallbacks.valueCandidates,
        new Set([catIdx])
      ) ??
      data.columns[1]?.name ??
      "value";

    const pieData = rows.map((r) => ({
      name: String(readCellByName(r, colMap, catField, 0) ?? ""),
      value: toFiniteNumber(readCellByName(r, colMap, valField, 1)) ?? 0,
    }));

    if (chartVariant === "FUNNEL") {
      return {
        clickField: catField,
        option: {
          animation: false,
          color: palette,
          textStyle: rootTextStyle,
          tooltip: baseTooltip,
          legend: {
            bottom: 0,
            type: "scroll",
            textStyle: { color: tokens.subtext, fontFamily, fontSize: 11 },
          },
          series: [
            {
              type: "funnel",
              top: 24,
              bottom: 32,
              left: "12%",
              width: "76%",
              sort: "descending",
              gap: 3,
              label: { color: tokens.text, fontFamily, fontSize: 11 },
              data: pieData,
            },
          ],
        },
      };
    }

    if (chartVariant === "TREEMAP") {
      return {
        clickField: catField,
        option: {
          animation: false,
          color: palette,
          textStyle: rootTextStyle,
          tooltip: baseTooltip,
          series: [
            {
              type: "treemap",
              roam: false,
              breadcrumb: { show: false },
              label: { color: "#ffffff", fontFamily, fontSize: 11 },
              itemStyle: { borderColor: tokens.tooltipBg, borderWidth: 2, gapWidth: 2 },
              data: pieData,
            },
          ],
        },
      };
    }

    const isRose = chartVariant === "ROSE";
    return {
      clickField: catField,
      option: {
        animation: false,
        color: palette,
        textStyle: rootTextStyle,
        tooltip: baseTooltip,
        legend: {
          bottom: 0,
          type: "scroll",
          textStyle: { color: tokens.subtext, fontFamily, fontSize: 11 },
        },
        series: [
          {
            type: "pie",
            ...(isRose
              ? { roseType: "area", radius: ["20%", "70%"] }
              : { radius: [chartSpec.pie.innerRadius, chartSpec.pie.outerRadius] }),
            center: ["50%", "45%"],
            label: { color: tokens.text, fontFamily, fontSize: 11 },
            data: pieData,
          },
        ],
      },
    };
  }

  if (kind === "SCATTER") {
    const xField =
      readLogicStr(encoding, "x") ?? data.columns[0]?.name ?? "x";
    const yField =
      readLogicStr(encoding, "y") ?? data.columns[1]?.name ?? "y";
    const sizeField = readLogicStr(encoding, "size");
    const defaultSize = chartSpec.scatter.defaultSymbolSize;
    const minSize = chartSpec.scatter.minSymbolSize;
    const maxSize = chartSpec.scatter.maxSymbolSize;

    const scatterItems = rows.map((r) => {
      const xv = toFiniteNumber(readCellByName(r, colMap, xField, 0)) ?? 0;
      const yv = toFiniteNumber(readCellByName(r, colMap, yField, 1)) ?? 0;
      const rawSize = sizeField
        ? toFiniteNumber(readCellByName(r, colMap, sizeField, 2))
        : null;
      const clampedSize =
        rawSize !== null
          ? Math.max(minSize, Math.min(maxSize, rawSize))
          : defaultSize;
      return {
        value: rawSize !== null ? [xv, yv, rawSize] : [xv, yv],
        symbolSize: clampedSize,
      };
    });

    return {
      clickField: xField,
      option: {
        animation: false,
        color: palette,
        textStyle: rootTextStyle,
        tooltip: baseTooltip,
        grid: baseGrid,
        xAxis: {
          type: "value",
          name: xField,
          splitNumber: 4,
          axisLabel: {
            color: tokens.subtext,
            fontFamily,
            fontSize: 11,
            hideOverlap: true,
          },
          axisLine: { lineStyle: { color: tokens.axisLine } },
          splitLine: { lineStyle: { color: tokens.splitLine, type: "dashed" } },
        },
        yAxis: {
          type: "value",
          name: yField,
          splitNumber: 4,
          axisLabel: {
            color: tokens.subtext,
            fontFamily,
            fontSize: 11,
            hideOverlap: true,
          },
          axisLine: { lineStyle: { color: tokens.axisLine } },
          splitLine: { lineStyle: { color: tokens.splitLine, type: "dashed" } },
        },
        series: [
          {
            type: "scatter",
            symbolSize: defaultSize,
            data: scatterItems,
          },
        ],
      },
    };
  }

  if (kind === "SANKEY") {
    const srcField =
      readLogicStr(encoding, "source") ??
      chartSpec.encodingFallbacks.sankey.source;
    const dstField =
      readLogicStr(encoding, "target") ??
      chartSpec.encodingFallbacks.sankey.target;
    const valField =
      readLogicStr(encoding, "value") ??
      chartSpec.encodingFallbacks.sankey.value;
    const nodeNames = new Set<string>();
    const links: { source: string; target: string; value: number }[] = [];

    for (const r of rows) {
      const s = String(readCellByName(r, colMap, srcField, 0) ?? "");
      const t = String(readCellByName(r, colMap, dstField, 1) ?? "");
      const v = toFiniteNumber(readCellByName(r, colMap, valField, 2)) ?? 1;
      if (s && t && s !== t) {
        nodeNames.add(s);
        nodeNames.add(t);
        links.push({ source: s, target: t, value: Math.max(0.01, v) });
      }
    }

    return {
      clickField: srcField,
      option: {
        animation: false,
        color: palette,
        textStyle: rootTextStyle,
        tooltip: baseTooltip,
        series: [
          {
            type: "sankey",
            emphasis: { focus: "adjacency" },
            label: { color: tokens.text, fontFamily, fontSize: 11 },
            data: Array.from(nodeNames).map((name) => ({ name })),
            links,
          },
        ],
      },
    };
  }

  if (kind === "HEATMAP") {
    const xField =
      readLogicStr(encoding, "x") ??
      matchCandidateColumn(data, colMap, chartSpec.encodingFallbacks.categoryCandidates) ??
      data.columns[0]?.name ??
      "x";
    const xIdx = colMap.get(xField) ?? colMap.get(xField.toLowerCase()) ?? 0;
    const yField =
      readLogicStr(encoding, "y") ?? data.columns[1]?.name ?? "y";
    const yIdx = colMap.get(yField) ?? colMap.get(yField.toLowerCase()) ?? 1;
    const valField =
      readLogicStr(encoding, "value") ??
      matchCandidateColumn(
        data,
        colMap,
        chartSpec.encodingFallbacks.valueCandidates,
        new Set([xIdx, yIdx])
      ) ??
      data.columns[2]?.name ??
      "value";

    const xCats: string[] = [];
    const yCats: string[] = [];
    const xSet = new Set<string>();
    const ySet = new Set<string>();
    let minV = Infinity;
    let maxV = -Infinity;

    for (const r of rows) {
      const xv = String(readCellByName(r, colMap, xField, 0) ?? "");
      const yv = String(readCellByName(r, colMap, yField, 1) ?? "");
      if (!xSet.has(xv)) {
        xSet.add(xv);
        xCats.push(xv);
      }
      if (!ySet.has(yv)) {
        ySet.add(yv);
        yCats.push(yv);
      }
    }

    const heatData: [number, number, number][] = [];
    for (const r of rows) {
      const xv = String(readCellByName(r, colMap, xField, 0) ?? "");
      const yv = String(readCellByName(r, colMap, yField, 1) ?? "");
      const val = toFiniteNumber(readCellByName(r, colMap, valField, 2)) ?? 0;
      minV = Math.min(minV, val);
      maxV = Math.max(maxV, val);
      heatData.push([xCats.indexOf(xv), yCats.indexOf(yv), val]);
    }

    const primaryColor = palette[0] ?? "#2563eb";
    return {
      clickField: xField,
      option: {
        animation: false,
        textStyle: rootTextStyle,
        tooltip: { position: "top", ...baseTooltip },
        grid: baseGrid,
        xAxis: {
          type: "category",
          data: xCats,
          axisLabel: { color: tokens.subtext, fontFamily, fontSize: 11 },
          axisLine: { lineStyle: { color: tokens.axisLine } },
        },
        yAxis: {
          type: "category",
          data: yCats,
          axisLabel: { color: tokens.subtext, fontFamily, fontSize: 11 },
          axisLine: { lineStyle: { color: tokens.axisLine } },
        },
        visualMap: {
          min: Number.isFinite(minV) ? minV : 0,
          max: Number.isFinite(maxV) && maxV > minV ? maxV : 100,
          orient: "horizontal",
          left: "center",
          bottom: 0,
          inRange: {
            color: isDark
              ? [tokens.tooltipBg, primaryColor]
              : [tokens.splitLine, primaryColor],
          },
          textStyle: { color: tokens.subtext, fontFamily, fontSize: 10 },
        },
        series: [
          {
            type: "heatmap",
            data: heatData,
            label: {
              show: heatData.length <= 48,
              color: tokens.text,
              fontFamily,
              fontSize: 10,
            },
          },
        ],
      },
    };
  }

  if (kind === "GAUGE") {
    const valField =
      readLogicStr(encoding, "value") ??
      matchCandidateColumn(data, colMap, chartSpec.encodingFallbacks.valueCandidates) ??
      "value";
    const val =
      toFiniteNumber(rows[0] ? readCellByName(rows[0], colMap, valField, 0) : 0) ??
      0;
    const min = readLogicNum(options, "min") ?? chartSpec.gauge.defaultMin;
    const max = readLogicNum(options, "max") ?? chartSpec.gauge.defaultMax;
    const unit = readLogicStr(options, "unit") ?? "";
    const thresholds = parseLogicThresholds(options);
    const gaugeColor = resolveLogicThresholdColor(
      val,
      thresholds,
      palette[0] ?? "#2563eb"
    );

    return {
      clickField: valField,
      option: {
        animation: false,
        textStyle: rootTextStyle,
        series: [
          {
            type: "gauge",
            min,
            max,
            progress: { show: true, width: 10, itemStyle: { color: gaugeColor } },
            axisLine: { lineStyle: { width: 10, color: [[1, tokens.axisLine]] } },
            axisTick: { show: false },
            splitLine: {
              length: 8,
              lineStyle: { width: 1.5, color: tokens.subtext },
            },
            axisLabel: {
              distance: 14,
              color: tokens.subtext,
              fontFamily,
              fontSize: 10,
            },
            pointer: { itemStyle: { color: gaugeColor } },
            detail: {
              valueAnimation: false,
              formatter: `{value}${unit ? ` ${unit}` : ""}`,
              color: tokens.text,
              fontFamily,
              fontSize: 16,
              offsetCenter: [0, "65%"],
            },
            data: [{ value: val, name: title }],
            title: {
              color: tokens.subtext,
              fontFamily,
              fontSize: 11,
              offsetCenter: [0, "88%"],
            },
          },
        ],
      },
    };
  }

  if (kind === "RADAR") {
    const indField =
      readLogicStr(encoding, "indicator") ??
      chartSpec.encodingFallbacks.radar.indicator;
    const valField =
      readLogicStr(encoding, "value") ??
      chartSpec.encodingFallbacks.radar.value;
    const explicitSer = readLogicStr(encoding, "series");
    const defaultSerCandidate = chartSpec.encodingFallbacks.radar.series;
    const serField =
      explicitSer ??
      (colMap.has(defaultSerCandidate) ? defaultSerCandidate : undefined);

    const indicators: string[] = [];
    const indSet = new Set<string>();
    const seriesMap = new Map<string, Map<string, number>>();

    let maxVal = 10;
    for (const r of rows) {
      const ind = String(readCellByName(r, colMap, indField, 0) ?? "");
      const val = toFiniteNumber(readCellByName(r, colMap, valField, 1)) ?? 0;
      const ser = serField
        ? String(readCellByName(r, colMap, serField, 2) ?? (title || "当前"))
        : title || "当前";
      maxVal = Math.max(maxVal, val * 1.15);
      if (ind && !indSet.has(ind)) {
        indSet.add(ind);
        indicators.push(ind);
      }
      if (!seriesMap.has(ser)) {
        seriesMap.set(ser, new Map());
      }
      seriesMap.get(ser)?.set(ind, val);
    }

    const radarOption: Record<string, unknown> = {
      animation: false,
      color: palette,
      textStyle: rootTextStyle,
      tooltip: baseTooltip,
      radar: {
        indicator:
          indicators.length > 0
            ? indicators.map((name) => ({ name, max: Math.ceil(maxVal) }))
            : [{ name: "默认", max: chartSpec.gauge.defaultMax }],
        axisName: { color: tokens.subtext, fontFamily, fontSize: 11 },
        splitLine: { lineStyle: { color: tokens.splitLine } },
        axisLine: { lineStyle: { color: tokens.axisLine } },
      },
      series: [
        {
          type: "radar",
          data: Array.from(seriesMap.entries()).map(([name, valMap]) => ({
            name,
            value: indicators.map((ind) => valMap.get(ind) ?? 0),
          })),
        },
      ],
    };
    if (seriesMap.size > 1) {
      radarOption.legend = {
        bottom: 0,
        textStyle: { color: tokens.subtext, fontFamily, fontSize: 11 },
      };
    }

    return {
      clickField: indField,
      option: radarOption,
    };
  }

  // GRAPH_TOPOLOGY ECharts graph renderer (10-column record_kind NODE/LINK + 3-column preview)
  const parsedGraph = parseGraphTopologyData(data);
  const catList = parsedGraph.categories;
  const graphOption: Record<string, unknown> = {
    animation: false,
    color: palette,
    textStyle: rootTextStyle,
    tooltip: baseTooltip,
    series: [
      {
        name: "节点",
        type: "graph",
        layout: "force",
        roam: "move",
        top: catList.length > 1 ? 58 : 16,
        bottom: 24,
        left: 20,
        right: 20,
        edgeSymbol: ["none", "arrow"],
        edgeSymbolSize: [4, 8],
        force: { repulsion: 130, edgeLength: 60, gravity: 0.15 },
        categories: catList.map((name) => ({ name })),
        label: {
          show: parsedGraph.nodes.length <= 60,
          position: "bottom",
          color: tokens.text,
          fontFamily,
          fontSize: 10,
        },
        data: parsedGraph.nodes.map((n) => ({
          id: n.id,
          nodeId: n.id,
          name: n.name,
          value: n.value,
          category: catList.indexOf(n.category),
          symbolSize: 20,
        })),
        links: parsedGraph.links.map((lk) => {
          const linkEntry: Record<string, unknown> = {
            source: lk.source,
            target: lk.target,
          };
          if (lk.value !== undefined) {
            linkEntry.value = lk.value;
          }
          if (lk.label) {
            linkEntry.label = {
              show: parsedGraph.links.length <= 40,
              formatter: lk.label,
              fontSize: 9,
              fontFamily,
              color: tokens.subtext,
            };
          }
          return linkEntry;
        }),
      },
    ],
  };
  if (catList.length > 1) {
    graphOption.legend = {
      top: 4,
      left: 8,
      right: 8,
      type: "scroll",
      textStyle: { color: tokens.subtext, fontFamily, fontSize: 10 },
    };
  }

  return {
    clickField: "node_id",
    option: graphOption,
  };
}

/**
 * Recursively checks whether a value tree contains any JS `function` reference.
 * Server-side ECharts SSR (`render.mjs`) requires 100% pure JSON option objects.
 */
export function containsFunctionValue(value: unknown): boolean {
  if (typeof value === "function") return true;
  if (Array.isArray(value)) {
    return value.some((item) => containsFunctionValue(item));
  }
  if (typeof value === "object" && value !== null) {
    return Object.values(value).some((v) => containsFunctionValue(v));
  }
  return false;
}

/**
 * Strips empty / null / undefined filter values before submitting `DASHBOARD` exports or share `locked_filters`.
 */
export function sanitizeExportFilters(
  values: Record<string, unknown>
): Record<string, unknown> {
  const cleaned: Record<string, unknown> = {};
  for (const [key, val] of Object.entries(values)) {
    if (isNonEmptyFilterValue(val)) {
      cleaned[key] = val;
    }
  }
  return cleaned;
}

/**
 * Merges visitor filter values with share-link `locked_filters`, ensuring locked keys can never be overridden.
 */
export function mergeLockedShareFilters(
  baseValues: Record<string, unknown>,
  lockedFilters: Record<string, unknown> | undefined
): Record<string, unknown> {
  if (!lockedFilters) {
    return { ...baseValues };
  }
  return {
    ...baseValues,
    ...lockedFilters,
  };
}

export interface GridLayoutBox {
  i: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface GridPositionEntry {
  id: string;
  position: {
    x: number;
    y: number;
    w: number;
    h: number;
  };
}

export interface MagneticSnapGuides {
  verticalCols: number[];
  horizontalRows: number[];
}

function boxesOverlap(a: GridLayoutBox, b: GridLayoutBox): boolean {
  return (
    a.x < b.x + b.w &&
    a.x + a.w > b.x &&
    a.y < b.y + b.h &&
    a.y + a.h > b.y
  );
}

/**
 * Resolves `FREE_FLOAT` (`锁定位置`) layout coordinates without vertical upward compaction
 * and without pushing unrelated stationary items out of their `(x, y)` slots (`03-STEP25` §25.4).
 */
export function resolveFreeFloatLayout<T extends GridLayoutBox>(
  layout: readonly T[],
  cols: number,
  activeItemId?: string | null,
  lockedSnapshot?: ReadonlyMap<string, GridLayoutBox>
): T[] {
  const safeCols = Math.max(2, cols);
  const clamped = layout.map((item) => {
    const frozen =
      activeItemId && item.i !== activeItemId
        ? lockedSnapshot?.get(item.i)
        : undefined;
    const src = frozen ?? item;
    const w = Math.max(2, Math.min(safeCols, Math.round(src.w)));
    const h = Math.max(2, Math.round(src.h));
    const x = Math.max(0, Math.min(safeCols - w, Math.round(src.x)));
    const y = Math.max(0, Math.round(src.y));
    return {
      ...item,
      x,
      y,
      w,
      h,
    };
  });

  if (!activeItemId) {
    return clamped;
  }

  const activeIdx = clamped.findIndex((it) => it.i === activeItemId);
  if (activeIdx < 0) {
    return clamped;
  }
  const active = clamped[activeIdx]!;
  const others = clamped.filter((_, idx) => idx !== activeIdx);
  if (!others.some((other) => boxesOverlap(active, other))) {
    return clamped;
  }

  // Search nearest non-overlapping slot for the active item while keeping every other item strictly locked.
  const prevActive = lockedSnapshot?.get(activeItemId);
  if (prevActive && !others.some((other) => boxesOverlap(prevActive, other))) {
    clamped[activeIdx] = {
      ...active,
      x: prevActive.x,
      y: prevActive.y,
      w: prevActive.w,
      h: prevActive.h,
    };
    return clamped;
  }

  const maxBottom = others.reduce((acc, o) => Math.max(acc, o.y + o.h), 0);
  for (let y = Math.max(0, active.y); y <= maxBottom + active.h; y += 1) {
    for (let x = 0; x <= safeCols - active.w; x += 1) {
      const candidate: GridLayoutBox = {
        i: active.i,
        x,
        y,
        w: active.w,
        h: active.h,
      };
      if (!others.some((other) => boxesOverlap(candidate, other))) {
        clamped[activeIdx] = { ...active, x, y };
        return clamped;
      }
    }
  }
  return clamped;
}

/**
 * Pure one-click layout tidying algorithm (`03-STEP25` §25.4 #3):
 * - Groups near-row cards (`|Δy| <= 1`) into clean horizontal bands with uniform top `y` and uniform `h`;
 * - When a row's total width is in `[cols - 2, cols)` (or exceeds `cols`), proportionally scales card widths
 *   to fill `cols` exactly and packs `x` left-to-right with zero ragged gaps.
 */
export function tidyDashboardLayout(
  items: readonly GridPositionEntry[],
  cols: number
): GridPositionEntry[] {
  if (items.length === 0) return [];
  const safeCols = Math.max(2, cols);

  const normalized = items.map((entry, idx) => ({
    id: entry.id,
    order: idx,
    x: Math.max(0, Math.min(safeCols - 2, Math.round(entry.position.x))),
    y: Math.max(0, Math.round(entry.position.y)),
    w: Math.max(2, Math.min(safeCols, Math.round(entry.position.w))),
    h: Math.max(2, Math.round(entry.position.h)),
  }));

  const sorted = [...normalized].sort(
    (a, b) => a.y - b.y || a.x - b.x || a.order - b.order
  );

  const bands: Array<typeof sorted> = [];
  for (const card of sorted) {
    let placed = false;
    for (const band of bands) {
      const anchorY = band[0]!.y;
      const horizConflict = band.some(
        (m) => card.x < m.x + m.w && card.x + card.w > m.x
      );
      const totalWidthIfAdded =
        band.reduce((sum, m) => sum + m.w, 0) + card.w;
      if (
        Math.abs(card.y - anchorY) <= 1 &&
        !horizConflict &&
        totalWidthIfAdded <= safeCols + 2
      ) {
        band.push(card);
        placed = true;
        break;
      }
    }
    if (!placed) {
      bands.push([card]);
    }
  }

  const tidiedById = new Map<string, GridPositionEntry["position"]>();
  let minAllowedY = 0;

  for (const band of bands) {
    band.sort((a, b) => a.x - b.x || a.order - b.order);
    const minBandY = band.reduce((acc, m) => Math.min(acc, m.y), band[0]!.y);
    const uniformY = Math.max(minAllowedY, minBandY);
    const uniformH = band.reduce((acc, m) => Math.max(acc, m.h), 2);
    const totalW = band.reduce((sum, m) => sum + m.w, 0);

    const shouldFillFullRow =
      (totalW >= safeCols - 2 && totalW < safeCols) || totalW > safeCols;

    if (shouldFillFullRow) {
      const scaledWidths = band.map((m) =>
        Math.max(2, Math.floor((m.w / totalW) * safeCols))
      );
      let diff = safeCols - scaledWidths.reduce((s, w) => s + w, 0);
      let cursorIdx = 0;
      while (diff !== 0 && scaledWidths.length > 0) {
        const idx = cursorIdx % scaledWidths.length;
        if (diff > 0) {
          scaledWidths[idx] = (scaledWidths[idx] ?? 2) + 1;
          diff -= 1;
        } else if ((scaledWidths[idx] ?? 2) > 2) {
          scaledWidths[idx] = (scaledWidths[idx] ?? 2) - 1;
          diff += 1;
        } else if (scaledWidths.every((w) => w <= 2)) {
          break;
        }
        cursorIdx += 1;
      }

      let curX = 0;
      band.forEach((m, idx) => {
        const w = Math.min(safeCols - curX, scaledWidths[idx] ?? m.w);
        tidiedById.set(m.id, {
          x: curX,
          y: uniformY,
          w: Math.max(2, w),
          h: uniformH,
        });
        curX += Math.max(2, w);
      });
    } else {
      let curX = 0;
      for (const m of band) {
        const nextX = Math.max(curX, Math.min(safeCols - m.w, m.x));
        tidiedById.set(m.id, {
          x: nextX,
          y: uniformY,
          w: m.w,
          h: uniformH,
        });
        curX = nextX + m.w;
      }
    }

    minAllowedY = uniformY + uniformH;
  }

  return items.map((orig) => ({
    id: orig.id,
    position: tidiedById.get(orig.id) ?? orig.position,
  }));
}

/**
 * Computes active magnetic alignment snap lines (`verticalCols`, `horizontalRows`) when the dragged
 * or resized card's edges align with any neighbor card edge (`03-STEP25` §25.4 #2).
 */
export function computeMagneticSnapGuides(
  active: GridLayoutBox | null,
  layout: readonly GridLayoutBox[]
): MagneticSnapGuides {
  if (!active) {
    return { verticalCols: [], horizontalRows: [] };
  }
  const vSet = new Set<number>();
  const hSet = new Set<number>();
  const activeEdgesX = [active.x, active.x + active.w];
  const activeEdgesY = [active.y, active.y + active.h];

  for (const item of layout) {
    if (item.i === active.i) continue;
    const itemEdgesX = [item.x, item.x + item.w];
    const itemEdgesY = [item.y, item.y + item.h];

    for (const ax of activeEdgesX) {
      if (itemEdgesX.includes(ax)) {
        vSet.add(ax);
      }
    }
    for (const ay of activeEdgesY) {
      if (itemEdgesY.includes(ay)) {
        hSet.add(ay);
      }
    }
  }

  return {
    verticalCols: Array.from(vSet).sort((a, b) => a - b),
    horizontalRows: Array.from(hSet).sort((a, b) => a - b),
  };
}
