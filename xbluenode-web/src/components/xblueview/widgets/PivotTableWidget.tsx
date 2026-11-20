import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { formatDisplayValue, toNumericValue } from "../utils";

export interface PivotTableWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  onCellClick?: (field: string, value: unknown) => void;
}

export function PivotTableWidget({
  widget,
  data,
  onCellClick,
}: PivotTableWidgetProps) {
  if (!data || data.columns.length === 0) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        暂无透视表数据
      </div>
    );
  }

  const rowKindIdx = data.columns.findIndex(
    (c) => c.name.toLowerCase() === "_row_kind" || c.name.toLowerCase() === "row_kind"
  );

  const visibleCols = data.columns
    .map((col, idx) => ({ col, idx }))
    .filter((entry) => entry.idx !== rowKindIdx);

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
                colSpan={visibleCols.length}
                className="px-3 py-6 text-center text-muted-foreground"
              >
                透视结果为空
              </td>
            </tr>
          ) : (
            data.rows.map((row, rIdx) => {
              const rowKindRaw =
                rowKindIdx >= 0 ? String(row[rowKindIdx] ?? "") : "";
              const firstCell = String(row[0] ?? "");
              const isGrandTotal =
                rowKindRaw === "GRAND_TOTAL" ||
                firstCell.includes("合计") ||
                firstCell === "TOTAL";
              const isSubtotal =
                rowKindRaw === "SUBTOTAL" || firstCell.includes("小计");

              return (
                <tr
                  key={rIdx}
                  onClick={() => {
                    if (onCellClick && !isGrandTotal && !isSubtotal && visibleCols[0]) {
                      onCellClick(visibleCols[0].col.name, row[visibleCols[0].idx]);
                    }
                  }}
                  className={
                    isGrandTotal
                      ? "bg-xblue-soft/60 font-semibold text-foreground"
                      : isSubtotal
                        ? "bg-muted/70 font-medium"
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
                          isNum ? "text-right font-mono-tech tabular-nums" : "text-left"
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
