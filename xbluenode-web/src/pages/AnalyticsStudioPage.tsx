import { useCallback, useEffect, useState } from "react";
import {
  Award,
  Code2,
  History,
  Save,
  TableProperties,
  Trash2,
  Wand2,
  X,
} from "lucide-react";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
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
import {
  extractNamedSqlParams,
  SqlEditor,
} from "@/components/analytics/SqlEditor";
import { TransformWizard } from "@/components/analytics/TransformWizard";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

type StudioTab = "sql" | "pivot" | "metrics" | "transform";

const EMPTY_CATALOG: AnalyticsCatalogResponse = {
  datasets: [],
  platform_views: [],
  live_connections: [],
};

export function AnalyticsStudioPage() {
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

  // Shared bottom result grid & chart state
  const [resultColumns, setResultColumns] = useState<QueryColumn[]>([]);
  const [resultRows, setResultRows] = useState<Record<string, unknown>[]>([]);
  const [resultRowCount, setResultRowCount] = useState<number>(0);
  const [resultTruncated, setResultTruncated] = useState<boolean>(false);
  const [resultDurationMs, setResultDurationMs] = useState<number | null>(null);
  const [resultExecutedSql, setResultExecutedSql] = useState<string>("");

  // Top-level Save & History Drawer state
  const [topSaveOpen, setTopSaveOpen] = useState(false);
  const [topSaveName, setTopSaveName] = useState("");
  const [topSaveShared, setTopSaveShared] = useState(true);
  const [topSaving, setTopSaving] = useState(false);
  const [historyDrawerOpen, setHistoryDrawerOpen] = useState(false);

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

  const handleRunSql = async (opts: {
    sql: string;
    params: Record<string, unknown>;
    maxRows: number;
    allowLive: boolean;
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

  const handleTopSaveConfirm = async () => {
    if (!topSaveName.trim() || !sqlText.trim()) return;
    setTopSaving(true);
    try {
      const pNames = extractNamedSqlParams(sqlText);
      const paramSpecs: QueryParamSpec[] = pNames.map((n) => ({
        name: n,
        data_type: "STRING",
      }));
      await handleSaveSqlQuery({
        name: topSaveName.trim(),
        sql: sqlText,
        parameters: paramSpecs,
        shared: topSaveShared,
      });
      setTopSaveName("");
      setTopSaveOpen(false);
    } catch (err) {
      setSqlError(err instanceof Error ? err.message : String(err));
    } finally {
      setTopSaving(false);
    }
  };

  const handleLoadSavedQuery = (sq: SavedQuery) => {
    if (typeof sq.spec?.sql === "string") {
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

  const handleSyncBottomResult = useCallback(
    (payload: {
      columns: QueryColumn[];
      rows: Record<string, unknown>[];
      sql: string;
      durationMs: number;
    }) => {
      setResultColumns(payload.columns);
      setResultRows(payload.rows);
      setResultRowCount(payload.rows.length);
      setResultTruncated(false);
      setResultDurationMs(payload.durationMs);
      setResultExecutedSql(payload.sql);
    },
    []
  );

  return (
    <div className="space-y-4" data-testid="analytics-studio-page">
      {/* Page Header */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            AxAnalytics · OLAP 与智能问数
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            受控 SQL 工作台、拖拽式透视制表、指标语义层与 AI 数据变换
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="neutral">数据集 {catalog.datasets.length}</Badge>
          <Badge variant="xblue">指标 {metrics.length}</Badge>

          <Button
            type="button"
            variant="outline"
            onClick={() => setTopSaveOpen((prev) => !prev)}
            data-testid="studio-save-query-btn"
          >
            <Save className="h-3.5 w-3.5 text-xblue" />
            保存为查询
          </Button>

          <Button
            type="button"
            variant="outline"
            onClick={() => setHistoryDrawerOpen((prev) => !prev)}
            data-testid="studio-history-drawer-btn"
          >
            <History className="h-3.5 w-3.5 text-xblue" />
            执行历史 ({historyEntries.length})
          </Button>
        </div>
      </div>

      {/* Top Save Query Dialog */}
      {topSaveOpen && (
        <Card
          className="flex flex-wrap items-center gap-2 bg-muted/20 p-3"
          data-testid="studio-save-dialog"
        >
          <Input
            value={topSaveName}
            onChange={(e) => setTopSaveName(e.target.value)}
            placeholder="输入保存查询名称，例如：orders_summary"
            className="h-8 max-w-xs text-xs font-mono-tech"
            data-testid="studio-save-name-input"
          />
          <label className="inline-flex items-center gap-1.5 text-xs cursor-pointer">
            <input
              type="checkbox"
              checked={topSaveShared}
              onChange={(e) => setTopSaveShared(e.target.checked)}
              className="accent-xblue"
            />
            <span>租户内共享</span>
          </label>
          <Button
            type="button"
            variant="primary"
            disabled={topSaving || !topSaveName.trim()}
            onClick={() => void handleTopSaveConfirm()}
            data-testid="studio-save-confirm-btn"
          >
            {topSaving ? "保存中..." : "确认保存"}
          </Button>
          <Button
            type="button"
            variant="ghost"
            onClick={() => setTopSaveOpen(false)}
          >
            取消
          </Button>
        </Card>
      )}

      {/* Top History & Saved Queries Drawer */}
      {historyDrawerOpen && (
        <Card
          className="p-3 space-y-2 bg-muted/20"
          data-testid="studio-history-drawer"
        >
          <div className="flex items-center justify-between border-b border-border pb-2 text-xs font-semibold">
            <span>查询审计历史与已保存资产</span>
            <button
              type="button"
              onClick={() => setHistoryDrawerOpen(false)}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 text-xs">
            <div className="space-y-1 max-h-40 overflow-y-auto">
              <div className="text-[11px] font-medium text-muted-foreground">
                已保存查询 ({savedQueries.length})
              </div>
              {savedQueries.length === 0 ? (
                <div className="py-2 text-muted-foreground">暂无保存查询</div>
              ) : (
                savedQueries.map((sq) => (
                  <div
                    key={sq.id}
                    className="flex items-center justify-between gap-2 rounded border border-border bg-background px-2.5 py-1.5"
                  >
                    <button
                      type="button"
                      onClick={() => {
                        handleLoadSavedQuery(sq);
                        setHistoryDrawerOpen(false);
                      }}
                      className="min-w-0 flex-1 text-left cursor-pointer"
                    >
                      <span className="font-mono-tech font-medium">
                        {sq.name}
                      </span>{" "}
                      <Badge variant="outline">{sq.kind}</Badge>
                    </button>
                    <button
                      type="button"
                      onClick={() => void handleDeleteSavedQuery(sq.id)}
                      className="text-muted-foreground hover:text-foreground cursor-pointer"
                    >
                      <Trash2 className="h-3 w-3" />
                    </button>
                  </div>
                ))
              )}
            </div>

            <div className="space-y-1 max-h-40 overflow-y-auto">
              <div className="text-[11px] font-medium text-muted-foreground">
                最近执行记录 ({historyEntries.length})
              </div>
              {historyEntries.length === 0 ? (
                <div className="py-2 text-muted-foreground">暂无历史记录</div>
              ) : (
                historyEntries.map((h) => (
                  <button
                    key={h.id}
                    type="button"
                    onClick={() => {
                      setSqlText(h.sql_text);
                      setActiveTab("sql");
                      setHistoryDrawerOpen(false);
                    }}
                    className="flex w-full items-center justify-between gap-2 rounded border border-border bg-background px-2.5 py-1.5 text-left hover:bg-muted/40 cursor-pointer"
                  >
                    <span className="truncate font-mono-tech text-[11px]">
                      {h.sql_text}
                    </span>
                    <div className="flex shrink-0 items-center gap-1">
                      <Badge
                        variant={
                          h.status === "SUCCEEDED" ? "xblue" : "neutral"
                        }
                      >
                        {h.status}
                      </Badge>
                    </div>
                  </button>
                ))
              )}
            </div>
          </div>
        </Card>
      )}

      {/* Main Studio Grid: Left SchemaTree + Right Workspace */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
        <div className="lg:col-span-3 lg:sticky lg:top-4 max-h-[760px]">
          <SchemaTree
            catalog={catalog}
            loading={catalogLoading}
            selectedPivotDataset={selectedPivotDataset}
            onRefresh={() => void loadCatalog()}
            onInsertIdentifier={handleInsertIdentifier}
            onSelectForPivot={handleSelectForPivot}
          />
        </div>

        <div className="lg:col-span-9 space-y-4">
          {/* Four Core Workspace Tabs: SQL / 透视 / 指标 / AI 处理 */}
          <div
            className="inline-flex flex-wrap items-center rounded-md border border-border bg-muted/40 p-0.5"
            data-testid="analytics-tabs"
          >
            <button
              type="button"
              onClick={() => setActiveTab("sql")}
              data-testid="analytics-tab-sql"
              className={`inline-flex items-center gap-1.5 rounded px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeTab === "sql"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <Code2 className="h-3.5 w-3.5 text-xblue" />
              SQL
            </button>

            <button
              type="button"
              onClick={() => setActiveTab("pivot")}
              data-testid="analytics-tab-pivot"
              className={`inline-flex items-center gap-1.5 rounded px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeTab === "pivot"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <TableProperties className="h-3.5 w-3.5 text-xblue" />
              透视
            </button>

            <button
              type="button"
              onClick={() => setActiveTab("metrics")}
              data-testid="analytics-tab-metrics"
              className={`inline-flex items-center gap-1.5 rounded px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeTab === "metrics"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <Award className="h-3.5 w-3.5 text-xblue" />
              指标
            </button>

            <button
              type="button"
              onClick={() => setActiveTab("transform")}
              data-testid="analytics-tab-transform"
              className={`inline-flex items-center gap-1.5 rounded px-3.5 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeTab === "transform"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <Wand2 className="h-3.5 w-3.5 text-xblue" />
              AI 处理
            </button>
          </div>

          {/* Active Workspace Tab Panel */}
          {activeTab === "sql" && (
            <div className="space-y-3">
              <SqlEditor
                sql={sqlText}
                onChangeSql={setSqlText}
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

              {sqlError && (
                <Card
                  className="border-foreground/40 bg-muted/40 p-3 text-xs"
                  data-testid="sql-error-card"
                >
                  <span className="font-mono-tech font-semibold">
                    SQL 守卫或执行异常：
                  </span>
                  {sqlError}
                </Card>
              )}
            </div>
          )}

          {activeTab === "pivot" && (
            <PivotBuilder
              catalog={catalog}
              selectedDataset={selectedPivotDataset}
              onSelectDataset={setSelectedPivotDataset}
              onSyncBottomResult={handleSyncBottomResult}
              onSavePivotQuery={handleSavePivotQuery}
            />
          )}

          {activeTab === "metrics" && (
            <MetricManager
              catalog={catalog}
              metrics={metrics}
              onRefreshMetrics={loadMetrics}
              onSyncBottomResult={handleSyncBottomResult}
            />
          )}

          {activeTab === "transform" && (
            <TransformWizard
              catalog={catalog}
              onMaterialized={loadCatalog}
              onSyncBottomResult={handleSyncBottomResult}
            />
          )}

          {/* Bottom Linked ResultGrid + ChartPanel */}
          <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
            <div className="xl:col-span-7">
              <ResultGrid
                columns={resultColumns}
                rows={resultRows}
                rowCount={resultRowCount}
                truncated={resultTruncated}
                durationMs={resultDurationMs}
                sql={resultExecutedSql}
              />
            </div>
            <div className="xl:col-span-5">
              <ChartPanel columns={resultColumns} rows={resultRows} />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
