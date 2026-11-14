import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  Award,
  CheckCircle2,
  Edit3,
  Play,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
  type CreateMetricRequest,
  type MetricCompareMode,
  type MetricDef,
  type MetricDimension,
  type MetricQuery,
  type MetricQueryResult,
  type QueryColumn,
} from "@/lib/api/analytics";
import type { RollupGrain } from "@/lib/api/enums";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

const GRAIN_OPTIONS: readonly RollupGrain[] = ["MINUTE", "HOUR", "DAY", "MONTH"];

export interface MetricManagerProps {
  catalog: AnalyticsCatalogResponse;
  metrics: MetricDef[];
  onRefreshMetrics: () => Promise<void> | void;
  onSyncBottomResult?: (payload: {
    columns: QueryColumn[];
    rows: Record<string, unknown>[];
    sql: string;
    durationMs: number;
  }) => void;
}

export function MetricManager({
  catalog,
  metrics,
  onRefreshMetrics,
  onSyncBottomResult,
}: MetricManagerProps) {
  const [selectedMetricName, setSelectedMetricName] = useState<string>("");
  const [multiSelectedNames, setMultiSelectedNames] = useState<string[]>([]);

  // Form state (create / edit)
  const [formMode, setFormMode] = useState<"closed" | "create" | "edit">(
    "closed"
  );
  const [formName, setFormName] = useState("");
  const [formDisplayName, setFormDisplayName] = useState("");
  const [formDescription, setFormDescription] = useState("");
  const [formDatasetId, setFormDatasetId] = useState("");
  const [formExpression, setFormExpression] = useState("SUM(amount)");
  const [formFilterSql, setFormFilterSql] = useState("");
  const [formTimeColumn, setFormTimeColumn] = useState("");
  const [formDefaultGrain, setFormDefaultGrain] = useState<RollupGrain>("MONTH");
  const [formDimensions, setFormDimensions] = useState<string[]>([]);
  const [formUnit, setFormUnit] = useState("");
  const [formFormat, setFormFormat] = useState("0,0.00");
  const [formSynonyms, setFormSynonyms] = useState("");
  const [formCertified, setFormCertified] = useState(false);
  const [submittingForm, setSubmittingForm] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  // Query trial state
  const [queryDims, setQueryDims] = useState<string[]>([]);
  const [queryGrain, setQueryGrain] = useState<RollupGrain>("MONTH");
  const [queryCompare, setQueryCompare] = useState<"" | MetricCompareMode>("");
  const [queryFromDate, setQueryFromDate] = useState("");
  const [queryToDate, setQueryToDate] = useState("");
  const [runningQuery, setRunningQuery] = useState(false);
  const [queryError, setQueryError] = useState<string | null>(null);
  const [lastMetricResult, setLastMetricResult] =
    useState<MetricQueryResult | null>(null);

  useEffect(() => {
    if (!selectedMetricName && metrics[0]) {
      setSelectedMetricName(metrics[0].name);
      setQueryGrain(metrics[0].default_grain);
    } else if (
      selectedMetricName &&
      !metrics.some((m) => m.name === selectedMetricName)
    ) {
      setSelectedMetricName(metrics[0]?.name ?? "");
    }
  }, [metrics, selectedMetricName]);

  const activeMetric = useMemo(
    () => metrics.find((m) => m.name === selectedMetricName) ?? null,
    [metrics, selectedMetricName]
  );

  const selectedFormDataset = useMemo(
    () =>
      (catalog.datasets ?? []).find((d) => d.id === formDatasetId) ??
      catalog.datasets?.[0] ??
      null,
    [catalog.datasets, formDatasetId]
  );

  const openCreateForm = () => {
    const firstDs = catalog.datasets?.[0];
    setFormMode("create");
    setFormName("");
    setFormDisplayName("");
    setFormDescription("");
    setFormDatasetId(firstDs?.id ?? "");
    setFormExpression("SUM(amount)");
    setFormFilterSql("");
    setFormTimeColumn("");
    setFormDefaultGrain("MONTH");
    setFormDimensions([]);
    setFormUnit("");
    setFormFormat("0,0.00");
    setFormSynonyms("");
    setFormCertified(false);
    setFormError(null);
  };

  const openEditForm = (m: MetricDef) => {
    setFormMode("edit");
    setFormName(m.name);
    setFormDisplayName(m.display_name);
    setFormDescription(m.description);
    setFormDatasetId(m.dataset_id);
    setFormExpression(m.expression);
    setFormFilterSql(m.filter_sql);
    setFormTimeColumn(m.time_column ?? "");
    setFormDefaultGrain(m.default_grain);
    setFormDimensions(m.dimensions.map((d) => d.column));
    setFormUnit(m.unit);
    setFormFormat(m.format);
    setFormSynonyms(m.synonyms.join(", "));
    setFormCertified(m.certified);
    setFormError(null);
  };

  const handleSaveForm = async () => {
    setSubmittingForm(true);
    setFormError(null);
    try {
      const dimsPayload: MetricDimension[] = formDimensions.map((col) => ({
        name: col,
        column: col,
        display_name: col,
      }));
      const synonymsArr = formSynonyms
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);

      if (formMode === "create") {
        const dsId = formDatasetId || selectedFormDataset?.id || "";
        const payload: CreateMetricRequest = {
          name: formName.trim(),
          display_name: formDisplayName.trim() || formName.trim(),
          description: formDescription.trim(),
          dataset_id: dsId,
          expression: formExpression.trim(),
          filter_sql: formFilterSql.trim(),
          time_column: formTimeColumn ? formTimeColumn : null,
          default_grain: formDefaultGrain,
          dimensions: dimsPayload,
          unit: formUnit.trim(),
          format: formFormat.trim(),
          synonyms: synonymsArr,
          certified: formCertified,
        };
        const created = await analyticsApi.createMetric(payload);
        await onRefreshMetrics();
        setSelectedMetricName(created.name);
      } else if (formMode === "edit") {
        await analyticsApi.updateMetric(formName, {
          display_name: formDisplayName.trim() || formName,
          description: formDescription.trim(),
          expression: formExpression.trim(),
          filter_sql: formFilterSql.trim(),
          time_column: formTimeColumn ? formTimeColumn : null,
          default_grain: formDefaultGrain,
          dimensions: dimsPayload,
          unit: formUnit.trim(),
          format: formFormat.trim(),
          synonyms: synonymsArr,
          certified: formCertified,
        });
        await onRefreshMetrics();
      }
      setFormMode("closed");
    } catch (err) {
      setFormError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmittingForm(false);
    }
  };

  const handleDeleteMetric = async (name: string) => {
    try {
      await analyticsApi.deleteMetric(name);
      setMultiSelectedNames((prev) => prev.filter((n) => n !== name));
      await onRefreshMetrics();
    } catch (err) {
      setQueryError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleRunMetricQuery = async () => {
    if (!activeMetric && multiSelectedNames.length === 0) return;
    setRunningQuery(true);
    setQueryError(null);
    try {
      let timeRange: MetricQuery["time_range"] | undefined;
      if (queryFromDate && queryToDate) {
        timeRange = {
          from: new Date(`${queryFromDate}T00:00:00Z`).toISOString(),
          to: new Date(`${queryToDate}T00:00:00Z`).toISOString(),
        };
      } else if (queryCompare) {
        const now = new Date();
        const from = new Date(now.getTime() - 30 * 24 * 3600 * 1000);
        timeRange = {
          from: from.toISOString(),
          to: now.toISOString(),
        };
      }

      const baseQuery: MetricQuery = {
        dimensions: queryDims,
        filters: [],
        ...(timeRange ? { time_range: timeRange } : {}),
        grain: queryGrain,
        ...(queryCompare ? { compare: queryCompare } : {}),
      };

      let res: MetricQueryResult;
      if (multiSelectedNames.length >= 2) {
        res = await analyticsApi.queryMultiMetrics({
          metric_names: multiSelectedNames,
          ...baseQuery,
        });
      } else {
        const targetName = multiSelectedNames[0] ?? activeMetric?.name ?? "";
        res = await analyticsApi.queryMetric(targetName, baseQuery);
      }

      setLastMetricResult(res);
      onSyncBottomResult?.({
        columns: res.columns,
        rows: res.rows,
        sql: res.sql,
        durationMs: res.duration_ms,
      });
    } catch (err) {
      setQueryError(err instanceof Error ? err.message : String(err));
    } finally {
      setRunningQuery(false);
    }
  };

  const datasetNameMap = useMemo(() => {
    const map: Record<string, string> = {};
    for (const d of catalog.datasets ?? []) {
      map[d.id] = d.name;
    }
    return map;
  }, [catalog.datasets]);

  return (
    <div className="space-y-3" data-testid="analytics-metric-manager">
      {/* Header Bar */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-xs">
        <div className="flex items-center gap-2">
          <Award className="h-3.5 w-3.5 text-xblue" />
          <span className="font-semibold">统一指标语义层</span>
          <Badge variant="neutral">共 {metrics.length} 项</Badge>
          {metrics.some((m) => m.broken) && (
            <Badge variant="outline" data-testid="metric-broken-summary">
              <AlertTriangle className="mr-1 h-3 w-3 text-xblue" />
              存在上游结构破损指标
            </Badge>
          )}
        </div>

        <Button
          type="button"
          variant="primary"
          onClick={openCreateForm}
          data-testid="metric-new-btn"
        >
          <Plus className="h-3.5 w-3.5" />
          新建语义指标
        </Button>
      </div>

      {/* Create / Edit Form */}
      {formMode !== "closed" && (
        <Card className="p-4 space-y-3 bg-muted/20" data-testid="metric-form-card">
          <div className="flex items-center justify-between border-b border-border pb-2 text-xs font-semibold">
            <span>
              {formMode === "create"
                ? "新建统一语义指标"
                : `编辑指标 · ${formName}`}
            </span>
            <button
              type="button"
              onClick={() => setFormMode("closed")}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
            <div className="space-y-1">
              <label className="text-muted-foreground">指标标识名 (a-z0-9_)</label>
              <Input
                value={formName}
                disabled={formMode === "edit"}
                onChange={(e) => setFormName(e.target.value)}
                placeholder="例如: paid_amount"
                className="h-8 text-xs font-mono-tech"
                data-testid="metric-form-name"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">展示名称</label>
              <Input
                value={formDisplayName}
                onChange={(e) => setFormDisplayName(e.target.value)}
                placeholder="例如: 已支付订单总金额"
                className="h-8 text-xs"
                data-testid="metric-form-display-name"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">来源数据集</label>
              <select
                value={formDatasetId || selectedFormDataset?.id || ""}
                disabled={formMode === "edit"}
                onChange={(e) => {
                  setFormDatasetId(e.target.value);
                  setFormDimensions([]);
                  setFormTimeColumn("");
                }}
                className="h-8 w-full rounded-md border border-input bg-background px-2.5 font-mono-tech text-xs"
                data-testid="metric-form-dataset"
              >
                {(catalog.datasets ?? []).map((ds) => (
                  <option key={ds.id} value={ds.id}>
                    {ds.name} ({ds.display_name || "dataset"})
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1 md:col-span-2">
              <label className="text-muted-foreground">
                聚合 SQL 表达式 (SUM / AVG / COUNT 等)
              </label>
              <Input
                value={formExpression}
                onChange={(e) => setFormExpression(e.target.value)}
                placeholder="SUM(amount)"
                className="h-8 text-xs font-mono-tech"
                data-testid="metric-form-expression"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">固定布尔过滤条件</label>
              <Input
                value={formFilterSql}
                onChange={(e) => setFormFilterSql(e.target.value)}
                placeholder="status = 'PAID'"
                className="h-8 text-xs font-mono-tech"
                data-testid="metric-form-filter"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">时间列 (可选)</label>
              <select
                value={formTimeColumn}
                onChange={(e) => setFormTimeColumn(e.target.value)}
                className="h-8 w-full rounded-md border border-input bg-background px-2.5 font-mono-tech text-xs"
              >
                <option value="">不绑定时间列</option>
                {(selectedFormDataset?.columns ?? [])
                  .filter(
                    (c) =>
                      c.data_type === "DATE" || c.data_type === "TIMESTAMP"
                  )
                  .map((c) => (
                    <option key={c.column_name} value={c.column_name}>
                      {c.column_name} ({c.data_type})
                    </option>
                  ))}
              </select>
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">默认时间粒度</label>
              <select
                value={formDefaultGrain}
                onChange={(e) =>
                  setFormDefaultGrain(e.target.value as RollupGrain)
                }
                className="h-8 w-full rounded-md border border-input bg-background px-2.5 font-mono-tech text-xs"
              >
                {GRAIN_OPTIONS.map((g) => (
                  <option key={g} value={g}>
                    {g}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">单位 / 同义词</label>
              <div className="flex gap-1.5">
                <Input
                  value={formUnit}
                  onChange={(e) => setFormUnit(e.target.value)}
                  placeholder="单位(CNY)"
                  className="h-8 w-24 text-xs font-mono-tech"
                />
                <Input
                  value={formSynonyms}
                  onChange={(e) => setFormSynonyms(e.target.value)}
                  placeholder="同义词逗号分隔"
                  className="h-8 flex-1 text-xs"
                />
              </div>
            </div>
          </div>

          {selectedFormDataset && selectedFormDataset.columns.length > 0 && (
            <div className="space-y-1 text-xs">
              <div className="text-muted-foreground">可下钻分析维度:</div>
              <div className="flex flex-wrap gap-1.5">
                {selectedFormDataset.columns.map((col) => {
                  const checked = formDimensions.includes(col.column_name);
                  return (
                    <label
                      key={col.column_name}
                      className={`inline-flex items-center gap-1 rounded border px-2 py-1 font-mono-tech text-[11px] cursor-pointer ${
                        checked
                          ? "border-xblue bg-xblue-soft text-xblue"
                          : "border-border bg-background text-foreground"
                      }`}
                    >
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(e) => {
                          if (e.target.checked) {
                            setFormDimensions((prev) => [
                              ...prev,
                              col.column_name,
                            ]);
                          } else {
                            setFormDimensions((prev) =>
                              prev.filter((d) => d !== col.column_name)
                            );
                          }
                        }}
                        className="accent-xblue"
                      />
                      {col.column_name}
                    </label>
                  );
                })}
              </div>
            </div>
          )}

          <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
            <label className="inline-flex items-center gap-1.5 text-xs cursor-pointer">
              <input
                type="checkbox"
                checked={formCertified}
                onChange={(e) => setFormCertified(e.target.checked)}
                className="accent-xblue"
                data-testid="metric-form-certified"
              />
              <span>标记为官方认证口径 (Certified)</span>
            </label>

            <div className="flex items-center gap-2">
              {formError && (
                <span className="font-mono-tech text-xs text-foreground">
                  {formError}
                </span>
              )}
              <Button
                type="button"
                variant="ghost"
                onClick={() => setFormMode("closed")}
              >
                取消
              </Button>
              <Button
                type="button"
                variant="primary"
                disabled={
                  submittingForm || !formName.trim() || !formExpression.trim()
                }
                onClick={() => void handleSaveForm()}
                data-testid="metric-form-submit"
              >
                {submittingForm ? "提交中..." : "保存指标定义"}
              </Button>
            </div>
          </div>
        </Card>
      )}

      {/* Metrics List + Trial Runner */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-3">
        {/* Metric List */}
        <Card className="lg:col-span-7 p-3 space-y-2">
          <div className="text-xs font-medium text-muted-foreground">
            已注册指标目录（勾选多项可联合对比查询）
          </div>

          {metrics.length === 0 ? (
            <div className="py-8 text-center text-xs text-muted-foreground">
              当前租户尚未定义语义指标，点击右上角「新建语义指标」创建
            </div>
          ) : (
            <div className="space-y-2 max-h-72 overflow-y-auto">
              {metrics.map((m) => {
                const isSelected = selectedMetricName === m.name;
                const isMulti = multiSelectedNames.includes(m.name);
                return (
                  <div
                    key={m.id}
                    data-testid={`metric-item-${m.name}`}
                    className={`rounded border p-2.5 transition-colors ${
                      isSelected
                        ? "border-xblue bg-xblue-soft/25"
                        : "border-border hover:bg-muted/40"
                    }`}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="flex items-start gap-2 min-w-0">
                        <input
                          type="checkbox"
                          checked={isMulti}
                          onChange={(e) => {
                            if (e.target.checked) {
                              setMultiSelectedNames((prev) => [
                                ...prev,
                                m.name,
                              ]);
                            } else {
                              setMultiSelectedNames((prev) =>
                                prev.filter((n) => n !== m.name)
                              );
                            }
                          }}
                          className="mt-1 accent-xblue"
                          title="加入多指标联合查询"
                        />
                        <button
                          type="button"
                          onClick={() => {
                            setSelectedMetricName(m.name);
                            setQueryDims([]);
                            setQueryGrain(m.default_grain);
                          }}
                          className="text-left min-w-0 cursor-pointer"
                        >
                          <div className="flex flex-wrap items-center gap-1.5">
                            <span className="text-xs font-semibold text-foreground">
                              {m.display_name}
                            </span>
                            <span className="font-mono-tech text-[11px] text-muted-foreground">
                              ({m.name})
                            </span>
                            {m.certified && (
                              <Badge
                                variant="xblue"
                                data-testid={`metric-certified-${m.name}`}
                              >
                                <CheckCircle2 className="mr-0.5 h-3 w-3" />
                                已认证
                              </Badge>
                            )}
                            {m.broken && (
                              <Badge
                                variant="neutral"
                                data-testid={`metric-broken-${m.name}`}
                              >
                                <AlertTriangle className="mr-0.5 h-3 w-3 text-xblue" />
                                结构破损
                              </Badge>
                            )}
                            <Badge variant="outline">v{m.version}</Badge>
                          </div>
                          <div className="mt-1 font-mono-tech text-[11px] text-muted-foreground truncate">
                            {m.expression}
                            {m.filter_sql ? ` WHERE ${m.filter_sql}` : ""} ·{" "}
                            {datasetNameMap[m.dataset_id] ?? "dataset"}
                          </div>
                        </button>
                      </div>

                      <div className="flex shrink-0 items-center gap-1">
                        <button
                          type="button"
                          onClick={() => openEditForm(m)}
                          data-testid={`metric-edit-${m.name}`}
                          className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
                          title="编辑指标"
                        >
                          <Edit3 className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={() => void handleDeleteMetric(m.name)}
                          data-testid={`metric-delete-${m.name}`}
                          className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
                          title="删除指标"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </Card>

        {/* Trial Runner Panel */}
        <Card className="lg:col-span-5 p-3 space-y-3 text-xs">
          <div className="flex items-center justify-between border-b border-border pb-2">
            <span className="font-semibold">指标试算与同环比分析</span>
            {multiSelectedNames.length >= 2 ? (
              <Badge variant="xblue">
                联合 {multiSelectedNames.length} 指标
              </Badge>
            ) : activeMetric ? (
              <Badge variant="neutral">{activeMetric.name}</Badge>
            ) : null}
          </div>

          {!activeMetric && multiSelectedNames.length === 0 ? (
            <div className="py-6 text-center text-muted-foreground">
              请从左侧选择一个或多个指标进行试算
            </div>
          ) : (
            <>
              {activeMetric && activeMetric.dimensions.length > 0 && (
                <div className="space-y-1">
                  <div className="text-muted-foreground">分组维度:</div>
                  <div className="flex flex-wrap gap-1.5">
                    {activeMetric.dimensions.map((dim) => {
                      const active = queryDims.includes(dim.name);
                      return (
                        <label
                          key={dim.name}
                          className={`inline-flex items-center gap-1 rounded border px-2 py-1 font-mono-tech text-[11px] cursor-pointer ${
                            active
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border bg-background"
                          }`}
                        >
                          <input
                            type="checkbox"
                            checked={active}
                            onChange={(e) => {
                              if (e.target.checked) {
                                setQueryDims((prev) => [...prev, dim.name]);
                              } else {
                                setQueryDims((prev) =>
                                  prev.filter((d) => d !== dim.name)
                                );
                              }
                            }}
                            className="accent-xblue"
                          />
                          {dim.display_name || dim.name}
                        </label>
                      );
                    })}
                  </div>
                </div>
              )}

              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="text-muted-foreground">时间粒度</label>
                  <select
                    value={queryGrain}
                    onChange={(e) =>
                      setQueryGrain(e.target.value as RollupGrain)
                    }
                    className="h-8 w-full rounded border border-input bg-background px-2 font-mono-tech text-xs"
                    data-testid="metric-query-grain"
                  >
                    {GRAIN_OPTIONS.map((g) => (
                      <option key={g} value={g}>
                        {g}
                      </option>
                    ))}
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="text-muted-foreground">同环比模式</label>
                  <select
                    value={queryCompare}
                    onChange={(e) =>
                      setQueryCompare(
                        e.target.value as "" | MetricCompareMode
                      )
                    }
                    className="h-8 w-full rounded border border-input bg-background px-2 text-xs"
                    data-testid="metric-query-compare"
                  >
                    <option value="">不对比</option>
                    <option value="PREVIOUS_PERIOD">环比 (PREVIOUS_PERIOD)</option>
                    <option value="SAME_PERIOD_LAST_YEAR">
                      同比 (SAME_PERIOD_LAST_YEAR)
                    </option>
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="text-muted-foreground">起始日期 (可选)</label>
                  <Input
                    type="date"
                    value={queryFromDate}
                    onChange={(e) => setQueryFromDate(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-muted-foreground">结束日期 (可选)</label>
                  <Input
                    type="date"
                    value={queryToDate}
                    onChange={(e) => setQueryToDate(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
              </div>

              <Button
                type="button"
                variant="primary"
                className="w-full"
                disabled={runningQuery || Boolean(activeMetric?.broken)}
                onClick={() => void handleRunMetricQuery()}
                data-testid="metric-run-query-btn"
              >
                <Play className="h-3.5 w-3.5" />
                {runningQuery ? "正在编译并计算..." : "运行指标试算"}
              </Button>

              {queryError && (
                <div className="rounded border border-foreground/30 bg-muted/50 p-2 font-mono-tech text-[11px]">
                  {queryError}
                </div>
              )}

              {lastMetricResult && (
                <div
                  className="rounded border border-border bg-muted/20 p-2 space-y-1"
                  data-testid="metric-query-summary"
                >
                  <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                    <span>返回 {lastMetricResult.rows.length} 行</span>
                    <span className="font-mono-tech">
                      {lastMetricResult.duration_ms} ms
                    </span>
                  </div>
                  <pre className="max-h-20 overflow-auto font-mono-tech text-[10px] text-foreground whitespace-pre-wrap">
                    {lastMetricResult.sql}
                  </pre>
                </div>
              )}
            </>
          )}
        </Card>
      </div>
    </div>
  );
}
