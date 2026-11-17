import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import {
  Activity,
  CheckCircle2,
  Clock,
  ExternalLink,
  GitBranch,
  Send,
  Share2,
  X,
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
import { Badge, Button, Input } from "@/components/ui/primitives";
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
  onClose: () => void;
  onAck: (alertId: string, comment: string) => Promise<void>;
  onResolve: (alertId: string, comment: string) => Promise<void>;
}

function formatFullTime(iso: string | null): string {
  if (!iso) return "—";
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return parsed.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
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

function getEventLabel(kind: AlertEventKind): string {
  switch (kind) {
    case "OPENED":
      return "告警触发 (OPENED)";
    case "RETRIGGERED":
      return "重复触发合并 (RETRIGGERED)";
    case "ACKED":
      return "人工确认 (ACKED)";
    case "ESCALATED":
      return "超时升级 (ESCALATED)";
    case "RESOLVED":
      return "告警恢复 (RESOLVED)";
    case "NOTIFIED":
      return "通知送达 (NOTIFIED)";
    case "NOTIFY_FAILED":
      return "通知失败 (NOTIFY_FAILED)";
    case "SILENCED":
      return "静默抑制 (SILENCED)";
    case "FLOW_STARTED":
      return "触发联动流程 (FLOW_STARTED)";
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
        暂无关联指标时序采样点
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
        aria-label="指标趋势与阈值线"
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
              阈值 {threshold}
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
              触发值 {Number(lastPoint.value.toFixed(4))}
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
  onClose,
  onAck,
  onResolve,
}: AlertDetailProps) {
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

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/35 backdrop-blur-[1px]"
      data-testid="alert-detail-backdrop"
      onClick={onClose}
    >
      <div
        className="h-full w-full max-w-xl bg-background border-l border-border shadow-xl flex flex-col overflow-hidden"
        data-testid="alert-detail-drawer"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Drawer Header */}
        <div className="flex items-start justify-between gap-3 border-b border-border p-4 bg-muted/20">
          <div className="space-y-1.5 min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <span
                className={cn(
                  "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium",
                  getSeverityBadgeClass(currentAlert.severity)
                )}
              >
                {currentAlert.severity}
              </span>
              <span
                data-testid="alert-detail-status"
                className={cn(
                  "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-semibold",
                  getStatusBadgeClass(currentAlert.status)
                )}
              >
                {currentAlert.status}
              </span>
              <span className="text-[11px] font-mono-tech text-muted-foreground truncate">
                {currentAlert.dedup_key}
              </span>
            </div>
            <h2 className="text-sm font-semibold text-foreground leading-snug">
              {currentAlert.title}
            </h2>
            {currentAlert.body && (
              <p className="text-xs text-muted-foreground leading-relaxed">
                {currentAlert.body}
              </p>
            )}
          </div>

          <Button
            type="button"
            variant="ghost"
            className="h-8 w-8 p-0 shrink-0"
            data-testid="close-alert-detail-btn"
            onClick={onClose}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>

        {/* Scrollable Content */}
        <div className="flex-1 overflow-y-auto p-4 space-y-5">
          {/* Deep Links & Metadata */}
          <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5 text-xs">
            <div className="grid grid-cols-2 gap-2">
              <div>
                <span className="text-muted-foreground">归属规则：</span>
                <span className="font-medium text-foreground">
                  {rule ? rule.name : currentAlert.rule_id}
                </span>
              </div>
              <div>
                <span className="text-muted-foreground">累计触发：</span>
                <span className="font-mono-tech font-medium text-foreground">
                  {currentAlert.occurrences} 次
                </span>
              </div>
              <div>
                <span className="text-muted-foreground">首次触发：</span>
                <span className="font-mono-tech text-[11px]">
                  {formatFullTime(currentAlert.first_seen_at)}
                </span>
              </div>
              <div>
                <span className="text-muted-foreground">最近触发：</span>
                <span className="font-mono-tech text-[11px]">
                  {formatFullTime(currentAlert.last_seen_at)}
                </span>
              </div>
              {currentAlert.acked_at && (
                <div>
                  <span className="text-muted-foreground">确认时间：</span>
                  <span className="font-mono-tech text-[11px]">
                    {formatFullTime(currentAlert.acked_at)}
                  </span>
                </div>
              )}
              {currentAlert.resolved_at && (
                <div>
                  <span className="text-muted-foreground">恢复原因：</span>
                  <span className="font-mono-tech text-[11px]">
                    {currentAlert.resolve_reason ?? "MANUAL"} (
                    {formatFullTime(currentAlert.resolved_at)})
                  </span>
                </div>
              )}
            </div>

            {rule && (
              <div className="pt-1 border-t border-border/70 text-[11px] text-muted-foreground">
                判定条件：{" "}
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
                    跳转拓扑节点 (/graph?node={currentAlert.node_id.slice(0, 8)})
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
                    查看联动流程运行 (/flow · {flowRunId.slice(0, 8)})
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
                  指标走势与触发阈值
                  {metricName && (
                    <Badge variant="outline">{metricName}</Badge>
                  )}
                </span>
                {currentAlert.last_value !== null && (
                  <span className="font-mono-tech text-[11px] text-muted-foreground">
                    最新值: {Number(currentAlert.last_value.toFixed(4))}
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
              <span>通知投递记录 (ax_notify_deliveries)</span>
              <Badge variant="outline">{currentAlert.deliveries.length}</Badge>
            </div>

            {currentAlert.deliveries.length === 0 ? (
              <div className="rounded-md border border-dashed border-border p-3 text-center text-[11px] text-muted-foreground">
                暂无外部渠道投递记录
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
                        尝试 #{del.attempt}
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
              <span>状态机事件时间线 (ax_alert_events)</span>
              <Badge variant="outline">{events.length}</Badge>
            </div>

            {eventsQuery.isLoading ? (
              <div className="text-xs text-muted-foreground py-4 text-center">
                加载时间线事件中...
              </div>
            ) : events.length === 0 ? (
              <div className="rounded-md border border-dashed border-border p-3 text-center text-[11px] text-muted-foreground">
                暂无告警事件流水
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
                          {formatFullTime(ev.created_at)}
                        </span>
                      </div>
                      {commentText && (
                        <div className="mt-1 rounded bg-muted px-2 py-1 text-[11px] text-foreground">
                          备注: {commentText}
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
        </div>

        {/* Drawer Action Footer */}
        {currentAlert.status !== "RESOLVED" && (
          <div className="border-t border-border bg-muted/20 p-3.5 space-y-2.5">
            <Input
              data-testid="alert-detail-comment-input"
              placeholder="处置备注（写入 ax_alert_events.detail）..."
              value={comment}
              onChange={(e) => setComment(e.target.value)}
              className="h-8 text-xs"
            />
            <div className="flex items-center justify-end gap-2">
              {currentAlert.status === "OPEN" && (
                <Button
                  type="button"
                  variant="outline"
                  className="h-8 border-xblue/40 text-xblue hover:bg-xblue-soft"
                  data-testid="alert-detail-ack-btn"
                  disabled={acting}
                  onClick={() => {
                    void onAck(currentAlert.id, comment);
                    setComment("");
                  }}
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  确认告警 (ACK)
                </Button>
              )}
              <Button
                type="button"
                variant="primary"
                className="h-8"
                data-testid="alert-detail-resolve-btn"
                disabled={acting}
                onClick={() => {
                  void onResolve(currentAlert.id, comment);
                  setComment("");
                }}
              >
                手动恢复 (RESOLVE)
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
