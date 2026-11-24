import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  CHART_ANIMATION_DURATION,
  CHART_BAR_MAX_WIDTH,
  DARK_CHART_PALETTE,
  HORIZONTAL_BAR_BORDER_RADIUS,
  LIGHT_CHART_PALETTE,
  VERTICAL_BAR_BORDER_RADIUS,
  buildLinearAreaGradient,
  buildThresholdMarkLine,
  enhanceClientEChartsOption,
  formatAxisCompactNumber,
  formatThousandSeparatedValue,
} from "./chartTheme.ts";

describe("Step 25 chartTheme design tokens & formatters", () => {
  it("provides 10-color accessible light and dark semantic palettes", () => {
    assert.equal(LIGHT_CHART_PALETTE.length, 10);
    assert.equal(DARK_CHART_PALETTE.length, 10);
    assert.equal(LIGHT_CHART_PALETTE[0], "#2563eb");
    assert.equal(DARK_CHART_PALETTE[0], "#3b82f6");
  });

  it("formats Chinese 万/亿 and English K/M/B compact axis numbers accurately", () => {
    assert.equal(formatAxisCompactNumber(0, "zh"), "0");
    assert.equal(formatAxisCompactNumber(980, "zh"), "980");
    assert.equal(formatAxisCompactNumber(12_000, "zh"), "1.2万");
    assert.equal(formatAxisCompactNumber(-12_000, "zh-CN"), "-1.2万");
    assert.equal(formatAxisCompactNumber(150_000_000, "zh"), "1.5亿");

    assert.equal(formatAxisCompactNumber(980, "en"), "980");
    assert.equal(formatAxisCompactNumber(12_000, "en"), "12K");
    assert.equal(formatAxisCompactNumber(150_000_000, "en-US"), "150M");
    assert.equal(formatAxisCompactNumber(2_500_000_000, "en"), "2.5B");
  });

  it("enhances ECharts options with 280ms animation, rounded bars, 22%->0% gradient, markLine, and tabular-nums tooltip", () => {
    const gradient = buildLinearAreaGradient("#2563eb");
    assert.equal(gradient.type, "linear");
    const stops = gradient.colorStops as Array<{ offset: number; color: string }>;
    assert.equal(stops[0]?.color, "rgba(37, 99, 235, 0.22)");
    assert.equal(stops[1]?.color, "rgba(37, 99, 235, 0)");

    const ml = buildThresholdMarkLine(5000, "预警线");
    assert.equal(((ml.data as Array<{ yAxis: number }>)[0])?.yAxis, 5000);

    const enhanced = enhanceClientEChartsOption(
      {
        tooltip: { trigger: "axis" },
        xAxis: { type: "category", data: ["PAID", "PENDING"] },
        yAxis: { type: "value" },
        series: [
          {
            name: "订单金额",
            type: "bar",
            data: [12_500, 3_400],
          },
        ],
      },
      {
        isDark: false,
        locale: "zh",
        unit: "元",
        decimals: 2,
        markLineValue: 10_000,
        markLineLabel: "目标",
      }
    );

    assert.equal(enhanced.animationDuration, CHART_ANIMATION_DURATION);
    const barSeries = (enhanced.series as Array<Record<string, unknown>>)[0];
    assert.equal(barSeries?.barMaxWidth, CHART_BAR_MAX_WIDTH);
    assert.deepEqual(
      (barSeries?.itemStyle as Record<string, unknown>)?.borderRadius,
      [...VERTICAL_BAR_BORDER_RADIUS]
    );
    assert.ok(barSeries?.markLine);

    const horizEnhanced = enhanceClientEChartsOption(
      {
        xAxis: { type: "value" },
        yAxis: { type: "category", data: ["A"] },
        series: [{ name: "A", type: "bar", data: [100] }],
      },
      { isDark: true, locale: "en" }
    );
    const horizBar = (horizEnhanced.series as Array<Record<string, unknown>>)[0];
    assert.deepEqual(
      (horizBar?.itemStyle as Record<string, unknown>)?.borderRadius,
      [...HORIZONTAL_BAR_BORDER_RADIUS]
    );

    assert.equal(formatThousandSeparatedValue(12500.5, 2, "元"), "12,500.50 元");
  });
});
