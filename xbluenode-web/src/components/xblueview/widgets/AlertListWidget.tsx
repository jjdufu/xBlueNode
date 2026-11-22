import { useState } from "react";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { useI18n } from "@/i18n";
import { axflowApi } from "@/lib/api/axflow";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { Button, StatusBadge } from "@/components/ui/primitives";
import { buildColumnIndexMap, getCell } from "../utils";

export interface AlertListWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  readOnly?: boolean;
  onAckSuccess?: () => void;
  onSelectNode?: (nodeId: string, title?: string) => void;
}

export function AlertListWidget({
  widget,
  data,
  readOnly = false,
  onAckSuccess,
  onSelectNode,
}: AlertListWidgetProps) {
  const { tx, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canAck = can("ALERT_ACK");

  const [ackingId, setAckingId] = useState<string | null>(null);
  const [ackedIds, setAckedIds] = useState<Set<string>>(new Set());
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  if (!data || data.rows.length === 0) {
    return (
      <div
        className="h-full flex items-center justify-center text-xs text-muted-foreground"
        data-testid={`widget-alerts-${widget.id}`}
      >
        {tx("暂无活动告警", "No active alerts")}
      </div>
    );
  }

  const colMap = buildColumnIndexMap(data);

  const handleAck = async (alertId: string) => {
    if (!canAck) return;
    setAckingId(alertId);
    setErrorMsg(null);
    try {
      await axflowApi.ackAlert(alertId, {
        comment: tx("驾驶舱确认", "Acknowledged in cockpit"),
      });
      setAckedIds((prev) => new Set(prev).add(alertId));
      onAckSuccess?.();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setAckingId(null);
    }
  };

  return (
    <div
      className="h-full flex flex-col gap-1.5 overflow-y-auto text-xs"
      data-testid={`widget-alerts-${widget.id}`}
    >
      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-2 py-1 text-[11px]">
          {errorMsg}
        </div>
      )}
      {data.rows.map((row, idx) => {
        const id = String(getCell(row, colMap, "id", 0) ?? idx);
        const title = String(
          getCell(row, colMap, "title", -1) ??
            getCell(row, colMap, "message", -1) ??
            row[4] ??
            row[1] ??
            tx("告警事件", "Alert")
        );
        const severity = String(getCell(row, colMap, "severity", 2) ?? "WARN");
        const rawStatus = String(getCell(row, colMap, "status", 3) ?? "OPEN");
        const status = ackedIds.has(id) ? "ACK" : rawStatus;
        const nodeIdRaw = getCell(row, colMap, "node_id", -1);
        const nodeId =
          typeof nodeIdRaw === "string" && nodeIdRaw ? nodeIdRaw : null;
        const seenAtRaw =
          getCell(row, colMap, "last_seen_at", -1) ??
          getCell(row, colMap, "first_seen_at", -1) ??
          getCell(row, colMap, "created_at", -1) ??
          "";
        const seenStr =
          typeof seenAtRaw === "string" && seenAtRaw.trim()
            ? fmtTime(seenAtRaw)
            : "";

        return (
          <div
            key={`${id}-${idx}`}
            className="rounded border border-border p-2 flex items-center justify-between gap-2 bg-background hover:bg-muted/30"
          >
            <div
              className={`min-w-0 flex-1 ${
                nodeId && onSelectNode ? "cursor-pointer" : ""
              }`}
              onClick={() => {
                if (nodeId && onSelectNode) {
                  onSelectNode(nodeId, title);
                }
              }}
            >
              <div className="flex items-center gap-1.5">
                <AlertTriangle
                  className={`h-3.5 w-3.5 shrink-0 ${
                    severity === "CRITICAL"
                      ? "text-red-500"
                      : "text-amber-500"
                  }`}
                />
                <span className="font-medium truncate">{title}</span>
                <StatusBadge status={severity} />
                <StatusBadge status={status} />
              </div>
              {seenStr && seenStr !== "-" && (
                <div className="text-[10px] font-mono-tech text-muted-foreground mt-0.5 truncate">
                  {seenStr}
                </div>
              )}
            </div>

            {!readOnly && canAck && status === "OPEN" && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-6 px-2 text-[11px] shrink-0"
                disabled={ackingId === id}
                onClick={() => void handleAck(id)}
              >
                <CheckCircle2 className="h-3 w-3 text-xblue" />
                {ackingId === id
                  ? tx("处理中…", "Saving…")
                  : tx("确认", "Ack")}
              </Button>
            )}
          </div>
        );
      })}
    </div>
  );
}
