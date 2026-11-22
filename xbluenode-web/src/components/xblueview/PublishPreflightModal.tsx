import {
  AlertTriangle,
  CheckCircle2,
  Crosshair,
  Loader2,
  ShieldCheck,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type {
  DashboardPreflightReport,
  DashboardPreflightStatus,
  SemanticZoneKind,
} from "@/lib/api/xblueview";
import { Badge, Button, Modal } from "@/components/ui/primitives";

export interface PublishPreflightModalProps {
  open: boolean;
  dashboardName: string;
  loading: boolean;
  confirming: boolean;
  error: string | null;
  report: DashboardPreflightReport | null;
  onClose: () => void;
  onLocateWidget: (widgetId: string) => void;
  onConfirmPublish: () => void;
}

export function formatSemanticZoneLabel(
  zone: SemanticZoneKind,
  isZh: boolean
): string {
  switch (zone) {
    case "CONTROL":
      return isZh ? "全局控制" : "Global Control";
    case "KPI":
      return isZh ? "核心指标" : "Core KPIs";
    case "SCENE":
      return isZh ? "孪生态势" : "Twin Scene";
    case "INSIGHT":
      return isZh ? "归因洞察" : "Attribution Insight";
    case "DECISION":
      return isZh ? "协同决策" : "Decision";
  }
}

export function PublishPreflightModal({
  open,
  dashboardName,
  loading,
  confirming,
  error,
  report,
  onClose,
  onLocateWidget,
  onConfirmPublish,
}: PublishPreflightModalProps) {
  const { isZh, tx, fmtNum } = useI18n();

  const renderStatusBadge = (status: DashboardPreflightStatus) => {
    if (status === "PASS") {
      return (
        <Badge variant="success" data-testid="preflight-status-badge">
          {tx("通过", "PASS")}
        </Badge>
      );
    }
    if (status === "WARN") {
      return (
        <Badge variant="warning" data-testid="preflight-status-badge">
          {tx("提示", "WARN")}
        </Badge>
      );
    }
    return (
      <Badge variant="danger" data-testid="preflight-status-badge">
        {tx("阻断", "BLOCKED")}
      </Badge>
    );
  };

  const canConfirmPublish =
    report !== null && !loading && !confirming && report.blocking_count === 0;

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="publish-preflight-modal"
      maxWidthClass="max-w-xl"
      title={
        <>
          <ShieldCheck className="h-4 w-4 text-xblue shrink-0" />
          <span className="truncate">
            {tx("发布自检", "Publish Preflight")} · {dashboardName}
          </span>
        </>
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={confirming}
          >
            {tx("取消", "Cancel")}
          </Button>
          <Button
            type="button"
            disabled={!canConfirmPublish}
            onClick={onConfirmPublish}
            data-testid="preflight-confirm-publish-btn"
          >
            {confirming ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <CheckCircle2 className="h-3.5 w-3.5" />
            )}
            <span>{tx("确认发布", "Publish")}</span>
          </Button>
        </>
      }
    >
      {loading ? (
        <div className="flex items-center justify-center gap-2 py-8 text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin text-xblue" />
          <span>{tx("自检中…", "Checking…")}</span>
        </div>
      ) : error ? (
        <div className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-red-700 dark:text-red-300">
          {error}
        </div>
      ) : report ? (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/25 px-3 py-2">
            <div className="flex flex-wrap items-center gap-3 text-xs">
              <span>
                {tx("检查组件", "Checked")}{" "}
                <strong
                  className="font-mono-tech tabular-nums"
                  data-testid="preflight-checked-count"
                >
                  {fmtNum(report.checked_widgets, 0)}
                </strong>
              </span>
              <span className="text-border">·</span>
              <span>
                {tx("阻断", "Blocking")}{" "}
                <strong className="font-mono-tech tabular-nums text-red-600 dark:text-red-400">
                  {fmtNum(report.blocking_count, 0)}
                </strong>
              </span>
              <span className="text-border">·</span>
              <span>
                {tx("提示", "Warnings")}{" "}
                <strong className="font-mono-tech tabular-nums text-amber-600 dark:text-amber-400">
                  {fmtNum(report.warning_count, 0)}
                </strong>
              </span>
            </div>
            {renderStatusBadge(report.status)}
          </div>

          {report.issues.length === 0 ? (
            <div className="rounded-md border border-emerald-500/30 bg-emerald-500/5 px-3 py-4 text-center text-emerald-700 dark:text-emerald-400">
              {tx("全部组件依赖与权限校验通过", "All widgets passed preflight")}
            </div>
          ) : (
            <div className="space-y-2 max-h-72 overflow-y-auto pr-0.5">
              {report.issues.map((issue, idx) => {
                const isBlocking = issue.severity === "BLOCKING";
                return (
                  <div
                    key={`${issue.widget_id ?? "global"}-${issue.code}-${idx}`}
                    data-testid={`preflight-issue-row-${idx}`}
                    className="flex items-start justify-between gap-2.5 rounded-md border border-border bg-background p-2.5"
                  >
                    <div className="min-w-0 flex-1 space-y-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Badge variant={isBlocking ? "danger" : "warning"}>
                          {isBlocking
                            ? tx("阻断", "BLOCKED")
                            : tx("提示", "WARN")}
                        </Badge>
                        {issue.semantic_zone && (
                          <Badge variant="outline">
                            {formatSemanticZoneLabel(issue.semantic_zone, isZh)}
                          </Badge>
                        )}
                        <span className="font-medium text-foreground truncate">
                          {issue.widget_title}
                        </span>
                        <Badge variant="neutral">{issue.code}</Badge>
                      </div>
                      <div className="flex items-start gap-1.5 text-muted-foreground leading-relaxed">
                        <AlertTriangle className="h-3.5 w-3.5 shrink-0 mt-0.5 text-amber-500" />
                        <span>{issue.message}</span>
                      </div>
                    </div>

                    {issue.widget_id && (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          if (issue.widget_id) {
                            onLocateWidget(issue.widget_id);
                          }
                        }}
                        data-testid="preflight-locate-btn"
                      >
                        <Crosshair className="h-3 w-3 text-xblue" />
                        {tx("定位", "Locate")}
                      </Button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      ) : null}
    </Modal>
  );
}
