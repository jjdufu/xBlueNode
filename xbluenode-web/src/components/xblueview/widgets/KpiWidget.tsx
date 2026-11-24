import { useId, useMemo } from "react";
import { ArrowDownRight, ArrowUpRight, Minus } from "lucide-react";
import { useI18n } from "@/i18n";
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

const TEMPORAL_COL_RE = /date|time|ts|bucket|period|day|month|year|created_at/i;

interface SparklineGeometry {
  linePath: string;
  areaPath: string;
  minPoint: { x: number; y: number; value: number };
  maxPoint: { x: number; y: number; value: number };
}

function buildSmoothSparkline(values: readonly number[]): SparklineGeometry | null {
  if (values.length < 2) return null;
  const width = 104;
  const height = 34;
  const padX = 4;
  const padY = 4;
  const minVal = Math.min(...values);
  const maxVal = Math.max(...values);
  const span = maxVal - minVal || 1;

  const pts = values.map((v, idx) => {
    const x =
      padX + (idx / Math.max(1, values.length - 1)) * (width - padX * 2);
    const y =
      height - padY - ((v - minVal) / span) * (height - padY * 2);
    return { x, y, value: v };
  });

  let minPoint = pts[0]!;
  let maxPoint = pts[0]!;
  for (const pt of pts) {
    if (pt.value <= minPoint.value) minPoint = pt;
    if (pt.value >= maxPoint.value) maxPoint = pt;
  }

  let linePath = `M ${pts[0]!.x.toFixed(1)} ${pts[0]!.y.toFixed(1)}`;
  for (let i = 1; i < pts.length; i += 1) {
    const prev = pts[i - 1]!;
    const curr = pts[i]!;
    const midX = ((prev.x + curr.x) / 2).toFixed(1);
    linePath += ` C ${midX} ${prev.y.toFixed(1)}, ${midX} ${curr.y.toFixed(1)}, ${curr.x.toFixed(1)} ${curr.y.toFixed(1)}`;
  }

  const lastPt = pts[pts.length - 1]!;
  const firstPt = pts[0]!;
  const areaPath = `${linePath} L ${lastPt.x.toFixed(1)} ${height} L ${firstPt.x.toFixed(1)} ${height} Z`;

  return {
    linePath,
    areaPath,
    minPoint,
    maxPoint,
  };
}

export function KpiWidget({ widget, data, onItemClick }: KpiWidgetProps) {
  const { tx } = useI18n();
  const gradientId = useId();
  const row = data?.rows[0];
  const colMap = buildColumnIndexMap(data);

  const valueField = readString(widget.encoding, "value") ?? "value";
  const compareField =
    readString(widget.encoding, "compare_value") ?? "compare_value";
  const sparklineField =
    readString(widget.options, "sparkline_field") ??
    readString(widget.encoding, "sparkline");

  const fallbackNumericIdx = useMemo(() => {
    if (!data) return 0;
    for (let i = 0; i < data.columns.length; i += 1) {
      const dt = String(data.columns[i]?.data_type ?? "").toUpperCase();
      if (
        dt === "INT64" ||
        dt === "FLOAT64" ||
        dt === "DECIMAL" ||
        dt === "NUMBER"
      ) {
        return i;
      }
    }
    return 0;
  }, [data]);

  const rawValue = row
    ? getCell(row, colMap, valueField, fallbackNumericIdx)
    : undefined;
  const rawCompare = row
    ? getCell(
        row,
        colMap,
        compareField,
        data && data.columns.length > 1 && fallbackNumericIdx === 0 ? 1 : -1
      )
    : undefined;

  const numVal = toNumericValue(rawValue);
  const numCompare = toNumericValue(rawCompare);

  const unit = readString(widget.options, "unit");
  const prefix = readString(widget.options, "prefix");
  const suffix = readString(widget.options, "suffix");
  const decimals = readNumber(widget.options, "decimals");
  const subtitle = readString(widget.options, "subtitle");
  const targetValue = readNumber(widget.options, "target_value");

  const thresholds = parseThresholds(widget.options);
  const accentColor =
    thresholds.length > 0
      ? resolveThresholdColor(numVal, thresholds, "#2563eb")
      : undefined;
  const sparkColor = accentColor ?? "#2563eb";

  const sparkline = useMemo<SparklineGeometry | null>(() => {
    if (!data || data.rows.length < 2) return null;
    const hasTemporalCol = data.columns.some((col) => {
      const dt = String(col.data_type ?? "").toUpperCase();
      return (
        dt === "DATE" ||
        dt === "TIMESTAMP" ||
        TEMPORAL_COL_RE.test(col.name)
      );
    });
    if (!sparklineField && !hasTemporalCol) {
      return null;
    }
    const targetField = sparklineField ?? valueField;
    const nums: number[] = [];
    for (const r of data.rows) {
      const cell = getCell(r, colMap, targetField, fallbackNumericIdx);
      const n = toNumericValue(cell);
      if (n !== null) {
        nums.push(n);
      }
    }
    return buildSmoothSparkline(nums);
  }, [colMap, data, fallbackNumericIdx, sparklineField, valueField]);

  let deltaPct: number | null = null;
  if (numVal !== null && numCompare !== null && numCompare !== 0) {
    deltaPct = ((numVal - numCompare) / Math.abs(numCompare)) * 100;
  }

  const targetProgress = useMemo(() => {
    if (
      numVal === null ||
      targetValue === undefined ||
      !Number.isFinite(targetValue) ||
      targetValue <= 0
    ) {
      return null;
    }
    const pct = (numVal / targetValue) * 100;
    return {
      pct,
      clampedWidth: Math.max(0, Math.min(100, pct)),
    };
  }, [numVal, targetValue]);

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
      <div className="flex items-center justify-between gap-3">
        <div
          className="text-2xl md:text-3xl font-semibold tracking-tight font-mono-tech tabular-nums truncate"
          style={accentColor ? { color: accentColor } : undefined}
          data-testid="kpi-value"
        >
          {formattedValue}
        </div>

        {sparkline && (
          <svg
            viewBox="0 0 104 34"
            className="h-9 w-24 shrink-0 overflow-visible"
            data-testid="kpi-sparkline"
            aria-label={tx("趋势火花线", "Trend sparkline")}
          >
            <defs>
              <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor={sparkColor} stopOpacity={0.22} />
                <stop offset="100%" stopColor={sparkColor} stopOpacity={0} />
              </linearGradient>
            </defs>
            <path d={sparkline.areaPath} fill={`url(#${gradientId})`} />
            <path
              d={sparkline.linePath}
              fill="none"
              stroke={sparkColor}
              strokeWidth={1.75}
              strokeLinecap="round"
            />
            <circle
              cx={sparkline.minPoint.x}
              cy={sparkline.minPoint.y}
              r={2.4}
              fill="#94a3b8"
              data-testid="kpi-sparkline-min"
            />
            <circle
              cx={sparkline.maxPoint.x}
              cy={sparkline.maxPoint.y}
              r={2.8}
              fill={sparkColor}
              data-testid="kpi-sparkline-max"
            />
          </svg>
        )}
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
            {tx("对比 ", "vs. ")}
            {formatDisplayValue(numCompare, { unit, prefix, suffix, decimals })}
          </span>
        )}
        {subtitle && <span className="truncate">{subtitle}</span>}
        {!row && <span>{tx("暂无指标数据", "No metric data")}</span>}
      </div>

      {targetProgress && (
        <div className="mt-2 space-y-1" data-testid="kpi-target-section">
          <div className="flex items-center justify-between text-[10px] font-mono-tech tabular-nums text-muted-foreground">
            <span>
              {tx("目标", "Target")}{" "}
              {formatDisplayValue(targetValue, {
                unit,
                prefix,
                suffix,
                decimals,
              })}
            </span>
            <span
              className="font-semibold text-foreground"
              data-testid="kpi-target-pct"
            >
              {targetProgress.pct.toFixed(1)}%
            </span>
          </div>
          <div
            className="h-1 w-full rounded-full bg-muted overflow-hidden"
            data-testid="kpi-target-progress"
          >
            <div
              className="h-full rounded-full transition-all duration-300"
              style={{
                width: `${targetProgress.clampedWidth.toFixed(1)}%`,
                backgroundColor: sparkColor,
              }}
              data-testid="kpi-target-progress-bar"
            />
          </div>
        </div>
      )}
    </div>
  );
}
