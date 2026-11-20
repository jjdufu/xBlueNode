import { useCallback, useEffect, useMemo, useState } from "react";
import {
  GitBranch,
  GitCompare,
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
  type ImpactRow,
  type SandboxGraphLink,
  type SandboxGraphNode,
  type ScenarioRecord,
} from "@/lib/api/axsim";
import { ActionList } from "@/components/axsim/ActionList";
import { DecisionPanel } from "@/components/axsim/DecisionPanel";
import { ImpactGraph } from "@/components/axsim/ImpactGraph";
import { ImpactTable } from "@/components/axsim/ImpactTable";
import { OverlayEditor } from "@/components/axsim/OverlayEditor";
import { ReplayTimeline } from "@/components/axsim/ReplayTimeline";
import { SandboxWizard } from "@/components/axsim/SandboxWizard";
import { ScenarioCompare } from "@/components/axsim/ScenarioCompare";
import { SimAsk } from "@/components/axsim/SimAsk";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

type WorkbenchTab = "topology" | "simulate" | "compare" | "decisions";

const WORKBENCH_TABS: ReadonlyArray<{
  key: WorkbenchTab;
  label: string;
  icon: typeof Layers;
}> = [
  { key: "topology", label: "拓扑与覆盖层", icon: Layers },
  { key: "simulate", label: "干预推演与回放", icon: Play },
  { key: "compare", label: "方案对比与问数", icon: GitCompare },
  { key: "decisions", label: "决策下发与影响", icon: ShieldCheck },
];

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

function formatScopeLabel(sb: AxSandboxRecord | null): string {
  if (!sb?.scope) return "全量图谱";
  const parts: string[] = [];
  if (Array.isArray(sb.scope.root_node_ids) && sb.scope.root_node_ids.length > 0) {
    parts.push(`${sb.scope.root_node_ids.length} 种子`);
  }
  if (sb.scope.depth != null) {
    parts.push(`深度 ${sb.scope.depth}`);
  }
  if (sb.scope.direction) {
    parts.push(sb.scope.direction);
  }
  if (Array.isArray(sb.scope.type_names) && sb.scope.type_names.length > 0) {
    parts.push(sb.scope.type_names.join(","));
  }
  return parts.length > 0 ? parts.join(" · ") : "全量图谱";
}

export function AxSimPage() {
  const [sandboxes, setSandboxes] = useState<AxSandboxRecord[]>([]);
  const [selectedSandboxId, setSelectedSandboxId] = useState<string>("");
  const [wizardOpen, setWizardOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<WorkbenchTab>("simulate");

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

  return (
    <div className="space-y-5" data-testid="axsim-workbench">
      {/* Header */}
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            AxSim · 因果推演沙盘
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            基于写时复制（CoW）范围克隆生产业务图，求解干预传导（稳态 / 分步 / 蒙特卡洛置信带），支持多方案对比、自然语言追问与审批下发
          </p>
        </div>

        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => void loadSandboxDetails(selectedSandboxId)}
            disabled={!selectedSandboxId || graphLoading}
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${graphLoading ? "animate-spin" : ""}`}
            />
            刷新
          </Button>
          <Button
            type="button"
            onClick={() => setWizardOpen((v) => !v)}
            data-testid="open-sandbox-wizard-btn"
          >
            <Plus className="h-3.5 w-3.5" />
            新建沙盘
          </Button>
        </div>
      </div>

      {/* Sandbox Selector & Badges */}
      <Card className="p-3.5 flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2.5">
          <div className="flex items-center gap-1.5 text-xs font-medium">
            <GitBranch className="h-4 w-4 text-xblue" />
            <span>当前沙盘:</span>
          </div>
          <select
            value={selectedSandboxId}
            onChange={(e) => setSelectedSandboxId(e.target.value)}
            className="h-8 min-w-[220px] rounded-md border border-input bg-background px-2.5 text-xs font-medium"
            data-testid="sandbox-selector"
          >
            {sandboxes.length === 0 && <option value="">暂无沙盘</option>}
            {sandboxes.map((sb) => (
              <option key={sb.id} value={sb.id}>
                {sb.name} ({sb.node_count} 节点 / {sb.link_count} 关系)
              </option>
            ))}
          </select>

          {activeSandbox && (
            <div className="flex flex-wrap items-center gap-1.5">
              <Badge
                variant={activeSandbox.status === "CLONING" ? "xblue" : "neutral"}
                data-testid="sandbox-status-badge"
              >
                {activeSandbox.status ?? "READY"}
              </Badge>
              <Badge variant="outline" data-testid="sandbox-scope-badge">
                范围: {formatScopeLabel(activeSandbox)}
              </Badge>
              <Badge variant="outline">
                {graphNodes.length || activeSandbox.node_count} 节点 /{" "}
                {graphLinks.length || activeSandbox.link_count} 关系
              </Badge>
              {staleCount > 0 && (
                <Badge
                  variant="outline"
                  className="border-amber-500/60 text-amber-600 dark:text-amber-400"
                  data-testid="sandbox-stale-badge"
                >
                  {staleCount} 个场景结果已过期
                </Badge>
              )}
            </div>
          )}
        </div>

        <Badge variant="xblue">ISOLATED SANDBOX</Badge>
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
          <span className="font-mono-tech font-semibold">推演提示：</span>
          {errorMsg}
        </Card>
      )}

      {/* 4-Tab Workbench Navigation */}
      <div
        className="flex flex-wrap gap-1.5 border-b border-border pb-2"
        role="tablist"
        aria-label="AxSim 推演工作台"
      >
        {WORKBENCH_TABS.map((tab) => {
          const Icon = tab.icon;
          const active = activeTab === tab.key;
          return (
            <button
              key={tab.key}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setActiveTab(tab.key)}
              data-testid={`axsim-tab-${tab.key}`}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer",
                active
                  ? "bg-primary text-primary-foreground"
                  : "border border-border bg-background text-muted-foreground hover:text-foreground hover:bg-muted/50"
              )}
            >
              <Icon className="h-3.5 w-3.5" />
              {tab.label}
            </button>
          );
        })}
      </div>

      {/* Tab Content */}
      {!selectedSandboxId ? (
        <Card className="p-8 text-center space-y-3">
          <div className="text-sm font-medium">尚未选择或创建推演沙盘</div>
          <p className="text-xs text-muted-foreground">
            点击右上角「新建沙盘」克隆生产业务图快照，即可开始隔离推演。
          </p>
          <div>
            <Button type="button" onClick={() => setWizardOpen(true)}>
              <Plus className="h-3.5 w-3.5" />
              创建第一个沙盘
            </Button>
          </div>
        </Card>
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
              <Card className="p-3.5 space-y-2.5" data-testid="scenarios-list-card">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="text-xs font-semibold">
                    已保存推演场景 ({scenarios.length})
                  </span>
                  {activeScenario?.result && (
                    <div className="flex flex-wrap items-center gap-2 text-[11px] font-mono-tech text-muted-foreground">
                      <span>
                        受影响: {activeScenario.result.affected_count}
                      </span>
                      <span>
                        迭代: {activeScenario.result.iterations} 轮
                      </span>
                      <Badge
                        variant={
                          activeScenario.result.converged ? "xblue" : "outline"
                        }
                      >
                        {activeScenario.result.converged ? "已收敛" : "未收敛"}
                      </Badge>
                    </div>
                  )}
                </div>

                {scenarios.length === 0 ? (
                  <div className="text-xs text-muted-foreground">
                    暂无已保存场景，请在上方配置干预动作并点击「创建并运行场景」。
                  </div>
                ) : (
                  <div className="flex flex-wrap gap-2">
                    {scenarios.map((sc) => {
                      const active = sc.id === selectedScenarioId;
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
                            {sc.params?.mode ?? "STEADY"} · {sc.status}
                          </Badge>
                          {sc.is_stale && (
                            <Button
                              type="button"
                              variant="outline"
                              className="h-6 px-1.5 text-[10px] border-amber-500/60 text-amber-600 dark:text-amber-400"
                              onClick={() => void handleRerunScenario(sc.id)}
                              title="覆盖层已变更，点击重算"
                            >
                              <RefreshCw className="h-2.5 w-2.5" />
                              重算
                            </Button>
                          )}
                          <button
                            type="button"
                            onClick={() => void handleDeleteScenario(sc.id)}
                            className="text-muted-foreground hover:text-red-600 cursor-pointer"
                            aria-label={`删除场景 ${sc.name}`}
                          >
                            <Trash2 className="h-3 w-3" />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                )}
              </Card>

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
              />
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
