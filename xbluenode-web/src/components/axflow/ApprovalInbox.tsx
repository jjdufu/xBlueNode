import { useState } from "react";
import { Check, RefreshCw, UserCheck, X } from "lucide-react";
import type {
  ApprovalDecision,
  ApprovalRecord,
  ApprovalStatus,
} from "@/lib/api/axflow";
import { Badge, Button, Input } from "@/components/ui/primitives";

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
  const [comments, setComments] = useState<Record<string, string>>({});

  const submitDecision = async (
    approvalId: string,
    decision: ApprovalDecision
  ) => {
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
      className="flex flex-col rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-approval-inbox"
    >
      <div className="p-3 border-b border-border flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-1.5">
          <UserCheck className="h-4 w-4 text-xblue" />
          <span className="text-xs font-semibold">人工审批收件箱</span>
          <Badge
            variant={approvals.length > 0 ? "xblue" : "neutral"}
            data-testid="approval-inbox-count"
          >
            {approvals.length}
          </Badge>
        </div>

        <div className="flex flex-wrap items-center gap-2 text-xs">
          <label className="inline-flex items-center gap-1 text-[11px] text-muted-foreground cursor-pointer">
            <input
              type="checkbox"
              data-testid="approval-mine-toggle"
              checked={mineOnly}
              onChange={(e) => onChangeMineOnly(e.target.checked)}
              className="h-3.5 w-3.5 accent-blue-600"
            />
            仅看指派给我
          </label>

          <select
            data-testid="approval-status-filter"
            value={statusFilter}
            onChange={(e) =>
              onChangeStatusFilter(e.target.value as ApprovalStatus | "ALL")
            }
            className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-[11px]"
          >
            <option value="PENDING">待审批 (PENDING)</option>
            <option value="APPROVED">已通过 (APPROVED)</option>
            <option value="REJECTED">已拒绝 (REJECTED)</option>
            <option value="ALL">全部记录</option>
          </select>

          <Button
            type="button"
            variant="ghost"
            className="h-7 px-2 py-1"
            data-testid="refresh-approvals-btn"
            onClick={() => void onRefresh()}
            title="刷新审批收件箱"
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <div className="max-h-96 overflow-y-auto divide-y divide-border">
        {approvals.length === 0 ? (
          <div
            className="py-10 text-center text-xs text-muted-foreground"
            data-testid="approval-inbox-empty"
          >
            当前没有匹配的审批事项
          </div>
        ) : (
          approvals.map((item) => {
            const isPending = item.status === "PENDING";
            const busy = decidingId === item.id;
            const detailEntries = Object.entries(item.detail);

            return (
              <div
                key={item.id}
                data-testid={`approval-item-${item.id}`}
                className="p-3 space-y-2.5 text-xs"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="space-y-0.5">
                    <div className="font-semibold text-foreground">
                      {item.title}
                    </div>
                    <div className="flex flex-wrap items-center gap-1.5 font-mono-tech text-[10px] text-muted-foreground">
                      <Badge variant="outline">
                        {item.source_kind === "FLOW"
                          ? "HUMAN_APPROVAL"
                          : item.source_kind}
                      </Badge>
                      {item.node_id && <span>节点: {item.node_id}</span>}
                      {item.run_id && (
                        <span>Run #{item.run_id.slice(0, 8)}</span>
                      )}
                      {item.assignee_roles.length > 0 && (
                        <span>角色: {item.assignee_roles.join(", ")}</span>
                      )}
                    </div>
                  </div>
                  <Badge
                    variant={
                      item.status === "PENDING"
                        ? "xblue"
                        : item.status === "APPROVED"
                          ? "neutral"
                          : "outline"
                    }
                  >
                    {item.status}
                  </Badge>
                </div>

                {detailEntries.length > 0 && (
                  <div className="rounded border border-border bg-muted/30 p-2 font-mono-tech text-[11px] space-y-0.5">
                    {detailEntries.map(([k, v]) => (
                      <div key={k} className="flex justify-between gap-2">
                        <span className="text-muted-foreground">{k}:</span>
                        <span className="truncate">
                          {typeof v === "string" ? v : JSON.stringify(v)}
                        </span>
                      </div>
                    ))}
                  </div>
                )}

                {isPending ? (
                  <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
                    <Input
                      data-testid={`approval-comment-${item.id}`}
                      value={comments[item.id] ?? ""}
                      onChange={(e) =>
                        setComments((prev) => ({
                          ...prev,
                          [item.id]: e.target.value,
                        }))
                      }
                      placeholder="填写审批意见（选填）"
                      className="h-8 text-xs flex-1"
                    />
                    <div className="flex items-center gap-1.5 shrink-0">
                      <Button
                        type="button"
                        className="h-8 px-3 text-xs"
                        data-testid={`approval-approve-${item.id}`}
                        disabled={busy}
                        onClick={() => void submitDecision(item.id, "APPROVE")}
                      >
                        <Check className="h-3.5 w-3.5" />
                        通过
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        className="h-8 px-3 text-xs"
                        data-testid={`approval-reject-${item.id}`}
                        disabled={busy}
                        onClick={() => void submitDecision(item.id, "REJECT")}
                      >
                        <X className="h-3.5 w-3.5" />
                        拒绝
                      </Button>
                    </div>
                  </div>
                ) : (
                  item.comment && (
                    <div className="text-[11px] text-muted-foreground">
                      审批意见：{item.comment}
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
