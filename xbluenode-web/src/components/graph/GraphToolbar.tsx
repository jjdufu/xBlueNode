import { useEffect, useMemo, useRef, useState } from "react";
import { ListTree, Pause, Play, Search, X } from "lucide-react";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import type { BlueNode, NodeSearchHit } from "@/lib/api/xblue";
import { Badge, Button, Card, Input, Select } from "@/components/ui/primitives";
import {
  resolveNodeTypeColor,
  translateNodeTypeLabel,
  type GraphColorMode,
} from "./SigmaCanvas";

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
  hideColorModeControls?: boolean;
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
  selectedNodeId,
  onSelectNode,
  onExpandNode,
  hideColorModeControls = false,
}: GraphToolbarProps) {
  const { isZh, tx } = useI18n();
  const { isDark } = useTheme();

  const [outlineOpen, setOutlineOpen] = useState(false);
  const [outlineFilter, setOutlineFilter] = useState("");
  const outlineRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!outlineOpen) return;
    const onDown = (ev: MouseEvent) => {
      if (
        outlineRef.current &&
        !outlineRef.current.contains(ev.target as Node)
      ) {
        setOutlineOpen(false);
      }
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") setOutlineOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [outlineOpen]);

  const focusedNode = useMemo(
    () =>
      visibleNodes.find((n) => n.id === selectedNodeId) ??
      visibleNodes[0] ??
      null,
    [visibleNodes, selectedNodeId]
  );

  const filteredOutlineNodes = useMemo(() => {
    const q = outlineFilter.trim().toLowerCase();
    if (!q) return visibleNodes;
    return visibleNodes.filter(
      (n) =>
        n.title.toLowerCase().includes(q) ||
        n.node_key.toLowerCase().includes(q) ||
        n.type_name.toLowerCase().includes(q)
    );
  }, [visibleNodes, outlineFilter]);

  return (
    <Card className="shrink-0 px-3 py-2 space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <form
          onSubmit={onSearchSubmit}
          className="flex flex-wrap items-center gap-2 min-w-0"
        >
          <div className="relative w-52 sm:w-64">
            <Search className="pointer-events-none absolute left-2.5 top-2.25 h-3.5 w-3.5 text-muted-foreground" />
            <Input
              value={searchQuery}
              onChange={(e) => onSearchQueryChange(e.target.value)}
              placeholder={tx(
                "搜索节点名称或主键 (如 华东仓)",
                "Search node title or key"
              )}
              className="pl-8"
              data-testid="graph-search-input"
            />
          </div>

          <Select
            value={typeFilter}
            onChange={(e) => onTypeFilterChange(e.target.value)}
            className="w-40"
            data-testid="graph-type-filter"
          >
            <option value="">{tx("全部节点类型", "All Types")}</option>
            {distinctTypes.map((t, idx) => (
              <option
                key={t}
                value={t}
                data-dot={resolveNodeTypeColor(t, idx, isDark)}
              >
                {translateNodeTypeLabel(t, isZh)}
              </option>
            ))}
          </Select>

          <Button
            type="submit"
            variant="outline"
            disabled={searching}
            data-testid="graph-search-btn"
          >
            {tx("检索", "Search")}
          </Button>
        </form>

        <div className="flex flex-wrap items-center gap-2 text-xs">
          {/* Active Focused Node Quick Pill + Collapsible Node Outline Popover */}
          {focusedNode && (
            <button
              type="button"
              onClick={() => onSelectNode(focusedNode.id)}
              onDoubleClick={() => {
                onSelectNode(focusedNode.id);
                onExpandNode(focusedNode.id);
              }}
              title={tx("双击展开 1 跳邻居", "Double-click to expand 1-hop")}
              data-testid={
                focusedNode.node_key
                  ? `graph-node-${focusedNode.node_key}`
                  : undefined
              }
              className="inline-flex h-8 items-center gap-1.5 rounded-md bg-xblue-soft/75 px-2.5 text-xs font-medium text-xblue cursor-pointer max-w-[220px]"
            >
              <span className="truncate">{focusedNode.title}</span>
              {focusedNode.node_key && (
                <span className="font-mono-tech text-[10px] opacity-80 shrink-0">
                  {focusedNode.node_key}
                </span>
              )}
            </button>
          )}

          <div ref={outlineRef} className="relative">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setOutlineOpen((v) => !v)}
              className={outlineOpen ? "bg-muted text-foreground" : undefined}
              data-testid="canvas-node-list"
            >
              <ListTree className="h-3.5 w-3.5 text-xblue" />
              <span>{tx("节点大纲", "Outline")}</span>
            </Button>

            {outlineOpen && (
              <div className="absolute right-0 top-full mt-1.5 w-72 rounded-md border border-border bg-card text-card-foreground shadow-lg z-40 overflow-hidden">
                <div className="flex items-center gap-1.5 border-b border-border px-2.5 py-2 bg-muted/20">
                  <Search className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                  <input
                    type="text"
                    value={outlineFilter}
                    onChange={(e) => setOutlineFilter(e.target.value)}
                    placeholder={tx("过滤画布节点…", "Filter canvas nodes…")}
                    className="w-full bg-transparent text-xs focus:outline-none"
                  />
                  {outlineFilter && (
                    <button
                      type="button"
                      onClick={() => setOutlineFilter("")}
                      className="text-muted-foreground hover:text-foreground cursor-pointer"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  )}
                </div>
                <div className="max-h-64 overflow-y-auto p-1 space-y-0.5">
                  {filteredOutlineNodes.map((n, idx) => {
                    const active = n.id === selectedNodeId;
                    return (
                      <button
                        key={n.id}
                        type="button"
                        onClick={() => {
                          onSelectNode(n.id);
                          setOutlineOpen(false);
                        }}
                        onDoubleClick={() => {
                          onSelectNode(n.id);
                          onExpandNode(n.id);
                          setOutlineOpen(false);
                        }}
                        className={`w-full flex items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-xs cursor-pointer ${
                          active
                            ? "bg-xblue-soft text-xblue font-medium"
                            : "hover:bg-muted/70 text-foreground"
                        }`}
                      >
                        <span className="inline-flex items-center gap-1.5 min-w-0 truncate">
                          <span
                            className="h-2 w-2 rounded-full shrink-0"
                            style={{
                              backgroundColor: resolveNodeTypeColor(
                                n.type_name,
                                idx,
                                isDark
                              ),
                            }}
                          />
                          <span className="truncate">{n.title}</span>
                        </span>
                        {n.node_key && (
                          <span className="font-mono-tech text-[10px] text-muted-foreground shrink-0">
                            {n.node_key}
                          </span>
                        )}
                      </button>
                    );
                  })}
                </div>
              </div>
            )}
          </div>

          {!hideColorModeControls && (
            <>
              <Select
                value={colorMode}
                onChange={(e) =>
                  onColorModeChange(e.target.value as GraphColorMode)
                }
                className="w-32"
                data-testid="graph-color-mode"
              >
                <option value="TYPE">{tx("按类型着色", "By Type")}</option>
                <option value="METRIC">{tx("按指标着色", "By Metric")}</option>
                <option value="ALGO">{tx("按算法结果", "By Algorithm")}</option>
              </Select>

              {colorMode === "METRIC" && (
                <Select
                  value={colorMetricKey}
                  onChange={(e) => onColorMetricKeyChange(e.target.value)}
                  className="w-32 font-mono-tech"
                >
                  {availableMetrics.length === 0 ? (
                    <option value="">{tx("无可用指标", "No Metrics")}</option>
                  ) : (
                    availableMetrics.map((m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ))
                  )}
                </Select>
              )}
            </>
          )}

          <Button
            type="button"
            variant="outline"
            onClick={onToggleFa2}
            data-testid="graph-fa2-toggle"
          >
            {fa2Running ? (
              <Pause className="h-3.5 w-3.5 text-xblue" />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}
            {fa2Running
              ? tx("暂停布局", "Pause")
              : tx("力导布局", "Force Layout")}
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
            <span>
              {tx("检索命中节点", "Search Hits")} ({searchHits.length})
            </span>
            <button
              type="button"
              onClick={onCloseSearch}
              className="hover:text-foreground cursor-pointer"
            >
              {tx("收起", "Hide")}
            </button>
          </div>
          {searchHits.length === 0 ? (
            <div className="text-xs text-muted-foreground py-1">
              {tx("未命中匹配节点", "No matching nodes")}
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
                    {translateNodeTypeLabel(hit.type_name, isZh)}
                  </Badge>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </Card>
  );
}
