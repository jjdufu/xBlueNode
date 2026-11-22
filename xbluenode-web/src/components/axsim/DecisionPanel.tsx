import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  GitPullRequestArrow,
  RefreshCw,
  Send,
  ShieldCheck,
} from "lucide-react";
import {
  axsimApi,
  type DecisionChangeInput,
  type DecisionRecord,
  type DecisionStatus,
  type SandboxGraphNode,
  type ScenarioRecord,
} from "@/lib/api/axsim";
import { axflowApi, type FlowRecord } from "@/lib/api/axflow";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";

export interface DecisionPanelProps {
  scenario: ScenarioRecord | null;
  nodes: SandboxGraphNode[];
  onDecisionTracked?: (decision: DecisionRecord) => void;
}

interface DraftChangeItem {
  id: string;
  included: boolean;
  node_id: string;
  metric: string;
  base_value: number | null;
  target_value: string;
}

export function DecisionPanel({
  scenario,
  nodes,
  onDecisionTracked,
}: DecisionPanelProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canSimApply = can("SIM_APPLY");

  const [title, setTitle] = useState("");
  const [toleranceRatio, setToleranceRatio] = useState("0.05");
  const [triggerFlowId, setTriggerFlowId] = useState("");
  const [flows, setFlows] = useState<FlowRecord[]>([]);
  const [draftChanges, setDraftChanges] = useState<DraftChangeItem[]>([]);
  const [useExplicitChanges, setUseExplicitChanges] = useState(false);

  const [decisions, setDecisions] = useState<DecisionRecord[]>([]);
  const [activeDecisionId, setActiveDecisionId] = useState<string | null>(null);

  const [submitting, setSubmitting] = useState(false);
  const [refreshingId, setRefreshingId] = useState<string | null>(null);
  const [rebasingId, setRebasingId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const statusMeta: Record<
    DecisionStatus,
    { zh: string; en: string; className: string }
  > = {
    PENDING_APPROVAL: {
      zh: "待审批",
      en: "Awaiting Approval",
      className:
        "border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    },
    APPROVED: {
      zh: "已批准",
      en: "Approved",
      className: "border-xblue/50 bg-xblue-soft text-xblue",
    },
    APPLIED: {
      zh: "已写入生产",
      en: "Applied",
      className:
        "border-emerald-500/50 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    },
    REJECTED: {
      zh: "已驳回",
      en: "Rejected",
      className:
        "border-red-500/50 bg-red-500/10 text-red-700 dark:text-red-300",
    },
    CONFLICT: {
      zh: "基线冲突",
      en: "Conflict",
      className:
        "border-red-500/60 bg-red-500/15 text-red-700 dark:text-red-300",
    },
    FAILED: {
      zh: "下发失败",
      en: "Failed",
      className:
        "border-red-500/50 bg-red-500/10 text-red-700 dark:text-red-300",
    },
  };

  const nodeMap = useMemo(() => {
    const map = new Map<string, SandboxGraphNode>();
    for (const n of nodes) {
      map.set(n.id, n);
    }
    return map;
  }, [nodes]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const res = await axflowApi.listFlows({ limit: 50 });
        if (!cancelled && Array.isArray(res?.items)) {
          setFlows(res.items);
        }
      } catch {
        // ignore if AxFlow unavailable
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!scenario) {
      setTitle("");
      setDraftChanges([]);
      return;
    }
    setTitle(tx(`采纳方案：${scenario.name}`, `Adopt: ${scenario.name}`));
    const derived: DraftChangeItem[] = (scenario.actions ?? []).map(
      (act, idx) => {
        const node = nodeMap.get(act.target_node_id);
        const baseVal =
          node && typeof node.metrics?.[act.metric_key] === "number"
            ? node.metrics[act.metric_key]
            : null;
        let targetVal = 0;
        if (act.mode === "ABSOLUTE" && typeof act.value === "number") {
          targetVal = act.value;
        } else if (act.mode === "ADD" && typeof act.value === "number") {
          targetVal = (baseVal ?? 0) + act.value;
        } else if (
          (act.mode === "RATIO" || act.delta_ratio != null) &&
          baseVal !== null
        ) {
          const ratio =
            typeof act.value === "number" ? act.value : (act.delta_ratio ?? 0);
          targetVal = baseVal * (1 + ratio);
        } else if (typeof act.value === "number") {
          targetVal = act.value;
        }

        return {
          id: `chg-${idx}-${act.target_node_id}-${act.metric_key}`,
          included: true,
          node_id: act.target_node_id,
          metric: act.metric_key,
          base_value: baseVal,
          target_value: String(
            Number.isFinite(targetVal) ? Number(targetVal.toFixed(4)) : 0
          ),
        };
      }
    );
    setDraftChanges(derived);
  }, [nodeMap, scenario, tx]);

  const activeDecision = useMemo(
    () =>
      decisions.find((d) => d.id === activeDecisionId) ?? decisions[0] ?? null,
    [activeDecisionId, decisions]
  );

  const upsertDecision = (rec: DecisionRecord) => {
    setDecisions((prev) => {
      const idx = prev.findIndex((d) => d.id === rec.id);
      if (idx === -1) return [rec, ...prev];
      return prev.map((d) => (d.id === rec.id ? rec : d));
    });
    setActiveDecisionId(rec.id);
    onDecisionTracked?.(rec);
  };

  const handleCreateDecision = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!scenario || !canSimApply) return;
    if (!title.trim()) {
      setErrorMsg(tx("请填写决策提案标题", "Proposal title is required"));
      return;
    }
    const tol = Number.parseFloat(toleranceRatio);
    setSubmitting(true);
    setErrorMsg(null);
    try {
      let changesPayload: DecisionChangeInput[] | undefined;
      if (useExplicitChanges) {
        changesPayload = draftChanges
          .filter((c) => c.included)
          .map((c) => ({
            node_id: c.node_id,
            metric: c.metric,
            target_value: Number.parseFloat(c.target_value) || 0,
          }));
      }

      const created = await axsimApi.createDecision(scenario.id, {
        title: title.trim(),
        ...(changesPayload && changesPayload.length > 0
          ? { changes: changesPayload }
          : {}),
        tolerance_ratio: Number.isFinite(tol) && tol >= 0 ? tol : 0,
        trigger_flow_id: triggerFlowId.trim() || null,
      });
      upsertDecision(created);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleRefreshDecision = async (decisionId: string) => {
    setRefreshingId(decisionId);
    setErrorMsg(null);
    try {
      const latest = await axsimApi.getDecision(decisionId);
      upsertDecision(latest);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshingId(null);
    }
  };

  const handleRebaseDecision = async (decisionId: string) => {
    if (!canSimApply) return;
    setRebasingId(decisionId);
    setErrorMsg(null);
    try {
      const rebased = await axsimApi.rebaseDecision(decisionId);
      upsertDecision(rebased);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setRebasingId(null);
    }
  };

  if (!scenario) {
    return (
      <Card
        className="p-4 text-xs text-muted-foreground"
        data-testid="decision-panel-empty"
      >
        {tx(
          "请先运行或选择一个推演场景",
          "Run or select a simulation scenario first"
        )}
      </Card>
    );
  }

  return (
    <div className="space-y-4" data-testid="decision-panel">
      <Card className="p-4 space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
          <div className="flex items-center gap-2 text-sm font-medium">
            <ShieldCheck className="h-4 w-4 text-xblue" />
            <span>{tx("决策提案与生产回写", "Decision Proposal & Writeback")}</span>
          </div>
          <Badge variant="outline">
            {tx("场景: ", "Scenario: ")}
            {scenario.name}
          </Badge>
        </div>

        {errorMsg && (
          <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
            {errorMsg}
          </div>
        )}

        <form
          onSubmit={(e) => void handleCreateDecision(e)}
          className="space-y-3"
        >
          <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("提案标题", "Proposal Title")}
              </label>
              <Input
                value={title}
                disabled={!canSimApply}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={tx(
                  "下调 F-1 产能并切换备选仓",
                  "Cut F-1 output and switch backup warehouse"
                )}
                className="h-9 text-xs"
                data-testid="decision-title-input"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("基线漂移容差", "Drift Tolerance")}
              </label>
              <Input
                type="number"
                step="any"
                min="0"
                disabled={!canSimApply}
                value={toleranceRatio}
                onChange={(e) => setToleranceRatio(e.target.value)}
                className="h-9 text-xs font-mono-tech"
                data-testid="decision-tolerance-input"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                {tx("联动编排流程", "Linked Flow")}
              </label>
              <select
                value={triggerFlowId}
                disabled={!canSimApply}
                onChange={(e) => setTriggerFlowId(e.target.value)}
                className="h-9 w-full rounded-md border border-input bg-background px-2 text-xs disabled:opacity-50"
                data-testid="decision-flow-select"
              >
                <option value="">{tx("不联动流程", "None")}</option>
                {flows.map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.name} ({tEnum(f.status)})
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="space-y-2 rounded-md border border-border bg-muted/20 p-3">
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
              <span className="font-medium">
                {tx("拟下发变更项", "Proposed Metric Changes")}
              </span>
              {canSimApply && (
                <label className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground cursor-pointer">
                  <input
                    type="checkbox"
                    checked={useExplicitChanges}
                    onChange={(e) => setUseExplicitChanges(e.target.checked)}
                  />
                  <span>{tx("自定义目标值", "Customize target values")}</span>
                </label>
              )}
            </div>

            {draftChanges.length === 0 ? (
              <div className="text-[11px] text-muted-foreground">
                {tx(
                  "自动按当前场景干预动作推导目标值",
                  "Target values derived automatically from scenario interventions"
                )}
              </div>
            ) : (
              <div className="space-y-1.5">
                {draftChanges.map((item) => {
                  const node = nodeMap.get(item.node_id);
                  return (
                    <div
                      key={item.id}
                      className="flex flex-wrap items-center justify-between gap-2 rounded border border-border bg-background px-2.5 py-1.5 text-xs"
                    >
                      <div className="flex items-center gap-2">
                        {useExplicitChanges && (
                          <input
                            type="checkbox"
                            checked={item.included}
                            onChange={(e) =>
                              setDraftChanges((prev) =>
                                prev.map((row) =>
                                  row.id === item.id
                                    ? { ...row, included: e.target.checked }
                                    : row
                                )
                              )
                            }
                          />
                        )}
                        <span className="font-medium">
                          {node
                            ? `[${node.type_name}] ${node.title}`
                            : item.node_id}
                        </span>
                        <Badge variant="outline">{item.metric}</Badge>
                      </div>

                      <div className="flex items-center gap-2 font-mono-tech text-[11px]">
                        <span className="text-muted-foreground">
                          {tx("基线: ", "Base: ")}
                          {fmtNum(item.base_value, 4)}
                        </span>
                        <span>→ {tx("目标:", "Target:")}</span>
                        {useExplicitChanges ? (
                          <Input
                            type="number"
                            step="any"
                            value={item.target_value}
                            onChange={(e) =>
                              setDraftChanges((prev) =>
                                prev.map((row) =>
                                  row.id === item.id
                                    ? { ...row, target_value: e.target.value }
                                    : row
                                )
                              )
                            }
                            className="h-7 w-24 text-xs font-mono-tech"
                          />
                        ) : (
                          <span className="font-semibold text-xblue">
                            {item.target_value}
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {canSimApply && (
            <div className="flex justify-end">
              <Button
                type="submit"
                disabled={submitting}
                data-testid="create-decision-btn"
              >
                <Send className="h-3.5 w-3.5" />
                {submitting
                  ? tx("提交中…", "Submitting…")
                  : tx("创建提案", "Create Proposal")}
              </Button>
            </div>
          )}
        </form>
      </Card>

      {/* Active decision detail & conflict/rebase */}
      {activeDecision && (
        <Card className="p-4 space-y-3" data-testid="decision-detail-card">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-medium">{activeDecision.title}</span>
              <span
                data-testid="decision-status-badge"
                className={`inline-flex items-center rounded border px-2 py-0.5 text-[11px] font-mono-tech font-medium ${
                  statusMeta[activeDecision.status]?.className ?? ""
                }`}
              >
                {statusMeta[activeDecision.status]
                  ? `${tx(statusMeta[activeDecision.status].zh, statusMeta[activeDecision.status].en)} · ${activeDecision.status}`
                  : activeDecision.status}
              </span>
            </div>

            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void handleRefreshDecision(activeDecision.id)}
                disabled={refreshingId === activeDecision.id}
                data-testid="decision-refresh-btn"
              >
                <RefreshCw
                  className={`h-3 w-3 ${
                    refreshingId === activeDecision.id ? "animate-spin" : ""
                  }`}
                />
                {tx("刷新状态", "Refresh")}
              </Button>

              {activeDecision.status === "CONFLICT" && canSimApply && (
                <Button
                  type="button"
                  size="sm"
                  onClick={() => void handleRebaseDecision(activeDecision.id)}
                  disabled={rebasingId === activeDecision.id}
                  data-testid="decision-rebase-btn"
                >
                  <GitPullRequestArrow className="h-3.5 w-3.5" />
                  {rebasingId === activeDecision.id
                    ? tx("变基中…", "Rebasing…")
                    : tx("变基重提", "Rebase")}
                </Button>
              )}
            </div>
          </div>

          <div className="flex flex-wrap gap-3 text-[11px] font-mono-tech text-muted-foreground">
            <span>
              {tx("决策 ID", "Decision ID")}: {activeDecision.id}
            </span>
            {activeDecision.approval_id && (
              <span>
                {tx("审批单", "Approval")}: {activeDecision.approval_id}
              </span>
            )}
            <span>
              {tx("容差", "Tolerance")}:{" "}
              {(activeDecision.tolerance_ratio * 100).toFixed(1)}%
            </span>
            {activeDecision.trigger_flow_id && (
              <span>
                {tx("联动流程", "Flow")}: {activeDecision.trigger_flow_id}
              </span>
            )}
          </div>

          {activeDecision.status === "APPLIED" && (
            <div className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-700 dark:text-emerald-300 flex items-center gap-2">
              <CheckCircle2 className="h-4 w-4 shrink-0" />
              <span>
                {tx(
                  "决策已通过审批并原子写入生产主图节点指标",
                  "Approved and atomically written back to production graph metrics"
                )}
              </span>
            </div>
          )}

          {/* Conflict Diff Table */}
          {(activeDecision.status === "CONFLICT" ||
            (activeDecision.conflict_report &&
              activeDecision.conflict_report.length > 0)) && (
            <div
              className="rounded-md border border-red-500/40 bg-red-500/5 p-3 space-y-2"
              data-testid="decision-conflict-panel"
            >
              <div className="flex items-center gap-1.5 text-xs font-semibold text-red-700 dark:text-red-300">
                <AlertTriangle className="h-4 w-4" />
                <span>
                  {tx(
                    "检测到生产基线漂移超过容差阈值，写入已中止，请核对差异并变基重提：",
                    "Production baseline drifted beyond tolerance; writeback halted. Review diff and rebase:"
                  )}
                </span>
              </div>

              <div className="overflow-x-auto">
                <table
                  className="w-full text-xs"
                  data-testid="decision-conflict-table"
                >
                  <thead>
                    <tr className="border-b border-border text-left text-muted-foreground">
                      <th className="py-1.5 pr-3">{tx("节点", "Node")}</th>
                      <th className="py-1.5 pr-3">{tx("指标", "Metric")}</th>
                      <th className="py-1.5 pr-3 text-right">
                        {tx("沙盘基线", "Sandbox Base")}
                      </th>
                      <th className="py-1.5 pr-3 text-right">
                        {tx("当前生产值", "Current Prod")}
                      </th>
                      <th className="py-1.5 pr-3 text-right">
                        {tx("拟写入目标", "Target")}
                      </th>
                      <th className="py-1.5 pr-3 text-right">
                        {tx("漂移幅度", "Drift")}
                      </th>
                      <th className="py-1.5">{tx("冲突原因", "Reason")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {activeDecision.conflict_report.map((c, idx) => {
                      const node = nodeMap.get(c.node_id);
                      return (
                        <tr
                          key={`${c.node_id}-${c.metric}-${idx}`}
                          className="border-b border-border/60"
                        >
                          <td className="py-1.5 pr-3 font-medium">
                            {node ? node.title : c.node_id}
                          </td>
                          <td className="py-1.5 pr-3 font-mono-tech">
                            {c.metric}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech">
                            {fmtNum(c.base_value, 4)}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech font-semibold text-red-600 dark:text-red-400">
                            {c.current_value ?? "-"}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech">
                            {fmtNum(c.target_value, 4)}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech text-red-600 dark:text-red-400">
                            {(c.drift_ratio * 100).toFixed(1)}%
                          </td>
                          <td className="py-1.5 text-muted-foreground">
                            {c.reason}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          )}

          {/* Decision Changes Table */}
          {Array.isArray(activeDecision.changes) &&
            activeDecision.changes.length > 0 && (
              <div className="overflow-x-auto">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="border-b border-border text-left text-muted-foreground">
                      <th className="py-1.5 pr-3">
                        {tx("目标节点", "Target Node")}
                      </th>
                      <th className="py-1.5 pr-3">{tx("指标", "Metric")}</th>
                      <th className="py-1.5 pr-3 text-right">
                        {tx("提案基线值", "Proposal Base")}
                      </th>
                      <th className="py-1.5 text-right">
                        {tx("目标下发值", "Target Value")}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {activeDecision.changes.map((chg, idx) => {
                      const node = nodeMap.get(chg.node_id);
                      return (
                        <tr
                          key={`${chg.node_id}-${chg.metric}-${idx}`}
                          className="border-b border-border/60"
                        >
                          <td className="py-1.5 pr-3 font-medium">
                            {node
                              ? `${node.title} (${node.node_key})`
                              : chg.node_id}
                          </td>
                          <td className="py-1.5 pr-3 font-mono-tech">
                            {chg.metric}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech text-muted-foreground">
                            {fmtNum(chg.base_value, 4)}
                          </td>
                          <td className="py-1.5 text-right font-mono-tech font-semibold text-xblue">
                            {fmtNum(chg.target_value, 4)}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
        </Card>
      )}

      {decisions.length > 1 && (
        <div className="flex flex-wrap items-center gap-1.5 text-xs">
          <span className="text-muted-foreground">
            {tx("历史提案:", "History:")}
          </span>
          {decisions.map((d) => (
            <button
              key={d.id}
              type="button"
              onClick={() => setActiveDecisionId(d.id)}
              className={`rounded border px-2 py-1 text-[11px] cursor-pointer ${
                activeDecision?.id === d.id
                  ? "border-xblue bg-xblue-soft text-xblue"
                  : "border-border bg-background"
              }`}
            >
              {d.title} ({d.status})
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
