import { useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  BarChart3,
  Clock,
  Code2,
  Pin,
  ShieldCheck,
  Table2,
} from "lucide-react";
import { useI18n } from "@/i18n";
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
  onOpenChartStudio?: () => void;
  onPinToDashboard?: () => void;
}

const ROW_HEIGHT = 32;
const OVERSCAN = 15;

const NUMERIC_TYPES: ReadonlySet<DataType> = new Set([
  "INT64",
  "FLOAT64",
  "DECIMAL",
]);

function formatCellValue(
  value: unknown,
  dataType?: string
): { text: string; isNull: boolean } {
  if (value === null || value === undefined) {
    return { text: "NULL", isNull: true };
  }
  if (typeof value === "number") {
    if (dataType === "DECIMAL") {
      return { text: value.toFixed(2), isNull: false };
    }
    return { text: String(value), isNull: false };
  }
  if (typeof value === "string") {
    if (dataType === "DECIMAL" && /^-?\d+(\.\d+)?$/.test(value.trim())) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) {
        return { text: parsed.toFixed(2), isNull: false };
      }
    }
    return { text: value, isNull: false };
  }
  if (typeof value === "boolean") {
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
  emptyText,
  onOpenChartStudio,
  onPinToDashboard,
}: ResultGridProps) {
  const { tx, tEnum, fmtNum } = useI18n();
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
      className="flex h-full flex-col rounded-md border border-border bg-card overflow-hidden"
      data-testid={testId}
    >
      {/* Top Status & Telemetry Bar */}
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border bg-muted/25 px-3 py-2 text-xs">
        <div className="flex flex-wrap items-center gap-2">
          <span className="inline-flex items-center gap-1.5 font-medium text-foreground">
            <Table2 className="h-3.5 w-3.5 text-xblue" />
            {tx("结果明细", "Result Grid")}
          </span>
          <span
            className="text-[11px] font-mono-tech tabular-nums text-muted-foreground"
            data-testid="result-row-count"
          >
            {fmtNum(effectiveRowCount, 0)} {tx("行", "rows")} ·{" "}
            {fmtNum(columns.length, 0)} {tx("列", "cols")}
          </span>
          {durationMs !== null && durationMs !== undefined && (
            <span
              className="inline-flex items-center gap-1 font-mono-tech text-[11px] text-muted-foreground"
              data-testid="result-duration-ms"
            >
              <Clock className="h-3 w-3" />
              {fmtNum(durationMs, 0)} ms
            </span>
          )}
          {maskedColumnCount > 0 && (
            <Badge variant="xblue" data-testid="result-pii-badge">
              <ShieldCheck className="mr-1 h-3 w-3" />
              {tx(
                `脱敏 ${fmtNum(maskedColumnCount, 0)} 列`,
                `${fmtNum(maskedColumnCount, 0)} PII cols`
              )}
            </Badge>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          {columns.length > 0 && onOpenChartStudio && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onOpenChartStudio}
              data-testid="result-open-chart-studio-btn"
            >
              <BarChart3 className="h-3 w-3 text-xblue" />
              {tx("制图", "Chart")}
            </Button>
          )}
          {columns.length > 0 && onPinToDashboard && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onPinToDashboard}
              data-testid="result-pin-dashboard-btn"
            >
              <Pin className="h-3 w-3 text-xblue" />
              {tx("钉入看板", "Pin")}
            </Button>
          )}
          {sql && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setShowExecutedSql((prev) => !prev)}
              data-testid="result-toggle-rewritten-sql"
            >
              <Code2 className="h-3 w-3" />
              {showExecutedSql
                ? tx("收起改写 SQL", "Hide SQL")
                : tx("守卫改写 SQL", "Guarded SQL")}
            </Button>
          )}
        </div>
      </div>

      {/* Truncated Banner */}
      {truncated && (
        <div
          className="flex items-center gap-2 border-b border-border bg-xblue-soft/60 px-3 py-1.5 text-xs text-foreground"
          data-testid="result-truncated-banner"
        >
          <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-xblue" />
          <span>
            {tx(
              `结果超出单次上限，已截断显示前 ${fmtNum(rows.length, 0)} 行。`,
              `Result truncated to the first ${fmtNum(rows.length, 0)} rows.`
            )}
          </span>
        </div>
      )}

      {/* Optional Rewritten Guarded SQL Preview */}
      {showExecutedSql && sql && (
        <div className="border-b border-border bg-muted/40 px-3 py-2">
          <div className="mb-1 text-[10px] uppercase tracking-wider text-muted-foreground">
            {tx("守卫改写查询语句", "SQL Guard Rewritten Query")}
          </div>
          <pre className="overflow-x-auto font-mono-tech text-[11px] text-foreground whitespace-pre-wrap break-all">
            {sql}
          </pre>
        </div>
      )}

      {/* Table Body with Virtual Scrolling */}
      {columns.length === 0 ? (
        <div className="flex flex-1 min-h-[180px] flex-col items-center justify-center gap-1.5 px-4 text-xs text-muted-foreground">
          <Table2 className="h-5 w-5 text-muted-foreground/45" />
          <span>{emptyText ?? tx("暂无查询结果", "Run a query to view results")}</span>
        </div>
      ) : (
        <div
          ref={scrollContainerRef}
          style={{ maxHeight: `${maxHeight}px` }}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          className="flex-1 min-h-0 overflow-auto"
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
                            ? tEnum(col.pii_category)
                            : tx("脱敏", "Masked")}
                          {col.masked ? tx("·脱敏", " · Masked") : ""}
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
                    {tx("返回 0 行记录", "Query returned 0 rows")}
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
                          const formatted = formatCellValue(
                            row[col.name],
                            col.data_type
                          );
                          const isNumeric = NUMERIC_TYPES.has(col.data_type);
                          return (
                            <td
                              key={col.name}
                              className={`border-r border-border last:border-r-0 px-3 py-1 whitespace-nowrap max-w-[320px] truncate ${
                                isNumeric || col.masked
                                  ? "font-mono-tech tabular-nums"
                                  : ""
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
