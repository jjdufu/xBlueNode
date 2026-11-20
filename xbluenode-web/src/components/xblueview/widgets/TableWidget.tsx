import { useMemo, useState } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
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

export function TableWidget({ widget, data, onRowClick }: TableWidgetProps) {
  const [sortColIdx, setSortColIdx] = useState<number | null>(null);
  const [sortAsc, setSortAsc] = useState(true);

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

  if (!data || data.columns.length === 0) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        暂无表格数据
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

  return (
    <div className="h-full flex flex-col overflow-hidden" data-testid={`widget-table-${widget.id}`}>
      <div className="flex-1 overflow-auto border border-border rounded">
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
            {sortedRows.length === 0 ? (
              <tr>
                <td
                  colSpan={data.columns.length}
                  className="px-3 py-6 text-center text-muted-foreground"
                >
                  查询返回 0 行
                </td>
              </tr>
            ) : (
              sortedRows.slice(0, 500).map((row, rIdx) => (
                <tr
                  key={rIdx}
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
                    const isNum = toNumericValue(val) !== null;
                    return (
                      <td
                        key={`${rIdx}-${col.name}`}
                        className={`px-2.5 py-1.5 whitespace-nowrap ${
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
              ))
            )}
          </tbody>
        </table>
      </div>
      {data.truncated && (
        <div className="pt-1 text-[10px] font-mono-tech text-muted-foreground text-right">
          已截断显示前 {data.row_count} 行
        </div>
      )}
    </div>
  );
}
