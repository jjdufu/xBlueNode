import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router";
import {
  ExternalLink,
  GitBranch,
  GitCompare,
  Kanban,
  Layers,
  Play,
  Plus,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  axsimApi,
  type AxSandboxRecord,
  type AxSimAction,
  type AxSimQuickResult,
  type Checkpoint,
  type DecisionRecord,
  type ImpactRow,
  type SandboxGraphLink,
  type SandboxGraphNode,
  type ScenarioRecord,
} from "@/lib/api/axsim";
import { axopsApi } from "@/lib/api/axops";
import { ActionList } from "@/components/axsim/ActionList";
import { DecisionPanel } from "@/components/axsim/DecisionPanel";
import { ImpactGraph } from "@/components/axsim/ImpactGraph";
import { ImpactTable } from "@/components/axsim/ImpactTable";
import { OverlayEditor } from "@/components/axsim/OverlayEditor";
import { ReplayTimeline } from "@/components/axsim/ReplayTimeline";
import { SandboxWizard } from "@/components/axsim/SandboxWizard";
import { ScenarioCompare } from "@/components/axsim/ScenarioCompare";
import { SimAsk } from "@/components/axsim/SimAsk";
import {
  Badge,
  Button,
  Card,
  EmptyState,
  PageHeader,
  Select,
  StatusBadge,
  Tabs,
  type TabOption,
} from "@/components/ui/primitives";
import { ResizableWorkspace } from "@/components/ui/ResizableWorkspace";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { MODULE_META } from "@/modules/meta";

type WorkbenchTab = "topology" | "simulate" | "compare" | "decisions";

interface TrackedSimDecision {
  readonly id: string;
  readonly title: string;
  readonly scenarioId: string;
}

function normalizeSandbox(raw: AxSandboxRecord): AxSandboxRecord {
  return {
    ...raw,
    status: raw.status ?? "READY",
    scope: raw.scope ?? {},
    node_count: typeof raw.node_count === "number" ? raw.node_count : 0,
    link_count: typeof raw.link_count === "number" ? raw.link_count : 0,
    actions: Array.isArray(raw.actions) ? raw.actions : [],
    sim_result: raw.sim_result ?? null,
  };
}

function resolveSimTab(raw: string | null): WorkbenchTab {
  const val = (raw ?? "").trim().toLowerCase();
  if (val === "topology" || val === "overlay") return "topology";
  if (val === "compare" || val === "ask") return "compare";
  if (val === "decisions") return "decisions";
  return "simulate";
}

export function AxSimPage() {
  const { tx, tEnum, fmtNum } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab");
  const sandboxParam = searchParams.get("sandbox");
  const { can } = usePrincipal();
  const canSimRun = can("SIM_RUN");
  const canSimApply = can("SIM_APPLY");

  const [sandboxes, setSandboxes] = useState<AxSandboxRecord[]>([]);
  const [selectedSandboxId, setSelectedSandboxId] = useState<string>(
    () => sandboxParam ?? ""
  );
  const [wizardOpen, setWizardOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<WorkbenchTab>(() =>
    resolveSimTab(tabParam)
  );

  useEffect(() => {
    if (tabParam) {
      setActiveTab(resolveSimTab(tabParam));
    }
  }, [tabParam]);

  useEffect(() => {
    if (sandboxParam) {
      setSelectedSandboxId(sandboxParam);
    }
  }, [sandboxParam]);

  const handleChangeTab = (next: WorkbenchTab) => {
    setActiveTab(next);
    const updated = new URLSearchParams(searchParams);
    if (next === "simulate") {
      updated.delete("tab");
    } else {
      updated.set("tab", next);
    }
    setSearchParams(updated, { replace: true });
  };

  // Merged sandbox graph state
  const [graphNodes, setGraphNodes] = useState<SandboxGraphNode[]>([]);
  const [graphLinks, setGraphLinks] = useState<SandboxGraphLink[]>([]);
  const [overlayHash, setOverlayHash] = useState<string>("");
  const [graphLoading, setGraphLoading] = useState(false);

  // Scenarios & simulation state
  const [scenarios, setScenarios] = useState<ScenarioRecord[]>([]);
  const [selectedScenarioId, setSelectedScenarioId] = useState<string>("");
  const [quickResult, setQuickResult] = useState<AxSimQuickResult | null>(null);
  const [quickActions, setQuickActions] = useState<AxSimAction[]>([]);

  // Graph & replay interaction state
  const [loadedImpacts, setLoadedImpacts] = useState<ImpactRow[]>([]);
  const [highlightedPath, setHighlightedPath] = useState<string[]>([]);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [activeCheckpoint, setActiveCheckpoint] = useState<Checkpoint | null>(
    null
  );

  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [trackedDecisions, setTrackedDecisions] = useState<
    TrackedSimDecision[]
  >([]);
  const [convertingRef, setConvertingRef] = useState<string | null>(null);
  const [createdWorkItemCode, setCreatedWorkItemCode] = useState<string | null>(
    null
  );

  const formatScopeLabel = useCallback(
    (sb: AxSandboxRecord | null): string => {
      if (!sb?.scope) return tx("全量图谱", "Full Graph");
      const parts: string[] = [];
      if (
        Array.isArray(sb.scope.root_node_ids) &&
        sb.scope.root_node_ids.length > 0
      ) {
        parts.push(
          tx(
            `${sb.scope.root_node_ids.length} 种子`,
            `${sb.scope.root_node_ids.length} seeds`
          )
        );
      }
      if (sb.scope.depth != null) {
        parts.push(tx(`深度 ${sb.scope.depth}`, `Depth ${sb.scope.depth}`));
      }
      if (sb.scope.direction) {
        parts.push(tEnum(sb.scope.direction));
      }
      if (
        Array.isArray(sb.scope.type_names) &&
        sb.scope.type_names.length > 0
      ) {
        parts.push(sb.scope.type_names.join(","));
      }
      return parts.length > 0
        ? parts.join(" · ")
        : tx("全量图谱", "Full Graph");
    },
    [tEnum, tx]
  );

  const handleDecisionTracked = useCallback(
    (dec: DecisionRecord) => {
      setTrackedDecisions((prev) => {
        if (prev.some((d) => d.id === dec.id)) {
          return prev;
        }
        return [
          {
            id: dec.id,
            title: dec.title,
            scenarioId: dec.scenario_id || selectedScenarioId,
          },
          ...prev,
        ];
      });
    },
    [selectedScenarioId]
  );

  const handleConvertSimToWorkItem = async (params: {
    sourceRef: string;
    title: string;
    nodeIds: string[];
  }) => {
    if (!canSimApply) return;
    setErrorMsg(null);
    setConvertingRef(params.sourceRef);
    try {
      const wi = await axopsApi.createWorkItem({
        title: `${tx("[推演决策]", "[Sim Decision]")} ${params.title}`,
        description: tx(
          `来自因果推演方案/决策提案 (${params.sourceRef})`,
          `Dispatched from simulation scenario/decision (${params.sourceRef})`
        ),
        item_kind: "CHANGE",
        priority: "P1",
        source_kind: "SIM_DECISION",
        source_ref: params.sourceRef,
        node_ids: params.nodeIds,
      });
      setCreatedWorkItemCode(wi.code?.trim() || `WI-${wi.item_no}`);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("创建工作项失败", "Failed to create work item")
      );
    } finally {
      setConvertingRef(null);
    }
  };

  const loadSandboxes = useCallback(async () => {
    try {
      const raw = await axsimApi.listSandboxes();
      const list = Array.isArray(raw) ? raw.map(normalizeSandbox) : [];
      setSandboxes(list);
      setSelectedSandboxId((prev) => {
        if (prev && list.some((s) => s.id === prev)) return prev;
        return list[0]?.id ?? "";
      });
    } catch {
      // Ignore offline/initial failure
    }
  }, []);

  const loadSandboxDetails = useCallback(async (sandboxId: string) => {
    if (!sandboxId) {
      setGraphNodes([]);
      setGraphLinks([]);
      setOverlayHash("");
      setScenarios([]);
      setSelectedScenarioId("");
      return;
    }

    setGraphLoading(true);
    try {
      const [graphRes, scenariosRes] = await Promise.all([
        axsimApi.getSandboxGraph(sandboxId).catch(() => null),
        axsimApi.listScenarios(sandboxId).catch(() => []),
      ]);

      const safeNodes = Array.isArray(graphRes?.nodes) ? graphRes.nodes : [];
      const safeLinks = Array.isArray(graphRes?.links) ? graphRes.links : [];
      const safeHash =
        typeof graphRes?.overlay_hash === "string"
          ? graphRes.overlay_hash
          : "";
      setGraphNodes(safeNodes);
      setGraphLinks(safeLinks);
      setOverlayHash(safeHash);

      const safeScenarios = Array.isArray(scenariosRes) ? scenariosRes : [];
      setScenarios((prev) => {
        const byId = new Map<string, ScenarioRecord>();
        for (const s of prev) {
          if (s.sandbox_id === sandboxId) {
            byId.set(s.id, s);
          }
        }
        for (const s of safeScenarios) {
          if (!byId.has(s.id)) {
            byId.set(s.id, s);
          }
        }
        return Array.from(byId.values());
      });
      setSelectedScenarioId((prev) => {
        if (prev) return prev;
        return safeScenarios[0]?.id ?? "";
      });
    } finally {
      setGraphLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSandboxes();
  }, [loadSandboxes]);

  useEffect(() => {
    setScenarios([]);
    setSelectedScenarioId("");
    setHighlightedPath([]);
    setSelectedNodeId(null);
    setActiveCheckpoint(null);
    setQuickResult(null);
    setQuickActions([]);
    void loadSandboxDetails(selectedSandboxId);
  }, [loadSandboxDetails, selectedSandboxId]);

  const activeSandbox = useMemo(
    () => sandboxes.find((s) => s.id === selectedSandboxId) ?? null,
    [sandboxes, selectedSandboxId]
  );

  const activeScenario = useMemo(
    () => scenarios.find((s) => s.id === selectedScenarioId) ?? null,
    [scenarios, selectedScenarioId]
  );

  // Poll running scenario until terminal state
  useEffect(() => {
    if (
      !activeScenario ||
      (activeScenario.status !== "PENDING" &&
        activeScenario.status !== "RUNNING")
    ) {
      return;
    }
    let cancelled = false;
    const timer = window.setInterval(() => {
      void (async () => {
        try {
          const latest = await axsimApi.getScenario(activeScenario.id);
          if (!cancelled && latest && latest.id) {
            setScenarios((prev) =>
              prev.map((s) => (s.id === latest.id ? latest : s))
            );
          }
        } catch {
          // keep polling
        }
      })();
    }, 1200);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeScenario]);

  const staleCount = useMemo(
    () => scenarios.filter((s) => s.is_stale).length,
    [scenarios]
  );

  const effectiveQuickResult = useMemo(
    () => quickResult ?? activeSandbox?.sim_result ?? null,
    [activeSandbox?.sim_result, quickResult]
  );

  const effectiveImpacts: ImpactRow[] = useMemo(() => {
    if (loadedImpacts.length > 0) return loadedImpacts;
    if (activeScenario?.result?.top_impacts) {
      return activeScenario.result.top_impacts;
    }
    if (effectiveQuickResult?.impacts) {
      return effectiveQuickResult.impacts.map((r) => ({
        node_id: r.node_id,
        type_name: r.type_name,
        title: r.title,
        metric: r.metric_key,
        before_val: r.before_val,
        after_val: r.after_val,
        delta: r.delta,
        delta_ratio:
          r.delta_ratio !== undefined
            ? r.delta_ratio
            : r.before_val !== 0
              ? r.delta / r.before_val
              : null,
        p10: null,
        p90: null,
        hop_distance: r.hop_distance,
        top_path: Array.isArray(r.top_path) ? r.top_path : [],
      }));
    }
    return [];
  }, [activeScenario, effectiveQuickResult, loadedImpacts]);

  const effectiveActions: AxSimAction[] = useMemo(() => {
    if (activeScenario?.actions) return activeScenario.actions;
    if (quickActions.length > 0) return quickActions;
    return activeSandbox?.actions ?? [];
  }, [activeSandbox?.actions, activeScenario?.actions, quickActions]);

  const handleSandboxCreated = (created: AxSandboxRecord) => {
    const normalized = normalizeSandbox(created);
    setSandboxes((prev) => [normalized, ...prev]);
    setSelectedSandboxId(normalized.id);
    setScenarios([]);
    setSelectedScenarioId("");
  };

  const handleSandboxUpdated = useCallback((updated: AxSandboxRecord) => {
    const normalized = normalizeSandbox(updated);
    setSandboxes((prev) =>
      prev.map((s) => (s.id === normalized.id ? normalized : s))
    );
  }, []);

  const handleSandboxDeleted = (deletedId: string) => {
    setSandboxes((prev) => {
      const remaining = prev.filter((s) => s.id !== deletedId);
      setSelectedSandboxId(remaining[0]?.id ?? "");
      return remaining;
    });
  };

  const handleOverlayChanged = async () => {
    await Promise.all([
      loadSandboxes(),
      loadSandboxDetails(selectedSandboxId),
    ]);
  };

  const handleScenarioCreated = (scenario: ScenarioRecord) => {
    setScenarios((prev) => {
      const exists = prev.some((s) => s.id === scenario.id);
      return exists
        ? prev.map((s) => (s.id === scenario.id ? scenario : s))
        : [scenario, ...prev];
    });
    setSelectedScenarioId(scenario.id);
    setQuickResult(null);
    setHighlightedPath([]);
  };

  const handleRerunScenario = async (scenarioId: string) => {
    if (!canSimRun) return;
    setErrorMsg(null);
    try {
      const updated = await axsimApi.rerunScenario(scenarioId);
      setScenarios((prev) =>
        prev.map((s) => (s.id === updated.id ? updated : s))
      );
      setSelectedScenarioId(updated.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDeleteScenario = async (scenarioId: string) => {
    if (!canSimRun) return;
    setErrorMsg(null);
    try {
      await axsimApi.deleteScenario(scenarioId);
      setScenarios((prev) => {
        const remaining = prev.filter((s) => s.id !== scenarioId);
        if (selectedScenarioId === scenarioId) {
          setSelectedScenarioId(remaining[0]?.id ?? "");
        }
        return remaining;
      });
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleSelectScenarioFromAsk = async (scenarioId: string) => {
    await loadSandboxDetails(selectedSandboxId);
    setSelectedScenarioId(scenarioId);
    setActiveTab("simulate");
  };

  const tabItems: readonly TabOption<WorkbenchTab>[] = [
    {
      key: "topology",
      label: tx("拓扑与覆盖层", "Topology & Overlay"),
      icon: <Layers className="h-3.5 w-3.5" />,
      testId: "axsim-tab-topology",
    },
    {
      key: "simulate",
      label: tx("干预推演", "Simulate & Replay"),
      icon: <Play className="h-3.5 w-3.5" />,
      count: scenarios.length,
      testId: "axsim-tab-simulate",
    },
    {
      key: "compare",
      label: tx("对比与追问", "Compare & Copilot"),
      icon: <GitCompare className="h-3.5 w-3.5" />,
      testId: "axsim-tab-compare",
    },
    {
      key: "decisions",
      label: tx("决策下发", "Decisions"),
      icon: <ShieldCheck className="h-3.5 w-3.5" />,
      testId: "axsim-tab-decisions",
    },
  ];

  return (
    <div className="space-y-4" data-testid="axsim-workbench">
      <PageHeader
        title={tx(MODULE_META.axsim.label, MODULE_META.axsim.enLabel)}
        badges={
          <Badge variant="xblue">{tx("隔离沙盘", "Isolated Sandbox")}</Badge>
        }
        actions={
          <>
            <Button
              type="button"
              variant="outline"
              onClick={() => void loadSandboxDetails(selectedSandboxId)}
              disabled={!selectedSandboxId || graphLoading}
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${graphLoading ? "animate-spin" : ""}`}
              />
              {tx("刷新", "Refresh")}
            </Button>
            {canSimRun && (
              <Button
                type="button"
                onClick={() => setWizardOpen((v) => !v)}
                data-testid="open-sandbox-wizard-btn"
              >
                <Plus className="h-3.5 w-3.5" />
                {tx("新建沙盘", "New Sandbox")}
              </Button>
            )}
          </>
        }
      />

      {/* Sandbox Selector & Badges */}
      <Card className="p-3.5 flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2.5">
          <div className="flex items-center gap-1.5 text-xs font-medium">
            <GitBranch className="h-4 w-4 text-xblue" />
            <span>{tx("当前沙盘:", "Sandbox:")}</span>
          </div>
          <Select
            value={selectedSandboxId}
            onChange={(e) => setSelectedSandboxId(e.target.value)}
            className="min-w-[220px] font-medium"
            data-testid="sandbox-selector"
          >
            {sandboxes.length === 0 && (
              <option value="">{tx("暂无沙盘", "No sandboxes")}</option>
            )}
            {sandboxes.map((sb) => (
              <option key={sb.id} value={sb.id}>
                {sb.name} (
                {tx(
                  `${sb.node_count} 节点 / ${sb.link_count} 关系`,
                  `${sb.node_count} nodes / ${sb.link_count} links`
                )}
                )
              </option>
            ))}
          </Select>

          {activeSandbox && (
            <div className="flex flex-wrap items-center gap-1.5">
              <StatusBadge
                status={activeSandbox.status ?? "READY"}
                data-testid="sandbox-status-badge"
              />
              <Badge variant="outline" data-testid="sandbox-scope-badge">
                {tx("范围: ", "Scope: ")}
                {formatScopeLabel(activeSandbox)}
              </Badge>
              <Badge variant="outline">
                {tx(
                  `${fmtNum(graphNodes.length || activeSandbox.node_count, 0)} 节点 / ${fmtNum(graphLinks.length || activeSandbox.link_count, 0)} 关系`,
                  `${fmtNum(graphNodes.length || activeSandbox.node_count, 0)} nodes / ${fmtNum(graphLinks.length || activeSandbox.link_count, 0)} links`
                )}
              </Badge>
              {staleCount > 0 && (
                <Badge variant="warning" data-testid="sandbox-stale-badge">
                  {tx(
                    `${staleCount} 个场景已过期`,
                    `${staleCount} stale scenarios`
                  )}
                </Badge>
              )}
            </div>
          )}
        </div>
      </Card>

      {/* Wizard & Sandbox Lifecycle Controls */}
      <SandboxWizard
        open={wizardOpen}
        activeSandbox={activeSandbox}
        onClose={() => setWizardOpen(false)}
        onCreated={handleSandboxCreated}
        onUpdated={handleSandboxUpdated}
        onDeleted={handleSandboxDeleted}
        onOverlayChanged={() => void handleOverlayChanged()}
      />

      {errorMsg && (
        <Card className="p-3 border-red-500/40 bg-red-500/10 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </Card>
      )}

      {createdWorkItemCode && (
        <div
          data-testid="axsim-workitem-banner"
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-xblue/40 bg-xblue-soft px-3.5 py-2.5 text-xs text-xblue"
        >
          <div className="flex flex-wrap items-center gap-2">
            <Kanban className="h-3.5 w-3.5" />
            <span className="font-medium">
              {tx(
                `已创建工作项 ${createdWorkItemCode}`,
                `Work item ${createdWorkItemCode} created`
              )}
            </span>
            <Link
              to="/ops"
              data-testid="axsim-workitem-ops-link"
              className="inline-flex items-center gap-1 underline font-semibold"
            >
              {tx("查看交付看板", "Open Work Items")}
              <ExternalLink className="h-3 w-3" />
            </Link>
          </div>
          <button
            type="button"
            onClick={() => setCreatedWorkItemCode(null)}
            className="text-[11px] underline cursor-pointer shrink-0"
          >
            {tx("关闭", "Dismiss")}
          </button>
        </div>
      )}

      {/* 4-Tab Workbench Navigation */}
      <Tabs items={tabItems} value={activeTab} onChange={handleChangeTab} />

      {/* Tab Content */}
      {!selectedSandboxId ? (
        <EmptyState
          title={tx("暂无推演沙盘", "No simulation sandboxes")}
          action={
            canSimRun ? (
              <Button type="button" onClick={() => setWizardOpen(true)}>
                <Plus className="h-3.5 w-3.5" />
                {tx("新建沙盘", "New Sandbox")}
              </Button>
            ) : undefined
          }
        />
      ) : (
        <>
          {activeTab === "topology" && (
            <OverlayEditor
              sandboxId={selectedSandboxId}
              nodes={graphNodes}
              links={graphLinks}
              overlayHash={overlayHash}
              loading={graphLoading}
              onRefreshGraph={handleOverlayChanged}
            />
          )}

          {activeTab === "simulate" && (
            <div className="min-h-[680px]">
              <ResizableWorkspace
                storageKey="axpoint.layout.sim"
                panes={[
                  {
                    id: "controls",
                    defaultSize: 38,
                    minSize: 24,
                    maxSize: 55,
                    collapsible: true,
                    handleTestId: "resizable-handle-left",
                    className: "pr-1.5 overflow-y-auto",
                    children: (
                      <div className="space-y-4">
                        <ActionList
                          sandboxId={selectedSandboxId}
                          nodes={graphNodes}
                          onScenarioCreated={handleScenarioCreated}
                          onQuickSimulated={(res, acts) => {
                            setQuickResult(res);
                            setQuickActions(acts);
                            setSelectedScenarioId("");
                          }}
                        />

                        {/* Saved Scenarios Strip */}
                        <Card
                          className="p-3.5 space-y-2.5"
                          data-testid="scenarios-list-card"
                        >
                          <div className="flex flex-wrap items-center justify-between gap-2">
                            <span className="text-xs font-semibold">
                              {tx(
                                `已保存推演场景 (${scenarios.length})`,
                                `Saved Scenarios (${scenarios.length})`
                              )}
                            </span>
                            {activeScenario?.result && (
                              <div className="flex flex-wrap items-center gap-2 text-[11px] font-mono-tech text-muted-foreground tabular-nums">
                                <span>
                                  {tx("受影响: ", "Affected: ")}
                                  {activeScenario.result.affected_count}
                                </span>
                                <span>
                                  {tx("迭代: ", "Iterations: ")}
                                  {activeScenario.result.iterations}
                                </span>
                                <Badge
                                  variant={
                                    activeScenario.result.converged
                                      ? "success"
                                      : "warning"
                                  }
                                >
                                  {activeScenario.result.converged
                                    ? tx("已收敛", "Converged")
                                    : tx("未收敛", "Unconverged")}
                                </Badge>
                              </div>
                            )}
                          </div>

                          {scenarios.length === 0 ? (
                            <div className="text-xs text-muted-foreground">
                              {tx("暂无已保存场景", "No saved scenarios")}
                            </div>
                          ) : (
                            <div className="flex flex-wrap gap-2">
                              {scenarios.map((sc) => {
                                const active = sc.id === selectedScenarioId;
                                const linkedDecision = trackedDecisions.find(
                                  (d) => d.scenarioId === sc.id
                                );
                                const refId = linkedDecision?.id ?? sc.id;
                                const nodeIds = Array.from(
                                  new Set(
                                    (sc.actions ?? [])
                                      .map((a) => a.target_node_id)
                                      .filter((nid) => Boolean(nid))
                                  )
                                );
                                return (
                                  <div
                                    key={sc.id}
                                    data-testid={`scenario-chip-${sc.name}`}
                                    className={`inline-flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-xs ${
                                      active
                                        ? "border-xblue bg-xblue-soft/50"
                                        : "border-border bg-background"
                                    }`}
                                  >
                                    <button
                                      type="button"
                                      onClick={() => {
                                        setSelectedScenarioId(sc.id);
                                        setQuickResult(null);
                                      }}
                                      className="font-medium hover:text-xblue cursor-pointer"
                                    >
                                      {sc.name}
                                    </button>
                                    <Badge variant="outline">
                                      {tEnum(sc.params?.mode ?? "STEADY")} ·{" "}
                                      {tEnum(sc.status)}
                                    </Badge>
                                    {canSimApply && (
                                      <Button
                                        type="button"
                                        variant="outline"
                                        size="sm"
                                        className="h-6 px-2 text-[10px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                                        data-testid={`scenario-to-workitem-btn-${sc.id}`}
                                        disabled={convertingRef === refId}
                                        onClick={() =>
                                          void handleConvertSimToWorkItem({
                                            sourceRef: refId,
                                            title:
                                              linkedDecision?.title ?? sc.name,
                                            nodeIds,
                                          })
                                        }
                                      >
                                        <Kanban className="h-2.5 w-2.5" />
                                        {tx("转工作项", "To Work Item")}
                                      </Button>
                                    )}
                                    {sc.is_stale && canSimRun && (
                                      <Button
                                        type="button"
                                        variant="outline"
                                        size="sm"
                                        className="h-6 px-1.5 text-[10px] border-amber-500/60 text-amber-600 dark:text-amber-400"
                                        onClick={() =>
                                          void handleRerunScenario(sc.id)
                                        }
                                        title={tx("重算场景", "Rerun scenario")}
                                      >
                                        <RefreshCw className="h-2.5 w-2.5" />
                                        {tx("重算", "Rerun")}
                                      </Button>
                                    )}
                                    {canSimRun && (
                                      <button
                                        type="button"
                                        onClick={() =>
                                          void handleDeleteScenario(sc.id)
                                        }
                                        className="text-muted-foreground hover:text-red-600 cursor-pointer"
                                        aria-label={tx(
                                          `删除场景 ${sc.name}`,
                                          `Delete scenario ${sc.name}`
                                        )}
                                      >
                                        <Trash2 className="h-3 w-3" />
                                      </button>
                                    )}
                                  </div>
                                );
                              })}
                            </div>
                          )}
                        </Card>
                      </div>
                    ),
                  },
                  {
                    id: "canvas",
                    defaultSize: 62,
                    minSize: 38,
                    className: "pl-1.5 overflow-y-auto",
                    children: (
                      <div className="space-y-4">
                        <ReplayTimeline
                          scenario={activeScenario}
                          nodes={graphNodes}
                          onCheckpointChange={setActiveCheckpoint}
                        />

                        <ImpactGraph
                          nodes={graphNodes}
                          links={graphLinks}
                          impacts={effectiveImpacts}
                          actions={effectiveActions}
                          highlightedPath={highlightedPath}
                          checkpointChanges={activeCheckpoint?.changes ?? null}
                          activeStepLabel={activeCheckpoint?.label ?? null}
                          selectedNodeId={selectedNodeId}
                          onSelectNode={setSelectedNodeId}
                          onClearPath={() => setHighlightedPath([])}
                        />

                        <ImpactTable
                          scenario={activeScenario}
                          quickResult={effectiveQuickResult}
                          nodes={graphNodes}
                          highlightedPath={highlightedPath}
                          onHighlightPath={(path) => setHighlightedPath(path)}
                          onSelectNode={(id) => setSelectedNodeId(id)}
                          onImpactsLoaded={setLoadedImpacts}
                        />
                      </div>
                    ),
                  },
                ]}
              />
            </div>
          )}

          {activeTab === "compare" && (
            <div className="space-y-4">
              <ScenarioCompare
                sandboxId={selectedSandboxId}
                scenarios={scenarios}
              />
              <SimAsk
                sandboxId={selectedSandboxId}
                scenarios={scenarios}
                activeScenarioId={selectedScenarioId || null}
                onSelectScenario={(id) => void handleSelectScenarioFromAsk(id)}
                onScenarioCreated={() => {
                  void loadSandboxDetails(selectedSandboxId);
                }}
              />
            </div>
          )}

          {activeTab === "decisions" && (
            <div className="space-y-4">
              <DecisionPanel
                scenario={activeScenario}
                nodes={graphNodes}
                onDecisionTracked={handleDecisionTracked}
              />

              {canSimApply && (trackedDecisions.length > 0 || activeScenario) && (
                <Card
                  className="p-3.5 space-y-2.5"
                  data-testid="axsim-workitem-dispatch-card"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                    <div className="flex items-center gap-2 font-medium">
                      <Kanban className="h-3.5 w-3.5 text-xblue" />
                      <span>
                        {tx("交付工作项派发", "Work Item Dispatch")}
                      </span>
                    </div>
                  </div>

                  <div className="divide-y divide-border/60">
                    {trackedDecisions.map((dec) => {
                      const sc = scenarios.find((s) => s.id === dec.scenarioId);
                      const nodeIds = Array.from(
                        new Set(
                          (sc?.actions ?? activeScenario?.actions ?? [])
                            .map((a) => a.target_node_id)
                            .filter((nid) => Boolean(nid))
                        )
                      );
                      return (
                        <div
                          key={dec.id}
                          data-testid={`decision-workitem-row-${dec.id}`}
                          className="flex flex-wrap items-center justify-between gap-2 py-2 text-xs"
                        >
                          <div className="flex items-center gap-2">
                            <Badge variant="xblue">
                              {tx("决策提案", "Decision")}
                            </Badge>
                            <span className="font-medium">{dec.title}</span>
                            <span className="font-mono-tech text-[11px] text-muted-foreground">
                              {dec.id}
                            </span>
                          </div>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                            data-testid={`convert-decision-workitem-btn-${dec.id}`}
                            disabled={convertingRef === dec.id}
                            onClick={() =>
                              void handleConvertSimToWorkItem({
                                sourceRef: dec.id,
                                title: dec.title,
                                nodeIds,
                              })
                            }
                          >
                            <Kanban className="h-3 w-3" />
                            {convertingRef === dec.id
                              ? tx("创建中…", "Creating…")
                              : tx("转工作项", "To Work Item")}
                          </Button>
                        </div>
                      );
                    })}

                    {trackedDecisions.length === 0 && activeScenario && (
                      <div className="flex flex-wrap items-center justify-between gap-2 py-2 text-xs">
                        <div className="flex items-center gap-2">
                          <Badge variant="outline">
                            {tx("推演场景", "Scenario")}
                          </Badge>
                          <span className="font-medium">
                            {activeScenario.name}
                          </span>
                        </div>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                          data-testid="convert-active-scenario-workitem-btn"
                          disabled={convertingRef === activeScenario.id}
                          onClick={() =>
                            void handleConvertSimToWorkItem({
                              sourceRef: activeScenario.id,
                              title: activeScenario.name,
                              nodeIds: Array.from(
                                new Set(
                                  (activeScenario.actions ?? [])
                                    .map((a) => a.target_node_id)
                                    .filter((nid) => Boolean(nid))
                                )
                              ),
                            })
                          }
                        >
                          <Kanban className="h-3 w-3" />
                          {tx("转工作项", "To Work Item")}
                        </Button>
                      </div>
                    )}
                  </div>
                </Card>
              )}

              <ImpactTable
                scenario={activeScenario}
                quickResult={effectiveQuickResult}
                nodes={graphNodes}
                highlightedPath={highlightedPath}
                onHighlightPath={(path) => {
                  setHighlightedPath(path);
                  setActiveTab("simulate");
                }}
                onSelectNode={(id) => {
                  setSelectedNodeId(id);
                  setActiveTab("simulate");
                }}
                onImpactsLoaded={setLoadedImpacts}
              />
            </div>
          )}
        </>
      )}
    </div>
  );
}
