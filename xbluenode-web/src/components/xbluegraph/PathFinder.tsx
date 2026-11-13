import { useEffect, useState } from "react";
import { ArrowRight, Route } from "lucide-react";
import {
  xblueApi,
  type BlueNode,
  type PathResponse,
} from "@/lib/api/xblue";
import { Badge, Button } from "@/components/ui/primitives";

export interface PathFinderProps {
  nodes: BlueNode[];
  onPathFound: (result: PathResponse) => void;
  onClearHighlight: () => void;
}

export function PathFinder({
  nodes,
  onPathFound,
  onClearHighlight,
}: PathFinderProps) {
  const [fromId, setFromId] = useState<string>("");
  const [toId, setToId] = useState<string>("");
  const [maxDepth, setMaxDepth] = useState<number>(6);
  const [weighted, setWeighted] = useState<boolean>(false);
  const [loading, setLoading] = useState(false);
  const [pathResult, setPathResult] = useState<PathResponse | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!fromId && nodes[0]) {
      setFromId(nodes[0].id);
    }
    if (!toId && nodes.length > 1 && nodes[nodes.length - 1]) {
      setToId(nodes[nodes.length - 1]?.id ?? "");
    }
  }, [fromId, toId, nodes]);

  const handleFindPath = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!fromId || !toId) {
      setErrorMsg("请选择起点与终点节点");
      return;
    }
    if (fromId === toId) {
      setErrorMsg("起点与终点不能相同");
      return;
    }

    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await xblueApi.queryPath({
        from: fromId,
        to: toId,
        max_depth: maxDepth,
        weighted,
      });
      const normalized: PathResponse = {
        path: Array.isArray(res?.path) ? res.path : [],
        nodes: Array.isArray(res?.nodes) ? res.nodes : [],
        links: Array.isArray(res?.links) ? res.links : [],
        total_cost: res?.total_cost ?? null,
      };
      setPathResult(normalized);
      onPathFound(normalized);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const resolveTitle = (id: string): string => {
    const inResult = pathResult?.nodes.find((n) => n.id === id);
    if (inResult) return `${inResult.title} (${inResult.node_key})`;
    const inCanvas = nodes.find((n) => n.id === id);
    if (inCanvas) return `${inCanvas.title} (${inCanvas.node_key})`;
    return id.slice(0, 8);
  };

  return (
    <form
      onSubmit={(e) => void handleFindPath(e)}
      className="space-y-3 text-xs"
      data-testid="path-finder-panel"
    >
      <div className="border-b border-border pb-2">
        <h3 className="font-semibold flex items-center gap-1.5">
          <Route className="h-3.5 w-3.5 text-xblue" />
          最短路径查找
        </h3>
        <p className="text-[11px] text-muted-foreground mt-0.5">
          双向 BFS 或按边传导权重倒数（1 / weight）执行 Dijkstra 最短路径搜索并在画布高亮
        </p>
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">起点节点 (from)</label>
        <select
          value={fromId}
          onChange={(e) => setFromId(e.target.value)}
          className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs"
          data-testid="path-from-select"
        >
          <option value="">选择起点</option>
          {nodes.map((n) => (
            <option key={n.id} value={n.id}>
              {n.title} ({n.node_key} · {n.type_name})
            </option>
          ))}
        </select>
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">终点节点 (to)</label>
        <select
          value={toId}
          onChange={(e) => setToId(e.target.value)}
          className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs"
          data-testid="path-to-select"
        >
          <option value="">选择终点</option>
          {nodes.map((n) => (
            <option key={n.id} value={n.id}>
              {n.title} ({n.node_key} · {n.type_name})
            </option>
          ))}
        </select>
      </div>

      <div className="grid grid-cols-2 gap-2 items-end">
        <div className="space-y-1">
          <label className="text-muted-foreground block">最大跳数</label>
          <select
            value={maxDepth}
            onChange={(e) => setMaxDepth(Number(e.target.value))}
            className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
            data-testid="path-max-depth-select"
          >
            {[2, 3, 4, 5, 6, 7, 8].map((d) => (
              <option key={d} value={d}>
                {d} 跳
              </option>
            ))}
          </select>
        </div>

        <label className="inline-flex items-center gap-2 py-2 cursor-pointer">
          <input
            type="checkbox"
            checked={weighted}
            onChange={(e) => setWeighted(e.target.checked)}
            data-testid="path-weighted-checkbox"
          />
          按权重 (1 / weight)
        </label>
      </div>

      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-2.5 py-1.5">
          {errorMsg}
        </div>
      )}

      <div className="flex gap-2">
        <Button
          type="submit"
          className="flex-1"
          disabled={loading}
          data-testid="path-find-submit"
        >
          <Route className="h-3.5 w-3.5" />
          {loading ? "寻路计算中..." : "查找最短路径"}
        </Button>

        {pathResult && (
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              setPathResult(null);
              onClearHighlight();
            }}
            data-testid="path-clear-btn"
          >
            清除高亮
          </Button>
        )}
      </div>

      {pathResult && (
        <div
          className="rounded border border-border p-3 space-y-2 bg-muted/20"
          data-testid="path-result-box"
        >
          {pathResult.path.length === 0 ? (
            <div className="text-muted-foreground">
              在最大跳数范围内未找到连通路径。
            </div>
          ) : (
            <>
              <div className="flex items-center justify-between">
                <span className="font-semibold">
                  命中路径（{Math.max(0, pathResult.path.length - 1)} 跳）
                </span>
                {pathResult.total_cost !== null && (
                  <Badge variant="xblue">
                    总代价: {pathResult.total_cost.toFixed(3)}
                  </Badge>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-1.5">
                {pathResult.path.map((nodeId, idx) => (
                  <span
                    key={`${nodeId}-${idx}`}
                    className="inline-flex items-center gap-1"
                  >
                    <Badge variant="neutral">{resolveTitle(nodeId)}</Badge>
                    {idx < pathResult.path.length - 1 && (
                      <ArrowRight className="h-3 w-3 text-xblue" />
                    )}
                  </span>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </form>
  );
}
