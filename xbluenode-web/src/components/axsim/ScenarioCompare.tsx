import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  GitCompare,
  Sparkles,
} from "lucide-react";
import {
  axsimApi,
  type CompareScenariosResponse,
  type ScenarioRecord,
} from "@/lib/api/axsim";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

export interface ScenarioCompareProps {
  sandboxId: string;
  scenarios: ScenarioRecord[];
}

function formatRatio(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) return "-";
  const pct = val * 100;
  return `${pct > 0 ? "+" : ""}${pct.toFixed(1)}%`;
}

export function ScenarioCompare({
  sandboxId,
  scenarios,
}: ScenarioCompareProps) {
  const { tx, fmtNum } = useI18n();
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [typeFilter, setTypeFilter] = useState("");
  const [metricsFilter, setMetricsFilter] = useState("");
  const [compareData, setCompareData] =
    useState<CompareScenariosResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [briefingLoading, setBriefingLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    setCompareData(null);
    const validIds = new Set(scenarios.map((s) => s.id));
    setSelectedIds((prev) => {
      const kept = prev.filter((id) => validIds.has(id));
      if (kept.length >= 2) return kept;
      return scenarios.slice(0, Math.min(2, scenarios.length)).map((s) => s.id);
    });
  }, [sandboxId, scenarios]);

  const toggleScenario = (id: string) => {
    setSelectedIds((prev) => {
      if (prev.includes(id)) {
        return prev.filter((item) => item !== id);
      }
      if (prev.length >= 5) {
        return prev;
      }
      return [...prev, id];
    });
  };

  const runCompare = async (withBriefing: boolean) => {
    if (selectedIds.length < 2) {
      setErrorMsg(
        tx(
          "请勾选至少 2 个方案（最多 5 个）",
          "Select 2 to 5 scenarios to compare"
        )
      );
      return;
    }
    setErrorMsg(null);
    if (withBriefing) {
      setBriefingLoading(true);
    } else {
      setLoading(true);
    }

    try {
      const metricsArr = metricsFilter
        .split(",")
        .map((m) => m.trim())
        .filter((m) => m.length > 0);
      const res = await axsimApi.compareScenarios({
        ids: selectedIds,
        ...(typeFilter.trim() ? { type_name: typeFilter.trim() } : {}),
        ...(metricsArr.length > 0 ? { metrics: metricsArr } : {}),
        briefing: withBriefing,
      });
      setCompareData(res);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
      setBriefingLoading(false);
    }
  };

  const scenarioNameMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const s of scenarios) {
      map.set(s.id, s.name);
    }
    if (compareData) {
      for (const col of compareData.scenarios) {
        map.set(col.id, col.name);
      }
    }
    return map;
  }, [compareData, scenarios]);

  return (
    <Card className="p-4 space-y-4" data-testid="scenario-compare-card">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2 text-sm font-medium">
          <GitCompare className="h-4 w-4 text-xblue" />
          <span>{tx("多方案并排对比", "Scenario Comparison")}</span>
        </div>
        <Badge variant="outline">
          {tx("2–5 个方案", "2–5 Scenarios")}
        </Badge>
      </div>

      {/* Scenario checkboxes */}
      <div className="space-y-2">
        <div className="text-xs text-muted-foreground">
          {tx(
            `待对比场景（已选 ${selectedIds.length}/5）：`,
            `Scenarios (${selectedIds.length}/5 selected):`
          )}
        </div>
        {scenarios.length === 0 ? (
          <div className="text-xs text-muted-foreground">
            {tx("暂无已保存场景", "No saved scenarios")}
          </div>
        ) : (
          <div className="flex flex-wrap gap-2">
            {scenarios.map((sc) => {
              const checked = selectedIds.includes(sc.id);
              return (
                <label
                  key={sc.id}
                  className={`inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs cursor-pointer ${
                    checked
                      ? "border-xblue bg-xblue-soft/60 text-xblue font-medium"
                      : "border-border bg-background hover:bg-muted/50"
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() => toggleScenario(sc.id)}
                    data-testid={`compare-select-${sc.name}`}
                  />
                  <span>{sc.name}</span>
                  {sc.is_stale && (
                    <Badge variant="warning" className="text-[10px]">
                      {tx("已过期", "Stale")}
                    </Badge>
                  )}
                </label>
              );
            })}
          </div>
        )}
      </div>

      {/* Filter & Actions */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Input
            value={typeFilter}
            onChange={(e) => setTypeFilter(e.target.value)}
            placeholder="Order / Warehouse"
            className="h-8 w-44 text-xs font-mono-tech"
          />
          <Input
            value={metricsFilter}
            onChange={(e) => setMetricsFilter(e.target.value)}
            placeholder="fulfillment,inventory"
            className="h-8 w-48 text-xs font-mono-tech"
          />
        </div>

        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => void runCompare(false)}
            disabled={selectedIds.length < 2 || loading || briefingLoading}
            data-testid="compare-run-btn"
          >
            <GitCompare className="h-3.5 w-3.5" />
            {loading
              ? tx("对比中…", "Comparing…")
              : tx("对比方案", "Compare")}
          </Button>
          <Button
            type="button"
            onClick={() => void runCompare(true)}
            disabled={selectedIds.length < 2 || loading || briefingLoading}
            data-testid="compare-briefing-btn"
          >
            <Sparkles className="h-3.5 w-3.5" />
            {briefingLoading
              ? tx("生成中…", "Generating…")
              : tx("生成简报", "AI Briefing")}
          </Button>
        </div>
      </div>

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      {/* AI Briefing Card */}
      {compareData?.briefing && (
        <div
          data-testid="compare-briefing-card"
          className="rounded-md border border-xblue/40 bg-xblue-soft/30 p-3.5 space-y-2.5 text-xs"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2 font-semibold text-xblue">
              <Sparkles className="h-4 w-4" />
              <span>{tx("AI 决策简报", "AI Decision Briefing")}</span>
            </div>
            {compareData.briefing.recommended_scenario_id && (
              <Badge variant="xblue">
                {tx("推荐方案：", "Recommended: ")}
                {scenarioNameMap.get(
                  compareData.briefing.recommended_scenario_id
                ) ?? compareData.briefing.recommended_scenario_id}
              </Badge>
            )}
          </div>

          {compareData.briefing.error && (
            <div className="rounded border border-amber-500/40 bg-amber-500/10 px-2.5 py-1.5 text-amber-700 dark:text-amber-300 flex items-center gap-1.5">
              <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
              <span>{compareData.briefing.error}</span>
            </div>
          )}

          {compareData.briefing.summary && (
            <p className="leading-relaxed text-foreground">
              {compareData.briefing.summary}
            </p>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            {Array.isArray(compareData.briefing.reasons) &&
              compareData.briefing.reasons.length > 0 && (
                <div className="space-y-1">
                  <div className="font-medium text-emerald-700 dark:text-emerald-400 flex items-center gap-1">
                    <CheckCircle2 className="h-3.5 w-3.5" />
                    {tx("推荐依据", "Rationale")}
                  </div>
                  <ul className="list-disc pl-4 space-y-1 text-muted-foreground">
                    {compareData.briefing.reasons.map((r, i) => (
                      <li key={i}>{r}</li>
                    ))}
                  </ul>
                </div>
              )}

            {Array.isArray(compareData.briefing.risks) &&
              compareData.briefing.risks.length > 0 && (
                <div className="space-y-1">
                  <div className="font-medium text-amber-700 dark:text-amber-400 flex items-center gap-1">
                    <AlertTriangle className="h-3.5 w-3.5" />
                    {tx("潜在风险", "Risks")}
                  </div>
                  <ul className="list-disc pl-4 space-y-1 text-muted-foreground">
                    {compareData.briefing.risks.map((w, i) => (
                      <li key={i}>{w}</li>
                    ))}
                  </ul>
                </div>
              )}
          </div>
        </div>
      )}

      {/* KPI Matrix */}
      {compareData && (
        <div className="space-y-4">
          <div className="space-y-2">
            <div className="text-xs font-semibold">
              {tx("汇总指标对比矩阵", "Aggregate KPI Matrix")}
            </div>
            <div className="overflow-x-auto">
              <table className="w-full text-xs" data-testid="compare-kpi-table">
                <thead>
                  <tr className="border-b border-border text-left text-muted-foreground">
                    <th className="py-2 pr-3">{tx("对象类型", "Type")}</th>
                    <th className="py-2 pr-3">{tx("指标", "Metric")}</th>
                    <th className="py-2 pr-3 text-right">
                      {tx("基线总和", "Baseline Sum")}
                    </th>
                    {compareData.scenarios.map((col) => (
                      <th key={col.id} className="py-2 px-2 text-right">
                        {col.name}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {(compareData.kpis ?? []).map((row) => (
                    <tr
                      key={`${row.type_name}-${row.metric}`}
                      className="border-b border-border/60"
                    >
                      <td className="py-2 pr-3 font-medium">{row.type_name}</td>
                      <td className="py-2 pr-3 font-mono-tech">{row.metric}</td>
                      <td className="py-2 pr-3 text-right font-mono-tech text-muted-foreground">
                        {fmtNum(row.before_sum, 4)}
                      </td>
                      {compareData.scenarios.map((col) => {
                        const cell = row.values?.[col.id];
                        const delta = cell?.delta ?? 0;
                        const tone =
                          delta > 0
                            ? "text-emerald-600 dark:text-emerald-400"
                            : delta < 0
                              ? "text-red-600 dark:text-red-400"
                              : "text-muted-foreground";
                        return (
                          <td
                            key={col.id}
                            className="py-2 px-2 text-right font-mono-tech"
                          >
                            {cell ? (
                              <div>
                                <div className="font-medium">
                                  {fmtNum(cell.after_sum, 4)}
                                </div>
                                <div className={`text-[11px] ${tone}`}>
                                  {formatRatio(cell.delta_ratio)} (
                                  {tx(
                                    `${cell.affected_nodes} 节点`,
                                    `${cell.affected_nodes} nodes`
                                  )}
                                  )
                                </div>
                              </div>
                            ) : (
                              "-"
                            )}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                  {(compareData.kpis ?? []).length === 0 && (
                    <tr>
                      <td
                        colSpan={3 + compareData.scenarios.length}
                        className="py-4 text-center text-muted-foreground"
                      >
                        {tx("暂无汇总对比行", "No aggregate KPI rows")}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          </div>

          {/* Key Nodes Matrix */}
          <div className="space-y-2">
            <div className="text-xs font-semibold">
              {tx("关键节点差异矩阵", "Key Node Spread Matrix")}
            </div>
            <div className="overflow-x-auto">
              <table
                className="w-full text-xs"
                data-testid="compare-nodes-table"
              >
                <thead>
                  <tr className="border-b border-border text-left text-muted-foreground">
                    <th className="py-2 pr-3">{tx("节点名称", "Node")}</th>
                    <th className="py-2 pr-3">
                      {tx("类型 · 指标", "Type · Metric")}
                    </th>
                    <th className="py-2 pr-3 text-right">
                      {tx("基线值", "Baseline")}
                    </th>
                    <th className="py-2 pr-3 text-right">
                      {tx("差异跨度", "Max Spread")}
                    </th>
                    {compareData.scenarios.map((col) => (
                      <th key={col.id} className="py-2 px-2 text-right">
                        {col.name}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {(compareData.nodes ?? []).map((row) => (
                    <tr
                      key={`${row.node_id}-${row.metric}`}
                      className="border-b border-border/60"
                    >
                      <td className="py-2 pr-3 font-medium">{row.title}</td>
                      <td className="py-2 pr-3 font-mono-tech text-muted-foreground">
                        {row.type_name}.{row.metric}
                      </td>
                      <td className="py-2 pr-3 text-right font-mono-tech text-muted-foreground">
                        {fmtNum(row.before_val, 4)}
                      </td>
                      <td className="py-2 pr-3 text-right font-mono-tech">
                        {fmtNum(row.max_spread, 4)}
                      </td>
                      {compareData.scenarios.map((col) => {
                        const cell = row.values?.[col.id];
                        const delta = cell?.delta ?? 0;
                        const tone =
                          delta > 0
                            ? "text-emerald-600 dark:text-emerald-400"
                            : delta < 0
                              ? "text-red-600 dark:text-red-400"
                              : "text-muted-foreground";
                        return (
                          <td
                            key={col.id}
                            className="py-2 px-2 text-right font-mono-tech"
                          >
                            {cell ? (
                              <div>
                                <span className="font-medium">
                                  {fmtNum(cell.after_val, 4)}
                                </span>
                                <span className={`ml-1 text-[11px] ${tone}`}>
                                  ({formatRatio(cell.delta_ratio)})
                                </span>
                              </div>
                            ) : (
                              "-"
                            )}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                  {(compareData.nodes ?? []).length === 0 && (
                    <tr>
                      <td
                        colSpan={4 + compareData.scenarios.length}
                        className="py-4 text-center text-muted-foreground"
                      >
                        {tx("暂无节点差异行", "No node-level rows")}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          </div>
        </div>
      )}
    </Card>
  );
}
