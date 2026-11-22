import { useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCheck,
  CheckCircle2,
  Eye,
  Filter,
  Kanban,
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
import { Badge, Button, Card, Input, Select } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export interface AlertTableProps {
  alerts: readonly AlertRecord[];
  rules: readonly RuleDef[];
  knownTypeNames: readonly string[];
  filters: AlertListQuery;
  selectedAlertId: string | null;
  loading: boolean;
  actingAlertId: string | null;
  convertingAlertId?: string | null;
  batchAcking: boolean;
  canAck: boolean;
  onFiltersChange: (next: AlertListQuery) => void;
  onSelectAlert: (alertId: string) => void;
  onAckAlert: (alertId: string) => Promise<void>;
  onResolveAlert: (alertId: string) => Promise<void>;
  onConvertWorkItem?: (alert: AlertRecord) => Promise<void>;
  onBatchAck: (alertIds: string[], comment: string) => Promise<void>;
  onRefresh: () => void;
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
  convertingAlertId = null,
  batchAcking,
  canAck,
  onFiltersChange,
  onSelectAlert,
  onAckAlert,
  onResolveAlert,
  onConvertWorkItem,
  onBatchAck,
  onRefresh,
}: AlertTableProps) {
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();
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
    if (!canAck) return;
    if (allOpenSelected) {
      setSelectedIds([]);
    } else {
      setSelectedIds([...openAlertIds]);
    }
  };

  const toggleSelectOne = (id: string) => {
    if (!canAck) return;
    setSelectedIds((prev) =>
      prev.includes(id) ? prev.filter((item) => item !== id) : [...prev, id]
    );
  };

  const handleBatchSubmit = async () => {
    if (!canAck || validSelectedIds.length === 0) return;
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
            <span>{tx("运营告警队列", "Operational Alert Queue")}</span>
            <Badge variant="outline">
              {tx(`${fmtNum(alerts.length, 0)} 条`, `${fmtNum(alerts.length, 0)} items`)}
            </Badge>
          </div>
          <div className="flex items-center gap-2">
            {hasFilters && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                data-testid="alert-filter-reset-btn"
                onClick={() => onFiltersChange({})}
              >
                <RotateCcw className="h-3 w-3" />
                {tx("重置筛选", "Reset")}
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="alert-refresh-btn"
              onClick={onRefresh}
              disabled={loading}
            >
              <RefreshCw className={cn("h-3 w-3", loading && "animate-spin")} />
              {tx("刷新", "Refresh")}
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2.5">
          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("状态", "Status")}
            </label>
            <Select
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
              className="h-8 w-full"
            >
              <option value="">{tx("全部状态", "All Statuses")}</option>
              <option value="OPEN">{tx("待处理", "Open")}</option>
              <option value="ACK">{tx("已确认", "Acknowledged")}</option>
              <option value="RESOLVED">{tx("已恢复", "Resolved")}</option>
            </Select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("级别", "Severity")}
            </label>
            <Select
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
              className="h-8 w-full"
            >
              <option value="">{tx("全部级别", "All Severities")}</option>
              <option value="CRITICAL">{tEnum("CRITICAL")}</option>
              <option value="WARN">{tEnum("WARN")}</option>
              <option value="INFO">{tEnum("INFO")}</option>
            </Select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("归属规则", "Rule")}
            </label>
            <Select
              data-testid="alert-filter-rule"
              value={filters.rule_id ?? ""}
              onChange={(e) =>
                onFiltersChange({
                  ...filters,
                  rule_id: e.target.value || undefined,
                })
              }
              className="h-8 w-full"
            >
              <option value="">{tx("全部规则", "All Rules")}</option>
              {rules.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.name}
                </option>
              ))}
            </Select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("节点类型", "Node Type")}
            </label>
            <Input
              list="axalerts-known-type-names"
              data-testid="alert-filter-type-name"
              placeholder="Warehouse"
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
        {canAck && openAlertIds.length > 0 && (
          <div
            className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2"
            data-testid="batch-ack-bar"
          >
            <span className="text-xs text-foreground">
              {tx("已选待确认告警", "Selected open alerts")}{" "}
              <span className="font-mono-tech font-semibold text-xblue">
                {validSelectedIds.length}
              </span>{" "}
              / {openAlertIds.length}
            </span>

            <div className="flex flex-wrap items-center gap-2">
              <Input
                data-testid="batch-ack-comment-input"
                placeholder={tx("批量确认备注", "Batch acknowledgment note")}
                value={batchComment}
                onChange={(e) => setBatchComment(e.target.value)}
                className="h-8 w-52 text-xs"
              />
              <Button
                type="button"
                variant="primary"
                size="sm"
                data-testid="batch-ack-btn"
                disabled={validSelectedIds.length === 0 || batchAcking}
                onClick={() => {
                  void handleBatchSubmit();
                }}
              >
                <CheckCheck className="h-3.5 w-3.5" />
                {batchAcking
                  ? tx("确认中…", "Acknowledging…")
                  : tx(
                      `批量确认 (${validSelectedIds.length})`,
                      `Acknowledge (${validSelectedIds.length})`
                    )}
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
              <th className="py-2.5 px-3 w-9 whitespace-nowrap">
                <input
                  type="checkbox"
                  data-testid="alert-select-all"
                  aria-label={tx("全选待确认告警", "Select all open alerts")}
                  disabled={!canAck || openAlertIds.length === 0}
                  checked={allOpenSelected}
                  onChange={toggleSelectAll}
                  className="h-3.5 w-3.5 rounded border-border accent-xblue disabled:opacity-30 cursor-pointer"
                />
              </th>
              <th className="py-2.5 px-3 whitespace-nowrap">
                {tx("级别 / 状态", "Severity / Status")}
              </th>
              <th className="py-2.5 px-3 whitespace-nowrap">
                {tx("告警摘要与归属规则", "Alert Summary & Rule")}
              </th>
              <th className="py-2.5 px-3 whitespace-nowrap">
                {tx("观测值 / 频次", "Value / Count")}
              </th>
              <th className="py-2.5 px-3 whitespace-nowrap">
                {tx("最近触发时间", "Last Triggered")}
              </th>
              <th className="py-2.5 px-3 text-right whitespace-nowrap">
                {tx("处置动作", "Actions")}
              </th>
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
                  {tx("暂无告警记录", "No alerts")}
                </td>
              </tr>
            ) : (
              alerts.map((alert) => {
                const isSelectedRow = selectedAlertId === alert.id;
                const isChecked = validSelectedIds.includes(alert.id);
                const isActing = actingAlertId === alert.id;
                const isConverting = convertingAlertId === alert.id;
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
                        aria-label={alert.title}
                        disabled={!canAck || alert.status !== "OPEN"}
                        checked={isChecked}
                        onChange={() => toggleSelectOne(alert.id)}
                        className="h-3.5 w-3.5 rounded border-border accent-xblue disabled:opacity-30 cursor-pointer"
                      />
                    </td>

                    <td className="py-3 px-3 align-top whitespace-nowrap">
                      <div className="flex flex-col items-start gap-1.5">
                        <span
                          title={tEnum(alert.severity)}
                          className={cn(
                            "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium",
                            getSeverityBadgeClass(alert.severity)
                          )}
                        >
                          <AlertTriangle className="h-3 w-3" />
                          {tEnum(alert.severity)}
                        </span>
                        <span
                          data-testid={`alert-status-${alert.id}`}
                          title={tEnum(alert.status)}
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
                        <Badge variant="outline">
                          {tx("规则", "Rule")}: {ruleName}
                        </Badge>
                        {nodeTitle && (
                          <Badge variant="neutral">
                            {tx("对象", "Object")}: {nodeTitle}
                          </Badge>
                        )}
                        {alert.escalated_at && (
                          <Badge variant="xblue">
                            {tx("已升级", "Escalated")}
                          </Badge>
                        )}
                        {alert.resolve_reason && (
                          <Badge variant="outline">
                            {tx("恢复", "Resolved")}: {alert.resolve_reason}
                          </Badge>
                        )}
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top font-mono-tech whitespace-nowrap">
                      <div className="text-foreground font-medium">
                        {alert.last_value !== null
                          ? fmtNum(alert.last_value, 4)
                          : "—"}
                      </div>
                      <div className="text-[11px] text-muted-foreground mt-0.5">
                        {tx(`触发 ×${fmtNum(alert.occurrences, 0)}`, `Hits ×${fmtNum(alert.occurrences, 0)}`)}
                      </div>
                    </td>

                    <td className="py-3 px-3 align-top font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                      <div>{fmtTime(alert.last_seen_at)}</div>
                      <div className="text-[10px] opacity-80 mt-0.5">
                        {tx("首见", "First")} {fmtTime(alert.first_seen_at, true)}
                      </div>
                    </td>

                    <td
                      className="py-3 px-3 align-top text-right whitespace-nowrap"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <div className="inline-flex items-center justify-end gap-1.5">
                        {canAck && alert.status === "OPEN" && (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            className="border-xblue/40 text-xblue hover:bg-xblue-soft"
                            data-testid={`ack-alert-btn-${alert.id}`}
                            disabled={isActing}
                            onClick={() => {
                              void onAckAlert(alert.id);
                            }}
                          >
                            <CheckCircle2 className="h-3 w-3" />
                            {tx("确认", "Acknowledge")}
                          </Button>
                        )}

                        {canAck && alert.status !== "RESOLVED" && (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            data-testid={`resolve-alert-btn-${alert.id}`}
                            disabled={isActing}
                            onClick={() => {
                              void onResolveAlert(alert.id);
                            }}
                          >
                            {tx("手动恢复", "Resolve")}
                          </Button>
                        )}

                        {canAck && onConvertWorkItem && (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            className="border-xblue/40 text-xblue hover:bg-xblue-soft"
                            data-testid={`convert-alert-workitem-btn-${alert.id}`}
                            disabled={isConverting}
                            onClick={() => {
                              void onConvertWorkItem(alert);
                            }}
                          >
                            <Kanban className="h-3 w-3" />
                            {isConverting
                              ? tx("创建中…", "Creating…")
                              : tx("转工作项", "Convert to Work Item")}
                          </Button>
                        )}

                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          data-testid={`open-alert-detail-btn-${alert.id}`}
                          onClick={() => onSelectAlert(alert.id)}
                        >
                          <Eye className="h-3.5 w-3.5" />
                          {tx("详情", "Details")}
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
