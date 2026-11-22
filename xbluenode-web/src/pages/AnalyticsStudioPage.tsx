import { useCallback, useEffect, useRef, useState } from "react";
import { Award, Code2, TableProperties, Wand2 } from "lucide-react";
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
import { MetricManager } from "@/components/analytics/MetricManager";
import { PivotBuilder } from "@/components/analytics/PivotBuilder";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import { SchemaTree } from "@/components/analytics/SchemaTree";
import { SqlEditor } from "@/components/analytics/SqlEditor";
import { TransformWizard } from "@/components/analytics/TransformWizard";
import {
  Card,
  PageHeader,
  SegmentedControl,
  StatText,
} from "@/components/ui/primitives";

type StudioTab = "sql" | "pivot" | "metrics" | "transform";

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
  const userEditedSqlRef = useRef(false);

  const [activeTab, setActiveTab] = useState<StudioTab>("sql");
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
      setActiveTab("sql");
    } else if (sq.kind === "PIVOT" && typeof sq.spec?.dataset === "string") {
      setSelectedPivotDataset(sq.spec.dataset);
      setActiveTab("pivot");
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
    setActiveTab("sql");
    setSqlText((prev) => {
      const trimmed = prev.trimEnd();
      if (!trimmed) return identifier;
      return `${trimmed} ${identifier}`;
    });
  };

  const handleSelectForPivot = (datasetOrViewName: string) => {
    setSelectedPivotDataset(datasetOrViewName);
    setActiveTab("pivot");
  };

  return (
    <div
      className="flex flex-1 min-h-0 flex-col gap-3"
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
            <SegmentedControl
              value={activeTab}
              onChange={(next) => setActiveTab(next)}
              items={[
                {
                  key: "sql",
                  label: "SQL",
                  icon: <Code2 className="h-3.5 w-3.5" />,
                  testId: "analytics-tab-sql",
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

      {/* 2-Pane Full-Height Studio Layout: Left SchemaTree + Right Workspace */}
      <div className="flex flex-1 min-h-0 flex-col lg:flex-row gap-3 items-stretch overflow-hidden">
        <div className="w-full lg:w-68 shrink-0 h-60 lg:h-full min-h-0 flex flex-col">
          <SchemaTree
            catalog={catalog}
            loading={catalogLoading}
            selectedPivotDataset={selectedPivotDataset}
            onRefresh={() => void loadCatalog()}
            onInsertIdentifier={handleInsertIdentifier}
            onSelectForPivot={handleSelectForPivot}
          />
        </div>

        <div className="min-w-0 flex-1 w-full h-full min-h-0 flex flex-col overflow-y-auto">
          {/* Tab 1: SQL Workspace + Equal-Height Stretched ResultGrid & ChartPanel */}
          {activeTab === "sql" && (
            <div className="flex flex-1 min-h-0 flex-col gap-3">
              <div className="shrink-0">
                <SqlEditor
                  sql={sqlText}
                  onChangeSql={handleChangeSql}
                  catalog={catalog}
                  running={sqlRunning}
                  explaining={sqlExplaining}
                  explainResult={explainResult}
                  onCloseExplain={() => setExplainResult(null)}
                  onRunSql={(opts) => void handleRunSql(opts)}
                  onExplainSql={(opts) => void handleExplainSql(opts)}
                  onSaveQuery={handleSaveSqlQuery}
                  savedQueries={savedQueries}
                  historyEntries={historyEntries}
                  onLoadSavedQuery={handleLoadSavedQuery}
                  onDeleteSavedQuery={(id) => void handleDeleteSavedQuery(id)}
                />
              </div>

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

              <div className="grid flex-1 min-h-[240px] grid-cols-1 xl:grid-cols-12 gap-3 items-stretch">
                <div className="xl:col-span-7 min-w-0 flex flex-col min-h-0">
                  <ResultGrid
                    columns={resultColumns}
                    rows={resultRows}
                    rowCount={resultRowCount}
                    truncated={resultTruncated}
                    durationMs={resultDurationMs}
                    sql={resultExecutedSql}
                  />
                </div>
                <div className="xl:col-span-5 min-w-0 flex flex-col min-h-0">
                  <ChartPanel columns={resultColumns} rows={resultRows} />
                </div>
              </div>
            </div>
          )}

          {/* Tab 2: Pivot Workspace (isolated result matrix inside PivotBuilder) */}
          {activeTab === "pivot" && (
            <PivotBuilder
              catalog={catalog}
              selectedDataset={selectedPivotDataset}
              onSelectDataset={setSelectedPivotDataset}
              onSavePivotQuery={handleSavePivotQuery}
            />
          )}

          {/* Tab 3: Semantic Metrics Workspace (isolated result panel inside MetricManager) */}
          {activeTab === "metrics" && (
            <MetricManager
              catalog={catalog}
              metrics={metrics}
              onRefreshMetrics={loadMetrics}
            />
          )}

          {/* Tab 4: AI Transform Wizard (isolated preview grid inside TransformWizard) */}
          {activeTab === "transform" && (
            <TransformWizard
              catalog={catalog}
              onMaterialized={loadCatalog}
            />
          )}
        </div>
      </div>
    </div>
  );
}
