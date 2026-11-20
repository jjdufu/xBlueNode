import { useEffect, useState } from "react";
import { Play, Plus, Sliders, Trash2, Zap } from "lucide-react";
import {
  axsimApi,
  type AxSimAction,
  type AxSimQuickResult,
  type SandboxGraphNode,
  type ScenarioRecord,
  type SimMode,
  type SimParams,
} from "@/lib/api/axsim";
import type { InterventionMode } from "@/lib/api/enums";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

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

const MODE_OPTIONS: ReadonlyArray<{ value: InterventionMode; label: string }> = [
  { value: "RATIO", label: "RATIO (比例变动)" },
  { value: "ADD", label: "ADD (增量叠加)" },
  { value: "ABSOLUTE", label: "ABSOLUTE (绝对设定)" },
];

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
  const [actions, setActions] = useState<ActionDraft[]>(() => [
    createEmptyAction(nodes),
  ]);
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

  // Sync first empty node when nodes load asynchronously
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
    setActions((prev) => (prev.length > 1 ? prev.filter((r) => r.id !== id) : prev));
  };

  const buildPayload = (): {
    wireActions: AxSimAction[];
    params: SimParams;
  } | null => {
    const wireActions: AxSimAction[] = [];
    for (const draft of actions) {
      if (!draft.target_node_id) {
        setErrorMsg("请为每条干预选择目标节点");
        return null;
      }
      if (!draft.metric_key.trim()) {
        setErrorMsg("请填写干预指标名 (metric_key)");
        return null;
      }
      const numVal = Number.parseFloat(draft.value);
      if (!Number.isFinite(numVal)) {
        setErrorMsg("干预数值必须是有效数字");
        return null;
      }
      if (draft.mode === "RATIO" && numVal <= -1) {
        setErrorMsg("RATIO 模式下变动比例必须大于 -1（例如 -0.3 表示下降 30%）");
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

    const parsedHorizon = Number.parseInt(horizonSteps, 10);
    const parsedCkptEvery = Number.parseInt(checkpointEvery, 10);
    const parsedDamping = Number.parseFloat(damping);
    const parsedMaxHops = maxHops.trim() ? Number.parseInt(maxHops, 10) : null;

    const params: SimParams = {
      mode: simMode,
      damping:
        Number.isFinite(parsedDamping) && parsedDamping > 0 && parsedDamping <= 1
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
        parsedMaxHops !== null && Number.isFinite(parsedMaxHops) && parsedMaxHops >= 1
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

    return { wireActions, params };
  };

  const handleRunScenario = async (e: React.FormEvent) => {
    e.preventDefault();
    setErrorMsg(null);
    const built = buildPayload();
    if (!built) return;

    const effectiveName =
      scenarioName.trim() ||
      `sim-${simMode.toLowerCase()}-${new Date().toISOString().slice(11, 19).replace(/:/g, "")}`;

    setRunning(true);
    try {
      const created = await axsimApi.createScenario(sandboxId, {
        name: effectiveName,
        description: scenarioDesc.trim() || undefined,
        actions: built.wireActions,
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
    setErrorMsg(null);
    const built = buildPayload();
    if (!built) return;
    setQuickRunning(true);
    try {
      const res = await axsimApi.quickSimulate(
        sandboxId,
        built.wireActions,
        built.params
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
          <span>干预设定 (do 算子) 与传导参数</span>
        </div>
        <Button
          type="button"
          variant="outline"
          className="h-7 px-2.5 text-xs"
          onClick={addActionRow}
          data-testid="add-action-btn"
        >
          <Plus className="h-3.5 w-3.5" />
          添加干预项
        </Button>
      </div>

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      <form onSubmit={(e) => void handleRunScenario(e)} className="space-y-4">
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
                      干预目标节点
                    </label>
                    <select
                      value={row.target_node_id}
                      onChange={(e) =>
                        updateAction(row.id, { target_node_id: e.target.value })
                      }
                      className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                      data-testid={`action-node-select-${idx}`}
                    >
                      {nodes.map((n) => (
                        <option key={n.id} value={n.id}>
                          [{n.type_name}] {n.node_key} · {n.title}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="md:col-span-2 space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      指标键 (metric_key)
                    </label>
                    <Input
                      list={`metrics-datalist-${row.id}`}
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
                      干预方式
                    </label>
                    <select
                      value={row.mode}
                      onChange={(e) =>
                        updateAction(row.id, {
                          mode: e.target.value as InterventionMode,
                        })
                      }
                      className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                      data-testid={`action-mode-select-${idx}`}
                    >
                      {MODE_OPTIONS.map((opt) => (
                        <option key={opt.value} value={opt.value}>
                          {opt.label}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="md:col-span-2 space-y-1">
                    <label className="text-[11px] text-muted-foreground flex items-center justify-between">
                      <span>数值</span>
                      {ratioPctLabel && (
                        <span className="font-mono-tech text-xblue">
                          {ratioPctLabel}
                        </span>
                      )}
                    </label>
                    <Input
                      type="number"
                      step="any"
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
                      生效步
                    </label>
                    <Input
                      type="number"
                      min={0}
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
                      disabled={actions.length <= 1}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </div>

                {row.mode === "RATIO" && (
                  <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
                    <span className="text-muted-foreground">快捷比例:</span>
                    {RATIO_PRESETS.map((p) => (
                      <button
                        key={p.value}
                        type="button"
                        onClick={() => updateAction(row.id, { value: p.value })}
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

        {/* Simulation Params */}
        <div className="rounded-md border border-border p-3 space-y-3 bg-muted/10">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-xs font-medium">传导求解参数 (SimParams)</span>
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
                稳态求解 (STEADY)
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
                分步回放 (STEPPED)
              </button>
            </div>
          </div>

          <div className="grid grid-cols-2 md:grid-cols-4 gap-2.5 text-xs">
            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                阻尼系数 (0–1]
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
                最大传导跳数 (留空不限)
              </label>
              <Input
                type="number"
                min={1}
                value={maxHops}
                onChange={(e) => setMaxHops(e.target.value)}
                placeholder="不限"
                className="h-8 text-xs font-mono-tech"
              />
            </div>

            <div className="space-y-1">
              <label className="text-[11px] text-muted-foreground">
                推演步数 (horizon_steps)
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
                检查点间隔 (checkpoint_every)
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
              <span>启用蒙特卡洛不确定性抽样（输出 P10–P90 置信区间）</span>
              {uncertaintyEnabled && <Badge variant="xblue">MC SAMPLING</Badge>}
            </label>

            {uncertaintyEnabled && (
              <div className="grid grid-cols-3 gap-2.5">
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    抽样次数 (samples, 1–500)
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
                    边权扰动标准差 (sigma_rel)
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
                    随机种子 (seed)
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
              场景名称 (用于保存、对比与决策)
            </label>
            <Input
              value={scenarioName}
              onChange={(e) => setScenarioName(e.target.value)}
              placeholder="如：F1_cut_30pct"
              className="h-9 text-xs"
              data-testid="scenario-name-input"
            />
          </div>
          <div className="md:col-span-4 space-y-1">
            <label className="text-[11px] text-muted-foreground">
              备注说明 (可选)
            </label>
            <Input
              value={scenarioDesc}
              onChange={(e) => setScenarioDesc(e.target.value)}
              placeholder="推演假设描述..."
              className="h-9 text-xs"
            />
          </div>
          <div className="md:col-span-4 flex items-center justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              onClick={() => void handleQuickSimulate()}
              disabled={quickRunning || running || nodes.length === 0}
              data-testid="quick-simulate-btn"
            >
              <Zap className="h-3.5 w-3.5" />
              {quickRunning ? "试算中..." : "快速试算"}
            </Button>
            <Button
              type="submit"
              disabled={running || quickRunning || nodes.length === 0}
              data-testid="run-scenario-btn"
            >
              <Play className="h-3.5 w-3.5" />
              {running ? "求解中..." : "创建并运行场景"}
            </Button>
          </div>
        </div>
      </form>
    </Card>
  );
}
