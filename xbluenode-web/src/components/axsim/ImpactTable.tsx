import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Route, Search } from "lucide-react";
import {
  axsimApi,
  type AxSimQuickResult,
  type ImpactRow,
  type SandboxGraphNode,
  type ScenarioRecord,
} from "@/lib/api/axsim";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export interface ImpactTableProps {
  scenario: ScenarioRecord | null;
  quickResult?: AxSimQuickResult | null;
  nodes?: SandboxGraphNode[];
  highlightedPath?: string[];
  onHighlightPath?: (path: string[]) => void;
  onSelectNode?: (nodeId: string) => void;
  onImpactsLoaded?: (rows: ImpactRow[]) => void;
}

function formatNumber(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "—";
  return Number.isInteger(val) ? String(val) : val.toFixed(2);
}

function formatSignedDelta(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "—";
  const absFormatted = Number.isInteger(val) ? String(val) : val.toFixed(2);
  return val > 0 ? `+${absFormatted}` : absFormatted;
}

function formatRatioPct(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "—";
  const pct = val * 100;
  return `${pct > 0 ? "+" : ""}${pct.toFixed(1)}%`;
}

function escapeCsvCell(raw: string): string {
  if (raw.includes(",") || raw.includes('"') || raw.includes("\n")) {
    return `"${raw.replace(/"/g, '""')}"`;
  }
  return raw;
}

export function ImpactTable({
  scenario,
  quickResult = null,
  nodes = [],
  highlightedPath = [],
  onHighlightPath,
  onSelectNode,
  onImpactsLoaded,
}: ImpactTableProps) {
  const [rows, setRows] = useState<ImpactRow[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);

  const [searchText, setSearchText] = useState("");
  const [typeFilter, setTypeFilter] = useState("");
  const [metricFilter, setMetricFilter] = useState("");
  const [minAbsDelta, setMinAbsDelta] = useState("");

  const nodeTitleMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const n of nodes) {
      map.set(n.id, n.title || n.node_key);
    }
    return map;
  }, [nodes]);

  const fetchFirstPage = useCallback(async () => {
    if (!scenario) {
      if (quickResult && Array.isArray(quickResult.impacts)) {
        const mapped: ImpactRow[] = quickResult.impacts.map((r) => ({
          node_id: r.node_id,
          type_name: r.type_name,
          title: r.title,
          metric: r.metric_key,
          before_val: r.before_val,
          after_val: r.after_val,
          delta: r.delta,
          delta_ratio:
            r.delta_ratio !== undefined
              ? r.delta_ratio
              : r.before_val !== 0
                ? r.delta / r.before_val
                : null,
          p10: null,
          p90: null,
          hop_distance: r.hop_distance,
          top_path: Array.isArray(r.top_path) ? r.top_path : [],
        }));
        setRows(mapped);
        setNextCursor(null);
        onImpactsLoaded?.(mapped);
      } else {
        setRows([]);
        setNextCursor(null);
        onImpactsLoaded?.([]);
      }
      return;
    }

    setLoading(true);
    try {
      const page = await axsimApi.listImpacts(scenario.id, {
        type_name: typeFilter.trim() || undefined,
        metric: metricFilter.trim() || undefined,
        limit: 50,
      });
      const fetchedItems = Array.isArray(page?.items) ? page.items : [];
      const fallbackTop = Array.isArray(scenario.result?.top_impacts)
        ? scenario.result.top_impacts
        : [];
      const finalItems =
        fetchedItems.length > 0 || typeFilter.trim() || metricFilter.trim()
          ? fetchedItems
          : fallbackTop;
      setRows(finalItems);
      setNextCursor(
        typeof page?.next_cursor === "string" ? page.next_cursor : null
      );
      onImpactsLoaded?.(finalItems);
    } catch {
      const fallbackTop = Array.isArray(scenario.result?.top_impacts)
        ? scenario.result.top_impacts
        : [];
      setRows(fallbackTop);
      setNextCursor(null);
      onImpactsLoaded?.(fallbackTop);
    } finally {
      setLoading(false);
    }
  }, [metricFilter, onImpactsLoaded, quickResult, scenario, typeFilter]);

  useEffect(() => {
    void fetchFirstPage();
  }, [fetchFirstPage]);

  const handleLoadMore = async () => {
    if (!scenario || !nextCursor) return;
    setLoadingMore(true);
    try {
      const page = await axsimApi.listImpacts(scenario.id, {
        type_name: typeFilter.trim() || undefined,
        metric: metricFilter.trim() || undefined,
        limit: 50,
        cursor: nextCursor,
      });
      const moreItems = Array.isArray(page?.items) ? page.items : [];
      setRows((prev) => {
        const merged = [...prev, ...moreItems];
        onImpactsLoaded?.(merged);
        return merged;
      });
      setNextCursor(
        typeof page?.next_cursor === "string" ? page.next_cursor : null
      );
    } finally {
      setLoadingMore(false);
    }
  };

  const availableTypes = useMemo(
    () => Array.from(new Set(rows.map((r) => r.type_name))).sort(),
    [rows]
  );

  const availableMetrics = useMemo(
    () => Array.from(new Set(rows.map((r) => r.metric))).sort(),
    [rows]
  );

  const filteredRows = useMemo(() => {
    const q = searchText.trim().toLowerCase();
    const minDelta = minAbsDelta.trim()
      ? Number.parseFloat(minAbsDelta)
      : Number.NaN;

    return rows.filter((r) => {
      if (
        typeFilter.trim() &&
        r.type_name.toLowerCase() !== typeFilter.trim().toLowerCase()
      ) {
        return false;
      }
      if (
        metricFilter.trim() &&
        r.metric.toLowerCase() !== metricFilter.trim().toLowerCase()
      ) {
        return false;
      }
      if (
        Number.isFinite(minDelta) &&
        Math.abs(r.delta ?? 0) < minDelta
      ) {
        return false;
      }
      if (q) {
        const hay =
          `${r.title} ${r.node_id} ${r.type_name} ${r.metric}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [metricFilter, minAbsDelta, rows, searchText, typeFilter]);

  const exportCsv = () => {
    const header = [
      "node_id",
      "type_name",
      "title",
      "metric",
      "before_val",
      "after_val",
      "delta",
      "delta_ratio",
      "p10",
      "p90",
      "hop_distance",
      "top_path",
    ];
    const lines = [
      header.join(","),
      ...filteredRows.map((r) =>
        [
          escapeCsvCell(r.node_id),
          escapeCsvCell(r.type_name),
          escapeCsvCell(r.title),
          escapeCsvCell(r.metric),
          r.before_val ?? "",
          r.after_val,
          r.delta ?? "",
          r.delta_ratio ?? "",
          r.p10 ?? "",
          r.p90 ?? "",
          r.hop_distance,
          escapeCsvCell((r.top_path ?? []).join("->")),
        ].join(",")
      ),
    ];
    const blob = new Blob(["\uFEFF" + lines.join("\n")], {
      type: "text/csv;charset=utf-8;",
    });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `axsim-impacts-${scenario?.name ?? "quick"}.csv`;
    link.click();
    URL.revokeObjectURL(url);
  };

  const pathKey = highlightedPath.join("->");

  return (
    <Card className="p-4 space-y-3" data-testid="impact-table-card">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <h3 className="text-sm font-medium">因果影响传导明细表</h3>
          <Badge variant="neutral">{filteredRows.length} 行</Badge>
          {scenario?.is_stale && (
            <Badge variant="outline" className="border-amber-500/60 text-amber-600 dark:text-amber-400">
              覆盖层已变更 · 结果过期
            </Badge>
          )}
        </div>

        <Button
          type="button"
          variant="outline"
          className="h-7 px-2.5 text-xs"
          onClick={exportCsv}
          disabled={filteredRows.length === 0}
          data-testid="impact-export-csv-btn"
        >
          <Download className="h-3.5 w-3.5" />
          导出 CSV
        </Button>
      </div>

      {/* Filters */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2">
        <div className="relative">
          <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2.5 top-2.5" />
          <Input
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            placeholder="搜索节点名 / ID..."
            className="h-8 pl-8 text-xs"
            data-testid="impact-search-input"
          />
        </div>

        <Input
          list="impact-type-options"
          value={typeFilter}
          onChange={(e) => setTypeFilter(e.target.value)}
          placeholder="过滤类型 type_name..."
          className="h-8 text-xs font-mono-tech"
          data-testid="impact-filter-type"
        />
        <datalist id="impact-type-options">
          {availableTypes.map((t) => (
            <option key={t} value={t} />
          ))}
        </datalist>

        <Input
          list="impact-metric-options"
          value={metricFilter}
          onChange={(e) => setMetricFilter(e.target.value)}
          placeholder="过滤指标 metric_key..."
          className="h-8 text-xs font-mono-tech"
          data-testid="impact-filter-metric"
        />
        <datalist id="impact-metric-options">
          {availableMetrics.map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>

        <Input
          type="number"
          step="any"
          min="0"
          value={minAbsDelta}
          onChange={(e) => setMinAbsDelta(e.target.value)}
          placeholder="最小绝对变化 |delta|..."
          className="h-8 text-xs font-mono-tech"
          data-testid="impact-filter-min-delta"
        />
      </div>

      <div className="overflow-x-auto">
        <table className="w-full text-xs" data-testid="impact-table">
          <thead>
            <tr className="border-b border-border text-left text-muted-foreground">
              <th className="py-2 pr-3">节点名称</th>
              <th className="py-2 pr-3">类型</th>
              <th className="py-2 pr-3">指标</th>
              <th className="py-2 pr-3 text-right">基线值</th>
              <th className="py-2 pr-3 text-right">推演后</th>
              <th className="py-2 pr-3 text-right">变化量 (Δ)</th>
              <th className="py-2 pr-3 text-right">变化率 (%)</th>
              <th className="py-2 pr-3 text-right">P10–P90 置信带</th>
              <th className="py-2 pr-3">跳数</th>
              <th className="py-2">主传导路径 (top_path)</th>
            </tr>
          </thead>
          <tbody>
            {loading && (
              <tr>
                <td colSpan={10} className="py-6 text-center text-muted-foreground">
                  加载传导影响明细中...
                </td>
              </tr>
            )}
            {!loading &&
              filteredRows.map((row) => {
                const deltaVal = row.delta ?? 0;
                const deltaTone =
                  deltaVal > 0
                    ? "text-emerald-600 dark:text-emerald-400"
                    : deltaVal < 0
                      ? "text-red-600 dark:text-red-400"
                      : "text-muted-foreground";
                const rowPath = Array.isArray(row.top_path) ? row.top_path : [];
                const isPathActive =
                  rowPath.length > 0 && rowPath.join("->") === pathKey;

                return (
                  <tr
                    key={`${row.node_id}-${row.metric}`}
                    data-testid={`impact-row-${row.type_name}-${row.metric}`}
                    className={`border-b border-border/60 hover:bg-muted/30 ${
                      isPathActive ? "bg-xblue-soft/40" : ""
                    }`}
                  >
                    <td className="py-2 pr-3 font-medium">
                      <button
                        type="button"
                        onClick={() => onSelectNode?.(row.node_id)}
                        className="text-left hover:text-xblue cursor-pointer"
                      >
                        {row.title}
                      </button>
                    </td>
                    <td className="py-2 pr-3">
                      <Badge variant="outline">{row.type_name}</Badge>
                    </td>
                    <td className="py-2 pr-3 font-mono-tech">{row.metric}</td>
                    <td className="py-2 pr-3 text-right font-mono-tech text-muted-foreground">
                      {formatNumber(row.before_val)}
                    </td>
                    <td className="py-2 pr-3 text-right font-mono-tech font-medium">
                      {formatNumber(row.after_val)}
                    </td>
                    <td className={`py-2 pr-3 text-right font-mono-tech font-medium ${deltaTone}`}>
                      {formatSignedDelta(row.delta)}
                    </td>
                    <td className={`py-2 pr-3 text-right font-mono-tech ${deltaTone}`}>
                      {formatRatioPct(row.delta_ratio)}
                    </td>
                    <td className="py-2 pr-3 text-right font-mono-tech text-muted-foreground">
                      {row.p10 != null && row.p90 != null
                        ? `${formatNumber(row.p10)} ~ ${formatNumber(row.p90)}`
                        : "—"}
                    </td>
                    <td className="py-2 pr-3 font-mono-tech">
                      {row.hop_distance} hop
                    </td>
                    <td className="py-2">
                      {rowPath.length > 0 ? (
                        <button
                          type="button"
                          onClick={() => onHighlightPath?.(rowPath)}
                          data-testid={`highlight-path-${row.node_id}`}
                          className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[11px] font-mono-tech cursor-pointer ${
                            isPathActive
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground hover:text-foreground"
                          }`}
                        >
                          <Route className="h-3 w-3" />
                          {rowPath
                            .map((id) => nodeTitleMap.get(id) ?? id.slice(0, 6))
                            .join(" → ")}
                        </button>
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            {!loading && filteredRows.length === 0 && (
              <tr>
                <td colSpan={10} className="py-6 text-center text-muted-foreground">
                  {scenario
                    ? "当前筛选条件下暂无受影响指标行"
                    : "运行推演场景或选择已有场景后，将在此展示影响明细"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {nextCursor && (
        <div className="flex justify-center pt-1">
          <Button
            type="button"
            variant="outline"
            className="h-8 text-xs"
            onClick={() => void handleLoadMore()}
            disabled={loadingMore}
            data-testid="impact-load-more-btn"
          >
            {loadingMore ? "加载中..." : "加载更多影响明细"}
          </Button>
        </div>
      )}
    </Card>
  );
}
