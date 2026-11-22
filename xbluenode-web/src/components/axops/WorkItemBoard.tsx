import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router";
import {
  Clock,
  ExternalLink,
  Kanban,
  List,
  MessageSquarePlus,
  Plus,
  RefreshCw,
  Share2,
} from "lucide-react";
import { axguardApi, type GuardUserRecord } from "@/lib/api/axguard";
import {
  axopsApi,
  type WorkItem,
  type WorkItemDetail,
  type WorkItemKind,
  type WorkItemPriority,
  type WorkItemSourceKind,
  type WorkItemStatus,
} from "@/lib/api/axops";
import {
  Badge,
  Button,
  Card,
  Drawer,
  Input,
  Modal,
  Select,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { WORK_ITEM_STATUS_COLUMNS } from "./constants";
import {
  formatWorkItemCode,
  getAllowedWorkItemTransitions,
  groupWorkItemsByStatus,
} from "./logic";

type WorkItemViewMode = "kanban" | "list";

function getPriorityBadgeClass(priority: WorkItemPriority): string {
  switch (priority) {
    case "P0":
      return "border border-red-500/50 bg-red-500/10 text-red-600 dark:text-red-400";
    case "P1":
      return "border border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-300";
    case "P2":
      return "border border-xblue/40 bg-xblue-soft text-xblue";
    case "P3":
      return "border border-border bg-muted text-muted-foreground";
  }
}

export function WorkItemBoard() {
  const { isZh, tx, tEnum, fmtNum, fmtTime } = useI18n();
  const { me, can, canAny } = usePrincipal();
  const canWrite = canAny(["OPS_ADMIN", "FLOW_RUN", "ALERT_ACK"]);
  const canReadUsers = can("GUARD_ADMIN");

  const [viewMode, setViewMode] = useState<WorkItemViewMode>("kanban");
  const [statusFilter, setStatusFilter] = useState<WorkItemStatus | "">("");
  const [priorityFilter, setPriorityFilter] = useState<WorkItemPriority | "">(
    ""
  );
  const [sourceFilter, setSourceFilter] = useState<WorkItemSourceKind | "">("");
  const [assigneeFilter, setAssigneeFilter] = useState("");

  const [tenantUsers, setTenantUsers] = useState<GuardUserRecord[]>([]);
  const [items, setItems] = useState<WorkItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [draggingItemId, setDraggingItemId] = useState<string | null>(null);
  const [dragOverColumn, setDragOverColumn] = useState<WorkItemStatus | null>(
    null
  );
  const [patchingId, setPatchingId] = useState<string | null>(null);

  // Create modal state
  const [createOpen, setCreateOpen] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newDesc, setNewDesc] = useState("");
  const [newKind, setNewKind] = useState<WorkItemKind>("TASK");
  const [newPriority, setNewPriority] = useState<WorkItemPriority>("P2");
  const [newAssignee, setNewAssignee] = useState("");
  const [newNodeIds, setNewNodeIds] = useState("");
  const [creating, setCreating] = useState(false);

  // Slide-over detail drawer state
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<WorkItemDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [commentText, setCommentText] = useState("");
  const [postingComment, setPostingComment] = useState(false);

  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [noticeMsg, setNoticeMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!canReadUsers) {
      setTenantUsers([]);
      return;
    }
    let cancelled = false;
    void axguardApi
      .listUsers()
      .then((list) => {
        if (!cancelled && Array.isArray(list)) {
          setTenantUsers(list);
        }
      })
      .catch(() => {
        // Non-admin principals may not have user directory access; fall back gracefully.
      });
    return () => {
      cancelled = true;
    };
  }, [canReadUsers]);

  const assigneeOptions = useMemo(() => {
    const map = new Map<string, string>();
    for (const u of tenantUsers) {
      map.set(
        u.id,
        u.display_name && u.display_name !== u.username
          ? `${u.username} (${u.display_name})`
          : u.username
      );
    }
    if (me?.user_id && me.username && !map.has(me.user_id)) {
      map.set(me.user_id, me.username);
    }
    for (const item of items) {
      if (item.assignee && !map.has(item.assignee)) {
        map.set(
          item.assignee,
          item.assignee.length > 16
            ? `${item.assignee.slice(0, 8)}…`
            : item.assignee
        );
      }
    }
    return Array.from(map.entries()).map(([id, label]) => ({ id, label }));
  }, [items, me?.user_id, me?.username, tenantUsers]);

  const userLabelById = useMemo(() => {
    const map = new Map<string, string>();
    for (const opt of assigneeOptions) {
      map.set(opt.id, opt.label);
    }
    return map;
  }, [assigneeOptions]);

  const getTransitionActionLabel = useCallback(
    (target: WorkItemStatus): string => {
      switch (target) {
        case "IN_PROGRESS":
          return tx("开始处理", "Start");
        case "BLOCKED":
          return tx("标记阻塞", "Block");
        case "DONE":
          return tx("标记完成", "Complete");
        case "OPEN":
          return tx("重新打开", "Reopen");
        case "CANCELED":
          return tx("取消", "Cancel");
      }
    },
    [tx]
  );

  const loadWorkItems = useCallback(async () => {
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axopsApi.listWorkItems({
        status: viewMode === "list" && statusFilter ? statusFilter : undefined,
        priority: priorityFilter || undefined,
        source_kind: sourceFilter || undefined,
        assignee: assigneeFilter.trim() || undefined,
        limit: 200,
      });
      setItems(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("加载工作项失败", "Failed to load work items")
      );
    } finally {
      setLoading(false);
    }
  }, [
    assigneeFilter,
    priorityFilter,
    sourceFilter,
    statusFilter,
    tx,
    viewMode,
  ]);

  useEffect(() => {
    void loadWorkItems();
  }, [loadWorkItems]);

  const loadDetail = useCallback(
    async (id: string) => {
      setDetailLoading(true);
      try {
        const res = await axopsApi.getWorkItem(id);
        setDetail(res);
      } catch (err) {
        setErrorMsg(
          err instanceof Error
            ? err.message
            : tx("读取工作项详情失败", "Failed to load work item detail")
        );
      } finally {
        setDetailLoading(false);
      }
    },
    [tx]
  );

  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      return;
    }
    void loadDetail(selectedId);
  }, [loadDetail, selectedId]);

  const kanbanBuckets = useMemo(() => groupWorkItemsByStatus(items), [items]);
  const draggingItem = useMemo(
    () =>
      draggingItemId
        ? items.find((i) => i.id === draggingItemId) ?? null
        : null,
    [draggingItemId, items]
  );

  const handleTransitionStatus = async (
    itemId: string,
    targetStatus: WorkItemStatus
  ) => {
    if (!canWrite) return;
    const existing = items.find((i) => i.id === itemId);
    if (existing && existing.status === targetStatus) return;

    setPatchingId(itemId);
    setErrorMsg(null);
    try {
      const updated = await axopsApi.patchWorkItem(itemId, {
        status: targetStatus,
      });
      setItems((prev) =>
        prev.map((item) => (item.id === itemId ? updated : item))
      );
      if (selectedId === itemId) {
        await loadDetail(itemId);
      }
      setNoticeMsg(
        tx(
          `工作项 ${formatWorkItemCode(updated)} 已流转至 ${tEnum(targetStatus)}`,
          `Moved ${formatWorkItemCode(updated)} to ${tEnum(targetStatus)}`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("更新工作项状态失败", "Failed to update status")
      );
    } finally {
      setPatchingId(null);
    }
  };

  const handleCreateSubmit = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!newTitle.trim()) {
      setErrorMsg(tx("工作项标题不能为空", "Title is required"));
      return;
    }
    const parsedNodeIds = newNodeIds
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    setCreating(true);
    setErrorMsg(null);
    try {
      const created = await axopsApi.createWorkItem({
        title: newTitle.trim(),
        description: newDesc.trim(),
        item_kind: newKind,
        priority: newPriority,
        assignee: newAssignee.trim() || null,
        source_kind: "MANUAL",
        node_ids: parsedNodeIds,
      });
      setItems((prev) => [created, ...prev]);
      setCreateOpen(false);
      setNewTitle("");
      setNewDesc("");
      setNewAssignee("");
      setNewNodeIds("");
      setNoticeMsg(
        tx(
          `已创建工作项 ${formatWorkItemCode(created)}`,
          `Created ${formatWorkItemCode(created)}`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("创建工作项失败", "Failed to create work item")
      );
    } finally {
      setCreating(false);
    }
  };

  const handleAddComment = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!canWrite || !selectedId || !commentText.trim()) return;
    setPostingComment(true);
    setErrorMsg(null);
    try {
      await axopsApi.addWorkItemComment(selectedId, commentText.trim());
      setCommentText("");
      await loadDetail(selectedId);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("添加备注失败", "Failed to post comment")
      );
    } finally {
      setPostingComment(false);
    }
  };

  return (
    <div className="space-y-4" data-testid="axops-workitems-panel">
      {/* Toolbar & Filters */}
      <Card className="p-3.5 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <div className="inline-flex items-center rounded-md border border-border bg-muted/40 p-0.5 text-xs">
              <button
                type="button"
                data-testid="workitem-view-kanban"
                onClick={() => setViewMode("kanban")}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded px-2.5 py-1 font-medium cursor-pointer",
                  viewMode === "kanban"
                    ? "bg-background text-foreground border border-border"
                    : "text-muted-foreground hover:text-foreground"
                )}
              >
                <Kanban className="h-3.5 w-3.5 text-xblue" />
                {tx("看板", "Kanban")}
              </button>
              <button
                type="button"
                data-testid="workitem-view-list"
                onClick={() => setViewMode("list")}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded px-2.5 py-1 font-medium cursor-pointer",
                  viewMode === "list"
                    ? "bg-background text-foreground border border-border"
                    : "text-muted-foreground hover:text-foreground"
                )}
              >
                <List className="h-3.5 w-3.5 text-xblue" />
                {tx("列表", "List")}
              </button>
            </div>
            <Badge variant="outline">
              {tx(`${fmtNum(items.length, 0)} 项`, `${fmtNum(items.length, 0)} items`)}
            </Badge>
          </div>

          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={loading}
              onClick={() => void loadWorkItems()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              {tx("刷新", "Refresh")}
            </Button>
            {canWrite && (
              <Button
                type="button"
                variant="primary"
                size="sm"
                data-testid="open-create-workitem-btn"
                onClick={() => setCreateOpen(true)}
              >
                <Plus className="h-3.5 w-3.5" />
                {tx("新建工作项", "New Work Item")}
              </Button>
            )}
          </div>
        </div>

        {/* Filter Bar */}
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2.5 text-xs">
          {viewMode === "list" && (
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("状态", "Status")}
              </label>
              <Select
                data-testid="workitem-filter-status"
                value={statusFilter}
                onChange={(e) =>
                  setStatusFilter(e.target.value as WorkItemStatus | "")
                }
                className="h-8 w-full text-xs"
              >
                <option value="">{tx("全部状态", "All Statuses")}</option>
                {WORK_ITEM_STATUS_COLUMNS.map((c) => (
                  <option key={c.status} value={c.status}>
                    {isZh ? c.label : c.enLabel}
                  </option>
                ))}
              </Select>
            </div>
          )}

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("优先级", "Priority")}
            </label>
            <Select
              data-testid="workitem-filter-priority"
              value={priorityFilter}
              onChange={(e) =>
                setPriorityFilter(e.target.value as WorkItemPriority | "")
              }
              className="h-8 w-full text-xs"
            >
              <option value="">{tx("全部优先级", "All Priorities")}</option>
              {(["P0", "P1", "P2", "P3"] as const).map((p) => (
                <option key={p} value={p}>
                  {tEnum(p)}
                </option>
              ))}
            </Select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("来源渠道", "Source")}
            </label>
            <Select
              data-testid="workitem-filter-source"
              value={sourceFilter}
              onChange={(e) =>
                setSourceFilter(e.target.value as WorkItemSourceKind | "")
              }
              className="h-8 w-full text-xs"
            >
              <option value="">{tx("全部来源", "All Sources")}</option>
              {(
                [
                  "MANUAL",
                  "FLOW_ACTION",
                  "ALERT",
                  "SIM_DECISION",
                  "TEMPLATE",
                ] as const
              ).map((sk) => (
                <option key={sk} value={sk}>
                  {tEnum(sk)}
                </option>
              ))}
            </Select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              {tx("负责人", "Assignee")}
            </label>
            <Select
              data-testid="workitem-filter-assignee"
              value={assigneeFilter}
              onChange={(e) => setAssigneeFilter(e.target.value)}
              className="h-8 w-full text-xs"
            >
              <option value="">{tx("全部负责人", "All Assignees")}</option>
              {assigneeOptions.map((opt) => (
                <option key={opt.id} value={opt.id}>
                  {opt.label}
                </option>
              ))}
            </Select>
          </div>
        </div>
      </Card>

      {errorMsg && (
        <div
          role="alert"
          className="rounded-md border border-red-500/40 bg-red-500/10 px-3.5 py-2 text-xs text-red-700 dark:text-red-300"
        >
          {errorMsg}
        </div>
      )}

      {noticeMsg && (
        <div
          data-testid="axops-workitems-notice"
          className="rounded-md border border-xblue/35 bg-xblue-soft px-3.5 py-2 text-xs text-xblue"
        >
          {noticeMsg}
        </div>
      )}

      {/* Kanban View */}
      {viewMode === "kanban" ? (
        <div
          className="grid grid-cols-1 md:grid-cols-3 xl:grid-cols-5 gap-3"
          data-testid="workitem-kanban-board"
        >
          {WORK_ITEM_STATUS_COLUMNS.map((col) => {
            const colItems = kanbanBuckets[col.status] ?? [];
            const isOver = dragOverColumn === col.status;
            const showDropPlaceholder =
              canWrite &&
              isOver &&
              draggingItemId !== null &&
              draggingItem?.status !== col.status;

            return (
              <div
                key={col.status}
                data-testid={`workitem-column-${col.status}`}
                onDragOver={(e) => {
                  if (!canWrite) return;
                  e.preventDefault();
                  if (dragOverColumn !== col.status) {
                    setDragOverColumn(col.status);
                  }
                }}
                onDragLeave={(e) => {
                  if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
                    setDragOverColumn((prev) =>
                      prev === col.status ? null : prev
                    );
                  }
                }}
                onDrop={(e) => {
                  if (!canWrite) return;
                  e.preventDefault();
                  setDragOverColumn(null);
                  const droppedId =
                    e.dataTransfer.getData("text/plain") || draggingItemId;
                  setDraggingItemId(null);
                  if (droppedId) {
                    void handleTransitionStatus(droppedId, col.status);
                  }
                }}
                className={cn(
                  "rounded-md border p-2.5 space-y-2.5 min-h-[340px] flex flex-col transition-colors",
                  isOver && canWrite
                    ? "border-xblue bg-xblue-soft/30"
                    : "border-border bg-muted/20"
                )}
              >
                <div className="flex items-center justify-between px-1 pb-1.5 border-b border-border/70">
                  <span
                    className={cn(
                      "inline-flex items-center rounded border px-2 py-0.5 text-[11px] font-semibold",
                      col.accentClass
                    )}
                  >
                    {isZh ? col.label : col.enLabel}
                  </span>
                  <Badge variant="outline">{colItems.length}</Badge>
                </div>

                <div className="space-y-2 flex-1">
                  {showDropPlaceholder && (
                    <div
                      data-testid={`workitem-drop-placeholder-${col.status}`}
                      className="h-16 rounded-md border-2 border-dashed border-xblue bg-xblue-soft/25 flex items-center justify-center text-xs text-xblue"
                    >
                      {getTransitionActionLabel(col.status)}
                    </div>
                  )}

                  {colItems.length === 0 && !showDropPlaceholder ? (
                    <div className="py-10 text-center text-[11px] text-muted-foreground">
                      {tx("暂无工作项", "No items")}
                    </div>
                  ) : (
                    colItems.map((item) => {
                      const code = formatWorkItemCode(item);
                      const nextTransitions = getAllowedWorkItemTransitions(
                        item.status
                      );
                      const isBeingDragged = draggingItemId === item.id;
                      const assigneeLabel = item.assignee
                        ? userLabelById.get(item.assignee) ??
                          `${item.assignee.slice(0, 8)}…`
                        : null;

                      return (
                        <Card
                          key={item.id}
                          draggable={canWrite}
                          data-testid={`workitem-card-${code}`}
                          onDragStart={(e) => {
                            if (!canWrite) {
                              e.preventDefault();
                              return;
                            }
                            setDraggingItemId(item.id);
                            e.dataTransfer.effectAllowed = "move";
                            e.dataTransfer.setData("text/plain", item.id);
                          }}
                          onDragEnd={() => {
                            setDraggingItemId(null);
                            setDragOverColumn(null);
                          }}
                          onClick={() => setSelectedId(item.id)}
                          className={cn(
                            "p-2.5 space-y-2 cursor-pointer hover:border-xblue/60 transition-all bg-card",
                            isBeingDragged && "opacity-50 scale-[0.98]"
                          )}
                        >
                          <div className="flex items-center justify-between gap-1.5">
                            <span className="font-mono-tech text-[11px] font-semibold text-xblue">
                              {code}
                            </span>
                            <span
                              className={cn(
                                "rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-semibold",
                                getPriorityBadgeClass(item.priority)
                              )}
                            >
                              {item.priority}
                            </span>
                          </div>

                          <div className="text-xs font-medium text-foreground leading-snug line-clamp-2">
                            {item.title}
                          </div>

                          <div className="flex flex-wrap items-center gap-1">
                            <Badge variant="outline" className="text-[10px]">
                              {tEnum(item.item_kind)}
                            </Badge>
                            <Badge variant="neutral" className="text-[10px]">
                              {tEnum(item.source_kind)}
                            </Badge>
                            {assigneeLabel && (
                              <Badge
                                variant="outline"
                                className="text-[10px]"
                                title={item.assignee ?? undefined}
                              >
                                @{assigneeLabel}
                              </Badge>
                            )}
                          </div>

                          {item.node_ids.length > 0 && (
                            <div
                              className="flex flex-wrap gap-1"
                              onClick={(e) => e.stopPropagation()}
                            >
                              {item.node_ids.map((nid) => (
                                <Link
                                  key={nid}
                                  to={`/graph?node=${encodeURIComponent(nid)}`}
                                  data-testid={`workitem-node-link-${nid}`}
                                  className="inline-flex items-center gap-1 rounded border border-xblue/35 bg-xblue-soft px-1.5 py-0.5 text-[10px] font-mono-tech text-xblue hover:opacity-90"
                                >
                                  <Share2 className="h-2.5 w-2.5" />
                                  {nid.slice(0, 8)}
                                </Link>
                              ))}
                            </div>
                          )}

                          {/* Quick Transition Buttons (RBAC gated) */}
                          {canWrite && nextTransitions.length > 0 && (
                            <div
                              className="flex flex-wrap gap-1 pt-1.5 border-t border-border/60"
                              onClick={(e) => e.stopPropagation()}
                            >
                              {nextTransitions.slice(0, 2).map((target) => (
                                <button
                                  key={target}
                                  type="button"
                                  disabled={patchingId === item.id}
                                  data-testid={`workitem-quick-${code}-${target}`}
                                  onClick={() =>
                                    void handleTransitionStatus(item.id, target)
                                  }
                                  className="rounded border border-border bg-muted/50 px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                                >
                                  {getTransitionActionLabel(target)}
                                </button>
                              ))}
                            </div>
                          )}
                        </Card>
                      );
                    })
                  )}
                </div>
              </div>
            );
          })}
        </div>
      ) : (
        /* List View */
        <Card className="overflow-hidden" data-testid="workitem-list-table">
          <div className="overflow-x-auto">
            <table className="w-full text-left border-collapse text-xs">
              <thead>
                <tr className="border-b border-border bg-muted/40 text-[11px] text-muted-foreground">
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("编号", "Code")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("标题 / 类型", "Title / Kind")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("优先级", "Priority")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("状态", "Status")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("负责人", "Assignee")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("来源", "Source")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("关联节点", "Linked Nodes")}
                  </th>
                  <th className="py-2.5 px-3 whitespace-nowrap">
                    {tx("更新时间", "Updated")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {items.length === 0 ? (
                  <tr>
                    <td
                      colSpan={8}
                      className="py-10 text-center text-muted-foreground"
                    >
                      {tx("暂无匹配工作项", "No matching work items")}
                    </td>
                  </tr>
                ) : (
                  items.map((item) => {
                    const code = formatWorkItemCode(item);
                    const assigneeLabel = item.assignee
                      ? userLabelById.get(item.assignee) ??
                        `${item.assignee.slice(0, 8)}…`
                      : "—";

                    return (
                      <tr
                        key={item.id}
                        data-testid={`workitem-row-${code}`}
                        onClick={() => setSelectedId(item.id)}
                        className="hover:bg-muted/30 cursor-pointer"
                      >
                        <td className="py-2.5 px-3 font-mono-tech font-semibold text-xblue whitespace-nowrap">
                          {code}
                        </td>
                        <td className="py-2.5 px-3">
                          <div className="font-medium text-foreground">
                            {item.title}
                          </div>
                          <div className="text-[11px] text-muted-foreground">
                            {tEnum(item.item_kind)}
                          </div>
                        </td>
                        <td className="py-2.5 px-3">
                          <span
                            className={cn(
                              "rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-semibold",
                              getPriorityBadgeClass(item.priority)
                            )}
                          >
                            {item.priority}
                          </span>
                        </td>
                        <td
                          className="py-2.5 px-3"
                          onClick={(e) => e.stopPropagation()}
                        >
                          {canWrite ? (
                            <Select
                              value={item.status}
                              disabled={patchingId === item.id}
                              onChange={(e) =>
                                void handleTransitionStatus(
                                  item.id,
                                  e.target.value as WorkItemStatus
                                )
                              }
                              className="h-7 text-[11px]"
                            >
                              {WORK_ITEM_STATUS_COLUMNS.map((c) => (
                                <option key={c.status} value={c.status}>
                                  {isZh ? c.label : c.enLabel}
                                </option>
                              ))}
                            </Select>
                          ) : (
                            <StatusBadge status={item.status} />
                          )}
                        </td>
                        <td className="py-2.5 px-3 text-[11px] font-mono-tech">
                          {assigneeLabel}
                        </td>
                        <td className="py-2.5 px-3 text-[11px]">
                          <Badge variant="outline">
                            {tEnum(item.source_kind)}
                          </Badge>
                        </td>
                        <td
                          className="py-2.5 px-3"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <div className="flex flex-wrap gap-1">
                            {item.node_ids.length === 0 ? (
                              <span className="text-muted-foreground">—</span>
                            ) : (
                              item.node_ids.map((nid) => (
                                <Link
                                  key={nid}
                                  to={`/graph?node=${encodeURIComponent(nid)}`}
                                  className="inline-flex items-center gap-1 rounded border border-xblue/35 bg-xblue-soft px-1.5 py-0.5 text-[10px] font-mono-tech text-xblue"
                                >
                                  {nid.slice(0, 8)}
                                  <ExternalLink className="h-2.5 w-2.5" />
                                </Link>
                              ))
                            )}
                          </div>
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                          {fmtTime(item.updated_at, true)}
                        </td>
                      </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>
        </Card>
      )}

      {/* Create Work Item Modal */}
      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        testId="create-workitem-modal"
        title={tx("新建工作项", "New Work Item")}
      >
        <form
          onSubmit={(e) => void handleCreateSubmit(e)}
          className="space-y-3 text-xs"
        >
          <div className="space-y-1">
            <label className="font-medium">{tx("标题", "Title")} *</label>
            <Input
              required
              data-testid="new-workitem-title-input"
              value={newTitle}
              onChange={(e) => setNewTitle(e.target.value)}
              placeholder={tx(
                "华东中心仓安全库存补货核查",
                "Verify East Hub safety stock replenishment"
              )}
              className="h-8 text-xs"
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="font-medium">{tx("类型", "Kind")}</label>
              <Select
                data-testid="new-workitem-kind-select"
                value={newKind}
                onChange={(e) => setNewKind(e.target.value as WorkItemKind)}
                className="h-8 w-full text-xs"
              >
                {(["TASK", "ISSUE", "CHANGE"] as const).map((k) => (
                  <option key={k} value={k}>
                    {tEnum(k)}
                  </option>
                ))}
              </Select>
            </div>

            <div className="space-y-1">
              <label className="font-medium">{tx("优先级", "Priority")}</label>
              <Select
                data-testid="new-workitem-priority-select"
                value={newPriority}
                onChange={(e) =>
                  setNewPriority(e.target.value as WorkItemPriority)
                }
                className="h-8 w-full text-xs"
              >
                {(["P0", "P1", "P2", "P3"] as const).map((p) => (
                  <option key={p} value={p}>
                    {tEnum(p)}
                  </option>
                ))}
              </Select>
            </div>
          </div>

          <div className="space-y-1">
            <label className="font-medium">{tx("描述", "Description")}</label>
            <Textarea
              data-testid="new-workitem-desc-input"
              value={newDesc}
              onChange={(e) => setNewDesc(e.target.value)}
              rows={3}
              placeholder={tx(
                "异常背景、处置范围与验收标准",
                "Context, scope, and acceptance criteria"
              )}
              className="text-xs"
            />
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="font-medium">{tx("负责人", "Assignee")}</label>
              <Select
                value={newAssignee}
                onChange={(e) => setNewAssignee(e.target.value)}
                className="h-8 w-full text-xs"
              >
                <option value="">{tx("未指派", "Unassigned")}</option>
                {assigneeOptions.map((opt) => (
                  <option key={opt.id} value={opt.id}>
                    {opt.label}
                  </option>
                ))}
              </Select>
            </div>
            <div className="space-y-1">
              <label className="font-medium">
                {tx("关联图节点 ID", "Linked Node IDs")}
              </label>
              <Input
                data-testid="new-workitem-nodes-input"
                value={newNodeIds}
                onChange={(e) => setNewNodeIds(e.target.value)}
                placeholder="00000000-0000-0000-0000-000000000021"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>

          <div className="flex justify-end gap-2 pt-2 border-t border-border">
            <Button
              type="button"
              variant="outline"
              onClick={() => setCreateOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="submit"
              disabled={creating}
              data-testid="submit-create-workitem-btn"
            >
              {creating ? tx("创建中…", "Creating…") : tx("创建", "Create")}
            </Button>
          </div>
        </form>
      </Modal>

      {/* Work Item Detail Drawer */}
      <Drawer
        open={Boolean(selectedId)}
        onClose={() => setSelectedId(null)}
        testId="workitem-detail-drawer"
        closeTestId="close-workitem-detail-btn"
        widthClass="max-w-xl"
        title={
          <div className="flex items-center gap-2 min-w-0">
            <Badge variant="xblue" data-testid="workitem-detail-code">
              {detail ? formatWorkItemCode(detail) : "…"}
            </Badge>
            {detail && (
              <>
                <Badge variant="outline">{tEnum(detail.item_kind)}</Badge>
                <span
                  className={cn(
                    "rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-semibold shrink-0",
                    getPriorityBadgeClass(detail.priority)
                  )}
                >
                  {detail.priority}
                </span>
              </>
            )}
            <span className="truncate">
              {detail?.title ?? tx("加载中…", "Loading…")}
            </span>
          </div>
        }
        footer={
          canWrite ? (
            <form
              onSubmit={(e) => void handleAddComment(e)}
              className="w-full flex items-center gap-2"
            >
              <Input
                data-testid="workitem-comment-input"
                value={commentText}
                onChange={(e) => setCommentText(e.target.value)}
                placeholder={tx("协作进展或排查备注", "Progress note or comment")}
                className="h-8 text-xs flex-1"
              />
              <Button
                type="submit"
                size="sm"
                disabled={postingComment || !commentText.trim()}
                data-testid="workitem-comment-submit-btn"
              >
                <MessageSquarePlus className="h-3.5 w-3.5" />
                {postingComment
                  ? tx("提交中…", "Posting…")
                  : tx("添加备注", "Comment")}
              </Button>
            </form>
          ) : undefined
        }
      >
        {detailLoading && !detail ? (
          <div className="py-12 text-center text-muted-foreground">
            {tx("加载中…", "Loading…")}
          </div>
        ) : detail ? (
          <div className="space-y-4">
            {/* Status & Metadata */}
            <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-muted-foreground">
                  {tx("状态", "Status")}
                </span>
                {canWrite ? (
                  <Select
                    data-testid="workitem-detail-status-select"
                    value={detail.status}
                    disabled={patchingId === detail.id}
                    onChange={(e) =>
                      void handleTransitionStatus(
                        detail.id,
                        e.target.value as WorkItemStatus
                      )
                    }
                    className="h-8 text-xs font-semibold"
                  >
                    {WORK_ITEM_STATUS_COLUMNS.map((c) => (
                      <option key={c.status} value={c.status}>
                        {isZh ? c.label : c.enLabel}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <StatusBadge status={detail.status} />
                )}
              </div>

              {detail.description && (
                <p className="text-foreground leading-relaxed pt-1 border-t border-border/60">
                  {detail.description}
                </p>
              )}

              <div className="grid grid-cols-2 gap-2 pt-1 border-t border-border/60 text-[11px] font-mono-tech text-muted-foreground">
                <div>
                  {tx("来源", "Source")}: {tEnum(detail.source_kind)}
                </div>
                <div className="truncate">
                  {tx("负责人", "Assignee")}:{" "}
                  {detail.assignee
                    ? userLabelById.get(detail.assignee) ??
                      `${detail.assignee.slice(0, 8)}…`
                    : "—"}
                </div>
                <div>
                  {tx("创建", "Created")}: {fmtTime(detail.created_at, true)}
                </div>
                <div>
                  {tx("关闭", "Closed")}: {fmtTime(detail.closed_at, true)}
                </div>
              </div>

              {detail.node_ids.length > 0 && (
                <div className="pt-2 border-t border-border/60 space-y-1.5">
                  <div className="text-[11px] text-muted-foreground">
                    {tx("关联图节点", "Linked Graph Nodes")}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {detail.node_ids.map((nid) => (
                      <Link
                        key={nid}
                        to={`/graph?node=${encodeURIComponent(nid)}`}
                        data-testid={`detail-node-badge-${nid}`}
                        className="inline-flex items-center gap-1 rounded-md border border-xblue/40 bg-xblue-soft px-2 py-1 text-[11px] font-mono-tech text-xblue hover:opacity-90"
                      >
                        <Share2 className="h-3 w-3" />
                        /graph?node={nid.slice(0, 8)}
                        <ExternalLink className="h-3 w-3" />
                      </Link>
                    ))}
                  </div>
                </div>
              )}
            </div>

            {/* Event Timeline */}
            <div className="space-y-2" data-testid="workitem-event-timeline">
              <div className="flex items-center gap-1.5 font-semibold">
                <Clock className="h-3.5 w-3.5 text-xblue" />
                <span>{tx("事件时间线", "Event Timeline")}</span>
                <Badge variant="outline">{detail.events.length}</Badge>
              </div>

              {detail.events.length === 0 ? (
                <div className="rounded border border-dashed border-border p-4 text-center text-muted-foreground">
                  {tx("暂无事件记录", "No events recorded")}
                </div>
              ) : (
                <ol className="relative border-l border-border ml-2 space-y-3 pl-4 py-1">
                  {detail.events.map((ev) => {
                    const commentVal =
                      typeof ev.detail?.comment === "string"
                        ? ev.detail.comment
                        : null;
                    return (
                      <li
                        key={ev.id}
                        data-testid={`workitem-event-${ev.event_kind}`}
                        className="relative"
                      >
                        <span className="absolute -left-[21px] top-1.5 h-2.5 w-2.5 rounded-full border border-background bg-xblue" />
                        <div className="flex items-center justify-between gap-2">
                          <span className="font-medium text-foreground">
                            {tEnum(ev.event_kind)}
                          </span>
                          <span className="font-mono-tech text-[11px] text-muted-foreground">
                            {fmtTime(ev.created_at, true)}
                          </span>
                        </div>
                        {commentVal ? (
                          <div className="mt-1 rounded bg-muted px-2.5 py-1.5 text-foreground">
                            {commentVal}
                          </div>
                        ) : (
                          Object.keys(ev.detail ?? {}).length > 0 && (
                            <div className="mt-1 font-mono-tech text-[11px] text-muted-foreground break-all">
                              {JSON.stringify(ev.detail)}
                            </div>
                          )
                        )}
                      </li>
                    );
                  })}
                </ol>
              )}
            </div>
          </div>
        ) : null}
      </Drawer>
    </div>
  );
}
