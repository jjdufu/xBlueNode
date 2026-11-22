import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import {
  Activity,
  CheckCircle2,
  Clock,
  ExternalLink,
  GitBranch,
  Kanban,
  Send,
  Share2,
} from "lucide-react";
import {
  extractConditionThreshold,
  formatConditionSummary,
  getAlert,
  listAlertEvents,
  xblueApi,
  type AlertEventKind,
  type AlertEventRecord,
  type AlertRecord,
  type MetricSeriesPoint,
  type NotifyChannelRecord,
  type NotifyDeliveryStatus,
  type RuleDef,
} from "@/lib/api";
import { Badge, Button, Drawer, Input } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import {
  getSeverityBadgeClass,
  getStatusBadgeClass,
} from "./AlertTable";

export interface AlertDetailProps {
  alert: AlertRecord;
  rule: RuleDef | null;
  channels: readonly NotifyChannelRecord[];
  acting: boolean;
  canAck: boolean;
  convertingWorkItem?: boolean;
  createdWorkItemCode?: string | null;
  onClose: () => void;
  onAck: (alertId: string, comment: string) => Promise<void>;
  onResolve: (alertId: string, comment: string) => Promise<void>;
  onConvertWorkItem?: (alert: AlertRecord) => Promise<void>;
}

function getDeliveryBadgeClass(status: NotifyDeliveryStatus): string {
  switch (status) {
    case "SENT":
      return "border border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400";
    case "FAILED":
      return "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400";
    case "PENDING":
      return "border border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400";
  }
}

function MetricTrendSparkline({
  points,
  threshold,
  triggerValue,
}: {
  points: readonly MetricSeriesPoint[];
  threshold: number | null;
  triggerValue: number | null;
}) {
  const { tx, fmtNum } = useI18n();
  const numericPoints = useMemo(() => {
    const valid = points
      .filter(
        (p): p is { ts: string; value: number } =>
          typeof p.value === "number" && Number.isFinite(p.value)
      )
      .slice(-24);
    if (valid.length > 0) return valid;
    if (triggerValue !== null && Number.isFinite(triggerValue)) {
      return [{ ts: new Date().toISOString(), value: triggerValue }];
    }
    return [];
  }, [points, triggerValue]);

  if (numericPoints.length === 0) {
    return (
      <div className="text-[11px] text-muted-foreground py-3 text-center border border-dashed border-border rounded-md">
        {tx("暂无时序采样点", "No time-series points")}
      </div>
    );
  }

  const width = 420;
  const height = 110;
  const padX = 16;
  const padY = 16;

  const allValues = numericPoints.map((p) => p.value);
  if (threshold !== null && Number.isFinite(threshold)) {
    allValues.push(threshold);
  }
  const minVal = Math.min(...allValues);
  const maxVal = Math.max(...allValues);
  const span = maxVal - minVal > 1e-9 ? maxVal - minVal : 1;

  const toX = (idx: number): number => {
    if (numericPoints.length === 1) return width / 2;
    return padX + (idx / (numericPoints.length - 1)) * (width - padX * 2);
  };
  const toY = (val: number): number =>
    height - padY - ((val - minVal) / span) * (height - padY * 2);

  const polylinePoints = numericPoints
    .map((pt, idx) => `${toX(idx).toFixed(1)},${toY(pt.value).toFixed(1)}`)
    .join(" ");

  const lastIdx = numericPoints.length - 1;
  const lastPoint = numericPoints[lastIdx];
  const thresholdY =
    threshold !== null && Number.isFinite(threshold) ? toY(threshold) : null;

  return (
    <div
      className="rounded-md border border-border bg-muted/20 p-2.5"
      data-testid="alert-metric-sparkline"
    >
      <svg
        viewBox={`0 0 ${width} ${height}`}
        className="w-full h-28 overflow-visible"
        role="img"
        aria-label={tx("指标趋势与阈值线", "Metric trend and threshold")}
      >
        {thresholdY !== null && (
          <g>
            <line
              x1={padX}
              y1={thresholdY}
              x2={width - padX}
              y2={thresholdY}
              stroke="#ef4444"
              strokeWidth="1.25"
              strokeDasharray="4 3"
            />
            <text
              x={width - padX}
              y={Math.max(11, thresholdY - 4)}
              textAnchor="end"
              className="fill-red-600 dark:fill-red-400 text-[10px] font-mono-tech"
            >
              {tx("阈值", "Threshold")} {fmtNum(threshold, 4)}
            </text>
          </g>
        )}

        {numericPoints.length > 1 && (
          <polyline
            fill="none"
            stroke="#2563eb"
            strokeWidth="2"
            points={polylinePoints}
          />
        )}

        {lastPoint && (
          <g>
            <circle
              cx={toX(lastIdx)}
              cy={toY(lastPoint.value)}
              r="4"
              fill="#2563eb"
            />
            <text
              x={Math.min(width - padX, toX(lastIdx) + 6)}
              y={Math.max(12, toY(lastPoint.value) - 6)}
              className="fill-foreground text-[10px] font-mono-tech"
            >
              {tx("触发值", "Value")} {fmtNum(lastPoint.value, 4)}
            </text>
          </g>
        )}
      </svg>
    </div>
  );
}

export function AlertDetail({
  alert,
  rule,
  channels,
  acting,
  canAck,
  convertingWorkItem = false,
  createdWorkItemCode = null,
  onClose,
  onAck,
  onResolve,
  onConvertWorkItem,
}: AlertDetailProps) {
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();
  const [comment, setComment] = useState("");

  const detailQuery = useQuery({
    queryKey: ["axalerts", "alert-detail", alert.id, alert.status, alert.occurrences],
    queryFn: () => getAlert(alert.id),
  });

  const eventsQuery = useQuery({
    queryKey: ["axalerts", "alert-events", alert.id, alert.status, alert.occurrences],
    queryFn: () => listAlertEvents(alert.id, { limit: 100 }),
  });

  const currentAlert = detailQuery.data ?? alert;
  const events: AlertEventRecord[] = eventsQuery.data?.items ?? [];

  const getEventLabel = (kind: AlertEventKind): string => {
    switch (kind) {
      case "OPENED":
        return tx("告警触发", "Alert Opened");
      case "RETRIGGERED":
        return tx("重复触发合并", "Retriggered");
      case "ACKED":
        return tx("人工确认", "Acknowledged");
      case "ESCALATED":
        return tx("超时升级", "Escalated");
      case "RESOLVED":
        return tx("告警恢复", "Resolved");
      case "NOTIFIED":
        return tx("通知送达", "Notified");
      case "NOTIFY_FAILED":
        return tx("通知失败", "Notification Failed");
      case "SILENCED":
        return tx("静默抑制", "Silenced");
      case "FLOW_STARTED":
        return tx("触发联动流程", "Flow Triggered");
    }
  };

  const metricName = useMemo(() => {
    if (typeof currentAlert.detail.metric === "string" && currentAlert.detail.metric) {
      return currentAlert.detail.metric;
    }
    if (rule && typeof rule.target.metric === "string" && rule.target.metric) {
      return rule.target.metric;
    }
    return null;
  }, [currentAlert.detail, rule]);

  const seriesQuery = useQuery({
    queryKey: ["axalerts", "alert-series", currentAlert.node_id, metricName],
    enabled: Boolean(currentAlert.node_id && metricName),
    queryFn: async () => {
      if (!currentAlert.node_id || !metricName) return [];
      const now = new Date();
      const from = new Date(now.getTime() - 6 * 3600 * 1000).toISOString();
      try {
        const res = await xblueApi.queryNodeMetrics(
          currentAlert.node_id,
          metricName,
          {
            from,
            to: now.toISOString(),
            grain: "RAW",
          }
        );
        return Array.isArray(res) ? res : [];
      } catch {
        return [];
      }
    },
  });

  const channelMap = useMemo(() => {
    const map = new Map<string, NotifyChannelRecord>();
    for (const ch of channels) {
      map.set(ch.id, ch);
    }
    return map;
  }, [channels]);

  const flowRunId = useMemo(() => {
    if (
      typeof currentAlert.detail.flow_run_id === "string" &&
      currentAlert.detail.flow_run_id.length > 0
    ) {
      return currentAlert.detail.flow_run_id;
    }
    for (const ev of events) {
      if (ev.event === "FLOW_STARTED") {
        if (typeof ev.detail.flow_run_id === "string" && ev.detail.flow_run_id) {
          return ev.detail.flow_run_id;
        }
        if (typeof ev.detail.run_id === "string" && ev.detail.run_id) {
          return ev.detail.run_id;
        }
      }
    }
    return null;
  }, [currentAlert.detail, events]);

  const thresholdNumber = rule ? extractConditionThreshold(rule.condition) : null;

  const footerContent =
    canAck && currentAlert.status !== "RESOLVED" ? (
      <div className="w-full space-y-2.5">
        <Input
          data-testid="alert-detail-comment-input"
          placeholder={tx("处置备注", "Resolution note")}
          value={comment}
          onChange={(e) => setComment(e.target.value)}
          className="h-8 text-xs"
        />
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            {createdWorkItemCode && (
              <Link
                to="/ops"
                data-testid="alert-detail-workitem-link"
                className="inline-flex items-center gap-1 rounded-md border border-xblue/40 bg-xblue-soft px-2.5 py-1 text-[11px] font-mono-tech font-semibold text-xblue"
              >
                {createdWorkItemCode}
                <ExternalLink className="h-3 w-3" />
              </Link>
            )}
            {onConvertWorkItem && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="border-xblue/40 text-xblue hover:bg-xblue-soft"
                data-testid="alert-detail-convert-workitem-btn"
                disabled={convertingWorkItem}
                onClick={() => void onConvertWorkItem(currentAlert)}
              >
                <Kanban className="h-3.5 w-3.5" />
                {convertingWorkItem
                  ? tx("创建中…", "Creating…")
                  : tx("转工作项", "Convert to Work Item")}
              </Button>
            )}
          </div>
          <div className="flex items-center gap-2">
            {currentAlert.status === "OPEN" && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="border-xblue/40 text-xblue hover:bg-xblue-soft"
                data-testid="alert-detail-ack-btn"
                disabled={acting}
                onClick={() => {
                  void onAck(currentAlert.id, comment);
                  setComment("");
                }}
              >
                <CheckCircle2 className="h-3.5 w-3.5" />
                {tx("确认", "Acknowledge")}
              </Button>
            )}
            <Button
              type="button"
              variant="primary"
              size="sm"
              data-testid="alert-detail-resolve-btn"
              disabled={acting}
              onClick={() => {
                void onResolve(currentAlert.id, comment);
                setComment("");
              }}
            >
              {tx("手动恢复", "Resolve")}
            </Button>
          </div>
        </div>
      </div>
    ) : undefined;

  return (
    <Drawer
      open={true}
      onClose={onClose}
      widthClass="max-w-xl"
      testId="alert-detail-drawer"
      closeTestId="close-alert-detail-btn"
      title={
        <div className="flex items-center gap-2 min-w-0">
          <span
            className={cn(
              "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium shrink-0",
              getSeverityBadgeClass(currentAlert.severity)
            )}
          >
            {tEnum(currentAlert.severity)}
          </span>
          <span
            data-testid="alert-detail-status"
            title={tEnum(currentAlert.status)}
            className={cn(
              "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-semibold shrink-0",
              getStatusBadgeClass(currentAlert.status)
            )}
          >
            {currentAlert.status}
          </span>
          <span className="truncate">{currentAlert.title}</span>
        </div>
      }
      footer={footerContent}
    >
      {currentAlert.body && (
        <p className="text-xs text-muted-foreground leading-relaxed">
          {currentAlert.body}
        </p>
      )}

      {/* Deep Links & Metadata */}
      <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5 text-xs">
        <div className="grid grid-cols-2 gap-2">
          <div>
            <span className="text-muted-foreground">
              {tx("归属规则：", "Rule: ")}
            </span>
            <span className="font-medium text-foreground">
              {rule ? rule.name : currentAlert.rule_id}
            </span>
          </div>
          <div>
            <span className="text-muted-foreground">
              {tx("累计触发：", "Occurrences: ")}
            </span>
            <span className="font-mono-tech font-medium text-foreground">
              {fmtNum(currentAlert.occurrences, 0)}
            </span>
          </div>
          <div>
            <span className="text-muted-foreground">
              {tx("首次触发：", "First Seen: ")}
            </span>
            <span className="font-mono-tech text-[11px]">
              {fmtTime(currentAlert.first_seen_at)}
            </span>
          </div>
          <div>
            <span className="text-muted-foreground">
              {tx("最近触发：", "Last Seen: ")}
            </span>
            <span className="font-mono-tech text-[11px]">
              {fmtTime(currentAlert.last_seen_at)}
            </span>
          </div>
          {currentAlert.acked_at && (
            <div>
              <span className="text-muted-foreground">
                {tx("确认时间：", "Acknowledged: ")}
              </span>
              <span className="font-mono-tech text-[11px]">
                {fmtTime(currentAlert.acked_at)}
              </span>
            </div>
          )}
          {currentAlert.resolved_at && (
            <div>
              <span className="text-muted-foreground">
                {tx("恢复原因：", "Resolved: ")}
              </span>
              <span className="font-mono-tech text-[11px]">
                {currentAlert.resolve_reason ?? "MANUAL"} (
                {fmtTime(currentAlert.resolved_at)})
              </span>
            </div>
          )}
        </div>

        {rule && (
          <div className="pt-1 border-t border-border/70 text-[11px] text-muted-foreground">
            {tx("判定条件：", "Condition: ")}{" "}
            <span className="font-mono-tech text-foreground">
              {formatConditionSummary(rule.condition)}
            </span>
          </div>
        )}

        {/* Deep Links */}
        {(currentAlert.node_id || flowRunId) && (
          <div className="flex flex-wrap items-center gap-2 pt-2 border-t border-border">
            {currentAlert.node_id && (
              <Link
                to={`/graph?node=${encodeURIComponent(currentAlert.node_id)}`}
                data-testid="alert-deep-link-graph"
                className="inline-flex items-center gap-1.5 rounded-md border border-xblue/35 bg-xblue-soft px-2.5 py-1 text-[11px] font-medium text-xblue hover:opacity-90"
              >
                <Share2 className="h-3 w-3" />
                {tx("查看拓扑节点", "View Topology Node")} ({currentAlert.node_id.slice(0, 8)})
                <ExternalLink className="h-3 w-3" />
              </Link>
            )}

            {flowRunId && (
              <Link
                to="/flow"
                data-testid="alert-deep-link-flow"
                className="inline-flex items-center gap-1.5 rounded-md border border-border bg-background px-2.5 py-1 text-[11px] font-medium text-foreground hover:bg-muted"
              >
                <GitBranch className="h-3 w-3 text-xblue" />
                {tx("查看联动流程", "View Linked Flow")} ({flowRunId.slice(0, 8)})
                <ExternalLink className="h-3 w-3" />
              </Link>
            )}
          </div>
        )}
      </div>

      {/* Metric Trend & Threshold Section */}
      {(currentAlert.node_id || currentAlert.last_value !== null) && (
        <div className="space-y-2">
          <div className="flex items-center justify-between text-xs font-medium">
            <span className="inline-flex items-center gap-1.5">
              <Activity className="h-3.5 w-3.5 text-xblue" />
              {tx("指标趋势与触发阈值", "Metric Trend & Threshold")}
              {metricName && <Badge variant="outline">{metricName}</Badge>}
            </span>
            {currentAlert.last_value !== null && (
              <span className="font-mono-tech text-[11px] text-muted-foreground">
                {tx("最新值", "Latest")}: {fmtNum(currentAlert.last_value, 4)}
              </span>
            )}
          </div>
          <MetricTrendSparkline
            points={seriesQuery.data ?? []}
            threshold={thresholdNumber}
            triggerValue={currentAlert.last_value}
          />
        </div>
      )}

      {/* Notification Deliveries */}
      <div className="space-y-2" data-testid="alert-detail-deliveries">
        <div className="flex items-center gap-1.5 text-xs font-medium">
          <Send className="h-3.5 w-3.5 text-xblue" />
          <span>{tx("通知投递记录", "Notification Deliveries")}</span>
          <Badge variant="outline">{currentAlert.deliveries.length}</Badge>
        </div>

        {currentAlert.deliveries.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-center text-[11px] text-muted-foreground">
            {tx("暂无投递记录", "No delivery records")}
          </div>
        ) : (
          <div className="space-y-1.5">
            {currentAlert.deliveries.map((del) => {
              const ch = channelMap.get(del.channel_id);
              return (
                <div
                  key={del.id}
                  data-testid={`alert-delivery-${del.id}`}
                  className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-card px-3 py-2 text-xs"
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge variant="neutral">
                      {ch ? `${ch.name} (${ch.kind})` : del.channel_id.slice(0, 8)}
                    </Badge>
                    <Badge variant="outline">{del.event}</Badge>
                    <span
                      className={cn(
                        "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium",
                        getDeliveryBadgeClass(del.status)
                      )}
                    >
                      {del.status}
                    </span>
                  </div>
                  <div className="text-[11px] font-mono-tech text-muted-foreground">
                    #{del.attempt}
                    {del.response_code !== null && ` · HTTP ${del.response_code}`}
                    {del.error_message && (
                      <span className="text-red-600 dark:text-red-400 ml-1.5">
                        {del.error_message}
                      </span>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Alert Lifecycle Timeline */}
      <div className="space-y-2" data-testid="alert-detail-timeline">
        <div className="flex items-center gap-1.5 text-xs font-medium">
          <Clock className="h-3.5 w-3.5 text-xblue" />
          <span>{tx("事件时间线", "Event Timeline")}</span>
          <Badge variant="outline">{events.length}</Badge>
        </div>

        {eventsQuery.isLoading ? (
          <div className="text-xs text-muted-foreground py-4 text-center">
            {tx("加载中…", "Loading…")}
          </div>
        ) : events.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-center text-[11px] text-muted-foreground">
            {tx("暂无事件记录", "No events")}
          </div>
        ) : (
          <ol className="relative border-l border-border ml-2 space-y-3 pl-4 py-1">
            {events.map((ev) => {
              const commentText =
                typeof ev.detail.comment === "string" && ev.detail.comment
                  ? ev.detail.comment
                  : null;
              return (
                <li
                  key={ev.id}
                  data-testid={`alert-event-${ev.event}`}
                  className="relative text-xs"
                >
                  <span className="absolute -left-[21px] top-1.5 h-2.5 w-2.5 rounded-full border border-background bg-xblue" />
                  <div className="flex flex-wrap items-center justify-between gap-1">
                    <span className="font-medium text-foreground">
                      {getEventLabel(ev.event)}
                    </span>
                    <span className="font-mono-tech text-[11px] text-muted-foreground">
                      {fmtTime(ev.created_at)}
                    </span>
                  </div>
                  {commentText && (
                    <div className="mt-1 rounded bg-muted px-2 py-1 text-[11px] text-foreground">
                      {tx("备注", "Note")}: {commentText}
                    </div>
                  )}
                  {Object.keys(ev.detail).length > 0 && !commentText && (
                    <div className="mt-1 font-mono-tech text-[11px] text-muted-foreground break-all">
                      {JSON.stringify(ev.detail)}
                    </div>
                  )}
                </li>
              );
            })}
          </ol>
        )}
      </div>
    </Drawer>
  );
}
