import { AlertCircle, AlertTriangle, CheckCircle2, Crosshair } from "lucide-react";
import type { ValidationIssue } from "@/lib/api/xblue";
import { Badge } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface IssueListProps {
  issues: ValidationIssue[];
  validating: boolean;
  onLocateIssue: (issue: ValidationIssue) => void;
}

export function IssueList({
  issues,
  validating,
  onLocateIssue,
}: IssueListProps) {
  const errorCount = issues.filter((i) => i.severity === "ERROR").length;
  const warnCount = issues.filter((i) => i.severity === "WARN").length;

  return (
    <div
      className="rounded-md border border-border bg-card p-3 space-y-2 text-xs"
      data-testid="studio-issue-list"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <span className="font-semibold">实时模型校验诊断</span>
          {validating ? (
            <Badge variant="outline">500ms 防抖校验中...</Badge>
          ) : errorCount === 0 && warnCount === 0 ? (
            <span className="inline-flex items-center gap-1 text-emerald-600 dark:text-emerald-400 font-medium">
              <CheckCircle2 className="h-3.5 w-3.5" />
              校验通过 · 0 阻断错误
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
                        {issue.code}
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
                  定位
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
