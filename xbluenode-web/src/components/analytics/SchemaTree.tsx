import { useMemo, useState } from "react";
import {
  Cable,
  ChevronDown,
  ChevronRight,
  Database,
  Layers,
  Plus,
  RefreshCw,
  Search,
  ShieldAlert,
  TableProperties,
} from "lucide-react";
import type { AnalyticsCatalogResponse } from "@/lib/api/analytics";
import { Badge, Input } from "@/components/ui/primitives";

export interface SchemaTreeProps {
  catalog: AnalyticsCatalogResponse;
  loading?: boolean;
  selectedPivotDataset?: string;
  onRefresh?: () => void;
  onInsertIdentifier?: (identifier: string) => void;
  onSelectForPivot?: (datasetOrViewName: string) => void;
}

export function SchemaTree({
  catalog,
  loading = false,
  selectedPivotDataset,
  onRefresh,
  onInsertIdentifier,
  onSelectForPivot,
}: SchemaTreeProps) {
  const [search, setSearch] = useState("");
  const [expandedItems, setExpandedItems] = useState<Record<string, boolean>>({});

  const normalizedSearch = search.trim().toLowerCase();

  const filteredDatasets = useMemo(() => {
    const list = Array.isArray(catalog.datasets) ? catalog.datasets : [];
    if (!normalizedSearch) return list;
    return list.filter((ds) => {
      if (
        ds.name.toLowerCase().includes(normalizedSearch) ||
        ds.display_name.toLowerCase().includes(normalizedSearch)
      ) {
        return true;
      }
      return ds.columns.some(
        (col) =>
          col.column_name.toLowerCase().includes(normalizedSearch) ||
          col.business_term.toLowerCase().includes(normalizedSearch) ||
          col.description.toLowerCase().includes(normalizedSearch)
      );
    });
  }, [catalog.datasets, normalizedSearch]);

  const filteredViews = useMemo(() => {
    const list = Array.isArray(catalog.platform_views)
      ? catalog.platform_views
      : [];
    if (!normalizedSearch) return list;
    return list.filter((v) => {
      if (
        v.name.toLowerCase().includes(normalizedSearch) ||
        v.description.toLowerCase().includes(normalizedSearch)
      ) {
        return true;
      }
      return v.columns.some((c) =>
        c.name.toLowerCase().includes(normalizedSearch)
      );
    });
  }, [catalog.platform_views, normalizedSearch]);

  const filteredLive = useMemo(() => {
    const list = Array.isArray(catalog.live_connections)
      ? catalog.live_connections
      : [];
    if (!normalizedSearch) return list;
    return list.filter(
      (c) =>
        c.name.toLowerCase().includes(normalizedSearch) ||
        c.alias.toLowerCase().includes(normalizedSearch) ||
        c.connector_type.toLowerCase().includes(normalizedSearch)
    );
  }, [catalog.live_connections, normalizedSearch]);

  const isExpanded = (key: string, defaultOpen = false): boolean => {
    if (normalizedSearch.length > 0) return true;
    if (key in expandedItems) return Boolean(expandedItems[key]);
    return defaultOpen;
  };

  const toggleExpand = (key: string, defaultOpen = false) => {
    setExpandedItems((prev) => ({
      ...prev,
      [key]: !(key in prev ? Boolean(prev[key]) : defaultOpen),
    }));
  };

  return (
    <div
      className="flex h-full flex-col rounded-md border border-border bg-card overflow-hidden"
      data-testid="analytics-schema-tree"
    >
      {/* Tree Header */}
      <div className="flex items-center justify-between border-b border-border bg-muted/30 px-3 py-2">
        <div className="flex items-center gap-1.5 text-xs font-semibold">
          <Database className="h-3.5 w-3.5 text-xblue" />
          <span>资产对象目录</span>
        </div>
        {onRefresh && (
          <button
            type="button"
            onClick={onRefresh}
            disabled={loading}
            title="刷新目录"
            className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer disabled:opacity-40"
            data-testid="schema-tree-refresh-btn"
          >
            <RefreshCw className={`h-3 w-3 ${loading ? "animate-spin" : ""}`} />
          </button>
        )}
      </div>

      {/* Search Box */}
      <div className="border-b border-border p-2">
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-2 h-3.5 w-3.5 text-muted-foreground" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="过滤表名、列名、业务术语..."
            className="h-7 pl-8 text-xs"
            data-testid="schema-tree-search"
          />
        </div>
      </div>

      {/* Scrollable Sections */}
      <div className="flex-1 overflow-y-auto p-2 space-y-3 text-xs">
        {/* 1. Datasets */}
        <div>
          <div className="mb-1 flex items-center justify-between px-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            <span>DuckLake 数据集</span>
            <span className="font-mono-tech">{filteredDatasets.length}</span>
          </div>

          {filteredDatasets.length === 0 ? (
            <div className="px-2 py-2 text-[11px] text-muted-foreground">
              暂无匹配的数据集
            </div>
          ) : (
            <div className="space-y-1">
              {filteredDatasets.map((ds, idx) => {
                const key = `ds:${ds.id}`;
                const open = isExpanded(key, idx === 0);
                const isSelectedPivot = selectedPivotDataset === ds.name;
                return (
                  <div
                    key={ds.id}
                    className={`rounded border ${
                      isSelectedPivot ? "border-xblue/40 bg-xblue-soft/20" : "border-border/70"
                    }`}
                    data-testid={`schema-dataset-${ds.name}`}
                  >
                    <div className="flex items-center justify-between gap-1 px-2 py-1.5 hover:bg-muted/50">
                      <button
                        type="button"
                        onClick={() => toggleExpand(key, idx === 0)}
                        className="flex min-w-0 flex-1 items-center gap-1 text-left cursor-pointer"
                      >
                        {open ? (
                          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
                        ) : (
                          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
                        )}
                        <span className="truncate font-mono-tech font-medium text-foreground">
                          {ds.name}
                        </span>
                        {ds.display_name && ds.display_name !== ds.name && (
                          <span className="truncate text-[10px] text-muted-foreground">
                            {ds.display_name}
                          </span>
                        )}
                      </button>

                      <div className="flex shrink-0 items-center gap-1">
                        <span className="font-mono-tech text-[10px] text-muted-foreground">
                          {ds.row_count}
                        </span>
                        {onInsertIdentifier && (
                          <button
                            type="button"
                            onClick={() => onInsertIdentifier(ds.name)}
                            title={`插入 ${ds.name} 到 SQL`}
                            data-testid={`schema-insert-${ds.name}`}
                            className="rounded px-1 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
                          >
                            <Plus className="h-3 w-3" />
                          </button>
                        )}
                        {onSelectForPivot && (
                          <button
                            type="button"
                            onClick={() => onSelectForPivot(ds.name)}
                            title={`对 ${ds.name} 进行透视`}
                            data-testid={`schema-pivot-${ds.name}`}
                            className="rounded px-1 py-0.5 text-[10px] text-xblue hover:bg-xblue-soft cursor-pointer"
                          >
                            <TableProperties className="h-3 w-3" />
                          </button>
                        )}
                      </div>
                    </div>

                    {open && ds.columns.length > 0 && (
                      <div className="border-t border-border/60 bg-background/60 px-2 py-1 space-y-0.5">
                        {ds.columns.map((col) => (
                          <button
                            key={col.column_name}
                            type="button"
                            onClick={() =>
                              onInsertIdentifier?.(col.column_name)
                            }
                            data-testid={`schema-col-${ds.name}-${col.column_name}`}
                            className="flex w-full items-center justify-between gap-1 rounded px-1.5 py-1 text-left hover:bg-muted/60 cursor-pointer"
                          >
                            <div className="flex min-w-0 items-center gap-1.5">
                              <span className="truncate font-mono-tech text-[11px] text-foreground">
                                {col.column_name}
                              </span>
                              {col.business_term && (
                                <Badge
                                  variant="outline"
                                  className="px-1 py-0 text-[9px]"
                                >
                                  {col.business_term}
                                </Badge>
                              )}
                              {col.pii_category !== "NONE" && (
                                <Badge
                                  variant="xblue"
                                  className="px-1 py-0 text-[9px]"
                                  data-testid={`schema-pii-${ds.name}-${col.column_name}`}
                                >
                                  <ShieldAlert className="mr-0.5 h-2.5 w-2.5" />
                                  {col.pii_category}
                                </Badge>
                              )}
                            </div>
                            <span className="shrink-0 font-mono-tech text-[10px] text-muted-foreground">
                              {col.data_type}
                            </span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* 2. Platform Federation Views */}
        <div>
          <div className="mb-1 flex items-center justify-between px-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            <span className="inline-flex items-center gap-1">
              <Layers className="h-3 w-3 text-xblue" />
              平台联邦视图 (ax.*)
            </span>
            <span className="font-mono-tech">{filteredViews.length}</span>
          </div>

          {filteredViews.length === 0 ? (
            <div className="px-2 py-1.5 text-[11px] text-muted-foreground">
              暂无联邦视图
            </div>
          ) : (
            <div className="space-y-1">
              {filteredViews.map((view) => {
                const key = `view:${view.name}`;
                const open = isExpanded(key, false);
                return (
                  <div
                    key={view.name}
                    className="rounded border border-border/70"
                    data-testid={`schema-view-${view.name}`}
                  >
                    <div className="flex items-center justify-between gap-1 px-2 py-1.5 hover:bg-muted/50">
                      <button
                        type="button"
                        onClick={() => toggleExpand(key, false)}
                        className="flex min-w-0 flex-1 items-center gap-1 text-left cursor-pointer"
                      >
                        {open ? (
                          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
                        ) : (
                          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
                        )}
                        <span className="truncate font-mono-tech font-medium text-foreground">
                          {view.name}
                        </span>
                      </button>

                      <div className="flex shrink-0 items-center gap-1">
                        {onInsertIdentifier && (
                          <button
                            type="button"
                            onClick={() => onInsertIdentifier(view.name)}
                            title={`插入 ${view.name}`}
                            className="rounded px-1 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
                          >
                            <Plus className="h-3 w-3" />
                          </button>
                        )}
                        {onSelectForPivot && (
                          <button
                            type="button"
                            onClick={() => onSelectForPivot(view.name)}
                            title={`对 ${view.name} 进行透视`}
                            className="rounded px-1 py-0.5 text-[10px] text-xblue hover:bg-xblue-soft cursor-pointer"
                          >
                            <TableProperties className="h-3 w-3" />
                          </button>
                        )}
                      </div>
                    </div>

                    {open && view.columns.length > 0 && (
                      <div className="border-t border-border/60 bg-background/60 px-2 py-1 space-y-0.5">
                        {view.description && (
                          <div className="px-1.5 py-0.5 text-[10px] text-muted-foreground">
                            {view.description}
                          </div>
                        )}
                        {view.columns.map((col) => (
                          <button
                            key={col.name}
                            type="button"
                            onClick={() => onInsertIdentifier?.(col.name)}
                            className="flex w-full items-center justify-between gap-1 rounded px-1.5 py-1 text-left hover:bg-muted/60 cursor-pointer"
                          >
                            <span className="truncate font-mono-tech text-[11px] text-foreground">
                              {col.name}
                            </span>
                            <span className="shrink-0 font-mono-tech text-[10px] text-muted-foreground">
                              {col.data_type}
                            </span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* 3. Live External Connections */}
        <div>
          <div className="mb-1 flex items-center justify-between px-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            <span className="inline-flex items-center gap-1">
              <Cable className="h-3 w-3 text-xblue" />
              直连关系库 (live_*)
            </span>
            <span className="font-mono-tech">{filteredLive.length}</span>
          </div>

          {filteredLive.length === 0 ? (
            <div className="px-2 py-1.5 text-[11px] text-muted-foreground">
              暂无启用直连的外部库
            </div>
          ) : (
            <div className="space-y-1">
              {filteredLive.map((live) => (
                <button
                  key={live.id}
                  type="button"
                  onClick={() => onInsertIdentifier?.(live.alias)}
                  data-testid={`schema-live-${live.alias}`}
                  className="flex w-full items-center justify-between gap-1 rounded border border-border/70 px-2 py-1.5 text-left hover:bg-muted/50 cursor-pointer"
                >
                  <div className="min-w-0">
                    <div className="truncate font-mono-tech text-[11px] font-medium text-foreground">
                      {live.alias}
                    </div>
                    <div className="truncate text-[10px] text-muted-foreground">
                      {live.name}
                    </div>
                  </div>
                  <Badge variant="neutral" className="shrink-0 text-[10px]">
                    {live.connector_type}
                  </Badge>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
