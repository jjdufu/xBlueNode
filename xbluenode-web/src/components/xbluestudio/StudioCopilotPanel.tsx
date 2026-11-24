import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertCircle,
  AlertTriangle,
  Crosshair,
  Database,
  FileText,
  History,
  Info,
  Loader2,
  PanelRightClose,
  PanelRightOpen,
  RotateCcw,
  Send,
  Sparkles,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { Dataset } from "@/lib/api/axhub";
import type { CopilotActionMode } from "@/lib/api/enums";
import {
  countSchemaDiffChanges,
  xblueApi,
  type CopilotFinding,
  type DraftSnapshotReason,
  type DraftSnapshotSummary,
  type SchemaDraft,
  type StudioCopilotRequest,
  type StudioCopilotResponse,
  type ValidationIssue,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Card,
  IconButton,
  Select,
  Tabs,
  Textarea,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface StudioCopilotPanelProps {
  collapsed: boolean;
  onToggleCollapsed: () => void;
  activeDraft: SchemaDraft | null;
  selectedNodeName: string | null;
  selectedEdgeName: string | null;
  activeInspectorTab: string;
  datasets: Dataset[];
  onEnsureDraft: () => Promise<SchemaDraft | null>;
  onDraftUpdated: (draft: SchemaDraft, notice?: string) => void;
  onDiagnosticsUpdated: (passCount: number, issues: ValidationIssue[]) => void;
  onLocateTargetPath?: (path: string) => void;
}

interface QuickActionDef {
  readonly key: string;
  readonly zh: string;
  readonly en: string;
  readonly buildPrompt: (
    focusTarget: string,
    tx: (zh: string, en: string) => string
  ) => string;
}

const QUICK_ACTIONS: readonly QuickActionDef[] = [
  {
    key: "health",
    zh: "完备度体检",
    en: "Health Check",
    buildPrompt: (target, tx) =>
      tx(
        `完备度体检：检查 ${target} 的指标定义、计量单位与数据映射完备度`,
        `Health check: inspect metrics, units, and mappings on ${target}`
      ),
  },
  {
    key: "units",
    zh: "补全单位描述",
    en: "Fill Units",
    buildPrompt: (target, tx) =>
      tx(
        `补全单位描述：为 ${target} 缺失单位或业务说明的指标补齐标准定义`,
        `Fill units: populate missing units and descriptions on ${target}`
      ),
  },
  {
    key: "rollups",
    zh: "推导聚合指标",
    en: "Suggest Rollups",
    buildPrompt: (target, tx) =>
      tx(
        `推导聚合指标：基于上下游关联关系为 ${target} 推导跨边汇总指标`,
        `Suggest rollups: derive cross-link aggregation rules for ${target}`
      ),
  },
  {
    key: "mappings",
    zh: "检查映射对齐",
    en: "Check Mappings",
    buildPrompt: (target, tx) =>
      tx(
        `检查映射对齐：核查 ${target} 的主键列、属性类型与源数据集列对齐情况`,
        `Check mappings: verify key columns and field types for ${target}`
      ),
  },
  {
    key: "formulas",
    zh: "生成派生公式",
    en: "Derive Formulas",
    buildPrompt: (target, tx) =>
      tx(
        `生成派生公式：基于 ${target} 现有指标合成比率或缺口派生计算公式`,
        `Derive formulas: synthesize ratio or gap expressions on ${target}`
      ),
  },
  {
    key: "pii",
    zh: "审查敏感字段",
    en: "Audit PII",
    buildPrompt: (target, tx) =>
      tx(
        `审查敏感字段：识别 ${target} 潜在个人隐私或敏感属性并建议脱敏分级`,
        `Audit PII: identify sensitive attributes on ${target} and suggest masking tiers`
      ),
  },
];

export function StudioCopilotPanel({
  collapsed,
  onToggleCollapsed,
  activeDraft,
  selectedNodeName,
  selectedEdgeName,
  activeInspectorTab,
  datasets,
  onEnsureDraft,
  onDraftUpdated,
  onDiagnosticsUpdated,
  onLocateTargetPath,
}: StudioCopilotPanelProps) {
  const { tx, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const [mode, setMode] = useState<CopilotActionMode>("DIAGNOSE_ONLY");
  const [prompt, setPrompt] = useState<string>("");
  const [includeSpecDoc, setIncludeSpecDoc] = useState<boolean>(false);
  const [attachedDatasetIds, setAttachedDatasetIds] = useState<string[]>([]);
  const [running, setRunning] = useState<boolean>(false);
  const [restoringSnapId, setRestoringSnapId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const [lastResult, setLastResult] = useState<{
    draftId: string;
    response: StudioCopilotResponse;
  } | null>(null);
  const [snapshots, setSnapshots] = useState<DraftSnapshotSummary[]>([]);

  const focusLabel = useMemo(() => {
    if (selectedNodeName) {
      return tx(`焦点：${selectedNodeName}`, `Focus: ${selectedNodeName}`);
    }
    if (selectedEdgeName) {
      return tx(`焦点：${selectedEdgeName}`, `Focus: ${selectedEdgeName}`);
    }
    return tx("焦点：全局子图", "Focus: Subgraph");
  }, [selectedNodeName, selectedEdgeName, tx]);

  const focusTargetText =
    selectedNodeName ?? selectedEdgeName ?? tx("全局子图", "Subgraph");

  const attachedDatasets = useMemo(
    () => datasets.filter((ds) => attachedDatasetIds.includes(ds.id)),
    [datasets, attachedDatasetIds]
  );

  const unattachedDatasets = useMemo(
    () => datasets.filter((ds) => Boolean(ds.id) && !attachedDatasetIds.includes(ds.id)),
    [datasets, attachedDatasetIds]
  );

  const refreshSnapshots = useCallback(async (draftId: string) => {
    if (!draftId) {
      setSnapshots([]);
      return;
    }
    try {
      const res = await xblueApi.listDraftSnapshots(draftId, 12);
      setSnapshots(Array.isArray(res?.items) ? res.items : []);
    } catch {
      setSnapshots([]);
    }
  }, []);

  useEffect(() => {
    const id = activeDraft?.id ?? "";
    if (!id) {
      setSnapshots([]);
      return;
    }
    void refreshSnapshots(id);
  }, [activeDraft?.id, refreshSnapshots]);

  const formatReason = (reason: DraftSnapshotReason): string => {
    switch (reason) {
      case "COPILOT_PATCH":
        return tx("AI 改写", "Copilot");
      case "BATCH_CONFIRM":
        return tx("批量确认", "Confirm");
      case "PRE_MERGE":
        return tx("合并前", "Pre-Merge");
      case "MANUAL":
      default:
        return tx("手动快照", "Manual");
    }
  };

  const handleSend = async () => {
    const trimmed = prompt.trim();
    if (!trimmed || running || !canEdit) return;
    setRunning(true);
    setErrorMsg(null);

    try {
      const draft = await onEnsureDraft();
      if (!draft?.id) {
        setErrorMsg(tx("草案初始化失败", "Draft unavailable"));
        return;
      }

      const effectiveSpecRef = includeSpecDoc || trimmed.includes("@spec");
      const req: StudioCopilotRequest = {
        mode,
        prompt: trimmed,
        focus: {
          selected_node_type: selectedNodeName ?? null,
          selected_edge_type: selectedEdgeName ?? null,
          active_tab: activeInspectorTab || null,
        },
        ...(effectiveSpecRef || attachedDatasetIds.length > 0
          ? {
              refs: {
                ...(attachedDatasetIds.length > 0
                  ? { dataset_ids: attachedDatasetIds }
                  : {}),
                ...(effectiveSpecRef ? { include_spec_doc: true } : {}),
              },
            }
          : {}),
      };

      const res = await xblueApi.runDraftCopilot(draft.id, req);
      const nextIssues = Array.isArray(res?.issues) ? res.issues : draft.issues;
      const nextPass =
        typeof res?.pass_count === "number"
          ? res.pass_count
          : (draft.pass_count ?? 0);

      setLastResult({ draftId: draft.id, response: res });
      onDiagnosticsUpdated(nextPass, nextIssues);

      if (res?.mutated) {
        const refreshedDraft = await xblueApi
          .getDraft(draft.id)
          .catch(() => null);
        if (refreshedDraft && refreshedDraft.id) {
          onDraftUpdated(
            refreshedDraft,
            tx(
              `AI 改写已写入草案（前置快照 #${res.snapshot_no ?? 1}）`,
              `Applied draft patch (snapshot #${res.snapshot_no ?? 1})`
            )
          );
        }
        await refreshSnapshots(draft.id);
      }
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("助手调用失败", "Copilot request failed")
      );
    } finally {
      setRunning(false);
    }
  };

  const handleRestoreSnapshot = async (
    draftId: string,
    snapshotId: string,
    snapshotNo?: number | null
  ) => {
    if (!canEdit || restoringSnapId !== null) return;
    setRestoringSnapId(snapshotId);
    setErrorMsg(null);
    try {
      const restored = await xblueApi.restoreDraftSnapshot(draftId, snapshotId);
      const labelNo = snapshotNo ?? "";
      onDraftUpdated(
        restored,
        tx(
          `已回退至快照 ${labelNo ? `#${labelNo}` : ""}`.trim(),
          `Restored snapshot ${labelNo ? `#${labelNo}` : ""}`.trim()
        )
      );
      await refreshSnapshots(draftId);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("回退快照失败", "Snapshot rollback failed")
      );
    } finally {
      setRestoringSnapId(null);
    }
  };

  const renderFindingSeverityIcon = (finding: CopilotFinding) => {
    if (finding.severity === "ERROR") {
      return (
        <AlertCircle className="h-3.5 w-3.5 text-red-600 dark:text-red-400 shrink-0 mt-0.5" />
      );
    }
    if (finding.severity === "INFO") {
      return <Info className="h-3.5 w-3.5 text-xblue shrink-0 mt-0.5" />;
    }
    return (
      <AlertTriangle className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
    );
  };

  if (collapsed) {
    return (
      <Card
        data-testid="studio-copilot-rail"
        className="w-full lg:w-10 shrink-0 h-10 lg:h-full p-1.5 flex lg:flex-col items-center justify-between gap-2"
      >
        <IconButton
          size="sm"
          data-testid="studio-copilot-expand-btn"
          onClick={onToggleCollapsed}
          aria-label={tx("展开助手", "Expand Copilot")}
          title={tx("AI 助手", "Copilot")}
        >
          <PanelRightOpen className="h-4 w-4 text-xblue" />
        </IconButton>
        <Sparkles className="h-3.5 w-3.5 text-xblue hidden lg:block" />
      </Card>
    );
  }

  const findings = Array.isArray(lastResult?.response?.diagnosis?.findings)
    ? lastResult.response.diagnosis.findings
    : [];
  const diffCounts = countSchemaDiffChanges(lastResult?.response?.diff);

  return (
    <Card
      data-testid="studio-copilot-panel"
      className="w-full lg:w-[320px] xl:w-[348px] shrink-0 h-full min-h-0 p-3 flex flex-col gap-2.5 overflow-hidden text-xs"
    >
      {/* Header */}
      <div className="shrink-0 flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 font-semibold text-sm min-w-0">
          <Sparkles className="h-4 w-4 text-xblue shrink-0" />
          <span className="truncate">{tx("AI 助手", "Copilot")}</span>
        </div>
        <IconButton
          size="sm"
          data-testid="studio-copilot-collapse-btn"
          onClick={onToggleCollapsed}
          aria-label={tx("收起", "Collapse")}
          title={tx("收起", "Collapse")}
        >
          <PanelRightClose className="h-4 w-4" />
        </IconButton>
      </div>

      {/* Focus Context Bar */}
      <div
        data-testid="copilot-focus-bar"
        className="shrink-0 flex items-center justify-between gap-2 rounded-md border border-xblue/25 bg-xblue-soft/30 px-2.5 py-1.5"
      >
        <span className="inline-flex items-center gap-1.5 font-mono-tech font-medium text-xblue truncate">
          <Crosshair className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">{focusLabel}</span>
        </span>
      </div>

      {/* Mode Tabs */}
      <Tabs<CopilotActionMode>
        className="shrink-0"
        value={mode}
        onChange={setMode}
        items={[
          {
            key: "DIAGNOSE_ONLY",
            label: tx("诊断", "Diagnose"),
            testId: "copilot-mode-diagnose",
          },
          {
            key: "PATCH_DRAFT",
            label: tx("改写草案", "Patch Draft"),
            disabled: !canEdit,
            testId: "copilot-mode-patch",
          },
        ]}
      />

      {/* Quick Action Chips */}
      <div
        data-testid="copilot-quick-actions"
        className="shrink-0 flex flex-wrap gap-1.5"
      >
        {QUICK_ACTIONS.map((chip) => (
          <button
            key={chip.key}
            type="button"
            data-testid={`copilot-chip-${chip.key}`}
            disabled={!canEdit || running}
            onClick={() => setPrompt(chip.buildPrompt(focusTargetText, tx))}
            className="rounded-full border border-border bg-muted/40 px-2.5 py-1 text-[11px] font-medium text-foreground hover:border-xblue hover:bg-xblue-soft hover:text-xblue transition-colors disabled:opacity-45 cursor-pointer"
          >
            {tx(chip.zh, chip.en)}
          </button>
        ))}
      </div>

      {/* Context Attachment Pills (@spec + @dataset) */}
      <div className="shrink-0 space-y-1.5">
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            data-testid="copilot-ref-spec-toggle"
            aria-pressed={includeSpecDoc}
            onClick={() => setIncludeSpecDoc((v) => !v)}
            className={cn(
              "inline-flex h-7 items-center gap-1 rounded-md border px-2 font-mono-tech text-[11px] transition-colors cursor-pointer shrink-0",
              includeSpecDoc
                ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                : "border-border bg-background text-muted-foreground hover:text-foreground"
            )}
          >
            <FileText className="h-3 w-3" />
            <span>@spec</span>
          </button>

          <Select
            data-testid="copilot-dataset-select"
            value=""
            disabled={unattachedDatasets.length === 0}
            aria-label={tx("数据集挂载", "Attach Dataset")}
            onChange={(e) => {
              const nextId = e.target.value;
              if (nextId && !attachedDatasetIds.includes(nextId)) {
                setAttachedDatasetIds((prev) => [...prev, nextId]);
              }
            }}
            className="h-7 flex-1 min-w-0 font-mono-tech"
          >
            <option value="">@dataset</option>
            {unattachedDatasets.map((ds) => (
              <option key={ds.id} value={ds.id}>
                @{ds.name} ({ds.display_name || ds.name})
              </option>
            ))}
          </Select>
        </div>

        {(includeSpecDoc || attachedDatasets.length > 0) && (
          <div
            className="flex flex-wrap items-center gap-1"
            data-testid="copilot-attached-refs"
          >
            {includeSpecDoc && (
              <Badge variant="xblue" className="gap-1">
                <span>@spec</span>
                <button
                  type="button"
                  onClick={() => setIncludeSpecDoc(false)}
                  aria-label={tx("移除规格书引用", "Remove spec ref")}
                  className="hover:opacity-75 cursor-pointer"
                >
                  <X className="h-2.5 w-2.5" />
                </button>
              </Badge>
            )}
            {attachedDatasets.map((ds) => (
              <Badge
                key={ds.id}
                variant="xblue"
                data-testid={`copilot-dataset-pill-${ds.name}`}
                className="gap-1"
              >
                <Database className="h-2.5 w-2.5" />
                <span>@{ds.name}</span>
                <button
                  type="button"
                  onClick={() =>
                    setAttachedDatasetIds((prev) =>
                      prev.filter((id) => id !== ds.id)
                    )
                  }
                  aria-label={tx(`移除 ${ds.name}`, `Remove ${ds.name}`)}
                  className="hover:opacity-75 cursor-pointer"
                >
                  <X className="h-2.5 w-2.5" />
                </button>
              </Badge>
            ))}
          </div>
        )}
      </div>

      {/* Prompt Input + Send Button */}
      <div className="shrink-0 space-y-1.5">
        <Textarea
          rows={3}
          data-testid="copilot-prompt-input"
          value={prompt}
          disabled={!canEdit || running}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void handleSend();
            }
          }}
          placeholder="Warehouse.safety_stock (FLOAT64)"
          className="text-xs resize-none"
        />
        <div className="flex items-center justify-between gap-2">
          <Badge
            variant={mode === "PATCH_DRAFT" ? "warning" : "outline"}
            className="text-[10px]"
          >
            {mode === "PATCH_DRAFT"
              ? tx("前置快照保护", "Snapshot Guard")
              : tx("只读零副作用", "Read-Only")}
          </Badge>
          <Button
            type="button"
            size="sm"
            data-testid="copilot-send-btn"
            disabled={running || !prompt.trim() || !canEdit}
            onClick={() => void handleSend()}
          >
            {running ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Send className="h-3.5 w-3.5" />
            )}
            {running ? tx("执行中…", "Running…") : tx("发送", "Send")}
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div
          data-testid="copilot-error-banner"
          className="shrink-0 rounded border border-red-500/40 bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-600 dark:text-red-400"
        >
          {errorMsg}
        </div>
      )}

      {/* Scrollable Results & Snapshots Stream */}
      <div className="flex-1 min-h-0 overflow-y-auto space-y-2.5 pr-0.5 border-t border-border pt-2">
        {lastResult && (
          <div className="space-y-2" data-testid="copilot-result-section">
            {lastResult.response.mutated && (
              <div
                data-testid="copilot-patch-summary-card"
                className="rounded-md border border-amber-500/50 bg-amber-500/10 p-2.5 space-y-2"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-1.5 font-mono-tech text-[11px]">
                    <Badge variant="warning" className="tabular-nums">
                      #{lastResult.response.snapshot_no ?? 1}
                    </Badge>
                    <span className="tabular-nums font-semibold">
                      +{diffCounts.added} · ~{diffCounts.changed} · -
                      {diffCounts.removed}
                    </span>
                  </div>
                  {lastResult.response.snapshot_id && canEdit && (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid="copilot-rollback-latest-btn"
                      disabled={restoringSnapId !== null}
                      onClick={() => {
                        if (lastResult.response.snapshot_id) {
                          void handleRestoreSnapshot(
                            lastResult.draftId,
                            lastResult.response.snapshot_id,
                            lastResult.response.snapshot_no
                          );
                        }
                      }}
                      className="h-6 px-2"
                    >
                      <RotateCcw className="h-3 w-3" />
                      {tx("回退快照", "Rollback")}
                    </Button>
                  )}
                </div>
              </div>
            )}

            {lastResult.response.diagnosis?.summary && (
              <div className="rounded-md border border-border bg-muted/25 px-2.5 py-2 text-foreground leading-relaxed">
                {lastResult.response.diagnosis.summary}
              </div>
            )}

            {findings.length > 0 && (
              <div className="space-y-1.5" data-testid="copilot-findings-list">
                {findings.map((finding, idx) => (
                  <div
                    key={`${finding.target_path}-${idx}`}
                    data-testid={`copilot-finding-${idx}`}
                    className="rounded-md border border-border bg-background p-2 space-y-1"
                  >
                    <div className="flex items-start justify-between gap-1.5">
                      <div className="flex items-start gap-1.5 min-w-0">
                        {renderFindingSeverityIcon(finding)}
                        <button
                          type="button"
                          onClick={() =>
                            onLocateTargetPath?.(finding.target_path)
                          }
                          className="font-mono-tech text-[11px] font-semibold text-xblue hover:underline truncate text-left cursor-pointer"
                        >
                          {finding.target_path || "schema_doc"}
                        </button>
                      </div>
                      <Badge
                        variant={
                          finding.severity === "ERROR"
                            ? "danger"
                            : finding.severity === "INFO"
                              ? "xblue"
                              : "warning"
                        }
                        className="text-[10px]"
                      >
                        {finding.severity}
                      </Badge>
                    </div>
                    <div className="text-foreground leading-snug">
                      {finding.rationale}
                    </div>
                    {finding.suggested_fix && (
                      <div className="rounded bg-muted/50 px-2 py-1 text-[11px] text-muted-foreground font-mono-tech">
                        {finding.suggested_fix}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Recent Draft Snapshots */}
        <div className="space-y-1.5" data-testid="copilot-snapshots-section">
          <div className="flex items-center justify-between text-[11px] text-muted-foreground">
            <span className="inline-flex items-center gap-1 font-medium">
              <History className="h-3 w-3 text-xblue" />
              {tx("草案快照", "Snapshots")}
            </span>
            <span className="font-mono-tech tabular-nums">
              {snapshots.length}
            </span>
          </div>

          {snapshots.length === 0 ? (
            <div className="rounded border border-dashed border-border py-3 px-2 text-center text-[11px] text-muted-foreground">
              {tx("暂无快照记录", "No snapshots")}
            </div>
          ) : (
            <div className="space-y-1">
              {snapshots.map((snap) => (
                <div
                  key={snap.id}
                  data-testid={`copilot-snapshot-row-${snap.snapshot_no}`}
                  className="flex items-center justify-between gap-2 rounded border border-border bg-background px-2 py-1.5"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-1.5">
                      <span className="font-mono-tech font-semibold text-xblue tabular-nums">
                        #{snap.snapshot_no}
                      </span>
                      <Badge variant="outline" className="text-[10px]">
                        {formatReason(snap.reason)}
                      </Badge>
                    </div>
                    {snap.created_at && (
                      <div className="text-[10px] font-mono-tech text-muted-foreground truncate">
                        {fmtTime(snap.created_at, true)}
                      </div>
                    )}
                  </div>
                  {canEdit && activeDraft?.id && (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      data-testid={`copilot-restore-snapshot-${snap.snapshot_no}`}
                      disabled={restoringSnapId !== null}
                      onClick={() =>
                        void handleRestoreSnapshot(
                          activeDraft.id,
                          snap.id,
                          snap.snapshot_no
                        )
                      }
                      className="h-6 px-2 shrink-0"
                    >
                      <RotateCcw className="h-3 w-3" />
                      {restoringSnapId === snap.id
                        ? tx("回退中…", "Restoring…")
                        : tx("回退快照", "Rollback")}
                    </Button>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </Card>
  );
}
