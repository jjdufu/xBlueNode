import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, ListChecks, Play, Plus, Save, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import type { GroundingMode } from "@/lib/api/enums";
import {
  xblueApi,
  type CompetencyQuestionSpec,
  type HarnessRunRecord,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  EmptyState,
  Input,
  Modal,
  Select,
  StatusBadge,
  Switch,
} from "@/components/ui/primitives";

const GROUNDING_MODES: readonly GroundingMode[] = [
  "SEMANTIC_METRIC",
  "DOMAIN_OBJECT",
  "GUARDED_SQL",
  "GROUNDED_DOC",
  "ABSTAINED",
];

function parseCsvTokens(raw: string): string[] {
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export interface CompetencySuiteModalProps {
  open: boolean;
  subgraphSlug: string;
  activeDraftId: string | null;
  canEdit: boolean;
  onClose: () => void;
}

export function CompetencySuiteModal({
  open,
  subgraphSlug,
  activeDraftId,
  canEdit,
  onClose,
}: CompetencySuiteModalProps) {
  const { tx, tEnum, fmtTime } = useI18n();
  const [questions, setQuestions] = useState<CompetencyQuestionSpec[]>([]);
  const [runs, setRuns] = useState<HarnessRunRecord[]>([]);
  const [latestRun, setLatestRun] = useState<HarnessRunRecord | null>(null);
  const [runMaterialize, setRunMaterialize] = useState<boolean>(true);
  const [loading, setLoading] = useState<boolean>(false);
  const [saving, setSaving] = useState<boolean>(false);
  const [verifying, setVerifying] = useState<boolean>(false);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadSuiteAndRuns = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const [suiteRes, runsRes] = await Promise.all([
        xblueApi.getCompetencySuite(subgraphSlug),
        xblueApi.listHarnessRuns(subgraphSlug).catch(() => ({ items: [] as HarnessRunRecord[] })),
      ]);
      const loadedQuestions = Array.isArray(suiteRes?.questions) ? suiteRes.questions : [];
      const loadedRuns = Array.isArray(runsRes?.items) ? runsRes.items : [];
      setQuestions(loadedQuestions);
      setRuns(loadedRuns);
      setLatestRun(loadedRuns[0] ?? null);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [subgraphSlug]);

  useEffect(() => {
    if (!open) return;
    setStatusMsg(null);
    void loadSuiteAndRuns();
  }, [open, loadSuiteAndRuns]);

  const handleAddQuestion = () => {
    const nextIdx = questions.length + 1;
    setQuestions((prev) => [
      ...prev,
      {
        id: `cq_${nextIdx}`,
        question: "",
        expected_metrics: [],
        expected_node_types: [],
        tolerance: 0.001,
        must_not_abstain: true,
      },
    ]);
  };

  const updateQuestion = (
    idx: number,
    updater: (cur: CompetencyQuestionSpec) => CompetencyQuestionSpec
  ) => {
    setQuestions((prev) =>
      prev.map((item, i) => (i === idx ? updater(item) : item))
    );
  };

  const removeQuestion = (idx: number) => {
    setQuestions((prev) => prev.filter((_, i) => i !== idx));
  };

  const handleSave = async () => {
    if (!canEdit || saving) return;
    setSaving(true);
    setErrorMsg(null);
    setStatusMsg(null);
    try {
      const cleaned = questions
        .map((q, idx) => ({
          ...q,
          id: q.id.trim() || `cq_${idx + 1}`,
          question: q.question.trim(),
        }))
        .filter((q) => q.question.length > 0);
      const saved = await xblueApi.putCompetencySuite(subgraphSlug, {
        questions: cleaned,
      });
      setQuestions(saved.questions);
      setStatusMsg(tx("已保存", "Saved"));
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleVerify = async () => {
    if (!canEdit || verifying) return;
    setVerifying(true);
    setErrorMsg(null);
    setStatusMsg(null);
    try {
      const cleaned = questions
        .map((q, idx) => ({
          ...q,
          id: q.id.trim() || `cq_${idx + 1}`,
          question: q.question.trim(),
        }))
        .filter((q) => q.question.length > 0);
      await xblueApi.putCompetencySuite(subgraphSlug, { questions: cleaned });
      const run = await xblueApi.verifyCompetencySuite(subgraphSlug, {
        ...(activeDraftId ? { draft_id: activeDraftId } : {}),
        run_dev_materialize: subgraphSlug !== "main" && runMaterialize,
      });
      setLatestRun(run);
      setRuns((prev) => [run, ...prev.filter((r) => r.id !== run.id)].slice(0, 10));
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setVerifying(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      maxWidthClass="max-w-4xl"
      testId="competency-suite-modal"
      closeTestId="close-competency-suite-btn"
      title={
        <div className="flex items-center gap-2">
          <ListChecks className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("能力问答验收", "Competency QA")}</span>
          <Badge variant="outline" className="font-mono-tech">
            {subgraphSlug}
          </Badge>
          {latestRun && (
            <StatusBadge
              status={latestRun.status}
              data-testid="competency-latest-status"
            />
          )}
        </div>
      }
      footer={
        <div className="flex flex-wrap items-center justify-between gap-2 w-full">
          <div className="flex items-center gap-3 text-xs">
            {subgraphSlug !== "main" && (
              <label className="inline-flex items-center gap-2 text-muted-foreground">
                <Switch
                  checked={runMaterialize}
                  onCheckedChange={setRunMaterialize}
                  aria-label={tx("先试灌沙盒", "Materialize First")}
                />
                <span>{tx("先试灌沙盒", "Materialize First")}</span>
              </label>
            )}
            {statusMsg && (
              <span className="text-emerald-600 dark:text-emerald-400">
                {statusMsg}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Button variant="outline" onClick={onClose}>
              {tx("关闭", "Close")}
            </Button>
            {canEdit && (
              <>
                <Button
                  variant="outline"
                  disabled={saving || verifying}
                  onClick={() => void handleSave()}
                  data-testid="competency-save-btn"
                >
                  <Save className="h-3.5 w-3.5" />
                  {saving ? tx("保存中…", "Saving…") : tx("保存套件", "Save")}
                </Button>
                <Button
                  variant="primary"
                  disabled={verifying || questions.length === 0}
                  onClick={() => void handleVerify()}
                  data-testid="competency-verify-btn"
                >
                  <Play className="h-3.5 w-3.5" />
                  {verifying ? tx("核验中…", "Verifying…") : tx("运行核验", "Verify")}
                </Button>
              </>
            )}
          </div>
        </div>
      }
    >
      <div className="space-y-4 p-4 overflow-y-auto text-xs">
        {errorMsg && (
          <div
            data-testid="competency-error"
            className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-red-600 dark:text-red-400"
          >
            {errorMsg}
          </div>
        )}

        <div className="flex items-center justify-between">
          <div className="font-semibold text-foreground">
            {tx("验收题集", "Questions")} ({questions.length})
          </div>
          {canEdit && (
            <Button
              size="sm"
              variant="outline"
              onClick={handleAddQuestion}
              data-testid="competency-add-question-btn"
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新增题目", "Add")}
            </Button>
          )}
        </div>

        {loading ? (
          <div className="py-6 text-center text-muted-foreground">
            {tx("加载中…", "Loading…")}
          </div>
        ) : questions.length === 0 ? (
          <EmptyState
            title={tx("暂无验收题", "No Questions")}
            action={
              canEdit ? (
                <Button size="sm" onClick={handleAddQuestion}>
                  <Plus className="h-3.5 w-3.5" />
                  {tx("新增题目", "Add")}
                </Button>
              ) : undefined
            }
          />
        ) : (
          <div className="space-y-2.5">
            {questions.map((q, idx) => (
              <div
                key={`${q.id}-${idx}`}
                data-testid={`competency-question-row-${idx}`}
                className="rounded-md border border-border bg-muted/20 p-3 space-y-2"
              >
                <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-center">
                  <div className="md:col-span-2">
                    <Input
                      value={q.id}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateQuestion(idx, (cur) => ({ ...cur, id: e.target.value }))
                      }
                      placeholder="cq_1"
                      className="font-mono-tech text-xs"
                      aria-label={tx("题号", "Question ID")}
                    />
                  </div>
                  <div className="md:col-span-6">
                    <Input
                      value={q.question}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateQuestion(idx, (cur) => ({
                          ...cur,
                          question: e.target.value,
                        }))
                      }
                      placeholder={tx(
                        "华东仓当前库存总量是多少？",
                        "What is total East warehouse inventory?"
                      )}
                      aria-label={tx("问数语句", "Question")}
                    />
                  </div>
                  <div className="md:col-span-3">
                    <Select
                      value={q.expected_grounding_mode ?? ""}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateQuestion(idx, (cur) => ({
                          ...cur,
                          expected_grounding_mode: e.target.value
                            ? (e.target.value as GroundingMode)
                            : undefined,
                        }))
                      }
                      className="w-full"
                      aria-label={tx("接地模式", "Grounding Mode")}
                    >
                      <option value="">{tx("任意接地", "Any Grounding")}</option>
                      {GROUNDING_MODES.map((m) => (
                        <option key={m} value={m}>
                          {tEnum(m)}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <div className="md:col-span-1 flex justify-end">
                    {canEdit && (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => removeQuestion(idx)}
                        aria-label={tx("删除", "Delete")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    )}
                  </div>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-3 gap-2">
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground block">
                      {tx("期望指标", "Expected Metrics")}
                    </label>
                    <Input
                      value={(q.expected_metrics ?? []).join(", ")}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateQuestion(idx, (cur) => ({
                          ...cur,
                          expected_metrics: parseCsvTokens(e.target.value),
                        }))
                      }
                      placeholder="stock_qty, order_amount"
                      className="font-mono-tech text-xs"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground block">
                      {tx("期望对象", "Expected Types")}
                    </label>
                    <Input
                      value={(q.expected_node_types ?? []).join(", ")}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateQuestion(idx, (cur) => ({
                          ...cur,
                          expected_node_types: parseCsvTokens(e.target.value),
                        }))
                      }
                      placeholder="Warehouse, Order"
                      className="font-mono-tech text-xs"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground block">
                      {tx("期望数值", "Expected Number")}
                    </label>
                    <Input
                      value={
                        q.expected_number !== undefined ? String(q.expected_number) : ""
                      }
                      disabled={!canEdit}
                      onChange={(e) => {
                        const raw = e.target.value.trim();
                        const parsed = raw === "" ? undefined : Number(raw);
                        updateQuestion(idx, (cur) => ({
                          ...cur,
                          expected_number:
                            parsed !== undefined && Number.isFinite(parsed)
                              ? parsed
                              : undefined,
                        }));
                      }}
                      placeholder="1200"
                      className="font-mono-tech text-xs"
                    />
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}

        {latestRun && (
          <div
            data-testid="competency-run-report"
            className="rounded-md border border-border p-3 space-y-3"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2 font-semibold">
                {latestRun.status === "PASS" ? (
                  <CheckCircle2 className="h-4 w-4 text-emerald-600" />
                ) : (
                  <AlertTriangle className="h-4 w-4 text-amber-500" />
                )}
                <span>{tx("最近核验结果", "Latest Verification")}</span>
                <Badge variant="xblue" className="font-mono-tech">
                  {latestRun.passed_count} / {latestRun.total_questions}
                </Badge>
              </div>
              <span className="font-mono-tech text-[11px] text-muted-foreground">
                {fmtTime(latestRun.created_at)}
              </span>
            </div>

            {latestRun.gap_diagnostics.length > 0 && (
              <div className="space-y-1.5" data-testid="competency-gap-list">
                {latestRun.gap_diagnostics.map((gap, gIdx) => (
                  <div
                    key={`${gap.question_id}-${gIdx}`}
                    data-testid={`competency-gap-item-${gIdx}`}
                    className="rounded border border-amber-500/30 bg-amber-500/10 px-2.5 py-2 flex flex-wrap items-center justify-between gap-2"
                  >
                    <div className="space-y-0.5 min-w-0">
                      <div className="flex items-center gap-1.5">
                        <Badge variant="warning">{tEnum(gap.gap_kind)}</Badge>
                        <span className="font-mono-tech font-medium">
                          {gap.question_id}
                        </span>
                        <span className="font-mono-tech text-muted-foreground">
                          · {gap.target_ref}
                        </span>
                      </div>
                      <div className="text-foreground">{gap.message}</div>
                    </div>
                    <Badge variant="outline" className="font-mono-tech">
                      {gap.suggested_tool}
                    </Badge>
                  </div>
                ))}
              </div>
            )}

            {runs.length > 1 && (
              <div className="pt-2 border-t border-border flex flex-wrap items-center gap-2">
                <span className="text-muted-foreground">
                  {tx("历史记录", "History")}
                </span>
                {runs.slice(0, 5).map((r) => (
                  <button
                    key={r.id}
                    type="button"
                    onClick={() => setLatestRun(r)}
                    className="inline-flex items-center gap-1 rounded border border-border px-2 py-0.5 font-mono-tech text-[11px] hover:bg-muted cursor-pointer"
                  >
                    <span>
                      {r.passed_count}/{r.total_questions}
                    </span>
                    <span>· {tEnum(r.status)}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}
