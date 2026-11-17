import { useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCheck,
  CheckCircle2,
  Eye,
  Filter,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import type {
  AlertListQuery,
  AlertRecord,
  AlertStatus,
  RuleDef,
  Severity,
} from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface AlertTableProps {
  alerts: readonly AlertRecord[];
  rules: readonly RuleDef[];
  knownTypeNames: readonly string[];
  filters: AlertListQuery;
  selectedAlertId: string | null;
  loading: boolean;
  actingAlertId: string | null;
  batchAcking: boolean;
  onFiltersChange: (next: AlertListQuery) => void;
  onSelectAlert: (alertId: string) => void;
  onAckAlert: (alertId: string) => Promise<void>;
  onResolveAlert: (alertId: string) => Promise<void>;
  onBatchAck: (alertIds: string[], comment: string) => Promise<void>;
  onRefresh: () => void;
}

function formatShortTime(iso: string): string {
  if (!iso) return "—";
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return parsed.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

export function getSeverityBadgeClass(severity: Severity): string {
  switch (severity) {
    case "CRITICAL":
      return "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400";
    case "WARN":
      return "border border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400";
    case "INFO":
      return "border border-xblue/30 bg-xblue-soft text-xblue";
  }
}

export function getStatusBadgeClass(status: AlertStatus): string {
  switch (status) {
    case "OPEN":
      return "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400";
    case "ACK":
      return "border border-xblue/35 bg-xblue-soft text-xblue";
    case "RESOLVED":
      return "border border-border bg-muted text-muted-foreground";
  }
}

export function AlertTable({
  alerts,
  rules,
  knownTypeNames,
  filters,
  selectedAlertId,
  loading,
  actingAlertId,
  batchAcking,
  onFiltersChange,
  onSelectAlert,
  onAckAlert,
  onResolveAlert,
  onBatchAck,
  onRefresh,
}: AlertTableProps) {
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [batchComment, setBatchComment] = useState("");

  const ruleNameMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const r of rules) {
      map.set(r.id, r.name);
    }
    return map;
  }, [rules]);

  const openAlertIds = useMemo(
    () => alerts.filter((a) => a.status === "OPEN").map((a) => a.id),
    [alerts]
  );

  const validSelectedIds = useMemo(() => {
    const openSet = new Set(openAlertIds);
    return selectedIds.filter((id) => openSet.has(id));
  }, [selectedIds, openAlertIds]);

  const allOpenSelected =
    openAlertIds.length > 0 && validSelectedIds.length === openAlertIds.length;

  const toggleSelectAll = () => {
    if (allOpenSelected) {
      setSelectedIds([]);
    } else {
      setSelectedIds([...openAlertIds]);
    }
  };

  const toggleSelectOne = (id: string) => {
    setSelectedIds((prev) =>
      prev.includes(id) ? prev.filter((item) => item !== id) : [...prev, id]
    );
  };

  const handleBatchSubmit = async () => {
    if (validSelectedIds.length === 0) return;
    await onBatchAck(validSelectedIds, batchComment);
    setSelectedIds([]);
    setBatchComment("");
  };

  const hasFilters = Boolean(
    filters.status || filters.severity || filters.rule_id || filters.type_name
  );

  return (
    <Card className="overflow-hidden" data-testid="alert-table-card">
      {/* Filter Toolbar */}
      <div className="border-b border-border bg-muted/30 p-3.5 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2 text-xs font-medium text-foreground">
            <Filter className="h-3.5 w-3.5 text-xblue" />
            <span>运营告警队列</span>
            <Badge variant="outline">{alerts.length} 条记录</Badge>
          </div>
          <div className="flex items-center gap-2">
            {hasFilters && (
              <Button
                type="button"
                variant="ghost"
                className="h-8 px-2.5 text-xs"
                data-testid="alert-filter-reset-btn"
                onClick={() => onFiltersChange({})}
              >
                <RotateCcw className="h-3 w-3" />
                重置筛选
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              className="h-8 px-2.5 text-xs"
              data-testid="alert-refresh-btn"
              onClick={onRefresh}
              disabled={loading}
            >
              <RefreshCw className={cn("h-3 w-3", loading && "animate-spin")} />
              刷新 (15s 轮询)
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2.5">
          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              生命周期状态 (Status)
            </label>
            <select
              data-testid="alert-filter-status"
              value={filters.status ?? ""}
              onChange={(e) => {
                const val = e.target.value;
                onFiltersChange({
                  ...filters,
                  status:
                    val === "OPEN" || val === "ACK" || val === "RESOLVED"
                      ? val
                      : undefined,
                });
              }}
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-foreground"
            >
              <option value="">全部状态 (ALL)</option>
              <option value="OPEN">OPEN · 待确认</option>
              <option value="ACK">ACK · 已确认处理中</option>
              <option value="RESOLVED">RESOLVED · 已恢复</option>
            </select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              严重级别 (Severity)
            </label>
            <select
              data-testid="alert-filter-severity"
              value={filters.severity ?? ""}
              onChange={(e) => {
                const val = e.target.value;
                onFiltersChange({
                  ...filters,
                  severity:
                    val === "CRITICAL" || val === "WARN" || val === "INFO"
                      ? val
                      : undefined,
                });
              }}
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-foreground"
            >
              <option value="">全部级别 (ALL)</option>
              <option value="CRITICAL">CRITICAL · 紧急</option>
              <option value="WARN">WARN · 警告</option>
              <option value="INFO">INFO · 提示</option>
            </select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              归属规则 (Rule)
            </label>
            <select
              data-testid="alert-filter-rule"
              value={filters.rule_id ?? ""}
              onChange={(e) =>
                onFiltersChange({
                  ...filters,
                  rule_id: e.target.value || undefined,
                })
              }
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-foreground"
            >
              <option value="">全部规则</option>
              {rules.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.name}
                </option>
              ))}
            </select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              业务节点类型 (Type Name)
            </label>
            <Input
              list="axalerts-known-type-names"
              data-testid="alert-filter-type-name"
              placeholder="例如 Warehouse"
              value={filters.type_name ?? ""}
              onChange={(e) =>
                onFiltersChange({
                  ...filters,
                  type_name: e.target.value.trim() || undefined,
                })
              }
              className="h-8 text-xs"
            />
            <datalist id="axalerts-known-type-names">
              {knownTypeNames.map((tn) => (
                <option key={tn} value={tn} />
              ))}
            </datalist>
          </div>
        </div>

        {/* Batch ACK Bar */}
        {openAlertIds.length > 0 && (
          <div
            className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2"
            data-testid="batch-ack-bar"
          >
            <label className="inline-flex items-center gap-2 text-xs text-foreground cursor-pointer">
              <input
                type="checkbox"
                data-testid="alert-select-all"
                checked={allOpenSelected}
                onChange={toggleSelectAll}
                className="h-3.5 w-3.5 rounded border-border accent-xblue"
              />
              <span>
                全选待确认告警（已选{" "}
                <span className="font-mono-tech font-semibold text-xblue">
                  {validSelectedIds.length}
                </span>{" "}
                / {openAlertIds.length}）
              </span>
            </label>

            <div className="flex flex-wrap items-center gap-2">
              <Input
                data-testid="batch-ack-comment-input"
                placeholder="批量确认备注（可选）..."
                value={batchComment}
                onChange={(e) => setBatchComment(e.target.value)}
                className="h-8 w-52 text-xs"
              />
              <Button
                type="button"
                variant="primary"
                className="h-8 px-3 text-xs"
                data-testid="batch-ack-btn"
                disabled={validSelectedIds.length === 0 || batchAcking}
                onClick={() => {
                  void handleBatchSubmit();
                }}
              >
                <CheckCheck className="h-3.5 w-3.5" />
                {batchAcking
                  ? "批量确认中..."
                  : `批量确认 ACK (${validSelectedIds.length})`}
              </Button>
            </div>
          </div>
        )}
      </div>

      {/* Table Body */}
      <div className="overflow-x-auto">
        <table className="w-full text-left border-collapse text-xs">
          <thead>
            <tr className="border-b border-border bg-muted/40 text-[11px] text-muted-foreground font-medium">
              <th className="py-2.5 px-3 w-9">选</th>
              <th className="py-2.5 px-3">级别 / 状态</th>
              <th className="py-2.5 px-3">告警摘要与归属规则</th>
              <th className="py-2.5 px-3">观测值 / 频次</th>
              <th className="py-2.5 px-3">最近触发时间</th>
              <th className="py-2.5 px-3 text-right">处置动作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {alerts.length === 0 ? (
              <tr>
                <td
                  colSpan={6}
                  className="py-10 text-center text-muted-foreground text-xs"
                  data-testid="alert-empty-state"
                >
                  当前筛选条件下暂无告警记录
                </td>
              </tr>
            ) : (
              alerts.map((alert) => {
                const isSelectedRow = selectedAlertId === alert.id;
                const isChecked = validSelectedIds.includes(alert.id);
                const isActing = actingAlertId === alert.id;
                const ruleName =
                  ruleNameMap.get(alert.rule_id) ??
                  alert.rule_id.slice(0, 8);
                const nodeTitle =
                  typeof alert.detail.node_title === "string"
                    ? alert.detail.node_title
                    : typeof alert.detail.type_name === "string"
                      ? alert.detail.type_name
                      : null;

                return (
                  <tr
                    key={alert.id}
                    data-testid={`alert-row-${alert.id}`}
                    onClick={() => onSelectAlert(alert.id)}
                    className={cn(
                      "transition-colors cursor-pointer hover:bg-muted/40",
                      isSelectedRow && "bg-xblue-soft/50"
                    )}
                  >
                    <td
                      className="py-3 px-3 align-top"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <input
                        type="checkbox"
                        data-testid={`alert-select-${alert.id}`}
                        disabled={alert.status !== "OPEN"}
                        checked={isChecked}
                        onChange={() => toggleSelectOne(alert.id)}
                        className="h-3.5 w-3.5 rounded border-border accent-xblue disabled:opacity-30"
                      />
                    </td>

                    <td className="py-3 px-3 align-top whitespace-nowrap">
                      <div className="flex flex-col items-start gap-1.5">
                        <span
                          className={cn(
                            "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium",
                            getSeverityBadgeClass(alert.severity)
                          )}
                        >
                          <AlertTriangle className="h-3 w-3" />
                          {alert.severity}
                        </span>
                        <span
                          data-testid={`alert-status-${alert.id}`}
                          className={cn(
                            "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-semibold",
                            getStatusBadgeClass(alert.status)
                          )}
                        >
                          {alert.status}
                        </span>
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top max-w-md">
                      <div className="font-medium text-foreground leading-snug">
                        {alert.title}
                      </div>
                      {alert.body && (
                        <div className="text-[11px] text-muted-foreground line-clamp-1 mt-0.5">
                          {alert.body}
                        </div>
                      )}
                      <div className="flex flex-wrap items-center gap-1.5 mt-1.5">
                        <Badge variant="outline">规则: {ruleName}</Badge>
                        {nodeTitle && (
                          <Badge variant="neutral">对象: {nodeTitle}</Badge>
                        )}
                        {alert.escalated_at && (
                          <Badge variant="xblue">已升级 ESCALATED</Badge>
                        )}
                        {alert.resolve_reason && (
                          <Badge variant="outline">
                            恢复: {alert.resolve_reason}
                          </Badge>
                        )}
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top font-mono-tech whitespace-nowrap">
                      <div className="text-foreground font-medium">
                        {alert.last_value !== null
                          ? Number(alert.last_value.toFixed(4))
                          : "—"}
                      </div>
                      <div className="text-[11px] text-muted-foreground mt-0.5">
                        触发 ×{alert.occurrences}
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                      <div>{formatShortTime(alert.last_seen_at)}</div>
                      <div className="text-[10px] opacity-80 mt-0.5">
                        首见 {formatShortTime(alert.first_seen_at)}
                      </div>
                    </td>

                    <td
                      className="py-3 px-3 align-top text-right whitespace-nowrap"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <div className="inline-flex items-center justify-end gap-1.5">
                        {alert.status === "OPEN" && (
                          <Button
                            type="button"
                            variant="outline"
                            className="h-7 px-2.5 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                            data-testid={`ack-alert-btn-${alert.id}`}
                            disabled={isActing}
                            onClick={() => {
                              void onAckAlert(alert.id);
                            }}
                          >
                            <CheckCircle2 className="h-3 w-3" />
                            确认 (ACK)
                          </Button>
                        )}

                        {alert.status !== "RESOLVED" && (
                          <Button
                            type="button"
                            variant="outline"
                            className="h-7 px-2.5 text-[11px]"
                            data-testid={`resolve-alert-btn-${alert.id}`}
                            disabled={isActing}
                            onClick={() => {
                              void onResolveAlert(alert.id);
                            }}
                          >
                            手动恢复 (RESOLVE)
                          </Button>
                        )}

                        <Button
                          type="button"
                          variant="ghost"
                          className="h-7 px-2 text-[11px]"
                          data-testid={`open-alert-detail-btn-${alert.id}`}
                          onClick={() => onSelectAlert(alert.id)}
                        >
                          <Eye className="h-3.5 w-3.5" />
                          详情
                        </Button>
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </Card>
  );
}
