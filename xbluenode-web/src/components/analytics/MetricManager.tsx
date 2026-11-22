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
import { useI18n } from "@/i18n";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
  type CreateMetricRequest,
  type MetricCompareMode,
  type MetricDef,
  type MetricDimension,
  type MetricQuery,
  type MetricQueryResult,
} from "@/lib/api/analytics";
import type { RollupGrain } from "@/lib/api/enums";
import { usePrincipal } from "@/lib/permissions";
import { ChartPanel } from "@/components/analytics/ChartPanel";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Input,
  Select,
} from "@/components/ui/primitives";

const GRAIN_OPTIONS: readonly RollupGrain[] = ["MINUTE", "HOUR", "DAY", "MONTH"];

export interface MetricManagerProps {
  catalog: AnalyticsCatalogResponse;
  metrics: MetricDef[];
  onRefreshMetrics: () => Promise<void> | void;
}

export function MetricManager({
  catalog,
  metrics,
  onRefreshMetrics,
}: MetricManagerProps) {
  const { tx, fmtNum } = useI18n();
  const { canAny } = usePrincipal();
  const canEditMetrics = canAny(["STUDIO_EDIT", "DATA_SYNC", "DATA_ADMIN"]);

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

  const [pendingDeleteMetric, setPendingDeleteMetric] =
    useState<MetricDef | null>(null);
  const [deletingMetric, setDeletingMetric] = useState(false);

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

  const grainLabel = (g: RollupGrain): string => {
    switch (g) {
      case "MINUTE":
        return tx("分钟", "Minute");
      case "HOUR":
        return tx("小时", "Hour");
      case "DAY":
        return tx("日", "Day");
      case "MONTH":
        return tx("月", "Month");
      default:
        return g;
    }
  };

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
    if (!canEditMetrics) return;
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
    if (!canEditMetrics) return;
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
    if (!canEditMetrics) return;
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

  const handleConfirmDeleteMetric = async () => {
    if (!pendingDeleteMetric || !canEditMetrics) return;
    setDeletingMetric(true);
    try {
      const name = pendingDeleteMetric.name;
      await analyticsApi.deleteMetric(name);
      setMultiSelectedNames((prev) => prev.filter((n) => n !== name));
      setPendingDeleteMetric(null);
      await onRefreshMetrics();
    } catch (err) {
      setQueryError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingMetric(false);
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
    <div className="space-y-4" data-testid="analytics-metric-manager">
      {/* Header Bar */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-xs">
        <div className="flex items-center gap-2">
          <Award className="h-3.5 w-3.5 text-xblue" />
          <span className="font-semibold">
            {tx("统一指标语义层", "Semantic Metrics")}
          </span>
          <Badge variant="neutral">{fmtNum(metrics.length, 0)}</Badge>
          {metrics.some((m) => m.broken) && (
            <Badge variant="warning" data-testid="metric-broken-summary">
              <AlertTriangle className="mr-1 h-3 w-3" />
              {tx("存在上游结构异常", "Broken upstream schema")}
            </Badge>
          )}
        </div>

        <Button
          type="button"
          variant="primary"
          disabled={!canEditMetrics}
          onClick={openCreateForm}
          data-testid="metric-new-btn"
        >
          <Plus className="h-3.5 w-3.5" />
          {tx("新建指标", "New Metric")}
        </Button>
      </div>

      {/* Create / Edit Form */}
      {formMode !== "closed" && (
        <Card
          className="p-4 space-y-3 bg-muted/20"
          data-testid="metric-form-card"
        >
          <div className="flex items-center justify-between border-b border-border pb-2 text-xs font-semibold">
            <span>
              {formMode === "create"
                ? tx("新建语义指标", "Create Semantic Metric")
                : `${tx("编辑指标", "Edit Metric")} · ${formName}`}
            </span>
            <button
              type="button"
              onClick={() => setFormMode("closed")}
              aria-label={tx("关闭", "Close")}
              className="text-muted-foreground hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("指标标识", "Metric Key")}
              </label>
              <Input
                value={formName}
                disabled={formMode === "edit"}
                onChange={(e) => setFormName(e.target.value)}
                placeholder="paid_amount"
                className="h-8 text-xs font-mono-tech"
                data-testid="metric-form-name"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("显示名称", "Display Name")}
              </label>
              <Input
                value={formDisplayName}
                onChange={(e) => setFormDisplayName(e.target.value)}
                placeholder={tx("已支付订单总金额", "Total Paid Order Amount")}
                className="h-8 text-xs"
                data-testid="metric-form-display-name"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("来源数据集", "Source Dataset")}
              </label>
              <Select
                value={formDatasetId || selectedFormDataset?.id || ""}
                disabled={formMode === "edit"}
                onChange={(e) => {
                  setFormDatasetId(e.target.value);
                  setFormDimensions([]);
                  setFormTimeColumn("");
                }}
                className="h-8 w-full font-mono-tech"
                data-testid="metric-form-dataset"
              >
                {(catalog.datasets ?? []).map((ds) => (
                  <option key={ds.id} value={ds.id}>
                    {ds.display_name && ds.display_name !== ds.name
                      ? `${ds.name} · ${ds.display_name}`
                      : ds.name}
                  </option>
                ))}
              </Select>
            </div>

            <div className="space-y-1 md:col-span-2">
              <label className="text-muted-foreground">
                {tx("聚合表达式", "SQL Aggregation Expression")}
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
              <label className="text-muted-foreground">
                {tx("固定过滤条件", "Filter Predicate")}
              </label>
              <Input
                value={formFilterSql}
                onChange={(e) => setFormFilterSql(e.target.value)}
                placeholder="status = 'PAID'"
                className="h-8 text-xs font-mono-tech"
                data-testid="metric-form-filter"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("时间列", "Time Column")}
              </label>
              <Select
                value={formTimeColumn}
                onChange={(e) => setFormTimeColumn(e.target.value)}
                className="h-8 w-full font-mono-tech"
              >
                <option value="">{tx("无", "None")}</option>
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
              </Select>
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("默认粒度", "Default Grain")}
              </label>
              <Select
                value={formDefaultGrain}
                onChange={(e) =>
                  setFormDefaultGrain(e.target.value as RollupGrain)
                }
                className="h-8 w-full"
              >
                {GRAIN_OPTIONS.map((g) => (
                  <option key={g} value={g}>
                    {grainLabel(g)}
                  </option>
                ))}
              </Select>
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("单位 / 同义词", "Unit / Synonyms")}
              </label>
              <div className="flex gap-1.5">
                <Input
                  value={formUnit}
                  onChange={(e) => setFormUnit(e.target.value)}
                  placeholder="CNY"
                  className="h-8 w-24 text-xs font-mono-tech"
                />
                <Input
                  value={formSynonyms}
                  onChange={(e) => setFormSynonyms(e.target.value)}
                  placeholder={tx("实收金额, 已付款金额", "paid_total, revenue")}
                  className="h-8 flex-1 text-xs"
                />
              </div>
            </div>
          </div>

          {selectedFormDataset && selectedFormDataset.columns.length > 0 && (
            <div className="space-y-1 text-xs">
              <div className="text-muted-foreground">
                {tx("可下钻维度", "Drill-down Dimensions")}:
              </div>
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
              <span>{tx("标记为官方认证口径", "Certified Metric")}</span>
            </label>

            <div className="flex items-center gap-2">
              {formError && (
                <span className="font-mono-tech text-xs text-red-600 dark:text-red-400">
                  {formError}
                </span>
              )}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => setFormMode("closed")}
              >
                {tx("取消", "Cancel")}
              </Button>
              <Button
                type="button"
                variant="primary"
                size="sm"
                disabled={
                  submittingForm || !formName.trim() || !formExpression.trim()
                }
                onClick={() => void handleSaveForm()}
                data-testid="metric-form-submit"
              >
                {submittingForm
                  ? tx("保存中…", "Saving…")
                  : tx("保存", "Save")}
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
            {tx("指标目录", "Registered Metrics")}
          </div>

          {metrics.length === 0 ? (
            <EmptyState
              title={tx("暂无语义指标", "No semantic metrics defined")}
            />
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
                          title={tx("联合试算", "Compare metric")}
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
                            {m.display_name !== m.name && (
                              <span className="font-mono-tech text-[11px] text-muted-foreground">
                                {m.name}
                              </span>
                            )}
                            {m.certified && (
                              <Badge
                                variant="xblue"
                                data-testid={`metric-certified-${m.name}`}
                              >
                                <CheckCircle2 className="mr-0.5 h-3 w-3" />
                                {tx("已认证", "Certified")}
                              </Badge>
                            )}
                            {m.broken && (
                              <Badge
                                variant="warning"
                                data-testid={`metric-broken-${m.name}`}
                              >
                                <AlertTriangle className="mr-0.5 h-3 w-3" />
                                {tx("结构破损", "Broken")}
                              </Badge>
                            )}
                            <Badge variant="outline">v{m.version}</Badge>
                          </div>
                          <div className="mt-1 font-mono-tech text-[11px] text-muted-foreground truncate">
                            {m.expression}
                            {m.filter_sql ? ` WHERE ${m.filter_sql}` : ""} ·{" "}
                            {datasetNameMap[m.dataset_id] ?? "-"}
                          </div>
                        </button>
                      </div>

                      <div className="flex shrink-0 items-center gap-1">
                        <button
                          type="button"
                          disabled={!canEditMetrics}
                          onClick={() => openEditForm(m)}
                          data-testid={`metric-edit-${m.name}`}
                          className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-40 cursor-pointer"
                          title={tx("编辑", "Edit")}
                        >
                          <Edit3 className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          disabled={!canEditMetrics}
                          onClick={() => setPendingDeleteMetric(m)}
                          data-testid={`metric-delete-${m.name}`}
                          className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-40 cursor-pointer"
                          title={tx("删除", "Delete")}
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
            <span className="font-semibold">
              {tx("指标试算与同环比", "Metric Trial & Comparison")}
            </span>
            {multiSelectedNames.length >= 2 ? (
              <Badge variant="xblue">
                {tx(
                  `联合 ${fmtNum(multiSelectedNames.length, 0)} 项`,
                  `${fmtNum(multiSelectedNames.length, 0)} metrics`
                )}
              </Badge>
            ) : activeMetric ? (
              <Badge variant="neutral">{activeMetric.name}</Badge>
            ) : null}
          </div>

          {!activeMetric && multiSelectedNames.length === 0 ? (
            <div className="py-6 text-center text-muted-foreground">
              {tx("从左侧选择指标进行试算", "Select a metric on the left")}
            </div>
          ) : (
            <>
              {activeMetric && activeMetric.dimensions.length > 0 && (
                <div className="space-y-1">
                  <div className="text-muted-foreground">
                    {tx("分组维度", "Dimensions")}:
                  </div>
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
                  <label className="text-muted-foreground">
                    {tx("时间粒度", "Time Grain")}
                  </label>
                  <Select
                    value={queryGrain}
                    onChange={(e) =>
                      setQueryGrain(e.target.value as RollupGrain)
                    }
                    className="h-8 w-full"
                    data-testid="metric-query-grain"
                  >
                    {GRAIN_OPTIONS.map((g) => (
                      <option key={g} value={g}>
                        {grainLabel(g)}
                      </option>
                    ))}
                  </Select>
                </div>

                <div className="space-y-1">
                  <label className="text-muted-foreground">
                    {tx("对比模式", "Comparison")}
                  </label>
                  <Select
                    value={queryCompare}
                    onChange={(e) =>
                      setQueryCompare(
                        e.target.value as "" | MetricCompareMode
                      )
                    }
                    className="h-8 w-full"
                    data-testid="metric-query-compare"
                  >
                    <option value="">{tx("无对比", "None")}</option>
                    <option value="PREVIOUS_PERIOD">
                      {tx("环比", "Previous Period")}
                    </option>
                    <option value="SAME_PERIOD_LAST_YEAR">
                      {tx("同比", "Year-over-Year")}
                    </option>
                  </Select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="text-muted-foreground">
                    {tx("开始日期", "From Date")}
                  </label>
                  <Input
                    type="date"
                    value={queryFromDate}
                    onChange={(e) => setQueryFromDate(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-muted-foreground">
                    {tx("结束日期", "To Date")}
                  </label>
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
                {runningQuery
                  ? tx("计算中…", "Computing…")
                  : tx("运行试算", "Run Metric")}
              </Button>

              {queryError && (
                <div className="rounded border border-red-500/30 bg-red-500/5 p-2 font-mono-tech text-[11px] text-red-600 dark:text-red-400">
                  {queryError}
                </div>
              )}

              {lastMetricResult && (
                <div
                  className="rounded border border-border bg-muted/20 p-2 space-y-1"
                  data-testid="metric-query-summary"
                >
                  <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                    <span>
                      {fmtNum(lastMetricResult.rows.length, 0)}{" "}
                      {tx("行", "rows")}
                    </span>
                    <span className="font-mono-tech">
                      {fmtNum(lastMetricResult.duration_ms, 0)} ms
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

      {/* Metric-scoped Isolated ResultGrid + ChartPanel */}
      {lastMetricResult && (
        <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
          <div className="xl:col-span-7">
            <ResultGrid
              columns={lastMetricResult.columns}
              rows={lastMetricResult.rows}
              rowCount={lastMetricResult.rows.length}
              durationMs={lastMetricResult.duration_ms}
              sql={lastMetricResult.sql}
            />
          </div>
          <div className="xl:col-span-5">
            <ChartPanel
              columns={lastMetricResult.columns}
              rows={lastMetricResult.rows}
            />
          </div>
        </div>
      )}

      <ConfirmDialog
        open={Boolean(pendingDeleteMetric)}
        onClose={() => setPendingDeleteMetric(null)}
        onConfirm={() => void handleConfirmDeleteMetric()}
        loading={deletingMetric}
        title={tx("删除语义指标", "Delete Metric")}
        description={tx(
          `确定删除指标「${pendingDeleteMetric?.display_name ?? ""}」？`,
          `Delete metric "${pendingDeleteMetric?.display_name ?? ""}"?`
        )}
      />
    </div>
  );
}
