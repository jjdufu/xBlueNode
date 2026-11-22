import { useEffect, useState } from "react";
import { Share2 } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueApi,
  type BlueNode,
  type SubgraphResponse,
  type TraversalDirection,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Input,
  NumberInput,
  Select,
} from "@/components/ui/primitives";

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
  const { tx } = useI18n();
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
      setErrorMsg(tx("需指定起始节点", "Start node required"));
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
        tx(
          `已合并 ${safeNodes.length} 节点 · ${safeLinks.length} 关系${
            res?.truncated ? "（截断）" : ""
          }`,
          `Merged ${safeNodes.length} nodes · ${safeLinks.length} edges${
            res?.truncated ? " (truncated)" : ""
          }`
        )
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
          {tx("多跳邻居扩展", "Multi-Hop Expansion")}
        </h3>
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">
          {tx("中心节点", "Center Node")}
        </label>
        <Select
          value={targetId}
          onChange={(e) => setTargetId(e.target.value)}
          className="w-full"
          data-testid="neighbor-target-select"
        >
          <option value="">{tx("未指定", "None")}</option>
          {nodes.map((n) => (
            <option key={n.id} value={n.id}>
              {n.title} ({n.node_key} · {n.type_name})
            </option>
          ))}
        </Select>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("扩展跳数", "Hops")}
          </label>
          <Select
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            className="w-full font-mono-tech"
            data-testid="neighbor-depth-select"
          >
            {[1, 2, 3, 4].map((d) => (
              <option key={d} value={d}>
                {tx(`${d} 跳`, `${d} Hops`)}
              </option>
            ))}
          </Select>
        </div>

        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("遍历方向", "Direction")}
          </label>
          <Select
            value={direction}
            onChange={(e) => setDirection(e.target.value as TraversalDirection)}
            className="w-full font-mono-tech"
            data-testid="neighbor-direction-select"
          >
            <option value="BOTH">{tx("双向", "Both")}</option>
            <option value="OUT">{tx("下游出边", "Outbound")}</option>
            <option value="IN">{tx("上游入边", "Inbound")}</option>
          </Select>
        </div>
      </div>

      <div className="space-y-1">
        <label className="text-muted-foreground block">
          {tx("关系类型过滤", "Relation Filter")}
        </label>
        <Input
          value={linkTypesInput}
          onChange={(e) => setLinkTypesInput(e.target.value)}
          placeholder="SUPPLIES,FULFILLS"
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
                    linkTypesInput
                      .split(",")
                      .map((s) => s.trim())
                      .includes(lt)
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
        <label className="text-muted-foreground block">
          {tx("返回节点上限", "Node Limit")}
        </label>
        <NumberInput
          min={10}
          max={2000}
          step={10}
          value={limit}
          onChange={setLimit}
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
        {loading
          ? tx("扩展中…", "Expanding…")
          : tx("扩展邻居", "Expand Neighbors")}
      </Button>
    </form>
  );
}
