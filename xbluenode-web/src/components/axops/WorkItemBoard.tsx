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
  X,
} from "lucide-react";
import {
  axopsApi,
  type WorkItem,
  type WorkItemDetail,
  type WorkItemKind,
  type WorkItemPriority,
  type WorkItemSourceKind,
  type WorkItemStatus,
} from "@/lib/api/axops";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  WORK_ITEM_EVENT_LABELS,
  WORK_ITEM_KIND_LABELS,
  WORK_ITEM_PRIORITY_LABELS,
  WORK_ITEM_SOURCE_LABELS,
  WORK_ITEM_STATUS_COLUMNS,
} from "./constants";
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

function formatShortTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function WorkItemBoard() {
  const [viewMode, setViewMode] = useState<WorkItemViewMode>("kanban");
  const [statusFilter, setStatusFilter] = useState<WorkItemStatus | "">("");
  const [priorityFilter, setPriorityFilter] = useState<WorkItemPriority | "">(
    ""
  );
  const [sourceFilter, setSourceFilter] = useState<WorkItemSourceKind | "">("");
  const [assigneeFilter, setAssigneeFilter] = useState("");

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
      setErrorMsg(err instanceof Error ? err.message : "加载工作项失败");
    } finally {
      setLoading(false);
    }
  }, [assigneeFilter, priorityFilter, sourceFilter, statusFilter, viewMode]);

  useEffect(() => {
    void loadWorkItems();
  }, [loadWorkItems]);

  const loadDetail = useCallback(async (id: string) => {
    setDetailLoading(true);
    try {
      const res = await axopsApi.getWorkItem(id);
      setDetail(res);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "读取工作项详情失败");
    } finally {
      setDetailLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      return;
    }
    void loadDetail(selectedId);
  }, [loadDetail, selectedId]);

  const kanbanBuckets = useMemo(() => groupWorkItemsByStatus(items), [items]);

  const handleTransitionStatus = async (
    itemId: string,
    targetStatus: WorkItemStatus
  ) => {
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
        `工作项 ${formatWorkItemCode(updated)} 已流转至 ${targetStatus}`
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "更新工作项状态失败");
    } finally {
      setPatchingId(null);
    }
  };

  const handleCreateSubmit = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!newTitle.trim()) {
      setErrorMsg("请输入工作项标题");
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
      setNoticeMsg(`已创建工作项 ${formatWorkItemCode(created)}`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "创建工作项失败");
    } finally {
      setCreating(false);
    }
  };

  const handleAddComment = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!selectedId || !commentText.trim()) return;
    setPostingComment(true);
    setErrorMsg(null);
    try {
      await axopsApi.addWorkItemComment(selectedId, commentText.trim());
      setCommentText("");
      await loadDetail(selectedId);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "发表备注失败");
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
                看板视图
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
                列表视图
              </button>
            </div>
            <Badge variant="outline">共 {items.length} 项</Badge>
          </div>

          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              className="h-8 px-2.5 text-xs"
              disabled={loading}
              onClick={() => void loadWorkItems()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              刷新
            </Button>
            <Button
              type="button"
              variant="primary"
              className="h-8 px-3 text-xs"
              data-testid="open-create-workitem-btn"
              onClick={() => setCreateOpen(true)}
            >
              <Plus className="h-3.5 w-3.5" />
              新建工作项
            </Button>
          </div>
        </div>

        {/* Filter Bar */}
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2.5 text-xs">
          {viewMode === "list" && (
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                状态筛选
              </label>
              <select
                data-testid="workitem-filter-status"
                value={statusFilter}
                onChange={(e) =>
                  setStatusFilter(e.target.value as WorkItemStatus | "")
                }
                className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
              >
                <option value="">全部状态</option>
                {WORK_ITEM_STATUS_COLUMNS.map((c) => (
                  <option key={c.status} value={c.status}>
                    {c.label}
                  </option>
                ))}
              </select>
            </div>
          )}

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              优先级 (Priority)
            </label>
            <select
              data-testid="workitem-filter-priority"
              value={priorityFilter}
              onChange={(e) =>
                setPriorityFilter(e.target.value as WorkItemPriority | "")
              }
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
            >
              <option value="">全部优先级</option>
              {(["P0", "P1", "P2", "P3"] as const).map((p) => (
                <option key={p} value={p}>
                  {WORK_ITEM_PRIORITY_LABELS[p]}
                </option>
              ))}
            </select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              来源渠道 (Source)
            </label>
            <select
              data-testid="workitem-filter-source"
              value={sourceFilter}
              onChange={(e) =>
                setSourceFilter(e.target.value as WorkItemSourceKind | "")
              }
              className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
            >
              <option value="">全部来源</option>
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
                  {sk} · {WORK_ITEM_SOURCE_LABELS[sk]}
                </option>
              ))}
            </select>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              负责人 ID (Assignee)
            </label>
            <Input
              data-testid="workitem-filter-assignee"
              value={assigneeFilter}
              onChange={(e) => setAssigneeFilter(e.target.value)}
              placeholder="按负责人 UUID 过滤..."
              className="h-8 text-xs font-mono-tech"
            />
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

            return (
              <div
                key={col.status}
                data-testid={`workitem-column-${col.status}`}
                onDragOver={(e) => {
                  e.preventDefault();
                  setDragOverColumn(col.status);
                }}
                onDragLeave={() => {
                  if (dragOverColumn === col.status) {
                    setDragOverColumn(null);
                  }
                }}
                onDrop={(e) => {
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
                  isOver
                    ? "border-xblue bg-xblue-soft/30"
                    : "border-border bg-muted/20"
                )}
              >
                <div className="flex items-center justify-between px-1 pb-1.5 border-b border-border/70">
                  <span
                    className={cn(
                      "inline-flex items-center rounded border px-2 py-0.5 text-[11px] font-mono-tech font-semibold",
                      col.accentClass
                    )}
                  >
                    {col.label}
                  </span>
                  <Badge variant="outline">{colItems.length}</Badge>
                </div>

                <div className="space-y-2 flex-1">
                  {colItems.length === 0 ? (
                    <div className="py-10 text-center text-[11px] text-muted-foreground">
                      拖拽卡片至此列
                    </div>
                  ) : (
                    colItems.map((item) => {
                      const code = formatWorkItemCode(item);
                      const nextTransitions = getAllowedWorkItemTransitions(
                        item.status
                      );
                      return (
                        <Card
                          key={item.id}
                          draggable
                          data-testid={`workitem-card-${code}`}
                          onDragStart={(e) => {
                            setDraggingItemId(item.id);
                            e.dataTransfer.setData("text/plain", item.id);
                          }}
                          onDragEnd={() => {
                            setDraggingItemId(null);
                            setDragOverColumn(null);
                          }}
                          onClick={() => setSelectedId(item.id)}
                          className="p-2.5 space-y-2 cursor-pointer hover:border-xblue/60 transition-colors bg-card"
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
                              {item.item_kind}
                            </Badge>
                            <Badge variant="neutral" className="text-[10px]">
                              {WORK_ITEM_SOURCE_LABELS[item.source_kind] ??
                                item.source_kind}
                            </Badge>
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

                          {/* Quick Transition Buttons */}
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
                                className="rounded border border-border bg-muted/50 px-1.5 py-0.5 text-[10px] font-mono-tech text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                              >
                                → {target}
                              </button>
                            ))}
                          </div>
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
                  <th className="py-2.5 px-3">编号</th>
                  <th className="py-2.5 px-3">标题 / 类型</th>
                  <th className="py-2.5 px-3">优先级</th>
                  <th className="py-2.5 px-3">状态</th>
                  <th className="py-2.5 px-3">来源</th>
                  <th className="py-2.5 px-3">关联业务节点</th>
                  <th className="py-2.5 px-3">更新时间</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {items.length === 0 ? (
                  <tr>
                    <td
                      colSpan={7}
                      className="py-10 text-center text-muted-foreground"
                    >
                      暂无符合条件的工作项
                    </td>
                  </tr>
                ) : (
                  items.map((item) => {
                    const code = formatWorkItemCode(item);
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
                            {WORK_ITEM_KIND_LABELS[item.item_kind]}
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
                          <select
                            value={item.status}
                            disabled={patchingId === item.id}
                            onChange={(e) =>
                              void handleTransitionStatus(
                                item.id,
                                e.target.value as WorkItemStatus
                              )
                            }
                            className="h-7 rounded border border-input bg-background px-2 text-[11px] font-mono-tech"
                          >
                            {WORK_ITEM_STATUS_COLUMNS.map((c) => (
                              <option key={c.status} value={c.status}>
                                {c.status}
                              </option>
                            ))}
                          </select>
                        </td>
                        <td className="py-2.5 px-3 text-[11px]">
                          <Badge variant="outline">
                            {WORK_ITEM_SOURCE_LABELS[item.source_kind]}
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
                          {formatShortTime(item.updated_at)}
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
      {createOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4"
          data-testid="create-workitem-modal"
          onClick={() => setCreateOpen(false)}
        >
          <Card
            className="w-full max-w-lg p-5 space-y-4 bg-background shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="text-sm font-semibold">新建交付追踪工作项</div>
              <button
                type="button"
                onClick={() => setCreateOpen(false)}
                className="text-muted-foreground hover:text-foreground cursor-pointer"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <form
              onSubmit={(e) => void handleCreateSubmit(e)}
              className="space-y-3 text-xs"
            >
              <div className="space-y-1">
                <label className="font-medium">标题 *</label>
                <Input
                  required
                  data-testid="new-workitem-title-input"
                  value={newTitle}
                  onChange={(e) => setNewTitle(e.target.value)}
                  placeholder="例如：核查华东中心仓安全库存补货计划"
                  className="h-8 text-xs"
                />
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="font-medium">工作项类型</label>
                  <select
                    data-testid="new-workitem-kind-select"
                    value={newKind}
                    onChange={(e) => setNewKind(e.target.value as WorkItemKind)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {(["TASK", "ISSUE", "CHANGE"] as const).map((k) => (
                      <option key={k} value={k}>
                        {WORK_ITEM_KIND_LABELS[k]}
                      </option>
                    ))}
                  </select>
                </div>

                <div className="space-y-1">
                  <label className="font-medium">优先级</label>
                  <select
                    data-testid="new-workitem-priority-select"
                    value={newPriority}
                    onChange={(e) =>
                      setNewPriority(e.target.value as WorkItemPriority)
                    }
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    {(["P0", "P1", "P2", "P3"] as const).map((p) => (
                      <option key={p} value={p}>
                        {WORK_ITEM_PRIORITY_LABELS[p]}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="space-y-1">
                <label className="font-medium">详细描述</label>
                <Textarea
                  data-testid="new-workitem-desc-input"
                  value={newDesc}
                  onChange={(e) => setNewDesc(e.target.value)}
                  rows={3}
                  placeholder="补充背景说明、验收条件或SOP指引..."
                  className="text-xs"
                />
              </div>

              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="font-medium">负责人 UUID（可选）</label>
                  <Input
                    value={newAssignee}
                    onChange={(e) => setNewAssignee(e.target.value)}
                    placeholder="sec_users.id"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div className="space-y-1">
                  <label className="font-medium">
                    关联图节点 UUID（逗号分隔）
                  </label>
                  <Input
                    data-testid="new-workitem-nodes-input"
                    value={newNodeIds}
                    onChange={(e) => setNewNodeIds(e.target.value)}
                    placeholder="00000000-..."
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
                  取消
                </Button>
                <Button
                  type="submit"
                  disabled={creating}
                  data-testid="submit-create-workitem-btn"
                >
                  {creating ? "创建中..." : "确认创建"}
                </Button>
              </div>
            </form>
          </Card>
        </div>
      )}

      {/* Work Item Detail Slide-Over */}
      {selectedId && (
        <div
          className="fixed inset-0 z-50 flex justify-end bg-black/40"
          data-testid="workitem-detail-backdrop"
          onClick={() => setSelectedId(null)}
        >
          <div
            className="h-full w-full max-w-xl bg-background border-l border-border shadow-xl flex flex-col overflow-hidden"
            data-testid="workitem-detail-drawer"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="px-4 py-3 border-b border-border flex items-start justify-between gap-3 bg-muted/20">
              <div className="space-y-1 min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant="xblue" data-testid="workitem-detail-code">
                    {detail ? formatWorkItemCode(detail) : "..."}
                  </Badge>
                  {detail && (
                    <>
                      <Badge variant="outline">{detail.item_kind}</Badge>
                      <span
                        className={cn(
                          "rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-semibold",
                          getPriorityBadgeClass(detail.priority)
                        )}
                      >
                        {detail.priority}
                      </span>
                    </>
                  )}
                </div>
                <h3 className="text-sm font-semibold text-foreground">
                  {detail?.title ?? "正在读取详情..."}
                </h3>
              </div>

              <button
                type="button"
                data-testid="close-workitem-detail-btn"
                onClick={() => setSelectedId(null)}
                className="rounded p-1 text-muted-foreground hover:text-foreground cursor-pointer"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
              {detailLoading && !detail ? (
                <div className="py-12 text-center text-muted-foreground">
                  加载工作项详情与事件流水中...
                </div>
              ) : detail ? (
                <>
                  {/* Status & Metadata */}
                  <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <span className="text-muted-foreground">当前状态流转:</span>
                      <select
                        data-testid="workitem-detail-status-select"
                        value={detail.status}
                        disabled={patchingId === detail.id}
                        onChange={(e) =>
                          void handleTransitionStatus(
                            detail.id,
                            e.target.value as WorkItemStatus
                          )
                        }
                        className="h-8 rounded border border-input bg-background px-2.5 text-xs font-mono-tech font-semibold"
                      >
                        {WORK_ITEM_STATUS_COLUMNS.map((c) => (
                          <option key={c.status} value={c.status}>
                            {c.label}
                          </option>
                        ))}
                      </select>
                    </div>

                    {detail.description && (
                      <p className="text-foreground leading-relaxed pt-1 border-t border-border/60">
                        {detail.description}
                      </p>
                    )}

                    <div className="grid grid-cols-2 gap-2 pt-1 border-t border-border/60 text-[11px] font-mono-tech text-muted-foreground">
                      <div>
                        来源: {WORK_ITEM_SOURCE_LABELS[detail.source_kind]}
                      </div>
                      <div className="truncate">
                        来源引用: {detail.source_ref || "—"}
                      </div>
                      <div>创建: {formatShortTime(detail.created_at)}</div>
                      <div>关闭: {formatShortTime(detail.closed_at)}</div>
                    </div>

                    {detail.node_ids.length > 0 && (
                      <div className="pt-2 border-t border-border/60 space-y-1.5">
                        <div className="text-[11px] text-muted-foreground">
                          关联业务图节点（点击跳转图浏览器）:
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
                      <span>审计与协作事件时间线 (ax_work_item_events)</span>
                      <Badge variant="outline">{detail.events.length}</Badge>
                    </div>

                    {detail.events.length === 0 ? (
                      <div className="rounded border border-dashed border-border p-4 text-center text-muted-foreground">
                        暂无事件记录
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
                                  {WORK_ITEM_EVENT_LABELS[ev.event_kind] ??
                                    ev.event_kind}
                                </span>
                                <span className="font-mono-tech text-[11px] text-muted-foreground">
                                  {formatShortTime(ev.created_at)}
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
                </>
              ) : null}
            </div>

            {/* Comment Composer Footer */}
            <form
              onSubmit={(e) => void handleAddComment(e)}
              className="border-t border-border bg-muted/20 p-3.5 flex items-center gap-2"
            >
              <Input
                data-testid="workitem-comment-input"
                value={commentText}
                onChange={(e) => setCommentText(e.target.value)}
                placeholder="输入协作进展或排查备注..."
                className="h-8 text-xs flex-1"
              />
              <Button
                type="submit"
                disabled={postingComment || !commentText.trim()}
                className="h-8 px-3 text-xs shrink-0"
                data-testid="workitem-comment-submit-btn"
              >
                <MessageSquarePlus className="h-3.5 w-3.5" />
                {postingComment ? "发送中..." : "添加备注"}
              </Button>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}
