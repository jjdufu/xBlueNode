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

export interface DecisionPanelProps {
  scenario: ScenarioRecord | null;
  nodes: SandboxGraphNode[];
}

interface DraftChangeItem {
  id: string;
  included: boolean;
  node_id: string;
  metric: string;
  base_value: number | null;
  target_value: string;
}

const STATUS_BADGE_MAP: Record<
  DecisionStatus,
  { label: string; className: string }
> = {
  PENDING_APPROVAL: {
    label: "待审批 (PENDING_APPROVAL)",
    className: "border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  },
  APPROVED: {
    label: "已批准 (APPROVED)",
    className: "border-xblue/50 bg-xblue-soft text-xblue",
  },
  APPLIED: {
    label: "已写入生产 (APPLIED)",
    className: "border-emerald-500/50 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  },
  REJECTED: {
    label: "已驳回 (REJECTED)",
    className: "border-red-500/50 bg-red-500/10 text-red-700 dark:text-red-300",
  },
  CONFLICT: {
    label: "基线冲突 (CONFLICT)",
    className: "border-red-500/60 bg-red-500/15 text-red-700 dark:text-red-300",
  },
  FAILED: {
    label: "下发失败 (FAILED)",
    className: "border-red-500/50 bg-red-500/10 text-red-700 dark:text-red-300",
  },
};

export function DecisionPanel({ scenario, nodes }: DecisionPanelProps) {
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

  // Derive draft decision changes whenever scenario changes
  useEffect(() => {
    if (!scenario) {
      setTitle("");
      setDraftChanges([]);
      return;
    }
    setTitle(`采纳方案：${scenario.name}`);
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
  }, [nodeMap, scenario]);

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
  };

  const handleCreateDecision = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!scenario) return;
    if (!title.trim()) {
      setErrorMsg("请输入决策提案标题");
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
      <Card className="p-4 text-xs text-muted-foreground" data-testid="decision-panel-empty">
        请先运行并选择一个推演场景，再发起决策下发提案。
      </Card>
    );
  }

  return (
    <div className="space-y-4" data-testid="decision-panel">
      <Card className="p-4 space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
          <div className="flex items-center gap-2 text-sm font-medium">
            <ShieldCheck className="h-4 w-4 text-xblue" />
            <span>决策提案与生产回写（审批门禁 · 冲突检测）</span>
          </div>
          <Badge variant="outline">场景: {scenario.name}</Badge>
        </div>

        {errorMsg && (
          <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
            {errorMsg}
          </div>
        )}

        <form onSubmit={(e) => void handleCreateDecision(e)} className="space-y-3">
          <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
            <div className="space-y-1">
              <label className="text-muted-foreground">决策提案标题</label>
              <Input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="例如：下调 F-1 产能并切换备选仓"
                className="h-9 text-xs"
                data-testid="decision-title-input"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                基线漂移容差 (tolerance_ratio)
              </label>
              <Input
                type="number"
                step="any"
                min="0"
                value={toleranceRatio}
                onChange={(e) => setToleranceRatio(e.target.value)}
                className="h-9 text-xs font-mono-tech"
                data-testid="decision-tolerance-input"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground">
                审批后联动触发 AxFlow (可选)
              </label>
              <select
                value={triggerFlowId}
                onChange={(e) => setTriggerFlowId(e.target.value)}
                className="h-9 w-full rounded-md border border-input bg-background px-2 text-xs"
                data-testid="decision-flow-select"
              >
                <option value="">不触发联动流程</option>
                {flows.map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.name} ({f.status})
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="space-y-2 rounded-md border border-border bg-muted/20 p-3">
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
              <span className="font-medium">拟下发变更项清单</span>
              <label className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground cursor-pointer">
                <input
                  type="checkbox"
                  checked={useExplicitChanges}
                  onChange={(e) => setUseExplicitChanges(e.target.checked)}
                />
                <span>自定义调整目标值（不勾选则自动由场景干预推导）</span>
              </label>
            </div>

            {draftChanges.length === 0 ? (
              <div className="text-[11px] text-muted-foreground">
                将使用当前场景的干预动作自动换算下发变更。
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
                          {node ? `[${node.type_name}] ${node.title}` : item.node_id}
                        </span>
                        <Badge variant="outline">{item.metric}</Badge>
                      </div>

                      <div className="flex items-center gap-2 font-mono-tech text-[11px]">
                        <span className="text-muted-foreground">
                          沙盘基线: {item.base_value ?? "—"}
                        </span>
                        <span>→ 目标值:</span>
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

          <div className="flex justify-end">
            <Button
              type="submit"
              disabled={submitting}
              data-testid="create-decision-btn"
            >
              <Send className="h-3.5 w-3.5" />
              {submitting ? "提交提案中..." : "创建决策提案"}
            </Button>
          </div>
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
                  STATUS_BADGE_MAP[activeDecision.status]?.className ?? ""
                }`}
              >
                {STATUS_BADGE_MAP[activeDecision.status]?.label ??
                  activeDecision.status}
              </span>
            </div>

            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="outline"
                className="h-7 px-2.5 text-xs"
                onClick={() => void handleRefreshDecision(activeDecision.id)}
                disabled={refreshingId === activeDecision.id}
                data-testid="decision-refresh-btn"
              >
                <RefreshCw
                  className={`h-3 w-3 ${
                    refreshingId === activeDecision.id ? "animate-spin" : ""
                  }`}
                />
                刷新审批状态
              </Button>

              {activeDecision.status === "CONFLICT" && (
                <Button
                  type="button"
                  onClick={() => void handleRebaseDecision(activeDecision.id)}
                  disabled={rebasingId === activeDecision.id}
                  className="h-7 px-2.5 text-xs"
                  data-testid="decision-rebase-btn"
                >
                  <GitPullRequestArrow className="h-3.5 w-3.5" />
                  {rebasingId === activeDecision.id
                    ? "变基中..."
                    : "Rebase 变基重提"}
                </Button>
              )}
            </div>
          </div>

          <div className="flex flex-wrap gap-3 text-[11px] font-mono-tech text-muted-foreground">
            <span>决策 ID: {activeDecision.id}</span>
            {activeDecision.approval_id && (
              <span>审批单 ID: {activeDecision.approval_id}</span>
            )}
            <span>容差: {(activeDecision.tolerance_ratio * 100).toFixed(1)}%</span>
            {activeDecision.trigger_flow_id && (
              <span>联动流程: {activeDecision.trigger_flow_id}</span>
            )}
          </div>

          {activeDecision.status === "APPLIED" && (
            <div className="rounded-md border border-emerald-500/40 bg-emerald-500/10 px-3 py-2 text-xs text-emerald-700 dark:text-emerald-300 flex items-center gap-2">
              <CheckCircle2 className="h-4 w-4 shrink-0" />
              <span>
                决策已通过审批并原子写入生产主图节点指标（已记录 SIM_APPLY 审计）。
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
                  检测到生产基线漂移冲突（超过容差阈值），已中止写入生产。请核对差异并点击「Rebase 变基重提」：
                </span>
              </div>

              <div className="overflow-x-auto">
                <table className="w-full text-xs" data-testid="decision-conflict-table">
                  <thead>
                    <tr className="border-b border-border text-left text-muted-foreground">
                      <th className="py-1.5 pr-3">节点</th>
                      <th className="py-1.5 pr-3">指标</th>
                      <th className="py-1.5 pr-3 text-right">沙盘基线</th>
                      <th className="py-1.5 pr-3 text-right">当前生产值</th>
                      <th className="py-1.5 pr-3 text-right">拟写入目标</th>
                      <th className="py-1.5 pr-3 text-right">漂移幅度</th>
                      <th className="py-1.5">冲突原因</th>
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
                          <td className="py-1.5 pr-3 font-mono-tech">{c.metric}</td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech">
                            {c.base_value ?? "—"}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech font-semibold text-red-600 dark:text-red-400">
                            {c.current_value ?? "—"}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech">
                            {c.target_value}
                          </td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech text-red-600 dark:text-red-400">
                            {(c.drift_ratio * 100).toFixed(1)}%
                          </td>
                          <td className="py-1.5 text-muted-foreground">{c.reason}</td>
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
                      <th className="py-1.5 pr-3">目标节点</th>
                      <th className="py-1.5 pr-3">指标</th>
                      <th className="py-1.5 pr-3 text-right">提案基线值</th>
                      <th className="py-1.5 text-right">目标下发值</th>
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
                            {node ? `${node.title} (${node.node_key})` : chg.node_id}
                          </td>
                          <td className="py-1.5 pr-3 font-mono-tech">{chg.metric}</td>
                          <td className="py-1.5 pr-3 text-right font-mono-tech text-muted-foreground">
                            {chg.base_value ?? "—"}
                          </td>
                          <td className="py-1.5 text-right font-mono-tech font-semibold text-xblue">
                            {chg.target_value}
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
          <span className="text-muted-foreground">历史提案:</span>
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
