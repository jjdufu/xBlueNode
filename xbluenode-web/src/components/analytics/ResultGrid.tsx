import { useMemo, useRef, useState } from "react";
import { AlertTriangle, Clock, Code2, ShieldCheck, Table2 } from "lucide-react";
import type { QueryColumn } from "@/lib/api/analytics";
import type { DataType } from "@/lib/api/enums";
import { Badge, Button } from "@/components/ui/primitives";

export interface ResultGridProps {
  columns: QueryColumn[];
  rows: Record<string, unknown>[];
  rowCount?: number;
  truncated?: boolean;
  durationMs?: number | null;
  sql?: string;
  maxHeight?: number;
  testId?: string;
  emptyText?: string;
}

const ROW_HEIGHT = 32;
const OVERSCAN = 15;

const NUMERIC_TYPES: ReadonlySet<DataType> = new Set([
  "INT64",
  "FLOAT64",
  "DECIMAL",
]);

function formatCellValue(value: unknown): { text: string; isNull: boolean } {
  if (value === null || value === undefined) {
    return { text: "NULL", isNull: true };
  }
  if (typeof value === "string") {
    return { text: value, isNull: false };
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return { text: String(value), isNull: false };
  }
  try {
    return { text: JSON.stringify(value), isNull: false };
  } catch {
    return { text: String(value), isNull: false };
  }
}

export function ResultGrid({
  columns,
  rows,
  rowCount,
  truncated = false,
  durationMs = null,
  sql,
  maxHeight = 340,
  testId = "analytics-result-grid",
  emptyText = "执行查询后在此展示结构化结果集",
}: ResultGridProps) {
  const scrollContainerRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [showExecutedSql, setShowExecutedSql] = useState(false);

  const effectiveRowCount = rowCount ?? rows.length;

  const maskedColumnCount = useMemo(
    () =>
      columns.filter((c) => c.masked || c.pii_category !== "NONE").length,
    [columns]
  );

  const { visibleRows, startIndex, topPad, bottomPad } = useMemo(() => {
    const total = rows.length;
    if (total === 0) {
      return {
        visibleRows: [] as Record<string, unknown>[],
        startIndex: 0,
        topPad: 0,
        bottomPad: 0,
      };
    }
    const start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
    const visibleCount = Math.ceil(maxHeight / ROW_HEIGHT) + OVERSCAN * 2;
    const end = Math.min(total, start + visibleCount);
    return {
      visibleRows: rows.slice(start, end),
      startIndex: start,
      topPad: start * ROW_HEIGHT,
      bottomPad: Math.max(0, (total - end) * ROW_HEIGHT),
    };
  }, [maxHeight, rows, scrollTop]);

  return (
    <div
      className="rounded-md border border-border bg-card overflow-hidden"
      data-testid={testId}
    >
      {/* Top Status & Telemetry Bar */}
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border bg-muted/30 px-3 py-2 text-xs">
        <div className="flex flex-wrap items-center gap-2">
          <span className="inline-flex items-center gap-1.5 font-medium text-foreground">
            <Table2 className="h-3.5 w-3.5 text-xblue" />
            结果明细
          </span>
          <Badge variant="neutral" data-testid="result-row-count">
            {effectiveRowCount.toLocaleString()} 行
          </Badge>
          <Badge variant="outline">{columns.length} 列</Badge>
          {durationMs !== null && durationMs !== undefined && (
            <span
              className="inline-flex items-center gap-1 font-mono-tech text-[11px] text-muted-foreground"
              data-testid="result-duration-ms"
            >
              <Clock className="h-3 w-3" />
              {durationMs} ms
            </span>
          )}
          {maskedColumnCount > 0 && (
            <Badge variant="xblue" data-testid="result-pii-badge">
              <ShieldCheck className="mr-1 h-3 w-3" />
              PII 脱敏 {maskedColumnCount} 列
            </Badge>
          )}
        </div>

        {sql && (
          <Button
            type="button"
            variant="ghost"
            className="px-2 py-1 text-[11px]"
            onClick={() => setShowExecutedSql((prev) => !prev)}
            data-testid="result-toggle-rewritten-sql"
          >
            <Code2 className="h-3 w-3" />
            {showExecutedSql ? "隐藏守卫 SQL" : "守卫改写 SQL"}
          </Button>
        )}
      </div>

      {/* Truncated Banner */}
      {truncated && (
        <div
          className="flex items-center gap-2 border-b border-border bg-xblue-soft/60 px-3 py-1.5 text-xs text-foreground"
          data-testid="result-truncated-banner"
        >
          <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-xblue" />
          <span>
            结果集超出单次安全上限，已截断为前{" "}
            <strong className="font-mono-tech">{rows.length.toLocaleString()}</strong>{" "}
            行展示。建议增加过滤条件或聚合维度。
          </span>
        </div>
      )}

      {/* Optional Rewritten Guarded SQL Preview */}
      {showExecutedSql && sql && (
        <div className="border-b border-border bg-muted/40 px-3 py-2">
          <div className="mb-1 text-[10px] uppercase tracking-wider text-muted-foreground">
            SQL Guard Rewritten Query
          </div>
          <pre className="overflow-x-auto font-mono-tech text-[11px] text-foreground whitespace-pre-wrap break-all">
            {sql}
          </pre>
        </div>
      )}

      {/* Table Body with Virtual Scrolling */}
      {columns.length === 0 ? (
        <div className="flex h-36 items-center justify-center px-4 text-xs text-muted-foreground">
          {emptyText}
        </div>
      ) : (
        <div
          ref={scrollContainerRef}
          style={{ maxHeight: `${maxHeight}px` }}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          className="overflow-auto"
          data-testid="result-scroll-viewport"
        >
          <table
            className="w-full border-collapse text-left text-xs"
            data-testid="result-table"
          >
            <thead className="sticky top-0 z-10 bg-muted/95 backdrop-blur-xs border-b border-border">
              <tr>
                <th className="w-12 border-r border-border px-2.5 py-2 text-right font-mono-tech text-[10px] font-medium text-muted-foreground">
                  #
                </th>
                {columns.map((col) => (
                  <th
                    key={col.name}
                    className="border-r border-border last:border-r-0 px-3 py-2 font-medium whitespace-nowrap"
                  >
                    <div className="flex items-center gap-1.5">
                      <span className="font-mono-tech text-foreground">
                        {col.name}
                      </span>
                      <span className="font-mono-tech text-[10px] text-muted-foreground">
                        {col.data_type}
                      </span>
                      {(col.masked || col.pii_category !== "NONE") && (
                        <Badge
                          variant="xblue"
                          className="px-1 py-0 text-[10px]"
                          data-testid={`col-pii-${col.name}`}
                        >
                          {col.pii_category !== "NONE"
                            ? col.pii_category
                            : "MASKED"}
                          {col.masked ? "·脱敏" : ""}
                        </Badge>
                      )}
                    </div>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.length === 0 ? (
                <tr>
                  <td
                    colSpan={columns.length + 1}
                    className="px-4 py-8 text-center text-xs text-muted-foreground"
                  >
                    查询执行成功，返回 0 行记录
                  </td>
                </tr>
              ) : (
                <>
                  {topPad > 0 && (
                    <tr aria-hidden="true" style={{ height: `${topPad}px` }}>
                      <td colSpan={columns.length + 1} />
                    </tr>
                  )}
                  {visibleRows.map((row, idx) => {
                    const rowIndex = startIndex + idx;
                    return (
                      <tr
                        key={rowIndex}
                        style={{ height: `${ROW_HEIGHT}px` }}
                        className="border-b border-border last:border-b-0 hover:bg-muted/40 transition-colors"
                      >
                        <td className="border-r border-border px-2.5 py-1 text-right font-mono-tech text-[10px] text-muted-foreground">
                          {rowIndex + 1}
                        </td>
                        {columns.map((col) => {
                          const formatted = formatCellValue(row[col.name]);
                          const isNumeric = NUMERIC_TYPES.has(col.data_type);
                          return (
                            <td
                              key={col.name}
                              className={`border-r border-border last:border-r-0 px-3 py-1 whitespace-nowrap max-w-[320px] truncate ${
                                isNumeric || col.masked ? "font-mono-tech" : ""
                              } ${isNumeric ? "text-right" : "text-left"} ${
                                formatted.isNull
                                  ? "italic text-muted-foreground/70"
                                  : "text-foreground"
                              }`}
                              title={formatted.text}
                            >
                              {formatted.text}
                            </td>
                          );
                        })}
                      </tr>
                    );
                  })}
                  {bottomPad > 0 && (
                    <tr aria-hidden="true" style={{ height: `${bottomPad}px` }}>
                      <td colSpan={columns.length + 1} />
                    </tr>
                  )}
                </>
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
