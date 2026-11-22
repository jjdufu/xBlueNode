import React, { useCallback, useEffect, useMemo, useState } from "react";
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
import { useI18n } from "@/i18n";
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
  DataType,
  PiiCategory,
  QualityRuleKind,
  SourceKind,
} from "@/lib/api/enums";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Input,
  Modal,
  Select,
  StatusBadge,
  Tabs,
} from "@/components/ui/primitives";

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

const ISO_TIMESTAMP_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/;

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
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canSync = can("DATA_SYNC");
  const canAdmin = can("DATA_ADMIN");

  const [filterQ, setFilterQ] = useState("");
  const [filterTag, setFilterTag] = useState("");
  const [filterSourceKind, setFilterSourceKind] = useState<string>("");

  const [syncingId, setSyncingId] = useState<string | null>(null);
  const [pendingDeleteDs, setPendingDeleteDs] = useState<Dataset | null>(null);
  const [deletingDs, setDeletingDs] = useState(false);
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
  const [savingAllCols, setSavingAllCols] = useState(false);

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

  const formatSourceKind = useCallback(
    (kind: SourceKind): string => {
      switch (kind) {
        case "NOSQL_COLLECTION":
          return tx("文档集合", "NoSQL Collection");
        case "TIMESERIES":
          return tx("时序表", "Time-Series");
        case "STREAM":
          return tx("流数据", "Stream");
        case "API_RESOURCE":
          return tx("接口资源", "API Resource");
        default:
          return tEnum(kind);
      }
    },
    [tEnum, tx]
  );

  const formatRuleKind = useCallback(
    (kind: QualityRuleKind): string => {
      switch (kind) {
        case "NOT_NULL":
          return tx("非空校验", "Not Null");
        case "UNIQUE":
          return tx("唯一性校验", "Unique");
        case "RANGE":
          return tx("数值区间", "Range");
        case "REGEX":
          return tx("正则匹配", "Regex");
        case "ALLOWED_VALUES":
          return tx("枚举集合", "Allowed Values");
        case "FRESHNESS":
          return tx("时效性", "Freshness");
        case "ROW_COUNT_MIN":
          return tx("最小行数", "Min Rows");
        default:
          return kind;
      }
    },
    [tx]
  );

  const formatPreviewCell = useCallback(
    (val: unknown, colName: string, dataType: DataType): string => {
      if (val === null || val === undefined) return "NULL";
      if (typeof val === "number") {
        const isId =
          colName === "id" ||
          colName.endsWith("_id") ||
          colName.endsWith("_no");
        return isId ? String(val) : fmtNum(val, 4);
      }
      if (typeof val === "string") {
        if (
          (dataType === "TIMESTAMP" || dataType === "DATE") &&
          ISO_TIMESTAMP_RE.test(val)
        ) {
          return fmtTime(val);
        }
        return val;
      }
      if (typeof val === "object") {
        return JSON.stringify(val);
      }
      return String(val);
    },
    [fmtNum, fmtTime]
  );

  const selectedDataset = useMemo(
    () => datasets.find((d) => d.id === selectedDatasetId) ?? null,
    [datasets, selectedDatasetId]
  );

  const handleTriggerSync = useCallback(
    async (ds: Dataset) => {
      if (!canSync) return;
      setSyncingId(ds.id);
      setErrorMsg(null);
      try {
        const res = await axhubApi.syncDataset(ds.id);
        setFeedbackMsg(
          res.existing
            ? tx(
                `数据集「${ds.name}」正在同步 (${res.job_id.slice(0, 8)})`,
                `Sync already running for "${ds.name}" (${res.job_id.slice(0, 8)})`
              )
            : tx(
                `已触发「${ds.name}」同步 (${res.job_id.slice(0, 8)})`,
                `Triggered sync for "${ds.name}" (${res.job_id.slice(0, 8)})`
              )
        );
        await onRefreshDatasets();
      } catch (err) {
        setErrorMsg(err instanceof Error ? err.message : String(err));
      } finally {
        setSyncingId(null);
      }
    },
    [canSync, onRefreshDatasets, tx]
  );

  const handleConfirmDeleteDataset = async () => {
    if (!pendingDeleteDs || !canAdmin) return;
    setDeletingDs(true);
    setErrorMsg(null);
    try {
      const target = pendingDeleteDs;
      await axhubApi.deleteDataset(target.id);
      setFeedbackMsg(
        tx(`已删除数据集「${target.name}」`, `Deleted dataset "${target.name}"`)
      );
      setPendingDeleteDs(null);
      await onRefreshDatasets();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingDs(false);
    }
  };

  const columns = useMemo(
    () =>
      columnHelper.columns([
        columnHelper.accessor("name", {
          header: tx("数据集", "Dataset"),
          cell: (info) => {
            const ds = info.row.original;
            return (
              <div>
                <div className="font-mono-tech font-semibold text-foreground">
                  {ds.name}
                </div>
                {ds.display_name && ds.display_name !== ds.name && (
                  <div className="text-[11px] text-muted-foreground">
                    {ds.display_name}
                  </div>
                )}
              </div>
            );
          },
        }),
        columnHelper.accessor("source_kind", {
          header: tx("来源类型", "Source"),
          cell: (info) => (
            <Badge variant="outline">
              {formatSourceKind(info.getValue())}
            </Badge>
          ),
        }),
        columnHelper.accessor("sync_mode", {
          header: tx("同步模式", "Sync Mode"),
          cell: (info) => (
            <Badge variant="neutral">{tEnum(info.getValue())}</Badge>
          ),
        }),
        columnHelper.accessor("row_count", {
          header: tx("行数 / 版本", "Rows / Version"),
          cell: (info) => {
            const ds = info.row.original;
            return (
              <span className="font-mono-tech">
                {fmtNum(ds.row_count, 0)} {tx("行", "rows")} · v
                {ds.schema_version}
              </span>
            );
          },
        }),
        columnHelper.accessor("status", {
          header: tx("状态", "Status"),
          cell: (info) => {
            const st = info.getValue();
            return (
              <StatusBadge status={st === "SYNCING" ? "RUNNING" : st} />
            );
          },
        }),
        columnHelper.display({
          id: "actions",
          header: tx("操作", "Actions"),
          cell: (info) => {
            const ds = info.row.original;
            return (
              <div className="flex items-center gap-1.5">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    onSelectDataset(ds.id);
                    onChangeDetailTab("preview");
                  }}
                  data-testid={`preview-dataset-btn-${ds.name}`}
                >
                  <Eye className="h-3 w-3" />
                  {tx("预览", "Preview")}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!canSync || syncingId === ds.id}
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
                  {tx("同步", "Sync")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={!canAdmin}
                  onClick={(e) => {
                    e.stopPropagation();
                    setPendingDeleteDs(ds);
                  }}
                  aria-label={tx("删除", "Delete")}
                  data-testid={`delete-dataset-btn-${ds.name}`}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              </div>
            );
          },
        }),
      ]),
    [
      canAdmin,
      canSync,
      fmtNum,
      formatSourceKind,
      handleTriggerSync,
      onChangeDetailTab,
      onSelectDataset,
      syncingId,
      tEnum,
      tx,
    ]
  );

  const table = useTable({
    features: datasetTableFeatures,
    columns,
    data: datasets,
  });

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

  const isColumnDirty = useCallback(
    (col: ColumnMeta): boolean => {
      const draft = colEdits[col.column_name];
      if (!draft) return false;
      return (
        draft.pii_category !== col.pii_category ||
        draft.pii_confirmed !== col.pii_confirmed ||
        draft.business_term !== col.business_term ||
        draft.unit !== col.unit ||
        draft.description !== col.description
      );
    },
    [colEdits]
  );

  const dirtyColumns = useMemo(
    () => columnsMeta.filter(isColumnDirty),
    [columnsMeta, isColumnDirty]
  );

  const handleSaveColumnMeta = async (colName: string) => {
    if (!selectedDataset || !canSync) return;
    const draft = colEdits[colName];
    if (!draft) return;
    setSavingCol(colName);
    setErrorMsg(null);
    try {
      await axhubApi.updateColumnMeta(selectedDataset.id, colName, draft);
      setFeedbackMsg(
        tx(`已保存列「${colName}」配置`, `Saved column "${colName}"`)
      );
      await loadColumnsMeta(selectedDataset.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingCol(null);
    }
  };

  const handleSaveAllColumnsMeta = async () => {
    if (!selectedDataset || !canSync || dirtyColumns.length === 0) return;
    setSavingAllCols(true);
    setErrorMsg(null);
    try {
      for (const col of dirtyColumns) {
        const draft = colEdits[col.column_name];
        if (draft) {
          await axhubApi.updateColumnMeta(
            selectedDataset.id,
            col.column_name,
            draft
          );
        }
      }
      setFeedbackMsg(
        tx(
          `已批量保存 ${dirtyColumns.length} 列元数据`,
          `Saved ${dirtyColumns.length} modified columns`
        )
      );
      await loadColumnsMeta(selectedDataset.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingAllCols(false);
    }
  };

  const handleCreateQualityRule = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedDataset || !canSync) return;
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
    if (!selectedDataset || !canSync) return;
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
    if (!canAdmin || !whDatasetName.trim() || !whSlug.trim()) return;
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
    if (!canAdmin) return;
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
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold">
              {tx("数据集资产", "Datasets")}
            </h2>
            <Badge variant="neutral">{fmtNum(datasets.length, 0)}</Badge>
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
              {tx("实时推送", "Webhooks")}
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => void onRefreshDatasets()}
              data-testid="refresh-datasets-btn"
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {tx("刷新", "Refresh")}
            </Button>
          </div>
        </div>

        {errorMsg && (
          <div
            className="rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs flex items-center justify-between"
            data-testid="dataset-error-banner"
          >
            <span className="text-red-600 dark:text-red-400">{errorMsg}</span>
            <button
              type="button"
              className="text-muted-foreground hover:text-foreground cursor-pointer"
              onClick={() => setErrorMsg(null)}
            >
              {tx("关闭", "Dismiss")}
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
              {tx("关闭", "Dismiss")}
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
            placeholder="demo_orders"
            data-testid="dataset-filter-q"
          />
          <Select
            value={filterSourceKind}
            onChange={(e) => setFilterSourceKind(e.target.value)}
            data-testid="dataset-filter-source-kind"
          >
            <option value="">{tx("全部来源", "All Sources")}</option>
            {SOURCE_KINDS.map((sk) => (
              <option key={sk} value={sk}>
                {formatSourceKind(sk)}
              </option>
            ))}
          </Select>
          <Input
            value={filterTag}
            onChange={(e) => setFilterTag(e.target.value)}
            placeholder="supply_chain"
            data-testid="dataset-filter-tag"
          />
          <Button
            type="submit"
            variant="outline"
            data-testid="dataset-filter-btn"
          >
            {tx("筛选", "Filter")}
          </Button>
        </form>

        {/* Dataset Table */}
        <div className="overflow-x-auto rounded border border-border">
          <table
            className="w-full text-left text-xs"
            data-testid="dataset-table"
          >
            <thead className="bg-muted border-b border-border">
              {table.getHeaderGroups().map((headerGroup) => (
                <tr key={headerGroup.id}>
                  {headerGroup.headers.map((header) => (
                    <th
                      key={header.id}
                      className="px-3 py-2 font-medium text-muted-foreground whitespace-nowrap"
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
                    className="px-3 py-8 text-center text-muted-foreground"
                  >
                    {tx("暂无数据集", "No datasets registered")}
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
                          ? "bg-xblue-soft/40"
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
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-card px-3 pt-2">
            <div className="flex flex-wrap items-center gap-2 text-xs pb-2">
              <span className="text-muted-foreground">
                {tx("当前数据集", "Selected")}:
              </span>
              <span className="font-mono-tech font-semibold text-xblue">
                {selectedDataset.name}
              </span>
              <Badge variant="outline">
                {tEnum(selectedDataset.schema_policy)}
              </Badge>
              {selectedDataset.last_snapshot_id !== null && (
                <span className="font-mono-tech text-[11px] text-muted-foreground">
                  #{fmtNum(selectedDataset.last_snapshot_id, 0)}
                </span>
              )}
            </div>

            <Tabs
              className="border-b-0"
              value={activeDetailTab}
              onChange={(next) => onChangeDetailTab(next)}
              items={[
                {
                  key: "preview",
                  label: tx("数据预览", "Preview"),
                  testId: "dataset-tab-preview",
                },
                {
                  key: "columns",
                  label: tx("列与敏感项", "Columns & PII"),
                  count:
                    dirtyColumns.length > 0 ? dirtyColumns.length : undefined,
                  testId: "dataset-tab-columns",
                },
                {
                  key: "quality",
                  label: tx("质量规则", "Quality Rules"),
                  testId: "dataset-tab-quality",
                },
                {
                  key: "history",
                  label: tx("同步历史", "Sync History"),
                  testId: "dataset-tab-history",
                },
                {
                  key: "lineage",
                  label: tx("血缘拓扑", "Lineage"),
                  testId: "dataset-tab-lineage",
                },
                {
                  key: "webhooks",
                  label: tx("实时推送", "Webhooks"),
                  testId: "dataset-tab-webhooks",
                },
              ]}
            />
          </div>

          {/* Tab 1: Bounded Data Preview + Snapshot Time-Travel */}
          {activeDetailTab === "preview" && (
            <Card className="p-4 space-y-3" data-testid="dataset-preview-panel">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex items-center gap-2 text-xs font-semibold">
                  <span>
                    {tx("样本预览", "Sample Preview")} ·{" "}
                    {selectedDataset.lake_table}
                  </span>
                  {previewData && previewData.rows.length > 0 && (
                    <Badge variant="neutral">
                      {fmtNum(previewData.rows.length, 0)} {tx("行", "rows")}
                    </Badge>
                  )}
                </div>
                <div className="flex items-center gap-2 text-xs">
                  <Clock className="h-3.5 w-3.5 text-xblue" />
                  <span className="text-muted-foreground">
                    {tx("快照版本", "Snapshot")}:
                  </span>
                  <Select
                    value={selectedSnapshotId}
                    onChange={(e) => setSelectedSnapshotId(e.target.value)}
                    className="h-8 font-mono-tech"
                    data-testid="snapshot-selector"
                  >
                    <option value="">{tx("当前最新", "Latest (HEAD)")}</option>
                    {snapshots.map((snap) => (
                      <option
                        key={snap.snapshot_id}
                        value={String(snap.snapshot_id)}
                      >
                        #{snap.snapshot_id} (+{fmtNum(snap.rows_added, 0)}/-
                        {fmtNum(snap.rows_deleted, 0)}) ·{" "}
                        {fmtTime(snap.committed_at, true)}
                      </option>
                    ))}
                  </Select>
                </div>
              </div>

              {previewLoading ? (
                <div className="py-6 text-center text-xs text-muted-foreground">
                  {tx("读取预览中…", "Loading sample rows…")}
                </div>
              ) : !previewData || previewData.columns.length === 0 ? (
                <EmptyState
                  title={tx("暂无可预览数据", "No preview rows available")}
                />
              ) : (
                <div className="max-h-[440px] overflow-auto rounded border border-border">
                  <table
                    className="w-full border-collapse text-left text-xs"
                    data-testid="dataset-preview-table"
                  >
                    <thead className="sticky top-0 z-10 bg-muted border-b border-border">
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
                                <Badge variant="xblue">
                                  {tEnum(col.pii_category)}
                                </Badge>
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
                          className="border-b border-border last:border-0 hover:bg-muted/35"
                        >
                          {previewData.columns.map((col) => {
                            const rawCell = row[col.name];
                            const isNull =
                              rawCell === null || rawCell === undefined;
                            return (
                              <td
                                key={col.name}
                                className={`px-3 py-1.5 font-mono-tech whitespace-nowrap ${
                                  isNull
                                    ? "italic text-muted-foreground/70"
                                    : "text-foreground"
                                }`}
                              >
                                {formatPreviewCell(
                                  rawCell,
                                  col.name,
                                  col.data_type
                                )}
                              </td>
                            );
                          })}
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
              <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
                <div className="flex items-center gap-2">
                  <ShieldAlert className="h-4 w-4 text-xblue" />
                  <h3 className="text-xs font-semibold">
                    {tx("列级元数据与敏感分级", "Column Metadata & PII")}
                  </h3>
                  {dirtyColumns.length > 0 && (
                    <Badge variant="warning">
                      {tx(
                        `${dirtyColumns.length} 项未保存`,
                        `${dirtyColumns.length} unsaved`
                      )}
                    </Badge>
                  )}
                </div>

                <Button
                  type="button"
                  size="sm"
                  disabled={
                    !canSync || dirtyColumns.length === 0 || savingAllCols
                  }
                  onClick={() => void handleSaveAllColumnsMeta()}
                  data-testid="save-all-col-meta-btn"
                >
                  <Save className="h-3.5 w-3.5" />
                  {savingAllCols
                    ? tx("保存中…", "Saving…")
                    : tx("保存全部", "Save All")}
                </Button>
              </div>

              {columnsMeta.length === 0 ? (
                <EmptyState
                  title={tx("暂无列元数据", "No column metadata available")}
                />
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
                    const dirty = isColumnDirty(col);
                    return (
                      <div
                        key={col.column_name}
                        className={`grid grid-cols-1 md:grid-cols-12 gap-2 items-center rounded border p-2.5 text-xs transition-colors ${
                          dirty
                            ? "border-amber-500/50 bg-amber-500/5"
                            : "border-border bg-background"
                        }`}
                        data-testid={`column-meta-row-${col.column_name}`}
                      >
                        <div className="md:col-span-2 font-mono-tech min-w-0">
                          <div className="flex items-center gap-1.5">
                            <span
                              className="font-semibold truncate"
                              title={col.column_name}
                            >
                              {col.column_name}
                            </span>
                            {dirty && (
                              <span
                                className="h-2 w-2 rounded-full bg-amber-500 shrink-0"
                                title={tx("未保存变更", "Unsaved changes")}
                              />
                            )}
                          </div>
                          <div className="text-[11px] text-muted-foreground">
                            {col.data_type}
                          </div>
                        </div>

                        <div className="md:col-span-2">
                          <Select
                            value={draft.pii_category}
                            disabled={!canSync}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  pii_category: e.target.value as PiiCategory,
                                },
                              }))
                            }
                            className="h-8 w-full"
                            data-testid={`col-pii-select-${col.column_name}`}
                          >
                            {PII_CATEGORIES.map((cat) => (
                              <option key={cat} value={cat}>
                                {tEnum(cat)}
                              </option>
                            ))}
                          </Select>
                        </div>

                        <label className="md:col-span-2 inline-flex items-center gap-1.5 cursor-pointer">
                          <input
                            type="checkbox"
                            checked={draft.pii_confirmed}
                            disabled={!canSync}
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
                          <span>{tx("人工锁定", "Confirmed")}</span>
                        </label>

                        <div className="md:col-span-2">
                          <Input
                            className="h-8 text-xs"
                            value={draft.business_term}
                            disabled={!canSync}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  business_term: e.target.value,
                                },
                              }))
                            }
                            placeholder={tx("业务术语", "Business term")}
                          />
                        </div>

                        <div className="md:col-span-3">
                          <Input
                            className="h-8 text-xs"
                            value={draft.description}
                            disabled={!canSync}
                            onChange={(e) =>
                              setColEdits((prev) => ({
                                ...prev,
                                [col.column_name]: {
                                  ...draft,
                                  description: e.target.value,
                                },
                              }))
                            }
                            placeholder={tx("字段说明", "Column description")}
                          />
                        </div>

                        <div className="md:col-span-1 flex justify-end">
                          <Button
                            type="button"
                            variant={dirty ? "primary" : "outline"}
                            size="sm"
                            disabled={!canSync || savingCol === col.column_name}
                            onClick={() =>
                              void handleSaveColumnMeta(col.column_name)
                            }
                            aria-label={tx("保存", "Save")}
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
              <div className="flex items-center justify-between border-b border-border pb-2">
                <h3 className="text-xs font-semibold">
                  {tx("数据质量守卫规则", "Data Quality Rules")}
                </h3>
                <Badge variant="neutral">{fmtNum(qualityRules.length, 0)}</Badge>
              </div>

              <form
                onSubmit={(e) => void handleCreateQualityRule(e)}
                className="grid grid-cols-1 md:grid-cols-5 gap-2"
              >
                <Select
                  value={newRuleKind}
                  disabled={!canSync}
                  onChange={(e) =>
                    setNewRuleKind(e.target.value as QualityRuleKind)
                  }
                  data-testid="quality-rule-kind-select"
                >
                  {QUALITY_RULE_KINDS.map((rk) => (
                    <option key={rk} value={rk}>
                      {formatRuleKind(rk)}
                    </option>
                  ))}
                </Select>
                <Input
                  value={newRuleCol}
                  disabled={!canSync}
                  onChange={(e) => setNewRuleCol(e.target.value)}
                  placeholder="amount"
                  data-testid="quality-rule-column-input"
                />
                <Input
                  value={newRuleParams}
                  disabled={!canSync}
                  onChange={(e) => setNewRuleParams(e.target.value)}
                  placeholder='{"min": 0}'
                  className="font-mono-tech text-xs"
                  data-testid="quality-rule-params-input"
                />
                <Select
                  value={newRuleOnFail}
                  disabled={!canSync}
                  onChange={(e) =>
                    setNewRuleOnFail(e.target.value as QualityOnFail)
                  }
                  data-testid="quality-rule-onfail-select"
                >
                  <option value="WARN">{tEnum("WARN")}</option>
                  <option value="BLOCK">{tx("阻断提交", "Block Commit")}</option>
                </Select>
                <Button
                  type="submit"
                  variant="outline"
                  disabled={!canSync}
                  data-testid="quality-rule-add-btn"
                >
                  <Plus className="h-3.5 w-3.5" />
                  {tx("新增规则", "Add Rule")}
                </Button>
              </form>

              {qualityRules.length === 0 ? (
                <EmptyState
                  title={tx("暂无质量校验规则", "No quality rules configured")}
                />
              ) : (
                <div className="space-y-1.5">
                  {qualityRules.map((rule) => (
                    <div
                      key={rule.id}
                      className="rounded border border-border px-3 py-2 text-xs flex items-center justify-between"
                    >
                      <div className="flex flex-wrap items-center gap-2 font-mono-tech">
                        <Badge
                          variant={
                            rule.on_fail === "BLOCK" ? "danger" : "warning"
                          }
                        >
                          {rule.on_fail === "BLOCK"
                            ? tx("阻断", "Block")
                            : tEnum("WARN")}
                        </Badge>
                        <span className="font-semibold">
                          {formatRuleKind(rule.rule_kind)}
                        </span>
                        <span>
                          {tx("列", "Col")}: {rule.column_name ?? "*"}
                        </span>
                        <span className="text-muted-foreground">
                          {JSON.stringify(rule.params)}
                        </span>
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        disabled={!canSync}
                        onClick={() => void handleDeleteQualityRule(rule.id)}
                        aria-label={tx("删除", "Delete")}
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
              <div className="flex items-center justify-between border-b border-border pb-2">
                <h3 className="text-xs font-semibold">
                  {tx("实时推送端点", "Webhook Endpoints")}
                </h3>
                <Badge variant="neutral">{fmtNum(webhooks.length, 0)}</Badge>
              </div>

              <form
                onSubmit={(e) => void handleCreateWebhook(e)}
                className="grid grid-cols-1 md:grid-cols-5 gap-2"
              >
                <Input
                  value={whDatasetName}
                  disabled={!canAdmin}
                  onChange={(e) => setWhDatasetName(e.target.value)}
                  placeholder="iot_events"
                  data-testid="webhook-dataset-input"
                />
                <Input
                  value={whSlug}
                  disabled={!canAdmin}
                  onChange={(e) => setWhSlug(e.target.value)}
                  placeholder="demo-events"
                  data-testid="webhook-slug-input"
                />
                <Select
                  value={whAuthScheme}
                  disabled={!canAdmin}
                  onChange={(e) =>
                    setWhAuthScheme(e.target.value as WebhookAuthScheme)
                  }
                >
                  <option value="HMAC_SHA256">HMAC-SHA256</option>
                  <option value="BEARER">Bearer</option>
                </Select>
                <Input
                  value={whRecordPath}
                  disabled={!canAdmin}
                  onChange={(e) => setWhRecordPath(e.target.value)}
                  placeholder="$"
                  className="font-mono-tech text-xs"
                />
                <Button
                  type="submit"
                  variant="outline"
                  disabled={!canAdmin}
                  data-testid="webhook-create-btn"
                >
                  <Plus className="h-3.5 w-3.5" />
                  {tx("创建端点", "Create")}
                </Button>
              </form>

              {webhooks.length === 0 ? (
                <EmptyState
                  title={tx("暂无推送端点", "No webhook endpoints")}
                />
              ) : (
                <div className="space-y-1.5">
                  {webhooks.map((wh) => (
                    <div
                      key={wh.id}
                      className="rounded border border-border px-3 py-2 text-xs flex items-center justify-between font-mono-tech"
                    >
                      <div className="flex items-center gap-2">
                        <Badge variant="xblue">
                          {wh.auth_scheme === "HMAC_SHA256"
                            ? "HMAC-SHA256"
                            : "Bearer"}
                        </Badge>
                        <span className="font-semibold">{wh.slug}</span>
                        <span className="text-muted-foreground">
                          {wh.record_path}
                        </span>
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        disabled={!canAdmin}
                        onClick={() => void handleDeleteWebhook(wh.id)}
                        aria-label={tx("删除", "Delete")}
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

      {/* One-time Webhook Secret Modal */}
      <Modal
        open={Boolean(createdWebhookSecret)}
        onClose={() => setCreatedWebhookSecret(null)}
        title={tx("端点凭证", "Webhook Credentials")}
        footer={
          <Button
            type="button"
            onClick={() => setCreatedWebhookSecret(null)}
          >
            {tx("完成", "Done")}
          </Button>
        }
      >
        {createdWebhookSecret && (
          <div className="space-y-2 font-mono-tech text-xs">
            <p className="font-sans text-muted-foreground">
              {tx(
                "签名密钥仅展示一次，关闭后无法再次读取明文。",
                "This signing secret is shown once and cannot be retrieved later."
              )}
            </p>
            <div className="rounded border border-border bg-muted/40 p-2.5 break-all">
              <div className="text-[11px] text-muted-foreground">URL</div>
              <div>{createdWebhookSecret.url}</div>
            </div>
            <div className="rounded border border-border bg-muted/40 p-2.5 break-all">
              <div className="text-[11px] text-muted-foreground">Secret</div>
              <div>{createdWebhookSecret.secret}</div>
            </div>
          </div>
        )}
      </Modal>

      <ConfirmDialog
        open={Boolean(pendingDeleteDs)}
        onClose={() => setPendingDeleteDs(null)}
        onConfirm={() => void handleConfirmDeleteDataset()}
        loading={deletingDs}
        title={tx("删除数据集", "Delete Dataset")}
        description={tx(
          `确定删除数据集「${pendingDeleteDs?.name ?? ""}」及其湖表快照元数据？`,
          `Delete dataset "${pendingDeleteDs?.name ?? ""}" and its lake snapshot metadata?`
        )}
      />
    </div>
  );
}
