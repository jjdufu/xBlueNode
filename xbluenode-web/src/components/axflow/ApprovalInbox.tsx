import { useState } from "react";
import { Check, RefreshCw, UserCheck, X } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type {
  ApprovalDecision,
  ApprovalRecord,
  ApprovalStatus,
} from "@/lib/api/axflow";
import { Badge, Button, Input, Select } from "@/components/ui/primitives";

export interface ApprovalInboxProps {
  approvals: ApprovalRecord[];
  statusFilter: ApprovalStatus | "ALL";
  mineOnly: boolean;
  decidingId: string | null;
  onChangeStatusFilter: (next: ApprovalStatus | "ALL") => void;
  onChangeMineOnly: (mine: boolean) => void;
  onDecide: (
    approvalId: string,
    decision: ApprovalDecision,
    comment: string
  ) => Promise<void>;
  onRefresh: () => Promise<void>;
}

export function ApprovalInbox({
  approvals,
  statusFilter,
  mineOnly,
  decidingId,
  onChangeStatusFilter,
  onChangeMineOnly,
  onDecide,
  onRefresh,
}: ApprovalInboxProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canDecide = can("APPROVAL_DECIDE");

  const [comments, setComments] = useState<Record<string, string>>({});

  const submitDecision = async (
    approvalId: string,
    decision: ApprovalDecision
  ) => {
    if (!canDecide) return;
    const comment = comments[approvalId]?.trim() ?? "";
    await onDecide(approvalId, decision, comment);
    setComments((prev) => {
      const next = { ...prev };
      delete next[approvalId];
      return next;
    });
  };

  return (
    <div
      className="flex flex-col h-[320px] rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-approval-inbox"
    >
      <div className="p-3 border-b border-border flex flex-wrap items-center justify-between gap-2 shrink-0">
        <div className="flex items-center gap-1.5">
          <UserCheck className="h-4 w-4 text-xblue shrink-0" />
          <span className="text-xs font-semibold">
            {tx("人工审批收件箱", "Approval Inbox")}
          </span>
          <Badge
            variant={approvals.length > 0 ? "xblue" : "neutral"}
            data-testid="approval-inbox-count"
          >
            {approvals.length}
          </Badge>
        </div>

        <div className="flex flex-wrap items-center gap-1.5 text-xs">
          <label className="inline-flex items-center gap-1 text-[11px] text-muted-foreground cursor-pointer">
            <input
              type="checkbox"
              data-testid="approval-mine-toggle"
              checked={mineOnly}
              onChange={(e) => onChangeMineOnly(e.target.checked)}
              className="h-3.5 w-3.5 accent-blue-600"
            />
            {tx("仅指派给我", "Mine Only")}
          </label>

          <Select
            data-testid="approval-status-filter"
            value={statusFilter}
            onChange={(e) =>
              onChangeStatusFilter(e.target.value as ApprovalStatus | "ALL")
            }
            className="h-7 px-2 font-mono-tech text-[11px]"
          >
            <option value="PENDING">{tx("待审批", "Pending")}</option>
            <option value="APPROVED">{tx("已批准", "Approved")}</option>
            <option value="REJECTED">{tx("已驳回", "Rejected")}</option>
            <option value="ALL">{tx("全部", "All")}</option>
          </Select>

          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-2"
            data-testid="refresh-approvals-btn"
            onClick={() => void onRefresh()}
            title={tx("刷新审批", "Refresh")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto divide-y divide-border">
        {approvals.length === 0 ? (
          <div
            className="h-full flex items-center justify-center p-6 text-center text-xs text-muted-foreground"
            data-testid="approval-inbox-empty"
          >
            {tx("暂无匹配审批事项", "No matching approvals")}
          </div>
        ) : (
          approvals.map((item) => {
            const isPending = item.status === "PENDING";
            const busy = decidingId === item.id;
            const detailEntries = Object.entries(item.detail ?? {});

            return (
              <div
                key={item.id}
                data-testid={`approval-item-${item.id}`}
                className="p-3 space-y-2.5 text-xs"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="space-y-0.5 min-w-0">
                    <div className="font-semibold text-foreground truncate">
                      {item.title}
                    </div>
                    <div className="flex flex-wrap items-center gap-1.5 font-mono-tech text-[10px] text-muted-foreground">
                      <Badge variant="outline">
                        {item.source_kind === "FLOW"
                          ? "HUMAN_APPROVAL"
                          : item.source_kind}
                      </Badge>
                      {item.node_id && (
                        <span>
                          {tx("节点:", "Step:")} {item.node_id}
                        </span>
                      )}
                      {item.run_id && (
                        <span>Run #{item.run_id.slice(0, 8)}</span>
                      )}
                      {item.assignee_roles.length > 0 && (
                        <span>{item.assignee_roles.join(", ")}</span>
                      )}
                    </div>
                  </div>
                  <Badge
                    variant={
                      item.status === "PENDING"
                        ? "xblue"
                        : item.status === "APPROVED"
                          ? "success"
                          : "danger"
                    }
                  >
                    {item.status}
                  </Badge>
                </div>

                {detailEntries.length > 0 && (
                  <div className="flex flex-wrap gap-1 rounded border border-border bg-muted/25 p-2">
                    {detailEntries.map(([k, v]) => (
                      <Badge key={k} variant="outline">
                        {k}:{" "}
                        {typeof v === "number"
                          ? fmtNum(v, 2)
                          : typeof v === "string"
                            ? v
                            : JSON.stringify(v)}
                      </Badge>
                    ))}
                  </div>
                )}

                {isPending ? (
                  <div className="space-y-2">
                    <Input
                      data-testid={`approval-comment-${item.id}`}
                      value={comments[item.id] ?? ""}
                      disabled={!canDecide || busy}
                      onChange={(e) =>
                        setComments((prev) => ({
                          ...prev,
                          [item.id]: e.target.value,
                        }))
                      }
                      placeholder={tx("审批意见（选填）", "Comment (optional)")}
                      className="h-8 w-full text-xs"
                    />
                    <div className="flex items-center justify-end gap-1.5">
                      <Button
                        type="button"
                        size="sm"
                        className="h-7 px-3"
                        data-testid={`approval-approve-${item.id}`}
                        disabled={!canDecide || busy}
                        onClick={() => void submitDecision(item.id, "APPROVE")}
                      >
                        <Check className="h-3.5 w-3.5" />
                        {tx("通过", "Approve")}
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-3"
                        data-testid={`approval-reject-${item.id}`}
                        disabled={!canDecide || busy}
                        onClick={() => void submitDecision(item.id, "REJECT")}
                      >
                        <X className="h-3.5 w-3.5" />
                        {tx("拒绝", "Reject")}
                      </Button>
                    </div>
                  </div>
                ) : (
                  item.comment && (
                    <div className="text-[11px] text-muted-foreground">
                      {tx("意见：", "Comment: ")}
                      {item.comment}
                    </div>
                  )
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
