/**
 * Pure data-contract and transformation logic for xBlueView widgets and dashboard state.
 * Kept free of JSX and bundler path aliases so Node 24 `node:test` can execute it natively.
 */

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
