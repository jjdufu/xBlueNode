import { useNavigate } from "react-router";
import {
  AlertTriangle,
  CheckSquare,
  Clock,
  FlaskConical,
  ShieldAlert,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  PulseAlertItem,
  PulseApprovalItem,
  PulseSimulationItem,
} from "@/lib/api/axops";
import type { DeadlineWindowStatus } from "@/lib/api/enums";
import {
  Badge,
  Button,
  Card,
  EmptyState,
  StatusBadge,
} from "@/components/ui/primitives";

export interface ActionAndAlertFeedProps {
  activeAlerts: PulseAlertItem[];
  pendingApprovals: PulseApprovalItem[];
  recentSimulations: PulseSimulationItem[];
}

function deadlineBadgeVariant(
  status: DeadlineWindowStatus | null
): "danger" | "warning" | "success" | "outline" {
  switch (status) {
    case "IMMEDIATE_BREACH_UNAVOIDABLE_BY_LAG":
      return "danger";
    case "ACTIONABLE_WINDOW":
      return "warning";
    case "SATISFIED":
      return "success";
    default:
      return "outline";
  }
}

export function ActionAndAlertFeed({
  activeAlerts,
  pendingApprovals,
  recentSimulations,
}: ActionAndAlertFeedProps) {
  const navigate = useNavigate();
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();

  const totalActionable = activeAlerts.length + pendingApprovals.length;

  const stripSeverityTag = (title: string): string =>
    title.replace(/^\[(WARN|WARNING|ERROR|CRITICAL|INFO)\]\s*/i, "");

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 items-start">
      {/* Left: Live Alerts & Pending Approvals */}
      <Card
        className="p-4 flex flex-col gap-3"
        data-testid="dashboard-alert-approval-feed"
      >
        <div className="flex items-center justify-between gap-2 border-b border-border pb-2.5">
          <div className="flex items-center gap-2 min-w-0">
            <ShieldAlert className="h-4 w-4 text-xblue shrink-0" />
            <h2 className="text-sm font-semibold truncate">
              {tx("实时告警与待批动作", "Live Alerts & Pending Approvals")}
            </h2>
          </div>
          <div className="flex items-center gap-1.5 shrink-0">
            <Badge
              variant={activeAlerts.length > 0 ? "danger" : "outline"}
              className="font-mono-tech tabular-nums"
            >
              {tx(
                `${fmtNum(activeAlerts.length, 0)} 告警`,
                `${fmtNum(activeAlerts.length, 0)} Alerts`
              )}
            </Badge>
            <Badge
              variant={pendingApprovals.length > 0 ? "warning" : "outline"}
              className="font-mono-tech tabular-nums"
            >
              {tx(
                `${fmtNum(pendingApprovals.length, 0)} 待批`,
                `${fmtNum(pendingApprovals.length, 0)} Pending`
              )}
            </Badge>
          </div>
        </div>

        {totalActionable === 0 ? (
          <EmptyState
            title={tx("当前无未关闭告警或待批动作", "No active alerts or pending approvals")}
          />
        ) : (
          <div className="space-y-2">
            {activeAlerts.slice(0, 5).map((alert) => (
              <div
                key={`alert-${alert.id}`}
                data-testid={`pulse-alert-item-${alert.id}`}
                className="flex items-center justify-between gap-2.5 rounded-md border border-border bg-background/60 px-3 py-2"
              >
                <div className="min-w-0 flex-1 space-y-0.5">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <StatusBadge status={alert.severity} />
                    <span
                      title={alert.title}
                      className="text-xs font-medium text-foreground truncate"
                    >
                      {stripSeverityTag(alert.title)}
                    </span>
                  </div>
                  <div className="flex items-center gap-2 flex-wrap text-[11px] font-mono-tech text-muted-foreground">
                    {alert.subgraph_slug && <span>{alert.subgraph_slug}</span>}
                    {alert.last_value !== null && (
                      <span>
                        {tx("当前值", "Val")}: {fmtNum(alert.last_value, 2)}
                      </span>
                    )}
                    <span>×{fmtNum(alert.occurrences, 0)}</span>
                    <span>{fmtTime(alert.last_seen_at, true)}</span>
                  </div>
                </div>

                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  data-testid={`pulse-alert-handle-${alert.id}`}
                  onClick={() =>
                    void navigate(
                      `/flow?tab=alerts&alertId=${encodeURIComponent(alert.id)}`
                    )
                  }
                >
                  <AlertTriangle className="h-3 w-3 text-amber-600 dark:text-amber-400" />
                  {tx("处置", "Handle")}
                </Button>
              </div>
            ))}

            {pendingApprovals.slice(0, 5).map((appr) => (
              <div
                key={`appr-${appr.id}`}
                data-testid={`pulse-approval-item-${appr.id}`}
                className="flex items-center justify-between gap-2.5 rounded-md border border-border bg-background/60 px-3 py-2"
              >
                <div className="min-w-0 flex-1 space-y-0.5">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <Badge variant="warning">{tx("待批", "Pending")}</Badge>
                    <span
                      title={appr.title}
                      className="text-xs font-medium text-foreground truncate"
                    >
                      {appr.title}
                    </span>
                  </div>
                  <div className="flex items-center gap-2 flex-wrap text-[11px] font-mono-tech text-muted-foreground min-w-0">
                    <span>{tEnum(appr.source_kind)}</span>
                    {appr.source_ref && (
                      <span title={appr.source_ref} className="truncate min-w-0">
                        {appr.source_ref}
                      </span>
                    )}
                    <span>{fmtTime(appr.created_at, true)}</span>
                  </div>
                </div>

                <Button
                  type="button"
                  variant="primary"
                  size="sm"
                  data-testid={`pulse-approval-review-${appr.id}`}
                  onClick={() => void navigate("/flow")}
                >
                  <CheckSquare className="h-3 w-3" />
                  {tx("审批", "Review")}
                </Button>
              </div>
            ))}
          </div>
        )}
      </Card>

      {/* Right: Recent Simulations & Deadline Step Redlines */}
      <Card
        className="p-4 flex flex-col gap-3"
        data-testid="dashboard-simulation-feed"
      >
        <div className="flex items-center justify-between gap-2 border-b border-border pb-2.5">
          <div className="flex items-center gap-2 min-w-0">
            <FlaskConical className="h-4 w-4 text-xblue shrink-0" />
            <h2 className="text-sm font-semibold truncate">
              {tx("近期推演沙盘与红线预警", "Recent Simulations & Redlines")}
            </h2>
          </div>
          <Badge variant="outline" className="font-mono-tech tabular-nums">
            {tx(
              `${fmtNum(recentSimulations.length, 0)} 方案`,
              `${fmtNum(recentSimulations.length, 0)} Scenarios`
            )}
          </Badge>
        </div>

        {recentSimulations.length === 0 ? (
          <EmptyState
            title={tx("暂无推演场景记录", "No simulation scenarios yet")}
            action={
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void navigate("/sim")}
              >
                {tx("推演", "Simulate")}
              </Button>
            }
          />
        ) : (
          <div className="space-y-2">
            {recentSimulations.slice(0, 5).map((sim) => {
              const simHref = `/sim?sandbox=${encodeURIComponent(
                sim.sandbox_id
              )}&scenario=${encodeURIComponent(sim.scenario_id)}`;

              return (
                <div
                  key={sim.scenario_id}
                  data-testid={`pulse-sim-item-${sim.scenario_id}`}
                  className="flex items-center justify-between gap-2.5 rounded-md border border-border bg-background/60 px-3 py-2"
                >
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="flex items-center gap-1.5 flex-wrap min-w-0">
                      <StatusBadge status={sim.status} />
                      <span
                        title={sim.name}
                        className="text-xs font-medium text-foreground truncate"
                      >
                        {sim.name}
                      </span>
                      {sim.deadline_status && (
                        <Badge
                          variant={deadlineBadgeVariant(sim.deadline_status)}
                          data-testid={`pulse-sim-deadline-status-${sim.scenario_id}`}
                        >
                          {tEnum(sim.deadline_status)}
                        </Badge>
                      )}
                    </div>

                    <div className="flex items-center gap-2 flex-wrap text-[11px] font-mono-tech text-muted-foreground min-w-0">
                      <span className="shrink-0">{sim.subgraph_slug}</span>
                      <span
                        title={sim.sandbox_name}
                        className="truncate min-w-0 flex-1 max-w-[340px]"
                      >
                        {sim.sandbox_name}
                      </span>
                      <span className="shrink-0">
                        {tx(
                          `影响 ${fmtNum(sim.affected_count, 0)} 项`,
                          `${fmtNum(sim.affected_count, 0)} impacts`
                        )}
                      </span>
                      {sim.earliest_deadline_step !== null && (
                        <span
                          data-testid={`pulse-sim-deadline-step-${sim.scenario_id}`}
                          className="inline-flex items-center gap-1 text-amber-700 dark:text-amber-400 font-medium shrink-0"
                        >
                          <Clock className="h-3 w-3 shrink-0" />
                          <span>
                            {sim.earliest_deadline_step >= 0
                              ? tx(
                                  `最晚第 ${sim.earliest_deadline_step} 步干预`,
                                  `Intervene by step ${sim.earliest_deadline_step}`
                                )
                              : tx(
                                  `越线步 ${sim.earliest_breach_step ?? 0}`,
                                  `Breach step ${sim.earliest_breach_step ?? 0}`
                                )}
                          </span>
                        </span>
                      )}
                    </div>
                  </div>

                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-testid={`pulse-sim-open-${sim.scenario_id}`}
                    onClick={() => void navigate(simHref)}
                  >
                    <FlaskConical className="h-3 w-3 text-xblue" />
                    {tx("推演", "Simulate")}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </Card>
    </div>
  );
}
