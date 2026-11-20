import { Pause, Play, Search, Share2 } from "lucide-react";
import type { BlueNode, NodeSearchHit } from "@/lib/api/xblue";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import type { GraphColorMode } from "./SigmaCanvas";

export interface GraphToolbarProps {
  searchQuery: string;
  onSearchQueryChange: (val: string) => void;
  onSearchSubmit: (e: React.FormEvent) => void;
  searching: boolean;
  searchOpen: boolean;
  onCloseSearch: () => void;
  searchHits: NodeSearchHit[];
  onPickSearchHit: (hit: NodeSearchHit) => void;
  typeFilter: string;
  onTypeFilterChange: (val: string) => void;
  distinctTypes: string[];
  colorMode: GraphColorMode;
  onColorModeChange: (mode: GraphColorMode) => void;
  colorMetricKey: string;
  onColorMetricKeyChange: (key: string) => void;
  availableMetrics: string[];
  fa2Running: boolean;
  onToggleFa2: () => void;
  visibleNodes: BlueNode[];
  visibleEdgeCount: number;
  selectedNodeId: string | null;
  expanding: boolean;
  onSelectNode: (nodeId: string) => void;
  onExpandNode: (nodeId: string) => void;
}

export function GraphToolbar({
  searchQuery,
  onSearchQueryChange,
  onSearchSubmit,
  searching,
  searchOpen,
  onCloseSearch,
  searchHits,
  onPickSearchHit,
  typeFilter,
  onTypeFilterChange,
  distinctTypes,
  colorMode,
  onColorModeChange,
  colorMetricKey,
  onColorMetricKeyChange,
  availableMetrics,
  fa2Running,
  onToggleFa2,
  visibleNodes,
  visibleEdgeCount,
  selectedNodeId,
  expanding,
  onSelectNode,
  onExpandNode,
}: GraphToolbarProps) {
  return (
    <Card className="p-3 space-y-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <form
          onSubmit={onSearchSubmit}
          className="flex flex-1 min-w-[260px] items-center gap-2"
        >
          <Input
            value={searchQuery}
            onChange={(e) => onSearchQueryChange(e.target.value)}
            placeholder="全文 + 向量混合检索业务节点（如 华东仓 / W-1）..."
            className="h-8 text-xs"
            data-testid="graph-search-input"
          />
          <select
            value={typeFilter}
            onChange={(e) => onTypeFilterChange(e.target.value)}
            className="h-8 rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
            data-testid="graph-type-filter"
          >
            <option value="">全部节点类型</option>
            {distinctTypes.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
          <Button
            type="submit"
            variant="outline"
            className="px-3 py-1.5 text-xs"
            disabled={searching}
            data-testid="graph-search-btn"
          >
            <Search className="h-3.5 w-3.5" />
            检索
          </Button>
        </form>

        <div className="flex flex-wrap items-center gap-2 text-xs">
          <select
            value={colorMode}
            onChange={(e) => onColorModeChange(e.target.value as GraphColorMode)}
            className="h-8 rounded-md border border-input bg-background px-2 text-xs"
            data-testid="graph-color-mode"
          >
            <option value="TYPE">按类型着色</option>
            <option value="METRIC">按指标着色</option>
            <option value="ALGO">按图算法结果</option>
          </select>

          {colorMode === "METRIC" && (
            <select
              value={colorMetricKey}
              onChange={(e) => onColorMetricKeyChange(e.target.value)}
              className="h-8 rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
            >
              {availableMetrics.length === 0 ? (
                <option value="">无可用指标</option>
              ) : (
                availableMetrics.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))
              )}
            </select>
          )}

          <Button
            type="button"
            variant="outline"
            className="px-2.5 py-1 text-xs"
            onClick={onToggleFa2}
            data-testid="graph-fa2-toggle"
          >
            {fa2Running ? (
              <Pause className="h-3.5 w-3.5 text-xblue" />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}
            {fa2Running ? "暂停力布局" : "力导布局"}
          </Button>

          <span className="rounded border border-border bg-muted px-2.5 py-1 font-mono-tech">
            节点{" "}
            <strong className="text-xblue" data-testid="canvas-node-count">
              {visibleNodes.length}
            </strong>
          </span>
          <span className="rounded border border-border bg-muted px-2.5 py-1 font-mono-tech">
            关系 <strong data-testid="canvas-edge-count">{visibleEdgeCount}</strong>
          </span>

          <Button
            type="button"
            variant="outline"
            className="px-2.5 py-1 text-xs"
            disabled={!selectedNodeId || expanding}
            onClick={() => {
              if (selectedNodeId) onExpandNode(selectedNodeId);
            }}
            data-testid="expand-node-btn"
          >
            <Share2 className="h-3.5 w-3.5 text-xblue" />
            {expanding ? "展开中..." : "展开选中 1 跳"}
          </Button>
        </div>
      </div>

      {/* Hybrid Search Dropdown Results */}
      {searchOpen && (
        <div
          className="rounded border border-border bg-background p-2 space-y-1.5"
          data-testid="graph-search-results"
        >
          <div className="flex items-center justify-between text-[11px] text-muted-foreground">
            <span>混合检索命中 ({searchHits.length})</span>
            <button
              type="button"
              onClick={onCloseSearch}
              className="hover:text-foreground cursor-pointer"
            >
              收起
            </button>
          </div>
          {searchHits.length === 0 ? (
            <div className="text-xs text-muted-foreground py-1">
              未命中匹配的业务节点。
            </div>
          ) : (
            <div className="flex flex-wrap gap-1.5">
              {searchHits.map((hit) => (
                <button
                  key={hit.id}
                  type="button"
                  onClick={() => onPickSearchHit(hit)}
                  data-testid={`search-hit-${hit.node_key}`}
                  className="rounded border border-border px-2.5 py-1 text-xs flex items-center gap-1.5 hover:border-xblue hover:bg-xblue-soft transition-colors cursor-pointer"
                >
                  <span className="font-medium">{hit.title}</span>
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    ({hit.node_key})
                  </span>
                  <Badge variant="xblue">
                    {(hit.channels ?? []).join("+") || hit.type_name}
                  </Badge>
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Quick Canvas Node Index Strip */}
      {visibleNodes.length > 0 && (
        <div
          className="flex items-center gap-1.5 overflow-x-auto pt-1 border-t border-border"
          data-testid="canvas-node-list"
        >
          <span className="text-[11px] text-muted-foreground shrink-0">
            画布节点（单击查看详情 / 双击展开 1 跳）:
          </span>
          {visibleNodes.slice(0, 30).map((node) => {
            const active = node.id === selectedNodeId;
            return (
              <button
                key={node.id}
                type="button"
                onClick={() => onSelectNode(node.id)}
                onDoubleClick={() => {
                  onSelectNode(node.id);
                  onExpandNode(node.id);
                }}
                data-testid={`graph-node-${node.node_key}`}
                className={`shrink-0 rounded border px-2 py-0.5 text-[11px] font-mono-tech transition-colors cursor-pointer ${
                  active
                    ? "border-xblue bg-xblue-soft text-xblue font-medium"
                    : "border-border bg-background hover:bg-muted"
                }`}
              >
                {node.title} ({node.node_key})
              </button>
            );
          })}
        </div>
      )}
    </Card>
  );
}
