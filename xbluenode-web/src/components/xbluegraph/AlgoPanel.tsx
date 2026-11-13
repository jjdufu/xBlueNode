import { useState } from "react";
import { Cpu, Play } from "lucide-react";
import {
  xblueApi,
  type AlgoRunResponse,
  type AlgoTopEntry,
  type TraversalDirection,
} from "@/lib/api/xblue";
import type { GraphAlgo } from "@/lib/api/enums";
import { Badge, Button, Input } from "@/components/ui/primitives";

const ALGO_OPTIONS: { value: GraphAlgo; label: string }[] = [
  { value: "PAGERANK", label: "PAGERANK · 幂迭代中心性" },
  { value: "DEGREE", label: "DEGREE · 归一化度中心性" },
  { value: "LABEL_PROPAGATION", label: "LABEL_PROPAGATION · 标签传播社区" },
  { value: "CONNECTED_COMPONENTS", label: "CONNECTED_COMPONENTS · 连通分量" },
  { value: "WEAKLY_CONNECTED", label: "WEAKLY_CONNECTED · 弱连通分量" },
  { value: "STRONGLY_CONNECTED", label: "STRONGLY_CONNECTED · 强连通分量" },
];

export type AlgoVisualMode = "NONE" | "SCORE_SIZE" | "COMMUNITY_COLOR";

export interface AlgoPanelProps {
  onApplyVisualMapping: (
    mode: AlgoVisualMode,
    entries: AlgoTopEntry[]
  ) => void;
  onSelectNodeId?: (nodeId: string) => void;
}

export function AlgoPanel({
  onApplyVisualMapping,
  onSelectNodeId,
}: AlgoPanelProps) {
  const [algo, setAlgo] = useState<GraphAlgo>("PAGERANK");
  const [damping, setDamping] = useState("0.85");
  const [maxIter, setMaxIter] = useState("100");
  const [seed, setSeed] = useState("42");
  const [degreeDir, setDegreeDir] = useState<TraversalDirection>("BOTH");
  const [weakMode, setWeakMode] = useState(false);

  const [running, setRunning] = useState(false);
  const [runResult, setRunResult] = useState<AlgoRunResponse | null>(null);
  const [visualMode, setVisualMode] = useState<AlgoVisualMode>("NONE");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const buildAlgoParams = (): Record<string, unknown> => {
    if (algo === "PAGERANK") {
      return {
        damping: Number(damping) || 0.85,
        max_iter: Number(maxIter) || 100,
        tol: 1e-6,
      };
    }
    if (algo === "DEGREE") {
      return { direction: degreeDir };
    }
    if (algo === "LABEL_PROPAGATION") {
      return {
        max_iter: Number(maxIter) || 50,
        seed: Number(seed) || 42,
      };
    }
    if (algo === "CONNECTED_COMPONENTS") {
      return { weak: weakMode };
    }
    return {};
  };

  const handleRun = async (e: React.FormEvent) => {
    e.preventDefault();
    setRunning(true);
    setErrorMsg(null);
    try {
      const res = await xblueApi.runAlgorithm(algo, {
        params: buildAlgoParams(),
      });
      const safeTop = Array.isArray(res?.top) ? res.top : [];
      const normalized: AlgoRunResponse = {
        algo: res.algo ?? algo,
        params_hash: res.params_hash ?? "",
        node_count: res.node_count ?? 0,
        edge_count: res.edge_count ?? 0,
        duration_ms: res.duration_ms ?? 0,
        top: safeTop,
      };
      setRunResult(normalized);

      const defaultMode: AlgoVisualMode =
        algo === "PAGERANK" || algo === "DEGREE"
          ? "SCORE_SIZE"
          : "COMMUNITY_COLOR";
      setVisualMode(defaultMode);
      onApplyVisualMapping(defaultMode, safeTop);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setRunning(false);
    }
  };

  const switchVisual = (mode: AlgoVisualMode) => {
    setVisualMode(mode);
    onApplyVisualMapping(mode, runResult?.top ?? []);
  };

  return (
    <div className="space-y-3 text-xs" data-testid="algo-panel">
      <div className="border-b border-border pb-2">
        <h3 className="font-semibold flex items-center gap-1.5">
          <Cpu className="h-3.5 w-3.5 text-xblue" />
          图算法分析引擎
        </h3>
        <p className="text-[11px] text-muted-foreground mt-0.5">
          基于内存图结构计算中心性与社区划分，支持映射画布节点尺寸与社区分组着色
        </p>
      </div>

      <form onSubmit={(e) => void handleRun(e)} className="space-y-2.5">
        <div className="space-y-1">
          <label className="text-muted-foreground block">选择图算法</label>
          <select
            value={algo}
            onChange={(e) => setAlgo(e.target.value as GraphAlgo)}
            className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
            data-testid="algo-select"
          >
            {ALGO_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
        </div>

        {/* Dynamic Params */}
        {algo === "PAGERANK" && (
          <div className="grid grid-cols-2 gap-2">
            <div className="space-y-1">
              <label className="text-muted-foreground block">阻尼系数</label>
              <Input
                value={damping}
                onChange={(e) => setDamping(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">最大迭代</label>
              <Input
                value={maxIter}
                onChange={(e) => setMaxIter(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
          </div>
        )}

        {algo === "DEGREE" && (
          <div className="space-y-1">
            <label className="text-muted-foreground block">统计方向</label>
            <select
              value={degreeDir}
              onChange={(e) =>
                setDegreeDir(e.target.value as TraversalDirection)
              }
              className="h-8 w-full rounded border border-input bg-background px-2 text-xs font-mono-tech"
            >
              <option value="BOTH">总度 (BOTH)</option>
              <option value="IN">入度 (IN)</option>
              <option value="OUT">出度 (OUT)</option>
            </select>
          </div>
        )}

        {algo === "LABEL_PROPAGATION" && (
          <div className="grid grid-cols-2 gap-2">
            <div className="space-y-1">
              <label className="text-muted-foreground block">最大轮次</label>
              <Input
                value={maxIter}
                onChange={(e) => setMaxIter(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">确定性种子</label>
              <Input
                value={seed}
                onChange={(e) => setSeed(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
          </div>
        )}

        {algo === "CONNECTED_COMPONENTS" && (
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={weakMode}
              onChange={(e) => setWeakMode(e.target.checked)}
            />
            按无向弱连通分量计算 (weak=true)
          </label>
        )}

        {errorMsg && (
          <div className="rounded border border-foreground/30 bg-muted px-2.5 py-1.5">
            {errorMsg}
          </div>
        )}

        <Button
          type="submit"
          className="w-full"
          disabled={running}
          data-testid="algo-run-btn"
        >
          <Play className="h-3.5 w-3.5" />
          {running ? "正在执行图算法..." : "运行算法"}
        </Button>
      </form>

      {runResult && (
        <div className="space-y-2.5 pt-2 border-t border-border" data-testid="algo-results">
          <div className="flex flex-wrap items-center justify-between gap-1 text-[11px] font-mono-tech text-muted-foreground">
            <span>
              节点 {runResult.node_count} · 边 {runResult.edge_count}
            </span>
            <Badge variant="xblue">{runResult.duration_ms} ms</Badge>
          </div>

          {/* Visual Mapping Controls */}
          <div className="flex flex-wrap gap-1.5">
            <Button
              type="button"
              variant={visualMode === "SCORE_SIZE" ? "primary" : "outline"}
              className="px-2 py-1 text-[11px]"
              onClick={() => switchVisual("SCORE_SIZE")}
              data-testid="algo-apply-size"
            >
              按得分映射大小
            </Button>
            <Button
              type="button"
              variant={visualMode === "COMMUNITY_COLOR" ? "primary" : "outline"}
              className="px-2 py-1 text-[11px]"
              onClick={() => switchVisual("COMMUNITY_COLOR")}
              data-testid="algo-apply-community"
            >
              按社区编号着色
            </Button>
            <Button
              type="button"
              variant="ghost"
              className="px-2 py-1 text-[11px]"
              onClick={() => switchVisual("NONE")}
              data-testid="algo-clear-mapping"
            >
              重置映射
            </Button>
          </div>

          {/* Top Ranking List */}
          <div className="max-h-56 overflow-y-auto space-y-1">
            {runResult.top.map((entry, idx) => (
              <button
                key={entry.node_id}
                type="button"
                onClick={() => onSelectNodeId?.(entry.node_id)}
                className="w-full rounded border border-border px-2.5 py-1.5 flex items-center justify-between text-left hover:bg-muted transition-colors cursor-pointer"
                data-testid={`algo-top-row-${idx}`}
              >
                <span className="truncate">
                  <span className="font-mono-tech text-muted-foreground mr-1.5">
                    #{idx + 1}
                  </span>
                  <span className="font-medium">{entry.title}</span>
                </span>
                <span className="font-mono-tech text-[11px] shrink-0 ml-2">
                  {entry.score !== null
                    ? entry.score.toFixed(4)
                    : entry.community !== null
                      ? `C-${entry.community}`
                      : "—"}
                </span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
