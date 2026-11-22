import { useEffect, useMemo, useState } from "react";
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  FlaskConical,
  Play,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { AggFn } from "@/lib/api/enums";
import {
  xblueApi,
  type AggRuleSpec,
  type BlueNode,
  type DerivedRuleSpec,
  type EdgeTypeSpec,
  type EvaluateExprResponse,
  type ExprIssue,
  type ExprType,
  type LinkDirection,
  type NodeTypeSpec,
  type ValidationIssue,
} from "@/lib/api/xblue";
import { Badge, Button, Input, Select, Switch } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface RuleEditorProps {
  nodeIndex: number;
  nodeType: NodeTypeSpec;
  allNodeTypes: NodeTypeSpec[];
  edgeTypes: EdgeTypeSpec[];
  issues: ValidationIssue[];
  subgraph?: string;
  onChangeAggRules: (next: AggRuleSpec[]) => void;
  onChangeDerivedRules: (next: DerivedRuleSpec[]) => void;
}

interface LiveExprDiagnostic {
  valid: boolean;
  inferred_type?: ExprType;
  issues: ExprIssue[];
}

const AGG_OPS: readonly AggFn[] = [
  "SUM",
  "AVG",
  "COUNT",
  "COUNT_DISTINCT",
  "MIN",
  "MAX",
  "MEDIAN",
  "LAST",
];

function checkLocalExprSyntax(
  expr: string,
  tx: (zh: string, en: string) => string
): string | null {
  const trimmed = expr.trim();
  if (!trimmed) return tx("表达式为空", "Expression required");
  if (trimmed.length > 2000) return tx("超出 2000 字符上限", "Exceeds 2000 chars");
  let depth = 0;
  let inQuotes = false;
  for (let i = 0; i < trimmed.length; i += 1) {
    const ch = trimmed[i];
    if (ch === "'") {
      inQuotes = !inQuotes;
    } else if (!inQuotes) {
      if (ch === "(") depth += 1;
      if (ch === ")") depth -= 1;
      if (depth < 0) return tx("右括号多余", "Unmatched ')'");
    }
  }
  if (inQuotes) return tx("单引号未闭合", "Unclosed quote");
  if (depth !== 0) return tx("括号未闭合", "Unclosed '('");
  return null;
}

function formatEvalValue(val: unknown): string {
  if (val === null || val === undefined) return "null";
  if (typeof val === "number") {
    return Number.isInteger(val)
      ? String(val)
      : Number(val.toFixed(6)).toString();
  }
  if (typeof val === "boolean") return val ? "true" : "false";
  if (typeof val === "string") return `'${val}'`;
  return JSON.stringify(val);
}

export function RuleEditor({
  nodeIndex,
  nodeType,
  allNodeTypes,
  edgeTypes,
  issues,
  subgraph = "main",
  onChangeAggRules,
  onChangeDerivedRules,
}: RuleEditorProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const ownMetrics = useMemo(
    () =>
      nodeType.properties
        .filter((p) => p.kind === "METRIC" && p.name.trim().length > 0)
        .map((p) => p.name),
    [nodeType.properties]
  );
  const ownAttrs = useMemo(
    () =>
      nodeType.properties
        .filter((p) => p.kind === "ATTRIBUTE" && p.name.trim().length > 0)
        .map((p) => p.name),
    [nodeType.properties]
  );

  const [exprDiagnostics, setExprDiagnostics] = useState<
    Record<string, LiveExprDiagnostic>
  >({});

  const [recomputing, setRecomputing] = useState(false);
  const [recomputeStatus, setRecomputeStatus] = useState<string | null>(null);

  const [sampleNodes, setSampleNodes] = useState<BlueNode[]>([]);
  const [loadingSampleNodes, setLoadingSampleNodes] = useState(false);
  const [selectedSampleNodeId, setSelectedSampleNodeId] = useState<string>("");
  const [dryRunExpr, setDryRunExpr] = useState<string>("");
  const [evaluating, setEvaluating] = useState(false);
  const [evalResult, setEvalResult] = useState<EvaluateExprResponse | null>(
    null
  );
  const [evalError, setEvalError] = useState<string | null>(null);

  useEffect(() => {
    const targets: { key: string; typeName: string; expression: string }[] = [];

    for (const dRule of nodeType.derived_rules) {
      const trimmed = dRule.expression.trim();
      if (trimmed.length > 0 && nodeType.name.trim().length > 0) {
        const key = `${subgraph}::${nodeType.name}::${trimmed}`;
        targets.push({
          key,
          typeName: nodeType.name,
          expression: dRule.expression,
        });
      }
    }

    for (const aRule of nodeType.agg_rules) {
      const filterTrimmed = (aRule.filter ?? "").trim();
      const filterScope = (aRule.source_type_name || nodeType.name).trim();
      if (filterTrimmed.length > 0 && filterScope.length > 0) {
        const key = `${subgraph}::${filterScope}::${filterTrimmed}`;
        targets.push({
          key,
          typeName: filterScope,
          expression: filterTrimmed,
        });
      }
    }

    if (targets.length === 0) return;

    let cancelled = false;
    const timer = window.setTimeout(() => {
      void Promise.all(
        targets.map(async (item) => {
          try {
            const res = await xblueApi.validateExpr({
              type_name: item.typeName,
              expression: item.expression,
              subgraph,
            });
            return { key: item.key, res };
          } catch {
            return null;
          }
        })
      ).then((results) => {
        if (cancelled) return;
        setExprDiagnostics((prev) => {
          const next = { ...prev };
          for (const entry of results) {
            if (entry) {
              next[entry.key] = {
                valid: entry.res.valid,
                inferred_type: entry.res.inferred_type,
                issues: entry.res.issues,
              };
            }
          }
          return next;
        });
      });
    }, 300);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [nodeType.name, nodeType.derived_rules, nodeType.agg_rules, subgraph]);

  useEffect(() => {
    setRecomputeStatus(null);
    setEvalResult(null);
    setEvalError(null);

    const defaultFormula =
      nodeType.derived_rules[0]?.expression ||
      (ownMetrics.length >= 2
        ? `if(${ownMetrics[1]} > 0, ${ownMetrics[0]} / ${ownMetrics[1]}, null)`
        : (ownMetrics[0] ?? "0"));
    setDryRunExpr(defaultFormula);

    const typeName = nodeType.name.trim();
    if (!typeName) {
      setSampleNodes([]);
      setSelectedSampleNodeId("");
      return;
    }

    let cancelled = false;
    setLoadingSampleNodes(true);
    void xblueApi
      .listNodes({ type_name: typeName, limit: 20 })
      .then((res) => {
        if (cancelled) return;
        const rawList =
          Array.isArray(res?.items) && res.items.length > 0
            ? res.items
            : Array.isArray(res?.nodes)
              ? res.nodes
              : [];
        const filtered = rawList.filter(
          (n) => !n.type_name || n.type_name === typeName
        );
        const list = filtered.length > 0 ? filtered : rawList;
        setSampleNodes(list);
        setSelectedSampleNodeId((prev) =>
          prev && list.some((n) => n.id === prev) ? prev : (list[0]?.id ?? "")
        );
      })
      .catch(() => {
        if (!cancelled) {
          setSampleNodes([]);
          setSelectedSampleNodeId("");
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoadingSampleNodes(false);
        }
      });

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodeType.name]);

  const handleRecomputeAggregate = async () => {
    if (!canEdit) return;
    setRecomputing(true);
    setRecomputeStatus(null);
    try {
      const res = await xblueApi.recomputeAggregate({
        type_name: nodeType.name,
      });
      const shortJob = res.job_id ? res.job_id.slice(0, 8) : "queued";
      setRecomputeStatus(
        tx(
          `已提交重算任务 (${res.type_name ?? nodeType.name} · job=${shortJob})`,
          `Recompute queued (${res.type_name ?? nodeType.name} · job=${shortJob})`
        )
      );
    } catch (err) {
      setRecomputeStatus(
        err instanceof Error
          ? err.message
          : tx("重算任务提交失败", "Failed to queue recompute")
      );
    } finally {
      setRecomputing(false);
    }
  };

  const executeDryRun = async (exprOverride?: string) => {
    const targetExpr = (exprOverride ?? dryRunExpr).trim();
    if (!selectedSampleNodeId) {
      setEvalError(tx("需指定样本节点", "Sample node required"));
      return;
    }
    if (!targetExpr) {
      setEvalError(tx("试算表达式为空", "Expression required"));
      return;
    }
    setEvaluating(true);
    setEvalError(null);
    try {
      const res = await xblueApi.evaluateExpr({
        node_id: selectedSampleNodeId,
        expression: targetExpr,
      });
      setEvalResult(res);
    } catch (err) {
      setEvalResult(null);
      setEvalError(
        err instanceof Error
          ? err.message
          : tx("样本试算失败", "Dry-run failed")
      );
    } finally {
      setEvaluating(false);
    }
  };

  const addAggRule = () => {
    if (!canEdit) return;
    const firstEdge = edgeTypes.find(
      (et) =>
        et.src_type_name === nodeType.name || et.dst_type_name === nodeType.name
    );
    const defaultDir: LinkDirection =
      firstEdge && firstEdge.dst_type_name === nodeType.name ? "IN" : "OUT";
    const defaultNeighbor = firstEdge
      ? defaultDir === "OUT"
        ? firstEdge.dst_type_name
        : firstEdge.src_type_name
      : (allNodeTypes[0]?.name ?? "");
    const neighborMetrics =
      allNodeTypes
        .find((nt) => nt.name === defaultNeighbor)
        ?.properties.filter((p) => p.kind === "METRIC")
        .map((p) => p.name) ?? [];

    onChangeAggRules([
      ...nodeType.agg_rules,
      {
        target_metric: ownMetrics[0] ?? "",
        op: "SUM",
        via_link_type: firstEdge?.name ?? "",
        direction: defaultDir,
        source_type_name: defaultNeighbor,
        source_metric: neighborMetrics[0] ?? "",
        filter: "",
        weight_by_link: false,
      },
    ]);
  };

  const updateAggRule = (idx: number, patch: Partial<AggRuleSpec>) => {
    if (!canEdit) return;
    onChangeAggRules(
      nodeType.agg_rules.map((r, i) => (i === idx ? { ...r, ...patch } : r))
    );
  };

  const removeAggRule = (idx: number) => {
    if (!canEdit) return;
    onChangeAggRules(nodeType.agg_rules.filter((_, i) => i !== idx));
  };

  const addDerivedRule = () => {
    if (!canEdit) return;
    const nextFormula =
      ownMetrics.length >= 2
        ? `if(${ownMetrics[1]} > 0, ${ownMetrics[0]} / ${ownMetrics[1]}, null)`
        : "0";
    onChangeDerivedRules([
      ...nodeType.derived_rules,
      {
        target_metric: ownMetrics[0] ?? "",
        expression: nextFormula,
        enabled: true,
      },
    ]);
    if (!dryRunExpr || dryRunExpr === "0") {
      setDryRunExpr(nextFormula);
    }
  };

  const updateDerivedRule = (idx: number, patch: Partial<DerivedRuleSpec>) => {
    if (!canEdit) return;
    onChangeDerivedRules(
      nodeType.derived_rules.map((r, i) =>
        i === idx ? { ...r, ...patch } : r
      )
    );
  };

  const removeDerivedRule = (idx: number) => {
    if (!canEdit) return;
    onChangeDerivedRules(nodeType.derived_rules.filter((_, i) => i !== idx));
  };

  const insertTokenIntoDerived = (idx: number, token: string) => {
    if (!canEdit) return;
    const cur = nodeType.derived_rules[idx];
    if (!cur) return;
    const sep =
      cur.expression.length > 0 &&
      !cur.expression.endsWith(" ") &&
      !cur.expression.endsWith("(")
        ? " "
        : "";
    updateDerivedRule(idx, {
      expression: `${cur.expression}${sep}${token}`,
    });
  };

  const selectedSampleNode = sampleNodes.find(
    (n) => n.id === selectedSampleNodeId
  );

  return (
    <div className="space-y-4 text-xs" data-testid="rule-editor">
      {/* Top Action Bar: Full Aggregate Recompute Trigger */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded border border-border bg-muted/20 px-2.5 py-2">
        <div className="flex items-center gap-1.5">
          <span className="font-semibold">
            {tx("传播重算", "Aggregate Recompute")} ({nodeType.name})
          </span>
          <Badge variant="outline">{tx("拓扑序", "DAG Order")}</Badge>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="recompute-agg-btn"
          disabled={!canEdit || recomputing || !nodeType.name.trim()}
          onClick={() => void handleRecomputeAggregate()}
        >
          <RefreshCw
            className={cn(
              "h-3 w-3 text-xblue",
              recomputing && "animate-spin"
            )}
          />
          {recomputing
            ? tx("重算中…", "Queueing…")
            : tx("全量重算", "Recompute All")}
        </Button>
      </div>

      {recomputeStatus && (
        <div
          className="rounded border border-border bg-muted/40 px-2.5 py-1.5 font-mono-tech text-[11px]"
          data-testid="recompute-agg-status"
        >
          {recomputeStatus}
        </div>
      )}

      {/* Section 1: Dynamic Aggregation Rules */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <span className="font-semibold">
            {tx("动态聚合规则", "Aggregation Rules")}
          </span>
          {canEdit && (
            <Button
              variant="outline"
              size="sm"
              data-testid="add-agg-rule-btn"
              onClick={addAggRule}
            >
              <Plus className="h-3 w-3" />
              {tx("新增聚合", "Add Rollup")}
            </Button>
          )}
        </div>

        {nodeType.agg_rules.length === 0 ? (
          <div className="rounded border border-dashed border-border p-3 text-center text-muted-foreground">
            {tx("暂无动态聚合规则", "No aggregation rules")}
          </div>
        ) : (
          <div className="space-y-2">
            {nodeType.agg_rules.map((rule, idx) => {
              const sourceNode = allNodeTypes.find(
                (nt) => nt.name === rule.source_type_name
              );
              const sourceMetricCandidates =
                sourceNode?.properties
                  .filter((p) => p.kind === "METRIC")
                  .map((p) => p.name) ?? [];
              const rulePathPrefix = `node_types[${nodeIndex}].agg_rules[${idx}]`;
              const ruleIssues = issues.filter((iss) =>
                iss.path.startsWith(rulePathPrefix)
              );
              const filterTrimmed = (rule.filter ?? "").trim();
              const filterScope = (
                rule.source_type_name || nodeType.name
              ).trim();
              const filterKey =
                filterTrimmed.length > 0 && filterScope.length > 0
                  ? `${subgraph}::${filterScope}::${filterTrimmed}`
                  : "";
              const filterDiag = filterKey
                ? exprDiagnostics[filterKey]
                : undefined;
              const localFilterErr =
                filterTrimmed.length > 0 && !filterDiag
                  ? checkLocalExprSyntax(filterTrimmed, tx)
                  : null;

              return (
                <div
                  key={idx}
                  className="rounded border border-border bg-muted/20 p-2.5 space-y-2"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-mono-tech text-[11px] font-semibold text-xblue truncate">
                      {rule.target_metric || "?"} = {rule.op}(
                      {rule.source_type_name || "?"}.{rule.source_metric || "?"}{" "}
                      via {rule.via_link_type || "?"} {rule.direction})
                    </span>
                    {canEdit && (
                      <button
                        type="button"
                        onClick={() => removeAggRule(idx)}
                        className="rounded p-1 text-muted-foreground hover:text-red-600 cursor-pointer shrink-0"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    )}
                  </div>

                  <div className="grid grid-cols-2 gap-2">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("目标指标", "Target Metric")}
                      </label>
                      {ownMetrics.length > 0 ? (
                        <Select
                          value={rule.target_metric}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateAggRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          className="h-7 w-full px-1.5 font-mono-tech"
                        >
                          <option value="">{tx("未指定", "None")}</option>
                          {ownMetrics.map((m) => (
                            <option key={m} value={m}>
                              {m}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          value={rule.target_metric}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateAggRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          placeholder="total_demand"
                          className="h-7 px-2 text-xs font-mono-tech"
                        />
                      )}
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("聚合算子", "Operator")}
                      </label>
                      <Select
                        value={rule.op}
                        disabled={!canEdit}
                        onChange={(e) =>
                          updateAggRule(idx, { op: e.target.value as AggFn })
                        }
                        className="h-7 w-full px-1.5 font-mono-tech"
                      >
                        {AGG_OPS.map((op) => (
                          <option key={op} value={op}>
                            {op}
                          </option>
                        ))}
                      </Select>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("关联关系", "Via Relation")}
                      </label>
                      <Select
                        value={rule.via_link_type}
                        disabled={!canEdit}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            via_link_type: e.target.value,
                          })
                        }
                        className="h-7 w-full px-1.5 font-mono-tech"
                      >
                        <option value="">{tx("未指定", "None")}</option>
                        {edgeTypes.map((et, eIdx) => (
                          <option key={`${et.name}-${eIdx}`} value={et.name}>
                            {et.name} ({et.src_type_name}→{et.dst_type_name})
                          </option>
                        ))}
                      </Select>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("遍历方向", "Direction")}
                      </label>
                      <Select
                        value={rule.direction}
                        disabled={!canEdit}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            direction: e.target.value as LinkDirection,
                          })
                        }
                        className="h-7 w-full px-1.5 font-mono-tech"
                      >
                        <option value="OUT">
                          {tx("出边下游 (OUT)", "Outbound (OUT)")}
                        </option>
                        <option value="IN">
                          {tx("入边上游 (IN)", "Inbound (IN)")}
                        </option>
                      </Select>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 gap-2 items-end">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("邻居类型", "Neighbor Type")}
                      </label>
                      <Select
                        value={rule.source_type_name}
                        disabled={!canEdit}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            source_type_name: e.target.value,
                          })
                        }
                        className="h-7 w-full px-1.5 font-mono-tech"
                      >
                        <option value="">{tx("未指定", "None")}</option>
                        {allNodeTypes.map((nt) => (
                          <option key={nt.name} value={nt.name}>
                            {nt.name} ({nt.display_name})
                          </option>
                        ))}
                      </Select>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        {tx("邻居指标", "Neighbor Metric")}
                      </label>
                      {sourceMetricCandidates.length > 0 ? (
                        <Select
                          value={rule.source_metric}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateAggRule(idx, {
                              source_metric: e.target.value,
                            })
                          }
                          className="h-7 w-full px-1.5 font-mono-tech"
                        >
                          <option value="">{tx("未指定", "None")}</option>
                          {sourceMetricCandidates.map((sm) => (
                            <option key={sm} value={sm}>
                              {sm}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          value={rule.source_metric}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateAggRule(idx, {
                              source_metric: e.target.value,
                            })
                          }
                          placeholder="demand"
                          className="h-7 px-2 text-xs font-mono-tech"
                        />
                      )}
                    </div>
                  </div>

                  <div className="flex items-center justify-between rounded bg-background/60 px-2 py-1.5">
                    <span className="text-[11px] text-muted-foreground">
                      {tx("按关系权重加权", "Weight by link")}
                    </span>
                    <Switch
                      checked={rule.weight_by_link}
                      disabled={!canEdit}
                      onCheckedChange={(checked) =>
                        updateAggRule(idx, {
                          weight_by_link: checked,
                        })
                      }
                      aria-label={tx("按关系权重加权", "Weight by link")}
                    />
                  </div>

                  <div className="space-y-1.5 pt-0.5">
                    <div className="flex items-center justify-between gap-2 leading-normal">
                      <label className="text-[11px] font-medium text-muted-foreground">
                        {tx("过滤条件", "Filter Expression")}
                      </label>
                      {filterDiag?.valid && filterDiag.inferred_type && (
                        <Badge
                          variant="xblue"
                          data-testid={`agg-filter-inferred-type-${idx}`}
                        >
                          {filterDiag.inferred_type}
                        </Badge>
                      )}
                    </div>
                    <Input
                      data-testid={`agg-filter-input-${idx}`}
                      value={rule.filter ?? ""}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateAggRule(idx, { filter: e.target.value })
                      }
                      placeholder="prop.status != 'CANCELED'"
                      className="h-8 px-2.5 text-xs font-mono-tech"
                    />
                    {localFilterErr && (
                      <div className="text-[11px] text-red-600 dark:text-red-400">
                        {localFilterErr}
                      </div>
                    )}
                    {filterDiag?.issues.map((fIss, fIdx) => (
                      <div
                        key={fIdx}
                        data-testid={`agg-filter-issue-${idx}-${fIdx}`}
                        className="text-[11px] text-red-600 dark:text-red-400 flex items-center gap-1 font-mono-tech"
                      >
                        <AlertCircle className="h-3 w-3 shrink-0" />
                        <span>
                          {fIss.column !== undefined
                            ? tx(
                                `列 ${fIss.column}: ${fIss.message}`,
                                `Col ${fIss.column}: ${fIss.message}`
                              )
                            : fIss.message}
                        </span>
                      </div>
                    ))}
                  </div>

                  {ruleIssues.map((iss, iIdx) => (
                    <div
                      key={iIdx}
                      className="text-[11px] text-red-600 dark:text-red-400 flex items-center gap-1"
                    >
                      <AlertCircle className="h-3 w-3 shrink-0" />
                      [{iss.code}] {iss.message}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Section 2: Derived Formula Rules */}
      <div className="space-y-2 border-t border-border pt-3">
        <div className="flex items-center justify-between">
          <span className="font-semibold">
            {tx("派生指标公式", "Derived Formulas")}
          </span>
          {canEdit && (
            <Button
              variant="outline"
              size="sm"
              data-testid="add-derived-rule-btn"
              onClick={addDerivedRule}
            >
              <Plus className="h-3 w-3 text-xblue" />
              {tx("新增公式", "Add Formula")}
            </Button>
          )}
        </div>

        {nodeType.derived_rules.length === 0 ? (
          <div className="rounded border border-dashed border-border p-3 text-center text-muted-foreground">
            {tx("暂无派生公式规则", "No derived formulas")}
          </div>
        ) : (
          <div className="space-y-2.5">
            {nodeType.derived_rules.map((dRule, idx) => {
              const exprTrimmed = dRule.expression.trim();
              const diagKey =
                exprTrimmed.length > 0 && nodeType.name.trim().length > 0
                  ? `${subgraph}::${nodeType.name}::${exprTrimmed}`
                  : "";
              const backendDiag = diagKey
                ? exprDiagnostics[diagKey]
                : undefined;
              const localErr = !backendDiag
                ? checkLocalExprSyntax(dRule.expression, tx)
                : null;
              const rulePathPrefix = `node_types[${nodeIndex}].derived_rules[${idx}]`;
              const serverIssues = issues.filter((iss) =>
                iss.path.startsWith(rulePathPrefix)
              );
              const backendIssues = backendDiag?.issues ?? [];
              const hasIssue =
                Boolean(localErr) ||
                backendIssues.length > 0 ||
                backendDiag?.valid === false ||
                serverIssues.length > 0;

              return (
                <div
                  key={idx}
                  className="rounded-md border border-border bg-muted/20 p-2.5 space-y-2"
                >
                  {/* Row 1: Target Metric + Active Switch + Delete (Zero horizontal crowding) */}
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2 flex-1 min-w-0">
                      <span className="text-[11px] text-muted-foreground shrink-0">
                        {tx("目标指标", "Metric")}
                      </span>
                      {ownMetrics.length > 0 ? (
                        <Select
                          value={dRule.target_metric}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateDerivedRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          className="h-8 flex-1 min-w-0 font-mono-tech"
                        >
                          <option value="">{tx("未指定", "None")}</option>
                          {ownMetrics.map((m) => (
                            <option key={m} value={m}>
                              {m}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          value={dRule.target_metric}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateDerivedRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          placeholder="fill_rate"
                          className="h-8 flex-1 min-w-0 px-2 text-xs font-mono-tech"
                        />
                      )}
                    </div>

                    <div className="flex items-center gap-2 shrink-0">
                      <span className="text-[11px] text-muted-foreground">
                        {tx("启用", "Active")}
                      </span>
                      <Switch
                        checked={dRule.enabled}
                        disabled={!canEdit}
                        onCheckedChange={(checked) =>
                          updateDerivedRule(idx, {
                            enabled: checked,
                          })
                        }
                        aria-label={tx("启用", "Active")}
                      />
                      {canEdit && (
                        <button
                          type="button"
                          onClick={() => removeDerivedRule(idx)}
                          className="rounded p-1 text-muted-foreground hover:text-red-600 cursor-pointer"
                          title={tx("删除公式", "Delete formula")}
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  </div>

                  {/* Monospace Expression Input */}
                  <textarea
                    rows={2}
                    data-testid={`derived-expr-input-${idx}`}
                    value={dRule.expression}
                    disabled={!canEdit}
                    onChange={(e) =>
                      updateDerivedRule(idx, { expression: e.target.value })
                    }
                    placeholder="if(total_demand > 0, inventory / total_demand, null)"
                    className={cn(
                      "w-full rounded-md border bg-background px-2.5 py-1.5 font-mono-tech text-xs leading-relaxed focus:outline-none focus:ring-1 focus:ring-xblue disabled:opacity-50",
                      hasIssue ? "border-red-500/60" : "border-input"
                    )}
                  />

                  {/* Single-line Validation & Dry-Run Transfer Bar (Replaces redundant AST box) */}
                  <div className="flex items-center justify-between gap-2 text-[11px]">
                    <div className="flex items-center gap-1.5">
                      {backendDiag?.valid && backendDiag.inferred_type && (
                        <Badge
                          variant="xblue"
                          data-testid={`derived-inferred-type-${idx}`}
                        >
                          {backendDiag.inferred_type}
                        </Badge>
                      )}
                      {hasIssue ? (
                        <Badge variant="danger">
                          <AlertCircle className="h-3 w-3 mr-1" />
                          {tx("异常", "Error")}
                        </Badge>
                      ) : (
                        <Badge variant="success">
                          <CheckCircle2 className="h-3 w-3 mr-1" />
                          {tx("就绪", "Valid")}
                        </Badge>
                      )}
                    </div>
                    <button
                      type="button"
                      onClick={() => {
                        setDryRunExpr(dRule.expression);
                        if (selectedSampleNodeId) {
                          void executeDryRun(dRule.expression);
                        }
                      }}
                      className="text-[11px] font-medium text-xblue hover:underline cursor-pointer"
                    >
                      {tx("带入样本试算", "Load in Dry-Run")}
                    </button>
                  </div>

                  {/* Categorized Quick Insert Chips */}
                  {canEdit && (
                    <div className="space-y-1 pt-0.5">
                      {ownMetrics.length > 0 && (
                        <div className="flex flex-wrap items-center gap-1">
                          <span className="text-[10px] text-muted-foreground w-9 shrink-0">
                            {tx("指标", "Metrics")}
                          </span>
                          {ownMetrics.map((m) => (
                            <button
                              key={`m-${m}`}
                              type="button"
                              onClick={() => insertTokenIntoDerived(idx, m)}
                              className="rounded border border-xblue/30 bg-xblue-soft px-1.5 py-0.5 font-mono-tech text-[10px] text-xblue cursor-pointer"
                            >
                              {m}
                            </button>
                          ))}
                        </div>
                      )}
                      {ownAttrs.length > 0 && (
                        <div className="flex flex-wrap items-center gap-1">
                          <span className="text-[10px] text-muted-foreground w-9 shrink-0">
                            {tx("属性", "Attrs")}
                          </span>
                          {ownAttrs.map((a) => (
                            <button
                              key={`a-${a}`}
                              type="button"
                              onClick={() =>
                                insertTokenIntoDerived(idx, `prop.${a}`)
                              }
                              className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono-tech text-[10px] cursor-pointer"
                            >
                              prop.{a}
                            </button>
                          ))}
                        </div>
                      )}
                      <div className="flex flex-wrap items-center gap-1">
                        <span className="text-[10px] text-muted-foreground w-9 shrink-0">
                          {tx("函数", "Funcs")}
                        </span>
                        {["if(", "safe_div(", "coalesce(", "round("].map(
                          (fn) => (
                            <button
                              key={fn}
                              type="button"
                              onClick={() => insertTokenIntoDerived(idx, fn)}
                              className="rounded border border-border bg-background px-1.5 py-0.5 font-mono-tech text-[10px] text-muted-foreground hover:text-foreground cursor-pointer"
                            >
                              {fn})
                            </button>
                          )
                        )}
                      </div>
                    </div>
                  )}

                  {localErr && (
                    <div className="text-[11px] text-red-600 dark:text-red-400">
                      {localErr}
                    </div>
                  )}
                  {backendIssues.map((bIss, bIdx) => (
                    <div
                      key={bIdx}
                      data-testid={`derived-expr-issue-${idx}-${bIdx}`}
                      className="text-[11px] text-red-600 dark:text-red-400 flex items-center gap-1 font-mono-tech"
                    >
                      <AlertCircle className="h-3 w-3 shrink-0" />
                      <span>
                        {bIss.column !== undefined
                          ? tx(
                              `列 ${bIss.column}: ${bIss.message}`,
                              `Col ${bIss.column}: ${bIss.message}`
                            )
                          : bIss.message}
                      </span>
                    </div>
                  ))}
                  {serverIssues.map((iss, sIdx) => (
                    <div
                      key={sIdx}
                      className="text-[11px] text-red-600 dark:text-red-400"
                    >
                      [{iss.code}] {iss.message}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Section 3: Sample Runtime Node Dry-Run Evaluator */}
      <div
        className="space-y-2.5 border-t border-border pt-3"
        data-testid="expr-dryrun-panel"
      >
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5">
            <FlaskConical className="h-3.5 w-3.5 text-xblue" />
            <span className="font-semibold">
              {tx("样本节点试算", "Sample Node Dry-Run")}
            </span>
          </div>
          <Badge variant="outline">
            {loadingSampleNodes
              ? tx("加载中…", "Loading…")
              : tx(`样本 ${sampleNodes.length}`, `${sampleNodes.length} Samples`)}
          </Badge>
        </div>

        <div className="rounded border border-border bg-muted/20 p-2.5 space-y-2">
          <div className="space-y-2">
            <div>
              <label className="block text-[11px] text-muted-foreground mb-0.5">
                {tx("样本节点", "Sample Node")} ({nodeType.name})
              </label>
              <Select
                data-testid="expr-dryrun-node-select"
                value={selectedSampleNodeId}
                onChange={(e) => setSelectedSampleNodeId(e.target.value)}
                disabled={sampleNodes.length === 0}
                className="h-8 w-full font-mono-tech"
              >
                {sampleNodes.length === 0 ? (
                  <option value="">
                    {tx("暂无运行时实例", "No runtime instances")}
                  </option>
                ) : (
                  sampleNodes.map((n) => (
                    <option key={n.id} value={n.id}>
                      {n.title} ({n.node_key})
                    </option>
                  ))
                )}
              </Select>
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center justify-between gap-2">
                <label className="text-[11px] text-muted-foreground">
                  {tx("试算表达式", "Expression")}
                </label>
                <Button
                  type="button"
                  size="sm"
                  data-testid="expr-dryrun-submit-btn"
                  disabled={
                    evaluating ||
                    !selectedSampleNodeId ||
                    dryRunExpr.trim().length === 0
                  }
                  onClick={() => void executeDryRun()}
                >
                  <Play className="h-3 w-3" />
                  {evaluating
                    ? tx("计算中…", "Running…")
                    : tx("试算", "Evaluate")}
                </Button>
              </div>
              <textarea
                rows={2}
                data-testid="expr-dryrun-input"
                value={dryRunExpr}
                onChange={(e) => setDryRunExpr(e.target.value)}
                placeholder="inventory / total_demand"
                className="w-full rounded-md border border-input bg-background px-2.5 py-1.5 text-xs font-mono-tech leading-relaxed focus:outline-none focus:ring-1 focus:ring-xblue"
              />
            </div>
          </div>

          {selectedSampleNode &&
            Object.keys(selectedSampleNode.metrics ?? {}).length > 0 && (
              <div className="flex flex-wrap items-center gap-1 pt-0.5">
                {Object.entries(selectedSampleNode.metrics ?? {}).map(
                  ([mk, mv]) => (
                    <Badge key={mk} variant="outline">
                      {mk}: {fmtNum(mv, 2)}
                    </Badge>
                  )
                )}
              </div>
            )}

          {evalError && (
            <div className="text-[11px] text-red-600 dark:text-red-400 flex items-center gap-1">
              <AlertCircle className="h-3 w-3 shrink-0" />
              <span>{evalError}</span>
            </div>
          )}

          {evalResult && (
            <div
              className="rounded border border-border bg-background px-2.5 py-2 flex flex-wrap items-center justify-between gap-2"
              data-testid="expr-dryrun-result"
            >
              <div className="flex items-center gap-2 font-mono-tech">
                <span className="text-[11px] text-muted-foreground">
                  {tx("结果:", "Value:")}
                </span>
                <span
                  className="text-sm font-semibold text-foreground"
                  data-testid="expr-dryrun-value"
                >
                  {formatEvalValue(evalResult.value)}
                </span>
                <Badge variant="xblue" data-testid="expr-dryrun-value-type">
                  {evalResult.value_type}
                </Badge>
              </div>

              {evalResult.warnings.length > 0 && (
                <div
                  className="flex flex-wrap items-center gap-1.5"
                  data-testid="expr-dryrun-warnings"
                >
                  {evalResult.warnings.map((wCode) => (
                    <Badge key={wCode} variant="warning">
                      <AlertTriangle className="h-3 w-3 mr-1" />
                      {wCode}
                    </Badge>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
