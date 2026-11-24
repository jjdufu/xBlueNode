import { Suspense, lazy, useMemo, useState } from "react";
import { BarChartHorizontal, Flame, Loader2, Lock, Pin } from "lucide-react";
import { useI18n } from "@/i18n";
import type { FieldRef, PivotResult } from "@/lib/api/analytics";
import type { WidgetDataResponse } from "@/lib/api/xblueview";
import { useTheme } from "@/lib/theme";
import { Button } from "@/components/ui/primitives";
import { classifyPivotRow } from "@/components/xblueview/logic";
import { formatDisplayValue, toNumericValue } from "@/components/xblueview/utils";
import type {
  NormalizedPivotDimension,
  NormalizedPivotMeasure,
} from "./VTablePivotRenderer";

const LazyVTablePivotRenderer = lazy(() => import("./VTablePivotRenderer"));

export interface VTablePivotCanvasProps {
  pivotResult?: PivotResult | null;
  rowFields?: readonly FieldRef[];
  widgetData?: WidgetDataResponse | null;
  initialDataBars?: boolean;
  initialHeatmap?: boolean;
  initialFreeze?: boolean;
  canvasHeight?: number;
  maxTableHeightClass?: string;
  onPin?: () => void;
  onCellClick?: (
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => void;
}

interface UnifiedRow {
  kind: "DATA" | "SUBTOTAL" | "GRAND_TOTAL";
  dimValues: string[];
  measureCells: Array<{ display: string; num: number | null }>;
  rawRecord: Record<string, unknown>;
}

function computeCellHeatmapStyle(
  num: number | null,
  min: number,
  max: number,
  isDark: boolean
): string | undefined {
  if (num === null) return undefined;
  if (num < 0) {
    const denom = Math.max(Math.abs(min), 1e-9);
    const ratio = Math.min(1, Math.max(0.08, Math.abs(num) / denom));
    const alpha = (0.08 + ratio * (isDark ? 0.34 : 0.24)).toFixed(3);
    return isDark
      ? `rgba(251, 113, 133, ${alpha})`
      : `rgba(225, 29, 72, ${alpha})`;
  }
  const span = max - Math.min(0, min);
  const ratio =
    span > 1e-9
      ? Math.min(1, Math.max(0.06, (num - Math.min(0, min)) / span))
      : 0.25;
  const alpha = (0.06 + ratio * (isDark ? 0.32 : 0.22)).toFixed(3);
  return isDark
    ? `rgba(59, 130, 246, ${alpha})`
    : `rgba(37, 99, 235, ${alpha})`;
}

export function VTablePivotCanvas({
  pivotResult,
  rowFields = [],
  widgetData,
  initialDataBars = true,
  initialHeatmap = false,
  initialFreeze = true,
  canvasHeight = 220,
  maxTableHeightClass = "max-h-72",
  onPin,
  onCellClick,
}: VTablePivotCanvasProps) {
  const { tx } = useI18n();
  const { isDark } = useTheme();

  const [showDataBars, setShowDataBars] = useState(initialDataBars);
  const [showHeatmap, setShowHeatmap] = useState(initialHeatmap);
  const [freezeHeaders, setFreezeHeaders] = useState(initialFreeze);

  const normalized = useMemo<{
    dimensions: NormalizedPivotDimension[];
    measures: NormalizedPivotMeasure[];
    rows: UnifiedRow[];
    vtableRecords: Record<string, unknown>[];
  }>(() => {
    const grandLabel = tx("总计", "Grand Total");
    const subLabel = tx("小计", "Subtotal");

    if (pivotResult) {
      const maxHdrLen = pivotResult.row_headers.reduce(
        (acc, h) => Math.max(acc, h.length),
        0
      );
      const dimSlotCount = Math.max(1, rowFields.length, maxHdrLen);
      const dimensions: NormalizedPivotDimension[] = Array.from(
        { length: dimSlotCount },
        (_, idx) => ({
          key: `dim_${idx}`,
          title:
            rowFields[idx]?.label ||
            rowFields[idx]?.field ||
            (idx === 0
              ? tx("汇总维度", "Dimension")
              : `${tx("维度", "Dim")} ${idx + 1}`),
        })
      );

      const measureTitles = pivotResult.column_headers.map(
        (colHdr, cIdx) =>
          colHdr.filter(Boolean).join(" / ") ||
          `${tx("指标", "Measure")} ${cIdx + 1}`
      );

      const colMins = measureTitles.map(() => Number.POSITIVE_INFINITY);
      const colMaxs = measureTitles.map(() => Number.NEGATIVE_INFINITY);

      const unifiedRows: UnifiedRow[] = [];
      const vtableRecords: Record<string, unknown>[] = [];

      pivotResult.cells.forEach((cellRow, rIdx) => {
        const rawKind = pivotResult.row_kinds[rIdx] ?? "DATA";
        const kind: UnifiedRow["kind"] =
          rawKind === "GRAND_TOTAL"
            ? "GRAND_TOTAL"
            : rawKind === "SUBTOTAL"
              ? "SUBTOTAL"
              : "DATA";
        const rawHeaders = pivotResult.row_headers[rIdx] ?? [];
        const hasGrand = rawHeaders.some(
          (h) => h.includes("总计") || h.toLowerCase().includes("total")
        );
        const hasSub = rawHeaders.some(
          (h) => h.includes("小计") || h.toLowerCase().includes("subtotal")
        );

        const dimValues = Array.from({ length: dimSlotCount }, (_, dIdx) => {
          const val = rawHeaders[dIdx] ?? "";
          if (kind === "GRAND_TOTAL" && !hasGrand) {
            return dIdx === 0 ? grandLabel : "";
          }
          if (kind === "SUBTOTAL" && !hasSub && dIdx === dimSlotCount - 1) {
            return val ? `${val} · ${subLabel}` : subLabel;
          }
          return val;
        });

        const record: Record<string, unknown> = {};
        dimensions.forEach((dim, dIdx) => {
          record[dim.key] = dimValues[dIdx] || "-";
        });

        const measureCells = measureTitles.map((_, cIdx) => {
          const cellText = cellRow[cIdx] ?? "";
          const num = toNumericValue(
            typeof cellText === "string"
              ? cellText.replace(/,/g, "")
              : cellText
          );
          if (num !== null && kind === "DATA") {
            colMins[cIdx] = Math.min(colMins[cIdx]!, num);
            colMaxs[cIdx] = Math.max(colMaxs[cIdx]!, num);
          }
          record[`m_${cIdx}`] = num !== null ? num : cellText;
          return {
            display: cellText === "" ? "-" : String(cellText),
            num,
          };
        });

        unifiedRows.push({
          kind,
          dimValues,
          measureCells,
          rawRecord: record,
        });
        if (kind === "DATA") {
          vtableRecords.push(record);
        }
      });

      const measures: NormalizedPivotMeasure[] = measureTitles.map(
        (title, cIdx) => {
          const minVal = Number.isFinite(colMins[cIdx]!) ? colMins[cIdx]! : 0;
          const maxVal = Number.isFinite(colMaxs[cIdx]!) ? colMaxs[cIdx]! : 1;
          return {
            key: `m_${cIdx}`,
            title,
            min: minVal,
            max: maxVal,
          };
        }
      );

      return {
        dimensions,
        measures,
        rows: unifiedRows,
        vtableRecords:
          vtableRecords.length > 0
            ? vtableRecords
            : unifiedRows.map((r) => r.rawRecord),
      };
    }

    if (widgetData && widgetData.columns.length > 0) {
      const visibleEntries = widgetData.columns
        .map((col, idx) => ({ col, idx }))
        .filter((e) => e.col.name.toLowerCase() !== "_row_kind");

      const dimEntries = visibleEntries.filter(({ col, idx }) => {
        if (idx === 0) return true;
        const dt = String(col.data_type).toUpperCase();
        return (
          dt !== "INT64" &&
          dt !== "FLOAT64" &&
          dt !== "DECIMAL" &&
          dt !== "NUMBER"
        );
      });
      const effectiveDims =
        dimEntries.length > 0 ? dimEntries : visibleEntries.slice(0, 1);
      const dimIdxSet = new Set(effectiveDims.map((e) => e.idx));
      const measureEntries = visibleEntries.filter(
        (e) => !dimIdxSet.has(e.idx)
      );

      const dimensions: NormalizedPivotDimension[] = effectiveDims.map((e) => ({
        key: e.col.name,
        title: e.col.name,
      }));

      const colMins = measureEntries.map(() => Number.POSITIVE_INFINITY);
      const colMaxs = measureEntries.map(() => Number.NEGATIVE_INFINITY);
      const unifiedRows: UnifiedRow[] = [];
      const vtableRecords: Record<string, unknown>[] = [];

      widgetData.rows.forEach((rawRow) => {
        const cls = classifyPivotRow(widgetData.columns, rawRow);
        const kind: UnifiedRow["kind"] =
          cls === "TOTAL"
            ? "GRAND_TOTAL"
            : cls === "SUBTOTAL"
              ? "SUBTOTAL"
              : "DATA";

        const fullObj: Record<string, unknown> = {};
        widgetData.columns.forEach((c, cIdx) => {
          fullObj[c.name] = rawRow[cIdx];
        });

        const dimValues = effectiveDims.map((e) =>
          formatDisplayValue(rawRow[e.idx])
        );
        const measureCells = measureEntries.map((e, mIdx) => {
          const val = rawRow[e.idx];
          const num = toNumericValue(val);
          if (num !== null && kind === "DATA") {
            colMins[mIdx] = Math.min(colMins[mIdx]!, num);
            colMaxs[mIdx] = Math.max(colMaxs[mIdx]!, num);
          }
          return {
            display: formatDisplayValue(val),
            num,
          };
        });

        unifiedRows.push({
          kind,
          dimValues,
          measureCells,
          rawRecord: fullObj,
        });
        if (kind === "DATA") {
          vtableRecords.push(fullObj);
        }
      });

      const measures: NormalizedPivotMeasure[] = measureEntries.map(
        (e, mIdx) => ({
          key: e.col.name,
          title: e.col.name,
          min: Number.isFinite(colMins[mIdx]!) ? colMins[mIdx]! : 0,
          max: Number.isFinite(colMaxs[mIdx]!) ? colMaxs[mIdx]! : 1,
        })
      );

      return {
        dimensions,
        measures,
        rows: unifiedRows,
        vtableRecords:
          vtableRecords.length > 0
            ? vtableRecords
            : unifiedRows.map((r) => r.rawRecord),
      };
    }

    return { dimensions: [], measures: [], rows: [], vtableRecords: [] };
  }, [pivotResult, rowFields, tx, widgetData]);

  return (
    <div className="flex flex-col min-h-0" data-testid="vtable-pivot-canvas">
      {/* Inline Visuals & Pin Controls */}
      <div
        className="flex flex-wrap items-center justify-between gap-2 border-b border-border bg-muted/20 px-3 py-1.5 text-xs"
        data-testid="vtable-pivot-toolbar"
      >
        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            aria-pressed={showDataBars}
            onClick={() => setShowDataBars((v) => !v)}
            data-testid="vtable-toggle-databars"
            className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-[11px] font-medium transition-colors cursor-pointer ${
              showDataBars
                ? "border-xblue bg-xblue-soft text-xblue"
                : "border-border bg-background text-muted-foreground hover:text-foreground"
            }`}
          >
            <BarChartHorizontal className="h-3 w-3" />
            <span>{tx("数据条", "Data Bars")}</span>
          </button>

          <button
            type="button"
            aria-pressed={showHeatmap}
            onClick={() => setShowHeatmap((v) => !v)}
            data-testid="vtable-toggle-heatmap"
            className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-[11px] font-medium transition-colors cursor-pointer ${
              showHeatmap
                ? "border-xblue bg-xblue-soft text-xblue"
                : "border-border bg-background text-muted-foreground hover:text-foreground"
            }`}
          >
            <Flame className="h-3 w-3" />
            <span>{tx("热力色阶", "Heatmap")}</span>
          </button>

          <button
            type="button"
            aria-pressed={freezeHeaders}
            onClick={() => setFreezeHeaders((v) => !v)}
            data-testid="vtable-toggle-freeze"
            className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-[11px] font-medium transition-colors cursor-pointer ${
              freezeHeaders
                ? "border-xblue bg-xblue-soft text-xblue"
                : "border-border bg-background text-muted-foreground hover:text-foreground"
            }`}
          >
            <Lock className="h-3 w-3" />
            <span>{tx("冻结行列头", "Freeze")}</span>
          </button>
        </div>

        {onPin && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={onPin}
            data-testid="vtable-pin-btn"
          >
            <Pin className="h-3 w-3 text-xblue" />
            {tx("钉入看板", "Pin")}
          </Button>
        )}
      </div>

      {/* Async Lazy-Loaded VisActor VTable Canvas */}
      {normalized.dimensions.length > 0 && normalized.measures.length > 0 && (
        <Suspense
          fallback={
            <div
              className="flex items-center justify-center gap-2 border-b border-border bg-card text-xs text-muted-foreground"
              style={{ height: `${canvasHeight}px` }}
              data-testid="vtable-loading"
            >
              <Loader2 className="h-4 w-4 animate-spin text-xblue" />
              <span>{tx("加载透视画布…", "Loading pivot canvas…")}</span>
            </div>
          }
        >
          <LazyVTablePivotRenderer
            rowDimensions={normalized.dimensions}
            measures={normalized.measures}
            records={normalized.vtableRecords}
            isDark={isDark}
            showDataBars={showDataBars}
            showHeatmap={showHeatmap}
            freezeHeaders={freezeHeaders}
            height={canvasHeight}
            onCellClick={onCellClick}
          />
        </Suspense>
      )}

      {/* Synchronized Semantic Summary & Detail Matrix */}
      <div className={`${maxTableHeightClass} overflow-auto`}>
        <table
          className="w-full border-collapse text-left text-xs"
          data-testid="pivot-result-table"
        >
          <thead
            className={`${
              freezeHeaders ? "sticky top-0 z-10" : ""
            } bg-muted border-b border-border`}
          >
            <tr>
              {normalized.dimensions.map((dim, dIdx) => (
                <th
                  key={dim.key}
                  className={`border-r border-border px-3 py-1.5 font-mono-tech font-semibold text-foreground whitespace-nowrap ${
                    freezeHeaders && dIdx === 0 ? "sticky left-0 z-20 bg-muted" : ""
                  }`}
                >
                  {dim.title}
                </th>
              ))}
              {normalized.measures.map((m) => (
                <th
                  key={m.key}
                  className="border-r border-border last:border-r-0 px-3 py-1.5 text-right font-mono-tech font-semibold text-foreground whitespace-nowrap"
                >
                  {m.title}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {normalized.rows.length === 0 ? (
              <tr>
                <td
                  colSpan={Math.max(
                    1,
                    normalized.dimensions.length + normalized.measures.length
                  )}
                  className="px-3 py-6 text-center text-muted-foreground"
                >
                  {tx("暂无结果", "Empty result")}
                </td>
              </tr>
            ) : (
              normalized.rows.map((row, rIdx) => {
                const isGrand = row.kind === "GRAND_TOTAL";
                const isSub = row.kind === "SUBTOTAL";
                const isSummary = isGrand || isSub;
                const rowClass = isGrand
                  ? "bg-xblue-soft/60 font-semibold border-t-2 border-border"
                  : isSub
                    ? "bg-muted/60 font-medium border-t border-border"
                    : onCellClick
                      ? "border-b border-border hover:bg-muted/40 cursor-pointer"
                      : "border-b border-border hover:bg-muted/30";

                return (
                  <tr
                    key={rIdx}
                    data-row-kind={
                      isGrand ? "TOTAL" : isSub ? "SUBTOTAL" : "DETAIL"
                    }
                    data-testid={
                      isGrand
                        ? "pivot-row-grand-total"
                        : isSub
                          ? "pivot-row-subtotal"
                          : `pivot-row-data-${rIdx}`
                    }
                    className={rowClass}
                    onClick={() => {
                      if (
                        onCellClick &&
                        !isSummary &&
                        normalized.dimensions[0]
                      ) {
                        const firstKey = normalized.dimensions[0].key;
                        onCellClick(
                          firstKey,
                          row.rawRecord[firstKey] ?? row.dimValues[0],
                          row.rawRecord
                        );
                      }
                    }}
                  >
                    {row.dimValues.map((hdrText, dIdx) => (
                      <td
                        key={`rh-${dIdx}`}
                        className={`border-r border-border px-3 py-1.5 font-mono-tech whitespace-nowrap ${
                          freezeHeaders && dIdx === 0 && !isSummary
                            ? "sticky left-0 z-10 bg-card"
                            : ""
                        }`}
                      >
                        {hdrText || (row.kind === "DATA" ? "-" : "")}
                      </td>
                    ))}
                    {row.measureCells.map((cell, cIdx) => {
                      const mMeta = normalized.measures[cIdx];
                      const maxAbs = mMeta
                        ? Math.max(Math.abs(mMeta.max), Math.abs(mMeta.min), 1)
                        : 1;
                      const barPct =
                        showDataBars && !isSummary && cell.num !== null
                          ? Math.min(
                              100,
                              Math.max(4, (Math.abs(cell.num) / maxAbs) * 100)
                            )
                          : 0;
                      const heatBg =
                        showHeatmap && !isSummary && mMeta
                          ? computeCellHeatmapStyle(
                              cell.num,
                              mMeta.min,
                              mMeta.max,
                              isDark
                            )
                          : undefined;

                      return (
                        <td
                          key={`cell-${cIdx}`}
                          style={
                            heatBg ? { backgroundColor: heatBg } : undefined
                          }
                          data-heatmap-active={heatBg ? "true" : undefined}
                          className="relative border-r border-border last:border-r-0 px-3 py-1.5 text-right font-mono-tech tabular-nums whitespace-nowrap overflow-hidden"
                        >
                          {barPct > 0 && (
                            <span
                              aria-hidden="true"
                              data-testid="vtable-databar-fill"
                              style={{ width: `${barPct.toFixed(1)}%` }}
                              className={`pointer-events-none absolute inset-y-1 left-1 rounded-xs ${
                                (cell.num ?? 0) < 0
                                  ? "bg-rose-500/20"
                                  : "bg-xblue/20"
                              }`}
                            />
                          )}
                          <span className="relative z-10">{cell.display}</span>
                        </td>
                      );
                    })}
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
