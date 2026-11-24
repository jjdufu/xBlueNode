import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import {
  Award,
  BarChart3,
  Code2,
  Sparkles,
  TableProperties,
  Wand2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
  type CatalogDatasetNode,
  type ExplainResult,
  type MetricDef,
  type PivotSpec,
  type QueryColumn,
  type QueryHistoryEntry,
  type QueryParamSpec,
  type SavedQuery,
} from "@/lib/api/analytics";
import { ChartPanel } from "@/components/analytics/ChartPanel";
import {
  ChartStudioPanel,
  type ChartStudioSeed,
} from "@/components/analytics/ChartStudioPanel";
import { MetricManager } from "@/components/analytics/MetricManager";
import {
  PinToDashboardModal,
  type PinChartDraftSpec,
} from "@/components/analytics/PinToDashboardModal";
import { PivotBuilder } from "@/components/analytics/PivotBuilder";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import { SchemaTree } from "@/components/analytics/SchemaTree";
import { SqlEditor } from "@/components/analytics/SqlEditor";
import { TransformWizard } from "@/components/analytics/TransformWizard";
import { ResizableWorkspace } from "@/components/ui/ResizableWorkspace";
import {
  Card,
  PageHeader,
  SegmentedControl,
  StatText,
} from "@/components/ui/primitives";
import { AxQueryPage } from "@/pages/AxQueryPage";

type StudioTab = "ask" | "sql" | "chart" | "pivot" | "metrics" | "transform";

function resolveAnalyticsTab(rawTab: string | null): StudioTab {
  const normalized = (rawTab ?? "").trim().toLowerCase();
  if (normalized === "ask" || normalized === "query" || normalized === "qa") {
    return "ask";
  }
  if (
    normalized === "chart" ||
    normalized === "studio" ||
    normalized === "visualize"
  ) {
    return "chart";
  }
  if (normalized === "pivot") {
    return "pivot";
  }
  if (normalized === "metrics" || normalized === "semantic") {
    return "metrics";
  }
  if (normalized === "transform" || normalized === "federation") {
    return "transform";
  }
  return "sql";
}

const EMPTY_CATALOG: AnalyticsCatalogResponse = {
  datasets: [],
  platform_views: [],
  live_connections: [],
};

function pickInitialDatasetSql(datasets: CatalogDatasetNode[]): string {
  if (datasets.length === 0) {
    return "SELECT * FROM demo_orders LIMIT 20";
  }
  const demoOrders = datasets.find((d) => d.name === "demo_orders");
  if (demoOrders) {
    return "SELECT * FROM demo_orders LIMIT 20";
  }
  const smallNonBench = datasets.find(
    (d) => !/bench|10m|million/i.test(d.name) && d.row_count <= 100_000
  );
  const chosen = smallNonBench ?? datasets[0];
  return chosen
    ? `SELECT * FROM ${chosen.name} LIMIT 20`
    : "SELECT * FROM demo_orders LIMIT 20";
}

export function AnalyticsStudioPage() {
  const { tx, fmtNum } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab");
  const userEditedSqlRef = useRef(false);

  const [activeTab, setActiveTab] = useState<StudioTab>(() =>
    resolveAnalyticsTab(tabParam)
  );
  const [catalog, setCatalog] =
    useState<AnalyticsCatalogResponse>(EMPTY_CATALOG);
  const [catalogLoading, setCatalogLoading] = useState(false);

  const [metrics, setMetrics] = useState<MetricDef[]>([]);
  const [savedQueries, setSavedQueries] = useState<SavedQuery[]>([]);
  const [historyEntries, setHistoryEntries] = useState<QueryHistoryEntry[]>([]);

  // SQL workspace state
  const [sqlText, setSqlText] = useState("SELECT * FROM demo_orders LIMIT 20");
  const [sqlRunning, setSqlRunning] = useState(false);
  const [sqlExplaining, setSqlExplaining] = useState(false);
  const [explainResult, setExplainResult] = useState<ExplainResult | null>(
    null
  );
  const [sqlError, setSqlError] = useState<string | null>(null);

  // Pivot workspace state
  const [selectedPivotDataset, setSelectedPivotDataset] = useState<string>("");

  // SQL-tab isolated result grid & chart state
  const [resultColumns, setResultColumns] = useState<QueryColumn[]>([]);
  const [resultRows, setResultRows] = useState<Record<string, unknown>[]>([]);
  const [resultRowCount, setResultRowCount] = useState<number>(0);
  const [resultTruncated, setResultTruncated] = useState<boolean>(false);
  const [resultDurationMs, setResultDurationMs] = useState<number | null>(null);
  const [resultExecutedSql, setResultExecutedSql] = useState<string>("");
  const [chartStudioSeed, setChartStudioSeed] =
    useState<ChartStudioSeed | null>(null);
  const [pinDraft, setPinDraft] = useState<PinChartDraftSpec | null>(null);

  useEffect(() => {
    setActiveTab(resolveAnalyticsTab(tabParam));
  }, [tabParam]);

  const handleSwitchTab = (next: StudioTab) => {
    setActiveTab(next);
    const updated = new URLSearchParams(searchParams);
    if (next === "sql") {
      updated.delete("tab");
    } else {
      updated.set("tab", next);
    }
    setSearchParams(updated, { replace: true });
  };

  const loadCatalog = useCallback(async () => {
    setCatalogLoading(true);
    try {
      const res = await analyticsApi.getCatalog();
      const normalized: AnalyticsCatalogResponse = {
        datasets: Array.isArray(res?.datasets) ? res.datasets : [],
        platform_views: Array.isArray(res?.platform_views)
          ? res.platform_views
          : [],
        live_connections: Array.isArray(res?.live_connections)
          ? res.live_connections
          : [],
      };
      setCatalog(normalized);
      if (normalized.datasets.length > 0 && !userEditedSqlRef.current) {
        setSqlText(pickInitialDatasetSql(normalized.datasets));
      }
      if (normalized.datasets[0]) {
        setSelectedPivotDataset((prev) => prev || normalized.datasets[0]!.name);
      }
    } catch {
      setCatalog(EMPTY_CATALOG);
    } finally {
      setCatalogLoading(false);
    }
  }, []);

  const loadMetrics = useCallback(async () => {
    try {
      const res = await analyticsApi.listMetrics();
      setMetrics(Array.isArray(res) ? res : []);
    } catch {
      setMetrics([]);
    }
  }, []);

  const loadSavedAndHistory = useCallback(async () => {
    const [savedRes, histRes] = await Promise.all([
      analyticsApi.listSavedQueries().catch(() => []),
      analyticsApi
        .listHistory({ limit: 30 })
        .catch(() => ({ items: [], next_cursor: null })),
    ]);
    setSavedQueries(Array.isArray(savedRes) ? savedRes : []);
    setHistoryEntries(Array.isArray(histRes?.items) ? histRes.items : []);
  }, []);

  useEffect(() => {
    void loadCatalog();
    void loadMetrics();
    void loadSavedAndHistory();
  }, [loadCatalog, loadMetrics, loadSavedAndHistory]);

  const handleChangeSql = (nextSql: string) => {
    userEditedSqlRef.current = true;
    setSqlText(nextSql);
  };

  const handleRunSql = async (opts: {
    sql: string;
    params: Record<string, unknown>;
    maxRows: number;
    allowLive: boolean;
    revealPii: boolean;
  }) => {
    setSqlRunning(true);
    setSqlError(null);
    try {
      const hasParams = Object.keys(opts.params).length > 0;
      const res = await analyticsApi.runSql({
        sql: opts.sql,
        ...(hasParams ? { params: opts.params } : {}),
        max_rows: opts.maxRows,
        allow_live: opts.allowLive,
        ...(opts.revealPii ? { reveal_pii: true } : {}),
      });
      setResultColumns(Array.isArray(res.columns) ? res.columns : []);
      setResultRows(Array.isArray(res.rows) ? res.rows : []);
      setResultRowCount(
        typeof res.row_count === "number" ? res.row_count : res.rows.length
      );
      setResultTruncated(Boolean(res.truncated));
      setResultDurationMs(res.duration_ms);
      setResultExecutedSql(res.sql);
      void loadSavedAndHistory();
    } catch (err) {
      setSqlError(err instanceof Error ? err.message : String(err));
    } finally {
      setSqlRunning(false);
    }
  };

  const handleExplainSql = async (opts: {
    sql: string;
    params: Record<string, unknown>;
    maxRows: number;
    allowLive: boolean;
    revealPii: boolean;
  }) => {
    setSqlExplaining(true);
    setSqlError(null);
    try {
      const hasParams = Object.keys(opts.params).length > 0;
      const res = await analyticsApi.explainSql({
        sql: opts.sql,
        ...(hasParams ? { params: opts.params } : {}),
        max_rows: opts.maxRows,
        allow_live: opts.allowLive,
        ...(opts.revealPii ? { reveal_pii: true } : {}),
      });
      setExplainResult(res);
    } catch (err) {
      setSqlError(err instanceof Error ? err.message : String(err));
    } finally {
      setSqlExplaining(false);
    }
  };

  const handleSaveSqlQuery = async (opts: {
    name: string;
    sql: string;
    parameters: QueryParamSpec[];
    shared: boolean;
  }) => {
    await analyticsApi.createSavedQuery({
      name: opts.name,
      kind: "SQL",
      spec: { sql: opts.sql },
      parameters: opts.parameters,
      shared: opts.shared,
    });
    await loadSavedAndHistory();
  };

  const handleSavePivotQuery = async (name: string, spec: PivotSpec) => {
    const specRecord: Record<string, unknown> = { ...spec };
    await analyticsApi.createSavedQuery({
      name,
      kind: "PIVOT",
      spec: specRecord,
      parameters: [],
      shared: true,
    });
    await loadSavedAndHistory();
  };

  const handleLoadSavedQuery = (sq: SavedQuery) => {
    if (typeof sq.spec?.sql === "string") {
      userEditedSqlRef.current = true;
      setSqlText(sq.spec.sql);
      handleSwitchTab("sql");
    } else if (sq.kind === "PIVOT" && typeof sq.spec?.dataset === "string") {
      setSelectedPivotDataset(sq.spec.dataset);
      handleSwitchTab("pivot");
    }
  };

  const handleDeleteSavedQuery = async (id: string) => {
    try {
      await analyticsApi.deleteSavedQuery(id);
      await loadSavedAndHistory();
    } catch {
      // Ignore delete error gracefully
    }
  };

  const handleInsertIdentifier = (identifier: string) => {
    userEditedSqlRef.current = true;
    handleSwitchTab("sql");
    setSqlText((prev) => {
      const trimmed = prev.trimEnd();
      if (!trimmed) return identifier;
      return `${trimmed} ${identifier}`;
    });
  };

  const handleSelectForPivot = (datasetOrViewName: string) => {
    setSelectedPivotDataset(datasetOrViewName);
    handleSwitchTab("pivot");
  };

  const handleOpenChartStudio = (seed: ChartStudioSeed) => {
    setChartStudioSeed(seed);
    handleSwitchTab("chart");
  };

  return (
    <div
      className="flex flex-1 min-h-0 flex-col gap-2.5 overflow-hidden"
      data-testid="analytics-studio-page"
    >
      <PageHeader
        className="shrink-0 pb-2.5"
        title={tx(
          MODULE_META.axanalytics.label,
          MODULE_META.axanalytics.enLabel
        )}
        badges={
          <div className="flex items-center gap-3 pl-1">
            <StatText
              label={tx("数据集", "Datasets")}
              value={fmtNum(catalog.datasets.length, 0)}
            />
            <span className="text-border">·</span>
            <StatText
              label={tx("指标", "Metrics")}
              value={fmtNum(metrics.length, 0)}
            />
          </div>
        }
        actions={
          <div data-testid="analytics-tabs">
            <SegmentedControl<StudioTab>
              value={activeTab}
              onChange={handleSwitchTab}
              items={[
                {
                  key: "ask",
                  label: tx("智能问答", "Ask"),
                  icon: <Sparkles className="h-3.5 w-3.5 text-xblue" />,
                  testId: "analytics-tab-ask",
                },
                {
                  key: "sql",
                  label: "SQL",
                  icon: <Code2 className="h-3.5 w-3.5" />,
                  testId: "analytics-tab-sql",
                },
                {
                  key: "chart",
                  label: tx("可视化制图", "Chart Studio"),
                  icon: <BarChart3 className="h-3.5 w-3.5 text-xblue" />,
                  testId: "analytics-tab-chart",
                },
                {
                  key: "pivot",
                  label: tx("透视表", "Pivot"),
                  icon: <TableProperties className="h-3.5 w-3.5" />,
                  testId: "analytics-tab-pivot",
                },
                {
                  key: "metrics",
                  label: tx("语义指标", "Metrics"),
                  icon: <Award className="h-3.5 w-3.5" />,
                  testId: "analytics-tab-metrics",
                },
                {
                  key: "transform",
                  label: tx("数据变换", "Transform"),
                  icon: <Wand2 className="h-3.5 w-3.5" />,
                  testId: "analytics-tab-transform",
                },
              ]}
            />
          </div>
        }
      />

      {activeTab === "ask" ? (
        <AxQueryPage
          embedded
          onOpenChartStudio={handleOpenChartStudio}
          onPinToDashboard={setPinDraft}
        />
      ) : (
        <div className="flex flex-1 min-h-0 overflow-hidden">
          <ResizableWorkspace
            storageKey="axpoint.layout.analytics"
            panes={[
              {
                id: "catalog",
                defaultSize: 20,
                minSize: 12,
                maxSize: 38,
                collapsible: true,
                handleTestId: "resizable-handle-left",
                className: "pr-1",
                children: (
                  <SchemaTree
                    catalog={catalog}
                    loading={catalogLoading}
                    selectedPivotDataset={selectedPivotDataset}
                    onRefresh={() => void loadCatalog()}
                    onInsertIdentifier={handleInsertIdentifier}
                    onSelectForPivot={handleSelectForPivot}
                  />
                ),
              },
              {
                id: "main",
                defaultSize: 80,
                minSize: 55,
                className: "pl-1 overflow-y-auto",
                children: (
                  <div className="flex flex-1 min-h-0 flex-col h-full">
                    {activeTab === "sql" && (
                      <ResizableWorkspace
                        storageKey="axpoint.layout.analytics-sql"
                        direction="vertical"
                        panes={[
                          {
                            id: "editor",
                            defaultSize: 44,
                            minSize: 24,
                            collapsible: true,
                            className: "pb-1 overflow-y-auto",
                            children: (
                              <div className="space-y-2">
                                <SqlEditor
                                  sql={sqlText}
                                  onChangeSql={handleChangeSql}
                                  catalog={catalog}
                                  running={sqlRunning}
                                  explaining={sqlExplaining}
                                  explainResult={explainResult}
                                  onCloseExplain={() => setExplainResult(null)}
                                  onRunSql={(opts) => void handleRunSql(opts)}
                                  onExplainSql={(opts) =>
                                    void handleExplainSql(opts)
                                  }
                                  onSaveQuery={handleSaveSqlQuery}
                                  savedQueries={savedQueries}
                                  historyEntries={historyEntries}
                                  onLoadSavedQuery={handleLoadSavedQuery}
                                  onDeleteSavedQuery={(id) =>
                                    void handleDeleteSavedQuery(id)
                                  }
                                />
                                {sqlError && (
                                  <Card
                                    className="shrink-0 border-red-500/30 bg-red-500/5 p-3 text-xs text-red-600 dark:text-red-400"
                                    data-testid="sql-error-card"
                                  >
                                    <span className="font-mono-tech font-semibold">
                                      {tx("执行异常：", "Query error: ")}
                                    </span>
                                    {sqlError}
                                  </Card>
                                )}
                              </div>
                            ),
                          },
                          {
                            id: "results",
                            defaultSize: 56,
                            minSize: 28,
                            className: "pt-1",
                            children: (
                              <div className="grid flex-1 min-h-[220px] h-full grid-cols-1 xl:grid-cols-12 gap-3 items-stretch">
                                <div className="xl:col-span-7 min-w-0 flex flex-col min-h-0">
                                  <ResultGrid
                                    columns={resultColumns}
                                    rows={resultRows}
                                    rowCount={resultRowCount}
                                    truncated={resultTruncated}
                                    durationMs={resultDurationMs}
                                    sql={resultExecutedSql}
                                    onOpenChartStudio={() =>
                                      handleOpenChartStudio({
                                        title: tx(
                                          "业务查询图表",
                                          "SQL Result Chart"
                                        ),
                                        dataSource: {
                                          kind: "SQL",
                                          sql: (
                                            resultExecutedSql || sqlText
                                          ).trim(),
                                        },
                                        columns: resultColumns,
                                        rows: resultRows,
                                        suggestedKind:
                                          resultRows.length <= 1 &&
                                          resultColumns.length <= 2
                                            ? "KPI_CARD"
                                            : "BAR_CHART",
                                      })
                                    }
                                    onPinToDashboard={() =>
                                      setPinDraft({
                                        name: tx(
                                          "业务查询图表",
                                          "SQL Result Chart"
                                        ),
                                        widgetKind:
                                          resultRows.length <= 1 &&
                                          resultColumns.length <= 2
                                            ? "KPI_CARD"
                                            : "BAR_CHART",
                                        dataSource: {
                                          kind: "SQL",
                                          sql: (
                                            resultExecutedSql || sqlText
                                          ).trim(),
                                        },
                                        encoding: {},
                                        options: {},
                                      })
                                    }
                                  />
                                </div>
                                <div className="xl:col-span-5 min-w-0 flex flex-col min-h-0">
                                  <ChartPanel
                                    columns={resultColumns}
                                    rows={resultRows}
                                  />
                                </div>
                              </div>
                            ),
                          },
                        ]}
                      />
                    )}

                    {activeTab === "chart" && (
                      <ChartStudioPanel
                        catalog={catalog}
                        metrics={metrics}
                        savedQueries={savedQueries}
                        seed={chartStudioSeed}
                      />
                    )}

                    {activeTab === "pivot" && (
                      <PivotBuilder
                        catalog={catalog}
                        selectedDataset={selectedPivotDataset}
                        onSelectDataset={setSelectedPivotDataset}
                        onSavePivotQuery={handleSavePivotQuery}
                      />
                    )}

                    {activeTab === "metrics" && (
                      <MetricManager
                        catalog={catalog}
                        metrics={metrics}
                        onRefreshMetrics={loadMetrics}
                        onOpenChartStudio={handleOpenChartStudio}
                        onPinToDashboard={setPinDraft}
                      />
                    )}

                    {activeTab === "transform" && (
                      <TransformWizard
                        catalog={catalog}
                        onMaterialized={loadCatalog}
                      />
                    )}
                  </div>
                ),
              },
            ]}
          />
        </div>
      )}

      <PinToDashboardModal
        open={Boolean(pinDraft)}
        draft={pinDraft}
        onClose={() => setPinDraft(null)}
      />
    </div>
  );
}
