import { useI18n } from "@/i18n";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { classifyPivotRow } from "../logic";
import { formatDisplayValue, toNumericValue } from "../utils";

export interface PivotTableWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  onCellClick?: (
    field: string,
    value: unknown,
    rowObj?: Record<string, unknown>
  ) => void;
}

export function PivotTableWidget({
  widget,
  data,
  onCellClick,
}: PivotTableWidgetProps) {
  const { tx } = useI18n();
  if (!data || data.columns.length === 0) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        {tx("暂无透视表数据", "No pivot data")}
      </div>
    );
  }

  const visibleCols = data.columns
    .map((col, idx) => ({ col, idx }))
    .filter((entry) => entry.col.name.toLowerCase() !== "_row_kind");

  return (
    <div
      className="h-full overflow-auto border border-border rounded"
      data-testid={`widget-pivot-${widget.id}`}
    >
      <table className="w-full border-collapse text-xs">
        <thead className="sticky top-0 bg-muted/95 border-b border-border z-10">
          <tr>
            {visibleCols.map(({ col }) => (
              <th
                key={col.name}
                className="px-2.5 py-1.5 text-left font-medium text-muted-foreground whitespace-nowrap"
              >
                {col.name}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {data.rows.length === 0 ? (
            <tr>
              <td
                colSpan={Math.max(1, visibleCols.length)}
                className="px-3 py-6 text-center text-muted-foreground"
              >
                {tx("暂无结果", "Empty result")}
              </td>
            </tr>
          ) : (
            data.rows.map((row, rIdx) => {
              const rowClass = classifyPivotRow(data.columns, row);
              const isGrandTotal = rowClass === "TOTAL";
              const isSubtotal = rowClass === "SUBTOTAL";
              const isSummary = isGrandTotal || isSubtotal;

              return (
                <tr
                  key={rIdx}
                  data-row-kind={rowClass}
                  onClick={() => {
                    if (onCellClick && !isSummary && visibleCols[0]) {
                      const rowObj: Record<string, unknown> = {};
                      data.columns.forEach((c, cIdx) => {
                        rowObj[c.name] = row[cIdx];
                      });
                      onCellClick(
                        visibleCols[0].col.name,
                        row[visibleCols[0].idx],
                        rowObj
                      );
                    }
                  }}
                  className={
                    isGrandTotal
                      ? "bg-xblue-soft/60 font-bold text-foreground"
                      : isSubtotal
                        ? "bg-muted/70 font-semibold text-foreground"
                        : onCellClick
                          ? "hover:bg-muted/40 cursor-pointer"
                          : "hover:bg-muted/30"
                  }
                >
                  {visibleCols.map(({ col, idx }) => {
                    const val = row[idx];
                    const isNum = toNumericValue(val) !== null;
                    return (
                      <td
                        key={`${rIdx}-${col.name}`}
                        className={`px-2.5 py-1.5 whitespace-nowrap ${
                          isNum
                            ? "text-right font-mono-tech tabular-nums"
                            : "text-left"
                        }`}
                      >
                        {formatDisplayValue(val)}
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
  );
}
