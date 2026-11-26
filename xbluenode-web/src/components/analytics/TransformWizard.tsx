import { useEffect, useState } from "react";
import {
  CheckCircle2,
  RefreshCw,
  Sparkles,
  Trash2,
  Wand2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
  type ApplyTransformResponse,
  type TransformDraft,
} from "@/lib/api/analytics";
import { usePrincipal } from "@/lib/permissions";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import {
  Badge,
  Button,
  Card,
  Input,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";

export interface TransformWizardProps {
  catalog: AnalyticsCatalogResponse;
  onMaterialized?: () => Promise<void> | void;
}

export function TransformWizard({
  catalog,
  onMaterialized,
}: TransformWizardProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { canAny } = usePrincipal();
  const canTransform = canAny(["DATA_SYNC", "DATA_ADMIN", "STUDIO_EDIT"]);

  const [instruction, setInstruction] = useState("");
  const [sourceDatasetIds, setSourceDatasetIds] = useState<string[]>([]);
  const [sourceFilter, setSourceFilter] = useState("");
  const [targetName, setTargetName] = useState("");

  const [draft, setDraft] = useState<TransformDraft | null>(null);
  const [editedSql, setEditedSql] = useState("");
  const [appliedInfo, setAppliedInfo] =
    useState<ApplyTransformResponse | null>(null);

  const [generating, setGenerating] = useState(false);
  const [updatingSql, setUpdatingSql] = useState(false);
  const [applying, setApplying] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    const datasets = catalog.datasets ?? [];
    const preferred =
      datasets.find((d) => d.name === "demo_orders") ?? datasets[0];
    if (sourceDatasetIds.length === 0 && preferred) {
      setSourceDatasetIds([preferred.id]);
    }
  }, [catalog.datasets, sourceDatasetIds.length]);

  const handleGenerateDraft = async () => {
    if (!canTransform || !instruction.trim() || sourceDatasetIds.length === 0) {
      return;
    }
    const effectiveTargetName = targetName.trim() || "clean_orders_summary";
    setGenerating(true);
    setErrorMsg(null);
    setAppliedInfo(null);
    try {
      const created = await analyticsApi.createTransformDraft({
        instruction: instruction.trim(),
        source_datasets: sourceDatasetIds,
        target_dataset_name: effectiveTargetName,
      });
      setDraft(created);
      setEditedSql(created.sql_text);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setGenerating(false);
    }
  };

  const handleUpdateDraftSql = async () => {
    if (!canTransform || !draft || !editedSql.trim()) return;
    setUpdatingSql(true);
    setErrorMsg(null);
    try {
      const updated = await analyticsApi.updateTransformDraft(
        draft.id,
        editedSql.trim()
      );
      setDraft(updated);
      setEditedSql(updated.sql_text);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setUpdatingSql(false);
    }
  };

  const handleApplyDraft = async () => {
    if (!canTransform || !draft) return;
    setApplying(true);
    setErrorMsg(null);
    try {
      const res = await analyticsApi.applyTransformDraft(draft.id);
      setDraft(res.draft);
      setAppliedInfo(res);
      await onMaterialized?.();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setApplying(false);
    }
  };

  const handleDiscardDraft = async () => {
    if (!canTransform || !draft) return;
    setDiscarding(true);
    setErrorMsg(null);
    try {
      const discarded = await analyticsApi.discardTransformDraft(draft.id);
      setDraft(discarded);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDiscarding(false);
    }
  };

  const renderDraftStatus = (status: TransformDraft["status"]) => {
    if (status === "APPLIED") {
      return (
        <Badge variant="success" data-testid="transform-draft-status">
          {tx("已物化", "Applied")}
        </Badge>
      );
    }
    if (status === "DISCARDED") {
      return (
        <Badge variant="outline" data-testid="transform-draft-status">
          {tx("已丢弃", "Discarded")}
        </Badge>
      );
    }
    return <StatusBadge status={status} data-testid="transform-draft-status" />;
  };

  return (
    <div className="space-y-3" data-testid="analytics-transform-wizard">
      {/* Step 1: Natural Language Specification */}
      <Card className="p-4 space-y-3">
        <div className="flex items-center justify-between border-b border-border pb-2 text-xs">
          <div className="flex items-center gap-2 font-semibold">
            <Wand2 className="h-3.5 w-3.5 text-xblue" />
            <span>{tx("智能数据变换与派生视图", "AI Data Transform")}</span>
          </div>
          <Badge variant="outline">{tEnum("DERIVED_VIEW")}</Badge>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-12 gap-3 text-xs">
          <div className="md:col-span-7 space-y-1.5">
            <label className="font-medium text-muted-foreground">
              {tx("清洗与聚合指令", "Transform Instruction")}
            </label>
            <Textarea
              rows={3}
              value={instruction}
              disabled={!canTransform}
              onChange={(e) => setInstruction(e.target.value)}
              placeholder={tx(
                "筛选已支付订单并按仓库编码汇总笔数与总金额",
                "Filter paid orders and aggregate order count and total amount by warehouse"
              )}
              className="text-xs"
              data-testid="transform-instruction-input"
            />
          </div>

          <div className="md:col-span-5 space-y-2.5">
            <div className="space-y-1.5">
              <div className="flex items-center justify-between gap-2">
                <label className="font-medium text-muted-foreground">
                  {tx("源数据集", "Source Datasets")} (
                  {(catalog.datasets ?? []).length})
                </label>
                {(catalog.datasets ?? []).length > 6 && (
                  <Input
                    value={sourceFilter}
                    onChange={(e) => setSourceFilter(e.target.value)}
                    placeholder={tx("过滤表名…", "Filter tables…")}
                    className="h-6 w-36 px-2 text-[11px] font-mono-tech"
                  />
                )}
              </div>
              <div className="flex flex-wrap gap-1.5 max-h-40 overflow-y-auto rounded border border-border/70 bg-background/40 p-2">
                {(catalog.datasets ?? []).length === 0 ? (
                  <span className="text-[11px] text-muted-foreground">
                    {tx("暂无源数据集", "No source datasets")}
                  </span>
                ) : (
                  (catalog.datasets ?? [])
                    .filter((ds) => {
                      const q = sourceFilter.trim().toLowerCase();
                      if (!q) return true;
                      return (
                        ds.name.toLowerCase().includes(q) ||
                        (ds.display_name ?? "").toLowerCase().includes(q)
                      );
                    })
                    .map((ds) => {
                      const checked = sourceDatasetIds.includes(ds.id);
                      return (
                        <label
                          key={ds.id}
                          className={`inline-flex items-center gap-1.5 rounded border px-2 py-1 font-mono-tech text-[11px] cursor-pointer ${
                            checked
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border bg-background hover:border-foreground/35"
                          }`}
                        >
                          <input
                            type="checkbox"
                            checked={checked}
                            disabled={!canTransform}
                            onChange={(e) => {
                              if (e.target.checked) {
                                setSourceDatasetIds((prev) => [...prev, ds.id]);
                              } else {
                                setSourceDatasetIds((prev) =>
                                  prev.filter((id) => id !== ds.id)
                                );
                              }
                            }}
                            className="accent-xblue"
                            data-testid={`transform-source-${ds.name}`}
                          />
                          {ds.name}
                        </label>
                      );
                    })
                )}
              </div>
            </div>

            <div className="space-y-1">
              <label className="font-medium text-muted-foreground">
                {tx("目标数据集标识", "Target Dataset Name")}
              </label>
              <div className="flex gap-2">
                <Input
                  value={targetName}
                  disabled={!canTransform}
                  onChange={(e) => setTargetName(e.target.value)}
                  placeholder="clean_orders_summary"
                  className="h-8 text-xs font-mono-tech"
                  data-testid="transform-target-name"
                />
                <Button
                  type="button"
                  variant="primary"
                  disabled={
                    !canTransform ||
                    generating ||
                    !instruction.trim() ||
                    sourceDatasetIds.length === 0
                  }
                  onClick={() => void handleGenerateDraft()}
                  data-testid="transform-generate-btn"
                >
                  <Sparkles className="h-3.5 w-3.5" />
                  {generating
                    ? tx("生成中…", "Generating…")
                    : tx("生成草案", "Generate")}
                </Button>
              </div>
            </div>
          </div>
        </div>
      </Card>

      {errorMsg && (
        <Card className="border-red-500/30 bg-red-500/5 p-3 text-xs text-red-600 dark:text-red-400">
          {errorMsg}
        </Card>
      )}

      {/* Step 2 & 3: Draft SQL Edit + Schema Diff + Preview + Apply/Discard */}
      {draft && (
        <Card className="p-4 space-y-3" data-testid="transform-draft-panel">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2 text-xs">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-semibold">
                {tx("变换草案", "Transform Draft")} ·{" "}
                <span className="font-mono-tech">
                  {draft.target_dataset_name}
                </span>
              </span>
              {renderDraftStatus(draft.status)}
              <Badge variant="outline">
                ~{fmtNum(draft.preview.row_count_estimate, 0)}{" "}
                {tx("行", "rows")}
              </Badge>
            </div>

            {draft.status === "DRAFT" && (
              <div className="flex items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!canTransform || updatingSql || !editedSql.trim()}
                  onClick={() => void handleUpdateDraftSql()}
                  data-testid="transform-repreview-btn"
                >
                  <RefreshCw
                    className={`h-3.5 w-3.5 text-xblue ${
                      updatingSql ? "animate-spin" : ""
                    }`}
                  />
                  {tx("刷新预览", "Refresh")}
                </Button>

                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={!canTransform || discarding}
                  onClick={() => void handleDiscardDraft()}
                  data-testid="transform-discard-btn"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  {tx("丢弃草案", "Discard")}
                </Button>

                <Button
                  type="button"
                  variant="primary"
                  size="sm"
                  disabled={!canTransform || applying}
                  onClick={() => void handleApplyDraft()}
                  data-testid="transform-apply-btn"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {applying
                    ? tx("物化中…", "Materializing…")
                    : tx("物化入湖", "Materialize")}
                </Button>
              </div>
            )}
          </div>

          {/* Column Schema Diff */}
          {(draft.preview.added_columns.length > 0 ||
            draft.preview.removed_columns.length > 0) && (
            <div
              className="flex flex-wrap items-center gap-1.5 text-xs"
              data-testid="transform-schema-diff"
            >
              <span className="text-muted-foreground">
                {tx("列变更", "Schema Diff")}:
              </span>
              {draft.preview.added_columns.map((col) => (
                <Badge key={`add-${col}`} variant="xblue">
                  +{col}
                </Badge>
              ))}
              {draft.preview.removed_columns.map((col) => (
                <Badge key={`rem-${col}`} variant="outline">
                  -{col}
                </Badge>
              ))}
            </div>
          )}

          {/* Editable SQL */}
          <div className="space-y-1">
            <label className="text-xs text-muted-foreground">
              {tx("变换查询语句", "Transform SQL")}
            </label>
            <Textarea
              rows={3}
              value={editedSql}
              disabled={draft.status !== "DRAFT" || !canTransform}
              onChange={(e) => setEditedSql(e.target.value)}
              className="font-mono-tech text-xs"
              data-testid="transform-sql-textarea"
            />
          </div>

          {appliedInfo && (
            <div
              className="rounded border border-xblue/35 bg-xblue-soft/40 p-2.5 text-xs"
              data-testid="transform-applied-banner"
            >
              {tx("已创建派生数据集 ", "Materialized dataset ")}
              <strong className="font-mono-tech">
                {appliedInfo.dataset.name}
              </strong>{" "}
              ({appliedInfo.job_id.slice(0, 8)})
            </div>
          )}

          {/* Preview Grid */}
          <ResultGrid
            columns={draft.preview.columns}
            rows={draft.preview.rows}
            rowCount={draft.preview.rows.length}
            maxHeight={240}
            testId="transform-preview-grid"
          />
        </Card>
      )}
    </div>
  );
}
