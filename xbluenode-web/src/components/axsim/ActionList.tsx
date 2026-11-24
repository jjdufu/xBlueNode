import { useEffect, useState } from "react";
import { Play, Plus, Sliders, Trash2, Zap } from "lucide-react";
import {
  axsimApi,
  type AxSimAction,
  type AxSimQuickResult,
  type SandboxGraphNode,
  type ScenarioRecord,
  type SimEventSpec,
  type SimGoalConstraint,
  type SimMode,
  type SimParams,
} from "@/lib/api/axsim";
import type { InterventionMode } from "@/lib/api/enums";
import {
  EventQueueEditor,
  compileEventsAndGoals,
  type EventDraft,
  type GoalDraft,
} from "@/components/axsim/EventQueueEditor";
import { Badge, Button, Card, Input, Select } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";

export interface ActionListProps {
  sandboxId: string;
  nodes: SandboxGraphNode[];
  onScenarioCreated: (scenario: ScenarioRecord) => void;
  onQuickSimulated?: (result: AxSimQuickResult, actions: AxSimAction[]) => void;
}

interface ActionDraft {
  id: string;
  target_node_id: string;
  metric_key: string;
  mode: InterventionMode;
  value: string;
  at_step: string;
}

const RATIO_PRESETS: ReadonlyArray<{ label: string; value: string }> = [
  { label: "-50%", value: "-0.5" },
  { label: "-30%", value: "-0.3" },
  { label: "-10%", value: "-0.1" },
  { label: "+20%", value: "0.2" },
];

function createEmptyAction(nodes: SandboxGraphNode[]): ActionDraft {
  const firstNode = nodes[0];
  const metricKeys = firstNode ? Object.keys(firstNode.metrics ?? {}) : [];
  return {
    id: `act-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    target_node_id: firstNode?.id ?? "",
    metric_key: metricKeys[0] ?? "output",
    mode: "RATIO",
    value: "-0.3",
    at_step: "0",
  };
}

export function ActionList({
  sandboxId,
  nodes,
  onScenarioCreated,
  onQuickSimulated,
}: ActionListProps) {
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canSimRun = can("SIM_RUN");

  const [actions, setActions] = useState<ActionDraft[]>(() => [
    createEmptyAction(nodes),
  ]);
  const [eventDrafts, setEventDrafts] = useState<EventDraft[]>([]);
  const [goalDrafts, setGoalDrafts] = useState<GoalDraft[]>([]);
  const [scenarioName, setScenarioName] = useState("");
  const [scenarioDesc, setScenarioDesc] = useState("");

  // SimParams state
  const [simMode, setSimMode] = useState<SimMode>("STEADY");
  const [horizonSteps, setHorizonSteps] = useState("12");
  const [checkpointEvery, setCheckpointEvery] = useState("1");
  const [damping, setDamping] = useState("0.85");
  const [maxHops, setMaxHops] = useState("");

  const [uncertaintyEnabled, setUncertaintyEnabled] = useState(false);
  const [uncSamples, setUncSamples] = useState("100");
  const [uncSigmaRel, setUncSigmaRel] = useState("0.2");
  const [uncSeed, setUncSeed] = useState("42");

  const [running, setRunning] = useState(false);
  const [quickRunning, setQuickRunning] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const modeOptions: ReadonlyArray<{ value: InterventionMode; label: string }> =
    [
      { value: "RATIO", label: tx("比例变动", "Ratio") },
      { value: "ADD", label: tx("增量叠加", "Additive") },
      { value: "ABSOLUTE", label: tx("绝对设定", "Absolute") },
    ];

  useEffect(() => {
    if (nodes.length === 0) return;
    setActions((prev) =>
      prev.map((a) => {
        if (a.target_node_id && nodes.some((n) => n.id === a.target_node_id)) {
          return a;
        }
        const first = nodes[0];
        const mKeys = Object.keys(first.metrics ?? {});
        return {
          ...a,
          target_node_id: first.id,
          metric_key: mKeys.includes(a.metric_key)
            ? a.metric_key
            : (mKeys[0] ?? a.metric_key),
        };
      })
    );
  }, [nodes]);

  const updateAction = (id: string, patch: Partial<ActionDraft>) => {
    setActions((prev) =>
      prev.map((row) => {
        if (row.id !== id) return row;
        const next = { ...row, ...patch };
        if (patch.target_node_id) {
          const targetNode = nodes.find((n) => n.id === patch.target_node_id);
          const keys = targetNode ? Object.keys(targetNode.metrics ?? {}) : [];
          if (keys.length > 0 && !keys.includes(next.metric_key)) {
            next.metric_key = keys[0];
          }
        }
        return next;
      })
    );
  };

  const addActionRow = () => {
    setActions((prev) => [...prev, createEmptyAction(nodes)]);
  };

  const removeActionRow = (id: string) => {
    setActions((prev) =>
      prev.length > 1 || eventDrafts.length > 0 || goalDrafts.length > 0
        ? prev.filter((r) => r.id !== id)
        : prev
    );
  };

  const handleEventsChanged = (next: EventDraft[]) => {
    setEventDrafts(next);
    if (next.length > 0 && simMode !== "STEPPED") {
      setSimMode("STEPPED");
    }
  };

  const buildPayload = (): {
    wireActions: AxSimAction[];
    scheduledEvents: SimEventSpec[];
    goalConstraints: SimGoalConstraint[];
    params: SimParams;
  } | null => {
    const compiled = compileEventsAndGoals(eventDrafts, goalDrafts, tx);
    if (!compiled.ok) {
      setErrorMsg(compiled.error);
      return null;
    }

    const wireActions: AxSimAction[] = [];
    for (const draft of actions) {
      if (!draft.target_node_id) {
        setErrorMsg(
          tx("干预行需指定目标节点", "Target node is required for each action")
        );
        return null;
      }
      if (!draft.metric_key.trim()) {
        setErrorMsg(tx("干预指标键不能为空", "Metric key is required"));
        return null;
      }
      const numVal = Number.parseFloat(draft.value);
      if (!Number.isFinite(numVal)) {
        setErrorMsg(
          tx("干预数值必须是有效数字", "Intervention value must be numeric")
        );
        return null;
      }
      if (draft.mode === "RATIO" && numVal <= -1) {
        setErrorMsg(
          tx(
            "比例变动值必须大于 -1（-0.3 表示下调 30%）",
            "Ratio delta must exceed -1 (-0.3 means -30%)"
          )
        );
        return null;
      }
      const atStep = Number.parseInt(draft.at_step, 10);
      wireActions.push({
        target_node_id: draft.target_node_id,
        metric_key: draft.metric_key.trim(),
        mode: draft.mode,
        value: numVal,
        at_step: Number.isFinite(atStep) && atStep >= 0 ? atStep : 0,
      });
    }

    if (
      wireActions.length === 0 &&
      compiled.scheduledEvents.length === 0 &&
      compiled.goalConstraints.length === 0
    ) {
      setErrorMsg(
        tx(
          "至少配置一条干预、事件或红线约束",
          "At least one action, event, or goal constraint is required"
        )
      );
      return null;
    }

    const parsedHorizon = Number.parseInt(horizonSteps, 10);
    const parsedCkptEvery = Number.parseInt(checkpointEvery, 10);
    const parsedDamping = Number.parseFloat(damping);
    const parsedMaxHops = maxHops.trim() ? Number.parseInt(maxHops, 10) : null;
    const effectiveMode: SimMode =
      compiled.scheduledEvents.length > 0 ? "STEPPED" : simMode;

    const params: SimParams = {
      mode: effectiveMode,
      damping:
        Number.isFinite(parsedDamping) &&
        parsedDamping > 0 &&
        parsedDamping <= 1
          ? parsedDamping
          : 0.85,
      horizon_steps:
        Number.isFinite(parsedHorizon) && parsedHorizon >= 1
          ? Math.min(104, parsedHorizon)
          : 12,
      checkpoint_every:
        Number.isFinite(parsedCkptEvery) && parsedCkptEvery >= 1
          ? parsedCkptEvery
          : 1,
      max_hops:
        parsedMaxHops !== null &&
        Number.isFinite(parsedMaxHops) &&
        parsedMaxHops >= 1
          ? parsedMaxHops
          : null,
      uncertainty: uncertaintyEnabled
        ? {
            samples: Math.min(
              500,
              Math.max(1, Number.parseInt(uncSamples, 10) || 100)
            ),
            coefficient_cv: Math.max(
              0.01,
              Number.parseFloat(uncSigmaRel) || 0.2
            ),
            seed: Number.parseInt(uncSeed, 10) || 42,
          }
        : null,
    };

    return {
      wireActions,
      scheduledEvents: compiled.scheduledEvents,
      goalConstraints: compiled.goalConstraints,
      params,
    };
  };

  const handleRunScenario = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSimRun) return;
    setErrorMsg(null);
    const built = buildPayload();
    if (!built) return;

    const effectiveName =
      scenarioName.trim() ||
      `sim-${(built.params.mode ?? simMode).toLowerCase()}-${new Date()
        .toISOString()
        .slice(11, 19)
        .replace(/:/g, "")}`;

    setRunning(true);
    try {
      const created = await axsimApi.createScenario(sandboxId, {
        name: effectiveName,
        description: scenarioDesc.trim() || undefined,
        actions: built.wireActions,
        scheduled_events:
          built.scheduledEvents.length > 0 ? built.scheduledEvents : undefined,
        goal_constraints:
          built.goalConstraints.length > 0 ? built.goalConstraints : undefined,
        params: built.params,
      });
      setScenarioName("");
      onScenarioCreated(created);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setRunning(false);
    }
  };

  const handleQuickSimulate = async () => {
    if (!canSimRun) return;
    setErrorMsg(null);
    const built = buildPayload();
    if (!built) return;
    setQuickRunning(true);
    try {
      const res = await axsimApi.quickSimulate(
        sandboxId,
        built.wireActions,
        built.params,
        {
          scheduled_events:
            built.scheduledEvents.length > 0
              ? built.scheduledEvents
              : undefined,
          goal_constraints:
            built.goalConstraints.length > 0
              ? built.goalConstraints
              : undefined,
        }
      );
      onQuickSimulated?.(res, built.wireActions);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setQuickRunning(false);
    }
  };

  return (
    <Card className="p-4 space-y-4" data-testid="action-list-panel">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2 text-sm font-medium">
          <Sliders className="h-4 w-4 text-xblue" />
          <span>{tx("干预设定", "Interventions")}</span>
        </div>
        {canSimRun && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={addActionRow}
            data-testid="add-action-btn"
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("添加干预", "Add Action")}
          </Button>
        )}
      </div>

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      <form
        onSubmit={(e) => void handleRunScenario(e)}
        className="space-y-4"
      >
        {/* Interventions list */}
        <div className="space-y-2.5">
          {actions.map((row, idx) => {
            const targetNode = nodes.find((n) => n.id === row.target_node_id);
            const nodeMetricKeys = targetNode
              ? Object.keys(targetNode.metrics ?? {})
              : [];
            const numericVal = Number.parseFloat(row.value);
            const ratioPctLabel =
              row.mode === "RATIO" && Number.isFinite(numericVal)
                ? `${numericVal >= 0 ? "+" : ""}${(numericVal * 100).toFixed(1)}%`
                : null;

            return (
              <div
                key={row.id}
                data-testid={`action-row-${idx}`}
                className="rounded-md border border-border bg-muted/20 p-3 space-y-2"
              >
                <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-end">
                  <div className="md:col-span-4 space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("目标节点", "Target Node")}
                    </label>
                    <Select
                      value={row.target_node_id}
                      disabled={!canSimRun}
                      onChange={(e) =>
                        updateAction(row.id, { target_node_id: e.target.value })
                      }
                      className="h-8 w-full"
                      data-testid={`action-node-select-${idx}`}
                    >
                      {nodes.map((n) => (
                        <option key={n.id} value={n.id}>
                          [{n.type_name}] {n.node_key} · {n.title}
                        </option>
                      ))}
                    </Select>
                  </div>

                  <div className="md:col-span-2 space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("指标键", "Metric Key")}
                    </label>
                    <Input
                      list={`metrics-datalist-${row.id}`}
                      disabled={!canSimRun}
                      value={row.metric_key}
                      onChange={(e) =>
                        updateAction(row.id, { metric_key: e.target.value })
                      }
                      className="h-8 text-xs font-mono-tech"
                      data-testid={`action-metric-input-${idx}`}
                    />
                    <datalist id={`metrics-datalist-${row.id}`}>
                      {nodeMetricKeys.map((k) => (
                        <option key={k} value={k} />
                      ))}
                    </datalist>
                  </div>

                  <div className="md:col-span-2 space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("干预方式", "Mode")}
                    </label>
                    <Select
                      value={row.mode}
                      disabled={!canSimRun}
                      onChange={(e) =>
                        updateAction(row.id, {
                          mode: e.target.value as InterventionMode,
                        })
                      }
                      className="h-8 w-full"
                      data-testid={`action-mode-select-${idx}`}
                    >
                      {modeOptions.map((opt) => (
                        <option key={opt.value} value={opt.value}>
                          {opt.label}
                        </option>
                      ))}
                    </Select>
                  </div>

                  <div className="md:col-span-2 space-y-1">
                    <label className="text-[11px] text-muted-foreground flex items-center justify-between">
                      <span>{tx("数值", "Value")}</span>
                      {ratioPctLabel && (
                        <span className="font-mono-tech text-xblue">
                          {ratioPctLabel}
                        </span>
                      )}
                    </label>
                    <Input
                      type="number"
                      step="any"
                      disabled={!canSimRun}
                      value={row.value}
                      onChange={(e) =>
                        updateAction(row.id, { value: e.target.value })
                      }
                      className="h-8 text-xs font-mono-tech"
                      data-testid={`action-value-input-${idx}`}
                    />
                  </div>

                  <div className="md:col-span-1 space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("生效步", "At Step")}
                    </label>
                    <Input
                      type="number"
                      min={0}
                      disabled={!canSimRun}
                      value={row.at_step}
                      onChange={(e) =>
                        updateAction(row.id, { at_step: e.target.value })
                      }
                      className="h-8 text-xs font-mono-tech"
                      data-testid={`action-step-input-${idx}`}
                    />
                  </div>

                  <div className="md:col-span-1 flex justify-end">
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-8 px-2 text-muted-foreground hover:text-red-600"
                      onClick={() => removeActionRow(row.id)}
                      disabled={
                        !canSimRun ||
                        (actions.length <= 1 &&
                          eventDrafts.length === 0 &&
                          goalDrafts.length === 0)
                      }
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </div>

                {row.mode === "RATIO" && canSimRun && (
                  <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
                    <span className="text-muted-foreground">
                      {tx("快捷比例:", "Presets:")}
                    </span>
                    {RATIO_PRESETS.map((p) => (
                      <button
                        key={p.value}
                        type="button"
                        onClick={() =>
                          updateAction(row.id, { value: p.value })
                        }
                        className={`rounded border px-1.5 py-0.5 font-mono-tech cursor-pointer ${
                          row.value === p.value
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border bg-background text-muted-foreground hover:text-foreground"
                        }`}
                      >
                        {p.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        <EventQueueEditor
          nodes={nodes}
          events={eventDrafts}
          goals={goalDrafts}
          disabled={!canSimRun}
          onChangeEvents={handleEventsChanged}
          onChangeGoals={setGoalDrafts}
        />

        {/* Simulation Params */}
        <div className="rounded-md border border-border p-3 space-y-3 bg-muted/10">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-xs font-medium">
              {tx("传导求解参数", "Propagation Settings")}
            </span>
            <div className="inline-flex rounded-md border border-border bg-background p-0.5">
              <button
                type="button"
                onClick={() => setSimMode("STEADY")}
                className={`rounded px-2.5 py-1 text-xs cursor-pointer ${
                  simMode === "STEADY"
                    ? "bg-muted font-medium text-foreground"
                    : "text-muted-foreground"
                }`}
                data-testid="sim-mode-steady"
              >
                {tx("稳态收敛", "Steady-State")}
              </button>
              <button
                type="button"
                onClick={() => setSimMode("STEPPED")}
                className={`rounded px-2.5 py-1 text-xs cursor-pointer ${
                  simMode === "STEPPED"
                    ? "bg-muted font-medium text-foreground"
                    : "text-muted-foreground"
                }`}
                data-testid="sim-mode-stepped"
              >
                {tx("分步演化", "Time-Stepped")}
              </button>
            </div>
          </div>

          <div className="grid grid-cols-2 md:grid-cols-4 gap-2.5 text-xs">
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("阻尼系数", "Damping")}
              </label>
              <Input
                type="number"
                step="any"
                min="0.001"
                max="1"
                value={damping}
                onChange={(e) => setDamping(e.target.value)}
                className="h-8 text-xs font-mono-tech"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("最大传导跳数", "Max Hops")}
              </label>
              <Input
                type="number"
                min={1}
                value={maxHops}
                onChange={(e) => setMaxHops(e.target.value)}
                placeholder={tx("不限", "Unlimited")}
                className="h-8 text-xs font-mono-tech"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("推演步数", "Horizon Steps")}
              </label>
              <Input
                type="number"
                min={1}
                max={104}
                value={horizonSteps}
                onChange={(e) => setHorizonSteps(e.target.value)}
                disabled={simMode !== "STEPPED"}
                className="h-8 text-xs font-mono-tech"
                data-testid="sim-horizon-steps"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                {tx("检查点间隔", "Checkpoint Step")}
              </label>
              <Input
                type="number"
                min={1}
                value={checkpointEvery}
                onChange={(e) => setCheckpointEvery(e.target.value)}
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>

          {/* Uncertainty sampling */}
          <div className="border-t border-border pt-2.5 space-y-2">
            <label className="inline-flex items-center gap-2 text-xs font-medium cursor-pointer">
              <input
                type="checkbox"
                checked={uncertaintyEnabled}
                onChange={(e) => setUncertaintyEnabled(e.target.checked)}
                data-testid="sim-uncertainty-toggle"
              />
              <span>
                {tx(
                  "蒙特卡洛不确定性抽样（P10–P90 置信带）",
                  "Monte Carlo Uncertainty (P10–P90 Band)"
                )}
              </span>
              {uncertaintyEnabled && (
                <Badge variant="xblue">{tx("蒙特卡洛", "MC")}</Badge>
              )}
            </label>

            {uncertaintyEnabled && (
              <div className="grid grid-cols-3 gap-2.5">
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("抽样次数", "Samples")}
                  </label>
                  <Input
                    type="number"
                    min={1}
                    max={500}
                    value={uncSamples}
                    onChange={(e) => setUncSamples(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                    data-testid="sim-uncertainty-samples"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("边权标准差", "Edge CV")}
                  </label>
                  <Input
                    type="number"
                    step="any"
                    min="0.001"
                    value={uncSigmaRel}
                    onChange={(e) => setUncSigmaRel(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                    data-testid="sim-uncertainty-sigma"
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("随机种子", "Random Seed")}
                  </label>
                  <Input
                    type="number"
                    value={uncSeed}
                    onChange={(e) => setUncSeed(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                    data-testid="sim-uncertainty-seed"
                  />
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Scenario Name & Run Buttons */}
        <div className="grid grid-cols-1 md:grid-cols-12 gap-2.5 items-end">
          <div className="md:col-span-4 space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("场景名称", "Scenario Name")}
            </label>
            <Input
              value={scenarioName}
              onChange={(e) => setScenarioName(e.target.value)}
              placeholder={tx("产能下调_15pct", "capacity_cut_15pct")}
              className="h-9 text-xs"
              data-testid="scenario-name-input"
            />
          </div>
          <div className="md:col-span-4 space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {tx("场景说明", "Description")}
            </label>
            <Input
              value={scenarioDesc}
              onChange={(e) => setScenarioDesc(e.target.value)}
              placeholder={tx("华东一厂减产假设", "Factory outage assumption")}
              className="h-9 text-xs"
            />
          </div>
          <div className="md:col-span-4 flex items-center justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              onClick={() => void handleQuickSimulate()}
              disabled={
                !canSimRun || quickRunning || running || nodes.length === 0
              }
              data-testid="quick-simulate-btn"
            >
              <Zap className="h-3.5 w-3.5" />
              {quickRunning
                ? tx("试算中…", "Simulating…")
                : tx("快速试算", "Quick Sim")}
            </Button>
            <Button
              type="submit"
              disabled={
                !canSimRun || running || quickRunning || nodes.length === 0
              }
              data-testid="run-scenario-btn"
            >
              <Play className="h-3.5 w-3.5" />
              {running
                ? tx("求解中…", "Solving…")
                : tx("运行场景", "Run Scenario")}
            </Button>
          </div>
        </div>
      </form>
    </Card>
  );
}
