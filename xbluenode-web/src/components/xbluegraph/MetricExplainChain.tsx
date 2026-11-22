import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  GitBranch,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueApi,
  type ExplainInputItem,
  type MetricExplainResponse,
  type MetricProducer,
} from "@/lib/api/xblue";
import { Badge } from "@/components/ui/primitives";

const MAX_EXPLAIN_DEPTH = 4;

const PRODUCER_LABELS_ZH: Record<MetricProducer, string> = {
  AGG: "AGG 聚合",
  DERIVED: "DERIVED 派生",
  SYNC: "SYNC 映射同步",
  MANUAL: "MANUAL 手工",
  ACTION: "ACTION 动作",
  ALGORITHM: "ALGORITHM 算法",
};

const PRODUCER_LABELS_EN: Record<MetricProducer, string> = {
  AGG: "AGG Rollup",
  DERIVED: "DERIVED Formula",
  SYNC: "SYNC Mapped",
  MANUAL: "MANUAL",
  ACTION: "ACTION",
  ALGORITHM: "ALGORITHM",
};

function formatNumberCompact(val: number | null | undefined): string {
  if (val === null || val === undefined || !Number.isFinite(val)) {
    return "null";
  }
  if (Number.isInteger(val)) {
    return String(val);
  }
  return Number(val.toFixed(4)).toString();
}

function formatRuleSummary(
  producer: MetricProducer,
  rule: unknown
): string | null {
  if (!rule) return null;
  if (typeof rule === "string") {
    return rule.trim() || null;
  }
  if (typeof rule !== "object") {
    return null;
  }
  const rec = rule as Record<string, unknown>;

  if (producer === "DERIVED") {
    const expr =
      typeof rec.expression === "string" ? rec.expression.trim() : "";
    if (!expr) return null;
    return expr;
  }

  if (producer === "AGG") {
    const op = typeof rec.op === "string" && rec.op ? rec.op : "SUM";
    const srcType =
      typeof rec.source_type_name === "string" && rec.source_type_name
        ? rec.source_type_name
        : "?";
    const srcMetric =
      typeof rec.source_metric === "string" && rec.source_metric
        ? rec.source_metric
        : "?";
    const viaLink =
      typeof rec.via_link_type === "string" && rec.via_link_type
        ? rec.via_link_type
        : "?";
    const dir =
      typeof rec.direction === "string" && rec.direction
        ? rec.direction
        : "OUT";
    const base = `${op}(${srcType}.${srcMetric}) via ${viaLink} (${dir})`;
    const extras: string[] = [];
    if (rec.weight_by_link === true) {
      extras.push("weight_by_link");
    }
    if (typeof rec.filter === "string" && rec.filter.trim().length > 0) {
      extras.push(`where ${rec.filter.trim()}`);
    }
    return extras.length > 0 ? `${base} [${extras.join(" · ")}]` : base;
  }

  if (producer === "SYNC") {
    const mapping =
      typeof rec.mapping_name === "string"
        ? rec.mapping_name
        : typeof rec.name === "string"
          ? rec.name
          : "";
    const dataset = typeof rec.dataset === "string" ? rec.dataset : "";
    if (mapping || dataset) {
      return [mapping, dataset].filter(Boolean).join(" ← ");
    }
  }

  return null;
}

interface ExplainChainLevelProps {
  nodeId: string;
  metric: string;
  depth: number;
  visited: ReadonlySet<string>;
}

function ExplainChainLevel({
  nodeId,
  metric,
  depth,
  visited,
}: ExplainChainLevelProps) {
  const { isZh, tx, fmtTime } = useI18n();
  const [report, setReport] = useState<MetricExplainResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const [expandedKeys, setExpandedKeys] = useState<Record<string, boolean>>({});

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setUnavailable(false);
    setExpandedKeys({});

    void xblueApi
      .explainNodeMetric(nodeId, metric)
      .then((res) => {
        if (!cancelled) {
          setReport(res);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setReport(null);
          setUnavailable(true);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [nodeId, metric]);

  const currentVisitSet = useMemo(() => {
    const next = new Set(visited);
    next.add(`${nodeId}::${metric}`);
    return next;
  }, [visited, nodeId, metric]);

  if (loading) {
    return (
      <div className="py-2 text-[11px] text-muted-foreground font-mono-tech">
        {tx(`追溯中 (${metric})…`, `Tracing (${metric})…`)}
      </div>
    );
  }

  if (unavailable || !report) {
    return (
      <div className="py-1.5 text-[11px] text-muted-foreground">
        {tx("暂无指标溯源记录", "No lineage record")}
      </div>
    );
  }

  const ruleText = formatRuleSummary(report.producer, report.rule);
  const formattedTime = report.computed_at
    ? fmtTime(report.computed_at, true)
    : null;
  const producerLabel = isZh
    ? PRODUCER_LABELS_ZH[report.producer]
    : PRODUCER_LABELS_EN[report.producer];

  const toggleInputExpand = (itemKey: string) => {
    setExpandedKeys((prev) => ({
      ...prev,
      [itemKey]: !prev[itemKey],
    }));
  };

  return (
    <div
      className="space-y-2"
      data-testid={depth === 0 ? "explain-level-root" : `explain-level-${depth}`}
    >
      {/* Producer Badge, Value, Computed Time & Warnings */}
      <div className="flex flex-wrap items-center justify-between gap-1.5">
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge
            variant={
              report.producer === "AGG" || report.producer === "DERIVED"
                ? "xblue"
                : "neutral"
            }
            data-testid={depth === 0 ? "explain-producer-badge" : undefined}
          >
            {producerLabel}
          </Badge>

          {report.value !== null && (
            <span className="font-mono-tech text-[11px] text-foreground">
              {report.metric} ={" "}
              <strong>{formatNumberCompact(report.value)}</strong>
            </span>
          )}

          {report.warnings.map((wCode) => (
            <Badge
              key={wCode}
              variant="warning"
              data-testid={
                depth === 0 ? `explain-warning-${wCode}` : undefined
              }
            >
              <AlertTriangle className="h-3 w-3 mr-1" />
              {wCode}
            </Badge>
          ))}
        </div>

        {formattedTime && (
          <span
            className="font-mono-tech text-[10px] text-muted-foreground"
            data-testid={depth === 0 ? "explain-computed-at" : undefined}
          >
            {formattedTime}
          </span>
        )}
      </div>

      {/* Rule Formula / Aggregation Summary */}
      {ruleText && (
        <div
          className="rounded border border-border bg-muted/30 px-2 py-1.5 font-mono-tech text-[11px] text-foreground break-all"
          data-testid={depth === 0 ? "explain-rule-summary" : undefined}
        >
          {ruleText}
        </div>
      )}

      {/* Upstream Calculation Chain Inputs */}
      {report.inputs.length > 0 && (
        <div className="space-y-1.5">
          <div className="text-[11px] font-medium text-muted-foreground flex items-center justify-between">
            <span>
              {tx("上游输入", "Upstream Inputs")} ({report.inputs.length})
            </span>
          </div>

          <div
            className="space-y-1.5"
            data-testid={depth === 0 ? "explain-inputs-list" : undefined}
          >
            {report.inputs.map((inp: ExplainInputItem, idx: number) => {
              const targetKey = `${inp.node_id}::${inp.metric}`;
              const itemKey = `${targetKey}::${idx}`;
              const isPropRef = inp.metric.startsWith("prop.");
              const isCycle = currentVisitSet.has(targetKey);
              const reachedMaxDepth = depth >= MAX_EXPLAIN_DEPTH;
              const canExpand =
                Boolean(inp.node_id) &&
                Boolean(inp.metric) &&
                !isPropRef &&
                !isCycle &&
                !reachedMaxDepth;
              const isExpanded = Boolean(expandedKeys[itemKey]);

              return (
                <div
                  key={itemKey}
                  data-testid={`explain-input-item-${inp.node_key || idx}-${inp.metric}`}
                  className="rounded border border-border bg-background p-2 space-y-1.5"
                >
                  <div className="flex flex-wrap items-center justify-between gap-1.5">
                    <div className="flex flex-wrap items-center gap-1.5 font-mono-tech text-[11px]">
                      <span className="font-sans font-medium text-foreground">
                        {inp.title || inp.node_key}
                      </span>
                      <span className="text-muted-foreground">
                        ({inp.node_key})
                      </span>
                      <span>·</span>
                      <span className="text-xblue font-medium">
                        {inp.metric}
                      </span>
                      <span>=</span>
                      <span className="font-semibold text-foreground">
                        {formatNumberCompact(inp.value)}
                      </span>
                      {inp.weight !== undefined && (
                        <Badge variant="outline">
                          weight={formatNumberCompact(inp.weight)}
                        </Badge>
                      )}
                      {inp.type_name && (
                        <Badge variant="outline">{inp.type_name}</Badge>
                      )}
                    </div>

                    {canExpand ? (
                      <button
                        type="button"
                        onClick={() => toggleInputExpand(itemKey)}
                        data-testid={`explain-expand-${inp.node_key || idx}-${inp.metric}`}
                        className="inline-flex items-center gap-0.5 rounded border border-border bg-muted/40 px-1.5 py-0.5 text-[10px] text-foreground hover:border-xblue hover:text-xblue cursor-pointer"
                      >
                        {isExpanded ? (
                          <>
                            <ChevronDown className="h-3 w-3 text-xblue" />
                            {tx("收起", "Collapse")}
                          </>
                        ) : (
                          <>
                            <ChevronRight className="h-3 w-3 text-xblue" />
                            {tx("展开上游", "Trace")}
                          </>
                        )}
                      </button>
                    ) : isCycle ? (
                      <Badge variant="outline">
                        {tx("环路截断", "Cycle Cut")}
                      </Badge>
                    ) : reachedMaxDepth && !isPropRef ? (
                      <Badge variant="outline">
                        {tx("深度上限", "Max Depth")}
                      </Badge>
                    ) : null}
                  </div>

                  {isExpanded && canExpand && (
                    <div className="border-l-2 border-xblue/40 pl-2.5 pt-1 ml-1">
                      <ExplainChainLevel
                        nodeId={inp.node_id}
                        metric={inp.metric}
                        depth={depth + 1}
                        visited={currentVisitSet}
                      />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

export interface MetricExplainChainProps {
  nodeId: string;
  metric: string;
}

export function MetricExplainChain({
  nodeId,
  metric,
}: MetricExplainChainProps) {
  const { tx } = useI18n();
  const emptySet = useMemo(() => new Set<string>(), []);

  return (
    <div
      className="rounded border border-border bg-card p-2.5 space-y-2 text-xs"
      data-testid="metric-explain-chain"
    >
      <div className="flex items-center justify-between border-b border-border pb-1.5">
        <span className="font-semibold flex items-center gap-1.5">
          <GitBranch className="h-3.5 w-3.5 text-xblue" />
          {tx("指标计算溯源链", "Metric Lineage")} ({metric})
        </span>
        <span className="font-mono-tech text-[10px] text-muted-foreground">
          Explain DAG
        </span>
      </div>

      <ExplainChainLevel
        nodeId={nodeId}
        metric={metric}
        depth={0}
        visited={emptySet}
      />
    </div>
  );
}
