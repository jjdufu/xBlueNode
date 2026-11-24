/**
 * Unified ECharts 6 visual design tokens, accessible 10-color light/dark palettes,
 * compact axis formatters, linear area gradients, threshold markLines, and crosshair
 * `tabular-nums` tooltip builders (`03-STEP25` §25.3.1).
 *
 * Kept free of JSX and `@/` path aliases so Node 24 `node:test` can import it directly.
 */

export const LIGHT_CHART_PALETTE: readonly string[] = [
  "#2563eb",
  "#0d9488",
  "#7c3aed",
  "#d97706",
  "#e11d48",
  "#0284c7",
  "#059669",
  "#4f46e5",
  "#ea580c",
  "#475569",
];

export const DARK_CHART_PALETTE: readonly string[] = [
  "#3b82f6",
  "#2dd4bf",
  "#a78bfa",
  "#fbbf24",
  "#fb7185",
  "#38bdf8",
  "#34d399",
  "#818cf8",
  "#fb923c",
  "#94a3b8",
];

export const CHART_ANIMATION_DURATION = 280;
export const CHART_BAR_MAX_WIDTH = 28;
export const VERTICAL_BAR_BORDER_RADIUS: readonly [
  number,
  number,
  number,
  number,
] = [4, 4, 0, 0];
export const HORIZONTAL_BAR_BORDER_RADIUS: readonly [
  number,
  number,
  number,
  number,
] = [0, 4, 4, 0];
export const CHART_LINE_SMOOTH = 0.25;
export const CHART_SYMBOL_SIZE = 5;
export const CHART_HOVER_SYMBOL_SIZE = 8;

export interface ChartSurfaceTokens {
  text: string;
  subtext: string;
  axisLine: string;
  splitLine: string;
  tooltipBg: string;
  tooltipBorder: string;
  crosshair: string;
}

export function getChartPalette(isDark: boolean): string[] {
  return [...(isDark ? DARK_CHART_PALETTE : LIGHT_CHART_PALETTE)];
}

export function getChartSurfaceTokens(isDark: boolean): ChartSurfaceTokens {
  if (isDark) {
    return {
      text: "#f4f4f5",
      subtext: "#a1a1aa",
      axisLine: "#3f3f46",
      splitLine: "#27272a",
      tooltipBg: "#18181b",
      tooltipBorder: "#3f3f46",
      crosshair: "#52525b",
    };
  }
  return {
    text: "#18181b",
    subtext: "#71717a",
    axisLine: "#e4e4e7",
    splitLine: "#f4f4f5",
    tooltipBg: "#ffffff",
    tooltipBorder: "#e4e4e7",
    crosshair: "#a1a1aa",
  };
}

function trimScaledDecimal(scaled: number): string {
  const fixed = Math.abs(scaled) >= 100 ? scaled.toFixed(0) : scaled.toFixed(2);
  return fixed.replace(/\.00$/, "").replace(/(\.\d)0$/, "$1");
}

/**
 * Formats numeric axis labels into locale-aware compact units:
 * - `zh`: `12,000 -> "1.2万"`, `150,000,000 -> "1.5亿"`
 * - `en`: `12,000 -> "12K"`, `150,000,000 -> "150M"`, `2,500,000,000 -> "2.5B"`
 */
export function formatAxisCompactNumber(
  val: number | string,
  locale: string = "zh"
): string {
  const num = typeof val === "number" ? val : Number(String(val).replace(/,/g, ""));
  if (!Number.isFinite(num)) {
    return String(val ?? "");
  }
  if (num === 0) {
    return "0";
  }

  const sign = num < 0 ? "-" : "";
  const abs = Math.abs(num);
  const isZh = locale.toLowerCase().startsWith("zh");

  if (isZh) {
    if (abs >= 100_000_000) {
      return `${sign}${trimScaledDecimal(abs / 100_000_000)}亿`;
    }
    if (abs >= 10_000) {
      return `${sign}${trimScaledDecimal(abs / 10_000)}万`;
    }
    return `${sign}${trimScaledDecimal(abs)}`;
  }

  if (abs >= 1_000_000_000) {
    return `${sign}${trimScaledDecimal(abs / 1_000_000_000)}B`;
  }
  if (abs >= 1_000_000) {
    return `${sign}${trimScaledDecimal(abs / 1_000_000)}M`;
  }
  if (abs >= 1_000) {
    return `${sign}${trimScaledDecimal(abs / 1_000)}K`;
  }
  return `${sign}${trimScaledDecimal(abs)}`;
}

export function hexToRgba(hex: string, alpha: number): string {
  const cleaned = hex.trim().replace(/^#/, "");
  if (!/^[0-9a-fA-F]{6}$/.test(cleaned)) {
    return `rgba(37, 99, 235, ${alpha})`;
  }
  const r = Number.parseInt(cleaned.slice(0, 2), 16);
  const g = Number.parseInt(cleaned.slice(2, 4), 16);
  const b = Number.parseInt(cleaned.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/**
 * Pure JSON ECharts vertical linear gradient (`22% -> 0%` opacity).
 */
export function buildLinearAreaGradient(hexColor: string): Record<string, unknown> {
  return {
    type: "linear",
    x: 0,
    y: 0,
    x2: 0,
    y2: 1,
    colorStops: [
      { offset: 0, color: hexToRgba(hexColor, 0.22) },
      { offset: 1, color: hexToRgba(hexColor, 0) },
    ],
  };
}

/**
 * Pure JSON ECharts `markLine` threshold specification.
 */
export function buildThresholdMarkLine(
  value: number,
  label?: string,
  color = "#e11d48",
  horizontal = false
): Record<string, unknown> {
  const datum: Record<string, unknown> = horizontal
    ? { xAxis: value }
    : { yAxis: value };
  if (label && label.trim().length > 0) {
    datum.name = label.trim();
  }
  return {
    silent: true,
    symbol: ["none", "none"],
    lineStyle: {
      color,
      type: "dashed",
      width: 1.5,
    },
    label: {
      show: true,
      position: "insideEndTop",
      formatter: label && label.trim().length > 0 ? `${label.trim()}: {c}` : "{c}",
      color,
      fontSize: 10,
    },
    data: [datum],
  };
}

function escapeHtml(raw: string): string {
  return raw
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function formatThousandSeparatedValue(
  raw: unknown,
  decimals?: number,
  unit?: string
): string {
  const num =
    typeof raw === "number"
      ? raw
      : typeof raw === "string" && raw.trim().length > 0
        ? Number(raw.replace(/,/g, ""))
        : Number.NaN;
  if (!Number.isFinite(num)) {
    return String(raw ?? "-");
  }
  const formatted =
    typeof decimals === "number" && decimals >= 0 && decimals <= 6
      ? num.toLocaleString("en-US", {
          minimumFractionDigits: decimals,
          maximumFractionDigits: decimals,
        })
      : num.toLocaleString("en-US", {
          maximumFractionDigits: Number.isInteger(num) ? 0 : 2,
        });
  return unit && unit.trim().length > 0 ? `${formatted} ${unit.trim()}` : formatted;
}

function extractTooltipScalar(rawVal: unknown): unknown {
  if (Array.isArray(rawVal)) {
    return rawVal.length > 0 ? rawVal[rawVal.length - 1] : 0;
  }
  if (typeof rawVal === "object" && rawVal !== null && "value" in rawVal) {
    const inner = (rawVal as Record<string, unknown>).value;
    return Array.isArray(inner) ? inner[inner.length - 1] : inner;
  }
  return rawVal;
}

export interface ClientChartEnhanceOptions {
  isDark: boolean;
  locale: string;
  unit?: string;
  decimals?: number;
  markLineValue?: number | null;
  markLineLabel?: string;
}

function patchValueAxisFormatter(
  axisNode: unknown,
  locale: string
): unknown {
  if (Array.isArray(axisNode)) {
    return axisNode.map((item) => patchValueAxisFormatter(item, locale));
  }
  if (typeof axisNode !== "object" || axisNode === null) {
    return axisNode;
  }
  const rec = axisNode as Record<string, unknown>;
  if (rec.type !== "value") {
    return rec;
  }
  const prevLabel =
    typeof rec.axisLabel === "object" && rec.axisLabel !== null
      ? (rec.axisLabel as Record<string, unknown>)
      : {};
  return {
    ...rec,
    axisLabel: {
      ...prevLabel,
      formatter: (v: number | string) => formatAxisCompactNumber(v, locale),
    },
  };
}

/**
 * Enhances a pure-JSON ECharts option on the client with spring transitions (`280ms`),
 * compact numeric axis labels (`万/亿` or `K/M/B`), rounded bars, smooth area gradients,
 * optional threshold `markLine`, and `tabular-nums` thousand-separator crosshair tooltips.
 */
export function enhanceClientEChartsOption(
  baseOption: Record<string, unknown>,
  opts: ClientChartEnhanceOptions
): Record<string, unknown> {
  const palette = getChartPalette(opts.isDark);
  const surface = getChartSurfaceTokens(opts.isDark);

  const rawTooltip =
    typeof baseOption.tooltip === "object" && baseOption.tooltip !== null
      ? (baseOption.tooltip as Record<string, unknown>)
      : {};
  const isAxisTrigger = rawTooltip.trigger === "axis";

  const xAxisRec =
    typeof baseOption.xAxis === "object" &&
    baseOption.xAxis !== null &&
    !Array.isArray(baseOption.xAxis)
      ? (baseOption.xAxis as Record<string, unknown>)
      : null;
  const isHorizontalBar = xAxisRec?.type === "value";

  const rawSeries = Array.isArray(baseOption.series) ? baseOption.series : [];
  const nextSeries = rawSeries.map((sItem, idx) => {
    if (typeof sItem !== "object" || sItem === null) return sItem;
    const s = { ...(sItem as Record<string, unknown>) };
    const seriesColor =
      (typeof s.itemStyle === "object" &&
      s.itemStyle !== null &&
      typeof (s.itemStyle as Record<string, unknown>).color === "string"
        ? ((s.itemStyle as Record<string, unknown>).color as string)
        : undefined) ??
      palette[idx % palette.length] ??
      "#2563eb";

    if (s.type === "bar") {
      const prevItemStyle =
        typeof s.itemStyle === "object" && s.itemStyle !== null
          ? (s.itemStyle as Record<string, unknown>)
          : {};
      const isStacked = typeof s.stack === "string" && s.stack.length > 0;
      s.barMaxWidth = CHART_BAR_MAX_WIDTH;
      s.itemStyle = {
        color: seriesColor,
        ...prevItemStyle,
        borderRadius: isStacked
          ? idx === rawSeries.length - 1
            ? isHorizontalBar
              ? [...HORIZONTAL_BAR_BORDER_RADIUS]
              : [...VERTICAL_BAR_BORDER_RADIUS]
            : [0, 0, 0, 0]
          : isHorizontalBar
            ? [...HORIZONTAL_BAR_BORDER_RADIUS]
            : [...VERTICAL_BAR_BORDER_RADIUS],
      };
    } else if (s.type === "line") {
      s.smooth = CHART_LINE_SMOOTH;
      s.symbolSize = CHART_SYMBOL_SIZE;
      s.emphasis = {
        focus: "series",
        scale: CHART_HOVER_SYMBOL_SIZE / CHART_SYMBOL_SIZE,
      };
      if (s.areaStyle !== undefined && s.areaStyle !== null) {
        s.areaStyle = {
          color: buildLinearAreaGradient(seriesColor),
        };
      }
    }

    if (
      idx === 0 &&
      typeof opts.markLineValue === "number" &&
      Number.isFinite(opts.markLineValue) &&
      !s.markLine &&
      (s.type === "bar" || s.type === "line")
    ) {
      s.markLine = buildThresholdMarkLine(
        opts.markLineValue,
        opts.markLineLabel,
        opts.isDark ? "#fb7185" : "#e11d48",
        isHorizontalBar
      );
    }
    return s;
  });

  const tooltipFormatter = (params: unknown): string => {
    const items = Array.isArray(params) ? params : [params];
    const valid = items.filter(
      (it): it is Record<string, unknown> => typeof it === "object" && it !== null
    );
    if (valid.length === 0) return "";

    const headerRaw = String(valid[0]?.axisValueLabel ?? valid[0]?.name ?? "");
    const rowsHtml = valid
      .map((entry, idx) => {
        const dotColor =
          typeof entry.color === "string"
            ? entry.color
            : palette[idx % palette.length] ?? "#2563eb";
        const seriesName = escapeHtml(String(entry.seriesName ?? entry.name ?? ""));
        const valStr = escapeHtml(
          formatThousandSeparatedValue(
            extractTooltipScalar(entry.value ?? entry.data),
            opts.decimals,
            opts.unit
          )
        );
        return `<div style="display:flex;align-items:center;justify-content:space-between;gap:14px;line-height:1.6;">
          <span style="display:inline-flex;align-items:center;gap:6px;color:${surface.subtext};">
            <span style="display:inline-block;width:7px;height:7px;border-radius:9999px;background:${escapeHtml(dotColor)};"></span>
            <span>${seriesName}</span>
          </span>
          <span style="font-variant-numeric:tabular-nums;font-weight:600;color:${surface.text};">${valStr}</span>
        </div>`;
      })
      .join("");

    const headerHtml = headerRaw
      ? `<div style="margin-bottom:4px;font-weight:600;font-size:11px;color:${surface.text};">${escapeHtml(headerRaw)}</div>`
      : "";
    return `<div style="min-width:136px;font-size:11px;font-variant-numeric:tabular-nums;">${headerHtml}${rowsHtml}</div>`;
  };

  return {
    ...baseOption,
    animation: true,
    animationDuration: CHART_ANIMATION_DURATION,
    animationEasing: "cubicOut",
    color: palette,
    ...(baseOption.xAxis !== undefined
      ? { xAxis: patchValueAxisFormatter(baseOption.xAxis, opts.locale) }
      : {}),
    ...(baseOption.yAxis !== undefined
      ? { yAxis: patchValueAxisFormatter(baseOption.yAxis, opts.locale) }
      : {}),
    tooltip: {
      ...rawTooltip,
      backgroundColor: surface.tooltipBg,
      borderColor: surface.tooltipBorder,
      borderWidth: 1,
      padding: [8, 10],
      ...(isAxisTrigger
        ? {
            axisPointer: {
              type: "cross",
              lineStyle: { color: surface.crosshair, width: 1, type: "dashed" },
              crossStyle: { color: surface.crosshair, width: 1 },
            },
          }
        : {}),
      formatter: tooltipFormatter,
    },
    series: nextSeries,
  };
}
