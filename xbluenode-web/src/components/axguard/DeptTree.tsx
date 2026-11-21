import { type FormEvent, useMemo, useState } from "react";
import {
  Building2,
  ChevronDown,
  ChevronRight,
  Edit3,
  FolderTree,
  Plus,
  Trash2,
} from "lucide-react";
import { ApiError } from "@/lib/api";
import { axguardApi, type DepartmentRecord } from "@/lib/api/axguard";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import {
  buildDepartmentTree,
  collectDescendantDeptIds,
  type DepartmentTreeNode,
} from "./logic";

interface DeptTreeProps {
  departments: DepartmentRecord[];
  onReload: () => Promise<void>;
}

const DEPT_SLUG_REGEX = /^[a-z0-9_-]{1,64}$/;

export function DeptTree({ departments, onReload }: DeptTreeProps) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [editingMode, setEditingMode] = useState<"create" | "edit">("create");
  const [deptId, setDeptId] = useState("");
  const [deptName, setDeptName] = useState("");
  const [parentId, setParentId] = useState<string>("");
  const [saving, setSaving] = useState(false);
  const [feedback, setFeedback] = useState<{
    kind: "ok" | "err";
    text: string;
  } | null>(null);

  const treeRoots = useMemo(
    () => buildDepartmentTree(departments),
    [departments]
  );

  const forbiddenParentIds = useMemo(() => {
    if (editingMode !== "edit" || !deptId) {
      return new Set<string>();
    }
    return collectDescendantDeptIds(departments, deptId);
  }, [departments, editingMode, deptId]);

  const startCreateChild = (parentSlug: string | null) => {
    setEditingMode("create");
    setDeptId("");
    setDeptName("");
    setParentId(parentSlug ?? "");
    setFeedback(null);
  };

  const startEdit = (record: DepartmentRecord) => {
    setEditingMode("edit");
    setDeptId(record.dept_id);
    setDeptName(record.name);
    setParentId(record.parent_id ?? "");
    setFeedback(null);
  };

  const handleSave = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const cleanId = deptId.trim();
    const cleanName = deptName.trim();
    if (!DEPT_SLUG_REGEX.test(cleanId)) {
      setFeedback({
        kind: "err",
        text: "部门编码仅允许小写字母、数字、下划线或连字符（1～64 位）",
      });
      return;
    }
    if (!cleanName) {
      setFeedback({ kind: "err", text: "请输入部门名称" });
      return;
    }

    setSaving(true);
    setFeedback(null);
    try {
      await axguardApi.upsertDepartment(cleanId, {
        name: cleanName,
        parent_id: parentId.trim() ? parentId.trim() : null,
      });
      await onReload();
      setFeedback({
        kind: "ok",
        text:
          editingMode === "create"
            ? `已创建部门 ${cleanName} (${cleanId})`
            : `已更新部门 ${cleanName} (${cleanId}) 层级与名称`,
      });
      if (editingMode === "create") {
        setDeptId("");
        setDeptName("");
      }
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setFeedback({
          kind: "err",
          text: `层级冲突 [409 ${err.errorCode}]：${err.message}`,
        });
      } else {
        setFeedback({
          kind: "err",
          text:
            err instanceof ApiError
              ? `[${err.errorCode}] ${err.message}`
              : String(err),
        });
      }
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (record: DepartmentRecord) => {
    if (record.dept_id === "default") {
      setFeedback({ kind: "err", text: "系统根默认部门 (default) 不可删除" });
      return;
    }
    setFeedback(null);
    try {
      await axguardApi.deleteDepartment(record.dept_id);
      await onReload();
      setFeedback({
        kind: "ok",
        text: `已删除部门 ${record.name} (${record.dept_id})`,
      });
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setFeedback({
          kind: "err",
          text: `无法删除部门 ${record.dept_id}（409 冲突）：该部门下仍存在子部门、关联用户或归属资源（${err.message}）。`,
        });
      } else {
        setFeedback({
          kind: "err",
          text:
            err instanceof ApiError
              ? `[${err.errorCode}] ${err.message}`
              : String(err),
        });
      }
    }
  };

  const renderNode = (node: DepartmentTreeNode) => {
    const hasChildren = node.children.length > 0;
    const isCollapsed = collapsed.has(node.record.dept_id);
    const isSelected =
      editingMode === "edit" && deptId === node.record.dept_id;

    return (
      <div key={node.record.dept_id} className="space-y-1">
        <div
          style={{ paddingLeft: `${node.depth * 20 + 8}px` }}
          className={cn(
            "flex items-center justify-between gap-2 rounded-md border px-3 py-2 text-xs transition-colors",
            isSelected
              ? "border-xblue bg-xblue-soft/50"
              : "border-border hover:bg-muted/40"
          )}
        >
          <div className="flex items-center gap-2 min-w-0">
            {hasChildren ? (
              <button
                type="button"
                onClick={() =>
                  setCollapsed((prev) => {
                    const next = new Set(prev);
                    if (next.has(node.record.dept_id)) {
                      next.delete(node.record.dept_id);
                    } else {
                      next.add(node.record.dept_id);
                    }
                    return next;
                  })
                }
                className="text-muted-foreground hover:text-foreground cursor-pointer"
              >
                {isCollapsed ? (
                  <ChevronRight className="h-3.5 w-3.5" />
                ) : (
                  <ChevronDown className="h-3.5 w-3.5" />
                )}
              </button>
            ) : (
              <Building2 className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
            )}

            <span className="font-medium truncate">{node.record.name}</span>
            <Badge variant="outline">{node.record.dept_id}</Badge>
            {node.record.parent_id && (
              <span className="text-[11px] text-muted-foreground font-mono-tech">
                上级: {node.record.parent_id}
              </span>
            )}
          </div>

          <div className="flex items-center gap-1 shrink-0">
            <Button
              variant="ghost"
              type="button"
              onClick={() => startCreateChild(node.record.dept_id)}
              className="px-2 py-1 text-[11px]"
            >
              <Plus className="h-3 w-3 text-xblue" />
              新增下级
            </Button>
            <Button
              variant="ghost"
              type="button"
              onClick={() => startEdit(node.record)}
              className="px-2 py-1 text-[11px]"
            >
              <Edit3 className="h-3 w-3" />
              编辑
            </Button>
            {node.record.dept_id !== "default" && (
              <Button
                variant="ghost"
                type="button"
                onClick={() => void handleDelete(node.record)}
                data-testid={`delete-dept-${node.record.dept_id}`}
                className="px-2 py-1 text-[11px]"
              >
                <Trash2 className="h-3 w-3" />
              </Button>
            )}
          </div>
        </div>

        {hasChildren && !isCollapsed && (
          <div className="space-y-1">
            {node.children.map((child) => renderNode(child))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="space-y-4" data-testid="axguard-depts-panel">
      {feedback && (
        <div
          role="alert"
          data-testid="dept-feedback-banner"
          className={cn(
            "rounded-md border px-3 py-2 text-xs",
            feedback.kind === "ok"
              ? "border-xblue/40 bg-xblue-soft text-foreground"
              : "border-foreground/40 bg-muted text-foreground"
          )}
        >
          {feedback.text}
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
        <Card className="lg:col-span-7 p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-3">
            <div>
              <div className="text-xs font-semibold flex items-center gap-1.5">
                <FolderTree className="h-3.5 w-3.5 text-xblue" />
                组织部门层级树（Cedar Entity Hierarchy）
              </div>
              <p className="text-[11px] text-muted-foreground">
                上级部门主体在 Cedar 判定中自动继承对下级部门归属资源的部门内访问关系。
              </p>
            </div>
            <Button
              variant="outline"
              type="button"
              onClick={() => startCreateChild(null)}
            >
              <Plus className="h-3.5 w-3.5" />
              新建顶级部门
            </Button>
          </div>

          {treeRoots.length === 0 ? (
            <div className="py-8 text-center text-xs text-muted-foreground">
              暂无部门数据
            </div>
          ) : (
            <div className="space-y-1.5">{treeRoots.map((n) => renderNode(n))}</div>
          )}
        </Card>

        <Card className="lg:col-span-5 p-4 space-y-4">
          <div className="border-b border-border pb-3">
            <div className="text-xs font-semibold">
              {editingMode === "create"
                ? "新建部门节点"
                : `编辑部门 / 调整上级 (${deptId})`}
            </div>
            <div className="text-[11px] text-muted-foreground">
              部门编码创建后作为主键引用（如 default / east / south）。
            </div>
          </div>

          <form onSubmit={(e) => void handleSave(e)} className="space-y-3">
            <div className="space-y-1">
              <label className="text-xs font-medium">部门编码 (dept_id) *</label>
              <Input
                required
                disabled={editingMode === "edit"}
                placeholder="如 east / east_logistics"
                value={deptId}
                onChange={(e) => setDeptId(e.target.value)}
                data-testid="dept-id-input"
                className="h-8 text-xs font-mono-tech"
              />
            </div>

            <div className="space-y-1">
              <label className="text-xs font-medium">部门名称 *</label>
              <Input
                required
                placeholder="如 华东大区"
                value={deptName}
                onChange={(e) => setDeptName(e.target.value)}
                data-testid="dept-name-input"
                className="h-8 text-xs"
              />
            </div>

            <div className="space-y-1">
              <label className="text-xs font-medium">上级部门 (parent_id)</label>
              <select
                value={parentId}
                onChange={(e) => setParentId(e.target.value)}
                className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
              >
                <option value="">（无上级 · 顶级部门）</option>
                {departments
                  .filter((d) => !forbiddenParentIds.has(d.dept_id))
                  .map((d) => (
                    <option key={d.dept_id} value={d.dept_id}>
                      {d.name} ({d.dept_id})
                    </option>
                  ))}
              </select>
            </div>

            <div className="flex justify-end gap-2 pt-2">
              {editingMode === "edit" && (
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => startCreateChild(null)}
                >
                  切换为新建
                </Button>
              )}
              <Button
                type="submit"
                disabled={saving || !deptId.trim() || !deptName.trim()}
                data-testid="save-dept-btn"
              >
                {saving
                  ? "保存中..."
                  : editingMode === "create"
                    ? "创建部门"
                    : "保存层级变更"}
              </Button>
            </div>
          </form>
        </Card>
      </div>
    </div>
  );
}
