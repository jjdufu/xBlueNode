import { ArrowDownRight, ArrowUpRight, Minus } from "lucide-react";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import {
  buildColumnIndexMap,
  formatDisplayValue,
  getCell,
  parseThresholds,
  readNumber,
  readString,
  resolveThresholdColor,
  toNumericValue,
} from "../utils";

export interface KpiWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  onItemClick?: (field: string, value: unknown) => void;
}

export function KpiWidget({ widget, data, onItemClick }: KpiWidgetProps) {
  const row = data?.rows[0];
  const colMap = buildColumnIndexMap(data);

  const valueField = readString(widget.encoding, "value") ?? "value";
  const compareField =
    readString(widget.encoding, "compare_value") ?? "compare_value";

  const rawValue = row ? getCell(row, colMap, valueField, 0) : undefined;
  const rawCompare = row
    ? getCell(row, colMap, compareField, data && data.columns.length > 1 ? 1 : -1)
    : undefined;

  const numVal = toNumericValue(rawValue);
  const numCompare = toNumericValue(rawCompare);

  const unit = readString(widget.options, "unit");
  const prefix = readString(widget.options, "prefix");
  const suffix = readString(widget.options, "suffix");
  const decimals = readNumber(widget.options, "decimals");
  const subtitle = readString(widget.options, "subtitle");

  const thresholds = parseThresholds(widget.options);
  const accentColor =
    thresholds.length > 0
      ? resolveThresholdColor(numVal, thresholds, "#2563eb")
      : undefined;

  let deltaPct: number | null = null;
  if (numVal !== null && numCompare !== null && numCompare !== 0) {
    deltaPct = ((numVal - numCompare) / Math.abs(numCompare)) * 100;
  }

  const formattedValue = formatDisplayValue(rawValue, {
    unit,
    prefix,
    suffix,
    decimals,
  });

  const clickable = Boolean(onItemClick && rawValue !== undefined);

  return (
    <div
      className={`h-full flex flex-col justify-center px-2 py-1 ${
        clickable ? "cursor-pointer hover:opacity-90" : ""
      }`}
      data-testid={`widget-kpi-${widget.id}`}
      onClick={() => {
        if (clickable && onItemClick) {
          const field = data?.columns[0]?.name ?? valueField;
          onItemClick(field, rawValue);
        }
      }}
    >
      <div
        className="text-2xl md:text-3xl font-semibold tracking-tight font-mono-tech tabular-nums truncate"
        style={accentColor ? { color: accentColor } : undefined}
        data-testid="kpi-value"
      >
        {formattedValue}
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        {deltaPct !== null && (
          <span
            className={`inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 font-mono-tech text-[11px] font-medium ${
              deltaPct > 0
                ? "bg-xblue-soft text-xblue"
                : deltaPct < 0
                  ? "bg-muted text-foreground"
                  : "bg-muted text-muted-foreground"
            }`}
          >
            {deltaPct > 0 ? (
              <ArrowUpRight className="h-3 w-3" />
            ) : deltaPct < 0 ? (
              <ArrowDownRight className="h-3 w-3" />
            ) : (
              <Minus className="h-3 w-3" />
            )}
            {deltaPct > 0 ? "+" : ""}
            {deltaPct.toFixed(1)}%
          </span>
        )}
        {numCompare !== null && (
          <span className="font-mono-tech text-[11px]">
            对比 {formatDisplayValue(numCompare, { unit, prefix, suffix, decimals })}
          </span>
        )}
        {subtitle && <span className="truncate">{subtitle}</span>}
        {!row && <span>暂无指标数据</span>}
      </div>
    </div>
  );
}
