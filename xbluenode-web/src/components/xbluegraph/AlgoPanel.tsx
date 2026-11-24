import { useState } from "react";
import { Cpu, Play } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueApi,
  type AlgoRunResponse,
  type AlgoTopEntry,
  type TraversalDirection,
} from "@/lib/api/xblue";
import type { GraphAlgo } from "@/lib/api/enums";
import { Badge, Button, Input, Select, Switch } from "@/components/ui/primitives";

const ALGO_OPTIONS: readonly {
  value: GraphAlgo;
  zh: string;
  en: string;
}[] = [
  { value: "PAGERANK", zh: "PageRank 中心性", en: "PageRank Centrality" },
  { value: "DEGREE", zh: "归一化度中心性", en: "Degree Centrality" },
  {
    value: "LABEL_PROPAGATION",
    zh: "标签传播社区发现",
    en: "Label Propagation",
  },
  {
    value: "CONNECTED_COMPONENTS",
    zh: "连通分量分析",
    en: "Connected Components",
  },
  {
    value: "WEAKLY_CONNECTED",
    zh: "弱连通分量",
    en: "Weakly Connected",
  },
  {
    value: "STRONGLY_CONNECTED",
    zh: "强连通分量",
    en: "Strongly Connected",
  },
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
  const { tx, tEnum, fmtNum } = useI18n();
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
        backend: res.backend ?? "PETGRAPH_MEMORY",
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
          {tx("图算法分析", "Graph Algorithms")}
        </h3>
      </div>

      <form onSubmit={(e) => void handleRun(e)} className="space-y-2.5">
        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("算法模型", "Algorithm")}
          </label>
          <Select
            value={algo}
            onChange={(e) => setAlgo(e.target.value as GraphAlgo)}
            className="w-full font-mono-tech"
            data-testid="algo-select"
          >
            {ALGO_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {tx(opt.zh, opt.en)}
              </option>
            ))}
          </Select>
        </div>

        {algo === "PAGERANK" && (
          <div className="grid grid-cols-2 gap-2">
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("阻尼系数", "Damping")}
              </label>
              <Input
                value={damping}
                onChange={(e) => setDamping(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("最大迭代", "Max Iterations")}
              </label>
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
            <label className="text-muted-foreground block">
              {tx("统计方向", "Direction")}
            </label>
            <Select
              value={degreeDir}
              onChange={(e) =>
                setDegreeDir(e.target.value as TraversalDirection)
              }
              className="h-8 w-full font-mono-tech"
            >
              <option value="BOTH">{tx("总度", "Total Degree")}</option>
              <option value="IN">{tx("入度", "In-Degree")}</option>
              <option value="OUT">{tx("出度", "Out-Degree")}</option>
            </Select>
          </div>
        )}

        {algo === "LABEL_PROPAGATION" && (
          <div className="grid grid-cols-2 gap-2">
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("最大轮次", "Max Rounds")}
              </label>
              <Input
                value={maxIter}
                onChange={(e) => setMaxIter(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("随机种子", "Random Seed")}
              </label>
              <Input
                value={seed}
                onChange={(e) => setSeed(e.target.value)}
                className="h-8 font-mono-tech text-xs"
              />
            </div>
          </div>
        )}

        {algo === "CONNECTED_COMPONENTS" && (
          <div className="flex items-center justify-between rounded-md bg-muted/30 px-2.5 py-1.5">
            <span>{tx("无向弱连通模式", "Weakly Connected Mode")}</span>
            <Switch
              checked={weakMode}
              onCheckedChange={setWeakMode}
              aria-label={tx("无向弱连通模式", "Weakly Connected Mode")}
            />
          </div>
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
          {running
            ? tx("运行中…", "Running…")
            : tx("运行算法", "Run Algorithm")}
        </Button>
      </form>

      {runResult && (
        <div
          className="space-y-2.5 pt-2 border-t border-border"
          data-testid="algo-results"
        >
          <div className="flex flex-wrap items-center justify-between gap-1 text-[11px] font-mono-tech text-muted-foreground">
            <span>
              {tx("节点", "Nodes")} {fmtNum(runResult.node_count, 0)} ·{" "}
              {tx("关系", "Edges")} {fmtNum(runResult.edge_count, 0)}
            </span>
            <div className="flex items-center gap-1">
              <Badge variant="outline" data-testid="algo-backend-badge">
                {tEnum(runResult.backend ?? "PETGRAPH_MEMORY")}
              </Badge>
              <Badge variant="xblue">{runResult.duration_ms} ms</Badge>
            </div>
          </div>

          <div className="flex flex-wrap gap-1.5">
            <Button
              type="button"
              size="sm"
              variant={visualMode === "SCORE_SIZE" ? "primary" : "outline"}
              onClick={() => switchVisual("SCORE_SIZE")}
              data-testid="algo-apply-size"
            >
              {tx("得分映射尺寸", "Scale by Score")}
            </Button>
            <Button
              type="button"
              size="sm"
              variant={visualMode === "COMMUNITY_COLOR" ? "primary" : "outline"}
              onClick={() => switchVisual("COMMUNITY_COLOR")}
              data-testid="algo-apply-community"
            >
              {tx("社区着色", "Color Communities")}
            </Button>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => switchVisual("NONE")}
              data-testid="algo-clear-mapping"
            >
              {tx("重置", "Reset")}
            </Button>
          </div>

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
