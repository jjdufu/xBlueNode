import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import { useI18n } from "@/i18n";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import {
  formatDisplayValue,
  isRecord,
  readNumber,
  readString,
  toNumericValue,
} from "../utils";

export interface TableWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  onRowClick?: (rowObj: Record<string, unknown>) => void;
}

interface ColumnFormatOption {
  label?: string;
  unit?: string;
  decimals?: number;
  align?: "left" | "right";
}

const VIRTUAL_ROW_HEIGHT = 32;
const VIRTUAL_OVERSCAN = 10;

export function TableWidget({ widget, data, onRowClick }: TableWidgetProps) {
  const { tx, fmtNum } = useI18n();
  const [sortColIdx, setSortColIdx] = useState<number | null>(null);
  const [sortAsc, setSortAsc] = useState(true);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(400);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const updateSize = () => {
      if (el.clientHeight > 0) {
        setViewportHeight(el.clientHeight);
      }
    };
    updateSize();
    const ro = new ResizeObserver(updateSize);
    ro.observe(el);
    return () => ro.disconnect();
  }, [data]);

  const columnFormats = useMemo(() => {
    const map = new Map<string, ColumnFormatOption>();
    const rawCols = widget.options?.columns;
    if (Array.isArray(rawCols)) {
      for (const item of rawCols) {
        if (!isRecord(item)) continue;
        const name = readString(item, "name") ?? readString(item, "field");
        if (!name) continue;
        map.set(name, {
          label: readString(item, "label"),
          unit: readString(item, "unit"),
          decimals: readNumber(item, "decimals"),
          align: readString(item, "align") === "right" ? "right" : undefined,
        });
      }
    } else if (isRecord(rawCols)) {
      for (const [colName, cfg] of Object.entries(rawCols)) {
        if (!isRecord(cfg)) continue;
        map.set(colName, {
          label: readString(cfg, "label"),
          unit: readString(cfg, "unit"),
          decimals: readNumber(cfg, "decimals"),
          align: readString(cfg, "align") === "right" ? "right" : undefined,
        });
      }
    }
    return map;
  }, [widget.options]);

  const sortedRows = useMemo(() => {
    const rows = data?.rows ?? [];
    if (sortColIdx === null) return rows;
    const copied = [...rows];
    copied.sort((a, b) => {
      const av = a[sortColIdx];
      const bv = b[sortColIdx];
      const an = toNumericValue(av);
      const bn = toNumericValue(bv);
      if (an !== null && bn !== null) {
        return sortAsc ? an - bn : bn - an;
      }
      const as = av === null || av === undefined ? "" : String(av);
      const bs = bv === null || bv === undefined ? "" : String(bv);
      return sortAsc ? as.localeCompare(bs) : bs.localeCompare(as);
    });
    return copied;
  }, [data?.rows, sortAsc, sortColIdx]);

  const conditionalColorEnabled =
    widget.options?.conditional_color === true ||
    readString(widget.options, "chart_variant") === "CONDITIONAL_TABLE";

  const colRanges = useMemo(() => {
    const ranges = new Map<number, { min: number; max: number }>();
    if (!conditionalColorEnabled || !data) return ranges;
    data.columns.forEach((_, cIdx) => {
      let min = Infinity;
      let max = -Infinity;
      for (const r of data.rows) {
        const n = toNumericValue(r[cIdx]);
        if (n !== null) {
          min = Math.min(min, n);
          max = Math.max(max, n);
        }
      }
      if (Number.isFinite(min) && Number.isFinite(max) && max > min) {
        ranges.set(cIdx, { min, max });
      }
    });
    return ranges;
  }, [conditionalColorEnabled, data]);

  if (!data || data.columns.length === 0) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        {tx("暂无表格数据", "No table data")}
      </div>
    );
  }

  const toggleSort = (idx: number) => {
    if (sortColIdx === idx) {
      setSortAsc((prev) => !prev);
    } else {
      setSortColIdx(idx);
      setSortAsc(true);
    }
  };

  const totalRows = sortedRows.length;
  const startIndex = Math.max(
    0,
    Math.floor(scrollTop / VIRTUAL_ROW_HEIGHT) - VIRTUAL_OVERSCAN
  );
  const visibleCount =
    Math.ceil(Math.max(viewportHeight, 320) / VIRTUAL_ROW_HEIGHT) +
    VIRTUAL_OVERSCAN * 2;
  const endIndex = Math.min(totalRows, startIndex + visibleCount);
  const topSpacerHeight = startIndex * VIRTUAL_ROW_HEIGHT;
  const bottomSpacerHeight = Math.max(
    0,
    (totalRows - endIndex) * VIRTUAL_ROW_HEIGHT
  );
  const visibleRows = sortedRows.slice(startIndex, endIndex);

  return (
    <div
      className="h-full flex flex-col overflow-hidden"
      data-testid={`widget-table-${widget.id}`}
    >
      <div
        ref={scrollRef}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        className="flex-1 overflow-auto border border-border rounded"
      >
        <table className="w-full border-collapse text-xs">
          <thead className="sticky top-0 bg-muted/90 backdrop-blur-xs z-10 border-b border-border">
            <tr>
              {data.columns.map((col, idx) => {
                const fmt = columnFormats.get(col.name);
                const isSorted = sortColIdx === idx;
                return (
                  <th
                    key={col.name}
                    onClick={() => toggleSort(idx)}
                    className={`px-2.5 py-1.5 font-medium text-muted-foreground select-none cursor-pointer hover:text-foreground whitespace-nowrap ${
                      fmt?.align === "right" ? "text-right" : "text-left"
                    }`}
                  >
                    <span className="inline-flex items-center gap-1">
                      {fmt?.label ?? col.name}
                      {isSorted &&
                        (sortAsc ? (
                          <ArrowUp className="h-3 w-3 text-xblue" />
                        ) : (
                          <ArrowDown className="h-3 w-3 text-xblue" />
                        ))}
                    </span>
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {totalRows === 0 ? (
              <tr>
                <td
                  colSpan={data.columns.length}
                  className="px-3 py-6 text-center text-muted-foreground"
                >
                  {tx("暂无行数据", "0 rows")}
                </td>
              </tr>
            ) : (
              <>
                {topSpacerHeight > 0 && (
                  <tr aria-hidden="true">
                    <td
                      colSpan={data.columns.length}
                      style={{
                        height: `${topSpacerHeight}px`,
                        padding: 0,
                        border: 0,
                      }}
                    />
                  </tr>
                )}
                {visibleRows.map((row, offset) => {
                  const rIdx = startIndex + offset;
                  return (
                    <tr
                      key={rIdx}
                      style={{ height: `${VIRTUAL_ROW_HEIGHT}px` }}
                      onClick={() => {
                        if (!onRowClick) return;
                        const obj: Record<string, unknown> = {};
                        data.columns.forEach((c, cIdx) => {
                          obj[c.name] = row[cIdx];
                        });
                        onRowClick(obj);
                      }}
                      className={
                        onRowClick
                          ? "hover:bg-muted/60 cursor-pointer transition-colors"
                          : "hover:bg-muted/30"
                      }
                    >
                      {data.columns.map((col, cIdx) => {
                        const fmt = columnFormats.get(col.name);
                        const val = row[cIdx];
                        const numVal = toNumericValue(val);
                        const isNum = numVal !== null;
                        const range = colRanges.get(cIdx);
                        const heatAlpha =
                          range && numVal !== null
                            ? 0.05 +
                              ((numVal - range.min) / (range.max - range.min)) *
                                0.2
                            : 0;
                        return (
                          <td
                            key={`${rIdx}-${col.name}`}
                            style={
                              heatAlpha > 0
                                ? {
                                    backgroundColor: `rgba(37, 99, 235, ${heatAlpha.toFixed(3)})`,
                                  }
                                : undefined
                            }
                            className={`px-2.5 py-1 whitespace-nowrap ${
                              isNum || fmt?.align === "right"
                                ? "text-right font-mono-tech tabular-nums"
                                : "text-left"
                            }`}
                          >
                            {formatDisplayValue(val, {
                              unit: fmt?.unit,
                              decimals: fmt?.decimals,
                            })}
                          </td>
                        );
                      })}
                    </tr>
                  );
                })}
                {bottomSpacerHeight > 0 && (
                  <tr aria-hidden="true">
                    <td
                      colSpan={data.columns.length}
                      style={{
                        height: `${bottomSpacerHeight}px`,
                        padding: 0,
                        border: 0,
                      }}
                    />
                  </tr>
                )}
              </>
            )}
          </tbody>
        </table>
      </div>
      {data.truncated && (
        <div className="pt-1 text-[10px] font-mono-tech text-muted-foreground text-right">
          {tx(
            `已截断前 ${fmtNum(data.row_count, 0)} 行`,
            `Showing first ${fmtNum(data.row_count, 0)} rows`
          )}
        </div>
      )}
    </div>
  );
}
