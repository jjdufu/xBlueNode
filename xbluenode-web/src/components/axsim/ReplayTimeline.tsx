import { useEffect, useMemo, useState } from "react";
import { Pause, Play, RotateCcw, SkipBack, SkipForward } from "lucide-react";
import {
  axsimApi,
  type Checkpoint,
  type SandboxGraphNode,
  type ScenarioRecord,
} from "@/lib/api/axsim";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

export interface ReplayTimelineProps {
  scenario: ScenarioRecord | null;
  nodes: SandboxGraphNode[];
  onCheckpointChange: (checkpoint: Checkpoint | null) => void;
}

export function ReplayTimeline({
  scenario,
  nodes,
  onCheckpointChange,
}: ReplayTimelineProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const [checkpoints, setCheckpoints] = useState<Checkpoint[]>([]);
  const [stepIdx, setStepIdx] = useState<number>(0);
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);

  const nodeMap = useMemo(() => {
    const map = new Map<string, SandboxGraphNode>();
    for (const n of nodes) {
      map.set(n.id, n);
    }
    return map;
  }, [nodes]);

  useEffect(() => {
    setPlaying(false);
    if (!scenario) {
      setCheckpoints([]);
      setStepIdx(0);
      onCheckpointChange(null);
      return;
    }

    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const res = await axsimApi.listCheckpoints(scenario.id);
        const items = Array.isArray(res?.items) ? res.items : [];
        if (!cancelled) {
          setCheckpoints(items);
          const initialIdx = items.length > 0 ? items.length - 1 : 0;
          setStepIdx(initialIdx);
          onCheckpointChange(items[initialIdx] ?? null);
        }
      } catch {
        if (!cancelled) {
          setCheckpoints([]);
          setStepIdx(0);
          onCheckpointChange(null);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [onCheckpointChange, scenario]);

  useEffect(() => {
    if (!playing || checkpoints.length <= 1) return;
    const timer = window.setInterval(() => {
      setStepIdx((prev) => {
        const next = prev + 1;
        if (next >= checkpoints.length) {
          setPlaying(false);
          return prev;
        }
        return next;
      });
    }, 800);
    return () => window.clearInterval(timer);
  }, [checkpoints.length, playing]);

  useEffect(() => {
    if (checkpoints.length === 0) {
      onCheckpointChange(null);
    } else {
      onCheckpointChange(checkpoints[stepIdx] ?? null);
    }
  }, [checkpoints, onCheckpointChange, stepIdx]);

  const activeCheckpoint = checkpoints[stepIdx] ?? null;

  const sparklineStats = useMemo(() => {
    return checkpoints.map((cp) => {
      let changedCount = 0;
      let maxAbsVal = 0;
      for (const metrics of Object.values(cp.changes ?? {})) {
        for (const val of Object.values(metrics)) {
          changedCount += 1;
          if (Math.abs(val) > maxAbsVal) maxAbsVal = Math.abs(val);
        }
      }
      return { seq: cp.seq, label: cp.label, changedCount, maxAbsVal };
    });
  }, [checkpoints]);

  const maxBar = useMemo(
    () => Math.max(1, ...sparklineStats.map((s) => s.changedCount)),
    [sparklineStats]
  );

  const changeRows = useMemo(() => {
    if (!activeCheckpoint) return [];
    const list: Array<{
      nodeId: string;
      title: string;
      typeName: string;
      metric: string;
      baseVal: number | null;
      stepVal: number;
      delta: number | null;
    }> = [];
    for (const [nodeId, metrics] of Object.entries(
      activeCheckpoint.changes ?? {}
    )) {
      const node = nodeMap.get(nodeId);
      for (const [metric, val] of Object.entries(metrics)) {
        const baseVal =
          node && typeof node.metrics?.[metric] === "number"
            ? node.metrics[metric]
            : null;
        list.push({
          nodeId,
          title: node?.title ?? nodeId.slice(0, 8),
          typeName: node?.type_name ?? "Node",
          metric,
          baseVal,
          stepVal: val,
          delta: baseVal !== null ? val - baseVal : null,
        });
      }
    }
    return list;
  }, [activeCheckpoint, nodeMap]);

  if (!scenario) {
    return null;
  }

  const modeCode = scenario.params?.mode ?? "STEADY";
  const isSingleOrEmpty = checkpoints.length <= 1;

  // Compact steady-state summary bar when STEADY or single checkpoint (t=0)
  if (!loading && isSingleOrEmpty) {
    return (
      <Card
        className="px-4 py-2.5 flex flex-wrap items-center justify-between gap-2 text-xs"
        data-testid="replay-timeline-card"
      >
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">
            {tx("传导回放", "Propagation Replay")}
          </span>
          <Badge variant="outline">{tEnum(modeCode)}</Badge>
          {activeCheckpoint && (
            <Badge variant="xblue" data-testid="replay-current-step-badge">
              t = {activeCheckpoint.seq} · {activeCheckpoint.label}
            </Badge>
          )}
          {scenario.result && (
            <span className="font-mono-tech text-[11px] text-muted-foreground">
              {tx(
                `收敛轮次 ${scenario.result.iterations} · 受影响节点 ${scenario.result.affected_count}`,
                `Iterations ${scenario.result.iterations} · Affected ${scenario.result.affected_count}`
              )}
            </span>
          )}
        </div>
        <span className="text-[11px] text-muted-foreground">
          {tx(
            "稳态终值已输出；切换「分步演化」可按时间步回放传导波次",
            "Steady-state equilibrium reached; use Time-Stepped mode to replay waves"
          )}
        </span>
      </Card>
    );
  }

  return (
    <Card className="p-4 space-y-3" data-testid="replay-timeline-card">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <h3 className="text-sm font-medium">
            {tx("传导检查点回放", "Propagation Replay")}
          </h3>
          <Badge variant="outline">{tEnum(modeCode)}</Badge>
          {activeCheckpoint && (
            <Badge variant="xblue" data-testid="replay-current-step-badge">
              t = {activeCheckpoint.seq} · {activeCheckpoint.label}
            </Badge>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 px-2"
            onClick={() => {
              setPlaying(false);
              setStepIdx(0);
            }}
            disabled={checkpoints.length === 0 || stepIdx === 0}
            aria-label={tx("回到起点", "First step")}
          >
            <RotateCcw className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 px-2"
            onClick={() => {
              setPlaying(false);
              setStepIdx((i) => Math.max(0, i - 1));
            }}
            disabled={checkpoints.length === 0 || stepIdx <= 0}
            aria-label={tx("上一步", "Previous step")}
          >
            <SkipBack className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant={playing ? "outline" : "primary"}
            size="sm"
            onClick={() => {
              if (!playing && stepIdx >= checkpoints.length - 1) {
                setStepIdx(0);
              }
              setPlaying((p) => !p);
            }}
            disabled={checkpoints.length <= 1}
            data-testid="replay-play-toggle"
          >
            {playing ? (
              <>
                <Pause className="h-3.5 w-3.5" />
                {tx("暂停", "Pause")}
              </>
            ) : (
              <>
                <Play className="h-3.5 w-3.5" />
                {tx("播放", "Play")}
              </>
            )}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 px-2"
            onClick={() => {
              setPlaying(false);
              setStepIdx((i) => Math.min(checkpoints.length - 1, i + 1));
            }}
            disabled={
              checkpoints.length === 0 || stepIdx >= checkpoints.length - 1
            }
            aria-label={tx("下一步", "Next step")}
          >
            <SkipForward className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {loading ? (
        <div className="py-3 text-xs text-muted-foreground">
          {tx("加载检查点序列中…", "Loading checkpoints…")}
        </div>
      ) : (
        <div className="space-y-3">
          {/* Slider */}
          <div className="space-y-1.5">
            <input
              type="range"
              min={0}
              max={Math.max(0, checkpoints.length - 1)}
              step={1}
              value={stepIdx}
              onChange={(e) => {
                setPlaying(false);
                setStepIdx(Number.parseInt(e.target.value, 10) || 0);
              }}
              className="w-full accent-[#2563eb] cursor-pointer"
              data-testid="replay-step-slider"
            />
            <div className="flex justify-between text-[10px] font-mono-tech text-muted-foreground">
              {checkpoints.map((cp, idx) => (
                <button
                  key={cp.seq}
                  type="button"
                  onClick={() => {
                    setPlaying(false);
                    setStepIdx(idx);
                  }}
                  className={`cursor-pointer hover:text-foreground ${
                    idx === stepIdx ? "text-xblue font-semibold" : ""
                  }`}
                >
                  t={cp.seq}
                </button>
              ))}
            </div>
          </div>

          {/* Sparkline bars per checkpoint */}
          <div className="flex items-end gap-1.5 h-10 rounded border border-border bg-muted/20 px-2.5 py-1.5">
            {sparklineStats.map((st, idx) => {
              const heightPct = Math.max(
                18,
                Math.round((st.changedCount / maxBar) * 100)
              );
              return (
                <button
                  key={st.seq}
                  type="button"
                  onClick={() => {
                    setPlaying(false);
                    setStepIdx(idx);
                  }}
                  title={tx(
                    `${st.label}: ${st.changedCount} 项变化`,
                    `${st.label}: ${st.changedCount} changes`
                  )}
                  className={`flex-1 rounded-t transition-colors cursor-pointer ${
                    idx === stepIdx
                      ? "bg-xblue"
                      : "bg-muted-foreground/30 hover:bg-muted-foreground/50"
                  }`}
                  style={{ height: `${heightPct}%` }}
                />
              );
            })}
          </div>

          {/* Step diff readout */}
          <div className="space-y-1.5">
            <div className="text-[11px] font-medium text-muted-foreground">
              {tx("步长变更快照", "Step Changes")} (
              {activeCheckpoint?.label ?? ""})
            </div>
            {changeRows.length === 0 ? (
              <div className="text-xs text-muted-foreground">
                {tx("该时间步无新增指标变化", "No metric changes at this step")}
              </div>
            ) : (
              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2">
                {changeRows.map((row) => (
                  <div
                    key={`${row.nodeId}-${row.metric}`}
                    data-testid={`replay-diff-${row.nodeId}-${row.metric}`}
                    className="rounded border border-border bg-background px-2.5 py-1.5 text-xs flex items-center justify-between gap-2"
                  >
                    <div className="truncate">
                      <span className="font-medium">{row.title}</span>
                      <span className="ml-1.5 font-mono-tech text-[11px] text-muted-foreground">
                        .{row.metric}
                      </span>
                    </div>
                    <div className="font-mono-tech text-[11px] shrink-0">
                      <span className="font-semibold">
                        {fmtNum(row.stepVal, 4)}
                      </span>
                      {row.delta !== null && (
                        <span
                          className={`ml-1.5 ${
                            row.delta < 0
                              ? "text-red-600 dark:text-red-400"
                              : row.delta > 0
                                ? "text-emerald-600 dark:text-emerald-400"
                                : "text-muted-foreground"
                          }`}
                        >
                          ({row.delta > 0 ? "+" : ""}
                          {fmtNum(row.delta, 4)})
                        </span>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
    </Card>
  );
}
