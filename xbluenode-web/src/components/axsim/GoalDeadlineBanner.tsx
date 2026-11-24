import { AlertTriangle, CheckCircle2, Clock, Flame } from "lucide-react";
import type {
  DeadlineReport,
  FiredEventRecord,
  SandboxGraphNode,
} from "@/lib/api/axsim";
import { Badge, Card } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

export interface GoalDeadlineBannerProps {
  deadlineReport?: DeadlineReport | null;
  firedEvents?: FiredEventRecord[];
  nodes: SandboxGraphNode[];
}

export function GoalDeadlineBanner({
  deadlineReport,
  firedEvents = [],
  nodes,
}: GoalDeadlineBannerProps) {
  const { tx, tEnum, fmtNum } = useI18n();

  const hasReport = Boolean(deadlineReport && deadlineReport.items.length > 0);
  const hasFired = firedEvents.length > 0;
  if (!hasReport && !hasFired) {
    return null;
  }

  const nodeTitleById = new Map<string, string>();
  for (const n of nodes) {
    nodeTitleById.set(n.id, n.title || n.node_key || n.id.slice(0, 8));
  }

  const toneClass = !deadlineReport
    ? "border-border bg-muted/20"
    : deadlineReport.status === "IMMEDIATE_BREACH_UNAVOIDABLE_BY_LAG"
      ? "border-red-500/50 bg-red-500/10"
      : deadlineReport.status === "ACTIONABLE_WINDOW"
        ? "border-amber-500/50 bg-amber-500/10"
        : "border-emerald-500/40 bg-emerald-500/10";

  return (
    <Card
      className={`p-3.5 space-y-2.5 ${toneClass}`}
      data-testid="goal-deadline-banner"
    >
      {deadlineReport && deadlineReport.items.length > 0 && (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2 text-sm font-semibold">
              {deadlineReport.status === "SATISFIED" ? (
                <CheckCircle2 className="h-4 w-4 text-emerald-600 dark:text-emerald-400" />
              ) : (
                <AlertTriangle className="h-4 w-4 text-amber-600 dark:text-amber-400" />
              )}
              <span>{tx("红线干预窗口", "Goal Intervention Window")}</span>
              <Badge
                variant={
                  deadlineReport.status === "SATISFIED"
                    ? "success"
                    : deadlineReport.status ===
                        "IMMEDIATE_BREACH_UNAVOIDABLE_BY_LAG"
                      ? "danger"
                      : "warning"
                }
                data-testid="deadline-window-status"
              >
                {tEnum(deadlineReport.status)}
              </Badge>
            </div>

            <div className="flex flex-wrap items-center gap-3 text-xs">
              {deadlineReport.earliest_breach_step != null && (
                <span
                  className="inline-flex items-center gap-1 font-medium text-red-700 dark:text-red-300"
                  data-testid="earliest-breach-step"
                >
                  <Clock className="h-3.5 w-3.5" />
                  {tx(
                    `最早失守 第 ${deadlineReport.earliest_breach_step} 步`,
                    `Breach Step ${deadlineReport.earliest_breach_step}`
                  )}
                </span>
              )}
              {deadlineReport.earliest_deadline_step != null && (
                <span
                  className="inline-flex items-center gap-1 font-semibold text-amber-800 dark:text-amber-200"
                  data-testid="earliest-deadline-step"
                >
                  <Clock className="h-3.5 w-3.5" />
                  {deadlineReport.earliest_deadline_step >= 0
                    ? tx(
                        `最晚干预 0..=${deadlineReport.earliest_deadline_step} 步`,
                        `Deadline 0..=${deadlineReport.earliest_deadline_step}`
                      )
                    : tx(
                        `传导滞后超期 (${deadlineReport.earliest_deadline_step})`,
                        `Past Lag Window (${deadlineReport.earliest_deadline_step})`
                      )}
                </span>
              )}
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
            {deadlineReport.items.map((diag) => {
              const breachNodeLabel = diag.breach_node_id
                ? (nodeTitleById.get(diag.breach_node_id) ??
                  diag.breach_node_id.slice(0, 8))
                : null;
              return (
                <div
                  key={diag.constraint_id}
                  className="rounded border border-border/70 bg-background/80 px-2.5 py-1.5 text-xs flex flex-wrap items-center justify-between gap-2"
                  data-testid={`deadline-item-${diag.constraint_id}`}
                >
                  <div className="font-medium truncate">
                    {diag.constraint_name || diag.constraint_id}
                    {breachNodeLabel ? ` · ${breachNodeLabel}` : ""}
                  </div>
                  <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
                    {diag.breached ? (
                      <>
                        <span>
                          {tx(
                            `失守步 ${diag.breach_step ?? 0}`,
                            `Breach ${diag.breach_step ?? 0}`
                          )}
                          {diag.breach_value != null
                            ? ` (${fmtNum(diag.breach_value, 2)})`
                            : ""}
                        </span>
                        {diag.min_lag_steps != null && (
                          <span>
                            {tx(
                              `最短延迟 ${diag.min_lag_steps} 步`,
                              `Lag ${diag.min_lag_steps}`
                            )}
                          </span>
                        )}
                        {diag.deadline_step != null && (
                          <span className="font-semibold text-foreground">
                            {tx(
                              `截止步 ${diag.deadline_step}`,
                              `Deadline ${diag.deadline_step}`
                            )}
                          </span>
                        )}
                      </>
                    ) : (
                      <span>{tEnum("SATISFIED")}</span>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {firedEvents.length > 0 && (
        <div
          className="flex flex-wrap items-center gap-1.5 pt-1 border-t border-border/60 text-xs"
          data-testid="fired-events-strip"
        >
          <span className="inline-flex items-center gap-1 font-medium text-muted-foreground mr-1">
            <Flame className="h-3.5 w-3.5 text-orange-500" />
            {tx("已触发事件", "Fired Events")}
          </span>
          {firedEvents.map((ev, idx) => (
            <Badge
              key={`${ev.event_id}-${ev.step}-${idx}`}
              variant="outline"
              data-testid={`fired-event-badge-${ev.event_id}-${ev.step}`}
            >
              {ev.event_name || ev.event_id} ·{" "}
              {tx(`第 ${ev.step} 步`, `Step ${ev.step}`)}
              {ev.trigger_value != null
                ? ` (${fmtNum(ev.trigger_value, 2)})`
                : ""}
            </Badge>
          ))}
        </div>
      )}
    </Card>
  );
}
