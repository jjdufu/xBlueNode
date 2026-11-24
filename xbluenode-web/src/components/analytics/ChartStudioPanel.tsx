import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Activity,
  BarChart2,
  BarChart3,
  BarChartHorizontal,
  CheckCircle2,
  CircleDot,
  Filter,
  Flame,
  Gauge,
  GitBranch,
  Hash,
  Layers,
  LayoutGrid,
  LineChart,
  PieChart,
  Pin,
  Play,
  Plus,
  Radar,
  Save,
  Table2,
  Trash2,
  TrendingUp,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  AnalyticsCatalogResponse,
  MetricDef,
  QueryColumn,
  SavedQuery,
} from "@/lib/api/analytics";
import type { DataType, WidgetKind } from "@/lib/api/enums";
import {
  xblueviewApi,
  type SavedChartRecord,
  type WidgetDataResponse,
  type WidgetDataSource,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { useTheme } from "@/lib/theme";
import {
  Badge,
  Button,
  Card,
  Input,
  SegmentedControl,
  Select,
  Textarea,
} from "@/components/ui/primitives";
import { EChartsWidget } from "@/components/xblueview/widgets/EChartsWidget";
import { KpiWidget } from "@/components/xblueview/widgets/KpiWidget";
import { TableWidget } from "@/components/xblueview/widgets/TableWidget";
import {
  PinToDashboardModal,
  type PinChartDraftSpec,
} from "./PinToDashboardModal";

export const CHART_STUDIO_SEED_STORAGE_KEY = "axpoint.chartStudioSeed.v1";

export interface ChartStudioSeed {
  title?: string;
  dataSource: WidgetDataSource;
  columns?: QueryColumn[];
  rows?: Record<string, unknown>[];
  suggestedKind?: WidgetKind;
  suggestedX?: string;
  suggestedY?: string[];
}

export interface ChartStudioPanelProps {
  catalog: AnalyticsCatalogResponse;
  metrics: MetricDef[];
  savedQueries: SavedQuery[];
  seed?: ChartStudioSeed | null;
}

type StudioChartId =
  | "bar"
  | "stacked_bar"
  | "horizontal_bar"
  | "line"
  | "area"
  | "dual_axis"
  | "pie"
  | "scatter"
  | "sankey"
  | "funnel"
  | "heatmap"
  | "radar"
  | "gauge"
  | "treemap"
  | "kpi"
  | "table";

interface StudioChartTypeDef {
  id: StudioChartId;
  widgetKind: WidgetKind;
  variant: string;
  zh: string;
  en: string;
  icon: typeof BarChart3;
}

const STUDIO_CHART_TYPES: readonly StudioChartTypeDef[] = [
  {
    id: "bar",
    widgetKind: "BAR_CHART",
    variant: "BAR",
    zh: "柱状图",
    en: "Bar",
    icon: BarChart3,
  },
  {
    id: "stacked_bar",
    widgetKind: "BAR_CHART",
    variant: "STACKED_BAR",
    zh: "堆叠柱图",
    en: "Stacked",
    icon: Layers,
  },
  {
    id: "horizontal_bar",
    widgetKind: "BAR_CHART",
    variant: "HORIZONTAL_BAR",
    zh: "条形图",
    en: "Horizontal",
    icon: BarChartHorizontal,
  },
  {
    id: "line",
    widgetKind: "LINE_CHART",
    variant: "LINE",
    zh: "折线图",
    en: "Line",
    icon: LineChart,
  },
  {
    id: "area",
    widgetKind: "LINE_CHART",
    variant: "AREA",
    zh: "渐变面积",
    en: "Area",
    icon: TrendingUp,
  },
  {
    id: "dual_axis",
    widgetKind: "BAR_CHART",
    variant: "DUAL_AXIS",
    zh: "双轴组合",
    en: "Dual Axis",
    icon: BarChart2,
  },
  {
    id: "pie",
    widgetKind: "PIE_CHART",
    variant: "DONUT",
    zh: "饼环玫瑰",
    en: "Pie & Rose",
    icon: PieChart,
  },
  {
    id: "scatter",
    widgetKind: "SCATTER",
    variant: "BUBBLE",
    zh: "散点气泡",
    en: "Bubble",
    icon: CircleDot,
  },
  {
    id: "sankey",
    widgetKind: "SANKEY",
    variant: "SANKEY",
    zh: "桑基流向",
    en: "Sankey",
    icon: GitBranch,
  },
  {
    id: "funnel",
    widgetKind: "PIE_CHART",
    variant: "FUNNEL",
    zh: "漏斗转化",
    en: "Funnel",
    icon: Filter,
  },
  {
    id: "heatmap",
    widgetKind: "HEATMAP",
    variant: "HEATMAP",
    zh: "热力矩阵",
    en: "Heatmap",
    icon: Flame,
  },
  {
    id: "radar",
    widgetKind: "RADAR",
    variant: "RADAR",
    zh: "雷达图",
    en: "Radar",
    icon: Radar,
  },
  {
    id: "gauge",
    widgetKind: "GAUGE",
    variant: "GAUGE",
    zh: "仪表盘",
    en: "Gauge",
    icon: Gauge,
  },
  {
    id: "treemap",
    widgetKind: "PIE_CHART",
    variant: "TREEMAP",
    zh: "矩形树图",
    en: "Treemap",
    icon: LayoutGrid,
  },
  {
    id: "kpi",
    widgetKind: "KPI_CARD",
    variant: "KPI_SPARKLINE",
    zh: "指标火花",
    en: "KPI Card",
    icon: Hash,
  },
  {
    id: "table",
    widgetKind: "TABLE",
    variant: "CONDITIONAL_TABLE",
    zh: "色阶明细",
    en: "Color Table",
    icon: Table2,
  },
];

const NUMERIC_TYPES: ReadonlySet<string> = new Set([
  "INT64",
  "FLOAT64",
  "DECIMAL",
  "NUMBER",
]);

function resolveStudioIdFromSaved(
  widgetKind: WidgetKind,
  options: Record<string, unknown>
): StudioChartId {
  const variant =
    typeof options.chart_variant === "string"
      ? options.chart_variant.toUpperCase()
      : "";
  if (variant === "STACKED_BAR") return "stacked_bar";
  if (variant === "HORIZONTAL_BAR") return "horizontal_bar";
  if (variant === "AREA") return "area";
  if (variant === "DUAL_AXIS") return "dual_axis";
  if (variant === "FUNNEL") return "funnel";
  if (variant === "TREEMAP") return "treemap";
  if (variant === "ROSE" || variant === "DONUT") return "pie";
  switch (widgetKind) {
    case "LINE_CHART":
      return "line";
    case "PIE_CHART":
      return "pie";
    case "SCATTER":
      return "scatter";
    case "SANKEY":
      return "sankey";
    case "HEATMAP":
      return "heatmap";
    case "RADAR":
      return "radar";
    case "GAUGE":
      return "gauge";
    case "KPI_CARD":
      return "kpi";
    case "TABLE":
      return "table";
    default:
      return "bar";
  }
}

function convertRecordsToWidgetData(
  columns: readonly QueryColumn[],
  rows: readonly Record<string, unknown>[]
): WidgetDataResponse {
  return {
    columns: columns.map((c) => ({
      name: c.name,
      data_type: c.data_type as DataType,
    })),
    rows: rows.map((rec) => columns.map((c) => rec[c.name] ?? null)),
    row_count: rows.length,
    truncated: false,
    generated_at: new Date().toISOString(),
    data_version: "studio-local",
    drill_level: 0,
  };
}

export function ChartStudioPanel({
  catalog,
  metrics,
  savedQueries,
  seed,
}: ChartStudioPanelProps) {
  const { isZh, tx, fmtNum } = useI18n();
  const { isDark } = useTheme();
  const { can } = usePrincipal();
  const canEditView = can("VIEW_EDIT");

  const [savedCharts, setSavedCharts] = useState<SavedChartRecord[]>([]);
  const [activeChartId, setActiveChartId] = useState<string>("");
  const [chartName, setChartName] = useState("");
  const [studioTypeId, setStudioTypeId] = useState<StudioChartId>("bar");
  const [pieStyle, setPieStyle] = useState<"DONUT" | "ROSE">("DONUT");

  const [sourceKind, setSourceKind] = useState<
    "SQL" | "METRIC" | "SAVED_QUERY"
  >("SQL");
  const [sqlText, setSqlText] = useState(
    "SELECT status, SUM(amount) AS total_amt FROM demo_orders GROUP BY status"
  );
  const [metricKey, setMetricKey] = useState("");
  const [savedQueryId, setSavedQueryId] = useState("");

  const [xField, setXField] = useState("");
  const [yFields, setYFields] = useState<string[]>([]);
  const [secondaryYField, setSecondaryYField] = useState("");
  const [seriesField, setSeriesField] = useState("");
  const [sizeField, setSizeField] = useState("");
  const [enableMarkLine, setEnableMarkLine] = useState(false);
  const [markLineValue, setMarkLineValue] = useState("");
  const [unit, setUnit] = useState("");
  const [decimals, setDecimals] = useState<string>("2");
  const [targetValue, setTargetValue] = useState("");

  const [previewData, setPreviewData] = useState<WidgetDataResponse | null>(
    null
  );
  const [previewLoading, setPreviewLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [statusNotice, setStatusNotice] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [pinDraft, setPinDraft] = useState<PinChartDraftSpec | null>(null);

  const loadSavedCharts = useCallback(async () => {
    try {
      const res = await xblueviewApi.listSavedCharts({ limit: 100 });
      setSavedCharts(Array.isArray(res?.items) ? res.items : []);
    } catch {
      setSavedCharts([]);
    }
  }, []);

  useEffect(() => {
    void loadSavedCharts();
  }, [loadSavedCharts]);

  const applySeed = useCallback(
    (incoming: ChartStudioSeed) => {
      setActiveChartId("");
      setStatusNotice(null);
      setErrorMsg(null);
      if (incoming.title) {
        setChartName(incoming.title);
      }
      if (incoming.suggestedKind) {
        setStudioTypeId(resolveStudioIdFromSaved(incoming.suggestedKind, {}));
      }
      if (incoming.dataSource.kind === "SQL") {
        setSourceKind("SQL");
        setSqlText(incoming.dataSource.sql);
      } else if (incoming.dataSource.kind === "METRIC") {
        setSourceKind("METRIC");
        setMetricKey(incoming.dataSource.metric);
      } else if (incoming.dataSource.kind === "SAVED_QUERY") {
        setSourceKind("SAVED_QUERY");
        setSavedQueryId(incoming.dataSource.saved_query_id);
      }

      if (incoming.columns && incoming.columns.length > 0 && incoming.rows) {
        const wd = convertRecordsToWidgetData(incoming.columns, incoming.rows);
        setPreviewData(wd);
        const firstDim =
          incoming.suggestedX ??
          incoming.columns.find(
            (c) => !NUMERIC_TYPES.has(String(c.data_type).toUpperCase())
          )?.name ??
          incoming.columns[0]?.name ??
          "";
        const firstNum =
          incoming.suggestedY && incoming.suggestedY.length > 0
            ? incoming.suggestedY
            : [
                incoming.columns.find(
                  (c) =>
                    c.name !== firstDim &&
                    NUMERIC_TYPES.has(String(c.data_type).toUpperCase())
                )?.name ??
                  incoming.columns[1]?.name ??
                  incoming.columns[0]?.name ??
                  "",
              ].filter(Boolean);
        setXField(firstDim);
        setYFields(firstNum);
      }
    },
    []
  );

  useEffect(() => {
    if (seed) {
      applySeed(seed);
      return;
    }
    try {
      if (typeof sessionStorage !== "undefined") {
        const raw = sessionStorage.getItem(CHART_STUDIO_SEED_STORAGE_KEY);
        if (raw) {
          sessionStorage.removeItem(CHART_STUDIO_SEED_STORAGE_KEY);
          const parsed = JSON.parse(raw) as ChartStudioSeed;
          if (parsed && typeof parsed === "object" && parsed.dataSource) {
            applySeed(parsed);
          }
        }
      }
    } catch {
      // Ignore invalid session seed
    }
  }, [applySeed, seed]);

  useEffect(() => {
    if (!metricKey && metrics[0]) {
      setMetricKey(metrics[0].name);
    }
  }, [metricKey, metrics]);

  useEffect(() => {
    if (!savedQueryId && savedQueries[0]) {
      setSavedQueryId(savedQueries[0].id);
    }
  }, [savedQueries, savedQueryId]);

  useEffect(() => {
    if (!seed && catalog.datasets.length > 0 && !previewData) {
      const hasDemoOrders = catalog.datasets.some(
        (d) => d.name === "demo_orders"
      );
      if (!hasDemoOrders && catalog.datasets[0]) {
        const ds = catalog.datasets[0];
        const col0 = ds.columns[0]?.column_name ?? "*";
        setSqlText(`SELECT ${col0} FROM ${ds.name} LIMIT 20`);
      }
    }
  }, [catalog.datasets, previewData, seed]);

  const activeDef = useMemo(
    () =>
      STUDIO_CHART_TYPES.find((item) => item.id === studioTypeId) ??
      STUDIO_CHART_TYPES[0]!,
    [studioTypeId]
  );

  const compiledDataSource = useMemo<WidgetDataSource>(() => {
    if (sourceKind === "METRIC") {
      return {
        kind: "METRIC",
        metric: metricKey || metrics[0]?.name || "",
        dimensions: xField ? [xField] : [],
      };
    }
    if (sourceKind === "SAVED_QUERY") {
      return {
        kind: "SAVED_QUERY",
        saved_query_id: savedQueryId || savedQueries[0]?.id || "",
      };
    }
    return {
      kind: "SQL",
      sql: sqlText.trim(),
    };
  }, [metricKey, metrics, savedQueries, savedQueryId, sourceKind, sqlText, xField]);

  const compiledEncoding = useMemo<Record<string, unknown>>(() => {
    const enc: Record<string, unknown> = {};
    if (xField) {
      enc.x = xField;
      if (activeDef.widgetKind === "PIE_CHART") {
        enc.category = xField;
      }
      if (activeDef.widgetKind === "SANKEY") {
        enc.source = xField;
      }
      if (activeDef.widgetKind === "RADAR") {
        enc.indicator = xField;
      }
    }
    if (yFields.length > 0) {
      enc.y = yFields;
      enc.value = yFields[0];
      if (activeDef.widgetKind === "SANKEY" && secondaryYField) {
        enc.target = secondaryYField;
      }
    }
    if (secondaryYField && activeDef.id === "dual_axis") {
      enc.secondary_y = secondaryYField;
    }
    if (seriesField) {
      enc.series = seriesField;
    }
    if (sizeField && activeDef.widgetKind === "SCATTER") {
      enc.size = sizeField;
    }
    return enc;
  }, [activeDef, secondaryYField, seriesField, sizeField, xField, yFields]);

  const compiledOptions = useMemo<Record<string, unknown>>(() => {
    const opts: Record<string, unknown> = {
      chart_variant: activeDef.id === "pie" ? pieStyle : activeDef.variant,
    };
    if (unit.trim()) {
      opts.unit = unit.trim();
    }
    const decNum = Number(decimals);
    if (Number.isFinite(decNum) && decNum >= 0 && decNum <= 6) {
      opts.decimals = decNum;
    }
    if (enableMarkLine) {
      opts.show_mark_line = true;
      const mlVal = Number(markLineValue);
      if (markLineValue.trim().length > 0 && Number.isFinite(mlVal)) {
        opts.mark_line_value = mlVal;
      }
    }
    const tgtVal = Number(targetValue);
    if (targetValue.trim().length > 0 && Number.isFinite(tgtVal) && tgtVal > 0) {
      opts.target_value = tgtVal;
    }
    if (activeDef.id === "table") {
      opts.conditional_color = true;
    }
    return opts;
  }, [
    activeDef,
    decimals,
    enableMarkLine,
    markLineValue,
    pieStyle,
    targetValue,
    unit,
  ]);

  const handleRunPreview = useCallback(async () => {
    setPreviewLoading(true);
    setErrorMsg(null);
    try {
      const res = await xblueviewApi.previewWidget({
        widget_kind: activeDef.widgetKind,
        data_source: compiledDataSource,
        encoding: compiledEncoding,
        options: compiledOptions,
      });
      setPreviewData(res);
      if (res.columns.length > 0) {
        const colNames = res.columns.map((c) => c.name);
        setXField((prev) =>
          prev && colNames.includes(prev) ? prev : (colNames[0] ?? "")
        );
        setYFields((prev) => {
          const valid = prev.filter((f) => colNames.includes(f));
          if (valid.length > 0) return valid;
          const numCol = res.columns.find((c, idx) =>
            idx > 0
              ? NUMERIC_TYPES.has(String(c.data_type).toUpperCase())
              : false
          );
          const fallback = numCol?.name ?? colNames[1] ?? colNames[0] ?? "";
          return fallback ? [fallback] : [];
        });
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setPreviewLoading(false);
    }
  }, [activeDef.widgetKind, compiledDataSource, compiledEncoding, compiledOptions]);

  const handleSelectSavedChart = (id: string) => {
    setActiveChartId(id);
    setStatusNotice(null);
    setErrorMsg(null);
    const found = savedCharts.find((c) => c.id === id);
    if (!found) return;

    setChartName(found.name);
    const resolvedId = resolveStudioIdFromSaved(found.widget_kind, found.options);
    setStudioTypeId(resolvedId);
    if (
      resolvedId === "pie" &&
      String(found.options.chart_variant ?? "").toUpperCase() === "ROSE"
    ) {
      setPieStyle("ROSE");
    } else {
      setPieStyle("DONUT");
    }

    if (found.data_source.kind === "SQL") {
      setSourceKind("SQL");
      setSqlText(found.data_source.sql);
    } else if (found.data_source.kind === "METRIC") {
      setSourceKind("METRIC");
      setMetricKey(found.data_source.metric);
    } else if (found.data_source.kind === "SAVED_QUERY") {
      setSourceKind("SAVED_QUERY");
      setSavedQueryId(found.data_source.saved_query_id);
    }

    const enc = found.encoding ?? {};
    setXField(typeof enc.x === "string" ? enc.x : "");
    if (Array.isArray(enc.y)) {
      setYFields(enc.y.filter((v): v is string => typeof v === "string"));
    } else if (typeof enc.value === "string") {
      setYFields([enc.value]);
    }
    setSecondaryYField(
      typeof enc.secondary_y === "string" ? enc.secondary_y : ""
    );
    setSeriesField(typeof enc.series === "string" ? enc.series : "");
    setSizeField(typeof enc.size === "string" ? enc.size : "");

    const opts = found.options ?? {};
    setUnit(typeof opts.unit === "string" ? opts.unit : "");
    setDecimals(
      typeof opts.decimals === "number" ? String(opts.decimals) : "2"
    );
    setEnableMarkLine(
      Boolean(opts.show_mark_line) || typeof opts.mark_line_value === "number"
    );
    setMarkLineValue(
      typeof opts.mark_line_value === "number"
        ? String(opts.mark_line_value)
        : ""
    );
    setTargetValue(
      typeof opts.target_value === "number" ? String(opts.target_value) : ""
    );

    void xblueviewApi
      .previewWidget({
        widget_kind: found.widget_kind,
        data_source: found.data_source,
        encoding: found.encoding,
        options: found.options,
      })
      .then((res) => setPreviewData(res))
      .catch(() => {});
  };

  const handleResetNew = () => {
    setActiveChartId("");
    setChartName("");
    setStatusNotice(null);
    setErrorMsg(null);
  };

  const handleSaveChart = async () => {
    if (!canEditView) return;
    const trimmed =
      chartName.trim() || tx("未命名分析图表", "Untitled Analytics Chart");
    setSaving(true);
    setErrorMsg(null);
    setStatusNotice(null);
    try {
      let saved: SavedChartRecord;
      if (activeChartId) {
        saved = await xblueviewApi.updateSavedChart(activeChartId, {
          name: trimmed,
          widget_kind: activeDef.widgetKind,
          data_source: compiledDataSource,
          encoding: compiledEncoding,
          options: compiledOptions,
        });
      } else {
        saved = await xblueviewApi.createSavedChart({
          name: trimmed,
          widget_kind: activeDef.widgetKind,
          data_source: compiledDataSource,
          encoding: compiledEncoding,
          options: compiledOptions,
        });
      }
      setActiveChartId(saved.id);
      setChartName(saved.name);
      setStatusNotice(tx("已保存", "Saved"));
      await loadSavedCharts();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteSaved = async () => {
    if (!activeChartId || !canEditView) return;
    try {
      await xblueviewApi.deleteSavedChart(activeChartId);
      setActiveChartId("");
      setStatusNotice(tx("已删除", "Deleted"));
      await loadSavedCharts();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleOpenPinModal = () => {
    setPinDraft({
      savedChartId: activeChartId || null,
      name:
        chartName.trim() ||
        `${isZh ? activeDef.zh : activeDef.en} · ${xField || "chart"}`,
      widgetKind: activeDef.widgetKind,
      dataSource: compiledDataSource,
      encoding: compiledEncoding,
      options: compiledOptions,
    });
  };

  const availableColumns = useMemo(
    () => previewData?.columns ?? [],
    [previewData]
  );

  const syntheticWidget = useMemo<WidgetRecord>(
    () => ({
      id: "studio-preview",
      tenant_id: "default",
      dashboard_id: "studio-dashboard",
      widget_kind: activeDef.widgetKind,
      title: chartName.trim() || (isZh ? activeDef.zh : activeDef.en),
      position: { x: 0, y: 0, w: 12, h: 6 },
      data_source: compiledDataSource,
      encoding: compiledEncoding,
      options: compiledOptions,
      drill: [],
      refresh_mode: "MANUAL",
      refresh_seconds: 60,
      deps: {
        dataset_ids: [],
        type_names: [],
        metrics: [],
        alerts: false,
        scenario_ids: [],
      },
      created_at: "",
      updated_at: "",
    }),
    [
      activeDef,
      chartName,
      compiledDataSource,
      compiledEncoding,
      compiledOptions,
      isZh,
    ]
  );

  return (
    <div
      className="flex flex-1 min-h-0 flex-col gap-3"
      data-testid="chart-studio-panel"
    >
      {/* Studio Top Bar: Asset Selector + Name + Save + Pin */}
      <Card className="shrink-0 p-2.5 flex flex-wrap items-center justify-between gap-2 text-xs">
        <div className="flex flex-wrap items-center gap-2 min-w-0 flex-1">
          <Select
            value={activeChartId}
            onChange={(e) => handleSelectSavedChart(e.target.value)}
            className="h-8 w-48 text-xs"
            data-testid="chart-studio-saved-select"
          >
            <option value="">{tx("图表资产库", "Saved Charts")}</option>
            {savedCharts.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </Select>

          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={handleResetNew}
            data-testid="chart-studio-new-btn"
          >
            <Plus className="h-3.5 w-3.5 text-xblue" />
            {tx("新建", "New")}
          </Button>

          {activeChartId && canEditView && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => void handleDeleteSaved()}
              data-testid="chart-studio-delete-btn"
            >
              <Trash2 className="h-3.5 w-3.5 text-red-500" />
              {tx("删除", "Delete")}
            </Button>
          )}

          <Input
            value={chartName}
            onChange={(e) => setChartName(e.target.value)}
            placeholder={tx("各状态订单金额分布图", "Orders by Status")}
            className="h-8 max-w-xs text-xs"
            data-testid="chart-studio-name-input"
          />

          {statusNotice && (
            <Badge variant="success" data-testid="chart-studio-status-badge">
              <CheckCircle2 className="mr-1 h-3 w-3" />
              {statusNotice}
            </Badge>
          )}
        </div>

        <div className="flex items-center gap-2 shrink-0">
          <Button
            type="button"
            variant="outline"
            disabled={previewLoading}
            onClick={() => void handleRunPreview()}
            data-testid="chart-studio-preview-btn"
          >
            <Play className="h-3.5 w-3.5 text-xblue" />
            {previewLoading ? tx("查询中…", "Loading…") : tx("刷新预览", "Preview")}
          </Button>

          <Button
            type="button"
            variant="outline"
            disabled={!canEditView || saving}
            onClick={() => void handleSaveChart()}
            data-testid="chart-studio-save-btn"
          >
            <Save className="h-3.5 w-3.5 text-xblue" />
            {saving ? tx("保存中…", "Saving…") : tx("保存图表", "Save Chart")}
          </Button>

          <Button
            type="button"
            disabled={!canEditView}
            onClick={handleOpenPinModal}
            data-testid="chart-studio-pin-btn"
          >
            <Pin className="h-3.5 w-3.5" />
            {tx("钉入看板", "Pin")}
          </Button>
        </div>
      </Card>

      {errorMsg && (
        <div
          className="shrink-0 rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300"
          data-testid="chart-studio-error"
        >
          {errorMsg}
        </div>
      )}

      {/* Main Studio Body: Left Encoding Shelves + Right Full-Size Live Preview */}
      <div className="grid flex-1 min-h-[420px] grid-cols-1 lg:grid-cols-12 gap-3 items-stretch">
        {/* Left Column: 16-Grid Chart Types + Data Source + Encoding Slots */}
        <Card className="lg:col-span-5 p-3 space-y-3 overflow-y-auto text-xs">
          {/* 16-Grid Chart Type Picker */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-muted-foreground">
              <span className="font-semibold text-foreground">
                {tx("图表类型", "Chart Type")}
              </span>
              <span className="font-mono-tech text-[10px]">16</span>
            </div>
            <div
              className="grid grid-cols-4 gap-1.5"
              data-testid="chart-studio-type-grid"
            >
              {STUDIO_CHART_TYPES.map((item) => {
                const Icon = item.icon;
                const active = studioTypeId === item.id;
                return (
                  <button
                    key={item.id}
                    type="button"
                    onClick={() => setStudioTypeId(item.id)}
                    data-testid={`chart-studio-kind-${item.id}`}
                    className={`flex flex-col items-center justify-center gap-1 rounded-md border px-1.5 py-2 text-[11px] transition-colors cursor-pointer ${
                      active
                        ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                        : "border-border bg-background text-muted-foreground hover:text-foreground hover:bg-muted/40"
                    }`}
                  >
                    <Icon className="h-3.5 w-3.5 shrink-0" />
                    <span className="truncate max-w-full">
                      {isZh ? item.zh : item.en}
                    </span>
                  </button>
                );
              })}
            </div>
          </div>

          {studioTypeId === "pie" && (
            <div className="flex items-center justify-between gap-2 pt-1">
              <span className="text-muted-foreground">
                {tx("扇区样式", "Sector Style")}
              </span>
              <SegmentedControl<"DONUT" | "ROSE">
                value={pieStyle}
                onChange={setPieStyle}
                items={[
                  { key: "DONUT", label: tx("环形图", "Donut") },
                  { key: "ROSE", label: tx("玫瑰图", "Rose") },
                ]}
              />
            </div>
          )}

          {/* Data Source Configuration */}
          <div className="space-y-2 border-t border-border pt-2.5">
            <div className="flex items-center justify-between gap-2">
              <span className="font-semibold text-foreground">
                {tx("数据来源", "Data Source")}
              </span>
              <SegmentedControl<"SQL" | "METRIC" | "SAVED_QUERY">
                value={sourceKind}
                onChange={setSourceKind}
                items={[
                  { key: "SQL", label: "SQL" },
                  { key: "METRIC", label: tx("指标", "Metric") },
                  { key: "SAVED_QUERY", label: tx("查询", "Saved") },
                ]}
              />
            </div>

            {sourceKind === "SQL" && (
              <Textarea
                rows={3}
                value={sqlText}
                onChange={(e) => setSqlText(e.target.value)}
                placeholder="SELECT status, SUM(amount) AS total_amt FROM demo_orders GROUP BY status"
                className="font-mono-tech text-xs"
                data-testid="chart-studio-sql-input"
              />
            )}

            {sourceKind === "METRIC" && (
              <Select
                value={metricKey}
                onChange={(e) => setMetricKey(e.target.value)}
                className="h-8 w-full font-mono-tech text-xs"
                data-testid="chart-studio-metric-select"
              >
                {metrics.length === 0 && (
                  <option value="">{tx("暂无指标", "No metrics")}</option>
                )}
                {metrics.map((m) => (
                  <option key={m.id} value={m.name}>
                    {m.display_name} ({m.name})
                  </option>
                ))}
              </Select>
            )}

            {sourceKind === "SAVED_QUERY" && (
              <Select
                value={savedQueryId}
                onChange={(e) => setSavedQueryId(e.target.value)}
                className="h-8 w-full text-xs"
                data-testid="chart-studio-saved-query-select"
              >
                {savedQueries.length === 0 && (
                  <option value="">{tx("暂无保存查询", "No saved queries")}</option>
                )}
                {savedQueries.map((sq) => (
                  <option key={sq.id} value={sq.id}>
                    {sq.name}
                  </option>
                ))}
              </Select>
            )}
          </div>

          {/* Visual Channel Encoding Slots */}
          <div className="space-y-2.5 border-t border-border pt-2.5">
            <div className="font-semibold text-foreground">
              {tx("视觉通道编码", "Visual Encoding")}
            </div>

            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {tx("横轴维度", "X / Dimension")}
                </label>
                <Select
                  value={xField}
                  onChange={(e) => setXField(e.target.value)}
                  className="h-8 w-full font-mono-tech text-xs"
                  data-testid="chart-studio-x-select"
                >
                  <option value="">{tx("自动推断", "Auto")}</option>
                  {availableColumns.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name}
                    </option>
                  ))}
                </Select>
              </div>

              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {tx("纵轴度量", "Y / Measure")}
                </label>
                <Select
                  value={yFields[0] ?? ""}
                  onChange={(e) =>
                    setYFields(e.target.value ? [e.target.value] : [])
                  }
                  className="h-8 w-full font-mono-tech text-xs"
                  data-testid="chart-studio-y-select"
                >
                  <option value="">{tx("自动推断", "Auto")}</option>
                  {availableColumns.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name}
                    </option>
                  ))}
                </Select>
              </div>
            </div>

            {availableColumns.length > 2 &&
              (studioTypeId === "bar" ||
                studioTypeId === "stacked_bar" ||
                studioTypeId === "line" ||
                studioTypeId === "area") && (
                <div className="space-y-1">
                  <div className="text-muted-foreground">
                    {tx("多序列度量", "Multi-Series Measures")}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {availableColumns
                      .filter((c) => c.name !== xField)
                      .map((col) => {
                        const checked = yFields.includes(col.name);
                        return (
                          <label
                            key={col.name}
                            className={`inline-flex items-center gap-1 rounded border px-2 py-0.5 font-mono-tech text-[11px] cursor-pointer ${
                              checked
                                ? "border-xblue bg-xblue-soft text-xblue"
                                : "border-border bg-background"
                            }`}
                          >
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={(e) => {
                                if (e.target.checked) {
                                  setYFields((prev) => [...prev, col.name]);
                                } else {
                                  setYFields((prev) =>
                                    prev.filter((item) => item !== col.name)
                                  );
                                }
                              }}
                              className="accent-xblue"
                            />
                            {col.name}
                          </label>
                        );
                      })}
                  </div>
                </div>
              )}

            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {studioTypeId === "sankey"
                    ? tx("目标节点列", "Target Field")
                    : tx("右轴折线", "Secondary Y")}
                </label>
                <Select
                  value={secondaryYField}
                  onChange={(e) => setSecondaryYField(e.target.value)}
                  className="h-8 w-full font-mono-tech text-xs"
                  data-testid="chart-studio-secondary-y-select"
                >
                  <option value="">{tx("无", "None")}</option>
                  {availableColumns.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name}
                    </option>
                  ))}
                </Select>
              </div>

              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {studioTypeId === "scatter"
                    ? tx("气泡大小列", "Bubble Size")
                    : tx("颜色分组", "Color Group")}
                </label>
                <Select
                  value={studioTypeId === "scatter" ? sizeField : seriesField}
                  onChange={(e) =>
                    studioTypeId === "scatter"
                      ? setSizeField(e.target.value)
                      : setSeriesField(e.target.value)
                  }
                  className="h-8 w-full font-mono-tech text-xs"
                  data-testid="chart-studio-series-select"
                >
                  <option value="">{tx("无", "None")}</option>
                  {availableColumns.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name}
                    </option>
                  ))}
                </Select>
              </div>
            </div>

            {/* Formatting, Threshold MarkLine & KPI Target */}
            <div className="grid grid-cols-3 gap-2 pt-1">
              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {tx("单位", "Unit")}
                </label>
                <Input
                  value={unit}
                  onChange={(e) => setUnit(e.target.value)}
                  placeholder={tx("元", "CNY")}
                  className="h-8 text-xs"
                  data-testid="chart-studio-unit-input"
                />
              </div>

              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {tx("小数位", "Decimals")}
                </label>
                <Select
                  value={decimals}
                  onChange={(e) => setDecimals(e.target.value)}
                  className="h-8 w-full font-mono-tech text-xs"
                  data-testid="chart-studio-decimals-select"
                >
                  <option value="0">0</option>
                  <option value="1">1</option>
                  <option value="2">2</option>
                  <option value="4">4</option>
                </Select>
              </div>

              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {tx("目标值", "Target")}
                </label>
                <Input
                  value={targetValue}
                  onChange={(e) => setTargetValue(e.target.value)}
                  placeholder="10000"
                  className="h-8 font-mono-tech text-xs"
                  data-testid="chart-studio-target-input"
                />
              </div>
            </div>

            <div className="flex items-center justify-between gap-2 pt-1">
              <label className="inline-flex items-center gap-1.5 cursor-pointer">
                <input
                  type="checkbox"
                  checked={enableMarkLine}
                  onChange={(e) => setEnableMarkLine(e.target.checked)}
                  className="accent-xblue"
                  data-testid="chart-studio-markline-toggle"
                />
                <span>{tx("阈值警戒线", "Threshold Line")}</span>
              </label>
              {enableMarkLine && (
                <Input
                  value={markLineValue}
                  onChange={(e) => setMarkLineValue(e.target.value)}
                  placeholder="5000"
                  className="h-7 w-32 font-mono-tech text-xs"
                  data-testid="chart-studio-markline-input"
                />
              )}
            </div>
          </div>
        </Card>

        {/* Right Column: Live Full-Size Preview & Data Contract Telemetry */}
        <Card className="lg:col-span-7 flex flex-col overflow-hidden">
          <div className="flex items-center justify-between gap-2 border-b border-border bg-muted/25 px-3 py-2 text-xs shrink-0">
            <div className="flex items-center gap-2 min-w-0">
              <Activity className="h-3.5 w-3.5 text-xblue shrink-0" />
              <span className="font-semibold truncate">
                {syntheticWidget.title}
              </span>
              <Badge variant="outline">
                {isZh ? activeDef.zh : activeDef.en}
              </Badge>
            </div>
            <div
              className="flex items-center gap-2 text-[11px] font-mono-tech tabular-nums text-muted-foreground"
              data-testid="chart-studio-contract-check"
            >
              {previewData ? (
                <>
                  <span>
                    {fmtNum(previewData.row_count, 0)} {tx("行", "rows")} ·{" "}
                    {fmtNum(previewData.columns.length, 0)} {tx("列", "cols")}
                  </span>
                  <Badge variant="xblue">{tx("就绪", "Ready")}</Badge>
                </>
              ) : (
                <span>{tx("待预览", "Idle")}</span>
              )}
            </div>
          </div>

          <div
            className="flex-1 min-h-[340px] p-4 flex flex-col"
            data-testid="chart-studio-preview-stage"
          >
            {!previewData ? (
              <div className="flex flex-1 flex-col items-center justify-center gap-2.5 text-xs text-muted-foreground">
                <BarChart3 className="h-6 w-6 text-muted-foreground/45" />
                <span>{tx("暂无预览数据", "No preview data")}</span>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => void handleRunPreview()}
                  data-testid="chart-studio-empty-run-btn"
                >
                  <Play className="h-3.5 w-3.5 text-xblue" />
                  {tx("刷新预览", "Preview")}
                </Button>
              </div>
            ) : activeDef.widgetKind === "KPI_CARD" ? (
              <div className="flex-1 flex items-center justify-center max-w-md w-full mx-auto">
                <div className="w-full rounded-lg border border-border p-5 bg-muted/10">
                  <KpiWidget widget={syntheticWidget} data={previewData} />
                </div>
              </div>
            ) : activeDef.widgetKind === "TABLE" ? (
              <div className="flex-1 min-h-[300px]">
                <TableWidget widget={syntheticWidget} data={previewData} />
              </div>
            ) : (
              <div className="flex-1 min-h-[300px]">
                <EChartsWidget
                  widget={syntheticWidget}
                  data={previewData}
                  isDark={isDark}
                />
              </div>
            )}
          </div>
        </Card>
      </div>

      <PinToDashboardModal
        open={Boolean(pinDraft)}
        draft={pinDraft}
        onClose={() => setPinDraft(null)}
        onPinned={(_resp, savedId) => {
          setActiveChartId(savedId);
          void loadSavedCharts();
        }}
      />
    </div>
  );
}
