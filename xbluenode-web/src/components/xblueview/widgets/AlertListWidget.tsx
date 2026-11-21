import { useState } from "react";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { axflowApi } from "@/lib/api/axflow";
import type { WidgetDataResponse, WidgetRecord } from "@/lib/api/xblueview";
import { Badge, Button } from "@/components/ui/primitives";
import { buildColumnIndexMap, getCell } from "../utils";

export interface AlertListWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  readOnly?: boolean;
  onAckSuccess?: () => void;
  onSelectNode?: (nodeId: string, title?: string) => void;
}

function formatAlertTime(raw: unknown): string {
  if (raw === undefined || raw === null) return "";
  const str = String(raw).trim();
  if (!str) return "";
  const parsed = new Date(str);
  if (Number.isNaN(parsed.getTime())) {
    return str;
  }
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${parsed.getFullYear()}-${pad(parsed.getMonth() + 1)}-${pad(
    parsed.getDate()
  )} ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}:${pad(
    parsed.getSeconds()
  )}`;
}

export function AlertListWidget({
  widget,
  data,
  readOnly = false,
  onAckSuccess,
  onSelectNode,
}: AlertListWidgetProps) {
  const [ackingId, setAckingId] = useState<string | null>(null);
  const [ackedIds, setAckedIds] = useState<Set<string>>(new Set());
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  if (!data || data.rows.length === 0) {
    return (
      <div
        className="h-full flex items-center justify-center text-xs text-muted-foreground"
        data-testid={`widget-alerts-${widget.id}`}
      >
        暂无活动告警
      </div>
    );
  }

  const colMap = buildColumnIndexMap(data);

  const handleAck = async (alertId: string) => {
    setAckingId(alertId);
    setErrorMsg(null);
    try {
      await axflowApi.ackAlert(alertId, { comment: "驾驶舱内确认" });
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
            "未命名告警"
        );
        const severity = String(getCell(row, colMap, "severity", 2) ?? "WARN");
        const rawStatus = String(getCell(row, colMap, "status", 3) ?? "OPEN");
        const status = ackedIds.has(id) ? "ACK" : rawStatus;
        const nodeIdRaw = getCell(row, colMap, "node_id", -1);
        const nodeId = typeof nodeIdRaw === "string" && nodeIdRaw ? nodeIdRaw : null;
        const seenAtRaw =
          getCell(row, colMap, "last_seen_at", -1) ??
          getCell(row, colMap, "first_seen_at", -1) ??
          getCell(row, colMap, "created_at", -1) ??
          "";
        const formattedTime = formatAlertTime(seenAtRaw);

        return (
          <div
            key={`${id}-${idx}`}
            className="rounded border border-border p-2 flex items-center justify-between gap-2 bg-background hover:bg-muted/30"
          >
            <div
              className={`min-w-0 flex-1 ${nodeId && onSelectNode ? "cursor-pointer" : ""}`}
              onClick={() => {
                if (nodeId && onSelectNode) {
                  onSelectNode(nodeId, title);
                }
              }}
            >
              <div className="flex items-center gap-1.5">
                <AlertTriangle
                  className={`h-3.5 w-3.5 shrink-0 ${
                    severity === "CRITICAL" ? "text-foreground" : "text-xblue"
                  }`}
                />
                <span className="font-medium truncate">{title}</span>
                <Badge variant={severity === "CRITICAL" ? "neutral" : "xblue"}>
                  {severity}
                </Badge>
                <Badge variant="outline">{status}</Badge>
              </div>
              {formattedTime.length > 0 && (
                <div className="text-[10px] font-mono-tech text-muted-foreground mt-0.5 truncate">
                  {formattedTime}
                </div>
              )}
            </div>

            {!readOnly && status === "OPEN" && (
              <Button
                type="button"
                variant="outline"
                className="px-2 py-1 text-[11px] shrink-0"
                disabled={ackingId === id}
                onClick={() => void handleAck(id)}
              >
                <CheckCircle2 className="h-3 w-3 text-xblue" />
                {ackingId === id ? "处理中" : "确认"}
              </Button>
            )}
          </div>
        );
      })}
    </div>
  );
}
