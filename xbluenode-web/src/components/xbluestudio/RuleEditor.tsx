import React, { useEffect, useMemo, useState } from "react";
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
import { Badge, Button, Input } from "@/components/ui/primitives";
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

const BUILTIN_FUNCS = new Set([
  "abs",
  "sqrt",
  "exp",
  "ln",
  "log10",
  "floor",
  "ceil",
  "round",
  "pow",
  "min",
  "max",
  "clamp",
  "if",
  "coalesce",
  "is_null",
  "contains",
  "starts_with",
  "lower",
  "upper",
  "safe_div",
]);

const KEYWORDS = new Set(["and", "or", "not", "true", "false", "null", "prop"]);

function highlightExpression(expr: string): React.ReactNode[] {
  const tokenRegex =
    /('(?:''|[^'])*'|\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b|\bprop\.[a-zA-Z_][a-zA-Z0-9_]*\b|\b[a-zA-Z_][a-zA-Z0-9_]*\b|==|!=|<=|>=|&&|\|\||[+\-*/%^(),<>!])/g;
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null = tokenRegex.exec(expr);
  let keyIdx = 0;

  while (match !== null) {
    if (match.index > lastIndex) {
      parts.push(
        <span key={`ws-${keyIdx++}`}>{expr.slice(lastIndex, match.index)}</span>
      );
    }
    const tok = match[0];
    if (tok.startsWith("'")) {
      parts.push(
        <span
          key={`str-${keyIdx++}`}
          className="text-emerald-600 dark:text-emerald-400"
        >
          {tok}
        </span>
      );
    } else if (/^\d/.test(tok)) {
      parts.push(
        <span
          key={`num-${keyIdx++}`}
          className="text-amber-600 dark:text-amber-400"
        >
          {tok}
        </span>
      );
    } else if (tok.startsWith("prop.")) {
      parts.push(
        <span key={`prop-${keyIdx++}`} className="text-xblue font-medium">
          {tok}
        </span>
      );
    } else if (BUILTIN_FUNCS.has(tok)) {
      parts.push(
        <span key={`fn-${keyIdx++}`} className="text-xblue font-semibold">
          {tok}
        </span>
      );
    } else if (KEYWORDS.has(tok)) {
      parts.push(
        <span
          key={`kw-${keyIdx++}`}
          className="text-muted-foreground font-semibold"
        >
          {tok}
        </span>
      );
    } else if (/^[a-zA-Z_]/.test(tok)) {
      parts.push(
        <span key={`id-${keyIdx++}`} className="text-foreground font-medium">
          {tok}
        </span>
      );
    } else {
      parts.push(
        <span key={`op-${keyIdx++}`} className="text-muted-foreground">
          {tok}
        </span>
      );
    }
    lastIndex = match.index + tok.length;
    match = tokenRegex.exec(expr);
  }

  if (lastIndex < expr.length) {
    parts.push(<span key={`tail-${keyIdx}`}>{expr.slice(lastIndex)}</span>);
  }
  return parts;
}

function checkLocalExprSyntax(expr: string): string | null {
  const trimmed = expr.trim();
  if (!trimmed) return "表达式不能为空";
  if (trimmed.length > 2000) return "表达式超出 2000 字符上限";
  let depth = 0;
  let inQuotes = false;
  for (let i = 0; i < trimmed.length; i += 1) {
    const ch = trimmed[i];
    if (ch === "'") {
      inQuotes = !inQuotes;
    } else if (!inQuotes) {
      if (ch === "(") depth += 1;
      if (ch === ")") depth -= 1;
      if (depth < 0) return "右括号 ')' 多余";
    }
  }
  if (inQuotes) return "单引号字符串未闭合";
  if (depth !== 0) return "括号 '(' 与 ')' 未配对闭合";
  return null;
}

function formatEvalValue(val: unknown): string {
  if (val === null || val === undefined) return "null";
  if (typeof val === "number") {
    return Number.isInteger(val) ? String(val) : Number(val.toFixed(6)).toString();
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

  // Live backend validation map keyed by `${scopeType}::${expression}`
  const [exprDiagnostics, setExprDiagnostics] = useState<
    Record<string, LiveExprDiagnostic>
  >({});

  // Full aggregate recompute state
  const [recomputing, setRecomputing] = useState(false);
  const [recomputeStatus, setRecomputeStatus] = useState<string | null>(null);

  // Sample runtime node dry-run evaluator state
  const [sampleNodes, setSampleNodes] = useState<BlueNode[]>([]);
  const [loadingSampleNodes, setLoadingSampleNodes] = useState(false);
  const [selectedSampleNodeId, setSelectedSampleNodeId] = useState<string>("");
  const [dryRunExpr, setDryRunExpr] = useState<string>("");
  const [evaluating, setEvaluating] = useState(false);
  const [evalResult, setEvalResult] = useState<EvaluateExprResponse | null>(
    null
  );
  const [evalError, setEvalError] = useState<string | null>(null);

  // Debounced live backend expression validation for derived formulas & agg filters
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
            // Graceful fallback to local syntax check if type is not published yet
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

  // Load sample runtime nodes when selected nodeType.name changes
  useEffect(() => {
    setRecomputeStatus(null);
    setEvalResult(null);
    setEvalError(null);

    const defaultFormula =
      nodeType.derived_rules[0]?.expression ||
      (ownMetrics.length >= 2
        ? `if(${ownMetrics[1]} > 0, ${ownMetrics[0]} / ${ownMetrics[1]}, null)`
        : ownMetrics[0] ?? "0");
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
    setRecomputing(true);
    setRecomputeStatus(null);
    try {
      const res = await xblueApi.recomputeAggregate({
        type_name: nodeType.name,
      });
      const shortJob = res.job_id ? res.job_id.slice(0, 8) : "queued";
      setRecomputeStatus(
        `已入队全量聚合重算任务 (${res.type_name ?? nodeType.name} · job=${shortJob})`
      );
    } catch (err) {
      setRecomputeStatus(
        err instanceof Error ? err.message : "触发聚合重算失败"
      );
    } finally {
      setRecomputing(false);
    }
  };

  const executeDryRun = async (exprOverride?: string) => {
    const targetExpr = (exprOverride ?? dryRunExpr).trim();
    if (!selectedSampleNodeId) {
      setEvalError("请先选择一个已存在的运行时样本节点");
      return;
    }
    if (!targetExpr) {
      setEvalError("试算表达式不能为空");
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
      setEvalError(err instanceof Error ? err.message : "表达式样本试算失败");
    } finally {
      setEvaluating(false);
    }
  };

  const addAggRule = () => {
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
      : allNodeTypes[0]?.name ?? "";
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
    onChangeAggRules(
      nodeType.agg_rules.map((r, i) => (i === idx ? { ...r, ...patch } : r))
    );
  };

  const removeAggRule = (idx: number) => {
    onChangeAggRules(nodeType.agg_rules.filter((_, i) => i !== idx));
  };

  const addDerivedRule = () => {
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
    onChangeDerivedRules(
      nodeType.derived_rules.map((r, i) =>
        i === idx ? { ...r, ...patch } : r
      )
    );
  };

  const removeDerivedRule = (idx: number) => {
    onChangeDerivedRules(nodeType.derived_rules.filter((_, i) => i !== idx));
  };

  const insertTokenIntoDerived = (idx: number, token: string) => {
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
        <div className="space-y-0.5">
          <div className="font-semibold flex items-center gap-1.5">
            <span>传播重算控制 ({nodeType.name})</span>
            <Badge variant="outline">DAG 拓扑序</Badge>
          </div>
          <div className="text-[11px] text-muted-foreground">
            对企业主图该类型全部实例入队 GRAPH_AGGREGATE 重算任务
          </div>
        </div>
        <Button
          type="button"
          variant="outline"
          className="h-7 px-2.5 py-1 text-xs"
          data-testid="recompute-agg-btn"
          disabled={recomputing || !nodeType.name.trim()}
          onClick={() => void handleRecomputeAggregate()}
        >
          <RefreshCw
            className={cn(
              "h-3 w-3 text-xblue",
              recomputing && "animate-spin"
            )}
          />
          {recomputing ? "入队重算中..." : "触发全量重算"}
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
          <div>
            <span className="font-semibold">动态聚合规则 (agg_rules)</span>
            <span className="ml-2 text-[11px] text-muted-foreground">
              沿拓扑关系聚合邻居节点指标
            </span>
          </div>
          <Button
            variant="outline"
            className="h-7 px-2 py-1 text-xs"
            data-testid="add-agg-rule-btn"
            onClick={addAggRule}
          >
            <Plus className="h-3 w-3" />
            新增聚合规则
          </Button>
        </div>

        {nodeType.agg_rules.length === 0 ? (
          <div className="rounded border border-dashed border-border p-3 text-center text-muted-foreground">
            暂无跨关系动态聚合规则。
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
                  ? checkLocalExprSyntax(filterTrimmed)
                  : null;

              return (
                <div
                  key={idx}
                  className="rounded border border-border bg-muted/20 p-2.5 space-y-2"
                >
                  <div className="flex items-center justify-between">
                    <span className="font-mono-tech text-[11px] font-semibold text-xblue">
                      {rule.target_metric || "?"} = {rule.op}(
                      {rule.source_type_name || "?"}.{rule.source_metric || "?"}{" "}
                      via {rule.via_link_type || "?"} {rule.direction})
                    </span>
                    <button
                      type="button"
                      onClick={() => removeAggRule(idx)}
                      className="rounded p-1 text-muted-foreground hover:text-red-600 cursor-pointer"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>

                  <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        目标指标 (target_metric)
                      </label>
                      {ownMetrics.length > 0 ? (
                        <select
                          value={rule.target_metric}
                          onChange={(e) =>
                            updateAggRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          className="h-7 w-full rounded border border-input bg-background px-1.5 font-mono-tech text-xs"
                        >
                          <option value="">选择指标...</option>
                          {ownMetrics.map((m) => (
                            <option key={m} value={m}>
                              {m}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <Input
                          value={rule.target_metric}
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
                        聚合算子 (op)
                      </label>
                      <select
                        value={rule.op}
                        onChange={(e) =>
                          updateAggRule(idx, { op: e.target.value as AggFn })
                        }
                        className="h-7 w-full rounded border border-input bg-background px-1.5 font-mono-tech text-xs"
                      >
                        {AGG_OPS.map((op) => (
                          <option key={op} value={op}>
                            {op}
                          </option>
                        ))}
                      </select>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        关联关系 (via_link_type)
                      </label>
                      <select
                        value={rule.via_link_type}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            via_link_type: e.target.value,
                          })
                        }
                        className="h-7 w-full rounded border border-input bg-background px-1.5 font-mono-tech text-xs"
                      >
                        <option value="">选择关系...</option>
                        {edgeTypes.map((et, eIdx) => (
                          <option key={`${et.name}-${eIdx}`} value={et.name}>
                            {et.name} ({et.src_type_name}→{et.dst_type_name})
                          </option>
                        ))}
                      </select>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        遍历方向 (direction)
                      </label>
                      <select
                        value={rule.direction}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            direction: e.target.value as LinkDirection,
                          })
                        }
                        className="h-7 w-full rounded border border-input bg-background px-1.5 font-mono-tech text-xs"
                      >
                        <option value="OUT">OUT (本节点为源)</option>
                        <option value="IN">IN (本节点为目标)</option>
                      </select>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 md:grid-cols-3 gap-2 items-end">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        邻居类型 (source_type_name)
                      </label>
                      <select
                        value={rule.source_type_name}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            source_type_name: e.target.value,
                          })
                        }
                        className="h-7 w-full rounded border border-input bg-background px-1.5 font-mono-tech text-xs"
                      >
                        <option value="">选择源类型...</option>
                        {allNodeTypes.map((nt) => (
                          <option key={nt.name} value={nt.name}>
                            {nt.name} ({nt.display_name})
                          </option>
                        ))}
                      </select>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-0.5">
                        邻居指标 (source_metric)
                      </label>
                      {sourceMetricCandidates.length > 0 ? (
                        <select
                          value={rule.source_metric}
                          onChange={(e) =>
                            updateAggRule(idx, {
                              source_metric: e.target.value,
                            })
                          }
                          className="h-7 w-full rounded border border-input bg-background px-1.5 font-mono-tech text-xs"
                        >
                          <option value="">选择源指标...</option>
                          {sourceMetricCandidates.map((sm) => (
                            <option key={sm} value={sm}>
                              {sm}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <Input
                          value={rule.source_metric}
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

                    <label className="flex items-center gap-1.5 h-7 cursor-pointer">
                      <input
                        type="checkbox"
                        checked={rule.weight_by_link}
                        onChange={(e) =>
                          updateAggRule(idx, {
                            weight_by_link: e.target.checked,
                          })
                        }
                      />
                      <span className="text-[11px]">
                        按边权重加权 (weight_by_link)
                      </span>
                    </label>
                  </div>

                  <div className="space-y-1">
                    <div className="flex items-center justify-between">
                      <label className="block text-[11px] text-muted-foreground">
                        子节点过滤表达式 (filter，可选)
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
                      onChange={(e) =>
                        updateAggRule(idx, { filter: e.target.value })
                      }
                      placeholder="例如 prop.status != 'CANCELED'"
                      className="h-7 px-2 text-xs font-mono-tech"
                    />
                    {localFilterErr && (
                      <div className="text-[11px] text-red-600 dark:text-red-400">
                        语法提示: {localFilterErr}
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
                            ? `列 ${fIss.column}: ${fIss.message}`
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
          <div>
            <span className="font-semibold">派生指标规则 (derived_rules)</span>
            <span className="ml-2 text-[11px] text-muted-foreground">
              节点内确定性表达式计算
            </span>
          </div>
          <Button
            variant="outline"
            className="h-7 px-2 py-1 text-xs"
            data-testid="add-derived-rule-btn"
            onClick={addDerivedRule}
          >
            <Plus className="h-3 w-3 text-xblue" />
            新增派生公式
          </Button>
        </div>

        {nodeType.derived_rules.length === 0 ? (
          <div className="rounded border border-dashed border-border p-3 text-center text-muted-foreground">
            暂无节点内派生公式规则。
          </div>
        ) : (
          <div className="space-y-2">
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
                ? checkLocalExprSyntax(dRule.expression)
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
                  className="rounded border border-border bg-muted/20 p-2.5 space-y-2"
                >
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2 flex-1">
                      <span className="text-[11px] text-muted-foreground shrink-0">
                        目标指标:
                      </span>
                      {ownMetrics.length > 0 ? (
                        <select
                          value={dRule.target_metric}
                          onChange={(e) =>
                            updateDerivedRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-xs"
                        >
                          <option value="">选择指标...</option>
                          {ownMetrics.map((m) => (
                            <option key={m} value={m}>
                              {m}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <Input
                          value={dRule.target_metric}
                          onChange={(e) =>
                            updateDerivedRule(idx, {
                              target_metric: e.target.value,
                            })
                          }
                          placeholder="fill_rate"
                          className="h-7 w-40 px-2 text-xs font-mono-tech"
                        />
                      )}

                      <label className="flex items-center gap-1 cursor-pointer ml-2">
                        <input
                          type="checkbox"
                          checked={dRule.enabled}
                          onChange={(e) =>
                            updateDerivedRule(idx, {
                              enabled: e.target.checked,
                            })
                          }
                        />
                        <span className="text-[11px]">启用</span>
                      </label>
                    </div>

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
                        <Badge
                          variant="outline"
                          className="border-red-500/40 text-red-600 dark:text-red-400"
                        >
                          <AlertCircle className="h-3 w-3 mr-1" />
                          需修正
                        </Badge>
                      ) : (
                        <Badge
                          variant="outline"
                          className="border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
                        >
                          <CheckCircle2 className="h-3 w-3 mr-1" />
                          语法就绪
                        </Badge>
                      )}
                      <button
                        type="button"
                        onClick={() => {
                          setDryRunExpr(dRule.expression);
                          if (selectedSampleNodeId) {
                            void executeDryRun(dRule.expression);
                          }
                        }}
                        title="带入样本节点试算"
                        className="rounded border border-border bg-background px-1.5 py-0.5 text-[10px] text-muted-foreground hover:text-xblue cursor-pointer"
                      >
                        试算
                      </button>
                      <button
                        type="button"
                        onClick={() => removeDerivedRule(idx)}
                        className="rounded p-1 text-muted-foreground hover:text-red-600 cursor-pointer"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </div>

                  {/* Monospace Expression Input */}
                  <textarea
                    rows={2}
                    data-testid={`derived-expr-input-${idx}`}
                    value={dRule.expression}
                    onChange={(e) =>
                      updateDerivedRule(idx, { expression: e.target.value })
                    }
                    placeholder="if(total_demand > 0, inventory / total_demand, null)"
                    className={cn(
                      "w-full rounded border bg-background px-2.5 py-1.5 font-mono-tech text-xs focus:outline-none focus:ring-1 focus:ring-foreground",
                      hasIssue ? "border-red-500/60" : "border-input"
                    )}
                  />

                  {/* Live Syntax Highlighted Preview */}
                  <div className="rounded border border-border bg-background/80 px-2.5 py-1.5 font-mono-tech text-[11px] overflow-x-auto">
                    <span className="text-muted-foreground mr-1.5 select-none">
                      AST 预览:
                    </span>
                    {highlightExpression(dRule.expression || "")}
                  </div>

                  {/* Reference Quick Insert Chips */}
                  <div className="flex flex-wrap items-center gap-1">
                    <span className="text-[10px] text-muted-foreground">
                      引用插入:
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
                    {ownAttrs.map((a) => (
                      <button
                        key={`a-${a}`}
                        type="button"
                        onClick={() => insertTokenIntoDerived(idx, `prop.${a}`)}
                        className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono-tech text-[10px] cursor-pointer"
                      >
                        prop.{a}
                      </button>
                    ))}
                    {["if(", "safe_div(", "coalesce(", "round("].map((fn) => (
                      <button
                        key={fn}
                        type="button"
                        onClick={() => insertTokenIntoDerived(idx, fn)}
                        className="rounded border border-border bg-background px-1.5 py-0.5 font-mono-tech text-[10px] text-muted-foreground hover:text-foreground cursor-pointer"
                      >
                        {fn})
                      </button>
                    ))}
                  </div>

                  {localErr && (
                    <div className="text-[11px] text-red-600 dark:text-red-400">
                      语法提示: {localErr}
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
                          ? `列 ${bIss.column}: ${bIss.message}`
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
            <span className="font-semibold">样本节点试算 (Dry-Run 求值器)</span>
            <span className="text-[11px] text-muted-foreground">
              只读快照试算，不写库
            </span>
          </div>
          <Badge variant="outline">
            {loadingSampleNodes
              ? "加载中..."
              : `样本 ${sampleNodes.length} 个`}
          </Badge>
        </div>

        <div className="rounded border border-border bg-muted/20 p-2.5 space-y-2">
          <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-end">
            <div className="md:col-span-5">
              <label className="block text-[11px] text-muted-foreground mb-0.5">
                运行时样本节点 ({nodeType.name})
              </label>
              <select
                data-testid="expr-dryrun-node-select"
                value={selectedSampleNodeId}
                onChange={(e) => setSelectedSampleNodeId(e.target.value)}
                disabled={sampleNodes.length === 0}
                className="h-7 w-full rounded border border-input bg-background px-2 font-mono-tech text-xs"
              >
                {sampleNodes.length === 0 ? (
                  <option value="">暂无运行时节点实例</option>
                ) : (
                  sampleNodes.map((n) => (
                    <option key={n.id} value={n.id}>
                      {n.title} ({n.node_key})
                    </option>
                  ))
                )}
              </select>
            </div>

            <div className="md:col-span-5">
              <label className="block text-[11px] text-muted-foreground mb-0.5">
                试算表达式 (expression)
              </label>
              <Input
                data-testid="expr-dryrun-input"
                value={dryRunExpr}
                onChange={(e) => setDryRunExpr(e.target.value)}
                placeholder="例如 inventory / total_demand"
                className="h-7 px-2 text-xs font-mono-tech"
              />
            </div>

            <div className="md:col-span-2">
              <Button
                type="button"
                className="h-7 w-full px-2 py-1 text-xs"
                data-testid="expr-dryrun-submit-btn"
                disabled={
                  evaluating ||
                  !selectedSampleNodeId ||
                  dryRunExpr.trim().length === 0
                }
                onClick={() => void executeDryRun()}
              >
                <Play className="h-3 w-3" />
                {evaluating ? "计算中" : "执行试算"}
              </Button>
            </div>
          </div>

          {selectedSampleNode && (
            <div className="font-mono-tech text-[10px] text-muted-foreground truncate">
              当前快照 metrics: {JSON.stringify(selectedSampleNode.metrics ?? {})}
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
                  求值结果:
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
                    <Badge
                      key={wCode}
                      variant="outline"
                      className="border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-300"
                    >
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

