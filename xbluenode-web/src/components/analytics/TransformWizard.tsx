import { useEffect, useState } from "react";
import {
  CheckCircle2,
  RefreshCw,
  Sparkles,
  Trash2,
  Wand2,
} from "lucide-react";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
  type ApplyTransformResponse,
  type QueryColumn,
  type TransformDraft,
} from "@/lib/api/analytics";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";

export interface TransformWizardProps {
  catalog: AnalyticsCatalogResponse;
  onMaterialized?: () => Promise<void> | void;
  onSyncBottomResult?: (payload: {
    columns: QueryColumn[];
    rows: Record<string, unknown>[];
    sql: string;
    durationMs: number;
  }) => void;
}

export function TransformWizard({
  catalog,
  onMaterialized,
  onSyncBottomResult,
}: TransformWizardProps) {
  const [instruction, setInstruction] = useState("");
  const [sourceDatasetIds, setSourceDatasetIds] = useState<string[]>([]);
  const [targetName, setTargetName] = useState("derived_clean_orders");

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
    const first = catalog.datasets?.[0];
    if (sourceDatasetIds.length === 0 && first) {
      setSourceDatasetIds([first.id]);
    }
  }, [catalog.datasets, sourceDatasetIds.length]);

  const handleGenerateDraft = async () => {
    if (
      !instruction.trim() ||
      sourceDatasetIds.length === 0 ||
      !targetName.trim()
    ) {
      return;
    }
    setGenerating(true);
    setErrorMsg(null);
    setAppliedInfo(null);
    try {
      const created = await analyticsApi.createTransformDraft({
        instruction: instruction.trim(),
        source_datasets: sourceDatasetIds,
        target_dataset_name: targetName.trim(),
      });
      setDraft(created);
      setEditedSql(created.sql_text);
      onSyncBottomResult?.({
        columns: created.preview.columns,
        rows: created.preview.rows,
        sql: created.sql_text,
        durationMs: 0,
      });
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setGenerating(false);
    }
  };

  const handleUpdateDraftSql = async () => {
    if (!draft || !editedSql.trim()) return;
    setUpdatingSql(true);
    setErrorMsg(null);
    try {
      const updated = await analyticsApi.updateTransformDraft(
        draft.id,
        editedSql.trim()
      );
      setDraft(updated);
      setEditedSql(updated.sql_text);
      onSyncBottomResult?.({
        columns: updated.preview.columns,
        rows: updated.preview.rows,
        sql: updated.sql_text,
        durationMs: 0,
      });
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setUpdatingSql(false);
    }
  };

  const handleApplyDraft = async () => {
    if (!draft) return;
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
    if (!draft) return;
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

  return (
    <div className="space-y-3" data-testid="analytics-transform-wizard">
      {/* Step 1: Natural Language Specification */}
      <Card className="p-4 space-y-3">
        <div className="flex items-center justify-between border-b border-border pb-2 text-xs">
          <div className="flex items-center gap-2 font-semibold">
            <Wand2 className="h-3.5 w-3.5 text-xblue" />
            <span>AI 数据处理与派生视图向导</span>
          </div>
          <Badge variant="outline">
            指令 → SQL 守卫试运行 → 物化 DERIVED_VIEW
          </Badge>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-12 gap-3 text-xs">
          <div className="md:col-span-7 space-y-1.5">
            <label className="font-medium text-muted-foreground">
              自然语言清洗 / 关联 / 聚合指令
            </label>
            <Textarea
              rows={3}
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              placeholder="例如：筛选 status = 'PAID' 的有效订单，剔除异常金额并按 warehouse_code 汇总订单笔数与总金额"
              className="text-xs"
              data-testid="transform-instruction-input"
            />
          </div>

          <div className="md:col-span-5 space-y-2.5">
            <div className="space-y-1">
              <label className="font-medium text-muted-foreground">
                勾选上游源数据集
              </label>
              <div className="flex flex-wrap gap-1.5 max-h-20 overflow-y-auto">
                {(catalog.datasets ?? []).length === 0 ? (
                  <span className="text-[11px] text-muted-foreground">
                    暂无可用源数据集
                  </span>
                ) : (
                  (catalog.datasets ?? []).map((ds) => {
                    const checked = sourceDatasetIds.includes(ds.id);
                    return (
                      <label
                        key={ds.id}
                        className={`inline-flex items-center gap-1 rounded border px-2 py-1 font-mono-tech text-[11px] cursor-pointer ${
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
                目标派生数据集名称 (a-z0-9_)
              </label>
              <div className="flex gap-2">
                <Input
                  value={targetName}
                  onChange={(e) => setTargetName(e.target.value)}
                  placeholder="derived_paid_orders"
                  className="h-8 text-xs font-mono-tech"
                  data-testid="transform-target-name"
                />
                <Button
                  type="button"
                  variant="primary"
                  disabled={
                    generating ||
                    !instruction.trim() ||
                    sourceDatasetIds.length === 0 ||
                    !targetName.trim()
                  }
                  onClick={() => void handleGenerateDraft()}
                  data-testid="transform-generate-btn"
                >
                  <Sparkles className="h-3.5 w-3.5" />
                  {generating ? "生成中..." : "生成草案"}
                </Button>
              </div>
            </div>
          </div>
        </div>
      </Card>

      {errorMsg && (
        <Card className="border-foreground/40 bg-muted/40 p-3 text-xs">
          <span className="font-mono-tech font-semibold">变换操作失败：</span>
          {errorMsg}
        </Card>
      )}

      {/* Step 2 & 3: Draft SQL Edit + Schema Diff + 100-Row Preview + Apply/Discard */}
      {draft && (
        <Card className="p-4 space-y-3" data-testid="transform-draft-panel">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2 text-xs">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-semibold">
                变换草案 ·{" "}
                <span className="font-mono-tech">
                  {draft.target_dataset_name}
                </span>
              </span>
              <Badge
                variant={draft.status === "APPLIED" ? "xblue" : "neutral"}
                data-testid="transform-draft-status"
              >
                {draft.status}
              </Badge>
              <Badge variant="outline">
                预计 {draft.preview.row_count_estimate} 行
              </Badge>
            </div>

            {draft.status === "DRAFT" && (
              <div className="flex items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  disabled={updatingSql || !editedSql.trim()}
                  onClick={() => void handleUpdateDraftSql()}
                  data-testid="transform-repreview-btn"
                >
                  <RefreshCw
                    className={`h-3.5 w-3.5 text-xblue ${
                      updatingSql ? "animate-spin" : ""
                    }`}
                  />
                  更新 SQL 并预览
                </Button>

                <Button
                  type="button"
                  variant="ghost"
                  disabled={discarding}
                  onClick={() => void handleDiscardDraft()}
                  data-testid="transform-discard-btn"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  丢弃草案
                </Button>

                <Button
                  type="button"
                  variant="primary"
                  disabled={applying}
                  onClick={() => void handleApplyDraft()}
                  data-testid="transform-apply-btn"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {applying ? "物化入湖中..." : "确认物化"}
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
              <span className="text-muted-foreground">列结构变更:</span>
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
              变换 SELECT 语句（可手改后点击「更新 SQL 并预览」）
            </label>
            <Textarea
              rows={3}
              value={editedSql}
              disabled={draft.status !== "DRAFT"}
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
              已创建派生数据集{" "}
              <strong className="font-mono-tech">
                {appliedInfo.dataset.name}
              </strong>
              （任务 ID:{" "}
              <span className="font-mono-tech">{appliedInfo.job_id}</span>
              ），已自动入队 DuckLake 物化同步。
            </div>
          )}

          {/* 100-Row Preview Grid */}
          <ResultGrid
            columns={draft.preview.columns}
            rows={draft.preview.rows}
            rowCount={draft.preview.rows.length}
            maxHeight={220}
            testId="transform-preview-grid"
          />
        </Card>
      )}
    </div>
  );
}
