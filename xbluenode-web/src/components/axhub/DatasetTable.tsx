import { useCallback, useEffect, useMemo, useState } from "react";
import {
  createColumnHelper,
  tableFeatures,
  useTable,
} from "@tanstack/react-table";
import {
  Clock,
  Eye,
  Play,
  Plus,
  RefreshCw,
  Save,
  ShieldAlert,
  Trash2,
  Webhook,
} from "lucide-react";
import {
  axhubApi,
  type ColumnMeta,
  type CreateWebhookResponse,
  type Dataset,
  type LakeSnapshotInfo,
  type PreviewResponse,
  type QualityOnFail,
  type QualityRule,
  type WebhookAuthScheme,
  type WebhookConfig,
} from "@/lib/api/axhub";
import type {
  PiiCategory,
  QualityRuleKind,
  SourceKind,
} from "@/lib/api/enums";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

const SOURCE_KINDS: SourceKind[] = [
  "FILE_TABLE",
  "SQL_TABLE",
  "NOSQL_COLLECTION",
  "TIMESERIES",
  "STREAM",
  "API_RESOURCE",
  "WEBHOOK_STREAM",
  "DERIVED_VIEW",
];

const PII_CATEGORIES: PiiCategory[] = [
  "NONE",
  "PHONE",
  "ID_CARD",
  "EMAIL",
  "BANK_CARD",
  "NAME",
  "ADDRESS",
  "SECRET",
];

const QUALITY_RULE_KINDS: QualityRuleKind[] = [
  "NOT_NULL",
  "UNIQUE",
  "RANGE",
  "REGEX",
  "ALLOWED_VALUES",
  "FRESHNESS",
  "ROW_COUNT_MIN",
];

const datasetTableFeatures = tableFeatures({});
const columnHelper = createColumnHelper<typeof datasetTableFeatures, Dataset>();

export type DatasetDetailTab =
  | "preview"
  | "columns"
  | "quality"
  | "history"
  | "lineage"
  | "webhooks";

export interface DatasetTableProps {
  datasets: Dataset[];
  selectedDatasetId: string | null;
  activeDetailTab: DatasetDetailTab;
  onSelectDataset: (id: string) => void;
  onChangeDetailTab: (tab: DatasetDetailTab) => void;
  onFilterChange: (filters: {
    q?: string;
    tag?: string;
    source_kind?: SourceKind;
  }) => Promise<void>;
  onRefreshDatasets: () => Promise<void>;
  renderHistorySlot?: (dataset: Dataset) => React.ReactNode;
  renderLineageSlot?: (dataset: Dataset) => React.ReactNode;
}

export function DatasetTable({
  datasets,
  selectedDatasetId,
  activeDetailTab,
  onSelectDataset,
  onChangeDetailTab,
  onFilterChange,
  onRefreshDatasets,
  renderHistorySlot,
  renderLineageSlot,
}: DatasetTableProps) {
  const [filterQ, setFilterQ] = useState("");
  const [filterTag, setFilterTag] = useState("");
  const [filterSourceKind, setFilterSourceKind] = useState<string>("");

  const [syncingId, setSyncingId] = useState<string | null>(null);
  const [feedbackMsg, setFeedbackMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Preview & Snapshots state
  const [snapshots, setSnapshots] = useState<LakeSnapshotInfo[]>([]);
  const [selectedSnapshotId, setSelectedSnapshotId] = useState<string>("");
  const [previewData, setPreviewData] = useState<PreviewResponse | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);

  // Column Meta & PII state
  const [columnsMeta, setColumnsMeta] = useState<ColumnMeta[]>([]);
  const [colEdits, setColEdits] = useState<
    Record<
      string,
      {
        pii_category: PiiCategory;
        pii_confirmed: boolean;
        business_term: string;
        unit: string;
        description: string;
      }
    >
  >({});
  const [savingCol, setSavingCol] = useState<string | null>(null);

  // Quality rules state
  const [qualityRules, setQualityRules] = useState<QualityRule[]>([]);
  const [newRuleKind, setNewRuleKind] = useState<QualityRuleKind>("NOT_NULL");
  const [newRuleCol, setNewRuleCol] = useState("");
  const [newRuleParams, setNewRuleParams] = useState("{}");
  const [newRuleOnFail, setNewRuleOnFail] = useState<QualityOnFail>("WARN");

  // Webhooks state
  const [webhooks, setWebhooks] = useState<WebhookConfig[]>([]);
  const [whDatasetName, setWhDatasetName] = useState("");
  const [whSlug, setWhSlug] = useState("");
  const [whAuthScheme, setWhAuthScheme] =
    useState<WebhookAuthScheme>("HMAC_SHA256");
  const [whRecordPath, setWhRecordPath] = useState("$");
  const [createdWebhookSecret, setCreatedWebhookSecret] =
    useState<CreateWebhookResponse | null>(null);

  const selectedDataset = useMemo(
    () => datasets.find((d) => d.id === selectedDatasetId) ?? null,
    [datasets, selectedDatasetId]
  );

  const handleTriggerSync = useCallback(
    async (ds: Dataset) => {
      setSyncingId(ds.id);
      setErrorMsg(null);
      try {
        const res = await axhubApi.syncDataset(ds.id);
        setFeedbackMsg(
          res.existing
            ? `数据集「${ds.name}」已有进行中的同步任务 (${res.job_id})`
            : `已触发数据集「${ds.name}」同步任务 (${res.job_id})`
        );
        await onRefreshDatasets();
      } catch (err) {
        setErrorMsg(err instanceof Error ? err.message : String(err));
      } finally {
        setSyncingId(null);
      }
    },
    [onRefreshDatasets]
  );

  const columns = useMemo(
    () =>
      columnHelper.columns([
        columnHelper.accessor("name", {
          header: "数据集名称",
          cell: (info) => {
            const ds = info.row.original;
            return (
              <div>
                <div className="font-mono-tech font-semibold text-foreground">
                  {ds.name}
                </div>
                {ds.display_name && (
                  <div className="text-[11px] text-muted-foreground">
                    {ds.display_name}
                  </div>
                )}
              </div>
            );
          },
        }),
        columnHelper.accessor("source_kind", {
          header: "来源类型",
          cell: (info) => <Badge variant="outline">{info.getValue()}</Badge>,
        }),
        columnHelper.accessor("sync_mode", {
          header: "同步模式",
          cell: (info) => <Badge variant="neutral">{info.getValue()}</Badge>,
        }),
        columnHelper.accessor("row_count", {
          header: "行数 / 版本",
          cell: (info) => {
            const ds = info.row.original;
            return (
              <span className="font-mono-tech">
                {ds.row_count} 行 · v{ds.schema_version}
              </span>
            );
          },
        }),
        columnHelper.accessor("status", {
          header: "状态",
          cell: (info) => {
            const st = info.getValue();
            return (
              <Badge variant={st === "READY" ? "xblue" : "outline"}>{st}</Badge>
            );
          },
        }),
        columnHelper.display({
          id: "actions",
          header: "操作",
          cell: (info) => {
            const ds = info.row.original;
            return (
              <div className="flex items-center gap-1.5">
                <Button
                  type="button"
                  variant="outline"
                  className="px-2.5 py-1 text-[11px]"
                  onClick={(e) => {
                    e.stopPropagation();
                    onSelectDataset(ds.id);
                    onChangeDetailTab("preview");
                  }}
                  data-testid={`preview-dataset-btn-${ds.name}`}
                >
                  <Eye className="h-3 w-3" />
                  预览
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  className="px-2.5 py-1 text-[11px]"
                  disabled={syncingId === ds.id}
                  onClick={(e) => {
                    e.stopPropagation();
                    void handleTriggerSync(ds);
                  }}
                  data-testid={`sync-dataset-btn-${ds.name}`}
                >
                  {syncingId === ds.id ? (
                    <RefreshCw className="h-3 w-3 animate-spin" />
                  ) : (
                    <Play className="h-3 w-3" />
                  )}
                  同步
                </Button>
              </div>
            );
          },
        }),
      ]),
    [handleTriggerSync, onChangeDetailTab, onSelectDataset, syncingId]
  );

  const table = useTable({
    features: datasetTableFeatures,
    columns,
    data: datasets,
  });

  // Load preview & snapshots when selectedDataset or snapshot changes
  const loadPreviewAndSnapshots = useCallback(
    async (datasetId: string, snapIdStr: string) => {
      setPreviewLoading(true);
      setErrorMsg(null);
      try {
        const snapNum =
          snapIdStr !== "" && !Number.isNaN(Number(snapIdStr))
            ? Number(snapIdStr)
            : undefined;
        const [snapsRes, prevRes] = await Promise.all([
          axhubApi.listSnapshots(datasetId).catch(() => []),
          axhubApi.previewDataset(datasetId, 50, snapNum),
        ]);
        setSnapshots(Array.isArray(snapsRes) ? snapsRes : []);
        setPreviewData({
          columns: Array.isArray(prevRes?.columns) ? prevRes.columns : [],
          rows: Array.isArray(prevRes?.rows) ? prevRes.rows : [],
        });
      } catch (err) {
        setErrorMsg(err instanceof Error ? err.message : String(err));
      } finally {
        setPreviewLoading(false);
      }
    },
    []
  );

  const loadColumnsMeta = useCallback(async (datasetId: string) => {
    try {
      const cols = await axhubApi.listColumns(datasetId);
      const safeCols = Array.isArray(cols) ? cols : [];
      setColumnsMeta(safeCols);
      const nextEdits: Record<
        string,
        {
          pii_category: PiiCategory;
          pii_confirmed: boolean;
          business_term: string;
          unit: string;
          description: string;
        }
      > = {};
      for (const c of safeCols) {
        nextEdits[c.column_name] = {
          pii_category: c.pii_category,
          pii_confirmed: c.pii_confirmed,
          business_term: c.business_term,
          unit: c.unit,
          description: c.description,
        };
      }
      setColEdits(nextEdits);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const loadQualityRules = useCallback(async (datasetId: string) => {
    try {
      const rules = await axhubApi.listQualityRules(datasetId);
      setQualityRules(Array.isArray(rules) ? rules : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const loadWebhooks = useCallback(async () => {
    try {
      const res = await axhubApi.listWebhooks();
      setWebhooks(Array.isArray(res?.items) ? res.items : []);
    } catch {
      // Non-blocking if user lacks DATA_ADMIN
    }
  }, []);

  useEffect(() => {
    setSelectedSnapshotId("");
  }, [selectedDatasetId]);

  useEffect(() => {
    if (!selectedDataset) return;
    if (activeDetailTab === "preview") {
      void loadPreviewAndSnapshots(selectedDataset.id, selectedSnapshotId);
    } else if (activeDetailTab === "columns") {
      void loadColumnsMeta(selectedDataset.id);
    } else if (activeDetailTab === "quality") {
      void loadQualityRules(selectedDataset.id);
    } else if (activeDetailTab === "webhooks") {
      void loadWebhooks();
    }
  }, [
    selectedDataset,
    activeDetailTab,
    selectedSnapshotId,
    loadPreviewAndSnapshots,
    loadColumnsMeta,
    loadQualityRules,
    loadWebhooks,
  ]);

  const applyFilters = (e: React.FormEvent) => {
    e.preventDefault();
    void onFilterChange({
      q: filterQ.trim() || undefined,
      tag: filterTag.trim() || undefined,
      source_kind: filterSourceKind
        ? (filterSourceKind as SourceKind)
        : undefined,
    });
  };

  const handleSaveColumnMeta = async (colName: string) => {
    if (!selectedDataset) return;
    const draft = colEdits[colName];
    if (!draft) return;
    setSavingCol(colName);
    setErrorMsg(null);
    try {
      await axhubApi.updateColumnMeta(selectedDataset.id, colName, draft);
      setFeedbackMsg(`已更新列「${colName}」元数据与 PII 策略`);
      await loadColumnsMeta(selectedDataset.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingCol(null);
    }
  };

  const handleCreateQualityRule = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedDataset) return;
    setErrorMsg(null);
    try {
      const parsedParams: unknown = JSON.parse(newRuleParams || "{}");
      const paramsObj =
        typeof parsedParams === "object" &&
        parsedParams !== null &&
        !Array.isArray(parsedParams)
          ? (parsedParams as Record<string, unknown>)
          : {};
      await axhubApi.createQualityRule(selectedDataset.id, {
        rule_kind: newRuleKind,
        column_name: newRuleCol.trim() || null,
        params: paramsObj,
        on_fail: newRuleOnFail,
      });
      setNewRuleCol("");
      setNewRuleParams("{}");
      await loadQualityRules(selectedDataset.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDeleteQualityRule = async (ruleId: string) => {
    if (!selectedDataset) return;
    setErrorMsg(null);
    try {
      await axhubApi.deleteQualityRule(ruleId);
      await loadQualityRules(selectedDataset.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCreateWebhook = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!whDatasetName.trim() || !whSlug.trim()) return;
    setErrorMsg(null);
    try {
      const created = await axhubApi.createWebhook({
        dataset_name: whDatasetName.trim(),
        slug: whSlug.trim(),
        auth_scheme: whAuthScheme,
        record_path: whRecordPath.trim() || "$",
      });
      setCreatedWebhookSecret(created);
      setWhDatasetName("");
      setWhSlug("");
      await loadWebhooks();
      await onRefreshDatasets();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDeleteWebhook = async (id: string) => {
    setErrorMsg(null);
    try {
      await axhubApi.deleteWebhook(id);
      await loadWebhooks();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="space-y-4" data-testid="datasets-section">
      <Card className="p-4 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
          <div>
            <h2 className="text-sm font-semibold">
              DuckLake 数据集资产列表 ({datasets.length})
            </h2>
            <p className="text-xs text-muted-foreground">
              统一管理快照版本、增量游标、列级 PII 标注、数据质量守卫与实时 Webhook 流式入湖
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                if (!selectedDataset && datasets[0]) {
                  onSelectDataset(datasets[0].id);
                }
                onChangeDetailTab("webhooks");
                void loadWebhooks();
              }}
              data-testid="open-webhooks-btn"
            >
              <Webhook className="h-3.5 w-3.5" />
              Webhook 接入管理
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => void onRefreshDatasets()}
              data-testid="refresh-datasets-btn"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              刷新列表
            </Button>
          </div>
        </div>

        {errorMsg && (
          <div
            className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs flex items-center justify-between"
            data-testid="dataset-error-banner"
          >
            <span>{errorMsg}</span>
            <button
              type="button"
              className="text-muted-foreground hover:text-foreground cursor-pointer"
              onClick={() => setErrorMsg(null)}
            >
              关闭
            </button>
          </div>
        )}

        {feedbackMsg && (
          <div
            className="rounded border border-xblue/30 bg-xblue-soft px-3 py-2 text-xs text-xblue flex items-center justify-between"
            data-testid="dataset-feedback-banner"
          >
            <span>{feedbackMsg}</span>
            <button
              type="button"
              className="text-xblue cursor-pointer"
              onClick={() => setFeedbackMsg(null)}
            >
              关闭
            </button>
          </div>
        )}

        {/* Filter Bar */}
        <form
          onSubmit={applyFilters}
          className="grid grid-cols-1 md:grid-cols-4 gap-2"
        >
          <Input
            value={filterQ}
            onChange={(e) => setFilterQ(e.target.value)}
            placeholder="按名称或关键词筛选..."
            data-testid="dataset-filter-q"
          />
          <select
            value={filterSourceKind}
            onChange={(e) => setFilterSourceKind(e.target.value)}
            className="h-9 rounded-md border border-input bg-background px-3 text-xs"
            data-testid="dataset-filter-source-kind"
          >
            <option value="">全部来源类型</option>
            {SOURCE_KINDS.map((sk) => (
              <option key={sk} value={sk}>
                {sk}
              </option>
            ))}
          </select>
          <Input
            value={filterTag}
            onChange={(e) => setFilterTag(e.target.value)}
            placeholder="按标签过滤 (tag)..."
            data-testid="dataset-filter-tag"
          />
          <Button type="submit" variant="outline" data-testid="dataset-filter-btn">
            筛选数据集
          </Button>
        </form>

        {/* TanStack Table v9 */}
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full text-left text-xs" data-testid="dataset-table">
            <thead className="bg-muted border-b border-border">
              {table.getHeaderGroups().map((headerGroup) => (
                <tr key={headerGroup.id}>
                  {headerGroup.headers.map((header) => (
                    <th
                      key={header.id}
                      className="px-3 py-2 font-medium text-muted-foreground"
                    >
                      {header.isPlaceholder ? null : (
                        <table.FlexRender header={header} />
                      )}
                    </th>
                  ))}
                </tr>
              ))}
            </thead>
            <tbody>
              {table.getRowModel().rows.length === 0 ? (
                <tr>
                  <td
                    colSpan={6}
                    className="px-3 py-6 text-center text-muted-foreground"
                  >
                    暂无数据集，请从上方数据源发现创建或上传文件入湖。
                  </td>
                </tr>
              ) : (
                table.getRowModel().rows.map((row) => {
                  const ds = row.original;
                  const isSelected = ds.id === selectedDatasetId;
                  return (
                    <tr
                      key={row.id}
                      onClick={() => onSelectDataset(ds.id)}
                      data-testid={`dataset-row-${ds.name}`}
                      className={`border-b border-border last:border-0 cursor-pointer transition-colors ${
                        isSelected
                          ? "bg-xblue-soft/60"
                          : "hover:bg-muted/40"
                      }`}
                    >
                      {row.getAllCells().map((cell) => (
                        <td key={cell.id} className="px-3 py-2.5">
                          <table.FlexRender cell={cell} />
                        </td>
                      ))}
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </Card>

      {/* Selected Dataset Governance & Inspector Workspace */}
      {selectedDataset && (
        <div className="space-y-3" data-testid="dataset-inspector">
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-card px-3 py-2">
            <div className="flex items-center gap-2 text-xs">
              <span className="text-muted-foreground">当前选中数据集:</span>
              <span className="font-mono-tech font-semibold text-xblue">
                {selectedDataset.name}
              </span>
              <Badge variant="outline">{selectedDataset.schema_policy}</Badge>
              {selectedDataset.last_snapshot_id !== null && (
                <span className="font-mono-tech text-[11px] text-muted-foreground">
                  最新快照 #{selectedDataset.last_snapshot_id}
                </span>
              )}
            </div>

            <div className="flex flex-wrap items-center gap-1">
              {(
                [
                  { key: "preview", label: "数据预览与时间旅行" },
                  { key: "columns", label: "列元数据与 PII" },
                  { key: "quality", label: "数据质量规则" },
                  { key: "history", label: "同步历史" },
                  { key: "lineage", label: "血缘拓扑" },
                  { key: "webhooks", label: "Webhook 管理" },
                ] as const
              ).map((tab) => (
                <button
                  key={tab.key}
                  type="button"
                  onClick={() => onChangeDetailTab(tab.key)}
                  data-testid={`dataset-tab-${tab.key}`}
                  className={`rounded px-2.5 py-1 text-xs font-medium transition-colors cursor-pointer ${
                    activeDetailTab === tab.key
                      ? "bg-xblue text-white"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted"
                  }`}
                >
                  {tab.label}
                </button>
              ))}
            </div>
          </div>

          {/* Tab 1: Data Preview + Snapshot Time-Travel */}
          {activeDetailTab === "preview" && (
            <Card className="p-4 space-y-3" data-testid="dataset-preview-panel">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="text-xs font-semibold">
                  湖表数据抽样预览 · {selectedDataset.lake_table}
                </div>
                <div className="flex items-center gap-2 text-xs">
                  <Clock className="h-3.5 w-3.5 text-xblue" />
                  <span className="text-muted-foreground">
                    DuckLake 快照时间旅行:
                  </span>
                  <select
                    value={selectedSnapshotId}
                    onChange={(e) => setSelectedSnapshotId(e.target.value)}
                    className="h-8 rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                    data-testid="snapshot-selector"
                  >
                    <option value="">最新当前版本 (HEAD)</option>
                    {snapshots.map((snap) => (
                      <option
                        key={snap.snapshot_id}
                        value={String(snap.snapshot_id)}
                      >
                        Snapshot #{snap.snapshot_id} (+{snap.rows_added}/-
                        {snap.rows_deleted}) · {snap.committed_at.slice(0, 19)}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              {previewLoading ? (
                <div className="py-6 text-center text-xs text-muted-foreground">
                  正在从 DuckLake 读取数据预览...
                </div>
              ) : !previewData || previewData.columns.length === 0 ? (
                <div className="py-6 text-center text-xs text-muted-foreground">
                  暂无可预览列数据（请先执行一次同步）。
                </div>
              ) : (
                <div className="overflow-x-auto rounded border border-border">
                  <table
                    className="w-full text-left text-xs"
                    data-testid="dataset-preview-table"
                  >
                    <thead className="bg-muted border-b border-border">
                      <tr>
                        {previewData.columns.map((col) => (
                          <th
                            key={col.name}
                            className="px-3 py-2 font-mono-tech font-medium whitespace-nowrap"
                          >
                            <div className="flex items-center gap-1.5">
                              <span>{col.name}</span>
                              <span className="text-[10px] text-muted-foreground">
                                {col.data_type}
                              </span>
                              {col.pii_category !== "NONE" && (
                                <Badge variant="xblue">{col.pii_category}</Badge>
                              )}
                            </div>
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {previewData.rows.map((row, idx) => (
                        <tr
                          key={idx}
                          className="border-b border-border last:border-0"
                        >
                          {previewData.columns.map((col) => (
                            <td
                              key={col.name}
                              className="px-3 py-1.5 font-mono-tech whitespace-nowrap"
                            >
                              {row[col.name] === null ||
                              row[col.name] === undefined
                                ? "NULL"
                                : typeof row[col.name] === "object"
                                  ? JSON.stringify(row[col.name])
                                  : String(row[col.name])}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </Card>
          )}

          {/* Tab 2: Column Metadata & PII Human Confirmation */}
          {activeDetailTab === "columns" && (
            <Card className="p-4 space-y-3" data-testid="dataset-columns-panel">
              <div className="flex items-center gap-2 border-b border-border pb-2">
                <ShieldAlert className="h-4 w-4 text-xblue" />
                <div>
                  <h3 className="text-xs font-semibold">
                    列级元数据与 PII 敏感类别人工确认
                  </h3>
                  <p className="text-[11px] text-muted-foreground">
                    勾选「人工确认」后，后续自动同步列画像不再覆盖该列的 PII 分类设置
                  </p>
                </div>
              </div>

              {columnsMeta.length === 0 ? (
                <div className="py-4 text-xs text-muted-foreground">
                  暂无列元数据。
                </div>
              ) : (
                <div className="space-y-2">
                  {columnsMeta.map((col) => {
                    const draft = colEdits[col.column_name] ?? {
                      pii_category: col.pii_category,
                      pii_confirmed: col.pii_confirmed,
                      business_term: col.business_term,
                      unit: col.unit,
                      description: col.description,
                    };
                    return (
                      <div
                        key={col.column_name}
                        className="grid grid-cols-1 md:grid-cols-12 gap-2 items-center rounded border border-border p-2.5 text-xs"
                        data-testid={`column-meta-row-${col.column_name}`}
                      >
                        <div className="md:col-span-2 font-mono-tech">
                          <div className="font-semibold">{col.column_name}</div>
                          <div className="text-[11px] text-muted-foreground">
                            {col.data_type}
                          </div>
                        </div>

                        <div className="md:col-span-2">
                          <select
                            value={draft.pii_category}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  pii_category: e.target.value as PiiCategory,
                                },
                              }))
                            }
                            className="h-8 w-full rounded border border-input bg-background px-2 text-xs font-mono-tech"
                            data-testid={`col-pii-select-${col.column_name}`}
                          >
                            {PII_CATEGORIES.map((cat) => (
                              <option key={cat} value={cat}>
                                PII: {cat}
                              </option>
                            ))}
                          </select>
                        </div>

                        <label className="md:col-span-2 inline-flex items-center gap-1.5 cursor-pointer">
                          <input
                            type="checkbox"
                            checked={draft.pii_confirmed}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  pii_confirmed: e.target.checked,
                                },
                              }))
                            }
                            data-testid={`col-pii-confirmed-${col.column_name}`}
                          />
                          人工确认
                        </label>

                        <div className="md:col-span-2">
                          <Input
                            className="h-8 text-xs"
                            value={draft.business_term}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  business_term: e.target.value,
                                },
                              }))
                            }
                            placeholder="业务术语"
                          />
                        </div>

                        <div className="md:col-span-3">
                          <Input
                            className="h-8 text-xs"
                            value={draft.description}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  description: e.target.value,
                                },
                              }))
                            }
                            placeholder="列业务说明 / 单位"
                          />
                        </div>

                        <div className="md:col-span-1 flex justify-end">
                          <Button
                            type="button"
                            variant="outline"
                            className="px-2 py-1 text-[11px]"
                            disabled={savingCol === col.column_name}
                            onClick={() =>
                              void handleSaveColumnMeta(col.column_name)
                            }
                            data-testid={`save-col-meta-${col.column_name}`}
                          >
                            <Save className="h-3 w-3" />
                          </Button>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </Card>
          )}

          {/* Tab 3: Quality Rules */}
          {activeDetailTab === "quality" && (
            <Card className="p-4 space-y-3" data-testid="dataset-quality-panel">
              <div className="border-b border-border pb-2">
                <h3 className="text-xs font-semibold">
                  数据质量规则配置（入湖事务前校验）
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  BLOCK 策略在规则不满足时自动回滚 DuckLake 快照事务；WARN 仅记录至同步校验报告
                </p>
              </div>

              <form
                onSubmit={(e) => void handleCreateQualityRule(e)}
                className="grid grid-cols-1 md:grid-cols-5 gap-2"
              >
                <select
                  value={newRuleKind}
                  onChange={(e) =>
                    setNewRuleKind(e.target.value as QualityRuleKind)
                  }
                  className="h-9 rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  data-testid="quality-rule-kind-select"
                >
                  {QUALITY_RULE_KINDS.map((rk) => (
                    <option key={rk} value={rk}>
                      {rk}
                    </option>
                  ))}
                </select>
                <Input
                  value={newRuleCol}
                  onChange={(e) => setNewRuleCol(e.target.value)}
                  placeholder="目标列名（表级规则留空）"
                  data-testid="quality-rule-column-input"
                />
                <Input
                  value={newRuleParams}
                  onChange={(e) => setNewRuleParams(e.target.value)}
                  placeholder='参数 JSON，如 {"min":0}'
                  className="font-mono-tech text-xs"
                  data-testid="quality-rule-params-input"
                />
                <select
                  value={newRuleOnFail}
                  onChange={(e) =>
                    setNewRuleOnFail(e.target.value as QualityOnFail)
                  }
                  className="h-9 rounded-md border border-input bg-background px-2 text-xs"
                  data-testid="quality-rule-onfail-select"
                >
                  <option value="WARN">失败动作: WARN</option>
                  <option value="BLOCK">失败动作: BLOCK</option>
                </select>
                <Button
                  type="submit"
                  variant="outline"
                  data-testid="quality-rule-add-btn"
                >
                  <Plus className="h-3.5 w-3.5" />
                  新增规则
                </Button>
              </form>

              {qualityRules.length === 0 ? (
                <div className="text-xs text-muted-foreground py-2">
                  当前数据集暂未配置质量校验规则。
                </div>
              ) : (
                <div className="space-y-1.5">
                  {qualityRules.map((rule) => (
                    <div
                      key={rule.id}
                      className="rounded border border-border px-3 py-2 text-xs flex items-center justify-between"
                    >
                      <div className="flex items-center gap-2 font-mono-tech">
                        <Badge
                          variant={rule.on_fail === "BLOCK" ? "xblue" : "neutral"}
                        >
                          {rule.on_fail}
                        </Badge>
                        <span className="font-semibold">{rule.rule_kind}</span>
                        <span>列: {rule.column_name ?? "*"}</span>
                        <span className="text-muted-foreground">
                          {JSON.stringify(rule.params)}
                        </span>
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        className="px-2 py-1 text-xs"
                        onClick={() => void handleDeleteQualityRule(rule.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </Card>
          )}

          {/* Tab 4: Sync History Slot */}
          {activeDetailTab === "history" && renderHistorySlot?.(selectedDataset)}

          {/* Tab 5: Lineage Topology Slot */}
          {activeDetailTab === "lineage" && renderLineageSlot?.(selectedDataset)}

          {/* Tab 6: Webhook Management */}
          {activeDetailTab === "webhooks" && (
            <Card className="p-4 space-y-3" data-testid="webhooks-panel">
              <div className="border-b border-border pb-2">
                <h3 className="text-xs font-semibold">
                  Webhook 实时事件接入端点管理
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  创建 HMAC-SHA256 或 Bearer 签名校验的推送入口，微批幂等刷入 DuckLake 流式表
                </p>
              </div>

              <form
                onSubmit={(e) => void handleCreateWebhook(e)}
                className="grid grid-cols-1 md:grid-cols-5 gap-2"
              >
                <Input
                  value={whDatasetName}
                  onChange={(e) => setWhDatasetName(e.target.value)}
                  placeholder="目标数据集名 (如 iot_events)"
                  data-testid="webhook-dataset-input"
                />
                <Input
                  value={whSlug}
                  onChange={(e) => setWhSlug(e.target.value)}
                  placeholder="路径标识 slug (如 demo-events)"
                  data-testid="webhook-slug-input"
                />
                <select
                  value={whAuthScheme}
                  onChange={(e) =>
                    setWhAuthScheme(e.target.value as WebhookAuthScheme)
                  }
                  className="h-9 rounded-md border border-input bg-background px-2 text-xs"
                >
                  <option value="HMAC_SHA256">HMAC_SHA256</option>
                  <option value="BEARER">BEARER</option>
                </select>
                <Input
                  value={whRecordPath}
                  onChange={(e) => setWhRecordPath(e.target.value)}
                  placeholder="JSONPath 记录路径 ($)"
                  className="font-mono-tech text-xs"
                />
                <Button
                  type="submit"
                  variant="outline"
                  data-testid="webhook-create-btn"
                >
                  <Plus className="h-3.5 w-3.5" />
                  创建端点
                </Button>
              </form>

              {createdWebhookSecret && (
                <div className="rounded border border-xblue/40 bg-xblue-soft p-3 text-xs space-y-1 font-mono-tech">
                  <div className="font-semibold text-xblue">
                    Webhook 已创建（密钥仅展示一次，请立即妥善保存）
                  </div>
                  <div>URL: {createdWebhookSecret.url}</div>
                  <div>Secret: {createdWebhookSecret.secret}</div>
                </div>
              )}

              {webhooks.length === 0 ? (
                <div className="text-xs text-muted-foreground py-2">
                  暂无已配置的 Webhook 接收端点。
                </div>
              ) : (
                <div className="space-y-1.5">
                  {webhooks.map((wh) => (
                    <div
                      key={wh.id}
                      className="rounded border border-border px-3 py-2 text-xs flex items-center justify-between font-mono-tech"
                    >
                      <div className="flex items-center gap-2">
                        <Badge variant="xblue">{wh.auth_scheme}</Badge>
                        <span className="font-semibold">{wh.slug}</span>
                        <span className="text-muted-foreground">
                          path={wh.record_path}
                        </span>
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        className="px-2 py-1 text-xs"
                        onClick={() => void handleDeleteWebhook(wh.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </Card>
          )}
        </div>
      )}
    </div>
  );
}
