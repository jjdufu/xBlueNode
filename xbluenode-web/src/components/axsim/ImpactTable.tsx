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
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";

export interface ImpactTableProps {
  scenario: ScenarioRecord | null;
  quickResult?: AxSimQuickResult | null;
  nodes?: SandboxGraphNode[];
  highlightedPath?: string[];
  onHighlightPath?: (path: string[]) => void;
  onSelectNode?: (nodeId: string) => void;
  onImpactsLoaded?: (rows: ImpactRow[]) => void;
}

function formatRatioPct(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "-";
  const pct = val * 100;
  return `${pct > 0 ? "+" : ""}${pct.toFixed(1)}%`;
}

function formatConfidenceBound(
  val: number | null | undefined,
  fmtNum: (n: number | null | undefined, maxDecimals?: number) => string
): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "-";
  if (Math.abs(val) > 0 && Math.abs(val) < 1) {
    return fmtNum(val, 4);
  }
  return val.toFixed(2);
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
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canExport = can("EXPORT_DATA");

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
      if (Number.isFinite(minDelta) && Math.abs(r.delta ?? 0) < minDelta) {
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

  const formatSignedMetric = (val: number | null | undefined): string => {
    if (val === null || val === undefined || !Number.isFinite(val)) return "-";
    const formatted = fmtNum(val, 4);
    return val > 0 ? `+${formatted}` : formatted;
  };

  const exportCsv = () => {
    if (!canExport) return;
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
          <h3 className="text-sm font-medium">
            {tx("因果影响传导明细", "Impact Breakdown")}
          </h3>
          <Badge variant="neutral">
            {tx(`${filteredRows.length} 行`, `${filteredRows.length} rows`)}
          </Badge>
          {scenario?.is_stale && (
            <Badge variant="warning">
              {tx("覆盖层已变更 · 已过期", "Overlay Changed · Stale")}
            </Badge>
          )}
        </div>

        {canExport && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={exportCsv}
            disabled={filteredRows.length === 0}
            data-testid="impact-export-csv-btn"
          >
            <Download className="h-3.5 w-3.5" />
            {tx("导出 CSV", "Export CSV")}
          </Button>
        )}
      </div>

      {/* Filters */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2">
        <div className="relative">
          <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2.5 top-2.5" />
          <Input
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            placeholder={tx("节点名称 / ID", "Node title / ID")}
            className="h-8 pl-8 text-xs"
            data-testid="impact-search-input"
          />
        </div>

        <Input
          list="impact-type-options"
          value={typeFilter}
          onChange={(e) => setTypeFilter(e.target.value)}
          placeholder="Order / Warehouse"
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
          placeholder="fulfillment / inventory"
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
          placeholder={tx("最小绝对变化 |Δ|", "Min |Δ|")}
          className="h-8 text-xs font-mono-tech"
          data-testid="impact-filter-min-delta"
        />
      </div>

      <div className="overflow-x-auto">
        <table className="w-full text-xs" data-testid="impact-table">
          <thead>
            <tr className="border-b border-border text-left text-muted-foreground">
              <th className="py-2 pr-3">{tx("节点名称", "Node")}</th>
              <th className="py-2 pr-3">{tx("类型", "Type")}</th>
              <th className="py-2 pr-3">{tx("指标", "Metric")}</th>
              <th className="py-2 pr-3 text-right">
                {tx("基线值", "Before")}
              </th>
              <th className="py-2 pr-3 text-right">
                {tx("推演后", "After")}
              </th>
              <th className="py-2 pr-3 text-right">{tx("变化量", "Delta")}</th>
              <th className="py-2 pr-3 text-right">
                {tx("变化率", "Delta %")}
              </th>
              <th className="py-2 pr-3 text-right">
                {tx("P10–P90 置信带", "P10–P90 Band")}
              </th>
              <th className="py-2 pr-3">{tx("跳数", "Hops")}</th>
              <th className="py-2">{tx("主导传导路径", "Top Path")}</th>
            </tr>
          </thead>
          <tbody>
            {loading && (
              <tr>
                <td
                  colSpan={10}
                  className="py-6 text-center text-muted-foreground"
                >
                  {tx("加载传导明细中…", "Loading impact rows…")}
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
                const rowPath = Array.isArray(row.top_path)
                  ? row.top_path
                  : [];
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
                      {fmtNum(row.before_val, 4)}
                    </td>
                    <td className="py-2 pr-3 text-right font-mono-tech font-medium">
                      {fmtNum(row.after_val, 4)}
                    </td>
                    <td
                      className={`py-2 pr-3 text-right font-mono-tech font-medium ${deltaTone}`}
                    >
                      {formatSignedMetric(row.delta)}
                    </td>
                    <td
                      className={`py-2 pr-3 text-right font-mono-tech ${deltaTone}`}
                    >
                      {formatRatioPct(row.delta_ratio)}
                    </td>
                    <td className="py-2 pr-3 text-right font-mono-tech text-muted-foreground">
                      {row.p10 != null && row.p90 != null
                        ? `${formatConfidenceBound(row.p10, fmtNum)} ~ ${formatConfidenceBound(row.p90, fmtNum)}`
                        : "-"}
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
                        <span className="text-muted-foreground">-</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            {!loading && filteredRows.length === 0 && (
              <tr>
                <td
                  colSpan={10}
                  className="py-6 text-center text-muted-foreground"
                >
                  {scenario
                    ? tx("暂无受影响指标行", "No affected metric rows")
                    : tx("运行或选择推演场景后查看明细", "Run or select a scenario")}
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
            size="sm"
            onClick={() => void handleLoadMore()}
            disabled={loadingMore}
            data-testid="impact-load-more-btn"
          >
            {loadingMore
              ? tx("加载中…", "Loading…")
              : tx("加载更多", "Load More")}
          </Button>
        </div>
      )}
    </Card>
  );
}
