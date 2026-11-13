import { useEffect, useState } from "react";
import { Share2 } from "lucide-react";
import {
  xblueApi,
  type BlueNode,
  type SubgraphResponse,
  type TraversalDirection,
} from "@/lib/api/xblue";
import { Badge, Button, Input } from "@/components/ui/primitives";

export interface NeighborExpandProps {
  nodes: BlueNode[];
  selectedNodeId: string | null;
  knownLinkTypes: string[];
  onMergeSubgraph: (res: SubgraphResponse) => void;
}

export function NeighborExpand({
  nodes,
  selectedNodeId,
  knownLinkTypes,
  onMergeSubgraph,
}: NeighborExpandProps) {
  const [targetId, setTargetId] = useState<string>(selectedNodeId ?? "");
  const [depth, setDepth] = useState<number>(1);
  const [direction, setDirection] = useState<TraversalDirection>("BOTH");
  const [linkTypesInput, setLinkTypesInput] = useState<string>("");
  const [limit, setLimit] = useState<number>(200);
  const [loading, setLoading] = useState(false);
  const [resultSummary, setResultSummary] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    if (selectedNodeId) {
      setTargetId(selectedNodeId);
    } else if (!targetId && nodes[0]) {
      setTargetId(nodes[0].id);
    }
  }, [selectedNodeId, nodes, targetId]);

  const toggleLinkTypeTag = (lt: string) => {
    const current = linkTypesInput
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (current.includes(lt)) {
      setLinkTypesInput(current.filter((item) => item !== lt).join(","));
    } else {
      setLinkTypesInput([...current, lt].join(","));
    }
  };

  const handleExpand = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!targetId) {
      setErrorMsg("请先选择起始业务节点");
      return;
    }
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await xblueApi.queryNeighbors({
        node_id: targetId,
        depth,
        direction,
        link_types: linkTypesInput.trim() || undefined,
        limit,
      });
      const safeNodes = Array.isArray(res?.nodes) ? res.nodes : [];
      const safeLinks = Array.isArray(res?.links) ? res.links : [];
      onMergeSubgraph({
        nodes: safeNodes,
        links: safeLinks,
        truncated: Boolean(res?.truncated),
      });
      setResultSummary(
        `已返回 ${safeNodes.length} 个邻居节点、${safeLinks.length} 条关系边${
          res?.truncated ? "（已达截断上限）" : ""
        }`
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <form
      onSubmit={(e) => void handleExpand(e)}
      className="space-y-3 text-xs"
      data-testid="neighbor-expand-panel"
    >
      <div className="border-b border-border pb-2">
        <h3 className="font-semibold flex items-center gap-1.5">
          <Share2 className="h-3.5 w-3.5 text-xblue" />
          多跳邻居扩展
        </h3>
        <p className="text-[11px] text-muted-foreground mt-0.5">
          支持 1～4 跳有向或双向遍历及关系类型过滤，增量去重合并至当前画布
        </p>
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">中心节点</label>
        <select
          value={targetId}
          onChange={(e) => setTargetId(e.target.value)}
          className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs"
          data-testid="neighbor-target-select"
        >
          <option value="">请选择节点</option>
          {nodes.map((n) => (
            <option key={n.id} value={n.id}>
              {n.title} ({n.node_key} · {n.type_name})
            </option>
          ))}
        </select>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div className="space-y-1">
          <label className="text-muted-foreground block">扩展跳数 (1..4)</label>
          <select
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
            data-testid="neighbor-depth-select"
          >
            {[1, 2, 3, 4].map((d) => (
              <option key={d} value={d}>
                {d} 跳
              </option>
            ))}
          </select>
        </div>

        <div className="space-y-1">
          <label className="text-muted-foreground block">遍历方向</label>
          <select
            value={direction}
            onChange={(e) => setDirection(e.target.value as TraversalDirection)}
            className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
            data-testid="neighbor-direction-select"
          >
            <option value="BOTH">双向 (BOTH)</option>
            <option value="OUT">出边下游 (OUT)</option>
            <option value="IN">入边上游 (IN)</option>
          </select>
        </div>
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">
          关系类型过滤（逗号分隔，留空不限）
        </label>
        <Input
          value={linkTypesInput}
          onChange={(e) => setLinkTypesInput(e.target.value)}
          placeholder="例如 SUPPLIES,FULFILLS"
          className="font-mono-tech text-xs"
          data-testid="neighbor-link-types-input"
        />
        {knownLinkTypes.length > 0 && (
          <div className="flex flex-wrap gap-1 pt-1">
            {knownLinkTypes.map((lt) => (
              <button
                key={lt}
                type="button"
                onClick={() => toggleLinkTypeTag(lt)}
                className="cursor-pointer"
              >
                <Badge
                  variant={
                    linkTypesInput.split(",").map((s) => s.trim()).includes(lt)
                      ? "xblue"
                      : "outline"
                  }
                >
                  {lt}
                </Badge>
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">返回节点上限</label>
        <Input
          type="number"
          min={10}
          max={2000}
          value={limit}
          onChange={(e) => setLimit(Number(e.target.value) || 200)}
          className="font-mono-tech text-xs"
        />
      </div>

      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-2.5 py-1.5">
          {errorMsg}
        </div>
      )}

      {resultSummary && (
        <div
          className="rounded border border-xblue/30 bg-xblue-soft px-2.5 py-1.5 text-xblue"
          data-testid="neighbor-expand-summary"
        >
          {resultSummary}
        </div>
      )}

      <Button
        type="submit"
        className="w-full"
        disabled={loading}
        data-testid="neighbor-expand-submit"
      >
        <Share2 className="h-3.5 w-3.5" />
        {loading ? "正在查询并合并邻居..." : "执行多跳扩展并合并画布"}
      </Button>
    </form>
  );
}
