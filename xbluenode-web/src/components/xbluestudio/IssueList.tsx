import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  Crosshair,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { ValidationIssue } from "@/lib/api/xblue";
import { Badge } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface IssueListProps {
  issues: ValidationIssue[];
  validating: boolean;
  embedded?: boolean;
  onLocateIssue: (issue: ValidationIssue) => void;
}

const ISSUE_CODE_LABELS: Readonly<
  Record<string, { zh: string; en: string }>
> = {
  DUPLICATE_NODE_TYPE: { zh: "节点类型重名", en: "Duplicate Node Type" },
  INVALID_NODE_NAME: { zh: "节点类型命名不合法", en: "Invalid Node Name" },
  DUPLICATE_PROPERTY: { zh: "属性或指标重名", en: "Duplicate Property" },
  INVALID_PROPERTY_NAME: { zh: "字段命名不合法", en: "Invalid Field Name" },
  UNKNOWN_SRC_TYPE: { zh: "未知源节点类型", en: "Unknown Source Type" },
  UNKNOWN_DST_TYPE: { zh: "未知目标节点类型", en: "Unknown Target Type" },
  DUPLICATE_EDGE_TYPE: { zh: "关系类型重复", en: "Duplicate Relation" },
  INVALID_IMPACT_WEIGHT: { zh: "传导权重越界", en: "Invalid Impact Weight" },
  UNKNOWN_TARGET_METRIC: { zh: "目标指标未定义", en: "Undefined Target Metric" },
  UNKNOWN_SOURCE_METRIC: { zh: "源指标未定义", en: "Undefined Source Metric" },
  UNKNOWN_LINK_TYPE: { zh: "关联关系未定义", en: "Undefined Relation" },
  RULE_CYCLE_DETECTED: { zh: "指标计算存在环路", en: "Metric Rule Cycle" },
  EXPR_INVALID: { zh: "表达式语法错误", en: "Invalid Expression" },
  EXPR_TYPE_MISMATCH: { zh: "表达式类型不匹配", en: "Expression Type Mismatch" },
  MAPPING_DATASET_MISSING: { zh: "源数据集缺失", en: "Missing Dataset" },
  MAPPING_NODE_TYPE_MISSING: { zh: "映射目标类型缺失", en: "Missing Target Type" },
};

export function IssueList({
  issues,
  validating,
  embedded = false,
  onLocateIssue,
}: IssueListProps) {
  const { isZh, tx } = useI18n();
  const errorCount = issues.filter((i) => i.severity === "ERROR").length;
  const warnCount = issues.filter((i) => i.severity === "WARN").length;

  return (
    <div
      className={cn(
        "space-y-2 text-xs",
        !embedded && "rounded-md border border-border bg-card p-3"
      )}
      data-testid="studio-issue-list"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          {!embedded && (
            <span className="font-semibold">
              {tx("模型校验诊断", "Schema Diagnostics")}
            </span>
          )}
          {validating ? (
            <Badge variant="outline">
              {tx("校验中…", "Validating…")}
            </Badge>
          ) : errorCount === 0 && warnCount === 0 ? (
            <span className="inline-flex items-center gap-1 text-emerald-600 dark:text-emerald-400 font-medium">
              <CheckCircle2 className="h-3.5 w-3.5" />
              {tx("校验通过 · 0 阻断错误", "Valid · 0 Blocking Errors")}
            </span>
          ) : null}
        </div>

        <div className="flex items-center gap-1.5 font-mono-tech">
          <span
            data-testid="issue-error-count"
            className={cn(
              "rounded px-1.5 py-0.5 text-[11px]",
              errorCount > 0
                ? "bg-red-500/15 text-red-600 dark:text-red-400 font-semibold"
                : "bg-muted text-muted-foreground"
            )}
          >
            ERROR {errorCount}
          </span>
          <span
            data-testid="issue-warn-count"
            className={cn(
              "rounded px-1.5 py-0.5 text-[11px]",
              warnCount > 0
                ? "bg-amber-500/15 text-amber-600 dark:text-amber-400 font-semibold"
                : "bg-muted text-muted-foreground"
            )}
          >
            WARN {warnCount}
          </span>
        </div>
      </div>

      {issues.length > 0 && (
        <div className="max-h-44 overflow-y-auto divide-y divide-border rounded border border-border">
          {issues.map((issue, idx) => {
            const isErr = issue.severity === "ERROR";
            const codeMeta = ISSUE_CODE_LABELS[issue.code];
            const codeLabel = codeMeta
              ? isZh
                ? `${codeMeta.zh} (${issue.code})`
                : `${codeMeta.en} (${issue.code})`
              : issue.code;
            return (
              <button
                key={`${issue.code}-${issue.path}-${idx}`}
                type="button"
                data-testid={`validation-issue-${idx}`}
                onClick={() => onLocateIssue(issue)}
                className="w-full flex items-start justify-between gap-2 px-2.5 py-2 text-left hover:bg-muted/50 transition-colors cursor-pointer"
              >
                <div className="flex items-start gap-2 min-w-0">
                  {isErr ? (
                    <AlertCircle className="h-3.5 w-3.5 text-red-600 dark:text-red-400 shrink-0 mt-0.5" />
                  ) : (
                    <AlertTriangle className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
                  )}
                  <div className="min-w-0 space-y-0.5">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span
                        className={cn(
                          "font-mono-tech text-[10px] font-semibold rounded px-1 py-0.2",
                          isErr
                            ? "bg-red-500/15 text-red-600 dark:text-red-400"
                            : "bg-amber-500/15 text-amber-600 dark:text-amber-400"
                        )}
                      >
                        {codeLabel}
                      </span>
                      <span className="font-mono-tech text-[11px] text-muted-foreground truncate">
                        {issue.path || "schema_doc"}
                      </span>
                    </div>
                    <div className="text-foreground">{issue.message}</div>
                  </div>
                </div>
                <span className="inline-flex items-center gap-0.5 text-[10px] text-xblue shrink-0 mt-0.5">
                  <Crosshair className="h-3 w-3" />
                  {tx("定位", "Locate")}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
