import { useState } from "react";
import { Check, RefreshCw, X } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type {
  ApprovalDecision,
  ApprovalRecord,
  ApprovalStatus,
} from "@/lib/api/axflow";
import {
  Badge,
  Button,
  Input,
  Select,
  Switch,
} from "@/components/ui/primitives";

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
  const { tx, tEnum, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canDecide = can("APPROVAL_DECIDE");

  const [comments, setComments] = useState<Record<string, string>>({});
  const [confirmRejectId, setConfirmRejectId] = useState<string | null>(null);

  const submitDecision = async (
    approvalId: string,
    decision: ApprovalDecision
  ) => {
    if (!canDecide) return;
    const comment = comments[approvalId]?.trim() ?? "";
    setConfirmRejectId(null);
    await onDecide(approvalId, decision, comment);
    setComments((prev) => {
      const next = { ...prev };
      delete next[approvalId];
      return next;
    });
  };

  return (
    <div
      className="flex flex-col gap-3 text-xs"
      data-testid="axflow-approval-inbox"
    >
      {/* Filter Toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/20 px-3 py-2">
        <div className="inline-flex items-center gap-2">
          <Switch
            data-testid="approval-mine-toggle"
            checked={mineOnly}
            onCheckedChange={onChangeMineOnly}
            aria-label={tx("仅指派给我", "Mine Only")}
          />
          <span className="text-[11px] text-muted-foreground">
            {tx("仅指派给我", "Mine Only")}
          </span>
        </div>

        <div className="flex items-center gap-1.5">
          <Select
            data-testid="approval-status-filter"
            value={statusFilter}
            onChange={(e) =>
              onChangeStatusFilter(e.target.value as ApprovalStatus | "ALL")
            }
            className="h-7 w-28 px-2 font-mono-tech text-[11px]"
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

      {/* Approval Items */}
      {approvals.length === 0 ? (
        <div
          className="rounded-md border border-dashed border-border py-12 text-center text-xs text-muted-foreground"
          data-testid="approval-inbox-empty"
        >
          {tx("暂无匹配审批事项", "No matching approvals")}
        </div>
      ) : (
        <div className="space-y-2.5">
          {approvals.map((item) => {
            const isPending = item.status === "PENDING";
            const busy = decidingId === item.id;
            const detailEntries = Object.entries(item.detail ?? {});
            const isConfirmingReject = confirmRejectId === item.id;

            return (
              <div
                key={item.id}
                data-testid={`approval-item-${item.id}`}
                className="rounded-md border border-border bg-card p-3 space-y-2.5"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="space-y-1 min-w-0">
                    <div className="font-semibold text-foreground truncate">
                      {item.title}
                    </div>
                    <div className="flex flex-wrap items-center gap-1.5 text-[10px] text-muted-foreground">
                      <Badge variant="outline">
                        {item.source_kind === "FLOW"
                          ? tEnum("HUMAN_APPROVAL")
                          : tEnum(item.source_kind)}
                      </Badge>
                      {item.node_id && (
                        <span className="font-mono-tech">
                          {tx("节点:", "Step:")} {item.node_id}
                        </span>
                      )}
                      {item.run_id && (
                        <span className="font-mono-tech">
                          Run #{item.run_id.slice(0, 8)}
                        </span>
                      )}
                      {item.assignee_roles.length > 0 && (
                        <span>
                          {item.assignee_roles.map((r) => tEnum(r)).join(" · ")}
                        </span>
                      )}
                    </div>
                  </div>
                  <Badge
                    variant={
                      item.status === "PENDING"
                        ? "warning"
                        : item.status === "APPROVED"
                          ? "success"
                          : "danger"
                    }
                  >
                    {tEnum(item.status)}
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
                      placeholder={tx("审批意见", "Approval comment")}
                      className="h-8 w-full text-xs"
                    />
                    <div className="flex items-center justify-end gap-1.5">
                      {isConfirmingReject ? (
                        <>
                          <span className="text-[11px] text-red-600 dark:text-red-400 mr-auto">
                            {tx(
                              "驳回将终止当前待审分支",
                              "Rejecting terminates this gated branch"
                            )}
                          </span>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            onClick={() => setConfirmRejectId(null)}
                          >
                            {tx("取消", "Cancel")}
                          </Button>
                          <Button
                            type="button"
                            variant="danger"
                            size="sm"
                            data-testid={`approval-reject-${item.id}`}
                            disabled={!canDecide || busy}
                            onClick={() =>
                              void submitDecision(item.id, "REJECT")
                            }
                          >
                            <X className="h-3.5 w-3.5" />
                            {tx("拒绝", "Reject")}
                          </Button>
                        </>
                      ) : (
                        <>
                          <Button
                            type="button"
                            size="sm"
                            className="h-7 px-3 bg-emerald-600 hover:bg-emerald-700 text-white border-transparent"
                            data-testid={`approval-approve-${item.id}`}
                            disabled={!canDecide || busy}
                            onClick={() =>
                              void submitDecision(item.id, "APPROVE")
                            }
                          >
                            <Check className="h-3.5 w-3.5" />
                            {tx("通过", "Approve")}
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            className="h-7 px-3 text-red-600 border-red-500/35 hover:bg-red-500/10 dark:text-red-400"
                            data-testid={`approval-reject-${item.id}`}
                            disabled={!canDecide || busy}
                            onClick={() => setConfirmRejectId(item.id)}
                          >
                            <X className="h-3.5 w-3.5" />
                            {tx("拒绝", "Reject")}
                          </Button>
                        </>
                      )}
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
          })}
        </div>
      )}
    </div>
  );
}
